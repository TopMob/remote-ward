use core_protocol::{DatagramHeader, PacketError};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ReassembledFrame {
    pub frame_id: u32,
    pub timestamp_us: u64,
    pub is_keyframe: bool,
    pub data: Vec<u8>,
}

struct PartialFrame {
    total_packets: u16,
    received_packets: u16,
    timestamp_us: u64,
    is_keyframe: bool,
    created_at: Instant,
    chunks: Vec<Option<Vec<u8>>>,
}

/// Сборщик фрагментированных кадров из датаграмм с защитой от рассинхрона и переупорядочивания
pub struct FrameReassembler {
    pending_frames: HashMap<u32, PartialFrame>,
    max_frame_age: Duration,
    latest_completed_frame_id: u32,
    last_cleanup: Instant,
}

impl FrameReassembler {
    pub fn new(max_frame_age: Duration) -> Self {
        Self {
            pending_frames: HashMap::new(),
            max_frame_age,
            latest_completed_frame_id: 0,
            last_cleanup: Instant::now(),
        }
    }

    /// Обработать входящий сырой пакет датаграммы
    pub fn process_packet(
        &mut self,
        packet_bytes: &[u8],
    ) -> Result<Option<ReassembledFrame>, PacketError> {
        let (header, payload) = DatagramHeader::parse(packet_bytes)?;
        let frame_id = header.get_frame_id();
        let packet_idx = header.get_packet_index() as usize;
        let total_packets = header.get_total_packets();

        // Защита от поврежденных или аномальных пакетов
        if total_packets == 0 || total_packets > 4096 {
            return Ok(None);
        }

        // Отбрасываем пакеты уже завершенных или слишком старых кадров
        if frame_id < self.latest_completed_frame_id
            && (self.latest_completed_frame_id.wrapping_sub(frame_id)) < 1000
        {
            return Ok(None);
        }

        // Периодическая очистка устаревших кадров раз в 50 мс вместо каждого пакета
        if self.last_cleanup.elapsed() >= Duration::from_millis(50) {
            self.cleanup_stale_frames();
            self.last_cleanup = Instant::now();
        }

        // Защита от переполнения очереди ожидания
        if self.pending_frames.len() >= 64 && !self.pending_frames.contains_key(&frame_id) {
            self.cleanup_stale_frames();
            if self.pending_frames.len() >= 64 {
                // Удаляем самый старый незавершенный кадр
                if let Some(&oldest_id) = self.pending_frames.keys().next() {
                    self.pending_frames.remove(&oldest_id);
                }
            }
        }

        let partial = self
            .pending_frames
            .entry(frame_id)
            .or_insert_with(|| PartialFrame {
                total_packets,
                received_packets: 0,
                timestamp_us: header.get_timestamp_us(),
                is_keyframe: header.is_keyframe(),
                created_at: Instant::now(),
                chunks: vec![None; total_packets as usize],
            });

        if packet_idx < partial.chunks.len() && partial.chunks[packet_idx].is_none() {
            partial.chunks[packet_idx] = Some(payload.to_vec());
            partial.received_packets += 1;

            if partial.received_packets == partial.total_packets {
                // Все фрагменты получены — собираем целый кадр
                let total_size: usize = partial.chunks.iter().map(|c| c.as_ref().map_or(0, |b| b.len())).sum();
                let mut full_data = Vec::with_capacity(total_size);
                for c in partial.chunks.drain(..).flatten() {
                    full_data.extend_from_slice(&c);
                }

                let completed = ReassembledFrame {
                    frame_id,
                    timestamp_us: partial.timestamp_us,
                    is_keyframe: partial.is_keyframe,
                    data: full_data,
                };

                self.pending_frames.remove(&frame_id);
                if frame_id > self.latest_completed_frame_id {
                    self.latest_completed_frame_id = frame_id;
                }

                return Ok(Some(completed));
            }
        }

        Ok(None)
    }

    fn cleanup_stale_frames(&mut self) {
        let now = Instant::now();
        let max_age = self.max_frame_age;
        self.pending_frames
            .retain(|_, frame| now.duration_since(frame.created_at) < max_age);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_protocol::FramePacketizer;

    #[test]
    fn test_reassemble_in_order() {
        let mut reassembler = FrameReassembler::new(Duration::from_millis(100));
        let fake_payload = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let packets: Vec<_> =
            FramePacketizer::packetize(42, 1000, true, &fake_payload, 4).collect();

        let mut completed = None;
        for (header, chunk) in packets {
            let mut buf = Vec::new();
            buf.extend_from_slice(bytemuck::bytes_of(&header));
            buf.extend_from_slice(chunk);
            if let Some(frame) = reassembler.process_packet(&buf).unwrap() {
                completed = Some(frame);
            }
        }

        let frame = completed.expect("Frame must be reassembled");
        assert_eq!(frame.frame_id, 42);
        assert!(frame.is_keyframe);
        assert_eq!(frame.data, fake_payload);
    }

    #[test]
    fn test_reassemble_out_of_order() {
        let mut reassembler = FrameReassembler::new(Duration::from_millis(100));
        let fake_payload = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut packets: Vec<_> =
            FramePacketizer::packetize(100, 2000, false, &fake_payload, 4).collect();

        // Меняем порядок пакетов: последний отправляем первым!
        packets.reverse();

        let mut completed = None;
        for (header, chunk) in packets {
            let mut buf = Vec::new();
            buf.extend_from_slice(bytemuck::bytes_of(&header));
            buf.extend_from_slice(chunk);
            if let Some(frame) = reassembler.process_packet(&buf).unwrap() {
                completed = Some(frame);
            }
        }

        let frame = completed.expect("Frame must be reassembled despite out-of-order packets");
        assert_eq!(frame.frame_id, 100);
        assert!(!frame.is_keyframe);
        assert_eq!(frame.data, fake_payload);
    }
}
