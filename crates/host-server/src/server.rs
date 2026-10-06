use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

use core_protocol::{
    ControlMessage, FramePacketizer, InputEvent, ServerHello, VideoCodec,
};
use host_capture::WgcCaptureSession;
use host_encode::NvencEncoder;
use host_input::WindowsInputInjector;

pub struct HostConfig {
    pub video_port: u16,
    pub control_port: u16,
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub preferred_codec: VideoCodec,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            video_port: 48000,
            control_port: 48001,
            bitrate_kbps: 20_000, // 20 Мбит/с для отличного качества в 1440p
            fps: 60,
            preferred_codec: VideoCodec::H264, // H.264 по умолчанию для гарантированной аппаратной совместимости
        }
    }
}

pub struct ServerState {
    pub client_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    pub force_keyframe: Arc<AtomicBool>,
    pub selected_codec: Arc<RwLock<VideoCodec>>,
    pub is_running: Arc<AtomicBool>,
    pub frame_counter: Arc<AtomicU64>,
}

pub struct RemoteWardHost {
    config: HostConfig,
    state: Arc<ServerState>,
    input_injector: Arc<WindowsInputInjector>,
}

impl RemoteWardHost {
    pub fn new(config: HostConfig) -> Self {
        let state = Arc::new(ServerState {
            client_endpoint: Arc::new(RwLock::new(None)),
            force_keyframe: Arc::new(AtomicBool::new(true)),
            selected_codec: Arc::new(RwLock::new(config.preferred_codec)),
            is_running: Arc::new(AtomicBool::new(true)),
            frame_counter: Arc::new(AtomicU64::new(0)),
        });

        let input_injector = Arc::new(WindowsInputInjector::new());

        Self {
            config,
            state,
            input_injector,
        }
    }

    /// Запуск управляющего и потокового сервера
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("============================================================");
        tracing::info!("       ЗАПУСК СЕРВЕРА REMOTE-WARD НА ХОСТЕ (RTX 2060)       ");
        tracing::info!("============================================================");

        // 1. Создаем сокет для управления и ввода
        let control_socket = Arc::new(
            UdpSocket::bind(format!("0.0.0.0:{}", self.config.control_port)).await?,
        );
        tracing::info!(
            "Управляющий сокет (Control & Input) слушает на порту {}",
            self.config.control_port
        );

        // 2. Создаем сокет для видеопотока
        let video_socket = Arc::new(
            std::net::UdpSocket::bind(format!("0.0.0.0:{}", self.config.video_port))?,
        );
        tracing::info!(
            "Видеосокет (UDP Stream) открыт на порту {}",
            self.config.video_port
        );

