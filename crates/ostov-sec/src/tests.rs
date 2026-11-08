//! Тесты крейта. Хранение идёт на SQLite в памяти и повторяется на PostgreSQL, если задан
//! `OSTOV_PG_URL` (каждый тест в своей схеме). Хеши здесь облегчённые (`HashParams::LIGHT`).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;

use ostov_model::{Rights, RoleDef, SystemRights};
use ostov_store::Driver;
use ostov_store::drivers::postgres::PostgresDriver;
use ostov_store::drivers::sqlite::SqliteDriver;

use crate::password::{HashParams, hash_password, verify_password};
use crate::{
    NewUser, Password, SecError, Sessions, User, add_user, create_tables, delay_secs, has_users,
    list_users, rights, set_active, set_password, set_role, system_rights, verify,
};

const GOOD_PASSWORD: &str = "правильный-пароль";

/// Чистые базы с таблицей пользователей: SQLite и, если задан `OSTOV_PG_URL`, PostgreSQL.
fn bases(test: &str) -> Vec<Box<dyn Driver>> {
    let mut bases: Vec<Box<dyn Driver>> = vec![Box::new(SqliteDriver::open_in_memory().unwrap())];
    if let Ok(url) = std::env::var("OSTOV_PG_URL") {
        let schema = format!("ostov_sec_{test}");
        let mut db = PostgresDriver::connect(&url).unwrap();
        db.execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
            .unwrap();
        db.execute(&format!("CREATE SCHEMA {schema}"), &[]).unwrap();
        db.execute(&format!("SET search_path TO {schema}"), &[])
            .unwrap();
        bases.push(Box::new(db));
    }
    for db in &mut bases {
        create_tables(db.as_mut()).unwrap();
    }
    bases
}

/// Роли из настоящего `config/roles` плюс `manager` — не встроенная роль с правом `admin`.
fn roles() -> Vec<RoleDef> {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let mut roles = ostov_meta::load(&config).unwrap().roles;
    roles.push(RoleDef {
        name: "manager".into(),
        title: "Управляющий".into(),
        objects: Vec::new(),
        system: SystemRights {
            admin: true,
            ..SystemRights::default()
        },
    });
    roles
}

fn new_user(login: &str, role: &str) -> NewUser {
    NewUser {
        login: login.into(),
        display_name: format!("Пользователь {login}"),
        role: role.into(),
        password: GOOD_PASSWORD.into(),
    }
}

fn add(db: &mut dyn Driver, login: &str, role: &str) -> User {
    add_user(db, &new_user(login, role), &roles()).unwrap()
}

fn invalid(result: Result<User, SecError>) -> String {
    match result {
        Err(SecError::Invalid(reason)) => reason,
        other => panic!("ожидалась ошибка проверки: {other:?}"),
    }
}

// ---------------------------------------------------------------- пароли

#[test]
fn password_hash_is_argon2id_phc() {
    let password = Password::new(GOOD_PASSWORD);
    let phc = hash_password(&password, HashParams::LIGHT).unwrap();
    assert!(phc.starts_with("$argon2id$"), "{phc}");
    assert!(verify_password(&password, &phc));
    assert!(!verify_password(&Password::new("другой-пароль-123"), &phc));
    let again = hash_password(&password, HashParams::LIGHT).unwrap();
    assert_ne!(phc, again, "соль у каждого хеша своя");
}

#[test]
fn owasp_params_are_written_into_hash() {
    let phc = hash_password(&Password::new(GOOD_PASSWORD), HashParams::OWASP).unwrap();
    assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
}

#[test]
fn password_never_shows_in_debug() {
    let user = new_user("ivan", "storekeeper");
    let printed = format!("{user:?} {:?}", Password::new(GOOD_PASSWORD));
    assert!(!printed.contains(GOOD_PASSWORD), "{printed}");
    assert!(printed.contains("***"), "{printed}");
}

