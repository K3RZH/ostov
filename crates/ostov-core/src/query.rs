//! Запросы конфигурации через `ostov-sql` (СТ-13, СТ-14): проверка и права до базы,
//! потом выполнение и разбор строк по типам полей.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ostov_sql::{Column, Param};
use ostov_store::{Decimal, Driver, Value};

use crate::{CoreError, FieldKind, FieldValue};

/// Событие журнала запуска (К-21): режим тестирования конфигурации показывает, что делало ядро.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceEvent {
    /// Команда `Engine` целиком.
    Command {
        name: String,
        took: Duration,
        ok: bool,
    },
    /// Запрос `ostov-sql`: SQL после проверки и перевода под базу.
    Query {
        sql: String,
        params: Vec<Value>,
        rows: usize,
        took: Duration,
    },
    /// Правило проведения документа (Rust или Python).
    Rule {
        document: String,
        took: Duration,
        ok: bool,
    },
}

/// Куда ядро отдаёт события; без него журнал не ведётся.
pub type Trace = Arc<dyn Fn(&TraceEvent) + Send + Sync>;

/// Ответ на запрос: колонки по порядку и строки в типах полей.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRows {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<FieldValue>>,
}

/// Чтение базы из правила документа (`ctx.query`): в той же транзакции и с правами того,
/// кто проводит.
pub trait Reader {
    fn query(
        &mut self,
        sql: &str,
        params: &BTreeMap<String, Param>,
    ) -> Result<QueryRows, CoreError>;
}

pub struct DbReader<'a> {
    pub db: &'a mut dyn Driver,
    pub model: &'a ostov_model::Config,
    pub can_read: &'a dyn Fn(&str) -> bool,
    pub trace: Option<&'a Trace>,
}

impl Reader for DbReader<'_> {
    fn query(
        &mut self,
        sql: &str,
        params: &BTreeMap<String, Param>,
    ) -> Result<QueryRows, CoreError> {
        run(self.db, self.model, self.can_read, sql, params, self.trace)
    }
}

pub fn run(
    db: &mut dyn Driver,
    model: &ostov_model::Config,
    can_read: &dyn Fn(&str) -> bool,
    sql: &str,
    params: &BTreeMap<String, Param>,
    trace: Option<&Trace>,
) -> Result<QueryRows, CoreError> {
    let prepared =
        ostov_sql::prepare(sql, params, model, can_read, db.dialect()).map_err(CoreError::Query)?;
    let started = Instant::now();
    let raw = db.query(&prepared.sql, &prepared.params)?;
    if let Some(trace) = trace {
        trace(&TraceEvent::Query {
            sql: prepared.sql.clone(),
            params: prepared.params.clone(),
            rows: raw.len(),
            took: started.elapsed(),
        });
    }
    let rows = raw
        .into_iter()
        .map(|row| {
            if row.len() != prepared.columns.len() {
                return Err(CoreError::Invalid(format!(
                    "база вернула {} колонок вместо {}",
                    row.len(),
                    prepared.columns.len()
                )));
            }
            row.into_iter()
                .zip(&prepared.columns)
                .map(|(value, column)| read(column, value))
                .collect()
        })
        .collect::<Result<_, _>>()?;
    Ok(QueryRows {
        columns: prepared.columns,
        rows,
    })
}

/// Значение колонки в тип поля: деньги приходят целыми единицами, ссылки — id.
/// Что не сходится с типом колонки — ошибка, а не тихо не то число.
fn read(column: &Column, value: Value) -> Result<FieldValue, CoreError> {
    Ok(match (column.kind, value) {
        (_, Value::Null) => FieldValue::Null,
        (Some(FieldKind::Decimal { scale }), Value::Int(units)) => FieldValue::Decimal(
            Decimal::new(units, scale).map_err(|e| wrong(column, &e.to_string()))?,
        ),
        (Some(FieldKind::Int) | None, Value::Int(n)) => FieldValue::Int(n),
        (Some(FieldKind::Ref), Value::Int(id)) => FieldValue::Ref(id),
        (Some(FieldKind::Text) | None, Value::Text(s)) => FieldValue::Text(s),
        (Some(FieldKind::Date), Value::Text(s)) => FieldValue::Date(s),
        (kind, value) => {
            let got = match value {
                Value::Int(_) => "целое число",
                Value::Text(_) => "строку",
                Value::Bytes(_) => "байты",
                Value::Null => "пусто",
            };
            let need = kind.map_or_else(|| "число или строка".to_owned(), |k| k.to_string());
            return Err(wrong(
                column,
                &format!("база вернула {got}, а нужно {need}"),
            ));
        }
    })
}

fn wrong(column: &Column, why: &str) -> CoreError {
    CoreError::Invalid(format!("Колонка «{}» запроса: {why}", column.name))
}
