use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ostov_store::{Decimal, Dialect, Driver, Value};

use crate::CoreError;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub id: i64,
    pub version: i64,
    pub fields: BTreeMap<String, FieldValue>,
}

/// Справочник: таблица с `id`, `version` и полями.
/// Пока описание задаётся в коде, потом его будет давать ostov-meta.
#[derive(Debug, Clone)]
pub struct Catalog {
    table: String,
    fields: Vec<FieldDef>,
}

impl Catalog {
    pub fn new(table: &str, fields: Vec<FieldDef>) -> Result<Self, CoreError> {
        check_ident(table)?;
        let mut seen = BTreeSet::new();
        for field in &fields {
            check_ident(&field.name)?;
            if matches!(field.name.as_str(), "id" | "version") {
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
        Ok(Self {
            table: table.to_owned(),
            fields,
        })
    }

    pub fn create_table(&self, db: &mut dyn Driver) -> Result<(), CoreError> {
        let d = db.dialect();
        let mut columns = vec![
            format!("{} {}", d.quote_ident("id"), d.id_column()),
            format!("{} BIGINT NOT NULL", d.quote_ident("version")),
        ];
        for field in &self.fields {
            let ty = match field.kind {
                FieldKind::Int | FieldKind::Decimal { .. } => "BIGINT",
                FieldKind::Text => "TEXT",
            };
            columns.push(format!("{} {ty}", d.quote_ident(&field.name)));
        }
        let sql = format!(
            "CREATE TABLE {} ({})",
            d.quote_ident(&self.table),
            columns.join(", ")
        );
        db.execute(&sql, &[])?;
        Ok(())
    }

    pub fn insert(
        &self,
        db: &mut dyn Driver,
        fields: &BTreeMap<String, FieldValue>,
    ) -> Result<Element, CoreError> {
        let mut values = vec![Value::Int(1)];
        values.extend(self.to_values(fields)?);

        let d = db.dialect();
        let mut names = vec![d.quote_ident("version")];
        names.extend(self.fields.iter().map(|f| d.quote_ident(&f.name)));
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
            d.quote_ident(&self.table),
            names.join(", "),
            placeholders(d, 1, names.len()),
            d.quote_ident("id"),
        );

        let id = match db.query(&sql, &values)?.first().and_then(|row| row.first()) {
            Some(Value::Int(id)) => *id,
            _ => return Err(CoreError::Invalid("база не вернула id новой записи".into())),
        };
        self.get(db, id)?.ok_or_else(|| self.not_found(id))
    }

    pub fn get(&self, db: &mut dyn Driver, id: i64) -> Result<Option<Element>, CoreError> {
        let d = db.dialect();
        let mut names = vec![d.quote_ident("id"), d.quote_ident("version")];
        names.extend(self.fields.iter().map(|f| d.quote_ident(&f.name)));
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = {}",
            names.join(", "),
            d.quote_ident(&self.table),
            d.quote_ident("id"),
            d.placeholder(1),
        );

        match db.query(&sql, &[Value::Int(id)])?.into_iter().next() {
            Some(row) => self.element_from(row).map(Some),
            None => Ok(None),
        }
    }

    /// Сохраняет запись, только если её версия не изменилась с момента чтения (ADR-023).
    pub fn update(&self, db: &mut dyn Driver, element: &Element) -> Result<Element, CoreError> {
        let mut values = self.to_values(&element.fields)?;
        let n = values.len();

        let d = db.dialect();
        let version = d.quote_ident("version");
        let mut sets: Vec<String> = self
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{} = {}", d.quote_ident(&f.name), d.placeholder(i + 1)))
            .collect();
        sets.push(format!("{version} = {version} + 1"));
        let sql = format!(
            "UPDATE {} SET {} WHERE {} = {} AND {version} = {}",
            d.quote_ident(&self.table),
            sets.join(", "),
            d.quote_ident("id"),
            d.placeholder(n + 1),
            d.placeholder(n + 2),
        );
        values.push(Value::Int(element.id));
        values.push(Value::Int(element.version));

        if db.execute(&sql, &values)? == 0 {
            return Err(match self.get(db, element.id)? {
                Some(_) => CoreError::VersionConflict {
                    table: self.table.clone(),
                    id: element.id,
                },
                None => self.not_found(element.id),
            });
        }
        self.get(db, element.id)?
            .ok_or_else(|| self.not_found(element.id))
    }

    fn to_values(&self, fields: &BTreeMap<String, FieldValue>) -> Result<Vec<Value>, CoreError> {
        if let Some(unknown) = fields
            .keys()
            .find(|k| !self.fields.iter().any(|f| &f.name == *k))
        {
            return Err(CoreError::Invalid(format!(
                "в {} нет поля {unknown}",
                self.table
            )));
        }
        self.fields
            .iter()
            .map(|def| to_value(def, fields.get(&def.name).unwrap_or(&FieldValue::Null)))
            .collect()
    }

    fn element_from(&self, row: Vec<Value>) -> Result<Element, CoreError> {
        let mut values = row.into_iter();
        let (Some(Value::Int(id)), Some(Value::Int(version))) = (values.next(), values.next())
        else {
            return Err(CoreError::Invalid(format!(
                "в {} нет id или версии",
                self.table
            )));
        };
        let fields = self
            .fields
            .iter()
            .zip(values)
            .map(|(def, value)| Ok((def.name.clone(), from_value(def, value)?)))
            .collect::<Result<_, CoreError>>()?;
        Ok(Element {
            id,
            version,
            fields,
        })
    }

