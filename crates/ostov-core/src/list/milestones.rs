use ostov_store::{Driver, Value};

use super::cursor::Cursor;
use super::{ListPage, ListQuery, ListSource, Plan, TitleColumns};
use crate::CoreError;

/// Шаг вех по умолчанию: ключ каждой 1000-й строки (проба 26.10).
pub const MILESTONE_STEP: u64 = 1000;

/// Вехи списка: ключи каждой `step`-й строки для одного порядка и поиска.
/// По ним прыжок в любое место — это одно окно от ближайшей вехи, а не OFFSET.
/// Строятся лениво: только до того места, куда прыгнули, дальше — при следующих прыжках.
/// Заново — при смене сортировки, поиска или после изменений.
#[derive(Debug, Clone)]
pub struct Milestones {
    table: String,
    query: ListQuery,
    titles: TitleColumns,
    step: u64,
    keys: Vec<Cursor>,
    total: u64,
    /// Список прочитан до конца: больше вех нет.
    done: bool,
}

impl Milestones {
    pub fn build(
        db: &mut dyn Driver,
        source: &dyn ListSource,
        query: &ListQuery,
        step: u64,
    ) -> Result<Self, CoreError> {
        Self::build_titled(db, source, query, step, &TitleColumns::new())
    }

    /// Вехи, где ссылки сортируются по названиям из `titles` (как в `list_titled`).
    /// Сразу считается только число строк; сами вехи — по мере прыжков.
    pub fn build_titled(
        db: &mut dyn Driver,
        source: &dyn ListSource,
        query: &ListQuery,
        step: u64,
        titles: &TitleColumns,
    ) -> Result<Self, CoreError> {
        if step == 0 {
            return Err(CoreError::Invalid("шаг вех должен быть больше нуля".into()));
        }
        let plan = Plan::new(source, query, titles)?;
        let total = plan.count(db)?;
        Ok(Self {
            table: plan.table,
            query: query.clone(),
            titles: titles.clone(),
            step,
            keys: Vec::new(),
            total,
            done: total == 0,
        })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// Дочитывает вехи, пока их не станет `wanted` или список не кончится. Читаются только
    /// id и поле сортировки, от последней известной вехи.
    fn extend(&mut self, db: &mut dyn Driver, plan: &Plan, wanted: usize) -> Result<(), CoreError> {
        if self.done || self.keys.len() >= wanted {
            return Ok(());
        }
        let missing = u64::try_from(wanted - self.keys.len()).unwrap_or(u64::MAX);
        let need = missing.saturating_mul(self.step);
        let mut params = Vec::new();
        let sql = {
            let d = db.dialect();
            let mut cols = vec![d.quote_ident("id")];
            for &(i, _) in &plan.keys {
                cols.push(plan.sort_expr(d, i));
            }
            let mut filters: Vec<String> = plan.filter_sql(d, &mut params).into_iter().collect();
            if let Some(after) = self.keys.last() {
                filters.push(plan.after_sql(d, after, &mut params)?);
            }
            let mut sql = format!(
                "SELECT {} FROM {}",
                cols.join(", "),
                d.quote_ident(&plan.table)
            );
            if !filters.is_empty() {
                sql.push_str(&format!(" WHERE {}", filters.join(" AND ")));
            }
            params.push(Value::Int(i64::try_from(need).unwrap_or(i64::MAX)));
            sql.push_str(&format!(
                " ORDER BY {} LIMIT {}",
                plan.order_sql(d),
                d.placeholder(params.len())
            ));
            sql
        };
        let rows = db.query(&sql, &params)?;
        let read = u64::try_from(rows.len()).unwrap_or(u64::MAX);
        for (row, n) in rows.into_iter().zip(1u64..) {
            if n % self.step != 0 {
                continue;
            }
            let mut values = row.into_iter();
            let Some(Value::Int(id)) = values.next() else {
                return Err(CoreError::Invalid(format!(
                    "в {} строка без id",
                    plan.table
                )));
            };
            self.keys.push(Cursor {
                id,
                values: values.collect(),
            });
        }
        if read < need {
            self.done = true;
        }
        Ok(())
    }

    /// Окно, начиная со строки номер `position` (с нуля). Так работают и ползунок,
    /// и «перейти на страницу N»: позиция = (N - 1) × размер страницы.
    pub fn seek(
        &mut self,
        db: &mut dyn Driver,
        source: &dyn ListSource,
        position: u64,
        size: usize,
    ) -> Result<ListPage, CoreError> {
        if source.table() != self.table {
            return Err(CoreError::Invalid(format!(
                "вехи построены для {}, а не для {}",
                self.table,
                source.table()
            )));
        }
        if position >= self.total {
            return Ok(ListPage {
                rows: Vec::new(),
                next_key: None,
                total: Some(self.total),
            });
        }
        let plan = Plan::new(source, &self.query, &self.titles)?;
        // Веха номер k — ключ строки k × step; после неё начинается строка k × step (с нуля).
        let wanted = usize::try_from(position / self.step).unwrap_or(usize::MAX);
        self.extend(db, &plan, wanted)?;
        let mark = wanted.min(self.keys.len());
        let start = mark.checked_sub(1).and_then(|i| self.keys.get(i));
        let passed = u64::try_from(mark)
            .unwrap_or(u64::MAX)
            .saturating_mul(self.step);
        let skip = usize::try_from(position - passed).unwrap_or(usize::MAX);
        let (rows, next) = plan.fetch(db, start, skip, size)?;
        Ok(ListPage {
            rows,
            next_key: next.map(|c| c.encode()),
            total: Some(self.total),
        })
    }
}
