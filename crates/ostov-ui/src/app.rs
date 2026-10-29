//! Логика переходов между страницами приложения. Без ядра: только выбор нужной страницы входа.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::settings::Settings;
use crate::shell::Shell;
use crate::{AppWindow, NavRow, RecentDb, Shape, TabItem, Theme};

/// Страницы единственного окна (совпадают со свойством `page` в app-window.slint).
const PAGE_START: i32 = 0;
const PAGE_LOGIN: i32 = 1;
const PAGE_MAIN: i32 = 2;

/// Защита локальной базы в `RecentDb.protection`: пароль на файл и пользователи независимы.
const PROTECTION_USERS: i32 = 1;
const PROTECTION_PASSWORD: i32 = 2;
/// Пароль на файл и пользователи вместе: сначала пароль файла, потом вход пользователя.
const PROTECTION_BOTH: i32 = 3;

/// Какая страница входа нужна после выбора базы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginKind {
    /// Сервер: логин и пароль.
    Server,
    /// Локальная база с пользователями: логин и пароль.
    LocalUser,
    /// Локальная база, зашифрованная паролем файла.
    LocalPassword,
    /// Локальная база без защиты: сразу открываем.
    None,
}

/// Выбирает страницу входа по типу и защите базы (0 нет, 1 пользователи, 2 пароль на файл).
pub fn login_kind(server: bool, protection: i32) -> LoginKind {
    if server {
        return LoginKind::Server;
    }
    match protection {
        PROTECTION_USERS => LoginKind::LocalUser,
        PROTECTION_PASSWORD | PROTECTION_BOTH => LoginKind::LocalPassword,
        _ => LoginKind::None,
    }
}

/// Настраивает страницу входа под выбранный вид. Задаёт всё заново: после прошлого входа
/// в свойствах остались бы тексты другого вида.
pub fn configure_login(
    window: &AppWindow,
    kind: LoginKind,
    source_name: &str,
    source_detail: &str,
) {
    window.set_source_name(source_name.into());
    window.set_source_detail(source_detail.into());
    window.set_error("".into());
    window.set_login("".into());
    window.set_password("".into());
    match kind {
        LoginKind::Server => {
            window.set_heading("Вход на сервер".into());
            window.set_is_server(true);
            window.set_change_text("Сменить".into());
            window.set_show_login(true);
            window.set_hint("".into());
            window.set_remember_text("Запомнить сервер".into());
            window.set_button_text("Войти".into());
        }
        LoginKind::LocalUser => {
            window.set_heading("Вход в базу".into());
            window.set_is_server(false);
            window.set_change_text("Сменить базу".into());
            window.set_show_login(true);
            window.set_hint("".into());
            window.set_remember_text("Запомнить пользователя".into());
            window.set_button_text("Войти".into());
        }
        LoginKind::LocalPassword => {
            window.set_heading("Пароль базы".into());
            window.set_is_server(false);
            window.set_change_text("Сменить базу".into());
            window.set_show_login(false);
            window.set_hint("База зашифрована. Без пароля открыть её нельзя.".into());
            window.set_remember_text("".into());
            window.set_button_text("Открыть".into());
        }
        LoginKind::None => {}
    }
}

