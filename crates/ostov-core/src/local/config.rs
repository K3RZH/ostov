//! Конфигурация для `LocalEngine`: объекты ядра, построенные из модели `ostov-model`,
//! и правила проведения, которые пока написаны в коде.

use std::collections::BTreeMap;

use ostov_model::{CatalogDef, DocumentDef, RegisterDef, RegisterKind, TablePartDef};
use ostov_store::{Decimal, Driver};

use crate::{
    AccumulationRegister, Catalog, CoreError, Document, DocumentKind, FieldDef, FieldKind,
    FieldValue, Fields, Movement, MovementKind, TablePart,
};

pub type Parts = BTreeMap<String, Vec<Fields>>;

pub struct DocumentConfig {
    pub kind: DocumentKind,
    pub rule: Rule,
    /// Пересчёт перед записью: суммы считает ядро, а не интерфейс.
    pub prepare: Prepare,
}

pub struct Config {
    /// Название для людей (`title` манифеста): его показывает `hello()`.
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

impl Config {
    /// Объекты ядра из модели конфигурации (её читает `ostov-meta`).
    /// Логика проведения пока в коде: правило ищется по имени документа.
    /// Регистр пока один — `stock`: под него написаны правила и отчёты.
    pub fn from_model(model: &ostov_model::Config) -> Result<Self, CoreError> {
        let catalogs = model
            .catalogs
            .iter()
            .map(|c| Ok((c.name.clone(), Catalog::new(&c.name, c.fields.clone())?)))
            .collect::<Result<BTreeMap<_, _>, CoreError>>()?;

        let stock = match model.registers.as_slice() {
            [r] if r.name == STOCK && r.kind == RegisterKind::Accumulation => {
                AccumulationRegister::new(&r.name, r.dimensions.clone(), r.resources.clone())?
            }
            _ => {
                return Err(CoreError::Invalid(format!(
                    "пока поддерживается ровно один регистр накопления {STOCK}"
                )));
            }
        };

        let mut documents = BTreeMap::new();
        for d in &model.documents {
            if let Some(other) = d.registers.iter().find(|r| *r != STOCK) {
                return Err(CoreError::Invalid(format!(
                    "документ {} двигает регистр {other}, а пока есть только {STOCK}",
                    d.name
                )));
            }
            let parts = d
                .table_parts
                .iter()
                .map(|p| TablePart::new(&p.name, p.fields.clone()))
                .collect::<Result<_, _>>()?;
            let kind = DocumentKind::new(&d.name, d.fields.clone(), parts)?;
            let (rule, prepare) = if d.registers.is_empty() {
                (no_movements as Rule, no_prepare as Prepare)
            } else {
                handlers(&d.name).ok_or_else(|| {
                    CoreError::Invalid(format!("для документа {} нет правила проведения", d.name))
                })?
            };
            documents.insert(
                d.name.clone(),
                DocumentConfig {
                    kind,
                    rule,
                    prepare,
                },
            );
        }

        Ok(Self {
            name: model.title.clone(),
            version: model.version.clone(),
            catalogs,
            documents,
            stock,
        })
    }
}

const STOCK: &str = "stock";

type Rule = fn(&Document) -> Result<Vec<Movement>, CoreError>;
type Prepare = fn(&mut Fields, &mut Parts) -> Result<(), CoreError>;

/// Правила проведения, пока их не пишут на Python (`on_post`).
fn handlers(document: &str) -> Option<(Rule, Prepare)> {
    match document {
        "receipt" => Some((|doc| movements(doc, MovementKind::Receipt), receipt_totals)),
        "issue" => Some((|doc| movements(doc, MovementKind::Issue), no_prepare)),
        _ => None,
    }
}

fn no_movements(_: &Document) -> Result<Vec<Movement>, CoreError> {
    Ok(Vec::new())
}

fn no_prepare(_: &mut Fields, _: &mut Parts) -> Result<(), CoreError> {
    Ok(())
}

/// Конфигурация «Склад» по умолчанию — та же, что в `config/` (тест в `ostov-meta` сверяет).
pub fn warehouse() -> Result<Config, CoreError> {
    Config::from_model(&warehouse_model())
}

/// Модель «Склада» в коде: для `LocalConnector` без загруженной конфигурации.
pub fn warehouse_model() -> ostov_model::Config {
    let name = || FieldDef::new("name", FieldKind::Text);
    let date = || FieldDef::new("date", FieldKind::Date);
    let qty = || FieldDef::new("qty", FieldKind::Decimal { scale: 3 });
    let money = |n: &str| FieldDef::new(n, FieldKind::Decimal { scale: 2 });
    let catalog = |name: &str, fields| CatalogDef {
        name: name.to_owned(),
        title: name.to_owned(),
        fields,
    };
    let lines = |fields| TablePartDef {
        name: "lines".to_owned(),
        title: "lines".to_owned(),
        fields,
    };
    let document = |name: &str, fields, part| DocumentDef {
        name: name.to_owned(),
        title: name.to_owned(),
        fields,
        table_parts: vec![part],
        registers: vec![STOCK.to_owned()],
    };

    ostov_model::Config {
        name: "warehouse".to_owned(),
        title: "Склад".to_owned(),
        version: "0.1.0".to_owned(),
        catalogs: vec![
            catalog(
                "goods",
                vec![name(), FieldDef::new("unit", FieldKind::Text)],
            ),
            catalog("suppliers", vec![name()]),
            catalog("warehouses", vec![name()]),
        ],
        documents: vec![
            document(
                "issue",
                vec![date(), FieldDef::reference("warehouse", "warehouses")],
                lines(vec![FieldDef::reference("goods", "goods"), qty()]),
            ),
            document(
                "receipt",
                vec![
                    date(),
                    FieldDef::reference("supplier", "suppliers"),
                    FieldDef::reference("warehouse", "warehouses"),
                    money("amount"),
                ],
                lines(vec![
                    FieldDef::reference("goods", "goods"),
                    qty(),
                    money("price"),
                    money("amount"),
                ]),
            ),
        ],
        registers: vec![RegisterDef {
            name: STOCK.to_owned(),
            title: STOCK.to_owned(),
            kind: RegisterKind::Accumulation,
            dimensions: vec![
                FieldDef::reference("warehouse", "warehouses"),
                FieldDef::reference("goods", "goods"),
            ],
            resources: vec![qty()],
        }],
    }
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
