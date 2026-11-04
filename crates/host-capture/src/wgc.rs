use std::sync::Arc;
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
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
}

struct WgcCaptureHandler {
    callback: OnFrameCallback,
}

impl GraphicsCaptureApiHandler for WgcCaptureHandler {
    type Flags = WgcFlags;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            callback: ctx.flags.callback,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
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
    capture_control: Option<CaptureControl<WgcCaptureHandler, Box<dyn std::error::Error + Send + Sync>>>,
    pub width: u32,
    pub height: u32,
}

impl WgcCaptureSession {
    /// Получить список всех доступных мониторов системы (индекс, ширина, высота)
    pub fn enumerate_monitors() -> Result<Vec<(usize, u32, u32)>, CaptureError> {
        let monitors = Monitor::enumerate()
            .map_err(|e| CaptureError::InitializationFailed(format!("Ошибка перечисления мониторов: {:?}", e)))?;
        let mut result = Vec::new();
        for (i, m) in monitors.into_iter().enumerate() {
            let w = m.width().unwrap_or(1920);
            let h = m.height().unwrap_or(1080);
            result.push((i, w, h));
        }
        Ok(result)
    }

    /// Запуск сессии захвата основного дисплея с передачей каждого готового GPU кадра в колбэк
    pub fn start<F>(callback: F) -> Result<Self, CaptureError>
    where
        F: Fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u32, u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        Self::start_monitor(0, true, callback)
    }

    /// Запуск сессии захвата конкретного монитора с выбором захвата курсора
    pub fn start_monitor<F>(monitor_index: usize, with_cursor: bool, callback: F) -> Result<Self, CaptureError>
    where
        F: Fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u32, u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        let all_monitors = Monitor::enumerate().ok();
        let monitor = if let Some(ref list) = all_monitors {
            if monitor_index < list.len() {
                list[monitor_index].clone()
            } else {
                Monitor::primary()
                    .map_err(|e| CaptureError::InitializationFailed(format!("Монитор не найден: {:?}", e)))?
            }
        } else {
            Monitor::primary()
                .map_err(|e| CaptureError::InitializationFailed(format!("Монитор не найден: {:?}", e)))?
        };

        let width = monitor
            .width()
            .map_err(|e| CaptureError::InitializationFailed(format!("Ширина монитора: {:?}", e)))?;
        let height = monitor
            .height()
            .map_err(|e| CaptureError::InitializationFailed(format!("Высота монитора: {:?}", e)))?;

        let cb_arc: OnFrameCallback = Arc::new(callback);

        let cursor_setting = if with_cursor {
            CursorCaptureSettings::WithCursor
        } else {
            CursorCaptureSettings::WithoutCursor
        };

        let settings = Settings::new(
            monitor,
            cursor_setting,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            WgcFlags {
                callback: cb_arc,
            },
        );

        let capture_control = WgcCaptureHandler::start_free_threaded(settings)
            .map_err(|e| CaptureError::InitializationFailed(format!("Ошибка старта WGC: {:?}", e)))?;

        Ok(Self {
            capture_control: Some(capture_control),
            width,
            height,
        })
    }

    /// Остановка сессии захвата
    pub fn stop(&mut self) {
        if let Some(control) = self.capture_control.take() {
            let _ = control.stop();
        }
    }
}

impl Drop for WgcCaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}
