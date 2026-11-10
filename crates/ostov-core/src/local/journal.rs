//! Журнал изменений (СТ-34): событие пишется в той же транзакции, что и само изменение,
//! откат изменения откатывает и запись.

use std::time::{SystemTime, UNIX_EPOCH};

use ostov_api::{EngineError, EngineResult, ErrorCode, JournalEntry, JournalPage};
use ostov_sec::{Action, Change};
use ostov_store::Driver;

use super::LocalEngine;
use super::access::sec_error;
use crate::{CoreError, FieldDef, FieldKind, FieldValue, Fields};

impl LocalEngine {
    /// Событие от имени того, кто вошёл (`""` — база без пользователей).
    pub(super) fn journal(
        &self,
        db: &mut dyn Driver,
        action: Action,
        object: &str,
        object_id: Option<i64>,
        changes: &[Change],
        note: &str,
    ) -> Result<(), CoreError> {
        let login = self
            .access()
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        record(db, &login, action, object, object_id, changes, note)
    }

    /// Изменённые поля: заголовок и значения текстом. `old = None` — новая запись.
    pub(super) fn changes(
        &self,
        db: &mut dyn Driver,
        defs: &[FieldDef],
        old: Option<&Fields>,
        new: &Fields,
    ) -> Vec<Change> {
        let mut changes = Vec::new();
        for def in defs {
            let before = old.and_then(|f| f.get(&def.name));
            let after = new.get(&def.name);
            let empty = |v: Option<&FieldValue>| matches!(v, None | Some(FieldValue::Null));
            if before == after || (empty(before) && empty(after)) {
                continue;
            }
            changes.push(Change {
                field: def.title.clone(),
                old: before.and_then(|v| self.show(db, def, v)),
                new: after.and_then(|v| self.show(db, def, v)),
            });
        }
        changes
    }

    fn show(&self, db: &mut dyn Driver, def: &FieldDef, value: &FieldValue) -> Option<String> {
        Some(match value {
            FieldValue::Null => return None,
            FieldValue::Int(n) => n.to_string(),
            FieldValue::Text(s) => s.clone(),
            FieldValue::Decimal(d) => d.to_string().replace('.', ","),
            FieldValue::Date(d) => super::ru_date(d),
            FieldValue::Ref(id) => def
                .target
                .as_ref()
                .filter(|_| def.kind == FieldKind::Ref)
                .and_then(|t| self.config.catalogs.get(t))
                .and_then(|c| c.titles(db, &[*id]).ok())
                .and_then(|mut t| t.remove(id))
                .unwrap_or_else(|| format!("#{id}")),
        })
    }

    pub(super) fn run_admin_journal(
        &self,
        after_key: Option<&str>,
        size: u32,
    ) -> EngineResult<JournalPage> {
        self.need_system(|s| s.admin, "администрирование")?;
        let after_id = match after_key {
            None => None,
            Some(key) => Some(key.parse().map_err(|_| {
                EngineError::new(ErrorCode::Validation, "испорченный ключ страницы журнала")
            })?),
        };
        let query = ostov_sec::JournalQuery {
            after_id,
            size,
            ..ostov_sec::JournalQuery::default()
        };
        let page = ostov_sec::journal::list(&mut **self.db(), &query).map_err(sec_error)?;
        let entries = page
            .rows
            .into_iter()
            .map(|row| {
                let title = self
                    .config
                    .meta
                    .objects
                    .iter()
                    .find(|o| o.name == row.object)
                    .map_or(row.object.clone(), |o| o.title.clone());
                JournalEntry {
                    id: row.id,
                    when: row.at,
                    user: row.login,
                    object: match row.object_id {
                        Some(id) if !title.is_empty() => format!("{title} #{id}"),
                        _ => title,
                    },
                    action: row.action_title,
                    details: row.details,
                }
            })
            .collect();
        Ok(JournalPage {
            entries,
            next_key: page.next_after_id.map(|id| id.to_string()),
        })
    }
}

/// Событие от имени `login`: вход пишется под тем, кто пытался войти.
pub(super) fn record(
    db: &mut dyn Driver,
    login: &str,
    action: Action,
    object: &str,
    object_id: Option<i64>,
    changes: &[Change],
    note: &str,
) -> Result<(), CoreError> {
    let at = utc_now();
    ostov_sec::journal::record(
        db,
        &ostov_sec::Entry {
            at: &at,
            login,
            action,
            object,
            object_id,
            changes,
            note,
        },
    )?;
    Ok(())
}

/// Сейчас по UTC: `2026-11-08T12:34:56Z`.
fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (year, month, day) = civil(i64::try_from(secs / 86_400).unwrap_or(0));
    let rest = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// День от 1970-01-01 → (год, месяц, число), алгоритм Хиннанта.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::civil;

    #[test]
    fn days_to_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(20_400), (2025, 11, 8));
    }
}
