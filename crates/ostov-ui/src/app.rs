//! Логика переходов между страницами приложения. Без ядра: только выбор нужной страницы входа.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::login::{delay_secs, verify};
use crate::settings::Settings;
use crate::shell::Shell;
use crate::users::{Directory, ROLES};
use crate::{AppWindow, NavRow, RecentDb, Shape, TabItem, Theme, UserRow};

/// Страницы единственного окна (совпадают со свойством `page` в app-window.slint).
const PAGE_START: i32 = 0;
const PAGE_LOGIN: i32 = 1;
const PAGE_MAIN: i32 = 2;

/// Защита локальной базы в `RecentDb.protection`: пароль на файл и пользователи независимы.
const PROTECTION_USERS: i32 = 1;
const PROTECTION_PASSWORD: i32 = 2;
/// Пароль на файл и пользователи вместе: сначала пароль файла, потом вход пользователя.
const PROTECTION_BOTH: i32 = 3;
/// Сервер без администратора: первый вход создаёт его (СТ-36).
const PROTECTION_FIRST_RUN: i32 = 4;

/// Какая страница входа нужна после выбора базы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginKind {
    /// Сервер: логин и пароль.
    Server,
    /// Первый запуск сервера: создание администратора.
    ServerFirstRun,
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
        return if protection == PROTECTION_FIRST_RUN {
            LoginKind::ServerFirstRun
        } else {
            LoginKind::Server
        };
    }
    match protection {
        PROTECTION_USERS => LoginKind::LocalUser,
        PROTECTION_PASSWORD | PROTECTION_BOTH => LoginKind::LocalPassword,
        _ => LoginKind::None,
    }
}

/// Текст главной кнопки входа для вида страницы.
fn button_text(kind: LoginKind) -> &'static str {
    match kind {
        LoginKind::LocalPassword => "Открыть",
        LoginKind::ServerFirstRun => "Создать администратора",
        _ => "Войти",
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
    window.set_locked(false);
    window.set_login("".into());
    window.set_password("".into());
    window.set_repeat("".into());
    window.set_show_repeat(kind == LoginKind::ServerFirstRun);
    window.set_login_label("Логин".into());
    window.set_password_label("Пароль".into());
    window.set_button_text(button_text(kind).into());
    match kind {
        LoginKind::Server => {
            window.set_heading("Вход на сервер".into());
            window.set_is_server(true);
            window.set_change_text("Сменить".into());
            window.set_show_login(true);
            window.set_hint("".into());
            window.set_remember_text("Запомнить сервер".into());
        }
        LoginKind::ServerFirstRun => {
            window.set_heading("Первая настройка сервера".into());
            window.set_is_server(true);
            window.set_change_text("Сменить".into());
            window.set_show_login(true);
            window.set_login_label("Логин администратора".into());
            window.set_password_label("Пароль администратора".into());
            window.set_hint(
                "Сервер ещё не настроен. Создайте администратора: паролей по умолчанию нет.".into(),
            );
            window.set_remember_text("".into());
        }
        LoginKind::LocalUser => {
            window.set_heading("Вход в базу".into());
            window.set_is_server(false);
            window.set_change_text("Сменить базу".into());
            window.set_show_login(true);
            window.set_hint("".into());
            window.set_remember_text("Запомнить пользователя".into());
        }
        LoginKind::LocalPassword => {
            window.set_heading("Пароль базы".into());
            window.set_is_server(false);
            window.set_change_text("Сменить базу".into());
            window.set_show_login(false);
            window.set_hint("База зашифрована. Без пароля открыть её нельзя.".into());
            window.set_remember_text("".into());
        }
        LoginKind::None => {}
    }
}

/// Показывает отказ и блокирует кнопку на время задержки, считая секунды таймером.
fn start_lock(
    window: &AppWindow,
    timer: &slint::Timer,
    kind: LoginKind,
    message: &'static str,
    seconds: u32,
) {
    let remaining = Rc::new(Cell::new(seconds));
    show_lock(window, kind, message, seconds);
    let weak = window.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            let Some(window) = weak.upgrade() else { return };
            let left = remaining.get().saturating_sub(1);
            remaining.set(left);
            if left == 0 {
                window.set_locked(false);
                window.set_error("".into());
                window.set_button_text(button_text(kind).into());
            } else {
                show_lock(&window, kind, message, left);
            }
        },
    );
}

fn show_lock(window: &AppWindow, kind: LoginKind, message: &str, seconds: u32) {
    window.set_locked(true);
    window.set_error(format!("{message} Повторить можно через {seconds} с.").into());
    window.set_button_text(format!("{} (через {seconds} с)", button_text(kind)).into());
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
            title: tab.title.as_str().into(),
            icon: tab.icon.into(),
            modified: tab.modified,
            view: tab.view.into(),
            preview: tab.preview,
        })
        .collect();
    window.set_nav(ModelRc::new(VecModel::from(rows)));
    window.set_tabs(ModelRc::new(VecModel::from(tabs)));
    window.set_active_tab(i32::try_from(shell.active_tab()).unwrap_or_default());
}

