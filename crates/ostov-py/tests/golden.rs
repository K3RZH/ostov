//! Золотой тест: один сценарий на базе со встроенными правилами «Склада» и на базе с правилами
//! из `config/documents/*.py` даёт одно и то же — документы, суммы, отчёты и ошибки нехватки.

#![cfg(feature = "python")]
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{model, repo_config};
use ostov_api::{Connector, Engine, Fields, ObjectData, PostAction, ReportQuery, Value};
use ostov_core::LocalConnector;
use ostov_py::PyRules;

struct Base {
    dir: PathBuf,
}

impl Base {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("ostov-py-golden-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn create(&self, connector: &LocalConnector) -> Arc<dyn Engine> {
        let path = self.dir.join("shop.db").to_string_lossy().into_owned();
        connector.create_local(&path, None, None).unwrap()
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn refer(id: i64) -> Value {
    Value::Ref { id, title: None }
}

fn dec(units: i64, scale: u8) -> Value {
    Value::Decimal { units, scale }
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

/// Строки: (товар, количество в тысячных, цена в копейках или `None`).
fn receipt(
    day: &str,
    warehouse: i64,
    supplier: i64,
    lines: &[(i64, i64, Option<i64>)],
) -> ObjectData {
    let lines = lines
        .iter()
        .map(|&(goods, qty, price)| {
            let mut line = fields(&[("goods", refer(goods)), ("qty", dec(qty, 3))]);
            if let Some(price) = price {
                line.insert("price".into(), dec(price, 2));
            }
            line
        })
        .collect();
    ObjectData {
        fields: fields(&[
            ("date", Value::Date(day.into())),
            ("warehouse", refer(warehouse)),
            ("supplier", refer(supplier)),
        ]),
        tables: [("lines".to_owned(), lines)].into(),
        ..ObjectData::default()
    }
}

fn issue(day: &str, warehouse: i64, goods: i64, qty: i64) -> ObjectData {
    ObjectData {
        fields: fields(&[
            ("date", Value::Date(day.into())),
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

fn report(engine: &dyn Engine, name: &str, params: &[(&str, Value)]) -> String {
    let query = ReportQuery {
        report: name.into(),
        params: fields(params),
        after_key: None,
        size: 1000,
    };
    format!(
        "{name} {params:?}: {:?}",
        engine.run_report(&query).map(|page| page.rows)
    )
}

/// Сценарий из тестов ядра: поступления (с округлением и без цены), расход, расход задним
/// числом, уменьшение старого поступления, отмена проведения, отчёты. Каждый шаг — строка журнала.
fn scenario(engine: &dyn Engine) -> Vec<String> {
    let mut log = Vec::new();
    let add = |object: &str, name: &str| {
        let data = ObjectData {
            fields: fields(&[("name", Value::Text(name.into()))]),
            ..ObjectData::default()
        };
        engine.save(object, &data, PostAction::Write).unwrap().id
    };
    let flour = add("goods", "Мука");
    let sugar = add("goods", "Сахар");
    let main = add("warehouses", "Основной");
    let kitchen = add("warehouses", "Кухня");
    let mill = add("suppliers", "Мельница");

    let save = |log: &mut Vec<String>, object: &str, data: &ObjectData| {
        let saved = engine.save(object, data, PostAction::Post);
        let outcome = saved
            .as_ref()
            .map(|s| format!("{:?}", engine.get(object, s.id)))
            .unwrap_or_else(|e| format!("ошибка {e:?}"));
        log.push(format!("save {object}: {outcome}"));
        saved.ok().map(|s| s.id)
    };

    // 2,5 × 3,33 = 8,325 → 8,33 и 1,005 × 1,00 → 1,01 (половина — от нуля); без цены — без суммы.
    let first = save(
        &mut log,
        "receipt",
        &receipt(
            "2026-01-10",
            main,
            mill,
            &[
                (flour, 2_500, Some(333)),
                (sugar, 1_005, Some(100)),
                (flour, 7_500, None),
            ],
        ),
    )
    .unwrap();
    save(&mut log, "issue", &issue("2026-01-20", main, flour, 3_000));
    save(&mut log, "issue", &issue("2026-01-05", main, sugar, 1_000));
    save(
        &mut log,
        "receipt",
        &receipt("2026-02-06", kitchen, mill, &[(sugar, 2_000, Some(1_000))]),
    );
    save(&mut log, "issue", &issue("2026-02-10", main, flour, 6_000));

    let mut shrunk = engine.get("receipt", first).unwrap();
    if let Some(line) = shrunk
        .tables
        .get_mut("lines")
        .and_then(|lines| lines.first_mut())
    {
        line.insert("qty".into(), dec(1_000, 3));
    }
    save(&mut log, "receipt", &shrunk);

    log.push(format!(
        "unpost receipt: {:?}",
        engine.unpost("receipt", first)
    ));
    let issues = engine
        .list(&ostov_api::ListQuery {
            object: "issue".into(),
            size: 10,
            ..Default::default()
        })
        .unwrap();
    let last = issues.rows.last().unwrap();
    log.push(format!(
        "unpost issue: {:?}",
        engine.unpost("issue", last.id)
    ));

    log.push(report(engine, "stock_balance", &[]));
    log.push(report(
        engine,
        "stock_balance",
        &[("date", Value::Date("2026-01-15".into()))],
    ));
    let january = [
        ("from", Value::Date("2026-01-01".into())),
        ("to", Value::Date("2026-01-31".into())),
    ];
    log.push(report(engine, "stock_turnover", &january));
    let february = [
        ("from", Value::Date("2026-02-01".into())),
        ("to", Value::Date("2026-02-28".into())),
    ];
    log.push(report(engine, "stock_turnover", &february));
    log
}

#[test]
fn python_rules_give_the_same_result_as_builtin() {
    let model = model();
    let rules = PyRules::load(&repo_config(), &model).unwrap();

    let builtin_base = Base::new("builtin");
    let builtin = scenario(&*builtin_base.create(&LocalConnector::new(model.clone())));
    let python_base = Base::new("python");
    let python_connector = LocalConnector::new(model).with_rules(Arc::new(rules));
    let python = scenario(&*python_base.create(&python_connector));

    assert_eq!(builtin.len(), python.len());
    for (step, (b, p)) in builtin.iter().zip(&python).enumerate() {
        assert_eq!(b, p, "шаг {step} разошёлся");
    }

    // Совпадение не пустое: суммы посчитаны, а нехватка и запрет отмены сработали.
    let joined = python.join("\n");
    assert!(joined.contains("units: 833, scale: 2"), "8,33 в строке");
    assert!(joined.contains("units: 101, scale: 2"), "1,01 в строке");
    assert!(joined.contains("units: 934, scale: 2"), "итог 9,34");
    assert!(
        joined.contains("Не хватает остатка на 05.01.2026"),
        "расход задним числом"
    );
    assert!(
        joined.contains("Не хватает остатка на 10.02.2026"),
        "уменьшение прихода"
    );
    assert!(
        python[0].contains("\"amount\": Null"),
        "строка без цены — без суммы"
    );
}

/// Ошибки логики через движок — как их увидит интерфейс: код, текст и трассировка в details.
#[test]
fn script_errors_reach_the_user_through_engine() {
    let model = model();
    let scripts = common::Scripts::new("engine-errors");
    scripts
        .write(
            "receipt",
            "import ostov\n\ndef on_post(doc, ctx):\n    if doc.lines[0].qty > 100:\n        raise ostov.UserError('Слишком много за раз')\n    raise ValueError('сломано')\n",
        )
        .write("issue", common::QUIET_ISSUE);
    let rules = scripts.load(&model).unwrap();
    let base = Base::new("errors");
    let engine = base.create(&LocalConnector::new(model).with_rules(Arc::new(rules)));
    let add = |object: &str, name: &str| {
        let data = ObjectData {
            fields: fields(&[("name", Value::Text(name.into()))]),
            ..ObjectData::default()
        };
        engine.save(object, &data, PostAction::Write).unwrap().id
    };
    let (flour, main, mill) = (
        add("goods", "Мука"),
        add("warehouses", "Основной"),
        add("suppliers", "Мельница"),
    );

    let big = receipt("2026-01-10", main, mill, &[(flour, 200_000, Some(100))]);
    let error = engine.save("receipt", &big, PostAction::Post).unwrap_err();
    assert_eq!(error.code, ostov_api::ErrorCode::Validation);
    assert_eq!(error.message, "Слишком много за раз");

    let small = receipt("2026-01-10", main, mill, &[(flour, 1_000, Some(100))]);
    let error = engine
        .save("receipt", &small, PostAction::Post)
        .unwrap_err();
    assert_eq!(error.code, ostov_api::ErrorCode::Validation);
    assert_eq!(
        error.message,
        "Ошибка в логике документа «Поступление»: ValueError: сломано"
    );
    assert!(
        error.details.contains("receipt.py\", line 6"),
        "{}",
        error.details
    );

    let list = ostov_api::ListQuery {
        object: "receipt".into(),
        size: 10,
        ..Default::default()
    };
    assert_eq!(
        engine.list(&list).unwrap().total,
        Some(0),
        "транзакция откатилась"
    );
}
