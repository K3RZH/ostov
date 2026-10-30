use ostov_store::Driver;

use crate::CoreError;

/// Выполняет `f` в одной транзакции. Откат будет при ошибке `f`, при панике внутри неё
/// и при неудачном `COMMIT` — соединение не остаётся с открытой транзакцией.
pub fn in_transaction<T>(
    db: &mut dyn Driver,
    f: impl FnOnce(&mut dyn Driver) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    db.begin()?;
    let tx = Transaction {
        db,
        committed: false,
    };
    tx.run(f)
}

struct Transaction<'a> {
    db: &'a mut dyn Driver,
    committed: bool,
}

impl Transaction<'_> {
    fn run<T>(
        mut self,
        f: impl FnOnce(&mut dyn Driver) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let value = f(&mut *self.db)?;
        self.db.commit()?;
        self.committed = true;
        Ok(value)
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            // Важнее исходная ошибка или паника, чем ошибка самого отката.
            let _ = self.db.rollback();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use ostov_store::drivers::sqlite::SqliteDialect;
    use ostov_store::{Dialect, Row, StoreError, Value};

    use super::*;
    use crate::testing::memory_db;

    /// Драйвер-пустышка: записывает вызовы, `COMMIT` можно заставить падать.
    #[derive(Default)]
    struct Fake {
        calls: Vec<&'static str>,
        fail_commit: bool,
    }

    impl Driver for Fake {
        fn dialect(&self) -> &dyn Dialect {
            &SqliteDialect
        }
        fn execute(&mut self, _: &str, _: &[Value]) -> Result<usize, StoreError> {
            self.calls.push("execute");
            Ok(1)
        }
        fn query(&mut self, _: &str, _: &[Value]) -> Result<Vec<Row>, StoreError> {
            Ok(Vec::new())
        }
        fn begin(&mut self) -> Result<(), StoreError> {
            self.calls.push("begin");
            Ok(())
        }
        fn commit(&mut self) -> Result<(), StoreError> {
            self.calls.push("commit");
            if self.fail_commit {
                return Err(StoreError::Database("database is locked".into()));
            }
            Ok(())
        }
        fn rollback(&mut self) -> Result<(), StoreError> {
            self.calls.push("rollback");
            Ok(())
        }
    }

    #[test]
    fn commits_on_success() {
        let mut db = Fake::default();
        in_transaction(&mut db, |db| Ok(db.execute("", &[])?)).unwrap();
        assert_eq!(db.calls, ["begin", "execute", "commit"]);
    }

    #[test]
    fn rolls_back_on_error() {
        let mut db = Fake::default();
        let result: Result<(), _> =
            in_transaction(&mut db, |_| Err(CoreError::Invalid("нет".into())));
        assert!(result.is_err());
        assert_eq!(db.calls, ["begin", "rollback"]);
    }

    #[test]
    fn rolls_back_when_commit_fails() {
        let mut db = Fake {
            fail_commit: true,
            ..Fake::default()
        };
        assert!(in_transaction(&mut db, |_| Ok(())).is_err());
        assert_eq!(db.calls, ["begin", "commit", "rollback"]);
    }

    #[test]
    fn rolls_back_on_panic_and_connection_stays_usable() {
        let mut db = memory_db();
        db.execute("CREATE TABLE t (v INTEGER)", &[]).unwrap();
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            let _: Result<(), _> = in_transaction(&mut db, |db| {
                db.execute("INSERT INTO t VALUES (1)", &[])?;
                panic!("сбой в правиле проведения");
            });
        }));
        assert!(panicked.is_err());

        // Транзакция откатилась, новая открывается без «transaction within a transaction».
        in_transaction(&mut db, |db| {
            Ok(db.execute("INSERT INTO t VALUES (2)", &[])?)
        })
        .unwrap();
        let rows = db.query("SELECT v FROM t", &[]).unwrap();
        assert_eq!(rows, vec![vec![Value::Int(2)]]);
    }
}
