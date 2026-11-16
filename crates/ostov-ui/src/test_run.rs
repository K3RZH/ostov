//! «Запустить» в конфигураторе без интерфейса: тестовая база, план её изменений, сброс и
//! процесс предпросмотра (документ «Состав конфигурации», К-16 и К-24).
//!
//! Тестовая база лежит в самой папке конфигурации: `<папка>/.ostov-test/test.sqlite`, рядом
//! `.gitignore` со строкой `*`, чтобы база не попала в git. Имя постоянное, из манифеста не
//! берётся. Файл базы держит один процесс (ADR-024, `<база>.lock`): конфигуратор берёт замок
//! только на время плана, клиент предпросмотра держит его, пока открыт.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ostov_api::{EngineError, ErrorCode, SchemaChange, SchemaPlan};

use crate::local_base::Config;

const TEST_DIR: &str = ".ostov-test";
const TEST_FILE: &str = "test.sqlite";
/// Сколько ждать, пока предпросмотр закроется сам, прежде чем снять его (`kill`).
const STOP_WAIT: Duration = Duration::from_secs(3);

/// Файл тестовой базы для папки конфигурации.
pub fn base_path(config_dir: &Path) -> PathBuf {
    config_dir.join(TEST_DIR).join(TEST_FILE)
}

/// Тестовая база готова к запуску: план того, что сделает с ней открытие.
#[derive(Debug)]
pub struct Prepared {
    pub base: PathBuf,
    /// База создана сейчас, при первом запуске или после сброса.
    pub created: bool,
    pub plan: SchemaPlan,
}

/// Почему запустить нельзя.
#[derive(Debug)]
pub enum RunError {
    /// Конфигурация не прочиталась: ошибки для панели «Проверка».
    Config(Vec<ostov_meta::Error>),
    /// Ядро отказало; у `AlreadyOpen` текст говорит, что делать.
    Engine(EngineError),
}

impl From<EngineError> for RunError {
    fn from(error: EngineError) -> Self {
        Self::Engine(explain(error))
    }
}

/// Читает папку, при первом запуске создаёт тестовую базу и считает план её изменений.
/// База после этого закрыта: открывать её будет клиент предпросмотра.
///
/// # Errors
/// Конфигурация с ошибками, база занята другой программой (`AlreadyOpen`), прочие ошибки ядра.
pub fn prepare(config_dir: &Path) -> Result<Prepared, RunError> {
    let config = Config::load(config_dir).map_err(RunError::Config)?;
    let base = base_path(config_dir);
    let created = ensure_base(&config, &base)?;
    let plan = config.plan(&base.to_string_lossy())?;
    Ok(Prepared {
        base,
        created,
        plan,
    })
}

/// Удаляет тестовую базу и создаёт её заново по текущей конфигурации; данные в ней пропадают.
///
/// # Errors
/// Базу держит другая программа: её не трогаем (`AlreadyOpen`). Файл не удалить, ошибки ядра.
pub fn reset(config_dir: &Path) -> Result<Prepared, RunError> {
    let config = Config::load(config_dir).map_err(RunError::Config)?;
    let base = base_path(config_dir);
    // План берёт тот же замок, что и открытие: занятую базу так видно до удаления. Остальные
    // ошибки плана (файл испорчен, не база Ostov) как раз повод для сброса.
    if let Err(error) = config.plan(&base.to_string_lossy())
        && error.code == ErrorCode::AlreadyOpen
    {
        return Err(error.into());
    }
    remove_base(&base)?;
    prepare(config_dir)
}

/// Создаёт базу, если её нет; `true`, если создала.
fn ensure_base(config: &Config, base: &Path) -> Result<bool, EngineError> {
    if base.exists() {
        return Ok(false);
    }
    if let Some(dir) = base.parent() {
        std::fs::create_dir_all(dir).map_err(|error| unavailable("папку тестовой базы", &error))?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, "*\n").map_err(|error| unavailable(".gitignore", &error))?;
        }
    }
    // Созданную базу сразу закрываем: замок нужен клиенту предпросмотра.
    drop(config.create(&base.to_string_lossy(), None, None)?);
    Ok(true)
}

