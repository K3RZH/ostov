//! «Ostov Конфигуратор»: `cargo run -p ostov-ui --bin configurator [папка конфигурации]`.
//! Без папки открывается та, что была открыта в прошлый раз (`~/.config/ostov/configurator.conf`).
//! «Запустить» запускает этот же файл клиентом: `configurator --preview --config <папка> --base <файл>`.
// В выпускной сборке на Windows окно открывается без консоли; в отладочной консоль остаётся для логов.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use ostov_ui::configurator_ui::{apply_settings, create_window};
use ostov_ui::settings::Settings;
use slint::ComponentHandle;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // Клиент предпросмотра из той же библиотеки, что и `ostov-ui`: второй бинарник рядом не нужен.
    if ostov_ui::launch::is_client(&args) {
        return ostov_ui::app::run_client(args);
    }
    let folder = match args.as_slice() {
        [] => ostov_ui::settings::last_config_folder(),
        // Полный путь: его запоминаем и потом передаём клиенту, текущая папка у них может быть другой.
        [dir] => match std::path::absolute(PathBuf::from(dir)) {
            Ok(dir) => {
                ostov_ui::settings::remember_config_folder(&dir);
                Some(dir)
            }
            Err(error) => {
                eprintln!("папка конфигурации: {error}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("использование: configurator [папка конфигурации]");
            return ExitCode::FAILURE;
        }
    };
    let result = create_window(folder).and_then(|window| {
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
