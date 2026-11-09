//! Сессии в памяти сервера: токен → пользователь. После перезапуска все входят заново (СТ-32).

use std::collections::HashMap;
use std::fmt::Write;

use crate::{SecError, User};

/// Без действий сессия живёт 30 минут.
pub const IDLE_SECS: u64 = 30 * 60;
/// Даже с действиями — не дольше 12 часов от входа.
pub const ABSOLUTE_SECS: u64 = 12 * 60 * 60;
/// 32 байта — 256 бит: токен не подобрать перебором.
const TOKEN_BYTES: usize = 32;

/// Открытые сессии. Время всегда передаёт вызывающий, в секундах.
#[derive(Debug)]
pub struct Sessions {
    idle_secs: u64,
    absolute_secs: u64,
    open: HashMap<String, Session>,
}

#[derive(Debug)]
struct Session {
    user: User,
    opened: u64,
    last_seen: u64,
}

impl Sessions {
    pub fn new(idle_secs: u64, absolute_secs: u64) -> Self {
        Self {
            idle_secs,
            absolute_secs,
            open: HashMap::new(),
        }
    }

    /// Новая сессия; токен — 64 hex-символа.
    ///
    /// # Errors
    /// `SecError::Crypto`, если система не дала случайных чисел.
    pub fn open(&mut self, user: User, now: u64) -> Result<String, SecError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| SecError::Crypto(e.to_string()))?;
        let token = bytes.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        let session = Session {
            user,
            opened: now,
            last_seen: now,
        };
        self.open.insert(token.clone(), session);
        Ok(token)
    }

    /// Пользователь живой сессии; продлевает её. Истёкшую закрывает и возвращает `None`.
    pub fn touch(&mut self, token: &str, now: u64) -> Option<User> {
        let session = self.open.get_mut(token)?;
        let idle_over = now >= session.last_seen.saturating_add(self.idle_secs);
        let absolute_over = now >= session.opened.saturating_add(self.absolute_secs);
        if idle_over || absolute_over {
            self.open.remove(token);
            return None;
        }
        session.last_seen = now;
        Some(session.user.clone())
    }

    pub fn close(&mut self, token: &str) {
        self.open.remove(token);
    }

    /// Закрывает все сессии пользователя: после отключения или смены роли он входит заново.
    pub fn close_user(&mut self, login: &str) {
        self.open.retain(|_, session| session.user.login != login);
    }
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new(IDLE_SECS, ABSOLUTE_SECS)
    }
}