/// Файл базы и спутники SQLite. Файл замка остаётся: его держит система, а не содержимое.
fn remove_base(base: &Path) -> Result<(), EngineError> {
    let name = base.as_os_str();
    for suffix in ["", "-wal", "-shm"] {
        let mut file = name.to_owned();
        file.push(suffix);
        match std::fs::remove_file(PathBuf::from(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(unavailable("файл тестовой базы", &error)),
        }
    }
    Ok(())
}

fn unavailable(what: &str, error: &std::io::Error) -> EngineError {
    EngineError::new(
        ErrorCode::Unavailable,
        format!("Не удалось записать {what}: {error}."),
    )
}

/// Текст ядра «база уже открыта» говорит, что случилось; здесь добавляем, что делать.
fn explain(error: EngineError) -> EngineError {
    if error.code != ErrorCode::AlreadyOpen {
        return error;
    }
    EngineError::new(
        ErrorCode::AlreadyOpen,
        "Тестовую базу держит другая программа, например клиент Ostov, открытый не из \
         конфигуратора. Закройте её и нажмите «Запустить» ещё раз.",
    )
}

/// Строки плана для диалога: что открытие сделает с таблицами базы.
pub fn plan_lines(plan: &SchemaPlan) -> Vec<String> {
    plan.changes
        .iter()
        .map(|change| match change {
            SchemaChange::NewTable { table } => format!("Новая таблица {table}"),
            SchemaChange::NewField { table, field, kind } => {
                format!("Новое поле {table}.{field}, тип {kind}")
            }
            SchemaChange::RebuiltRegister { register } => {
                format!("Регистр {register} создаётся заново: движений в нём нет")
            }
        })
        .collect()
}

/// Есть ли в папке правила на Python (`*.py`). Служебные папки (`.ostov-test`, `_stubs`) не смотрим.
pub fn has_python(config_dir: &Path) -> bool {
    // Файлы конфигурации лежат на один уровень ниже папки; глубже смотреть незачем.
    const DEPTH: usize = 2;
    fn walk(dir: &Path, depth: usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            let name = entry.file_name();
            let service = name.to_string_lossy().starts_with(['.', '_']);
            if path.is_dir() {
                depth > 0 && !service && walk(&path, depth - 1)
            } else {
                path.extension().is_some_and(|ext| ext == "py")
            }
        })
    }
    walk(config_dir, DEPTH)
}

/// Предупреждение для конфигурации с файлами `.py`.
pub const NO_PYTHON: &str =
    "Сборка без Python: работают встроенные правила, файлы .py не читаются.";

/// Клиент предпросмотра, запущенный конфигуратором: тот же исполняемый файл с `--preview`.
/// stdin клиента — труба от конфигуратора: её закрытие просит клиент завершиться
/// (`launch::quit_when_stdin_closes`), а при закрытии конфигуратора она закрывается сама.
#[derive(Debug)]
pub struct Preview {
    child: Child,
}

