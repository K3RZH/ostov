//! Точка запуска приложения Ostov.

fn main() -> Result<(), slint::PlatformError> {
    ostov_ui::app::run()
}
