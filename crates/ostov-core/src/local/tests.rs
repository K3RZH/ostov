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
        LocalConnector::default()
            .create_local(&self.path, None, None)
            .unwrap()
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
    let engine = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
    assert_eq!(engine.list(&query("goods", 10)).unwrap().total, Some(1));

    let err = LocalConnector::default()
        .create_local(&base.path, None, None)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Validation);
    let missing = format!("{}.missing", base.path);
    assert_eq!(
        LocalConnector::default()
            .open_local(&missing, None)
            .err()
            .unwrap()
            .code,
        ErrorCode::NotFound
    );
    let err = LocalConnector::default()
        .open_local(&base.path, Some("secret"))
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Validation);

    let other = base.dir.join("other.db").to_string_lossy().into_owned();
    std::fs::write(&other, b"").unwrap();
    assert_eq!(
        LocalConnector::default()
            .open_local(&other, None)
            .err()
            .unwrap()
            .code,
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
    let engine = LocalConnector::default()
        .create_demo(&base.path, 300)
        .unwrap();

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
    let err = LocalConnector::default()
        .open_local(&base.path, None)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::AlreadyOpen);

    drop(first);
    let again = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
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
    let engine = LocalConnector::default()
        .create_demo(&base.path, 400)
        .unwrap();
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

fn dated(mut data: ObjectData, date: &str) -> ObjectData {
    data.fields.insert("date".into(), Value::Date(date.into()));
    data
}

fn report(engine: &dyn Engine, name: &str, params: &[(&str, Value)]) -> Vec<Fields> {
    let q = ostov_api::ReportQuery {
        report: name.into(),
        params: fields(params),
        after_key: None,
        size: 1000,
    };
    engine
        .run_report(&q)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.values)
        .collect()
}

fn date(d: &str) -> Value {
    Value::Date(d.into())
}

#[test]
fn stock_reports_by_date_and_period() {
    let base = Base::new("reports");
    let engine = base.create();
    let flour = add(&*engine, "goods", "Мука");
    let sugar = add(&*engine, "goods", "Сахар");
    let main = add(&*engine, "warehouses", "Основной");
    let kitchen = add(&*engine, "warehouses", "Кухня");
    let mill = add(&*engine, "suppliers", "Мельница");

    let post = |data: ObjectData, object: &str| {
        engine.save(object, &data, PostAction::Post).unwrap();
    };
    post(
        dated(receipt(main, mill, &[(flour, 10_000, 100)]), "2026-01-10"),
        "receipt",
    );
    post(dated(issue(main, flour, 3_000), "2026-01-20"), "issue");
    post(
        dated(receipt(main, mill, &[(flour, 5_000, 100)]), "2026-02-05"),
        "receipt",
    );
    post(
        dated(receipt(kitchen, mill, &[(sugar, 2_000, 100)]), "2026-02-06"),
        "receipt",
    );

    // Обороты за февраль.
    let feb = report(
        &*engine,
        "stock_turnover",
        &[("from", date("2026-02-01")), ("to", date("2026-02-28"))],
    );
    assert_eq!(feb.len(), 2);
    let flour_row = feb
        .iter()
        .find(|r| {
            r["goods"]
                == Value::Ref {
                    id: flour,
                    title: Some("Мука".into()),
                }
        })
        .unwrap();
    assert_eq!(
        flour_row["warehouse"],
        Value::Ref {
            id: main,
            title: Some("Основной".into())
        }
    );
    assert_eq!(flour_row["qty_opening"], dec(7_000, 3));
    assert_eq!(flour_row["qty_receipt"], dec(5_000, 3));
    assert_eq!(flour_row["qty_issue"], dec(0, 3));
    assert_eq!(flour_row["qty_closing"], dec(12_000, 3));

    // Отбор по складу.
    let only_main = report(
        &*engine,
        "stock_turnover",
        &[
            ("from", date("2026-02-01")),
            ("to", date("2026-02-28")),
            ("warehouse", refer(main)),
        ],
    );
    assert_eq!(only_main.len(), 1);

    // Остатки на 15 января и текущие.
    let jan15 = report(&*engine, "stock_balance", &[("date", date("2026-01-15"))]);
    assert_eq!(jan15.len(), 1);
    assert_eq!(jan15[0]["qty"], dec(10_000, 3));
    let now = report(&*engine, "stock_balance", &[]);
    assert_eq!(now.len(), 2);

    // Ошибки.
    let bad = |params: &[(&str, Value)], name: &str| {
        let q = ostov_api::ReportQuery {
            report: name.into(),
            params: fields(params),
            after_key: None,
            size: 100,
        };
        engine.run_report(&q).unwrap_err().code
    };
    assert_eq!(
        bad(&[("from", date("2026-13-01"))], "stock_turnover"),
        ErrorCode::Validation
    );
    assert_eq!(
        bad(
            &[("from", date("2026-03-01")), ("to", date("2026-02-01"))],
            "stock_turnover"
        ),
        ErrorCode::Validation
    );
    assert_eq!(
        bad(&[("color", text("red"))], "stock_balance"),
        ErrorCode::Validation
    );
    assert_eq!(bad(&[], "sales"), ErrorCode::NotFound);
}

