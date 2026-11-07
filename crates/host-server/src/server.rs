use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

use core_protocol::{
    ControlMessage, FramePacketizer, InputEvent, InputPacket, ServerHello, VideoCodec,
    PROTOCOL_VERSION,
};
use core_transport::{CongestionConfig, CongestionController, PacedSender};
use host_capture::ScreenCaptureSession;
use host_encode::NvencEncoder;
use host_input::WindowsInputInjector;

type FrameCallback = Arc<
    dyn Fn(
            *mut std::ffi::c_void,
            *mut std::ffi::c_void,
            u32,
            u32,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + Sync,
>;

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
            bitrate_kbps: 45_000, // 45 Мбит/с для кристальной четкости в динамике без "мыла"
            fps: 60,
            preferred_codec: VideoCodec::H264, // H.264 по умолчанию для гарантированной аппаратной совместимости
        }
    }
}

pub struct ServerState {
    pub client_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    pub client_control_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    pub last_client_activity: Arc<parking_lot::Mutex<Instant>>,
    pub session_id: Arc<AtomicU64>,
    pub force_keyframe: Arc<AtomicBool>,
    pub selected_codec: Arc<RwLock<VideoCodec>>,
    pub is_running: Arc<AtomicBool>,
    pub frame_counter: Arc<AtomicU64>,
    pub target_bitrate_kbps: Arc<AtomicU32>,
    pub target_fps: Arc<AtomicU32>,
    pub display_width: Arc<AtomicU32>,
    pub display_height: Arc<AtomicU32>,
    pub target_monitor_index: Arc<AtomicU32>,
    pub local_cursor_enabled: Arc<AtomicBool>,
    pub congestion: Arc<RwLock<CongestionController>>,
}

pub struct RemoteWardHost {
    config: HostConfig,
    state: Arc<ServerState>,
    input_injector: Arc<WindowsInputInjector>,
}

impl RemoteWardHost {
    pub fn new(config: HostConfig) -> Self {
        let initial_bitrate = config.bitrate_kbps;
        let cc_config = CongestionConfig {
            initial_bitrate_kbps: initial_bitrate,
            ..Default::default()
        };
        let congestion = Arc::new(RwLock::new(CongestionController::new(cc_config)));

        let state = Arc::new(ServerState {
            client_endpoint: Arc::new(RwLock::new(None)),
            client_control_endpoint: Arc::new(RwLock::new(None)),
            last_client_activity: Arc::new(parking_lot::Mutex::new(Instant::now())),
            session_id: Arc::new(AtomicU64::new(0)),
            force_keyframe: Arc::new(AtomicBool::new(true)),
            selected_codec: Arc::new(RwLock::new(config.preferred_codec)),
            is_running: Arc::new(AtomicBool::new(true)),
            frame_counter: Arc::new(AtomicU64::new(0)),
            target_bitrate_kbps: Arc::new(AtomicU32::new(initial_bitrate)),
            target_fps: Arc::new(AtomicU32::new(config.fps)),
            display_width: Arc::new(AtomicU32::new(0)),
            display_height: Arc::new(AtomicU32::new(0)),
            target_monitor_index: Arc::new(AtomicU32::new(0)),
            local_cursor_enabled: Arc::new(AtomicBool::new(false)),
            congestion,
        });

        let input_injector = Arc::new(WindowsInputInjector::new());

        Self {
            config,
            state,
            input_injector,
        }
    }

    /// Получить состояние сервера
    pub fn state(&self) -> Arc<ServerState> {
        Arc::clone(&self.state)
    }

    /// Получить инжектор ввода
    pub fn input_injector(&self) -> Arc<WindowsInputInjector> {
        Arc::clone(&self.input_injector)
    }

    /// Получить конфигурацию сервера
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Остановка сервера
    pub fn stop(&self) {
        self.state.is_running.store(false, Ordering::Relaxed);
    }

    /// Запуск управляющего и потокового сервера
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("============================================================");
        tracing::info!("       ЗАПУСК СЕРВЕРА REMOTE-WARD НА ХОСТЕ (RTX 2060)       ");
        tracing::info!("============================================================");

        // 1. Создаем сокет для управления и ввода с оптимизацией буферов и QoS DSCP
        let control_socket =
            Arc::new(UdpSocket::bind(format!("0.0.0.0:{}", self.config.control_port)).await?);
        core_transport::optimize_udp_socket(&control_socket);
        tracing::info!(
            "Управляющий сокет (Control & Input) слушает на порту {} с QoS DSCP 46",
            self.config.control_port
        );

