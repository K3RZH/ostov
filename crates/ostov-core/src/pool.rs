//! Пул соединений сервера: сессия берёт соединение на время команды и сразу отдаёт.
//! Без пула каждая сессия держала своё, и сотня пользователей упиралась в предел
//! соединений PostgreSQL.

use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ostov_store::{Driver, StoreError};

pub type Connect = Box<dyn Fn() -> Result<Box<dyn Driver + Send>, StoreError> + Send + Sync>;

/// Сколько команда ждёт свободного соединения, прежде чем сдаться с `Busy`.
const WAIT: Duration = Duration::from_secs(30);

pub struct Pool {
    connect: Connect,
    max: usize,
    wait: Duration,
    state: Mutex<State>,
    back: Condvar,
}

struct State {
    free: Vec<Box<dyn Driver + Send>>,
    /// Сколько соединений открыто: свободные и выданные.
    open: usize,
}

impl Pool {
    /// Первое соединение открывается сразу: так видно, что база доступна.
    pub fn new(connect: Connect, max: usize) -> Result<Self, StoreError> {
        let first = connect()?;
        Ok(Self {
            connect,
            max: max.max(1),
            wait: WAIT,
            state: Mutex::new(State {
                free: vec![first],
                open: 1,
            }),
            back: Condvar::new(),
        })
    }

    /// Сколько ждать свободного соединения (по умолчанию 30 с).
    pub fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// Свободное соединение, новое (если до предела) или ожидание, пока вернут.
    pub fn get(&self) -> Result<Pooled<'_>, StoreError> {
        let until = Instant::now() + self.wait;
        let mut state = self.lock();
        loop {
            if let Some(db) = state.free.pop() {
                return Ok(self.pooled(db));
            }
            if state.open < self.max {
                state.open += 1;
                drop(state);
                return match (self.connect)() {
                    Ok(db) => Ok(self.pooled(db)),
                    Err(e) => {
                        self.lock().open -= 1;
                        self.back.notify_one();
                        Err(e)
                    }
                };
            }
            let Some(left) = until.checked_duration_since(Instant::now()) else {
                return Err(StoreError::Busy(format!(
                    "все {} соединений с базой заняты",
                    self.max
                )));
            };
            state = self
                .back
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Закрывает свободные соединения; выданные закроются, когда их вернут.
    pub fn clear(&self) {
        let mut state = self.lock();
        let freed = state.free.len();
        state.free.clear();
        state.open -= freed;
    }

    fn pooled(&self, db: Box<dyn Driver + Send>) -> Pooled<'_> {
        Pooled {
            pool: self,
            db: Some(db),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Соединение из пула; при выходе из области видимости возвращается в пул.
pub struct Pooled<'a> {
    pool: &'a Pool,
    db: Option<Box<dyn Driver + Send>>,
}

impl Deref for Pooled<'_> {
    type Target = Box<dyn Driver + Send>;

    fn deref(&self) -> &Self::Target {
        self.db
            .as_ref()
            .unwrap_or_else(|| unreachable!("соединение уже возвращено"))
    }
}

impl DerefMut for Pooled<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.db
            .as_mut()
            .unwrap_or_else(|| unreachable!("соединение уже возвращено"))
    }
}

impl Drop for Pooled<'_> {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            self.pool.lock().free.push(db);
            self.pool.back.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ostov_store::drivers::sqlite::SqliteDriver;

    use super::*;

    fn counted(opened: &Arc<AtomicUsize>) -> Connect {
        let opened = Arc::clone(opened);
        Box::new(move || {
            opened.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(SqliteDriver::open_in_memory()?) as Box<dyn Driver + Send>)
        })
    }

    #[test]
    fn reuses_and_keeps_to_limit() {
        let opened = Arc::new(AtomicUsize::new(0));
        let pool = Pool::new(counted(&opened), 2)
            .unwrap()
            .with_wait(Duration::from_millis(200));
        {
            let mut a = pool.get().unwrap();
            let _b = pool.get().unwrap();
            assert_eq!(opened.load(Ordering::SeqCst), 2);
            a.query("SELECT 1", &[]).unwrap();
            // Оба выданы: третьего нет, ждём и сдаёмся с Busy.
            assert!(matches!(pool.get(), Err(StoreError::Busy(_))));
        }
        // Вернули — берём те же, новых не открываем.
        drop(pool.get().unwrap());
        drop(pool.get().unwrap());
        assert_eq!(opened.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn waits_until_one_comes_back() {
        let opened = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(Pool::new(counted(&opened), 1).unwrap());
        let held = pool.get().unwrap();
        let other = Arc::clone(&pool);
        let waiting = std::thread::spawn(move || {
            let started = Instant::now();
            drop(other.get().unwrap());
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(300));
        drop(held);
        assert!(waiting.join().unwrap() >= Duration::from_millis(250));
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn broken_base_is_an_error_not_a_hang() {
        let tries = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&tries);
        let connect: Connect = Box::new(move || {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Box::new(SqliteDriver::open_in_memory()?) as Box<dyn Driver + Send>)
            } else {
                Err(StoreError::Database("база недоступна".into()))
            }
        });
        let pool = Pool::new(connect, 2).unwrap();
        let _first = pool.get().unwrap();
        assert!(matches!(pool.get(), Err(StoreError::Database(_))));
        // Неудачная попытка не съела место в пуле: попробуем снова — снова честная ошибка.
        assert!(matches!(pool.get(), Err(StoreError::Database(_))));
    }
}
