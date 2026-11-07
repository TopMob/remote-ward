use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton as WinitMouseButton, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Fullscreen, Window, WindowId};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetDC, GetMonitorInfoW, MonitorFromWindow, ReleaseDC, SetStretchBltMode, StretchDIBits,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_STYLE, HWND_NOTOPMOST,
    HWND_TOP, SWP_FRAMECHANGED, SWP_NOZORDER, SWP_SHOWWINDOW, WS_CAPTION, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

use crate::config::AppConfig;
use crate::decoder::{DecodedFrame, MftVideoDecoder};
use crate::input_mapper::keycode_to_scancode;
use crate::launcher::{LauncherAction, LauncherUI};
use core_protocol::{
    ButtonState, ClientHello, ControlMessage, InputEvent, MouseButton, VideoCodec,
};
use core_transport::FrameReassembler;

pub struct ClientConfig {
    pub host_control_addr: SocketAddr,
    pub video_port: u16,
    pub width: u32,
    pub height: u32,
    pub preferred_codec: VideoCodec,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            host_control_addr: SocketAddr::from(([127, 0, 0, 1], 48001)),
            video_port: 48000,
            width: 2560,
            height: 1440,
            preferred_codec: VideoCodec::H264,
        }
    }
}

pub struct StreamingSession {
    pub input_tx: mpsc::UnboundedSender<InputEvent>,
    pub control_tx: mpsc::UnboundedSender<ControlMessage>,
    pub frame_rx: mpsc::Receiver<DecodedFrame>,
    pub buf_pool_tx: crossbeam_channel::Sender<Vec<u8>>,
    pub is_running: Arc<AtomicBool>,
    pub current_rtt_us: Arc<AtomicU32>,
    pub current_monitor_shared: Arc<AtomicU32>,
    pub host_addr: SocketAddr,
}

pub struct HostHandle {
    pub host: Arc<host_server::RemoteWardHost>,
    pub task: tokio::task::JoinHandle<()>,
}

pub struct RemoteWardClientApp {
    pub app_config: AppConfig,
    pub window: Option<Window>,
    pub launcher_ui: LauncherUI,
    pub egui_ctx: egui::Context,
    pub egui_winit_state: Option<egui_winit::State>,
    pub session: Option<StreamingSession>,
    pub session_rx: Option<mpsc::Receiver<Result<StreamingSession, String>>>,
    pub connecting_target: Option<SocketAddr>,
    pub host_handle: Option<HostHandle>,
    pub renderer: Option<crate::renderer::D3D11Renderer>,
    pub keyboard_hook: Option<crate::keyboard_hook::SystemKeyboardHook>,
    pub cursor_grabbed: bool,
    pub local_cursor_enabled: bool,
    pub ctrl_down: bool,
    pub shift_down: bool,
    pub rctrl_down: bool,
    pub last_monitor_switch: Instant,
    pub frames_rendered: u64,
    pub last_stats_instant: Instant,
    pub last_fps: u32,
    pub last_decode_ms: f64,
    pub bgra_buffer: Vec<u8>,
    pub last_frame_size: (u32, u32),
}

#[inline(always)]
fn clamp_u8(val: i32) -> u8 {
    val.clamp(0, 255) as u8
}

pub fn nv12_to_bgra(nv12: &[u8], width: usize, height: usize, bgra: &mut [u8]) {
    use rayon::prelude::*;

    let frame_y_size = width * height;
    if nv12.len() < frame_y_size + frame_y_size / 2 {
        return;
    }

    let (y_plane, uv_plane) = nv12.split_at(frame_y_size);

    bgra.par_chunks_exact_mut(width * 4 * 2)
        .enumerate()
        .for_each(|(pair_idx, two_rows_bgra)| {
            let y_row0 = pair_idx * 2;
            let y_row1 = y_row0 + 1;

            let uv_row_start = pair_idx * width;
            if uv_row_start + width > uv_plane.len() {
                return;
            }
            let uv_row = &uv_plane[uv_row_start..uv_row_start + width];

            let y_start0 = y_row0 * width;
            let y_start1 = y_row1 * width;

            if y_start1 + width > y_plane.len() {
                return;
            }

            let y_line0 = &y_plane[y_start0..y_start0 + width];
            let y_line1 = &y_plane[y_start1..y_start1 + width];

            let (row0_bgra, row1_bgra) = two_rows_bgra.split_at_mut(width * 4);

            for x in 0..width {
                let uv_idx = (x / 2) * 2;
                let u_raw = uv_row[uv_idx] as i32;
                let v_raw = uv_row[uv_idx + 1] as i32;

                let u = u_raw - 128;
                let v = v_raw - 128;

                let r_uv = 1436 * v;
                let g_uv = -352 * u - 731 * v;
                let b_uv = 1814 * u;

                let y0 = (y_line0[x] as i32 - 16).max(0);
                let c0 = 1192 * y0;
                let idx0 = x * 4;
                row0_bgra[idx0] = clamp_u8((c0 + b_uv + 512) >> 10);
                row0_bgra[idx0 + 1] = clamp_u8((c0 + g_uv + 512) >> 10);
                row0_bgra[idx0 + 2] = clamp_u8((c0 + r_uv + 512) >> 10);
                row0_bgra[idx0 + 3] = 255;

                let y1 = (y_line1[x] as i32 - 16).max(0);
                let c1 = 1192 * y1;
                let idx1 = x * 4;
                row1_bgra[idx1] = clamp_u8((c1 + b_uv + 512) >> 10);
                row1_bgra[idx1 + 1] = clamp_u8((c1 + g_uv + 512) >> 10);
                row1_bgra[idx1 + 2] = clamp_u8((c1 + r_uv + 512) >> 10);
                row1_bgra[idx1 + 3] = 255;
            }
        });
}

