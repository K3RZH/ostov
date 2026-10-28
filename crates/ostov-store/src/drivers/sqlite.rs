use std::path::Path;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, params_from_iter};

use crate::driver::{Dialect, Driver, Row, StoreError, Value};

pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn placeholder(&self, n: usize) -> String {
        format!("?{n}")
    }
}

/// База режима «один компьютер»: файл SQLite рядом с приложением.
pub struct SqliteDriver {
    conn: Connection,
}

impl SqliteDriver {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(Self { conn })
    }
}

impl Driver for SqliteDriver {
    fn dialect(&self) -> &dyn Dialect {
        &SqliteDialect
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<usize, StoreError> {
        Ok(self
            .conn
            .execute(sql, params_from_iter(params.iter().map(to_sql)))?)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>, StoreError> {
        let mut stmt = self.conn.prepare(sql)?;
        let columns: Vec<String> = stmt.column_names().map(str::to_owned).collect();
        let mut rows = stmt.query(params_from_iter(params.iter().map(to_sql)))?;

        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let mut values = Vec::with_capacity(columns.len());
            for (i, column) in columns.iter().enumerate() {
                values.push(from_sql(row.get_ref(i)?, column)?);
            }
            result.push(values);
        }
        Ok(result)
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        Ok(self.conn.execute_batch("BEGIN")?)
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        Ok(self.conn.execute_batch("COMMIT")?)
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        Ok(self.conn.execute_batch("ROLLBACK")?)
    }
}

fn to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Int(v) => SqlValue::Integer(*v),
        Value::Text(v) => SqlValue::Text(v.clone()),
        Value::Bytes(v) => SqlValue::Blob(v.clone()),
    }
}

fn from_sql(value: ValueRef<'_>, column: &str) -> Result<Value, StoreError> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => Value::Int(v),
        ValueRef::Text(bytes) => Value::Text(
            String::from_utf8(bytes.to_vec())
                .map_err(|_| StoreError::Database(format!("не UTF-8 в колонке {column}")))?,
        ),
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
        ValueRef::Real(_) => return Err(StoreError::UnsupportedType(column.to_owned())),
    })
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decimal;

    fn db() -> SqliteDriver {
        let mut db = SqliteDriver::open_in_memory().unwrap();
        db.execute(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL, price INTEGER)",
            &[],
        )
        .unwrap();
        db
    }

    #[test]
    fn insert_and_select() {
        let mut db = db();
        let price: Decimal = "12.50".parse().unwrap();
        let n = db
            .execute(
                "INSERT INTO items (name, price) VALUES (?1, ?2)",
                &["Мука".into(), price.into()],
            )
            .unwrap();
        assert_eq!(n, 1);
        db.execute(
            "INSERT INTO items (name, price) VALUES (?1, ?2)",
            &["Соль".into(), Value::Null],
        )
        .unwrap();

        let rows = db
            .query("SELECT name, price FROM items ORDER BY id", &[])
            .unwrap();
        assert_eq!(rows[0], vec![Value::Text("Мука".into()), Value::Int(1250)]);
        assert_eq!(rows[1], vec![Value::Text("Соль".into()), Value::Null]);
    }

    #[test]
    fn params_are_not_sql() {
        let mut db = db();
        let evil = "x'); DROP TABLE items; --";
        db.execute("INSERT INTO items (name) VALUES (?1)", &[evil.into()])
            .unwrap();
        let rows = db.query("SELECT name FROM items", &[]).unwrap();
        assert_eq!(rows, vec![vec![Value::Text(evil.into())]]);
    }

    #[test]
    fn rollback_discards_changes() {
        let mut db = db();
        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('a')", &[])
            .unwrap();
        db.rollback().unwrap();
        assert!(db.query("SELECT * FROM items", &[]).unwrap().is_empty());

        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('b')", &[])
            .unwrap();
        db.commit().unwrap();
        assert_eq!(db.query("SELECT * FROM items", &[]).unwrap().len(), 1);
    }

    #[test]
    fn float_column_is_rejected() {
        let mut db = db();
        let err = db.query("SELECT 1.5 AS x", &[]).unwrap_err();
        assert!(matches!(err, StoreError::UnsupportedType(c) if c == "x"));
    }

    #[test]
    fn dialect() {
        let d = SqliteDialect;
        assert_eq!(d.placeholder(3), "?3");
        assert_eq!(d.quote_ident(r#"we"ird"#), r#""we""ird""#);
    }

    #[test]
    fn file_database() {
        let dir = std::env::temp_dir().join(format!("ostov-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        {
            let mut db = SqliteDriver::open(&path).unwrap();
            db.execute("CREATE TABLE t (v INTEGER)", &[]).unwrap();
            db.execute("INSERT INTO t VALUES (?1)", &[42.into()])
                .unwrap();
        }
        let mut db = SqliteDriver::open(&path).unwrap();
        assert_eq!(
            db.query("SELECT v FROM t", &[]).unwrap(),
            vec![vec![Value::Int(42)]]
        );
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
