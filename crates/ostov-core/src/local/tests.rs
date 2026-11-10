//! Только через `Engine` и `Connector` — так, как будет звать интерфейс.

use std::path::PathBuf;
use std::sync::Arc;

use ostov_api::{
    Connector, Engine, ErrorCode, Fields, ListQuery, ObjectData, PostAction, Row, Value,
};

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
    assert_eq!(page.rows[0].values["posted"], Value::Bool(true));
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
        ErrorCode::BadCredentials
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
        .filter(|r| r.values["posted"] == Value::Bool(true))
        .count();
    let deleted = all
        .iter()
        .filter(|r| r.values["deleted"] == Value::Bool(true))
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
    ostov_sec::create_tables(&mut db).unwrap();
    if let Some(receipt) = config.documents.get_mut("receipt") {
        struct Broken;
        impl crate::PostingRule for Broken {
            fn movements(&self, _: &crate::Document) -> Result<crate::Moves, crate::CoreError> {
                panic!("ошибка в правиле проведения")
            }
        }
        receipt.rule = Box::new(Broken);
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
        db.execute("DROP INDEX stock__key", &[]).unwrap();
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
    let index = db
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'stock__key'",
            &[],
        )
        .unwrap();
    assert_eq!(index, vec![vec![ostov_store::Value::Int(1)]]);
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
    assert_eq!(
        err.message,
        "Не хватает остатка на 15.01.2026: Склад «Основной», Товар «Мука» — Количество стало бы −3,000. Списывают в этот день: этот документ."
    );
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
    assert_eq!(
        err.message,
        "Не хватает остатка на 10.02.2026: Склад «Основной», Товар «Мука» — Количество стало бы −2,000. Списывают в этот день: Расход №1."
    );

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
            icon: None,
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

#[test]
fn meta_describes_objects_and_menu() {
    let base = Base::new("meta");
    let meta = base.create().get_meta().unwrap();
    let receipt = meta.objects.iter().find(|o| o.name == "receipt").unwrap();
    assert_eq!(receipt.kind, ostov_api::ObjectKind::Document);
    assert_eq!(receipt.icon.as_deref(), Some("stock-in"));
    assert!(
        receipt
            .fields
            .iter()
            .any(|f| f.name == "amount" && f.read_only)
    );
    assert_eq!(receipt.table_parts[0].name, "lines");
    let supplier = receipt
        .fields
        .iter()
        .find(|f| f.name == "supplier")
        .unwrap();
    assert_eq!(supplier.target.as_deref(), Some("suppliers"));
    assert!(receipt.rights.post);

    let [section] = meta.menu.as_slice() else {
        panic!("{:?}", meta.menu);
    };
    assert_eq!(section.object, None);
    assert_eq!(section.children.len(), 5);
}

