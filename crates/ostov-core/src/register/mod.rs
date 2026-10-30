//! Регистры. Пока только регистр накопления (остатки и обороты).

mod accumulation;
mod report;

pub use accumulation::{AccumulationRegister, Movement, MovementKind};
pub use report::{Balance, MAX_REPORT_ROWS, Turnover};