pub fn render_bgra_to_hwnd(hwnd: HWND, bgra: &[u8], width: u32, height: u32) {
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_invalid() {
            return;
        }

        let mut rect = RECT::default();
        let (offset_x, offset_y, target_w, target_h) = if GetClientRect(hwnd, &mut rect).is_ok() {
            let dest_w = rect.right - rect.left;
            let dest_h = rect.bottom - rect.top;
            if dest_w > 0 && dest_h > 0 && width > 0 && height > 0 {
                let scale_x = dest_w as f64 / width as f64;
                let scale_y = dest_h as f64 / height as f64;
                let scale = scale_x.min(scale_y);

                let tw = (width as f64 * scale).round() as i32;
                let th = (height as f64 * scale).round() as i32;
                let ox = (dest_w - tw) / 2;
                let oy = (dest_h - th) / 2;
                (ox, oy, tw, th)
            } else {
                (0, 0, dest_w, dest_h)
            }
        } else {
            (0, 0, width as i32, height as i32)
        };

        let _ = SetStretchBltMode(hdc, COLORONCOLOR);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default()],
        };

        let _ = StretchDIBits(
            hdc,
            offset_x,
            offset_y,
            target_w,
            target_h,
            0,
            0,
            width as i32,
            height as i32,
            Some(bgra.as_ptr() as *const _),
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );

        let _ = ReleaseDC(hwnd, hdc);
    }
}

impl RemoteWardClientApp {
    pub fn new(app_config: AppConfig) -> Self {
        let egui_ctx = egui::Context::default();
        LauncherUI::setup_theme(&egui_ctx);
        let launcher_ui = LauncherUI::new(&app_config);

        Self {
            app_config,
            window: None,
            launcher_ui,
            egui_ctx,
            egui_winit_state: None,
            session: None,
            session_rx: None,
            connecting_target: None,
            host_handle: None,
            renderer: None,
            keyboard_hook: None,
            cursor_grabbed: false,
            local_cursor_enabled: false,
            ctrl_down: false,
            shift_down: false,
            rctrl_down: false,
            last_monitor_switch: Instant::now() - Duration::from_secs(1),
            frames_rendered: 0,
            last_stats_instant: Instant::now(),
            last_fps: 60,
            last_decode_ms: 0.0,
            bgra_buffer: Vec::new(),
            last_frame_size: (0, 0),
        }
    }

    #[allow(dead_code)]
    fn get_hwnd(&self) -> Option<HWND> {
        let window = self.window.as_ref()?;
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                return Some(HWND(win32_handle.hwnd.get() as *mut _));
            }
        }
        None
    }

    fn get_letterbox(&self) -> (f64, f64, f64, f64) {
        if let Some(hwnd) = self.get_hwnd() {
            let mut rect = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut rect) }.is_ok() {
                let dest_w = (rect.right - rect.left) as f64;
                let dest_h = (rect.bottom - rect.top) as f64;
                let (host_w, host_h) = if self.last_frame_size.0 > 0 && self.last_frame_size.1 > 0 {
                    (self.last_frame_size.0 as f64, self.last_frame_size.1 as f64)
                } else {
                    (2560.0, 1440.0)
                };

                if dest_w > 0.0 && dest_h > 0.0 && host_w > 0.0 && host_h > 0.0 {
                    let scale = (dest_w / host_w).min(dest_h / host_h);
                    let target_w = (host_w * scale).round();
                    let target_h = (host_h * scale).round();
                    let offset_x = ((dest_w - target_w) / 2.0).round();
                    let offset_y = ((dest_h - target_h) / 2.0).round();
                    return (offset_x, offset_y, target_w, target_h);
                }
            }
        }
        (0.0, 0.0, 1920.0, 1080.0)
    }

    pub fn set_fullscreen_mode(&mut self, fullscreen: bool) {
        let window = match self.window.as_ref() {
            Some(w) => w,
            None => return,
        };

        if fullscreen {
            tracing::info!(
                "Переключение окна в честный полноэкранный режим (Borderless Fullscreen)..."
            );
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));

            // Гарантируем через Win32 API, что окно занимает весь физический экран монитора
            if let Some(hwnd) = self.get_hwnd() {
                unsafe {
                    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut mi = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                        let rc = mi.rcMonitor;
                        let w = rc.right - rc.left;
                        let h = rc.bottom - rc.top;

                        let style = GetWindowLongW(hwnd, GWL_STYLE);
                        let clean_style = (style
                            & !(WS_CAPTION.0 as i32
                                | WS_THICKFRAME.0 as i32
                                | WS_MINIMIZEBOX.0 as i32
                                | WS_MAXIMIZEBOX.0 as i32
                                | WS_SYSMENU.0 as i32))
                            | WS_POPUP.0 as i32;
                        SetWindowLongW(hwnd, GWL_STYLE, clean_style);

                        let _ = SetWindowPos(
                            hwnd,
                            HWND_TOP,
                            rc.left,
                            rc.top,
                            w,
                            h,
                            SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                        );

                        if let Some(ref mut renderer) = self.renderer {
                            renderer.resize(w as u32, h as u32);
                        }
                    }
                }
            }
        } else {
            tracing::info!("Возврат окна в режим компактного лаунчера (960x640)...");
            window.set_fullscreen(None);

            if let Some(hwnd) = self.get_hwnd() {
                unsafe {
                    let style = GetWindowLongW(hwnd, GWL_STYLE);
                    let windowed_style = (style & !(WS_POPUP.0 as i32))
                        | (WS_OVERLAPPED.0 as i32
                            | WS_CAPTION.0 as i32
                            | WS_SYSMENU.0 as i32
                            | WS_MINIMIZEBOX.0 as i32);
                    SetWindowLongW(hwnd, GWL_STYLE, windowed_style);

                    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut mi = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let (x, y) = if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                        let mon_w = mi.rcMonitor.right - mi.rcMonitor.left;
                        let mon_h = mi.rcMonitor.bottom - mi.rcMonitor.top;
                        (
                            mi.rcMonitor.left + (mon_w - 960) / 2,
                            mi.rcMonitor.top + (mon_h - 640) / 2,
                        )
                    } else {
                        (100, 100)
                    };

                    let _ = SetWindowPos(
                        hwnd,
                        HWND_NOTOPMOST,
                        x,
                        y,
                        960,
                        640,
                        SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    );

                    if let Some(ref mut renderer) = self.renderer {
                        renderer.resize(960, 640);
                    }
                }
            }
        }
    }

    fn toggle_hud_overlay(&mut self) {
        self.launcher_ui.show_overlay = !self.launcher_ui.show_overlay;
        if let Some(ref window) = self.window {
            if self.launcher_ui.show_overlay {
                window.set_cursor_visible(true);
                let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
                self.cursor_grabbed = false;
                if let Some(ref hook) = self.keyboard_hook {
                    hook.set_active(false);
                }
                tracing::info!("HUD Оверлей ОТКРЫТ (Ctrl+Shift+O): курсор освобожден для UI");
            } else {
                window.set_cursor_visible(true);
                let grab_mode = if self.cursor_grabbed {
                    winit::window::CursorGrabMode::Confined
                } else {
                    winit::window::CursorGrabMode::None
                };
                let _ = window.set_cursor_grab(grab_mode);
                if let Some(ref hook) = self.keyboard_hook {
                    hook.set_active(true);
                }
                tracing::info!("HUD Оверлей ЗАКРЫТ: курсор возвращен в стрим");
            }
        }
    }

    fn disconnect_to_launcher(&mut self) {
        if let Some(ref session) = self.session {
            let _ = session.control_tx.send(ControlMessage::Disconnect {
                session_id: 1,
                reason: "Сеанс завершен пользователем на клиенте".into(),
            });
            session.is_running.store(false, Ordering::Relaxed);
        }
        self.session = None;
        self.connecting_target = None;
        self.session_rx = None;
        self.launcher_ui.show_overlay = false;

        self.set_fullscreen_mode(false);

        if let Some(ref window) = self.window {
            window.set_cursor_visible(true);
            let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
            self.cursor_grabbed = false;
            window.set_title("Remote-Ward");
        }

        if let Some(ref hook) = self.keyboard_hook {
            hook.set_active(false);
        }

        tracing::info!("Сеанс стриминга завершен. Окно вернулось в лаунчер Remote-Ward!");
    }

    fn start_connection(&mut self, target_addr: SocketAddr) {
        self.connecting_target = Some(target_addr);
        self.launcher_ui.error_message = None;

        let (tx, rx) = mpsc::channel(1);
        self.session_rx = Some(rx);

        let config_clone = self.app_config.clone();
        tokio::spawn(async move {
            let res = spawn_streaming_pipeline(target_addr, &config_clone).await;
            let _ = tx.send(res).await;
        });
    }
}

