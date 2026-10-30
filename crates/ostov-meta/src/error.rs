use std::fmt;
use std::path::PathBuf;

/// Ошибка в файле конфигурации: где и почему.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub file: PathBuf,
    /// Строка, считая с 1. `None`, когда место неизвестно: файл не открылся или слишком большой.
    pub line: Option<u64>,
    pub reason: String,
}

impl Error {
    pub fn new(file: impl Into<PathBuf>, line: Option<u64>, reason: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = self.file.display();
        match self.line {
            Some(line) => write!(f, "{file}:{line} — {}", self.reason),
            None => write!(f, "{file} — {}", self.reason),
        }
    }
}

impl std::error::Error for Error {}
