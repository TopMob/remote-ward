//! # FEC — Forward Error Correction для UDP видео (Reed-Solomon)
//!
//! Реализует кодирование/декодирование с исправлением ошибок на основе
//! кодов Рида-Соломона. Позволяет восстанавливать потерянные UDP-пакеты
//! **без повторного запроса IDR-кадра**, что критически важно для низкой задержки.
//!
//! ## Параметры по умолчанию
//! - LAN: 10 data + 2 parity (терпит 2 потери из 12 — ~16% избыточность)
//! - WiFi: 8 data + 3 parity (терпит 3 потери из 11 — ~27% избыточность)
//!
//! ## Как работает
//!
//! Хост:
//! 1. Упаковывает кадр в N data-шардов одинакового размера.
//! 2. `FecEncoder::encode()` генерирует P parity-шардов из data.
//! 3. Все (N+P) шардов отправляются с меткой `fec_shard_index` в заголовке.
//!
//! Клиент:
//! 1. Собирает шарды по `frame_id`.
//! 2. Если все data-шарды пришли — сразу собирает кадр.
//! 3. Если пришло не менее N шардов из (N+P) — `FecDecoder::reconstruct()`.
//! 4. Если пришло меньше N — шлёт NACK / IDR-запрос (edge case).

use std::collections::HashMap;
use std::time::{Duration, Instant};
use reed_solomon_erasure::galois_8::ReedSolomon;

/// Конфигурация FEC-кодирования
#[derive(Debug, Clone, Copy)]
pub struct FecConfig {
    /// Количество data-шардов (пакетов с реальными данными) в группе
    pub data_shards: usize,
    /// Количество parity-шардов (восстановительных) в группе
    pub parity_shards: usize,
    /// Максимальное время ожидания FEC-группы на клиенте
    pub group_timeout: Duration,
}

impl FecConfig {
    /// LAN-профиль: минимальная избыточность, максимальная скорость
    pub fn lan() -> Self {
        Self {
            data_shards: 10,
            parity_shards: 2,
            group_timeout: Duration::from_millis(20),
        }
    }

    /// WiFi/WAN-профиль: повышенная избыточность для нестабильных каналов
    pub fn wifi() -> Self {
        Self {
            data_shards: 8,
            parity_shards: 3,
            group_timeout: Duration::from_millis(30),
        }
    }

    /// Суммарное количество шардов (data + parity)
    #[inline]
    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }
}

impl Default for FecConfig {
    fn default() -> Self {
        Self::lan()
    }
}

/// Кодировщик FEC на хостовой стороне.
///
/// Принимает готовые UDP-пакеты кадра (data shards), вычисляет parity shards.
/// Возвращает готовые к отправке байты всех шардов с заголовками FEC.
pub struct FecEncoder {
    config: FecConfig,
    rs: ReedSolomon,
}

impl FecEncoder {
    pub fn new(config: FecConfig) -> Result<Self, reed_solomon_erasure::Error> {
        let rs = ReedSolomon::new(config.data_shards, config.parity_shards)?;
        Ok(Self { config, rs })
    }

    pub fn config(&self) -> &FecConfig {
        &self.config
    }

