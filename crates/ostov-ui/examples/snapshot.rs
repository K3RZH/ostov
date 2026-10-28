//! Снимок окна в PPM для проверки вида без экрана:
//! `SLINT_BACKEND=winit-software cargo run -p ostov-ui --example snapshot -- светлая|тёмная файл.ppm [экран]`.
//! Экраны: showcase, main, start, login-server, login-local, password, login-error.

use std::io::Write;

use slint::ComponentHandle;

fn save<T>(window: &T, dark: bool, path: &str) -> Result<(), Box<dyn std::error::Error>>
where
    T: ComponentHandle,
    for<'a> ostov_ui::Theme<'a>: slint::Global<'a, T>,
{
    window.global::<ostov_ui::Theme>().set_dark(dark);
    window.show()?;
    let snapshot = window.window().take_snapshot()?;
    let mut file = std::fs::File::create(path)?;
    write!(
        file,
        "P6\n{} {}\n255\n",
        snapshot.width(),
        snapshot.height()
    )?;
    for pixel in snapshot.as_slice() {
        file.write_all(&[pixel.r, pixel.g, pixel.b])?;
    }
    println!("{path}: {}x{}", snapshot.width(), snapshot.height());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dark = args.next().is_some_and(|theme| theme == "тёмная");
    let path = args.next().unwrap_or_else(|| "snapshot.ppm".to_owned());
    let screen = args.next().unwrap_or_else(|| "showcase".to_owned());

    match screen.as_str() {
        "main" => {
            let window = ostov_ui::MainWindow::new()?;
            let mut shell = ostov_ui::shell::Shell::demo();
            shell.click(3);
            shell.click(15);
            ostov_ui::app::refresh(&window, &shell);
            save(&window, dark, &path)
        }
        "start" => save(&ostov_ui::StartWindow::new()?, dark, &path),
        "login-server" => save(&ostov_ui::LoginWindow::new()?, dark, &path),
        "login-error" => {
            let window = ostov_ui::LoginWindow::new()?;
            window.set_error("Неверный логин или пароль. Повторить можно через 4 секунды.".into());
            window.set_button_text("Войти (через 4 с)".into());
            save(&window, dark, &path)
        }
        "login-local" => {
            let window = ostov_ui::LoginWindow::new()?;
            window.set_heading("Вход в базу".into());
            window.set_is_server(false);
            window.set_source_name("Кафе-2026.sqlite".into());
            window.set_source_detail("~/Ostov/Кафе-2026.sqlite".into());
            window.set_change_text("Сменить базу".into());
            window.set_remember_text("Запомнить пользователя".into());
            save(&window, dark, &path)
        }
        "password" => {
            let window = ostov_ui::LoginWindow::new()?;
            window.set_heading("Пароль базы".into());
            window.set_is_server(false);
            window.set_source_name("Бар-2025.sqlite".into());
            window.set_source_detail("~/Документы/Бар-2025.sqlite".into());
            window.set_change_text("Сменить базу".into());
            window.set_show_login(false);
            window.set_hint("База зашифрована. Без пароля открыть её нельзя.".into());
            window.set_remember_text("".into());
            window.set_button_text("Открыть".into());
            save(&window, dark, &path)
        }
        _ => save(&ostov_ui::ShowcaseWindow::new()?, dark, &path),
    }
}