#[test]
fn required_fields_must_be_filled() {
    let base = Base::new("required");
    let engine = changed_model(|m| {
        m.catalogs[0].fields[0].required = true;
        m.catalogs[0].fields[0].title = "Наименование".into();
        m.documents[1].fields[0].required = true;
    })
    .create_local(&base.path, None, None)
    .unwrap();

    for name in [Value::Null, text("  ")] {
        let data = ObjectData {
            fields: fields(&[("name", name), ("unit", text("кг"))]),
            ..ObjectData::default()
        };
        let err = engine.save("goods", &data, PostAction::Write).unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert!(err.message.contains("Наименование"), "{}", err.message);
    }
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");

    // Поступление без даты не записать; с датой — можно.
    let mut no_date = receipt(main, mill, &[(flour, 1_000, 100)]);
    no_date.fields.remove("date");
    assert_eq!(
        engine
            .save("receipt", &no_date, PostAction::Write)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    engine
        .save("receipt", &dated(no_date, "2026-02-01"), PostAction::Post)
        .unwrap();
}

#[test]
fn list_rows_carry_version_and_filter_by_status() {
    let base = Base::new("status");
    let engine = LocalConnector::default()
        .create_demo(&base.path, 60)
        .unwrap();
    let all = |q: ListQuery| -> Vec<Row> {
        let mut rows = Vec::new();
        let mut q = ListQuery { size: 1000, ..q };
        loop {
            let page = engine.list(&q).unwrap();
            rows.extend(page.rows);
            match page.next_key {
                Some(key) => q.after_key = Some(key),
                None => return rows,
            }
        }
    };
    let flag = |r: &Row, name: &str| r.values[name] == Value::Bool(true);

    // Пометка на удаление по версии прямо из строки списка, без лишнего get.
    let first = all(ListQuery {
        posted: Some(false),
        deleted: Some(false),
        ..query("receipt", 1)
    })
    .remove(0);
    let Value::Int(version) = first.values["version"] else {
        panic!("{:?}", first.values);
    };
    engine
        .mark_deleted("receipt", first.id, version, true)
        .unwrap();

    let receipts = all(query("receipt", 1));
    let posted = all(ListQuery {
        posted: Some(true),
        ..query("receipt", 1)
    });
    let drafts = all(ListQuery {
        posted: Some(false),
        ..query("receipt", 1)
    });
    let deleted = all(ListQuery {
        deleted: Some(true),
        ..query("receipt", 1)
    });
    assert!(!posted.is_empty() && !drafts.is_empty() && !deleted.is_empty());
    assert!(posted.iter().all(|r| flag(r, "posted")));
    assert!(drafts.iter().all(|r| !flag(r, "posted")));
    assert!(deleted.iter().all(|r| flag(r, "deleted")));
    assert!(deleted.iter().any(|r| r.id == first.id));
    assert_eq!(posted.len() + drafts.len(), receipts.len());

    // Отбор вместе с поиском и через вехи.
    let page = engine
        .list(&ListQuery {
            posted: Some(false),
            ..query("receipt", 5)
        })
        .unwrap();
    assert_eq!(page.total, Some(drafts.len() as u64));
    let seek = engine.seek(&page.token, 0, 5).unwrap();
    assert!(seek.rows.iter().all(|r| !flag(r, "posted")));

    // У справочника нет проведения; число в поиске не ловит служебные флаги.
    let err = engine
        .list(&ListQuery {
            posted: Some(true),
            ..query("goods", 10)
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    let found = engine
        .list(&ListQuery {
            search: Some("1".into()),
            ..query("goods", 100)
        })
        .unwrap();
    assert!(found.rows.len() < all(query("goods", 1)).len());
}

#[test]
fn settings_are_kept_in_base() {
    let base = Base::new("settings");
    let engine = base.create();
    assert_eq!(engine.get_settings("receipt.list").unwrap(), None);

    let settings = fields(&[
        ("widths", text("120,80,200")),
        ("dense", Value::Bool(true)),
        ("rows", Value::Int(50)),
        ("zoom", dec(125, 2)),
        ("from", Value::Date("2026-01-01".into())),
        (
            "warehouse",
            Value::Ref {
                id: 3,
                title: Some("Основной".into()),
            },
        ),
        ("note", text("a:b:c")),
        ("empty", Value::Null),
    ]);
    engine.save_settings("receipt.list", &settings).unwrap();
    drop(engine);

    let engine = LocalConnector::default()
        .open_local(&base.path, None)
        .unwrap();
    let mut expected = settings.clone();
    expected.insert("warehouse".into(), Value::Ref { id: 3, title: None });
    assert_eq!(engine.get_settings("receipt.list").unwrap(), Some(expected));

    // Сохранение заменяет набор целиком.
    engine
        .save_settings("receipt.list", &fields(&[("rows", Value::Int(20))]))
        .unwrap();
    assert_eq!(
        engine.get_settings("receipt.list").unwrap(),
        Some(fields(&[("rows", Value::Int(20))]))
    );
    assert_eq!(engine.get_settings("goods.list").unwrap(), None);
}

fn roles_model() -> ostov_model::Config {
    let mut model = super::warehouse_model();
    let read = ostov_model::Rights {
        read: true,
        ..ostov_model::Rights::default()
    };
    let work = ostov_model::Rights {
        read: true,
        create: true,
        update: true,
        post: true,
        unpost: true,
        delete: false,
    };
    model.roles.push(ostov_model::RoleDef {
        name: "storekeeper".into(),
        title: "Кладовщик".into(),
        objects: vec![
            ("goods".into(), read),
            ("warehouses".into(), read),
            ("issue".into(), work),
        ],
        system: ostov_model::SystemRights::default(),
    });
    model
}

const ADMIN_PASSWORD: &str = "admin-password-1";
const KEEPER_PASSWORD: &str = "keeper-password-1";

#[test]
fn base_without_users_needs_no_login() {
    let base = Base::new("no-users");
    let engine = base.create();
    assert!(!engine.hello().unwrap().login_required);
    add(&*engine, "goods", "Мука");
    assert!(
        engine
            .get_meta()
            .unwrap()
            .objects
            .iter()
            .all(|o| o.rights.delete)
    );
}

#[test]
fn first_admin_turns_login_on() {
    let base = Base::new("first-admin");
    let engine = LocalConnector::new(roles_model())
        .create_local(&base.path, None, None)
        .unwrap();
    let session = engine.create_first_admin("Boss", ADMIN_PASSWORD).unwrap();
    assert_eq!(session.login, "boss");
    assert_eq!(session.roles, vec!["admin".to_owned()]);
    assert!(session.system.admin);
    assert_eq!(
        engine
            .create_first_admin("boss2", ADMIN_PASSWORD)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    engine
        .admin_add_user(&ostov_api::NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик Иван".into(),
            role: "storekeeper".into(),
            password: KEEPER_PASSWORD.into(),
        })
        .unwrap();
    let users = engine.admin_users().unwrap();
    assert_eq!(users.len(), 2);
    drop(engine);

    // Переоткрыли: без входа ничего нельзя.
    let engine = LocalConnector::new(roles_model())
        .open_local(&base.path, None)
        .unwrap();
    assert!(engine.hello().unwrap().login_required);
    assert_eq!(
        engine.list(&query("goods", 10)).unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        engine.login("boss", "wrong-password-1").unwrap_err().code,
        ErrorCode::BadCredentials
    );
    engine.login("BOSS", ADMIN_PASSWORD).unwrap();
    assert!(!engine.hello().unwrap().login_required);
    engine.list(&query("goods", 10)).unwrap();
    engine.logout().unwrap();
    assert!(engine.hello().unwrap().login_required);
}

#[test]
fn rights_are_checked_in_core() {
    let base = Base::new("rights");
    let engine = LocalConnector::new(roles_model())
        .create_local(
            &base.path,
            None,
            Some(&ostov_api::NewAdmin {
                login: "boss".into(),
                password: ADMIN_PASSWORD.into(),
            }),
        )
        .unwrap();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &dated(receipt(main, mill, &[(flour, 10_000, 100)]), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();
    engine
        .admin_add_user(&ostov_api::NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик".into(),
            role: "storekeeper".into(),
            password: KEEPER_PASSWORD.into(),
        })
        .unwrap();
    engine.set_closing_date(Some("2026-02-28")).unwrap();
    engine.logout().unwrap();

    engine.login("keeper", KEEPER_PASSWORD).unwrap();
    let forbidden = |r: ostov_api::EngineResult<ostov_api::Saved>| {
        let err = r.unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.message);
        err.message
    };
    // Читать товары можно, менять — нет; поступления не видно вовсе.
    engine.get("goods", flour).unwrap();
    let msg = forbidden(engine.save(
        "goods",
        &engine.get("goods", flour).unwrap(),
        PostAction::Write,
    ));
    assert_eq!(msg, "Нет права на изменение «Товары».");
    assert_eq!(
        engine.list(&query("receipt", 10)).unwrap_err().code,
        ErrorCode::Forbidden
    );
    // Расход проводить можно, помечать на удаление — нет.
    let issue_id = engine
        .save(
            "issue",
            &dated(issue(main, flour, 1_000), "2026-03-01"),
            PostAction::Post,
        )
        .unwrap()
        .id;
    let version = engine.get("issue", issue_id).unwrap().version;
    forbidden(engine.mark_deleted("issue", issue_id, version, true));
    // Закрытый период кладовщику закрыт, а дату закрытия он менять не может.
    assert_eq!(
        engine
            .save(
                "issue",
                &dated(issue(main, flour, 1_000), "2026-02-15"),
                PostAction::Post,
            )
            .unwrap_err()
            .code,
        ErrorCode::PeriodClosed
    );
    assert_eq!(
        engine.set_closing_date(None).unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(engine.admin_users().unwrap_err().code, ErrorCode::Forbidden);

    // Метаданные — только то, что можно смотреть, и меню тоже.
    let meta = engine.get_meta().unwrap();
    let names: Vec<&str> = meta.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["goods", "warehouses", "issue"]);
    let goods = &meta.objects[0];
    assert!(goods.rights.read && !goods.rights.update);
    let menu: Vec<_> = meta.menu[0]
        .children
        .iter()
        .filter_map(|i| i.object.clone())
        .collect();
    assert_eq!(menu, ["goods", "warehouses", "issue"]);

    // Администратор правит закрытый период: у него право closed_period.
    engine.logout().unwrap();
    engine.login("boss", ADMIN_PASSWORD).unwrap();
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 1_000), "2026-02-15"),
            PostAction::Post,
        )
        .unwrap();

    // Последнего администратора не отключить; кладовщика — можно, и он больше не войдёт.
    assert_eq!(
        engine
            .admin_set_user_active("boss", false)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    engine.admin_set_user_active("keeper", false).unwrap();
    engine.logout().unwrap();
    assert_eq!(
        engine.login("keeper", KEEPER_PASSWORD).unwrap_err().code,
        ErrorCode::Forbidden
    );
}

