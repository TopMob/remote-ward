use core_protocol::{DatagramHeader, PacketError};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ReassembledFrame {
    pub session_id: u64,
    pub frame_id: u32,
    pub timestamp_us: u64,
    pub is_keyframe: bool,
    pub data: Vec<u8>,
}

struct PartialFrame {
    session_id: u64,
    total_packets: u16,
    received_packets: u16,
    timestamp_us: u64,
    is_keyframe: bool,
    created_at: Instant,
    chunks: Vec<Option<Vec<u8>>>,
}

/// Сравнивает порядковые номера кадров с корректной обработкой переполнения u32
#[inline]
pub fn is_newer_frame(new_id: u32, old_id: u32) -> bool {
    (new_id.wrapping_sub(old_id) as i32) > 0
}

/// Высокопроизводительный сборщик фрагментированных видеокадров с жесткими лимитами памяти
pub struct FrameReassembler {
    /// Очередь неполных кадров, строго отсортированная по frame_id
    pending_frames: BTreeMap<u32, PartialFrame>,
    max_frame_age: Duration,
    latest_completed_frame_id: u32,
    has_completed_any: bool,
    last_cleanup: Instant,
    max_pending_frames: usize,
    expected_session_id: Option<u64>,
}

impl FrameReassembler {
    pub const MAX_PACKETS_PER_FRAME: u16 = 1024; // 1024 * 1380 байт = ~1.4 МБ на один кадр

    pub fn new(max_frame_age: Duration) -> Self {
        Self {
            pending_frames: BTreeMap::new(),
            max_frame_age,
            latest_completed_frame_id: 0,
            has_completed_any: false,
            last_cleanup: Instant::now(),
            max_pending_frames: 8, // Не более 8 неполных кадров одновременно
            expected_session_id: None,
        }
    }

    /// Привязка сборщика к конкретному идентификатору сессии
    pub fn set_expected_session_id(&mut self, session_id: Option<u64>) {
        self.expected_session_id = session_id;
        self.pending_frames.clear();
        self.has_completed_any = false;
    }

    /// Обработка входящего сырого пакета датаграммы
    pub fn process_packet(
        &mut self,
        packet_bytes: &[u8],
    ) -> Result<Option<ReassembledFrame>, PacketError> {
        let (header, payload) = DatagramHeader::parse(packet_bytes)?;

        // Фильтрация чужих пакетов при наличии активной сессии
        if let Some(expected_sid) = self.expected_session_id {
            if header.get_session_id() != expected_sid {
                return Ok(None);
            }
        }

        // Пакеты Hole Punch не содержат видеоданных
        if header.is_hole_punch() {
            return Ok(None);
        }

        let frame_id = header.get_frame_id();
        let packet_idx = header.get_packet_index() as usize;
        let total_packets = header.get_total_packets();

        // Защита от поврежденных или вредоносных пакетов
        if total_packets == 0
            || total_packets > Self::MAX_PACKETS_PER_FRAME
            || packet_idx >= total_packets as usize
        {
            return Ok(None);
        }

        // Отбрасываем пакеты уже завершенных или устаревших кадров
        if self.has_completed_any && !is_newer_frame(frame_id, self.latest_completed_frame_id) {
            return Ok(None);
        }

        // Периодическая очистка устаревших кадров раз в 25 мс
        if self.last_cleanup.elapsed() >= Duration::from_millis(25) {
            self.cleanup_stale_frames();
            self.last_cleanup = Instant::now();
        }

        // Защита от переполнения очереди ожидания
        if self.pending_frames.len() >= self.max_pending_frames
            && !self.pending_frames.contains_key(&frame_id)
        {
            self.cleanup_stale_frames();
            if self.pending_frames.len() >= self.max_pending_frames {
                // Из BTreeMap гарантированно удаляем самый старый по frame_id кадр
                if let Some(&oldest_id) = self.pending_frames.keys().next() {
                    self.pending_frames.remove(&oldest_id);
                }
            }
        }

        let partial = self.pending_frames.entry(frame_id).or_insert_with(|| {
            PartialFrame {
                session_id: header.get_session_id(),
                total_packets,
                received_packets: 0,
                timestamp_us: header.get_timestamp_us(),
                is_keyframe: header.is_keyframe(),
                created_at: Instant::now(),
                chunks: vec![None; total_packets as usize],
            }
        });

        // Проверяем, не был ли этот фрагмент уже получен (дедупликация)
        if packet_idx < partial.chunks.len() && partial.chunks[packet_idx].is_none() {
            partial.chunks[packet_idx] = Some(payload.to_vec());
            partial.received_packets += 1;

            if partial.received_packets == partial.total_packets {
                // Все фрагменты получены — собираем целый кадр
                let total_size: usize = partial
                    .chunks
                    .iter()
                    .map(|c| c.as_ref().map_or(0, |b| b.len()))
                    .sum();

                let mut full_data = Vec::with_capacity(total_size);
                for chunk in partial.chunks.drain(..).flatten() {
                    full_data.extend_from_slice(&chunk);
                }

                let completed = ReassembledFrame {
                    session_id: partial.session_id,
                    frame_id,
                    timestamp_us: partial.timestamp_us,
                    is_keyframe: partial.is_keyframe,
                    data: full_data,
                };

                let is_keyframe = partial.is_keyframe;
                self.pending_frames.remove(&frame_id);

                if !self.has_completed_any || is_newer_frame(frame_id, self.latest_completed_frame_id) {
                    self.latest_completed_frame_id = frame_id;
                    self.has_completed_any = true;
                }

                // При получении ключевого кадра сбрасываем более старые неполные P-кадры
                if is_keyframe {
                    self.pending_frames.retain(|&id, _| is_newer_frame(id, frame_id));
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
            FramePacketizer::packetize(1234, 42, 1000, true, &fake_payload, 4).collect();

        let mut completed = None;
        for (header, chunk) in packets {
            let mut buf = vec![0u8; 1500];
            let len = FramePacketizer::write_datagram(&header, chunk, &mut buf);
            if let Some(frame) = reassembler.process_packet(&buf[..len]).unwrap() {
                completed = Some(frame);
            }
        }

        let frame = completed.expect("Кадр должен быть собран");
        assert_eq!(frame.session_id, 1234);
        assert_eq!(frame.frame_id, 42);
        assert!(frame.is_keyframe);
        assert_eq!(frame.data, fake_payload);
    }

    #[test]
    fn test_reassemble_out_of_order() {
        let mut reassembler = FrameReassembler::new(Duration::from_millis(100));
        let fake_payload = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut packets: Vec<_> =
            FramePacketizer::packetize(1234, 100, 2000, false, &fake_payload, 4).collect();

        // Меняем порядок фрагментов на обратный
        packets.reverse();

        let mut completed = None;
        for (header, chunk) in packets {
            let mut buf = vec![0u8; 1500];
            let len = FramePacketizer::write_datagram(&header, chunk, &mut buf);
            if let Some(frame) = reassembler.process_packet(&buf[..len]).unwrap() {
                completed = Some(frame);
            }
        }

        let frame = completed.expect("Кадр должен быть собран несмотря на переупорядочивание фрагментов");
        assert_eq!(frame.frame_id, 100);
        assert!(!frame.is_keyframe);
        assert_eq!(frame.data, fake_payload);
    }

    #[test]
    fn test_sequence_rollover() {
        assert!(is_newer_frame(1, 0));
        assert!(is_newer_frame(0, u32::MAX));
        assert!(!is_newer_frame(u32::MAX, 0));
        assert!(!is_newer_frame(10, 15));
    }
}
