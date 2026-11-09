//! SQL через платформу (СТ-13, СТ-14, ADR-004): текст запроса из конфигурации → SQL под диалект
//! с параметрами. Сам крейт базу не трогает.
//!
//! Путь запроса: разбор (`sqlparser`, общий диалект) → белый список → имена метаданных
//! в таблицы базы → права → SQL заново под `ostov_store::Dialect`. Значения от пользователя
//! идут только параметрами; строки из текста запроса тоже уходят параметрами.

mod error;
mod names;
mod translate;

use std::collections::BTreeMap;

use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

pub use error::{Pos, SqlError};

/// Больше значимых токенов не разбираем. Цепочку `a OR b OR …` разборщик строит без
/// счётчика глубины, а глубокое дерево переполняет стек при обходе и при освобождении.
pub const MAX_TOKENS: usize = 2000;

/// Глубже выражения не переводим: дальше ошибка, а не переполнение стека. В debug-сборке
/// уровень обхода занимает около 10 КБ, 64 уровня влезают в стек 1 МБ главного потока Windows.
pub const MAX_DEPTH: usize = 64;

/// Больше строк запрос не вернёт (СТ-51): `LIMIT` больше этого или без `LIMIT` → этот.
pub const MAX_LIMIT: u64 = 1000;

/// Длиннее запрос не разбираем: защита от огромного текста.
pub const MAX_QUERY_LEN: usize = 64 * 1024;

/// Готовый к выполнению запрос.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// SQL под диалект: физические имена в кавычках, плейсхолдеры драйвера.
    pub sql: String,
    /// Параметры по порядку плейсхолдеров.
    pub params: Vec<ostov_store::Value>,
    /// Колонки результата: имя и тип, чтобы ядро разобрало строки (деньги лежат целым).
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Тип поля метаданных; `None` — выражение (COUNT и т. п.), тогда целое.
    pub kind: Option<ostov_model::FieldKind>,
}

/// Проверяет запрос и переводит его под диалект.
///
/// `can_read(имя)` спрашивается для каждого справочника и документа в запросе;
/// табличная часть — по праву документа, регистры без проверки.
pub fn prepare(
    query: &str,
    params: &BTreeMap<String, ostov_store::Value>,
    config: &ostov_model::Config,
    can_read: &dyn Fn(&str) -> bool,
    dialect: &dyn ostov_store::Dialect,
) -> Result<Prepared, SqlError> {
    if query.len() > MAX_QUERY_LEN {
        return Err(SqlError::TooLong { len: query.len() });
    }
    let dialect_in = GenericDialect {};
    let tokens = Tokenizer::new(&dialect_in, query)
        .tokenize_with_location()
        .map_err(|e| SqlError::Parse(e.to_string()))?;
    let meaningful = tokens
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .count();
    if meaningful > MAX_TOKENS {
        return Err(SqlError::TooComplex);
    }
    // Вложенные скобки разборщик сам ограничивает глубиной 50.
    let statements = Parser::new(&dialect_in)
        .with_tokens_with_locations(tokens)
        .parse_statements()
        .map_err(|e| SqlError::Parse(e.to_string()))?;
    if statements.len() != 1 {
        return Err(SqlError::NotOneStatement {
            count: statements.len(),
        });
    }
    match statements.into_iter().next() {
        Some(Statement::Query(q)) => translate::run(*q, params, config, can_read, dialect),
        _ => Err(SqlError::OnlySelect),
    }
}

#[cfg(test)]
mod tests;
