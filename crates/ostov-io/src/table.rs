//! Прочитанная таблица — общий результат `read_xlsx` и `read_xml`.

use std::fmt;

/// Значение ячейки. Число хранится текстом (`2.5`): точное значение без `f64` дальше границы файла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Text(String),
    Number(String),
    /// `ГГГГ-ММ-ДД`.
    Date(String),
    Bool(bool),
    /// Ошибка формулы Excel (`#ДЕЛ/0!`).
    Error(String),
}

impl Cell {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Text(text) => text.trim().is_empty(),
            Self::Number(_) | Self::Date(_) | Self::Bool(_) | Self::Error(_) => false,
        }
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => Ok(()),
            Self::Text(text) | Self::Number(text) | Self::Date(text) | Self::Error(text) => {
                write!(f, "{text}")
            }
            Self::Bool(flag) => write!(f, "{}", if *flag { "Да" } else { "Нет" }),
        }
    }
}

/// Лист: заголовки (первая строка) и строки данных.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Все листы книги; прочитан `sheet`.
    pub sheets: Vec<String>,
    pub sheet: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// Номер первой строки данных, как его видит человек (в xlsx обычно 2).
    pub first_row: usize,
}

impl Table {
    /// Номер строки `index` для сообщений: как в Excel.
    pub fn row_number(&self, index: usize) -> usize {
        self.first_row + index
    }
}
