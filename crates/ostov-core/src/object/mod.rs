//! Объекты: справочники и документы, общее описание полей.

use std::collections::{BTreeMap, BTreeSet};

use ostov_store::{Decimal, Dialect, Driver, Value};

use crate::CoreError;

pub mod catalog;
pub mod document;

pub type Fields = BTreeMap<String, FieldValue>;

/// Описания полей общие с `ostov-meta`, живут в `ostov-model`.
pub use ostov_model::{FieldDef, FieldKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Null,
    Int(i64),
    Text(String),
    Decimal(Decimal),
    Date(String),
    Ref(i64),
}

// Имена таблиц и колонок — только ASCII в нижнем регистре (ADR-025), кириллица живёт в метаданных.
pub(crate) fn check_ident(name: &str) -> Result<(), CoreError> {
    let mut chars = name.chars();
    let ok = name.len() <= 63
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(CoreError::Invalid(format!(
            "недопустимое имя «{name}»: нужны латинские буквы, цифры и _"
        )))
    }
}

pub(crate) fn check_fields(fields: &[FieldDef], reserved: &[&str]) -> Result<(), CoreError> {
    let mut seen = BTreeSet::new();
    for field in fields {
        check_ident(&field.name)?;
        if reserved.contains(&field.name.as_str()) {
            return Err(CoreError::Invalid(format!(
                "имя поля {} занято платформой",
                field.name
            )));
        }
        if !seen.insert(field.name.as_str()) {
            return Err(CoreError::Invalid(format!(
                "поле {} описано дважды",
                field.name
            )));
        }
        if let FieldKind::Decimal { scale } = field.kind {
            Decimal::zero(scale)?;
        }
    }
    Ok(())
}

pub(crate) fn column_defs(d: &dyn Dialect, fields: &[FieldDef]) -> Vec<String> {
    fields
        .iter()
        .map(|f| {
            let ty = match f.kind {
                FieldKind::Int | FieldKind::Decimal { .. } | FieldKind::Ref => "BIGINT",
                FieldKind::Text | FieldKind::Date => "TEXT",
            };
            format!("{} {ty}", d.quote_ident(&f.name))
        })
        .collect()
}

fn is_empty(value: &FieldValue) -> bool {
    match value {
        FieldValue::Null => true,
        FieldValue::Text(s) => s.trim().is_empty(),
        _ => false,
    }
}

pub(crate) fn names(d: &dyn Dialect, fields: &[FieldDef]) -> Vec<String> {
    fields.iter().map(|f| d.quote_ident(&f.name)).collect()
}

