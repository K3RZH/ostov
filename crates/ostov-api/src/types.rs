use std::collections::BTreeMap;

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
}

/// Метаданные — те же типы, что строит `ostov-meta` из конфигурации: одно описание на всех.
pub use ostov_model::{
    FieldDef, FieldKind, MenuItem, Meta, ObjectKind, ObjectMeta, Rights, TablePartDef,
};

// ---------------------------------------------------------------- списки

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    /// Поля строки плюс служебные: у документов `number`, `posted`, у всех `deleted`.
    pub values: Fields,
}

/// Окно списка (Дизайн §3.1): сортировка, поиск и отбор идут в базе.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    pub object: String,
    /// Поле сортировки; `None` — по id. Пустые значения всегда в конце.
    pub order: Option<String>,
    pub descending: bool,
    /// Подстрока по текстовым полям.
    pub search: Option<String>,
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
