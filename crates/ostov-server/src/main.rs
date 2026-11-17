//! `ostov-server` — сервер режима 2.
//!
//! ```text
//! ostov-server --settings /etc/ostov/server.yaml
//! ```
//!
//! Настройки — в файле (`settings.rs`), без аргументов — файл по умолчанию; его нет —
//! создаётся с умолчаниями. Флаги и `OSTOV_DB_URL` перекрывают файл, поэтому и старый запуск
//! `--config config` с `OSTOV_DB_URL` работает как раньше.

mod settings;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use settings::Settings;

use ostov_core::local::Rules;
use ostov_server::{Base, CA_FILE, Connect, Hub, Tls, serve};
use ostov_store::Driver;

const USAGE: &str = "ostov-server [--settings <файл server.yaml>] [--config <папка конфигурации>] \
[--listen 0.0.0.0:7700] [--tls <папка сертификата>] [--host <имя>]... [--query-timeout <секунд, 30>] \
[--db-connections <число, 10>]\nФайл настроек с умолчаниями для пакета: ostov-server --print-settings.
Без --settings и --config — файл настроек по умолчанию \
(/etc/ostov/server.yaml, на Windows %ProgramData%\\Ostov\\server.yaml); его нет — создаётся.\n\
База — в файле настроек или в переменной OSTOV_DB_URL: строка PostgreSQL или sqlite:<файл>.\n\
Сброс пароля: ostov-server [--settings <файл>] --reset-password <логин> — новый пароль спросит \
с клавиатуры, сервер не запускается.";

/// Флаги командной строки: что задано, перекрывает файл настроек.
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    settings: Option<PathBuf>,
    config: Option<PathBuf>,
    listen: Option<SocketAddr>,
    tls: Option<PathBuf>,
    /// Имена и адреса, на которые выписан сертификат: по ним клиенты подключаются.
    hosts: Vec<String>,
    /// Не запускать сервер, а сменить пароль этому пользователю.
    reset_password: Option<String>,
    /// Сколько может идти один запрос к базе (СТ-51).
    query_timeout: Option<Duration>,
    /// Соединений с базой на все сессии.
    connections: Option<usize>,
    /// Напечатать файл настроек с умолчаниями (для пакетов) и выйти.
    print_settings: bool,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut out = Self::default();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("после {arg} нужно значение"));
            match arg.as_str() {
                "--settings" => out.settings = Some(PathBuf::from(value()?)),
                "--config" => out.config = Some(PathBuf::from(value()?)),
                "--listen" => {
                    let raw = value()?;
                    let listen = raw
                        .parse()
                        .map_err(|_| format!("«{raw}» — не адрес вида 0.0.0.0:7700"))?;
                    out.listen = Some(listen);
                }
                "--tls" => out.tls = Some(PathBuf::from(value()?)),
                "--host" => out.hosts.push(value()?),
                "--reset-password" => out.reset_password = Some(value()?),
                "--query-timeout" => {
                    let raw = value()?;
                    let secs: u64 = raw
                        .parse()
                        .ok()
                        .filter(|s| *s > 0)
                        .ok_or(format!("«{raw}» — не число секунд больше нуля"))?;
                    out.query_timeout = Some(Duration::from_secs(secs));
                }
                "--db-connections" => {
                    let raw = value()?;
                    let connections = raw
                        .parse()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or(format!("«{raw}» — не число соединений больше нуля"))?;
                    out.connections = Some(connections);
                }
                "--print-settings" => out.print_settings = true,
                "--help" | "-h" => return Err(USAGE.to_owned()),
                other => return Err(format!("непонятный параметр {other}\n{USAGE}")),
            }
        }
        Ok(out)
    }

    /// Итоговые настройки: файл (если он есть в этом запуске), поверх — флаги, поверх —
    /// `OSTOV_DB_URL`. Старый запуск только с `--config` файла не читает.
    fn resolve(self, env_db: Option<String>) -> Result<Settings, String> {
        let mut s = match (&self.settings, &self.config) {
            (Some(path), _) => {
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                settings::load(path, &dir)?
            }
            (None, Some(_)) => Settings {
                config: None,
                listen: SocketAddr::from(([0, 0, 0, 0], 7700)),
                tls: PathBuf::from("tls"),
                hosts: Vec::new(),
                database: String::new(),
                query_timeout: ostov_store::drivers::QUERY_TIMEOUT,
                connections: ostov_server::CONNECTIONS,
            },
            (None, None) => settings::load(&settings::default_path(), &settings::data_dir())?,
        };
        if let Some(config) = self.config {
            s.config = Some(config);
        }
        if let Some(listen) = self.listen {
            s.listen = listen;
        }
        if let Some(tls) = self.tls {
            s.tls = tls;
        }
        if let Some(timeout) = self.query_timeout {
            s.query_timeout = timeout;
        }
        if let Some(connections) = self.connections {
            s.connections = connections;
        }
        if let Some(url) = env_db.filter(|u| !u.is_empty()) {
            s.database = url;
        }
        if s.database.is_empty() {
            return Err(format!(
                "не задана база: database в файле настроек или переменная OSTOV_DB_URL\n{USAGE}"
            ));
        }
        let mut hosts = self.hosts;
        let defaults = ["localhost".to_owned(), "127.0.0.1".to_owned()];
        for host in s.hosts.drain(..).chain(defaults).chain(computer_name()) {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        s.hosts = hosts;
        Ok(s)
    }
}

