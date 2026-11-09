//! Логика переходов между страницами приложения: выбор базы, вход, главная.
//! Локальная база и сервер открываются через ядро (`Connector`), вход проверяет ядро.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::admin_ui::AdminUi;
use crate::engine::{
    Engine, EngineResult, Meta, NewAdmin, NewUser, RoleInfo, SessionInfo, SystemRights,
    call_in_background,
};
use crate::list_ui::ListUi;
use crate::receipts_ui::ReceiptsUi;
use crate::settings::Settings;
use crate::shell::Shell;
use crate::users::Directory;
use crate::{AppWindow, NavRow, RecentDb, Shape, TabItem, Theme, UserRow};

/// Страницы единственного окна (совпадают со свойством `page` в app-window.slint).
const PAGE_START: i32 = 0;
const PAGE_LOGIN: i32 = 1;
const PAGE_MAIN: i32 = 2;

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
            window.set_remember_text("Запомнить пользователя".into());
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

fn index(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

/// Переносит состояние оболочки в окно.
/// Вкладки в виде для Slint.
pub fn tab_items(tabs: &[crate::shell::Tab]) -> Vec<TabItem> {
    tabs.iter()
        .map(|tab| TabItem {
            title: tab.title.as_str().into(),
            icon: tab.icon.into(),
            modified: tab.modified,
            view: tab.view.into(),
            object: tab.object.as_str().into(),
            preview: tab.preview,
        })
        .collect()
}

/// Обновляет вкладки окна на месте: строки модели не пересоздаются, поэтому вкладка, которую
/// сейчас тащат, не теряет захват мыши (замена всей модели пересоздаёт все вкладки).
pub fn set_tabs(window: &AppWindow, items: Vec<TabItem>) {
    let model = window.get_tabs();
    if let Some(vec) = model.as_any().downcast_ref::<VecModel<TabItem>>() {
        for (i, item) in items.iter().enumerate() {
            if i < vec.row_count() {
                if vec.row_data(i).as_ref() != Some(item) {
                    vec.set_row_data(i, item.clone());
                }
            } else {
                vec.push(item.clone());
            }
        }
        while vec.row_count() > items.len() {
            vec.remove(vec.row_count() - 1);
        }
        return;
    }
    window.set_tabs(ModelRc::new(VecModel::from(items)));
}

/// Строки дерева навигации в виде для Slint.
pub fn nav_items(rows: Vec<crate::shell::NavRowData>) -> Vec<NavRow> {
    rows.into_iter()
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
        .collect()
}

pub fn refresh(window: &AppWindow, shell: &Shell) {
    let tabs = tab_items(shell.tabs());
    window.set_nav(ModelRc::new(VecModel::from(nav_items(shell.nav_rows()))));
    set_tabs(window, tabs);
    window.set_active_tab(i32::try_from(shell.active_tab()).unwrap_or_default());
}

/// Переносит список пользователей в окно.
pub fn refresh_users(window: &AppWindow, directory: &Directory) {
    let rows: Vec<UserRow> = directory
        .users()
        .iter()
        .map(|user| UserRow {
            login: user.login.as_str().into(),
            name: user.display_name.as_str().into(),
            role: directory.role_title(&user.role).into(),
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

/// Готовит главную страницу для выбранной базы: дерево навигации из метаданных.
fn open_main(window: &AppWindow, session: &Session) -> Shell {
    let shell = session
        .meta
        .as_ref()
        .map_or_else(Shell::empty, |meta| Shell::from_meta(meta, session.system));
    let base = session.name.as_str();
    window.set_base_name(base.trim_end_matches(".sqlite").into());
    window.set_database_name(base.into());
    window.set_mode_text(
        if session.server {
            "Режим: сервер"
        } else {
            "Режим: локально"
        }
        .into(),
    );
    refresh(window, &shell);
    shell
}

/// Строка списка для только что созданной базы. Сервер: путь хранит адрес. Локальная: файл
/// `<имя>.sqlite` в папке. Нужен ли вход, при открытии скажет ядро (`hello`), здесь не храним.
pub fn new_database(server: bool, name: &str, folder: &str, address: &str) -> RecentDb {
    if server {
        return RecentDb {
            name: name.into(),
            path: address.into(),
            when: "".into(),
            server: true,
            protection: 0,
        };
    }
    let file = format!("{name}.sqlite");
    RecentDb {
        path: short_home(&format!("{}/{file}", folder.trim_end_matches('/'))).into(),
        name: file.into(),
        when: "".into(),
        server: false,
        protection: 0,
    }
}

/// Заменяет домашнюю папку на `~`: так путь короче и привычнее.
fn short_home(path: &str) -> String {
    // Сравниваем с разделителем: иначе при HOME=/home/ann путь /home/ann2/… стал бы «~2/…».
    let home = std::env::var("HOME").unwrap_or_default();
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home).filter(|rest| rest.starts_with('/')) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// Строка списка для добавленной существующей базы.
pub fn existing_database(server: bool, file: &str, address: &str, name: &str) -> RecentDb {
    if server {
        return RecentDb {
            name: name.into(),
            path: address.into(),
            when: "".into(),
            server: true,
            protection: 0,
        };
    }
    RecentDb {
        name: file.rsplit('/').next().unwrap_or(file).into(),
        path: short_home(file).into(),
        when: "".into(),
        server: false,
        protection: 0,
    }
}

/// Ставит базу первой в списке недавних и сохраняет список.
fn add_recent(app: &AppWindow, db: RecentDb) {
    let rows = crate::recent::push_front(app.get_recent().iter(), db);
    if let Err(message) = crate::recent::save(&rows) {
        app.set_start_error(message.into());
    }
    app.set_recent(ModelRc::new(VecModel::from(rows)));
    app.set_selected_recent(0);
}

/// «Убрать из списка»: забывается запись, запомненный логин и (у сервера) копия сертификата.
/// Файл базы и сама база на сервере не трогаются. Список сохраняется первым: если файл не
/// записался, в окне остаётся то, что лежит на диске, а не то, что вернулось бы после перезапуска.
fn forget_recent(app: &AppWindow, row: i32) {
    // Ответ ядра на открытие вернётся к строке, которой уже нет.
    if app.get_start_busy() {
        return;
    }
    let recent = app.get_recent();
    let Some(position) = index(row) else { return };
    let Some(db) = recent.row_data(position) else {
        return;
    };
    app.set_start_error("".into());
    let rows = crate::recent::remove(recent.iter(), db.server, &db.path);
    if let Err(message) = crate::recent::save(&rows) {
        app.set_start_error(message.into());
        return;
    }
    crate::recent::remember_login(&db.path, None);
    if db.server
        && let Err(error) = crate::remote_base::forget_certificate(&db.path)
    {
        app.set_start_error(error.message.into());
    }
    let selected = crate::recent::selection_after_remove(position, rows.len());
    app.set_recent(ModelRc::new(VecModel::from(rows)));
    app.set_selected_recent(i32::try_from(selected).unwrap_or(0));
}

/// Подключает подтверждение «Убрать из списка» (окно в app-window.slint) к `forget_recent`.
pub fn wire_forget(app: &AppWindow) {
    let weak = app.as_weak();
    app.on_forget_confirmed(move |row| {
        if let Some(app) = weak.upgrade() {
            forget_recent(&app, row);
        }
    });
}

/// Пользователь в шапке: имя из сессии ядра. База без пользователей вход не спрашивает.
fn show_user(app: &AppWindow, user: Option<&SessionInfo>, roles: &[RoleInfo]) {
    let name = user.map(|user| {
        if user.display_name.trim().is_empty() {
            user.login.as_str()
        } else {
            user.display_name.as_str()
        }
    });
    app.set_user_text(name.unwrap_or("Без входа").into());
    app.set_user_initials(name.map(initials).unwrap_or_default().into());
    app.set_user_roles_text(
        user.map_or(
            "База без пользователей".to_owned(),
            |user| {
                user.roles
                    .iter()
                    .map(|name| {
                        roles
                            .iter()
                            .find(|role| role.name == *name)
                            .map_or(name.as_str(), |role| role.title.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        )
        .into(),
    );
    // Сменить пользователя можно, только если база спрашивает вход.
    app.set_can_switch_user(user.is_some());
}

/// Через сколько секунд сообщение в строке состояния сменяется на «Готово».
const STATUS_SECONDS: u64 = 6;

/// Показывает сообщение в строке состояния; через несколько секунд там снова «Готово».
fn show_status(app: &AppWindow, message: &str) {
    app.set_status_message(message.into());
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(STATUS_SECONDS), move || {
        if let Some(app) = weak.upgrade() {
            app.set_status_message("Готово".into());
        }
    });
}

/// Можно ли сейчас закрыть базу или сменить пользователя: окна и несохранённые документы
/// принадлежат текущему пользователю и базе, закрыть их молча значит потерять правки.
fn can_leave(
    app: &AppWindow,
    shell: &Rc<RefCell<Shell>>,
    windows: &crate::windows::Windows,
) -> bool {
    let mut extra = 0;
    windows.each(|_| extra += 1);
    if extra > 0 {
        show_status(app, "Сначала закройте дополнительные окна.");
        return false;
    }
    if shell.borrow().tabs().iter().any(|tab| tab.modified) {
        show_status(app, "Сначала сохраните или закройте изменённые документы.");
        return false;
    }
    true
}

/// Инициалы для кружка в шапке: первые буквы двух первых слов.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Размер дополнительного окна с вкладками.
pub const HOST_SIZE: slint::LogicalSize = slint::LogicalSize {
    width: 1000.0,
    height: 700.0,
};

/// Дополнительное окно с вкладками (страница 4): те же настройки вида, что у главного.
pub fn prepare_host_window(window: &AppWindow, settings: Settings) {
    apply_settings(window, settings);
    window.set_page(4);
    window.window().set_size(HOST_SIZE);
}

/// Применяет настройки интерфейса: тема и скругление глобальные, окно тоже запоминает выбор.
fn apply_settings(window: &AppWindow, settings: Settings) {
    // Каждому окну нужны маска и разбор поля даты; настройки применяются к каждому окну.
    crate::date_input::install(window);
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
pub fn current_settings(window: &AppWindow) -> Settings {
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
    /// Путь к файлу базы или адрес сервера: по нему запоминается логин.
    key: String,
    server: bool,
    /// Текущий вид страницы входа (у базы «пароль и пользователи» их два подряд).
    stage: Option<LoginKind>,
    /// Открытая база. Пока она здесь, файл занят: другая программа его не откроет.
    engine: Option<Arc<dyn Engine>>,
    /// Метаданные базы для этого пользователя (меню и объекты уже обрезаны по правам).
    meta: Option<Meta>,
    /// Какие пункты «Администрирования» показывать. Права всё равно проверяет ядро на каждой
    /// команде.
    system: SystemRights,
    /// Вошёл ли пользователь (в базе без пользователей входа нет).
    logged_in: bool,
}

/// Встроенная роль администратора платформы (`ostov-model`). Нужна только чтобы предложить её
/// первой при создании первого пользователя; кто что может, решают права, их проверяет ядро.
const ADMIN_ROLE: &str = "admin";

/// Системные права вошедшего: по ним строятся пункты «Администрирования». База без пользователей
/// пускает без входа, там пользователь и есть администратор: у него все права.
fn system_rights(user: Option<&SessionInfo>) -> SystemRights {
    user.map_or(SystemRights::ALL, |user| user.system)
}

/// Экраны главной страницы, которым нужна открытая база.
#[derive(Clone)]
struct Screens {
    shell: Rc<RefCell<Shell>>,
    receipts: ReceiptsUi,
    users: UsersUi,
    admin: AdminUi,
    lists: ListUi,
}

/// Открывает главную страницу выбранной базы и подключает к ней экраны.
fn enter_main(app: &AppWindow, screens: &Screens, session: &Session) {
    *screens.shell.borrow_mut() = open_main(app, session);
    screens.receipts.after(app, &screens.shell.borrow());
    match &session.engine {
        Some(engine) => {
            screens.receipts.attach(engine.clone());
            let objects = session.meta.as_ref().map_or(&[][..], |meta| &meta.objects);
            screens.lists.start(engine.clone(), objects);
        }
        None => {
            screens.receipts.show_error("База не открыта.");
            screens.lists.clear();
        }
    }
    let roles = session
        .meta
        .as_ref()
        .map_or(&[][..], |meta| &meta.roles[..]);
    let titles: Vec<slint::SharedString> = roles
        .iter()
        .map(|role| role.title.as_str().into())
        .collect();
    app.set_roles(ModelRc::new(VecModel::from(titles)));
    // В базе без пользователей первым нужен администратор: предлагаем его роль сразу.
    let default_role = if session.logged_in {
        None
    } else {
        roles.iter().position(|role| role.name == ADMIN_ROLE)
    };
    app.set_nu_role(
        default_role
            .and_then(|i| i32::try_from(i).ok())
            .unwrap_or(0),
    );
    screens.users.clear();
    if session.system.admin {
        screens.users.reload(app);
    }
    screens.admin.start(session.system);
    app.set_password("".into());
    app.set_repeat("".into());
    show_page(app, PAGE_MAIN);
}

/// Открывает локальную базу в фоне и спрашивает ядро, нужен ли вход (`hello`).
fn open_local(app: &AppWindow, db: RecentDb, session: &Rc<RefCell<Session>>, screens: &Screens) {
    app.set_start_busy(true);
    let path = db.path.to_string();
    let weak = app.as_weak();
    let (session, screens) = (session.clone(), screens.clone());
    call_in_background(
        move || {
            let engine = crate::local_base::open(&path)?;
            let hello = engine.hello()?;
            // Без входа метаданные можно взять сразу; со входом их даст ядро после login.
            let meta = if hello.login_required {
                None
            } else {
                Some(engine.get_meta()?)
            };
            Ok((engine, hello, meta))
        },
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            app.set_start_busy(false);
            let (engine, hello, meta) = match result {
                Ok(opened) => opened,
                Err(error) => {
                    app.set_start_error(error.message.into());
                    return;
                }
            };
            let kind = if hello.login_required {
                LoginKind::LocalUser
            } else {
                LoginKind::None
            };
            *session.borrow_mut() = Session {
                name: db.name.to_string(),
                key: db.path.to_string(),
                stage: Some(kind),
                engine: Some(engine),
                meta,
                system: system_rights(None),
                ..Session::default()
            };
            if kind == LoginKind::None {
                show_user(&app, None, &[]);
                enter_main(&app, &screens, &session.borrow());
                return;
            }
            configure_login(&app, kind, &db.name, &db.path);
            prefill_login(&app, &db.path);
            show_page(&app, PAGE_LOGIN);
        },
    );
}

/// Подставляет на страницу входа логин, который просили запомнить для этой базы. Пароль не
/// запоминается никогда.
fn prefill_login(app: &AppWindow, key: &str) {
    if let Some(login) = crate::recent::remembered_login(key) {
        app.set_login(login.into());
    }
    app.set_remember(true);
}

/// Подключается к серверу в фоне и спрашивает `hello`: первый запуск (нужен администратор) или
/// обычный вход. Сертификат берётся из сохранённого при добавлении сервера.
fn open_remote(app: &AppWindow, db: RecentDb, session: &Rc<RefCell<Session>>) {
    app.set_start_busy(true);
    let address = db.path.to_string();
    let weak = app.as_weak();
    let session = session.clone();
    call_in_background(
        move || {
            let pem = crate::remote_base::stored_certificate(&address)?;
            crate::remote_base::check(&address, &pem)
        },
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            app.set_start_busy(false);
            let (engine, hello) = match result {
                Ok(connected) => connected,
                Err(error) => {
                    app.set_start_error(error.message.into());
                    return;
                }
            };
            let kind = if hello.first_run {
                LoginKind::ServerFirstRun
            } else {
                LoginKind::Server
            };
            *session.borrow_mut() = Session {
                name: db.name.to_string(),
                key: db.path.to_string(),
                server: true,
                stage: Some(kind),
                engine: Some(engine),
                ..Session::default()
            };
            configure_login(&app, kind, &db.name, &db.path);
            if kind == LoginKind::Server {
                prefill_login(&app, &db.path);
            }
            show_page(&app, PAGE_LOGIN);
        },
    );
}

/// Закрывает сессию с базой. Сервер просим завершить сессию (иначе токен живёт до простоя);
/// локальная база закрывается, когда ядро выпускают из рук: так освобождается файл.
fn release(session: Session) {
    if let (true, Some(engine)) = (session.server, session.engine) {
        call_in_background(move || engine.logout(), |_| {});
    }
}

/// Вход: пароль проверяет ядро и держит задержку после неудач (СТ-32). На первом запуске
/// сервера вместо входа создаётся администратор (СТ-36).
fn submit_login(app: &AppWindow, session: &Rc<RefCell<Session>>, screens: &Screens) {
    let (engine, kind, key) = {
        let session = session.borrow();
        (
            session.engine.clone(),
            session.stage.unwrap_or(LoginKind::LocalUser),
            session.key.clone(),
        )
    };
    let Some(engine) = engine else {
        return;
    };
    let first_run = kind == LoginKind::ServerFirstRun;
    let login = app.get_login().trim().to_owned();
    let password = app.get_password().to_string();
    if first_run && password != app.get_repeat().as_str() {
        app.set_error("Пароли не совпадают.".into());
        return;
    }
    let remember = (!first_run && app.get_remember()).then(|| login.clone());
    app.set_error("".into());
    app.set_locked(true);
    app.set_button_text("Проверяем…".into());
    let weak = app.as_weak();
    let (session, screens) = (session.clone(), screens.clone());
    // Пока ядро проверяет пароль, можно уйти «Назад» и открыть другую базу: такой ответ лишний.
    let asked = engine.clone();
    call_in_background(
        move || {
            let user = if first_run {
                engine.create_first_admin(&login, &password)?
            } else {
                engine.login(&login, &password)?
            };
            let meta = engine.get_meta()?;
            Ok((user, meta))
        },
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            let current = session
                .borrow()
                .engine
                .as_ref()
                .is_some_and(|engine| Arc::ptr_eq(engine, &asked));
            drop(asked);
            if !current {
                return;
            }
            app.set_locked(false);
            app.set_button_text(button_text(kind).into());
            match result {
                Ok((user, meta)) => {
                    if !first_run {
                        crate::recent::remember_login(&key, remember.as_deref());
                    }
                    show_user(&app, Some(&user), &meta.roles);
                    {
                        let mut session = session.borrow_mut();
                        session.meta = Some(meta);
                        session.system = system_rights(Some(&user));
                        session.logged_in = true;
                    }
                    enter_main(&app, &screens, &session.borrow());
                }
                Err(error) => {
                    app.set_password("".into());
                    app.set_repeat("".into());
                    app.set_error(error.message.into());
                }
            }
        },
    );
}

/// Создаёт файл базы в фоне; в список база попадает, только если ядро её создало.
fn create_local(app: &AppWindow, db: RecentDb) {
    let file_password = app
        .get_new_protect_password()
        .then(|| app.get_new_password().to_string());
    let admin = app.get_new_protect_users().then(|| NewAdmin {
        login: app.get_new_admin_login().trim().to_owned(),
        password: app.get_new_admin_password().to_string(),
    });
    let path = db.path.to_string();
    app.set_create_error("".into());
    app.set_create_busy(true);
    let weak = app.as_weak();
    call_in_background(
        // Базу сразу закрываем: открывать её будет «Открыть», как любую другую.
        move || {
            crate::local_base::create(&path, file_password.as_deref(), admin.as_ref()).map(|_| ())
        },
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            app.set_create_busy(false);
            match result {
                Ok(()) => {
                    add_recent(&app, db);
                    reset_create(&app);
                }
                Err(error) => app.set_create_error(error.message.into()),
            }
        },
    );
}

/// Очищает окно «Создать базу» и закрывает его. Пароли в свойствах окна не оставляем.
fn reset_create(app: &AppWindow) {
    app.set_new_name("".into());
    app.set_new_address("".into());
    app.set_new_cert("".into());
    app.set_new_admin_login("".into());
    app.set_new_admin_password("".into());
    app.set_new_admin_repeat("".into());
    app.set_new_password("".into());
    app.set_new_password_repeat("".into());
    app.set_new_protect_password(false);
    app.set_new_protect_users(false);
    app.set_create_error("".into());
    app.set_creating(false);
}

/// Проверяет добавляемый файл ядром (`check_local`: база Ostov нужной конфигурации) без записи в него.
fn add_local(app: &AppWindow, db: RecentDb) {
    let path = db.path.to_string();
    app.set_add_error("".into());
    app.set_add_busy(true);
    let weak = app.as_weak();
    call_in_background(
        move || crate::local_base::check(&path).map(|_| ()),
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            app.set_add_busy(false);
            match result {
                Ok(()) => {
                    add_recent(&app, db);
                    reset_add(&app);
                }
                Err(error) => app.set_add_error(error.message.into()),
            }
        },
    );
}

fn reset_add(app: &AppWindow) {
    app.set_add_file("".into());
    app.set_add_address("".into());
    app.set_add_cert("".into());
    app.set_add_name("".into());
    app.set_add_error("".into());
    app.set_adding(false);
}

/// Первый запуск сервера из окна «Создать базу»: создаёт администратора и сохраняет сертификат.
/// Пароль не короче 12 символов проверяет сервер (СТ-32), его ошибка показывается в окне.
fn create_remote_base(app: &AppWindow, db: RecentDb) {
    let cert_file = app.get_new_cert().trim().to_owned();
    let admin = NewAdmin {
        login: app.get_new_admin_login().trim().to_owned(),
        password: app.get_new_admin_password().to_string(),
    };
    let address = db.path.to_string();
    app.set_create_error("".into());
    app.set_create_busy(true);
    let weak = app.as_weak();
    call_in_background(
        move || {
            let pem = crate::remote_base::read_certificate(&cert_file)?;
            // Сессию закрываем сразу: открывать сервер будет «Открыть», как любую базу.
            crate::remote_base::create(&address, &pem, &admin)?;
            crate::remote_base::store_certificate(&address, &pem)
        },
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            app.set_create_busy(false);
            match result {
                Ok(()) => {
                    add_recent(&app, db);
                    reset_create(&app);
                }
                Err(error) => app.set_create_error(error.message.into()),
            }
        },
    );
}

/// Добавляет сервер: проверяет адрес и сертификат подключением и сохраняет сертификат.
fn add_remote(app: &AppWindow, db: RecentDb) {
    let cert_file = app.get_add_cert().trim().to_owned();
    let address = db.path.to_string();
    app.set_add_error("".into());
    app.set_add_busy(true);
    let weak = app.as_weak();
    call_in_background(
        move || {
            let pem = crate::remote_base::read_certificate(&cert_file)?;
            crate::remote_base::check(&address, &pem)?;
            crate::remote_base::store_certificate(&address, &pem)
        },
        move |result| {
            let Some(app) = weak.upgrade() else { return };
            app.set_add_busy(false);
            match result {
                Ok(()) => {
                    add_recent(&app, db);
                    reset_add(&app);
                }
                Err(error) => app.set_add_error(error.message.into()),
            }
        },
    );
}

/// Системное окно выбора сертификата сервера; результат попадает в окно «Создать» или «Добавить».
fn pick_certificate(app: slint::Weak<AppWindow>, for_create: bool) {
    let _ = slint::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("Сертификат сервера")
            .add_filter("Сертификат", &["crt", "pem", "cer"])
            .pick_file()
            .await
        else {
            return;
        };
        let Some(app) = app.upgrade() else { return };
        let path = short_home(&file.path().to_string_lossy());
        if for_create {
            app.set_new_cert(path.into());
        } else {
            app.set_add_cert(path.into());
        }
    });
}

