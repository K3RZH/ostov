//! НТ-П14: проведение не замедляется с ростом истории регистра. Замер, а не проверка в CI:
//! время на общих машинах скачет. Запуск:
//! `cargo test --release -p ostov-core --test history_speed -- --ignored --nocapture`
//! (с `OSTOV_PG_URL` — и на PostgreSQL).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::time::{Duration, Instant};

use ostov_api::{Engine, Fields, ObjectData, PostAction, Value};
use ostov_core::LocalEngine;
use ostov_core::local::{Config, warehouse_model};
use ostov_store::Driver;

const LINES: usize = 100;
/// Движений на каждый товар документа: 100 × 10 000 = 1 млн.
const HISTORY: i64 = 10_000;
const RUNS: usize = 7;

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

fn document(
    day: &str,
    warehouse: i64,
    supplier: Option<i64>,
    goods: &[i64],
    qty: i64,
) -> ObjectData {
    let mut header = fields(&[
        ("date", Value::Date(day.into())),
        ("warehouse", refer(warehouse)),
    ]);
    if let Some(supplier) = supplier {
        header.insert("supplier".into(), refer(supplier));
    }
    let lines = goods
        .iter()
        .map(|g| {
            fields(&[
                ("goods", refer(*g)),
                (
                    "qty",
                    Value::Decimal {
                        units: qty,
                        scale: 3,
                    },
                ),
            ])
        })
        .collect();
    ObjectData {
        fields: header,
        tables: [("lines".to_owned(), lines)].into(),
        ..ObjectData::default()
    }
}

/// Склад, поставщик и товары документа, у каждого товара приход на 1 000.
fn prepare(engine: &LocalEngine) -> (i64, Vec<i64>) {
    let main = add(engine, "warehouses", "Основной");
    let mill = add(engine, "suppliers", "Мельница");
    let goods: Vec<i64> = (0..LINES)
        .map(|n| add(engine, "goods", &format!("товар {n}")))
        .collect();
    engine
        .save(
            "receipt",
            &document("2026-01-01", main, Some(mill), &goods, 1_000_000),
            PostAction::Post,
        )
        .unwrap();
    (main, goods)
}

/// Медиана проведения расхода на 100 строк после последнего дня истории.
fn post_issue(engine: &LocalEngine, warehouse: i64, goods: &[i64]) -> Duration {
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let data = document("2026-12-01", warehouse, None, goods, 1);
            let started = Instant::now();
            engine.save("issue", &data, PostAction::Post).unwrap();
            started.elapsed()
        })
        .collect();
    times.sort();
    times[RUNS / 2]
}

/// История: по `HISTORY` приходов на каждый товар документа за 2025 год, итоги сходятся.
fn add_history(db: &mut dyn Driver, warehouse: i64, goods: &[i64]) {
    db.begin().unwrap();
    let d = db.dialect();
    let insert = format!(
        "INSERT INTO stock (recorder, recorder_id, kind, period, warehouse, goods, qty) \
         VALUES ('receipt', {}, 1, {}, {}, {}, 1000)",
        d.placeholder(1),
        d.placeholder(2),
        d.placeholder(3),
        d.placeholder(4),
    );
    let update = format!(
        "UPDATE stock__totals SET qty = qty + {} WHERE warehouse = {} AND goods = {}",
        d.placeholder(1),
        d.placeholder(2),
        d.placeholder(3),
    );
    for (n, g) in goods.iter().enumerate() {
        for i in 0..HISTORY {
            let day = format!("2025-{:02}-{:02}", 1 + i % 12, 1 + i % 28);
            let id = i64::try_from(n).unwrap() * HISTORY + i + 1_000_000;
            db.execute(
                &insert,
                &[
                    id.into(),
                    ostov_store::Value::Text(day),
                    warehouse.into(),
                    (*g).into(),
                ],
            )
            .unwrap();
        }
        db.execute(
            &update,
            &[(HISTORY * 1000).into(), warehouse.into(), (*g).into()],
        )
        .unwrap();
    }
    db.commit().unwrap();
}

fn measure(name: &str, open: impl Fn() -> Box<dyn Driver + Send>) {
    let config = || Config::from_model(&warehouse_model()).unwrap();
    let empty = LocalEngine::install(open(), config()).unwrap();
    let (main, goods) = prepare(&empty);
    let before = post_issue(&empty, main, &goods);
    drop(empty);

    let mut db = open();
    let started = Instant::now();
    add_history(&mut *db, main, &goods);
    let filled = started.elapsed();
    drop(db);
    let engine = LocalEngine::new(open(), config());
    let after = post_issue(&engine, main, &goods);
    let ratio = after.as_secs_f64() / before.as_secs_f64();
    println!(
        "НТ-П14 {name}: пустая база {before:?}, после 1 млн движений {after:?}, ×{ratio:.2} \
         (история записана за {filled:?})"
    );
    assert!(ratio <= 2.0, "проведение замедлилось в {ratio:.2} раза");
}

#[test]
#[ignore = "замер: cargo test --release -p ostov-core --test history_speed -- --ignored --nocapture"]
fn posting_does_not_slow_with_history_sqlite() {
    let dir = std::env::temp_dir().join(format!("ostov-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shop.db");
    measure("SQLite", || {
        Box::new(ostov_store::drivers::sqlite::SqliteDriver::open(&path).unwrap())
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "замер на PostgreSQL: нужен OSTOV_PG_URL"]
fn posting_does_not_slow_with_history_postgres() {
    let Ok(url) = std::env::var("OSTOV_PG_URL") else {
        return;
    };
    let schema = "ostov_t_history";
    {
        let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url).unwrap();
        db.execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
            .unwrap();
        db.execute(&format!("CREATE SCHEMA {schema}"), &[]).unwrap();
    }
    measure("PostgreSQL", || {
        let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url).unwrap();
        db.execute(&format!("SET search_path TO {schema}"), &[])
            .unwrap();
        Box::new(db)
    });
}