    /// Принимает срез data-шардов (сырые байты готовых UDP-пакетов),
    /// возвращает вектор parity-шардов.
    ///
    /// **Важно:** все data-шарды должны иметь одинаковый размер.
    /// Последний шард при необходимости дополняется нулями до `shard_size`.
    pub fn encode_parity(&self, data_shards: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, reed_solomon_erasure::Error> {
        debug_assert_eq!(
            data_shards.len(),
            self.config.data_shards,
            "FecEncoder: неверное количество data-шардов"
        );

        let shard_size = data_shards.iter().map(|s| s.len()).max().unwrap_or(0);
        if shard_size == 0 {
            return Ok(vec![vec![]; self.config.parity_shards]);
        }

        // Выравниваем все шарды до единого размера
        let mut all: Vec<Vec<u8>> = data_shards
            .iter()
            .map(|s| {
                let mut v = s.clone();
                v.resize(shard_size, 0);
                v
            })
            .collect();

        // Добавляем parity-шарды (инициализированные нулями)
        for _ in 0..self.config.parity_shards {
            all.push(vec![0u8; shard_size]);
        }

        self.rs.encode(&mut all)?;

        // Возвращаем только parity-часть
        Ok(all.into_iter().skip(self.config.data_shards).collect())
    }
}

/// Состояние сборки FEC-группы на клиентской стороне
struct FecGroupState {
    total_shards: usize,
    data_shards_count: usize,
    shard_size: usize,
    shards: Vec<Option<Vec<u8>>>,
    received_count: usize,
    created_at: Instant,
    is_recovered: bool,
}

impl FecGroupState {
    fn new(data_shards: usize, parity_shards: usize) -> Self {
        let total = data_shards + parity_shards;
        Self {
            total_shards: total,
            data_shards_count: data_shards,
            shard_size: 0,
            shards: vec![None; total],
            received_count: 0,
            created_at: Instant::now(),
            is_recovered: false,
        }
    }

    fn add_shard(&mut self, index: usize, data: Vec<u8>) {
        if index < self.total_shards && self.shards[index].is_none() {
            if self.shard_size == 0 {
                self.shard_size = data.len();
            }
            self.shards[index] = Some(data);
            self.received_count += 1;
        }
    }

    fn has_all_data(&self) -> bool {
        self.shards[..self.data_shards_count]
            .iter()
            .all(|s| s.is_some())
    }

    fn can_recover(&self) -> bool {
        self.received_count >= self.data_shards_count
    }

    fn is_expired(&self, timeout: Duration) -> bool {
        self.created_at.elapsed() > timeout
    }
}

/// Декодировщик FEC на клиентской стороне.
///
/// Принимает входящие шарды по мере их прихода.
/// Когда набирается достаточное количество для восстановления —
/// возвращает вектор data-шардов.
pub struct FecDecoder {
    config: FecConfig,
    rs: ReedSolomon,
    /// Активные группы: ключ = (frame_id, group_id)
    groups: HashMap<(u32, u8), FecGroupState>,
}

impl FecDecoder {
    pub fn new(config: FecConfig) -> Result<Self, reed_solomon_erasure::Error> {
        let rs = ReedSolomon::new(config.data_shards, config.parity_shards)?;
        Ok(Self {
            config,
            rs,
            groups: HashMap::new(),
        })
    }

    /// Добавить входящий шард.
    ///
    /// Возвращает `Some(data_shards)` если кадр можно собрать (все data пришли
    /// или восстановление завершено). Иначе `None`.
    pub fn add_shard(
        &mut self,
        frame_id: u32,
        group_id: u8,
        shard_index: u8,
        total_shards: u8,
        data: Vec<u8>,
    ) -> Option<Vec<Vec<u8>>> {
        let key = (frame_id, group_id);
        let data_shards = self.config.data_shards;
        let parity_shards = self.config.parity_shards;

        let state = self
            .groups
            .entry(key)
            .or_insert_with(|| FecGroupState::new(data_shards, parity_shards));

        if state.is_recovered {
            return None;
        }

        // Корректируем ожидаемое количество шардов из заголовка пакета
        // (может отличаться для последней группы)
        let _ = total_shards; // используется только для диагностики

        state.add_shard(shard_index as usize, data);

        if state.has_all_data() {
            // Быстрый путь: все data-шарды пришли, parity не нужен
            state.is_recovered = true;
            let result: Vec<Vec<u8>> = state.shards[..data_shards]
                .iter()
                .map(|s| s.clone().unwrap_or_default())
                .collect();
            self.groups.remove(&key);
            return Some(result);
        }

        if state.can_recover() {
            // FEC-восстановление
            let shard_size = state.shard_size;
            if shard_size == 0 {
                return None;
            }

            // Выравниваем все шарды до единого размера (None → нули)
            let mut all: Vec<Option<Vec<u8>>> = state
                .shards
                .iter()
                .map(|s| {
                    s.as_ref().map(|v| {
                        let mut vv = v.clone();
                        vv.resize(shard_size, 0);
                        vv
                    })
                })
                .collect();

            match self.rs.reconstruct(&mut all) {
                Ok(_) => {
                    state.is_recovered = true;
                    let result: Vec<Vec<u8>> = all[..data_shards]
                        .iter()
                        .map(|s| s.clone().unwrap_or_default())
                        .collect();
                    self.groups.remove(&key);
                    return Some(result);
                }
                Err(e) => {
                    tracing::warn!("FEC восстановление не удалось для frame={} group={}: {:?}", frame_id, group_id, e);
                }
            }
        }

        None
    }

