use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Fullscreen, Window, WindowId};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetDC, ReleaseDC, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    SRCCOPY, SetStretchBltMode, HALFTONE, COLORONCOLOR, SetBrushOrgEx,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use core_protocol::{
    ButtonState, ClientHello, ControlMessage, InputEvent, MouseButton, VideoCodec,
};
use core_transport::FrameReassembler;
use crate::decoder::{DecodedFrame, MftVideoDecoder};
use crate::input_mapper::keycode_to_scancode;

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
            host_control_addr: "127.0.0.1:48001".parse().unwrap(),
            video_port: 48000,
            width: 2560,
            height: 1440,
            preferred_codec: VideoCodec::H264, // H.264 по умолчанию для 100% аппаратной совместимости
        }
    }
}

pub struct RemoteWardClientApp {
    config: ClientConfig,
    window: Option<Window>,
    input_tx: mpsc::UnboundedSender<InputEvent>,
    frame_rx: mpsc::Receiver<DecodedFrame>,
    is_running: Arc<AtomicBool>,
    current_rtt_us: Arc<AtomicU32>,
    cursor_grabbed: bool,
    frames_rendered: u64,
    last_stats_instant: Instant,
    bgra_buffer: Vec<u8>,
    last_frame_size: (u32, u32),
}

#[inline(always)]
fn clamp_u8(val: i32) -> u8 {
    val.clamp(0, 255) as u8
}

/// Высокоскоростное параллельное преобразование NV12 (YUV 4:2:0) в 32-bit BGRA
pub fn nv12_to_bgra(nv12: &[u8], width: usize, height: usize, bgra: &mut [u8]) {
    use rayon::prelude::*;

    let frame_y_size = width * height;
    if nv12.len() < frame_y_size + frame_y_size / 2 {
        return;
    }

    let (y_plane, uv_plane) = nv12.split_at(frame_y_size);

    // Обрабатываем строки параллельно парами (так как одна UV-строка приходится на две Y-строки)
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
            let y_row0_slice = &y_plane[y_start0..y_start0 + width];
            let y_row1_slice = &y_plane[y_start1..y_start1 + width];

            let (row0_bgra, row1_bgra) = two_rows_bgra.split_at_mut(width * 4);

            for x in (0..width).step_by(2) {
                let u = uv_row[x] as i32 - 128;
                let v = uv_row[x + 1] as i32 - 128;

                // Точная матрица BT.709 Limited Range (согласовано с VUI энкодера NVENC):
                // R = 1.16438 * (Y - 16) + 1.79274 * V
                // G = 1.16438 * (Y - 16) - 0.21325 * U - 0.53291 * V
                // B = 1.16438 * (Y - 16) + 2.11240 * U
                let r_add = 459 * v + 128;
                let g_add = -55 * u - 136 * v + 128;
                let b_add = 541 * u + 128;

                // Точка (x, y0)
                let c00 = 298 * ((y_row0_slice[x] as i32) - 16);
                let idx00 = x * 4;
                row0_bgra[idx00] = clamp_u8((c00 + b_add) >> 8);
                row0_bgra[idx00 + 1] = clamp_u8((c00 + g_add) >> 8);
                row0_bgra[idx00 + 2] = clamp_u8((c00 + r_add) >> 8);
                row0_bgra[idx00 + 3] = 255;

                // Точка (x + 1, y0)
                let c01 = 298 * ((y_row0_slice[x + 1] as i32) - 16);
                let idx01 = (x + 1) * 4;
                row0_bgra[idx01] = clamp_u8((c01 + b_add) >> 8);
                row0_bgra[idx01 + 1] = clamp_u8((c01 + g_add) >> 8);
                row0_bgra[idx01 + 2] = clamp_u8((c01 + r_add) >> 8);
                row0_bgra[idx01 + 3] = 255;

                // Точка (x, y1)
                let c10 = 298 * ((y_row1_slice[x] as i32) - 16);
                let idx10 = x * 4;
                row1_bgra[idx10] = clamp_u8((c10 + b_add) >> 8);
                row1_bgra[idx10 + 1] = clamp_u8((c10 + g_add) >> 8);
                row1_bgra[idx10 + 2] = clamp_u8((c10 + r_add) >> 8);
                row1_bgra[idx10 + 3] = 255;

                // Точка (x + 1, y1)
                let c11 = 298 * ((y_row1_slice[x + 1] as i32) - 16);
                let idx11 = (x + 1) * 4;
                row1_bgra[idx11] = clamp_u8((c11 + b_add) >> 8);
                row1_bgra[idx11 + 1] = clamp_u8((c11 + g_add) >> 8);
                row1_bgra[idx11 + 2] = clamp_u8((c11 + r_add) >> 8);
                row1_bgra[idx11 + 3] = 255;
            }
        });
}

