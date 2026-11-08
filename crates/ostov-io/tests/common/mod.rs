//! Общее для тестов: локальная база «Склада» как `&dyn Engine` и чтение xlsx через calamine.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use calamine::{Data, Reader, Xlsx, open_workbook};
use ostov_api::{Connector, Engine, Fields, ObjectData, PostAction, Value};
use ostov_core::LocalConnector;

/// Временная папка с базой; удаляется после теста.
pub struct Base {
    pub dir: PathBuf,
}

impl Base {
    pub fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ostov-io-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    pub fn engine(&self) -> Arc<dyn Engine> {
        self.engine_with(&LocalConnector::new(model()))
    }

    pub fn engine_with(&self, connector: &LocalConnector) -> Arc<dyn Engine> {
        let path = self.dir.join("shop.db").to_string_lossy().into_owned();
        connector.create_local(&path, None, None).unwrap()
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// «Склад» из настоящего `config/`: с обязательными полями и ролями.
pub fn model() -> ostov_model::Config {
    ostov_meta::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"))
        .unwrap()
}

pub fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

pub fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

pub fn refer(id: i64) -> Value {
    Value::Ref { id, title: None }
}

pub fn dec(units: i64, scale: u8) -> Value {
    Value::Decimal { units, scale }
}

pub fn add(engine: &dyn Engine, object: &str, values: &[(&str, Value)]) -> i64 {
    let data = ObjectData {
        fields: fields(values),
        ..ObjectData::default()
    };
    engine.save(object, &data, PostAction::Write).unwrap().id
}

/// Строки: (товар, количество в тысячных, цена в копейках).
pub fn receipt(day: &str, warehouse: i64, supplier: i64, lines: &[(i64, i64, i64)]) -> ObjectData {
    let lines = lines
        .iter()
        .map(|&(goods, qty, price)| {
            fields(&[
                ("goods", refer(goods)),
                ("qty", dec(qty, 3)),
                ("price", dec(price, 2)),
            ])
        })
        .collect();
    ObjectData {
        fields: fields(&[
            ("date", Value::Date(day.into())),
            ("warehouse", refer(warehouse)),
            ("supplier", refer(supplier)),
        ]),
        tables: [("lines".to_owned(), lines)].into(),
        ..ObjectData::default()
    }
}

/// Первый лист xlsx: имя и строки ячеек.
pub fn read_sheet(path: &std::path::Path) -> (String, Vec<Vec<Data>>) {
    let mut book: Xlsx<_> = open_workbook(path).unwrap();
    let name = book.sheet_names()[0].clone();
    let range = book.worksheet_range(&name).unwrap();
    (name, range.rows().map(<[Data]>::to_vec).collect())
}

pub fn s(text: &str) -> Data {
    Data::String(text.to_owned())
}