/// Запускает приложение: выбор базы → вход → главная. Ядро подключается позже через Engine API.
/// Обработчики навигации и вкладок окна: выбор, закрытие, порядок, перетаскивание. Подключаются
/// так же в тестовом стенде (`examples/drag_e2e.rs`), который шлёт окну настоящие события мыши.
pub fn wire_tabs(
    app: &AppWindow,
    shell: &Rc<RefCell<Shell>>,
    receipts: &crate::receipts_ui::ReceiptsUi,
    windows: &crate::windows::Windows,
) {
    app.on_nav_clicked({
        let (app, shell, receipts, windows) = (
            app.as_weak(),
            shell.clone(),
            receipts.clone(),
            windows.clone(),
        );
        move |id| {
            let Some(app) = app.upgrade() else { return };
            windows.set_active(crate::workspace::WindowRef::Main);
            let mut shell = shell.borrow_mut();
            receipts.before(&app, &mut shell);
            if let Some(id) = index(id) {
                shell.click(id);
            }
            receipts.after(&app, &shell);
            refresh(&app, &shell);
            drop(shell);
            // Группу могли раскрыть: дерево в дополнительных окнах то же.
            windows.refresh_hosts();
        }
    });

    app.on_tab_selected({
        let (app, shell, receipts, windows) = (
            app.as_weak(),
            shell.clone(),
            receipts.clone(),
            windows.clone(),
        );
        move |tab| {
            let Some(app) = app.upgrade() else { return };
            windows.set_active(crate::workspace::WindowRef::Main);
            let mut shell = shell.borrow_mut();
            receipts.before(&app, &mut shell);
            if let Some(tab) = index(tab) {
                shell.select_tab(tab);
            }
            receipts.after(&app, &shell);
            refresh(&app, &shell);
        }
    });

    app.on_tab_reordered({
        let (app, shell, receipts, windows) = (
            app.as_weak(),
            shell.clone(),
            receipts.clone(),
            windows.clone(),
        );
        move |from, to| {
            let Some(app) = app.upgrade() else { return };
            let (Some(from), Some(to)) = (index(from), index(to)) else {
                return;
            };
            windows.set_active(crate::workspace::WindowRef::Main);
            receipts.before(&app, &mut shell.borrow_mut());
            windows.reorder(&app, &shell, crate::workspace::WindowRef::Main, from, to);
            receipts.after(&app, &shell.borrow());
        }
    });

    app.on_tab_drag_moved({
        let (app, shell, windows) = (app.as_weak(), shell.clone(), windows.clone());
        move |tab, x, y, out| {
            let Some(app) = app.upgrade() else { return };
            let Some(tab) = index(tab) else { return };
            windows.drag_moved(
                &app,
                &shell,
                crate::workspace::WindowRef::Main,
                tab,
                &app,
                x,
                y,
                out,
            );
        }
    });

    app.on_tab_drag_ended({
        let (app, shell, windows) = (app.as_weak(), shell.clone(), windows.clone());
        move |tab, x, y, out| {
            let Some(app) = app.upgrade() else { return };
            let Some(tab) = index(tab) else { return };
            let main = crate::workspace::WindowRef::Main;
            windows.set_active(main);
            if windows.drag_ended(&app, &shell, main, tab, &app, x, y, out) || !out {
                return;
            }
            // Запасной путь (чистый Wayland): окно создаётся при отпускании.
            windows.detach_from_main(&app, &shell, tab);
        }
    });

    app.on_tab_detached({
        let (app, shell, windows) = (app.as_weak(), shell.clone(), windows.clone());
        move |tab| {
            let Some(app) = app.upgrade() else { return };
            let Some(tab) = index(tab) else { return };
            windows.detach_from_main(&app, &shell, tab);
        }
    });
    app.on_tab_closed({
        let (app, shell, receipts) = (app.as_weak(), shell.clone(), receipts.clone());
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
}

pub fn run() -> Result<(), slint::PlatformError> {
    use slint::Model;

    // Идентификатор приложения: по нему оболочка связывает окно с ярлыком ostov.desktop.
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id("ostov")?;

    let app = AppWindow::new()?;
    apply_settings(&app, Settings::load());
    app.set_app_version(env!("CARGO_PKG_VERSION").into());
    app.set_recent(ModelRc::new(VecModel::from(crate::recent::load())));
    wire_forget(&app);
    if let Some((title, version)) = crate::local_base::config_title() {
        app.set_config_name(title.into());
        app.set_config_info(format!("Конфигурация · версия {version}").into());
    }
    let shell = Rc::new(RefCell::new(Shell::empty()));
    let receipts = crate::receipts_ui::setup(&app, shell.clone());
    let windows = crate::windows::Windows::new(&receipts);
    let session = Rc::new(RefCell::new(Session::default()));
    let directory = Rc::new(RefCell::new(Directory::default()));
    let screens = Screens {
        shell: shell.clone(),
        receipts: receipts.clone(),
        users: UsersUi {
            main: app.as_weak(),
            windows: windows.clone(),
            directory: directory.clone(),
            session: session.clone(),
        },
        admin: AdminUi::new(app.as_weak(), windows.clone(), {
            let session = session.clone();
            move || session.borrow().engine.clone()
        }),
        lists: ListUi::new(app.as_weak(), windows.clone()),
    };

    // Закрытие окна пользователем завершает приложение.
    app.window().on_close_requested(quit_on_close);

    app.on_open({
        let app = app.as_weak();
        let (session, screens) = (session.clone(), screens.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let recent = app.get_recent();
            let Some(db) = index(app.get_selected_recent()).and_then(|i| recent.row_data(i)) else {
                return;
            };
            app.set_start_error("".into());
            if !db.server {
                open_local(&app, db, &session, &screens);
                return;
            }
            open_remote(&app, db, &session);
        }
    });

    app.on_submit({
        let app = app.as_weak();
        let (session, screens) = (session.clone(), screens.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let kind = session.borrow().stage.unwrap_or(LoginKind::None);
            match kind {
                LoginKind::LocalUser | LoginKind::Server | LoginKind::ServerFirstRun => {
                    submit_login(&app, &session, &screens);
                }
                // Пароль файла ядро пока не поддерживает (В-2), поддельной проверки нет.
                LoginKind::LocalPassword | LoginKind::None => {
                    app.set_error("Пароль файла базы пока не поддерживается ядром.".into());
                }
            }
        }
    });

    // «Назад к выбору базы» и «Сменить»: вернуться к списку баз.
    let to_start = {
        let app = app.as_weak();
        let (session, receipts, admin) = (session.clone(), receipts.clone(), screens.admin.clone());
        let lists = screens.lists.clone();
        move || {
            let Some(app) = app.upgrade() else { return };
            // Списки держат движок: забываем их до закрытия базы, иначе файл остался бы занят.
            lists.clear();
            // Закрываем базу: иначе файл остаётся занят и его не открыть снова.
            release(std::mem::take(&mut *session.borrow_mut()));
            receipts.detach();
            admin.clear();
            app.set_password("".into());
            app.set_repeat("".into());
            app.set_error("".into());
            show_page(&app, PAGE_START);
        }
    };
    app.on_back(to_start.clone());
    app.on_change_source(to_start.clone());

    // Меню под аватаром: закрыть базу и вернуться к выбору.
    app.on_close_base({
        let (app, shell, windows) = (app.as_weak(), shell.clone(), windows.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            if can_leave(&app, &shell, &windows) {
                to_start();
            }
        }
    });

    // Меню под аватаром: выйти и войти под другим пользователем в той же открытой базе.
    app.on_switch_user({
        let (app, shell, windows) = (app.as_weak(), shell.clone(), windows.clone());
        let (session, screens) = (session.clone(), screens.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            if !can_leave(&app, &shell, &windows) {
                return;
            }
            let Some(engine) = session.borrow().engine.clone() else {
                return;
            };
            let weak = app.as_weak();
            let (session, screens) = (session.clone(), screens.clone());
            call_in_background(
                move || engine.logout(),
                move |result| {
                    let Some(app) = weak.upgrade() else { return };
                    if let Err(error) = result {
                        show_status(&app, &error.message);
                        return;
                    }
                    let (name, key, kind) = {
                        let mut session = session.borrow_mut();
                        session.meta = None;
                        session.system = SystemRights::default();
                        session.logged_in = false;
                        let kind = if session.server {
                            LoginKind::Server
                        } else {
                            LoginKind::LocalUser
                        };
                        session.stage = Some(kind);
                        (session.name.clone(), session.key.clone(), kind)
                    };
                    // Данные прошлого пользователя уходят с экрана: у следующего другие права.
                    screens.receipts.detach();
                    screens.users.clear();
                    screens.admin.clear();
                    screens.lists.clear();
                    configure_login(&app, kind, &name, &key);
                    prefill_login(&app, &key);
                    show_page(&app, PAGE_LOGIN);
                },
            );
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
            );
            if db.server {
                create_remote_base(&app, db);
                return;
            }
            create_local(&app, db);
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
        let (app, windows) = (app.as_weak(), windows.clone());
        move |dark| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                dark,
                ..current_settings(&app)
            };
            apply_settings(&app, settings);
            windows.each(|window| apply_settings(window, settings));
            settings.save();
        }
    });

    app.on_radius_chosen({
        let (app, windows) = (app.as_weak(), windows.clone());
        move |radius| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                radius,
                ..current_settings(&app)
            };
            apply_settings(&app, settings);
            windows.each(|window| apply_settings(window, settings));
            settings.save();
        }
    });

    app.on_density_chosen({
        let (app, windows) = (app.as_weak(), windows.clone());
        move |density| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                density,
                ..current_settings(&app)
            };
            apply_settings(&app, settings);
            windows.each(|window| apply_settings(window, settings));
            settings.save();
        }
    });

    app.on_font_chosen({
        let (app, windows) = (app.as_weak(), windows.clone());
        move |font| {
            let Some(app) = app.upgrade() else { return };
            let settings = Settings {
                font,
                ..current_settings(&app)
            };
            apply_settings(&app, settings);
            windows.each(|window| apply_settings(window, settings));
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

    app.on_choose_cert_new({
        let app = app.as_weak();
        move || pick_certificate(app.clone(), true)
    });
    app.on_choose_cert_add({
        let app = app.as_weak();
        move || pick_certificate(app.clone(), false)
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
            if db.server {
                add_remote(&app, db);
                return;
            }
            add_local(&app, db);
        }
    });

    wire_users(&app, &screens.users);
    screens.admin.attach_window(&app);
    screens.lists.attach_window(&app);
    windows.on_host_created({
        let (main, users, admin) = (app.as_weak(), screens.users.clone(), screens.admin.clone());
        let lists = screens.lists.clone();
        move |window| {
            lists.attach_window(window);
            if let Some(main) = main.upgrade() {
                refresh_users(window, &users.directory.borrow());
                window.set_roles(main.get_roles());
                wire_users(window, &users);
                admin.attach_window(window);
            }
        }
    });

    wire_tabs(&app, &shell, &receipts, &windows);

    app.show()?;
    slint::run_event_loop_until_quit()
}

/// «Пользователи и роли» через ядро: список, добавление, включение и отключение. Справочник
/// один на все окна; проверки (пароль, роль, последний администратор) делает ядро.
#[derive(Clone)]
struct UsersUi {
    main: slint::Weak<AppWindow>,
    windows: crate::windows::Windows,
    directory: Rc<RefCell<Directory>>,
    session: Rc<RefCell<Session>>,
}

impl UsersUi {
    fn show_all(&self) {
        if let Some(main) = self.main.upgrade() {
            refresh_users(&main, &self.directory.borrow());
        }
        self.windows
            .each(|host| refresh_users(host, &self.directory.borrow()));
    }

    /// Забывает пользователей прошлой базы.
    fn clear(&self) {
        *self.directory.borrow_mut() = Directory::default();
        self.show_all();
    }

    /// Выполняет команду ядра в фоне и перечитывает список; `done` получает текст ошибки ядра.
    fn run(
        &self,
        window: &AppWindow,
        command: impl FnOnce(&dyn Engine) -> EngineResult<()> + Send + 'static,
        done: impl FnOnce(&AppWindow, Result<(), String>) + 'static,
    ) {
        let Some(engine) = self.session.borrow().engine.clone() else {
            return;
        };
        let (users, weak) = (self.clone(), window.as_weak());
        call_in_background(
            move || {
                command(&*engine)?;
                engine.admin_users()
            },
            move |result| {
                let Some(window) = weak.upgrade() else { return };
                match result {
                    Ok(list) => {
                        let roles = users
                            .session
                            .borrow()
                            .meta
                            .as_ref()
                            .map(|meta| meta.roles.clone())
                            .unwrap_or_default();
                        *users.directory.borrow_mut() = Directory::new(list, roles);
                        users.show_all();
                        done(&window, Ok(()));
                    }
                    Err(error) => done(&window, Err(error.message)),
                }
            },
        );
    }

    /// В базе появился первый пользователь: ядро теперь требует вход, текущая сессия без
    /// пользователя больше ничего не может. Показываем страницу входа той же открытой базы.
    fn ask_login(&self) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let name = {
            let mut session = self.session.borrow_mut();
            session.stage = Some(LoginKind::LocalUser);
            session.name.clone()
        };
        configure_login(&main, LoginKind::LocalUser, &name, "");
        main.set_hint("В базе появился администратор: войдите под ним.".into());
        show_page(&main, PAGE_LOGIN);
    }

    fn reload(&self, window: &AppWindow) {
        self.run(
            window,
            |_| Ok(()),
            |window, result| window.set_user_message(result.err().unwrap_or_default().into()),
        );
    }
}

