//! Журнал изменений `ostov_journal` (СТ-34): запись и чтение страницами. Изменить или удалить
//! запись нельзя — таких функций нет. Пароли в журнал не попадают (СТ-37).

use ostov_store::{Dialect, Driver, Row, StoreError, Value};

use crate::SecError;

pub const JOURNAL_TABLE: &str = "ostov_journal";
pub const MAX_PAGE_SIZE: u32 = 1000;
/// Длинная правка не раздувает журнал: дальше текст обрезается с «…».
const MAX_DETAILS_CHARS: usize = 4000;
const EMPTY_VALUE: &str = "—";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    MarkDeleted,
    Unmark,
    Post,
    Unpost,
    Login,
    LoginFailed,
    UserAdded,
    UserEnabled,
    UserDisabled,
    RoleChanged,
    PasswordChanged,
    ClosingDateSet,
    BackupRestored,
    ConfigUpdated,
}

impl Action {
    pub const ALL: [Self; 16] = [
        Self::Create,
        Self::Update,
        Self::MarkDeleted,
        Self::Unmark,
        Self::Post,
        Self::Unpost,
        Self::Login,
        Self::LoginFailed,
        Self::UserAdded,
        Self::UserEnabled,
        Self::UserDisabled,
        Self::RoleChanged,
        Self::PasswordChanged,
        Self::ClosingDateSet,
        Self::BackupRestored,
        Self::ConfigUpdated,
    ];

    /// Код в колонке `action`.
    pub fn code(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::MarkDeleted => "mark_deleted",
            Self::Unmark => "unmark",
            Self::Post => "post",
            Self::Unpost => "unpost",
            Self::Login => "login",
            Self::LoginFailed => "login_failed",
            Self::UserAdded => "user_added",
            Self::UserEnabled => "user_enabled",
            Self::UserDisabled => "user_disabled",
            Self::RoleChanged => "role_changed",
            Self::PasswordChanged => "password_changed",
            Self::ClosingDateSet => "closing_date_set",
            Self::BackupRestored => "backup_restored",
            Self::ConfigUpdated => "config_updated",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Create => "Создание",
            Self::Update => "Изменение",
            Self::MarkDeleted => "Пометка на удаление",
            Self::Unmark => "Снятие пометки на удаление",
            Self::Post => "Проведение",
            Self::Unpost => "Отмена проведения",
            Self::Login => "Вход",
            Self::LoginFailed => "Неудачный вход",
            Self::UserAdded => "Добавление пользователя",
            Self::UserEnabled => "Включение пользователя",
            Self::UserDisabled => "Отключение пользователя",
            Self::RoleChanged => "Смена роли",
            Self::PasswordChanged => "Смена пароля",
            Self::ClosingDateSet => "Установка даты закрытия",
            Self::BackupRestored => "Восстановление копии",
            Self::ConfigUpdated => "Обновление конфигурации",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.code() == code)
    }

    /// Изменения полей пишутся только при создании и правке. У остальных действий
    /// `changes` игнорируются: так пароль не попадёт в журнал даже по ошибке вызывающего.
    fn keeps_changes(self) -> bool {
        matches!(self, Self::Create | Self::Update)
    }
}

/// Изменение поля: `field` — заголовок поля, значения уже текстом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub field: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Событие для записи. Время передаёт вызывающий: модуль часы не берёт.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// UTC, `2026-11-08T12:34:56Z`.
    pub at: &'a str,
    /// `""`, если в базе нет пользователей.
    pub login: &'a str,
    pub action: Action,
    /// `""` для событий безопасности.
    pub object: &'a str,
    pub object_id: Option<i64>,
    /// Только для `Create` и `Update`.
    pub changes: &'a [Change],
    /// Пояснение для людей. Без секретов: модуль не отличит пароль от текста.
    pub note: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalQuery {
    /// Ключ из прошлой страницы; `None` — с самых новых.
    pub after_id: Option<i64>,
    /// От 1 до `MAX_PAGE_SIZE`.
    pub size: u32,
    pub object: Option<String>,
    pub object_id: Option<i64>,
    pub login: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    pub id: i64,
    pub at: String,
    pub login: String,
    /// Код действия; `action_title` — по-русски (для неизвестного кода — сам код).
    pub action: String,
    pub action_title: String,
    pub object: String,
    pub object_id: Option<i64>,
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    /// От новых к старым.
    pub rows: Vec<JournalRow>,
    /// `after_id` следующей страницы; `None` — страниц больше нет.
    pub next_after_id: Option<i64>,
}

pub(crate) fn create_table(db: &mut dyn Driver) -> Result<(), SecError> {
    let d = db.dialect();
    let table = table(d);
    let create = format!(
        "CREATE TABLE IF NOT EXISTS {table} ({} {}, at TEXT NOT NULL, login TEXT NOT NULL, \
         action TEXT NOT NULL, object TEXT NOT NULL, object_id BIGINT NULL, \
         details TEXT NOT NULL)",
        d.quote_ident("id"),
        d.id_column()
    );
    let index = format!(
        "CREATE INDEX IF NOT EXISTS {} ON {table} (object, object_id)",
        d.quote_ident("ostov_journal_object")
    );
    db.execute(&create, &[])?;
    db.execute(&index, &[])?;
    Ok(())
}

