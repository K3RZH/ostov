//! Себестоимость скользящей средней (СТ-К10, ADR-042) и отказ при приходе задним числом (СТ-53).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;

use ostov_core::{
    AccumulationRegister, CoreError, Cost, FieldDef, FieldKind, FieldValue, Fields, Movement,
    MovementKind,
};
use ostov_store::drivers::postgres::PostgresDriver;
use ostov_store::drivers::sqlite::SqliteDriver;
use ostov_store::{Decimal, Driver};

use MovementKind::{Issue, Receipt};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn register() -> AccumulationRegister {
    AccumulationRegister::new(
        "stock",
        vec![
            FieldDef::new("store", FieldKind::Text),
            FieldDef::new("item", FieldKind::Int),
            FieldDef::new("batch", FieldKind::Text),
        ],
        vec![
            FieldDef::new("qty", FieldKind::Decimal { scale: 3 }),
            FieldDef::new("amount", FieldKind::Decimal { scale: 2 }),
        ],
    )
    .unwrap()
}

fn cost() -> Cost {
    Cost {
        qty: "qty".into(),
        sum: "amount".into(),
        by: vec!["store".into(), "item".into()],
    }
}

struct Stock {
    db: Box<dyn Driver>,
    reg: AccumulationRegister,
}

impl Stock {
    fn new(mut db: Box<dyn Driver>) -> Self {
        let reg = register().with_cost(cost()).unwrap();
        reg.create_tables(&mut *db).unwrap();
        Self { db, reg }
    }

    fn sqlite() -> Self {
        Self::new(Box::new(SqliteDriver::open_in_memory().unwrap()))
    }

    /// Своя схема на каждый тест: тесты идут параллельно.
    fn postgres(test: &str) -> Self {
        let url = std::env::var("OSTOV_PG_URL").unwrap();
        let schema = format!("ostov_t_cost_{test}");
        let mut db = PostgresDriver::connect(&url).unwrap();
        for sql in [
            format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
            format!("CREATE SCHEMA {schema}"),
            format!("SET search_path TO {schema}"),
        ] {
            db.execute(&sql, &[]).unwrap();
        }
        Self::new(Box::new(db))
    }

    fn post(&mut self, doc: (&str, i64), day: &str, moves: &[Movement]) -> Result<(), CoreError> {
        self.reg
            .replace_dated(&mut *self.db, doc.0, doc.1, Some(day), moves)
    }

    fn amount(&mut self, doc: (&str, i64)) -> Vec<Decimal> {
        self.reg
            .movements_of(&mut *self.db, doc.0, doc.1)
            .unwrap()
            .iter()
            .map(|m| m.resources["amount"])
            .collect()
    }

    fn balance(&mut self, batch: &str) -> (Decimal, Decimal) {
        let b = self.reg.balance(&mut *self.db, &dims(batch)).unwrap();
        (b["qty"], b["amount"])
    }
}

fn dims(batch: &str) -> Fields {
    Fields::from([
        ("store".to_owned(), FieldValue::Text("main".into())),
        ("item".to_owned(), FieldValue::Int(1)),
        ("batch".to_owned(), FieldValue::Text(batch.into())),
    ])
}

fn mv(kind: MovementKind, batch: &str, qty: &str, amount: Option<&str>) -> Movement {
    let mut resources = BTreeMap::from([("qty".to_owned(), d(qty))]);
    if let Some(a) = amount {
        resources.insert("amount".into(), d(a));
    }
    Movement {
        kind,
        dims: dims(batch),
        resources,
    }
}

const R1: (&str, i64) = ("receipt", 1);
const R2: (&str, i64) = ("receipt", 2);
const R3: (&str, i64) = ("receipt", 3);
const I1: (&str, i64) = ("issue", 1);
const I2: (&str, i64) = ("issue", 2);

