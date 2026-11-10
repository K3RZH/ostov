//! Списки окнами: страница по ключу сортировки, без OFFSET (ADR-022), и прыжки по вехам
//! (ADR-014). Сортировка, поиск и отбор идут в базе, а не в интерфейсе (СТ-15).

mod cursor;
mod milestones;

use std::collections::HashMap;

use ostov_store::{Driver, Value};

use crate::object::{FieldDef, FieldKind, Fields, read_fields, to_value};
use crate::{Catalog, CoreError, DocumentKind};
use cursor::Cursor;
pub use milestones::{MILESTONE_STEP, Milestones};

/// Служебные флаги и версия: по ним не ищут, для этого есть отбор.
const SERVICE: [&str; 3] = ["posted", "deleted", "version"];

/// Больше строк за раз не отдаём (СТ-51).
pub const MAX_WINDOW: usize = 1000;

/// То, что можно показать списком: таблица с `id` и полями.
pub trait ListSource {
    fn table(&self) -> &str;
    fn list_fields(&self) -> Vec<FieldDef>;
}

impl ListSource for Catalog {
    fn table(&self) -> &str {
        Catalog::table(self)
    }

    fn list_fields(&self) -> Vec<FieldDef> {
        let mut fields = self.fields().to_vec();
        for flag in ["deleted", "version"] {
            fields.push(FieldDef::new(flag, FieldKind::Int));
        }
        fields
    }
}

impl ListSource for DocumentKind {
    fn table(&self) -> &str {
        DocumentKind::table(self)
    }