        // Фоновый поток сопряжения видеоканала (Hole Punch / NAT traversal)
        {
            let video_socket_punch = Arc::clone(&video_socket);
            let state_punch = Arc::clone(&self.state);
            std::thread::spawn(move || {
                let mut punch_buf = [0u8; 128];
                while state_punch.is_running.load(Ordering::Relaxed) {
                    if let Ok((len, src)) = video_socket_punch.recv_from(&mut punch_buf) {
                        if len > 0 {
                            let mut endpoint = state_punch.client_endpoint.write().unwrap();
                            if endpoint.as_ref() != Some(&src) {
                                tracing::info!(
                                    "Видеосокет хоста успешно сопряжен с клиентом: {} (получен пакет пробивки)!",
                                    src
                                );
                                *endpoint = Some(src);
                                state_punch.force_keyframe.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            });
        }

        // Фоновая задача обработки управляющих сообщений и ввода
        let control_task = {
            let socket = Arc::clone(&control_socket);
            let state = Arc::clone(&self.state);
            let injector = Arc::clone(&self.input_injector);
            let config_fps = self.config.fps;
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while state.is_running.load(Ordering::Relaxed) {
                    match socket.recv_from(&mut buf).await {
                        Ok((len, src)) => {
                            let data = &buf[..len];
                            if data.is_empty() {
                                continue;
                            }

                            // 1. Проверяем пакеты ввода (MSG_TYPE_INPUT)
                            if data[0] == core_protocol::MSG_TYPE_INPUT {
                                if let Ok(input_event) = InputEvent::from_packet(data) {
                                    if let Err(e) = injector.inject(&input_event) {
                                        tracing::trace!("Ошибка инжекции ввода: {:?}", e);
                                    }
                                }
                                continue;
                            }

                            // 2. Проверяем управляющие сообщения (MSG_TYPE_CONTROL)
                            if let Ok(ctrl_msg) = ControlMessage::from_packet(data) {
                                match ctrl_msg {
                                    ControlMessage::ClientHello(hello) => {
                                        tracing::info!(
                                            "Клиент подключился: {} (v{}) с адреса {}",
                                            hello.client_name,
                                            hello.client_version,
                                            src
                                        );

                                        // Выбираем лучший кодек из поддерживаемых клиентом
                                        let selected = if hello.supported_codecs.contains(&VideoCodec::HEVC) {
                                            VideoCodec::HEVC
                                        } else {
                                            VideoCodec::H264
                                        };

                                        {
                                            let mut current_codec = state.selected_codec.write().unwrap();
                                            *current_codec = selected;
                                        }

                                        // Сохраняем адрес видеопотока клиента
                                        {
                                            let mut endpoint = state.client_endpoint.write().unwrap();
                                            let client_video_addr = if hello.video_port > 0 {
                                                SocketAddr::new(src.ip(), hello.video_port)
                                            } else {
                                                src
                                            };
                                            *endpoint = Some(client_video_addr);
                                        }

                                        // Форсируем первый I-кадр (IDR) для немедленного отображения
                                        state.force_keyframe.store(true, Ordering::SeqCst);

                                        // Отправляем ответ ServerHello
                                        let response = ControlMessage::ServerHello(ServerHello {
                                            server_name: "remote-ward-host".into(),
                                            server_version: "0.1.0".into(),
                                            selected_codec: selected,
                                            width: hello.screen_width,
                                            height: hello.screen_height,
                                            target_fps: config_fps,
                                            session_id: "session-001".into(),
                                        });

                                        if let Ok(resp_bytes) = response.to_packet() {
                                            let _ = socket.send_to(&resp_bytes, src).await;
                                            tracing::info!(
                                                "Сессия установлена. Кодек: {:?}, адрес клиента: {}",
                                                selected, src
                                            );
                                        }
                                    }

                                    ControlMessage::RequestKeyframe { reason } => {
                                        tracing::debug!("Клиент запросил ключевой кадр: {}", reason);
                                        state.force_keyframe.store(true, Ordering::SeqCst);
                                    }

                                    ControlMessage::Disconnect { reason } => {
                                        tracing::info!("Клиент отключился: {}", reason);
                                        let mut endpoint = state.client_endpoint.write().unwrap();
                                        *endpoint = None;
                                    }

                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Ошибка чтения control сокета: {:?}", e);
                        }
                    }
                }
            })
        };

        // 3. Запуск конвейера захвата и кодирования кадров
        let state_capture = Arc::clone(&self.state);
        let video_socket_clone = Arc::clone(&video_socket);
        let fps = self.config.fps;
        let bitrate_kbps = self.config.bitrate_kbps;

        // Канал синхронизации первого кадра
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<(u32, u32)>(1);

        let encoder_lock = Arc::new(std::sync::Mutex::new(None));
        let last_codec_lock = Arc::new(std::sync::Mutex::new(VideoCodec::H264));

        let encoder_clone = Arc::clone(&encoder_lock);
        let last_codec_clone = Arc::clone(&last_codec_lock);

        // Поток захвата кадров WGC
        let capture_session = WgcCaptureSession::start(move |texture_raw, device_raw, width, height| {
            let mut enc_guard = encoder_clone.lock().unwrap();
            let mut last_codec_guard = last_codec_clone.lock().unwrap();

            // Ленивая инициализация энкодера на GPU при первом кадре
            if enc_guard.is_none() {
                tracing::info!("Первый кадр WGC: {}x{}. Инициализация энкодера GPU...", width, height);
                let codec = *state_capture.selected_codec.read().unwrap();
                *last_codec_guard = codec;
                match NvencEncoder::new_from_raw_device(device_raw, width, height, fps, bitrate_kbps, codec) {
                    Ok(enc) => {
                        *enc_guard = Some(enc);
                        let _ = init_tx.try_send((width, height));
                        tracing::info!(
                            "Аппаратный энкодер NVENC успешно привязан к текстурам WGC: {}x{} @ {} FPS",
                            width, height, fps
                        );
                    }
                    Err(e) => {
                        tracing::error!("Ошибка инициализации NvencEncoder: {:?}", e);
                        return Err(Box::new(std::io::Error::other(
                            format!("Encoder init failed: {:?}", e),
                        )));
                    }
                }
            }

            // Проверяем, есть ли подключенный клиент
            let client_addr = *state_capture.client_endpoint.read().unwrap();
            let target_addr = match client_addr {
                Some(addr) => addr,
                None => return Ok(()), // Холостой режим при отсутствии клиентов
            };

            let encoder = match enc_guard.as_mut() {
                Some(e) => e,
                None => return Ok(()),
            };

            // Проверяем смену кодека на лету
            let current_codec = *state_capture.selected_codec.read().unwrap();
            if current_codec != *last_codec_guard {
                tracing::info!("Смена кодека на лету: {:?} -> {:?}", *last_codec_guard, current_codec);
                if let Ok(new_enc) = NvencEncoder::new_from_raw_device(device_raw, width, height, fps, bitrate_kbps, current_codec) {
                    *encoder = new_enc;
                    *last_codec_guard = current_codec;
                }
            }

            let force_idr = state_capture.force_keyframe.swap(false, Ordering::SeqCst);
            let frame_id = state_capture.frame_counter.fetch_add(1, Ordering::SeqCst);

            // Метка времени в микросекундах
            let timestamp_us = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;

            // Кодирование кадра на GPU в NVENC
            match encoder.encode_frame_raw(texture_raw, force_idr) {
                Ok(encoded) => {
                    // Фрагментация на UDP-датаграммы MTU 1380
                    let packets = FramePacketizer::packetize_to_datagrams(
                        frame_id as u32,
                        timestamp_us,
                        encoded.is_keyframe,
                        &encoded.data,
                        core_protocol::DEFAULT_MAX_PAYLOAD_SIZE,
                    );

                    let is_kf = encoded.is_keyframe;
                    if frame_id == 0 || is_kf {
                        tracing::info!(
                            "Отправлен {} #{}: {} байт ({} пакетов) клиенту {}",
                            if is_kf { "KEYFRAME" } else { "кадр" },
                            frame_id,
                            encoded.data.len(),
                            packets.len(),
                            target_addr
                        );
                    }

                    // Отправка клиенту через UDP сокет
                    let socket = &video_socket_clone;
                    for packet in &packets {
                        if let Err(e) = socket.send_to(packet, target_addr) {
                            tracing::warn!("Ошибка отправки видеопакета: {:?}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Ошибка кодирования кадра #{}: {:?}", frame_id, e);
                }
            }

            Ok(())
        })?;

        tracing::info!(
            "Сессия захвата экрана запущена: разрешение дисплея {}x{}",
            capture_session.width,
            capture_session.height
        );

        if let Ok((w, h)) = init_rx.try_recv() {
            tracing::info!("Пайплайн WGC -> NVENC активен ({}x{})", w, h);
        }

        // Ожидаем завершения или сигнала прерывания
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Получен сигнал завершения (Ctrl+C). Остановка сервера...");
            }
            _ = control_task => {}
        }

        self.state.is_running.store(false, Ordering::Relaxed);
        tracing::info!("Сервер Remote-Ward успешно остановлен.");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wgc_nvenc_pipeline() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::Duration;

        let encoded_count = Arc::new(AtomicU32::new(0));
        let ec = Arc::clone(&encoded_count);

        let encoder_opt = Arc::new(std::sync::Mutex::new(None));
        let enc_clone = Arc::clone(&encoder_opt);

        let session = WgcCaptureSession::start(move |tex, dev, w, h| {
            let mut enc_guard = enc_clone.lock().unwrap();
            if enc_guard.is_none() {
                println!("Init Nvenc with dev={:?}, {}x{}", dev, w, h);
                match NvencEncoder::new_from_raw_device(dev, w, h, 60, 20_000, VideoCodec::H264) {
                    Ok(e) => {
                        println!("Nvenc created successfully!");
                        *enc_guard = Some(e);
                    }
                    Err(err) => {
                        println!("Nvenc creation failed: {:?}", err);
                        return Err(Box::new(std::io::Error::other(format!("{:?}", err))));
                    }
                }
            }

            if let Some(ref mut encoder) = *enc_guard {
                match encoder.encode_frame_raw(tex, false) {
                    Ok(frame) => {
                        println!("Frame encoded: len={}, keyframe={}", frame.data.len(), frame.is_keyframe);
                        ec.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(err) => {
                        println!("Frame encode failed: {:?}", err);
                    }
                }
            }
            Ok(())
        });

        match session {
            Ok(mut s) => {
                std::thread::sleep(Duration::from_millis(1500));
                let count = encoded_count.load(Ordering::SeqCst);
                println!("Frames successfully encoded by NVENC in 1.5s: {}", count);
                s.stop();
            }
            Err(e) => {
                println!("Capture session start failed: {:?}", e);
            }
        }
    }
}
