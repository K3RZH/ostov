//! Импорт (СТ-40): сопоставление колонок с полями, проверка всех строк без записи, запись.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use ostov_api::{
    Engine, EngineError, FieldDef, FieldKind, Fields, ListQuery, Meta, ObjectData, ObjectKind,
    ObjectMeta, PostAction, Value,
};

use crate::values::{excel_day_to_iso, parse_date, parse_decimal};
use crate::{Cell, IoError, Table};

/// Окно поиска ссылки: столько совпадений по подстроке смотрим, чтобы найти точное.
const SEARCH_SIZE: u32 = 1000;
/// Поле-название справочника, по которому ищутся ссылки и повторы.
const NAME_FIELD: &str = "name";

/// Куда идёт колонка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Skip,
    /// Поле шапки (у справочника — его поле).
    Field(String),
    /// Поле строки табличной части документа.
    Line {
        part: String,
        field: String,
    },
}

/// Сопоставление колонок таблицы с полями объекта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub object: String,
    /// По колонке на каждый заголовок таблицы, в том же порядке.
    pub columns: Vec<Target>,
    /// Документ: строки с одинаковым значением в этой колонке — один документ.
    /// `None` — каждая строка отдельным документом.
    pub group_by: Option<usize>,
    /// Документ: провести после записи; иначе — черновик.
    pub post: bool,
}

/// Ошибка строки: «строка N, колонка «…»: причина». Номер строки — как в Excel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    pub row: usize,
    pub column: Option<String>,
    pub reason: String,
}

impl fmt::Display for RowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.column {
            Some(column) => write!(
                f,
                "строка {}, колонка «{column}»: {}",
                self.row, self.reason
            ),
            None => write!(f, "строка {}: {}", self.row, self.reason),
        }
    }
}

/// Объект, готовый к записи, и строки таблицы, из которых он собран.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub rows: Vec<usize>,
    pub data: ObjectData,
}

/// Результат проверки: что будет записано и что мешает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub object: String,
    pub post: bool,
    pub items: Vec<Planned>,
    pub errors: Vec<RowError>,
}

/// Запись упала на объекте из строк `rows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFailure {
    pub rows: Vec<usize>,
    pub error: EngineError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub saved: usize,
    pub ids: Vec<i64>,
    pub failure: Option<WriteFailure>,
}

/// Предложение по умолчанию: колонка — поле, если заголовок совпал с `title` или `name`
/// без учёта регистра и пробелов; сначала поля шапки, потом строк.
///
/// # Errors
/// Нет такого объекта.
pub fn suggest(meta: &Meta, object: &str, table: &Table) -> Result<Mapping, IoError> {
    let object_meta = find_object(meta, object)?;
    let columns = table
        .headers
        .iter()
        .map(|header| {
            let key = normalize(header);
            let same =
                |field: &FieldDef| normalize(&field.title) == key || normalize(&field.name) == key;
            if let Some(field) = object_meta.fields.iter().find(|f| same(f)) {
                return Target::Field(field.name.clone());
            }
            object_meta
                .table_parts
                .iter()
                .find_map(|part| {
                    part.fields
                        .iter()
                        .find(|f| same(f))
                        .map(|field| Target::Line {
                            part: part.name.clone(),
                            field: field.name.clone(),
                        })
                })
                .unwrap_or(Target::Skip)
        })
        .collect();
    Ok(Mapping {
        object: object.to_owned(),
        columns,
        group_by: None,
        post: false,
    })
}

/// Проверяет все строки и собирает объекты, ничего не записывая.
///
/// # Errors
/// Ошибка ядра при чтении метаданных, сопоставление не подходит к объекту (нет поля, колонки).
/// Ошибки данных — не `Err`, а `Plan::errors`.
pub fn plan(engine: &dyn Engine, table: &Table, mapping: &Mapping) -> Result<Plan, IoError> {
    let meta = engine.get_meta()?;
    let object = find_object(&meta, &mapping.object)?;
    let columns = resolve(object, table, mapping)?;
    let mut builder = Builder {
        table,
        columns: &columns,
        object,
        refs: Refs {
            engine,
            meta: &meta,
            found: HashMap::new(),
        },
        errors: Vec::new(),
        names: HashSet::new(),
    };
    let items = groups(table, mapping, object)
        .into_iter()
        .filter_map(|rows| builder.item(rows))
        .collect();
    Ok(Plan {
        object: mapping.object.clone(),
        post: mapping.post && object.kind == ObjectKind::Document,
        items,
        errors: builder.errors,
    })
}

