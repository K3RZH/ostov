//! Метаданные: чтение файлов конфигурации из папки `config/` (Дизайн §4).
//!
//! Разбор строгий: неизвестные ключи, якоря и алиасы запрещены, размер файла и вложенность
//! ограничены. Ошибки несут файл, строку и причину (СТ-02). Результат — модель `ostov-model`,
//! её приложение передаёт в ядро; ядро от этого крейта не зависит.

pub mod check;
mod convert;
mod demo;
pub mod error;
pub mod field_type;
pub mod load;
pub mod model;

pub use check::check;
pub use demo::demo;
pub use error::Error;
pub use field_type::FieldType;
pub use load::{FORMAT_VERSION, load, parse};
pub use model::{
    Catalog, ConfigFiles, Document, Field, Located, Manifest, Register, RegisterKind, Subsystem,
    TablePart,
};
