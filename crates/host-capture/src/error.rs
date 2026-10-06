use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("Ошибка Windows API: {0}")]
    Windows(#[from] windows::core::Error),

    #[error("Не найден подходящий видеоадаптер (GPU)")]
    NoAdapterFound,

    #[error("Не найден монитор/выход дисплея с индексом {0}")]
    OutputNotFound(u32),

    #[error("Таймаут ожидания нового кадра ({0} мс) — изображение не изменилось")]
    Timeout(u32),

    #[error("Доступ к захвату потерян (DXGI_ERROR_ACCESS_LOST) — требуется переинициализация")]
    AccessLost,

    #[error("Режим дисплея был изменен")]
    ModeChanged,

    #[error("Ошибка инициализации захвата: {0}")]
    InitializationFailed(String),

    #[error("Ошибка захвата кадра: {0}")]
    AcquireFailed(String),
}
