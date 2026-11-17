//! Файл настроек сервера (`server.yaml`): всё, что можно задать флагами, в одном файле.
//! Службе (systemd, служба Windows) хватает одного аргумента `--settings <файл>`, а пароль
//! базы не виден ни в командной строке, ни в unit-файле. Файла нет — сервер создаёт его
//! с умолчаниями и стартует без правок: SQLite и сертификаты рядом, встроенный «Склад».

use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// Что лежит в файле; чего нет — по умолчанию. Неизвестный ключ — ошибка, а не тишина.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    config: Option<String>,
    listen: Option<String>,
    tls: Option<String>,
    hosts: Option<Vec<String>>,
    database: Option<String>,
    query_timeout: Option<u64>,
    db_connections: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Папка конфигурации; `None` — встроенный «Склад».
    pub config: Option<PathBuf>,
    pub listen: SocketAddr,
    pub tls: PathBuf,
    pub hosts: Vec<String>,
    /// Строка подключения PostgreSQL или `sqlite:<файл>`.
    pub database: String,
    pub query_timeout: Duration,
    pub connections: usize,
}

/// Где служба ищет настройки без `--settings`.
pub fn default_path() -> PathBuf {
    if cfg!(windows) {
        data_dir().join("server.yaml")
    } else {
        PathBuf::from("/etc/ostov/server.yaml")
    }
}

/// Куда по умолчанию кладутся база и сертификаты.
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
        PathBuf::from(base).join("Ostov")
    } else {
        PathBuf::from("/var/lib/ostov")
    }
}

