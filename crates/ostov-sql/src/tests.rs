#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;

use ostov_model::FieldKind;
use ostov_store::Value;
use ostov_store::drivers::postgres::PostgresDialect;
use ostov_store::drivers::sqlite::SqliteDialect;

use super::*;

fn config() -> ostov_model::Config {
    ostov_meta::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"))
        .unwrap()
}

fn params(pairs: &[(&str, Value)]) -> BTreeMap<String, Param> {
    pairs
        .iter()
        .map(|(k, v)| {
            let p = match v {
                Value::Null => Param::Null,
                Value::Int(n) => Param::Int(*n),
                Value::Text(t) => Param::Text(t.clone()),
                Value::Bytes(_) => panic!("байты параметром не передаются"),
            };
            ((*k).to_owned(), p)
        })
        .collect()
}

fn all(_: &str) -> bool {
    true
}

fn sqlite(query: &str, p: &[(&str, Value)]) -> Result<Prepared, SqlError> {
    prepare(query, &params(p), &config(), &all, &SqliteDialect)
}

fn pg(query: &str, p: &[(&str, Value)]) -> Result<Prepared, SqlError> {
    prepare(query, &params(p), &config(), &all, &PostgresDialect)
}

fn err(query: &str) -> SqlError {
    sqlite(query, &[]).unwrap_err()
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

#[test]
fn simple_select_on_both_dialects() {
    let q = "SELECT name, unit FROM goods WHERE name = :name ORDER BY name";
    let p = [("name", text("Мука"))];
    let s = sqlite(q, &p).unwrap();
    assert_eq!(
        s.sql,
        "SELECT \"t1\".\"name\" AS \"name\", \"t1\".\"unit\" AS \"unit\" FROM \"goods\" AS \"t1\" \
         WHERE (\"t1\".\"name\" = ?1) ORDER BY \"t1\".\"name\" LIMIT 1000"
    );
    assert_eq!(s.params, vec![text("Мука")]);
    assert_eq!(
        s.columns,
        vec![
            Column {
                name: "name".into(),
                kind: Some(FieldKind::Text)
            },
            Column {
                name: "unit".into(),
                kind: Some(FieldKind::Text)
            },
        ]
    );
    let p = pg(q, &p).unwrap();
    assert!(p.sql.contains("= $1)"), "{}", p.sql);
    assert_eq!(p.sql.replace("$1", "?1"), s.sql);
}

#[test]
fn join_group_by_and_money_kind() {
    let q = "SELECT s.name AS supplier, SUM(r.amount) AS total, COUNT(*) \
             FROM receipt AS r INNER JOIN suppliers AS s ON s.id = r.supplier \
             WHERE r.posted = 1 AND r.deleted = 0 \
             GROUP BY s.name HAVING SUM(r.amount) > 0 ORDER BY total DESC LIMIT 10";
    let s = sqlite(q, &[]).unwrap();
    assert_eq!(
        s.sql,
        "SELECT \"t2\".\"name\" AS \"supplier\", CAST(SUM(\"t1\".\"amount\") AS BIGINT) AS \"total\", COUNT(*) AS \"count\" \
         FROM \"receipt\" AS \"t1\" INNER JOIN \"suppliers\" AS \"t2\" ON (\"t2\".\"id\" = \"t1\".\"supplier\") \
         WHERE ((\"t1\".\"posted\" = 1) AND (\"t1\".\"deleted\" = 0)) \
         GROUP BY \"t2\".\"name\" HAVING (CAST(SUM(\"t1\".\"amount\") AS BIGINT) > 0) \
         ORDER BY \"total\" DESC LIMIT 10"
    );
    let kinds: Vec<_> = s
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("supplier", Some(FieldKind::Text)),
            ("total", Some(FieldKind::Decimal { scale: 2 })),
            ("count", None),
        ]
    );
}

