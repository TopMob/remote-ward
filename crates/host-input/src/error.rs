use thiserror::Error;

#[derive(Error, Debug)]
pub enum InputError {
    #[error("Ошибка вызова Windows SendInput: отправлено {sent} из {expected} событий, код ошибки Win32: {error_code}")]
    SendInputFailed { sent: u32, expected: u32, error_code: u32 },

    #[error("Некорректные параметры ввода: {0}")]
    InvalidParameters(String),
}