/// Склад плюс регистр продаж: расход списывает со склада и пишет продажу.
fn sales_model() -> ostov_model::Config {
    use crate::{FieldDef, FieldKind};
    let mut model = super::warehouse_model();
    model.registers.push(ostov_model::RegisterDef {
        name: "sales".into(),
        title: "Продажи".into(),
        kind: ostov_model::RegisterKind::Accumulation,
        dimensions: vec![FieldDef::reference("goods", "goods")],
        resources: vec![FieldDef::new("qty", FieldKind::Decimal { scale: 3 })],
    });
    let issue = model
        .documents
        .iter_mut()
        .find(|d| d.name == "issue")
        .unwrap();
    issue.registers.push("sales".into());
    model
}

/// Правила так, как их подключит Python: свои для расхода, встроенные для остального.
struct SalesRules;

struct IssueWithSales;

impl crate::PostingRule for IssueWithSales {
    fn movements(&self, doc: &crate::Document) -> Result<crate::Moves, crate::CoreError> {
        let mut stock = Vec::new();
        let mut sales = Vec::new();
        for line in &doc.parts["lines"] {
            let crate::FieldValue::Decimal(qty) = line["qty"] else {
                continue;
            };
            let goods = line["goods"].clone();
            stock.push(crate::Movement {
                kind: crate::MovementKind::Issue,
                dims: [
                    ("warehouse".to_owned(), doc.header["warehouse"].clone()),
                    ("goods".to_owned(), goods.clone()),
                ]
                .into(),
                resources: [("qty".to_owned(), qty)].into(),
            });
            sales.push(crate::Movement {
                kind: crate::MovementKind::Receipt,
                dims: [("goods".to_owned(), goods)].into(),
                resources: [("qty".to_owned(), qty)].into(),
            });
        }
        Ok([("stock".to_owned(), stock), ("sales".to_owned(), sales)].into())
    }
}

impl super::Rules for SalesRules {
    fn rule(&self, document: &ostov_model::DocumentDef) -> Option<Box<dyn crate::PostingRule>> {
        match document.name.as_str() {
            "issue" => Some(Box::new(IssueWithSales)),
            _ => super::BuiltinRules.rule(document),
        }
    }
}

fn sales_engine(base: &Base, model: &ostov_model::Config) -> super::LocalEngine {
    let db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    let config = super::Config::from_model_with(model, &SalesRules).unwrap();
    super::LocalEngine::install(Box::new(db), config).unwrap()
}

