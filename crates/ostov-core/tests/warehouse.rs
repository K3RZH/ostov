//! Сценарии склада через публичный API ядра: справочник, документы, регистр, проведение.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;

use ostov_core::{
    AccumulationRegister, Catalog, CoreError, Document, DocumentKind, FieldDef, FieldKind,
    FieldValue, Fields, Movement, MovementKind, TablePart, post, unpost,
};
use ostov_store::Decimal;
use ostov_store::drivers::sqlite::SqliteDriver;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn fields(pairs: &[(&str, FieldValue)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.to_owned())
}

fn goods() -> Catalog {
    Catalog::new(
        "goods",
        vec![
            FieldDef::new("name", FieldKind::Text),
            FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
        ],
    )
    .unwrap()
}

fn doc_kind(table: &str) -> DocumentKind {
    let lines = TablePart::new(
        "lines",
        vec![
            FieldDef::new("item", FieldKind::Int),
            FieldDef::new("qty", FieldKind::Decimal { scale: 3 }),
            FieldDef::new("amount", FieldKind::Decimal { scale: 2 }),
        ],
    )
    .unwrap();
    DocumentKind::new(
        table,
        vec![FieldDef::new("store", FieldKind::Text)],
        vec![lines],
    )
    .unwrap()
}

fn stock() -> AccumulationRegister {
    AccumulationRegister::new(
        "stock",
        vec![
            FieldDef::new("store", FieldKind::Text),
            FieldDef::new("item", FieldKind::Int),
        ],
        vec![
            FieldDef::new("qty", FieldKind::Decimal { scale: 3 }),
            FieldDef::new("amount", FieldKind::Decimal { scale: 2 }),
        ],
    )
    .unwrap()
}

struct Warehouse {
    db: SqliteDriver,
    goods: Catalog,
    receipt: DocumentKind,
    issue: DocumentKind,
    stock: AccumulationRegister,
}

impl Warehouse {
    fn new(db: SqliteDriver, stock: AccumulationRegister) -> Self {
        let mut w = Self {
            db,
            goods: goods(),
            receipt: doc_kind("receipt"),
            issue: doc_kind("issue"),
            stock,
        };
        w.goods.create_table(&mut w.db).unwrap();
        w.receipt.create_tables(&mut w.db).unwrap();
        w.issue.create_tables(&mut w.db).unwrap();
        w.stock.create_tables(&mut w.db).unwrap();
        w
    }

    fn in_memory() -> Self {
        Self::new(SqliteDriver::open_in_memory().unwrap(), stock())
    }

    fn item(&mut self, name: &str, price: &str) -> i64 {
        let f = fields(&[
            ("name", text(name)),
            ("price", FieldValue::Decimal(d(price))),
        ]);
        self.goods.insert(&mut self.db, &f).unwrap().id
    }

    /// Строка документа: сумма = округление(количество × цена) до копеек.
    fn line(&mut self, item: i64, qty: &str) -> Fields {
        let good = self.goods.get(&mut self.db, item).unwrap().unwrap();
        let FieldValue::Decimal(price) = good.fields["price"] else {
            panic!("нет цены");
        };
        let amount = d(qty).checked_mul(price, 2).unwrap();
        fields(&[
            ("item", FieldValue::Int(item)),
            ("qty", FieldValue::Decimal(d(qty))),
            ("amount", FieldValue::Decimal(amount)),
        ])
    }

    fn doc(&mut self, kind: MovementKind, store: &str, lines: Vec<Fields>) -> i64 {
        let header = fields(&[("store", text(store))]);
        let parts = BTreeMap::from([("lines".to_owned(), lines)]);
        let doc_kind = self.kind(kind).clone();
        doc_kind.insert(&mut self.db, &header, &parts).unwrap().id
    }

    fn kind(&self, kind: MovementKind) -> &DocumentKind {
        match kind {
            MovementKind::Receipt => &self.receipt,
            MovementKind::Issue => &self.issue,
        }
    }

    fn post(&mut self, kind: MovementKind, id: i64) -> Result<(), CoreError> {
        let doc_kind = self.kind(kind).clone();
        post(&mut self.db, &doc_kind, &self.stock, id, |doc| {
            movements(kind, doc)
        })
    }

    fn unpost(&mut self, kind: MovementKind, id: i64) -> Result<(), CoreError> {
        let doc_kind = self.kind(kind).clone();
        unpost(&mut self.db, &doc_kind, &self.stock, id)
    }

