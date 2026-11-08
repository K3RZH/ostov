//! Импорт и экспорт Excel (xlsx) и XML (СТ-40, СТ-41, СТ-42). Всё идёт через `ostov_api::Engine`,
//! как у интерфейса: права, журнал и проверки применяет ядро, а код один для локальной базы
//! и для сервера.

mod error;
mod export;
mod import;
mod limits;
mod table;
mod values;
mod xlsx;
mod xml;

pub use error::IoError;
pub use export::{export_list, export_report};
pub use import::{
    Mapping, Plan, Planned, RowError, Target, WriteFailure, Written, plan, suggest, write,
};
pub use limits::Limits;
pub use table::{Cell, Table};
pub use xlsx::{read_xlsx, read_xlsx_sheet};
pub use xml::{export_list_xml, read_xml};