#[test]
fn report_pages() {
    let base = Base::new("report-pages");
    let engine = LocalConnector::default()
        .create_demo(&base.path, 200)
        .unwrap();
    let all = report(&*engine, "stock_balance", &[]);
    assert!(all.len() > 5, "{}", all.len());

    let mut q = ostov_api::ReportQuery {
        report: "stock_balance".into(),
        params: Fields::new(),
        after_key: None,
        size: 4,
    };
    let mut paged = Vec::new();
    loop {
        let page = engine.run_report(&q).unwrap();
        assert_eq!(page.total, Some(all.len() as u64));
        paged.extend(page.rows.into_iter().map(|r| r.values));
        match page.next_key {
            Some(key) => q.after_key = Some(key),
            None => break,
        }
    }
    assert_eq!(paged, all);
}

#[test]
fn old_base_gets_dates_in_movements() {
    use ostov_store::Driver;

    let base = Base::new("upgrade");
    let before = {
        let engine = LocalConnector::default()
            .create_demo(&base.path, 120)
            .unwrap();
        report(&*engine, "stock_balance", &[("date", date("2026-03-31"))])
    };

    // База «до отчётов»: в движениях нет колонки period.
    {
        let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
        db.execute("DROP INDEX stock__period", &[]).unwrap();
        db.execute("ALTER TABLE stock DROP COLUMN period", &[])
            .unwrap();
    }

    let engine = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
    let after = report(&*engine, "stock_balance", &[("date", date("2026-03-31"))]);
    assert_eq!(after, before);
    drop(engine);

    let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    let empty = db
        .query("SELECT COUNT(*) FROM stock WHERE period IS NULL", &[])
        .unwrap();
    assert_eq!(empty, vec![vec![ostov_store::Value::Int(0)]]);
}

/// Склад с мукой: приход 10 кг 1 февраля.
fn stocked(base: &Base) -> (Arc<dyn Engine>, i64, i64, i64) {
    let engine = base.create();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    let r = engine
        .save(
            "receipt",
            &dated(receipt(main, mill, &[(flour, 10_000, 100)]), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();
    (engine, flour, main, r.id)
}

#[test]
fn issue_before_receipt_is_rejected() {
    let base = Base::new("backdated");
    let (engine, flour, main, _) = stocked(&base);

    // Сейчас муки 10 кг, но 15 января её ещё не было.
    let err = engine
        .save(
            "issue",
            &dated(issue(main, flour, 3_000), "2026-01-15"),
            PostAction::Post,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.contains("2026-01-15"), "{}", err.message);
    assert_eq!(engine.list(&query("issue", 10)).unwrap().total, Some(0));

    // В тот же день, что и приход, — можно: проверяется итог дня.
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 3_000), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();
}

#[test]
fn shrinking_old_receipt_below_later_issues_is_rejected() {
    let base = Base::new("shrink");
    let (engine, flour, main, receipt_id) = stocked(&base);
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 6_000), "2026-02-10"),
            PostAction::Post,
        )
        .unwrap();

    let mut doc = engine.get("receipt", receipt_id).unwrap();
    if let Some(line) = doc.tables.get_mut("lines").and_then(|l| l.first_mut()) {
        line.insert("qty".into(), dec(4_000, 3));
    }
    let err = engine.save("receipt", &doc, PostAction::Post).unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.contains("2026-02-10"), "{}", err.message);

    // Перенос прихода на день позже расхода тоже ломает историю.
    let mut later = engine.get("receipt", receipt_id).unwrap();
    later.fields.insert("date".into(), date("2026-02-20"));
    assert_eq!(
        engine
            .save("receipt", &later, PostAction::Post)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );

    // Ничего не изменилось.
    let balance = report(&*engine, "stock_balance", &[("date", date("2026-02-10"))]);
    assert_eq!(balance[0]["qty"], dec(4_000, 3));
}

