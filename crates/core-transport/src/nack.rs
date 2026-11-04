//! # NACK — Selective Retransmit для UDP видео
//!
//! Вместо запроса нового IDR-кадра при потере одного пакета, клиент
//! отправляет NACK — запрос повторной отправки конкретных пакетов.
//!
//! ## Алгоритм (клиентская сторона — NackTracker)
//!
//! 1. Reassembler получает пакет → обновляет битмап полученных пакетов кадра.
//! 2. При получении пакета с `packet_index > ожидаемого` — детектируется пропуск.
//! 3. Через `NACK_DELAY_MS` (6ms) без получения пропущенных пакетов →
//!    отправляем `ControlMessage::Nack { missing_packet_indices }`.
//! 4. При получении LAST_PACKET_IN_FRAME — если кадр ещё не полный,
//!    немедленно шлём NACK для всех отсутствующих.
//!
//! ## Алгоритм (серверная сторона — NackBuffer)
//!
//! 1. При отправке каждого пакета — сохраняем копию в кольцевом буфере.
//! 2. При получении NACK — ищем нужные пакеты в буфере и повторно отправляем.
//! 3. Буфер ограничен `MAX_FRAMES_BUFFERED` кадрами.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Задержка перед отправкой NACK после детектирования пробела
/// 6ms << 16.6ms (1 кадр при 60fps)
pub const NACK_DELAY: Duration = Duration::from_millis(6);

/// Максимальное количество кадров в NACK-буфере сервера
pub const MAX_FRAMES_BUFFERED: usize = 8;

/// Максимальное количество пакетов, которые NACK-буфер хранит на кадр
pub const MAX_PACKETS_PER_FRAME: usize = 512;

// ─── Клиентская сторона ───────────────────────────────────────────────────────

/// Информация о конкретном пропущенном пакете
struct MissingPacket {
    packet_index: u16,
    detected_at: Instant,
    nack_sent: bool,
}

/// Трекер пропущенных пакетов на клиентской стороне.
///
/// Используется совместно с `FrameReassembler`: при обнаружении пробела
/// в sequence пакетов кадра регистрирует пропущенный пакет.
pub struct NackTracker {
    /// Пропущенные пакеты по frame_id
    missing: HashMap<u32, Vec<MissingPacket>>,
    nack_delay: Duration,
}

impl NackTracker {
    pub fn new() -> Self {
        Self {
            missing: HashMap::new(),
            nack_delay: NACK_DELAY,
        }
    }

    pub fn with_delay(delay: Duration) -> Self {
        Self {
            missing: HashMap::new(),
            nack_delay: delay,
        }
    }

    /// Зарегистрировать пропущенный пакет кадра
    pub fn register_missing(&mut self, frame_id: u32, packet_index: u16) {
        let entry = self.missing.entry(frame_id).or_default();
        // Не добавлять дубликаты
        if !entry.iter().any(|m| m.packet_index == packet_index) {
            entry.push(MissingPacket {
                packet_index,
                detected_at: Instant::now(),
                nack_sent: false,
            });
        }
    }

    /// Отметить пакет как полученный (пришёл после NACK или сам дошёл)
    pub fn mark_received(&mut self, frame_id: u32, packet_index: u16) {
        if let Some(missing) = self.missing.get_mut(&frame_id) {
            missing.retain(|m| m.packet_index != packet_index);
            if missing.is_empty() {
                self.missing.remove(&frame_id);
            }
        }
    }

    /// Удалить все записи для завершённого кадра
    pub fn frame_complete(&mut self, frame_id: u32) {
        self.missing.remove(&frame_id);
    }

    /// Получить список NACK-запросов, готовых к отправке (задержка истекла).
    ///
    /// Возвращает вектор `(frame_id, missing_indices)`.
    /// После вызова маркирует возвращённые пакеты как "NACK отправлен".
    pub fn drain_pending_nacks(&mut self) -> Vec<(u32, Vec<u16>)> {
        let delay = self.nack_delay;
        let mut result = Vec::new();

        for (frame_id, missing) in &mut self.missing {
            let pending: Vec<u16> = missing
                .iter_mut()
                .filter(|m| !m.nack_sent && m.detected_at.elapsed() >= delay)
                .map(|m| {
                    m.nack_sent = true;
                    m.packet_index
                })
                .collect();

            if !pending.is_empty() {
                result.push((*frame_id, pending));
            }
        }

        result
    }

    /// Немедленные NACK'и для всех оставшихся пакетов кадра
    /// (вызывается при получении LAST_PACKET_IN_FRAME но кадр ещё неполный)
    pub fn drain_immediate_nacks(&mut self, frame_id: u32) -> Vec<u16> {
        if let Some(missing) = self.missing.get_mut(&frame_id) {
            let indices: Vec<u16> = missing
                .iter_mut()
                .filter(|m| !m.nack_sent)
                .map(|m| {
                    m.nack_sent = true;
                    m.packet_index
                })
                .collect();
            indices
        } else {
            Vec::new()
        }
    }

    /// Удалить устаревшие записи (кадры старше threshold)
    pub fn cleanup_old(&mut self, max_age: Duration) {
        self.missing.retain(|_, missing| {
            missing.retain(|m| m.detected_at.elapsed() < max_age);
            !missing.is_empty()
        });
    }