fn render_bgra_to_hwnd(
    hwnd: HWND,
    bgra: &[u8],
    width: u32,
    height: u32,
) {
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_err() {
            return;
        }
        let dest_w = rect.right - rect.left;
        let dest_h = rect.bottom - rect.top;
        if dest_w <= 0 || dest_h <= 0 {
            return;
        }

        let hdc = GetDC(hwnd);
        if hdc.is_invalid() {
            return;
        }

        // Сохранение правильных пропорций кадра (Letterbox / Pillarbox)
        let scale_x = dest_w as f64 / width as f64;
        let scale_y = dest_h as f64 / height as f64;
        let scale = scale_x.min(scale_y);
        let target_w = (width as f64 * scale).round() as i32;
        let target_h = (height as f64 * scale).round() as i32;
        let offset_x = (dest_w - target_w) / 2;
        let offset_y = (dest_h - target_h) / 2;

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width as i32;
        bmi.bmiHeader.biHeight = -(height as i32); // Отрицательная высота для формата Top-Down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        // Для 1:1 попиксельного отображения используем COLORONCOLOR (без растра и шума),
        // а при масштабировании — качественную интерполяцию HALFTONE
        let blt_mode = if target_w == width as i32 && target_h == height as i32 {
            COLORONCOLOR
        } else {
            let _ = SetBrushOrgEx(hdc, 0, 0, None);
            HALFTONE
        };
        let _ = SetStretchBltMode(hdc, blt_mode);

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
    pub fn new(
        config: ClientConfig,
        input_tx: mpsc::UnboundedSender<InputEvent>,
        frame_rx: mpsc::Receiver<DecodedFrame>,
        is_running: Arc<AtomicBool>,
        current_rtt_us: Arc<AtomicU32>,
    ) -> Self {
        Self {
            config,
            window: None,
            input_tx,
            frame_rx,
            is_running,
            current_rtt_us,
            cursor_grabbed: true,
            frames_rendered: 0,
            last_stats_instant: Instant::now(),
            bgra_buffer: Vec::new(),
            last_frame_size: (0, 0),
        }
    }

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
                let fw = if self.last_frame_size.0 > 0 { self.last_frame_size.0 as f64 } else { self.config.width as f64 };
                let fh = if self.last_frame_size.1 > 0 { self.last_frame_size.1 as f64 } else { self.config.height as f64 };
                if dest_w > 0.0 && dest_h > 0.0 && fw > 0.0 && fh > 0.0 {
                    let scale = (dest_w / fw).min(dest_h / fh);
                    let target_w = (fw * scale).round();
                    let target_h = (fh * scale).round();
                    let offset_x = ((dest_w - target_w) / 2.0).round();
                    let offset_y = ((dest_h - target_h) / 2.0).round();
                    return (offset_x, offset_y, target_w, target_h);
                }
            }
        }
        (0.0, 0.0, self.config.width as f64, self.config.height as f64)
    }

    fn draw_current_frame(&self) {
        if let Some(hwnd) = self.get_hwnd() {
            if !self.bgra_buffer.is_empty() && self.last_frame_size.0 > 0 && self.last_frame_size.1 > 0 {
                render_bgra_to_hwnd(
                    hwnd,
                    &self.bgra_buffer,
                    self.last_frame_size.0,
                    self.last_frame_size.1,
                );
            }
        }
    }
}

