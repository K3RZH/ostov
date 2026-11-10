//! Перевод между типами ядра и контракта `ostov-api`.

use ostov_api::{EngineError, ErrorCode};
use ostov_store::Decimal;

use crate::{CoreError, FieldValue, Fields};

pub fn value_to_api(v: &FieldValue) -> ostov_api::Value {
    match v {
        FieldValue::Null => ostov_api::Value::Null,
        FieldValue::Int(i) => ostov_api::Value::Int(*i),
        FieldValue::Text(s) => ostov_api::Value::Text(s.clone()),
        FieldValue::Decimal(d) => ostov_api::Value::Decimal {
            units: d.units(),
            scale: d.scale(),
        },
        FieldValue::Date(s) => ostov_api::Value::Date(s.clone()),
        FieldValue::Ref(id) => ostov_api::Value::Ref {
            id: *id,
            title: None,
        },
    }
}

pub fn value_from_api(v: &ostov_api::Value) -> Result<FieldValue, EngineError> {
    Ok(match v {
        ostov_api::Value::Null => FieldValue::Null,
        ostov_api::Value::Int(i) => FieldValue::Int(*i),
        ostov_api::Value::Text(s) => FieldValue::Text(s.clone()),
        ostov_api::Value::Decimal { units, scale } => {
            FieldValue::Decimal(Decimal::new(*units, *scale).map_err(|e| invalid(e.to_string()))?)
        }
        ostov_api::Value::Date(s) => FieldValue::Date(s.clone()),
        ostov_api::Value::Ref { id, .. } => FieldValue::Ref(*id),
        ostov_api::Value::Bool(_) => return Err(invalid("флажков в ядре пока нет".into())),
    })
}

/// Параметр запроса: даты строкой, ссылки и флажки целым — как они лежат в базе.
pub fn param_from_api(v: &ostov_api::Value) -> ostov_sql::Param {
    use ostov_api::Value;
    use ostov_sql::Param;
    match v {
        Value::Null => Param::Null,
        Value::Int(i) | Value::Ref { id: i, .. } => Param::Int(*i),
        Value::Bool(b) => Param::Int(i64::from(*b)),
        Value::Text(s) | Value::Date(s) => Param::Text(s.clone()),
        Value::Decimal { units, scale } => Param::Decimal {
            units: *units,
            scale: *scale,
        },
    }
}

pub fn fields_to_api(fields: &Fields) -> ostov_api::Fields {
    fields
        .iter()
        .map(|(k, v)| (k.clone(), value_to_api(v)))
        .collect()
}

pub fn fields_from_api(fields: &ostov_api::Fields) -> Result<Fields, EngineError> {
    fields
        .iter()
        .map(|(k, v)| Ok((k.clone(), value_from_api(v)?)))
        .collect()
}

pub fn invalid(message: String) -> EngineError {
    EngineError::new(ErrorCode::Validation, message)
}

pub fn error(e: CoreError) -> EngineError {
    if let CoreError::Security(e) = e {
        return super::access::sec_error(e);
    }
    if let CoreError::Script { message, details } = e {
        return EngineError {
            code: ErrorCode::Validation,
            message,
            details,
        };
    }
    let code = match &e {
        CoreError::VersionConflict { .. } => ErrorCode::VersionConflict,
        CoreError::NotFound { .. } => ErrorCode::NotFound,
        CoreError::PeriodClosed { .. } => ErrorCode::PeriodClosed,
        CoreError::Incompatible(_) => ErrorCode::Incompatible,
        CoreError::Security(_) | CoreError::Script { .. } => ErrorCode::Internal,
        CoreError::NegativeBalance(_) | CoreError::Invalid(_) => ErrorCode::Validation,
        CoreError::Query(ostov_sql::SqlError::NoRight { .. }) => ErrorCode::Forbidden,
        CoreError::Query(_) => ErrorCode::Validation,
        CoreError::Store(_) => {
            // Текст базы часто английский и технический — человеку общее сообщение.
            return EngineError {
                code: ErrorCode::Internal,
                message: "Ошибка базы данных. Повторите действие; если не поможет — сообщите администратору.".into(),
                details: e.to_string(),
            };
        }
    };
    EngineError::new(code, e.to_string())
}
