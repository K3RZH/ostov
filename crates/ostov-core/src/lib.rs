//! Ядро: объекты, транзакции, проведение, регистры.

pub mod error;
pub mod object;
pub mod posting;
pub mod register;
pub mod tx;

#[cfg(test)]
mod testing;

pub use error::CoreError;
pub use object::catalog::{Catalog, Element};
pub use object::document::{Document, DocumentKind, TablePart};
pub use object::{FieldDef, FieldKind, FieldValue, Fields};
pub use posting::{post, unpost};
pub use register::{AccumulationRegister, Movement, MovementKind};
pub use tx::in_transaction;
