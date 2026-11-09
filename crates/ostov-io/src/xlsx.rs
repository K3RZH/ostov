//! Чтение xlsx через calamine.

use std::path::Path;

use calamine::{DataRef, Reader, Xlsx, open_workbook};

use crate::values::{excel_day_to_iso, number_text};
use crate::{Cell, IoError, Limits, Table};

/// Степень сжатия проверяется у файлов больше этого: маленький файл бомбой не бывает,
/// а мелкие служебные XML сжимаются сильно и дали бы ложный отказ.
const RATIO_CHECK_FROM: u64 = 1024 * 1024;
/// В настоящем xlsx десятки файлов; тысячи — уже атака на оглавление.
const MAX_ENTRIES: usize = 10_000;
/// Колонок в строке заголовков: таблица для импорта шире не бывает.
const MAX_COLUMNS: usize = 1024;
/// Хранимых ячеек в листе (100 000 строк × 100 колонок); защита памяти от разреженных листов.
const MAX_CELLS: usize = 10_000_000;

/// Читает первый лист: заголовки и строки. Список всех листов — в `Table::sheets`.
///
/// # Errors
/// Файл не xlsx или битый, превышен лимит (`Limits`).
pub fn read_xlsx(path: &Path, limits: &Limits) -> Result<Table, IoError> {
    read_xlsx_sheet(path, None, limits)
}

/// Читает лист `sheet` (`None` — первый).
///
/// # Errors
/// Как у `read_xlsx`, и ещё нет такого листа.
pub fn read_xlsx_sheet(
    path: &Path,
    sheet: Option<&str>,
    limits: &Limits,
) -> Result<Table, IoError> {
    check_file_size(path, limits)?;
    check_archive(path, limits)?;
    let mut book: Xlsx<_> = open_workbook(path).map_err(|e| broken(path, &e))?;
    let sheets = book.sheet_names();
    let name = match sheet {
        Some(name) if sheets.iter().any(|s| s == name) => name.to_owned(),
        Some(name) => return Err(IoError::Invalid(format!("в книге нет листа «{name}»"))),
        None => sheets
            .first()
            .cloned()
            .ok_or_else(|| IoError::Invalid("в книге нет листов".into()))?,
    };
    let (headers, rows, header_row) =
        read_cells(&mut book, &name, limits).map_err(|e| match e {
            Read::Calamine(e) => broken(path, &e),
            Read::Limit(reason) => IoError::Limit(reason),
        })?;
    let first_row = usize::try_from(header_row)
        .unwrap_or(usize::MAX)
        .saturating_add(2);
    Ok(Table {
        sheets,
        sheet: name,
        headers,
        rows,
        first_row,
    })
}

/// Заголовки, строки данных и номер строки заголовков (с нуля).
type Cells = (Vec<String>, Vec<Vec<Cell>>, u32);

enum Read {
    Calamine(calamine::XlsxError),
    Limit(String),
}

/// Ячейки потоком, без плотного листа: `calamine::worksheet_range` строит прямоугольник
/// ширина × высота, и файл в 1 КБ с ячейкой в XFD1048576 запросил бы миллиарды ячеек.
/// Отказ — как только номер строки или число хранимых ячеек выходит за лимит.
fn read_cells<R: std::io::Read + std::io::Seek>(
    book: &mut Xlsx<R>,
    name: &str,
    limits: &Limits,
) -> Result<Cells, Read> {
    let mut reader = book.worksheet_cells_reader(name).map_err(Read::Calamine)?;
    let mut header_row: Option<u32> = None;
    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut stored: usize = 0;
    while let Some(found) = reader.next_cell().map_err(Read::Calamine)? {
        let (row, column) = found.get_position();
        let value = cell(found.get_value());
        let header = *header_row.get_or_insert(row);
        let column = usize::try_from(column).unwrap_or(usize::MAX);
        if row == header {
            if column >= MAX_COLUMNS {
                return Err(Read::Limit(format!("колонок больше {MAX_COLUMNS}")));
            }
            if headers.len() <= column {
                headers.resize(column + 1, String::new());
            }
            headers[column] = value.to_string().trim().to_owned();
            continue;
        }
        let index = usize::try_from(row - header - 1).unwrap_or(usize::MAX);
        if index >= limits.max_rows {
            return Err(Read::Limit(format!(
                "строк больше {}: в листе «{name}» есть строка {}",
                limits.max_rows,
                u64::from(row) + 1
            )));
        }
        // Ячейки правее заголовков импорту не нужны — и память на них не тратим.
        if column >= headers.len() || value == Cell::Empty {
            continue;
        }
        if too_long(&value, limits) {
            return Err(Read::Limit(format!(
                "строка {}, колонка {}: ячейка длиннее {} символов",
                u64::from(row) + 1,
                column + 1,
                limits.max_cell_chars
            )));
        }
        if rows.len() <= index {
            rows.resize(index + 1, Vec::new());
        }
        let line = &mut rows[index];
        if line.len() <= column {
            stored = stored.saturating_add(column + 1 - line.len());
            if stored > MAX_CELLS {
                return Err(Read::Limit(format!("в листе больше {MAX_CELLS} ячеек")));
            }
            line.resize(column + 1, Cell::Empty);
        }
        line[column] = value;
    }
    Ok((headers, rows, header_row.unwrap_or_default()))
}

