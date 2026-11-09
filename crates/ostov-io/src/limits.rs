/// Лимиты на входной файл (НТ-Б4): превышение — понятная ошибка, а не падение или зависание.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_file_bytes: u64,
    /// Распакованный xlsx целиком.
    pub max_unpacked_bytes: u64,
    /// Степень сжатия любого файла внутри xlsx.
    pub max_ratio: u64,
    /// НТ-П9.
    pub max_rows: usize,
    pub max_cell_chars: usize,
    pub max_xml_depth: usize,
}

const MB: u64 = 1024 * 1024;

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 50 * MB,
            max_unpacked_bytes: 200 * MB,
            max_ratio: 100,
            max_rows: 100_000,
            max_cell_chars: 10_000,
            max_xml_depth: 32,
        }
    }
}
