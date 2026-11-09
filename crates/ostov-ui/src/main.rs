//! Точка запуска приложения Ostov.

/// Вкладки как в браузере (окно идёт за указателем, перенос между окнами) нужны общие
/// координаты экрана, а чистый Wayland их приложению не отдаёт. Поэтому на Linux приложение
/// запускается через X11 (XWayland): процесс один раз заменяется копией без `WAYLAND_DISPLAY`.
/// Оставить Wayland: `OSTOV_WAYLAND=1`.
#[cfg(target_os = "linux")]
fn prefer_x11() {
    use std::os::unix::process::CommandExt;

    let wants_wayland = std::env::var_os("OSTOV_WAYLAND").is_some();
    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wants_wayland || !on_wayland || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // `exec` заменяет процесс и возвращается только при ошибке: тогда остаёмся на Wayland.
    let _ = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env_remove("WAYLAND_DISPLAY")
        .exec();
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(target_os = "linux")]
    prefer_x11();
    ostov_ui::app::run()
}
