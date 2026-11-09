//! `ostov-server` — сервер режима 2.
//!
//! ```text
//! set OSTOV_DB_URL=host=localhost user=ostov password=… dbname=ostov
//! ostov-server --config config --listen 0.0.0.0:7700 --tls tls --host server.local
//! ```
//!
//! База — в переменной `OSTOV_DB_URL`, чтобы пароль не был виден в списке процессов:
//! строка подключения PostgreSQL или `sqlite:путь` для маленькой установки.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ostov_core::local::Rules;
use ostov_server::{Base, CERT_FILE, Connect, Hub, Tls, serve};
use ostov_store::Driver;

const USAGE: &str = "ostov-server --config <папка конфигурации> [--listen 0.0.0.0:7700] \
[--tls <папка сертификата>] [--host <имя>]...\nБаза: переменная OSTOV_DB_URL — строка \
подключения PostgreSQL или sqlite:<файл>.\nСброс пароля: ostov-server --config <папка> \
--reset-password <логин> — новый пароль спросит с клавиатуры, сервер не запускается.";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    config: PathBuf,
    listen: SocketAddr,
    tls: PathBuf,
    /// Имена и адреса, на которые выписан сертификат: по ним клиенты подключаются.
    hosts: Vec<String>,
    /// Не запускать сервер, а сменить пароль этому пользователю.
    reset_password: Option<String>,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut config = None;
        let mut listen = SocketAddr::from(([0, 0, 0, 0], 7700));
        let mut tls = PathBuf::from("tls");
        let mut hosts = Vec::new();
        let mut reset_password = None;
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("после {arg} нужно значение"));
            match arg.as_str() {
                "--config" => config = Some(PathBuf::from(value()?)),
                "--listen" => {
                    let raw = value()?;
                    listen = raw
                        .parse()
                        .map_err(|_| format!("«{raw}» — не адрес вида 0.0.0.0:7700"))?;
                }
                "--tls" => tls = PathBuf::from(value()?),
                "--host" => hosts.push(value()?),
                "--reset-password" => reset_password = Some(value()?),
                "--help" | "-h" => return Err(USAGE.to_owned()),
                other => return Err(format!("непонятный параметр {other}\n{USAGE}")),
            }
        }
        let config = config.ok_or(format!("не указана папка конфигурации\n{USAGE}"))?;
        for default in ["localhost".to_owned(), "127.0.0.1".to_owned()]
            .into_iter()
            .chain(computer_name())
        {
            if !hosts.contains(&default) {
                hosts.push(default);
            }
        }
        Ok(Self {
            config,
            listen,
            tls,
            hosts,
            reset_password,
        })
    }
}

fn computer_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|name| name.to_lowercase())
        .filter(|name| !name.is_empty())
}

fn database(url: String) -> Connect {
    match url.strip_prefix("sqlite:").map(PathBuf::from) {
        Some(path) => Arc::new(move || {
            let db = ostov_store::drivers::sqlite::SqliteDriver::open(&path)?;
            Ok(Box::new(db) as Box<dyn Driver + Send>)
        }),
        None => Arc::new(move || {
            let db = ostov_store::drivers::postgres::PostgresDriver::connect(&url)?;
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
    let url = std::env::var("OSTOV_DB_URL")
        .map_err(|_| format!("не задана переменная OSTOV_DB_URL\n{USAGE}"))?;
    let model = ostov_meta::load(&args.config).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        format!("ошибки в конфигурации:\n{}", lines.join("\n"))
    })?;
    let title = model.title.clone();
    let rules = rules(&args.config, &model)?;
    let logic = if rules.is_some() {
        "Python"
    } else {
        "встроенная"
    };
    let mut base = Base::new(database(url), model);
    if let Some(rules) = rules {
        base = base.with_rules(rules);
    }
    if let Some(login) = &args.reset_password {
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
            args.tls.join(CERT_FILE).display()
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

    #[test]
    fn arguments() {
        let args = parse(&[
            "--config",
            "cfg",
            "--listen",
            "127.0.0.1:7801",
            "--host",
            "server.local",
        ])
        .unwrap();
        assert_eq!(args.config, PathBuf::from("cfg"));
        assert_eq!(args.listen, SocketAddr::from(([127, 0, 0, 1], 7801)));
        assert_eq!(args.tls, PathBuf::from("tls"));
        assert_eq!(args.hosts[..3], ["server.local", "localhost", "127.0.0.1"]);
        assert_eq!(args.reset_password, None);
        let reset = parse(&["--config", "c", "--reset-password", "boss"]).unwrap();
        assert_eq!(reset.reset_password.as_deref(), Some("boss"));

        assert!(parse(&[]).unwrap_err().contains("папка конфигурации"));
        assert!(parse(&["--config"]).unwrap_err().contains("нужно значение"));
        assert!(parse(&["--config", "c", "--listen", "где-то"]).is_err());
        assert!(
            parse(&["--config", "c", "--port", "1"])
                .unwrap_err()
                .contains("--port")
        );
    }
}
