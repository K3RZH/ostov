use postgres::types::{ToSql, Type};
use postgres::{Client, NoTls};

use crate::driver::{Dialect, Driver, Row, StoreError, Value};

pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn placeholder(&self, n: usize) -> String {
        format!("${n}")
    }
}

/// База режима «сервер в локальной сети».
pub struct PostgresDriver {
    client: Client,
}

impl PostgresDriver {
    // TODO(ADR-026): TLS вместо NoTls, когда дойдём до ostov-server.
    pub fn connect(url: &str) -> Result<Self, StoreError> {
        Ok(Self {
            client: Client::connect(url, NoTls)?,
        })
    }
}

type Param = Box<dyn ToSql + Sync>;

impl PostgresDriver {
    // PostgreSQL строго сверяет типы параметров, поэтому значения приводим
    // к тому типу, который сервер вывел для запроса.
    fn bind(
        &mut self,
        sql: &str,
        params: &[Value],
    ) -> Result<(postgres::Statement, Vec<Param>), StoreError> {
        let stmt = self.client.prepare(sql)?;
        if stmt.params().len() != params.len() {
            return Err(StoreError::Database(format!(
                "ожидается {} параметров, передано {}",
                stmt.params().len(),
                params.len()
            )));
        }
        let bound = params
            .iter()
            .zip(stmt.params())
            .enumerate()
            .map(|(i, (value, ty))| to_param(value, ty, i + 1))
            .collect::<Result<_, _>>()?;
        Ok((stmt, bound))
    }
}

impl Driver for PostgresDriver {
    fn dialect(&self) -> &dyn Dialect {
        &PostgresDialect
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<usize, StoreError> {
        let (stmt, params) = self.bind(sql, params)?;
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p.as_ref()).collect();
        let n = self.client.execute(&stmt, &refs)?;
        usize::try_from(n).map_err(|_| StoreError::Database(format!("слишком много строк: {n}")))
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>, StoreError> {
        let (stmt, params) = self.bind(sql, params)?;
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p.as_ref()).collect();
        self.client
            .query(&stmt, &refs)?
            .iter()
            .map(from_row)
            .collect()
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        Ok(self.client.batch_execute("BEGIN")?)
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        Ok(self.client.batch_execute("COMMIT")?)
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        Ok(self.client.batch_execute("ROLLBACK")?)
    }
}

fn is_text(ty: &Type) -> bool {
    [Type::TEXT, Type::VARCHAR, Type::BPCHAR, Type::NAME].contains(ty)
}

fn to_param(value: &Value, ty: &Type, n: usize) -> Result<Param, StoreError> {
    let unsupported = || StoreError::UnsupportedType(format!("параметр ${n}: {ty}"));
    let out_of_range =
        || StoreError::Database(format!("параметр ${n}: число не помещается в {ty}"));
    Ok(match value {
        Value::Int(v) if *ty == Type::INT8 => Box::new(*v),
        Value::Int(v) if *ty == Type::INT4 => {
            Box::new(i32::try_from(*v).map_err(|_| out_of_range())?)
        }
        Value::Int(v) if *ty == Type::INT2 => {
            Box::new(i16::try_from(*v).map_err(|_| out_of_range())?)
        }
        Value::Text(v) if is_text(ty) => Box::new(v.clone()),
        Value::Bytes(v) if *ty == Type::BYTEA => Box::new(v.clone()),
        Value::Null if *ty == Type::INT8 => Box::new(None::<i64>),
        Value::Null if *ty == Type::INT4 => Box::new(None::<i32>),
        Value::Null if *ty == Type::INT2 => Box::new(None::<i16>),
        Value::Null if is_text(ty) => Box::new(None::<String>),
        Value::Null if *ty == Type::BYTEA => Box::new(None::<Vec<u8>>),
        _ => return Err(unsupported()),
    })
}

