//! Ядро: объекты, транзакции, проведение, регистры.

pub mod catalog;
pub mod error;
pub mod tx;

pub use catalog::{Catalog, Element, FieldDef, FieldKind, FieldValue};
pub use error::CoreError;
pub use tx::in_transaction;