#[test]
fn table_part_and_register_names() {
    let s = sqlite(
        "SELECT r.number, l.line_no, l.qty FROM receipt r \
         LEFT JOIN receipt.lines l ON l.doc_id = r.id",
        &[],
    )
    .unwrap();
    assert!(
        s.sql
            .contains("FROM \"receipt\" AS \"t1\" LEFT JOIN \"receipt__lines\" AS \"t2\""),
        "{}",
        s.sql
    );
    assert_eq!(s.columns[2].kind, Some(FieldKind::Decimal { scale: 3 }));

    // Без псевдонима табличную часть называют полным именем.
    let s = sqlite("SELECT receipt.lines.qty FROM receipt.lines", &[]).unwrap();
    assert_eq!(
        s.sql,
        "SELECT \"t1\".\"qty\" AS \"qty\" FROM \"receipt__lines\" AS \"t1\" LIMIT 1000"
    );

    let s = sqlite(
        "SELECT goods, kind, period, recorder, recorder_id, qty FROM stock WHERE warehouse = :w",
        &[("w", Value::Int(1))],
    )
    .unwrap();
    assert!(s.sql.contains("FROM \"stock\" AS \"t1\""), "{}", s.sql);
    assert_eq!(s.columns[2].kind, Some(FieldKind::Date));
    assert_eq!(s.columns[3].kind, Some(FieldKind::Text));
}

#[test]
fn field_named_date_and_keywords_in_names() {
    let s = sqlite(
        "SELECT number, date, posted FROM receipt WHERE date BETWEEN :a AND :b ORDER BY date",
        &[("a", text("2026-01-01")), ("b", text("2026-12-31"))],
    )
    .unwrap();
    assert!(
        s.sql
            .contains("(\"t1\".\"date\" BETWEEN ?1 AND ?2) ORDER BY \"t1\".\"date\""),
        "{}",
        s.sql
    );
}

#[test]
fn lower_goes_through_dialect() {
    let q = "SELECT name FROM goods WHERE LOWER(name) LIKE LOWER(:p)";
    let p = [("p", text("%МУК%"))];
    assert!(
        sqlite(q, &p)
            .unwrap()
            .sql
            .contains("(ostov_lower(\"t1\".\"name\") LIKE ostov_lower(?1))")
    );
    assert!(
        pg(q, &p)
            .unwrap()
            .sql
            .contains("(LOWER(\"t1\".\"name\") LIKE LOWER($1))")
    );
}

#[test]
fn operators_in_between_null_coalesce() {
    let s = sqlite(
        "SELECT COALESCE(price, 0) AS price, qty * price AS sum, qty - qty \
         FROM receipt.lines \
         WHERE goods IN (:a, :b) AND NOT price IS NULL AND amount IS NOT NULL \
         AND line_no NOT IN (1, 2) AND -qty < 0 AND qty <> 0 AND (line_no >= 1 OR line_no <= 2)",
        &[("a", Value::Int(1)), ("b", Value::Int(2))],
    )
    .unwrap();
    assert!(
        s.sql.contains("(\"t1\".\"goods\" IN (?1, ?2))"),
        "{}",
        s.sql
    );
    assert!(
        s.sql.contains("(NOT (\"t1\".\"price\" IS NULL))"),
        "{}",
        s.sql
    );
    assert!(s.sql.contains("NOT IN (1, 2)"), "{}", s.sql);
    let kinds: Vec<_> = s.columns.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Some(FieldKind::Decimal { scale: 2 }),
            // 0,001 × 0,01: в единицах масштабы складываются.
            Some(FieldKind::Decimal { scale: 5 }),
            Some(FieldKind::Decimal { scale: 3 }),
        ]
    );
}

#[test]
fn string_literal_becomes_parameter() {
    let s = sqlite(
        "SELECT name FROM goods WHERE unit = 'кг' AND name = :n",
        &[("n", text("x"))],
    )
    .unwrap();
    assert!(
        s.sql.contains("= ?1) AND (\"t1\".\"name\" = ?2)"),
        "{}",
        s.sql
    );
    assert_eq!(s.params, vec![text("кг"), text("x")]);
}

#[test]
fn keywords_and_names_ignore_case() {
    let s = sqlite(
        "select Name from GOODS where NAME = :n",
        &[("n", text("x"))],
    )
    .unwrap();
    assert!(
        s.sql
            .starts_with("SELECT \"t1\".\"name\" AS \"name\" FROM \"goods\"")
    );
}

