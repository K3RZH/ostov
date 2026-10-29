//! Объекты: справочники и документы, общее описание полей.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ostov_store::{Decimal, Dialect, Driver, Value};

use crate::CoreError;

pub mod catalog;
pub mod document;

pub type Fields = BTreeMap<String, FieldValue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Int,
    Text,
    Decimal { scale: u8 },
}

impl fmt::Display for FieldKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => write!(f, "целое число"),
            Self::Text => write!(f, "строка"),
            Self::Decimal { scale } => write!(f, "число с {scale} знаками после запятой"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub kind: FieldKind,
}

impl FieldDef {
    pub fn new(name: &str, kind: FieldKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Null,
    Int(i64),
    Text(String),
    Decimal(Decimal),
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
                FieldKind::Int | FieldKind::Decimal { .. } => "BIGINT",
                FieldKind::Text => "TEXT",
            };
            format!("{} {ty}", d.quote_ident(&f.name))
        })
        .collect()
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
        .map(|def| to_value(def, fields.get(&def.name).unwrap_or(&FieldValue::Null)))
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
        _ => {
            return Err(CoreError::Invalid(format!(
                "в базе в поле {} не {}",
                def.name, def.kind
            )));
        }
    })
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
