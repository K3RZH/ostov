//! Ограничения по записям (СТ-52, ADR-043): роли видно только то, где поле объекта — одно из
//! значений свойства пользователя. Действуют в списках, карточках, отчётах и на записи:
//! записать можно только то, что потом увидишь. В запросах `ostov-sql` отбор добавляет
//! перевод запроса по `limits`.

use std::collections::BTreeMap;

use ostov_api::{EngineError, EngineResult, ErrorCode, ObjectData};
use ostov_model::Restriction;
use ostov_store::Driver;

use super::LocalEngine;
use super::convert::{error, fields_from_api, param_from_api, value_from_api, value_to_api};
use super::user_fields;
use crate::{CoreError, FieldDef, FieldValue, Fields};

/// Поле и допустимые значения; пустой список — не видно ничего.
pub(super) type Within = Vec<(String, Vec<FieldValue>)>;

/// Проверка конфигурации: объект и поле есть, свойство пользователя того же типа.
pub(super) fn check(model: &ostov_model::Config) -> Result<(), CoreError> {
    for role in &model.roles {
        for r in &role.restrictions {
            let bad = |why: String| {
                Err(CoreError::Invalid(format!(
                    "ограничение роли {} на {}.{}: {why}",
                    role.name, r.object, r.field
                )))
            };
            let Some(fields) = object_fields(model, &r.object) else {
                return bad("нет такого справочника, документа или регистра".into());
            };
            let Some(field) = fields.iter().find(|f| f.name == r.field) else {
                return bad("нет такого поля".into());
            };
            let Some(user) = model
                .user_fields
                .iter()
                .find(|u| u.field.name == r.user_field)
            else {
                return bad(format!("нет свойства пользователя {}", r.user_field));
            };
            if user.field.kind != field.kind || user.field.target != field.target {
                return bad(format!(
                    "свойство пользователя {} другого типа",
                    r.user_field
                ));
            }
        }
    }
    Ok(())
}

/// Поля шапки справочника или документа, измерения регистра.
fn object_fields<'a>(model: &'a ostov_model::Config, object: &str) -> Option<&'a [FieldDef]> {
    let catalog = model.catalogs.iter().find(|c| c.name == object);
    let document = model.documents.iter().find(|d| d.name == object);
    let register = model.registers.iter().find(|r| r.name == object);
    catalog
        .map(|c| c.fields.as_slice())
        .or_else(|| document.map(|d| d.fields.as_slice()))
        .or_else(|| register.map(|r| r.dimensions.as_slice()))
}

/// Поле, которое не подходит под ограничения, или `None`.
pub(super) fn misfit<'a>(within: &'a Within, fields: &Fields) -> Option<&'a str> {
    within
        .iter()
        .find(|(name, values)| {
            let value = fields.get(name).unwrap_or(&FieldValue::Null);
            *value == FieldValue::Null || !values.contains(value)
        })
        .map(|(name, _)| name.as_str())
}

impl LocalEngine {
    /// Логин того, кто вошёл, и ограничения его роли (на объект или на все, если `None`).
    fn rules(&self, object: Option<&str>) -> (Option<String>, Vec<&Restriction>) {
        let login = self
            .access()
            .user
            .as_ref()
            .map(|u| (u.login.clone(), u.role.clone()));
        let Some((login, role)) = login else {
            return (None, Vec::new());
        };
        let rules = self
            .config
            .roles
            .iter()
            .filter(|r| r.name == role)
            .flat_map(|r| &r.restrictions)
            .filter(|r| object.is_none_or(|o| r.object == o))
            .collect();
        (Some(login), rules)
    }

    /// Ограничения на объект для того, кто вошёл, со значениями его свойств.
    pub(super) fn within(&self, object: &str) -> EngineResult<Within> {
        if self.rules(Some(object)).1.is_empty() {
            return Ok(Within::new());
        }
        let mut db = self.db()?;
        self.within_on(&mut **db, object).map_err(error)
    }

    /// То же на уже взятом соединении: внутри команды второе брать нельзя.
    fn within_on(&self, db: &mut dyn Driver, object: &str) -> Result<Within, CoreError> {
        let (Some(login), rules) = self.rules(Some(object)) else {
            return Ok(Within::new());
        };
        let mut within = Within::with_capacity(rules.len());
        for r in rules {
            let def = self
                .config
                .meta
                .user_fields
                .iter()
                .find(|u| u.field.name == r.user_field)
                .ok_or_else(|| {
                    CoreError::Invalid(format!("нет свойства пользователя {}", r.user_field))
                })?;
            let values = user_fields::values(db, &login, &def.field)?;
            within.push((r.field.clone(), values));
        }
        Ok(within)
    }

    /// Ограничения роли на все объекты для запроса `ostov-sql` — заранее, до перевода:
    /// сам перевод в базу не ходит.
    pub(super) fn limits(
        &self,
        db: &mut dyn Driver,
    ) -> Result<BTreeMap<String, ostov_sql::Limits>, CoreError> {
        let mut objects: Vec<String> = self
            .rules(None)
            .1
            .iter()
            .map(|r| r.object.clone())
            .collect();
        objects.sort();
        objects.dedup();
        let mut limits = BTreeMap::new();
        for object in objects {
            let list = self
                .within_on(db, &object)?
                .into_iter()
                .map(|(field, values)| {
                    let params = values
                        .iter()
                        .map(|v| param_from_api(&value_to_api(v)))
                        .collect();
                    (field, params)
                })
                .collect();
            limits.insert(object, list);
        }
        Ok(limits)
    }

    /// Запись, которую пользователь не видит, для него как будто не существует.
    pub(super) fn need_visible(&self, object: &str, id: i64) -> EngineResult<()> {
        let within = self.within(object)?;
        if within.is_empty() {
            return Ok(());
        }
        let data = self.run_get(object, id)?;
        match misfit(&within, &fields_from_api(&data.fields)?) {
            Some(_) => Err(EngineError::new(
                ErrorCode::NotFound,
                format!("в {object} нет записи {id}"),
            )),
            None => Ok(()),
        }
    }

    /// Записать можно только то, что потом увидишь; изменить — только то, что видишь.
    pub(super) fn need_fits(&self, object: &str, data: &ObjectData) -> EngineResult<()> {
        let within = self.within(object)?;
        if within.is_empty() {
            return Ok(());
        }
        if let Some(field) = misfit(&within, &fields_from_api(&data.fields)?) {
            let title = self
                .config
                .meta
                .objects
                .iter()
                .find(|o| o.name == object)
                .and_then(|o| o.fields.iter().find(|f| f.name == field))
                .map_or(field, |f| f.title.as_str());
            return Err(EngineError::new(
                ErrorCode::Forbidden,
                format!("Поле «{title}»: можно выбрать только из ваших значений."),
            ));
        }
        match data.id {
            Some(id) => self.need_visible(object, id),
            None => Ok(()),
        }
    }

    /// Строки отчёта по регистру — только по разрешённым измерениям.
    pub(super) fn keep_within(
        within: &Within,
        rows: &mut Vec<ostov_api::Fields>,
    ) -> EngineResult<()> {
        if within.is_empty() {
            return Ok(());
        }
        let mut kept = Vec::with_capacity(rows.len());
        for row in rows.drain(..) {
            let mut fields = Fields::new();
            for (name, _) in within {
                if let Some(v) = row.get(name) {
                    fields.insert(name.clone(), value_from_api(v)?);
                }
            }
            if misfit(within, &fields).is_none() {
                kept.push(row);
            }
        }
        *rows = kept;
        Ok(())
    }
}
