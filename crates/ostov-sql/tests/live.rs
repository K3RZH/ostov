//! `Prepared` на живой базе «Склада»: SQLite всегда, PostgreSQL с `OSTOV_PG_URL`.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use common::{Data, SqliteBase, run, text};
use ostov_store::{Driver, Value};

fn int(v: i64) -> Value {
    Value::Int(v)
}

fn goods_by_name(db: &mut dyn Driver, d: &Data) {
    let rows = run(
        db,
        "SELECT id, name FROM goods WHERE name = :name",
        &[("name", text("Мука"))],
    );
    assert_eq!(rows, vec![vec![int(d.flour), text("Мука")]]);

    // Без учёта регистра, кириллица: LOWER идёт через диалект.
    let rows = run(
        db,
        "SELECT name FROM goods WHERE LOWER(name) LIKE LOWER(:p) ORDER BY name",
        &[("p", text("%АХ%"))],
    );
    assert_eq!(rows, vec![vec![text("Сахар")]]);
}

fn receipts_by_supplier(db: &mut dyn Driver, _: &Data) {
    let rows = run(
        db,
        "SELECT s.name AS supplier, SUM(r.amount) AS total, COUNT(*) AS docs \
         FROM receipt AS r INNER JOIN suppliers AS s ON s.id = r.supplier \
         WHERE r.posted = 1 GROUP BY s.name ORDER BY total DESC",
        &[],
    );
    // Суммы в копейках: 70 + 30 и 20.
    assert_eq!(
        rows,
        vec![
            vec![text("Мельница"), int(10_000), int(2)],
            vec![text("Бакалея"), int(2_000), int(1)],
        ]
    );
}

fn lines_with_document(db: &mut dyn Driver, d: &Data) {
    let rows = run(
        db,
        "SELECT r.number, l.line_no, g.name, l.qty, l.amount \
         FROM receipt r JOIN receipt.lines l ON l.doc_id = r.id \
         LEFT JOIN goods g ON g.id = l.goods \
         WHERE r.id = :id ORDER BY l.line_no",
        &[("id", int(d.r1))],
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][2..], [text("Мука"), int(2000), int(2000)]);
    assert_eq!(rows[1][2..], [text("Сахар"), int(1000), int(5000)]);
    assert_eq!(rows[0][0], rows[1][0]);
    assert!(rows[0][1] != rows[1][1]);
}

fn movements_by_warehouse(db: &mut dyn Driver, d: &Data) {
    let rows = run(
        db,
        "SELECT g.name, m.kind, m.qty, m.recorder, m.period \
         FROM stock m JOIN goods g ON g.id = m.goods \
         WHERE m.warehouse = :w ORDER BY m.period, m.id",
        &[("w", int(d.main))],
    );
    assert_eq!(
        rows,
        vec![
            vec![
                text("Мука"),
                int(1),
                int(2000),
                text("receipt"),
                text("2026-11-01")
            ],
            vec![
                text("Сахар"),
                int(1),
                int(1000),
                text("receipt"),
                text("2026-11-01")
            ],
            vec![
                text("Мука"),
                int(1),
                int(3000),
                text("receipt"),
                text("2026-11-02")
            ],
            vec![
                text("Мука"),
                int(-1),
                int(1500),
                text("issue"),
                text("2026-11-04")
            ],
        ]
    );

    // Остаток по складу и товару из движений: приход минус расход.
    let rows = run(
        db,
        "SELECT SUM(kind * qty) AS rest FROM stock \
         WHERE warehouse = :w AND goods = :g AND period <= :day",
        &[
            ("w", int(d.main)),
            ("g", int(d.flour)),
            ("day", text("2026-11-30")),
        ],
    );
    assert_eq!(rows, vec![vec![int(3500)]]);
}

fn misc(db: &mut dyn Driver, d: &Data) {
    // Строковый литерал уходит параметром, IN и BETWEEN, COALESCE, LIMIT.
    let rows = run(
        db,
        "SELECT number, COALESCE(amount, 0) FROM receipt \
         WHERE supplier IN (:a, :b) AND date BETWEEN '2026-11-02' AND '2026-11-03' \
         ORDER BY date LIMIT 1",
        &[("a", int(d.mill)), ("b", int(d.bakery))],
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], int(3000));
    let rows = run(
        db,
        "SELECT MIN(date), MAX(date) FROM receipt WHERE deleted = 0",
        &[],
    );
    assert_eq!(rows, vec![vec![text("2026-11-01"), text("2026-11-03")]]);
    let rows = run(
        db,
        "SELECT id FROM receipt WHERE id = :x OR id = :x",
        &[("x", int(d.r3))],
    );
    assert_eq!(rows, vec![vec![int(d.r3)]]);
    let _ = (d.r2, d.sugar, d.spare);
}

fn all(db: &mut dyn Driver, d: &Data) {
    goods_by_name(db, d);
    receipts_by_supplier(db, d);
    lines_with_document(db, d);
    movements_by_warehouse(db, d);
    misc(db, d);
}

#[test]
fn warehouse_on_sqlite() {
    let (base, data) = SqliteBase::new("live");
    all(&mut base.open(), &data);
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn warehouse_on_postgres() {
    let Some((mut db, data)) = common::postgres("ostov_t_sql_live") else {
        return;
    };
    all(&mut db, &data);
}
