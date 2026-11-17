//! Аргументы запуска клиента: `ostov-ui [--config <папка>] [--base <файл>] [--preview]`.
//!
//! Флагов три, поэтому разбор свой, на `std::env::args_os`: пути остаются `OsString` и доходят
//! до файловой системы как есть, с кириллицей и любыми байтами. Предпросмотр (`--preview`)
//! запускает конфигуратор: клиент сразу открывает тестовую базу с конфигурацией из папки
//! (документ «Состав конфигурации», К-24).

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

const CONFIG: &str = "--config";
const BASE: &str = "--base";
const PREVIEW: &str = "--preview";

/// С чем запущен клиент. Без аргументов: встроенная конфигурация и выбор базы, как раньше.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// Папка конфигурации вместо встроенной.
    pub config: Option<PathBuf>,
    /// Файл базы, который открыть сразу, без страницы выбора.
    pub base: Option<PathBuf>,
    /// Предпросмотр из конфигуратора: метка на экране, без сервера и без записи в недавние базы.
    pub preview: bool,
}

/// Почему аргументы не разобрались.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    UnknownArgument(String),
    MissingValue(&'static str),
    Repeated(&'static str),
    /// `--preview` без папки конфигурации или без файла базы.
    PreviewNeedsConfigAndBase,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArgument(argument) => write!(f, "неизвестный аргумент «{argument}»"),
            Self::MissingValue(flag) => write!(f, "после {flag} нужен путь"),
            Self::Repeated(flag) => write!(f, "{flag} указан дважды"),
            Self::PreviewNeedsConfigAndBase => write!(f, "{PREVIEW} нужны {CONFIG} и {BASE}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Подсказка для ошибки разбора.
pub const USAGE: &str = "использование: ostov-ui [--config <папка>] [--base <файл>] [--preview]";

/// Разбирает аргументы без имени программы.
///
/// # Errors
/// Неизвестный аргумент, флаг без пути или дважды, `--preview` без `--config` и `--base`.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Launch, LaunchError> {
    let mut launch = Launch::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some(CONFIG) => set_path(&mut launch.config, CONFIG, args.next())?,
            Some(BASE) => set_path(&mut launch.base, BASE, args.next())?,
            Some(PREVIEW) if launch.preview => return Err(LaunchError::Repeated(PREVIEW)),
            Some(PREVIEW) => launch.preview = true,
            _ => {
                return Err(LaunchError::UnknownArgument(
                    arg.to_string_lossy().into_owned(),
                ));
            }
        }
    }
    if launch.preview && (launch.config.is_none() || launch.base.is_none()) {
        return Err(LaunchError::PreviewNeedsConfigAndBase);
    }
    Ok(launch)
}

fn set_path(
    slot: &mut Option<PathBuf>,
    flag: &'static str,
    value: Option<OsString>,
) -> Result<(), LaunchError> {
    if slot.is_some() {
        return Err(LaunchError::Repeated(flag));
    }
    // Флаг вместо пути значит, что путь забыли: `--config --base x`.
    let value = value
        .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
        .ok_or(LaunchError::MissingValue(flag))?;
    *slot = Some(PathBuf::from(value));
    Ok(())
}

/// Аргументы предпросмотра для запуска клиента конфигуратором.
pub fn preview_args(config: &Path, base: &Path) -> [OsString; 5] {
    [
        PREVIEW.into(),
        CONFIG.into(),
        config.as_os_str().to_owned(),
        BASE.into(),
        base.as_os_str().to_owned(),
    ]
}

/// Запуск клиента, а не самого конфигуратора: первый аргумент флаг (`configurator --preview …`).
pub fn is_client(args: &[OsString]) -> bool {
    args.first()
        .is_some_and(|arg| arg.to_string_lossy().starts_with("--"))
}

