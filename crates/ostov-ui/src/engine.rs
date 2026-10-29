//! Заготовка контракта с ядром (Engine API, Дизайн-документ §3). ПРЕДЛОЖЕНИЕ: набор команд и
//! типы согласовать с Никитой, после согласования трейт переедет в `ostov-api`.
//!
//! Идея: `ostov-ui` знает только трейт [`Engine`]. В режиме 1 за ним будет ядро в процессе
//! (`ostov-core`, Local), в режиме 2 клиент gRPC (`ostov-client`, Remote). Подключение
//! (открыть файл, соединиться с сервером) делает [`Connector`]: он возвращает готовый `Engine`.
//!
//! Правила контракта: одна команда это одна транзакция; ядро не держит транзакцию между
//! командами; данные и ошибки сериализуемы, без ссылок на память ядра; вызовы идут в фоновом
//! потоке (см. [`run_in_background`]), чтобы не блокировать отрисовку.
//!
//! Сейчас все команды возвращают [`ErrorCode::NotImplemented`]: интерфейс на примерных данных.
//! Где какая команда понадобится, описано в `Вызовы-ядра-из-ostov-ui.md` в корне репозитория.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Версия протокола: сверяется при подключении (ИТ-06).
pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------- ошибки

/// Код ошибки: по нему интерфейс выбирает, что показать и куда (баннер, подсказка, диалог).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Команда ещё не реализована (заглушка).
    NotImplemented,
    /// Нет связи с сервером или базой.
    Unavailable,
    /// Неверный логин или пароль (счётчик неудач и задержка на стороне ядра, СТ-32).
    BadCredentials,
    /// Для входа нужно создать администратора (первый запуск сервера, СТ-36).
    NeedFirstAdmin,
    /// Файл базы зашифрован: нужен пароль файла (СТ-35).
    NeedFilePassword,
    /// База уже открыта другим экземпляром (СТ-55).
    AlreadyOpen,
    /// У пользователя нет права на действие (СТ-31).
    Forbidden,
    /// Объект изменили после того, как мы его прочитали (проверка версии).
    VersionConflict,
    /// Период закрыт: проведение или правка в этой дате запрещены.
    PeriodClosed,
    /// Данные не прошли проверку (обязательное поле, остаток ниже нуля и т. п.).
    Validation,
    /// Версия протокола или формата конфигурации не подходит (ИТ-06, СТ-07).
    Incompatible,
    /// Не найдено.
    NotFound,
    /// Внутренняя ошибка ядра.
    Internal,
}

/// Ошибка ядра: код, текст для человека («что делать»), технические подробности для журнала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub code: ErrorCode,
    pub message: String,
    pub details: String,
}

impl EngineError {
    pub fn not_implemented(command: &str) -> Self {
        Self {
            code: ErrorCode::NotImplemented,
            message: "Ядро ещё не подключено.".to_owned(),
            details: format!("команда {command} не реализована"),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for EngineError {}

pub type EngineResult<T> = Result<T, EngineError>;

// ---------------------------------------------------------------- сессия и метаданные

/// Ответ на `Hello`: версии и сведения о загруженной конфигурации (СТ-06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloInfo {
    pub protocol_version: u32,
    /// Название и версия конфигурации («Управление складом», «1.2.0»): панель бренда.
    pub config_name: String,
    pub config_version: String,
    /// Версия формата конфигурации (СТ-07).
    pub config_format: u32,
    /// Нужно ли входить (режим 2 и локальная база с пользователями).
    pub login_required: bool,
    /// Сервер ещё без администратора: первый вход создаёт его (СТ-36).
    pub first_run: bool,
}

/// Кто вошёл: строка состояния («Артём (кладовщик)»).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub login: String,
    pub display_name: String,
    pub roles: Vec<String>,
}

/// Вид объекта конфигурации (дерево конфигурации, Дизайн §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Catalog,
    Document,
    Register,
    Report,
    Enum,
}

/// Права на объект (СТ-30): интерфейс прячет или блокирует то, что запрещено, а проверяет ядро.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rights {
    pub read: bool,
    pub create: bool,
    pub update: bool,
    pub post: bool,
    pub unpost: bool,
    pub delete: bool,
}

/// Пункт меню или навигации: из него строится дерево слева (`shell.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub title: String,
    /// Имя значка из `assets/icons`; настраивается в свойствах элемента конфигурации.
    pub icon: String,
    /// Объект, который открывает пункт (`None`: группа).
    pub object: Option<String>,
    pub children: Vec<MenuItem>,
}

