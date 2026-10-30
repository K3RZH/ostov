use std::fmt;

use ostov_store::{Decimal, DecimalError, StoreError, Value};

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
    NegativeBalance(Box<Shortage>),
    /// Дата документа попадает в закрытый период (СТ-24).
    PeriodClosed {
        date: String,
        closed_through: String,
    },
    /// Конфигурация не ложится на базу без потери данных (реструктуризация).
    Incompatible(String),
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
            Self::NegativeBalance(s) => match &s.date {
                Some(date) => write!(
                    f,
                    "не хватает остатка на {date}: {} в {} стал бы {}",
                    s.resource, s.register, s.balance
                ),
                None => write!(
                    f,
                    "не хватает остатка: {} в {} стал бы {}",
                    s.resource, s.register, s.balance
                ),
            },
            Self::PeriodClosed {
                date,
                closed_through,
            } => write!(
                f,
                "период по {closed_through} закрыт: документ от {date} менять нельзя"
            ),
            Self::Incompatible(msg) | Self::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CoreError {}

/// Чего и где не хватает: из этого интерфейсу собирается понятное сообщение.
#[derive(Debug)]
pub struct Shortage {
    pub register: String,
    pub resource: String,
    pub balance: Decimal,
    /// День, на конец которого остаток ушёл бы в минус (если у движений есть даты).
    pub date: Option<String>,
    /// Измерения, по которым не хватает: имя и значение (для ссылки — id).
    pub dims: Vec<(String, Value)>,
    /// Документы, которые списывают в этот день: таблица документа и id.
    pub blockers: Vec<(String, i64)>,
}

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