    fn not_found(&self, id: i64) -> CoreError {
        CoreError::NotFound {
            table: self.table.clone(),
            id,
        }
    }
}

// Имена таблиц и колонок — только ASCII в нижнем регистре (ADR-025), кириллица живёт в метаданных.
fn check_ident(name: &str) -> Result<(), CoreError> {
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

fn placeholders(d: &dyn Dialect, from: usize, count: usize) -> String {
    (from..from + count)
        .map(|n| d.placeholder(n))
        .collect::<Vec<_>>()
        .join(", ")
}

fn to_value(def: &FieldDef, value: &FieldValue) -> Result<Value, CoreError> {
    Ok(match (def.kind, value) {
        (_, FieldValue::Null) => Value::Null,
        (FieldKind::Int, FieldValue::Int(v)) => Value::Int(*v),
        (FieldKind::Text, FieldValue::Text(v)) => Value::Text(v.clone()),
        (FieldKind::Decimal { scale }, FieldValue::Decimal(v)) => {
            let stored = v.rescale(scale)?;
            if stored != *v {
                return Err(CoreError::Invalid(format!(
                    "поле {}: больше {scale} знаков после запятой",
                    def.name
                )));
            }
            Value::Int(stored.units())
        }
        _ => {
            return Err(CoreError::Invalid(format!(
                "поле {}: ожидается {}",
                def.name, def.kind
            )));
        }
    })
}

fn from_value(def: &FieldDef, value: Value) -> Result<FieldValue, CoreError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_transaction;
    use ostov_store::drivers::sqlite::SqliteDriver;

    fn goods() -> Catalog {
        Catalog::new(
            "goods",
            vec![
                FieldDef::new("name", FieldKind::Text),
                FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
                FieldDef::new("min_qty", FieldKind::Int),
            ],
        )
        .unwrap()
    }

    fn db() -> SqliteDriver {
        let mut db = SqliteDriver::open_in_memory().unwrap();
        goods().create_table(&mut db).unwrap();
        db
    }

    fn fields(pairs: &[(&str, FieldValue)]) -> BTreeMap<String, FieldValue> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn price(s: &str) -> FieldValue {
        FieldValue::Decimal(s.parse().unwrap())
    }

    #[test]
    fn insert_and_get() {
        let mut db = db();
        let flour = goods()
            .insert(
                &mut db,
                &fields(&[
                    ("name", FieldValue::Text("Мука".into())),
                    ("price", price("12.5")),
                ]),
            )
            .unwrap();

        assert_eq!(flour.version, 1);
        assert_eq!(flour.fields["name"], FieldValue::Text("Мука".into()));
        assert_eq!(flour.fields["price"], price("12.50"));
        assert_eq!(flour.fields["min_qty"], FieldValue::Null);
        assert_eq!(goods().get(&mut db, flour.id).unwrap(), Some(flour));
        assert_eq!(goods().get(&mut db, 999).unwrap(), None);
    }

    #[test]
    fn update_checks_version() {
        let mut db = db();
        let cat = goods();
        let first = cat
            .insert(
                &mut db,
                &fields(&[("name", FieldValue::Text("Соль".into()))]),
            )
            .unwrap();

        let mut mine = first.clone();
        mine.fields.insert("min_qty".into(), FieldValue::Int(5));
        let saved = cat.update(&mut db, &mine).unwrap();
        assert_eq!(saved.version, 2);
        assert_eq!(saved.fields["min_qty"], FieldValue::Int(5));

        // Второй пользователь держит старую версию.
        let err = cat.update(&mut db, &first).unwrap_err();
        assert!(matches!(err, CoreError::VersionConflict { .. }));

        let ghost = Element { id: 999, ..first };
        assert!(matches!(
            cat.update(&mut db, &ghost),
            Err(CoreError::NotFound { .. })
        ));
    }

    #[test]
    fn rejects_bad_values() {
        let mut db = db();
        let cat = goods();
        for bad in [
            fields(&[("price", price("1.005"))]),
            fields(&[("name", FieldValue::Int(1))]),
            fields(&[("color", FieldValue::Text("red".into()))]),
        ] {
            assert!(
                matches!(cat.insert(&mut db, &bad), Err(CoreError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_names() {
        let text = |n: &str| FieldDef::new(n, FieldKind::Text);
        assert!(Catalog::new("goods; drop table x", vec![]).is_err());
        assert!(Catalog::new("Товары", vec![]).is_err());
        assert!(Catalog::new("goods", vec![text("id")]).is_err());
        assert!(Catalog::new("goods", vec![text("name"), text("name")]).is_err());
        assert!(
            Catalog::new(
                "goods",
                vec![FieldDef::new("p", FieldKind::Decimal { scale: 19 })]
            )
            .is_err()
        );
    }

    #[test]
    fn transaction_rolls_back_on_error() {
        let mut db = db();
        let cat = goods();
        let result: Result<(), CoreError> = in_transaction(&mut db, |db| {
            cat.insert(db, &fields(&[("name", FieldValue::Text("a".into()))]))?;
            Err(CoreError::Invalid("что-то пошло не так".into()))
        });
        assert!(result.is_err());
        assert_eq!(cat.get(&mut db, 1).unwrap(), None);

        let id = in_transaction(&mut db, |db| {
            Ok(cat
                .insert(db, &fields(&[("name", FieldValue::Text("b".into()))]))?
                .id)
        })
        .unwrap();
        assert!(cat.get(&mut db, id).unwrap().is_some());
    }
}
