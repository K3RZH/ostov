//! Пользователи в таблице `ostov_users`: добавление, отключение, роль, пароль, вход (СТ-30, 32, 36).

use std::sync::OnceLock;

use ostov_model::RoleDef;
use ostov_store::{Dialect, Driver, Row, StoreError, Value};

use crate::SecError;
use crate::password::{self, HashParams, Password, PasswordPolicy};
use crate::rights::{role_exists, system_rights};

pub const USERS_TABLE: &str = "ostov_users";
const LOGIN_MIN: usize = 3;
const LOGIN_MAX: usize = 64;
/// Первые ошибки входа без задержки: опечатка — ещё не подбор пароля.
const FREE_ATTEMPTS: i64 = 5;
/// Потолок задержки: дальше она не растёт, постоянной блокировки нет (СТ-32).
const MAX_DELAY_SECS: u64 = 30;
const COLUMNS: &str = "id, login, display_name, role, active, version, weak_password";

/// Данные для `add_user`. `Debug` пароль не показывает.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub password: Password,
}

/// Пользователь без пароля: в таком виде он уходит из крейта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub active: bool,
    pub version: i64,
    /// Пароль задан на одном компьютере и короче 12 символов: на сервер с ним не войти,
    /// пока администратор не сменит пароль (ADR-033).
    pub weak_password: bool,
}

pub(crate) fn create_table(db: &mut dyn Driver) -> Result<(), SecError> {
    let d = db.dialect();
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {} ({} {}, login TEXT NOT NULL UNIQUE, \
         display_name TEXT NOT NULL, role TEXT NOT NULL, password TEXT NOT NULL, \
         active BIGINT NOT NULL, failed BIGINT NOT NULL, version BIGINT NOT NULL, \
         weak_password BIGINT NOT NULL DEFAULT 0)",
        table(d),
        d.quote_ident("id"),
        d.id_column()
    );
    db.execute(&sql, &[])?;
    if !has_weak_column(db)? {
        let sql = format!(
            "ALTER TABLE {} ADD COLUMN weak_password BIGINT NOT NULL DEFAULT 0",
            table(db.dialect())
        );
        db.execute(&sql, &[])?;
    }
    Ok(())
}

/// Таблицы до ADR-033 без колонки `weak_password`. Спрашиваем каталог, а не пробуем запрос:
/// в PostgreSQL неудачный запрос обрывает транзакцию ядра.
fn has_weak_column(db: &mut dyn Driver) -> Result<bool, SecError> {
    let sql = if db.dialect().name() == "postgres" {
        format!(
            "SELECT 1 FROM information_schema.columns WHERE table_schema = current_schema() \
             AND table_name = '{USERS_TABLE}' AND column_name = 'weak_password'"
        )
    } else {
        format!("SELECT 1 FROM pragma_table_info('{USERS_TABLE}') WHERE name = 'weak_password'")
    };
    Ok(!db.query(&sql, &[])?.is_empty())
}

/// Есть ли хоть один пользователь: по этому ядро решает, нужен ли вход.
///
/// # Errors
/// Ошибка базы.
pub fn has_users(db: &mut dyn Driver) -> Result<bool, SecError> {
    let sql = format!("SELECT 1 FROM {} LIMIT 1", table(db.dialect()));
    Ok(!db.query(&sql, &[])?.is_empty())
}

/// Добавляет активного пользователя. Логин приводится к нижнему регистру.
///
/// # Errors
/// `Invalid` — логин, имя, роль или длина пароля по `policy` не подходят, логин занят.
pub fn add_user(
    db: &mut dyn Driver,
    user: &NewUser,
    roles: &[RoleDef],
    policy: PasswordPolicy,
) -> Result<User, SecError> {
    let login = normalize_login(&user.login)?;
    let display_name = user.display_name.trim();
    if display_name.is_empty() {
        return Err(SecError::Invalid(
            "имя пользователя не может быть пустым".into(),
        ));
    }
    if display_name.contains('\0') {
        return Err(SecError::Invalid(
            "имя пользователя: нельзя символ с кодом 0".into(),
        ));
    }
    check_role(&user.role, roles)?;
    user.password.check_length(policy)?;
    if find(db, &login, false)?.is_some() {
        return Err(SecError::Invalid(format!("логин «{login}» уже занят")));
    }
    let hash = password::hash_password(&user.password, HashParams::current())?;
    let d = db.dialect();
    let placeholders: Vec<String> = (1..=8).map(|n| d.placeholder(n)).collect();
    let sql = format!(
        "INSERT INTO {} (login, display_name, role, password, active, failed, version, \
         weak_password) VALUES ({})",
        table(d),
        placeholders.join(", ")
    );
    let values = [
        Value::from(login.as_str()),
        display_name.into(),
        user.role.as_str().into(),
        hash.into(),
        Value::Int(1),
        Value::Int(0),
        Value::Int(1),
        Value::Int(i64::from(user.password.is_weak())),
    ];
    db.execute(&sql, &values)?;
    get(db, &login, false)
}

