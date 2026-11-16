//! Ограничения по записям (СТ-52): `within` добавляет `поле IN (…)` к каждому источнику.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;

use ostov_store::Value;
use ostov_store::drivers::postgres::PostgresDialect;
use ostov_store::drivers::sqlite::SqliteDialect;

use super::*;

type Limits = Vec<(String, Vec<Param>)>;

fn config() -> ostov_model::Config {
    ostov_meta::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"))
        .unwrap()
}

/// Кладовщик видит склады 1 и 2 в расходах и остатках, поступления — только склада 3.
fn storekeeper(object: &str) -> Option<Limits> {
    let ids = |ids: &[i64]| ids.iter().map(|id| Param::Int(*id)).collect();
    match object {
        "issue" | "stock" => Some(vec![("warehouse".into(), ids(&[1, 2]))]),
        "receipt" => Some(vec![("warehouse".into(), ids(&[3]))]),
        _ => None,
    }
}

fn run(
    query: &str,
    p: &[(&str, Param)],
    within: &dyn Fn(&str) -> Option<Limits>,
    dialect: &dyn ostov_store::Dialect,
) -> Result<Prepared, SqlError> {
    let params: BTreeMap<String, Param> = p
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    prepare(query, &params, &config(), &|_| true, within, dialect)
}

fn sqlite(query: &str, p: &[(&str, Param)]) -> Prepared {
    run(query, p, &storekeeper, &SqliteDialect).unwrap()
}

fn pg(query: &str, p: &[(&str, Param)]) -> Prepared {
    run(query, p, &storekeeper, &PostgresDialect).unwrap()
}

#[test]
fn single_source_gets_in_on_both_dialects() {
    let q = "SELECT number FROM issue";
    assert_eq!(
        sqlite(q, &[]).sql,
        "SELECT \"t1\".\"number\" AS \"number\" FROM \"issue\" AS \"t1\" \
         WHERE (\"t1\".\"warehouse\" IN (?1, ?2)) LIMIT 1000"
    );
    let p = pg(q, &[]);
    assert!(
        p.sql
            .ends_with("WHERE (\"t1\".\"warehouse\" IN ($1, $2)) LIMIT 1000"),
        "{}",
        p.sql
    );
    assert_eq!(p.params, vec![Value::Int(1), Value::Int(2)]);
}

#[test]
fn user_params_come_first_and_do_not_mix() {
    let q = "SELECT number FROM issue WHERE number > :n OR number = :n";
    let p = sqlite(q, &[("n", Param::Int(5))]);
    assert_eq!(
        p.sql,
        "SELECT \"t1\".\"number\" AS \"number\" FROM \"issue\" AS \"t1\" \
         WHERE (((\"t1\".\"number\" > ?1) OR (\"t1\".\"number\" = ?2))) \
         AND (\"t1\".\"warehouse\" IN (?3, ?4)) LIMIT 1000"
    );
    assert_eq!(
        p.params,
        vec![Value::Int(5), Value::Int(5), Value::Int(1), Value::Int(2)]
    );
    // Параметр пользователя с тем же именем, что поле ограничения, — отдельный.
    let p = sqlite(
        "SELECT number FROM issue WHERE warehouse = :warehouse",
        &[("warehouse", Param::Int(9))],
    );
    assert_eq!(p.params, vec![Value::Int(9), Value::Int(1), Value::Int(2)]);
}

#[test]
fn join_of_two_restricted_sources() {
    let q = "SELECT i.number, s.qty FROM issue i \
             INNER JOIN stock s ON s.recorder_id = i.id AND s.recorder = 'issue'";
    let p = sqlite(q, &[]);
    assert_eq!(
        p.sql,
        "SELECT \"t1\".\"number\" AS \"number\", \"t2\".\"qty\" AS \"qty\" \
         FROM \"issue\" AS \"t1\" INNER JOIN \"stock\" AS \"t2\" \
         ON (((\"t2\".\"recorder_id\" = \"t1\".\"id\") AND (\"t2\".\"recorder\" = ?1))) \
         AND (\"t2\".\"warehouse\" IN (?2, ?3)) \
         WHERE (\"t1\".\"warehouse\" IN (?4, ?5)) LIMIT 1000"
    );
    assert_eq!(p.params.len(), 5);
    assert_eq!(p.params[0], Value::Text("issue".into()));
    let p = pg(q, &[]);
    assert!(p.sql.contains("IN ($2, $3)") && p.sql.contains("IN ($4, $5)"));
}

#[test]
fn left_join_keeps_restriction_in_on() {
    let q = "SELECT g.name, s.qty FROM goods g LEFT JOIN stock s ON s.goods = g.id";
    let p = sqlite(q, &[]);
    assert!(
        p.sql.contains(
            "LEFT JOIN \"stock\" AS \"t2\" ON ((\"t2\".\"goods\" = \"t1\".\"id\")) \
             AND (\"t2\".\"warehouse\" IN (?1, ?2))"
        ),
        "{}",
        p.sql
    );
    assert!(!p.sql.contains("WHERE"), "{}", p.sql);
}