/// Описание колонки списка или поля формы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMeta {
    pub name: String,
    pub title: String,
    /// Тип значения: строка, число, дата, ссылка на справочник, перечисление, флажок.
    pub kind: FieldKind,
    pub required: bool,
    pub read_only: bool,
    /// Ссылка и перечисление: откуда брать значения для выпадающего списка.
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Number,
    Money,
    Date,
    Reference,
    Enum,
    Flag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    pub name: String,
    pub title: String,
    pub kind: ObjectKind,
    pub fields: Vec<FieldMeta>,
    /// Табличные части (название, поля).
    pub tables: Vec<(String, Vec<FieldMeta>)>,
    pub rights: Rights,
}

/// `GetMeta`: всё, что нужно, чтобы нарисовать интерфейс текущего пользователя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub menu: Vec<MenuItem>,
    pub objects: Vec<ObjectMeta>,
    /// Роли, из которых администратор выбирает пользователю роль (экран «Пользователи и роли»).
    pub roles: Vec<String>,
}

// ---------------------------------------------------------------- данные

/// Строка списка: идентификатор и значения ячеек в порядке колонок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: u64,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub field: String,
    pub descending: bool,
}

/// Окно данных (§3.1): список не грузится целиком (СТ-15), следующая страница идёт по ключу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Ключ строки, после которой нужна страница; `None`: с начала.
    pub after_key: Option<String>,
    /// Сколько строк (в пределах лимита СТ-51).
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListQuery {
    pub object: String,
    pub order: Vec<SortKey>,
    /// Строка поиска по списку (поле «Поиск по списку»).
    pub filter: String,
    pub window: Window,
}

/// Ответ `List`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub rows: Vec<Row>,
    /// Позиция первой строки окна во всём списке.
    pub window_start: u64,
    /// Ключ для следующей страницы; `None`: конец списка.
    pub next_key: Option<String>,
    /// Примерный размер списка («≈1240»); точное число дорого (§3.1).
    pub total_estimate: u64,
    /// Маркер списка для `Seek`: привязан к порядку и отбору.
    pub token: String,
}

/// Объект целиком: поля шапки и табличные части (строки документа).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectData {
    pub id: u64,
    /// Версия для проверки изменений (оптимистичная блокировка).
    pub version: u64,
    pub posted: bool,
    pub fields: BTreeMap<String, String>,
    pub tables: BTreeMap<String, Vec<BTreeMap<String, String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub id: u64,
    pub version: u64,
}

// ---------------------------------------------------------------- отчёты, обмен, администрирование

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportQuery {
    pub report: String,
    pub params: BTreeMap<String, String>,
    pub window: Window,
}

/// Идентификатор фоновой задачи (импорт, экспорт, копия базы): ход виден в уведомлениях.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub active: bool,
}

/// Новый пользователь. Пароль идёт только по защищённому каналу, хэш (Argon2id) считает ядро.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub when: String,
    pub user: String,
    pub object: String,
    pub action: String,
    pub details: String,
}

// ---------------------------------------------------------------- Engine

