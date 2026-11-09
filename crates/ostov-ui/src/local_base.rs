//! Локальная база (режим 1) через ядро: создать файл и открыть файл. Конфигурация пока одна,
//! встроенная «Склад» (`ostov_meta::demo`, та же папка `config/`, которую проверяет
//! `ostov check`); выбор папки конфигурации появится вместе с конфигуратором.

use std::path::Path;
use std::sync::Arc;

use ostov_api::{Connector, Engine, EngineError, EngineResult, ErrorCode, NewAdmin};
use ostov_core::LocalConnector;

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
    Ok(LocalConnector::new(config))
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
