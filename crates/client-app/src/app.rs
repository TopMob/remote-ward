use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    SRCCOPY,
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
            preferred_codec: VideoCodec::HEVC,
        }
    }
}

pub struct RemoteWardClientApp {
    config: ClientConfig,
    window: Option<Window>,
    input_tx: mpsc::UnboundedSender<InputEvent>,
    frame_rx: mpsc::Receiver<DecodedFrame>,
    is_running: Arc<AtomicBool>,
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

                // Предвычисляем цветовые добавки для 2x2 блока
                let r_add = 409 * v + 128;
                let g_add = -100 * u - 208 * v + 128;
                let b_add = 516 * u + 128;

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

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width as i32;
        bmi.bmiHeader.biHeight = -(height as i32); // Отрицательная высота для формата Top-Down (сверху вниз)
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let _ = StretchDIBits(
            hdc,
            0,
            0,
            dest_w,
            dest_h,
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
    ) -> Self {
        Self {
            config,
            window: None,
            input_tx,
            frame_rx,
            is_running,
            cursor_grabbed: false,
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
                .with_inner_size(winit::dpi::LogicalSize::new(
                    self.config.width as f64 / 1.5,
                    self.config.height as f64 / 1.5,
                ))
                .with_drag_and_drop(false);

            match event_loop.create_window(window_attributes) {
                Ok(window) => {
                    tracing::info!("Окно клиента успешно создано");
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
                // В обычном оконном режиме передаем абсолютные координаты курсора на хост
                if !self.cursor_grabbed {
                    if let Some(ref window) = self.window {
                        let win_size = window.inner_size();
                        if win_size.width > 0 && win_size.height > 0 {
                            let host_x = (position.x * self.config.width as f64 / win_size.width as f64) as u32;
                            let host_y = (position.y * self.config.height as f64 / win_size.height as f64) as u32;
                            let _ = self.input_tx.send(InputEvent::MouseMoveAbsolute {
                                x: host_x.min(self.config.width.saturating_sub(1)),
                                y: host_y.min(self.config.height.saturating_sub(1)),
                            });
                        }
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let mut got_new_frame = false;
        while let Ok(frame) = self.frame_rx.try_recv() {
            self.frames_rendered += 1;

            let w = frame.width as usize;
            let h = frame.height as usize;
            let required_bgra_size = w * h * 4;
            if self.bgra_buffer.len() != required_bgra_size {
                self.bgra_buffer.resize(required_bgra_size, 0);
            }
            self.last_frame_size = (frame.width, frame.height);

            nv12_to_bgra(&frame.data, w, h, &mut self.bgra_buffer);
            got_new_frame = true;

            if self.last_stats_instant.elapsed() >= Duration::from_secs(1) {
                let fps = self.frames_rendered;
                self.frames_rendered = 0;
                self.last_stats_instant = Instant::now();
                tracing::info!(
                    "Клиент отображает видеопоток: {} FPS | Задержка декодирования кадра: {:.2} мс",
                    fps,
                    frame.latency_us as f64 / 1000.0
                );
            }
        }

        if got_new_frame {
            self.draw_current_frame();
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

    // 1. Создаем сокет для отправки управления и ввода
    let control_socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
    let host_ctrl_addr = config.host_control_addr;

    // 2. Создаем сокет для приема видео
    let video_socket = Arc::new(
        UdpSocket::bind(format!("0.0.0.0:{}", config.video_port)).await?,
    );
    tracing::info!("Сетевой видеосокет клиента открыт на порту {}", config.video_port);

    // Канал для передачи событий ввода из GUI потока в сетевой сокет
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<InputEvent>();
    // Канал для передачи декодированных кадров из сетевого потока в GUI поток
    let (frame_tx, frame_rx) = mpsc::channel::<DecodedFrame>(16);

    // 3. Отправляем ClientHello на хост
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

    // Фоновая задача отправки пользовательского ввода с минимальной задержкой
    let is_running_input = Arc::clone(&is_running);
    let ctrl_socket_input = Arc::clone(&control_socket);
    tokio::spawn(async move {
        while is_running_input.load(Ordering::Relaxed) {
            if let Some(event) = input_rx.recv().await {
                if let Ok(bytes) = event.to_packet() {
                    let _ = ctrl_socket_input.send_to(&bytes, host_ctrl_addr).await;
                }
            }
        }
    });

    // Фоновая задача приема видеопакетов и декодирования
    let is_running_video = Arc::clone(&is_running);
    let video_socket_task = Arc::clone(&video_socket);
    let ctrl_socket_task = Arc::clone(&control_socket);
    let width = config.width;
    let height = config.height;
    let preferred_codec = config.preferred_codec;

    std::thread::spawn(move || {
        let mut reassembler = FrameReassembler::new(Duration::from_millis(50));
        let effective_codec = if MftVideoDecoder::is_codec_supported(preferred_codec) {
            preferred_codec
        } else {
            VideoCodec::H264
        };
        let mut decoder_opt = MftVideoDecoder::new(width, height, effective_codec).ok();
        let mut buf = [0u8; 2048];

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            while is_running_video.load(Ordering::Relaxed) {
                match video_socket_task.recv_from(&mut buf).await {
                    Ok((len, _src)) => {
                        let datagram = &buf[..len];
                        if let Ok(Some(assembled_frame)) = reassembler.process_packet(datagram) {
                            if let Some(ref mut decoder) = decoder_opt {
                                match decoder.decode(&assembled_frame.data) {
                                    Ok(Some(decoded)) => {
                                        let _ = frame_tx.try_send(decoded);
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        tracing::warn!("Ошибка декодирования кадра #{}: {:?}", assembled_frame.frame_id, e);
                                        // Запрашиваем ключевой кадр при ошибке
                                        let req = ControlMessage::RequestKeyframe {
                                            reason: "decode error".into(),
                                        };
                                        if let Ok(req_bytes) = req.to_packet() {
                                            let _ = ctrl_socket_task.send_to(&req_bytes, host_ctrl_addr).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Ошибка чтения видеосокета: {:?}", e);
                    }
                }
            }
        });
    });

    // 4. Запуск оконного цикла событий Winit
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = RemoteWardClientApp::new(config, input_tx, frame_rx, Arc::clone(&is_running));
    event_loop.run_app(&mut app)?;

    is_running.store(false, Ordering::Relaxed);
    Ok(())
}