    fn get(&mut self, kind: MovementKind, id: i64) -> Document {
        let doc_kind = self.kind(kind).clone();
        doc_kind.get(&mut self.db, id).unwrap().unwrap()
    }

    fn balance(&mut self, store: &str, item: i64) -> (Decimal, Decimal) {
        let dims = fields(&[("store", text(store)), ("item", FieldValue::Int(item))]);
        let b = self.stock.balance(&mut self.db, &dims).unwrap();
        (b["qty"], b["amount"])
    }
}

fn movements(kind: MovementKind, doc: &Document) -> Result<Vec<Movement>, CoreError> {
    doc.parts["lines"]
        .iter()
        .map(|line| {
            let mut resources = BTreeMap::new();
            for name in ["qty", "amount"] {
                if let FieldValue::Decimal(v) = line[name] {
                    resources.insert(name.to_owned(), v);
                }
            }
            Ok(Movement {
                kind,
                dims: fields(&[
                    ("store", doc.header["store"].clone()),
                    ("item", line["item"].clone()),
                ]),
                resources,
            })
        })
        .collect()
}

use MovementKind::{Issue, Receipt};

#[test]
fn full_warehouse_flow() {
    let mut w = Warehouse::in_memory();
    let flour = w.item("Мука", "3.33");
    let salt = w.item("Соль", "1.20");

    let lines = vec![w.line(flour, "2.5"), w.line(salt, "10")];
    let r = w.doc(Receipt, "main", lines);
    w.post(Receipt, r).unwrap();
    // 2.5 × 3.33 = 8.325 → 8.33, половина округляется вверх
    assert_eq!(w.balance("main", flour), (d("2.5"), d("8.33")));
    assert_eq!(w.balance("main", salt), (d("10"), d("12")));

    let lines = vec![w.line(salt, "4")];
    let i = w.doc(Issue, "main", lines);
    w.post(Issue, i).unwrap();
    assert_eq!(w.balance("main", salt), (d("6"), d("7.2")));
    assert!(w.get(Issue, i).posted);
}

#[test]
fn stores_are_separate() {
    let mut w = Warehouse::in_memory();
    let flour = w.item("Мука", "50");
    let lines = vec![w.line(flour, "5")];
    let r = w.doc(Receipt, "kitchen", lines);
    w.post(Receipt, r).unwrap();

    let lines = vec![w.line(flour, "1")];
    let i = w.doc(Issue, "bar", lines);
    let err = w.post(Issue, i).unwrap_err();
    assert!(matches!(err, CoreError::NegativeBalance { .. }), "{err}");
    assert_eq!(w.balance("kitchen", flour), (d("5"), d("250")));
    assert_eq!(w.balance("bar", flour), (d("0"), d("0")));
}

#[test]
fn register_can_allow_negative() {
    let mut w = Warehouse::new(
        SqliteDriver::open_in_memory().unwrap(),
        stock().allow_negative(),
    );
    let flour = w.item("Мука", "50");
    let lines = vec![w.line(flour, "3")];
    let i = w.doc(Issue, "main", lines);
    w.post(Issue, i).unwrap();
    assert_eq!(w.balance("main", flour), (d("-3"), d("-150")));
}

#[test]
fn bad_movements_are_rejected_and_nothing_is_written() {
    let mut w = Warehouse::in_memory();
    let flour = w.item("Мука", "50");
    let lines = vec![w.line(flour, "5")];
    let r = w.doc(Receipt, "main", lines);
    let receipt = w.receipt.clone();

    let bad: Vec<Movement> = vec![
        // нет измерения store
        Movement {
            kind: Receipt,
            dims: fields(&[("item", FieldValue::Int(flour))]),
            resources: BTreeMap::new(),
        },
        // пустое измерение
        Movement {
            kind: Receipt,
            dims: fields(&[
                ("store", FieldValue::Null),
                ("item", FieldValue::Int(flour)),
            ]),
            resources: BTreeMap::new(),
        },
        // лишнее измерение
        Movement {
            kind: Receipt,
            dims: fields(&[
                ("store", text("main")),
                ("item", FieldValue::Int(flour)),
                ("color", text("red")),
            ]),
            resources: BTreeMap::new(),
        },
        // неизвестный ресурс
        Movement {
            kind: Receipt,
            dims: fields(&[("store", text("main")), ("item", FieldValue::Int(flour))]),
            resources: BTreeMap::from([("weight".to_owned(), d("1"))]),
        },
        // четыре знака в количестве с тремя
        Movement {
            kind: Receipt,
            dims: fields(&[("store", text("main")), ("item", FieldValue::Int(flour))]),
            resources: BTreeMap::from([("qty".to_owned(), d("1.0001"))]),
        },
    ];
    for movement in bad {
        let err = post(&mut w.db, &receipt, &w.stock, r, |_| {
            Ok(vec![movement.clone()])
        })
        .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)), "{movement:?}: {err}");
    }
    assert_eq!(w.balance("main", flour), (d("0"), d("0")));
    assert!(!w.get(Receipt, r).posted);
}