    fn list_fields(&self) -> Vec<FieldDef> {
        let mut fields = vec![FieldDef::new("number", FieldKind::Int)];
        fields.extend(self.header().iter().cloned());
        for flag in ["posted", "deleted", "version"] {
            fields.push(FieldDef::new(flag, FieldKind::Int));
        }
        fields
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    /// Поле сортировки; `None` — по id. Пустые значения всегда в конце.
    pub order: Option<String>,
    pub descending: bool,
    /// Следующие ключи сортировки: поле и «по убыванию». Без `order` нельзя.
    pub then_by: Vec<(String, bool)>,
    /// Поиск подстроки по текстовым полям.
    pub search: Option<String>,
    /// Только проведённые (`Some(true)`) или только черновики (`Some(false)`).
    pub posted: Option<bool>,
    /// Только помеченные на удаление или только непомеченные.
    pub deleted: Option<bool>,
    /// Точный отбор по полям; текст без учёта регистра.
    pub filter: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub id: i64,
    pub fields: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub rows: Vec<ListRow>,
    /// Ключ для следующей страницы; `None` — список кончился.
    pub next_key: Option<String>,
    /// Сколько всего строк; считаем только для первой страницы и после прыжка.
    pub total: Option<u64>,
}

/// Чем показывать ссылки: таблица справочника → её поле-название (обычно `name`).
/// По нему ссылочные колонки сортируются и ищутся по названию, а не по id.
pub type TitleColumns = HashMap<String, String>;

/// Окно строк: первое (`after = None`) или следующее за ключом из прошлой страницы.
pub fn list(
    db: &mut dyn Driver,
    source: &dyn ListSource,
    query: &ListQuery,
    after: Option<&str>,
    size: usize,
) -> Result<ListPage, CoreError> {
    list_titled(db, source, query, after, size, &TitleColumns::new())
}

/// То же, что `list`, но ссылки сортируются и ищутся по названиям из `titles`.
pub fn list_titled(
    db: &mut dyn Driver,
    source: &dyn ListSource,
    query: &ListQuery,
    after: Option<&str>,
    size: usize,
    titles: &TitleColumns,
) -> Result<ListPage, CoreError> {
    let plan = Plan::new(source, query, titles)?;
    let after = after.map(Cursor::decode).transpose()?;
    let total = match after {
        None => Some(plan.count(db)?),
        Some(_) => None,
    };
    let (rows, next) = plan.fetch(db, after.as_ref(), 0, size)?;
    Ok(ListPage {
        rows,
        next_key: next.map(|c| c.encode()),
        total,
    })
}

/// Разобранный запрос списка: всё проверено, остаётся собрать SQL.
struct Plan {
    table: String,
    fields: Vec<FieldDef>,
    /// Ключи сортировки: номер поля в `fields` и «по убыванию».
    keys: Vec<(usize, bool)>,
    /// Последний ключ — id, в направлении первого ключа.
    id_descending: bool,
    search: Option<String>,
    /// Если строка поиска — число, ищем ещё и по целым полям (номер документа).
    search_number: Option<i64>,
    posted: Option<bool>,
    deleted: Option<bool>,
    /// Номер поля в `fields` и значение для сравнения (текст уже в нижнем регистре).
    filter: Vec<(usize, Value)>,
    titles: TitleColumns,
}

impl Plan {
    fn new(
        source: &dyn ListSource,
        query: &ListQuery,
        titles: &TitleColumns,
    ) -> Result<Self, CoreError> {
        let fields = source.list_fields();
        if query.order.is_none() && !query.then_by.is_empty() {
            return Err(CoreError::Invalid(
                "следующие ключи сортировки без первого (order)".into(),
            ));
        }
        let mut keys: Vec<(usize, bool)> = Vec::with_capacity(query.then_by.len() + 1);
        let first = query.order.iter().map(|name| (name, query.descending));
        let rest = query.then_by.iter().map(|(name, desc)| (name, *desc));
        for (name, desc) in first.chain(rest) {
            let i = fields.iter().position(|f| &f.name == name).ok_or_else(|| {
                CoreError::Invalid(format!(
                    "в {} нет поля {name} для сортировки",
                    source.table()
                ))
            })?;
            if keys.iter().any(|(k, _)| *k == i) {
                return Err(CoreError::Invalid(format!(
                    "поле {name} в сортировке дважды"
                )));
            }
            keys.push((i, desc));
        }
        if query.posted.is_some() && !fields.iter().any(|f| f.name == "posted") {
            return Err(CoreError::Invalid(format!(
                "{} не проводится: отбора по проведению нет",
                source.table()
            )));
        }
        let mut filter = Vec::with_capacity(query.filter.len());
        for (name, value) in &query.filter {
            let found = fields.iter().enumerate().find(|(_, f)| &f.name == name);
            let Some((i, def)) = found else {
                return Err(CoreError::Invalid(format!(
                    "в {} нет поля {name} для отбора",
                    source.table()
                )));
            };
            let value = match to_value(def, value)? {
                Value::Text(text) => Value::Text(text.to_lowercase()),
                other => other,
            };
            filter.push((i, value));
        }
        let raw = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        Ok(Self {
            table: source.table().to_owned(),
            fields,
            keys,
            id_descending: query.descending,
            search: raw.map(|s| like_pattern(&s.to_lowercase())),
            search_number: raw.and_then(|s| s.parse().ok()),
            posted: query.posted,
            deleted: query.deleted,
            filter,
            titles: titles.clone(),
        })
    }

    fn count(&self, db: &mut dyn Driver) -> Result<u64, CoreError> {
        let mut params = Vec::new();
        let sql = {
            let d = db.dialect();
            let filter = self.filter_sql(d, &mut params);
            let mut sql = format!("SELECT COUNT(*) FROM {}", d.quote_ident(&self.table));
            if let Some(filter) = filter {
                sql.push_str(&format!(" WHERE {filter}"));
            }
            sql
        };
        match db.query(&sql, &params)?.first().and_then(|r| r.first()) {
            Some(Value::Int(n)) => Ok(u64::try_from(*n).unwrap_or(0)),
            _ => Err(CoreError::Invalid("база не вернула число строк".into())),
        }
    }