#[test]
fn unposting_receipt_with_later_issue_is_rejected() {
    let base = Base::new("unpost-history");
    let (engine, flour, main, receipt_id) = stocked(&base);
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 1_000), "2026-03-01"),
            PostAction::Post,
        )
        .unwrap();
    let err = engine.unpost("receipt", receipt_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
}

#[test]
fn connector_takes_model_from_outside() {
    let base = Base::new("model");
    let mut model = super::warehouse_model();
    model.catalogs[0]
        .fields
        .push(crate::FieldDef::new("barcode", crate::FieldKind::Text));
    let engine = LocalConnector::new(model.clone())
        .create_local(&base.path, None, None)
        .unwrap();
    let id = engine
        .save(
            "goods",
            &ObjectData {
                fields: fields(&[("name", text("Мука")), ("barcode", text("460"))]),
                ..ObjectData::default()
            },
            PostAction::Write,
        )
        .unwrap()
        .id;
    assert_eq!(
        engine.get("goods", id).unwrap().fields["barcode"],
        text("460")
    );
    drop(engine);

    // Встроенный «Склад» про штрихкод не знает, переданная модель — знает.
    let engine = LocalConnector::new(model)
        .open_local(&base.path, None)
        .unwrap();
    assert_eq!(
        engine.get("goods", id).unwrap().fields["barcode"],
        text("460")
    );
}

#[test]
fn model_without_rules_is_rejected() {
    let config = |change: fn(&mut ostov_model::Config)| {
        let mut model = super::warehouse_model();
        change(&mut model);
        super::Config::from_model(&model)
            .err()
            .map(|e| e.to_string())
    };
    assert_eq!(config(|_| ()), None);

    let unknown_doc = config(|m| m.documents[0].name = "transfer".into()).unwrap();
    assert!(unknown_doc.contains("transfer"), "{unknown_doc}");

    let other_register = config(|m| m.documents[0].registers = vec!["sales".into()]).unwrap();
    assert!(other_register.contains("sales"), "{other_register}");

    assert!(config(|m| m.registers.clear()).is_some());

    // Документ без движений правила не требует.
    assert_eq!(
        config(|m| {
            m.documents[0].name = "order".into();
            m.documents[0].registers.clear();
        }),
        None
    );
}

