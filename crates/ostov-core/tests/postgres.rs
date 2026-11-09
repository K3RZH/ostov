//! Ядро на живом PostgreSQL через `Engine`, как его зовёт интерфейс.
//! Идут, только если задан `OSTOV_PG_URL`, иначе видны как ignored; каждый тест — в своей схеме.

#![allow(clippy::unwrap_used, clippy::panic)]

use ostov_api::{Engine, ErrorCode, Fields, ListQuery, ObjectData, PostAction, ReportQuery, Value};
use ostov_core::LocalEngine;
use ostov_core::local::{Config, warehouse_model};
use ostov_store::Driver;
use ostov_store::drivers::postgres::PostgresDriver;

fn connect(schema: &str, fresh: bool) -> Option<PostgresDriver> {
    let url = std::env::var("OSTOV_PG_URL").ok()?;
    let mut db = PostgresDriver::connect(&url).unwrap();
    if fresh {
        db.execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), &[])
            .unwrap();
        db.execute(&format!("CREATE SCHEMA {schema}"), &[]).unwrap();
    }
    db.execute(&format!("SET search_path TO {schema}"), &[])
        .unwrap();
    Some(db)
}

fn install(schema: &str) -> Option<LocalEngine> {
    let db = connect(schema, true)?;
    let config = Config::from_model(&warehouse_model()).unwrap();
    Some(LocalEngine::install(Box::new(db), config).unwrap())
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

fn date(d: &str) -> Value {
    Value::Date(d.to_owned())
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

fn receipt(day: &str, supplier: i64, warehouse: i64, lines: &[(i64, i64, i64)]) -> ObjectData {
    ObjectData {
        fields: fields(&[
            ("date", date(day)),
            ("supplier", refer(supplier)),
            ("warehouse", refer(warehouse)),
        ]),
        tables: [(
            "lines".to_owned(),
            lines
                .iter()
                .map(|(goods, qty, price)| {
                    fields(&[
                        ("goods", refer(*goods)),
                        ("qty", dec(*qty, 3)),
                        ("price", dec(*price, 2)),
                    ])
                })
                .collect(),
        )]
        .into(),
        ..ObjectData::default()
    }
}

fn issue(day: &str, warehouse: i64, goods: i64, qty: i64) -> ObjectData {
    ObjectData {
        fields: fields(&[("date", date(day)), ("warehouse", refer(warehouse))]),
        tables: [(
            "lines".to_owned(),
            vec![fields(&[("goods", refer(goods)), ("qty", dec(qty, 3))])],
        )]
        .into(),
        ..ObjectData::default()
    }
}

fn query(object: &str) -> ListQuery {
    ListQuery {
        object: object.into(),
        size: 100,
        ..ListQuery::default()
    }
}

fn report(engine: &dyn Engine, name: &str, params: &[(&str, Value)]) -> Vec<Fields> {
    let q = ReportQuery {
        report: name.into(),
        params: fields(params),
        after_key: None,
        size: 100,
    };
    engine
        .run_report(&q)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.values)
        .collect()
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn warehouse_on_postgres() {
    let Some(engine) = install("ostov_t_flow") else {
        return;
    };
    let flour = add(&engine, "goods", "Мука");
    let sugar = add(&engine, "goods", "Сахар");
    let main = add(&engine, "warehouses", "Основной");
    let mill = add(&engine, "suppliers", "Мельница");
    let bakery = add(&engine, "suppliers", "Бакалея");

    // 2.5 кг по 3.33 = 8.325 → 8.33: суммы считает ядро.
    let r1 = engine
        .save(
            "receipt",
            &receipt("2026-02-01", mill, main, &[(flour, 2_500, 333)]),
            PostAction::Post,
        )
        .unwrap();
    assert_eq!(
        engine.get("receipt", r1.id).unwrap().fields["amount"],
        dec(833, 2)
    );
    engine
        .save(
            "receipt",
            &receipt("2026-02-03", bakery, main, &[(sugar, 1_000, 100)]),
            PostAction::Write,
        )
        .unwrap();
    engine
        .save(
            "issue",
            &issue("2026-02-05", main, flour, 1_000),
            PostAction::Post,
        )
        .unwrap();

    // Остаток по датам: расход раньше прихода не проходит, текст понятный.
    let err = engine
        .save(
            "issue",
            &issue("2026-01-15", main, flour, 1_000),
            PostAction::Post,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(err.message.contains("15.01.2026"), "{}", err.message);
    assert!(err.message.contains("«Мука»"), "{}", err.message);

    // Списки: поиск по названию ссылки без учёта регистра, сортировка по ней, отбор.
    let found = engine
        .list(&ListQuery {
            search: Some("МЕЛЬН".into()),
            ..query("receipt")
        })
        .unwrap();
    assert_eq!(found.rows.len(), 1);
    let exact = engine
        .list(&ListQuery {
            filter: fields(&[("name", text("МУКА"))]),
            ..query("goods")
        })
        .unwrap();
    assert_eq!(exact.total, Some(1));
    assert_eq!(found.rows[0].values["deleted"], Value::Bool(false));
    let sorted = engine
        .list(&ListQuery {
            order: Some("supplier".into()),
            ..query("receipt")
        })
        .unwrap();
    let titles: Vec<Value> = sorted
        .rows
        .iter()
        .map(|r| r.values["supplier"].clone())
        .collect();
    assert_eq!(
        titles,
        vec![
            Value::Ref {
                id: bakery,
                title: Some("Бакалея".into())
            },
            Value::Ref {
                id: mill,
                title: Some("Мельница".into())
            },
        ]
    );
    let drafts = engine
        .list(&ListQuery {
            posted: Some(false),
            ..query("receipt")
        })
        .unwrap();
    assert_eq!(drafts.total, Some(1));
    let seek = engine.seek(&drafts.token, 0, 10).unwrap();
    assert_eq!(seek.rows.len(), 1);

    // Отчёты.
    let balance = report(&engine, "stock_balance", &[]);
    assert_eq!(balance.len(), 1);
    assert_eq!(balance[0]["qty"], dec(1_500, 3));
    let feb = report(
        &engine,
        "stock_turnover",
        &[("from", date("2026-02-01")), ("to", date("2026-02-28"))],
    );
    assert_eq!(feb[0]["qty_receipt"], dec(2_500, 3));
    assert_eq!(feb[0]["qty_issue"], dec(1_000, 3));

    // Версии.
    let stale = engine.get("goods", flour).unwrap();
    let mut fresh = stale.clone();
    fresh.fields.insert("unit".into(), text("кг"));
    engine.save("goods", &fresh, PostAction::Write).unwrap();
    assert_eq!(
        engine
            .save("goods", &stale, PostAction::Write)
            .unwrap_err()
            .code,
        ErrorCode::VersionConflict
    );

    // Закрытие периода, настройки, метаданные.
    engine.set_closing_date(Some("2026-02-02")).unwrap();
    assert_eq!(
        engine.unpost("receipt", r1.id).unwrap_err().code,
        ErrorCode::PeriodClosed
    );
    engine
        .save_settings("receipt.list", &fields(&[("rows", Value::Int(20))]))
        .unwrap();
    assert_eq!(
        engine.get_settings("receipt.list").unwrap(),
        Some(fields(&[("rows", Value::Int(20))]))
    );
    assert_eq!(engine.get_meta().unwrap().objects.len(), 5);
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn restructure_on_postgres() {
    let schema = "ostov_t_restructure";
    let Some(engine) = install(schema) else {
        return;
    };
    let flour = add(&engine, "goods", "Мука");
    let main = add(&engine, "warehouses", "Основной");
    let mill = add(&engine, "suppliers", "Мельница");
    engine
        .save(
            "receipt",
            &receipt("2026-02-01", mill, main, &[(flour, 1_000, 100)]),
            PostAction::Post,
        )
        .unwrap();
    drop(engine);

    // Новое поле в справочнике.
    let mut model = warehouse_model();
    model.catalogs[0].fields.push(ostov_core::FieldDef::new(
        "barcode",
        ostov_core::FieldKind::Text,
    ));
    let config = Config::from_model(&model).unwrap();
    let engine = LocalEngine::open(Box::new(connect(schema, false).unwrap()), config).unwrap();
    let mut goods = engine.get("goods", flour).unwrap();
    assert_eq!(goods.fields["barcode"], Value::Null);
    goods.fields.insert("barcode".into(), text("460"));
    engine.save("goods", &goods, PostAction::Write).unwrap();
    drop(engine);

    // Смена типа отклоняется, база цела.
    let mut bad = warehouse_model();
    bad.catalogs[0].fields[1].kind = ostov_core::FieldKind::Int;
    let config = Config::from_model(&bad).unwrap();
    let Err(err) = LocalEngine::open(Box::new(connect(schema, false).unwrap()), config) else {
        panic!("смена типа прошла");
    };
    assert_eq!(err.code, ErrorCode::Incompatible);

    // База до отчётов: без даты в движениях — колонка добавляется и заполняется.
    {
        let mut db = connect(schema, false).unwrap();
        db.execute("DROP INDEX stock__period", &[]).unwrap();
        db.execute("ALTER TABLE stock DROP COLUMN period", &[])
            .unwrap();
    }
    let config = Config::from_model(&model).unwrap();
    let engine = LocalEngine::open(Box::new(connect(schema, false).unwrap()), config).unwrap();
    let jan = report(&engine, "stock_balance", &[("date", date("2026-01-31"))]);
    assert!(jan.is_empty());
    let feb = report(&engine, "stock_balance", &[("date", date("2026-02-28"))]);
    assert_eq!(feb[0]["qty"], dec(1_000, 3));
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn users_on_postgres() {
    let schema = "ostov_t_users";
    let Some(engine) = install(schema) else {
        return;
    };
    engine
        .create_first_admin("boss", "admin-password-1")
        .unwrap();
    add(&engine, "goods", "Мука");
    drop(engine);

    let config = Config::from_model(&warehouse_model()).unwrap();
    let engine = LocalEngine::open(Box::new(connect(schema, false).unwrap()), config).unwrap();
    assert!(engine.hello().unwrap().login_required);
    assert_eq!(
        engine.list(&query("goods")).unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        engine.login("boss", "wrong-password-1").unwrap_err().code,
        ErrorCode::BadCredentials
    );
    engine.login("boss", "admin-password-1").unwrap();
    assert_eq!(engine.list(&query("goods")).unwrap().rows.len(), 1);
    assert_eq!(
        engine
            .admin_set_user_active("boss", false)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
}
