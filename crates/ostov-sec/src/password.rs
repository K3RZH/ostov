//! Пароли: Argon2id в формате PHC, соль случайная (СТ-30). Текст пароля не печатается (СТ-37).

use std::fmt;

use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};

use crate::SecError;

pub const MIN_LEN: usize = 12;
/// Верхняя граница, чтобы огромный «пароль» не грузил хеширование.
pub const MAX_LEN: usize = 1024;

/// Пароль в открытом виде. `Debug` печатает `***`: пароль не попадает в логи и ошибки.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Длина от `MIN_LEN` до `MAX_LEN` символов.
    pub(crate) fn check_length(&self) -> Result<(), SecError> {
        let len = self.0.chars().count();
        if (MIN_LEN..=MAX_LEN).contains(&len) {
            Ok(())
        } else {
            Err(SecError::Invalid(format!(
                "пароль: от {MIN_LEN} до {MAX_LEN} символов"
            )))
        }
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

impl From<&str> for Password {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

/// Стоимость Argon2id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashParams {
    /// Память, КиБ.
    pub memory_kib: u32,
    pub passes: u32,
    pub threads: u32,
}

impl HashParams {
    /// Рекомендация OWASP для Argon2id: 19 МиБ, 2 прохода, 1 поток.
    pub const OWASP: Self = Self {
        memory_kib: 19_456,
        passes: 2,
        threads: 1,
    };
    /// Только для тестов: OWASP-хеш занимает десятки миллисекунд, тестов с ним сотни.
    pub const LIGHT: Self = Self {
        memory_kib: 64,
        passes: 1,
        threads: 1,
    };

    /// Что берут пользователи: в рабочем коде всегда OWASP.
    pub(crate) const fn current() -> Self {
        if cfg!(test) { Self::LIGHT } else { Self::OWASP }
    }
}

/// Хеш пароля строкой PHC (`$argon2id$v=19$m=…`).
///
/// # Errors
/// `SecError::Crypto`, если неверны параметры или нет случайных чисел для соли.
pub fn hash_password(password: &Password, params: HashParams) -> Result<String, SecError> {
    let params = Params::new(params.memory_kib, params.passes, params.threads, None)
        .map_err(|e| SecError::Crypto(e.to_string()))?;
    let hash: PasswordHash = Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(password.as_bytes())
        .map_err(|e| SecError::Crypto(e.to_string()))?;
    Ok(hash.to_string())
}

/// Совпадает ли пароль с хешем. Параметры берутся из самого хеша.
pub fn verify_password(password: &Password, phc: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), phc)
        .is_ok()
}
