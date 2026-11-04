use crate::frame::VideoCodec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyframeReason {
    Startup,
    PacketLoss,
    DecoderReset,
    StreamChange,
    UserRequest,
}

impl std::fmt::Display for KeyframeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup => write!(f, "Startup"),
            Self::PacketLoss => write!(f, "PacketLoss"),
            Self::DecoderReset => write!(f, "DecoderReset"),
            Self::StreamChange => write!(f, "StreamChange"),
            Self::UserRequest => write!(f, "UserRequest"),
        }
    }
}

impl From<&str> for KeyframeReason {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "startup" => Self::Startup,
            "decoder reset" | "decoder queue overflow" => Self::DecoderReset,
            "stream change" => Self::StreamChange,
            "user" | "user request" => Self::UserRequest,
            _ => Self::PacketLoss,
        }
    }
}

impl From<String> for KeyframeReason {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamingStrategy {
    /// Локальная сеть Ethernet / сверхбыстрый Wi-Fi 5-6GHz (RTT < 3 мс, 90-120 FPS, битрейт до 45 Мбит/с, FastSync)
    UltraLan,
    /// Стандартный домашний Wi-Fi (RTT 3-15 мс, 60 FPS, WMM QoS, сбалансированный FEC и NACK)
    StandardWifi,
    /// Удаленное подключение через интернет / WAN (RTT > 15 мс, усиленный FEC 10:3, консервативный битрейт)
    WanRemote,
}

impl Default for StreamingStrategy {
    fn default() -> Self {
        Self::StandardWifi
    }
}

fn default_refresh_rate() -> u32 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub client_name: String,
    pub client_version: String,
    pub protocol_version: u32,
    pub client_token: u64,
    pub supported_codecs: Vec<VideoCodec>,
    pub preferred_codec: Option<VideoCodec>,
    pub screen_width: u32,
    pub screen_height: u32,
    pub target_fps: u32,
    #[serde(default = "default_refresh_rate")]
    pub display_refresh_rate: u32,
    #[serde(default)]
    pub supports_tearing: bool,
    pub dpi_scale: f32,
    pub video_port: u16,
    /// Клиент поддерживает NACK (Selective Retransmit)
    pub supports_nack: bool,
    /// Клиент поддерживает FEC (Forward Error Correction)
    pub supports_fec: bool,
}

