//! НТ-Н3: десять клиентов одновременно через сервер — проведение, списки, отчёт остатков.
//! Ни одна успешная запись не теряется, остатки сходятся с движениями. Это прогон, а не
//! проверка в CI: `OSTOV_LOAD_SECS=3600 cargo test --release -p ostov-client --test load
//! -- --ignored --nocapture` (по умолчанию 30 с; с `OSTOV_PG_URL` — и на PostgreSQL).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use ostov_api::{
    Connector, Engine, ErrorCode, Fields, ListQuery, NewAdmin, NewUser, ObjectData, PostAction,
    ReportQuery, Value,
};
use ostov_client::RemoteConnector;
use ostov_core::local::warehouse_model;
use ostov_server::{Base, Connect, Hub, Tls, serve};
use ostov_store::Driver;

const CLIENTS: usize = 10;
const GOODS: usize = 5;
const PASSWORD: &str = "load-password-1";

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
    let dir = std::env::temp_dir().join(format!("ostov-load-{}-{name}", std::process::id()));
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

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn refer(id: i64) -> Value {
    Value::Ref { id, title: None }
}

fn add(engine: &dyn Engine, object: &str, name: &str) -> i64 {
    let data = ObjectData {
        fields: fields(&[("name", Value::Text(name.into()))]),
        ..ObjectData::default()
    };
    engine.save(object, &data, PostAction::Write).unwrap().id
}

fn doc(warehouse: i64, supplier: Option<i64>, goods: i64, qty: i64) -> ObjectData {
    let mut header = fields(&[
        ("date", Value::Date("2026-04-01".into())),
        ("warehouse", refer(warehouse)),
    ]);
    if let Some(supplier) = supplier {
        header.insert("supplier".into(), refer(supplier));
    }
    let line = fields(&[
        ("goods", refer(goods)),
        (
            "qty",
            Value::Decimal {
                units: qty,
                scale: 3,
            },
        ),
    ]);
    ObjectData {
        fields: header,
        tables: [("lines".to_owned(), vec![line])].into(),
        ..ObjectData::default()
    }
}

#[derive(Default)]
struct Counts {
    saved: AtomicU64,
    short: AtomicU64,
    lists: AtomicU64,
    reports: AtomicU64,
}

