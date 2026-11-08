//! XML (СТ-42): `<объект><item><поле>…</поле></item></объект>`. DTD и свои сущности запрещены,
//! внешние файлы не читаются никогда (НТ-Б4).

use std::path::Path;

use ostov_api::{Engine, ListQuery, Value};
use quick_xml::events::{BytesDecl, BytesText, Event};
use quick_xml::{Reader, Writer};

use crate::import::find_object;
use crate::values::decimal_text;
use crate::xlsx::check_file_size;
use crate::{Cell, IoError, Limits, Table};

/// Окно чтения списка: больше ядро не отдаёт за раз (СТ-51).
const PAGE_SIZE: u32 = 1000;
const ITEM: &str = "item";
/// Корень → запись → поле: глубже — вложенные элементы в поле, их не бывает.
const FIELD_DEPTH: usize = 3;

/// Читает записи: заголовки — имена дочерних элементов записей в порядке появления.
///
/// # Errors
/// Битый XML, DTD или сущность не из пяти стандартных, вложенность или лимиты превышены.
pub fn read_xml(path: &Path, limits: &Limits) -> Result<Table, IoError> {
    check_file_size(path, limits)?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| IoError::File(format!("{}: {e}", path.display())))?;
    parse(&text, limits)
}

fn parse(text: &str, limits: &Limits) -> Result<Table, IoError> {
    // trim_text не включаем: он обрезает каждый кусок текста отдельно и съедает пробелы
    // рядом с сущностями («Соль &amp; перец» → «Соль&перец»).
    let mut reader = Reader::from_str(text);
    let mut parser = Parser {
        limits,
        root: None,
        headers: Vec::new(),
        rows: Vec::new(),
        depth: 0,
        field: None,
        value: String::new(),
    };
    loop {
        let event = reader
            .read_event()
            .map_err(|e| broken(reader.error_position(), &e))?;
        match event {
            Event::DocType(_) => return Err(IoError::Invalid("XML с DTD не принимается".into())),
            Event::Start(element) => parser.start(element.name().as_ref())?,
            Event::Empty(element) => {
                parser.start(element.name().as_ref())?;
                parser.end()?;
            }
            Event::End(_) => parser.end()?,
            Event::Text(content) => parser.text(content.as_ref())?,
            Event::CData(content) => parser.text(content.as_ref())?,
            Event::GeneralRef(reference) => parser.text(&entity(&reference)?)?,
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
        }
    }
    let Some(root) = parser.root else {
        return Err(IoError::Invalid("в XML нет записей".into()));
    };
    Ok(Table {
        sheets: vec![root.clone()],
        sheet: root,
        headers: parser.headers,
        rows: parser.rows,
        // Строка в сообщениях — номер записи, с 1.
        first_row: 1,
    })
}

/// Разбор по уровням: 1 — корень, 2 — запись, 3 — поле.
struct Parser<'a> {
    limits: &'a Limits,
    root: Option<String>,
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
    depth: usize,
    /// Колонка текущего поля.
    field: Option<usize>,
    value: String,
}