impl ApplicationHandler for RemoteWardClientApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let window_attributes = Window::default_attributes()
                .with_title("Remote-Ward Client (Ultra-Low Latency Streaming)")
                .with_fullscreen(Some(Fullscreen::Borderless(None)))
                .with_drag_and_drop(false);

            match event_loop.create_window(window_attributes) {
                Ok(window) => {
                    tracing::info!("Окно клиента успешно создано в полноэкранном режиме (Borderless)");
                    window.set_cursor_visible(false);
                    let _ = window.set_cursor_grab(winit::window::CursorGrabMode::Confined);
                    self.window = Some(window);
                }
                Err(e) => {
                    tracing::error!("Ошибка создания окна клиента: {:?}", e);
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
        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("Закрытие окна клиентом...");
                self.is_running.store(false, Ordering::Relaxed);
                event_loop.exit();
            }

            WindowEvent::Focused(is_focused) => {
                if !is_focused {
                    // При потере фокуса сбрасываем зажатые кнопки мыши
                    let _ = self.input_tx.send(InputEvent::MouseButton {
                        button: MouseButton::Left,
                        state: ButtonState::Released,
                    });
                    let _ = self.input_tx.send(InputEvent::MouseButton {
                        button: MouseButton::Right,
                        state: ButtonState::Released,
                    });
                    let _ = self.input_tx.send(InputEvent::MouseButton {
                        button: MouseButton::Middle,
                        state: ButtonState::Released,
                    });
                }
            }

            WindowEvent::KeyboardInput { event: key_event, .. } => {
                if let PhysicalKey::Code(key_code) = key_event.physical_key {
                    // Переключение полноэкранного режима и захвата курсора по клавише F11
                    if key_code == winit::keyboard::KeyCode::F11 && key_event.state == ElementState::Pressed {
                        self.cursor_grabbed = !self.cursor_grabbed;
                        if let Some(ref window) = self.window {
                            window.set_cursor_visible(!self.cursor_grabbed);
                            let grab_mode = if self.cursor_grabbed {
                                winit::window::CursorGrabMode::Confined
                            } else {
                                winit::window::CursorGrabMode::None
                            };
                            let _ = window.set_cursor_grab(grab_mode);

                            if self.cursor_grabbed {
                                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                                tracing::info!("F11: Полноэкранный режим (Borderless) + Захват курсора ВКЛЮЧЕН");
                            } else {
                                window.set_fullscreen(None);
                                tracing::info!("F11: Оконный режим + Захват курсора ВЫКЛЮЧЕН");
                            }
                        }
                        return;
                    }

                    if let Some((scan_code, is_extended)) = keycode_to_scancode(key_code) {
                        let state = match key_event.state {
                            ElementState::Pressed => ButtonState::Pressed,
                            ElementState::Released => ButtonState::Released,
                        };

                        let input_event = InputEvent::Keyboard {
                            scan_code,
                            is_extended,
                            state,
                        };
                        let _ = self.input_tx.send(input_event);
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                // В обычном оконном режиме передаем точные координаты курсора с учетом Letterbox
                if !self.cursor_grabbed {
                    let (ox, oy, tw, th) = self.get_letterbox();
                    let fw = if self.last_frame_size.0 > 0 { self.last_frame_size.0 as f64 } else { self.config.width as f64 };
                    let fh = if self.last_frame_size.1 > 0 { self.last_frame_size.1 as f64 } else { self.config.height as f64 };
                    if tw > 0.0 && th > 0.0 {
                        let norm_x = ((position.x - ox) / tw).clamp(0.0, 1.0);
                        let norm_y = ((position.y - oy) / th).clamp(0.0, 1.0);
                        let host_x = (norm_x * (fw - 1.0)).round() as u32;
                        let host_y = (norm_y * (fh - 1.0)).round() as u32;
                        let _ = self.input_tx.send(InputEvent::MouseMoveAbsolute {
                            x: host_x,
                            y: host_y,
                        });
                    }
                }
            }

            WindowEvent::MouseInput { state: btn_state, button, .. } => {
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

                let _ = self.input_tx.send(InputEvent::MouseButton {
                    button: mb,
                    state,
                });
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let (delta_x, delta_y) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        ((x * 120.0) as i16, (y * 120.0) as i16)
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        (pos.x as i16, pos.y as i16)
                    }
                };

                let _ = self.input_tx.send(InputEvent::MouseWheel {
                    delta_y,
                    delta_x,
                });
            }

            WindowEvent::RedrawRequested => {
                self.draw_current_frame();
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
        // Относительное движение мыши для игр (Raw Mouse Input) передаем только при захваченном курсоре
        if self.cursor_grabbed {
            if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
                if dx != 0.0 || dy != 0.0 {
                    let _ = self.input_tx.send(InputEvent::MouseMoveRelative {
                        dx: dx as i32,
                        dy: dy as i32,
                    });
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Минимальное время ожидания: опрашиваем цикл событий каждые 1 мс, чтобы кадры выводились мгновенно
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(1)));

        let mut latest_frame = None;
        let mut frames_received_this_tick = 0u64;

        // Извлекаем только самый свежий кадр, пропуская устаревшие кадры из очереди!
        while let Ok(frame) = self.frame_rx.try_recv() {
            frames_received_this_tick += 1;
            latest_frame = Some(frame);
        }

        if let Some(frame) = latest_frame {
            self.frames_rendered += frames_received_this_tick;

            let w = frame.width as usize;
            let h = frame.height as usize;
            let required_bgra_size = w * h * 4;
            if self.bgra_buffer.len() != required_bgra_size {
                self.bgra_buffer.resize(required_bgra_size, 0);
            }
            self.last_frame_size = (frame.width, frame.height);

            // Конвертируем только 1 самый свежий кадр
            nv12_to_bgra(&frame.data, w, h, &mut self.bgra_buffer);

            if self.frames_rendered == 1 {
                tracing::info!("Первый видеокадр успешно выведен на экран клиента!");
            }

            if self.last_stats_instant.elapsed() >= Duration::from_secs(1) {
                let fps = self.frames_rendered;
                self.frames_rendered = 0;
                self.last_stats_instant = Instant::now();
                let decode_ms = frame.latency_us as f64 / 1000.0;
                let rtt_ms = self.current_rtt_us.load(Ordering::Relaxed) as f32 / 1000.0;
                tracing::info!(
                    "Клиент отображает видеопоток: {} FPS | RTT: {:.1} мс | Задержка декодирования кадра: {:.2} мс",
                    fps,
                    rtt_ms,
                    decode_ms
                );
                if let Some(ref window) = self.window {
                    window.set_title(&format!(
                        "Remote-Ward Client | {}x{} @ {} FPS | RTT: {:.1} мс | Декод: {:.1} мс",
                        w, h, fps, rtt_ms, decode_ms
                    ));
                }
            }

            if let Some(ref window) = self.window {
                window.request_redraw();
            }
        }
    }
}

