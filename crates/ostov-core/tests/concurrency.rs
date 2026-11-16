//! Одновременная работа (СТ-57, НТ-Н3): несколько подключений к одной базе, как у сервера.
//! PostgreSQL — если задан `OSTOV_PG_URL`, иначе тесты видны как ignored; SQLite — всегда.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ostov_api::{Engine, ErrorCode, Fields, ListQuery, ObjectData, PostAction, Value};
use ostov_core::LocalEngine;
use ostov_core::local::{Config, warehouse_model};
use ostov_store::Driver;
use ostov_store::drivers::postgres::PostgresDriver;
use ostov_store::drivers::sqlite::SqliteDriver;

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

/// Документ на одну строку: приход с поставщиком или расход без него.
fn doc(warehouse: i64, supplier: Option<i64>, goods: i64, qty: i64) -> ObjectData {
    let mut header = fields(&[
        ("date", Value::Date("2026-03-01".into())),
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

fn stock(engine: &dyn Engine, goods: i64) -> Value {
    let found = engine
        .query(
            "SELECT SUM(qty * kind) AS qty FROM stock WHERE goods = :g",
            &fields(&[("g", Value::Int(goods))]),
        )
        .unwrap();
    found.rows[0][0].clone()
}

/// Два движка на одну базу, как две сессии сервера.
struct Pair {
    a: Arc<LocalEngine>,
    b: Arc<LocalEngine>,
}

fn pg(schema: &str) -> PostgresDriver {
    let url = std::env::var("OSTOV_PG_URL").unwrap();
    let mut db = PostgresDriver::connect(&url).unwrap();
    db.execute(&format!("SET search_path TO {schema}"), &[])
        .unwrap();
    db
}

fn pg_pair(schema: &str) -> Pair {
    let url = std::env::var("OSTOV_PG_URL").unwrap();
    let mut fresh = PostgresDriver::connect(&url).unwrap();
    fresh
        .execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
        .unwrap();
    fresh
        .execute(&format!("CREATE SCHEMA {schema}"), &[])
        .unwrap();
    drop(fresh);
    let config = || Config::from_model(&warehouse_model()).unwrap();
    let a = LocalEngine::install(Box::new(pg(schema)), config()).unwrap();
    let b = LocalEngine::new(Box::new(pg(schema)), config());
    Pair {
        a: Arc::new(a),
        b: Arc::new(b),
    }
}

fn sqlite_pair(name: &str) -> (Pair, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("ostov-conc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shop.db");
    let config = || Config::from_model(&warehouse_model()).unwrap();
    let a = LocalEngine::install(Box::new(SqliteDriver::open(&path).unwrap()), config()).unwrap();
    let b = LocalEngine::new(Box::new(SqliteDriver::open(&path).unwrap()), config());
    (
        Pair {
            a: Arc::new(a),
            b: Arc::new(b),
        },
        dir,
    )
}

/// Оба движка одновременно делают своё; результаты по порядку.
fn together<T: Send + 'static>(
    pair: &Pair,
    a: impl FnOnce(&LocalEngine) -> T + Send + 'static,
    b: impl FnOnce(&LocalEngine) -> T + Send + 'static,
) -> (T, T) {
    let start = Arc::new(Barrier::new(2));
    let (ea, eb) = (Arc::clone(&pair.a), Arc::clone(&pair.b));
    let (sa, sb) = (Arc::clone(&start), Arc::clone(&start));
    let ta = std::thread::spawn(move || {
        sa.wait();
        a(&ea)
    });
    let tb = std::thread::spawn(move || {
        sb.wait();
        b(&eb)
    });
    (ta.join().unwrap(), tb.join().unwrap())
}

/// Приходы нового товара с двух сессий сразу: оба проходят, итог — сумма.
fn new_key_from_two_sessions(pair: &Pair) {
    let main = add(&*pair.a, "warehouses", "Основной");
    let mill = add(&*pair.a, "suppliers", "Мельница");
    for n in 0..10 {
        let goods = add(&*pair.a, "goods", &format!("товар {n}"));
        let (ra, rb) = together(
            pair,
            move |e| {
                e.save(
                    "receipt",
                    &doc(main, Some(mill), goods, 1_000),
                    PostAction::Post,
                )
            },
            move |e| {
                e.save(
                    "receipt",
                    &doc(main, Some(mill), goods, 2_000),
                    PostAction::Post,
                )
            },
        );
        ra.unwrap();
        rb.unwrap();
        assert_eq!(
            stock(&*pair.a, goods),
            Value::Decimal {
                units: 3_000,
                scale: 3
            }
        );
    }
}

/// Остаток 10, два расхода по 7 сразу: проходит ровно один, в минус не уходит.
fn no_oversell_from_two_sessions(pair: &Pair) {
    let main = add(&*pair.a, "warehouses", "Основной");
    let mill = add(&*pair.a, "suppliers", "Мельница");
    for n in 0..10 {
        let goods = add(&*pair.a, "goods", &format!("мука {n}"));
        pair.a
            .save(
                "receipt",
                &doc(main, Some(mill), goods, 10_000),
                PostAction::Post,
            )
            .unwrap();
        let (ra, rb) = together(
            pair,
            move |e| e.save("issue", &doc(main, None, goods, 7_000), PostAction::Post),
            move |e| e.save("issue", &doc(main, None, goods, 7_000), PostAction::Post),
        );
        let failed: Vec<_> = [ra, rb].into_iter().filter_map(Result::err).collect();
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert_eq!(failed[0].code, ErrorCode::Validation, "{failed:?}");
        assert_eq!(
            stock(&*pair.a, goods),
            Value::Decimal {
                units: 3_000,
                scale: 3
            }
        );
    }
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn new_key_from_two_sessions_on_postgres() {
    new_key_from_two_sessions(&pg_pair("ostov_t_conc_new"));
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn no_oversell_on_postgres() {
    no_oversell_from_two_sessions(&pg_pair("ostov_t_conc_oversell"));
}

#[test]
fn new_key_from_two_sessions_on_sqlite() {
    let (pair, dir) = sqlite_pair("new");
    new_key_from_two_sessions(&pair);
    drop(pair);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_oversell_on_sqlite() {
    let (pair, dir) = sqlite_pair("oversell");
    no_oversell_from_two_sessions(&pair);
    drop(pair);
    let _ = std::fs::remove_dir_all(dir);
}

/// Пока чужая транзакция держит итог товара X: проведение по товару Y, списки, отчёты
/// и запросы не ждут; проведение по X ждёт её, а не ломается.
#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn only_same_key_waits_on_postgres() {
    let schema = "ostov_t_conc_keys";
    let pair = pg_pair(schema);
    let main = add(&*pair.a, "warehouses", "Основной");
    let mill = add(&*pair.a, "suppliers", "Мельница");
    let x = add(&*pair.a, "goods", "X");
    let y = add(&*pair.a, "goods", "Y");
    for goods in [x, y] {
        pair.a
            .save(
                "receipt",
                &doc(main, Some(mill), goods, 10_000),
                PostAction::Post,
            )
            .unwrap();
    }

    let mut holder = pg(schema);
    holder.begin().unwrap();
    holder
        .query(
            "SELECT qty FROM stock__totals WHERE warehouse = $1 AND goods = $2 FOR UPDATE",
            &[ostov_store::Value::Int(main), ostov_store::Value::Int(x)],
        )
        .unwrap();

    let quick = |what: &str, f: &dyn Fn()| {
        let started = Instant::now();
        f();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{what} ждал {:?}",
            started.elapsed()
        );
    };
    quick("расход Y", &|| {
        pair.b
            .save("issue", &doc(main, None, y, 1_000), PostAction::Post)
            .unwrap();
    });
    quick("список", &|| {
        pair.b
            .list(&ListQuery {
                object: "issue".into(),
                size: 10,
                ..ListQuery::default()
            })
            .unwrap();
    });
    quick("запрос", &|| {
        stock(&*pair.b, x);
    });

    // Расход X ждёт, пока держатель не отпустит итог, и проходит после.
    let b = Arc::clone(&pair.b);
    let waiting = std::thread::spawn(move || {
        let started = Instant::now();
        let saved = b.save("issue", &doc(main, None, x, 1_000), PostAction::Post);
        (saved, started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(700));
    holder.rollback().unwrap();
    let (saved, waited) = waiting.join().unwrap();
    saved.unwrap();
    assert!(waited >= Duration::from_millis(500), "не ждал: {waited:?}");
    assert_eq!(
        stock(&*pair.a, x),
        Value::Decimal {
            units: 9_000,
            scale: 3
        }
    );
}

/// Расход «A, потом B» и расход «B, потом A» с двух сессий сразу: оба проходят,
/// никто не ждёт другого вечно (итоги блокируются в одном порядке).
fn opposite_order(pair: &Pair) {
    let main = add(&*pair.a, "warehouses", "Основной");
    let mill = add(&*pair.a, "suppliers", "Мельница");
    let a = add(&*pair.a, "goods", "A");
    let b = add(&*pair.a, "goods", "B");
    for goods in [a, b] {
        pair.a
            .save(
                "receipt",
                &doc(main, Some(mill), goods, 100_000),
                PostAction::Post,
            )
            .unwrap();
    }
    let two = move |first: i64, second: i64| {
        let mut data = doc(main, None, first, 1_000);
        let line = fields(&[
            ("goods", refer(second)),
            (
                "qty",
                Value::Decimal {
                    units: 1_000,
                    scale: 3,
                },
            ),
        ]);
        data.tables.get_mut("lines").unwrap().push(line);
        data
    };
    for _ in 0..10 {
        let (ra, rb) = together(
            pair,
            move |e| e.save("issue", &two(a, b), PostAction::Post),
            move |e| e.save("issue", &two(b, a), PostAction::Post),
        );
        ra.unwrap();
        rb.unwrap();
    }
    for goods in [a, b] {
        assert_eq!(
            stock(&*pair.a, goods),
            Value::Decimal {
                units: 80_000,
                scale: 3
            }
        );
    }
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn opposite_order_on_postgres() {
    opposite_order(&pg_pair("ostov_t_conc_order"));
}

#[test]
fn opposite_order_on_sqlite() {
    let (pair, dir) = sqlite_pair("order");
    opposite_order(&pair);
    drop(pair);
    let _ = std::fs::remove_dir_all(dir);
}
