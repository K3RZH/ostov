//! Точка запуска приложения Ostov: `ostov-ui [--config <папка>] [--base <файл>] [--preview]`.
// В выпускной сборке на Windows окно открывается без консоли; в отладочной консоль остаётся для логов.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    ostov_ui::launch::prefer_x11();
    let launch = match ostov_ui::launch::parse(std::env::args_os().skip(1)) {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("{error}\n{}", ostov_ui::launch::USAGE);
            return ExitCode::FAILURE;
        }
    };
    match ostov_ui::app::run_with(launch) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
