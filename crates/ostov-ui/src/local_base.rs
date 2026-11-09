//! Локальная база (режим 1) через ядро: создать файл и открыть файл. Конфигурация пока одна,
//! встроенная «Склад» (`ostov_meta::demo`, та же папка `config/`, которую проверяет
//! `ostov check`); выбор папки конфигурации появится вместе с конфигуратором.

use std::path::Path;
use std::sync::Arc;

use ostov_api::{BaseInfo, Connector, Engine, EngineError, EngineResult, ErrorCode, NewAdmin};
use ostov_core::{LocalConnector, PasswordPolicy};

/// Название и версия встроенной конфигурации для стартовой страницы.
pub fn config_title() -> Option<(String, String)> {
    ostov_meta::demo()
        .ok()
        .map(|config| (config.title, config.version))
}

fn connector() -> EngineResult<LocalConnector> {
    let config = ostov_meta::demo().map_err(|errors| {
        let first = errors.first().map(ToString::to_string).unwrap_or_default();
        EngineError::new(
            ErrorCode::Incompatible,
            format!("Встроенная конфигурация не прошла проверку: {first}"),
        )
    })?;
    // Один компьютер: любой пароль, в том числе пустой (ADR-033).
    Ok(LocalConnector::new(config).with_password_policy(PasswordPolicy::LOCAL))
}

/// Что за база в этом файле, не открывая и не записывая в него (для «Добавить существующую»).
/// База другой конфигурации не подходит: программа сейчас работает только со встроенной «Склад».
///
/// # Errors
/// Ошибки ядра как есть: файла нет (`NotFound`), это не база Ostov (`Incompatible`), база другой
/// конфигурации (`Incompatible` с её названием).
pub fn check(path: &str) -> EngineResult<BaseInfo> {
    let info = connector()?.check_local(&expand_home(path))?;
    let (title, _) = config_title().unwrap_or_default();
    ensure_embedded_config(&info, &config_name(), &title)?;
    Ok(info)
}

fn config_name() -> String {
    ostov_meta::demo()
        .map(|config| config.name)
        .unwrap_or_default()
}

fn ensure_embedded_config(info: &BaseInfo, name: &str, title: &str) -> EngineResult<()> {
    if info.config_name == name {
        return Ok(());
    }
    Err(EngineError::new(
        ErrorCode::Incompatible,
        format!(
            "Это база конфигурации «{}», а программа работает с «{title}».",
            info.config_title
        ),
    ))
}

/// Открывает файл базы.
///
/// # Errors
/// Ошибки ядра как есть: файла нет, это не база Ostov, база уже открыта другой программой.
pub fn open(path: &str) -> EngineResult<Arc<dyn Engine>> {
    connector()?.open_local(&expand_home(path), None)
}

/// Создаёт файл базы; папку создаёт, если её нет. С `admin` в базе сразу появляется
/// администратор, и дальше она спрашивает вход.
///
/// # Errors
/// Файл уже есть, папку не создать, пароль на файл (ядро его пока не поддерживает, вопрос В-2).
pub fn create(
    path: &str,
    file_password: Option<&str>,
    admin: Option<&NewAdmin>,
) -> EngineResult<Arc<dyn Engine>> {
    let path = expand_home(path);
    if let Some(dir) = Path::new(&path).parent() {
        std::fs::create_dir_all(dir).map_err(|error| {
            EngineError::new(
                ErrorCode::Unavailable,
                format!("Не удалось создать папку базы: {error}."),
            )
        })?;
    }
    connector()?.create_local(&path, file_password, admin)
}