pub(crate) fn placeholders(d: &dyn Dialect, from: usize, count: usize) -> String {
    (from..from + count)
        .map(|n| d.placeholder(n))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Значения полей в порядке описания; отсутствующие — NULL, лишние — ошибка.
pub(crate) fn to_values(
    table: &str,
    defs: &[FieldDef],
    fields: &Fields,
) -> Result<Vec<Value>, CoreError> {
    if let Some(unknown) = fields.keys().find(|k| !defs.iter().any(|f| &f.name == *k)) {
        return Err(CoreError::Invalid(format!("в {table} нет поля {unknown}")));
    }
    defs.iter()
        .map(|def| {
            let value = fields.get(&def.name).unwrap_or(&FieldValue::Null);
            if def.required && is_empty(value) {
                return Err(CoreError::Invalid(format!(
                    "не заполнено обязательное поле «{}»",
                    def.title
                )));
            }
            to_value(def, value)
        })
        .collect()
}

pub(crate) fn to_value(def: &FieldDef, value: &FieldValue) -> Result<Value, CoreError> {
    Ok(match (def.kind, value) {
        (_, FieldValue::Null) => Value::Null,
        (FieldKind::Int, FieldValue::Int(v)) => Value::Int(*v),
        (FieldKind::Text, FieldValue::Text(v)) => Value::Text(v.clone()),
        (FieldKind::Decimal { scale }, FieldValue::Decimal(v)) => {
            Value::Int(to_scale(def, *v, scale)?.units())
        }
        (FieldKind::Ref, FieldValue::Ref(id)) => Value::Int(*id),
        (FieldKind::Date, FieldValue::Date(d)) if is_date(d) => Value::Text(d.clone()),
        _ => {
            return Err(CoreError::Invalid(format!(
                "поле {}: ожидается {}",
                def.name, def.kind
            )));
        }
    })
}

/// Приводит число к масштабу поля; лишние знаки — ошибка, а не тихое округление.
pub(crate) fn to_scale(def: &FieldDef, value: Decimal, scale: u8) -> Result<Decimal, CoreError> {
    let stored = value.rescale(scale)?;
    if stored != value {
        return Err(CoreError::Invalid(format!(
            "поле {}: больше {scale} знаков после запятой",
            def.name
        )));
    }
    Ok(stored)
}

pub(crate) fn from_value(def: &FieldDef, value: Value) -> Result<FieldValue, CoreError> {
    Ok(match (def.kind, value) {
        (_, Value::Null) => FieldValue::Null,
        (FieldKind::Int, Value::Int(v)) => FieldValue::Int(v),
        (FieldKind::Text, Value::Text(v)) => FieldValue::Text(v),
        (FieldKind::Decimal { scale }, Value::Int(units)) => {
            FieldValue::Decimal(Decimal::new(units, scale)?)
        }
        (FieldKind::Ref, Value::Int(id)) => FieldValue::Ref(id),
        (FieldKind::Date, Value::Text(d)) => FieldValue::Date(d),
        _ => {
            return Err(CoreError::Invalid(format!(
                "в базе в поле {} не {}",
                def.name, def.kind
            )));
        }
    })
}

/// `ГГГГ-ММ-ДД` с настоящими месяцем и днём, с учётом високосных лет.
pub(crate) fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|p| p.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day)) = (num(0..4), num(5..7), num(8..10)) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    s.bytes()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=days).contains(&day)
}

pub(crate) fn read_fields(
    defs: &[FieldDef],
    values: impl Iterator<Item = Value>,
) -> Result<Fields, CoreError> {
    defs.iter()
        .zip(values)
        .map(|(def, value)| Ok((def.name.clone(), from_value(def, value)?)))
        .collect()
}

/// Ставит флаг 0/1 с проверкой версии; `false` — версия не совпала или записи нет.
pub(crate) fn set_flag(
    db: &mut dyn Driver,
    table: &str,
    column: &str,
    value: bool,
    id: i64,
    version: i64,
) -> Result<bool, CoreError> {
    let sql = {
        let d = db.dialect();
        let v = d.quote_ident("version");
        format!(
            "UPDATE {} SET {} = {}, {v} = {v} + 1 WHERE {} = {} AND {v} = {}",
            d.quote_ident(table),
            d.quote_ident(column),
            d.placeholder(1),
            d.quote_ident("id"),
            d.placeholder(2),
            d.placeholder(3),
        )
    };
    let params = [
        Value::Int(value.into()),
        Value::Int(id),
        Value::Int(version),
    ];
    Ok(db.execute(&sql, &params)? > 0)
}

pub(crate) fn returned_id(rows: &[Vec<Value>]) -> Result<i64, CoreError> {
    match rows.first().and_then(|row| row.first()) {
        Some(Value::Int(id)) => Ok(*id),
        _ => Err(CoreError::Invalid("база не вернула id новой записи".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::is_date;

    #[test]
    fn dates() {
        for ok in ["2026-10-30", "2024-02-29", "2000-02-29", "1999-12-31"] {
            assert!(is_date(ok), "{ok}");
        }
        for bad in [
            "2026-1-30",
            "2026-13-01",
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-10-00",
            "30.10.2026",
            "2026-10-3a",
            "+026-10-30",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }
}