#[test]
fn rules_are_pluggable_and_move_several_registers() {
    let base = Base::new("sales");
    let engine = sales_engine(&base, &sales_model());
    let flour = add(&engine, "goods", "Мука");
    let main = add(&engine, "warehouses", "Основной");
    let mill = add(&engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &dated(receipt(main, mill, &[(flour, 10_000, 100)]), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();
    let sold = engine
        .save(
            "issue",
            &dated(issue(main, flour, 3_000), "2026-02-10"),
            PostAction::Post,
        )
        .unwrap();

    assert_eq!(
        report(&engine, "stock_balance", &[])[0]["qty"],
        dec(7_000, 3)
    );
    let sales = report(&engine, "sales_balance", &[]);
    assert_eq!(sales.len(), 1);
    assert_eq!(
        sales[0]["goods"],
        Value::Ref {
            id: flour,
            title: Some("Мука".into())
        }
    );
    assert_eq!(sales[0]["qty"], dec(3_000, 3));
    let feb = report(
        &engine,
        "sales_turnover",
        &[("from", date("2026-02-01")), ("to", date("2026-02-28"))],
    );
    assert_eq!(feb[0]["qty_receipt"], dec(3_000, 3));

    // Нехватка на складе отменяет и продажу: всё в одной транзакции.
    let err = engine
        .save(
            "issue",
            &dated(issue(main, flour, 50_000), "2026-02-11"),
            PostAction::Post,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert_eq!(
        report(&engine, "sales_balance", &[])[0]["qty"],
        dec(3_000, 3)
    );

    // Отмена проведения убирает движения из обоих регистров.
    engine.unpost("issue", sold.id).unwrap();
    assert!(report(&engine, "sales_balance", &[]).is_empty());
    assert_eq!(
        report(&engine, "stock_balance", &[])[0]["qty"],
        dec(10_000, 3)
    );
    assert_eq!(
        engine
            .run_report(&ostov_api::ReportQuery {
                report: "orders_balance".into(),
                params: Fields::new(),
                after_key: None,
                size: 10,
            })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn rule_cannot_move_foreign_register() {
    let base = Base::new("foreign-register");
    // У расхода в модели только stock, а правило пишет ещё и в sales.
    let mut model = sales_model();
    model
        .documents
        .iter_mut()
        .find(|d| d.name == "issue")
        .unwrap()
        .registers
        .retain(|r| r == "stock");
    let engine = sales_engine(&base, &model);
    let flour = add(&engine, "goods", "Мука");
    let main = add(&engine, "warehouses", "Основной");
    let mill = add(&engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &dated(receipt(main, mill, &[(flour, 10_000, 100)]), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();
    let err = engine
        .save(
            "issue",
            &dated(issue(main, flour, 1_000), "2026-02-10"),
            PostAction::Post,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.contains("sales"), "{}", err.message);
    assert_eq!(
        report(&engine, "stock_balance", &[])[0]["qty"],
        dec(10_000, 3)
    );
}

#[test]
fn config_errors_about_registers_and_rules() {
    let config = |change: fn(&mut ostov_model::Config)| {
        let mut model = sales_model();
        change(&mut model);
        super::Config::from_model_with(&model, &SalesRules)
            .err()
            .map(|e| e.to_string())
    };
    assert_eq!(config(|_| ()), None);
    let unknown = config(|m| m.documents[0].registers.push("orders".into())).unwrap();
    assert!(unknown.contains("orders"), "{unknown}");
    // Документ с регистрами без правила — ошибка; без регистров — можно.
    let no_rule = config(|m| m.documents[1].name = "transfer".into()).unwrap();
    assert!(no_rule.contains("transfer"), "{no_rule}");
    assert_eq!(
        config(|m| {
            m.documents[1].name = "order".into();
            m.documents[1].registers.clear();
        }),
        None
    );
}

#[test]
fn new_register_appears_in_existing_base() {
    let base = Base::new("sales-later");
    let (engine, flour, main, _) = stocked(&base);
    drop(engine);
    let db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    let config = super::Config::from_model_with(&sales_model(), &SalesRules).unwrap();
    let engine = super::LocalEngine::open(Box::new(db), config).unwrap();
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 2_000), "2026-02-10"),
            PostAction::Post,
        )
        .unwrap();
    assert_eq!(
        report(&engine, "sales_balance", &[])[0]["qty"],
        dec(2_000, 3)
    );
}

#[test]
fn connector_opens_file_with_own_rules() {
    let base = Base::new("connector-rules");
    let connector = LocalConnector::new(sales_model()).with_rules(std::sync::Arc::new(SalesRules));
    let engine = connector.create_local(&base.path, None, None).unwrap();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &dated(receipt(main, mill, &[(flour, 5_000, 100)]), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 1_000), "2026-02-02"),
            PostAction::Post,
        )
        .unwrap();
    drop(engine);
    let engine = connector.open_local(&base.path, None).unwrap();
    assert_eq!(
        report(&*engine, "sales_balance", &[])[0]["qty"],
        dec(1_000, 3)
    );
}

fn with_admin(base: &Base) -> Arc<dyn Engine> {
    LocalConnector::new(roles_model())
        .create_local(
            &base.path,
            None,
            Some(&ostov_api::NewAdmin {
                login: "boss".into(),
                password: ADMIN_PASSWORD.into(),
            }),
        )
        .unwrap()
}

fn journal(engine: &dyn Engine) -> Vec<ostov_api::JournalEntry> {
    let mut all = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = engine.admin_journal(after.as_deref(), 3).unwrap();
        let Some(last) = page.last() else {
            return all;
        };
        after = Some(last.id.to_string());
        all.extend(page);
    }
}

#[test]
fn failed_logins_are_counted_and_slowed_down() {
    let base = Base::new("login-delay");
    let engine = with_admin(&base);
    engine.logout().unwrap();
    for _ in 0..4 {
        engine.login("boss", "wrong-password-1").unwrap_err();
    }
    // Пятая ошибка подряд — ответ с задержкой. Сверху время не ограничиваем: хэш в debug под
    // нагрузкой сам идёт больше секунды. Что первые четыре без задержки, проверяет
    // `ostov_sec::delay_secs`, здесь — что счётчик пережил откаты.
    let started = std::time::Instant::now();
    engine.login("boss", "wrong-password-1").unwrap_err();
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    let failed = ostov_store::Driver::query(
        &mut db,
        "SELECT failed FROM ostov_users WHERE login = 'boss'",
        &[],
    )
    .unwrap();
    assert_eq!(failed, vec![vec![ostov_store::Value::Int(5)]]);

    engine.login("boss", ADMIN_PASSWORD).unwrap();
    let failed = journal(&*engine)
        .iter()
        .filter(|e| e.action == "Неудачный вход")
        .count();
    assert_eq!(failed, 5);
    let all = format!("{:?}", journal(&*engine));
    assert!(!all.contains("wrong-password-1") && !all.contains(ADMIN_PASSWORD));
}

#[test]
fn journal_keeps_who_did_what() {
    let base = Base::new("journal");
    let engine = with_admin(&base);
    let flour = add(&*engine, "goods", "Мука");
    let mut goods = engine.get("goods", flour).unwrap();
    goods.fields.insert("name".into(), text("Мука в/с"));
    goods.fields.insert("unit".into(), text("кг"));
    engine.save("goods", &goods, PostAction::Write).unwrap();
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    let r = engine
        .save(
            "receipt",
            &dated(receipt(main, mill, &[(flour, 2_000, 100)]), "2026-02-01"),
            PostAction::Post,
        )
        .unwrap();

    // Нехватка откатывает и документ, и его запись в журнале.
    let before = journal(&*engine).len();
    engine
        .save(
            "issue",
            &dated(issue(main, flour, 9_000), "2026-02-05"),
            PostAction::Post,
        )
        .unwrap_err();
    assert_eq!(journal(&*engine).len(), before);

    engine.unpost("receipt", r.id).unwrap();
    let version = engine.get("receipt", r.id).unwrap().version;
    engine.mark_deleted("receipt", r.id, version, true).unwrap();
    engine.set_closing_date(Some("2026-01-31")).unwrap();

    let entries = journal(&*engine);
    let actions: Vec<&str> = entries.iter().rev().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "Добавление пользователя",
            "Создание",
            "Изменение",
            "Создание",
            "Создание",
            "Создание",
            "Проведение",
            "Отмена проведения",
            "Пометка на удаление",
            "Установка даты закрытия",
        ]
    );
    assert!(entries.iter().all(|e| e.user == "boss"));
    let update = entries.iter().find(|e| e.action == "Изменение").unwrap();
    assert_eq!(update.object, format!("Товары #{flour}"));
    assert!(
        update.details.contains("Наименование: Мука → Мука в/с")
            && update.details.contains("Единица измерения: — → кг"),
        "{}",
        update.details
    );
    let created = entries
        .iter()
        .find(|e| e.action == "Создание" && e.object.starts_with("Поступление"))
        .unwrap();
    assert!(
        created.details.contains("Поставщик: — → Мельница"),
        "{}",
        created.details
    );
    assert!(
        entries[0].details.contains("31.01.2026"),
        "{}",
        entries[0].details
    );
    let ids: Vec<i64> = entries.iter().map(|e| e.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted.dedup();
    assert_eq!(ids, sorted);

    // Журнал — только администратору.
    engine
        .admin_add_user(&ostov_api::NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик".into(),
            role: "storekeeper".into(),
            password: KEEPER_PASSWORD.into(),
        })
        .unwrap();
    engine.logout().unwrap();
    engine.login("keeper", KEEPER_PASSWORD).unwrap();
    assert_eq!(
        engine.admin_journal(None, 10).unwrap_err().code,
        ErrorCode::Forbidden
    );
}

