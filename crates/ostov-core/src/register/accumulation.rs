use std::collections::BTreeMap;

use ostov_store::{Decimal, Dialect, Driver, Value};

use crate::CoreError;
use crate::object::{
    FieldDef, FieldKind, Fields, check_fields, check_ident, column_defs, names, placeholders,
    to_scale, to_value,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementKind {
    Receipt,
    Issue,
}

impl MovementKind {
    fn sign(self) -> i64 {
        match self {
            Self::Receipt => 1,
            Self::Issue => -1,
        }
    }
}

/// Одна строка движения: что, где и сколько пришло или ушло.
#[derive(Debug, Clone)]
pub struct Movement {
    pub kind: MovementKind,
    pub dims: Fields,
    pub resources: BTreeMap<String, Decimal>,
}

/// Регистр накопления: движения документов и итоги по измерениям.
/// Итоги — кэш, их всегда можно пересчитать из движений (СТ-21).
#[derive(Debug, Clone)]
pub struct AccumulationRegister {
    pub(super) name: String,
    pub(super) dims: Vec<FieldDef>,
    pub(super) resources: Vec<FieldDef>,
    allow_negative: bool,
}

type Key = Vec<Value>;

impl AccumulationRegister {
    /// Измерения — целые или строки, ресурсы — только `Decimal`.
    pub fn new(
        name: &str,
        dims: Vec<FieldDef>,
        resources: Vec<FieldDef>,
    ) -> Result<Self, CoreError> {
        for ident in [
            name.to_owned(),
            format!("{name}__totals"),
            format!("{name}__recorder"),
            format!("{name}__period"),
        ] {
            check_ident(&ident)?;
        }
        let all: Vec<FieldDef> = dims.iter().chain(&resources).cloned().collect();
        check_fields(&all, &["id", "recorder", "recorder_id", "kind"])?;
        if dims.is_empty() {
            return Err(CoreError::Invalid(format!(
                "у регистра {name} нет измерений"
            )));
        }
        if let Some(bad) = dims
            .iter()
            .find(|f| matches!(f.kind, FieldKind::Decimal { .. }))
        {
            return Err(CoreError::Invalid(format!(
                "измерение {} не может быть числом с дробью",
                bad.name
            )));
        }
        if let Some(bad) = resources
            .iter()
            .find(|f| !matches!(f.kind, FieldKind::Decimal { .. }))
        {
            return Err(CoreError::Invalid(format!(
                "ресурс {} должен быть Decimal",
                bad.name
            )));
        }
        Ok(Self {
            name: name.to_owned(),
            dims,
            resources,
            allow_negative: false,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn dims(&self) -> &[FieldDef] {
        &self.dims
    }

    pub fn resources(&self) -> &[FieldDef] {
        &self.resources
    }

    pub(crate) fn has_movements(&self, db: &mut dyn Driver) -> Result<bool, CoreError> {
        let sql = format!(
            "SELECT 1 FROM {} LIMIT 1",
            db.dialect().quote_ident(&self.name)
        );
        Ok(!db.query(&sql, &[])?.is_empty())
    }

    /// Только для пустого регистра: перестроить таблицы под новые измерения.
    pub(crate) fn drop_tables(&self, db: &mut dyn Driver) -> Result<(), CoreError> {
        let statements = {
            let d = db.dialect();
            [self.totals(), self.name.clone()].map(|t| format!("DROP TABLE {}", d.quote_ident(&t)))
        };
        for sql in statements {
            db.execute(&sql, &[])?;
        }
        Ok(())
    }

    /// Разрешить уход итогов в минус. По умолчанию запрещено.
    pub fn allow_negative(mut self) -> Self {
        self.allow_negative = true;
        self
    }

    pub fn create_tables(&self, db: &mut dyn Driver) -> Result<(), CoreError> {
        let statements = {
            let d = db.dialect();
            let totals = d.quote_ident(&self.totals());
            let mut movements = vec![
                format!("{} {}", d.quote_ident("id"), d.id_column()),
                format!("{} TEXT NOT NULL", d.quote_ident("recorder")),
                format!("{} BIGINT NOT NULL", d.quote_ident("recorder_id")),
                format!("{} BIGINT NOT NULL", d.quote_ident("kind")),
                // Дата документа-регистратора: по ней отчёты на дату и за период.
                format!("{} TEXT", d.quote_ident("period")),
            ];
            movements.extend(column_defs(d, &self.dims));
            movements.extend(column_defs(d, &self.resources));
            let mut totals_cols: Vec<String> = column_defs(d, &self.dims)
                .into_iter()
                .chain(column_defs(d, &self.resources))
                .map(|c| format!("{c} NOT NULL"))
                .collect();
            totals_cols.push(format!("UNIQUE ({})", names(d, &self.dims).join(", ")));
            vec![
                format!(
                    "CREATE TABLE {} ({})",
                    d.quote_ident(&self.name),
                    movements.join(", ")
                ),
                format!(
                    "CREATE INDEX {} ON {} ({}, {})",
                    d.quote_ident(&format!("{}__recorder", self.name)),
                    d.quote_ident(&self.name),
                    d.quote_ident("recorder"),
                    d.quote_ident("recorder_id"),
                ),
                self.period_index(d),
                format!("CREATE TABLE {totals} ({})", totals_cols.join(", ")),
            ]
        };
        for sql in statements {
            db.execute(&sql, &[])?;
        }
        Ok(())
    }

    /// Текущий остаток по измерениям; если движений не было — нули.
    pub fn balance(
        &self,
        db: &mut dyn Driver,
        dims: &Fields,
    ) -> Result<BTreeMap<String, Decimal>, CoreError> {
        let key = self.key(dims)?;
        let amounts = match self.totals_row(db, &key, false)? {
            Some(amounts) => amounts,
            None => self.zeros()?,
        };
        Ok(self
            .resources
            .iter()
            .map(|r| r.name.clone())
            .zip(amounts)
            .collect())
    }

    /// Заменяет движения документа новыми и пересчитывает итоги.
    /// Если итог уйдёт в минус, возвращает ошибку — вызывающий откатывает транзакцию.
    pub fn replace(
        &self,
        db: &mut dyn Driver,
        recorder: &str,
        recorder_id: i64,
        movements: &[Movement],
    ) -> Result<(), CoreError> {
        self.replace_dated(db, recorder, recorder_id, None, movements)
    }

    /// То же, что `replace`, с датой документа (`ГГГГ-ММ-ДД`) для отчётов по периодам.
    pub fn replace_dated(
        &self,
        db: &mut dyn Driver,
        recorder: &str,
        recorder_id: i64,
        period: Option<&str>,
        movements: &[Movement],
    ) -> Result<(), CoreError> {
        let mut touched: Vec<Key> = Vec::new();
        // Самый ранний день, который меняется: старая дата документа или новая.
        let mut earliest = period.unwrap_or(NO_DATE).to_owned();

        for (sign, key, amounts, old) in self.recorded(db, recorder, recorder_id)? {
            self.add(db, &key, &amounts, -sign)?;
            push_unique(&mut touched, key);
            earliest = earliest.min(old);
        }
        let sql = {
            let d = db.dialect();
            format!(
                "DELETE FROM {} WHERE {} = {} AND {} = {}",
                d.quote_ident(&self.name),
                d.quote_ident("recorder"),
                d.placeholder(1),
                d.quote_ident("recorder_id"),
                d.placeholder(2),
            )
        };
        db.execute(&sql, &[recorder.into(), Value::Int(recorder_id)])?;

        for movement in movements {
            let key = self.key(&movement.dims)?;
            let amounts = self.amounts(movement)?;
            let place = (recorder, recorder_id, period);
            self.insert_movement(db, place, movement.kind, &key, &amounts)?;
            self.add(db, &key, &amounts, movement.kind.sign())?;
            push_unique(&mut touched, key);
        }

        if !self.allow_negative {
            for key in &touched {
                self.check_history(db, key, &earliest)?;
            }
        }
        Ok(())
    }

    /// Базы, созданные до появления дат в движениях: добавляет колонку `period`.
    /// Возвращает `true`, если колонку пришлось добавить (тогда даты нужно заполнить).
    pub fn upgrade(&self, db: &mut dyn Driver) -> Result<bool, CoreError> {
        let probe = {
            let d = db.dialect();
            format!(
                "SELECT {} FROM {} WHERE 1 = 0",
                d.quote_ident("period"),
                d.quote_ident(&self.name)
            )
        };
        if db.query(&probe, &[]).is_ok() {
            return Ok(false);
        }
        let statements = {
            let d = db.dialect();
            vec![
                format!(
                    "ALTER TABLE {} ADD COLUMN {} TEXT",
                    d.quote_ident(&self.name),
                    d.quote_ident("period")
                ),
                self.period_index(d),
            ]
        };
        for sql in statements {
            db.execute(&sql, &[])?;
        }
        Ok(true)
    }

    /// Проставляет движениям документов вида `recorder` дату из их поля `date_field`.
    pub fn fill_periods(
        &self,
        db: &mut dyn Driver,
        recorder: &str,
        date_field: &str,
    ) -> Result<usize, CoreError> {
        let sql = {
            let d = db.dialect();
            let me = d.quote_ident(&self.name);
            let doc = d.quote_ident(recorder);
            format!(
                "UPDATE {me} SET {period} = (SELECT {doc}.{date} FROM {doc} WHERE {doc}.{id} = {me}.{rid}) \
                 WHERE {rec} = {p1} AND {period} IS NULL",
                period = d.quote_ident("period"),
                date = d.quote_ident(date_field),
                id = d.quote_ident("id"),
                rid = d.quote_ident("recorder_id"),
                rec = d.quote_ident("recorder"),
                p1 = d.placeholder(1),
            )
        };
        Ok(db.execute(&sql, &[recorder.into()])?)
    }

    fn period_index(&self, d: &dyn Dialect) -> String {
        format!(
            "CREATE INDEX {} ON {} ({})",
            d.quote_ident(&format!("{}__period", self.name)),
            d.quote_ident(&self.name),
            d.quote_ident("period"),
        )
    }

    pub(crate) fn totals(&self) -> String {
        format!("{}__totals", self.name)
    }

    fn zeros(&self) -> Result<Vec<Decimal>, CoreError> {
        self.resources
            .iter()
            .map(|r| Ok(Decimal::zero(scale(r))?))
            .collect()
    }

    fn key(&self, dims: &Fields) -> Result<Key, CoreError> {
        if let Some(unknown) = dims
            .keys()
            .find(|k| !self.dims.iter().any(|d| &d.name == *k))
        {
            return Err(CoreError::Invalid(format!(
                "в регистре {} нет измерения {unknown}",
                self.name
            )));
        }
        self.dims
            .iter()
            .map(|def| match dims.get(&def.name).map(|v| to_value(def, v)) {
                Some(Ok(Value::Null)) | None => Err(CoreError::Invalid(format!(
                    "в движении {} не заполнено измерение {}",
                    self.name, def.name
                ))),
                Some(value) => value,
            })
            .collect()
    }

    fn amounts(&self, movement: &Movement) -> Result<Vec<Decimal>, CoreError> {
        if let Some(unknown) = movement
            .resources
            .keys()
            .find(|k| !self.resources.iter().any(|r| &r.name == *k))
        {
            return Err(CoreError::Invalid(format!(
                "в регистре {} нет ресурса {unknown}",
                self.name
            )));
        }
        self.resources
            .iter()
            .map(|def| match movement.resources.get(&def.name) {
                Some(v) => to_scale(def, *v, scale(def)),
                None => Ok(Decimal::zero(scale(def))?),
            })
            .collect()
    }

    fn where_key(&self, d: &dyn Dialect, from: usize) -> String {
        names(d, &self.dims)
            .into_iter()
            .enumerate()
            .map(|(i, name)| format!("{name} = {}", d.placeholder(from + i)))
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    fn totals_row(
        &self,
        db: &mut dyn Driver,
        key: &Key,
        lock: bool,
    ) -> Result<Option<Vec<Decimal>>, CoreError> {
        let sql = {
            let d = db.dialect();
            format!(
                "SELECT {} FROM {} WHERE {}{}",
                names(d, &self.resources).join(", "),
                d.quote_ident(&self.totals()),
                self.where_key(d, 1),
                if lock { d.lock_rows() } else { "" },
            )
        };
        match db.query(&sql, key)?.into_iter().next() {
            Some(row) => Ok(Some(self.decimals(row)?)),
            None => Ok(None),
        }
    }

    fn add(
        &self,
        db: &mut dyn Driver,
        key: &Key,
        amounts: &[Decimal],
        sign: i64,
    ) -> Result<(), CoreError> {
        let current = self.totals_row(db, key, true)?;
        let exists = current.is_some();
        let base = match current {
            Some(base) => base,
            None => self.zeros()?,
        };
        let mut values = Vec::with_capacity(amounts.len() + key.len());
        for (b, a) in base.iter().zip(amounts) {
            let delta = if sign < 0 { a.checked_neg()? } else { *a };
            values.push(Value::Int(b.checked_add(delta)?.units()));
        }

        let sql = {
            let d = db.dialect();
            let totals = d.quote_ident(&self.totals());
            let res = names(d, &self.resources);
            if exists {
                let sets: Vec<String> = res
                    .iter()
                    .enumerate()
                    .map(|(i, name)| format!("{name} = {}", d.placeholder(i + 1)))
                    .collect();
                format!(
                    "UPDATE {totals} SET {} WHERE {}",
                    sets.join(", "),
                    self.where_key(d, res.len() + 1)
                )
            } else {
                let cols: Vec<String> = res.into_iter().chain(names(d, &self.dims)).collect();
                format!(
                    "INSERT INTO {totals} ({}) VALUES ({})",
                    cols.join(", "),
                    placeholders(d, 1, cols.len())
                )
            }
        };
        values.extend(key.iter().cloned());
        db.execute(&sql, &values)?;
        Ok(())
    }

    fn insert_movement(
        &self,
        db: &mut dyn Driver,
        (recorder, recorder_id, period): (&str, i64, Option<&str>),
        kind: MovementKind,
        key: &Key,
        amounts: &[Decimal],
    ) -> Result<(), CoreError> {
        let sql = {
            let d = db.dialect();
            let cols: Vec<String> = ["recorder", "recorder_id", "kind", "period"]
                .iter()
                .map(|c| d.quote_ident(c))
                .chain(names(d, &self.dims))
                .chain(names(d, &self.resources))
                .collect();
            format!(
                "INSERT INTO {} ({}) VALUES ({})",
                d.quote_ident(&self.name),
                cols.join(", "),
                placeholders(d, 1, cols.len()),
            )
        };
        let mut values = vec![
            recorder.into(),
            Value::Int(recorder_id),
            Value::Int(kind.sign()),
            period.map_or(Value::Null, |p| Value::Text(p.to_owned())),
        ];
        values.extend(key.iter().cloned());
        values.extend(amounts.iter().map(|a| Value::Int(a.units())));
        db.execute(&sql, &values)?;
        Ok(())
    }

    fn recorded(
        &self,
        db: &mut dyn Driver,
        recorder: &str,
        recorder_id: i64,
    ) -> Result<Vec<Recorded>, CoreError> {
        let sql = {
            let d = db.dialect();
            let cols: Vec<String> = std::iter::once(d.quote_ident("kind"))
                .chain(names(d, &self.dims))
                .chain(names(d, &self.resources))
                .chain(std::iter::once(format!(
                    "COALESCE({}, '{NO_DATE}')",
                    d.quote_ident("period")
                )))
                .collect();
            format!(
                "SELECT {} FROM {} WHERE {} = {} AND {} = {}",
                cols.join(", "),
                d.quote_ident(&self.name),
                d.quote_ident("recorder"),
                d.placeholder(1),
                d.quote_ident("recorder_id"),
                d.placeholder(2),
            )
        };
        db.query(&sql, &[recorder.into(), Value::Int(recorder_id)])?
            .into_iter()
            .map(|mut row| {
                let Some(Value::Int(sign)) = row.first().cloned() else {
                    return Err(CoreError::Invalid(format!(
                        "битое движение в {}",
                        self.name
                    )));
                };
                let period = match row.pop() {
                    Some(Value::Text(p)) => p,
                    _ => NO_DATE.to_owned(),
                };
                let rest = row.split_off(1 + self.dims.len());
                let key = row.split_off(1);
                Ok((sign, key, self.decimals(rest)?, period))
            })
            .collect()
    }

    fn decimals(&self, row: Vec<Value>) -> Result<Vec<Decimal>, CoreError> {
        self.resources
            .iter()
            .zip(row)
            .map(|(def, value)| match value {
                Value::Int(units) => Ok(Decimal::new(units, scale(def))?),
                _ => Err(CoreError::Invalid(format!(
                    "в итогах {} не число",
                    def.name
                ))),
            })
            .collect()
    }

    /// Остаток не должен уходить в минус на конец ни одного дня, начиная с `from`:
    /// иначе документ задним числом «съедает» товар, которого тогда ещё не было.
    /// Внутри дня порядок документов не важен — проверяем итог дня.
    fn check_history(&self, db: &mut dyn Driver, key: &Key, from: &str) -> Result<(), CoreError> {
        let sql = {
            let d = db.dialect();
            let period = format!("COALESCE({}, '{NO_DATE}')", d.quote_ident("period"));
            let kind = d.quote_ident("kind");
            let mut cols = vec![period.clone()];
            for r in &self.resources {
                cols.push(format!(
                    "CAST(SUM({kind} * {}) AS BIGINT)",
                    d.quote_ident(&r.name)
                ));
            }
            format!(
                "SELECT {} FROM {} WHERE {} GROUP BY {period} ORDER BY {period}",
                cols.join(", "),
                d.quote_ident(&self.name),
                self.where_key(d, 1),
            )
        };
        let mut running = self.zeros()?;
        for row in db.query(&sql, key)? {
            let mut values = row.into_iter();
            let day = match values.next() {
                Some(Value::Text(day)) => day,
                _ => NO_DATE.to_owned(),
            };
            for ((def, total), value) in self.resources.iter().zip(running.iter_mut()).zip(values) {
                let delta = match value {
                    Value::Int(units) => Decimal::new(units, scale(def))?,
                    _ => Decimal::zero(scale(def))?,
                };
                *total = total.checked_add(delta)?;
                if day.as_str() >= from && total.is_negative() {
                    return Err(CoreError::NegativeBalance {
                        register: self.name.clone(),
                        resource: def.name.clone(),
                        balance: *total,
                        date: (day != NO_DATE).then(|| day.clone()),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Движения без даты (документ без поля даты) считаем самыми ранними.
pub(super) const NO_DATE: &str = "0000-00-00";

/// Движение документа: знак, ключ измерений, ресурсы, день.
type Recorded = (i64, Key, Vec<Decimal>, String);

pub(super) fn scale(def: &FieldDef) -> u8 {
    match def.kind {
        FieldKind::Decimal { scale } => scale,
        _ => 0,
    }
}

fn push_unique(keys: &mut Vec<Key>, key: Key) {
    if !keys.contains(&key) {
        keys.push(key);
    }
}