#[test]
fn limit_is_capped() {
    for (q, want) in [
        ("SELECT name FROM goods", "LIMIT 1000"),
        ("SELECT name FROM goods LIMIT 5", "LIMIT 5"),
        ("SELECT name FROM goods LIMIT 0", "LIMIT 0"),
        ("SELECT name FROM goods LIMIT 1000", "LIMIT 1000"),
        ("SELECT name FROM goods LIMIT 5000", "LIMIT 1000"),
        ("SELECT name FROM goods LIMIT 99999999999999999999", "LIMIT"),
    ] {
        match sqlite(q, &[]) {
            Ok(p) => assert!(p.sql.ends_with(want), "{q}: {}", p.sql),
            Err(e) => assert_eq!(e, SqlError::BadLimit, "{q}"),
        }
    }
    assert_eq!(err("SELECT name FROM goods LIMIT :n"), SqlError::BadLimit);
    assert_eq!(err("SELECT name FROM goods LIMIT -1"), SqlError::BadLimit);
    assert!(matches!(
        err("SELECT name FROM goods LIMIT 5 OFFSET 10"),
        SqlError::Forbidden { .. }
    ));
}

#[test]
fn parameters_missing_extra_repeated() {
    assert_eq!(
        err("SELECT name FROM goods WHERE name = :n"),
        SqlError::MissingParam { name: "n".into() }
    );
    assert_eq!(
        sqlite("SELECT name FROM goods", &[("n", text("x"))]).unwrap_err(),
        SqlError::ExtraParam { name: "n".into() }
    );
    let s = sqlite(
        "SELECT name FROM goods WHERE name = :n OR unit = :n",
        &[("n", text("x"))],
    )
    .unwrap();
    assert!(
        s.sql.contains("= ?1) OR (\"t1\".\"unit\" = ?2)"),
        "{}",
        s.sql
    );
    assert_eq!(s.params, vec![text("x"), text("x")]);
    let p = pg(
        "SELECT name FROM goods WHERE name = :n OR unit = :n",
        &[("n", text("x"))],
    )
    .unwrap();
    assert!(p.sql.contains("$1") && p.sql.contains("$2"));

    for bad in ["?", "$1", "@n", ":1"] {
        let q = format!("SELECT name FROM goods WHERE name = {bad}");
        // `?` и `:1` — плейсхолдеры не того вида, `@n` разборщик видит как имя.
        assert!(sqlite(&q, &[]).is_err(), "{q}");
    }
}

#[test]
fn rights_are_checked() {
    let config = config();
    let no_receipt = |name: &str| name != "receipt";
    let run = |q: &str| prepare(q, &BTreeMap::new(), &config, &no_receipt, &SqliteDialect);
    let want = SqlError::NoRight {
        title: "Поступление".into(),
    };
    assert_eq!(run("SELECT number FROM receipt").unwrap_err(), want);
    // Табличная часть — по праву документа.
    assert_eq!(run("SELECT qty FROM receipt.lines").unwrap_err(), want);
    assert_eq!(
        run("SELECT g.name FROM goods g JOIN receipt r ON r.id = g.id").unwrap_err(),
        want
    );
    // Регистры прав на чтение пока не имеют.
    assert!(run("SELECT qty FROM stock").is_ok());
    assert!(run("SELECT name FROM goods").is_ok());
    assert_eq!(
        run("SELECT number FROM receipt").unwrap_err().to_string(),
        "Нет права на просмотр «Поступление»"
    );
}

#[test]
fn unknown_names() {
    assert_eq!(
        err("SELECT colour FROM goods"),
        SqlError::NoField {
            field: "colour".into(),
            table: "Товары".into()
        }
    );
    assert_eq!(
        err("SELECT colour FROM goods").to_string(),
        "Нет поля «colour» у «Товары»"
    );
    assert_eq!(
        err("SELECT name FROM customers"),
        SqlError::NoTable {
            name: "customers".into()
        }
    );
    assert_eq!(
        err("SELECT qty FROM receipt.rows"),
        SqlError::NoTable {
            name: "receipt.rows".into()
        }
    );
    assert_eq!(
        err("SELECT x.name FROM goods"),
        SqlError::NoSource { name: "x".into() }
    );
    assert_eq!(
        err("SELECT g.colour FROM goods g"),
        SqlError::NoField {
            field: "colour".into(),
            table: "Товары".into()
        }
    );
    // Физическое имя таблицы — не имя метаданных.
    assert!(matches!(
        err("SELECT qty FROM receipt__lines"),
        SqlError::NoTable { .. }
    ));
    // Системные колонки чужих видов таблиц недоступны.
    assert!(matches!(
        err("SELECT posted FROM goods"),
        SqlError::NoField { .. }
    ));
    assert!(matches!(
        err("SELECT name FROM goods g JOIN suppliers s ON s.id = g.id"),
        SqlError::Ambiguous { .. }
    ));
    assert!(matches!(
        err("SELECT goods.name FROM goods JOIN goods ON goods.id = goods.id"),
        SqlError::DuplicateSource { .. }
    ));
    assert!(matches!(
        err("SELECT \"name\" FROM goods"),
        SqlError::QuotedName { .. }
    ));
}