    /// Строки после `after`, пропустив первые `skip`, не больше `size`.
    /// Вторым значением — ключ последней строки, если дальше ещё есть строки.
    fn fetch(
        &self,
        db: &mut dyn Driver,
        after: Option<&Cursor>,
        skip: usize,
        size: usize,
    ) -> Result<(Vec<ListRow>, Option<Cursor>), CoreError> {
        if size == 0 || size > MAX_WINDOW {
            return Err(CoreError::Invalid(format!(
                "размер окна должен быть от 1 до {MAX_WINDOW}"
            )));
        }
        let mut params = Vec::new();
        let sql = {
            let d = db.dialect();
            let id = d.quote_ident("id");
            let mut cols = vec![id.clone()];
            cols.extend(self.fields.iter().map(|f| d.quote_ident(&f.name)));
            // Значения сортировки — последними колонками: из них строится ключ страницы.
            for &(i, _) in &self.keys {
                cols.push(self.sort_expr(d, i));
            }

            let mut filters: Vec<String> = self.filter_sql(d, &mut params).into_iter().collect();
            if let Some(after) = after {
                filters.push(self.after_sql(d, after, &mut params)?);
            }
            let mut sql = format!(
                "SELECT {} FROM {}",
                cols.join(", "),
                d.quote_ident(&self.table)
            );
            if !filters.is_empty() {
                sql.push_str(&format!(" WHERE {}", filters.join(" AND ")));
            }
            sql.push_str(&format!(" ORDER BY {}", self.order_sql(d)));
            // На одну строку больше, чтобы узнать, есть ли следующая страница.
            params.push(Value::Int(
                i64::try_from(skip + size + 1).unwrap_or(i64::MAX),
            ));
            sql.push_str(&format!(" LIMIT {}", d.placeholder(params.len())));
            sql
        };

        let mut raw = db.query(&sql, &params)?;
        let more = raw.len() > skip + size;
        raw.truncate(skip + size);
        let mut rows = Vec::with_capacity(size);
        let mut last = None;
        for row in raw.into_iter().skip(skip) {
            let mut values = row.into_iter();
            let Some(Value::Int(id)) = values.next() else {
                return Err(CoreError::Invalid(format!(
                    "в {} строка без id",
                    self.table
                )));
            };
            let mut values: Vec<Value> = values.collect();
            let sort_values = values.split_off(values.len().saturating_sub(self.keys.len()));
            last = Some(Cursor {
                id,
                values: sort_values,
            });
            rows.push(ListRow {
                id,
                fields: read_fields(&self.fields, values.into_iter())?,
            });
        }
        Ok((rows, if more { last } else { None }))
    }

    fn order_sql(&self, d: &dyn ostov_store::Dialect) -> String {
        let dir = |desc: bool| if desc { "DESC" } else { "ASC" };
        let mut parts: Vec<String> = self
            .keys
            .iter()
            .map(|&(i, desc)| {
                let col = self.sort_expr(d, i);
                format!("({col} IS NULL), {col} {}", dir(desc))
            })
            .collect();
        parts.push(format!(
            "{} {}",
            d.quote_ident("id"),
            dir(self.id_descending)
        ));
        parts.join(", ")
    }

    // Строка после ключа: больше по первому ключу, или равна ему и больше по следующему,
    // и так до id. Пустые значения стоят в конце при любом направлении, поэтому после
    // пустого по ключу идут только пустые.
    fn after_sql(
        &self,
        d: &dyn ostov_store::Dialect,
        after: &Cursor,
        params: &mut Vec<Value>,
    ) -> Result<String, CoreError> {
        if after.values.len() != self.keys.len() {
            return Err(CoreError::Invalid(
                "ключ страницы от другой сортировки".into(),
            ));
        }
        let cmp = |desc: bool| if desc { "<" } else { ">" };
        let mut param = |v: Value| {
            params.push(v);
            d.placeholder(params.len())
        };
        let id = d.quote_ident("id");
        let mut sql = format!(
            "{id} {} {}",
            cmp(self.id_descending),
            param(Value::Int(after.id))
        );
        for (&(i, desc), value) in self.keys.iter().zip(&after.values).rev() {
            let col = self.sort_expr(d, i);
            sql = match value {
                Value::Null => format!("({col} IS NULL AND {sql})"),
                value => {
                    let a = param(value.clone());
                    let b = param(value.clone());
                    format!(
                        "({col} IS NULL OR {col} {} {a} OR ({col} = {b} AND {sql}))",
                        cmp(desc)
                    )
                }
            };
        }
        Ok(sql)
    }