// ---------------------------------------------------------------- пользователи

#[test]
fn add_user_checks_every_field() {
    for mut db in bases("add_checks") {
        let db = db.as_mut();
        let roles = roles();
        let try_add = |db: &mut dyn Driver, change: &dyn Fn(&mut NewUser)| {
            let mut user = new_user("ivan", "storekeeper");
            change(&mut user);
            add_user(db, &user, &roles)
        };
        let login_rule = "логин: от 3 до 64 символов";
        for login in ["iv", &"a".repeat(65), "иван", "ivan petrov", "ivan@shop"] {
            let reason = invalid(try_add(db, &|u| u.login = login.into()));
            assert!(reason.contains(login_rule), "{login}: {reason}");
        }
        let reason = invalid(try_add(db, &|u| u.display_name = "  ".into()));
        assert_eq!(reason, "имя пользователя не может быть пустым");
        let reason = invalid(try_add(db, &|u| u.role = "cashier".into()));
        assert_eq!(reason, "нет роли «cashier»");
        for password in ["коротко".to_owned(), "x".repeat(1025)] {
            let reason = invalid(try_add(db, &|u| u.password = password.as_str().into()));
            assert_eq!(reason, "пароль: от 12 до 1024 символов");
        }
        assert!(
            !has_users(db).unwrap(),
            "ни один неверный пользователь не записан"
        );
    }
}

#[test]
fn login_is_lowercased_and_unique() {
    for mut db in bases("login_unique") {
        let db = db.as_mut();
        assert!(!has_users(db).unwrap());
        let ivan = add(db, " Ivan.Petrov_2-x ", "storekeeper");
        assert_eq!(ivan.login, "ivan.petrov_2-x");
        assert!(ivan.active);
        assert!(has_users(db).unwrap());
        let reason = invalid(add_user(
            db,
            &new_user("IVAN.PETROV_2-X", "admin"),
            &roles(),
        ));
        assert_eq!(reason, "логин «ivan.petrov_2-x» уже занят");
        add(db, "anna", "admin");
        let logins: Vec<String> = list_users(db)
            .unwrap()
            .into_iter()
            .map(|u| u.login)
            .collect();
        assert_eq!(logins, ["anna", "ivan.petrov_2-x"]);
    }
}

#[test]
fn last_admin_cannot_be_disabled_or_demoted() {
    for mut db in bases("last_admin") {
        let db = db.as_mut();
        let roles = roles();
        add(db, "boss", "admin");
        add(db, "ivan", "storekeeper");
        assert!(matches!(
            set_active(db, "boss", false, &roles),
            Err(SecError::LastAdmin)
        ));
        assert!(matches!(
            set_role(db, "boss", "storekeeper", &roles),
            Err(SecError::LastAdmin)
        ));
        assert_eq!(
            SecError::LastAdmin.to_string(),
            "нельзя отключить последнего администратора"
        );
        // Не встроенная роль с правом admin тоже администратор.
        set_role(db, "boss", "manager", &roles).unwrap();
        assert!(matches!(
            set_active(db, "boss", false, &roles),
            Err(SecError::LastAdmin)
        ));
        // Обычного пользователя отключать можно.
        let ivan = set_active(db, "ivan", false, &roles).unwrap();
        assert!(!ivan.active);
        assert_eq!(ivan.version, 2, "правка поднимает версию");
    }
}

#[test]
fn one_of_two_admins_can_go() {
    for mut db in bases("two_admins") {
        let db = db.as_mut();
        let roles = roles();
        add(db, "boss", "admin");
        add(db, "deputy", "manager");
        let boss = set_active(db, "boss", false, &roles).unwrap();
        assert!(!boss.active);
        assert!(matches!(
            set_role(db, "deputy", "storekeeper", &roles),
            Err(SecError::LastAdmin)
        ));
        // Отключённый администратор не в счёт, но его можно вернуть.
        set_active(db, "boss", true, &roles).unwrap();
        set_role(db, "deputy", "storekeeper", &roles).unwrap();
    }
}

