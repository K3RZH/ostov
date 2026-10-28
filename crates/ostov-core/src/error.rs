use std::fmt;

use ostov_store::{DecimalError, StoreError};

#[derive(Debug)]
pub enum CoreError {
    Store(StoreError),
    NotFound {
        table: String,
        id: i64,
    },
    /// Запись успели изменить после того, как её прочитали (ADR-023).
    VersionConflict {
        table: String,
        id: i64,
    },
    Invalid(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::NotFound { table, id } => write!(f, "запись {id} в {table} не найдена"),
            Self::VersionConflict { table, id } => write!(
                f,
                "запись {id} в {table} уже изменил другой пользователь: откройте её заново"
            ),
            Self::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<StoreError> for CoreError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<DecimalError> for CoreError {
    fn from(e: DecimalError) -> Self {
        Self::Invalid(e.to_string())
    }
}
