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
    pub fn packetize(
        frame_id: u32,
        timestamp_us: u64,
        is_keyframe: bool,
        frame_data: &[u8],
        max_payload_size: usize,
    ) -> impl Iterator<Item = (super::packet::DatagramHeader, &[u8])> {
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

    /// Удобный метод для разбиения кадра на готовые к отправке байтовые датаграммы (заголовок + тело)
    pub fn packetize_to_datagrams(
        frame_id: u32,
        timestamp_us: u64,
        is_keyframe: bool,
        frame_data: &[u8],
        max_payload_size: usize,
    ) -> Vec<Vec<u8>> {
        Self::packetize(frame_id, timestamp_us, is_keyframe, frame_data, max_payload_size)
            .map(|(header, chunk)| {
                let mut buf = Vec::with_capacity(std::mem::size_of::<super::packet::DatagramHeader>() + chunk.len());
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
    fn test_frame_packetization() {
        let fake_frame = vec![42u8; 3000]; // 3000 bytes
        let max_payload = 1000;
        let packets: Vec<_> =
            FramePacketizer::packetize(100, 1_000_000, true, &fake_frame, max_payload).collect();

        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0].0.get_total_packets(), 3);
        assert_eq!(packets[0].0.get_packet_index(), 0);
        assert!(packets[0].0.is_keyframe());
        assert!(!packets[0].0.is_last_packet());
        assert_eq!(packets[0].1.len(), 1000);

        assert_eq!(packets[2].0.get_packet_index(), 2);
        assert!(packets[2].0.is_last_packet());
        assert_eq!(packets[2].1.len(), 1000);
    }
}