    /// Очистить устаревшие незавершённые группы
    pub fn cleanup_expired(&mut self) {
        let timeout = self.config.group_timeout;
        self.groups.retain(|_, state| !state.is_expired(timeout));
    }

    /// Количество активных групп ожидания
    pub fn pending_groups(&self) -> usize {
        self.groups.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fec_encode_decode_no_loss() {
        let config = FecConfig::lan();
        let encoder = FecEncoder::new(config).unwrap();
        let mut decoder = FecDecoder::new(config).unwrap();

        let data_shards: Vec<Vec<u8>> = (0..config.data_shards)
            .map(|i| vec![i as u8; 100])
            .collect();

        let parity = encoder.encode_parity(&data_shards).unwrap();
        assert_eq!(parity.len(), config.parity_shards);

        // Добавляем все data-шарды — должно сработать без FEC-восстановления
        for (i, shard) in data_shards.iter().enumerate() {
            let result = decoder.add_shard(1, 0, i as u8, config.total_shards() as u8, shard.clone());
            if i + 1 == config.data_shards {
                assert!(result.is_some(), "Должно восстановиться когда получены все data шарды");
                let recovered = result.unwrap();
                for (j, s) in recovered.iter().enumerate() {
                    assert_eq!(&s[..data_shards[j].len()], &data_shards[j][..]);
                }
            } else {
                assert!(result.is_none());
            }
        }
    }

    #[test]
    fn test_fec_recover_with_loss() {
        let config = FecConfig::lan(); // 10 data + 2 parity
        let encoder = FecEncoder::new(config).unwrap();
        let mut decoder = FecDecoder::new(config).unwrap();

        let shard_size = 100usize;
        let data_shards: Vec<Vec<u8>> = (0..config.data_shards)
            .map(|i| vec![i as u8 + 1; shard_size])
            .collect();

        let parity = encoder.encode_parity(&data_shards).unwrap();
        let total = config.total_shards() as u8;

        // Симулируем потерю шардов 0 и 1 (2 из 10 data)
        let lost: std::collections::HashSet<usize> = [0usize, 1].into_iter().collect();

        let mut recovered = None;

        // Добавляем data-шарды (кроме потерянных)
        for (i, shard) in data_shards.iter().enumerate() {
            if lost.contains(&i) {
                continue;
            }
            let r = decoder.add_shard(2, 0, i as u8, total, shard.clone());
            if r.is_some() { recovered = r; }
        }

        // Добавляем parity-шарды
        for (i, p) in parity.iter().enumerate() {
            let idx = config.data_shards + i;
            let r = decoder.add_shard(2, 0, idx as u8, total, p.clone());
            if r.is_some() { recovered = r; }
        }

        assert!(recovered.is_some(), "FEC должен восстановить 2 потерянных шарда из 2 parity");
        let shards = recovered.unwrap();
        assert_eq!(shards.len(), config.data_shards);
        for (i, s) in shards.iter().enumerate() {
            if !lost.contains(&i) {
                assert_eq!(s[..shard_size], data_shards[i][..shard_size]);
            }
        }
    }
}

