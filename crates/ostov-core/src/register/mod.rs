//! Регистры. Пока только регистр накопления (остатки и обороты).

mod accumulation;

pub use accumulation::{AccumulationRegister, Movement, MovementKind};
