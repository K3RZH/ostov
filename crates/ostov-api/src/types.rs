use std::collections::BTreeMap;
use std::fmt;

// ---------------------------------------------------------------- значения

/// Значение поля. Деньги и количества — целое в минимальных единицах и число знаков,
/// без float: интерфейс сам форматирует `units = 1250, scale = 2` как «12,50».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
    Decimal {
        units: i64,
        scale: u8,
    },
    Bool(bool),
    /// Дата `ГГГГ-ММ-ДД`.
    Date(String),
    /// Ссылка на элемент справочника или документ. `title` — его представление
    /// («Мельница»): ядро заполняет его в ответах, в запросах он не нужен.
    Ref {
        id: i64,
        title: Option<String>,
    },
}

pub type Fields = BTreeMap<String, Value>;

// ---------------------------------------------------------------- сессия и метаданные

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloInfo {
    pub protocol_version: u32,
    pub config_name: String,
    pub config_version: String,
    pub config_format: u32,
    pub login_required: bool,
    pub first_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub login: String,
    pub display_name: String,
    pub roles: Vec<String>,
    /// Системные права роли: по `system.admin` интерфейс показывает «Администрирование».
    pub system: SystemRights,
}

/// Что видно в файле базы без записи в него: для «Добавить существующую».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseInfo {
    /// Имя конфигурации (`name` манифеста), под которую создана база.
    pub config_name: String,
    /// Её название для людей.
    pub config_title: String,
    pub file_password: bool,
    /// В базе есть пользователи: после открытия нужен вход.
    pub login_required: bool,
}

/// Метаданные — те же типы, что строит `ostov-meta` из конфигурации: одно описание на всех.
pub use ostov_model::{
    FieldDef, FieldKind, MenuItem, Meta, ObjectKind, ObjectMeta, RegisterMeta, Rights, RoleInfo,
    SystemRights, TablePartDef,
};

// ---------------------------------------------------------------- списки

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    /// Поля строки плюс служебные: у документов `number`, `posted`, у всех `deleted` и `version`.
    /// `posted` и `deleted` — `Value::Bool`.
    pub values: Fields,
}

/// Ключ сортировки списка после первого.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SortKey {
    pub field: String,
    pub descending: bool,
}

/// Окно списка (Дизайн §3.1): сортировка, поиск и отбор идут в базе.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    pub object: String,
    /// Поле сортировки; `None` — по id. Пустые значения всегда в конце.
    pub order: Option<String>,
    pub descending: bool,
    /// Следующие ключи сортировки по порядку важности; без `order` нельзя.
    pub then_by: Vec<SortKey>,
    /// Подстрока по текстовым полям.
    pub search: Option<String>,
    /// Отбор: только проведённые (`Some(true)`) или только черновики (`Some(false)`).
    pub posted: Option<bool>,
    /// Отбор: только помеченные на удаление или только непомеченные.
    pub deleted: Option<bool>,
    /// Точный отбор: поле → значение, условия через «и». Текст — без учёта регистра,
    /// `Null` — пустое поле, ссылка — по id.
    pub filter: Fields,
    /// Ключ из прошлой страницы; `None` — с начала.
    pub after_key: Option<String>,
    /// Сколько строк, от 1 до 1000 (СТ-51).
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub rows: Vec<Row>,
    /// Непрозрачный ключ следующей страницы; `None` — конец списка.
    pub next_key: Option<String>,
    /// Число строк: на первой странице и после `seek`, дальше `None`.
    pub total: Option<u64>,
    /// Маркер списка (порядок + поиск) для `seek`.
    pub token: String,
}

// ---------------------------------------------------------------- объекты

/// Справочник или документ целиком. Новый объект — `id: None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectData {
    pub id: Option<i64>,
    pub version: i64,
    /// Номер документа; назначает ядро при первой записи.
    pub number: Option<i64>,
    pub posted: bool,
    pub deleted: bool,
    pub fields: Fields,
    pub tables: BTreeMap<String, Vec<Fields>>,
}

/// Что сделать с проведением при записи документа — всё одной транзакцией.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PostAction {
    /// Просто записать (проведённый документ так не меняется).
    #[default]
    Write,
    /// Записать и провести («Провести», «Провести и закрыть»).
    Post,
    /// Записать и снять с проведения.
    Unpost,
}

/// Одна запись из `Engine::save_batch`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchItem {
    pub object: String,
    pub data: ObjectData,
    pub post: PostAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub id: i64,
    pub version: i64,
    pub number: Option<i64>,
}

// ---------------------------------------------------------------- отчёты, обмен, администрирование

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportQuery {
    pub report: String,
    pub params: Fields,
    pub after_key: Option<String>,
    pub size: u32,
}

/// Ответ на запрос конфигурации (SQL через платформу, СТ-13): не больше 1000 строк.
/// Деньги — `Decimal`, даты — `Date`. Колонка, взятая прямо из поля-ссылки на справочник, —
/// `Ref` с названием; `posted` и `deleted` — `Bool`, как в списках.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// Пробное проведение (К-19): что записало бы «Провести». База не меняется.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrialPost {
    pub moves: Vec<TrialMove>,
    /// Остатки по каждому затронутому ключу регистра до и после проведения.
    pub balances: Vec<TrialBalance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialMove {
    pub register: String,
    /// `true` — приход, `false` — расход.
    pub receipt: bool,
    pub dims: Fields,
    pub resources: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialBalance {
    pub register: String,
    pub dims: Fields,
    pub before: Fields,
    pub after: Fields,
}

/// Что сделает с базой открытие с этой конфигурацией (СТ-50): база при этом не меняется.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaPlan {
    pub changes: Vec<SchemaChange>,
    /// Почему открыть нельзя: изменение потеряло бы данные. Тогда `changes` до отказа.
    pub refused: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaChange {
    /// Новый справочник, документ, табличная часть или регистр.
    NewTable { table: String },
    NewField {
        table: String,
        field: String,
        kind: String,
    },
    /// Регистр без движений создаётся заново: сменились измерения или убран ресурс.
    RebuiltRegister { register: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub active: bool,
}

/// Пароль идёт только по защищённому каналу, хэш считает ядро.
#[derive(Clone, PartialEq, Eq)]
pub struct NewUser {
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub password: String,
}

impl fmt::Debug for NewUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewUser")
            .field("login", &self.login)
            .field("display_name", &self.display_name)
            .field("role", &self.role)
            .field("password", &"***")
            .finish()
    }
}

/// Страница журнала: записи и ключ следующей; `None` — дальше записей нет.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalPage {
    pub entries: Vec<JournalEntry>,
    pub next_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Ключ страницы: следующую просить с `after_key = id` последней записи.
    pub id: i64,
    pub when: String,
    pub user: String,
    pub object: String,
    pub action: String,
    pub details: String,
}