/// `~/…` превращает в путь от домашней папки: так путь показывают окна выбора.
pub fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if !home.is_empty() => {
            format!("{}/{rest}", home.trim_end_matches('/'))
        }
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(name: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("ostov-ui-{name}-{}", std::process::id()));
        let path = dir.join("Склад.sqlite").to_string_lossy().into_owned();
        (dir, path)
    }

    #[test]
    fn created_base_opens_again_without_login() {
        let (dir, path) = temp_base("open");
        let created = create(&path, None, None);
        assert!(created.is_ok(), "{:?}", created.err());
        drop(created);
        let hello = open(&path).and_then(|engine| engine.hello());
        assert!(
            matches!(hello, Ok(ref info) if !info.login_required),
            "{hello:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn info(name: &str, title: &str) -> BaseInfo {
        BaseInfo {
            config_name: name.into(),
            config_title: title.into(),
            file_password: false,
            login_required: false,
        }
    }

    #[test]
    fn other_configuration_is_refused_with_its_title() {
        assert!(ensure_embedded_config(&info("warehouse", "Склад"), "warehouse", "Склад").is_ok());
        let error = ensure_embedded_config(&info("cafe", "Кафе"), "warehouse", "Склад").err();
        assert!(
            matches!(error, Some(ref e) if e.code == ErrorCode::Incompatible
            && e.message.contains("«Кафе»") && e.message.contains("«Склад»")),
            "{error:?}"
        );
    }

    // «Добавить существующую» не должна менять чужой файл (просьба Артёма, ядро: check_local).
    #[test]
    fn check_reads_base_without_changing_the_file() {
        let (dir, path) = temp_base("check");
        assert!(create(&path, None, None).is_ok());
        let before = std::fs::read(&path).unwrap_or_default();
        let checked = check(&path);
        assert!(
            matches!(checked, Ok(ref i) if !i.login_required),
            "{checked:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap_or_default(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn check_reports_missing_and_foreign_files() {
        let (dir, path) = temp_base("check-bad");
        let missing = check(&path);
        assert!(
            matches!(missing, Err(ref e) if e.code == ErrorCode::NotFound),
            "{missing:?}"
        );
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            &path,
            "это не база данных, просто текст на много байт".repeat(50),
        );
        let foreign = check(&path);
        assert!(
            matches!(foreign, Err(ref e) if e.code == ErrorCode::Incompatible),
            "{foreign:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // ADR-033: на одном компьютере подходит любой пароль, в том числе пустой, и вход с ним работает.
    #[test]
    fn local_base_accepts_any_admin_password() {
        for (name, password) in [("short-admin", "1234"), ("empty-admin", "")] {
            let (dir, path) = temp_base(name);
            let admin = NewAdmin {
                login: "admin".into(),
                password: password.into(),
            };
            let created = create(&path, None, Some(&admin));
            assert!(created.is_ok(), "пароль «{password}»: {:?}", created.err());
            drop(created);
            let user = open(&path).and_then(|engine| engine.login("admin", password));
            assert!(matches!(user, Ok(ref u) if u.login == "admin"), "{user:?}");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    // Кнопка «Сменить пароль»: после смены администратором старый пароль не подходит, новый подходит.
    #[test]
    fn admin_changes_user_password() {
        let (dir, path) = temp_base("set-password");
        let admin = NewAdmin {
            login: "admin".into(),
            password: "1".into(),
        };
        let engine = create(&path, None, Some(&admin));
        assert!(engine.is_ok(), "{:?}", engine.err());
        if let Ok(engine) = engine {
            assert!(engine.login("admin", "1").is_ok());
            let user = ostov_api::NewUser {
                login: "keeper".into(),
                display_name: "Кладовщик".into(),
                role: "storekeeper".into(),
                password: "старый".into(),
            };
            assert!(engine.admin_add_user(&user).is_ok());
            assert!(engine.admin_set_user_password("keeper", "новый").is_ok());
            assert!(engine.admin_set_user_password("нет-такого", "x").is_err());
            assert!(engine.login("keeper", "старый").is_err());
            assert!(engine.login("keeper", "новый").is_ok());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    // Просьба Артёма №5: первым пользователем не-администратор отклоняется, вход не включается.
    #[test]
    fn first_user_must_be_administrator() {
        let (dir, path) = temp_base("first-keeper");
        let engine = create(&path, None, None);
        assert!(engine.is_ok(), "{:?}", engine.err());
        if let Ok(engine) = engine {
            let keeper = engine.admin_add_user(&ostov_api::NewUser {
                login: "keeper".into(),
                display_name: "Кладовщик".into(),
                role: "storekeeper".into(),
                password: "длинный-пароль-1".into(),
            });
            assert!(keeper.is_err(), "кладовщик первым не должен приниматься");
            assert!(matches!(engine.hello(), Ok(info) if !info.login_required));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_file_is_not_overwritten() {
        let (dir, path) = temp_base("twice");
        assert!(create(&path, None, None).is_ok());
        assert!(create(&path, None, None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn base_with_admin_asks_login_and_checks_password() {
        let (dir, path) = temp_base("admin");
        let admin = NewAdmin {
            login: "admin".into(),
            password: "длинный-пароль-1".into(),
        };
        assert!(create(&path, None, Some(&admin)).is_ok());
        let engine = open(&path);
        assert!(engine.is_ok(), "{:?}", engine.err());
        if let Ok(engine) = engine {
            assert!(matches!(engine.hello(), Ok(info) if info.login_required));
            assert!(engine.login("admin", "не тот пароль").is_err());
            let user = engine.login("admin", "длинный-пароль-1");
            assert!(
                matches!(user, Ok(ref info) if info.login == "admin"),
                "{user:?}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    // На этом держится переход к входу после первого пользователя (UsersUi::ask_login).
    #[test]
    fn first_user_turns_login_on() {
        let (dir, path) = temp_base("first-user");
        let engine = create(&path, None, None);
        assert!(engine.is_ok(), "{:?}", engine.err());
        if let Ok(engine) = engine {
            let added = engine.admin_add_user(&ostov_api::NewUser {
                login: "admin".into(),
                display_name: "Администратор".into(),
                role: "admin".into(),
                password: "длинный-пароль-1".into(),
            });
            assert!(added.is_ok(), "{added:?}");
            assert!(matches!(engine.hello(), Ok(info) if info.login_required));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn home_prefix_expands() {
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(
                expand_home("~/Ostov/a.sqlite"),
                format!("{}/Ostov/a.sqlite", home.trim_end_matches('/'))
            );
        }
        assert_eq!(expand_home("/tmp/a.sqlite"), "/tmp/a.sqlite");
    }
}
