//! Демо-база для разработки интерфейса: те же товары, склады и поставщики, что в демо-данных
//! `ostov-ui`, и пачка поступлений, чтобы было что листать.

use std::sync::Arc;

use ostov_api::{Engine, EngineResult};
use ostov_store::{Decimal, Driver};

use super::config::{Config, DocumentConfig, Parts};
use super::convert::error;
use super::{LocalConnector, LocalEngine, create_base};
use crate::object::set_flag;
use crate::{CoreError, Document, FieldValue, Fields, in_transaction};

const GOODS: [(&str, &str); 8] = [
    ("Мука пшеничная в/с", "кг"),
    ("Говядина охлаждённая", "кг"),
    ("Молоко 3,2%", "л"),
    ("Сахар-песок", "кг"),
    ("Масло сливочное 82%", "кг"),
    ("Яйцо куриное С1", "шт"),
    ("Картофель", "кг"),
    ("Томаты", "кг"),
];
const SUPPLIERS: [&str; 6] = [
    "Мясной двор",
    "Овощи оптом",
    "Молочный комбинат",
    "Бакалея-Центр",
    "Рыбный ряд",
    "Пекарня №3",
];
const WAREHOUSES: [&str; 3] = ["Основной", "Кухня", "Бар"];

impl LocalConnector {
    /// Новая база с примерами: справочники и `receipts` поступлений за 2026 год —
    /// почти все проведены, каждое десятое нет, каждое тридцатое помечено на удаление;
    /// по части поступлений есть расходы.
    pub fn create_demo(&self, path: &str, receipts: usize) -> EngineResult<Arc<dyn Engine>> {
        let (mut db, config, lock) = create_base(path, self.config()?)?;
        in_transaction(&mut db, |db| fill(db, &config, receipts)).map_err(error)?;
        Ok(Arc::new(
            LocalEngine::new(Box::new(db), config).locked(lock),
        ))
    }
}

fn fill(db: &mut dyn Driver, config: &Config, receipts: usize) -> Result<(), CoreError> {
    let goods = catalog(
        db,
        config,
        "goods",
        GOODS.iter().map(|(n, u)| vec![("name", *n), ("unit", *u)]),
    )?;
    let warehouses = catalog(
        db,
        config,
        "warehouses",
        WAREHOUSES.iter().map(|n| vec![("name", *n)]),
    )?;
    let suppliers = catalog(
        db,
        config,
        "suppliers",
        SUPPLIERS.iter().map(|n| vec![("name", *n)]),
    )?;
    let receipt = document(config, "receipt")?;
    let issue = document(config, "issue")?;

    let mut rng = Rng(42);
    for n in 0..receipts {
        let date = date(n, receipts);
        let warehouse = rng.pick(&warehouses);
        let mut header = Fields::from([
            ("date".to_owned(), FieldValue::Date(date.clone())),
            ("warehouse".to_owned(), FieldValue::Ref(warehouse)),
            ("supplier".to_owned(), FieldValue::Ref(rng.pick(&suppliers))),
        ]);
        let lines: Vec<Fields> = (0..1 + rng.below(4))
            .map(|_| {
                Ok(Fields::from([
                    ("goods".to_owned(), FieldValue::Ref(rng.pick(&goods))),
                    (
                        "qty".to_owned(),
                        FieldValue::Decimal(Decimal::new(500 * (1 + rng.below(100)), 3)?),
                    ),
                    (
                        "price".to_owned(),
                        FieldValue::Decimal(Decimal::new(100 * (30 + rng.below(870)), 2)?),
                    ),
                ]))
            })
            .collect::<Result<_, CoreError>>()?;
        let mut parts = Parts::from([("lines".to_owned(), lines)]);
        (receipt.prepare)(&mut header, &mut parts)?;
        let doc = receipt.kind.insert_in(db, &header, &parts)?;

        if n % 30 == 29 {
            set_flag(
                db,
                receipt.kind.table(),
                "deleted",
                true,
                doc.id,
                doc.version,
            )?;
        } else if n % 10 != 9 {
            post(db, config, receipt, &doc)?;
            if n % 7 == 0 {
                // Расход половины первой строки с того же склада: остатка точно хватит.
                let first = doc
                    .parts
                    .get("lines")
                    .and_then(|l| l.first())
                    .cloned()
                    .unwrap_or_default();
                let half = match first.get("qty") {
                    Some(FieldValue::Decimal(q)) => Decimal::new(q.units() / 2, 3)?,
                    _ => continue,
                };
                let header = Fields::from([
                    ("date".to_owned(), FieldValue::Date(date)),
                    ("warehouse".to_owned(), FieldValue::Ref(warehouse)),
                ]);
                let line = Fields::from([
                    (
                        "goods".to_owned(),
                        first.get("goods").cloned().unwrap_or(FieldValue::Null),
                    ),
                    ("qty".to_owned(), FieldValue::Decimal(half)),
                ]);
                let out = issue.kind.insert_in(
                    db,
                    &header,
                    &Parts::from([("lines".to_owned(), vec![line])]),
                )?;
                post(db, config, issue, &out)?;
            }
        }
    }
    Ok(())
}

fn catalog<'a>(
    db: &mut dyn Driver,
    config: &Config,
    name: &str,
    items: impl Iterator<Item = Vec<(&'a str, &'a str)>>,
) -> Result<Vec<i64>, CoreError> {
    let catalog = config
        .catalogs
        .get(name)
        .ok_or_else(|| CoreError::Invalid(format!("в конфигурации нет справочника {name}")))?;
    items
        .map(|pairs| {
            let fields = pairs
                .into_iter()
                .map(|(k, v)| (k.to_owned(), FieldValue::Text(v.to_owned())))
                .collect();
            Ok(catalog.insert(db, &fields)?.id)
        })
        .collect()
}

fn document<'a>(config: &'a Config, name: &str) -> Result<&'a DocumentConfig, CoreError> {
    config
        .documents
        .get(name)
        .ok_or_else(|| CoreError::Invalid(format!("в конфигурации нет документа {name}")))
}

fn post(
    db: &mut dyn Driver,
    config: &Config,
    cfg: &DocumentConfig,
    doc: &Document,
) -> Result<(), CoreError> {
    let movements = (cfg.rule)(doc)?;
    config.stock.replace_dated(
        db,
        cfg.kind.table(),
        doc.id,
        crate::posting::period(doc).as_deref(),
        &movements,
    )?;
    cfg.kind.set_posted(db, doc.id, true)
}

/// Даты равномерно с 1 января по 30 октября 2026.
fn date(n: usize, total: usize) -> String {
    const DAYS: [u32; 10] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31];
    let span: usize = 302;
    let mut day = u32::try_from(n * span / total.max(1)).unwrap_or(0);
    let mut month = 1;
    for len in DAYS {
        if day < len {
            break;
        }
        day -= len;
        month += 1;
    }
    format!("2026-{month:02}-{:02}", day + 1)
}

/// Простой генератор: одинаковые данные при каждом создании базы.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: i64) -> i64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let n = u64::try_from(n.max(1)).unwrap_or(1);
        i64::try_from((self.0 >> 33) % n).unwrap_or(0)
    }

    fn pick(&mut self, ids: &[i64]) -> i64 {
        let n = i64::try_from(ids.len()).unwrap_or(1);
        let i = usize::try_from(self.below(n)).unwrap_or(0);
        ids.get(i).copied().unwrap_or(0)
    }
}
