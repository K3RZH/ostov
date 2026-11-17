//! Ограничения по записям на живой базе «Склада»: SQLite всегда, PostgreSQL с `OSTOV_PG_URL`.
//! Роль видит поступления, их строки и остатки только склада «Запасной».

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use common::{Data, SqliteBase, model, params};
use ostov_sql::{Param, prepare};
use ostov_store::{Driver, Row, Value};

type Limits = Vec<(String, Vec<Param>)>;

fn int(v: i64) -> Value {
    Value::Int(v)
}

fn run(
    db: &mut dyn Driver,
    within: &dyn Fn(&str) -> Option<Limits>,
    query: &str,
    p: &[(&str, Value)],
) -> Vec<Row> {
    let prepared = prepare(query, &params(p), &model(), &|_| true, within, db.dialect())
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    db.query(&prepared.sql, &prepared.params)
        .unwrap_or_else(|e| panic!("{query}\n{}\n{e:?}", prepared.sql))
}

fn checks(db: &mut dyn Driver, d: &Data) {
    let spare = d.spare;
    let only_spare = move |object: &str| -> Option<Limits> {
        matches!(object, "receipt" | "stock")
            .then(|| vec![("warehouse".into(), vec![Param::Int(spare)])])
    };
    let rows = run(db, &only_spare, "SELECT id FROM receipt ORDER BY id", &[]);
    assert_eq!(rows, vec![vec![int(d.r3)]]);

    // Табличная часть — по шапке документа.
    let rows = run(
        db,
        &only_spare,
        "SELECT COUNT(*) AS n FROM receipt.lines",
        &[],
    );
    assert_eq!(rows, vec![vec![int(1)]]);
    let rows = run(
        db,
        &only_spare,
        "SELECT r.id, l.qty FROM receipt r INNER JOIN receipt.lines l ON l.doc_id = r.id",
        &[],
    );
    assert_eq!(rows, vec![vec![int(d.r3), int(5000)]]);

    // Регистр с GROUP BY.
    let rows = run(
        db,
        &only_spare,
        "SELECT warehouse, SUM(qty) AS total FROM stock GROUP BY warehouse",
        &[],
    );
    assert_eq!(rows, vec![vec![int(spare), int(5000)]]);

    // LEFT JOIN: товары все, движения — только видимые.
    let rows = run(
        db,
        &only_spare,
        "SELECT g.name, m.qty FROM goods g LEFT JOIN stock m ON m.goods = g.id ORDER BY g.name",
        &[],
    );
    assert_eq!(
        rows,
        vec![
            vec![Value::Text("Мука".into()), Value::Null],
            vec![Value::Text("Сахар".into()), int(5000)],
        ]
    );

    // Параметр пользователя не открывает чужое.
    let by_id = "SELECT id FROM receipt WHERE id = :id";
    assert!(run(db, &only_spare, by_id, &[("id", int(d.r1))]).is_empty());
    assert_eq!(
        run(db, &only_spare, by_id, &[("id", int(d.r3))]),
        vec![vec![int(d.r3)]]
    );

    // Пустой список значений — не видно ничего.
    let nothing = |object: &str| -> Option<Limits> {
        (object == "stock").then(|| vec![("warehouse".into(), vec![])])
    };
    let rows = run(db, &nothing, "SELECT COUNT(*) AS n FROM stock", &[]);
    assert_eq!(rows, vec![vec![int(0)]]);
    let _ = (d.main, d.r2);
}

#[test]
fn within_on_sqlite() {
    let (base, data) = SqliteBase::new("within");
    checks(&mut base.open(), &data);
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn within_on_postgres() {
    let Some((mut db, data)) = common::postgres("ostov_t_sql_within") else {
        return;
    };
    checks(&mut db, &data);
}
