use ostov_store::{Driver, Value};

use super::{
    FieldDef, Fields, check_fields, check_ident, column_defs, names, placeholders, read_fields,
    returned_id, to_values,
};
use crate::CoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub id: i64,
    pub version: i64,
    pub fields: Fields,
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
        check_fields(&fields, &["id", "version"])?;
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
        columns.extend(column_defs(d, &self.fields));
        let sql = format!(
            "CREATE TABLE {} ({})",
            d.quote_ident(&self.table),
            columns.join(", ")
        );
        db.execute(&sql, &[])?;
        Ok(())
    }

    pub fn insert(&self, db: &mut dyn Driver, fields: &Fields) -> Result<Element, CoreError> {
        let mut values = vec![Value::Int(1)];
        values.extend(to_values(&self.table, &self.fields, fields)?);

        let d = db.dialect();
        let mut cols = vec![d.quote_ident("version")];
        cols.extend(names(d, &self.fields));
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
            d.quote_ident(&self.table),
            cols.join(", "),
            placeholders(d, 1, cols.len()),
            d.quote_ident("id"),
        );

        let id = returned_id(&db.query(&sql, &values)?)?;
        self.get(db, id)?.ok_or_else(|| self.not_found(id))
    }

    pub fn get(&self, db: &mut dyn Driver, id: i64) -> Result<Option<Element>, CoreError> {
        let d = db.dialect();
        let mut cols = vec![d.quote_ident("id"), d.quote_ident("version")];
        cols.extend(names(d, &self.fields));
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = {}",
            cols.join(", "),
            d.quote_ident(&self.table),
            d.quote_ident("id"),
            d.placeholder(1),
        );

        let Some(row) = db.query(&sql, &[Value::Int(id)])?.into_iter().next() else {
            return Ok(None);
        };
        let mut values = row.into_iter();
        let (Some(Value::Int(id)), Some(Value::Int(version))) = (values.next(), values.next())
        else {
            return Err(CoreError::Invalid(format!(
                "в {} нет id или версии",
                self.table
            )));
        };
        Ok(Some(Element {
            id,
            version,
            fields: read_fields(&self.fields, values)?,
        }))
    }

    /// Сохраняет запись, только если её версия не изменилась с момента чтения (ADR-023).
    pub fn update(&self, db: &mut dyn Driver, element: &Element) -> Result<Element, CoreError> {
        let mut values = to_values(&self.table, &self.fields, &element.fields)?;
        let n = values.len();

        let d = db.dialect();
        let version = d.quote_ident("version");
        let mut sets: Vec<String> = names(d, &self.fields)
            .into_iter()
            .enumerate()
            .map(|(i, name)| format!("{name} = {}", d.placeholder(i + 1)))
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

    fn not_found(&self, id: i64) -> CoreError {
        CoreError::NotFound {
            table: self.table.clone(),
            id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{FieldKind, FieldValue};
    use crate::{in_transaction, testing::*};

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

    fn db() -> ostov_store::drivers::sqlite::SqliteDriver {
        let mut db = memory_db();
        goods().create_table(&mut db).unwrap();
        db
    }

    #[test]
    fn insert_and_get() {
        let mut db = db();
        let flour = goods()
            .insert(
                &mut db,
                &fields(&[("name", text("Мука")), ("price", dec("12.5"))]),
            )
            .unwrap();

        assert_eq!(flour.version, 1);
        assert_eq!(flour.fields["name"], text("Мука"));
        assert_eq!(flour.fields["price"], dec("12.50"));
        assert_eq!(flour.fields["min_qty"], FieldValue::Null);
        assert_eq!(goods().get(&mut db, flour.id).unwrap(), Some(flour));
        assert_eq!(goods().get(&mut db, 999).unwrap(), None);
    }

    #[test]
    fn update_checks_version() {
        let mut db = db();
        let cat = goods();
        let first = cat
            .insert(&mut db, &fields(&[("name", text("Соль"))]))
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
            fields(&[("price", dec("1.005"))]),
            fields(&[("name", FieldValue::Int(1))]),
            fields(&[("color", text("red"))]),
        ] {
            assert!(
                matches!(cat.insert(&mut db, &bad), Err(CoreError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_names() {
        let t = |n: &str| FieldDef::new(n, FieldKind::Text);
        assert!(Catalog::new("goods; drop table x", vec![]).is_err());
        assert!(Catalog::new("Товары", vec![]).is_err());
        assert!(Catalog::new("goods", vec![t("id")]).is_err());
        assert!(Catalog::new("goods", vec![t("name"), t("name")]).is_err());
        let wide = FieldDef::new("p", FieldKind::Decimal { scale: 19 });
        assert!(Catalog::new("goods", vec![wide]).is_err());
    }

    #[test]
    fn transaction_rolls_back_on_error() {
        let mut db = db();
        let cat = goods();
        let result: Result<(), CoreError> = in_transaction(&mut db, |db| {
            cat.insert(db, &fields(&[("name", text("a"))]))?;
            Err(CoreError::Invalid("что-то пошло не так".into()))
        });
        assert!(result.is_err());
        assert_eq!(cat.get(&mut db, 1).unwrap(), None);

        let id = in_transaction(&mut db, |db| {
            Ok(cat.insert(db, &fields(&[("name", text("b"))]))?.id)
        })
        .unwrap();
        assert!(cat.get(&mut db, id).unwrap().is_some());
    }
}
