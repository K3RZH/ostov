//! `ctx.query`: правило на Python читает базу внутри транзакции проведения.

#![cfg(feature = "python")]
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{Scripts, model, repo_config};
use ostov_api::{
    Connector, Engine, ErrorCode, Fields, NewAdmin, NewUser, ObjectData, PostAction, Value,
};
use ostov_core::LocalConnector;
use ostov_py::PyRules;

const ISSUE: &str = r#"
import datetime
from decimal import Decimal

import ostov

kept = []


def on_post(doc, ctx):
    if kept:
        try:
            kept[0].query("SELECT name FROM goods")
        except ostov.QueryError:
            pass
        else:
            raise ostov.UserError("старый ctx ещё читает базу")
    kept.append(ctx)
    prices = ctx.query(
        "SELECT l.price, r.date FROM receipt.lines l JOIN receipt r ON r.id = l.doc_id "
        "WHERE l.price > :p AND r.date <= :d",
        p=Decimal("1.5"),
        d=datetime.date(2026, 12, 31),
    )
    got = [(row.price, row.date) for row in prices]
    if got != [(Decimal("2.00"), datetime.date(2026, 10, 30))]:
        raise ostov.UserError(f"цены: {got}")
    for row in ctx.query("SELECT goods, qty FROM issue.lines WHERE doc_id = :id", id=doc.id):
        ctx.issue("stock", warehouse=doc.warehouse, goods=row.goods, qty=row.qty)
"#;

struct Base {
    scripts: Scripts,
    dir: PathBuf,
}

impl Base {
    fn new(test: &str, issue: &str) -> Self {
        let scripts = Scripts::new(test);
        let receipt = std::fs::read_to_string(repo_config().join("documents/receipt.py")).unwrap();
        scripts.write("receipt", &receipt).write("issue", issue);
        let dir = std::env::temp_dir().join(format!("ostov-py-db-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { scripts, dir }
    }

    fn create(&self, model: ostov_model::Config, admin: Option<&NewAdmin>) -> Arc<dyn Engine> {
        let rules = self.scripts.load(&model).unwrap();
        let path = self.dir.join("shop.db").to_string_lossy().into_owned();
        LocalConnector::new(model)
            .with_rules(Arc::new(rules) as Arc<PyRules>)
            .create_local(&path, None, admin)
            .unwrap()
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

fn add(engine: &dyn Engine, object: &str, name: &str) -> i64 {
    let data = ObjectData {
        fields: fields(&[("name", Value::Text(name.into()))]),
        ..ObjectData::default()
    };
    engine.save(object, &data, PostAction::Write).unwrap().id
}

fn document(warehouse: i64, supplier: Option<i64>, line: Fields) -> ObjectData {
    let mut header = fields(&[
        ("date", Value::Date("2026-10-30".into())),
        ("warehouse", refer(warehouse)),
    ]);
    if let Some(supplier) = supplier {
        header.insert("supplier".into(), refer(supplier));
    }
    ObjectData {
        fields: header,
        tables: [("lines".to_owned(), vec![line])].into(),
        ..ObjectData::default()
    }
}

/// Склад с мукой: 10 кг по 2,00. Возвращает (склад, мука).
fn stocked(engine: &dyn Engine) -> (i64, i64) {
    let flour = add(engine, "goods", "Мука");
    let main = add(engine, "warehouses", "Основной");
    let mill = add(engine, "suppliers", "Мельница");
    let line = fields(&[
        ("goods", refer(flour)),
        ("qty", dec(10_000, 3)),
        ("price", dec(200, 2)),
    ]);
    engine
        .save(
            "receipt",
            &document(main, Some(mill), line),
            PostAction::Post,
        )
        .unwrap();
    (main, flour)
}

fn issue(warehouse: i64, goods: i64, qty: i64) -> ObjectData {
    let line = fields(&[("goods", refer(goods)), ("qty", dec(qty, 3))]);
    document(warehouse, None, line)
}

#[test]
fn on_post_reads_base_with_ctx_query() {
    let base = Base::new("ctx-query", ISSUE);
    let engine = base.create(model(), None);
    let (main, flour) = stocked(&*engine);
    let id = engine
        .save("issue", &issue(main, flour, 3_000), PostAction::Post)
        .unwrap()
        .id;
    // Второе проведение: ctx от первого уже не читает базу.
    engine.post("issue", id).unwrap();
    let found = engine
        .query(
            "SELECT SUM(qty) AS qty FROM stock WHERE recorder = :doc",
            &fields(&[("doc", Value::Text("issue".into()))]),
        )
        .unwrap();
    assert_eq!(found.rows, [vec![dec(3_000, 3)]]);
}

#[test]
fn bad_query_is_script_error() {
    let script = "def on_post(doc, ctx):\n    ctx.query(\"SELECT * FROM goods\")\n";
    let base = Base::new("ctx-query-bad", script);
    let engine = base.create(model(), None);
    let (main, flour) = stocked(&*engine);
    let err = engine
        .save("issue", &issue(main, flour, 1_000), PostAction::Post)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert!(
        err.message.contains("QueryError") && err.message.contains("SELECT *"),
        "{}",
        err.message
    );
    assert!(err.details.contains("on_post"), "{}", err.details);
}

#[test]
fn query_in_rule_has_rights_of_who_posts() {
    let mut model = model();
    let read = ostov_model::Rights {
        read: true,
        ..ostov_model::Rights::default()
    };
    let work = ostov_model::Rights {
        read: true,
        create: true,
        update: true,
        post: true,
        ..ostov_model::Rights::default()
    };
    model.roles.push(ostov_model::RoleDef {
        name: "picker".into(),
        title: "Сборщик".into(),
        objects: vec![
            ("goods".into(), read),
            ("warehouses".into(), read),
            ("issue".into(), work),
        ],
        system: ostov_model::SystemRights::default(),
    });
    let script = "def on_post(doc, ctx):\n    ctx.query(\"SELECT number FROM receipt\")\n";
    let base = Base::new("ctx-query-rights", script);
    let admin = NewAdmin {
        login: "boss".into(),
        password: "admin-password-1".into(),
    };
    let engine = base.create(model, Some(&admin));
    let (main, flour) = stocked(&*engine);
    engine
        .admin_add_user(&NewUser {
            login: "keeper".into(),
            display_name: "Кладовщик".into(),
            role: "picker".into(),
            password: "keeper-password-1".into(),
        })
        .unwrap();
    engine.logout().unwrap();
    engine.login("keeper", "keeper-password-1").unwrap();
    let err = engine
        .save("issue", &issue(main, flour, 1_000), PostAction::Post)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Validation);
    assert_eq!(err.message, "Нет права на просмотр «Поступление»");
}
