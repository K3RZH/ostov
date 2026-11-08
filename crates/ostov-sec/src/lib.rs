//! Безопасность: пользователи в таблице `ostov_users`, пароли Argon2id, вход с задержкой
//! после ошибок, права по ролям из конфигурации, сессии в памяти (СТ-30, 31, 32, 36, 37).
//!
//! Функции не открывают транзакций: ядро зовёт их внутри своей.

pub mod error;
pub mod password;
pub mod rights;
pub mod sessions;
pub mod users;

pub use error::SecError;
pub use password::Password;
pub use rights::{rights, system_rights};
pub use sessions::Sessions;
pub use users::{
    NewUser, User, add_user, create_tables, delay_secs, has_users, list_users, set_active,
    set_password, set_role, verify,
};

#[cfg(test)]
mod tests;