        // Определение публичного адреса хоста через STUN для удаленного подключения извне дома (WAN)
        {
            let stun_socket = Arc::clone(&control_socket);
            tokio::spawn(async move {
                if let Some(res) = core_transport::discover_public_endpoint(&stun_socket).await {
                    tracing::info!("============================================================");
                    tracing::info!(
                        "   ПУБЛИЧНЫЙ АДРЕС ХОСТА ДЛЯ ПОДКЛЮЧЕНИЯ ИЗ ИНТЕРНЕТА (WAN):  "
                    );
                    tracing::info!(
                        "   -> {} (RTT: {} мс)                           ",
                        res.public_addr,
                        res.rtt_ms
                    );
                    tracing::info!("============================================================");
                }
            });
        }

        // Фоновая рассылка UDP-маяков (LAN Discovery) для мгновенного обнаружения лаунчером клиента
        {
            let is_running_discovery = Arc::clone(&self.state.is_running);
            let control_port = self.config.control_port;
            let video_port = self.config.video_port;
            let fps = self.config.fps;
            let display_w_atomic = Arc::clone(&self.state.display_width);
            let display_h_atomic = Arc::clone(&self.state.display_height);

            tokio::spawn(async move {
                let broadcast_sock = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
                    Ok(s) => {
                        let _ = s.set_broadcast(true);
                        s
                    }
                    Err(e) => {
                        tracing::warn!("Не удалось открыть сокет LAN Discovery: {:?}", e);
                        return;
                    }
                };

                let computer_name =
                    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "RemoteWard-Host".to_string());
                let local_ip = core_transport::get_local_ip()
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "127.0.0.1".to_string());
                let broadcast_targets = get_broadcast_targets(&local_ip);

                tracing::info!(
                    "LAN Discovery маяк активен: имя '{}', IP {}, порт {}",
                    computer_name,
                    local_ip,
                    core_protocol::DISCOVERY_PORT
                );

