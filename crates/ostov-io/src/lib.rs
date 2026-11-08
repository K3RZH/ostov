//! Импорт и экспорт Excel (xlsx) и XML (СТ-40, СТ-41, СТ-42). Всё идёт через `ostov_api::Engine`,
//! как у интерфейса: права, журнал и проверки применяет ядро, а код один для локальной базы
//! и для сервера.

mod error;
mod export;
mod values;

pub use error::IoError;
pub use export::{export_list, export_report};