/// Переносит список пользователей в окно.
pub fn refresh_users(window: &AppWindow, directory: &Directory) {
    let rows: Vec<UserRow> = directory
        .users()
        .iter()
        .map(|user| UserRow {
            login: user.login.as_str().into(),
            name: user.name.as_str().into(),
            role: user.role.as_str().into(),
            active: user.active,
        })
        .collect();
    window.set_users(ModelRc::new(VecModel::from(rows)));
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
    window
        .global::<Theme>()
        .set_font_scale(settings.font_scale());
    window.global::<Theme>().set_row_height(settings.row_px());
    window.set_dark_theme(settings.dark);
    window.set_radius_mode(settings.radius);
    window.set_density_mode(settings.density);
    window.set_font_mode(settings.font);
}

/// Настройки, которые сейчас выбраны в окне.
fn current_settings(window: &AppWindow) -> Settings {
    Settings {
        dark: window.get_dark_theme(),
        radius: window.get_radius_mode(),
        density: window.get_density_mode(),
        font: window.get_font_mode(),
    }
}

/// Что выбрано на старте и на каком шаге входа мы сейчас.
#[derive(Default)]
struct Session {
    name: String,
    server: bool,
    protection: i32,
    /// Текущий вид страницы входа (у базы «пароль и пользователи» их два подряд).
    stage: Option<LoginKind>,
    /// Неудачные попытки подряд: от них зависит задержка.
    failures: u32,
}

