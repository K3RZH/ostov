//! Ограничения по записям (СТ-52, ADR-043): роли видно только то, где поле объекта — одно из
//! значений свойства пользователя. Действуют в списках, карточках, отчётах и на записи:
//! записать можно только то, что потом увидишь. Запросы `ostov-sql` к ограниченному объекту
//! пока закрыты: отбора по записям в переводе запроса ещё нет.

use ostov_api::{EngineError, EngineResult, ErrorCode, ObjectData};
use ostov_model::Restriction;

use super::LocalEngine;
use super::convert::{error, fields_from_api, value_from_api};
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
    fn rules(&self, object: &str) -> (Option<String>, Vec<&Restriction>) {
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
            .filter(|r| r.object == object)
            .collect();
        (Some(login), rules)
    }

    /// Есть ли у роли того, кто вошёл, ограничения на объект.
    pub(super) fn restricted(&self, object: &str) -> bool {
        !self.rules(object).1.is_empty()
    }

    /// Ограничения на объект для того, кто вошёл, со значениями его свойств.
    pub(super) fn within(&self, object: &str) -> EngineResult<Within> {
        let (Some(login), rules) = self.rules(object) else {
            return Ok(Within::new());
        };
        if rules.is_empty() {
            return Ok(Within::new());
        }
        let mut db = self.db()?;
        let mut within = Within::with_capacity(rules.len());
        for r in rules {
            let def = self
                .config
                .meta
                .user_fields
                .iter()
                .find(|u| u.field.name == r.user_field)
                .ok_or_else(|| {
                    error(CoreError::Invalid(format!(
                        "нет свойства пользователя {}",
                        r.user_field
                    )))
                })?;
            let values = user_fields::values(&mut **db, &login, &def.field).map_err(error)?;
            within.push((r.field.clone(), values));
        }
        Ok(within)
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

    /// Регистры `ostov-sql` по правам не проверяет: запрос, где упомянут регистр с ограничением
    /// для роли, закрыт целиком. Лишний отказ (так же назван столбец) безопаснее утечки.
    pub(super) fn need_query(&self, sql: &str) -> EngineResult<()> {
        let words: Vec<String> = sql
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map(str::to_lowercase)
            .collect();
        for register in self.config.registers.keys() {
            if self.restricted(register) && words.iter().any(|w| w == register) {
                return Err(EngineError::new(
                    ErrorCode::Forbidden,
                    format!(
                        "Запросы к регистру {register} для вашей роли закрыты: у неё ограничения \
                         по записям. Остатки — в отчёте."
                    ),
                ));
            }
        }
        Ok(())
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
