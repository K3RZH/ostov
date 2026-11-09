//! Приложение на Slint: ErpGrid, формы, темы.
//!
//! Дизайн: docs/Дизайн-документ.md §11. Требования: ИТ-01…04, ИТ-07.
//! Статус: тема (токены), кнопка и витрина компонентов.

// Код генерирует Slint: unwrap и expect в нём не наши, поэтому lint на внешних данных
// отключён только для этого модуля, остальной код проверяется как обычно.
mod generated {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    slint::include_modules!();
}
pub use generated::*;

pub mod admin;
pub mod admin_ui;
pub mod app;
pub mod date_input;
pub mod engine;
pub mod grid;
pub mod local_base;
pub mod object_list;
pub mod receipts;
pub mod receipts_ui;
pub mod recent;
pub mod remote_base;
pub mod settings;
pub mod shell;
pub mod sorting;
pub mod users;
pub mod values;
pub mod windows;
pub mod workspace;

/// Имя крейта (проверка, что каркас собран).
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_matches_package() {
        assert_eq!(super::NAME, "ostov-ui");
    }
}
