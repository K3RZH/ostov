//! Конфигурация для `LocalEngine`: объекты ядра, построенные из модели `ostov-model`,
//! и правила проведения документов (`PostingRule`), которые даёт `Rules`.

use std::collections::BTreeMap;

use ostov_model::{
    CatalogDef, DocumentDef, MenuItem, Meta, RegisterDef, RegisterKind, TablePartDef,
};
use ostov_store::{Decimal, Driver};

pub use crate::posting::Parts;
use crate::{
    AccumulationRegister, Catalog, CoreError, Document, DocumentKind, FieldDef, FieldKind,
    FieldValue, Fields, Movement, MovementKind, Moves, PostingRule, TablePart,
};

pub struct DocumentConfig {
    pub kind: DocumentKind,
    /// Регистры, по которым документ делает движения.
    pub registers: Vec<String>,
    pub rule: Box<dyn PostingRule>,
}

pub struct Config {
    /// Роли для прав (ostov-sec); встроенная `admin` сюда не входит.
    pub roles: Vec<ostov_model::RoleDef>,
    /// Что интерфейс получает по `GetMeta`: объекты с заголовками, меню, права.
    pub meta: Meta,
    /// Название для людей (`title` манифеста): его показывает `hello()`.
    pub name: String,
    pub version: String,
    pub catalogs: BTreeMap<String, Catalog>,
    pub documents: BTreeMap<String, DocumentConfig>,
    pub registers: BTreeMap<String, AccumulationRegister>,
}

/// Откуда брать правила документов. Встроенные — `BuiltinRules`, потом Python (`ostov-py`).
pub trait Rules {
    /// Правило документа или `None`, если для него ничего нет.
    fn rule(&self, document: &DocumentDef) -> Option<Box<dyn PostingRule>>;
}

impl Config {
    pub fn create_tables(&self, db: &mut dyn Driver) -> Result<(), CoreError> {
        for catalog in self.catalogs.values() {
            catalog.create_table(db)?;
        }
        for doc in self.documents.values() {
            doc.kind.create_tables(db)?;
        }
        for register in self.registers.values() {
            register.create_tables(db)?;
        }
        Ok(())
    }

    /// Регистры, которые двигает документ.
    pub fn registers_of(&self, document: &DocumentConfig) -> Vec<&AccumulationRegister> {
        document
            .registers
            .iter()
            .filter_map(|name| self.registers.get(name))
            .collect()
    }

    /// Объекты ядра из модели со встроенными правилами «Склада».
    pub fn from_model(model: &ostov_model::Config) -> Result<Self, CoreError> {
        Self::from_model_with(model, &BuiltinRules)
    }

    /// Объекты ядра из модели конфигурации (её читает `ostov-meta`) с правилами из `rules`.
    /// Документу с регистрами правило обязательно; без регистров — по желанию (пересчёт сумм).
    pub fn from_model_with(
        model: &ostov_model::Config,
        rules: &dyn Rules,
    ) -> Result<Self, CoreError> {
        let catalogs = model
            .catalogs
            .iter()
            .map(|c| Ok((c.name.clone(), Catalog::new(&c.name, c.fields.clone())?)))
            .collect::<Result<BTreeMap<_, _>, CoreError>>()?;

        let mut registers = BTreeMap::new();
        for r in &model.registers {
            match r.kind {
                RegisterKind::Accumulation => {
                    let register = AccumulationRegister::new(
                        &r.name,
                        r.dimensions.clone(),
                        r.resources.clone(),
                    )?;
                    registers.insert(r.name.clone(), register);
                }
            }
        }

        let mut documents = BTreeMap::new();
        for d in &model.documents {
            if let Some(missing) = d.registers.iter().find(|r| !registers.contains_key(*r)) {
                return Err(CoreError::Invalid(format!(
                    "документ {} двигает регистр {missing}, а его нет в конфигурации",
                    d.name
                )));
            }
            let parts = d
                .table_parts
                .iter()
                .map(|p| TablePart::new(&p.name, p.fields.clone()))
                .collect::<Result<_, _>>()?;
            let kind = DocumentKind::new(&d.name, d.fields.clone(), parts)?;
            let rule = match rules.rule(d) {
                Some(rule) => rule,
                None if d.registers.is_empty() => Box::new(NoMovements),
                None => {
                    return Err(CoreError::Invalid(format!(
                        "для документа {} нет правила проведения",
                        d.name
                    )));
                }
            };
            documents.insert(
                d.name.clone(),
                DocumentConfig {
                    kind,
                    registers: d.registers.clone(),
                    rule,
                },
            );
        }

        Ok(Self {
            roles: model.roles.clone(),
            meta: Meta::new(model),
            name: model.title.clone(),
            version: model.version.clone(),
            catalogs,
            documents,
            registers,
        })
    }
}

const STOCK: &str = "stock";

