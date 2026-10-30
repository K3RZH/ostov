//! Контракт между интерфейсом и ядром (Дизайн §3).
//!
//! Интерфейс знает только трейт [`Engine`]. В режиме 1 за ним ядро в процессе (`LocalEngine`),
//! в режиме 2 — gRPC-клиент (`ostov-client`). Открывает базу [`Connector`].
//! Одна команда — одна транзакция; данные и ошибки без ссылок на память ядра.
//!
//! За основу взята заготовка Артёма (`ostov-ui/src/engine.rs`). ПРЕДЛОЖЕНИЕ: отличия
//! описаны в `docs/Ядро-для-интерфейса.md` (ostov-orchestrator).

mod engine;
mod error;
mod types;

pub use engine::{Connector, Engine, NewAdmin, StubConnector, StubEngine};
pub use error::{EngineError, EngineResult, ErrorCode};
pub use types::*;

/// Версия протокола: сверяется при подключении (ИТ-06).
pub const PROTOCOL_VERSION: u32 = 1;
