//! Сортировка списка по нескольким колонкам через `Engine`: страницы и прыжки `seek`.
//! На SQLite всегда, на PostgreSQL — если задан `OSTOV_PG_URL`, иначе тест виден как ignored.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::cmp::Reverse;

use ostov_api::{Engine, Fields, ListQuery, ObjectData, PostAction, Row, SortKey, Value};
use ostov_core::LocalEngine;
use ostov_core::local::{Config, warehouse_model};
use ostov_store::Driver;

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

/// 40 поступлений: три поставщика и без поставщика, пять дат.
fn fill(engine: &dyn Engine) {
    let main = add(engine, "warehouses", "Основной");
    let suppliers = [
        add(engine, "suppliers", "Мельница"),
        add(engine, "suppliers", "Агро"),
        add(engine, "suppliers", "Бакалея"),
    ];
    for i in 0..40_usize {
        let mut header = fields(&[
            ("date", Value::Date(format!("2026-01-0{}", 1 + i % 5))),
            (
                "warehouse",
                Value::Ref {
                    id: main,
                    title: None,
                },
            ),
        ]);
        if i % 4 != 0 {
            let id = suppliers[i % 3];
            header.insert("supplier".into(), Value::Ref { id, title: None });
        }
        let data = ObjectData {
            fields: header,
            ..ObjectData::default()
        };
        engine.save("receipt", &data, PostAction::Write).unwrap();
    }
}

fn query(size: u32) -> ListQuery {
    ListQuery {
        object: "receipt".into(),
        order: Some("supplier".into()),
        then_by: vec![SortKey {
            field: "date".into(),
            descending: true,
        }],
        size,
        ..ListQuery::default()
    }
}

/// Поставщик по названию (пустой в конце), дата по убыванию, потом номер записи.
fn expected(engine: &dyn Engine) -> Vec<i64> {
    let all = engine
        .list(&ListQuery {
            object: "receipt".into(),
            size: 1000,
            ..ListQuery::default()
        })
        .unwrap()
        .rows;
    let key = |r: &Row| {
        let supplier = match &r.values["supplier"] {
            Value::Ref { title, .. } => title.clone(),
            _ => None,
        };
        let date = match &r.values["date"] {
            Value::Date(d) => d.clone(),
            other => panic!("{other:?}"),
        };
        (supplier.is_none(), supplier, Reverse(date), r.id)
    };
    let mut rows = all;
    rows.sort_by_key(key);
    rows.iter().map(|r| r.id).collect()
}

fn several_keys(engine: &dyn Engine) {
    fill(engine);
    let expected = expected(engine);
    assert_eq!(expected.len(), 40);
    for size in [1, 4, 1000] {
        let mut got = Vec::new();
        let mut after = None;
        loop {
            let page = engine
                .list(&ListQuery {
                    after_key: after,
                    ..query(size)
                })
                .unwrap();
            got.extend(page.rows.iter().map(|r| r.id));
            match page.next_key {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        assert_eq!(got, expected, "окно {size}");
    }
    let token = engine.list(&query(5)).unwrap().token;
    for position in [0, 3, 17, 30, 39] {
        let page = engine.seek(&token, position, 5).unwrap();
        let from = usize::try_from(position).unwrap();
        let to = (from + 5).min(expected.len());
        let got: Vec<i64> = page.rows.iter().map(|r| r.id).collect();
        assert_eq!(got, expected[from..to], "позиция {position}");
    }
}

#[test]
fn several_keys_on_sqlite() {
    let dir = std::env::temp_dir().join(format!("ostov-sorting-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = ostov_store::drivers::sqlite::SqliteDriver::open(dir.join("shop.db")).unwrap();
    let config = Config::from_model(&warehouse_model()).unwrap();
    let engine = LocalEngine::install(Box::new(db), config).unwrap();
    several_keys(&engine);
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn several_keys_on_postgres() {
    let url = std::env::var("OSTOV_PG_URL").unwrap();
    let mut db = ostov_store::drivers::postgres::PostgresDriver::connect(&url).unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS ostov_t_sorting CASCADE",
        "CREATE SCHEMA ostov_t_sorting",
        "SET search_path TO ostov_t_sorting",
    ] {
        db.execute(sql, &[]).unwrap();
    }
    let config = Config::from_model(&warehouse_model()).unwrap();
    let engine = LocalEngine::install(Box::new(db), config).unwrap();
    several_keys(&engine);
}