#[test]
fn register_definition_is_checked() {
    let text_dim = || FieldDef::new("store", FieldKind::Text);
    let qty = || FieldDef::new("qty", FieldKind::Decimal { scale: 3 });

    assert!(AccumulationRegister::new("stock", vec![], vec![qty()]).is_err());
    let money_dim = FieldDef::new("price", FieldKind::Decimal { scale: 2 });
    assert!(AccumulationRegister::new("stock", vec![money_dim], vec![qty()]).is_err());
    let int_resource = FieldDef::new("count", FieldKind::Int);
    assert!(AccumulationRegister::new("stock", vec![text_dim()], vec![int_resource]).is_err());
    let reserved = FieldDef::new("kind", FieldKind::Text);
    assert!(AccumulationRegister::new("stock", vec![reserved], vec![qty()]).is_err());
    assert!(AccumulationRegister::new("Остатки", vec![text_dim()], vec![qty()]).is_err());
}

#[test]
fn document_errors() {
    let mut w = Warehouse::in_memory();
    let header = fields(&[("store", text("main"))]);

    let unknown_part = BTreeMap::from([("services".to_owned(), vec![])]);
    let err = w
        .receipt
        .insert(&mut w.db, &header, &unknown_part)
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)));

    let wrong_line =
        BTreeMap::from([("lines".to_owned(), vec![fields(&[("item", text("мука"))])])]);
    let err = w
        .receipt
        .insert(&mut w.db, &header, &wrong_line)
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)));

    assert_eq!(w.receipt.get(&mut w.db, 404).unwrap(), None);
    assert!(matches!(
        w.post(Receipt, 404),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        w.unpost(Receipt, 404),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn posting_changes_version() {
    let mut w = Warehouse::in_memory();
    let flour = w.item("Мука", "50");
    let lines = vec![w.line(flour, "1")];
    let r = w.doc(Receipt, "main", lines);

    assert_eq!(w.get(Receipt, r).version, 1);
    w.post(Receipt, r).unwrap();
    let doc = w.get(Receipt, r);
    assert_eq!((doc.version, doc.posted), (2, true));
    w.unpost(Receipt, r).unwrap();
    let doc = w.get(Receipt, r);
    assert_eq!((doc.version, doc.posted), (3, false));
}

#[test]
fn failed_post_leaves_no_trace() {
    let mut w = Warehouse::in_memory();
    let flour = w.item("Мука", "50");
    let lines = vec![w.line(flour, "3")];
    let i = w.doc(Issue, "main", lines);
    assert!(w.post(Issue, i).is_err());

    // После неудачи тот же документ проводится, как только товар пришёл.
    let lines = vec![w.line(flour, "5")];
    let r = w.doc(Receipt, "main", lines);
    w.post(Receipt, r).unwrap();
    w.post(Issue, i).unwrap();
    assert_eq!(w.balance("main", flour), (d("2"), d("100")));
}

#[test]
fn data_survives_reopen() {
    let dir = std::env::temp_dir().join(format!("ostov-core-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shop.db");

    let (flour, r) = {
        let mut w = Warehouse::new(SqliteDriver::open(&path).unwrap(), stock());
        let flour = w.item("Мука", "50");
        let lines = vec![w.line(flour, "7")];
        let r = w.doc(Receipt, "main", lines);
        w.post(Receipt, r).unwrap();
        (flour, r)
    };

    let mut db = SqliteDriver::open(&path).unwrap();
    let dims = fields(&[("store", text("main")), ("item", FieldValue::Int(flour))]);
    assert_eq!(stock().balance(&mut db, &dims).unwrap()["qty"], d("7"));
    assert!(doc_kind("receipt").get(&mut db, r).unwrap().unwrap().posted);
    drop(db);
    std::fs::remove_dir_all(&dir).unwrap();
}