                while is_running_discovery.load(Ordering::Relaxed) {
                    let w = display_w_atomic.load(Ordering::Relaxed);
                    let h = display_h_atomic.load(Ordering::Relaxed);

                    let beacon = core_protocol::HostBeacon {
                        server_name: computer_name.clone(),
                        host_ip: local_ip.clone(),
                        control_port,
                        video_port,
                        display_width: if w > 0 { w } else { 2560 },
                        display_height: if h > 0 { h } else { 1440 },
                        max_fps: fps,
                        gpu_name: "NVIDIA GeForce RTX 2060 SUPER".to_string(),
                    };

                    if let Ok(bytes) = beacon.to_bytes() {
                        for target in &broadcast_targets {
                            let _ = broadcast_sock.send_to(&bytes, target).await;
                        }
                    }

                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
            });
        }

        // 2. Создаем сокет для видеопотока с буфером 4 МБ для защиты от сброса пакетов ОС и QoS
        let video_socket_raw =
            std::net::UdpSocket::bind(format!("0.0.0.0:{}", self.config.video_port))?;
        let sock_ref = socket2::SockRef::from(&video_socket_raw);
        let _ = sock_ref.set_send_buffer_size(4 * 1024 * 1024);
        let _ = sock_ref.set_recv_buffer_size(4 * 1024 * 1024);
        let _ = sock_ref.set_tos(0xB8); // DSCP 46 (Expedited Forwarding для приоритета WMM в роутерах)
        let video_socket = Arc::new(video_socket_raw);
        tracing::info!(
            "Видеосокет (UDP Stream) открыт на порту {} с буфером SO_SNDBUF 4 МБ и QoS DSCP 46",
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
                        if len >= core_protocol::DatagramHeader::SIZE {
                            if let Ok((hdr, _)) =
                                core_protocol::DatagramHeader::parse(&punch_buf[..len])
                            {
                                if hdr.flags & core_protocol::flags::HOLE_PUNCH != 0 {
                                    let is_matching_ip = {
                                        let ctrl =
                                            state_punch.client_control_endpoint.read().unwrap();
                                        ctrl.as_ref().is_none_or(|c| c.ip() == src.ip())
                                    };
                                    if is_matching_ip {
                                        let mut endpoint =
                                            state_punch.client_endpoint.write().unwrap();
                                        if endpoint.as_ref() != Some(&src) {
                                            tracing::info!(
                                                "Видеосокет хоста успешно сопряжен с клиентом: {} (получен HOLE_PUNCH session: {})!",
                                                src,
                                                hdr.get_session_id()
                                            );
                                            *endpoint = Some(src);
                                            state_punch
                                                .force_keyframe
                                                .store(true, Ordering::SeqCst);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }

        let paced_sender = Arc::new(PacedSender::new(Arc::clone(&video_socket), self.config.fps));

        // Фоновая задача обработки управляющих сообщений и ввода
        let _control_task = {
            let socket = Arc::clone(&control_socket);
            let state = Arc::clone(&self.state);
            let injector = Arc::clone(&self.input_injector);
            let paced_sender_ctrl = Arc::clone(&paced_sender);
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
                                let is_authorized = {
                                    let ctrl = state.client_control_endpoint.read().unwrap();
                                    let video = state.client_endpoint.read().unwrap();
                                    ctrl.as_ref().is_some_and(|c| c.ip() == src.ip())
                                        || video.as_ref().is_some_and(|v| v.ip() == src.ip())
                                };

                                if is_authorized {
                                    *state.last_client_activity.lock() = Instant::now();
                                    if let Ok(input_pkt) = InputPacket::from_packet(data) {
                                        if input_pkt.session_id
                                            == state.session_id.load(Ordering::Relaxed)
                                        {
                                            if let Err(e) = injector.inject(&input_pkt.event) {
                                                tracing::trace!("Ошибка инжекции ввода: {:?}", e);
                                            }
                                        }
                                    } else if let Ok(input_event) = InputEvent::from_packet(data) {
                                        if let Err(e) = injector.inject(&input_event) {
                                            tracing::trace!("Ошибка инжекции ввода: {:?}", e);
                                        }
                                    }
                                }
                                continue;
                            }

                            // 2. Проверяем управляющие сообщения (MSG_TYPE_CONTROL)
                            if let Ok(ctrl_msg) = ControlMessage::from_packet(data) {
                                *state.last_client_activity.lock() = Instant::now();
                                match ctrl_msg {
                                    ControlMessage::ClientHello(hello) => {
                                        *state.client_control_endpoint.write().unwrap() = Some(src);
                                        tracing::info!(
                                            "Клиент подключился: {} (v{}) с адреса {}",
                                            hello.client_name,
                                            hello.client_version,
                                            src
                                        );

                                        // Выбираем лучший кодек из поддерживаемых клиентом
                                        let selected =
                                            if hello.supported_codecs.contains(&VideoCodec::HEVC) {
                                                VideoCodec::HEVC
                                            } else {
                                                VideoCodec::H264
                                            };

                                        {
                                            let mut current_codec =
                                                state.selected_codec.write().unwrap();
                                            *current_codec = selected;
                                        }

                                        // Сохраняем адрес видеопотока клиента
                                        {
                                            let mut endpoint =
                                                state.client_endpoint.write().unwrap();
                                            let client_video_addr = if hello.video_port > 0 {
                                                SocketAddr::new(src.ip(), hello.video_port)
                                            } else {
                                                src
                                            };
                                            *endpoint = Some(client_video_addr);
                                        }

                                        // Форсируем первый I-кадр (IDR) для немедленного отображения
                                        state.force_keyframe.store(true, Ordering::SeqCst);

                                        // Синхронизируем инжектор ввода с реальным разрешением экрана хоста
                                        let real_w = state.display_width.load(Ordering::Relaxed);
                                        let real_h = state.display_height.load(Ordering::Relaxed);
                                        if real_w > 0 && real_h > 0 {
                                            injector.update_dimensions(real_w, real_h);
                                        }

                                        // Авто-согласование стратегии трансляции (UltraLAN, StandardWiFi, WanRemote)
                                        let is_local_lan = src.ip().is_loopback()
                                            || match src.ip() {
                                                std::net::IpAddr::V4(ipv4) => ipv4.is_private(),
                                                std::net::IpAddr::V6(_) => false,
                                            };

                                        let client_refresh_rate = hello.display_refresh_rate;
                                        let negotiated_fps =
                                            if is_local_lan && client_refresh_rate >= 120 {
                                                120
                                            } else if is_local_lan && client_refresh_rate >= 90 {
                                                90
                                            } else {
                                                config_fps
                                            };

                                        let strategy = if is_local_lan {
                                            if client_refresh_rate >= 90 {
                                                core_protocol::StreamingStrategy::UltraLan
                                            } else {
                                                core_protocol::StreamingStrategy::StandardWifi
                                            }
                                        } else {
                                            core_protocol::StreamingStrategy::WanRemote
                                        };

                                        let (fec_data, fec_parity) = match strategy {
                                            core_protocol::StreamingStrategy::UltraLan => (10, 1),
                                            core_protocol::StreamingStrategy::StandardWifi => {
                                                (10, 2)
                                            }
                                            core_protocol::StreamingStrategy::WanRemote => (10, 3),
                                        };

                                        tracing::info!(
                                            "[АВТО-СТРАТЕГИЯ] Согласована стратегия: {:?} [FPS: {}, FastSync: {}, Кодек: {:?}, FEC: {}:{}]",
                                            strategy, negotiated_fps, hello.supports_tearing, selected, fec_data, fec_parity
                                        );

                                        state.target_fps.store(negotiated_fps, Ordering::Relaxed);
                                        paced_sender_ctrl.set_target_fps(negotiated_fps);

                                        let response = ControlMessage::ServerHello(ServerHello {
                                            server_name: "remote-ward-host".into(),
                                            server_version: "0.1.0".into(),
                                            protocol_version: PROTOCOL_VERSION as u32,
                                            session_id: 1,
                                            selected_codec: selected,
                                            strategy,
                                            width: if real_w > 0 {
                                                real_w
                                            } else {
                                                hello.screen_width
                                            },
                                            height: if real_h > 0 {
                                                real_h
                                            } else {
                                                hello.screen_height
                                            },
                                            target_fps: negotiated_fps,
                                            nack_enabled: hello.supports_nack,
                                            fec_enabled: hello.supports_fec,
                                            fec_data_shards: fec_data,
                                            fec_parity_shards: fec_parity,
                                        });

                                        if let Ok(resp_bytes) = response.to_packet() {
                                            let _ = socket.send_to(&resp_bytes, src).await;
                                            tracing::info!(
                                                "Сессия установлена. Кодек: {:?}, адрес клиента: {}",
                                                selected, src
                                            );
                                        }
                                    }

                                    ControlMessage::RequestKeyframe { reason, .. } => {
                                        tracing::debug!(
                                            "Клиент запросил ключевой кадр: {}",
                                            reason
                                        );
                                        state.force_keyframe.store(true, Ordering::SeqCst);
                                    }

                                    ControlMessage::Nack {
                                        frame_id,
                                        missing_packet_indices,
                                        ..
                                    } => {
                                        if let Some(target) = *state.client_endpoint.read().unwrap()
                                        {
                                            let mut sanitized = missing_packet_indices;
                                            sanitized.sort_unstable();
                                            sanitized.dedup();
                                            if sanitized.len() > 32 {
                                                sanitized.truncate(32);
                                            }
                                            let count = paced_sender_ctrl.handle_nack_retransmit(
                                                frame_id, &sanitized, target,
                                            );
                                            tracing::trace!(
                                                "[NACK] Повторно отправлено {} пакетов для кадра #{}",
                                                count, frame_id
                                            );
                                        }
                                    }

                                    ControlMessage::Ping {
                                        sequence,
                                        send_timestamp_us,
                                        ..
                                    } => {
                                        let pong = ControlMessage::Pong {
                                            session_id: 1,
                                            sequence,
                                            send_timestamp_us,
                                        };
                                        if let Ok(pong_bytes) = pong.to_packet() {
                                            let _ = socket.send_to(&pong_bytes, src).await;
                                        }
                                    }

                                    ControlMessage::SpeedProbe {
                                        sequence,
                                        payload_size,
                                        ..
                                    } => {
                                        let ack = ControlMessage::SpeedProbeAck {
                                            session_id: 1,
                                            sequence,
                                            payload_size,
                                        };
                                        if let Ok(ack_bytes) = ack.to_packet() {
                                            let _ = socket.send_to(&ack_bytes, src).await;
                                        }
                                    }

                                    ControlMessage::ChangeStreamSettings {
                                        target_bitrate_kbps,
                                        ..
                                    } => {
                                        let clamped = target_bitrate_kbps.clamp(4_000, 60_000);
                                        let old = state
                                            .target_bitrate_kbps
                                            .swap(clamped, Ordering::SeqCst);
                                        tracing::info!(
                                            "[СЕТЬ] Клиент запросил изменение битрейта: {} -> {} кбит/с",
                                            old, clamped
                                        );
                                    }

                                    ControlMessage::CalibrationReport {
                                        min_rtt_us,
                                        avg_rtt_us,
                                        jitter_us,
                                        loss_rate,
                                        selected_bitrate_kbps,
                                        ..
                                    } => {
                                        let clamped = selected_bitrate_kbps.clamp(4_000, 60_000);
                                        state.target_bitrate_kbps.store(clamped, Ordering::SeqCst);
                                        tracing::info!(
                                            "[КАЛИБРОВКА] Отчет клиента: RTT мин/ср: {:.2}/{:.2} мс, Джиттер: {:.2} мс, Потери: {:.1}%. Установлен битрейт: {} кбит/с",
                                            min_rtt_us as f32 / 1000.0,
                                            avg_rtt_us as f32 / 1000.0,
                                            jitter_us as f32 / 1000.0,
                                            loss_rate * 100.0,
                                            clamped
                                        );
                                    }

                                    ControlMessage::ClientStats {
                                        rtt_us,
                                        packet_loss_rate,
                                        ..
                                    } => {
                                        // Полноценная динамическая ABR-адаптация через CongestionController (EWMA RTT + AIMD)
                                        if let Ok(mut cc) = state.congestion.write() {
                                            let new_bitrate =
                                                cc.update_feedback(rtt_us, packet_loss_rate);
                                            state
                                                .target_bitrate_kbps
                                                .store(new_bitrate, Ordering::SeqCst);
                                        }

                                        if packet_loss_rate > 0.08 {
                                            state.force_keyframe.store(true, Ordering::SeqCst);
                                        }
                                    }

                                    ControlMessage::SwitchMonitor { monitor_index, .. } => {
                                        let monitors = ScreenCaptureSession::enumerate_monitors()
                                            .unwrap_or_default();
                                        let count = if monitors.is_empty() {
                                            1
                                        } else {
                                            monitors.len() as u32
                                        };
                                        let actual_index = monitor_index % count;
                                        tracing::info!(
                                            "Клиент запросил переключение монитора: индекс {} (всего мониторов: {}) -> выбор: #{}",
                                            monitor_index, count, actual_index
                                        );
                                        state
                                            .target_monitor_index
                                            .store(actual_index, Ordering::SeqCst);
                                        state.force_keyframe.store(true, Ordering::SeqCst);
                                    }

                                    ControlMessage::SetLocalCursor { enabled, .. } => {
                                        tracing::info!("Клиент установил локальный аппаратный курсор (0 мс задержки): {}", enabled);
                                        state.local_cursor_enabled.store(enabled, Ordering::SeqCst);
                                    }

                                    ControlMessage::Disconnect { reason, .. } => {
                                        tracing::info!("Клиент отключился: {}", reason);
                                        *state.client_endpoint.write().unwrap() = None;
                                        *state.client_control_endpoint.write().unwrap() = None;
                                        injector.release_all();
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

        // Фоновый Watchdog активности клиента (Heartbeat / Stuck Keys protection)
        let _watchdog_task = {
            let state_wd = Arc::clone(&self.state);
            let injector_wd = Arc::clone(&self.input_injector);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(250));
                while state_wd.is_running.load(Ordering::Relaxed) {
                    interval.tick().await;
                    let has_client = {
                        let ctrl = state_wd.client_control_endpoint.read().unwrap();
                        let video = state_wd.client_endpoint.read().unwrap();
                        ctrl.is_some() || video.is_some()
                    };
                    if has_client {
                        let last_active = *state_wd.last_client_activity.lock();
                        if last_active.elapsed() > Duration::from_millis(2000) {
                            tracing::warn!("Таймаут сессии клиента (>2000 мс без активности). Сброс состояния клавиш.");
                            injector_wd.release_all();
                            *state_wd.client_endpoint.write().unwrap() = None;
                            *state_wd.client_control_endpoint.write().unwrap() = None;
                        }
                    }
                }
            })
        };

        // 3. Запуск конвейера захвата и кодирования кадров
        let _fps = self.config.fps;
        let bitrate_kbps = self.config.bitrate_kbps;

        // Канал синхронизации первого кадра
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<(u32, u32)>(1);

        // Структура исходящего кадра для развязки захвата экрана и сетевой отправки с пейсингом
        struct OutgoingVideoFrame {
            frame_id: u32,
            packets: Vec<Vec<u8>>,
            target_addr: SocketAddr,
        }

        // Выделенный асинхронный поток отправки видеопакетов (WGC больше не блокируется пейсингом!)
        let (sender_tx, mut sender_rx) = tokio::sync::mpsc::channel::<OutgoingVideoFrame>(4);
        let is_running_sender = Arc::clone(&self.state.is_running);
        let paced_sender_worker = Arc::clone(&paced_sender);

        std::thread::Builder::new()
            .name("video-sender".into())
            .spawn(move || {
                let _timer = core_transport::ScopedTimerPeriod::set(1);
                while is_running_sender.load(Ordering::Relaxed) {
                    match sender_rx.blocking_recv() {
                        Some(frame) => {
                            if let Err(e) = paced_sender_worker.send_frame_packets(
                                frame.frame_id,
                                &frame.packets,
                                frame.target_addr,
                            ) {
                                tracing::warn!("Ошибка отправки видеопакетов: {:?}", e);
                            }
                        }
                        None => break,
                    }
                }
            })
            .unwrap();

        let encoder_lock: Arc<std::sync::Mutex<Option<NvencEncoder>>> =
            Arc::new(std::sync::Mutex::new(None));
        let last_codec_lock = Arc::new(std::sync::Mutex::new(VideoCodec::H264));
        let last_bitrate_lock = Arc::new(std::sync::Mutex::new(bitrate_kbps));
        let last_fps_lock = Arc::new(std::sync::Mutex::new(self.config.fps));

        let encoder_clone = Arc::clone(&encoder_lock);
        let supervisor_encoder = Arc::clone(&encoder_lock);
        let last_codec_clone = Arc::clone(&last_codec_lock);
        let last_bitrate_clone = Arc::clone(&last_bitrate_lock);
        let last_fps_clone = Arc::clone(&last_fps_lock);
        let sender_tx_capture = sender_tx.clone();

        // 4. Запуск фонового захвата системного звука Windows (WASAPI Loopback)
        let state_audio = Arc::clone(&self.state);
        let video_socket_audio = Arc::clone(&video_socket);
        let mut audio_seq = 0u32;

        let _audio_capture =
            crate::audio_capture::WasapiAudioCapture::start(move |pcm_data, timestamp_us| {
                if let Some(target_addr) = *state_audio.client_endpoint.read().unwrap() {
                    if pcm_data.is_empty() {
                        return;
                    }
                    audio_seq = audio_seq.wrapping_add(1);

                    let mut packet_buf = [0u8; 4096];
                    let hdr_len = core_protocol::DatagramHeader::SIZE;
                    let total_len = hdr_len + pcm_data.len();
                    if total_len > packet_buf.len() {
                        return;
                    }

                    let header = core_protocol::DatagramHeader::new(
                        1,
                        core_protocol::ChannelId::AudioHostToClient,
                        0,
                        audio_seq,
                        0,
                        1,
                        timestamp_us,
                        pcm_data.len() as u16,
                    );
                    packet_buf[..hdr_len].copy_from_slice(header.as_bytes());
                    packet_buf[hdr_len..total_len].copy_from_slice(pcm_data);

                    let _ = video_socket_audio.send_to(&packet_buf[..total_len], target_addr);
                }
            });

        // 4. Пайплайн захвата экрана (WGC) и кодирования (NVENC)
        let state_capture = Arc::clone(&self.state);
        let injector_frame = Arc::clone(&self.input_injector);
        let on_frame: FrameCallback = Arc::new(move |texture_raw, device_raw, width, height| {
            let mut enc_guard = encoder_clone.lock().unwrap();
            let mut last_codec_guard = last_codec_clone.lock().unwrap();
            let mut last_bitrate_guard = last_bitrate_clone.lock().unwrap();
            let mut last_fps_guard = last_fps_clone.lock().unwrap();
            let current_target_bitrate = state_capture.target_bitrate_kbps.load(Ordering::Relaxed);
            let current_target_fps = state_capture.target_fps.load(Ordering::Relaxed);

            // Инициализация или переинициализация энкодера при смене разрешения/смене устройства D3D11/первом кадре
            let need_reinit = match enc_guard.as_ref() {
                Some(enc) => enc.dimensions() != (width, height) || enc.device_ptr() != device_raw,
                None => true,
            };

            if need_reinit {
                tracing::info!(
                    "(Пере)инициализация энкодера GPU для разрешения {}x{}...",
                    width,
                    height
                );
                injector_frame.update_dimensions(width, height);
                let codec = *state_capture.selected_codec.read().unwrap();
                *last_codec_guard = codec;
                match NvencEncoder::new_from_raw_device(
                    device_raw,
                    width,
                    height,
                    current_target_fps,
                    current_target_bitrate,
                    codec,
                ) {
                    Ok(enc) => {
                        *enc_guard = Some(enc);
                        *last_bitrate_guard = current_target_bitrate;
                        *last_fps_guard = current_target_fps;
                        state_capture.display_width.store(width, Ordering::Relaxed);
                        state_capture
                            .display_height
                            .store(height, Ordering::Relaxed);
                        let _ = init_tx.try_send((width, height));
                        tracing::info!(
                            "Аппаратный энкодер NVENC успешно привязан: {}x{} @ {} FPS (битрейт: {} кбит/с)",
                            width, height, current_target_fps, current_target_bitrate
                        );
                    }
                    Err(e) => {
                        tracing::error!("Ошибка инициализации NvencEncoder: {:?}", e);
                        return Err(Box::new(std::io::Error::other(format!(
                            "Encoder init failed: {:?}",
                            e
                        ))));
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
                tracing::info!(
                    "Смена кодека на лету: {:?} -> {:?}",
                    *last_codec_guard,
                    current_codec
                );
                if let Ok(new_enc) = NvencEncoder::new_from_raw_device(
                    device_raw,
                    width,
                    height,
                    current_target_fps,
                    current_target_bitrate,
                    current_codec,
                ) {
                    *encoder = new_enc;
                    *last_codec_guard = current_codec;
                    *last_bitrate_guard = current_target_bitrate;
                    *last_fps_guard = current_target_fps;
                }
            } else if current_target_bitrate != *last_bitrate_guard
                || current_target_fps != *last_fps_guard
            {
                // Динамическое изменение битрейта и частоты кадров (ABR и авто-согласование)
                if let Err(e) = encoder.reconfigure(current_target_bitrate, current_target_fps) {
                    tracing::warn!(
                        "Не удалось динамически перенастроить NVENC ({} кбит/с @ {} FPS): {:?}",
                        current_target_bitrate,
                        current_target_fps,
                        e
                    );
                } else {
                    *last_bitrate_guard = current_target_bitrate;
                    *last_fps_guard = current_target_fps;
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
                        1,
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

                    // Неблокирующая передача в асинхронный поток отправки — WGC завершается мгновенно (< 1.5 мс)
                    let outgoing = OutgoingVideoFrame {
                        frame_id: frame_id as u32,
                        packets,
                        target_addr,
                    };
                    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                        sender_tx_capture.try_send(outgoing)
                    {
                        tracing::debug!(
                            "Очередь видеопакетов заполнена, пропуск кадра #{}",
                            frame_id
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Ошибка кодирования кадра #{}: {:?}", frame_id, e);
                    // Сбрасываем энкодер для немедленной переинициализации на новом устройстве при сбросе GPU драйвера (TDR / Device Removed)
                    *enc_guard = None;
                }
            }

            Ok(())
        });

        // Запуск начальной сессии захвата (WGC с автоматическим отказоустойчивым переключением на DXGI)
        let initial_monitor = self.state.target_monitor_index.load(Ordering::Relaxed);
        let initial_cursor = self.state.local_cursor_enabled.load(Ordering::Relaxed);
        let on_frame_start = Arc::clone(&on_frame);

        let mut capture_session = ScreenCaptureSession::start_monitor(
            initial_monitor as usize,
            !initial_cursor,
            move |tex, dev, w, h| on_frame_start(tex, dev, w, h),
        )?;

        self.state
            .display_width
            .store(capture_session.width, Ordering::Relaxed);
        self.state
            .display_height
            .store(capture_session.height, Ordering::Relaxed);

        tracing::info!(
            "Сессия захвата экрана запущена [{:?}, монитор #{}]: разрешение дисплея {}x{}",
            capture_session.backend,
            initial_monitor,
            capture_session.width,
            capture_session.height
        );

        if let Ok((w, h)) = init_rx.try_recv() {
            tracing::info!("Пайплайн WGC -> NVENC активен ({}x{})", w, h);
        }

        // Цикл наблюдения за переключением мониторов и изменением параметров курсора
        let mut current_monitor = initial_monitor;
        let mut current_cursor = initial_cursor;
        let supervisor_state = Arc::clone(&self.state);
        let supervisor_injector = Arc::clone(&self.input_injector);
        let control_socket_ack = Arc::clone(&control_socket);

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Получен сигнал завершения (Ctrl+C). Остановка сервера...");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if !supervisor_state.is_running.load(Ordering::Relaxed) {
                        break;
                    }

                    let target_monitor = supervisor_state.target_monitor_index.load(Ordering::Relaxed);
                    let target_cursor = supervisor_state.local_cursor_enabled.load(Ordering::Relaxed);

                    if target_monitor != current_monitor || target_cursor != current_cursor {
                        tracing::info!(
                            "Динамическое переключение источника захвата: монитор #{} -> #{}, аппаратный курсор клиента: {} -> {}",
                            current_monitor, target_monitor, current_cursor, target_cursor
                        );
                        capture_session.stop();
                        // Сбрасываем старый энкодер NVENC, так как новая сессия захвата монитора
                        // может использовать другой контекст или ID3D11Device
                        *supervisor_encoder.lock().unwrap() = None;

                        // Даем 50 мс для полного завершения потока WGC и освобождения ресурсов GPU
                        tokio::time::sleep(Duration::from_millis(50)).await;

                        // Проверяем актуальный целевой монитор после паузы (на случай серии нажатий)
                        let target_monitor = supervisor_state.target_monitor_index.load(Ordering::Relaxed);
                        let target_cursor = supervisor_state.local_cursor_enabled.load(Ordering::Relaxed);

                        let on_frame_new = Arc::clone(&on_frame);
                        match ScreenCaptureSession::start_monitor(
                            target_monitor as usize,
                            !target_cursor,
                            move |tex, dev, w, h| on_frame_new(tex, dev, w, h),
                        ) {
                            Ok(new_session) => {
                                supervisor_state.display_width.store(new_session.width, Ordering::Relaxed);
                                supervisor_state.display_height.store(new_session.height, Ordering::Relaxed);
                                supervisor_injector.update_dimensions(new_session.width, new_session.height);
                                current_monitor = target_monitor;
                                current_cursor = target_cursor;
                                supervisor_state.force_keyframe.store(true, Ordering::SeqCst);
                                tracing::info!(
                                    "Захват успешно переключен на монитор #{} [{:?}]: {}x{}",
                                    target_monitor, new_session.backend, new_session.width, new_session.height
                                );

                                // Отправляем подтверждение клиенту о новом мониторе и его разрешении
                                let client_target = *supervisor_state.client_endpoint.read().unwrap();
                                if let Some(target_addr) = client_target {
                                    let ack = ControlMessage::SwitchMonitorAck {
                                        session_id: 1,
                                        monitor_index: target_monitor,
                                        width: new_session.width,
                                        height: new_session.height,
                                    };
                                    if let Ok(ack_pkt) = ack.to_packet() {
                                        let _ = control_socket_ack.send_to(&ack_pkt, target_addr).await;
                                    }
                                }

                                capture_session = new_session;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Не удалось переключить монитор на #{}: {:?}. Возврат к монитору #{}",
                                    target_monitor, e, current_monitor
                                );
                                supervisor_state.target_monitor_index.store(current_monitor, Ordering::Relaxed);
                                let on_frame_fallback = Arc::clone(&on_frame);
                                if let Ok(fallback_session) = ScreenCaptureSession::start_monitor(
                                    current_monitor as usize,
                                    !current_cursor,
                                    move |tex, dev, w, h| on_frame_fallback(tex, dev, w, h),
                                ) {
                                    capture_session = fallback_session;
                                }
                            }
                        }
                    }
                }
            }
        }

        self.state.is_running.store(false, Ordering::Relaxed);
        tracing::info!("Сервер Remote-Ward успешно остановлен.");

        Ok(())
    }
}

fn get_broadcast_targets(local_ip: &str) -> Vec<SocketAddr> {
    let mut targets = vec![
        SocketAddr::from(([255, 255, 255, 255], core_protocol::DISCOVERY_PORT)),
        SocketAddr::from(([127, 0, 0, 1], core_protocol::DISCOVERY_PORT)),
    ];
    if let Ok(ip) = local_ip.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        let subnet_bcast = std::net::Ipv4Addr::new(o[0], o[1], o[2], 255);
        targets.push(SocketAddr::from((
            subnet_bcast,
            core_protocol::DISCOVERY_PORT,
        )));
    }
    targets
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

        let session = ScreenCaptureSession::start(move |tex, dev, w, h| {
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
                        println!(
                            "Frame encoded: len={}, keyframe={}",
                            frame.data.len(),
                            frame.is_keyframe
                        );
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
