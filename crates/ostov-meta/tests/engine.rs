//! Модель из YAML работает в ядре: база создаётся, поступление проводится, остатки считаются.

#![allow(clippy::unwrap_used)]

mod common;

use common::repo_config;
use ostov_api::{Connector, ObjectData, PostAction, ReportQuery, Value};
use ostov_core::LocalConnector;

#[test]
fn yaml_config_runs_in_core() {
    let dir = std::env::temp_dir().join(format!("ostov-meta-engine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shop.db").to_string_lossy().into_owned();

    let engine = LocalConnector::new(ostov_meta::load(&repo_config()).unwrap())
        .create_local(&path, None, None)
        .unwrap();
    let add = |object: &str, name: &str| {
        let data = ObjectData {
            fields: [("name".to_owned(), Value::Text(name.to_owned()))].into(),
            ..ObjectData::default()
        };
        engine.save(object, &data, PostAction::Write).unwrap().id
    };
    let flour = add("goods", "Мука");
    let main = add("warehouses", "Основной");
    let mill = add("suppliers", "Мельница");
    let refer = |id| Value::Ref { id, title: None };
    let dec = |units, scale| Value::Decimal { units, scale };

    let receipt = ObjectData {
        fields: [
            ("date".to_owned(), Value::Date("2026-02-01".into())),
            ("supplier".to_owned(), refer(mill)),
            ("warehouse".to_owned(), refer(main)),
        ]
        .into(),
        tables: [(
            "lines".to_owned(),
            vec![
                [
                    ("goods".to_owned(), refer(flour)),
                    ("qty".to_owned(), dec(2_500, 3)),
                    ("price".to_owned(), dec(4_000, 2)),
                ]
                .into(),
            ],
        )]
        .into(),
        ..ObjectData::default()
    };
    let saved = engine.save("receipt", &receipt, PostAction::Post).unwrap();
    assert_eq!(
        engine.get("receipt", saved.id).unwrap().fields["amount"],
        dec(10_000, 2)
    );

    let report = engine
        .run_report(&ReportQuery {
            report: "stock_balance".into(),
            params: Default::default(),
            after_key: None,
            size: 10,
        })
        .unwrap();
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].values["qty"], dec(2_500, 3));

    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