#[test]
fn changes_of_missing_user_are_reported() {
    for mut db in bases("missing_user") {
        let db = db.as_mut();
        let error = set_active(db, "ghost", false, &roles()).unwrap_err();
        assert_eq!(error.to_string(), "пользователь «ghost» не найден");
        let error = set_role(db, "ghost", "manager", &roles()).unwrap_err();
        assert!(matches!(error, SecError::NotFound { .. }), "{error:?}");
        let reason = invalid(set_role(db, "ghost", "cashier", &roles()));
        assert_eq!(reason, "нет роли «cashier»");
    }
}

// ---------------------------------------------------------------- вход

#[test]
fn delay_grows_after_five_failures_and_caps() {
    let delays: Vec<u64> = (0..=12).map(delay_secs).collect();
    assert_eq!(delays, [0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 30, 30, 30]);
    assert_eq!(delay_secs(i64::MAX), 30);
}

#[test]
fn wrong_passwords_slow_down_until_success() {
    for mut db in bases("login_delay") {
        let db = db.as_mut();
        add(db, "ivan", "storekeeper");
        let wrong = Password::new("неправильный-пароль");
        let delays: Vec<u64> = (0..7)
            .map(|_| verify(db, "ivan", &wrong).unwrap_err().delay_secs())
            .collect();
        assert_eq!(delays, [0, 0, 0, 0, 1, 2, 4]);
        let user = verify(db, "IVAN", &GOOD_PASSWORD.into()).unwrap();
        assert_eq!(user.login, "ivan");
        assert_eq!(user.version, 1, "ошибки входа версию не меняют");
        let error = verify(db, "ivan", &wrong).unwrap_err();
        assert_eq!(error.delay_secs(), 0, "успешный вход обнуляет счётчик");
    }
}

#[test]
fn unknown_login_and_wrong_password_look_the_same() {
    for mut db in bases("same_error") {
        let db = db.as_mut();
        add(db, "ivan", "storekeeper");
        let unknown = verify(db, "nobody", &GOOD_PASSWORD.into()).unwrap_err();
        let wrong = verify(db, "ivan", &"неправильный-пароль".into()).unwrap_err();
        assert_eq!(unknown.to_string(), "неверный логин или пароль");
        assert_eq!(unknown.to_string(), wrong.to_string());
        assert!(matches!(
            unknown,
            SecError::WrongCredentials { delay_secs: 0 }
        ));
    }
}

#[test]
fn disabled_user_cannot_log_in() {
    for mut db in bases("disabled") {
        let db = db.as_mut();
        add(db, "boss", "admin");
        add(db, "ivan", "storekeeper");
        set_active(db, "ivan", false, &roles()).unwrap();
        let error = verify(db, "ivan", &GOOD_PASSWORD.into()).unwrap_err();
        assert_eq!(error.to_string(), "учётная запись отключена");
        // С неверным паролем отключённый получает обычный ответ: так не узнать, что он отключён.
        let error = verify(db, "ivan", &"неправильный-пароль".into()).unwrap_err();
        assert!(
            matches!(error, SecError::WrongCredentials { .. }),
            "{error:?}"
        );
    }
}

#[test]
fn new_password_replaces_old_and_resets_failures() {
    for mut db in bases("set_password") {
        let db = db.as_mut();
        add(db, "ivan", "storekeeper");
        let wrong = Password::new("неправильный-пароль");
        for _ in 0..6 {
            let _ = verify(db, "ivan", &wrong);
        }
        let reason = invalid(set_password(db, "ivan", &"короткий".into()));
        assert_eq!(reason, "пароль: от 12 до 1024 символов");
        let user = set_password(db, "ivan", &"новый-длинный-пароль".into()).unwrap();
        assert_eq!(user.version, 2);
        assert!(verify(db, "ivan", &GOOD_PASSWORD.into()).is_err());
        verify(db, "ivan", &"новый-длинный-пароль".into()).unwrap();
        assert_eq!(verify(db, "ivan", &wrong).unwrap_err().delay_secs(), 0);
    }
}