#[test]
fn script_error_keeps_trace_in_details() {
    struct Failing;
    impl crate::PostingRule for Failing {
        fn movements(&self, _: &crate::Document) -> Result<crate::Moves, crate::CoreError> {
            Err(crate::CoreError::Script {
                message: "Ошибка в логике документа «Поступление»: ValueError: x".into(),
                details: "Traceback (most recent call last): ...".into(),
            })
        }
    }
    struct Rules;
    impl super::Rules for Rules {
        fn rule(&self, _: &ostov_model::DocumentDef) -> Option<Box<dyn crate::PostingRule>> {
            Some(Box::new(Failing))
        }
    }
    let base = Base::new("script-error");
    let engine = LocalConnector::default()
        .with_rules(Arc::new(Rules))
        .create_local(&base.path, None, None)
        .unwrap();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    let err = engine
        .save(
            "receipt",
            &receipt(main, mill, &[(flour, 1_000, 100)]),
            PostAction::Post,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.starts_with("Ошибка в логике документа"));
    assert!(err.details.starts_with("Traceback"));
    assert_eq!(engine.list(&query("receipt", 10)).unwrap().total, Some(0));
}

#[test]
fn batch_is_all_or_nothing() {
    let base = Base::new("batch");
    let engine = with_admin(&base);
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    let flour = add(&*engine, "goods", "Мука");
    let item = |object: &str, data: ObjectData, post| ostov_api::BatchItem {
        object: object.into(),
        data,
        post,
    };
    let goods = |name: &str| ObjectData {
        fields: fields(&[("name", text(name))]),
        ..ObjectData::default()
    };
    let journal_before = journal(&*engine).len();

    // Третья запись не проходит (нехватка) — нет ни товара, ни поступления.
    let err = engine
        .save_batch(&[
            item("goods", goods("Сахар"), PostAction::Write),
            item(
                "receipt",
                dated(receipt(main, mill, &[(flour, 1_000, 100)]), "2026-02-01"),
                PostAction::Post,
            ),
            item(
                "issue",
                dated(issue(main, flour, 5_000), "2026-02-02"),
                PostAction::Post,
            ),
        ])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(
        err.message.starts_with("Запись 3: Не хватает остатка"),
        "{}",
        err.message
    );
    assert_eq!(engine.list(&query("goods", 10)).unwrap().total, Some(1));
    assert_eq!(engine.list(&query("receipt", 10)).unwrap().total, Some(0));
    assert_eq!(journal(&*engine).len(), journal_before);

    // Ошибка в данных ловится до записи — тоже с номером.
    let mut bad = goods("Соль");
    bad.fields.insert("price".into(), dec(1, 2));
    let err = engine
        .save_batch(&[
            item("goods", goods("Соль"), PostAction::Write),
            item("goods", bad, PostAction::Write),
        ])
        .unwrap_err();
    assert!(err.message.starts_with("Запись 2:"), "{}", err.message);

    let saved = engine
        .save_batch(&[
            item("goods", goods("Сахар"), PostAction::Write),
            item(
                "receipt",
                dated(receipt(main, mill, &[(flour, 1_000, 100)]), "2026-02-01"),
                PostAction::Post,
            ),
        ])
        .unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(
        engine.get("goods", saved[0].id).unwrap().fields["name"],
        text("Сахар")
    );
    assert!(engine.get("receipt", saved[1].id).unwrap().posted);
    assert_eq!(engine.list(&query("goods", 10)).unwrap().total, Some(2));

    // Права проверяются у каждой записи.
    engine
        .admin_add_user(&ostov_api::NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик".into(),
            role: "storekeeper".into(),
            password: KEEPER_PASSWORD.into(),
        })
        .unwrap();
    engine.logout().unwrap();
    engine.login("keeper", KEEPER_PASSWORD).unwrap();
    let err = engine
        .save_batch(&[item("goods", goods("Перец"), PostAction::Write)])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(err.message, "Запись 1: Нет права на создание «Товары».");
}

#[test]
fn failed_first_admin_leaves_no_base() {
    let base = Base::new("short-password");
    let admin = ostov_api::NewAdmin {
        login: "boss".into(),
        password: "short".into(),
    };
    let err = LocalConnector::default()
        .create_local(&base.path, None, Some(&admin))
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(!std::path::Path::new(&base.path).exists());
    // Файла нет — можно повторить с нормальным паролем.
    let admin = ostov_api::NewAdmin {
        password: ADMIN_PASSWORD.into(),
        ..admin
    };
    LocalConnector::default()
        .create_local(&base.path, None, Some(&admin))
        .unwrap();
}

