use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

use crate::error::CaptureError;

pub type OnFrameCallback = Arc<
    dyn Fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u32, u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + Sync,
>;

struct WgcFlags {
    callback: OnFrameCallback,
    stop_signal: Arc<AtomicBool>,
}

struct WgcCaptureHandler {
    callback: OnFrameCallback,
    stop_signal: Arc<AtomicBool>,
}

impl GraphicsCaptureApiHandler for WgcCaptureHandler {
    type Flags = WgcFlags;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            callback: ctx.flags.callback,
            stop_signal: ctx.flags.stop_signal,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.stop_signal.load(Ordering::Relaxed) {
            capture_control.stop();
            return Ok(());
        }

        let width = frame.width();
        let height = frame.height();
        let texture_raw: *mut std::ffi::c_void =
            unsafe { std::mem::transmute_copy(frame.as_raw_texture()) };
        let device_raw: *mut std::ffi::c_void =
            unsafe { std::mem::transmute_copy(frame.device()) };

        if let Err(e) = (self.callback)(texture_raw, device_raw, width, height) {
            tracing::warn!("Ошибка в колбэке кадра WGC: {:?}", e);
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        tracing::info!("WGC Capture session closed");
        Ok(())
    }
}

/// Управляемая сессия аппаратного захвата экрана через Windows Graphics Capture (WGC)
pub struct WgcCaptureSession {
    stop_signal: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<Result<(), CaptureError>>>,
    pub width: u32,
    pub height: u32,
}

impl WgcCaptureSession {
    /// Запуск сессии захвата основного дисплея с передачей каждого готового GPU кадра в колбэк
    pub fn start<F>(callback: F) -> Result<Self, CaptureError>
    where
        F: Fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u32, u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        let primary_monitor = Monitor::primary()
            .map_err(|e| CaptureError::InitializationFailed(format!("Монитор не найден: {:?}", e)))?;

        let width = primary_monitor
            .width()
            .map_err(|e| CaptureError::InitializationFailed(format!("Ширина монитора: {:?}", e)))?;
        let height = primary_monitor
            .height()
            .map_err(|e| CaptureError::InitializationFailed(format!("Высота монитора: {:?}", e)))?;

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_signal_clone = Arc::clone(&stop_signal);
        let cb_arc: OnFrameCallback = Arc::new(callback);

        let settings = Settings::new(
            primary_monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            WgcFlags {
                callback: cb_arc,
                stop_signal: stop_signal_clone,
            },
        );

        let thread_handle = thread::spawn(move || {
            let res = WgcCaptureHandler::start(settings);
            if let Err(ref e) = res {
                eprintln!("[WGC ERROR]: {:?}", e);
            }
            res.map_err(|e| CaptureError::AcquireFailed(format!("Ошибка WGC: {:?}", e)))
        });

        Ok(Self {
            stop_signal,
            thread_handle: Some(thread_handle),
            width,
            height,
        })
    }

    /// Остановка сессии захвата
    pub fn stop(&mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WgcCaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}
