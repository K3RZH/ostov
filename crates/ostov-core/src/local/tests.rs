//! Только через `Engine` и `Connector` — так, как будет звать интерфейс.

use std::path::PathBuf;
use std::sync::Arc;

use ostov_api::{Connector, Engine, ErrorCode, Fields, ListQuery, ObjectData, PostAction, Value};

use super::LocalConnector;

struct Base {
    dir: PathBuf,
    path: String,
}

impl Base {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ostov-local-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shop.db").to_string_lossy().into_owned();
        Self { dir, path }
    }

    fn create(&self) -> Arc<dyn Engine> {
        LocalConnector.create_local(&self.path, None, None).unwrap()
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn dec(units: i64, scale: u8) -> Value {
    Value::Decimal { units, scale }
}

fn refer(id: i64) -> Value {
    Value::Ref { id, title: None }
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn add(engine: &dyn Engine, object: &str, name: &str) -> i64 {
    let data = ObjectData {
        fields: fields(&[("name", text(name))]),
        ..ObjectData::default()
    };
    engine.save(object, &data, PostAction::Write).unwrap().id
}

/// Строки: (товар, количество в тысячных, цена в копейках).
fn receipt(warehouse: i64, supplier: i64, lines: &[(i64, i64, i64)]) -> ObjectData {
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
    ObjectData {
        fields: fields(&[
            ("date", Value::Date("2026-10-30".into())),
            ("warehouse", refer(warehouse)),
            ("supplier", refer(supplier)),
        ]),
        tables: [("lines".to_owned(), lines)].into(),
        ..ObjectData::default()
    }
}

fn issue(warehouse: i64, goods: i64, qty: i64) -> ObjectData {
    ObjectData {
        fields: fields(&[
            ("date", Value::Date("2026-10-31".into())),
            ("warehouse", refer(warehouse)),
        ]),
        tables: [(
            "lines".to_owned(),
            vec![fields(&[("goods", refer(goods)), ("qty", dec(qty, 3))])],
        )]
        .into(),
        ..ObjectData::default()
    }
}

fn query(object: &str, size: u32) -> ListQuery {
    ListQuery {
        object: object.into(),
        size,
        ..ListQuery::default()
    }
}

#[test]
fn receipt_is_saved_posted_and_listed() {
    let base = Base::new("flow");
    let engine = base.create();
    assert!(engine.hello().unwrap().config_name.contains("Склад"));

    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let supplier = add(&*engine, "suppliers", "Мельница");

    // 2.5 кг по 3.33 = 8.325 → 8.33
    let saved = engine
        .save(
            "receipt",
            &receipt(main, supplier, &[(flour, 2500, 333)]),
            PostAction::Post,
        )
        .unwrap();
    assert_eq!(saved.number, Some(1));

    let doc = engine.get("receipt", saved.id).unwrap();
    assert!(doc.posted);
    assert_eq!(doc.fields["amount"], dec(833, 2));
    assert_eq!(doc.tables["lines"][0]["amount"], dec(833, 2));
    let titled = |id, t: &str| Value::Ref {
        id,
        title: Some(t.to_owned()),
    };
    assert_eq!(doc.fields["supplier"], titled(supplier, "Мельница"));
    assert_eq!(doc.tables["lines"][0]["goods"], titled(flour, "Мука"));

    let page = engine.list(&query("receipt", 10)).unwrap();
    assert_eq!(page.total, Some(1));
    assert_eq!(page.rows[0].values["posted"], Value::Int(1));
    assert_eq!(page.rows[0].values["number"], Value::Int(1));
    assert_eq!(page.rows[0].values["warehouse"], titled(main, "Основной"));

    let err = engine
        .save("issue", &issue(main, flour, 3000), PostAction::Post)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation, "{err}");
    engine
        .save("issue", &issue(main, flour, 1000), PostAction::Post)
        .unwrap();
}

#[test]
fn stale_edit_is_rejected() {
    let base = Base::new("stale");
    let engine = base.create();
    let id = add(&*engine, "goods", "Соль");

    let mut first = engine.get("goods", id).unwrap();
    let second = first.clone();
    first.fields.insert("unit".into(), text("кг"));
    engine.save("goods", &first, PostAction::Write).unwrap();

    let err = engine
        .save("goods", &second, PostAction::Write)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::VersionConflict);
}

