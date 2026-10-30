use std::fmt;

use ostov_store::{Decimal, DecimalError, StoreError};

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
    /// Проведение увело бы остаток в минус.
    NegativeBalance {
        register: String,
        resource: String,
        balance: Decimal,
        /// День, на конец которого остаток ушёл бы в минус (если у движений есть даты).
        date: Option<String>,
    },
    /// Дата документа попадает в закрытый период (СТ-24).
    PeriodClosed {
        date: String,
        closed_through: String,
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
            Self::NegativeBalance {
                register,
                resource,
                balance,
                date,
            } => match date {
                Some(date) => write!(
                    f,
                    "не хватает остатка на {date}: {resource} в {register} стал бы {balance}"
                ),
                None => write!(
                    f,
                    "не хватает остатка: {resource} в {register} стал бы {balance}"
                ),
            },
            Self::PeriodClosed {
                date,
                closed_through,
            } => write!(
                f,
                "период по {closed_through} закрыт: документ от {date} менять нельзя"
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