#[test]
fn closed_period_freezes_documents() {
    let base = Base::new("closing");
    let (engine, flour, main, receipt_id) = stocked(&base);
    let issue_id = engine
        .save(
            "issue",
            &dated(issue(main, flour, 1_000), "2026-03-01"),
            PostAction::Post,
        )
        .unwrap()
        .id;
    engine.set_closing_date(Some("2026-02-28")).unwrap();
    assert_eq!(
        engine.closing_date().unwrap().as_deref(),
        Some("2026-02-28")
    );

    let closed = |r: ostov_api::EngineResult<ostov_api::Saved>| {
        assert_eq!(r.unwrap_err().code, ErrorCode::PeriodClosed);
    };
    // Февральское поступление: ни записать, ни провести, ни отменить, ни пометить.
    let receipt = engine.get("receipt", receipt_id).unwrap();
    closed(engine.save("receipt", &receipt, PostAction::Post));
    closed(engine.unpost("receipt", receipt_id));
    closed(engine.post("receipt", receipt_id));
    closed(engine.mark_deleted("receipt", receipt_id, receipt.version, true));
    // Новый документ задним числом и перенос мартовского в февраль.
    closed(engine.save(
        "issue",
        &dated(issue(main, flour, 1_000), "2026-02-15"),
        PostAction::Write,
    ));
    let mut moved = engine.get("issue", issue_id).unwrap();
    moved.fields.insert("date".into(), date("2026-02-20"));
    closed(engine.save("issue", &moved, PostAction::Post));

    // Март открыт.
    engine.unpost("issue", issue_id).unwrap();
    engine.post("issue", issue_id).unwrap();

    // Закрытие хранится в базе.
    drop(engine);
    let engine = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
    assert_eq!(
        engine.closing_date().unwrap().as_deref(),
        Some("2026-02-28")
    );
    closed(engine.unpost("receipt", receipt_id));

    engine.set_closing_date(None).unwrap();
    engine.unpost("issue", issue_id).unwrap();
    engine.unpost("receipt", receipt_id).unwrap();
    assert_eq!(
        engine
            .set_closing_date(Some("28.02.2026"))
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
}

#[test]
fn old_base_gets_closing_table() {
    let base = Base::new("closing-upgrade");
    drop(base.create());
    {
        let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
        ostov_store::Driver::execute(&mut db, "DROP TABLE ostov_closing", &[]).unwrap();
    }
    let engine = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
    assert_eq!(engine.closing_date().unwrap(), None);
    engine.set_closing_date(Some("2026-01-31")).unwrap();
}

fn changed_model(change: impl FnOnce(&mut ostov_model::Config)) -> LocalConnector {
    let mut model = super::warehouse_model();
    change(&mut model);
    LocalConnector::new(model)
}

#[test]
fn base_follows_config_changes() {
    let base = Base::new("restructure");
    let (engine, flour, main, _) = stocked(&base);
    drop(engine);

    let connector = changed_model(|m| {
        use crate::{FieldDef, FieldKind};
        m.catalogs[0]
            .fields
            .push(FieldDef::new("barcode", FieldKind::Text));
        m.catalogs.push(ostov_model::CatalogDef {
            name: "units".into(),
            title: "Единицы".into(),
            fields: vec![FieldDef::new("name", FieldKind::Text)],
        });
        let receipt = &mut m.documents[1];
        receipt
            .fields
            .push(FieldDef::new("comment", FieldKind::Text));
        receipt.table_parts.push(ostov_model::TablePartDef {
            name: "costs".into(),
            title: "Расходы".into(),
            fields: vec![FieldDef::new("amount", FieldKind::Decimal { scale: 2 })],
        });
        m.registers[0]
            .resources
            .push(FieldDef::new("weight", FieldKind::Decimal { scale: 3 }));
    });
    let engine = connector.open_local(&base.path, None).unwrap();

    // Старые данные на месте, новые поля пустые и записываются.
    let mut goods = engine.get("goods", flour).unwrap();
    assert_eq!(goods.fields["barcode"], Value::Null);
    goods.fields.insert("barcode".into(), text("460"));
    engine.save("goods", &goods, PostAction::Write).unwrap();
    add(&*engine, "units", "кг");
    let balance = report(&*engine, "stock_balance", &[("warehouse", refer(main))]);
    assert_eq!(balance[0]["qty"], dec(10_000, 3));
    assert_eq!(balance[0]["weight"], dec(0, 3));
    drop(engine);

    // Повторное открытие ничего не меняет.
    let engine = connector.open_local(&base.path, None).unwrap();
    assert_eq!(
        engine.get("goods", flour).unwrap().fields["barcode"],
        text("460")
    );
    drop(engine);

    // Старая конфигурация убрала бы ресурс weight с данными — её база не примет.
    let err = LocalConnector::default()
        .open_local(&base.path, None)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Incompatible);
}

#[test]
fn config_that_loses_data_is_refused() {
    let base = Base::new("restructure-refused");
    drop(stocked(&base).0);

    let refused = |connector: LocalConnector| {
        let err = connector.open_local(&base.path, None).err().unwrap();
        assert_eq!(err.code, ErrorCode::Incompatible, "{}", err.message);
        err.message
    };
    let unit = refused(changed_model(|m| {
        m.catalogs[0].fields[1].kind = crate::FieldKind::Int;
    }));
    assert!(unit.contains("unit"), "{unit}");
    refused(changed_model(|m| {
        m.documents[1].table_parts[0].fields[1].kind = crate::FieldKind::Decimal { scale: 2 };
    }));
    // В регистре есть движения: новое измерение не добавить.
    refused(changed_model(|m| {
        m.registers[0]
            .dimensions
            .push(crate::FieldDef::new("batch", crate::FieldKind::Text));
    }));

    // После отказа база цела и открывается как раньше.
    let engine = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
    assert_eq!(report(&*engine, "stock_balance", &[]).len(), 1);
}

#[test]
fn empty_register_is_rebuilt() {
    let base = Base::new("restructure-register");
    drop(base.create());
    let engine = changed_model(|m| {
        m.registers[0]
            .dimensions
            .push(crate::FieldDef::new("batch", crate::FieldKind::Text));
    })
    .open_local(&base.path, None)
    .unwrap();
    assert!(report(&*engine, "stock_balance", &[]).is_empty());
}

#[test]
fn base_before_restructuring_is_adopted() {
    let base = Base::new("restructure-old");
    drop(base.create());
    {
        let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
        use ostov_store::Driver;
        db.execute("DROP TABLE ostov_schema", &[]).unwrap();
        db.execute("DELETE FROM ostov_base WHERE key = 'schema'", &[])
            .unwrap();
    }
    let engine = changed_model(|m| {
        m.catalogs[0]
            .fields
            .push(crate::FieldDef::new("barcode", crate::FieldKind::Text));
    })
    .open_local(&base.path, None)
    .unwrap();
    add(&*engine, "goods", "Соль");
}