#[test]
fn register_with_group_by() {
    let p = sqlite(
        "SELECT warehouse, SUM(qty) AS total FROM stock GROUP BY warehouse ORDER BY warehouse",
        &[],
    );
    assert!(
        p.sql.contains(
            "FROM \"stock\" AS \"t1\" WHERE (\"t1\".\"warehouse\" IN (?1, ?2)) \
             GROUP BY \"t1\".\"warehouse\""
        ),
        "{}",
        p.sql
    );
}

#[test]
fn same_object_twice_is_restricted_twice() {
    let p = sqlite(
        "SELECT a.number FROM issue a INNER JOIN issue b ON b.id = a.id",
        &[],
    );
    assert!(
        p.sql.contains("AND (\"t2\".\"warehouse\" IN (?1, ?2))"),
        "{}",
        p.sql
    );
    assert!(
        p.sql.contains("WHERE (\"t1\".\"warehouse\" IN (?3, ?4))"),
        "{}",
        p.sql
    );
}

#[test]
fn empty_values_see_nothing_and_conditions_go_with_and() {
    let nothing = |object: &str| -> Option<Limits> {
        (object == "stock").then(|| {
            vec![
                ("warehouse".into(), vec![Param::Int(1)]),
                ("goods".into(), vec![]),
            ]
        })
    };
    let p = run("SELECT qty FROM stock", &[], &nothing, &SqliteDialect).unwrap();
    assert!(
        p.sql
            .contains("WHERE (\"t1\".\"warehouse\" IN (?1) AND 1 = 0)"),
        "{}",
        p.sql
    );
}

#[test]
fn table_part_goes_through_its_document_header() {
    let p = sqlite("SELECT l.qty FROM issue.lines l", &[]);
    assert_eq!(
        p.sql,
        "SELECT \"t1\".\"qty\" AS \"qty\" FROM \"issue__lines\" AS \"t1\" \
         WHERE (\"t1\".\"doc_id\" IN (SELECT \"t1_doc\".\"id\" FROM \"issue\" AS \"t1_doc\" \
         WHERE (\"t1_doc\".\"warehouse\" IN (?1, ?2)))) LIMIT 1000"
    );
}

#[test]
fn unrestricted_objects_are_untouched() {
    let p = sqlite("SELECT name FROM goods", &[]);
    assert!(!p.sql.contains("WHERE") && p.params.is_empty(), "{}", p.sql);
}

#[test]
fn restriction_on_unknown_field_is_an_error() {
    let bad = |_: &str| -> Option<Limits> { Some(vec![("shop".into(), vec![Param::Int(1)])]) };
    assert_eq!(
        run("SELECT name FROM goods", &[], &bad, &SqliteDialect).unwrap_err(),
        SqlError::BadRestriction {
            object: "goods".into(),
            field: "shop".into()
        }
    );
}

#[test]
fn empty_field_list_means_no_restriction() {
    let empty = |_: &str| -> Option<Limits> { Some(Vec::new()) };
    let p = run("SELECT number FROM issue", &[], &empty, &SqliteDialect).unwrap();
    assert!(!p.sql.contains("WHERE"), "{}", p.sql);
}

#[test]
fn null_value_and_wrong_money_scale() {
    let with_null = |object: &str| -> Option<Limits> {
        (object == "issue").then(|| vec![("warehouse".into(), vec![Param::Null, Param::Int(1)])])
    };
    let p = run("SELECT number FROM issue", &[], &with_null, &SqliteDialect).unwrap();
    assert_eq!(p.params, vec![Value::Null, Value::Int(1)]);
    let money = |object: &str| -> Option<Limits> {
        (object == "receipt").then(|| {
            vec![(
                "amount".into(),
                vec![Param::Decimal {
                    units: 100,
                    scale: 3,
                }],
            )]
        })
    };
    assert!(matches!(
        run("SELECT number FROM receipt", &[], &money, &SqliteDialect).unwrap_err(),
        SqlError::ScaleMismatch { .. }
    ));
}

#[test]
fn projection_param_keeps_its_number() {
    let p = sqlite(
        "SELECT :x AS a, number FROM issue WHERE number > :n",
        &[("x", Param::Text("x".into())), ("n", Param::Int(3))],
    );
    assert!(p.sql.starts_with("SELECT ?1 AS \"a\""), "{}", p.sql);
    assert!(p.sql.contains("(\"t1\".\"number\" > ?2)"), "{}", p.sql);
    assert_eq!(
        p.params,
        vec![
            Value::Text("x".into()),
            Value::Int(3),
            Value::Int(1),
            Value::Int(2)
        ]
    );
}

#[test]
fn table_part_twice_has_two_subqueries() {
    let p = sqlite(
        "SELECT a.qty FROM issue.lines a INNER JOIN issue.lines b ON b.id = a.id",
        &[],
    );
    assert!(p.sql.contains("FROM \"issue\" AS \"t2_doc\""), "{}", p.sql);
    assert!(p.sql.contains("FROM \"issue\" AS \"t1_doc\""), "{}", p.sql);
}
