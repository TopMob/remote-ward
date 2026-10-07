use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum VideoCodec {
    H264 = 1,
    HEVC = 2,
    AV1 = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PixelFormat {
    NV12 = 1,
    BGRA8 = 2,
    RGBA8 = 3,
    P010 = 4, // 10-bit HDR
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameMetadata {
    pub frame_id: u32,
    pub timestamp_us: u64,
    pub width: u32,
    pub height: u32,
    pub codec: VideoCodec,
    pub is_keyframe: bool,
    pub capture_latency_us: u32,
    pub encode_latency_us: u32,
}

/// Фрагментатор видеокадра на датаграммы подходящего размера под MTU
pub struct FramePacketizer;

impl FramePacketizer {
    /// Потоковый итератор по датаграммам без промежуточных выделений памяти
    pub fn packetize<'a>(
        session_id: u64,
        frame_id: u32,
        timestamp_us: u64,
        is_keyframe: bool,
        frame_data: &'a [u8],
        max_payload_size: usize,
    ) -> impl Iterator<Item = (super::packet::DatagramHeader, &'a [u8])> {
        let total_packets = if frame_data.is_empty() {
            1
        } else {
            frame_data.len().div_ceil(max_payload_size)
        } as u16;

        let mut flags_base = 0u8;
        if is_keyframe {
            flags_base |= super::packet::flags::KEYFRAME;
        }

        let chunks = frame_data.chunks(max_payload_size);
        let total = total_packets;

        chunks.enumerate().map(move |(idx, chunk)| {
            let mut flags = flags_base;
            let packet_index = idx as u16;
            if packet_index + 1 == total {
                flags |= super::packet::flags::LAST_PACKET_IN_FRAME;
            }

            let header = super::packet::DatagramHeader::new(
                session_id,
                super::packet::ChannelId::Video,
                flags,
                frame_id,
                packet_index,
                total,
                timestamp_us,
                chunk.len() as u16,
            );

            (header, chunk)
        })
    }

    /// Запись заголовка и полезной нагрузки в предоставленный вызывающей стороной буфер (Zero Allocation)
    #[inline]
    pub fn write_datagram(
        header: &super::packet::DatagramHeader,
        payload: &[u8],
        out_buf: &mut [u8],
    ) -> usize {
        let header_bytes = bytemuck::bytes_of(header);
        let total_len = header_bytes.len() + payload.len();
        debug_assert!(out_buf.len() >= total_len, "Предоставленный буфер меньше размера датаграммы");
        out_buf[..header_bytes.len()].copy_from_slice(header_bytes);
        out_buf[header_bytes.len()..total_len].copy_from_slice(payload);
        total_len
    }

    /// Разбиение кадра на готовые к отправке датаграммы (для тестов или фолбэка)
    pub fn packetize_to_datagrams(
        session_id: u64,
        frame_id: u32,
        timestamp_us: u64,
        is_keyframe: bool,
        frame_data: &[u8],
        max_payload_size: usize,
    ) -> Vec<Vec<u8>> {
        Self::packetize(session_id, frame_id, timestamp_us, is_keyframe, frame_data, max_payload_size)
            .map(|(header, chunk)| {
                let mut buf = Vec::with_capacity(super::packet::DatagramHeader::SIZE + chunk.len());
                buf.extend_from_slice(bytemuck::bytes_of(&header));
                buf.extend_from_slice(chunk);
                buf
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_packetization_zero_alloc() {
        let fake_frame = vec![42u8; 3000]; // 3000 bytes
        let max_payload = 1000;
        let packets: Vec<_> =
            FramePacketizer::packetize(123456, 100, 1_000_000, true, &fake_frame, max_payload).collect();

        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0].0.get_session_id(), 123456);
        assert_eq!(packets[0].0.get_total_packets(), 3);
        assert_eq!(packets[0].0.get_packet_index(), 0);
        assert!(packets[0].0.is_keyframe());
        assert!(!packets[0].0.is_last_packet());
        assert_eq!(packets[0].1.len(), 1000);

        assert_eq!(packets[2].0.get_packet_index(), 2);
        assert!(packets[2].0.is_last_packet());
        assert_eq!(packets[2].1.len(), 1000);

        let mut scratch = [0u8; 1500];
        let bytes_written = FramePacketizer::write_datagram(&packets[0].0, packets[0].1, &mut scratch);
        assert_eq!(bytes_written, super::super::packet::DatagramHeader::SIZE + 1000);
    }
}
