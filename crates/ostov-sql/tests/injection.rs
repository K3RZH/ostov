//! Корпус инъекций (НТ-Б1).
//!
//! Откуда payload'ы: OWASP Web Security Testing Guide, раздел «Testing for SQL Injection»
//! (WSTG-INPV-05, подразделы про SQLite и PostgreSQL), OWASP SQL Injection Prevention Cheat
//! Sheet, PayloadsAllTheThings «SQL Injection» (SQLite Injection, PostgreSQL Injection) и типы
//! проб sqlmap (boolean, time-based, UNION, stacked queries). Плюс свои пробы под Ostov:
//! физические имена таблиц, служебные таблицы `ostov_*`, внутренняя функция `ostov_lower`.
//!
//! Три группы:
//! - в параметрах: остаются данными, ничего не находят и ничего не ломают;
//! - в тексте запроса: отклоняются;
//! - в именах таблиц и колонок: отклоняются.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;

use common::{SqliteBase, run, text};
use ostov_store::Driver;
use ostov_store::drivers::postgres::PostgresDialect;
use ostov_store::drivers::sqlite::SqliteDialect;

const IN_PARAMS: &[&str] = &[
    "' OR '1'='1",
    "' OR 1=1 --",
    "' OR 1=1 /*",
    "\" OR \"\"=\"",
    "') OR ('1'='1",
    "admin'--",
    "Мука' --",
    "Мука'/*",
    "Мука' AND '1'='1",
    "'; DROP TABLE goods; --",
    "'; DELETE FROM goods; --",
    "'; UPDATE goods SET name = 'x'; --",
    "' UNION SELECT name FROM suppliers --",
    "' UNION ALL SELECT login FROM ostov_users --",
    "' AND 1=0 UNION ALL SELECT sqlite_version() --",
    "'||(SELECT name FROM suppliers LIMIT 1)||'",
    "1; ATTACH DATABASE 'evil.db' AS evil; --",
    "'; PRAGMA writable_schema=1; --",
    "' OR load_extension('evil') --",
    "' OR randomblob(1000000000) --",
    "'; SELECT pg_sleep(5); --",
    "' OR pg_sleep(5) IS NULL --",
    "'; COPY goods TO PROGRAM 'calc'; --",
    "$$; DROP TABLE goods; $$",
    "\\'; DROP TABLE goods; --",
    "1' ORDER BY 1 --",
    "%' OR '%'='",
    "%",
    "_",
    "Мука\0' OR 1=1",
    "\u{2019} OR 1=1 --",
    "\u{ff07} OR 1=1 --",
];

const IN_TEXT: &[&str] = &[
    "SELECT name FROM goods; DROP TABLE goods",
    "SELECT name FROM goods WHERE name = 'x'; DELETE FROM goods",
    "SELECT name FROM goods LIMIT 1; SELECT 1",
    "BEGIN; DROP TABLE goods; COMMIT",
    "SELECT name FROM goods WHERE name = 'x' /* */ UNION SELECT name FROM suppliers",
    "SELECT name FROM goods WHERE name = 'x' OR 1 = 1 UNION SELECT name FROM suppliers",
    "SELECT name FROM goods UNION ALL SELECT name FROM suppliers",
    "SELECT name FROM goods WHERE id = (SELECT MAX(id) FROM goods)",
    "SELECT name FROM goods WHERE id IN (SELECT id FROM goods)",
    "SELECT name FROM goods WHERE EXISTS (SELECT id FROM suppliers)",
    "SELECT name FROM (SELECT name FROM goods) g",
    "WITH t AS (SELECT name FROM goods) SELECT name FROM t",
    "WITH RECURSIVE t(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM t) SELECT n FROM t",
    "PRAGMA table_info(goods)",
    "PRAGMA writable_schema = 1",
    "ATTACH DATABASE 'evil.db' AS evil",
    "DETACH DATABASE evil",
    "VACUUM INTO 'copy.db'",
    "SELECT load_extension('evil.dll')",
    "SELECT name FROM goods WHERE load_extension('x') IS NULL",
    "SELECT sqlite_version()",
    "SELECT name FROM goods WHERE randomblob(1000000000) IS NOT NULL",
    "SELECT pg_sleep(10)",
    "SELECT pg_read_file('/etc/passwd')",
    "SELECT set_config('search_path', 'evil', false)",
    "SELECT current_setting('data_directory')",
    "COPY goods TO PROGRAM 'calc'",
    "CREATE TABLE evil (x TEXT)",
    "DROP TABLE goods",
    "ALTER TABLE goods RENAME TO evil",
    "INSERT INTO goods (name, version, deleted) VALUES ('x', 1, 0)",
    "UPDATE ostov_users SET hash = 'x'",
    "DELETE FROM goods WHERE 1 = 1",
    "EXPLAIN SELECT name FROM goods",
    "SELECT name FROM goods FOR UPDATE",
    "SELECT name FROM goods WHERE name = char(65)",
    "SELECT name FROM goods WHERE name = 'a' || 'b'",
    "SELECT CAST(name AS BLOB) FROM goods",
    "SELECT name FROM goods WHERE name = $$x$$",
    "SELECT name FROM goods WHERE name = E'\\x27'",
    "SELECT name FROM goods WHERE name = X'41'",
    "SELECT name FROM goods WHERE name LIKE 'a' ESCAPE '!'",
    "SELECT name FROM goods WHERE deleted = TRUE",
    "SELECT * FROM goods",
];

