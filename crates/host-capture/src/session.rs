use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::Interface;

use crate::dxgi::DxgiCapturer;
use crate::error::CaptureError;
use crate::wgc::{OnFrameCallback, WgcCaptureSession};

/// Активный бэкенд захвата экрана
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackend {
    /// Windows Graphics Capture (WinRT COM / Zero-Copy) — основной бэкенд
    Wgc,
    /// DXGI Desktop Duplication — резервный бэкенд при сбое или недоступности WGC
    Dxgi,
}

/// Унифицированная сессия захвата экрана с автоматическим отказоустойчивым переключением (Failover)
/// WGC (основной) -> DXGI Desktop Duplication (резервный)
pub struct ScreenCaptureSession {
    wgc_session: Option<WgcCaptureSession>,
    dxgi_stop: Option<Arc<AtomicBool>>,
    dxgi_thread: Option<std::thread::JoinHandle<()>>,
    pub backend: CaptureBackend,
    pub width: u32,
    pub height: u32,
}

impl ScreenCaptureSession {
    /// Получить список всех доступных мониторов (индекс, ширина, высота) с отказоустойчивостью
    pub fn enumerate_monitors() -> Result<Vec<(usize, u32, u32)>, CaptureError> {
        if let Ok(monitors) = WgcCaptureSession::enumerate_monitors() {
            if !monitors.is_empty() {
                return Ok(monitors);
            }
        }

        // Резервное перечисление через DXGI
        let factory: windows::Win32::Graphics::Dxgi::IDXGIFactory1 =
            unsafe { windows::Win32::Graphics::Dxgi::CreateDXGIFactory1()? };
        let mut result = Vec::new();
        let mut adapter_idx = 0;

        while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_idx) } {
            let mut out_idx = 0;
            while let Ok(output) = unsafe { adapter.EnumOutputs(out_idx) } {
                if let Ok(desc) = unsafe { output.GetDesc() } {
                    let w = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left)
                        .unsigned_abs();
                    let h = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top)
                        .unsigned_abs();
                    result.push((out_idx as usize, w, h));
                }
                out_idx += 1;
            }
            adapter_idx += 1;
        }

        if result.is_empty() {
            result.push((0, 1920, 1080));
        }
        Ok(result)
    }

    /// Запуск захвата основного дисплея с автоматическим отказоустойчивым переходом на DXGI
    pub fn start<F>(callback: F) -> Result<Self, CaptureError>
    where
        F: Fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                u32,
                u32,
            ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        Self::start_monitor(0, true, callback)
    }

    /// Запуск захвата указанного монитора с автоматическим отказоустойчивым переходом на DXGI при ошибке WGC
    pub fn start_monitor<F>(
        monitor_index: usize,
        with_cursor: bool,
        callback: F,
    ) -> Result<Self, CaptureError>
    where
        F: Fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                u32,
                u32,
            ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        let cb_arc: OnFrameCallback = Arc::new(callback);

        // 1. Попытка запуска через основной механизм: Windows Graphics Capture (WGC)
        let cb_wgc = Arc::clone(&cb_arc);
        match WgcCaptureSession::start_monitor(monitor_index, with_cursor, move |tex, dev, w, h| {
            cb_wgc(tex, dev, w, h)
        }) {
            Ok(wgc) => {
                let width = wgc.width;
                let height = wgc.height;
                tracing::info!(
                    "Аппаратный захват экрана активен через WGC [монитор #{}, {}x{}]",
                    monitor_index,
                    width,
                    height
                );
                return Ok(Self {
                    wgc_session: Some(wgc),
                    dxgi_stop: None,
                    dxgi_thread: None,
                    backend: CaptureBackend::Wgc,
                    width,
                    height,
                });
            }
            Err(e) => {
                tracing::warn!(
                    "Не удалось инициализировать захват WGC ({:?}). Активация резервного бэкенда DXGI Desktop Duplication...",
                    e
                );
            }
        }

        // 2. Резервный запуск через DXGI Desktop Duplication (Failover)
        Self::start_dxgi_monitor_internal(monitor_index, cb_arc)
    }

    /// Прямой запуск через DXGI Desktop Duplication (для принудительного использования или тестирования резерва)
    pub fn start_dxgi_monitor<F>(monitor_index: usize, callback: F) -> Result<Self, CaptureError>
    where
        F: Fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                u32,
                u32,
            ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        Self::start_dxgi_monitor_internal(monitor_index, Arc::new(callback))
    }

    fn start_dxgi_monitor_internal(
        monitor_index: usize,
        callback: OnFrameCallback,
    ) -> Result<Self, CaptureError> {
        let mut capturer = DxgiCapturer::new(0, monitor_index as u32)?;
        let width = capturer.width;
        let height = capturer.height;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);

        let thread = std::thread::Builder::new()
            .name("dxgi-capture-thread".into())
            .spawn(move || {
                while !stop_clone.load(Ordering::Relaxed) {
                    let mut is_timeout = false;
                    let mut need_recreate = false;
                    let mut other_err = None;

                    {
                        match capturer.acquire_frame(100) {
                            Ok(frame) => {
                                let tex_ptr = frame.texture.as_raw();
                                let dev_ptr = capturer.device.as_raw();
                                let fw = capturer.width;
                                let fh = capturer.height;
                                if let Err(e) = callback(tex_ptr, dev_ptr, fw, fh) {
                                    tracing::trace!("Ошибка в колбэке кадра DXGI: {:?}", e);
                                }
                            }
                            Err(CaptureError::Timeout(_)) => {
                                is_timeout = true;
                            }
                            Err(CaptureError::AccessLost) | Err(CaptureError::ModeChanged) => {
                                need_recreate = true;
                            }
                            Err(e) => {
                                other_err = Some(e);
                            }
                        }
                    }

                    if is_timeout {
                        continue;
                    }
                    if need_recreate {
                        tracing::warn!(
                            "DXGI Desktop Duplication: потеря контекста, пересоздание..."
                        );
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        if let Err(e) = capturer.recreate() {
                            tracing::warn!("Ошибка пересоздания DXGI дубликатора: {:?}", e);
                            std::thread::sleep(std::time::Duration::from_millis(200));
                        }
                    }
                    if let Some(e) = other_err {
                        tracing::debug!("Ошибка DXGI acquire_frame: {:?}, пауза...", e);
                        std::thread::sleep(std::time::Duration::from_millis(16));
                    }
                }
            })
            .map_err(|e| {
                CaptureError::InitializationFailed(format!(
                    "Не удалось запустить поток DXGI захвата: {:?}",
                    e
                ))
            })?;

        tracing::info!(
            "Резервный захват DXGI Desktop Duplication успешно запущен [монитор #{}, {}x{}]",
            monitor_index,
            width,
            height
        );

        Ok(Self {
            wgc_session: None,
            dxgi_stop: Some(stop_flag),
            dxgi_thread: Some(thread),
            backend: CaptureBackend::Dxgi,
            width,
            height,
        })
    }

    /// Остановка сессии захвата и освобождение всех системных ресурсов
    pub fn stop(&mut self) {
        if let Some(mut wgc) = self.wgc_session.take() {
            wgc.stop();
        }
        if let Some(stop) = self.dxgi_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.dxgi_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}
