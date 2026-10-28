//! Проведение документов: движения в регистр пишутся одной транзакцией с отметкой «проведён».

use ostov_store::Driver;

use crate::object::document::{Document, DocumentKind};
use crate::register::{AccumulationRegister, Movement};
use crate::{CoreError, in_transaction};

/// Проводит документ. Движения строит `rule` — потом это будет `on_post` из Python-конфигурации.
/// Повторное проведение заменяет прежние движения; любая ошибка откатывает всё.
pub fn post(
    db: &mut dyn Driver,
    kind: &DocumentKind,
    register: &AccumulationRegister,
    id: i64,
    rule: impl FnOnce(&Document) -> Result<Vec<Movement>, CoreError>,
) -> Result<(), CoreError> {
    in_transaction(db, |db| {
        let doc = kind.get(db, id)?.ok_or_else(|| kind.not_found(id))?;
        let movements = rule(&doc)?;
        register.replace(db, kind.table(), id, &movements)?;
        kind.set_posted(db, id, true)
    })
}

/// Отменяет проведение: движения удаляются, итоги пересчитываются.
pub fn unpost(
    db: &mut dyn Driver,
    kind: &DocumentKind,
    register: &AccumulationRegister,
    id: i64,
) -> Result<(), CoreError> {
    in_transaction(db, |db| {
        kind.get(db, id)?.ok_or_else(|| kind.not_found(id))?;
        register.replace(db, kind.table(), id, &[])?;
        kind.set_posted(db, id, false)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::object::document::TablePart;
    use crate::register::MovementKind;
    use crate::testing::*;
    use crate::{FieldDef, FieldKind, FieldValue, Fields};
    use ostov_store::Decimal;
    use ostov_store::drivers::sqlite::SqliteDriver;

    struct Setup {
        db: SqliteDriver,
        receipt: DocumentKind,
        issue: DocumentKind,
        stock: AccumulationRegister,
    }

    fn doc_kind(table: &str) -> DocumentKind {
        DocumentKind::new(
            table,
            vec![FieldDef::new("store", FieldKind::Text)],
            vec![
                TablePart::new(
                    "lines",
                    vec![
                        FieldDef::new("item", FieldKind::Int),
                        FieldDef::new("qty", FieldKind::Decimal { scale: 3 }),
                    ],
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    fn setup() -> Setup {
        let mut db = memory_db();
        let receipt = doc_kind("receipt");
        let issue = doc_kind("issue");
        let stock = AccumulationRegister::new(
            "stock",
            vec![
                FieldDef::new("store", FieldKind::Text),
                FieldDef::new("item", FieldKind::Int),
            ],
            vec![FieldDef::new("qty", FieldKind::Decimal { scale: 3 })],
        )
        .unwrap();
        receipt.create_tables(&mut db).unwrap();
        issue.create_tables(&mut db).unwrap();
        stock.create_tables(&mut db).unwrap();
        Setup {
            db,
            receipt,
            issue,
            stock,
        }
    }

    // Каждая строка документа — одно движение по складу и товару.
    fn rule(kind: MovementKind) -> impl FnOnce(&Document) -> Result<Vec<Movement>, CoreError> {
        move |doc| {
            doc.parts["lines"]
                .iter()
                .map(|line| {
                    let FieldValue::Decimal(qty) = line["qty"] else {
                        return Err(CoreError::Invalid("нет количества".into()));
                    };
                    Ok(Movement {
                        kind,
                        dims: fields(&[
                            ("store", doc.header["store"].clone()),
                            ("item", line["item"].clone()),
                        ]),
                        resources: BTreeMap::from([("qty".to_owned(), qty)]),
                    })
                })
                .collect()
        }
    }

    fn new_doc(s: &mut Setup, kind: MovementKind, lines: &[(i64, &str)]) -> i64 {
        let doc_kind = match kind {
            MovementKind::Receipt => &s.receipt,
            MovementKind::Issue => &s.issue,
        };
        let lines: Vec<Fields> = lines
            .iter()
            .map(|(item, qty)| fields(&[("item", FieldValue::Int(*item)), ("qty", dec(qty))]))
            .collect();
        doc_kind
            .insert(
                &mut s.db,
                &fields(&[("store", text("main"))]),
                &BTreeMap::from([("lines".to_owned(), lines)]),
            )
            .unwrap()
            .id
    }

    fn post_doc(s: &mut Setup, kind: MovementKind, id: i64) -> Result<(), CoreError> {
        let doc_kind = if kind == MovementKind::Receipt {
            &s.receipt
        } else {
            &s.issue
        };
        post(&mut s.db, doc_kind, &s.stock, id, rule(kind))
    }

    fn balance(s: &mut Setup, item: i64) -> Decimal {
        let dims = fields(&[("store", text("main")), ("item", FieldValue::Int(item))]);
        s.stock.balance(&mut s.db, &dims).unwrap()["qty"]
    }

    fn q(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn receipt_then_issue() {
        let mut s = setup();
        let r = new_doc(
            &mut s,
            MovementKind::Receipt,
            &[(1, "10"), (1, "5.5"), (2, "1")],
        );
        post_doc(&mut s, MovementKind::Receipt, r).unwrap();
        assert_eq!(balance(&mut s, 1), q("15.5"));
        assert_eq!(balance(&mut s, 2), q("1"));

        let i = new_doc(&mut s, MovementKind::Issue, &[(1, "3.25")]);
        post_doc(&mut s, MovementKind::Issue, i).unwrap();
        assert_eq!(balance(&mut s, 1), q("12.25"));
        assert!(s.issue.get(&mut s.db, i).unwrap().unwrap().posted);
        assert_eq!(balance(&mut s, 3), q("0"));
    }

    #[test]
    fn issue_beyond_balance_changes_nothing() {
        let mut s = setup();
        let r = new_doc(&mut s, MovementKind::Receipt, &[(1, "2")]);
        post_doc(&mut s, MovementKind::Receipt, r).unwrap();

        let i = new_doc(&mut s, MovementKind::Issue, &[(1, "1"), (1, "1.5")]);
        let err = post_doc(&mut s, MovementKind::Issue, i).unwrap_err();
        assert!(matches!(err, CoreError::NegativeBalance { .. }), "{err}");
        assert_eq!(balance(&mut s, 1), q("2"));
        assert!(!s.issue.get(&mut s.db, i).unwrap().unwrap().posted);
    }

    #[test]
    fn repost_replaces_movements() {
        let mut s = setup();
        let r = new_doc(&mut s, MovementKind::Receipt, &[(1, "10")]);
        post_doc(&mut s, MovementKind::Receipt, r).unwrap();
        post_doc(&mut s, MovementKind::Receipt, r).unwrap();
        assert_eq!(balance(&mut s, 1), q("10"));
    }

    #[test]
    fn unpost_respects_balance() {
        let mut s = setup();
        let r = new_doc(&mut s, MovementKind::Receipt, &[(1, "10")]);
        post_doc(&mut s, MovementKind::Receipt, r).unwrap();
        let i = new_doc(&mut s, MovementKind::Issue, &[(1, "4")]);
        post_doc(&mut s, MovementKind::Issue, i).unwrap();

        // Отменить приход нельзя: по нему уже списали.
        let err = unpost(&mut s.db, &s.receipt, &s.stock, r).unwrap_err();
        assert!(matches!(err, CoreError::NegativeBalance { .. }));
        assert_eq!(balance(&mut s, 1), q("6"));

        unpost(&mut s.db, &s.issue, &s.stock, i).unwrap();
        unpost(&mut s.db, &s.receipt, &s.stock, r).unwrap();
        assert_eq!(balance(&mut s, 1), q("0"));
        assert!(!s.receipt.get(&mut s.db, r).unwrap().unwrap().posted);
    }

    #[test]
    fn failing_rule_rolls_back() {
        let mut s = setup();
        let r = new_doc(&mut s, MovementKind::Receipt, &[(1, "10")]);
        let err = post(&mut s.db, &s.receipt, &s.stock, r, |_| {
            Err(CoreError::Invalid("ошибка в on_post".into()))
        });
        assert!(err.is_err());
        assert_eq!(balance(&mut s, 1), q("0"));
        assert!(!s.receipt.get(&mut s.db, r).unwrap().unwrap().posted);
    }

    #[test]
    fn document_keeps_lines_in_order() {
        let mut s = setup();
        let r = new_doc(
            &mut s,
            MovementKind::Receipt,
            &[(3, "1"), (1, "2"), (2, "3")],
        );
        let doc = s.receipt.get(&mut s.db, r).unwrap().unwrap();
        let items: Vec<_> = doc.parts["lines"]
            .iter()
            .map(|l| l["item"].clone())
            .collect();
        assert_eq!(
            items,
            vec![FieldValue::Int(3), FieldValue::Int(1), FieldValue::Int(2)]
        );
        assert_eq!(doc.version, 1);
        assert!(!doc.posted);
    }
}
