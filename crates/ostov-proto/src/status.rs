//! Ошибка ядра по сети: gRPC-код для чужих клиентов и прокси, а `pb::Error` целиком —
//! в details статуса, чтобы наш клиент получил тот же `EngineError`, что и локально.

use ostov_api::{EngineError, ErrorCode};
use prost::Message;
use tonic::{Code, Status};

use crate::pb;

pub fn to_status(e: &EngineError) -> Status {
    let code = match e.code {
        ErrorCode::NotImplemented => Code::Unimplemented,
        ErrorCode::Unavailable => Code::Unavailable,
        ErrorCode::BadCredentials => Code::Unauthenticated,
        ErrorCode::Forbidden | ErrorCode::PeriodClosed => Code::PermissionDenied,
        ErrorCode::VersionConflict => Code::Aborted,
        ErrorCode::Validation => Code::InvalidArgument,
        ErrorCode::NotFound => Code::NotFound,
        ErrorCode::NeedFirstAdmin
        | ErrorCode::NeedFilePassword
        | ErrorCode::AlreadyOpen
        | ErrorCode::Incompatible => Code::FailedPrecondition,
        ErrorCode::Internal => Code::Internal,
    };
    let body = pb::Error {
        code: pb::ErrorCode::from(e.code).into(),
        message: e.message.clone(),
        details: e.details.clone(),
    };
    Status::with_details(code, e.message.clone(), body.encode_to_vec().into())
}

/// Ошибка с нашего сервера — как есть; иначе (сеть, прокси, чужой сервер) — по gRPC-коду.
pub fn from_status(s: &Status) -> EngineError {
    if let Ok(body) = pb::Error::decode(s.details())
        && !s.details().is_empty()
    {
        return EngineError {
            code: pb::ErrorCode::try_from(body.code).map_or(ErrorCode::Internal, Into::into),
            message: body.message,
            details: body.details,
        };
    }
    let (code, message) = match s.code() {
        Code::Unavailable | Code::DeadlineExceeded | Code::Cancelled => (
            ErrorCode::Unavailable,
            "Нет связи с сервером. Проверьте сеть и что сервер запущен.",
        ),
        Code::Unauthenticated => (ErrorCode::BadCredentials, "Нужно войти заново."),
        Code::PermissionDenied => (ErrorCode::Forbidden, "Нет права на это действие."),
        Code::Unimplemented => (
            ErrorCode::NotImplemented,
            "Сервер не знает эту команду: версии программы и сервера разные.",
        ),
        _ => (
            ErrorCode::Internal,
            "Ошибка на стороне сервера. Повторите; если не поможет — сообщите администратору.",
        ),
    };
    EngineError {
        code,
        message: message.to_owned(),
        details: format!("{:?}: {}", s.code(), s.message()),
    }
}
