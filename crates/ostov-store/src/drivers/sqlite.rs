use std::path::Path;

use rusqlite::config::DbConfig;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OpenFlags, params_from_iter};

use crate::driver::{Dialect, Driver, Row, StoreError, Value};

pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn placeholder(&self, n: usize) -> String {
        format!("?{n}")
    }

    fn id_column(&self) -> &'static str {
        "INTEGER PRIMARY KEY"
    }

    // SQLite пишет одной транзакцией за раз, построчной блокировки у него нет.
    fn lock_rows(&self) -> &'static str {
        ""
    }

    // Встроенный LOWER в SQLite понимает только латиницу.
    fn lower(&self, expr: &str) -> String {
        format!("ostov_lower({expr})")
    }
}

/// База режима «один компьютер»: файл SQLite рядом с приложением.
pub struct SqliteDriver {
    conn: Connection,
    depth: crate::Depth,
}

impl SqliteDriver {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        add_functions(&conn)?;
        Ok(Self {
            conn,
            depth: crate::Depth::default(),
        })
    }

    /// Только чтение: файл не меняется, даже режим журнала.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        add_functions(&conn)?;
        Ok(Self {
            conn,
            depth: crate::Depth::default(),
        })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        add_functions(&conn)?;
        Ok(Self {
            conn,
            depth: crate::Depth::default(),
        })
    }
}

fn add_functions(conn: &Connection) -> Result<(), StoreError> {
    // Иначе SQLite молча считает "несуществующую_колонку" строкой, и опечатка в имени
    // даёт не ошибку, а неверный результат.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("ostov_lower", 1, flags, |ctx| {
        let text: Option<String> = ctx.get(0)?;
        Ok(text.map(|t| t.to_lowercase()))
    })?;
    Ok(())
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

    // Внешняя транзакция сразу берёт запись: иначе в WAL вторая сессия, прочитав, получает
    // «база занята» без ожидания, когда первая уже пишет.
    fn begin(&mut self) -> Result<(), StoreError> {
        let sql = self.depth.begin();
        let sql = if sql == "BEGIN" {
            "BEGIN IMMEDIATE".to_owned()
        } else {
            sql
        };
        self.conn.execute_batch(&sql)?;
        self.depth.entered();
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        self.conn.execute_batch(&self.depth.commit())?;
        self.depth.left();
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        let sql = self.depth.rollback();
        self.depth.left();
        Ok(self.conn.execute_batch(&sql)?)
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
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy(e.to_string())
            }
            _ => Self::Database(e.to_string()),
        }
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
    fn nested_transactions_use_savepoints() {
        let mut db = db();
        let names = |db: &mut SqliteDriver| -> Vec<Row> {
            db.query("SELECT name FROM items ORDER BY id", &[]).unwrap()
        };
        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('outer')", &[])
            .unwrap();
        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('inner')", &[])
            .unwrap();
        db.rollback().unwrap();
        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('kept')", &[])
            .unwrap();
        db.commit().unwrap();
        db.commit().unwrap();
        assert_eq!(
            names(&mut db),
            vec![
                vec![Value::Text("outer".into())],
                vec![Value::Text("kept".into())]
            ]
        );

        // Откат внешней убирает и то, что внутренняя уже «записала».
        db.begin().unwrap();
        db.begin().unwrap();
        db.execute("INSERT INTO items (name) VALUES ('lost')", &[])
            .unwrap();
        db.commit().unwrap();
        db.rollback().unwrap();
        assert_eq!(names(&mut db).len(), 2);
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
    fn unknown_quoted_column_is_an_error() {
        let mut db = db();
        assert!(
            db.query("SELECT \"no_such_column\" FROM items", &[])
                .is_err()
        );
    }

    #[test]
    fn lower_knows_cyrillic() {
        let mut db = SqliteDriver::open_in_memory().unwrap();
        let rows = db
            .query(
                "SELECT ostov_lower('МУКА Ёлка ABC'), ostov_lower(NULL)",
                &[],
            )
            .unwrap();
        assert_eq!(
            rows,
            vec![vec![Value::Text("мука ёлка abc".into()), Value::Null]]
        );
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
