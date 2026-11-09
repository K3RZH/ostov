//! Сетевой протокол (ADR-026): сгенерированный gRPC-код и перевод между ним и `ostov-api`.
//! Нужен только серверу и клиенту: приложение в режиме 1 сеть не тянет.

/// Сгенерировано из `proto/ostov/v1/engine.proto`.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
pub mod pb {
    tonic::include_proto!("ostov.v1");
}

impl std::fmt::Debug for pb::Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("login", &self.login)
            .field("password", &"***")
            .finish()
    }
}

impl std::fmt::Debug for pb::NewUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewUser")
            .field("login", &self.login)
            .field("display_name", &self.display_name)
            .field("role", &self.role)
            .field("password", &"***")
            .finish()
    }
}

impl std::fmt::Debug for pb::UserPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserPassword")
            .field("login", &self.login)
            .field("password", &"***")
            .finish()
    }
}

mod convert;
mod status;
#[cfg(test)]
mod tests;

pub use convert::{fields_from_pb, fields_to_pb, post_action_from_pb, session_from_pb};
pub use status::{from_status, to_status};

/// Заголовок с токеном сессии: `authorization: Bearer <токен>`.
pub const AUTH_HEADER: &str = "authorization";
