//! «Ostov Конфигуратор»: `cargo run -p ostov-ui --bin configurator`. Только просмотр.

use std::process::ExitCode;

use ostov_ui::configurator::Configuration;
use ostov_ui::configurator_ui::{apply_settings, create_window};
use ostov_ui::settings::Settings;
use slint::ComponentHandle;

fn main() -> ExitCode {
    let config = match Configuration::embedded() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let result = create_window(config).and_then(|window| {
        apply_settings(&window, Settings::load());
        ostov_ui::shortcuts::install(window.window());
        window.run()
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("не удалось открыть окно: {error}");
            ExitCode::FAILURE
        }
    }
}
