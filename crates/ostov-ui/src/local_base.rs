//! Локальная база (режим 1) через ядро: создать файл и открыть файл с выбранной конфигурацией.
//! Конфигурацию передаёт клиент: папка из `--config` (предпросмотр из конфигуратора) или
//! встроенная «Склад», пока в клиенте нет выбора папки конфигурации.

use std::path::Path;
use std::sync::Arc;

use ostov_api::{BaseInfo, Connector, Engine, EngineError, EngineResult, ErrorCode, NewAdmin};
use ostov_core::{LocalConnector, PasswordPolicy};

/// Конфигурация, с которой клиент открывает и создаёт локальные базы.
#[derive(Clone)]
pub struct Config {
    connector: LocalConnector,
    /// Имя из манифеста: по нему база помнит свою конфигурацию.
    name: String,
    pub title: String,
    pub version: String,
}

impl Config {
    /// Конфигурация из папки.
    ///
    /// # Errors
    /// Ошибки разбора и проверки файлов с файлом и строкой.
    pub fn load(dir: &Path) -> Result<Self, Vec<ostov_meta::Error>> {
        let model = ostov_meta::load(dir)?;
        Ok(Self {
            name: model.name.clone(),
            title: model.title.clone(),
            version: model.version.clone(),
            // Один компьютер: любой пароль, в том числе пустой (ADR-033).
            connector: LocalConnector::new(model).with_password_policy(PasswordPolicy::LOCAL),
        })
    }

    /// Встроенная «Склад» для клиента без `--config`.
    ///
    /// # Errors
    /// Только если встроенные файлы испорчены (их проверяет тест `ostov-meta`).
    pub fn embedded() -> Result<Self, Vec<ostov_meta::Error>> {
        let model = ostov_meta::demo()?;
        Ok(Self {
            name: model.name.clone(),
            title: model.title.clone(),
            version: model.version.clone(),
            connector: LocalConnector::new(model).with_password_policy(PasswordPolicy::LOCAL),
        })
    }

    /// Что за база в этом файле, не открывая и не записывая в него (для «Добавить существующую»).
    /// База другой конфигурации не подходит.
    ///
    /// # Errors
    /// Ошибки ядра как есть: файла нет (`NotFound`), это не база Ostov (`Incompatible`), база другой
    /// конфигурации (`Incompatible` с её названием).
    pub fn check(&self, path: &str) -> EngineResult<BaseInfo> {
        let info = self.connector.check_local(&expand_home(path))?;
        ensure_same_config(&info, &self.name, &self.title)?;
        Ok(info)
    }

    /// Открывает файл базы. Если конфигурация изменилась, ядро достраивает таблицы (СТ-50).
    ///
    /// # Errors
    /// Ошибки ядра как есть: файла нет, это не база Ostov, база уже открыта другой программой,
    /// изменение конфигурации потеряло бы данные.
    pub fn open(&self, path: &str) -> EngineResult<Arc<dyn Engine>> {
        self.connector.open_local(&expand_home(path), None)
    }

    /// Что изменится в базе при открытии с этой конфигурацией; база не меняется.
    ///
    /// # Errors
    /// Ошибки ядра как есть; база открыта другой программой: `AlreadyOpen`.
    pub fn plan(&self, path: &str) -> EngineResult<ostov_api::SchemaPlan> {
        self.connector.plan_local(&expand_home(path))
    }

    /// Создаёт файл базы; папку создаёт, если её нет. С `admin` в базе сразу появляется
    /// администратор, и дальше она спрашивает вход.
    ///
    /// # Errors
    /// Файл уже есть, папку не создать, пароль на файл (ядро его пока не поддерживает, вопрос В-2).
    pub fn create(
        &self,
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
        self.connector.create_local(&path, file_password, admin)
    }
}

