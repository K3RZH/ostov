//! Приложение на Slint: ErpGrid, формы, темы.
//!
//! Дизайн: docs/Дизайн-документ.md §11. Требования: ИТ-01…04, ИТ-07.
//! Статус: тема (токены), кнопка и витрина компонентов.

slint::include_modules!();

pub mod app;
pub mod login;
pub mod settings;
pub mod shell;

/// Имя крейта (проверка, что каркас собран).
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_matches_package() {
        assert_eq!(super::NAME, "ostov-ui");
    }
}
