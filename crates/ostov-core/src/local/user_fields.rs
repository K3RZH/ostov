//! Свойства пользователя (ADR-043): описаны в конфигурации (`Meta.user_fields`), значения —
//! строки таблицы по пользователю. На них опираются ограничения по записям (СТ-52).

use std::collections::BTreeMap;

use ostov_api::{EngineResult, UserFields};
use ostov_model::UserFieldDef;
use ostov_sec::Action;
use ostov_store::{Driver, Value};

use super::LocalEngine;
use super::convert::{error, invalid, value_from_api, value_to_api};
use crate::object::{from_value, to_value};
use crate::{CoreError, FieldDef, FieldKind, FieldValue, in_transaction};

const TABLE: &str = "ostov_user_fields";

pub(super) fn create_table(db: &mut dyn Driver) -> Result<(), CoreError> {
    let sql = {
        let d = db.dialect();
        let q = |c: &str| d.quote_ident(c);
        format!(
            "CREATE TABLE IF NOT EXISTS {} ({} TEXT NOT NULL, {} TEXT NOT NULL, {} BIGINT NOT NULL, \
             {} BIGINT, {} TEXT, PRIMARY KEY ({}, {}, {}))",
            q(TABLE),
            q("login"),
            q("field"),
            q("n"),
            q("num"),
            q("txt"),
            q("login"),
            q("field"),
            q("n"),
        )
    };
    db.execute(&sql, &[])?;
    Ok(())
}

/// Значения свойства пользователя в порядке записи.
pub(crate) fn values(
    db: &mut dyn Driver,
    login: &str,
    def: &FieldDef,
) -> Result<Vec<FieldValue>, CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "SELECT {}, {} FROM {} WHERE {} = {} AND {} = {} ORDER BY {}",
            d.quote_ident("num"),
            d.quote_ident("txt"),
            d.quote_ident(TABLE),
            d.quote_ident("login"),
            d.placeholder(1),
            d.quote_ident("field"),
            d.placeholder(2),
            d.quote_ident("n"),
        )
    };
    db.query(&sql, &[login.into(), def.name.as_str().into()])?
        .into_iter()
        .map(|row| {
            let mut row = row.into_iter();
            let value = match (row.next(), row.next()) {
                (Some(Value::Int(n)), _) => Value::Int(n),
                (_, Some(text)) => text,
                _ => Value::Null,
            };
            from_value(def, value)
        })
        .collect()
}

fn write(
    db: &mut dyn Driver,
    login: &str,
    def: &FieldDef,
    values: &[FieldValue],
) -> Result<(), CoreError> {
    let (delete, insert) = {
        let d = db.dialect();
        let q = |c: &str| d.quote_ident(c);
        (
            format!(
                "DELETE FROM {} WHERE {} = {} AND {} = {}",
                q(TABLE),
                q("login"),
                d.placeholder(1),
                q("field"),
                d.placeholder(2),
            ),
            format!(
                "INSERT INTO {} ({}, {}, {}, {}, {}) VALUES ({}, {}, {}, {}, {})",
                q(TABLE),
                q("login"),
                q("field"),
                q("n"),
                q("num"),
                q("txt"),
                d.placeholder(1),
                d.placeholder(2),
                d.placeholder(3),
                d.placeholder(4),
                d.placeholder(5),
            ),
        )
    };
    db.execute(&delete, &[login.into(), def.name.as_str().into()])?;
    for (n, value) in (0_i64..).zip(values) {
        let (num, txt) = match to_value(def, value)? {
            Value::Int(i) => (Value::Int(i), Value::Null),
            text => (Value::Null, text),
        };
        let row = [
            login.into(),
            def.name.as_str().into(),
            Value::Int(n),
            num,
            txt,
        ];
        db.execute(&insert, &row)?;
    }
    Ok(())
}

/// Свойства пользователя для конфигурации: имена — как у полей, ссылка — только на справочник.
pub(super) fn check(
    fields: &[UserFieldDef],
    catalogs: &BTreeMap<String, crate::Catalog>,
) -> Result<(), CoreError> {
    let defs: Vec<FieldDef> = fields.iter().map(|f| f.field.clone()).collect();
    crate::object::check_fields(&defs, &[])?;
    for f in &defs {
        let target = f.target.as_ref().is_some_and(|t| catalogs.contains_key(t));
        if f.kind == FieldKind::Ref && !target {
            return Err(CoreError::Invalid(format!(
                "свойство пользователя {}: ссылка только на справочник конфигурации",
                f.name
            )));
        }
    }
    Ok(())
}

