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
    pub dpi_scale: f32,
    pub video_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub server_name: String,
    pub server_version: String,
    pub protocol_version: u32,
    pub session_id: u64,
    pub selected_codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
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
    /// Калибровочный тестовый пакет для замера пропускной способности (с реальной полезной нагрузкой)
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
        let mut buf = Vec::with_capacity(64);
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
}