/// Запуск клиента Remote-Ward с сетевым потоком и циклом оконного приложения
pub async fn run_client(
    config: ClientConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("============================================================");
    tracing::info!("           ЗАПУСК КЛИЕНТА REMOTE-WARD (STREAMING)           ");
    tracing::info!("============================================================");

    let is_running = Arc::new(AtomicBool::new(true));
    let current_rtt_us = Arc::new(AtomicU32::new(0));

    // 1. Создаем сокет для отправки управления и ввода
    let control_socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
    let host_ctrl_addr = config.host_control_addr;

    // 2. Создаем сокет для приема видео с расширенным системным буфером SO_RCVBUF (8 МБ)
    let video_socket_std = std::net::UdpSocket::bind(format!("0.0.0.0:{}", config.video_port))?;
    let sock_ref = socket2::SockRef::from(&video_socket_std);
    let _ = sock_ref.set_recv_buffer_size(8 * 1024 * 1024);
    video_socket_std.set_nonblocking(true)?;
    let video_socket = Arc::new(UdpSocket::from_std(video_socket_std)?);
    tracing::info!(
        "Сетевой видеосокет клиента открыт на порту {} с буфером SO_RCVBUF 8 МБ",
        config.video_port
    );

    // Фоновая задача пробивки порта в Windows Firewall / NAT и отправки Heartbeat на видеопорт хоста
    let host_video_addr = SocketAddr::new(config.host_control_addr.ip(), config.video_port);
    let is_running_punch = Arc::clone(&is_running);
    let video_socket_punch = Arc::clone(&video_socket);
    tokio::spawn(async move {
        let punch_packet = [core_protocol::MSG_TYPE_CONTROL, 0x50, 0x55, 0x4E, 0x43, 0x48]; // PUNCH
        while is_running_punch.load(Ordering::Relaxed) {
            let _ = video_socket_punch.send_to(&punch_packet, host_video_addr).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    // Канал для передачи событий ввода из GUI потока в сетевой сокет
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<InputEvent>();
    // Канал для передачи декодированных кадров из потока декодера в GUI поток (емкость 2 для минимальной задержки)
    let (frame_tx, frame_rx) = mpsc::channel::<DecodedFrame>(2);
    // Канал для передачи собранных кадров из сетевого потока в поток декодера (емкость 3 кадра для устранения накопления очереди)
    let (raw_frame_tx, mut raw_frame_rx) = mpsc::channel::<core_transport::ReassembledFrame>(3);

    // 3. Быстрая калибровка сети перед началом трансляции (RTT, джиттер, потери, пропускная способность)
    let calibration = crate::calibration::run_network_calibration(&control_socket, host_ctrl_addr).await;

    // Отправляем хосту отчет о результатах калибровки сети
    let report_msg = ControlMessage::CalibrationReport {
        min_rtt_us: calibration.min_rtt_us,
        avg_rtt_us: calibration.avg_rtt_us,
        jitter_us: calibration.jitter_us,
        loss_rate: calibration.packet_loss_rate,
        selected_bitrate_kbps: calibration.target_bitrate_kbps,
    };
    if let Ok(report_bytes) = report_msg.to_packet() {
        let _ = control_socket.send_to(&report_bytes, host_ctrl_addr).await;
    }

    // Задаем вычисленный битрейт
    let settings_msg = ControlMessage::ChangeStreamSettings {
        target_bitrate_kbps: calibration.target_bitrate_kbps,
        target_fps: 60,
    };
    if let Ok(settings_bytes) = settings_msg.to_packet() {
        let _ = control_socket.send_to(&settings_bytes, host_ctrl_addr).await;
    }

    // 4. Отправляем ClientHello на хост
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "remote-ward-client".into(),
        client_version: "0.1.0".into(),
        supported_codecs: MftVideoDecoder::supported_codecs(),
        screen_width: config.width,
        screen_height: config.height,
        target_fps: 60,
        dpi_scale: 1.0,
        video_port: config.video_port,
    });

    let hello_bytes = hello.to_packet()?;
    control_socket.send_to(&hello_bytes, host_ctrl_addr).await?;
    tracing::info!("Отправлен ClientHello на хост {}", host_ctrl_addr);

    // Фоновая задача приема управляющих сообщений (ServerHello, Pong)
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
                            ControlMessage::ServerHello(hello) => {
                                tracing::info!(
                                    "Хост подтвердил подключение: {} (v{}) [Кодек: {:?}, {}x{} @ {} FPS]",
                                    hello.server_name, hello.server_version, hello.selected_codec,
                                    hello.width, hello.height, hello.target_fps
                                );
                            }
                            ControlMessage::Pong { sequence: _, send_timestamp_us } => {
                                let now_us = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_micros() as u64;
                                let rtt_us = now_us.saturating_sub(send_timestamp_us) as u32;
                                current_rtt_rx.store(rtt_us, Ordering::Relaxed);
                                tracing::trace!("Активный RTT: {:.2} мс", rtt_us as f32 / 1000.0);
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

    // Фоновая задача периодического Ping (каждые 1.5 секунды для контроля канала)
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
                sequence: seq,
                send_timestamp_us: now_us,
            };
            if let Ok(bytes) = ping.to_packet() {
                let _ = ctrl_socket_ping.send_to(&bytes, host_ctrl_addr).await;
            }
        }
    });

    // Фоновая задача отправки пользовательского ввода с минимальной задержкой (без 100% CPU спина)
    let is_running_input = Arc::clone(&is_running);
    let ctrl_socket_input = Arc::clone(&control_socket);
    tokio::spawn(async move {
        while let Some(event) = input_rx.recv().await {
            if !is_running_input.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(bytes) = event.to_packet() {
                let _ = ctrl_socket_input.send_to(&bytes, host_ctrl_addr).await;
            }
        }
    });

    // 5. Выделенный поток аппаратного декодирования MFT (полностью развязан от сетевого сокета)
    let is_running_decoder = Arc::clone(&is_running);
    let width = config.width;
    let height = config.height;
    let preferred_codec = config.preferred_codec;
    let frame_tx_decoder = frame_tx.clone();

    std::thread::Builder::new()
        .name("video-decoder".into())
        .spawn(move || {
            let effective_codec = if MftVideoDecoder::is_codec_supported(preferred_codec) {
                preferred_codec
            } else {
                VideoCodec::H264
            };
            let mut decoder_opt = MftVideoDecoder::new(width, height, effective_codec).ok();
            let mut frames_count = 0u64;

            while is_running_decoder.load(Ordering::Relaxed) {
                match raw_frame_rx.blocking_recv() {
                    Some(mut assembled_frame) => {
                        // Оптимизация задержки: если в очереди скопились кадры,
                        // мгновенно подтягиваем контекст или переходим к свежему Keyframe, не создавая задержки
                        while let Ok(next_frame) = raw_frame_rx.try_recv() {
                            if next_frame.is_keyframe {
                                assembled_frame = next_frame;
                            } else {
                                if let Some(ref mut decoder) = decoder_opt {
                                    let _ = decoder.decode(&assembled_frame.data);
                                }
                                assembled_frame = next_frame;
                            }
                        }

                        frames_count += 1;
                        if frames_count == 1 {
                            tracing::info!(
                                "Первый видеокадр #{} отправлен в аппаратный декодер ({} байт, keyframe={})!",
                                assembled_frame.frame_id,
                                assembled_frame.data.len(),
                                assembled_frame.is_keyframe
                            );
                        }

                        if let Some(ref mut decoder) = decoder_opt {
                            match decoder.decode(&assembled_frame.data) {
                                Ok(Some(decoded)) => {
                                    if frames_count == 1 {
                                        tracing::info!(
                                            "Первый видеокадр успешно декодирован в NV12 ({}x{})!",
                                            decoded.width,
                                            decoded.height
                                        );
                                    }
                                    let _ = frame_tx_decoder.try_send(decoded);
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    tracing::warn!(
                                        "Ошибка декодирования кадра #{}: {:?}",
                                        assembled_frame.frame_id,
                                        e
                                    );
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
        })
        .unwrap();

    // 6. Выделенный поток сетевого приема и сборки видеопакетов
    let is_running_video = Arc::clone(&is_running);
    let video_socket_task = Arc::clone(&video_socket);
    let ctrl_socket_task = Arc::clone(&control_socket);
    let current_rtt_stats = Arc::clone(&current_rtt_us);

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
                let mut packets_count = 0u64;
                let mut last_frame_id_opt: Option<u32> = None;
                let mut frames_in_window = 0u32;
                let mut gaps_in_window = 0u32;
                let mut last_stats_sent = Instant::now();
                let mut awaiting_clean_keyframe = false;
                let mut last_idr_request = Instant::now() - Duration::from_secs(1);

                while is_running_video.load(Ordering::Relaxed) {
                    // Периодическая отправка статистики потерь пакетов хосту (каждые 1 сек)
                    if last_stats_sent.elapsed() >= Duration::from_millis(1000) {
                        let total_frames = frames_in_window + gaps_in_window;
                        let loss_rate = if total_frames >= 10 {
                            gaps_in_window as f32 / total_frames as f32
                        } else {
                            0.0
                        };
                        let stats = ControlMessage::ClientStats {
                            rtt_us: current_rtt_stats.load(Ordering::Relaxed),
                            jitter_us: 0,
                            decode_latency_us: 0,
                            render_latency_us: 0,
                            packet_loss_rate: loss_rate,
                        };
                        if let Ok(bytes) = stats.to_packet() {
                            let _ = ctrl_socket_task.send_to(&bytes, host_ctrl_addr).await;
                        }
                        frames_in_window = 0;
                        gaps_in_window = 0;
                        last_stats_sent = Instant::now();
                    }

                    match video_socket_task.recv_from(&mut buf).await {
                        Ok((len, src)) => {
                            if len < 4 {
                                continue;
                            }
                            if buf[0] == core_protocol::MSG_TYPE_CONTROL {
                                continue;
                            }

                            packets_count += 1;
                            if packets_count == 1 {
                                tracing::info!(
                                    "Первый видеопакет успешно получен от {} ({} байт)!",
                                    src,
                                    len
                                );
                            }

                            let datagram = &buf[..len];
                            if let Ok(Some(assembled_frame)) = reassembler.process_packet(datagram) {
                                let is_gap = match last_frame_id_opt {
                                    Some(last_id) => assembled_frame.frame_id > last_id.wrapping_add(1),
                                    None => false,
                                };

                                if is_gap && !assembled_frame.is_keyframe {
                                    let gap = last_frame_id_opt.map_or(1, |lid| assembled_frame.frame_id.saturating_sub(lid + 1));
                                    gaps_in_window += gap;
                                    awaiting_clean_keyframe = true;

                                    // Лимит частоты запроса IDR (не чаще 1 раза в 300 мс)
                                    if last_idr_request.elapsed() >= Duration::from_millis(300) {
                                        let req = ControlMessage::RequestKeyframe {
                                            reason: "frame gap detected".into(),
                                        };
                                        if let Ok(req_bytes) = req.to_packet() {
                                            let _ = ctrl_socket_task.send_to(&req_bytes, host_ctrl_addr).await;
                                        }
                                        last_idr_request = Instant::now();
                                    }
                                }

                                if assembled_frame.is_keyframe {
                                    // Чистый ключевой кадр получен — возобновляем отображение
                                    awaiting_clean_keyframe = false;
                                }

                                // Если опорный кадр утерян, отбрасываем битые P-кадры до прихода IDR
                                // (устраняет эффект шлейфов и призрачных курсоров!)
                                if awaiting_clean_keyframe {
                                    if last_idr_request.elapsed() >= Duration::from_millis(300) {
                                        let req = ControlMessage::RequestKeyframe {
                                            reason: "awaiting clean keyframe".into(),
                                        };
                                        if let Ok(req_bytes) = req.to_packet() {
                                            let _ = ctrl_socket_task.send_to(&req_bytes, host_ctrl_addr).await;
                                        }
                                        last_idr_request = Instant::now();
                                    }
                                    continue;
                                }

                                frames_in_window += 1;
                                let frame_id = assembled_frame.frame_id;

                                // Передаем кадр в поток декодера (при переполнении запрашиваем чистый IDR, не допуская артефактов)
                                match raw_frame_tx.try_send(assembled_frame) {
                                    Ok(()) => {
                                        last_frame_id_opt = Some(frame_id);
                                    }
                                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                        tracing::warn!("Очередь декодера переполнена! Пропущен P-кадр #{}, запрашиваем IDR", frame_id);
                                        awaiting_clean_keyframe = true;
                                        if last_idr_request.elapsed() >= Duration::from_millis(200) {
                                            let req = ControlMessage::RequestKeyframe {
                                                reason: "decoder queue overflow".into(),
                                            };
                                            if let Ok(req_bytes) = req.to_packet() {
                                                let _ = ctrl_socket_task.send_to(&req_bytes, host_ctrl_addr).await;
                                            }
                                            last_idr_request = Instant::now();
                                        }
                                    }
                                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Ошибка чтения видеосокета: {:?}", e);
                        }
                    }
                }
            });
        })
        .unwrap();

    // 7. Запуск оконного цикла событий Winit
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(1)));

    let mut app = RemoteWardClientApp::new(
        config,
        input_tx,
        frame_rx,
        Arc::clone(&is_running),
        Arc::clone(&current_rtt_us),
    );
    event_loop.run_app(&mut app)?;

    is_running.store(false, Ordering::Relaxed);
    Ok(())
}
