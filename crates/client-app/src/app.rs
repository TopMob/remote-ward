use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{PhysicalKey};
use winit::window::{Window, WindowId};

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
                ));

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
                    // Переключение захвата курсора по клавише F11
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
                            tracing::info!(
                                "Захват курсора (Game Mouse Mode): {}",
                                if self.cursor_grabbed { "ВКЛЮЧЕН" } else { "ВЫКЛЮЧЕН" }
                            );
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
                // Отрисовка кадра
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
        // Относительное движение мыши для игр (Raw Mouse Input)
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if dx != 0.0 || dy != 0.0 {
                let _ = self.input_tx.send(InputEvent::MouseMoveRelative {
                    dx: dx as i32,
                    dy: dy as i32,
                });
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Проверяем поступление новых декодированных кадров
        while let Ok(frame) = self.frame_rx.try_recv() {
            self.frames_rendered += 1;

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