/// Вкладки как в браузере (окно идёт за указателем, перенос между окнами) нужны общие
/// координаты экрана, а чистый Wayland их приложению не отдаёт. Поэтому на Linux клиент
/// запускается через X11 (XWayland): процесс один раз заменяется копией без `WAYLAND_DISPLAY`.
/// Оставить Wayland: `OSTOV_WAYLAND=1`.
#[cfg(target_os = "linux")]
pub fn prefer_x11() {
    use std::os::unix::process::CommandExt;

    let wants_wayland = std::env::var_os("OSTOV_WAYLAND").is_some();
    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wants_wayland || !on_wayland || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // `exec` заменяет процесс и возвращается только при ошибке: тогда остаёмся на Wayland.
    let _ = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env_remove("WAYLAND_DISPLAY")
        .exec();
}

/// На других системах вкладкам хватает родного оконного протокола.
#[cfg(not(target_os = "linux"))]
pub fn prefer_x11() {}

/// Предпросмотр живёт, пока жив конфигуратор: тот держит трубу на stdin клиента и закрывает её,
/// когда останавливает предпросмотр или закрывается сам. Конец stdin завершает клиент; база
/// закрывается вместе с процессом, и замок файла (`<база>.lock`) освобождается.
pub fn quit_when_stdin_closes() {
    std::thread::spawn(|| {
        use std::io::Read;
        let mut buffer = [0_u8; 256];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buffer), Ok(read) if read > 0) {}
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_arguments_is_the_usual_client() {
        assert_eq!(parse(Vec::new()), Ok(Launch::default()));
    }

    #[test]
    fn preview_with_cyrillic_paths_keeps_them_as_is() {
        let launch = parse(args(&[
            "--preview",
            "--config",
            "/home/анна/Мои конфигурации/Склад",
            "--base",
            "/home/анна/Склад/.ostov-test/test.sqlite",
        ]));
        assert_eq!(
            launch,
            Ok(Launch {
                config: Some(PathBuf::from("/home/анна/Мои конфигурации/Склад")),
                base: Some(PathBuf::from("/home/анна/Склад/.ostov-test/test.sqlite")),
                preview: true,
            })
        );
    }

    #[test]
    fn preview_args_parse_back_to_the_same_launch() {
        let (config, base) = (
            Path::new("/tmp/кафе"),
            Path::new("/tmp/кафе/.ostov-test/t.sqlite"),
        );
        let launch = parse(preview_args(config, base)).expect("свои аргументы разбираются");
        assert!(launch.preview);
        assert_eq!(launch.config.as_deref(), Some(config));
        assert_eq!(launch.base.as_deref(), Some(base));
    }

    #[test]
    fn unknown_flag_and_stray_path_are_errors() {
        assert_eq!(
            parse(args(&["--server", "x"])),
            Err(LaunchError::UnknownArgument("--server".into()))
        );
        assert_eq!(
            parse(args(&["/tmp/a.sqlite"])),
            Err(LaunchError::UnknownArgument("/tmp/a.sqlite".into()))
        );
    }

    #[test]
    fn flag_without_value_or_twice_is_an_error() {
        assert_eq!(
            parse(args(&["--config"])),
            Err(LaunchError::MissingValue("--config"))
        );
        assert_eq!(
            parse(args(&["--config", "--base", "b"])),
            Err(LaunchError::MissingValue("--config"))
        );
        assert_eq!(
            parse(args(&["--base", "a", "--base", "b"])),
            Err(LaunchError::Repeated("--base"))
        );
    }

    #[test]
    fn preview_needs_config_and_base() {
        assert_eq!(
            parse(args(&["--preview", "--config", "c"])),
            Err(LaunchError::PreviewNeedsConfigAndBase)
        );
        // Без предпросмотра хватает любого из двух.
        assert!(parse(args(&["--config", "c"])).is_ok());
        assert!(parse(args(&["--base", "b"])).is_ok());
    }

    #[test]
    fn errors_read_as_russian_text() {
        let text = LaunchError::UnknownArgument("--x".into()).to_string();
        assert_eq!(text, "неизвестный аргумент «--x»");
    }

    #[test]
    fn client_mode_is_recognised_by_the_first_flag() {
        assert!(is_client(&args(&["--preview"])));
        assert!(!is_client(&args(&["/home/анна/конфигурация"])));
        assert!(!is_client(&[]));
    }
}
