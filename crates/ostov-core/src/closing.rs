//! Закрытие периода (СТ-24): документы с датой по дату закрытия включительно нельзя
//! записать, провести, отменить или пометить на удаление. Учёт за закрытый месяц не «плывёт».

use ostov_store::{Driver, Value};

use crate::object::is_date;
use crate::{CoreError, FieldValue, Fields, in_transaction};

const TABLE: &str = "ostov_closing";

/// Таблица с датой закрытия; создаётся, если её нет (и в старых базах тоже).
pub fn create_table(db: &mut dyn Driver) -> Result<(), CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "CREATE TABLE IF NOT EXISTS {} ({} BIGINT PRIMARY KEY, {} TEXT NOT NULL)",
            d.quote_ident(TABLE),
            d.quote_ident("id"),
            d.quote_ident("closed_through"),
        )
    };
    db.execute(&sql, &[])?;
    Ok(())
}

pub fn closing_date(db: &mut dyn Driver) -> Result<Option<String>, CoreError> {
    let sql = {
        let d = db.dialect();
        format!(
            "SELECT {} FROM {}",
            d.quote_ident("closed_through"),
            d.quote_ident(TABLE)
        )
    };
    Ok(match db.query(&sql, &[])?.into_iter().flatten().next() {
        Some(Value::Text(date)) => Some(date),
        _ => None,
    })
}

/// Закрывает период по `date` включительно; `None` — открыть всё.
pub fn set_closing_date(db: &mut dyn Driver, date: Option<&str>) -> Result<(), CoreError> {
    if let Some(date) = date.filter(|d| !is_date(d)) {
        return Err(CoreError::Invalid(format!("«{date}» — не дата ГГГГ-ММ-ДД")));
    }
    in_transaction(db, |db| {
        let (delete, insert) = {
            let d = db.dialect();
            let table = d.quote_ident(TABLE);
            (
                format!("DELETE FROM {table}"),
                format!(
                    "INSERT INTO {table} ({}, {}) VALUES (1, {})",
                    d.quote_ident("id"),
                    d.quote_ident("closed_through"),
                    d.placeholder(1)
                ),
            )
        };
        db.execute(&delete, &[])?;
        if let Some(date) = date {
            db.execute(&insert, &[Value::Text(date.to_owned())])?;
        }
        Ok(())
    })
}

/// Документ с такой шапкой можно менять, только если его дата позже закрытия.
/// Документы без поля `date` закрытие не касается.
pub(crate) fn check_open(db: &mut dyn Driver, header: &Fields) -> Result<(), CoreError> {
    let Some(FieldValue::Date(date)) = header.get("date") else {
        return Ok(());
    };
    match closing_date(db)? {
        Some(closed) if *date <= closed => Err(CoreError::PeriodClosed {
            date: date.clone(),
            closed_through: closed,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    fn dated(date: &str) -> Fields {
        Fields::from([("date".to_owned(), FieldValue::Date(date.to_owned()))])
    }

    #[test]
    fn closes_through_date_inclusive() {
        let mut db = memory_db();
        create_table(&mut db).unwrap();
        assert_eq!(closing_date(&mut db).unwrap(), None);
        check_open(&mut db, &dated("2026-01-31")).unwrap();

        set_closing_date(&mut db, Some("2026-01-31")).unwrap();
        assert_eq!(
            closing_date(&mut db).unwrap().as_deref(),
            Some("2026-01-31")
        );
        assert!(matches!(
            check_open(&mut db, &dated("2026-01-31")),
            Err(CoreError::PeriodClosed { .. })
        ));
        check_open(&mut db, &dated("2026-02-01")).unwrap();
        check_open(&mut db, &Fields::new()).unwrap();

        set_closing_date(&mut db, None).unwrap();
        check_open(&mut db, &dated("2026-01-01")).unwrap();
        assert!(set_closing_date(&mut db, Some("31.01.2026")).is_err());
    }
}
