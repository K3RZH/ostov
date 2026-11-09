//! Слой хранения: драйверы и диалекты СУБД, тип `Decimal`.

#![deny(clippy::float_arithmetic)]

pub mod decimal;
pub mod driver;
pub mod drivers;
mod nesting;

pub use decimal::{Decimal, DecimalError, MAX_SCALE};
pub use driver::{Dialect, Driver, Row, StoreError, Value};
pub(crate) use nesting::Depth;