#[test]
fn forbidden_constructions() {
    let cases: &[(&str, Check)] = &[
        ("INSERT INTO goods (name) VALUES ('x')", |e| {
            *e == SqlError::OnlySelect
        }),
        ("UPDATE goods SET name = 'x'", |e| {
            *e == SqlError::OnlySelect
        }),
        ("DELETE FROM goods", |e| *e == SqlError::OnlySelect),
        ("DROP TABLE goods", |e| *e == SqlError::OnlySelect),
        ("CREATE TABLE x (a INT)", |e| *e == SqlError::OnlySelect),
        ("ALTER TABLE goods ADD COLUMN x INT", |e| {
            *e == SqlError::OnlySelect
        }),
        ("PRAGMA table_info(goods)", not_select),
        ("PRAGMA writable_schema = 1", not_select),
        ("ATTACH DATABASE 'x.db' AS x", not_select),
        ("SELECT name FROM goods; SELECT name FROM goods", |e| {
            *e == SqlError::NotOneStatement { count: 2 }
        }),
        ("", |e| *e == SqlError::NotOneStatement { count: 0 }),
        ("SELECT * FROM goods", forbidden),
        ("SELECT g.* FROM goods g", forbidden),
        ("SELECT load_extension('x')", function),
        ("SELECT AVG(qty) FROM stock", function),
        (
            "SELECT name FROM goods WHERE sqlite_version() = '1'",
            function,
        ),
        ("SELECT pg_sleep(10)", function),
        ("SELECT UPPER(name) FROM goods", function),
        (
            "WITH x AS (SELECT name FROM goods) SELECT name FROM x",
            forbidden,
        ),
        (
            "WITH RECURSIVE x(n) AS (SELECT 1) SELECT n FROM x",
            forbidden,
        ),
        (
            "SELECT name FROM goods UNION SELECT name FROM suppliers",
            forbidden,
        ),
        ("SELECT name FROM (SELECT name FROM goods) AS g", forbidden),
        (
            "SELECT name FROM goods WHERE id IN (SELECT goods FROM stock)",
            forbidden,
        ),
        (
            "SELECT name FROM goods WHERE EXISTS (SELECT goods FROM stock)",
            forbidden,
        ),
        ("SELECT (SELECT 1) FROM goods", forbidden),
        ("SELECT DISTINCT name FROM goods", forbidden),
        ("SELECT name FROM goods, suppliers", forbidden),
        (
            "SELECT g.name FROM goods g RIGHT JOIN suppliers s ON s.id = g.id",
            forbidden,
        ),
        (
            "SELECT g.name FROM goods g CROSS JOIN suppliers s",
            forbidden,
        ),
        (
            "SELECT g.name FROM goods g JOIN suppliers s USING (id)",
            forbidden,
        ),
        (
            "SELECT g.name FROM goods g NATURAL JOIN suppliers s",
            forbidden,
        ),
        ("SELECT name FROM goods WHERE name || 'x' = 'y'", forbidden),
        ("SELECT qty / 2 FROM stock", forbidden),
        ("SELECT CASE WHEN qty > 0 THEN 1 END FROM stock", forbidden),
        ("SELECT CAST(qty AS TEXT) FROM stock", forbidden),
        ("SELECT name FROM goods WHERE deleted = TRUE", forbidden),
        ("SELECT 1.5", |e| matches!(e, SqlError::BadNumber { .. })),
        ("SELECT COUNT(DISTINCT name) FROM goods", forbidden),
        ("SELECT COUNT(*) OVER () FROM goods", forbidden),
        (
            "SELECT name FROM goods ORDER BY name NULLS FIRST",
            forbidden,
        ),
        ("SELECT name FROM goods FOR UPDATE", forbidden),
        ("SELECT name FROM goods WHERE name ILIKE 'x'", forbidden),
        ("SELECT price + qty FROM receipt.lines", |e| {
            matches!(e, SqlError::ScaleMismatch { left: 2, right: 3 })
        }),
    ];
    for (q, check) in cases {
        let e = err(q);
        assert!(check(&e), "{q}: {e:?}");
        assert!(!e.to_string().is_empty());
    }
}