fn reference_example_from_srs(s: &mut Stock) {
    // СТ-К10: остаток 10 кг на 500,00 ₽, приход 10 кг по 70,00 ₽ — 20 кг на 1 200,00 ₽,
    // расход 5 кг стоит 300,00 ₽, остаётся 15 кг на 900,00 ₽. Партии в среднюю не входят.
    s.post(R1, "2026-04-01", &[mv(Receipt, "A", "10", Some("500"))])
        .unwrap();
    s.post(R2, "2026-04-02", &[mv(Receipt, "B", "10", Some("700"))])
        .unwrap();
    s.post(I1, "2026-04-03", &[mv(Issue, "A", "5", None)])
        .unwrap();
    assert_eq!(s.amount(I1), [d("300.00")]);
    let (a, b) = (s.balance("A"), s.balance("B"));
    assert_eq!(a, (d("5"), d("200")));
    assert_eq!(b, (d("10"), d("700")));
    assert_eq!(a.0.checked_add(b.0).unwrap(), d("15"));
    assert_eq!(a.1.checked_add(b.1).unwrap(), d("900"));
}

fn last_issue_takes_the_whole_rest(s: &mut Stock) {
    // 3 шт за 100,00 ₽: 33,33 + 66,67 — без копеечного хвоста на нулевом остатке (СТ-К15).
    s.post(R1, "2026-04-01", &[mv(Receipt, "A", "3", Some("100"))])
        .unwrap();
    s.post(I1, "2026-04-02", &[mv(Issue, "A", "1", None)])
        .unwrap();
    s.post(I2, "2026-04-02", &[mv(Issue, "A", "2", None)])
        .unwrap();
    assert_eq!(s.amount(I1), [d("33.33")]);
    assert_eq!(s.amount(I2), [d("66.67")]);
    assert_eq!(s.balance("A"), (d("0"), d("0")));
}

fn lines_of_one_document_share_the_rest(s: &mut Stock) {
    s.post(R1, "2026-04-01", &[mv(Receipt, "A", "3", Some("100"))])
        .unwrap();
    let both = [mv(Issue, "A", "1", None), mv(Issue, "A", "2", None)];
    s.post(I1, "2026-04-02", &both).unwrap();
    assert_eq!(s.amount(I1), [d("33.33"), d("66.67")]);
    assert_eq!(s.balance("A"), (d("0"), d("0")));
}

fn average_is_taken_on_the_document_date(s: &mut Stock) {
    // Приход 5 апреля не влияет на расход 3 апреля: средняя на 3-е — 50,00 ₽.
    s.post(R1, "2026-04-01", &[mv(Receipt, "A", "10", Some("500"))])
        .unwrap();
    s.post(R2, "2026-04-05", &[mv(Receipt, "B", "10", Some("700"))])
        .unwrap();
    s.post(I1, "2026-04-03", &[mv(Issue, "A", "5", None)])
        .unwrap();
    assert_eq!(s.amount(I1), [d("250.00")]);
}

fn explicit_amount_is_kept(s: &mut Stock) {
    s.post(R1, "2026-04-01", &[mv(Receipt, "A", "10", Some("500"))])
        .unwrap();
    s.post(I1, "2026-04-02", &[mv(Issue, "A", "1", Some("80"))])
        .unwrap();
    assert_eq!(s.amount(I1), [d("80.00")]);
}

