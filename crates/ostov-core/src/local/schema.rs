//! Реструктуризация: база помнит, под какие поля создавались её таблицы (`ostov_schema`),
//! и при открытии с изменённой конфигурацией достраивает недостающее одной транзакцией.
//! Данные не удаляются: поле, убранное из конфигурации, остаётся в таблице.

use std::collections::BTreeMap;

use ostov_store::{Driver, Value};

use super::BASE_TABLE;
use super::config::Config;
use crate::object::column_defs;
use crate::{AccumulationRegister, CoreError, FieldDef, FieldKind};

const TABLE: &str = "ostov_schema";
const DIM: &str = "dim:";
const RES: &str = "res:";

/// Таблица → поле → тип (`decimal(3)`, `ref(goods)`; у регистра с ролью `dim:` / `res:`).
type Columns = BTreeMap<String, String>;

/// Записана ли структура: в базах до реструктуризации её нет.
pub(super) fn recorded(db: &mut dyn Driver) -> Result<bool, CoreError> {
    let sql = format!("SELECT value FROM {BASE_TABLE} WHERE key = 'schema'");
    Ok(!db.query(&sql, &[])?.is_empty())
}

/// Запоминает структуру только что созданных таблиц конфигурации.
pub(super) fn record(db: &mut dyn Driver, config: &Config) -> Result<(), CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "CREATE TABLE {} ({} TEXT NOT NULL, {} TEXT NOT NULL, {} TEXT NOT NULL, PRIMARY KEY ({}, {}))",
            d.quote_ident(TABLE),
            d.quote_ident("tbl"),
            d.quote_ident("col"),
            d.quote_ident("kind"),
            d.quote_ident("tbl"),
            d.quote_ident("col"),
        )
    };
    db.execute(&sql, &[])?;
    db.execute(
        &format!("INSERT INTO {BASE_TABLE} (key, value) VALUES ('schema', '1')"),
        &[],
    )?;
    for c in config.catalogs.values() {
        save(db, c.table(), c.fields(), "")?;
    }
    for d in config.documents.values() {
        save(db, d.kind.table(), d.kind.header(), "")?;
        for part in d.kind.parts() {
            save(db, &d.kind.part_table(part), part.fields(), "")?;
        }
    }
    save_register(db, &config.stock)
}

/// Доводит таблицы до конфигурации: новые объекты, табличные части и поля добавляются.
/// Смена типа поля и перестройка регистра с движениями отклоняются: данные пришлось бы терять.
pub(super) fn restructure(db: &mut dyn Driver, config: &Config) -> Result<(), CoreError> {
    let stored = load(db)?;
    for c in config.catalogs.values() {
        match stored.get(c.table()) {
            Some(columns) => add_fields(db, c.table(), c.fields(), columns)?,
            None => {
                c.create_table(db)?;
                save(db, c.table(), c.fields(), "")?;
            }
        }
    }
    for d in config.documents.values() {
        let kind = &d.kind;
        let Some(columns) = stored.get(kind.table()) else {
            kind.create_tables(db)?;
            save(db, kind.table(), kind.header(), "")?;
            for part in kind.parts() {
                save(db, &kind.part_table(part), part.fields(), "")?;
            }
            continue;
        };
        add_fields(db, kind.table(), kind.header(), columns)?;
        for part in kind.parts() {
            let table = kind.part_table(part);
            match stored.get(&table) {
                Some(columns) => add_fields(db, &table, part.fields(), columns)?,
                None => {
                    kind.create_part_table(db, part)?;
                    save(db, &table, part.fields(), "")?;
                }
            }
        }
    }
    restructure_register(db, &config.stock, stored.get(config.stock.name()))
}

fn restructure_register(
    db: &mut dyn Driver,
    reg: &AccumulationRegister,
    stored: Option<&Columns>,
) -> Result<(), CoreError> {
    let Some(stored) = stored else {
        reg.create_tables(db)?;
        return save_register(db, reg);
    };
    let wanted: Columns = roles(reg).collect();
    for (col, kind) in &wanted {
        if let Some(old) = stored.get(col).filter(|old| *old != kind) {
            return Err(changed(reg.name(), col, old, kind));
        }
    }
    let dims = |c: &Columns| -> Vec<String> {
        c.iter()
            .filter(|(_, k)| k.starts_with(DIM))
            .map(|(n, _)| n.clone())
            .collect()
    };
    let resource_removed = stored
        .iter()
        .any(|(col, kind)| kind.starts_with(RES) && !wanted.contains_key(col));
    if dims(stored) != dims(&wanted) || resource_removed {
        if reg.has_movements(db)? {
            return Err(CoreError::Incompatible(format!(
                "в регистре {} уже есть движения: измерения нельзя менять, а ресурсы — убирать",
                reg.name()
            )));
        }
        reg.drop_tables(db)?;
        reg.create_tables(db)?;
        forget(db, reg.name())?;
        return save_register(db, reg);
    }

    let added: Vec<&FieldDef> = reg
        .resources()
        .iter()
        .filter(|r| !stored.contains_key(&r.name))
        .collect();
    for r in added {
        let statements = {
            let d = db.dialect();
            let column = column_defs(d, std::slice::from_ref(r)).join("");
            [reg.name().to_owned(), reg.totals()].map(|table| {
                format!(
                    "ALTER TABLE {} ADD COLUMN {column} NOT NULL DEFAULT 0",
                    d.quote_ident(&table)
                )
            })
        };
        for sql in statements {
            db.execute(&sql, &[])?;
        }
        save(db, reg.name(), std::slice::from_ref(r), RES)?;
    }
    Ok(())
}