/// Все пользователи по логину, без паролей.
///
/// # Errors
/// Ошибка базы.
pub fn list_users(db: &mut dyn Driver) -> Result<Vec<User>, SecError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM {} ORDER BY login",
        table(db.dialect())
    );
    db.query(&sql, &[])?
        .into_iter()
        .map(user_from_row)
        .collect()
}

/// Включает или отключает пользователя. Удалять нельзя: по нему будет история в журнале.
///
/// # Errors
/// `LastAdmin` — это последний активный администратор; `NotFound`; `VersionConflict`.
pub fn set_active(
    db: &mut dyn Driver,
    login: &str,
    active: bool,
    roles: &[RoleDef],
) -> Result<User, SecError> {
    let user = get(db, login, true)?;
    if !active
        && user.active
        && is_admin(roles, &user.role)
        && !other_admin_exists(db, &user, roles)?
    {
        return Err(SecError::LastAdmin);
    }
    update(db, &user, &[("active", Value::Int(i64::from(active)))])
}

/// Меняет роль.
///
/// # Errors
/// `Invalid` — нет такой роли; `LastAdmin` — роль без `admin` у последнего администратора.
pub fn set_role(
    db: &mut dyn Driver,
    login: &str,
    role: &str,
    roles: &[RoleDef],
) -> Result<User, SecError> {
    check_role(role, roles)?;
    let user = get(db, login, true)?;
    let demoted = is_admin(roles, &user.role) && !is_admin(roles, role);
    if user.active && demoted && !other_admin_exists(db, &user, roles)? {
        return Err(SecError::LastAdmin);
    }
    update(db, &user, &[("role", role.into())])
}

/// Новый пароль; счётчик ошибок входа обнуляется. Старый пароль проверяет ядро через `verify`.
///
/// # Errors
/// `Invalid` — длина пароля по `policy`; `NotFound`; `VersionConflict`.
pub fn set_password(
    db: &mut dyn Driver,
    login: &str,
    new: &Password,
    policy: PasswordPolicy,
) -> Result<User, SecError> {
    new.check_length(policy)?;
    let user = get(db, login, true)?;
    let hash = password::hash_password(new, HashParams::current())?;
    update(
        db,
        &user,
        &[
            ("password", hash.into()),
            ("failed", Value::Int(0)),
            ("weak_password", Value::Int(i64::from(new.is_weak()))),
        ],
    )
}

/// Вход по логину и паролю.
///
/// # Errors
/// `WrongCredentials` — одна ошибка на неизвестный логин и неверный пароль, с задержкой
/// после пяти неудач подряд; `Disabled` — пароль верный, но учётная запись отключена.
pub fn verify(db: &mut dyn Driver, login: &str, password: &Password) -> Result<User, SecError> {
    let login = login.trim().to_lowercase();
    let Some((user, phc, failed)) = find_with_secret(db, &login)? else {
        // Хеш всё равно считаем: по времени ответа нельзя понять, есть ли такой логин.
        password::verify_password(password, dummy_hash());
        return Err(SecError::WrongCredentials { delay_secs: 0 });
    };
    if !password::verify_password(password, &phc) {
        let delay_secs = delay_secs(count_failure(db, user.id)?);
        return Err(SecError::WrongCredentials { delay_secs });
    }
    if failed != 0 {
        reset_failed(db, user.id)?;
    }
    if !user.active {
        return Err(SecError::Disabled);
    }
    Ok(user)
}

/// 0 при `failed < 5`, дальше 1, 2, 4, 8, 16, 30, 30… секунд.
pub fn delay_secs(failed: i64) -> u64 {
    if failed < FREE_ATTEMPTS {
        return 0;
    }
    u32::try_from(failed - FREE_ATTEMPTS)
        .ok()
        .and_then(|over| 2u64.checked_pow(over))
        .map_or(MAX_DELAY_SECS, |delay| delay.min(MAX_DELAY_SECS))
}

fn normalize_login(raw: &str) -> Result<String, SecError> {
    let login = raw.trim().to_lowercase();
    let allowed =
        |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-');
    if (LOGIN_MIN..=LOGIN_MAX).contains(&login.chars().count()) && login.chars().all(allowed) {
        Ok(login)
    } else {
        Err(SecError::Invalid(format!(
            "логин: от {LOGIN_MIN} до {LOGIN_MAX} символов — латинские буквы, цифры, «.», «_» и «-»"
        )))
    }
}

fn check_role(role: &str, roles: &[RoleDef]) -> Result<(), SecError> {
    if role_exists(roles, role) {
        Ok(())
    } else {
        Err(SecError::Invalid(format!("нет роли «{role}»")))
    }
}

fn is_admin(roles: &[RoleDef], role: &str) -> bool {
    system_rights(roles, role).admin
}