fn backdated_receipt_under_spent_goods_is_rejected(s: &mut Stock) {
    s.post(R1, "2026-04-01", &[mv(Receipt, "A", "10", Some("500"))])
        .unwrap();
    s.post(I1, "2026-04-03", &[mv(Issue, "A", "5", None)])
        .unwrap();

    // Приход 2 апреля поменял бы среднюю расхода 3 апреля.
    let early = [mv(Receipt, "B", "10", Some("700"))];
    let Err(CoreError::Invalid(why)) = s.post(R2, "2026-04-02", &early) else {
        panic!("приход задним числом прошёл");
    };
    assert!(why.contains("issue #1 от 2026-04-03"), "{why}");
    assert!(
        why.contains("Сначала отмените более поздний расход"),
        "{why}"
    );
    // Отказ не оставил следов: движений прихода нет, расход не пересчитан.
    assert!(s.amount(R2).is_empty());
    assert_eq!(s.amount(I1), [d("250.00")]);

    // В тот же день новый приход идёт после расхода — можно.
    s.post(R2, "2026-04-03", &early).unwrap();
    // Другой товар не мешает.
    let other = Movement {
        kind: Receipt,
        dims: Fields::from([
            ("store".to_owned(), FieldValue::Text("main".into())),
            ("item".to_owned(), FieldValue::Int(2)),
            ("batch".to_owned(), FieldValue::Text("A".into())),
        ]),
        resources: BTreeMap::from([("qty".to_owned(), d("1")), ("amount".into(), d("1"))]),
    };
    s.post(R3, "2026-04-01", &[other]).unwrap();

    // Убрать или поменять старый приход нельзя, перепровести без изменений — можно.
    let first = [mv(Receipt, "A", "10", Some("500"))];
    assert!(matches!(
        s.post(R1, "2026-04-01", &[]),
        Err(CoreError::Invalid(_))
    ));
    let cheaper = [mv(Receipt, "A", "10", Some("400"))];
    assert!(matches!(
        s.post(R1, "2026-04-01", &cheaper),
        Err(CoreError::Invalid(_))
    ));
    assert!(matches!(
        s.post(R1, "2026-04-04", &first),
        Err(CoreError::Invalid(_))
    ));
    s.post(R1, "2026-04-01", &first).unwrap();

    // Отменили расход — приход снова свободен.
    s.post(I1, "2026-04-03", &[]).unwrap();
    s.post(R1, "2026-04-01", &cheaper).unwrap();
}

#[test]
fn cost_register_never_goes_negative() {
    let mut db = SqliteDriver::open_in_memory().unwrap();
    let reg = register().with_cost(cost()).unwrap().allow_negative();
    reg.create_tables(&mut db).unwrap();
    let issue = [mv(Issue, "A", "1", None)];
    assert!(matches!(
        reg.replace_dated(&mut db, "issue", 1, Some("2026-04-01"), &issue),
        Err(CoreError::NegativeBalance(_))
    ));
}

#[test]
fn cost_description_is_checked() {
    let with = |qty: &str, sum: &str, by: &[&str]| {
        register()
            .with_cost(Cost {
                qty: qty.into(),
                sum: sum.into(),
                by: by.iter().map(|b| (*b).to_owned()).collect(),
            })
            .map(|_| ())
    };
    assert!(with("qty", "amount", &["store", "item"]).is_ok());
    assert!(with("qty", "qty", &["item"]).is_err());
    assert!(with("qty", "price", &["item"]).is_err());
    assert!(with("qty", "amount", &[]).is_err());
    assert!(with("qty", "amount", &["shelf"]).is_err());
    assert!(with("qty", "amount", &["item", "item"]).is_err());
}

/// Каждый сценарий — на SQLite и на PostgreSQL (СТ-11).
macro_rules! on_both {
    ($($name:ident),* $(,)?) => {
        mod sqlite {
            $(#[test]
            fn $name() {
                super::$name(&mut super::Stock::sqlite());
            })*
        }
        mod postgres {
            $(#[test]
            #[cfg_attr(not(pg_url), ignore = "нужен OSTOV_PG_URL")]
            fn $name() {
                super::$name(&mut super::Stock::postgres(stringify!($name)));
            })*
        }
    };
}

on_both!(
    reference_example_from_srs,
    last_issue_takes_the_whole_rest,
    lines_of_one_document_share_the_rest,
    average_is_taken_on_the_document_date,
    explicit_amount_is_kept,
    backdated_receipt_under_spent_goods_is_rejected,
);