fn computer_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|name| name.to_lowercase())
        .filter(|name| !name.is_empty())
}

fn database(url: String, timeout: Duration) -> Connect {
    match url.strip_prefix("sqlite:").map(PathBuf::from) {
        Some(path) => Arc::new(move || {
            let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&path)?;
            db.set_query_timeout(timeout)?;
            Ok(Box::new(db) as Box<dyn Driver + Send>)
        }),
        None => Arc::new(move || {
            let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url)?;
            db.set_query_timeout(timeout)?;
            Ok(Box::new(db) as Box<dyn Driver + Send>)
        }),
    }
}

/// Логика документов: `.py` из конфигурации, если они есть и сервер собран с Python.
fn rules(
    config: &std::path::Path,
    model: &ostov_model::Config,
) -> Result<Option<Arc<dyn Rules + Send + Sync>>, String> {
    let scripts = model
        .documents
        .iter()
        .any(|d| ostov_py::script_path(config, &d.name).exists());
    if !scripts {
        return Ok(None);
    }
    if !ostov_py::PYTHON_ENABLED {
        eprintln!(
            "Ostov: предупреждение — в конфигурации есть .py, но сервер собран без Python: работают встроенные правила документов"
        );
        return Ok(None);
    }
    let rules = ostov_py::PyRules::load(config, model).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        format!(
            "ошибки в логике документов:
{}",
            lines.join(
                "
"
            )
        )
    })?;
    Ok(Some(Arc::new(rules)))
}

fn run() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.print_settings {
        print!("{}", settings::template(&settings::data_dir()));
        return Ok(());
    }
    let reset = args.reset_password.clone();
    let args = args.resolve(std::env::var("OSTOV_DB_URL").ok())?;
    let model = match &args.config {
        Some(dir) => ostov_meta::load(dir),
        None => ostov_meta::demo(),
    }
    .map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        format!("ошибки в конфигурации:\n{}", lines.join("\n"))
    })?;
    let title = model.title.clone();
    let rules = match &args.config {
        Some(dir) => rules(dir, &model)?,
        None => None,
    };
    let logic = if rules.is_some() {
        "Python"
    } else {
        "встроенная"
    };
    if let Some(db) = args.database.strip_prefix("sqlite:")
        && let Some(dir) = Path::new(db).parent().filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut base = Base::new(database(args.database.clone(), args.query_timeout), model)
        .with_connections(args.connections);
    if let Some(rules) = rules {
        base = base.with_rules(rules);
    }
    if let Some(login) = &reset {
        return reset_password(&base, login);
    }
    // До рантайма: Hub ходит в базу синхронно.
    let hub = Hub::start(base).map_err(|e| format!("{} ({})", e.message, e.details))?;
    let tls = Tls::load_or_create(&args.tls, &args.hosts).map_err(|e| e.to_string())?;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(args.listen)
            .await
            .map_err(|e| format!("не удалось занять {}: {e}", args.listen))?;
        println!(
            "Ostov: сервер «{title}» слушает {}, логика документов: {logic}. Сертификат для клиентов: {}",
            args.listen,
            args.tls.join(CA_FILE).display()
        );
        let stop = async {
            let _ = tokio::signal::ctrl_c().await;
            println!("Ostov: сервер останавливается.");
        };
        serve(listener, Arc::new(hub), &tls, stop)
            .await
            .map_err(|e| e.to_string())
    })
}