type Check = fn(&SqlError) -> bool;

/// Не SELECT: отклонён на разборе или после него.
fn not_select(e: &SqlError) -> bool {
    matches!(e, SqlError::OnlySelect | SqlError::Parse(_))
}

fn forbidden(e: &SqlError) -> bool {
    matches!(e, SqlError::Forbidden { .. })
}

fn function(e: &SqlError) -> bool {
    matches!(e, SqlError::Function { .. })
}

#[test]
fn errors_are_russian_and_point_to_place() {
    assert_eq!(
        err("DELETE FROM goods").to_string(),
        "В запросе можно только SELECT"
    );
    let e = err("SELECT name\nFROM goods WHERE AVG(id) > 1").to_string();
    assert!(e.contains("«AVG»") && e.contains("строка 2"), "{e}");
    let e = err("SELECT * FROM goods").to_string();
    assert!(e.contains("SELECT *"), "{e}");
}

#[test]
fn deep_nesting_and_long_query_are_errors() {
    let deep = format!("SELECT {}1{} FROM goods", "(".repeat(500), ")".repeat(500));
    assert!(matches!(err(&deep), SqlError::Parse(_)));
    let long = format!(
        "SELECT name FROM goods WHERE name = '{}'",
        "x".repeat(MAX_QUERY_LEN)
    );
    assert!(matches!(err(&long), SqlError::TooLong { .. }));
}

#[test]
fn trailing_semicolon_and_comments_are_fine() {
    let s = sqlite("SELECT name -- комментарий\nFROM goods /* ещё */;", &[]).unwrap();
    assert_eq!(
        s.sql,
        "SELECT \"t1\".\"name\" AS \"name\" FROM \"goods\" AS \"t1\" LIMIT 1000"
    );
}

#[test]
fn money_and_integers_do_not_mix() {
    for q in [
        "SELECT price + 1 FROM receipt.lines",
        "SELECT name FROM goods g JOIN receipt.lines l ON l.goods = g.id WHERE l.price > 100",
        "SELECT qty FROM receipt.lines WHERE price + line_no > 0",
        "SELECT qty FROM receipt.lines WHERE price BETWEEN 1 AND 5",
        "SELECT qty FROM receipt.lines WHERE price IN (100, 200)",
    ] {
        assert!(
            matches!(err(q), SqlError::MoneyWithInt { scale: 2 }),
            "{q}: {:?}",
            err(q)
        );
    }
    // Ноль, параметр и умножение на целое — можно.
    assert!(
        sqlite(
            "SELECT qty * 2 AS twice FROM receipt.lines WHERE price > 0 AND price < :max",
            &[("max", Value::Int(10_000))]
        )
        .is_ok()
    );
}

fn with(query: &str, p: &[(&str, Param)]) -> Result<Prepared, SqlError> {
    let p = p
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    prepare(query, &p, &config(), &all, &SqliteDialect)
}

fn dec(units: i64, scale: u8) -> Param {
    Param::Decimal { units, scale }
}

#[test]
fn number_params_take_scale_of_money_field() {
    let q = "SELECT qty FROM receipt.lines WHERE price > :p";
    assert_eq!(
        with(q, &[("p", dec(25, 1))]).unwrap().params,
        [Value::Int(250)]
    );
    assert_eq!(
        with(q, &[("p", dec(2500, 3))]).unwrap().params,
        [Value::Int(250)]
    );
    assert_eq!(
        with(q, &[("p", Param::Int(100))]).unwrap().params,
        [Value::Int(10_000)]
    );
    let q = "SELECT qty FROM receipt.lines WHERE :p <= price";
    assert_eq!(
        with(q, &[("p", dec(1, 0))]).unwrap().params,
        [Value::Int(100)]
    );

    let q = "SELECT price FROM receipt.lines WHERE qty BETWEEN :a AND :b OR qty IN (:a, :b)";
    let s = with(q, &[("a", dec(15, 1)), ("b", Param::Int(2))]).unwrap();
    let (a, b) = (Value::Int(1500), Value::Int(2000));
    assert_eq!(s.params, [a.clone(), b.clone(), a, b]);

    // Целое поле и целый параметр как были.
    let q = "SELECT qty FROM receipt.lines WHERE line_no = :n";
    assert_eq!(
        with(q, &[("n", Param::Int(3))]).unwrap().params,
        [Value::Int(3)]
    );
    // Умножение не переводит: масштабы складываются.
    let q = "SELECT price * :k AS total FROM receipt.lines";
    let s = with(q, &[("k", dec(15, 1))]).unwrap();
    assert_eq!(s.params, [Value::Int(15)]);
    assert_eq!(s.columns[0].kind, Some(FieldKind::Decimal { scale: 3 }));
}

