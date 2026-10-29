//! Временная конфигурация «Склад» под экран поступлений Артёма, пока нет `ostov-meta`.
//! Когда появятся метаданные, всё это будет читаться из файлов конфигурации.

use std::collections::BTreeMap;

use ostov_store::{Decimal, Driver};

use crate::{
    AccumulationRegister, Catalog, CoreError, Document, DocumentKind, FieldDef, FieldKind,
    FieldValue, Fields, Movement, MovementKind, TablePart,
};

pub type Parts = BTreeMap<String, Vec<Fields>>;

pub struct DocumentConfig {
    pub kind: DocumentKind,
    pub rule: fn(&Document) -> Result<Vec<Movement>, CoreError>,
    /// Пересчёт перед записью: суммы считает ядро, а не интерфейс.
    pub prepare: fn(&mut Fields, &mut Parts) -> Result<(), CoreError>,
}

pub struct Config {
    pub name: String,
    pub version: String,
    pub catalogs: BTreeMap<String, Catalog>,
    pub documents: BTreeMap<String, DocumentConfig>,
    pub stock: AccumulationRegister,
}

impl Config {
    pub fn create_tables(&self, db: &mut dyn Driver) -> Result<(), CoreError> {
        for catalog in self.catalogs.values() {
            catalog.create_table(db)?;
        }
        for doc in self.documents.values() {
            doc.kind.create_tables(db)?;
        }
        self.stock.create_tables(db)
    }
}

pub fn warehouse() -> Result<Config, CoreError> {
    let name = || FieldDef::new("name", FieldKind::Text);
    let date = || FieldDef::new("date", FieldKind::Date);
    let qty = || FieldDef::new("qty", FieldKind::Decimal { scale: 3 });
    let money = |n: &str| FieldDef::new(n, FieldKind::Decimal { scale: 2 });

    let catalogs = [
        Catalog::new(
            "goods",
            vec![name(), FieldDef::new("unit", FieldKind::Text)],
        )?,
        Catalog::new("warehouses", vec![name()])?,
        Catalog::new("suppliers", vec![name()])?,
    ];

    let receipt = DocumentKind::new(
        "receipt",
        vec![
            date(),
            FieldDef::reference("supplier", "suppliers"),
            FieldDef::reference("warehouse", "warehouses"),
            money("amount"),
        ],
        vec![TablePart::new(
            "lines",
            vec![
                FieldDef::reference("goods", "goods"),
                qty(),
                money("price"),
                money("amount"),
            ],
        )?],
    )?;
    let issue = DocumentKind::new(
        "issue",
        vec![date(), FieldDef::reference("warehouse", "warehouses")],
        vec![TablePart::new(
            "lines",
            vec![FieldDef::reference("goods", "goods"), qty()],
        )?],
    )?;
    let stock = AccumulationRegister::new(
        "stock",
        vec![
            FieldDef::reference("warehouse", "warehouses"),
            FieldDef::reference("goods", "goods"),
        ],
        vec![qty()],
    )?;

    let documents = [
        DocumentConfig {
            kind: receipt,
            rule: |doc| movements(doc, MovementKind::Receipt),
            prepare: receipt_totals,
        },
        DocumentConfig {
            kind: issue,
            rule: |doc| movements(doc, MovementKind::Issue),
            prepare: |_, _| Ok(()),
        },
    ];

    Ok(Config {
        name: "Склад (временная конфигурация)".to_owned(),
        version: "0.1".to_owned(),
        catalogs: catalogs
            .into_iter()
            .map(|c| (c.table().to_owned(), c))
            .collect(),
        documents: documents
            .into_iter()
            .map(|d| (d.kind.table().to_owned(), d))
            .collect(),
        stock,
    })
}

/// Сумма строки = округление(количество × цена) до копеек, сумма документа — их итог (СТ-К15).
fn receipt_totals(header: &mut Fields, parts: &mut Parts) -> Result<(), CoreError> {
    let mut total = Decimal::zero(2)?;
    for line in parts.get_mut("lines").into_iter().flatten() {
        let (Some(FieldValue::Decimal(qty)), Some(FieldValue::Decimal(price))) =
            (line.get("qty"), line.get("price"))
        else {
            line.insert("amount".into(), FieldValue::Null);
            continue;
        };
        let amount = qty.checked_mul(*price, 2)?;
        total = total.checked_add(amount)?;
        line.insert("amount".into(), FieldValue::Decimal(amount));
    }
    header.insert("amount".into(), FieldValue::Decimal(total));
    Ok(())
}

/// Каждая строка — движение по складу документа и товару строки.
fn movements(doc: &Document, kind: MovementKind) -> Result<Vec<Movement>, CoreError> {
    let warehouse = doc
        .header
        .get("warehouse")
        .cloned()
        .unwrap_or(FieldValue::Null);
    doc.parts
        .get("lines")
        .into_iter()
        .flatten()
        .map(|line| {
            let qty = match line.get("qty") {
                Some(FieldValue::Decimal(q)) => *q,
                _ => return Err(CoreError::Invalid("в строке не указано количество".into())),
            };
            Ok(Movement {
                kind,
                dims: Fields::from([
                    ("warehouse".to_owned(), warehouse.clone()),
                    (
                        "goods".to_owned(),
                        line.get("goods").cloned().unwrap_or(FieldValue::Null),
                    ),
                ]),
                resources: BTreeMap::from([("qty".to_owned(), qty)]),
            })
        })
        .collect()
}
