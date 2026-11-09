//! Безопасность: пользователи в таблице `ostov_users`, пароли Argon2id, вход с задержкой
//! после ошибок, права по ролям из конфигурации, сессии в памяти, журнал изменений
//! `ostov_journal` (СТ-30, 31, 32, 34, 36, 37).
//!
//! Функции не открывают транзакций: ядро зовёт их внутри своей.

pub mod error;
pub mod journal;
pub mod password;
pub mod rights;
pub mod sessions;
pub mod users;

use ostov_store::Driver;

pub use error::SecError;
pub use journal::{Action, Change, Entry, JournalPage, JournalQuery, JournalRow};
pub use password::Password;
pub use rights::{rights, system_rights};
pub use sessions::Sessions;
pub use users::{
    NewUser, User, add_user, delay_secs, has_users, list_users, set_active, set_password, set_role,
    verify,
};

/// Создаёт `ostov_users` и `ostov_journal`, если их нет.
///
/// # Errors
/// Ошибка базы.
pub fn create_tables(db: &mut dyn Driver) -> Result<(), SecError> {
    users::create_table(db)?;
    journal::create_table(db)
}

#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod tests;