fn index(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

/// Переносит состояние оболочки в окно.
pub fn refresh(window: &AppWindow, shell: &Shell) {
    let rows: Vec<NavRow> = shell
        .nav_rows()
        .into_iter()
        .map(|row| NavRow {
            id: i32::try_from(row.id).unwrap_or_default(),
            title: row.title.into(),
            icon: row.icon.into(),
            level: i32::try_from(row.level).unwrap_or_default(),
            is_group: row.is_group,
            expanded: row.expanded,
            selected: row.selected,
            favorite: row.favorite,
        })
        .collect();
    let tabs: Vec<TabItem> = shell
        .tabs()
        .iter()
        .map(|tab| TabItem {
            title: tab.title.into(),
            icon: tab.icon.into(),
            modified: false,
        })
        .collect();
    window.set_nav(ModelRc::new(VecModel::from(rows)));
    window.set_tabs(ModelRc::new(VecModel::from(tabs)));
    window.set_active_tab(i32::try_from(shell.active_tab()).unwrap_or_default());
}

/// Показывает страницу и подгоняет размер окна: главная больше, чем выбор базы и вход.
/// Окно одно, поэтому ничего не прячем и не показываем заново.
fn show_page(window: &AppWindow, page: i32) {
    let (width, height) = if page == PAGE_MAIN {
        (1440.0, 900.0)
    } else {
        (1200.0, 760.0)
    };
    window.set_page(page);
    window
        .window()
        .set_size(slint::LogicalSize::new(width, height));
}

/// Готовит главную страницу для выбранной базы.
fn open_main(window: &AppWindow, base: &str, server: bool) -> Shell {
    let shell = Shell::demo();
    window.set_base_name(base.trim_end_matches(".sqlite").into());
    window.set_database_name(base.into());
    window.set_mode_text(
        if server {
            "Режим: сервер"
        } else {
            "Режим: локально"
        }
        .into(),
    );
    refresh(window, &shell);
    shell
}

/// Строка списка для только что созданной базы. Сервер: путь хранит адрес, защита не нужна.
/// Локальная: файл `<имя>.sqlite` в папке, защита по флажкам (можно и пароль, и пользователей).
pub fn new_database(
    server: bool,
    name: &str,
    folder: &str,
    address: &str,
    protect_password: bool,
    protect_users: bool,
) -> RecentDb {
    if server {
        return RecentDb {
            name: name.into(),
            path: address.into(),
            when: "только что".into(),
            server: true,
            protection: 0,
        };
    }
    let file = format!("{name}.sqlite");
    let protection = match (protect_password, protect_users) {
        (true, true) => PROTECTION_BOTH,
        (true, false) => PROTECTION_PASSWORD,
        (false, true) => PROTECTION_USERS,
        (false, false) => 0,
    };
    RecentDb {
        path: format!("{}/{file}", folder.trim_end_matches('/')).into(),
        name: file.into(),
        when: "только что".into(),
        server: false,
        protection,
    }
}

/// Заменяет домашнюю папку на `~`: так путь короче и привычнее.
fn short_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

/// Строка списка для добавленной существующей базы. Защиту файла заранее не знаем (ядра нет),
/// поэтому считаем, что её нет.
pub fn existing_database(server: bool, file: &str, address: &str, name: &str) -> RecentDb {
    if server {
        return RecentDb {
            name: name.into(),
            path: address.into(),
            when: "только что".into(),
            server: true,
            protection: 0,
        };
    }
    RecentDb {
        name: file.rsplit('/').next().unwrap_or(file).into(),
        path: file.into(),
        when: "только что".into(),
        server: false,
        protection: 0,
    }
}

/// Применяет настройки интерфейса: тема и скругление глобальные, окно тоже запоминает выбор.
fn apply_settings(window: &AppWindow, settings: Settings) {
    window.global::<Theme>().set_dark(settings.dark);
    window
        .global::<Shape>()
        .set_control_radius(settings.radius_px());
    window.set_dark_theme(settings.dark);
    window.set_radius_mode(settings.radius);
}

/// Запускает приложение: выбор базы → вход → главная. Ядро подключается позже через Engine API.
pub fn run() -> Result<(), slint::PlatformError> {
    use slint::Model;

    // Идентификатор приложения: по нему оболочка связывает окно с ярлыком ostov.desktop.
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id("ostov")?;

    let app = AppWindow::new()?;
    apply_settings(&app, Settings::load());
    // Что выбрано на старте: имя базы и признак «сервер».
    let chosen = Rc::new(RefCell::new((String::new(), false)));
    let shell = Rc::new(RefCell::new(Shell::demo()));

    // Закрытие окна пользователем завершает приложение.
    app.window().on_close_requested(quit_on_close);

    app.on_open({
        let app = app.as_weak();
        let (chosen, shell) = (chosen.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let recent = app.get_recent();
            let Some(db) = index(app.get_selected_recent()).and_then(|i| recent.row_data(i)) else {
                return;
            };
            *chosen.borrow_mut() = (db.name.to_string(), db.server);
            let kind = login_kind(db.server, db.protection);
            if kind == LoginKind::None {
                *shell.borrow_mut() = open_main(&app, &db.name, db.server);
                show_page(&app, PAGE_MAIN);
                return;
            }
            configure_login(&app, kind, &db.name, &db.path);
            show_page(&app, PAGE_LOGIN);
        }
    });

    app.on_submit({
        let app = app.as_weak();
        let (chosen, shell) = (chosen.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            // Проверку пароля сделает ядро (Engine API); пока пускаем без неё.
            let (name, server) = chosen.borrow().clone();
            *shell.borrow_mut() = open_main(&app, &name, server);
            app.set_password("".into());
            show_page(&app, PAGE_MAIN);
        }
    });

    app.on_back({
        let app = app.as_weak();
        move || {
            let Some(app) = app.upgrade() else { return };
            app.set_password("".into());
            show_page(&app, PAGE_START);
        }
    });

    app.on_create_confirmed({
        let app = app.as_weak();
        move || {
            let Some(app) = app.upgrade() else { return };
            let db = new_database(
                app.get_create_server(),
                app.get_new_name().trim(),
                app.get_new_folder().as_str(),
                app.get_new_address().trim(),
                app.get_new_protect_password(),
                app.get_new_protect_users(),
            );
            // Ядро ещё не создаёт файл базы: добавляем строку в список, чтобы пройти весь путь.
            let mut rows = vec![db];
            rows.extend(app.get_recent().iter());
            app.set_recent(ModelRc::new(VecModel::from(rows)));
            app.set_selected_recent(0);
            app.set_new_name("".into());
            app.set_new_address("".into());
            app.set_new_admin_login("".into());
            app.set_new_admin_password("".into());
            app.set_new_admin_repeat("".into());
            // Пароль базы здесь не храним: файл создаст ядро, ему и передадим.
            app.set_new_password("".into());
            app.set_new_password_repeat("".into());
            app.set_new_protect_password(false);
            app.set_new_protect_users(false);
            app.set_creating(false);
        }
    });

    app.on_choose_folder({
        let app = app.as_weak();
        move || {
            let app = app.clone();
            // Системное окно выбора папки: ждём ответ, не блокируя интерфейс.
            let _ = slint::spawn_local(async move {
                let Some(folder) = rfd::AsyncFileDialog::new()
                    .set_title("Папка для базы")
                    .pick_folder()
                    .await
                else {
                    return;
                };
                if let Some(app) = app.upgrade() {
                    app.set_new_folder(short_home(&folder.path().to_string_lossy()).into());
                }
            });
        }
    });

    app.on_theme_chosen({
        let app = app.as_weak();
        move |dark| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                dark,
                radius: app.get_radius_mode(),
            };
            apply_settings(&app, settings);
            settings.save();
        }
    });

    app.on_radius_chosen({
        let app = app.as_weak();
        move |radius| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                dark: app.get_dark_theme(),
                radius,
            };
            apply_settings(&app, settings);
            settings.save();
        }
    });

    app.on_choose_file({
        let app = app.as_weak();
        move || {
            let app = app.clone();
            let _ = slint::spawn_local(async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .set_title("Файл базы")
                    .add_filter("База данных", &["sqlite", "sqlite3", "db"])
                    .pick_file()
                    .await
                else {
                    return;
                };
                if let Some(app) = app.upgrade() {
                    app.set_add_file(short_home(&file.path().to_string_lossy()).into());
                }
            });
        }
    });

    app.on_add_confirmed({
        let app = app.as_weak();
        move || {
            let Some(app) = app.upgrade() else { return };
            let db = existing_database(
                app.get_add_server(),
                app.get_add_file().trim(),
                app.get_add_address().trim(),
                app.get_add_name().trim(),
            );
            let mut rows = vec![db];
            rows.extend(app.get_recent().iter());
            app.set_recent(ModelRc::new(VecModel::from(rows)));
            app.set_selected_recent(0);
            app.set_add_file("".into());
            app.set_add_address("".into());
            app.set_add_name("".into());
            app.set_adding(false);
        }
    });

    app.on_nav_clicked({
        let (app, shell) = (app.as_weak(), shell.clone());
        move |id| {
            let Some(app) = app.upgrade() else { return };
            if let Some(id) = index(id) {
                shell.borrow_mut().click(id);
            }
            refresh(&app, &shell.borrow());
        }
    });

    app.on_tab_selected({
        let (app, shell) = (app.as_weak(), shell.clone());
        move |tab| {
            let Some(app) = app.upgrade() else { return };
            if let Some(tab) = index(tab) {
                shell.borrow_mut().select_tab(tab);
            }
            refresh(&app, &shell.borrow());
        }
    });

    app.on_tab_closed({
        let (app, shell) = (app.as_weak(), shell);
        move |tab| {
            let Some(app) = app.upgrade() else { return };
            if let Some(tab) = index(tab) {
                shell.borrow_mut().close_tab(tab);
            }
            refresh(&app, &shell.borrow());
        }
    });

    app.show()?;
    slint::run_event_loop_until_quit()
}

fn quit_on_close() -> slint::CloseRequestResponse {
    let _ = slint::quit_event_loop();
    slint::CloseRequestResponse::HideWindow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_always_asks_login() {
        assert_eq!(login_kind(true, 0), LoginKind::Server);
        assert_eq!(login_kind(true, 2), LoginKind::Server);
    }

    #[test]
    fn local_depends_on_protection() {
        assert_eq!(login_kind(false, 1), LoginKind::LocalUser);
        assert_eq!(login_kind(false, 2), LoginKind::LocalPassword);
        assert_eq!(login_kind(false, 0), LoginKind::None);
    }
}