#[test]
fn first_user_must_be_admin() {
    let base = Base::new("first-keeper");
    let engine = LocalConnector::new(roles_model())
        .create_local(&base.path, None, None)
        .unwrap();
    let keeper = ostov_api::NewUser {
        login: "keeper".into(),
        display_name: "Кладовщик".into(),
        role: "storekeeper".into(),
        password: KEEPER_PASSWORD.into(),
    };
    assert_eq!(
        engine.admin_add_user(&keeper).unwrap_err().code,
        ErrorCode::Validation
    );
    assert!(!engine.hello().unwrap().login_required);
    engine.create_first_admin("boss", ADMIN_PASSWORD).unwrap();
    engine.admin_add_user(&keeper).unwrap();
    let session = engine.login("keeper", KEEPER_PASSWORD).unwrap();
    assert!(!session.system.admin);
}

#[test]
fn passwords_hidden_in_debug() {
    let admin = ostov_api::NewAdmin {
        login: "boss".into(),
        password: ADMIN_PASSWORD.into(),
    };
    let user = ostov_api::NewUser {
        login: "keeper".into(),
        display_name: "Кладовщик".into(),
        role: "storekeeper".into(),
        password: KEEPER_PASSWORD.into(),
    };
    assert!(!format!("{admin:?}").contains(ADMIN_PASSWORD));
    assert!(!format!("{user:?}").contains(KEEPER_PASSWORD));
}

#[test]
fn checking_base_does_not_write() {
    let base = Base::new("check");
    {
        let engine = base.create();
        add(&*engine, "goods", "Мука");
    }
    let before = std::fs::read(&base.path).unwrap();
    let info = LocalConnector::default().check_local(&base.path).unwrap();
    assert_eq!(info.config_name, "warehouse");
    assert_eq!(info.config_title, "Склад");
    assert!(!info.login_required);
    assert!(!info.file_password);
    assert_eq!(std::fs::read(&base.path).unwrap(), before);

    let other = base.dir.join("other.db").to_string_lossy().into_owned();
    std::fs::write(&other, b"").unwrap();
    let err = LocalConnector::default().check_local(&other).err().unwrap();
    assert_eq!(err.code, ErrorCode::Incompatible);
    assert_eq!(std::fs::read(&other).unwrap(), b"");
    let missing = format!("{}.missing", base.path);
    let err = LocalConnector::default()
        .check_local(&missing)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn base_of_other_configuration_is_not_touched() {
    let base = Base::new("other-config");
    let mut model = super::warehouse_model();
    model.name = "recipes".into();
    model.title = "Рецептуры".into();
    model.documents.clear();
    model.registers.clear();
    model.menu.clear();
    drop(
        LocalConnector::new(model)
            .create_local(&base.path, None, None)
            .unwrap(),
    );
    let before = std::fs::read(&base.path).unwrap();
    let info = LocalConnector::default().check_local(&base.path).unwrap();
    assert_eq!(info.config_name, "recipes");
    assert_eq!(info.config_title, "Рецептуры");
    let err = LocalConnector::default()
        .open_local(&base.path, None)
        .err()
        .unwrap();
    assert_eq!(err.code, ErrorCode::Incompatible);
    assert_eq!(std::fs::read(&base.path).unwrap(), before);
}

#[test]
fn exact_filter_by_fields() {
    let base = Base::new("filter");
    let engine = base.create();
    let flour = add(&*engine, "goods", "Мука");
    add(&*engine, "goods", "Мука в/с");
    add(&*engine, "goods", "мука");
    let main = add(&*engine, "warehouses", "Основной");
    let kitchen = add(&*engine, "warehouses", "Кухня");
    let supplier = add(&*engine, "suppliers", "Мельница");
    for warehouse in [main, main, kitchen] {
        let doc = receipt(warehouse, supplier, &[(flour, 1000, 100)]);
        engine.save("receipt", &doc, PostAction::Write).unwrap();
    }
    let with = |object: &str, filter: &[(&str, Value)]| ListQuery {
        filter: fields(filter),
        ..query(object, 10)
    };

    // Текст — точно, но без учёта регистра: «Мука в/с» не попадает.
    let page = engine
        .list(&with("goods", &[("name", text("МУКА"))]))
        .unwrap();
    assert_eq!(page.total, Some(2));
    let page = engine
        .list(&with("receipt", &[("warehouse", refer(main))]))
        .unwrap();
    assert_eq!(page.total, Some(2));
    let both = with(
        "receipt",
        &[("warehouse", refer(kitchen)), ("supplier", refer(supplier))],
    );
    assert_eq!(engine.list(&both).unwrap().total, Some(1));
    let empty = with("receipt", &[("warehouse", Value::Null)]);
    assert_eq!(engine.list(&empty).unwrap().total, Some(0));

    // Отбор живёт в токене: seek видит те же строки.
    let page = engine
        .list(&with("receipt", &[("warehouse", refer(main))]))
        .unwrap();
    let jump = engine.seek(&page.token, 1, 10).unwrap();
    assert_eq!(jump.rows.len(), 1);
    assert_eq!(jump.total, Some(2));

    let err = engine.list(&with("goods", &[("color", text("красный"))]));
    assert_eq!(err.unwrap_err().code, ErrorCode::Validation);
    let err = engine.list(&with("receipt", &[("warehouse", text("Основной"))]));
    assert_eq!(err.unwrap_err().code, ErrorCode::Validation);
}

#[test]
fn meta_has_registers_in_config_order() {
    let base = Base::new("meta-registers");
    let meta = base.create().get_meta().unwrap();
    let [stock] = meta.registers.as_slice() else {
        panic!("ожидался один регистр: {:?}", meta.registers);
    };
    assert_eq!(stock.name, "stock");
    let names = |defs: &[ostov_api::FieldDef]| -> Vec<String> {
        defs.iter().map(|d| d.name.clone()).collect()
    };
    assert_eq!(names(&stock.dimensions), ["warehouse", "goods"]);
    assert_eq!(names(&stock.resources), ["qty"]);
}

#[test]
fn old_base_with_title_instead_of_name_opens() {
    let base = Base::new("old-config");
    drop(base.create());
    {
        let mut db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
        ostov_store::Driver::execute(&mut db, "DELETE FROM ostov_base WHERE key = 'title'", &[])
            .unwrap();
        ostov_store::Driver::execute(
            &mut db,
            "UPDATE ostov_base SET value = 'Склад' WHERE key = 'config'",
            &[],
        )
        .unwrap();
    }
    let info = LocalConnector::default().check_local(&base.path).unwrap();
    assert_eq!(
        (info.config_name.as_str(), info.config_title.as_str()),
        ("Склад", "Склад")
    );
    drop(
        LocalConnector::default()
            .open_local(&base.path, None)
            .unwrap(),
    );
    let info = LocalConnector::default().check_local(&base.path).unwrap();
    assert_eq!(info.config_name, "warehouse");

    // Новое название той же конфигурации базу не отрезает.
    let mut model = super::warehouse_model();
    model.title = "Склад и кухня".into();
    drop(
        LocalConnector::new(model)
            .open_local(&base.path, None)
            .unwrap(),
    );
    let info = LocalConnector::default().check_local(&base.path).unwrap();
    assert_eq!(info.config_title, "Склад и кухня");
}

#[test]
fn admin_sets_user_password() {
    let base = Base::new("set-password");
    let engine = LocalConnector::new(roles_model())
        .create_local(&base.path, None, None)
        .unwrap();
    engine.create_first_admin("boss", ADMIN_PASSWORD).unwrap();
    engine
        .admin_add_user(&ostov_api::NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик".into(),
            role: "storekeeper".into(),
            password: KEEPER_PASSWORD.into(),
        })
        .unwrap();
    let short = engine.admin_set_user_password("keeper", "short");
    assert_eq!(short.unwrap_err().code, ErrorCode::Validation);
    let missing = engine.admin_set_user_password("nobody", "new-password-12");
    assert_eq!(missing.unwrap_err().code, ErrorCode::NotFound);
    engine
        .admin_set_user_password("Keeper", "new-password-12")
        .unwrap();
    let entries = engine.admin_journal(None, 10).unwrap();
    assert_eq!(entries[0].action, "Смена пароля");
    assert!(!entries[0].details.contains("new-password-12"));

    engine.logout().unwrap();
    let old = engine.login("keeper", KEEPER_PASSWORD);
    assert_eq!(old.unwrap_err().code, ErrorCode::BadCredentials);
    engine.login("keeper", "new-password-12").unwrap();
    let denied = engine.admin_set_user_password("boss", "keeper-took-it-1");
    assert_eq!(denied.unwrap_err().code, ErrorCode::Forbidden);
}