pub(crate) fn check_file_size(path: &Path, limits: &Limits) -> Result<(), IoError> {
    let size = std::fs::metadata(path)
        .map_err(|e| IoError::File(format!("{}: {e}", path.display())))?
        .len();
    if size > limits.max_file_bytes {
        return Err(IoError::Limit(format!(
            "файл больше {} МБ",
            limits.max_file_bytes / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Оглавление zip до распаковки: суммарный размер и степень сжатия каждого файла (zip-бомба).
fn check_archive(path: &Path, limits: &Limits) -> Result<(), IoError> {
    let file =
        std::fs::File::open(path).map_err(|e| IoError::File(format!("{}: {e}", path.display())))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| IoError::File(format!("{}: не xlsx — не zip-архив ({e})", path.display())))?;
    if archive.len() > MAX_ENTRIES {
        return Err(IoError::Limit(format!(
            "в xlsx больше {MAX_ENTRIES} файлов"
        )));
    }
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(|e| {
            IoError::File(format!("{}: битое оглавление xlsx ({e})", path.display()))
        })?;
        let size = entry.size();
        total = total.saturating_add(size);
        let ratio = size / entry.compressed_size().max(1);
        if size > RATIO_CHECK_FROM && ratio > limits.max_ratio {
            return Err(IoError::Limit(format!(
                "файл {} внутри xlsx сжат в {ratio} раз (допустимо {}): похоже на zip-бомбу",
                entry.name().unwrap_or_default(),
                limits.max_ratio
            )));
        }
        if total > limits.max_unpacked_bytes {
            return Err(IoError::Limit(format!(
                "распакованный xlsx больше {} МБ",
                limits.max_unpacked_bytes / (1024 * 1024)
            )));
        }
    }
    Ok(())
}

fn too_long(cell: &Cell, limits: &Limits) -> bool {
    match cell {
        Cell::Text(text) | Cell::Error(text) => text.chars().count() > limits.max_cell_chars,
        Cell::Empty | Cell::Number(_) | Cell::Date(_) | Cell::Bool(_) => false,
    }
}

fn cell(data: &DataRef<'_>) -> Cell {
    match data {
        DataRef::Empty => Cell::Empty,
        DataRef::String(text) => Cell::Text(text.clone()),
        DataRef::SharedString(text) => Cell::Text((*text).to_owned()),
        DataRef::Int(v) => Cell::Number(v.to_string()),
        DataRef::Float(v) => Cell::Number(number_text(*v)),
        DataRef::Bool(flag) => Cell::Bool(*flag),
        DataRef::DateTime(date) if !date.is_duration() => excel_date(date.as_f64())
            .map_or_else(|| Cell::Number(number_text(date.as_f64())), Cell::Date),
        DataRef::DateTime(date) => Cell::Number(number_text(date.as_f64())),
        DataRef::DateTimeIso(text) => Cell::Date(text.chars().take(10).collect()),
        DataRef::DurationIso(text) => Cell::Text(text.clone()),
        DataRef::Error(error) => Cell::Error(error.to_string()),
    }
}

/// День Excel из числа с дробной частью-временем.
fn excel_date(serial: f64) -> Option<String> {
    let day: i64 = number_text(serial).split('.').next()?.parse().ok()?;
    excel_day_to_iso(day)
}

fn broken(path: &Path, error: &dyn std::fmt::Display) -> IoError {
    IoError::File(format!(
        "{}: не удалось прочитать xlsx ({error})",
        path.display()
    ))
}