fn ensure_same_config(info: &BaseInfo, name: &str, title: &str) -> EngineResult<()> {
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

    fn demo() -> Config {
        Config::embedded().expect("встроенная конфигурация читается")
    }

    fn create(
        path: &str,
        file_password: Option<&str>,
        admin: Option<&NewAdmin>,
    ) -> EngineResult<Arc<dyn Engine>> {
        demo().create(path, file_password, admin)
    }

    fn open(path: &str) -> EngineResult<Arc<dyn Engine>> {
        demo().open(path)
    }

    fn check(path: &str) -> EngineResult<BaseInfo> {
        demo().check(path)
    }

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
        assert!(ensure_same_config(&info("warehouse", "Склад"), "warehouse", "Склад").is_ok());
        let error = ensure_same_config(&info("cafe", "Кафе"), "warehouse", "Склад").err();
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

    // Экран «Закрытие периода»: дата туда и обратно, формат ГГГГ-ММ-ДД, снятие закрытия.
    #[test]
    fn closing_date_round_trips() {
        let (dir, path) = temp_base("closing");
        let engine = create(&path, None, None);
        assert!(engine.is_ok(), "{:?}", engine.err());
        if let Ok(engine) = engine {
            assert!(matches!(engine.closing_date(), Ok(None)));
            assert!(engine.set_closing_date(Some("2026-01-31")).is_ok());
            let closed = |engine: &Arc<dyn Engine>| engine.closing_date().ok().flatten();
            assert_eq!(closed(&engine).as_deref(), Some("2026-01-31"));
            // Дату в другом виде ядро отклоняет, прежнее закрытие остаётся.
            assert!(engine.set_closing_date(Some("31.01.2026")).is_err());
            assert!(engine.set_closing_date(Some("2026-02-31")).is_err());
            assert_eq!(closed(&engine).as_deref(), Some("2026-01-31"));
            assert!(engine.set_closing_date(None).is_ok());
            assert!(matches!(engine.closing_date(), Ok(None)));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    // Экран «Журнал действий»: действия видны, новые сверху, страницы идут по ключу `id`.
    #[test]
    fn journal_shows_actions_newest_first_and_pages_by_key() {
        let (dir, path) = temp_base("journal");
        let engine = create(&path, None, None);
        assert!(engine.is_ok(), "{:?}", engine.err());
        if let Ok(engine) = engine {
            assert!(engine.set_closing_date(Some("2026-01-31")).is_ok());
            let added = engine.admin_add_user(&ostov_api::NewUser {
                login: "admin".into(),
                display_name: "Администратор".into(),
                role: "admin".into(),
                password: "1".into(),
            });
            assert!(added.is_ok(), "{added:?}");
            assert!(engine.login("admin", "1").is_ok());

            let all = engine.admin_journal(None, 100).unwrap_or_default();
            assert!(all.len() >= 3, "{all:?}");
            assert!(all.windows(2).all(|pair| pair[0].id > pair[1].id));
            assert!(
                all.iter()
                    .any(|entry| entry.action == "Установка даты закрытия"
                        && entry.details.contains("31.01.2026")),
                "{all:?}"
            );

            let first = engine.admin_journal(None, 2).unwrap_or_default();
            assert_eq!(first.len(), 2);
            let key = first.last().map(|entry| entry.id.to_string());
            let second = engine.admin_journal(key.as_deref(), 2).unwrap_or_default();
            let paged: Vec<i64> = first.iter().chain(&second).map(|entry| entry.id).collect();
            let expected: Vec<i64> = all.iter().map(|entry| entry.id).take(paged.len()).collect();
            assert_eq!(paged, expected);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    // Предпросмотр: база создаётся и открывается с конфигурацией из папки, а не со встроенной.
    #[test]
    fn config_from_folder_creates_and_opens_its_own_base() {
        let dir = crate::test_support::config_copy("local-folder");
        crate::test_support::replace_in(
            &dir.join("ostov.yaml"),
            "title: Склад",
            "title: Склад-проба",
        );
        let config = Config::load(&dir).expect("копия config/ читается");
        assert_eq!(config.title, "Склад-проба");
        let path = dir.join("test.sqlite").to_string_lossy().into_owned();
        assert!(config.create(&path, None, None).is_ok());
        let checked = config.check(&path);
        assert!(
            matches!(checked, Ok(ref info) if info.config_title == "Склад-проба"),
            "{checked:?}"
        );
        assert!(config.open(&path).is_ok());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
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
