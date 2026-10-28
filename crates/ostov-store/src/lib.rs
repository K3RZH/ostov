//! Слой хранения Ostov: драйверы и диалекты СУБД, тип [`Decimal`] для денег и количеств, миграции.
//!
//! Пока здесь только [`Decimal`]; драйверы SQLite и PostgreSQL появятся следующими.

#![deny(clippy::float_arithmetic)]

pub mod decimal;

pub use decimal::{Decimal, DecimalError, MAX_SCALE};