/// Контракт интерфейса с ядром. Все методы синхронные: интерфейс вызывает их из фонового
/// потока. По умолчанию каждая команда это заглушка «не реализовано», реализация переопределяет
/// нужные.
pub trait Engine: Send + Sync {
    /// Версия протокола и сведения о конфигурации. Первый вызов после подключения.
    fn hello(&self) -> EngineResult<HelloInfo> {
        Err(EngineError::not_implemented("Hello"))
    }
    /// Вход (режим 2 и локальная база с пользователями). Задержку после неудач ведёт ядро.
    fn login(&self, _login: &str, _password: &str) -> EngineResult<SessionInfo> {
        Err(EngineError::not_implemented("Login"))
    }
    fn logout(&self) -> EngineResult<()> {
        Err(EngineError::not_implemented("Logout"))
    }
    /// Создаёт первого администратора (первый запуск сервера, СТ-36) и входит под ним.
    fn create_first_admin(&self, _login: &str, _password: &str) -> EngineResult<SessionInfo> {
        Err(EngineError::not_implemented("CreateFirstAdmin"))
    }
    /// Описание объектов, форм, меню и прав текущего пользователя.
    fn get_meta(&self) -> EngineResult<Meta> {
        Err(EngineError::not_implemented("GetMeta"))
    }
    /// Окно строк списка (§3.1).
    fn list(&self, _query: &ListQuery) -> EngineResult<ListPage> {
        Err(EngineError::not_implemented("List"))
    }
    /// Прыжок ползунком в произвольное место списка по вехам (ADR-014): позиция от 0 до 1.
    fn seek(&self, _list_token: &str, _position: f64, _window_size: u32) -> EngineResult<ListPage> {
        Err(EngineError::not_implemented("Seek"))
    }
    fn get(&self, _object: &str, _id: u64) -> EngineResult<ObjectData> {
        Err(EngineError::not_implemented("Get"))
    }
    /// Запись объекта; `data.version` сверяется с хранимой (иначе `VersionConflict`).
    fn save(&self, _object: &str, _data: &ObjectData) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("Save"))
    }
    /// Пометка на удаление (СТ-30: «удаление» это пометка).
    fn delete(&self, _object: &str, _id: u64, _version: u64) -> EngineResult<()> {
        Err(EngineError::not_implemented("Delete"))
    }
    fn post(&self, _document: &str, _id: u64) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("Post"))
    }
    fn unpost(&self, _document: &str, _id: u64) -> EngineResult<Saved> {
        Err(EngineError::not_implemented("Unpost"))
    }
    fn run_report(&self, _query: &ReportQuery) -> EngineResult<ListPage> {
        Err(EngineError::not_implemented("RunReport"))
    }
    /// Запуск импорта или экспорта: ход приходит как фоновая задача.
    fn start_import(&self, _format: &str, _source: &str) -> EngineResult<JobId> {
        Err(EngineError::not_implemented("Import"))
    }
    fn start_export(&self, _format: &str, _target: &str) -> EngineResult<JobId> {
        Err(EngineError::not_implemented("Export"))
    }
    // Администрирование (экран «Пользователи и роли», журнал, копия базы).
    fn admin_users(&self) -> EngineResult<Vec<UserInfo>> {
        Err(EngineError::not_implemented("AdminUsers"))
    }
    fn admin_add_user(&self, _user: &NewUser) -> EngineResult<()> {
        Err(EngineError::not_implemented("AdminAddUser"))
    }
    /// Включает или отключает пользователя; последнего активного администратора отключить нельзя.
    fn admin_set_user_active(&self, _login: &str, _active: bool) -> EngineResult<()> {
        Err(EngineError::not_implemented("AdminSetUserActive"))
    }
    fn admin_journal(&self, _window: &Window) -> EngineResult<Vec<JournalEntry>> {
        Err(EngineError::not_implemented("AdminJournal"))
    }
    fn admin_backup(&self, _target: &str) -> EngineResult<JobId> {
        Err(EngineError::not_implemented("AdminBackup"))
    }
}

// ---------------------------------------------------------------- подключение

/// Параметры создания администратора вместе с базой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdmin {
    pub login: String,
    pub password: String,
}

/// Как открыть или создать базу. Возвращает готовый `Engine`: интерфейс дальше не знает,
/// что за ним, ядро в процессе или сервер.
pub trait Connector: Send + Sync {
    /// Открывает файл локальной базы (режим 1); `file_password` нужен зашифрованной базе.
    fn open_local(
        &self,
        _path: &str,
        _file_password: Option<&str>,
    ) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("OpenLocal"))
    }
    /// Создаёт файл локальной базы; администратор нужен, если включены пользователи.
    fn create_local(
        &self,
        _path: &str,
        _file_password: Option<&str>,
        _admin: Option<&NewAdmin>,
    ) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("CreateLocal"))
    }
    /// Соединяется с сервером (режим 2): TLS, проверка версии протокола.
    fn connect_remote(&self, _address: &str) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("ConnectRemote"))
    }
    /// Создаёт базу на сервере вместе с администратором.
    fn create_remote(
        &self,
        _address: &str,
        _database: &str,
        _admin: &NewAdmin,
    ) -> EngineResult<Arc<dyn Engine>> {
        Err(EngineError::not_implemented("CreateRemote"))
    }
}

/// Заглушка: все команды отвечают «не реализовано». Подставляется, пока ядра нет.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubEngine;

impl Engine for StubEngine {}

#[derive(Debug, Default, Clone, Copy)]
pub struct StubConnector;

impl Connector for StubConnector {}

// ---------------------------------------------------------------- фоновый вызов

/// Выполняет вызов ядра в фоновом потоке и отдаёт результат в поток интерфейса, где его можно
/// безопасно положить в свойства окна. Так отрисовка не блокируется долгим запросом (§3.1).
///
/// ```ignore
/// let engine = engine.clone();
/// run_in_background(app.as_weak(), move || engine.list(&query), |app, result| {
///     match result {
///         Ok(page) => { /* показать строки */ }
///         Err(error) => { /* показать error.message */ }
///     }
/// });
/// ```
pub fn run_in_background<T, C, F, D>(window: slint::Weak<C>, work: F, done: D)
where
    T: Send + 'static,
    C: slint::ComponentHandle + 'static,
    F: FnOnce() -> EngineResult<T> + Send + 'static,
    D: FnOnce(C, EngineResult<T>) + Send + 'static,
{
    std::thread::spawn(move || {
        let result = work();
        // Если окно уже закрыто, результат некому показывать.
        let _ = window.upgrade_in_event_loop(move |window| done(window, result));
    });
}
