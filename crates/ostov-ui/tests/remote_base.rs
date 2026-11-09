//! Окно «Подключиться к серверу» против настоящего сервера на localhost по TLS: те же вызовы,
//! что делает интерфейс (`remote_base::check`, `create`, затем `login`).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use ostov_api::{ErrorCode, NewAdmin};
use ostov_core::local::warehouse_model;
use ostov_server::{Base, Connect, Hub, Tls, serve};
use ostov_store::Driver;
use ostov_ui::remote_base::{check, create};

const STRONG: &str = "длинный-пароль-1";

struct Server {
    port: u16,
    cert: String,
    dir: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn address(&self) -> String {
        format!("localhost:{}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("ostov-ui-server-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("base.db");
    let connect: Connect = Arc::new(move || {
        let db = ostov_store::drivers::sqlite::SqliteDriver::open(&path)?;
        Ok(Box::new(db) as Box<dyn Driver + Send>)
    });
    let tls = Tls::load_or_create(&dir.join("tls"), &["localhost".to_owned()]).unwrap();
    let cert = tls.cert_pem.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (ready, port) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let hub = Arc::new(Hub::start(Base::new(connect, warehouse_model())).unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ready.send(listener.local_addr().unwrap().port()).unwrap();
            serve(listener, hub, &tls, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
    });
    Server {
        port: port.recv().unwrap(),
        cert,
        dir,
        stop: Some(stop),
        thread: Some(thread),
    }
}

fn admin(password: &str) -> NewAdmin {
    NewAdmin {
        login: "admin".into(),
        password: password.into(),
    }
}

#[test]
fn first_run_then_ordinary_login() {
    let server = start("first-run");
    let (_, hello) = check(&server.address(), &server.cert).unwrap();
    assert!(hello.first_run, "новый сервер ждёт первого администратора");

    let engine = create(&server.address(), &server.cert, &admin(STRONG)).unwrap();
    assert!(
        engine.get_meta().is_ok(),
        "после создания сессия уже открыта"
    );
    drop(engine);

    let (engine, hello) = check(&server.address(), &server.cert).unwrap();
    assert!(!hello.first_run);
    assert!(hello.login_required);
    assert!(engine.login("admin", "не тот пароль-1").is_err());
    let user = engine.login("admin", STRONG).unwrap();
    assert_eq!(user.login, "admin");
}

// Сервер не даёт короткий и пустой пароль (СТ-32), в интерфейсе это не смягчить.
#[test]
fn server_refuses_short_and_empty_password() {
    let server = start("short");
    for password in ["", "коротко", "11-символов"] {
        let error = create(&server.address(), &server.cert, &admin(password));
        assert!(error.is_err(), "пароль «{password}» не должен приниматься");
    }
    let (_, hello) = check(&server.address(), &server.cert).unwrap();
    assert!(
        hello.first_run,
        "после отказов администратора по-прежнему нет"
    );
}

#[test]
fn foreign_certificate_is_rejected() {
    let server = start("trusted");
    let other = start("foreign");
    let result = check(&server.address(), &other.cert);
    assert!(result.is_err(), "сертификат другого сервера не принимается");
}

#[test]
fn unreachable_address_reports_unavailable() {
    let server = start("down");
    let (address, cert) = (server.address(), server.cert.clone());
    drop(server);
    let error = check(&address, &cert).err().unwrap();
    assert_eq!(error.code, ErrorCode::Unavailable, "{error:?}");
}
