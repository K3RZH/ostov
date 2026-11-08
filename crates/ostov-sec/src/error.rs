use std::fmt;

use ostov_store::StoreError;

#[derive(Debug)]
pub enum SecError {
    Store(StoreError),
    /// Один ответ и на неизвестный логин, и на неверный пароль. Перед ответом вызывающий
    /// выжидает `delay_secs` (СТ-32).
    WrongCredentials {
        delay_secs: u64,
    },
    Disabled,
    /// Данные не прошли проверку: логин, имя, роль, длина пароля.
    Invalid(String),
    NotFound {
        login: String,
    },
    LastAdmin,
    /// Запись успели изменить после того, как её прочитали.
    VersionConflict {
        login: String,
    },
    /// Сбой хеширования или генератора случайных чисел; самого пароля здесь нет.
    Crypto(String),
}

impl SecError {
    /// Сколько выждать перед ответом клиенту.
    pub fn delay_secs(&self) -> u64 {
        match self {
            Self::WrongCredentials { delay_secs } => *delay_secs,
            Self::Store(_)
            | Self::Disabled
            | Self::Invalid(_)
            | Self::NotFound { .. }
            | Self::LastAdmin
            | Self::VersionConflict { .. }
            | Self::Crypto(_) => 0,
        }
    }
}

impl fmt::Display for SecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::WrongCredentials { .. } => write!(f, "неверный логин или пароль"),
            Self::Disabled => write!(f, "учётная запись отключена"),
            Self::Invalid(reason) => write!(f, "{reason}"),
            Self::NotFound { login } => write!(f, "пользователь «{login}» не найден"),
            Self::LastAdmin => write!(f, "нельзя отключить последнего администратора"),
            Self::VersionConflict { login } => write!(
                f,
                "пользователя «{login}» уже изменил другой администратор: обновите список"
            ),
            Self::Crypto(reason) => write!(f, "ошибка шифрования: {reason}"),
        }
    }
}

impl std::error::Error for SecError {}

impl From<StoreError> for SecError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}