/// Записывает проверенный план. Пока в `Engine` нет `save_batch`, объекты пишутся по одному:
/// при ошибке уже записанные остаются, а `Written::failure` говорит, на чём остановились.
///
/// # Errors
/// В плане есть ошибки — ничего не пишется.
pub fn write(engine: &dyn Engine, plan: &Plan) -> Result<Written, IoError> {
    if !plan.errors.is_empty() {
        return Err(IoError::Invalid(format!(
            "в данных ошибок: {} — запись не начата",
            plan.errors.len()
        )));
    }
    let action = if plan.post {
        PostAction::Post
    } else {
        PostAction::Write
    };
    let mut ids = Vec::with_capacity(plan.items.len());
    for item in &plan.items {
        match engine.save(&plan.object, &item.data, action) {
            Ok(saved) => ids.push(saved.id),
            Err(error) => {
                return Ok(Written {
                    saved: ids.len(),
                    ids,
                    failure: Some(WriteFailure {
                        rows: item.rows.clone(),
                        error,
                    }),
                });
            }
        }
    }
    Ok(Written {
        saved: ids.len(),
        ids,
        failure: None,
    })
}

pub(crate) fn find_object<'a>(meta: &'a Meta, name: &str) -> Result<&'a ObjectMeta, IoError> {
    meta.objects
        .iter()
        .find(|object| object.name == name)
        .ok_or_else(|| IoError::Invalid(format!("нет объекта «{name}»")))
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Колонка, привязанная к полю.
struct Column<'a> {
    index: usize,
    header: String,
    part: Option<&'a str>,
    field: &'a FieldDef,
}

fn resolve<'a>(
    object: &'a ObjectMeta,
    table: &Table,
    mapping: &Mapping,
) -> Result<Vec<Column<'a>>, IoError> {
    if mapping.columns.len() > table.headers.len() {
        return Err(IoError::Invalid(
            "в сопоставлении больше колонок, чем в таблице".into(),
        ));
    }
    let document = object.kind == ObjectKind::Document;
    if let Some(group) = mapping.group_by
        && (!document || group >= table.headers.len())
    {
        return Err(IoError::Invalid("колонка группировки не подходит".into()));
    }
    let mut columns = Vec::new();
    for (index, target) in mapping.columns.iter().enumerate() {
        let header = table.headers.get(index).cloned().unwrap_or_default();
        let (part, field) = match target {
            Target::Skip => continue,
            Target::Field(name) => (None, object.fields.iter().find(|f| &f.name == name)),
            Target::Line { part, field } if document => {
                let part = object.table_parts.iter().find(|p| &p.name == part);
                (
                    part.map(|p| p.name.as_str()),
                    part.and_then(|p| p.fields.iter().find(|f| &f.name == field)),
                )
            }
            Target::Line { .. } => (None, None),
        };
        let Some(field) = field else {
            return Err(IoError::Invalid(format!(
                "колонке «{header}» сопоставлено поле, которого нет у «{}»",
                object.title
            )));
        };
        columns.push(Column {
            index,
            header,
            part,
            field,
        });
    }
    Ok(columns)
}

/// Строки по объектам: у документа — по значению колонки группировки, иначе по одной.
/// Совсем пустые строки (их часто оставляют для красоты) пропускаются.
fn groups(table: &Table, mapping: &Mapping, object: &ObjectMeta) -> Vec<Vec<usize>> {
    let filled = table
        .rows
        .iter()
        .enumerate()
        .filter(|(_, cells)| cells.iter().any(|c| !c.is_empty()));
    let group = mapping
        .group_by
        .filter(|_| object.kind == ObjectKind::Document);
    let Some(group) = group else {
        return filled.map(|(row, _)| vec![row]).collect();
    };
    let mut order: Vec<Vec<usize>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (row, cells) in filled {
        let key = cells
            .get(group)
            .map(|c| c.to_string().trim().to_owned())
            .unwrap_or_default();
        if key.is_empty() {
            order.push(vec![row]);
            continue;
        }
        match index.get(&key) {
            Some(&at) => order[at].push(row),
            None => {
                index.insert(key, order.len());
                order.push(vec![row]);
            }
        }
    }
    order
}

struct Builder<'a> {
    table: &'a Table,
    columns: &'a [Column<'a>],
    object: &'a ObjectMeta,
    refs: Refs<'a>,
    errors: Vec<RowError>,
    /// Названия элементов справочника из этого файла: повтор — ошибка.
    names: HashSet<String>,
}