/// Добавляет запись. Транзакцию не открывает: ядро зовёт внутри своей, откат изменения
/// откатывает и запись.
///
/// # Errors
/// Ошибка базы.
pub fn record(db: &mut dyn Driver, entry: &Entry) -> Result<(), SecError> {
    let changes = if entry.action.keeps_changes() {
        entry.changes
    } else {
        &[]
    };
    let d = db.dialect();
    let placeholders: Vec<String> = (1..=6).map(|n| d.placeholder(n)).collect();
    let sql = format!(
        "INSERT INTO {} (at, login, action, object, object_id, details) VALUES ({})",
        table(d),
        placeholders.join(", ")
    );
    let values = [
        Value::from(entry.at),
        entry.login.into(),
        entry.action.code().into(),
        entry.object.into(),
        entry.object_id.into(),
        details(changes, entry.note).into(),
    ];
    db.execute(&sql, &values)?;
    Ok(())
}

/// Страница журнала от новых к старым с отбором по объекту, записи и логину.
///
/// # Errors
/// `Invalid` — размер страницы вне 1..=1000; ошибка базы.
pub fn list(db: &mut dyn Driver, query: &JournalQuery) -> Result<JournalPage, SecError> {
    if !(1..=MAX_PAGE_SIZE).contains(&query.size) {
        return Err(SecError::Invalid(format!(
            "размер страницы журнала: от 1 до {MAX_PAGE_SIZE}"
        )));
    }
    let d = db.dialect();
    let mut conditions = Vec::new();
    let mut values = Vec::new();
    let mut filter = |column: &str, value: Value| {
        values.push(value);
        let operator = if column == "id" { "<" } else { "=" };
        conditions.push(format!(
            "{column} {operator} {}",
            d.placeholder(values.len())
        ));
    };
    if let Some(after_id) = query.after_id {
        filter("id", Value::Int(after_id));
    }
    if let Some(object) = &query.object {
        filter("object", object.as_str().into());
    }
    if let Some(object_id) = query.object_id {
        filter("object_id", Value::Int(object_id));
    }
    if let Some(login) = &query.login {
        filter("login", login.as_str().into());
    }
    let condition = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    // Строкой больше: так видно, есть ли следующая страница.
    values.push(Value::Int(i64::from(query.size) + 1));
    let sql = format!(
        "SELECT id, at, login, action, object, object_id, details FROM {}{condition} \
         ORDER BY id DESC LIMIT {}",
        table(d),
        d.placeholder(values.len())
    );
    let mut rows = db
        .query(&sql, &values)?
        .into_iter()
        .map(row_from_db)
        .collect::<Result<Vec<_>, _>>()?;
    let size = usize::try_from(query.size).unwrap_or(usize::MAX);
    let next_after_id = if rows.len() > size {
        rows.truncate(size);
        rows.last().map(|row| row.id)
    } else {
        None
    };
    Ok(JournalPage {
        rows,
        next_after_id,
    })
}

/// `Количество: 2,500 → 3,000; Цена: — → 3,33` и через пробел `note`; не длиннее 4000 символов.
pub(crate) fn details(changes: &[Change], note: &str) -> String {
    let value = |v: &Option<String>| v.clone().unwrap_or_else(|| EMPTY_VALUE.to_owned());
    let mut text = changes
        .iter()
        .map(|c| format!("{}: {} → {}", c.field, value(&c.old), value(&c.new)))
        .collect::<Vec<_>>()
        .join("; ");
    if !note.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(note);
    }
    if text.chars().count() > MAX_DETAILS_CHARS {
        text = text.chars().take(MAX_DETAILS_CHARS - 1).collect();
        text.push('…');
    }
    text
}

fn row_from_db(row: Row) -> Result<JournalRow, SecError> {
    let mut values = row.into_iter();
    let id = int(values.next(), "id")?;
    let at = text(values.next(), "at")?;
    let login = text(values.next(), "login")?;
    let action = text(values.next(), "action")?;
    let object = text(values.next(), "object")?;
    let object_id = match values.next() {
        Some(Value::Null) => None,
        value => Some(int(value, "object_id")?),
    };
    let details = text(values.next(), "details")?;
    let action_title =
        Action::from_code(&action).map_or_else(|| action.clone(), |a| a.title().to_owned());
    Ok(JournalRow {
        id,
        at,
        login,
        action,
        action_title,
        object,
        object_id,
        details,
    })
}

fn int(value: Option<Value>, column: &str) -> Result<i64, SecError> {
    match value {
        Some(Value::Int(v)) => Ok(v),
        _ => Err(bad_column(column)),
    }
}

fn text(value: Option<Value>, column: &str) -> Result<String, SecError> {
    match value {
        Some(Value::Text(v)) => Ok(v),
        _ => Err(bad_column(column)),
    }
}

fn bad_column(column: &str) -> SecError {
    SecError::Store(StoreError::Database(format!(
        "в {JOURNAL_TABLE} неожиданное значение колонки {column}"
    )))
}

fn table(d: &dyn Dialect) -> String {
    d.quote_ident(JOURNAL_TABLE)
}