    /// Количество кадров с отслеживаемыми пропусками
    pub fn tracked_frames(&self) -> usize {
        self.missing.len()
    }
}

impl Default for NackTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Серверная сторона ───────────────────────────────────────────────────────

/// Запись об отправленном пакете для возможной повторной отправки
#[derive(Clone)]
pub struct BufferedPacket {
    pub frame_id: u32,
    pub packet_index: u16,
    /// Полные байты датаграммы (заголовок + полезная нагрузка)
    pub datagram: Vec<u8>,
    pub sent_at: Instant,
}

/// Кольцевой буфер отправленных пакетов на серверной стороне.
///
/// Хранит последние `MAX_FRAMES_BUFFERED` кадров.
/// При получении NACK ищет нужный пакет и возвращает его для повторной отправки.
pub struct NackBuffer {
    /// Кольцевая очередь: (frame_id, packets)
    frames: VecDeque<(u32, Vec<BufferedPacket>)>,
    max_frames: usize,
}

impl NackBuffer {
    pub fn new() -> Self {
        Self {
            frames: VecDeque::with_capacity(MAX_FRAMES_BUFFERED),
            max_frames: MAX_FRAMES_BUFFERED,
        }
    }

    pub fn with_capacity(max_frames: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(max_frames),
            max_frames,
        }
    }

    /// Начать запись нового кадра
    pub fn begin_frame(&mut self, frame_id: u32) {
        // Удаляем самый старый кадр если буфер заполнен
        if self.frames.len() >= self.max_frames {
            self.frames.pop_front();
        }
        self.frames.push_back((frame_id, Vec::with_capacity(32)));
    }

    /// Добавить отправленный пакет в буфер текущего кадра
    pub fn buffer_packet(&mut self, frame_id: u32, packet_index: u16, datagram: Vec<u8>) {
        // Находим запись для этого frame_id
        if let Some((_, packets)) = self.frames.iter_mut().rev().find(|(fid, _)| *fid == frame_id) {
            if packets.len() < MAX_PACKETS_PER_FRAME {
                packets.push(BufferedPacket {
                    frame_id,
                    packet_index,
                    datagram,
                    sent_at: Instant::now(),
                });
            }
        }
    }

    /// Найти пакеты по NACK-запросу.
    ///
    /// Возвращает список датаграмм готовых к повторной отправке.
    pub fn get_packets_for_nack(
        &self,
        frame_id: u32,
        missing_indices: &[u16],
    ) -> Vec<Vec<u8>> {
        let mut result = Vec::with_capacity(missing_indices.len());

        if let Some((_, packets)) = self.frames.iter().rev().find(|(fid, _)| *fid == frame_id) {
            for &idx in missing_indices {
                if let Some(pkt) = packets.iter().find(|p| p.packet_index == idx) {
                    result.push(pkt.datagram.clone());
                }
            }
        }

        result
    }

    /// Удалить кадры старше указанного порога
    pub fn cleanup_old(&mut self, max_age: Duration) {
        self.frames.retain(|(_, packets)| {
            packets.first().map_or(true, |p| p.sent_at.elapsed() < max_age)
        });
    }

    /// Количество кадров в буфере
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}

impl Default for NackBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nack_tracker_basic() {
        let mut tracker = NackTracker::with_delay(Duration::from_millis(0));

        tracker.register_missing(10, 3);
        tracker.register_missing(10, 7);

        // Задержка 0ms → сразу должны быть готовы
        let nacks = tracker.drain_pending_nacks();
        assert_eq!(nacks.len(), 1);
        assert_eq!(nacks[0].0, 10);
        assert!(nacks[0].1.contains(&3));
        assert!(nacks[0].1.contains(&7));

        // Повторный вызов — NACK уже отправлен, не дублировать
        let nacks2 = tracker.drain_pending_nacks();
        assert!(nacks2.is_empty());
    }

    #[test]
    fn test_nack_buffer_retransmit() {
        let mut buffer = NackBuffer::new();

        buffer.begin_frame(1);
        buffer.buffer_packet(1, 0, vec![1, 2, 3]);
        buffer.buffer_packet(1, 1, vec![4, 5, 6]);
        buffer.buffer_packet(1, 2, vec![7, 8, 9]);

        let retransmits = buffer.get_packets_for_nack(1, &[1, 2]);
        assert_eq!(retransmits.len(), 2);
        assert_eq!(retransmits[0], vec![4, 5, 6]);
        assert_eq!(retransmits[1], vec![7, 8, 9]);
    }

    #[test]
    fn test_nack_buffer_max_frames() {
        let mut buffer = NackBuffer::with_capacity(3);

        for i in 0..5u32 {
            buffer.begin_frame(i);
            buffer.buffer_packet(i, 0, vec![i as u8]);
        }

        // Только последние 3 кадра в буфере
        assert_eq!(buffer.frame_count(), 3);
        // Старые кадры 0 и 1 удалены
        assert!(buffer.get_packets_for_nack(0, &[0]).is_empty());
        // Кадры 2, 3, 4 доступны
        assert!(!buffer.get_packets_for_nack(4, &[0]).is_empty());
    }
}

