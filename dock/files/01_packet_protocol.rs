use bytemuck::{Pod, Zeroable};
use thiserror::Error;

/// Магическое число заголовка протокола Remote-Ward: 'R' 'W' (0x5257)
pub const PROTOCOL_MAGIC: u16 = 0x5257;

/// Максимальный рекомендуемый размер полезной нагрузки датаграммы (MTU safe ~1400 байт)
pub const DEFAULT_MAX_PAYLOAD_SIZE: usize = 1380;

/// Префикс типа сообщения на канале управления и ввода
pub const MSG_TYPE_CONTROL: u8 = 0x01;
pub const MSG_TYPE_INPUT: u8 = 0x02;

#[derive(Debug, Error)]
pub enum PacketError {
    #[error("Недопустимый размер пакета: ожидалось минимум {expected} байт, получено {actual}")]
    TooSmall { expected: usize, actual: usize },
    #[error("Неверная сигнатура протокола: ожидалось {expected:#06x}, получено {actual:#06x}")]
    InvalidMagic { expected: u16, actual: u16 },
    #[error("Неизвестный канал: {0}")]
    UnknownChannel(u8),
    #[error("Несоответствие размера полезной нагрузки: в заголовке {declared}, в буфере {actual}")]
    PayloadLengthMismatch { declared: usize, actual: usize },
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelId {
    Video = 0,
    AudioHostToClient = 1,
    AudioClientToHost = 2,
    Input = 3,
}

impl TryFrom<u8> for ChannelId {
    type Error = PacketError;

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0 => Ok(Self::Video),
            1 => Ok(Self::AudioHostToClient),
            2 => Ok(Self::AudioClientToHost),
            3 => Ok(Self::Input),
            unknown => Err(PacketError::UnknownChannel(unknown)),
        }
    }
}

pub mod flags {
    pub const KEYFRAME: u8 = 1 << 0;
    pub const LAST_PACKET_IN_FRAME: u8 = 1 << 1;
    pub const FEC_PARITY: u8 = 1 << 2;
}

/// Фиксированный бинарный заголовок для высокоскоростных датаграмм (Zero-Copy)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct DatagramHeader {
    /// Сигнатура PROTOCOL_MAGIC (в Big-Endian)
    pub magic: [u8; 2],
    /// Канал данных (ChannelId)
    pub channel: u8,
    /// Флаги (KEYFRAME, LAST_PACKET_IN_FRAME, FEC_PARITY)
    pub flags: u8,
    /// Порядковый номер кадра / пакета (Big-Endian)
    pub frame_id: [u8; 4],
    /// Индекс фрагмента в текущем кадре (0..total_packets-1)
    pub packet_index: [u8; 2],
    /// Общее количество фрагментов в данном кадре
    pub total_packets: [u8; 2],
    /// Метка времени хоста в микросекундах (Unix time или монотонный Instant)
    pub timestamp_us: [u8; 8],
    /// Длина полезной нагрузки, следующей сразу за заголовком
    pub payload_len: [u8; 2],
}

impl DatagramHeader {
    pub const SIZE: usize = std::mem::size_of::<Self>();

    pub fn new(
        channel: ChannelId,
        flags: u8,
        frame_id: u32,
        packet_index: u16,
        total_packets: u16,
        timestamp_us: u64,
        payload_len: u16,
    ) -> Self {
        Self {
            magic: PROTOCOL_MAGIC.to_be_bytes(),
            channel: channel as u8,
            flags,
            frame_id: frame_id.to_be_bytes(),
            packet_index: packet_index.to_be_bytes(),
            total_packets: total_packets.to_be_bytes(),
            timestamp_us: timestamp_us.to_be_bytes(),
            payload_len: payload_len.to_be_bytes(),
        }
    }

    pub fn get_magic(&self) -> u16 {
        u16::from_be_bytes(self.magic)
    }

    pub fn get_channel(&self) -> Result<ChannelId, PacketError> {
        ChannelId::try_from(self.channel)
    }

    pub fn get_frame_id(&self) -> u32 {
        u32::from_be_bytes(self.frame_id)
    }

    pub fn get_packet_index(&self) -> u16 {
        u16::from_be_bytes(self.packet_index)
    }

    pub fn get_total_packets(&self) -> u16 {
        u16::from_be_bytes(self.total_packets)
    }

    pub fn get_timestamp_us(&self) -> u64 {
        u64::from_be_bytes(self.timestamp_us)
    }

    pub fn get_payload_len(&self) -> u16 {
        u16::from_be_bytes(self.payload_len)
    }

    pub fn is_keyframe(&self) -> bool {
        (self.flags & flags::KEYFRAME) != 0
    }

    pub fn is_last_packet(&self) -> bool {
        (self.flags & flags::LAST_PACKET_IN_FRAME) != 0
    }

    /// Парсинг среза байтов в заголовок без выделения памяти
    pub fn parse(buf: &[u8]) -> Result<(&Self, &[u8]), PacketError> {
        if buf.len() < Self::SIZE {
            return Err(PacketError::TooSmall {
                expected: Self::SIZE,
                actual: buf.len(),
            });
        }

        let (header_bytes, payload) = buf.split_at(Self::SIZE);
        let header: &Self = bytemuck::from_bytes(header_bytes);

        if header.get_magic() != PROTOCOL_MAGIC {
            return Err(PacketError::InvalidMagic {
                expected: PROTOCOL_MAGIC,
                actual: header.get_magic(),
            });
        }

        let declared_len = header.get_payload_len() as usize;
        if payload.len() < declared_len {
            return Err(PacketError::PayloadLengthMismatch {
                declared: declared_len,
                actual: payload.len(),
            });
        }

        Ok((header, &payload[..declared_len]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datagram_header_serialization() {
        let original = DatagramHeader::new(
            ChannelId::Video,
            flags::KEYFRAME | flags::LAST_PACKET_IN_FRAME,
            12345,
            0,
            1,
            9876543210,
            4,
        );

        let mut buffer = Vec::new();
        buffer.extend_from_slice(bytemuck::bytes_of(&original));
        buffer.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // Payload

        let (parsed, payload) = DatagramHeader::parse(&buffer).expect("Must parse header");
        assert_eq!(parsed.get_magic(), PROTOCOL_MAGIC);
        assert_eq!(parsed.get_channel().unwrap(), ChannelId::Video);
        assert!(parsed.is_keyframe());
        assert!(parsed.is_last_packet());
        assert_eq!(parsed.get_frame_id(), 12345);
        assert_eq!(parsed.get_packet_index(), 0);
        assert_eq!(parsed.get_total_packets(), 1);
        assert_eq!(parsed.get_timestamp_us(), 9876543210);
        assert_eq!(parsed.get_payload_len(), 4);
        assert_eq!(payload, &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_invalid_magic() {
        let mut buffer = vec![0u8; DatagramHeader::SIZE + 10];
        buffer[0] = 0xFF;
        let result = DatagramHeader::parse(&buffer);
        assert!(matches!(result, Err(PacketError::InvalidMagic { .. })));
    }
}
