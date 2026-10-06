use std::time::Duration;

/// Калькулятор интервалов пейсинга для плавной отправки датаграмм видеокадра
pub struct PacketPacer {
    /// Доля времени кадра, на которую растягивается отправка (например, 0.6 = 60% от интервала кадра)
    pacing_factor: f32,
}

impl Default for PacketPacer {
    fn default() -> Self {
        Self { pacing_factor: 0.6 }
    }
}

impl PacketPacer {
    pub fn new(pacing_factor: f32) -> Self {
        Self {
            pacing_factor: pacing_factor.clamp(0.1, 0.95),
        }
    }

    /// Рассчитать рекомендуемую задержку между отправками фрагментов кадра
    pub fn calculate_packet_delay(&self, total_packets: usize, target_fps: u32) -> Duration {
        if total_packets <= 1 || target_fps == 0 {
            return Duration::ZERO;
        }

        let frame_interval_us = 1_000_000.0 / target_fps as f64;
        let pacing_window_us = frame_interval_us * self.pacing_factor as f64;
        let per_packet_delay_us = (pacing_window_us / total_packets as f64) as u64;

        Duration::from_micros(per_packet_delay_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pacer_calculation() {
        let pacer = PacketPacer::new(0.6); // 60% от 16.66 мс (60 FPS) = ~10 мс окно отправки
        let delay = pacer.calculate_packet_delay(20, 60);

        // 10 000 мкс / 20 пакетов = ~500 мкс на пакет
        assert!(delay.as_micros() >= 450 && delay.as_micros() <= 550);
    }

    #[test]
    fn test_single_packet_zero_delay() {
        let pacer = PacketPacer::default();
        let delay = pacer.calculate_packet_delay(1, 60);
        assert_eq!(delay, Duration::ZERO);
    }
}