const IN_NAMES: &[&str] = &[
    "SELECT \"name\" FROM goods",
    "SELECT name FROM \"goods\"",
    "SELECT name FROM \"goods\"\"; DROP TABLE goods; --\"",
    "SELECT `name` FROM goods",
    "SELECT [name] FROM goods",
    "SELECT g.\"name\" FROM goods g",
    "SELECT name AS \"x\"\"; DROP TABLE goods; --\" FROM goods",
    "SELECT name FROM goods AS \"g\"",
    "SELECT qty FROM receipt__lines",
    "SELECT qty FROM stock__totals",
    "SELECT login FROM ostov_users",
    "SELECT action FROM ostov_journal",
    "SELECT name FROM sqlite_master",
    "SELECT tablename FROM pg_catalog.pg_tables",
    "SELECT name FROM main.goods",
    "SELECT name FROM public.goods",
    "SELECT hash FROM goods",
    "SELECT goods.hash FROM goods",
    "SELECT name FROM goods WHERE ostov_lower(name) = 'x'",
];

fn rejected_on_both(query: &str) {
    let config = common::model();
    let none = BTreeMap::new();
    for dialect in [
        &SqliteDialect as &dyn ostov_store::Dialect,
        &PostgresDialect,
    ] {
        if let Ok(p) = ostov_sql::prepare(query, &none, &config, &|_| true, dialect) {
            panic!("пропущен запрос: {query}\n→ {}", p.sql);
        }
    }
}

#[test]
fn corpus_has_at_least_50_probes() {
    let total = IN_PARAMS.len() + IN_TEXT.len() + IN_NAMES.len();
    assert!(total >= 50, "{total}");
}

#[test]
fn injections_in_query_text_are_rejected() {
    for q in IN_TEXT {
        rejected_on_both(q);
    }
}

#[test]
fn injections_in_names_are_rejected() {
    for q in IN_NAMES {
        rejected_on_both(q);
    }
}

#[test]
fn comments_do_not_reach_output() {
    let p = ostov_sql::prepare(
        "SELECT name FROM goods -- ; DROP TABLE goods\nWHERE name = :n /* ; DELETE FROM goods */",
        &common::params(&[("n", text("x"))]),
        &common::model(),
        &|_| true,
        &SqliteDialect,
    )
    .unwrap();
    assert!(!p.sql.contains("DROP") && !p.sql.contains("DELETE") && !p.sql.contains("--"));
}

fn params_stay_data(db: &mut dyn Driver) {
    for payload in IN_PARAMS {
        let rows = run(
            db,
            "SELECT id, name FROM goods WHERE name = :name",
            &[("name", text(payload))],
        );
        assert!(rows.is_empty(), "{payload}: {rows:?}");
        let rows = run(
            db,
            "SELECT g.name FROM goods g JOIN suppliers s ON s.id = g.id \
             WHERE LOWER(g.name) = LOWER(:p) OR s.name = :p",
            &[("p", text(payload))],
        );
        assert!(rows.is_empty(), "{payload}: {rows:?}");
    }
    // Ничего не сломано и не изменено.
    let rows = run(db, "SELECT name FROM goods ORDER BY name", &[]);
    assert_eq!(rows, vec![vec![text("Мука")], vec![text("Сахар")]]);
    let rows = run(db, "SELECT COUNT(*) FROM suppliers", &[]);
    assert_eq!(rows, vec![vec![ostov_store::Value::Int(2)]]);
}

#[test]
fn injections_in_params_stay_data_on_sqlite() {
    let (base, _) = SqliteBase::new("injection");
    params_stay_data(&mut base.open());
}

#[test]
#[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
fn injections_in_params_stay_data_on_postgres() {
    let Some((mut db, _)) = common::postgres("ostov_t_sql_injection") else {
        return;
    };
    params_stay_data(&mut db);
}
