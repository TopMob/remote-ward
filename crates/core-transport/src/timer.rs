//! # High-Resolution Multimedia Timer for Windows
//!
//! Переключает системный таймер прерываний Windows с 15.625 мс (64 Гц)
//! на 1.0 мс (1000 Гц), устраняя микростаттеры и задержки в `sleep`, `WaitUntil`
//! и микросекундном пейсинге сетевых пакетов.

#[cfg(windows)]
pub struct ScopedTimerPeriod {
    period_ms: u32,
}

#[cfg(windows)]
impl ScopedTimerPeriod {
    /// Установить системное разрешение таймера в миллисекундах (обычно 1 мс).
    /// При выходе из области видимости (Drop) автоматически восстанавливает системное значение.
    pub fn set(period_ms: u32) -> Self {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(u_period: u32) -> u32;
        }
        unsafe {
            let res = timeBeginPeriod(period_ms);
            if res == 0 {
                tracing::debug!("Windows Multimedia Timer установлен на {} мс", period_ms);
            } else {
                tracing::warn!("Не удалось вызвать timeBeginPeriod({}): код {}", period_ms, res);
            }
        }
        Self { period_ms }
    }
}

#[cfg(windows)]
impl Drop for ScopedTimerPeriod {
    fn drop(&mut self) {
        #[link(name = "winmm")]
        extern "system" {
            fn timeEndPeriod(u_period: u32) -> u32;
        }
        unsafe {
            let res = timeEndPeriod(self.period_ms);
            if res == 0 {
                tracing::debug!("Windows Multimedia Timer успешно сброшен ({})", self.period_ms);
            }
        }
    }
}

#[cfg(not(windows))]
pub struct ScopedTimerPeriod;

#[cfg(not(windows))]
impl ScopedTimerPeriod {
    pub fn set(_period_ms: u32) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scoped_timer_creation() {
        let _timer = ScopedTimerPeriod::set(1);
        // Timer автоматически вернется в исходное состояние при drop
    }
}