impl Default for ClientHello {
    fn default() -> Self {
        Self {
            client_name: "remote-ward-client".into(),
            client_version: "0.1.0".into(),
            protocol_version: crate::packet::PROTOCOL_VERSION as u32,
            client_token: 0,
            supported_codecs: vec![VideoCodec::H264],
            preferred_codec: Some(VideoCodec::H264),
            screen_width: 1920,
            screen_height: 1080,
            target_fps: 60,
            display_refresh_rate: 60,
            supports_tearing: true,
            dpi_scale: 1.0,
            video_port: 48000,
            supports_nack: true,
            supports_fec: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub server_name: String,
    pub server_version: String,
    pub protocol_version: u32,
    pub session_id: u64,
    pub selected_codec: VideoCodec,
    #[serde(default)]
    pub strategy: StreamingStrategy,
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    /// Сервер включил NACK
    pub nack_enabled: bool,
    /// Сервер включил FEC
    pub fec_enabled: bool,
    /// Количество data-шардов в FEC-группе
    pub fec_data_shards: u8,
    /// Количество parity-шардов в FEC-группе
    pub fec_parity_shards: u8,
}

impl Default for ServerHello {
    fn default() -> Self {
        Self {
            server_name: "remote-ward-host".into(),
            server_version: "0.1.0".into(),
            protocol_version: crate::packet::PROTOCOL_VERSION as u32,
            session_id: 0,
            selected_codec: VideoCodec::H264,
            strategy: StreamingStrategy::StandardWifi,
            width: 1920,
            height: 1080,
            target_fps: 60,
            nack_enabled: true,
            fec_enabled: true,
            fec_data_shards: 10,
            fec_parity_shards: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    ClientHello(ClientHello),
    ServerHello(ServerHello),

    /// Запрос IDR / I-кадра (при потере пакетов или рассинхроне)
    RequestKeyframe {
        session_id: u64,
        reason: KeyframeReason,
    },

    /// NACK: запрос повторной отправки конкретного пакета в кадре.
    /// Используется вместо RequestKeyframe при наличии NACK-поддержки.
    /// Отправляется клиентом, когда обнаружен пропуск пакета в кадре
    /// (timeout ~6ms — меньше одного интервала кадра при 60fps).
    Nack {
        session_id: u64,
        frame_id: u32,
        /// Список индексов пропущенных пакетов (до 32 за раз)
        missing_packet_indices: Vec<u16>,
    },

    /// Изменение настроек на лету (битрейт, fps)
    ChangeStreamSettings {
        session_id: u64,
        target_bitrate_kbps: u32,
        target_fps: u32,
    },

    /// Телеметрия клиента
    ClientStats {
        session_id: u64,
        rtt_us: u32,
        jitter_us: u32,
        decode_latency_us: u32,
        render_latency_us: u32,
        packet_loss_rate: f32,
        /// Количество успешных FEC-восстановлений с момента последнего отчёта
        fec_recovered_count: u32,
        /// Количество NACK-запросов с момента последнего отчёта
        nack_sent_count: u32,
    },

    /// Завершение сессии
    Disconnect {
        session_id: u64,
        reason: String,
    },

    /// Пинг для измерения RTT и джиттера
    Ping {
        session_id: u64,
        sequence: u32,
        send_timestamp_us: u64,
    },

    /// Ответ на пинг
    Pong {
        session_id: u64,
        sequence: u32,
        send_timestamp_us: u64,
    },

    /// Калибровочный тестовый пакет для замера пропускной способности
    SpeedProbe {
        session_id: u64,
        sequence: u32,
        payload_size: u32,
    },

    /// Подтверждение получения тестового пакета
    SpeedProbeAck {
        session_id: u64,
        sequence: u32,
        payload_size: u32,
    },

    /// Отчет о калибровке сети
    CalibrationReport {
        session_id: u64,
        min_rtt_us: u32,
        avg_rtt_us: u32,
        jitter_us: u32,
        loss_rate: f32,
        selected_bitrate_kbps: u32,
    },

    /// Переключение активного монитора на хосте
    SwitchMonitor {
        session_id: u64,
        monitor_index: u32,
    },

    /// Подтверждение переключения монитора с новыми размерами
    SwitchMonitorAck {
        session_id: u64,
        monitor_index: u32,
        width: u32,
        height: u32,
    },

    /// Включение/выключение локального аппаратного курсора на клиенте
    SetLocalCursor {
        session_id: u64,
        enabled: bool,
    },
}

impl ControlMessage {
    /// Быстрая бинарная сериализация сообщения управления
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Десериализация сообщения управления
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Сериализация в сетевую датаграмму с префиксом MSG_TYPE_CONTROL
    pub fn to_packet(&self) -> Result<Vec<u8>, bincode::Error> {
        let mut buf = Vec::with_capacity(128);
        buf.push(crate::packet::MSG_TYPE_CONTROL);
        bincode::serialize_into(&mut buf, self)?;
        Ok(buf)
    }

    /// Строгая десериализация из сетевой датаграммы с префиксом MSG_TYPE_CONTROL
    pub fn from_packet(packet: &[u8]) -> Result<Self, bincode::Error> {
        if packet.first() == Some(&crate::packet::MSG_TYPE_CONTROL) {
            bincode::deserialize(&packet[1..])
        } else {
            Err(bincode::ErrorKind::Custom(
                "Пакет не содержит префикс MSG_TYPE_CONTROL".to_string(),
            )
            .into())
        }
    }

    /// Извлечь session_id из любого варианта ControlMessage (кроме Hello)
    pub fn session_id(&self) -> Option<u64> {
        match self {
            Self::ClientHello(_) | Self::ServerHello(_) => None,
            Self::RequestKeyframe { session_id, .. } => Some(*session_id),
            Self::Nack { session_id, .. } => Some(*session_id),
            Self::ChangeStreamSettings { session_id, .. } => Some(*session_id),
            Self::ClientStats { session_id, .. } => Some(*session_id),
            Self::Disconnect { session_id, .. } => Some(*session_id),
            Self::Ping { session_id, .. } => Some(*session_id),
            Self::Pong { session_id, .. } => Some(*session_id),
            Self::SpeedProbe { session_id, .. } => Some(*session_id),
            Self::SpeedProbeAck { session_id, .. } => Some(*session_id),
            Self::CalibrationReport { session_id, .. } => Some(*session_id),
            Self::SwitchMonitor { session_id, .. } => Some(*session_id),
            Self::SwitchMonitorAck { session_id, .. } => Some(*session_id),
            Self::SetLocalCursor { session_id, .. } => Some(*session_id),
        }
    }
}