impl Preview {
    /// Запускает клиент на тестовой базе папки `config_dir`.
    ///
    /// # Errors
    /// Свой исполняемый файл не найден или процесс не запустился.
    pub fn spawn(config_dir: &Path, base: &Path) -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        Self::start(
            Command::new(exe).args(crate::launch::preview_args(config_dir, base)),
        )
    }

    fn start(command: &mut Command) -> std::io::Result<Self> {
        let child = command.stdin(Stdio::piped()).spawn()?;
        Ok(Self { child })
    }

    /// Мягкая остановка: закрыть stdin и ждать до `STOP_WAIT`; не закрылся — снять. Ждёт,
    /// поэтому зовётся в фоне.
    pub fn stop(self) {
        self.stop_within(STOP_WAIT);
    }

    fn stop_within(mut self, wait: Duration) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{config_copy, config_dir, replace_in, temp_dir};

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(dir));
    }

    #[test]
    fn first_run_creates_the_base_and_its_gitignore() {
        let dir = config_copy("run-first");
        let first = prepare(&dir).expect("первый запуск готовит базу");
        assert!(first.created);
        assert_eq!(first.base, dir.join(".ostov-test").join("test.sqlite"));
        assert!(first.base.is_file());
        let ignore = std::fs::read_to_string(dir.join(".ostov-test").join(".gitignore"));
        assert_eq!(ignore.ok().as_deref(), Some("*\n"));
        assert_eq!(first.plan, SchemaPlan::default());
        // Второй запуск ту же базу не пересоздаёт.
        assert!(!prepare(&dir).expect("второй запуск").created);
        cleanup(&dir);
    }

    #[test]
    fn new_field_in_yaml_shows_up_in_the_plan() {
        let dir = config_copy("run-new-field");
        assert!(prepare(&dir).is_ok());
        let goods = dir.join("catalogs").join("goods.yaml");
        replace_in(
            &goods,
            "fields:\n",
            "fields:\n  - name: brand\n    title: Марка\n    type: text\n",
        );
        let plan = prepare(&dir).expect("план с новым полем").plan;
        assert_eq!(plan.refused, None);
        assert!(
            plan.changes.contains(&SchemaChange::NewField {
                table: "goods".into(),
                field: "brand".into(),
                kind: "text".into(),
            }),
            "{plan:?}"
        );
        assert_eq!(plan_lines(&plan), ["Новое поле goods.brand, тип text"]);
        cleanup(&dir);
    }

    #[test]
    fn changed_field_type_is_refused_and_reset_starts_over() {
        let dir = config_copy("run-refused");
        assert!(prepare(&dir).is_ok());
        let goods = dir.join("catalogs").join("goods.yaml");
        replace_in(
            &goods,
            "title: Единица измерения\n    type: text",
            "title: Единица измерения\n    type: int",
        );
        let plan = prepare(&dir).expect("план с отказом").plan;
        let why = plan.refused.expect("смену типа ядро отклоняет");
        assert!(why.contains("unit"), "{why}");

        let fresh = reset(&dir).expect("сброс пересоздаёт базу");
        assert!(fresh.created);
        assert_eq!(fresh.plan, SchemaPlan::default());
        cleanup(&dir);
    }

    #[test]
    fn base_held_by_an_engine_is_already_open_with_advice() {
        let dir = config_copy("run-held");
        let base = prepare(&dir).expect("база готова").base;
        let config = Config::load(&dir).expect("папка читается");
        let held = config.open(&base.to_string_lossy()).expect("база открывается");

        let Err(RunError::Engine(error)) = prepare(&dir) else {
            panic!("занятая база должна дать ошибку");
        };
        assert_eq!(error.code, ErrorCode::AlreadyOpen);
        assert!(error.message.contains("Закройте"), "{}", error.message);
        // Сброс занятую базу не трогает.
        assert!(matches!(reset(&dir), Err(RunError::Engine(ref e)) if e.code == ErrorCode::AlreadyOpen));
        assert!(base.is_file());

        drop(held);
        assert!(prepare(&dir).is_ok());
        cleanup(&dir);
    }

    #[test]
    fn broken_config_is_reported_before_touching_the_base() {
        let dir = config_copy("run-broken");
        replace_in(&dir.join("catalogs").join("goods.yaml"), "title: Товары", "title: [");
        assert!(matches!(prepare(&dir), Err(RunError::Config(ref errors)) if !errors.is_empty()));
        assert!(!base_path(&dir).exists());
        cleanup(&dir);
    }

    #[test]
    fn python_rules_are_found_only_where_they_are() {
        assert!(has_python(&config_dir()));
        let dir = temp_dir("run-no-python");
        std::fs::create_dir_all(dir.join("documents")).expect("папка");
        std::fs::write(dir.join("documents").join("receipt.yaml"), "").expect("файл");
        std::fs::create_dir_all(dir.join("_stubs")).expect("папка");
        std::fs::write(dir.join("_stubs").join("ostov.py"), "").expect("файл");
        assert!(!has_python(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plan_lines_name_every_kind_of_change() {
        let plan = SchemaPlan {
            changes: vec![
                SchemaChange::NewTable {
                    table: "units".into(),
                },
                SchemaChange::RebuiltRegister {
                    register: "stock".into(),
                },
            ],
            refused: None,
        };
        assert_eq!(
            plan_lines(&plan),
            [
                "Новая таблица units",
                "Регистр stock создаётся заново: движений в нём нет"
            ]
        );
    }

    // Мягкая остановка: процесс, который читает stdin, закрывается сам; глухой снимается.
    #[cfg(unix)]
    #[test]
    fn stop_closes_stdin_first_and_kills_only_a_deaf_process() {
        let started = Instant::now();
        let listening = Preview::start(&mut Command::new("cat")).expect("cat запускается");
        listening.stop_within(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(2));

        let deaf = Preview::start(Command::new("sleep").arg("30")).expect("sleep запускается");
        let started = Instant::now();
        deaf.stop_within(Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