struct NoMovements;

impl PostingRule for NoMovements {
    fn movements(&self, _: &Document) -> Result<Moves, CoreError> {
        Ok(Moves::new())
    }
}

/// Правила «Склада» на Rust, пока их не пишут на Python (`on_post`): поступление и расход
/// по регистру `stock`.
pub struct BuiltinRules;

impl Rules for BuiltinRules {
    fn rule(&self, document: &DocumentDef) -> Option<Box<dyn PostingRule>> {
        if document.registers != [STOCK] {
            return None;
        }
        match document.name.as_str() {
            "receipt" => Some(Box::new(StockRule {
                kind: MovementKind::Receipt,
                totals: true,
            })),
            "issue" => Some(Box::new(StockRule {
                kind: MovementKind::Issue,
                totals: false,
            })),
            _ => None,
        }
    }
}

struct StockRule {
    kind: MovementKind,
    /// Считать суммы строк и документа.
    totals: bool,
}

impl PostingRule for StockRule {
    fn movements(&self, doc: &Document) -> Result<Moves, CoreError> {
        Ok(Moves::from([(
            STOCK.to_owned(),
            movements(doc, self.kind)?,
        )]))
    }

    fn prepare(&self, header: &mut Fields, parts: &mut Parts) -> Result<(), CoreError> {
        if self.totals {
            receipt_totals(header, parts)?;
        }
        Ok(())
    }
}

/// Конфигурация «Склад» по умолчанию — та же, что в `config/` (тест в `ostov-meta` сверяет).
pub fn warehouse() -> Result<Config, CoreError> {
    Config::from_model(&warehouse_model())
}

/// Модель «Склада» в коде: для `LocalConnector` без загруженной конфигурации.
pub fn warehouse_model() -> ostov_model::Config {
    let field = |name: &str, title: &str, kind| FieldDef::new(name, kind).with_title(title);
    let reference =
        |name: &str, title: &str, target: &str| FieldDef::reference(name, target).with_title(title);
    let name = || field("name", "Наименование", FieldKind::Text);
    let date = || field("date", "Дата", FieldKind::Date);
    let qty = || field("qty", "Количество", FieldKind::Decimal { scale: 3 });
    let warehouse = || reference("warehouse", "Склад", "warehouses");
    let goods = || reference("goods", "Товар", "goods");
    // Сумму считает ядро.
    let total = || FieldDef {
        read_only: true,
        ..field("amount", "Сумма", FieldKind::Decimal { scale: 2 })
    };
    let catalog = |name: &str, title: &str, icon: &str, fields| CatalogDef {
        name: name.to_owned(),
        title: title.to_owned(),
        icon: Some(icon.to_owned()),
        fields,
    };
    let lines = |fields| TablePartDef {
        name: "lines".to_owned(),
        title: "Товары".to_owned(),
        fields,
    };
    let document = |name: &str, title: &str, icon: &str, fields, part| DocumentDef {
        name: name.to_owned(),
        title: title.to_owned(),
        icon: Some(icon.to_owned()),
        fields,
        table_parts: vec![part],
        registers: vec![STOCK.to_owned()],
    };
    let item = |object: &str| MenuItem {
        title: object.to_owned(),
        icon: None,
        object: Some(object.to_owned()),
        children: Vec::new(),
    };

    ostov_model::Config {
        name: "warehouse".to_owned(),
        title: "Склад".to_owned(),
        version: "0.1.0".to_owned(),
        catalogs: vec![
            catalog(
                "goods",
                "Товары",
                "box",
                vec![name(), field("unit", "Единица измерения", FieldKind::Text)],
            ),
            catalog("suppliers", "Поставщики", "users", vec![name()]),
            catalog("warehouses", "Склады", "warehouse", vec![name()]),
        ],
        documents: vec![
            document(
                "issue",
                "Расход",
                "stock-out",
                vec![date(), warehouse()],
                lines(vec![goods(), qty()]),
            ),
            document(
                "receipt",
                "Поступление",
                "stock-in",
                vec![
                    date(),
                    reference("supplier", "Поставщик", "suppliers"),
                    warehouse(),
                    total(),
                ],
                lines(vec![
                    goods(),
                    qty(),
                    field("price", "Цена", FieldKind::Decimal { scale: 2 }),
                    total(),
                ]),
            ),
        ],
        registers: vec![RegisterDef {
            name: STOCK.to_owned(),
            title: "Остатки товаров".to_owned(),
            kind: RegisterKind::Accumulation,
            dimensions: vec![warehouse(), goods()],
            resources: vec![qty()],
        }],
        menu: vec![MenuItem {
            title: "Склад".to_owned(),
            icon: Some("warehouse".to_owned()),
            object: None,
            children: ["goods", "warehouses", "suppliers", "receipt", "issue"]
                .map(item)
                .into(),
        }],
        roles: Vec::new(),
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