impl Builder<'_> {
    /// Объект из строк группы; `None`, если в нём ошибки (они уже в `errors`).
    fn item(&mut self, rows: Vec<usize>) -> Option<Planned> {
        let errors_before = self.errors.len();
        let first = *rows.first()?;
        let mut data = ObjectData::default();
        // Поля с ошибкой разбора: «не заполнено» про них было бы второй ошибкой о том же.
        let mut broken = HashSet::new();
        for column in self.columns.iter().filter(|c| c.part.is_none()) {
            match self.value(first, column) {
                Some(value) => {
                    data.fields.insert(column.field.name.clone(), value);
                }
                None => {
                    broken.insert(column.field.name.as_str());
                }
            }
        }
        self.check_required(first, &self.object.fields, &data.fields, None, &broken);
        let mut lines: BTreeMap<String, Vec<Fields>> = BTreeMap::new();
        for &row in &rows {
            let mut by_part: BTreeMap<&str, Fields> = BTreeMap::new();
            let mut broken = HashSet::new();
            for column in self.columns {
                let Some(part) = column.part else { continue };
                let entry = by_part.entry(part).or_default();
                match self.value(row, column) {
                    Some(value) => {
                        entry.insert(column.field.name.clone(), value);
                    }
                    None => {
                        broken.insert(column.field.name.as_str());
                    }
                }
            }
            for (part, line) in by_part {
                if broken.is_empty() && line.values().all(|v| *v == Value::Null) {
                    continue;
                }
                let part_fields = self
                    .object
                    .table_parts
                    .iter()
                    .find(|p| p.name == part)
                    .map(|p| p.fields.as_slice())
                    .unwrap_or_default();
                self.check_required(row, part_fields, &line, Some(part), &broken);
                lines.entry(part.to_owned()).or_default().push(line);
            }
        }
        data.tables = lines;
        if self.object.kind == ObjectKind::Catalog {
            self.check_new_name(first, &data.fields);
        }
        (self.errors.len() == errors_before).then_some(Planned {
            rows: rows.iter().map(|&r| self.table.row_number(r)).collect(),
            data,
        })
    }

    /// Значение ячейки по типу поля; ошибка уходит в `errors`, а не в результат.
    fn value(&mut self, row: usize, column: &Column<'_>) -> Option<Value> {
        let cell = self
            .table
            .rows
            .get(row)?
            .get(column.index)
            .unwrap_or(&Cell::Empty);
        match convert(cell, column.field, &mut self.refs) {
            Ok(value) => Some(value),
            Err(reason) => {
                self.errors.push(RowError {
                    row: self.table.row_number(row),
                    column: Some(column.header.clone()),
                    reason,
                });
                None
            }
        }
    }

    fn check_required(
        &mut self,
        row: usize,
        fields: &[FieldDef],
        values: &Fields,
        part: Option<&str>,
        broken: &HashSet<&str>,
    ) {
        for field in fields
            .iter()
            .filter(|f| f.required && !broken.contains(f.name.as_str()))
        {
            if matches!(values.get(&field.name), None | Some(Value::Null)) {
                let header = self
                    .columns
                    .iter()
                    .find(|c| c.part == part && c.field.name == field.name)
                    .map(|c| c.header.clone());
                self.errors.push(RowError {
                    row: self.table.row_number(row),
                    column: header,
                    reason: format!("не заполнено обязательное поле «{}»", field.title),
                });
            }
        }
    }

    /// В R1 импорт только создаёт: элемент с тем же названием уже есть — ошибка.
    fn check_new_name(&mut self, row: usize, fields: &Fields) {
        let Some(Value::Text(name)) = fields.get(NAME_FIELD) else {
            return;
        };
        let key = name.trim().to_lowercase();
        let reason = if !self.names.insert(key) {
            Some(format!("«{name}» уже есть выше в этом файле"))
        } else {
            match self.refs.find(&self.object.name, name) {
                Ok(_) | Err(Lookup::Ambiguous(_)) => {
                    Some(format!("уже есть «{name}» в «{}»", self.object.title))
                }
                Err(Lookup::Missing(_)) => None,
                Err(Lookup::Failed(reason)) => Some(reason),
            }
        };
        if let Some(reason) = reason {
            let header = self
                .columns
                .iter()
                .find(|c| c.part.is_none() && c.field.name == NAME_FIELD)
                .map(|c| c.header.clone());
            self.errors.push(RowError {
                row: self.table.row_number(row),
                column: header,
                reason,
            });
        }
    }
}