#[test]
fn local_policy_allows_any_password_but_strict_blocks_weak_ones() {
    let base = Base::new("password-policy");
    let local =
        || LocalConnector::new(roles_model()).with_password_policy(crate::PasswordPolicy::LOCAL);
    {
        let engine = local().create_local(&base.path, None, None).unwrap();
        engine.create_first_admin("boss", ADMIN_PASSWORD).unwrap();
        for (login, password) in [("keeper", "1234"), ("cook", "")] {
            engine
                .admin_add_user(&ostov_api::NewUser {
                    login: login.into(),
                    display_name: "Кладовщик".into(),
                    role: "storekeeper".into(),
                    password: password.into(),
                })
                .unwrap();
        }
        engine.logout().unwrap();
        engine.login("cook", "").unwrap();
    }

    // Та же база со строгой политикой (так её откроет сервер): слабых не пускает.
    let engine = LocalConnector::new(roles_model())
        .open_local(&base.path, None)
        .unwrap();
    let err = engine.login("keeper", "1234").unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("сменить"), "{}", err.message);
    engine.login("boss", ADMIN_PASSWORD).unwrap();
    let short = engine.admin_set_user_password("keeper", "12345");
    assert_eq!(short.unwrap_err().code, ErrorCode::Validation);
    engine
        .admin_set_user_password("keeper", KEEPER_PASSWORD)
        .unwrap();
    engine.logout().unwrap();
    engine.login("keeper", KEEPER_PASSWORD).unwrap();
    let failed = engine.admin_journal(None, 50);
    assert_eq!(failed.unwrap_err().code, ErrorCode::Forbidden);
}

#[test]
fn console_reset_needs_no_login_but_strict_password() {
    let base = Base::new("console-reset");
    let local =
        LocalConnector::new(roles_model()).with_password_policy(crate::PasswordPolicy::LOCAL);
    drop({
        let engine = local.create_local(&base.path, None, None).unwrap();
        engine.create_first_admin("boss", "").unwrap();
        engine
    });
    let config = crate::local::Config::from_model(&roles_model()).unwrap();
    let db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    let engine = crate::LocalEngine::open(Box::new(db), config).unwrap();
    assert_eq!(
        engine.reset_password("boss", "").unwrap_err().code,
        ErrorCode::Validation
    );
    engine.reset_password("Boss", ADMIN_PASSWORD).unwrap();
    engine.login("boss", ADMIN_PASSWORD).unwrap();
    let entries = engine.admin_journal(None, 10).unwrap();
    let reset = entries.iter().find(|e| e.action == "Смена пароля").unwrap();
    assert_eq!(reset.user, "консоль сервера");
    assert!(!reset.details.contains(ADMIN_PASSWORD));
}

#[test]
fn nul_in_text_is_validation_not_database_error() {
    let base = Base::new("nul-text");
    let engine = base.create();
    let data = ObjectData {
        fields: fields(&[("name", text("Мука\0"))]),
        ..ObjectData::default()
    };
    let err = engine.save("goods", &data, PostAction::Write).unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.contains("кодом 0"), "{}", err.message);
}

fn sql(engine: &dyn Engine, text: &str, params: &[(&str, Value)]) -> ostov_api::QueryResult {
    engine
        .query(text, &fields(params))
        .unwrap_or_else(|e| panic!("{text}: {e:?}"))
}

