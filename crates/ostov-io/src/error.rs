use std::fmt;

use ostov_api::EngineError;

#[derive(Debug)]
pub enum IoError {
    /// Ошибка ядра: права, проверки, остатки — как у интерфейса.
    Engine(EngineError),
    /// Файл не открылся, не прочитался или не записался.
    File(String),
    /// Данные или запрос не подходят: нет объекта, неверная колонка.
    Invalid(String),
    /// Превышен лимит на входной файл (НТ-Б4).
    Limit(String),
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(e) => write!(f, "{}", e.message),
            Self::File(reason) | Self::Invalid(reason) | Self::Limit(reason) => {
                write!(f, "{reason}")
            }
        }
    }
}

impl std::error::Error for IoError {}

impl From<EngineError> for IoError {
    fn from(e: EngineError) -> Self {
        Self::Engine(e)
    }
}

impl From<rust_xlsxwriter::XlsxError> for IoError {
    fn from(e: rust_xlsxwriter::XlsxError) -> Self {
        Self::File(format!("xlsx: {e}"))
    }
}