fn run(name: &str, connect: Option<Connect>) {
    let secs: u64 = std::env::var("OSTOV_LOAD_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let server = start(name, connect);
    let address = format!("localhost:{}", server.port);
    let connector = RemoteConnector::new(server.cert.clone());
    let admin = NewAdmin {
        login: "boss".into(),
        password: PASSWORD.into(),
    };
    let boss = connector.create_remote(&address, "", &admin).unwrap();
    let main = add(&*boss, "warehouses", "Основной");
    let mill = add(&*boss, "suppliers", "Мельница");
    let goods: Vec<i64> = (0..GOODS)
        .map(|n| add(&*boss, "goods", &format!("товар {n}")))
        .collect();
    for n in 0..CLIENTS {
        boss.admin_add_user(&NewUser {
            login: format!("user{n}"),
            display_name: format!("Пользователь {n}"),
            role: "admin".into(),
            password: PASSWORD.into(),
        })
        .unwrap();
    }

    let counts = Arc::new(Counts::default());
    let until = Instant::now() + Duration::from_secs(secs);
    let workers: Vec<_> = (0..CLIENTS)
        .map(|n| {
            let (connector, address, goods, counts) = (
                connector.clone(),
                address.clone(),
                goods.clone(),
                Arc::clone(&counts),
            );
            std::thread::spawn(move || {
                let engine = connector.connect_remote(&address).unwrap();
                engine.login(&format!("user{n}"), PASSWORD).unwrap();
                let mut step = n;
                while Instant::now() < until {
                    step += 1;
                    // Товар меняется реже, чем вид операции: по каждому товару и приходы, и расходы.
                    let g = goods[(step / 5) % goods.len()];
                    match step % 5 {
                        0 | 1 => {
                            let data = doc(main, Some(mill), g, 5_000);
                            engine
                                .save("receipt", &data, PostAction::Post)
                                .unwrap_or_else(|e| panic!("приход: {e:?}"));
                            counts.saved.fetch_add(1, Ordering::Relaxed);
                        }
                        2 | 3 => {
                            match engine.save("issue", &doc(main, None, g, 3_000), PostAction::Post)
                            {
                                Ok(_) => {
                                    counts.saved.fetch_add(1, Ordering::Relaxed);
                                }
                                // Нехватка остатка — нормальный исход; остальное — провал прогона.
                                Err(e) if e.code == ErrorCode::Validation => {
                                    counts.short.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => panic!("расход: {e:?}"),
                            }
                        }
                        _ => {
                            let page = engine
                                .list(&ListQuery {
                                    object: "issue".into(),
                                    order: Some("number".into()),
                                    descending: true,
                                    size: 50,
                                    ..ListQuery::default()
                                })
                                .unwrap();
                            assert!(page.rows.len() <= 50);
                            counts.lists.fetch_add(1, Ordering::Relaxed);
                            engine
                                .run_report(&ReportQuery {
                                    report: "stock_balance".into(),
                                    params: Fields::new(),
                                    after_key: None,
                                    size: 100,
                                })
                                .unwrap();
                            counts.reports.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    println!(
        "НТ-Н3 {name}: нагрузка закончилась — записано {}, нехваток {}, списков {}, отчётов {}",
        counts.saved.load(Ordering::Relaxed),
        counts.short.load(Ordering::Relaxed),
        counts.lists.load(Ordering::Relaxed),
        counts.reports.load(Ordering::Relaxed),
    );

    // Администратор всё это время молчал: сессия без действий закрывается через 30 минут.
    boss.login("boss", PASSWORD).unwrap();
    // Ни одна успешная запись не потерялась, остатки сходятся с движениями.
    let docs = boss
        .query(
            "SELECT COUNT(id) AS n FROM receipt WHERE posted = 1",
            &Fields::new(),
        )
        .unwrap()
        .rows[0][0]
        .clone();
    let issues = boss
        .query(
            "SELECT COUNT(id) AS n FROM issue WHERE posted = 1",
            &Fields::new(),
        )
        .unwrap()
        .rows[0][0]
        .clone();
    let (Value::Int(docs), Value::Int(issues)) = (docs, issues) else {
        panic!("число документов");
    };
    let saved = counts.saved.load(Ordering::Relaxed);
    assert_eq!(u64::try_from(docs + issues).unwrap(), saved);
    let moved = boss
        .query(
            "SELECT goods, SUM(qty * kind) AS qty FROM stock GROUP BY goods ORDER BY goods",
            &Fields::new(),
        )
        .unwrap();
    let report = boss
        .run_report(&ReportQuery {
            report: "stock_balance".into(),
            params: Fields::new(),
            after_key: None,
            size: 100,
        })
        .unwrap();
    for row in &moved.rows {
        let Value::Ref { id, .. } = row[0] else {
            panic!("{row:?}");
        };
        let total = report
            .rows
            .iter()
            .find(|r| matches!(r.values["goods"], Value::Ref { id: g, .. } if g == id))
            .map(|r| r.values["qty"].clone());
        assert_eq!(total, Some(row[1].clone()), "товар {id}");
        assert!(!matches!(row[1], Value::Decimal { units, .. } if units < 0));
    }
    println!(
        "НТ-Н3 {name}: {CLIENTS} клиентов, {secs} с — записано {saved}, нехваток {}, списков {}, \
         отчётов {}, потерь 0",
        counts.short.load(Ordering::Relaxed),
        counts.lists.load(Ordering::Relaxed),
        counts.reports.load(Ordering::Relaxed),
    );
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

#[test]
#[ignore = "прогон: OSTOV_LOAD_SECS=3600 cargo test --release -p ostov-client --test load -- --ignored --nocapture"]
fn ten_clients_on_sqlite() {
    run("sqlite", None);
}

#[test]
#[ignore = "прогон на PostgreSQL: нужен OSTOV_PG_URL"]
fn ten_clients_on_postgres() {
    let Some(connect) = postgres("ostov_t_load") else {
        return;
    };
    run("postgres", Some(connect));
}
