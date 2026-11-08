//! Выгрузка списка и отчёта в xlsx (СТ-41).

use std::collections::BTreeMap;
use std::path::Path;

use ostov_api::{Engine, ListQuery, Meta, ObjectKind, ObjectMeta, ReportQuery, Value};
use rust_xlsxwriter::{Format, Workbook, Worksheet};

use crate::IoError;
use crate::values::{decimal_format, decimal_number, excel_day};

/// Окно чтения списка и отчёта: больше ядро не отдаёт за раз (СТ-51).
const PAGE_SIZE: u32 = 1000;
/// Лимит Excel на имя листа.
const MAX_SHEET_NAME: usize = 31;
const DATE_FORMAT: &str = "dd.mm.yyyy";
/// Суффиксы колонок оборотов в порядке показа и их подписи.
const TURNOVER: [(&str, &str); 4] = [
    ("_opening", "начало"),
    ("_receipt", "приход"),
    ("_issue", "расход"),
    ("_closing", "конец"),
];

/// Колонка выгрузки: ключ в строке и заголовок.
struct Column {
    key: String,
    title: String,
    /// Признак «Да/Нет»: ядро отдаёт `posted` и `deleted` числом 0/1.
    flag: bool,
}

/// Выгружает список целиком (все страницы, с отбором, поиском и сортировкой из `query`).
/// Возвращает число строк.
///
/// # Errors
/// Ошибка ядра (права, неверный запрос), нет такого объекта, файл не записался.
pub fn export_list(engine: &dyn Engine, query: &ListQuery, path: &Path) -> Result<usize, IoError> {
    let meta = engine.get_meta()?;
    let object = find_object(&meta, &query.object)?;
    let columns = list_columns(object);
    let mut sheet = Sheet::new(&object.title, &columns);
    let mut page_query = ListQuery {
        size: PAGE_SIZE,
        after_key: None,
        ..query.clone()
    };
    loop {
        let page = engine.list(&page_query)?;
        for row in &page.rows {
            sheet.add_row(&columns, &row.values)?;
        }
        match page.next_key {
            Some(key) => page_query.after_key = Some(key),
            None => break,
        }
    }
    sheet.save(path)
}

/// Выгружает отчёт целиком. Колонки — по ключам первой строки: измерения, затем ресурсы.
///
/// # Errors
/// Ошибка ядра (права, неверные параметры), файл не записался.
pub fn export_report(
    engine: &dyn Engine,
    query: &ReportQuery,
    path: &Path,
) -> Result<usize, IoError> {
    let meta = engine.get_meta()?;
    let mut page_query = ReportQuery {
        size: PAGE_SIZE,
        after_key: None,
        ..query.clone()
    };
    let mut rows = Vec::new();
    loop {
        let page = engine.run_report(&page_query)?;
        rows.extend(page.rows.into_iter().map(|row| row.values));
        match page.next_key {
            Some(key) => page_query.after_key = Some(key),
            None => break,
        }
    }
    let columns = rows
        .first()
        .map(|first| report_columns(&meta, first))
        .unwrap_or_default();
    let mut sheet = Sheet::new(&query.report, &columns);
    for row in &rows {
        sheet.add_row(&columns, row)?;
    }
    sheet.save(path)
}

fn find_object<'a>(meta: &'a Meta, name: &str) -> Result<&'a ObjectMeta, IoError> {
    meta.objects
        .iter()
        .find(|object| object.name == name)
        .ok_or_else(|| IoError::Invalid(format!("нет объекта «{name}»")))
}

/// «Номер», поля шапки по метаданным, «Проведён», «Помечен»; у справочника — поля и «Помечен».
fn list_columns(object: &ObjectMeta) -> Vec<Column> {
    let column = |key: &str, title: &str| Column {
        key: key.to_owned(),
        title: title.to_owned(),
        flag: matches!(key, "posted" | "deleted"),
    };
    let document = object.kind == ObjectKind::Document;
    let mut columns = Vec::new();
    if document {
        columns.push(column("number", "Номер"));
    }
    columns.extend(object.fields.iter().map(|f| column(&f.name, &f.title)));
    if document {
        columns.push(column("posted", "Проведён"));
    }
    columns.push(column("deleted", "Помечен"));
    columns
}

/// Измерения (ссылки и прочие не-числа) по имени, затем ресурсы: значение или
/// начало, приход, расход, конец. Заголовки — по полям с тем же именем в метаданных.
fn report_columns(meta: &Meta, first: &ostov_api::Fields) -> Vec<Column> {
    let titles = field_titles(meta);
    let title = |name: &str| titles.get(name).cloned().unwrap_or_else(|| name.to_owned());
    let mut dimensions = Vec::new();
    let mut resources: Vec<(usize, Column)> = Vec::new();
    for (key, value) in first {
        let turnover = TURNOVER
            .iter()
            .enumerate()
            .find_map(|(i, (suffix, label))| {
                key.strip_suffix(suffix).map(|base| (i + 1, base, *label))
            });
        let column = match turnover {
            Some((order, base, label)) => (order, format!("{}: {label}", title(base))),
            None if matches!(value, Value::Decimal { .. }) => (0, title(key)),
            None => {
                dimensions.push(Column {
                    key: key.clone(),
                    title: title(key),
                    flag: false,
                });
                continue;
            }
        };
        resources.push((
            column.0,
            Column {
                key: key.clone(),
                title: column.1,
                flag: false,
            },
        ));
    }
    resources.sort_by_key(|(order, _)| *order);
    dimensions.extend(resources.into_iter().map(|(_, column)| column));
    dimensions
}

