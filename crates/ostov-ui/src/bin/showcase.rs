//! Витрина компонентов: `cargo run -p ostov-ui --bin showcase`.

use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    ostov_ui::ShowcaseWindow::new()?.run()
}