impl ApplicationHandler for RemoteWardClientApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // Лаунчер открывается в аккуратном компактном окне 960x640
            let window_attributes = Window::default_attributes()
                .with_title("Remote-Ward")
                .with_inner_size(winit::dpi::LogicalSize::new(960.0, 640.0))
                .with_resizable(true)
                .with_drag_and_drop(false);

            match event_loop.create_window(window_attributes) {
                Ok(window) => {
                    tracing::info!("Окно лаунчера Remote-Ward успешно создано (960x640)");
                    window.set_cursor_visible(true);
                    let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);

                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                            let hwnd = HWND(win32_handle.hwnd.get() as *mut _);

                            // Dummy input_tx для хука в режиме лаунчера
                            let (dummy_tx, _) = mpsc::unbounded_channel();
                            let is_running_dummy = Arc::new(AtomicBool::new(true));
                            let hook = crate::keyboard_hook::SystemKeyboardHook::install(
                                hwnd,
                                is_running_dummy,
                                dummy_tx,
                            );
                            hook.set_active(false);
                            self.keyboard_hook = Some(hook);

                            let inner_size = window.inner_size();
                            let render_w = if inner_size.width > 0 {
                                inner_size.width
                            } else {
                                960
                            };
                            let render_h = if inner_size.height > 0 {
                                inner_size.height
                            } else {
                                640
                            };
                            match crate::renderer::D3D11Renderer::new(hwnd, render_w, render_h) {
                                Ok(d3d11) => {
                                    tracing::info!("Аппаратный рендерер Direct3D 11 для лаунчера и стрима готов!");
                                    self.renderer = Some(d3d11);
                                }
                                Err(e) => {
                                    tracing::warn!("Не удалось инициализировать D3D11: {:?}", e);
                                }
                            }
                        }
                    }

                    // Инициализация egui-winit состояния
                    let egui_state = egui_winit::State::new(
                        self.egui_ctx.clone(),
                        egui::ViewportId::ROOT,
                        &window,
                        Some(window.scale_factor() as f32),
                        None,
                        None,
                    );
                    self.egui_winit_state = Some(egui_state);
                    self.window = Some(window);
                }
                Err(e) => {
                    tracing::error!("Ошибка создания окна приложения: {:?}", e);
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_overlay = self.launcher_ui.show_overlay;
        let is_streaming = self.session.is_some() && !is_overlay;

        // Если активен лаунчер или открыт оверлей, события окна передаются в egui
        if !is_streaming {
            if let Some(ref window) = self.window {
                if let Some(ref mut state) = self.egui_winit_state {
                    let response = state.on_window_event(window, &event);
                    if response.consumed {
                        return;
                    }
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("Закрытие окна приложения Remote-Ward...");
                self.disconnect_to_launcher();
                if let Some(h) = self.host_handle.take() {
                    h.host.stop();
                    h.task.abort();
                }
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if let Some(ref mut renderer) = self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }

            WindowEvent::Focused(is_focused) => {
                if let Some(ref hook) = self.keyboard_hook {
                    hook.set_active(
                        is_focused && self.session.is_some() && !self.launcher_ui.show_overlay,
                    );
                }
                if !is_focused && is_streaming {
                    if let Some(ref session) = self.session {
                        let _ = session.input_tx.send(InputEvent::MouseButton {
                            button: MouseButton::Left,
                            state: ButtonState::Released,
                        });
                        let _ = session.input_tx.send(InputEvent::MouseButton {
                            button: MouseButton::Right,
                            state: ButtonState::Released,
                        });
                    }
                }
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(key_code) = key_event.physical_key {
                    let is_pressed = key_event.state == ElementState::Pressed;

                    if key_code == KeyCode::ControlLeft || key_code == KeyCode::ControlRight {
                        self.ctrl_down = is_pressed;
                    }
                    if key_code == KeyCode::ShiftLeft || key_code == KeyCode::ShiftRight {
                        self.shift_down = is_pressed;
                    }
                    if key_code == KeyCode::ControlRight {
                        self.rctrl_down = is_pressed;
                    }

                    // ШОРТКАТ Ctrl + Shift + O: Внутриигровой HUD оверлей
                    if self.ctrl_down
                        && self.shift_down
                        && key_code == KeyCode::KeyO
                        && is_pressed
                        && self.session.is_some()
                    {
                        self.toggle_hud_overlay();
                        return;
                    }

                    // В режиме оверлея: клавиша Escape закрывает оверлей и возвращает мышь в игру
                    if is_overlay && key_code == KeyCode::Escape && is_pressed {
                        self.toggle_hud_overlay();
                        return;
                    }

                    // Экстренное завершение стрима: Right Ctrl + Esc
                    if (self.rctrl_down || key_code == KeyCode::ControlRight)
                        && key_code == KeyCode::Escape
                        && is_pressed
                    {
                        if self.session.is_some() {
                            self.disconnect_to_launcher();
                            return;
                        } else {
                            event_loop.exit();
                            return;
                        }
                    }

                    // Если мы находимся в активном стриме, обрабатываем управление и транслируем на хост
                    if let Some(ref session) = self.session {
                        if !is_overlay {
                            // F8: Переключение активного монитора хоста
                            if key_code == KeyCode::F8 && is_pressed {
                                if self.last_monitor_switch.elapsed() >= Duration::from_millis(300)
                                {
                                    self.last_monitor_switch = Instant::now();
                                    let next_idx = session
                                        .current_monitor_shared
                                        .load(Ordering::Relaxed)
                                        .wrapping_add(1);
                                    session
                                        .current_monitor_shared
                                        .store(next_idx, Ordering::Relaxed);
                                    let _ =
                                        session.control_tx.send(ControlMessage::SwitchMonitor {
                                            session_id: 1,
                                            monitor_index: next_idx,
                                        });
                                }
                                return;
                            }

                            // F10: Локальный аппаратный курсор
                            if key_code == KeyCode::F10 && is_pressed {
                                self.local_cursor_enabled = !self.local_cursor_enabled;
                                if let Some(ref window) = self.window {
                                    window.set_cursor_visible(self.local_cursor_enabled);
                                }
                                let _ = session.control_tx.send(ControlMessage::SetLocalCursor {
                                    session_id: 1,
                                    enabled: self.local_cursor_enabled,
                                });
                                return;
                            }

                            // F11 / Right Ctrl + H: Переключение режима мыши (Рабочий стол / Игровой захват 3D камеры)
                            if (key_code == KeyCode::F11
                                || (self.rctrl_down && key_code == KeyCode::KeyH))
                                && is_pressed
                            {
                                self.cursor_grabbed = !self.cursor_grabbed;
                                if let Some(ref window) = self.window {
                                    window.set_cursor_visible(!self.cursor_grabbed);
                                    let grab_mode = if self.cursor_grabbed {
                                        winit::window::CursorGrabMode::Confined
                                    } else {
                                        winit::window::CursorGrabMode::None
                                    };
                                    let _ = window.set_cursor_grab(grab_mode);
                                }
                                if self.cursor_grabbed {
                                    tracing::info!("Игровой режим (Raw Relative Mouse): курсор скрыт и захвачен для 3D-камеры");
                                } else {
                                    tracing::info!("Режим рабочего стола (Absolute Mouse): курсор виден для управления окнами");
                                }
                                return;
                            }

                            // Трансляция клавиши на хост через аппаратный Scan Code
                            if let Some((scan_code, is_extended)) = keycode_to_scancode(key_code) {
                                let state = match key_event.state {
                                    ElementState::Pressed => ButtonState::Pressed,
                                    ElementState::Released => ButtonState::Released,
                                };
                                let _ = session.input_tx.send(InputEvent::Keyboard {
                                    scan_code,
                                    is_extended,
                                    state,
                                });
                            }
                        }
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ref session) = self.session {
                    if !is_overlay && !self.cursor_grabbed {
                        let (ox, oy, tw, th) = self.get_letterbox();
                        let (fw, fh) = if self.last_frame_size.0 > 0 && self.last_frame_size.1 > 0 {
                            (self.last_frame_size.0 as f64, self.last_frame_size.1 as f64)
                        } else {
                            (2560.0, 1440.0)
                        };

                        if tw > 0.0 && th > 0.0 {
                            let norm_x = ((position.x - ox) / tw).clamp(0.0, 1.0);
                            let norm_y = ((position.y - oy) / th).clamp(0.0, 1.0);
                            let host_x = (norm_x * (fw - 1.0)).round() as u32;
                            let host_y = (norm_y * (fh - 1.0)).round() as u32;
                            let _ = session.input_tx.send(InputEvent::MouseMoveAbsolute {
                                x: host_x,
                                y: host_y,
                            });
                        }
                    }
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                if let Some(ref session) = self.session {
                    if !is_overlay {
                        let mb = match button {
                            WinitMouseButton::Left => MouseButton::Left,
                            WinitMouseButton::Right => MouseButton::Right,
                            WinitMouseButton::Middle => MouseButton::Middle,
                            WinitMouseButton::Back => MouseButton::X1,
                            WinitMouseButton::Forward => MouseButton::X2,
                            _ => MouseButton::Left,
                        };

                        let state = match btn_state {
                            ElementState::Pressed => ButtonState::Pressed,
                            ElementState::Released => ButtonState::Released,
                        };

                        let _ = session
                            .input_tx
                            .send(InputEvent::MouseButton { button: mb, state });
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(ref session) = self.session {
                    if !is_overlay {
                        let (delta_x, delta_y) = match delta {
                            winit::event::MouseScrollDelta::LineDelta(x, y) => {
                                ((x * 120.0) as i16, (y * 120.0) as i16)
                            }
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                                (pos.x as i16, pos.y as i16)
                            }
                        };
                        let _ = session
                            .input_tx
                            .send(InputEvent::MouseWheel { delta_y, delta_x });
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                if let Some(ref window) = self.window {
                    window.pre_present_notify();
                }
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if self.session.is_some() && self.cursor_grabbed && !self.launcher_ui.show_overlay {
            if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
                if dx != 0.0 || dy != 0.0 {
                    if let Some(ref session) = self.session {
                        let _ = session.input_tx.send(InputEvent::MouseMoveRelative {
                            dx: dx as i32,
                            dy: dy as i32,
                        });
                    }
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(1),
        ));

        // 1. Проверяем результат асинхронного подключения
        if let Some(ref mut rx) = self.session_rx {
            if let Ok(res) = rx.try_recv() {
                self.session_rx = None;
                self.connecting_target = None;
                match res {
                    Ok(session) => {
                        tracing::info!(
                            "Подключение успешно! Разворачиваем окно в полноэкранный стрим."
                        );
                        self.set_fullscreen_mode(true);

                        if let Some(ref window) = self.window {
                            window.set_cursor_visible(true);
                            let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
                            self.cursor_grabbed = false;
                        }
                        if let Some(ref hook) = self.keyboard_hook {
                            hook.update_sender(session.input_tx.clone());
                            hook.set_active(true);
                        }
                        self.session = Some(session);
                    }
                    Err(err) => {
                        tracing::warn!("Не удалось подключиться к хосту: {}", err);
                        self.launcher_ui.error_message =
                            Some(format!("Ошибка подключения: {}", err));
                        self.disconnect_to_launcher();
                    }
                }
            }
        }

        // 2. Отрисовка кадров
        let window = match self.window.as_ref() {
            Some(w) => w,
            None => return,
        };

        let inner_size = window.inner_size();
        let screen_w = inner_size.width;
        let screen_h = inner_size.height;

        let window_hwnd = if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                Some(HWND(win32_handle.hwnd.get() as *mut _))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(ref mut session) = self.session {
            // Режим СТРИМИНГА
            let mut latest_frame: Option<crate::decoder::DecodedFrame> = None;
            while let Ok(frame) = session.frame_rx.try_recv() {
                // Если в очереди было несколько кадров, возвращаем пропущенные буферы в пул
                // предотвращая утечку слотов пула и повторный heap-churn (Zero-Allocation)
                if let Some(prev) = latest_frame.replace(frame) {
                    let _ = session.buf_pool_tx.try_send(prev.data);
                }
            }

            let mut hud_action = LauncherAction::None;
            let mut overlay_draw_data = None;

            // Если открыт HUD оверлей, формируем интерфейс egui
            if self.launcher_ui.show_overlay {
                if let Some(ref mut state) = self.egui_winit_state {
                    let raw_input = state.take_egui_input(window);
                    let rtt = session.current_rtt_us.load(Ordering::Relaxed) as f32 / 1000.0;
                    let fps = self.last_fps;
                    let decode_ms = self.last_decode_ms;
                    let bitrate = self.app_config.bitrate_mbps as f32;

                    let full_output = self.egui_ctx.run(raw_input, |ctx| {
                        hud_action = self
                            .launcher_ui
                            .draw_in_stream_hud(ctx, rtt, fps, decode_ms, bitrate);
                    });
                    state.handle_platform_output(window, full_output.platform_output);
                    overlay_draw_data = Some((
                        full_output.shapes,
                        full_output.textures_delta,
                        full_output.pixels_per_point,
                    ));
                }
            }

            match hud_action {
                LauncherAction::Disconnect => {
                    self.disconnect_to_launcher();
                    return;
                }
                LauncherAction::SwitchMonitor => {
                    let next_idx = session
                        .current_monitor_shared
                        .load(Ordering::Relaxed)
                        .wrapping_add(1);
                    session
                        .current_monitor_shared
                        .store(next_idx, Ordering::Relaxed);
                    let _ = session.control_tx.send(ControlMessage::SwitchMonitor {
                        session_id: 1,
                        monitor_index: next_idx,
                    });
                }
                _ => {}
            }

            if let Some(frame) = latest_frame {
                self.frames_rendered += 1;
                self.last_decode_ms = frame.latency_us as f64 / 1000.0;
                self.last_frame_size = (frame.width, frame.height);

                if let Some(ref mut d3d11) = self.renderer {
                    let _ = d3d11.draw_nv12(&frame.data, frame.width, frame.height);

                    // Если оверлей открыт, рисуем его поверх видеокадра
                    if let Some((shapes, delta, ppp)) = overlay_draw_data {
                        let _ = d3d11.update_egui_textures(&delta);
                        let clipped = self.egui_ctx.tessellate(shapes, ppp);
                        let _ = d3d11.paint_egui(screen_w, screen_h, ppp, &clipped);
                    }

                    if let Err(e) = d3d11.present() {
                        tracing::warn!("Ошибка Direct3D11 Present(): {:?}", e);
                        if d3d11.is_device_lost() {
                            tracing::warn!("Обнаружен сброс GPU драйвера (DXGI_ERROR_DEVICE_REMOVED). Переинициализация D3D11Renderer...");
                            self.renderer = None; // Освобождаем старый Device и SwapChain
                            if let Some(hwnd) = window_hwnd {
                                if let Ok(new_r) =
                                    crate::renderer::D3D11Renderer::new(hwnd, screen_w, screen_h)
                                {
                                    self.renderer = Some(new_r);
                                }
                            }
                        }
                    }
                }

                // Возвращаем буфер в пул для следующего кадра (Zero-Allocation)
                let _ = session.buf_pool_tx.try_send(frame.data);
            }

            // Статистика FPS
            if self.last_stats_instant.elapsed() >= Duration::from_secs(1) {
                self.last_fps = self.frames_rendered as u32;
                self.frames_rendered = 0;
                self.last_stats_instant = Instant::now();
                let rtt_ms = session.current_rtt_us.load(Ordering::Relaxed) as f32 / 1000.0;
                window.set_title(&format!(
                    "Remote-Ward | {} FPS [FastSync] | RTT: {:.1} мс | Декод: {:.1} мс (Ctrl+Shift+O: меню)",
                    self.last_fps, rtt_ms, self.last_decode_ms
                ));
            }
        } else {
            // Режим ЛАУНЧЕРА
            // Обновляем статус встроенного хост-сервера для отображения в UI
            if let Some(ref h) = self.host_handle {
                let state = h.host.state();
                let is_running = state.is_running.load(Ordering::Relaxed);
                let client = *state.client_endpoint.read().unwrap();
                let frames = state.frame_counter.load(Ordering::Relaxed);
                self.launcher_ui.host_server_info = Some(crate::launcher::HostLiveInfo {
                    is_running,
                    client_addr: client,
                    frames_streamed: frames,
                });
            } else {
                self.launcher_ui.host_server_info = None;
            }

            let mut launcher_action = LauncherAction::None;
            let mut launcher_draw_data = None;

            if let Some(ref mut state) = self.egui_winit_state {
                let raw_input = state.take_egui_input(window);
                let full_output = self.egui_ctx.run(raw_input, |ctx| {
                    launcher_action = self.launcher_ui.draw_launcher(ctx, &mut self.app_config);
                });
                state.handle_platform_output(window, full_output.platform_output);
                launcher_draw_data = Some((
                    full_output.shapes,
                    full_output.textures_delta,
                    full_output.pixels_per_point,
                ));
            }

            match launcher_action {
                LauncherAction::Connect(target_addr) => {
                    self.start_connection(target_addr);
                }
                LauncherAction::ToggleHostServer => {
                    if self.host_handle.is_none() {
                        tracing::info!("Запуск встроенного хост-сервера Remote-Ward...");
                        let host = Arc::new(host_server::RemoteWardHost::new(
                            host_server::HostConfig::default(),
                        ));
                        let host_clone = Arc::clone(&host);
                        let task = tokio::spawn(async move {
                            if let Err(e) = host_clone.run().await {
                                tracing::error!("Ошибка работы хост-сервера: {:?}", e);
                            }
                        });
                        self.host_handle = Some(HostHandle { host, task });
                    } else {
                        tracing::info!("Остановка встроенного хост-сервера Remote-Ward...");
                        if let Some(h) = self.host_handle.take() {
                            h.host.stop();
                            h.task.abort();
                        }
                    }
                }
                _ => {}
            }

            if let Some(ref mut d3d11) = self.renderer {
                // Очистка SwapChain глубоким обсидиановым фоном #0c0a14
                d3d11.clear([0.047, 0.039, 0.078, 1.0]);

                if let Some((shapes, delta, ppp)) = launcher_draw_data {
                    let _ = d3d11.update_egui_textures(&delta);
                    let clipped = self.egui_ctx.tessellate(shapes, ppp);
                    let _ = d3d11.paint_egui(screen_w, screen_h, ppp, &clipped);
                }

                let _ = d3d11.present();
            }
        }
    }
}

/// Асинхронное создание и запуск всех сетевых компонентов стрима
async fn spawn_streaming_pipeline(
    host_ctrl_addr: SocketAddr,
    config: &AppConfig,
) -> Result<StreamingSession, String> {
    let host_video_port = 48000u16;
    let is_running = Arc::new(AtomicBool::new(true));
    let current_rtt_us = Arc::new(AtomicU32::new(0));

    // 1. Создаем сокет управления на динамическом свободном порту
    let control_socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Не удалось создать сокет управления: {:?}", e))?;
    let control_socket = Arc::new(control_socket);
    core_transport::optimize_udp_socket(&control_socket);

    // 2. Создаем видеосокет на динамическом свободном порту (0.0.0.0:0) с буфером 8 МБ
    let video_socket_std = std::net::UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("Не удалось открыть локальный видеосокет: {:?}", e))?;
    let local_video_port = video_socket_std
        .local_addr()
        .map_err(|e| format!("Ошибка определения локального видеопорта: {:?}", e))?
        .port();

    let sock_ref = socket2::SockRef::from(&video_socket_std);
    let _ = sock_ref.set_recv_buffer_size(8 * 1024 * 1024);
    let _ = video_socket_std.set_nonblocking(true);
    let video_socket = UdpSocket::from_std(video_socket_std)
        .map_err(|e| format!("Tokio UDP socket error: {:?}", e))?;
    let video_socket = Arc::new(video_socket);
    core_transport::optimize_udp_socket(&video_socket);

    // 3. Фоновая отправка PUNCH пакетов для пробивки NAT на видеопорт хоста (48000)
    let host_video_addr = SocketAddr::new(host_ctrl_addr.ip(), host_video_port);
    let is_running_punch = Arc::clone(&is_running);
    let video_socket_punch = Arc::clone(&video_socket);
    tokio::spawn(async move {
        let punch_header = core_protocol::DatagramHeader::new_hole_punch(1);
        let punch_bytes = punch_header.as_bytes();
        while is_running_punch.load(Ordering::Relaxed) {
            let _ = video_socket_punch
                .send_to(punch_bytes, host_video_addr)
                .await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<InputEvent>();
    let (frame_tx, frame_rx) = mpsc::channel::<DecodedFrame>(4);
    let (raw_frame_tx, mut raw_frame_rx) = mpsc::channel::<core_transport::ReassembledFrame>(4);
    let current_monitor_shared = Arc::new(AtomicU32::new(0));
    let current_monitor_shared_ctrl = Arc::clone(&current_monitor_shared);

    // 4. Сетевая калибровка
    let session_id = 1u64;
    let _calibration =
        crate::calibration::run_network_calibration(&control_socket, host_ctrl_addr, session_id)
            .await;

    let target_bitrate = (config.bitrate_mbps * 1000).max(10_000);
    let target_fps = config.target_fps;

    let settings_msg = ControlMessage::ChangeStreamSettings {
        session_id,
        target_bitrate_kbps: target_bitrate,
        target_fps,
    };
    if let Ok(settings_bytes) = settings_msg.to_packet() {
        let _ = control_socket
            .send_to(&settings_bytes, host_ctrl_addr)
            .await;
    }

    // 5. Отправляем ClientHello с динамическим портом local_video_port
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "remote-ward-client".into(),
        client_version: "0.1.0".into(),
        protocol_version: core_protocol::PROTOCOL_VERSION as u32,
        client_token: 0,
        supported_codecs: MftVideoDecoder::supported_codecs(),
        preferred_codec: Some(config.preferred_codec),
        screen_width: 2560,
        screen_height: 1440,
        target_fps,
        display_refresh_rate: target_fps,
        supports_tearing: true,
        dpi_scale: 1.0,
        video_port: local_video_port,
        supports_nack: true,
        supports_fec: true,
    });
    let hello_bytes = hello.to_packet().map_err(|e| format!("{:?}", e))?;
    control_socket
        .send_to(&hello_bytes, host_ctrl_addr)
        .await
        .map_err(|e| format!("{:?}", e))?;

    // 6. Фоновый прием ServerHello, Pong
    let is_running_ctrl_rx = Arc::clone(&is_running);
    let ctrl_socket_rx = Arc::clone(&control_socket);
    let current_rtt_rx = Arc::clone(&current_rtt_us);
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while is_running_ctrl_rx.load(Ordering::Relaxed) {
            match ctrl_socket_rx.recv_from(&mut buf).await {
                Ok((len, _src)) => {
                    if let Ok(msg) = ControlMessage::from_packet(&buf[..len]) {
                        match msg {
                            ControlMessage::ServerHello(h) => {
                                tracing::info!(
                                    "Хост подтвердил сеанс: {} [{}x{} @ {} FPS]",
                                    h.server_name,
                                    h.width,
                                    h.height,
                                    h.target_fps
                                );
                            }
                            ControlMessage::SwitchMonitorAck { monitor_index, .. } => {
                                current_monitor_shared_ctrl.store(monitor_index, Ordering::Relaxed);
                            }
                            ControlMessage::Pong {
                                send_timestamp_us, ..
                            } => {
                                let now_us = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_micros()
                                    as u64;
                                let rtt = now_us.saturating_sub(send_timestamp_us) as u32;
                                current_rtt_rx.store(rtt, Ordering::Relaxed);
                            }
                            _ => {}
                        }
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    });

    // 7. Фоновый Ping каждые 1.5 секунды
    let is_running_ping = Arc::clone(&is_running);
    let ctrl_socket_ping = Arc::clone(&control_socket);
    tokio::spawn(async move {
        let mut seq = 1000u32;
        while is_running_ping.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            seq += 1;
            let now_us = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;
            let ping = ControlMessage::Ping {
                session_id,
                sequence: seq,
                send_timestamp_us: now_us,
            };
            if let Ok(b) = ping.to_packet() {
                let _ = ctrl_socket_ping.send_to(&b, host_ctrl_addr).await;
            }
        }
    });

    // 8. Передача пользовательского ввода (Zero-Allocation 1000 Hz)
    let is_running_input = Arc::clone(&is_running);
    let ctrl_socket_input = Arc::clone(&control_socket);
    tokio::spawn(async move {
        let mut input_buf = [0u8; 64];
        while let Some(event) = input_rx.recv().await {
            if !is_running_input.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(len) = event.write_to_slice(&mut input_buf) {
                let _ = ctrl_socket_input
                    .send_to(&input_buf[..len], host_ctrl_addr)
                    .await;
            }
        }
    });

    // 9. Поток декодера MFT
    let is_running_dec = Arc::clone(&is_running);
    let preferred_codec = config.preferred_codec;
    let frame_tx_decoder = frame_tx.clone();
    let initial_width = 2560u32;
    let initial_height = 1440u32;

    // Пул буферов кадров (Zero-Allocation Buffer Pool)
    let (buf_pool_tx, buf_pool_rx) = crossbeam_channel::bounded::<Vec<u8>>(8);
    let buf_pool_tx_session = buf_pool_tx.clone();

    std::thread::Builder::new()
        .name("video-decoder".into())
        .spawn(move || {
            // SAFETY: Инициализация COM MTA для MFT декодера в системном потоке
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                );
            }

            let effective_codec = if MftVideoDecoder::is_codec_supported(preferred_codec) {
                preferred_codec
            } else {
                VideoCodec::H264
            };
            let mut decoder_opt =
                MftVideoDecoder::new(initial_width, initial_height, effective_codec).ok();

            while is_running_dec.load(Ordering::Relaxed) {
                match raw_frame_rx.blocking_recv() {
                    Some(assembled_frame) => {
                        let mut buffer = buf_pool_rx.try_recv().unwrap_or_else(|_| {
                            Vec::with_capacity((initial_width * initial_height * 3 / 2) as usize)
                        });
                        if let Some(ref mut decoder) = decoder_opt {
                            match decoder.decode_into(&assembled_frame.data, &mut buffer) {
                                Ok(Some((w, h, lat))) => {
                                    let _ = frame_tx_decoder.try_send(DecodedFrame {
                                        width: w,
                                        height: h,
                                        data: buffer,
                                        latency_us: lat,
                                    });
                                }
                                Ok(None) => {
                                    let _ = buf_pool_tx.try_send(buffer);
                                }
                                Err(e) => {
                                    tracing::warn!("Ошибка декодера: {:?}", e);
                                    if assembled_frame.is_keyframe {
                                        if let Ok(mut new_dec) = MftVideoDecoder::new(
                                            initial_width,
                                            initial_height,
                                            effective_codec,
                                        ) {
                                            if let Ok(Some((w, h, lat))) = new_dec
                                                .decode_into(&assembled_frame.data, &mut buffer)
                                            {
                                                let _ = frame_tx_decoder.try_send(DecodedFrame {
                                                    width: w,
                                                    height: h,
                                                    data: buffer,
                                                    latency_us: lat,
                                                });
                                            } else {
                                                let _ = buf_pool_tx.try_send(buffer);
                                            }
                                            *decoder = new_dec;
                                        } else {
                                            let _ = buf_pool_tx.try_send(buffer);
                                        }
                                    } else {
                                        let _ = buf_pool_tx.try_send(buffer);
                                    }
                                }
                            }
                        } else if assembled_frame.is_keyframe {
                            if let Ok(mut new_dec) =
                                MftVideoDecoder::new(initial_width, initial_height, effective_codec)
                            {
                                if let Ok(Some((w, h, lat))) =
                                    new_dec.decode_into(&assembled_frame.data, &mut buffer)
                                {
                                    let _ = frame_tx_decoder.try_send(DecodedFrame {
                                        width: w,
                                        height: h,
                                        data: buffer,
                                        latency_us: lat,
                                    });
                                } else {
                                    let _ = buf_pool_tx.try_send(buffer);
                                }
                                decoder_opt = Some(new_dec);
                            } else {
                                let _ = buf_pool_tx.try_send(buffer);
                            }
                        } else {
                            let _ = buf_pool_tx.try_send(buffer);
                        }
                    }
                    None => break,
                }
            }

            // SAFETY: Все COM-объекты декодера (IMFTransform) обязаны быть освобождены ДО вызова CoUninitialize()
            drop(decoder_opt);

            // SAFETY: Деинициализация COM-апартаментов при завершении потока декодера
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        })
        .map_err(|e| format!("Не удалось запустить поток декодера: {:?}", e))?;

    // 10. Поток сборки видеопакетов (FrameReassembler + NACK + FEC)
    let is_running_rx = Arc::clone(&is_running);
    let video_socket_task = Arc::clone(&video_socket);
    let ctrl_socket_task = Arc::clone(&control_socket);
    let current_rtt_stats = Arc::clone(&current_rtt_us);
    let audio_player = Arc::new(crate::audio_player::WasapiAudioPlayer::new().ok());
    let audio_player_task = Arc::clone(&audio_player);

    std::thread::Builder::new()
        .name("video-receiver".into())
        .spawn(move || {
            let mut reassembler = FrameReassembler::new(Duration::from_millis(50));
            let mut buf = [0u8; 2048];

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                let mut last_frame_id_opt: Option<u32> = None;
                let mut waiting_for_clean_keyframe = false;
                let mut packets_received_window = 0u32;
                let mut packets_lost_window = 0u32;
                let mut nack_sent_window = 0u32;
                let mut last_stats_sent = Instant::now();
                let mut last_nack_instant = Instant::now() - Duration::from_millis(50);
                let mut last_idr_request = Instant::now() - Duration::from_secs(1);

                while is_running_rx.load(Ordering::Relaxed) {
                    if last_stats_sent.elapsed() >= Duration::from_millis(1000) {
                        let total_pkts = packets_received_window + packets_lost_window;
                        let loss_rate = if total_pkts >= 50 {
                            packets_lost_window as f32 / total_pkts as f32
                        } else {
                            0.0
                        };
                        let stats = ControlMessage::ClientStats {
                            session_id,
                            rtt_us: current_rtt_stats.load(Ordering::Relaxed),
                            jitter_us: 0,
                            decode_latency_us: 0,
                            render_latency_us: 0,
                            packet_loss_rate: loss_rate,
                            fec_recovered_count: 0,
                            nack_sent_count: nack_sent_window,
                        };
                        if let Ok(bytes) = stats.to_packet() {
                            let _ = ctrl_socket_task.send_to(&bytes, host_ctrl_addr).await;
                        }
                        packets_received_window = 0;
                        packets_lost_window = 0;
                        nack_sent_window = 0;
                        last_stats_sent = Instant::now();
                    }

                    match video_socket_task.recv_from(&mut buf).await {
                        Ok((len, _src)) => {
                            if len < 4 || buf[0] == core_protocol::MSG_TYPE_CONTROL {
                                continue;
                            }

                            if len < core_protocol::DatagramHeader::SIZE {
                                continue;
                            }

                            let (hdr, payload) =
                                match core_protocol::DatagramHeader::parse(&buf[..len]) {
                                    Ok(res) => res,
                                    Err(_) => continue,
                                };

                            if matches!(
                                hdr.get_channel(),
                                Ok(core_protocol::ChannelId::AudioHostToClient)
                            ) {
                                if let Some(ref player) = *audio_player_task {
                                    // 16-bit stereo PCM = 4 байта на сэмпл, строго выравниваем длину
                                    let aligned_len = payload.len() & !3;
                                    if aligned_len > 0 {
                                        player.queue_pcm(&payload[..aligned_len]);
                                    }
                                }
                                continue;
                            }

                            packets_received_window += 1;
                            let datagram = &buf[..len];
                            let parsed_header = Some((
                                hdr.get_frame_id(),
                                hdr.get_packet_index(),
                                hdr.get_total_packets(),
                            ));

                            match reassembler.process_packet(datagram) {
                                Ok(Some(assembled_frame)) => {
                                    if assembled_frame.is_keyframe {
                                        waiting_for_clean_keyframe = false;
                                        last_frame_id_opt = Some(assembled_frame.frame_id);
                                        let _ = raw_frame_tx.try_send(assembled_frame);
                                    } else {
                                        let is_gap = match last_frame_id_opt {
                                            Some(last_id) => core_transport::is_newer_frame(
                                                assembled_frame.frame_id,
                                                last_id.wrapping_add(1),
                                            ),
                                            None => true,
                                        };

                                        if is_gap {
                                            waiting_for_clean_keyframe = true;
                                            let gap = last_frame_id_opt.map_or(1, |lid| {
                                                assembled_frame
                                                    .frame_id
                                                    .wrapping_sub(lid.wrapping_add(1))
                                                    .max(1)
                                            });
                                            packets_lost_window += gap * 20;

                                            if last_idr_request.elapsed()
                                                >= Duration::from_millis(50)
                                            {
                                                let req = ControlMessage::RequestKeyframe {
                                                    session_id,
                                                    reason:
                                                        core_protocol::KeyframeReason::PacketLoss,
                                                };
                                                if let Ok(req_bytes) = req.to_packet() {
                                                    let _ = ctrl_socket_task
                                                        .send_to(&req_bytes, host_ctrl_addr)
                                                        .await;
                                                }
                                                last_idr_request = Instant::now();
                                            }
                                        }

                                        if waiting_for_clean_keyframe {
                                            continue;
                                        }

                                        last_frame_id_opt = Some(assembled_frame.frame_id);
                                        let _ = raw_frame_tx.try_send(assembled_frame);
                                    }
                                }

                                Ok(None) => {
                                    if let Some((current_fid, packet_idx, total_pkts)) =
                                        parsed_header
                                    {
                                        let is_last_packet = packet_idx + 1 == total_pkts;
                                        let missing_opt = if is_last_packet {
                                            reassembler.get_missing_packets(current_fid)
                                        } else if packet_idx > 5 {
                                            reassembler.get_missing_packets_in_flight(current_fid)
                                        } else {
                                            None
                                        };

                                        if let Some(missing) = missing_opt {
                                            if !missing.is_empty()
                                                && last_nack_instant.elapsed()
                                                    >= Duration::from_millis(8)
                                            {
                                                nack_sent_window += missing.len() as u32;
                                                let nack_msg = ControlMessage::Nack {
                                                    session_id,
                                                    frame_id: current_fid,
                                                    missing_packet_indices: missing,
                                                };
                                                if let Ok(nack_bytes) = nack_msg.to_packet() {
                                                    let _ = ctrl_socket_task
                                                        .send_to(&nack_bytes, host_ctrl_addr)
                                                        .await;
                                                }
                                                last_nack_instant = Instant::now();
                                            }
                                        }
                                    }
                                }
                                Err(_) => {}
                            }
                        }
                        Err(_) => {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    }
                }
            });
        })
        .map_err(|e| format!("Не удалось запустить видеоприемник: {:?}", e))?;

    // 11. Канал команд управления
    let (control_cmd_tx, mut control_cmd_rx) = mpsc::unbounded_channel::<ControlMessage>();
    let ctrl_socket_cmds = Arc::clone(&control_socket);
    tokio::spawn(async move {
        while let Some(msg) = control_cmd_rx.recv().await {
            if let Ok(bytes) = msg.to_packet() {
                let _ = ctrl_socket_cmds.send_to(&bytes, host_ctrl_addr).await;
            }
        }
    });

    Ok(StreamingSession {
        input_tx,
        control_tx: control_cmd_tx,
        frame_rx,
        buf_pool_tx: buf_pool_tx_session,
        is_running,
        current_rtt_us,
        current_monitor_shared,
        host_addr: host_ctrl_addr,
    })
}

/// Запуск клиента Remote-Ward (открывает Launcher UI с возможностью перехода в стрим)
pub async fn run_client(
    app_config: AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _timer = core_transport::ScopedTimerPeriod::set(1);

    tracing::info!("============================================================");
    tracing::info!("              ЗАПУСК REMOTE-WARD LAUNCHER GUI               ");
    tracing::info!("============================================================");

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        Instant::now() + Duration::from_millis(1),
    ));

    let mut app = RemoteWardClientApp::new(app_config);
    event_loop.run_app(&mut app)?;

    Ok(())
}
