//! Снимок окна в PPM для проверки вида без экрана:
//! `SLINT_BACKEND=winit-software cargo run -p ostov-ui --example snapshot -- светлая|тёмная файл.ppm [экран]`.
//! Экраны: showcase, main, settings, start, start-empty, create-local, create-server, add-file, add-server,
//! login-server, login-first-run, login-local, password, login-error.

use std::io::Write;

use ostov_ui::app::{LoginKind, configure_login};
use slint::{ComponentHandle, ModelRc, VecModel};

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

/// Окно приложения на нужной странице: размер как в приложении (главная больше остальных).
fn app_window(page: i32) -> Result<ostov_ui::AppWindow, Box<dyn std::error::Error>> {
    let window = ostov_ui::AppWindow::new()?;
    let (width, height) = if page == 2 {
        (1440.0, 900.0)
    } else {
        (1200.0, 760.0)
    };
    window
        .window()
        .set_size(slint::LogicalSize::new(width, height));
    window.set_page(page);
    Ok(window)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dark = args.next().is_some_and(|theme| theme == "тёмная");
    let path = args.next().unwrap_or_else(|| "snapshot.ppm".to_owned());
    let screen = args.next().unwrap_or_else(|| "showcase".to_owned());

    match screen.as_str() {
        "main" => {
            let window = app_window(2)?;
            let mut shell = ostov_ui::shell::Shell::demo();
            shell.click(3);
            shell.click(15);
            ostov_ui::app::refresh(&window, &shell);
            window.set_base_name("Кафе-2026".into());
            save(&window, dark, &path)
        }
        "users" | "user-dialog" => {
            let window = app_window(2)?;
            let mut shell = ostov_ui::shell::Shell::demo();
            shell.click(16);
            shell.click(17);
            ostov_ui::app::refresh(&window, &shell);
            ostov_ui::app::refresh_users(&window, &ostov_ui::users::Directory::demo());
            window.set_roles(ModelRc::new(VecModel::from(
                ostov_ui::users::ROLES
                    .iter()
                    .map(|role| (*role).into())
                    .collect::<Vec<slint::SharedString>>(),
            )));
            window.set_adding_user(screen == "user-dialog");
            window.set_nu_role(1);
            save(&window, dark, &path)
        }
        "receipts" | "receipts-large" | "receipt-form" | "tabs-many" => {
            use std::cell::RefCell;
            use std::rc::Rc;

            use slint::ComponentHandle;
            let window = app_window(2)?;
            let shell = Rc::new(RefCell::new(ostov_ui::shell::Shell::demo()));
            let receipts = ostov_ui::receipts_ui::setup(&window, shell.clone());
            {
                let mut shell = shell.borrow_mut();
                shell.click(2);
                shell.click(3);
            }
            if screen == "receipts-large" {
                window
                    .global::<ostov_ui::Theme>()
                    .set_font_scale(16.0 / 14.0);
                window.global::<ostov_ui::Theme>().set_row_height(40.0);
            }
            if screen == "receipt-form" {
                window.global::<ostov_ui::ReceiptsData>().invoke_open(2);
            }
            if screen == "tabs-many" {
                // Много закреплённых вкладок: панель должна сжимать и прокручивать.
                let mut shell = shell.borrow_mut();
                for id in [4, 5, 7, 8, 10, 11, 12, 13] {
                    receipts.before(&window, &mut shell);
                    shell.click(id);
                    let active = shell.active_tab();
                    shell.set_modified(active, true);
                    receipts.after(&window, &shell);
                }
            }
            ostov_ui::app::refresh(&window, &shell.borrow());
            window.set_base_name("Кафе-2026".into());
            save(&window, dark, &path)
        }
        "settings" => {
            let window = app_window(2)?;
            window.set_settings_open(true);
            window.set_dark_theme(dark);
            window.set_radius_mode(1);
            save(&window, dark, &path)
        }
        "add-file" | "add-server" => {
            let window = app_window(0)?;
            window.set_adding(true);
            window.set_add_server(screen == "add-server");
            save(&window, dark, &path)
        }
        "start" => save(&app_window(0)?, dark, &path),
        "start-empty" => {
            let window = app_window(0)?;
            window.set_recent(ModelRc::new(VecModel::default()));
            save(&window, dark, &path)
        }
        "create-password" => {
            let window = app_window(0)?;
            window.set_creating(true);
            window.set_new_name("Кафе-2026".into());
            window.set_new_protect_password(true);
            window.set_new_password("secret".into());
            window.set_new_password_repeat("secre".into());
            window.set_new_protect_users(true);
            window.set_new_admin_login("admin".into());
            save(&window, dark, &path)
        }
        "create-local" | "create-server" => {
            let window = app_window(0)?;
            window.set_creating(true);
            window.set_create_server(screen == "create-server");
            window.set_new_name("Кафе-2026".into());
            save(&window, dark, &path)
        }
        "login-server" => {
            let window = app_window(1)?;
            configure_login(
                &window,
                LoginKind::Server,
                "Кафе — общая база",
                "kafe-srv.local:7443",
            );
            save(&window, dark, &path)
        }
        "login-error" => {
            let window = app_window(1)?;
            configure_login(
                &window,
                LoginKind::Server,
                "Кафе — общая база",
                "kafe-srv.local:7443",
            );
            window.set_error("Неверный логин или пароль. Повторить можно через 4 секунды.".into());
            window.set_button_text("Войти (через 4 с)".into());
            window.set_locked(true);
            save(&window, dark, &path)
        }
        "login-first-run" => {
            let window = app_window(1)?;
            configure_login(
                &window,
                LoginKind::ServerFirstRun,
                "Новый сервер",
                "new-srv.local:7443",
            );
            save(&window, dark, &path)
        }
        "login-local" => {
            let window = app_window(1)?;
            configure_login(
                &window,
                LoginKind::LocalUser,
                "Кафе-2026.sqlite",
                "~/Ostov/Кафе-2026.sqlite",
            );
            save(&window, dark, &path)
        }
        "password" => {
            let window = app_window(1)?;
            configure_login(
                &window,
                LoginKind::LocalPassword,
                "Бар-2025.sqlite",
                "~/Документы/Бар-2025.sqlite",
            );
            save(&window, dark, &path)
        }
        _ => save(&ostov_ui::ShowcaseWindow::new()?, dark, &path),
    }
}
