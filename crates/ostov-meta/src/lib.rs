//! Метаданные: чтение файлов конфигурации из папки `config/` (Дизайн §4).
//!
//! Разбор строгий: неизвестные ключи, якоря и алиасы запрещены, размер файла и вложенность
//! ограничены. Ошибки несут файл, строку и причину (СТ-02).

pub mod error;
pub mod field_type;
pub mod load;
pub mod model;

pub use error::Error;
pub use field_type::FieldType;
pub use load::{FORMAT_VERSION, load};
pub use model::{
    Catalog, Config, Document, Field, Located, Manifest, Register, RegisterKind, TablePart,
};