#[test]
fn list_pages_and_seek() {
    let base = Base::new("list");
    let engine = base.create();
    for i in 1..=30 {
        add(&*engine, "goods", &format!("товар {i:02}"));
    }
    let q = ListQuery {
        order: Some("name".into()),
        descending: true,
        ..query("goods", 10)
    };
    let first = engine.list(&q).unwrap();
    assert_eq!(first.total, Some(30));
    assert_eq!(first.rows[0].values["name"], text("товар 30"));

    let next = engine
        .list(&ListQuery {
            after_key: first.next_key.clone(),
            ..q.clone()
        })
        .unwrap();
    assert_eq!(next.rows[0].values["name"], text("товар 20"));

    let jump = engine.seek(&first.token, 25, 10).unwrap();
    let names: Vec<_> = jump.rows.iter().map(|r| r.values["name"].clone()).collect();
    assert_eq!(
        names,
        (1..=5)
            .rev()
            .map(|i| text(&format!("товар {i:02}")))
            .collect::<Vec<_>>()
    );
    assert!(jump.next_key.is_none());
}

#[test]
fn deletion_mark_blocks_posting() {
    let base = Base::new("deleted");
    let engine = base.create();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let supplier = add(&*engine, "suppliers", "Мельница");
    let saved = engine
        .save(
            "receipt",
            &receipt(main, supplier, &[(flour, 1000, 100)]),
            PostAction::Write,
        )
        .unwrap();

    let marked = engine
        .mark_deleted("receipt", saved.id, saved.version, true)
        .unwrap();
    let err = engine.post("receipt", saved.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);

    engine
        .mark_deleted("receipt", saved.id, marked.version, false)
        .unwrap();
    assert!(engine.post("receipt", saved.id).is_ok());
    assert!(engine.unpost("receipt", saved.id).is_ok());
}

#[test]
fn opening_and_creating_bases() {
    let base = Base::new("open");
    {
        let engine = base.create();
        add(&*engine, "goods", "Мука");
    }
    let engine = LocalConnector.open_local(&base.path, None).unwrap();
    assert_eq!(engine.list(&query("goods", 10)).unwrap().total, Some(1));

    let err = LocalConnector
        .create_local(&base.path, None, None)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Validation);
    let missing = format!("{}.missing", base.path);
    assert_eq!(
        LocalConnector
            .open_local(&missing, None)
            .err()
            .unwrap()
            .code,
        ErrorCode::NotFound
    );
    let err = LocalConnector
        .open_local(&base.path, Some("secret"))
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Validation);

    let other = base.dir.join("other.db").to_string_lossy().into_owned();
    std::fs::write(&other, b"").unwrap();
    assert_eq!(
        LocalConnector.open_local(&other, None).err().unwrap().code,
        ErrorCode::Incompatible
    );
}

