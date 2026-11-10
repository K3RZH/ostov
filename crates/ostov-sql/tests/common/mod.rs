//! Общее для тестов на живой базе: «Склад» через ostov-core, данные через Engine,
//! запросы через `prepare` и драйвер.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use ostov_api::{Engine, Fields, ObjectData, PostAction, Value as ApiValue};
use ostov_core::LocalEngine;
use ostov_core::local::Config;
use ostov_sql::{Param, Prepared, SqlError, prepare};
use ostov_store::drivers::postgres::PostgresDriver;
use ostov_store::drivers::sqlite::SqliteDriver;
use ostov_store::{Driver, Row, Value};

pub fn model() -> ostov_model::Config {
    ostov_meta::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"))
        .unwrap()
}

/// Что лежит в базе после `fill`.
pub struct Data {
    pub flour: i64,
    pub sugar: i64,
    pub main: i64,
    pub spare: i64,
    pub mill: i64,
    pub bakery: i64,
    pub r1: i64,
    pub r2: i64,
    pub r3: i64,
}

fn fields(pairs: &[(&str, ApiValue)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn refer(id: i64) -> ApiValue {
    ApiValue::Ref { id, title: None }
}

fn dec(units: i64, scale: u8) -> ApiValue {
    ApiValue::Decimal { units, scale }
}

fn add(engine: &dyn Engine, object: &str, name: &str) -> i64 {
    let data = ObjectData {
        fields: fields(&[("name", ApiValue::Text(name.to_owned()))]),
        ..ObjectData::default()
    };
    engine.save(object, &data, PostAction::Write).unwrap().id
}

/// Строки: (товар, количество в тысячных, цена в копейках).
fn receipt(
    engine: &dyn Engine,
    day: &str,
    warehouse: i64,
    supplier: i64,
    lines: &[(i64, i64, i64)],
) -> i64 {
    let lines = lines
        .iter()
        .map(|&(goods, qty, price)| {
            fields(&[
                ("goods", refer(goods)),
                ("qty", dec(qty, 3)),
                ("price", dec(price, 2)),
            ])
        })
        .collect();
    let data = ObjectData {
        fields: fields(&[
            ("date", ApiValue::Date(day.into())),
            ("warehouse", refer(warehouse)),
            ("supplier", refer(supplier)),
        ]),
        tables: [("lines".to_owned(), lines)].into(),
        ..ObjectData::default()
    };
    engine.save("receipt", &data, PostAction::Post).unwrap().id
}

fn issue(engine: &dyn Engine, day: &str, warehouse: i64, goods: i64, qty: i64) {
    let data = ObjectData {
        fields: fields(&[
            ("date", ApiValue::Date(day.into())),
            ("warehouse", refer(warehouse)),
        ]),
        tables: [(
            "lines".to_owned(),
            vec![fields(&[("goods", refer(goods)), ("qty", dec(qty, 3))])],
        )]
        .into(),
        ..ObjectData::default()
    };
    engine.save("issue", &data, PostAction::Post).unwrap();
}

/// Два поступления от «Мельницы», одно от «Бакалеи», один расход.
pub fn fill(engine: &dyn Engine) -> Data {
    let flour = add(engine, "goods", "Мука");
    let sugar = add(engine, "goods", "Сахар");
    let main = add(engine, "warehouses", "Основной");
    let spare = add(engine, "warehouses", "Запасной");
    let mill = add(engine, "suppliers", "Мельница");
    let bakery = add(engine, "suppliers", "Бакалея");
    // 2 кг по 10 ₽ и 1 кг по 50 ₽ = 70 ₽.
    let r1 = receipt(
        engine,
        "2026-11-01",
        main,
        mill,
        &[(flour, 2000, 1000), (sugar, 1000, 5000)],
    );
    // 3 кг по 10 ₽ = 30 ₽.
    let r2 = receipt(engine, "2026-11-02", main, mill, &[(flour, 3000, 1000)]);
    // 5 кг по 4 ₽ = 20 ₽, на другой склад.
    let r3 = receipt(engine, "2026-11-03", spare, bakery, &[(sugar, 5000, 400)]);
    issue(engine, "2026-11-04", main, flour, 1500);
    Data {
        flour,
        sugar,
        main,
        spare,
        mill,
        bakery,
        r1,
        r2,
        r3,
    }
}

/// Папка с файлом SQLite; удаляется после теста.
pub struct SqliteBase {
    dir: PathBuf,
}

impl SqliteBase {
    /// База «Склада» с данными `fill`.
    pub fn new(test: &str) -> (Self, Data) {
        let dir = std::env::temp_dir().join(format!("ostov-sql-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = Self { dir };
        let db = SqliteDriver::open(base.path()).unwrap();
        let engine =
            LocalEngine::install(Box::new(db), Config::from_model(&model()).unwrap()).unwrap();
        let data = fill(&engine);
        drop(engine);
        (base, data)
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join("shop.db")
    }

    pub fn open(&self) -> SqliteDriver {
        SqliteDriver::open(self.path()).unwrap()
    }
}

impl Drop for SqliteBase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn pg_connect(url: &str, schema: &str, fresh: bool) -> PostgresDriver {
    let mut db = PostgresDriver::connect(url).unwrap();
    if fresh {
        db.execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
            .unwrap();
        db.execute(&format!("CREATE SCHEMA {schema}"), &[]).unwrap();
    }
    db.execute(&format!("SET search_path TO {schema}"), &[])
        .unwrap();
    db
}

/// База «Склада» в своей схеме PostgreSQL; `None` без `OSTOV_PG_URL`.
pub fn postgres(schema: &str) -> Option<(PostgresDriver, Data)> {
    let url = std::env::var("OSTOV_PG_URL").ok()?;
    let db = pg_connect(&url, schema, true);
    let engine = LocalEngine::install(Box::new(db), Config::from_model(&model()).unwrap()).unwrap();
    let data = fill(&engine);
    drop(engine);
    Some((pg_connect(&url, schema, false), data))
}

pub fn params(pairs: &[(&str, Value)]) -> BTreeMap<String, Param> {
    pairs
        .iter()
        .map(|(k, v)| {
            let p = match v {
                Value::Null => Param::Null,
                Value::Int(n) => Param::Int(*n),
                Value::Text(t) => Param::Text(t.clone()),
                Value::Bytes(_) => panic!("байты параметром не передаются"),
            };
            ((*k).to_owned(), p)
        })
        .collect()
}

pub fn try_prepare(
    db: &dyn Driver,
    query: &str,
    p: &[(&str, Value)],
) -> Result<Prepared, SqlError> {
    prepare(query, &params(p), &model(), &|_| true, db.dialect())
}

/// Проверить запрос и выполнить его через драйвер.
pub fn run(db: &mut dyn Driver, query: &str, p: &[(&str, Value)]) -> Vec<Row> {
    let prepared = try_prepare(db, query, p).unwrap();
    db.query(&prepared.sql, &prepared.params)
        .unwrap_or_else(|e| panic!("{query}\n{}\n{e:?}", prepared.sql))
}

pub fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}