/// Есть ли ещё активный администратор, кроме `user`. Строки блокируются до конца транзакции
/// ядра: два администратора не отключат друг друга одновременно.
fn other_admin_exists(
    db: &mut dyn Driver,
    user: &User,
    roles: &[RoleDef],
) -> Result<bool, SecError> {
    let d = db.dialect();
    let sql = format!(
        "SELECT role FROM {} WHERE active = 1 AND id <> {}{}",
        table(d),
        d.placeholder(1),
        d.lock_rows()
    );
    let rows = db.query(&sql, &[Value::Int(user.id)])?;
    for row in rows {
        if is_admin(roles, &text(row.into_iter().next(), "role")?) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Меняет колонки с проверкой версии (как объекты ядра, ADR-023).
fn update(db: &mut dyn Driver, user: &User, columns: &[(&str, Value)]) -> Result<User, SecError> {
    let d = db.dialect();
    let assignments: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, (column, _))| format!("{column} = {}", d.placeholder(i + 1)))
        .collect();
    let n = columns.len();
    let sql = format!(
        "UPDATE {} SET {}, version = version + 1 WHERE id = {} AND version = {}",
        table(d),
        assignments.join(", "),
        d.placeholder(n + 1),
        d.placeholder(n + 2)
    );
    let mut values: Vec<Value> = columns.iter().map(|(_, value)| value.clone()).collect();
    values.extend([Value::Int(user.id), Value::Int(user.version)]);
    if db.execute(&sql, &values)? == 0 {
        return Err(SecError::VersionConflict {
            login: user.login.clone(),
        });
    }
    get(db, &user.login, false)
}

/// +1 к ошибкам входа и их число после этого. Прибавляет сам SQL: одновременные попытки
/// не теряются. Счётчик — не правка администратора, версию не меняет.
fn count_failure(db: &mut dyn Driver, id: i64) -> Result<i64, SecError> {
    let d = db.dialect();
    // RETURNING есть и в PostgreSQL, и в SQLite с 3.35.
    let sql = format!(
        "UPDATE {} SET failed = failed + 1 WHERE id = {} RETURNING failed",
        table(d),
        d.placeholder(1)
    );
    let row = db.query(&sql, &[Value::Int(id)])?.into_iter().next();
    int(row.and_then(|row| row.into_iter().next()), "failed")
}

fn reset_failed(db: &mut dyn Driver, id: i64) -> Result<(), SecError> {
    let d = db.dialect();
    let sql = format!(
        "UPDATE {} SET failed = 0 WHERE id = {}",
        table(d),
        d.placeholder(1)
    );
    db.execute(&sql, &[Value::Int(id)])?;
    Ok(())
}

fn get(db: &mut dyn Driver, login: &str, lock: bool) -> Result<User, SecError> {
    let login = login.trim().to_lowercase();
    find(db, &login, lock)?.ok_or(SecError::NotFound { login })
}

fn find(db: &mut dyn Driver, login: &str, lock: bool) -> Result<Option<User>, SecError> {
    let d = db.dialect();
    let sql = format!(
        "SELECT {COLUMNS} FROM {} WHERE login = {}{}",
        table(d),
        d.placeholder(1),
        if lock { d.lock_rows() } else { "" }
    );
    let row = db.query(&sql, &[login.into()])?.into_iter().next();
    row.map(user_from_row).transpose()
}

/// Пользователь, хеш его пароля и число ошибок входа подряд.
fn find_with_secret(
    db: &mut dyn Driver,
    login: &str,
) -> Result<Option<(User, String, i64)>, SecError> {
    let d = db.dialect();
    let sql = format!(
        "SELECT {COLUMNS}, password, failed FROM {} WHERE login = {}",
        table(d),
        d.placeholder(1)
    );
    let Some(mut row) = db.query(&sql, &[login.into()])?.into_iter().next() else {
        return Ok(None);
    };
    let failed = int(row.pop(), "failed")?;
    let phc = text(row.pop(), "password")?;
    Ok(Some((user_from_row(row)?, phc, failed)))
}

fn user_from_row(row: Row) -> Result<User, SecError> {
    let mut values = row.into_iter();
    Ok(User {
        id: int(values.next(), "id")?,
        login: text(values.next(), "login")?,
        display_name: text(values.next(), "display_name")?,
        role: text(values.next(), "role")?,
        active: int(values.next(), "active")? != 0,
        version: int(values.next(), "version")?,
        weak_password: int(values.next(), "weak_password")? != 0,
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
        "в {USERS_TABLE} неожиданное значение колонки {column}"
    )))
}

fn table(d: &dyn Dialect) -> String {
    d.quote_ident(USERS_TABLE)
}

/// Хеш, с которым сверяется пароль неизвестного логина. Считается один раз, с теми же
/// параметрами, что настоящие, чтобы время проверки совпадало.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        password::hash_password(
            &Password::new("ostov-dummy-password"),
            HashParams::current(),
        )
        .unwrap_or_default()
    })
}
