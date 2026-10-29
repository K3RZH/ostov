//! Списки окнами: страница по ключу сортировки, без OFFSET (ADR-022), и прыжки по вехам
//! (ADR-014). Сортировка, поиск и отбор идут в базе, а не в интерфейсе (СТ-15).

mod cursor;
mod milestones;

use ostov_store::{Driver, Value};

use crate::object::{FieldDef, FieldKind, Fields, read_fields};
use crate::{Catalog, CoreError, DocumentKind};
use cursor::Cursor;
pub use milestones::{MILESTONE_STEP, Milestones};

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
        fields.push(FieldDef::new("deleted", FieldKind::Int));
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
        for flag in ["posted", "deleted"] {
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
    /// Поиск подстроки по текстовым полям.
    pub search: Option<String>,
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

/// Окно строк: первое (`after = None`) или следующее за ключом из прошлой страницы.
pub fn list(
    db: &mut dyn Driver,
    source: &dyn ListSource,
    query: &ListQuery,
    after: Option<&str>,
    size: usize,
) -> Result<ListPage, CoreError> {
    let plan = Plan::new(source, query)?;
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
    /// Номер поля сортировки в `fields`.
    order: Option<usize>,
    descending: bool,
    search: Option<String>,
}

impl Plan {
    fn new(source: &dyn ListSource, query: &ListQuery) -> Result<Self, CoreError> {
        let fields = source.list_fields();
        let order = match &query.order {
            None => None,
            Some(name) => Some(fields.iter().position(|f| &f.name == name).ok_or_else(|| {
                CoreError::Invalid(format!(
                    "в {} нет поля {name} для сортировки",
                    source.table()
                ))
            })?),
        };
        let search = query
            .search
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(like_pattern);
        if search.is_some() && !fields.iter().any(|f| f.kind == FieldKind::Text) {
            return Err(CoreError::Invalid(format!(
                "в {} нет текстовых полей для поиска",
                source.table()
            )));
        }
        Ok(Self {
            table: source.table().to_owned(),
            fields,
            order,
            descending: query.descending,
            search,
        })
    }

    fn count(&self, db: &mut dyn Driver) -> Result<u64, CoreError> {
        let mut params = Vec::new();
        let sql = {
            let d = db.dialect();
            let filter = self.search_sql(d, &mut params);
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

            let mut filters: Vec<String> = self.search_sql(d, &mut params).into_iter().collect();
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
            let values: Vec<Value> = values.collect();
            last = Some(Cursor {
                id,
                value: self
                    .order
                    .map(|i| values.get(i).cloned().unwrap_or(Value::Null)),
            });
            rows.push(ListRow {
                id,
                fields: read_fields(&self.fields, values.into_iter())?,
            });
        }
        Ok((rows, if more { last } else { None }))
    }

    fn order_sql(&self, d: &dyn ostov_store::Dialect) -> String {
        let dir = if self.descending { "DESC" } else { "ASC" };
        let id = d.quote_ident("id");
        match self.order {
            None => format!("{id} {dir}"),
            Some(i) => {
                let col = d.quote_ident(&self.fields[i].name);
                format!("({col} IS NULL), {col} {dir}, {id} {dir}")
            }
        }
    }

    // Пустые значения стоят в конце при любом направлении, поэтому после пустого
    // идут только пустые с большим (меньшим) id.
    fn after_sql(
        &self,
        d: &dyn ostov_store::Dialect,
        after: &Cursor,
        params: &mut Vec<Value>,
    ) -> Result<String, CoreError> {
        let cmp = if self.descending { "<" } else { ">" };
        let id = d.quote_ident("id");
        let mut param = |v: Value| {
            params.push(v);
            d.placeholder(params.len())
        };
        Ok(match (self.order, &after.value) {
            (None, None) => format!("{id} {cmp} {}", param(Value::Int(after.id))),
            (Some(i), Some(Value::Null)) => {
                let col = d.quote_ident(&self.fields[i].name);
                format!(
                    "({col} IS NULL AND {id} {cmp} {})",
                    param(Value::Int(after.id))
                )
            }
            (Some(i), Some(value)) => {
                let col = d.quote_ident(&self.fields[i].name);
                let a = param(value.clone());
                let b = param(value.clone());
                let c = param(Value::Int(after.id));
                format!("({col} IS NULL OR {col} {cmp} {a} OR ({col} = {b} AND {id} {cmp} {c}))")
            }
            _ => {
                return Err(CoreError::Invalid(
                    "ключ страницы от другой сортировки".into(),
                ));
            }
        })
    }

    fn search_sql(&self, d: &dyn ostov_store::Dialect, params: &mut Vec<Value>) -> Option<String> {
        let pattern = self.search.as_ref()?;
        let parts: Vec<String> = self
            .fields
            .iter()
            .filter(|f| f.kind == FieldKind::Text)
            .map(|f| {
                params.push(Value::Text(pattern.clone()));
                format!(
                    "{} LIKE {} ESCAPE '\\'",
                    d.quote_ident(&f.name),
                    d.placeholder(params.len())
                )
            })
            .collect();
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
