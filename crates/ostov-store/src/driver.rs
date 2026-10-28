use std::fmt;

use crate::Decimal;

/// Значение параметра или колонки. Float нет: деньги и количества идут как `Decimal`
/// и хранятся целым в минимальных единицах.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
    Bytes(Vec<u8>),
}

pub type Row = Vec<Value>;

#[derive(Debug)]
pub enum StoreError {
    Database(String),
    /// Тип колонки или параметра, которого нет в `Value` (например float).
    UnsupportedType(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(msg) => write!(f, "ошибка базы данных: {msg}"),
            Self::UnsupportedType(what) => write!(f, "неподдерживаемый тип: {what}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Особенности SQL конкретной СУБД.
pub trait Dialect {
    fn name(&self) -> &'static str;

    /// Параметр с номером `n`, начиная с 1.
    fn placeholder(&self, n: usize) -> String;

    /// Описание колонки `id` с автоувеличением, для `CREATE TABLE`.
    fn id_column(&self) -> &'static str;

    /// Хвост `SELECT`, блокирующий прочитанные строки до конца транзакции.
    fn lock_rows(&self) -> &'static str {
        " FOR UPDATE"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

/// Подключение к базе. SQL всегда с параметрами, значения в строку запроса не склеиваем.
pub trait Driver {
    fn dialect(&self) -> &dyn Dialect;

    /// Возвращает число изменённых строк.
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<usize, StoreError>;

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>, StoreError>;

    fn begin(&mut self) -> Result<(), StoreError>;
    fn commit(&mut self) -> Result<(), StoreError>;
    fn rollback(&mut self) -> Result<(), StoreError>;
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::Text(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<Decimal> for Value {
    fn from(v: Decimal) -> Self {
        Self::Int(v.units())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Into::into)
    }
}