/// Настройки из файла `path`. Файла нет — создаёт его с умолчаниями (данные — в `data`).
pub fn load(path: &Path, data: &Path) -> Result<Settings, String> {
    let shown = path.display();
    if !path.exists() {
        create(path, data).map_err(|e| format!("не удалось создать {shown}: {e}"))?;
        println!("Ostov: создан файл настроек {shown} — сервер работает с умолчаниями.");
    }
    check_private(path)?;
    let text =
        fs::read_to_string(path).map_err(|e| format!("не удалось прочитать {shown}: {e}"))?;
    let file: File = if text.trim().is_empty() {
        File::default()
    } else {
        serde_saphyr::from_str(&text).map_err(|e| format!("{shown}: {e}"))?
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let place = |p: &str| {
        let p = PathBuf::from(p);
        if p.is_relative() { dir.join(p) } else { p }
    };
    let listen = match file.listen {
        Some(raw) => raw
            .parse()
            .map_err(|_| format!("{shown}: listen «{raw}» — не адрес вида 0.0.0.0:7700"))?,
        None => SocketAddr::from(([0, 0, 0, 0], 7700)),
    };
    let database = match file.database {
        Some(url) => match url.strip_prefix("sqlite:") {
            Some(db) => format!("sqlite:{}", place(db).display()),
            None => url,
        },
        None => format!("sqlite:{}", dir.join("base.db").display()),
    };
    let query_timeout = file.query_timeout.unwrap_or(30);
    let connections = file.db_connections.unwrap_or(ostov_server::CONNECTIONS);
    if query_timeout == 0 || connections == 0 {
        return Err(format!(
            "{shown}: query_timeout и db_connections — числа больше нуля"
        ));
    }
    Ok(Settings {
        config: file.config.as_deref().map(place),
        listen,
        tls: place(file.tls.as_deref().unwrap_or("tls")),
        hosts: file.hosts.unwrap_or_default(),
        database,
        query_timeout: Duration::from_secs(query_timeout),
        connections,
    })
}

/// В файле может быть пароль базы: на Linux его не должен читать кто угодно.
fn check_private(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o007 != 0 {
            return Err(format!(
                "{} читают все пользователи компьютера, а в нём пароль базы: \
                 chmod 640 и владелец — пользователь службы",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create(path: &Path, data: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::create_dir_all(data)?;
    let text = template(data);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(text.as_bytes())
}

/// Файл с умолчаниями и подсказками: его же кладут в пакеты.
pub fn template(data: &Path) -> String {
    let data = data.display().to_string().replace('\\', "/");
    format!(
        "# Настройки сервера Ostov. Флаги командной строки и OSTOV_DB_URL их перекрывают.\n\
         # Относительные пути — от папки этого файла.\n\
         \n\
         # Папка конфигурации (YAML и .py). Без неё — встроенный «Склад».\n\
         # config: /usr/share/ostov/config\n\
         \n\
         listen: 0.0.0.0:7700\n\
         \n\
         # Сертификаты: клиентам раздают ca.crt из этой папки.\n\
         tls: {data}/tls\n\
         \n\
         # Имена и адреса, по которым клиенты подключаются (localhost и имя компьютера есть всегда).\n\
         # hosts: [server.local, 192.168.1.10]\n\
         \n\
         # База: sqlite:<файл> или строка PostgreSQL, например\n\
         # database: host=localhost user=ostov password=секрет dbname=ostov\n\
         database: sqlite:{data}/base.db\n\
         \n\
         # Сколько секунд может идти один запрос и сколько соединений с базой на все сессии.\n\
         query_timeout: 30\n\
         db_connections: 10\n"
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ostov-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_created_and_works_as_is() {
        let dir = temp("new");
        let path = dir.join("etc").join("server.yaml");
        let data = dir.join("data");
        let s = load(&path, &data).unwrap();
        assert!(path.exists() && data.exists());
        assert_eq!(s.config, None);
        assert_eq!(s.listen, SocketAddr::from(([0, 0, 0, 0], 7700)));
        assert_eq!(s.tls, data.join("tls"));
        assert!(s.database.starts_with("sqlite:") && s.database.ends_with("base.db"));
        assert_eq!(s.query_timeout, Duration::from_secs(30));
        assert_eq!(s.connections, 10);
        // Второй запуск читает тот же файл и не трогает его.
        let before = fs::read_to_string(&path).unwrap();
        assert_eq!(load(&path, &data).unwrap(), s);
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn values_and_relative_paths() {
        let dir = temp("values");
        let path = dir.join("server.yaml");
        fs::write(
            &path,
            "config: cfg\nlisten: 127.0.0.1:7801\ntls: certs\nhosts: [server.local]\n\
             database: \"host=db user=ostov password=x dbname=ostov\"\nquery_timeout: 90\n\
             db_connections: 40\n",
        )
        .unwrap();
        let s = load(&path, &dir).unwrap();
        assert_eq!(s.config, Some(dir.join("cfg")));
        assert_eq!(s.listen, SocketAddr::from(([127, 0, 0, 1], 7801)));
        assert_eq!(s.tls, dir.join("certs"));
        assert_eq!(s.hosts, ["server.local"]);
        assert_eq!(s.database, "host=db user=ostov password=x dbname=ostov");
        assert_eq!(s.query_timeout, Duration::from_secs(90));
        assert_eq!(s.connections, 40);

        fs::write(&path, "database: sqlite:base/ostov.db\n").unwrap();
        let s = load(&path, &dir).unwrap();
        let expected = format!("sqlite:{}", dir.join("base/ostov.db").display());
        assert_eq!(s.database, expected);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mistakes_are_explained() {
        let dir = temp("bad");
        let path = dir.join("server.yaml");
        for (text, says) in [
            ("listen: где-то\n", "listen"),
            ("port: 7700\n", "port"),
            ("query_timeout: 0\n", "больше нуля"),
            ("db_connections: 0\n", "больше нуля"),
        ] {
            fs::write(&path, text).unwrap();
            let err = load(&path, &dir).unwrap_err();
            assert!(err.contains(says), "{text}: {err}");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn file_readable_by_everyone_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("open");
        let path = dir.join("server.yaml");
        fs::write(&path, "listen: 0.0.0.0:7700\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&path, &dir).unwrap_err().contains("chmod 640"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load(&path, &dir).is_ok());
        let _ = fs::remove_dir_all(dir);
    }
}
