//! Клиент через трейт `Engine` — как его зовёт интерфейс — против настоящего сервера на
//! localhost по TLS. База — SQLite; при заданном `OSTOV_PG_URL` сценарий идёт и на PostgreSQL.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use ostov_api::{
    Connector, Engine, ErrorCode, Fields, ListQuery, NewAdmin, ObjectData, PostAction, ReportQuery,
    Value,
};
use ostov_client::RemoteConnector;
use ostov_core::local::warehouse_model;
use ostov_server::{Base, Connect, Hub, Tls, serve};
use ostov_store::Driver;

struct Server {
    port: u16,
    cert: String,
    dir: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
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

fn start(name: &str, connect: Option<Connect>) -> Server {
    let dir = std::env::temp_dir().join(format!("ostov-client-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let connect = connect.unwrap_or_else(|| {
        let path = dir.join("base.db");
        Arc::new(move || {
            let db = ostov_store::drivers::sqlite::SqliteDriver::open(&path)?;
            Ok(Box::new(db) as Box<dyn Driver + Send>)
        })
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

fn postgres(schema: &'static str) -> Option<Connect> {
    let url = std::env::var("OSTOV_PG_URL").ok()?;
    {
        let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url).unwrap();
        db.execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
            .unwrap();
        db.execute(&format!("CREATE SCHEMA {schema}"), &[]).unwrap();
    }
    Some(Arc::new(move || {
        let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url)?;
        db.execute(&format!("SET search_path TO {schema}"), &[])?;
        Ok(Box::new(db) as Box<dyn Driver + Send>)
    }))
}

fn admin() -> NewAdmin {
    NewAdmin {
        login: "boss".into(),
        password: "admin-password-1".into(),
    }
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn add(engine: &dyn Engine, object: &str, name: &str) -> i64 {
    let data = ObjectData {
        fields: fields(&[("name", Value::Text(name.into()))]),
        ..ObjectData::default()
    };
    engine.save(object, &data, PostAction::Write).unwrap().id
}

fn refer(id: i64) -> Value {
    Value::Ref { id, title: None }
}

fn dec(units: i64, scale: u8) -> Value {
    Value::Decimal { units, scale }
}

fn query(object: &str) -> ListQuery {
    ListQuery {
        object: object.into(),
        size: 100,
        ..ListQuery::default()
    }
}

/// Тот же сценарий, что интерфейс проходит с локальной базой, — только через сервер.
fn warehouse_over_network(connect: Option<Connect>, name: &str) {
    let server = start(name, connect);
    let address = format!("localhost:{}", server.port);
    let connector = RemoteConnector::new(server.cert.clone());

    let fresh = connector.connect_remote(&address).unwrap();
    assert!(fresh.hello().unwrap().first_run);
    assert_eq!(
        fresh.list(&query("goods")).unwrap_err().code,
        ErrorCode::NeedFirstAdmin
    );
    drop(fresh);

    let engine = connector.create_remote(&address, "", &admin()).unwrap();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    let receipt = ObjectData {
        fields: fields(&[
            ("date", Value::Date("2026-02-01".into())),
            ("supplier", refer(mill)),
            ("warehouse", refer(main)),
        ]),
        tables: [(
            "lines".to_owned(),
            vec![fields(&[
                ("goods", refer(flour)),
                ("qty", dec(2_500, 3)),
                ("price", dec(333, 2)),
            ])],
        )]
        .into(),
        ..ObjectData::default()
    };
    let saved = engine.save("receipt", &receipt, PostAction::Post).unwrap();
    let doc = engine.get("receipt", saved.id).unwrap();
    assert_eq!(doc.fields["amount"], dec(833, 2));
    assert!(doc.posted);

    let found = engine
        .list(&ListQuery {
            search: Some("МЕЛЬН".into()),
            ..query("receipt")
        })
        .unwrap();
    assert_eq!(found.rows.len(), 1);
    assert_eq!(
        found.rows[0].values["supplier"],
        Value::Ref {
            id: mill,
            title: Some("Мельница".into())
        }
    );
    let seek = engine.seek(&found.token, 0, 10).unwrap();
    assert_eq!(seek.rows.len(), 1);

    let balance = engine
        .run_report(&ReportQuery {
            report: "stock_balance".into(),
            params: Fields::new(),
            after_key: None,
            size: 10,
        })
        .unwrap();
    assert_eq!(balance.rows[0].values["qty"], dec(2_500, 3));

    let meta = engine.get_meta().unwrap();
    assert_eq!(meta.objects.len(), 5);
    engine
        .save_settings("receipt.list", &fields(&[("rows", Value::Int(20))]))
        .unwrap();
    assert_eq!(
        engine.get_settings("receipt.list").unwrap(),
        Some(fields(&[("rows", Value::Int(20))]))
    );
    assert_eq!(engine.get_settings("нет").unwrap(), None);
    engine.set_closing_date(Some("2026-01-31")).unwrap();
    assert_eq!(
        engine.closing_date().unwrap().as_deref(),
        Some("2026-01-31")
    );
    let journal = engine.admin_journal(None, 100).unwrap();
    assert!(journal.iter().any(|e| e.action == "Проведение"));
    assert_eq!(engine.admin_users().unwrap().len(), 1);

    // Ошибка ядра приходит как локально.
    let err = engine.post("receipt", 999).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // Выход и вход заново.
    engine.logout().unwrap();
    assert_eq!(
        engine.list(&query("goods")).unwrap_err().code,
        ErrorCode::BadCredentials
    );
    assert_eq!(
        engine.login("boss", "wrong-password-1").unwrap_err().code,
        ErrorCode::BadCredentials
    );
    let session = engine.login("boss", "admin-password-1").unwrap();
    assert_eq!(session.roles, ["admin"]);
    assert_eq!(engine.list(&query("goods")).unwrap().rows.len(), 1);
}

#[test]
fn warehouse_over_network_sqlite() {
    warehouse_over_network(None, "flow");
}

#[test]
fn warehouse_over_network_postgres() {
    let Some(connect) = postgres("ostov_t_client") else {
        return;
    };
    warehouse_over_network(Some(connect), "flow-pg");
}

#[test]
fn connection_problems_are_explained() {
    let server = start("problems", None);
    let connector = RemoteConnector::new(server.cert.clone());

    let wrong_port = connector.connect_remote("localhost:1").err().unwrap();
    assert_eq!(wrong_port.code, ErrorCode::Unavailable);
    assert!(wrong_port.message.starts_with("Нет связи с сервером"));

    let stranger = std::env::temp_dir().join(format!("ostov-client-{}-other", std::process::id()));
    let other = Tls::load_or_create(&stranger, &["localhost".to_owned()]).unwrap();
    let untrusted = RemoteConnector::new(other.cert_pem)
        .connect_remote(&format!("localhost:{}", server.port))
        .err()
        .unwrap();
    assert_eq!(untrusted.code, ErrorCode::Unavailable);
    let _ = std::fs::remove_dir_all(stranger);

    assert_eq!(
        connector
            .connect_remote("localhost:порт")
            .err()
            .unwrap()
            .code,
        ErrorCode::Validation
    );
}
