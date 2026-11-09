//! Настоящая программа `ostov-server` отдельным процессом: конфигурация из `config/`,
//! база — файл SQLite, сертификат создаётся при запуске; клиент подключается по сети.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use ostov_proto::pb;
use ostov_proto::pb::engine_client::EngineClient;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};

struct Process(Child, PathBuf);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_for(path: &Path, port: u16) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        if path.exists() && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("сервер не запустился за 30 секунд");
}

#[tokio::test]
async fn server_program_runs_with_yaml_config() {
    let dir = std::env::temp_dir().join(format!("ostov-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_ostov-server"))
        .args(["--config", config.to_str().unwrap()])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--tls", dir.join("tls").to_str().unwrap()])
        .env(
            "OSTOV_DB_URL",
            format!("sqlite:{}", dir.join("base.db").display()),
        )
        .stdout(std::fs::File::create(dir.join("out.txt")).unwrap())
        .stderr(std::fs::File::create(dir.join("err.txt")).unwrap())
        .spawn()
        .unwrap();
    let _server = Process(child, dir.clone());
    let cert = dir.join("tls").join(ostov_server::CERT_FILE);
    let check = cert.clone();
    tokio::task::spawn_blocking(move || wait_for(&check, port))
        .await
        .unwrap();

    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(std::fs::read(&cert).unwrap()))
        .domain_name("localhost");
    let channel = Endpoint::from_shared(format!("https://localhost:{port}"))
        .unwrap()
        .tls_config(tls)
        .unwrap()
        .connect()
        .await
        .unwrap();
    let out = std::fs::read_to_string(dir.join("out.txt")).unwrap();
    let err = std::fs::read_to_string(dir.join("err.txt")).unwrap();
    // В config/ есть receipt.py и issue.py: с Python сервер берёт их, без — предупреждает.
    if cfg!(feature = "python") {
        assert!(
            out.contains("логика документов: Python"),
            "{out}
{err}"
        );
    } else {
        assert!(out.contains("логика документов: встроенная"), "{out}");
        assert!(err.contains("собран без Python"), "{err}");
    }
    let mut client = EngineClient::new(channel);
    let hello = client.hello(pb::Empty {}).await.unwrap().into_inner();
    assert_eq!(hello.config_name, "Склад");
    assert!(hello.first_run);

    let admin = client
        .create_first_admin(pb::Credentials {
            login: "boss".into(),
            password: "admin-password-1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut request = tonic::Request::new(pb::Empty {});
    request.metadata_mut().insert(
        ostov_proto::AUTH_HEADER,
        format!("Bearer {}", admin.token).parse().unwrap(),
    );
    let meta = client.get_meta(request).await.unwrap().into_inner();
    // Заголовки из YAML, а не из встроенного «Склада».
    let receipt = meta.objects.iter().find(|o| o.name == "receipt").unwrap();
    assert_eq!(receipt.title, "Поступление");
}

#[test]
fn server_program_explains_bad_start() {
    let run = |args: &[&str], db: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ostov-server"));
        command.args(args).env_remove("OSTOV_DB_URL");
        if let Some(db) = db {
            command.env("OSTOV_DB_URL", db);
        }
        let out = command.output().unwrap();
        assert!(!out.status.success());
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    assert!(run(&[], None).contains("папка конфигурации"));
    assert!(run(&["--config", "config"], None).contains("OSTOV_DB_URL"));
    let missing = std::env::temp_dir().join("ostov-нет-такой-папки");
    assert!(
        run(
            &["--config", missing.to_str().unwrap()],
            Some("sqlite:never.db")
        )
        .contains("конфигурации")
    );
}