// ---------------------------------------------------------------- права

#[test]
fn rights_come_from_role_yaml() {
    let roles = roles();
    let storekeeper = |object| rights(&roles, "storekeeper", object);
    assert_eq!(
        storekeeper("goods"),
        Rights {
            read: true,
            create: true,
            update: true,
            delete: true,
            ..Rights::default()
        }
    );
    assert_eq!(
        storekeeper("warehouses"),
        Rights {
            read: true,
            ..Rights::default()
        }
    );
    assert!(storekeeper("issue").post && !storekeeper("issue").delete);
    assert_eq!(
        storekeeper("stock"),
        Rights::default(),
        "объекта нет в роли — прав нет"
    );
    assert_eq!(
        system_rights(&roles, "storekeeper"),
        SystemRights {
            closed_period: true,
            ..SystemRights::default()
        }
    );
    assert_eq!(rights(&roles, "cashier", "goods"), Rights::default());
}

#[test]
fn admin_role_gets_everything() {
    let roles = roles();
    assert_eq!(rights(&roles, "admin", "goods"), Rights::ALL);
    assert_eq!(rights(&roles, "admin", "anything"), Rights::ALL);
    assert_eq!(system_rights(&roles, "admin"), SystemRights::ALL);
}

// ---------------------------------------------------------------- сессии

fn some_user() -> User {
    User {
        id: 1,
        login: "ivan".into(),
        display_name: "Иван".into(),
        role: "storekeeper".into(),
        active: true,
        version: 1,
    }
}

#[test]
fn session_token_is_64_random_hex_chars() {
    let mut sessions = Sessions::default();
    let first = sessions.open(some_user(), 0).unwrap();
    let second = sessions.open(some_user(), 0).unwrap();
    assert_eq!(first.len(), 64);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "{first}");
    assert_ne!(first, second);
}

#[test]
fn session_expires_after_idle_time() {
    let mut sessions = Sessions::new(100, 1_000);
    let token = sessions.open(some_user(), 0).unwrap();
    assert_eq!(sessions.touch(&token, 99), Some(some_user()));
    assert!(sessions.touch(&token, 198).is_some(), "touch продлевает");
    assert_eq!(sessions.touch(&token, 298), None);
    assert_eq!(sessions.touch(&token, 299), None, "истёкшая закрыта");
}

#[test]
fn session_expires_after_absolute_time_even_if_active() {
    let mut sessions = Sessions::default();
    let token = sessions.open(some_user(), 0).unwrap();
    let step = 20 * 60;
    let mut now = 0;
    while now + step < 12 * 60 * 60 {
        now += step;
        assert!(sessions.touch(&token, now).is_some(), "активна на {now}");
    }
    assert_eq!(sessions.touch(&token, 12 * 60 * 60), None);
}

#[test]
fn closed_session_is_gone() {
    let mut sessions = Sessions::default();
    let token = sessions.open(some_user(), 0).unwrap();
    sessions.close(&token);
    assert_eq!(sessions.touch(&token, 1), None);
    sessions.close("нет-такого-токена");
}

#[test]
fn disabled_user_loses_all_sessions() {
    let mut sessions = Sessions::default();
    let first = sessions.open(some_user(), 0).unwrap();
    let second = sessions.open(some_user(), 0).unwrap();
    let anna = User {
        login: "anna".into(),
        ..some_user()
    };
    let other = sessions.open(anna, 0).unwrap();
    sessions.close_user("ivan");
    assert_eq!(sessions.touch(&first, 1), None);
    assert_eq!(sessions.touch(&second, 1), None);
    assert!(sessions.touch(&other, 1).is_some());
}