#[test]
fn wrong_requests() {
    let base = Base::new("wrong");
    let engine = base.create();
    assert_eq!(
        engine.get("services", 1).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        engine.get("goods", 404).unwrap_err().code,
        ErrorCode::NotFound
    );

    let data = ObjectData {
        fields: fields(&[("name", text("Мука"))]),
        ..ObjectData::default()
    };
    assert_eq!(
        engine
            .save("goods", &data, PostAction::Post)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    let flag = ObjectData {
        fields: fields(&[("name", Value::Bool(true))]),
        ..ObjectData::default()
    };
    assert_eq!(
        engine
            .save("goods", &flag, PostAction::Write)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        engine.seek("мусор", 0, 10).unwrap_err().code,
        ErrorCode::Validation
    );
    assert_eq!(
        engine.login("a", "b").unwrap_err().code,
        ErrorCode::NotImplemented
    );
}

#[test]
fn demo_base_has_data_to_scroll() {
    let base = Base::new("demo");
    let engine = LocalConnector.create_demo(&base.path, 300).unwrap();

    let page = engine.list(&query("receipt", 50)).unwrap();
    assert_eq!(page.total, Some(300));
    let first = &page.rows[0].values;
    assert!(matches!(
        &first["supplier"],
        Value::Ref { title: Some(_), .. }
    ));
    assert!(matches!(&first["date"], Value::Date(d) if d.starts_with("2026-01")));

    let all: Vec<_> = (0..6)
        .flat_map(|i| engine.seek(&page.token, i * 50, 50).unwrap().rows)
        .collect();
    let posted = all
        .iter()
        .filter(|r| r.values["posted"] == Value::Int(1))
        .count();
    let deleted = all
        .iter()
        .filter(|r| r.values["deleted"] == Value::Int(1))
        .count();
    assert_eq!(all.len(), 300);
    assert_eq!(deleted, 10);
    assert_eq!(posted, 270);
    assert!(engine.list(&query("issue", 10)).unwrap().total.unwrap() > 0);
    assert_eq!(engine.list(&query("goods", 50)).unwrap().total, Some(8));
}

#[test]
fn base_opens_only_once() {
    let base = Base::new("lock");
    let first = base.create();
    let err = LocalConnector.open_local(&base.path, None).err().unwrap();
    assert_eq!(err.code, ErrorCode::AlreadyOpen);

    drop(first);
    let again = LocalConnector.open_local(&base.path, None).unwrap();
    assert!(again.hello().is_ok());
}

fn title(v: &Value) -> Option<String> {
    match v {
        Value::Ref { title, .. } => title.clone(),
        _ => None,
    }
}

#[test]
fn receipts_search_by_supplier_number_and_date() {
    let base = Base::new("search");
    let engine = base.create();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    let farm = add(&*engine, "suppliers", "Ферма");
    for supplier in [mill, farm, mill] {
        engine
            .save(
                "receipt",
                &receipt(main, supplier, &[(flour, 1000, 100)]),
                PostAction::Write,
            )
            .unwrap();
    }
    let found = |s: &str| {
        let q = ListQuery {
            search: Some(s.into()),
            ..query("receipt", 10)
        };
        engine.list(&q).unwrap()
    };
    // Регистр не важен, в том числе для кириллицы.
    assert_eq!(found("мельн").total, Some(2));
    assert_eq!(found("МЕЛЬН").total, Some(2));
    assert_eq!(found("Мельн").total, Some(2));
    assert_eq!(found("основ").total, Some(3));
    assert_eq!(found("Основ").total, Some(3));
    assert_eq!(found("2026-10").total, Some(3));
    // Число ищется точно по номеру и как подстрока в дате: «2» есть и в «2026-10-30».
    let by_number = found("2");
    assert!(
        by_number
            .rows
            .iter()
            .any(|r| r.values["number"] == Value::Int(2))
    );
    assert_eq!(found("ничего такого").total, Some(0));
}

#[test]
fn receipts_sort_by_supplier_title_across_pages() {
    let base = Base::new("sort");
    let engine = LocalConnector.create_demo(&base.path, 400).unwrap();
    for descending in [false, true] {
        let q = ListQuery {
            order: Some("supplier".into()),
            descending,
            ..query("receipt", 37)
        };
        let mut titles = Vec::new();
        let mut page = engine.list(&q).unwrap();
        let token = page.token.clone();
        loop {
            titles.extend(page.rows.iter().map(|r| title(&r.values["supplier"])));
            let Some(key) = page.next_key.clone() else {
                break;
            };
            page = engine
                .list(&ListQuery {
                    after_key: Some(key),
                    ..q.clone()
                })
                .unwrap();
        }
        assert_eq!(titles.len(), 400);
        let mut sorted = titles.clone();
        sorted.sort();
        if descending {
            sorted.reverse();
        }
        assert_eq!(titles, sorted);

        let jump = engine.seek(&token, 150, 37).unwrap();
        let jumped: Vec<_> = jump
            .rows
            .iter()
            .map(|r| title(&r.values["supplier"]))
            .collect();
        assert_eq!(jumped, titles[150..187].to_vec());
    }
}

#[test]
fn panic_in_command_becomes_error_and_engine_keeps_working() {
    let base = Base::new("panic");
    let mut config = super::warehouse().unwrap();
    let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    config.create_tables(&mut db).unwrap();
    if let Some(receipt) = config.documents.get_mut("receipt") {
        receipt.rule = |_| panic!("ошибка в правиле проведения");
    }
    let engine = super::LocalEngine::new(Box::new(db), config);

    let flour = add(&engine, "goods", "Мука");
    let main = add(&engine, "warehouses", "Основной");
    let mill = add(&engine, "suppliers", "Мельница");
    let doc = receipt(main, mill, &[(flour, 1000, 100)]);

    let err = engine.save("receipt", &doc, PostAction::Post).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.details.contains("ошибка в правиле проведения"),
        "{}",
        err.details
    );
    assert_eq!(engine.list(&query("receipt", 10)).unwrap().total, Some(0));

    // Тот же движок дальше работает: документ записывается без проведения.
    engine.save("receipt", &doc, PostAction::Write).unwrap();
    assert_eq!(engine.list(&query("receipt", 10)).unwrap().total, Some(1));
}