fn add_fields(
    db: &mut dyn Driver,
    table: &str,
    fields: &[FieldDef],
    stored: &Columns,
) -> Result<(), CoreError> {
    for field in fields {
        let kind = code(field);
        match stored.get(&field.name) {
            Some(old) if *old == kind => {}
            Some(old) => return Err(changed(table, &field.name, old, &kind)),
            None => {
                let sql = {
                    let d = db.dialect();
                    format!(
                        "ALTER TABLE {} ADD COLUMN {}",
                        d.quote_ident(table),
                        column_defs(d, std::slice::from_ref(field)).join("")
                    )
                };
                db.execute(&sql, &[])?;
                save(db, table, std::slice::from_ref(field), "")?;
            }
        }
    }
    Ok(())
}

fn changed(table: &str, field: &str, old: &str, new: &str) -> CoreError {
    CoreError::Incompatible(format!(
        "поле {field} в {table}: было {old}, стало {new} — тип поля с данными так не меняется; \
         заведите новое поле"
    ))
}

fn code(field: &FieldDef) -> String {
    match field.kind {
        FieldKind::Int => "int".to_owned(),
        FieldKind::Text => "text".to_owned(),
        FieldKind::Date => "date".to_owned(),
        FieldKind::Decimal { scale } => format!("decimal({scale})"),
        FieldKind::Ref => format!("ref({})", field.target.as_deref().unwrap_or_default()),
    }
}

fn roles(reg: &AccumulationRegister) -> impl Iterator<Item = (String, String)> + '_ {
    let dims = reg
        .dims()
        .iter()
        .map(|f| (f.name.clone(), format!("{DIM}{}", code(f))));
    let res = reg
        .resources()
        .iter()
        .map(|f| (f.name.clone(), format!("{RES}{}", code(f))));
    dims.chain(res)
}

fn save_register(db: &mut dyn Driver, reg: &AccumulationRegister) -> Result<(), CoreError> {
    save(db, reg.name(), reg.dims(), DIM)?;
    save(db, reg.name(), reg.resources(), RES)
}

fn save(
    db: &mut dyn Driver,
    table: &str,
    fields: &[FieldDef],
    role: &str,
) -> Result<(), CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "INSERT INTO {} ({}, {}, {}) VALUES ({}, {}, {})",
            d.quote_ident(TABLE),
            d.quote_ident("tbl"),
            d.quote_ident("col"),
            d.quote_ident("kind"),
            d.placeholder(1),
            d.placeholder(2),
            d.placeholder(3),
        )
    };
    for field in fields {
        db.execute(
            &sql,
            &[
                Value::Text(table.to_owned()),
                Value::Text(field.name.clone()),
                Value::Text(format!("{role}{}", code(field))),
            ],
        )?;
    }
    Ok(())
}

fn forget(db: &mut dyn Driver, table: &str) -> Result<(), CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "DELETE FROM {} WHERE {} = {}",
            d.quote_ident(TABLE),
            d.quote_ident("tbl"),
            d.placeholder(1)
        )
    };
    db.execute(&sql, &[Value::Text(table.to_owned())])?;
    Ok(())
}

fn load(db: &mut dyn Driver) -> Result<BTreeMap<String, Columns>, CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "SELECT {}, {}, {} FROM {}",
            d.quote_ident("tbl"),
            d.quote_ident("col"),
            d.quote_ident("kind"),
            d.quote_ident(TABLE)
        )
    };
    let mut stored: BTreeMap<String, Columns> = BTreeMap::new();
    for row in db.query(&sql, &[])? {
        if let [Value::Text(table), Value::Text(col), Value::Text(kind)] = row.as_slice() {
            stored
                .entry(table.clone())
                .or_default()
                .insert(col.clone(), kind.clone());
        }
    }
    Ok(stored)
}
