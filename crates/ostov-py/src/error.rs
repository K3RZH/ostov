use std::fmt;
use std::path::PathBuf;

/// Ошибка загрузки `.py`: «файл:строка — причина», как в `ostov check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyLoadError {
    pub file: PathBuf,
    /// Строка, считая с 1; `None`, когда место в файле неизвестно.
    pub line: Option<u64>,
    pub reason: String,
}

impl PyLoadError {
    pub fn new(file: impl Into<PathBuf>, line: Option<u64>, reason: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for PyLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = self.file.display();
        match self.line {
            Some(line) => write!(f, "{file}:{line} — {}", self.reason),
            None => write!(f, "{file} — {}", self.reason),
        }
    }
}

impl std::error::Error for PyLoadError {}