#[test]
fn number_params_that_do_not_fit_are_errors() {
    let q = "SELECT qty FROM receipt.lines WHERE price > :p";
    for p in [dec(1234, 3), Param::Int(i64::MAX)] {
        let e = with(q, &[("p", p)]).unwrap_err();
        assert_eq!(
            e,
            SqlError::ParamScale {
                name: "p".into(),
                scale: 2
            }
        );
        assert!(e.to_string().contains("«p»"), "{e}");
    }
    let q = "SELECT qty FROM receipt.lines WHERE line_no = :p";
    assert!(matches!(
        with(q, &[("p", dec(25, 1))]),
        Err(SqlError::MoneyWithInt { scale: 1 })
    ));
    assert!(matches!(
        err("SELECT qty FROM receipt.lines WHERE price = qty"),
        SqlError::ScaleMismatch { left: 2, right: 3 }
    ));
}

#[test]
fn sum_only_of_numbers_and_coalesce_scales_match() {
    assert!(matches!(
        err("SELECT SUM(name) FROM goods"),
        SqlError::SumNotNumber { .. }
    ));
    assert!(matches!(
        err("SELECT SUM(date) FROM receipt"),
        SqlError::SumNotNumber { .. }
    ));
    assert!(matches!(
        err("SELECT COALESCE(price, qty) FROM receipt.lines"),
        SqlError::ScaleMismatch { .. }
    ));
}

#[test]
fn result_columns_are_named_and_unique() {
    assert_eq!(
        err("SELECT SUM(qty), SUM(qty) FROM stock"),
        SqlError::DuplicateColumn { name: "sum".into() }
    );
    let s = sqlite("SELECT SUM(qty) AS total FROM stock ORDER BY total", &[]).unwrap();
    assert!(
        s.sql
            .ends_with("AS \"total\" FROM \"stock\" AS \"t1\" ORDER BY \"total\" LIMIT 1000")
    );
    // Псевдоним внутри выражения ORDER BY не подставляется.
    assert!(matches!(
        err("SELECT SUM(qty) AS total FROM stock ORDER BY total + 1"),
        SqlError::NoField { .. }
    ));
}

#[test]
fn same_string_literal_is_one_placeholder() {
    let s = sqlite(
        "SELECT COALESCE(unit, '—') AS u, COUNT(*) FROM goods GROUP BY COALESCE(unit, '—')",
        &[],
    )
    .unwrap();
    assert_eq!(s.params, vec![text("—")]);
    assert_eq!(s.sql.matches("?1").count(), 2, "{}", s.sql);
}

#[test]
fn long_operator_chains_are_errors_not_stack_overflow() {
    // Разборщик строит цепочку без счётчика глубины; проверяем в потоке со стеком 1 МБ,
    // как у главного потока Windows.
    let run = |n: usize| {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                let chain = vec!["id = 1"; n].join(" OR ");
                sqlite(&format!("SELECT name FROM goods WHERE {chain}"), &[])
            })
            .unwrap()
            .join()
            .unwrap()
    };
    assert!(run(50).is_ok());
    assert_eq!(run(100).unwrap_err(), SqlError::TooComplex);
    assert_eq!(run(200).unwrap_err(), SqlError::TooComplex);
    assert_eq!(run(600).unwrap_err(), SqlError::TooComplex);
    assert!(matches!(
        run(20_000).unwrap_err(),
        SqlError::TooLong { .. } | SqlError::TooComplex
    ));
    let plus = vec!["qty"; 5000].join(" + ");
    assert_eq!(
        err(&format!("SELECT {plus} FROM stock")),
        SqlError::TooComplex
    );
}

#[test]
fn nul_in_text_is_rejected() {
    let query = "SELECT name FROM goods WHERE name = :n";
    assert_eq!(
        sqlite(query, &[("n", text("Мука\0"))]).unwrap_err(),
        SqlError::NulInText
    );
    assert_eq!(
        pg("SELECT name FROM goods WHERE name = 'a\0b'", &[]).unwrap_err(),
        SqlError::NulInText
    );
}