/// Подключает «Пользователи и роли» к окну `window`.
fn wire_users(window: &AppWindow, users: &UsersUi) {
    window.on_user_toggle({
        let (window, users) = (window.as_weak(), users.clone());
        move |row| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let Some((login, active)) = index(row).and_then(|i| {
                users
                    .directory
                    .borrow()
                    .get(i)
                    .map(|u| (u.login.clone(), u.active))
            }) else {
                return;
            };
            users.run(
                &window,
                move |engine| engine.admin_set_user_active(&login, !active),
                |window, result| window.set_user_message(result.err().unwrap_or_default().into()),
            );
        }
    });

    window.on_user_password({
        let (window, users) = (window.as_weak(), users.clone());
        move |row| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let Some(login) =
                index(row).and_then(|i| users.directory.borrow().get(i).map(|u| u.login.clone()))
            else {
                return;
            };
            window.set_pw_login(login.into());
            window.set_pw_password("".into());
            window.set_pw_repeat("".into());
            window.set_pw_error("".into());
            window.set_changing_password(true);
        }
    });

    window.on_password_confirmed({
        let (window, users) = (window.as_weak(), users.clone());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let login = window.get_pw_login().to_string();
            let password = window.get_pw_password().to_string();
            window.set_pw_error("".into());
            let main = users.main.clone();
            users.run(
                &window,
                move |engine| engine.admin_set_user_password(&login, &password),
                move |window, result| match result {
                    // Правила пароля (длина на сервере, нет такого логина) знает ядро.
                    Err(message) => window.set_pw_error(message.into()),
                    Ok(()) => {
                        let login = window.get_pw_login();
                        window.set_pw_password("".into());
                        window.set_pw_repeat("".into());
                        window.set_changing_password(false);
                        if let Some(main) = main.upgrade() {
                            show_status(&main, &format!("Пароль пользователя «{login}» сменён."));
                        }
                    }
                },
            );
        }
    });

    window.on_user_add_confirmed({
        let (window, users) = (window.as_weak(), users.clone());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            // Команде нужно имя роли, а в окне заголовки: имя берём из метаданных по номеру.
            let role = index(window.get_nu_role()).and_then(|i| {
                users
                    .session
                    .borrow()
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.roles.get(i).map(|role| role.name.clone()))
            });
            let Some(role) = role else {
                window.set_user_error("Выберите роль.".into());
                return;
            };
            // В базе без пользователей первый добавленный включает вход. Что первым должен быть
            // администратор, проверяет ядро, его ошибка видна в окне.
            let first = !users.session.borrow().logged_in;
            let login = window.get_nu_login().trim().to_owned();
            let name = window.get_nu_name().trim().to_owned();
            let user = NewUser {
                display_name: if name.is_empty() { login.clone() } else { name },
                login,
                role,
                password: window.get_nu_password().to_string(),
            };
            window.set_user_error("".into());
            let after = users.clone();
            users.run(
                &window,
                move |engine| engine.admin_add_user(&user),
                move |window, result| match result {
                    Err(message) => window.set_user_error(message.into()),
                    Ok(()) => {
                        window.set_user_message("".into());
                        window.set_nu_login("".into());
                        window.set_nu_name("".into());
                        window.set_nu_role(0);
                        window.set_nu_password("".into());
                        window.set_nu_repeat("".into());
                        window.set_adding_user(false);
                        if first {
                            after.ask_login();
                        }
                    }
                },
            );
        }
    });
}

