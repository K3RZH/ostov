//! Точка запуска приложения Ostov: `ostov-ui [--config <папка>] [--base <файл>] [--preview]`.
// В выпускной сборке на Windows окно открывается без консоли; в отладочной консоль остаётся для логов.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    ostov_ui::app::run_client(std::env::args_os().skip(1))
}
