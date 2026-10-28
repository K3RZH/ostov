//! Логика переходов между окнами первого запуска. Без ядра: только выбор нужного окна входа.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};

use crate::shell::Shell;
use crate::{LoginWindow, MainWindow, NavRow, StartWindow, TabItem};

/// Какое окно входа нужно после выбора режима и базы.
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

/// Выбирает окно входа по режиму и защите базы (0 нет, 1 пользователи, 2 пароль на файл).
pub fn login_kind(server: bool, protection: i32) -> LoginKind {
    if server {
        return LoginKind::Server;
    }
    match protection {
        1 => LoginKind::LocalUser,
        2 => LoginKind::LocalPassword,
        _ => LoginKind::None,
    }
}

/// Настраивает окно входа под выбранный вид.
pub fn configure_login(
    window: &LoginWindow,
    kind: LoginKind,
    source_name: &str,
    source_path: &str,
) {
    match kind {
        LoginKind::Server => {}
        LoginKind::LocalUser => {
            window.set_heading("Вход в базу".into());
            window.set_is_server(false);
            window.set_source_name(source_name.into());
            window.set_source_detail(source_path.into());
            window.set_change_text("Сменить базу".into());
            window.set_remember_text("Запомнить пользователя".into());
        }
        LoginKind::LocalPassword => {
            window.set_heading("Пароль базы".into());
            window.set_is_server(false);
            window.set_source_name(source_name.into());
            window.set_source_detail(source_path.into());
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
pub fn refresh(window: &MainWindow, shell: &Shell) {
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

/// Открывает главное окно для выбранной базы и закрывает окно, из которого пришли.
fn open_main(main: &MainWindow, base: &str, server: bool) -> Shell {
    let shell = Shell::demo();
    main.set_base_name(base.trim_end_matches(".sqlite").into());
    main.set_database_name(base.into());
    main.set_mode_text(
        if server {
            "Режим: сервер"
        } else {
            "Режим: локально"
        }
        .into(),
    );
    refresh(main, &shell);
    shell
}

/// Связывает окна: выбор режима → вход → назад. Ядро подключается позже через Engine API.
pub fn run() -> Result<(), slint::PlatformError> {
    use slint::{ComponentHandle, Model};

    // Идентификатор приложения: по нему оболочка связывает окно с ярлыком ostov.desktop.
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id("ostov")?;

    let start = StartWindow::new()?;
    let login = LoginWindow::new()?;
    let main = MainWindow::new()?;
    // Что выбрано на старте: имя базы и режим «сервер».
    let chosen = Rc::new(RefCell::new((String::new(), false)));
    let shell = Rc::new(RefCell::new(Shell::demo()));

    // Закрытие любого окна пользователем завершает приложение.
    start.window().on_close_requested(quit_on_close);
    login.window().on_close_requested(quit_on_close);
    main.window().on_close_requested(quit_on_close);

    start.on_open({
        let (start, login, main) = (start.as_weak(), login.as_weak(), main.as_weak());
        let (chosen, shell) = (chosen.clone(), shell.clone());
        move || {
            let (Some(start), Some(login), Some(main)) =
                (start.upgrade(), login.upgrade(), main.upgrade())
            else {
                return;
            };
            let recent = start.get_recent();
            let server = start.get_mode() == 1;
            let picked = index(start.get_selected_recent()).and_then(|i| recent.row_data(i));
            let (name, path, protection) = picked
                .map(|db| (db.name.to_string(), db.path.to_string(), db.protection))
                .unwrap_or_default();
            *chosen.borrow_mut() = (name.clone(), server);
            let kind = login_kind(server, protection);
            if kind == LoginKind::None {
                *shell.borrow_mut() = open_main(&main, &name, server);
                if main.show().is_ok() {
                    let _ = start.hide();
                }
                return;
            }
            configure_login(&login, kind, &name, &path);
            if login.show().is_ok() {
                let _ = start.hide();
            }
        }
    });

    login.on_submit({
        let (login, main) = (login.as_weak(), main.as_weak());
        let (chosen, shell) = (chosen.clone(), shell.clone());
        move || {
            let (Some(login), Some(main)) = (login.upgrade(), main.upgrade()) else {
                return;
            };
            // Проверку пароля сделает ядро (Engine API); пока пускаем без неё.
            let (name, server) = chosen.borrow().clone();
            *shell.borrow_mut() = open_main(&main, &name, server);
            if main.show().is_ok() {
                let _ = login.hide();
            }
        }
    });

    login.on_back({
        let (start, login) = (start.as_weak(), login.as_weak());
        move || {
            let (Some(start), Some(login)) = (start.upgrade(), login.upgrade()) else {
                return;
            };
            if start.show().is_ok() {
                let _ = login.hide();
            }
        }
    });

    main.on_nav_clicked({
        let (main, shell) = (main.as_weak(), shell.clone());
        move |id| {
            let Some(main) = main.upgrade() else { return };
            if let Some(id) = index(id) {
                shell.borrow_mut().click(id);
            }
            refresh(&main, &shell.borrow());
        }
    });

    main.on_tab_selected({
        let (main, shell) = (main.as_weak(), shell.clone());
        move |tab| {
            let Some(main) = main.upgrade() else { return };
            if let Some(tab) = index(tab) {
                shell.borrow_mut().select_tab(tab);
            }
            refresh(&main, &shell.borrow());
        }
    });

    main.on_tab_closed({
        let (main, shell) = (main.as_weak(), shell);
        move |tab| {
            let Some(main) = main.upgrade() else { return };
            if let Some(tab) = index(tab) {
                shell.borrow_mut().close_tab(tab);
            }
            refresh(&main, &shell.borrow());
        }
    });

    start.show()?;
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