fn convert(cell: &Cell, field: &FieldDef, refs: &mut Refs<'_>) -> Result<Value, String> {
    if cell.is_empty() {
        return Ok(Value::Null);
    }
    if let Cell::Error(error) = cell {
        return Err(format!("в ячейке ошибка формулы {error}"));
    }
    let text = cell.to_string().trim().to_owned();
    match field.kind {
        FieldKind::Text => Ok(Value::Text(text)),
        FieldKind::Int => parse_decimal(&text, 0)
            .map(Value::Int)
            .map_err(|_| format!("«{text}» — не целое число")),
        FieldKind::Decimal { scale } => {
            parse_decimal(&text, scale).map(|units| Value::Decimal { units, scale })
        }
        FieldKind::Date => date(cell, &text).map(Value::Date),
        FieldKind::Ref => {
            let target = field.target.as_deref().unwrap_or_default();
            refs.find(target, &text)
                .map(|id| Value::Ref { id, title: None })
                .map_err(|lookup| lookup.reason())
        }
    }
}

fn date(cell: &Cell, text: &str) -> Result<String, String> {
    let parsed = match cell {
        Cell::Date(date) => parse_date(date),
        // Дата Excel без формата даты — номер дня.
        // Время после точки отбрасываем: 46032.5 — это 11.01.2026, полдень.
        Cell::Number(number) => number
            .split('.')
            .next()
            .and_then(|day| day.parse::<i64>().ok())
            .and_then(excel_day_to_iso),
        _ => parse_date(text),
    };
    parsed.ok_or_else(|| format!("«{text}» — не дата: нужно ДД.ММ.ГГГГ или ГГГГ-ММ-ДД"))
}

enum Lookup {
    Missing(String),
    Ambiguous(String),
    Failed(String),
}

impl Lookup {
    fn reason(self) -> String {
        match self {
            Self::Missing(reason) | Self::Ambiguous(reason) | Self::Failed(reason) => reason,
        }
    }
}

/// Найденное по названию: id, «нет» или «неоднозначно». Ошибка поиска не кешируется.
#[derive(Clone)]
enum Found {
    One(i64),
    Missing(String),
    Ambiguous(String),
}

/// Поиск элементов по названию с кешем: (объект, название в нижнем регистре) → результат.
/// На 10 000 строк запросов столько, сколько разных названий.
struct Refs<'a> {
    engine: &'a dyn Engine,
    meta: &'a Meta,
    found: HashMap<(String, String), Found>,
}

impl Refs<'_> {
    fn find(&mut self, target: &str, title: &str) -> Result<i64, Lookup> {
        let title = title.trim();
        let key = (target.to_owned(), title.to_lowercase());
        let found = match self.found.get(&key) {
            Some(found) => found.clone(),
            None => {
                let found = self.search(target, title, &key.1)?;
                self.found.insert(key, found.clone());
                found
            }
        };
        match found {
            Found::One(id) => Ok(id),
            Found::Missing(reason) => Err(Lookup::Missing(reason)),
            Found::Ambiguous(reason) => Err(Lookup::Ambiguous(reason)),
        }
    }

    /// Все страницы поиска по подстроке, затем точное совпадение без учёта регистра.
    fn search(&self, target: &str, title: &str, lower: &str) -> Result<Found, Lookup> {
        let target_title = self
            .meta
            .objects
            .iter()
            .find(|o| o.name == target)
            .map_or_else(|| target.to_owned(), |o| o.title.clone());
        let mut query = ListQuery {
            object: target.to_owned(),
            search: Some(title.to_owned()),
            size: SEARCH_SIZE,
            ..ListQuery::default()
        };
        let mut matches = Vec::new();
        loop {
            let page = self.engine.list(&query).map_err(|e| {
                Lookup::Failed(format!(
                    "не удалось искать в «{target_title}»: {}",
                    e.message
                ))
            })?;
            matches.extend(
                page.rows
                    .iter()
                    .filter(|row| match row.values.get(NAME_FIELD) {
                        Some(Value::Text(name)) => name.trim().to_lowercase() == lower,
                        _ => false,
                    })
                    .map(|row| row.id),
            );
            match page.next_key {
                Some(key) => query.after_key = Some(key),
                None => break,
            }
        }
        Ok(match matches.as_slice() {
            [id] => Found::One(*id),
            [] => Found::Missing(format!("нет «{title}» в «{target_title}»")),
            many => Found::Ambiguous(format!(
                "«{title}» в «{target_title}» неоднозначно: найдено {}",
                many.len()
            )),
        })
    }
}
