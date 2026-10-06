use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("Ошибка Windows API: {0}")]
    Windows(#[from] windows::core::Error),

    #[error("Аппаратный энкодер для кодека {0:?} не найден")]
    EncoderNotFound(core_protocol::VideoCodec),

    #[error("Не удалось настроить медиа-тип энкодера: {0}")]
    MediaTypeConfigFailed(String),

    #[error("Ошибка при обработке входного кадра (ProcessInput): {0}")]
    ProcessInputFailed(String),

    #[error("Ошибка при получении сжатого кадра (ProcessOutput): {0}")]
    ProcessOutputFailed(String),
}
