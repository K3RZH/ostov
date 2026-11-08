//! Чтение xlsx через calamine.

use std::path::Path;

use calamine::{Data, Reader, Xlsx, open_workbook};

use crate::values::excel_day_to_iso;
use crate::{Cell, IoError, Limits, Table};

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
    let range = book.worksheet_range(&name).map_err(|e| broken(path, &e))?;
    let height = range.height();
    if height.saturating_sub(1) > limits.max_rows {
        return Err(IoError::Limit(format!(
            "строк больше {}: в листе «{name}» их {}",
            limits.max_rows,
            height - 1
        )));
    }
    let header_row = range.start().map_or(0, |(row, _)| row);
    let mut rows = range.rows();
    let headers = rows
        .next()
        .map(|cells| {
            cells
                .iter()
                .map(|c| cell(c).to_string().trim().to_owned())
                .collect()
        })
        .unwrap_or_default();
    let first_row = usize::try_from(header_row)
        .unwrap_or(usize::MAX)
        .saturating_add(2);
    let mut data = Vec::with_capacity(height.saturating_sub(1));
    for (index, cells) in rows.enumerate() {
        let row: Vec<Cell> = cells.iter().map(cell).collect();
        if let Some(column) = row.iter().position(|c| too_long(c, limits)) {
            return Err(IoError::Limit(format!(
                "строка {}, колонка {}: ячейка длиннее {} символов",
                first_row + index,
                column + 1,
                limits.max_cell_chars
            )));
        }
        data.push(row);
    }
    Ok(Table {
        sheets,
        sheet: name,
        headers,
        rows: data,
        first_row,
    })
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

fn too_long(cell: &Cell, limits: &Limits) -> bool {
    match cell {
        Cell::Text(text) | Cell::Error(text) => text.chars().count() > limits.max_cell_chars,
        Cell::Empty | Cell::Number(_) | Cell::Date(_) | Cell::Bool(_) => false,
    }
}

fn cell(data: &Data) -> Cell {
    match data {
        Data::Empty => Cell::Empty,
        Data::String(text) => Cell::Text(text.clone()),
        Data::Int(v) => Cell::Number(v.to_string()),
        // Display у f64 даёт кратчайшую запись, которая читается обратно тем же числом: 2.5, 0.1.
        Data::Float(v) => Cell::Number(v.to_string()),
        Data::Bool(flag) => Cell::Bool(*flag),
        Data::DateTime(date) if !date.is_duration() => excel_date(date.as_f64())
            .map_or_else(|| Cell::Number(date.as_f64().to_string()), Cell::Date),
        Data::DateTime(date) => Cell::Number(date.as_f64().to_string()),
        Data::DateTimeIso(text) => Cell::Date(text.chars().take(10).collect()),
        Data::DurationIso(text) => Cell::Text(text.clone()),
        Data::Error(error) => Cell::Error(error.to_string()),
    }
}

/// День Excel из числа с дробной частью-временем.
fn excel_date(serial: f64) -> Option<String> {
    let day: i64 = serial.to_string().split('.').next()?.parse().ok()?;
    excel_day_to_iso(day)
}

fn broken(path: &Path, error: &dyn std::fmt::Display) -> IoError {
    IoError::File(format!(
        "{}: не удалось прочитать xlsx ({error})",
        path.display()
    ))
}
