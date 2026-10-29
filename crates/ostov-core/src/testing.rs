use ostov_store::drivers::sqlite::SqliteDriver;

use crate::{FieldValue, Fields};

pub fn memory_db() -> SqliteDriver {
    SqliteDriver::open_in_memory().unwrap()
}

pub fn fields(pairs: &[(&str, FieldValue)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

pub fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.to_owned())
}

pub fn dec(s: &str) -> FieldValue {
    FieldValue::Decimal(s.parse().unwrap())
}
