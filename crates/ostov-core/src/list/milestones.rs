use ostov_store::{Driver, Value};

use super::cursor::Cursor;
use super::{ListPage, ListQuery, ListSource, Plan};
use crate::CoreError;

/// Шаг вех по умолчанию: ключ каждой 1000-й строки (проба 26.10).
pub const MILESTONE_STEP: u64 = 1000;

/// Вехи списка: ключи каждой `step`-й строки для одного порядка и поиска.
/// По ним прыжок в любое место — это одно окно от ближайшей вехи, а не OFFSET.
/// Строятся заново при смене сортировки, поиска или после больших изменений.
#[derive(Debug, Clone)]
pub struct Milestones {
    table: String,
    query: ListQuery,
    step: u64,
    keys: Vec<Cursor>,
    total: u64,
}

impl Milestones {
    pub fn build(
        db: &mut dyn Driver,
        source: &dyn ListSource,
        query: &ListQuery,
        step: u64,
    ) -> Result<Self, CoreError> {
        if step == 0 {
            return Err(CoreError::Invalid("шаг вех должен быть больше нуля".into()));
        }
        let plan = Plan::new(source, query)?;
        let mut params = Vec::new();
        let sql = {
            let d = db.dialect();
            let mut cols = vec![d.quote_ident("id")];
            if let Some(i) = plan.order {
                cols.push(d.quote_ident(&plan.fields[i].name));
            }
            let mut sql = format!(
                "SELECT {} FROM {}",
                cols.join(", "),
                d.quote_ident(&plan.table)
            );
            if let Some(filter) = plan.search_sql(d, &mut params) {
                sql.push_str(&format!(" WHERE {filter}"));
            }
            sql.push_str(&format!(" ORDER BY {}", plan.order_sql(d)));
            sql
        };

        // Читаем только id и поле сортировки; позже — потоком, без всего списка в памяти.
        let rows = db.query(&sql, &params)?;
        let total = u64::try_from(rows.len()).unwrap_or(u64::MAX);
        let keys = rows
            .into_iter()
            .zip(1u64..)
            .filter(|(_, n)| n % step == 0)
            .map(|(row, _)| {
                let mut values = row.into_iter();
                match values.next() {
                    Some(Value::Int(id)) => Ok(Cursor {
                        id,
                        value: plan.order.map(|_| values.next().unwrap_or(Value::Null)),
                    }),
                    _ => Err(CoreError::Invalid(format!(
                        "в {} строка без id",
                        plan.table
                    ))),
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            table: plan.table,
            query: query.clone(),
            step,
            keys,
            total,
        })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// Окно, начиная со строки номер `position` (с нуля). Так работают и ползунок,
    /// и «перейти на страницу N»: позиция = (N - 1) × размер страницы.
    pub fn seek(
        &self,
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
        let plan = Plan::new(source, &self.query)?;
        // Веха номер k — ключ строки k × step; после неё начинается строка k × step (с нуля).
        let mark = usize::try_from(position / self.step)
            .unwrap_or(usize::MAX)
            .min(self.keys.len());
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