/// Запускает приложение: выбор базы → вход → главная. Ядро подключается позже через Engine API.
pub fn run() -> Result<(), slint::PlatformError> {
    use slint::Model;

    // Идентификатор приложения: по нему оболочка связывает окно с ярлыком ostov.desktop.
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id("ostov")?;

    let app = AppWindow::new()?;
    apply_settings(&app, Settings::load());
    let shell = Rc::new(RefCell::new(Shell::demo()));
    let receipts = crate::receipts_ui::setup(&app, shell.clone());
    let directory = Rc::new(RefCell::new(Directory::demo()));
    refresh_users(&app, &directory.borrow());
    app.set_roles(ModelRc::new(VecModel::from(
        ROLES
            .iter()
            .map(|role| (*role).into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let session = Rc::new(RefCell::new(Session::default()));
    // Таймер задержки после неудачных попыток входа (должен жить, пока идёт отсчёт).
    let lock_timer = Rc::new(slint::Timer::default());

    // Закрытие окна пользователем завершает приложение.
    app.window().on_close_requested(quit_on_close);

    app.on_open({
        let app = app.as_weak();
        let (session, shell, lock_timer) = (session.clone(), shell.clone(), lock_timer.clone());
        let receipts = receipts.clone();
        move || {
            let Some(app) = app.upgrade() else { return };
            let recent = app.get_recent();
            let Some(db) = index(app.get_selected_recent()).and_then(|i| recent.row_data(i)) else {
                return;
            };
            let kind = login_kind(db.server, db.protection);
            *session.borrow_mut() = Session {
                name: db.name.to_string(),
                server: db.server,
                protection: db.protection,
                stage: Some(kind),
                failures: 0,
            };
            lock_timer.stop();
            if kind == LoginKind::None {
                *shell.borrow_mut() = open_main(&app, &db.name, db.server);
                receipts.after(&app, &shell.borrow());
                show_page(&app, PAGE_MAIN);
                return;
            }
            configure_login(&app, kind, &db.name, &db.path);
            show_page(&app, PAGE_LOGIN);
        }
    });

    app.on_submit({
        let app = app.as_weak();
        let (session, shell, lock_timer) = (session.clone(), shell.clone(), lock_timer.clone());
        let receipts = receipts.clone();
        move || {
            let Some(app) = app.upgrade() else { return };
            let kind = session.borrow().stage.unwrap_or(LoginKind::None);
            let result = verify(
                kind,
                app.get_login().as_str(),
                app.get_password().as_str(),
                app.get_repeat().as_str(),
            );
            if let Err(rejection) = result {
                let failures = {
                    let mut session = session.borrow_mut();
                    if rejection.counts {
                        session.failures += 1;
                    }
                    session.failures
                };
                let wait = if rejection.counts {
                    delay_secs(failures)
                } else {
                    0
                };
                if wait > 0 {
                    start_lock(&app, &lock_timer, kind, rejection.message, wait);
                } else {
                    app.set_error(rejection.message.into());
                }
                return;
            }
            // Проверку сделает ядро (Engine API); пока пускаем, если данные не пустые.
            let (name, server, protection) = {
                let mut session = session.borrow_mut();
                session.failures = 0;
                (session.name.clone(), session.server, session.protection)
            };
            if kind == LoginKind::LocalPassword && protection == PROTECTION_BOTH {
                // Файл открыт: теперь вход пользователя.
                session.borrow_mut().stage = Some(LoginKind::LocalUser);
                configure_login(&app, LoginKind::LocalUser, &name, &app.get_source_detail());
                return;
            }
            *shell.borrow_mut() = open_main(&app, &name, server);
            receipts.after(&app, &shell.borrow());
            app.set_password("".into());
            app.set_repeat("".into());
            show_page(&app, PAGE_MAIN);
        }
    });

    // «Назад к выбору базы» и «Сменить»: вернуться к списку баз.
    let to_start = {
        let app = app.as_weak();
        let lock_timer = lock_timer.clone();
        move || {
            let Some(app) = app.upgrade() else { return };
            lock_timer.stop();
            app.set_password("".into());
            app.set_repeat("".into());
            app.set_error("".into());
            show_page(&app, PAGE_START);
        }
    };
    app.on_back(to_start.clone());
    app.on_change_source(to_start);

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
                ..current_settings(&app)
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
                radius,
                ..current_settings(&app)
            };
            apply_settings(&app, settings);
            settings.save();
        }
    });

    app.on_density_chosen({
        let app = app.as_weak();
        move |density| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                density,
                ..current_settings(&app)
            };
            apply_settings(&app, settings);
            settings.save();
        }
    });

    app.on_font_chosen({
        let app = app.as_weak();
        move |font| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                font,
                ..current_settings(&app)
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

    app.on_user_toggle({
        let app = app.as_weak();
        let directory = directory.clone();
        move |row| {
            let Some(app) = app.upgrade() else { return };
            let result = index(row).map_or(Ok(()), |i| directory.borrow_mut().toggle(i));
            app.set_user_message(result.err().unwrap_or_default().into());
            refresh_users(&app, &directory.borrow());
        }
    });

    app.on_user_add_confirmed({
        let app = app.as_weak();
        let directory = directory.clone();
        move || {
            let Some(app) = app.upgrade() else { return };
            let role = index(app.get_nu_role())
                .and_then(|i| ROLES.get(i).copied())
                .unwrap_or(ROLES[1]);
            let result = directory.borrow_mut().add(
                app.get_nu_login().as_str(),
                app.get_nu_name().as_str(),
                role,
            );
            match result {
                Err(message) => app.set_user_error(message.into()),
                Ok(()) => {
                    refresh_users(&app, &directory.borrow());
                    app.set_user_message("".into());
                    app.set_user_error("".into());
                    app.set_nu_login("".into());
                    app.set_nu_name("".into());
                    app.set_nu_role(1);
                    // Пароль пользователя здесь не храним: его задаст ядро (хэш Argon2id).
                    app.set_nu_password("".into());
                    app.set_nu_repeat("".into());
                    app.set_adding_user(false);
                }
            }
        }
    });

    app.on_nav_clicked({
        let (app, shell, receipts) = (app.as_weak(), shell.clone(), receipts.clone());
        move |id| {
            let Some(app) = app.upgrade() else { return };
            let mut shell = shell.borrow_mut();
            receipts.before(&app, &mut shell);
            if let Some(id) = index(id) {
                shell.click(id);
            }
            receipts.after(&app, &shell);
            refresh(&app, &shell);
        }
    });

    app.on_tab_selected({
        let (app, shell, receipts) = (app.as_weak(), shell.clone(), receipts.clone());
        move |tab| {
            let Some(app) = app.upgrade() else { return };
            let mut shell = shell.borrow_mut();
            receipts.before(&app, &mut shell);
            if let Some(tab) = index(tab) {
                shell.select_tab(tab);
            }
            receipts.after(&app, &shell);
            refresh(&app, &shell);
        }
    });

    app.on_tab_closed({
        let (app, shell, receipts) = (app.as_weak(), shell, receipts);
        move |tab| {
            let Some(app) = app.upgrade() else { return };
            let mut shell = shell.borrow_mut();
            receipts.before(&app, &mut shell);
            if let Some(tab) = index(tab) {
                shell.close_tab(tab);
            }
            receipts.after(&app, &shell);
            refresh(&app, &shell);
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
        assert_eq!(login_kind(true, 4), LoginKind::ServerFirstRun);
    }

    #[test]
    fn local_depends_on_protection() {
        assert_eq!(login_kind(false, 1), LoginKind::LocalUser);
        assert_eq!(login_kind(false, 2), LoginKind::LocalPassword);
        assert_eq!(login_kind(false, 3), LoginKind::LocalPassword);
        assert_eq!(login_kind(false, 0), LoginKind::None);
    }
}
