//! Отчёты по регистру накопления: обороты за период и остатки на дату.

use std::collections::BTreeMap;

use ostov_store::{Decimal, Driver, Value};

use super::accumulation::{AccumulationRegister, scale};
use crate::CoreError;
use crate::object::{Fields, from_value, is_date, to_value};

/// Больше строк отчёт пока не отдаёт: для склада кафе с большим запасом.
pub const MAX_REPORT_ROWS: usize = 10_000;

/// Обороты по одному набору измерений (например, склад + товар).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turnover {
    pub dims: Fields,
    pub opening: BTreeMap<String, Decimal>,
    pub receipt: BTreeMap<String, Decimal>,
    pub issue: BTreeMap<String, Decimal>,
    pub closing: BTreeMap<String, Decimal>,
}

/// Остаток по набору измерений: измерения и значения ресурсов.
pub type Balance = (Fields, BTreeMap<String, Decimal>);

// Движения без даты (документ без поля даты) считаем самыми ранними.
const NO_DATE: &str = "0000-00-00";
const NO_END: &str = "9999-12-31";

impl AccumulationRegister {
    /// Обороты с `from` по `to` включительно (`ГГГГ-ММ-ДД`): начальный остаток, приход,
    /// расход и конечный остаток по каждому набору измерений. Без `from` — с начала учёта,
    /// без `to` — без верхней границы. `filter` — отбор по измерениям, например склад.
    /// Наборы, где всё по нулям, в отчёт не попадают.
    pub fn turnovers(
        &self,
        db: &mut dyn Driver,
        from: Option<&str>,
        to: Option<&str>,
        filter: &Fields,
    ) -> Result<Vec<Turnover>, CoreError> {
        for date in from.iter().chain(to.iter()) {
            if !is_date(date) {
                return Err(CoreError::Invalid(format!("«{date}» — не дата ГГГГ-ММ-ДД")));
            }
        }
        let (from, to) = (from.unwrap_or(NO_DATE), to.unwrap_or(NO_END));
        if from > to {
            return Err(CoreError::Invalid(format!(
                "период с {from} по {to}: начало позже конца"
            )));
        }
        if let Some(unknown) = filter
            .keys()
            .find(|k| !self.dims.iter().any(|d| &d.name == *k))
        {
            return Err(CoreError::Invalid(format!(
                "в регистре {} нет измерения {unknown}",
                self.name
            )));
        }

        let mut params = vec![Value::Text(from.to_owned()), Value::Text(to.to_owned())];
        let sql = {
            let d = db.dialect();
            let period = format!("COALESCE({}, '{NO_DATE}')", d.quote_ident("period"));
            let kind = d.quote_ident("kind");
            let (p_from, p_to) = (d.placeholder(1), d.placeholder(2));
            let dims: Vec<String> = self.dims.iter().map(|f| d.quote_ident(&f.name)).collect();

            let mut cols = dims.clone();
            for r in &self.resources {
                let r = d.quote_ident(&r.name);
                cols.push(format!(
                    "CAST(SUM(CASE WHEN {period} < {p_from} THEN {kind} * {r} ELSE 0 END) AS BIGINT)"
                ));
                cols.push(format!(
                    "CAST(SUM(CASE WHEN {period} >= {p_from} AND {kind} = 1 THEN {r} ELSE 0 END) AS BIGINT)"
                ));
                cols.push(format!(
                    "CAST(SUM(CASE WHEN {period} >= {p_from} AND {kind} = -1 THEN {r} ELSE 0 END) AS BIGINT)"
                ));
            }

            let mut filters = vec![format!("{period} <= {p_to}")];
            for def in self.dims.iter().filter(|d| filter.contains_key(&d.name)) {
                let value = filter
                    .get(&def.name)
                    .map(|v| to_value(def, v))
                    .transpose()?;
                params.push(value.unwrap_or(Value::Null));
                filters.push(format!(
                    "{} = {}",
                    d.quote_ident(&def.name),
                    d.placeholder(params.len())
                ));
            }
            params.push(Value::Int(
                i64::try_from(MAX_REPORT_ROWS + 1).unwrap_or(i64::MAX),
            ));
            format!(
                "SELECT {} FROM {} WHERE {} GROUP BY {} ORDER BY {} LIMIT {}",
                cols.join(", "),
                d.quote_ident(&self.name),
                filters.join(" AND "),
                dims.join(", "),
                dims.join(", "),
                d.placeholder(params.len()),
            )
        };

        let rows = db.query(&sql, &params)?;
        if rows.len() > MAX_REPORT_ROWS {
            return Err(CoreError::Invalid(format!(
                "в отчёте больше {MAX_REPORT_ROWS} строк — уточните отбор"
            )));
        }
        let mut report = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = row.into_iter();
            let mut dims = Fields::new();
            for def in &self.dims {
                let value = values.next().unwrap_or(Value::Null);
                dims.insert(def.name.clone(), from_value(def, value)?);
            }
            let mut t = Turnover {
                dims,
                opening: BTreeMap::new(),
                receipt: BTreeMap::new(),
                issue: BTreeMap::new(),
                closing: BTreeMap::new(),
            };
            let mut any = false;
            for def in &self.resources {
                let mut next = || -> Result<Decimal, CoreError> {
                    match values.next() {
                        Some(Value::Int(units)) => Ok(Decimal::new(units, scale(def))?),
                        Some(Value::Null) | None => Ok(Decimal::zero(scale(def))?),
                        Some(_) => Err(CoreError::Invalid(format!(
                            "в оборотах {} не число",
                            def.name
                        ))),
                    }
                };
                let (opening, receipt, issue) = (next()?, next()?, next()?);
                let closing = opening.checked_add(receipt)?.checked_sub(issue)?;
                any |= !(opening.is_zero() && receipt.is_zero() && issue.is_zero());
                t.opening.insert(def.name.clone(), opening);
                t.receipt.insert(def.name.clone(), receipt);
                t.issue.insert(def.name.clone(), issue);
                t.closing.insert(def.name.clone(), closing);
            }
            if any {
                report.push(t);
            }
        }
        Ok(report)
    }

    /// Остатки на конец дня `date` (без даты — текущие): только ненулевые.
    pub fn balances(
        &self,
        db: &mut dyn Driver,
        date: Option<&str>,
        filter: &Fields,
    ) -> Result<Vec<Balance>, CoreError> {
        Ok(self
            .turnovers(db, None, date, filter)?
            .into_iter()
            .filter(|t| t.closing.values().any(|v| !v.is_zero()))
            .map(|t| (t.dims, t.closing))
            .collect())
    }
}