#[test]
fn query_reads_money_refs_and_dates() {
    let base = Base::new("query");
    let engine = base.create();
    let flour = add(&*engine, "goods", "Мука");
    let main = add(&*engine, "warehouses", "Основной");
    let mill = add(&*engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &receipt(main, mill, &[(flour, 10_000, 150)]),
            PostAction::Post,
        )
        .unwrap();

    let q = "SELECT g.name, l.qty, l.price, l.goods FROM receipt.lines l \
             JOIN goods g ON g.id = l.goods WHERE l.price >= :min";
    let found = sql(&*engine, q, &[("min", dec(15, 1))]);
    assert_eq!(found.columns, ["name", "qty", "price", "goods"]);
    assert_eq!(
        found.rows,
        [vec![
            text("Мука"),
            dec(10_000, 3),
            dec(150, 2),
            Value::Ref {
                id: flour,
                title: Some("Мука".into())
            }
        ]]
    );
    // Целый параметр рядом с деньгами — целые рубли, а не копейки.
    assert!(sql(&*engine, q, &[("min", Value::Int(2))]).rows.is_empty());

    let found = sql(&*engine, "SELECT date, number, posted FROM receipt", &[]);
    assert_eq!(
        found.rows,
        [vec![
            Value::Date("2026-10-30".into()),
            Value::Int(1),
            Value::Bool(true)
        ]]
    );
    let found = sql(
        &*engine,
        "SELECT SUM(amount) AS total FROM receipt WHERE posted = :yes",
        &[("yes", Value::Bool(true))],
    );
    assert_eq!(found.rows, [vec![dec(1_500, 2)]]);

    let err = engine
        .query("DELETE FROM goods", &Fields::new())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert_eq!(err.message, "В запросе можно только SELECT");
    let err = engine
        .query(
            "SELECT price FROM receipt.lines WHERE price > :p",
            &fields(&[("p", dec(1, 3))]),
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.contains("«p»"), "{}", err.message);
}

#[test]
fn query_checks_rights_of_who_asks() {
    let base = Base::new("query-rights");
    let engine = LocalConnector::new(roles_model())
        .create_local(
            &base.path,
            None,
            Some(&ostov_api::NewAdmin {
                login: "boss".into(),
                password: ADMIN_PASSWORD.into(),
            }),
        )
        .unwrap();
    add(&*engine, "goods", "Мука");
    engine
        .admin_add_user(&ostov_api::NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик".into(),
            role: "storekeeper".into(),
            password: KEEPER_PASSWORD.into(),
        })
        .unwrap();
    engine.logout().unwrap();
    let err = engine
        .query("SELECT name FROM goods", &Fields::new())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);

    engine.login("keeper", KEEPER_PASSWORD).unwrap();
    let found = sql(&*engine, "SELECT name FROM goods", &[]);
    assert_eq!(found.rows, [vec![text("Мука")]]);
    let err = engine
        .query("SELECT number FROM receipt", &Fields::new())
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(err.message, "Нет права на просмотр «Поступление»");
}

/// Расход списывает строки, прочитав их запросом: так правило видит базу внутри своей транзакции.
struct IssueByQuery;

impl crate::PostingRule for IssueByQuery {
    fn movements(&self, _: &crate::Document) -> Result<crate::Moves, crate::CoreError> {
        Err(crate::CoreError::Invalid("правилу нужна база".into()))
    }

    fn movements_reading(
        &self,
        doc: &crate::Document,
        read: &mut dyn crate::Reader,
    ) -> Result<crate::Moves, crate::CoreError> {
        let params = [("id".to_owned(), ostov_sql::Param::Int(doc.id))].into();
        let found = read.query(
            "SELECT goods, qty FROM issue.lines WHERE doc_id = :id",
            &params,
        )?;
        let stock = found
            .rows
            .into_iter()
            .map(|row| {
                let [goods, crate::FieldValue::Decimal(qty)] = &row[..] else {
                    panic!("{row:?}");
                };
                crate::Movement {
                    kind: crate::MovementKind::Issue,
                    dims: [
                        ("warehouse".to_owned(), doc.header["warehouse"].clone()),
                        ("goods".to_owned(), goods.clone()),
                    ]
                    .into(),
                    resources: [("qty".to_owned(), *qty)].into(),
                }
            })
            .collect();
        Ok([("stock".to_owned(), stock)].into())
    }
}

struct QueryRules;

impl super::Rules for QueryRules {
    fn rule(&self, document: &ostov_model::DocumentDef) -> Option<Box<dyn crate::PostingRule>> {
        match document.name.as_str() {
            "issue" => Some(Box::new(IssueByQuery)),
            _ => super::BuiltinRules.rule(document),
        }
    }
}

#[test]
fn posting_rule_reads_base_in_its_transaction() {
    let base = Base::new("rule-query");
    let db = ostov_store::drivers::sqlite::SqliteDriver::open(&base.path).unwrap();
    let config = super::Config::from_model_with(&super::warehouse_model(), &QueryRules).unwrap();
    let engine = super::LocalEngine::install(Box::new(db), config).unwrap();
    let flour = add(&engine, "goods", "Мука");
    let main = add(&engine, "warehouses", "Основной");
    let mill = add(&engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &receipt(main, mill, &[(flour, 10_000, 100)]),
            PostAction::Post,
        )
        .unwrap();
    // Новый документ и проведение — одна транзакция: правило уже видит его строки.
    let id = engine
        .save("issue", &issue(main, flour, 3_000), PostAction::Post)
        .unwrap()
        .id;
    engine.post("issue", id).unwrap();
    let found = sql(
        &engine,
        "SELECT SUM(qty) AS qty FROM stock WHERE recorder = :doc",
        &[("doc", text("issue"))],
    );
    assert_eq!(found.rows, [vec![dec(3_000, 3)]]);
}

#[test]
fn journal_page_says_when_it_ends() {
    let base = Base::new("journal-pages");
    let engine = with_admin(&base);
    for name in ["Мука", "Сахар", "Соль"] {
        add(&*engine, "goods", name);
    }
    let all = journal(&*engine);
    assert!(all.len() >= 4, "{all:?}");
    // Страницы по 2: ключ есть, пока записи не кончились, и нет пустой последней страницы.
    for size in [1, 2, u32::try_from(all.len()).unwrap()] {
        let mut paged = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = engine.admin_journal_page(after.as_deref(), size).unwrap();
            assert!(!page.entries.is_empty(), "пустая страница при size {size}");
            paged.extend(page.entries);
            match page.next_key {
                Some(key) => after = Some(key),
                None => break,
            }
        }
        assert_eq!(paged, all, "size {size}");
    }
}