fn quit_on_close() -> slint::CloseRequestResponse {
    let _ = slint::quit_event_loop();
    slint::CloseRequestResponse::HideWindow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_two_first_letters() {
        assert_eq!(initials("Артём Кержанцев"), "АК");
        assert_eq!(initials("admin"), "A");
        assert_eq!(initials("иван петров сидорович"), "ИП");
        assert_eq!(initials("  "), "");
    }

    #[test]
    fn new_local_base_is_file_in_folder() {
        let db = new_database(false, "Склад", "/data/Ostov/", "");
        assert_eq!(db.name, "Склад.sqlite");
        assert_eq!(db.path, "/data/Ostov/Склад.sqlite");
        assert!(!db.server);
    }

    #[test]
    fn short_home_needs_separator() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let home = home.trim_end_matches('/');
        if home.is_empty() {
            return;
        }
        assert_eq!(
            short_home(&format!("{home}/Ostov/a.sqlite")),
            "~/Ostov/a.sqlite"
        );
        let neighbour = format!("{home}2/a.sqlite");
        assert_eq!(short_home(&neighbour), neighbour);
    }

    #[test]
    fn base_without_users_has_all_rights() {
        assert_eq!(system_rights(None), SystemRights::ALL);
    }

    #[test]
    fn rights_come_from_the_role_not_from_its_name() {
        let user = |system: SystemRights| SessionInfo {
            login: "u".into(),
            display_name: "".into(),
            roles: vec!["any".into()],
            system,
        };
        // Роль из конфигурации с `system.admin` тоже видит раздел, а право закрывать период
        // не тянет за собой право администратора.
        let admin = SystemRights {
            admin: true,
            ..SystemRights::default()
        };
        let accountant = SystemRights {
            set_closing_date: true,
            ..SystemRights::default()
        };
        assert_eq!(system_rights(Some(&user(admin))), admin);
        assert_eq!(system_rights(Some(&user(accountant))), accountant);
        assert_eq!(
            system_rights(Some(&user(SystemRights::default()))),
            SystemRights::default()
        );
    }
}