    /// Название, на которое указывает ссылка: таблица справочника и её поле-название.
    fn title_of(&self, i: usize) -> Option<(&str, &str)> {
        let field = self.fields.get(i)?;
        if field.kind != FieldKind::Ref {
            return None;
        }
        let target = field.target.as_deref()?;
        let column = self.titles.get(target)?;
        Some((target, column))
    }

    /// Чем сортировать поле: у ссылки — названием элемента, у остальных — самим полем.
    pub(crate) fn sort_expr(&self, d: &dyn ostov_store::Dialect, i: usize) -> String {
        let name = self.fields.get(i).map_or("id", |f| f.name.as_str());
        let own = format!("{}.{}", d.quote_ident(&self.table), d.quote_ident(name));
        match self.title_of(i) {
            Some((target, column)) => format!(
                "(SELECT {} FROM {} WHERE {}.{} = {own})",
                d.quote_ident(column),
                d.quote_ident(target),
                d.quote_ident(target),
                d.quote_ident("id"),
            ),
            None => own,
        }
    }

    /// Поиск и отбор по статусу вместе.
    pub(crate) fn filter_sql(
        &self,
        d: &dyn ostov_store::Dialect,
        params: &mut Vec<Value>,
    ) -> Option<String> {
        let mut parts: Vec<String> = self.search_sql(d, params).into_iter().collect();
        for (flag, wanted) in [("posted", self.posted), ("deleted", self.deleted)] {
            if let Some(wanted) = wanted {
                params.push(Value::Int(wanted.into()));
                parts.push(format!(
                    "{}.{} = {}",
                    d.quote_ident(&self.table),
                    d.quote_ident(flag),
                    d.placeholder(params.len())
                ));
            }
        }
        for (i, value) in &self.filter {
            let Some(field) = self.fields.get(*i) else {
                continue;
            };
            let col = format!(
                "{}.{}",
                d.quote_ident(&self.table),
                d.quote_ident(&field.name)
            );
            if *value == Value::Null {
                parts.push(format!("{col} IS NULL"));
                continue;
            }
            params.push(value.clone());
            let col = if field.kind == FieldKind::Text {
                d.lower(&col)
            } else {
                col
            };
            parts.push(format!("{col} = {}", d.placeholder(params.len())));
        }
        (!parts.is_empty()).then(|| parts.join(" AND "))
    }

    /// Поиск: подстрока в тексте, дате и названиях ссылок; число — ещё и по целым полям
    /// (номер документа). Если искать негде, ничего не находится.
    fn search_sql(&self, d: &dyn ostov_store::Dialect, params: &mut Vec<Value>) -> Option<String> {
        let pattern = self.search.as_ref()?;
        let mut parts = Vec::new();
        for (i, f) in self.fields.iter().enumerate() {
            let col = format!("{}.{}", d.quote_ident(&self.table), d.quote_ident(&f.name));
            match f.kind {
                FieldKind::Text | FieldKind::Date => {
                    params.push(Value::Text(pattern.clone()));
                    parts.push(format!(
                        "{} LIKE {} ESCAPE '\\'",
                        d.lower(&col),
                        d.placeholder(params.len())
                    ));
                }
                FieldKind::Ref => {
                    if let Some((target, column)) = self.title_of(i) {
                        params.push(Value::Text(pattern.clone()));
                        parts.push(format!(
                            "{col} IN (SELECT {} FROM {} WHERE {} LIKE {} ESCAPE '\\')",
                            d.quote_ident("id"),
                            d.quote_ident(target),
                            d.lower(&d.quote_ident(column)),
                            d.placeholder(params.len()),
                        ));
                    }
                }
                FieldKind::Int if SERVICE.contains(&f.name.as_str()) => {}
                FieldKind::Int => {
                    if let Some(n) = self.search_number {
                        params.push(Value::Int(n));
                        parts.push(format!("{col} = {}", d.placeholder(params.len())));
                    }
                }
                FieldKind::Decimal { .. } => {}
            }
        }
        if parts.is_empty() {
            return Some("(1 = 0)".to_owned());
        }
        Some(format!("({})", parts.join(" OR ")))
    }
}

/// `%` и `_` в строке поиска — обычные символы, а не шаблон LIKE.
fn like_pattern(search: &str) -> String {
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

#[cfg(test)]
mod tests;
