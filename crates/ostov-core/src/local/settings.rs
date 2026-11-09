//! Настройки пользователя (вехи, ширины и порядок колонок, вид списка): ключ → набор полей.
//! Пока пользователь один; с ostov-sec ключ будет ещё и по пользователю.

use ostov_api::{Fields, Value};
use ostov_store::Driver;

use crate::{CoreError, in_transaction};

const TABLE: &str = "ostov_settings";

pub(super) fn create_table(db: &mut dyn Driver) -> Result<(), CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "CREATE TABLE IF NOT EXISTS {} ({} TEXT NOT NULL, {} TEXT NOT NULL, {} TEXT NOT NULL, PRIMARY KEY ({}, {}))",
            d.quote_ident(TABLE),
            d.quote_ident("key"),
            d.quote_ident("name"),
            d.quote_ident("value"),
            d.quote_ident("key"),
            d.quote_ident("name"),
        )
    };
    db.execute(&sql, &[])?;
    Ok(())
}

pub(super) fn get(db: &mut dyn Driver, key: &str) -> Result<Option<Fields>, CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "SELECT {}, {} FROM {} WHERE {} = {}",
            d.quote_ident("name"),
            d.quote_ident("value"),
            d.quote_ident(TABLE),
            d.quote_ident("key"),
            d.placeholder(1)
        )
    };
    let rows = db.query(&sql, &[ostov_store::Value::Text(key.to_owned())])?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut fields = Fields::new();
    for row in rows {
        if let [
            ostov_store::Value::Text(name),
            ostov_store::Value::Text(value),
        ] = row.as_slice()
        {
            fields.insert(name.clone(), decode(value)?);
        }
    }
    Ok(Some(fields))
}

/// Заменяет весь набор под ключом.
pub(super) fn save(db: &mut dyn Driver, key: &str, values: &Fields) -> Result<(), CoreError> {
    let (delete, insert) = {
        let d = db.dialect();
        let table = d.quote_ident(TABLE);
        (
            format!(
                "DELETE FROM {table} WHERE {} = {}",
                d.quote_ident("key"),
                d.placeholder(1)
            ),
            format!(
                "INSERT INTO {table} ({}, {}, {}) VALUES ({}, {}, {})",
                d.quote_ident("key"),
                d.quote_ident("name"),
                d.quote_ident("value"),
                d.placeholder(1),
                d.placeholder(2),
                d.placeholder(3)
            ),
        )
    };
    let text = |s: &str| ostov_store::Value::Text(s.to_owned());
    in_transaction(db, |db| {
        db.execute(&delete, &[text(key)])?;
        for (name, value) in values {
            db.execute(&insert, &[text(key), text(name), text(&encode(value))])?;
        }
        Ok(())
    })
}

fn encode(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Int(n) => format!("int:{n}"),
        Value::Text(s) => format!("text:{s}"),
        Value::Decimal { units, scale } => format!("dec:{units}:{scale}"),
        Value::Bool(b) => format!("bool:{}", u8::from(*b)),
        Value::Date(d) => format!("date:{d}"),
        Value::Ref { id, .. } => format!("ref:{id}"),
    }
}

fn decode(raw: &str) -> Result<Value, CoreError> {
    let broken = || CoreError::Invalid(format!("испорченная настройка «{raw}»"));
    let (tag, rest) = raw.split_once(':').unwrap_or((raw, ""));
    Ok(match tag {
        "null" => Value::Null,
        "int" => Value::Int(rest.parse().map_err(|_| broken())?),
        "text" => Value::Text(rest.to_owned()),
        "dec" => {
            let (units, scale) = rest.split_once(':').ok_or_else(broken)?;
            Value::Decimal {
                units: units.parse().map_err(|_| broken())?,
                scale: scale.parse().map_err(|_| broken())?,
            }
        }
        "bool" => Value::Bool(rest == "1"),
        "date" => Value::Date(rest.to_owned()),
        "ref" => Value::Ref {
            id: rest.parse().map_err(|_| broken())?,
            title: None,
        },
        _ => return Err(broken()),
    })
}