impl LocalEngine {
    pub(super) fn run_admin_user_fields(&self, login: &str) -> EngineResult<UserFields> {
        self.need_system(|s| s.admin, "администрирование")?;
        let mut db = self.db()?;
        let (_, login) = user(&mut **db, login)?;
        let mut out = UserFields::new();
        for def in &self.config.meta.user_fields {
            let def = &def.field;
            let mut list: Vec<ostov_api::Value> = values(&mut **db, &login, def)
                .map_err(error)?
                .iter()
                .map(value_to_api)
                .collect();
            self.titles(&mut **db, def, &mut list)?;
            out.insert(def.name.clone(), list);
        }
        Ok(out)
    }

    pub(super) fn run_admin_set_user_fields(
        &self,
        login: &str,
        fields: &UserFields,
    ) -> EngineResult<()> {
        self.need_system(|s| s.admin, "администрирование")?;
        let mut checked = Vec::with_capacity(fields.len());
        for (name, given) in fields {
            let Some(def) = self
                .config
                .meta
                .user_fields
                .iter()
                .find(|f| &f.field.name == name)
            else {
                return Err(invalid(format!(
                    "в конфигурации нет свойства пользователя {name}"
                )));
            };
            let mut list = Vec::with_capacity(given.len());
            for value in given {
                let value = value_from_api(value)?;
                if value != FieldValue::Null && !list.contains(&value) {
                    to_value(&def.field, &value).map_err(error)?;
                    list.push(value);
                }
            }
            if !def.many && list.len() > 1 {
                return Err(invalid(format!(
                    "свойство «{}»: только одно значение",
                    def.field.title
                )));
            }
            checked.push((def.field.clone(), list));
        }
        let mut db = self.db()?;
        let (id, login) = user(&mut **db, login)?;
        in_transaction(&mut **db, |db| {
            for (def, list) in &checked {
                self.check_refs(db, def, list)?;
                write(db, &login, def, list)?;
            }
            let names: Vec<&str> = checked.iter().map(|(d, _)| d.title.as_str()).collect();
            let note = format!("свойства пользователя {login}: {}", names.join(", "));
            self.journal(db, Action::Update, "", Some(id), &[], &note)
        })
        .map_err(error)
    }

    /// Ссылка свойства должна вести на существующий элемент справочника.
    fn check_refs(
        &self,
        db: &mut dyn Driver,
        def: &FieldDef,
        list: &[FieldValue],
    ) -> Result<(), CoreError> {
        let ids: Vec<i64> = list
            .iter()
            .filter_map(|v| match v {
                FieldValue::Ref(id) => Some(*id),
                _ => None,
            })
            .collect();
        let Some(catalog) = def
            .target
            .as_ref()
            .and_then(|t| self.config.catalogs.get(t))
        else {
            return Ok(());
        };
        if ids.is_empty() {
            return Ok(());
        }
        let found = catalog.titles(db, &ids)?;
        match ids.iter().find(|id| !found.contains_key(id)) {
            Some(id) => Err(CoreError::Invalid(format!(
                "свойство «{}»: в справочнике нет элемента {id}",
                def.title
            ))),
            None => Ok(()),
        }
    }

    fn titles(
        &self,
        db: &mut dyn Driver,
        def: &FieldDef,
        list: &mut [ostov_api::Value],
    ) -> EngineResult<()> {
        let Some(catalog) = def
            .target
            .as_ref()
            .and_then(|t| self.config.catalogs.get(t))
        else {
            return Ok(());
        };
        let ids: Vec<i64> = list
            .iter()
            .filter_map(|v| match v {
                ostov_api::Value::Ref { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let titles = catalog.titles(db, &ids).map_err(error)?;
        for value in list {
            if let ostov_api::Value::Ref { id, title } = value {
                *title = titles.get(id).cloned();
            }
        }
        Ok(())
    }
}

/// Пользователь по логину без учёта регистра и пробелов: id и логин, как он записан.
fn user(db: &mut dyn Driver, login: &str) -> EngineResult<(i64, String)> {
    let wanted = login.trim().to_lowercase();
    ostov_sec::list_users(db)
        .map_err(|e| error(CoreError::Security(e)))?
        .into_iter()
        .find(|u| u.login == wanted)
        .map(|u| (u.id, u.login))
        .ok_or_else(|| {
            ostov_api::EngineError::new(
                ostov_api::ErrorCode::NotFound,
                format!("пользователя {login} нет"),
            )
        })
}