/// Заголовки полей всех объектов: имя поля → первый встреченный `title`.
fn field_titles(meta: &Meta) -> BTreeMap<String, String> {
    let mut titles = BTreeMap::new();
    for object in &meta.objects {
        let parts = object.table_parts.iter().flat_map(|part| &part.fields);
        for field in object.fields.iter().chain(parts) {
            titles
                .entry(field.name.clone())
                .or_insert_with(|| field.title.clone());
        }
    }
    titles
}

/// Лист с закреплённой строкой заголовков и форматами чисел и дат.
struct Sheet {
    workbook: Workbook,
    rows: u32,
    date: Format,
    decimals: BTreeMap<u8, Format>,
}

impl Sheet {
    fn new(name: &str, columns: &[Column]) -> Self {
        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        // Имя листа — для удобства; если Excel его не примет, остаётся стандартное.
        let _ = sheet.set_name(sheet_name(name));
        let header = Format::new().set_bold();
        for (col, column) in (0u16..).zip(columns) {
            let _ = sheet.write_string_with_format(0, col, &column.title, &header);
        }
        let _ = sheet.set_freeze_panes(1, 0);
        Self {
            workbook,
            rows: 0,
            date: Format::new().set_num_format(DATE_FORMAT),
            decimals: BTreeMap::new(),
        }
    }

    fn add_row(&mut self, columns: &[Column], values: &ostov_api::Fields) -> Result<(), IoError> {
        self.rows += 1;
        let row = self.rows;
        for (col, column) in (0u16..).zip(columns) {
            let value = match values.get(&column.key) {
                Some(Value::Int(v)) if column.flag => &Value::Bool(*v != 0),
                Some(value) => value,
                None => continue,
            };
            self.write(row, col, value)?;
        }
        Ok(())
    }

    fn write(&mut self, row: u32, col: u16, value: &Value) -> Result<(), IoError> {
        let decimals = &mut self.decimals;
        let sheet = sheet(&mut self.workbook)?;
        match value {
            Value::Null => {}
            Value::Int(v) => {
                // Целые до 2^53 Excel хранит точно; больших в учёте нет, а если есть — текстом.
                if v.unsigned_abs() < (1 << 53) {
                    sheet.write_number(
                        row,
                        col,
                        v.to_string().parse::<f64>().unwrap_or_default(),
                    )?;
                } else {
                    sheet.write_string(row, col, v.to_string())?;
                }
            }
            Value::Text(text) => {
                sheet.write_string(row, col, text)?;
            }
            Value::Decimal { units, scale } => {
                let format = decimals
                    .entry(*scale)
                    .or_insert_with(|| Format::new().set_num_format(decimal_format(*scale)));
                match decimal_number(*units, *scale) {
                    Some(number) => sheet.write_number_with_format(row, col, number, format)?,
                    None => {
                        sheet.write_string(row, col, crate::values::decimal_text(*units, *scale))?
                    }
                };
            }
            Value::Bool(flag) => {
                sheet.write_string(row, col, if *flag { "Да" } else { "Нет" })?;
            }
            Value::Date(date) => {
                match excel_day(date).and_then(|day| day.to_string().parse::<f64>().ok()) {
                    Some(day) => {
                        sheet.write_number_with_format(row, col, day, &self.date)?;
                    }
                    None => {
                        sheet.write_string(row, col, date)?;
                    }
                }
            }
            Value::Ref { id, title } => {
                let text = title.clone().unwrap_or_else(|| id.to_string());
                sheet.write_string(row, col, text)?;
            }
        }
        Ok(())
    }

    fn save(mut self, path: &Path) -> Result<usize, IoError> {
        sheet(&mut self.workbook)?.autofit();
        self.workbook
            .save(path)
            .map_err(|e| IoError::File(format!("{}: {e}", path.display())))?;
        usize::try_from(self.rows).map_err(|_| IoError::Invalid("слишком много строк".into()))
    }
}

fn sheet(workbook: &mut Workbook) -> Result<&mut Worksheet, IoError> {
    workbook
        .worksheet_from_index(0)
        .map_err(|e| IoError::File(format!("xlsx: {e}")))
}

/// Имя листа без запрещённых Excel символов и не длиннее 31.
fn sheet_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
        .take(MAX_SHEET_NAME)
        .collect();
    if cleaned.trim().is_empty() {
        "Лист1".to_owned()
    } else {
        cleaned
    }
}