/// Пароль с клавиатуры, а не в параметрах: иначе он останется в истории команд и в списке
/// процессов.
fn reset_password(base: &Base, login: &str) -> Result<(), String> {
    let first = ask(&format!("Новый пароль для {login} (от 12 символов): "))?;
    let second = ask("Ещё раз: ")?;
    if first != second {
        return Err("пароли не совпадают, ничего не изменено".into());
    }
    base.reset_password(login, &first).map_err(|e| e.message)?;
    println!(
        "Ostov: пароль пользователя {login} сменён. Если сервер запущен, перезапустите его: \
         иначе открытые сессии этого пользователя доживут до конца по времени."
    );
    Ok(())
}

fn ask(prompt: &str) -> Result<String, String> {
    use std::io::Write;
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("не удалось прочитать пароль: {e}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("Ostov: {why}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(|a| (*a).to_owned()))
    }

    fn old_style(args: &[&str]) -> Result<Settings, String> {
        parse(args)?.resolve(Some("sqlite:base.db".into()))
    }

    #[test]
    fn arguments() {
        let args = old_style(&[
            "--config",
            "cfg",
            "--listen",
            "127.0.0.1:7801",
            "--host",
            "server.local",
        ])
        .unwrap();
        assert_eq!(args.config, Some(PathBuf::from("cfg")));
        assert_eq!(args.listen, SocketAddr::from(([127, 0, 0, 1], 7801)));
        assert_eq!(args.tls, PathBuf::from("tls"));
        assert_eq!(args.hosts[..3], ["server.local", "localhost", "127.0.0.1"]);
        assert_eq!(args.database, "sqlite:base.db");
        assert_eq!(args.query_timeout, Duration::from_secs(30));
        let slow = old_style(&["--config", "c", "--query-timeout", "90"]).unwrap();
        assert_eq!(slow.query_timeout, Duration::from_secs(90));
        assert!(parse(&["--config", "c", "--query-timeout", "0"]).is_err());
        assert_eq!(args.connections, 10);
        let many = old_style(&["--config", "c", "--db-connections", "40"]).unwrap();
        assert_eq!(many.connections, 40);
        assert!(parse(&["--config", "c", "--db-connections", "0"]).is_err());
        let reset = parse(&["--config", "c", "--reset-password", "boss"]).unwrap();
        assert_eq!(reset.reset_password.as_deref(), Some("boss"));

        // Старый запуск без базы — понятная ошибка.
        let no_db = parse(&["--config", "c"])
            .unwrap()
            .resolve(None)
            .unwrap_err();
        assert!(no_db.contains("OSTOV_DB_URL"), "{no_db}");
        assert!(parse(&["--config"]).unwrap_err().contains("нужно значение"));
        assert!(parse(&["--config", "c", "--listen", "где-то"]).is_err());
        assert!(
            parse(&["--config", "c", "--port", "1"])
                .unwrap_err()
                .contains("--port")
        );
    }

    #[test]
    fn settings_file_with_flags_and_env_on_top() {
        let dir = std::env::temp_dir().join(format!("ostov-server-args-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server.yaml");
        std::fs::write(
            &file,
            "listen: 127.0.0.1:7801\nhosts: [server.local]\ndatabase: sqlite:base.db\n\
             db_connections: 20\n",
        )
        .unwrap();
        let path = file.display().to_string();

        let s = parse(&["--settings", &path])
            .unwrap()
            .resolve(None)
            .unwrap();
        assert_eq!(s.config, None);
        assert_eq!(s.listen, SocketAddr::from(([127, 0, 0, 1], 7801)));
        assert_eq!(s.hosts[..3], ["server.local", "localhost", "127.0.0.1"]);
        assert_eq!(
            s.database,
            format!("sqlite:{}", dir.join("base.db").display())
        );
        assert_eq!(s.connections, 20);

        // Флаги и OSTOV_DB_URL перекрывают файл.
        let s = parse(&[
            "--settings",
            &path,
            "--db-connections",
            "5",
            "--host",
            "x.local",
        ])
        .unwrap()
        .resolve(Some("host=db dbname=ostov".into()))
        .unwrap();
        assert_eq!(s.connections, 5);
        assert_eq!(s.database, "host=db dbname=ostov");
        assert_eq!(s.hosts[..2], ["x.local", "server.local"]);

        // Файла нет — создаётся рядом, сервер стартует с умолчаниями.
        let fresh = dir.join("new").join("server.yaml");
        let fresh_path = fresh.display().to_string();
        let s = parse(&["--settings", &fresh_path])
            .unwrap()
            .resolve(None)
            .unwrap();
        assert!(fresh.exists());
        assert!(s.database.starts_with("sqlite:"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