fn from_row(row: &postgres::Row) -> Result<Row, StoreError> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(i, column)| {
            let ty = column.type_();
            let value = if *ty == Type::INT8 {
                row.try_get::<_, Option<i64>>(i)?.map(Value::Int)
            } else if *ty == Type::INT4 {
                row.try_get::<_, Option<i32>>(i)?
                    .map(|v| Value::Int(v.into()))
            } else if *ty == Type::INT2 {
                row.try_get::<_, Option<i16>>(i)?
                    .map(|v| Value::Int(v.into()))
            } else if is_text(ty) {
                row.try_get::<_, Option<String>>(i)?.map(Value::Text)
            } else if *ty == Type::BYTEA {
                row.try_get::<_, Option<Vec<u8>>>(i)?.map(Value::Bytes)
            } else {
                return Err(StoreError::UnsupportedType(format!(
                    "колонка {}: {ty}",
                    column.name()
                )));
            };
            Ok(value.unwrap_or(Value::Null))
        })
        .collect()
}

impl From<postgres::Error> for StoreError {
    fn from(e: postgres::Error) -> Self {
        Self::Database(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decimal;

    #[test]
    fn dialect() {
        let d = PostgresDialect;
        assert_eq!(d.placeholder(2), "$2");
        assert_eq!(d.quote_ident("a\"b"), "\"a\"\"b\"");
    }

    // Тесты с живой базой идут, только если задан OSTOV_PG_URL,
    // например host=localhost user=postgres password=postgres dbname=ostov_test
    fn db() -> Option<PostgresDriver> {
        let url = std::env::var("OSTOV_PG_URL").ok()?;
        let mut db = PostgresDriver::connect(&url).unwrap();
        db.execute(
            "CREATE TEMP TABLE items (id BIGSERIAL PRIMARY KEY, name TEXT, price BIGINT, qty INTEGER, data BYTEA)",
            &[],
        )
        .unwrap();
        Some(db)
    }

    #[test]
    fn insert_and_select() {
        let Some(mut db) = db() else { return };
        let price: Decimal = "12.50".parse().unwrap();
        db.execute(
            "INSERT INTO items (name, price, qty, data) VALUES ($1, $2, $3, $4)",
            &[
                "Мука".into(),
                price.into(),
                3.into(),
                Value::Bytes(vec![1, 2]),
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO items (name, price, qty, data) VALUES ($1, $2, $3, $4)",
            &[Value::Null, Value::Null, Value::Null, Value::Null],
        )
        .unwrap();

        let rows = db
            .query("SELECT name, price, qty, data FROM items ORDER BY id", &[])
            .unwrap();
        assert_eq!(
            rows[0],
            vec![
                Value::Text("Мука".into()),
                Value::Int(1250),
                Value::Int(3),
                Value::Bytes(vec![1, 2])
            ]
        );
        assert_eq!(rows[1], vec![Value::Null; 4]);
    }

    #[test]
    fn params_are_not_sql() {
        let Some(mut db) = db() else { return };
        let evil = "x'); DROP TABLE items; --";
        db.execute("INSERT INTO items (name) VALUES ($1)", &[evil.into()])
            .unwrap();
        let rows = db.query("SELECT name FROM items", &[]).unwrap();
        assert_eq!(rows, vec![vec![Value::Text(evil.into())]]);
    }

    #[test]
    fn rollback_discards_changes() {
        let Some(mut db) = db() else { return };
        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('a')", &[])
            .unwrap();
        db.rollback().unwrap();
        assert!(db.query("SELECT id FROM items", &[]).unwrap().is_empty());
    }

    #[test]
    fn rejects_float_and_overflow() {
        let Some(mut db) = db() else { return };
        let err = db.query("SELECT 1.5::float8 AS x", &[]).unwrap_err();
        assert!(matches!(err, StoreError::UnsupportedType(_)));
        let err = db
            .execute(
                "INSERT INTO items (qty) VALUES ($1)",
                &[Value::Int(i64::MAX)],
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::Database(_)));
    }
}
