use std::fmt;

/// По коду интерфейс выбирает, что и где показать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotImplemented,
    /// Нет связи с сервером или базой.
    Unavailable,
    BadCredentials,
    /// Первый запуск сервера: нужно создать администратора (СТ-36).
    NeedFirstAdmin,
    /// Файл базы зашифрован (СТ-35).
    NeedFilePassword,
    /// База уже открыта другим экземпляром (СТ-55).
    AlreadyOpen,
    Forbidden,
    /// Объект изменили после чтения (ADR-023).
    VersionConflict,
    PeriodClosed,
    /// Данные не прошли проверку: неверное поле, остаток ушёл бы в минус и т. п.
    Validation,
    /// Не подходит версия протокола или формата конфигурации (ИТ-06, СТ-07).
    Incompatible,
    NotFound,
    Internal,
}

/// `message` — для человека («что делать»), `details` — в журнал.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub code: ErrorCode,
    pub message: String,
    pub details: String,
}

impl EngineError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: String::new(),
        }
    }

    pub fn not_implemented(command: &str) -> Self {
        Self {
            code: ErrorCode::NotImplemented,
            message: "Ядро ещё не подключено.".to_owned(),
            details: format!("команда {command} не реализована"),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for EngineError {}

pub type EngineResult<T> = Result<T, EngineError>;
