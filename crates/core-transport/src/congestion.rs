/// Параметры адаптивного контроллера битрейта и перегрузки сети
#[derive(Debug, Clone)]
pub struct CongestionConfig {
    pub min_bitrate_kbps: u32,
    pub max_bitrate_kbps: u32,
    pub initial_bitrate_kbps: u32,
    /// Порог превышения задержки относительно базового RTT для выявления bufferbloat в роутере (микросекунды)
    pub bufferbloat_threshold_us: u32,
}

impl Default for CongestionConfig {
    fn default() -> Self {
        Self {
            min_bitrate_kbps: 8_000,
            max_bitrate_kbps: 50_000,
            initial_bitrate_kbps: 25_000,
            bufferbloat_threshold_us: 15_000, // 15 мс очередь в буфере роутера
        }
    }
}

/// Адаптивный контроллер битрейта (ABR / Congestion Controller) на базе задержки и потерь (Delay & Loss sensitive)
pub struct CongestionController {
    config: CongestionConfig,
    current_bitrate_kbps: u32,
    smoothed_rtt_us: f32,
    min_rtt_us: u32,
    needs_keyframe: bool,
}

impl CongestionController {
    pub fn new(config: CongestionConfig) -> Self {
        let initial = config.initial_bitrate_kbps.clamp(config.min_bitrate_kbps, config.max_bitrate_kbps);
        Self {
            config,
            current_bitrate_kbps: initial,
            smoothed_rtt_us: 0.0,
            min_rtt_us: u32::MAX,
            needs_keyframe: false,
        }
    }

    pub fn current_bitrate_kbps(&self) -> u32 {
        self.current_bitrate_kbps
    }

    pub fn take_keyframe_request(&mut self) -> bool {
        let req = self.needs_keyframe;
        self.needs_keyframe = false;
        req
    }

    pub fn smoothed_rtt_us(&self) -> u32 {
        self.smoothed_rtt_us as u32
    }

    /// Принудительно установить битрейт (например, по результатам калибровки)
    pub fn set_target_bitrate(&mut self, bitrate_kbps: u32) {
        self.current_bitrate_kbps = bitrate_kbps.clamp(
            self.config.min_bitrate_kbps,
            self.config.max_bitrate_kbps,
        );
    }

    /// Обновление состояния контроллера при получении телеметрии от клиента
    pub fn update_feedback(&mut self, rtt_us: u32, packet_loss_rate: f32) -> u32 {
        if rtt_us > 0 {
            if self.min_rtt_us == u32::MAX || rtt_us < self.min_rtt_us {
                self.min_rtt_us = rtt_us;
            }

            // Экспоненциальное скользящее среднее (EWMA, alpha = 0.2)
            if self.smoothed_rtt_us == 0.0 {
                self.smoothed_rtt_us = rtt_us as f32;
            } else {
                self.smoothed_rtt_us = self.smoothed_rtt_us * 0.8 + (rtt_us as f32) * 0.2;
            }
        }

        let old_bitrate = self.current_bitrate_kbps;

        // 1. Критическая перегрузка / потеря пакетов
        if packet_loss_rate > 0.02 {
            // Мультипликативное снижение битрейта пропорционально потерям
            let backoff_factor = (1.0 - (packet_loss_rate * 2.0).min(0.35)).max(0.65);
            let target = (self.current_bitrate_kbps as f32 * backoff_factor) as u32;
            self.current_bitrate_kbps = target.max(self.config.min_bitrate_kbps);

            if packet_loss_rate >= 0.05 {
                self.needs_keyframe = true;
            }

            tracing::debug!(
                "[ABR] Потери {:.1}%, снижаем битрейт: {} -> {} кбит/с",
                packet_loss_rate * 100.0,
                old_bitrate,
                self.current_bitrate_kbps
            );
        }
        // 2. Предотвращение Bufferbloat (пинг вырос, но пакеты пока не теряются)
        else if self.min_rtt_us != u32::MAX
            && rtt_us > self.min_rtt_us + self.config.bufferbloat_threshold_us
            && self.current_bitrate_kbps > self.config.min_bitrate_kbps + 4_000
        {
            let target = self.current_bitrate_kbps.saturating_sub(2_000);
            self.current_bitrate_kbps = target.max(self.config.min_bitrate_kbps);
            tracing::debug!(
                "[ABR] Bufferbloat (RTT {} > {} + {} мкс), плавный откат: {} -> {} кбит/с",
                rtt_us,
                self.min_rtt_us,
                self.config.bufferbloat_threshold_us,
                old_bitrate,
                self.current_bitrate_kbps
            );
        }
        // 3. Сеть стабильна и свободна — аддитивное повышение битрейта (AIMD)
        else if packet_loss_rate < 0.005 && self.current_bitrate_kbps < self.config.max_bitrate_kbps {
            let increment = 1_000;
            self.current_bitrate_kbps = (self.current_bitrate_kbps + increment)
                .min(self.config.max_bitrate_kbps);
            tracing::trace!(
                "[ABR] Сеть свободна, повышение битрейта: {} -> {} кбит/с",
                old_bitrate,
                self.current_bitrate_kbps
            );
        }

        self.current_bitrate_kbps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_congestion_controller_reactions() {
        let config = CongestionConfig {
            min_bitrate_kbps: 10_000,
            max_bitrate_kbps: 40_000,
            initial_bitrate_kbps: 20_000,
            bufferbloat_threshold_us: 10_000,
        };

        let mut controller = CongestionController::new(config);
        assert_eq!(controller.current_bitrate_kbps(), 20_000);

        // Стабильный пинг 5 мс без потерь -> плавный рост
        controller.update_feedback(5_000, 0.0);
        assert_eq!(controller.current_bitrate_kbps(), 21_000);

        // Потери 5% -> резкий сброс и запрос ключевого кадра
        controller.update_feedback(5_000, 0.05);
        assert!(controller.current_bitrate_kbps() < 21_000);
        assert!(controller.take_keyframe_request());

        // Bufferbloat: пинг подскочил с 5 мс до 25 мс (> 5 + 10 мс)
        let before = controller.current_bitrate_kbps();
        controller.update_feedback(25_000, 0.0);
        assert!(controller.current_bitrate_kbps() <= before);
    }
}