impl Parser<'_> {
    fn start(&mut self, name: &str) -> Result<(), IoError> {
        self.depth += 1;
        if self.depth > self.limits.max_xml_depth {
            return Err(IoError::Limit(format!(
                "вложенность XML глубже {}",
                self.limits.max_xml_depth
            )));
        }
        match self.depth {
            1 => self.root = Some(name.to_owned()),
            2 => {
                if self.rows.len() >= self.limits.max_rows {
                    return Err(IoError::Limit(format!(
                        "записей больше {}",
                        self.limits.max_rows
                    )));
                }
                self.rows.push(vec![Cell::Empty; self.headers.len()]);
            }
            FIELD_DEPTH => {
                let column = match self.headers.iter().position(|h| h == name) {
                    Some(column) => column,
                    None => {
                        self.headers.push(name.to_owned());
                        self.headers.len() - 1
                    }
                };
                self.field = Some(column);
                self.value.clear();
            }
            _ => {
                return Err(IoError::Invalid(format!(
                    "запись {}: в поле вложенный элемент <{name}>",
                    self.rows.len()
                )));
            }
        }
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), IoError> {
        if self.field.is_none() {
            return Ok(());
        }
        self.value.push_str(text);
        if self.value.chars().count() > self.limits.max_cell_chars {
            return Err(IoError::Limit(format!(
                "запись {}: значение длиннее {} символов",
                self.rows.len(),
                self.limits.max_cell_chars
            )));
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), IoError> {
        if self.depth == FIELD_DEPTH
            && let (Some(column), Some(row)) = (self.field.take(), self.rows.last_mut())
        {
            if row.len() <= column {
                row.resize(column + 1, Cell::Empty);
            }
            let value = std::mem::take(&mut self.value);
            row[column] = if value.trim().is_empty() {
                Cell::Empty
            } else {
                Cell::Text(value)
            };
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }
}

/// Только пять стандартных сущностей и коды символов (`&#1052;`); остальное — ошибка.
fn entity(reference: &quick_xml::events::BytesRef<'_>) -> Result<String, IoError> {
    if reference.is_char_ref() {
        return match reference.resolve_char_ref() {
            Ok(Some(c)) => Ok(c.to_string()),
            _ => Err(IoError::Invalid(format!(
                "неверный код символа &{};",
                &**reference
            ))),
        };
    }
    let text = match &**reference {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "apos" => "'",
        "quot" => "\"",
        other => {
            return Err(IoError::Invalid(format!(
                "сущность &{other}; не принимается: допустимы только &lt; &gt; &amp; &apos; &quot;"
            )));
        }
    };
    Ok(text.to_owned())
}

fn broken(position: u64, error: &quick_xml::Error) -> IoError {
    IoError::Invalid(format!("XML не разобран (позиция {position}): {error}"))
}

/// Выгружает список: корень — имя объекта, записи `<item>`, внутри поля объекта по `name`.
/// Значения — текстом, как в xlsx: дата `ДД.ММ.ГГГГ`, ссылка — название, признак — Да/Нет.
///
/// # Errors
/// Ошибка ядра, нет такого объекта, файл не записался.
pub fn export_list_xml(
    engine: &dyn Engine,
    query: &ListQuery,
    path: &Path,
) -> Result<usize, IoError> {
    let meta = engine.get_meta()?;
    let object = find_object(&meta, &query.object)?;
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    let io = |e: std::io::Error| IoError::File(format!("{}: {e}", path.display()));
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(io)?;
    let mut count = 0;
    let mut page_query = ListQuery {
        size: PAGE_SIZE,
        after_key: None,
        ..query.clone()
    };
    let mut items = Vec::new();
    loop {
        let page = engine.list(&page_query)?;
        items.extend(page.rows.into_iter().map(|row| row.values));
        match page.next_key {
            Some(key) => page_query.after_key = Some(key),
            None => break,
        }
    }
    writer
        .create_element(object.name.as_str())
        .write_inner_content(|writer| {
            for values in &items {
                writer.create_element(ITEM).write_inner_content(|writer| {
                    for field in &object.fields {
                        let Some(text) = values.get(&field.name).and_then(text_value) else {
                            continue;
                        };
                        writer
                            .create_element(field.name.as_str())
                            .write_text_content(BytesText::new(&text))?;
                    }
                    Ok(())
                })?;
                count += 1;
            }
            Ok(())
        })
        .map_err(io)?;
    std::fs::write(path, writer.into_inner()).map_err(io)?;
    Ok(count)
}

fn text_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Int(v) => Some(v.to_string()),
        Value::Text(text) => Some(text.clone()),
        Value::Decimal { units, scale } => Some(decimal_text(*units, *scale)),
        Value::Bool(flag) => Some(if *flag { "Да" } else { "Нет" }.to_owned()),
        Value::Date(date) => Some(dotted(date)),
        Value::Ref { id, title } => Some(title.clone().unwrap_or_else(|| id.to_string())),
    }
}

/// `2026-01-10` → `10.01.2026`; не дата — как есть.
fn dotted(date: &str) -> String {
    let mut parts = date.split('-');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(year), Some(month), Some(day), None) => format!("{day}.{month}.{year}"),
        _ => date.to_owned(),
    }
}
