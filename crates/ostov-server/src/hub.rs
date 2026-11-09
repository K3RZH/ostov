//! Сессии сервера: каждому вошедшему клиенту — свой экземпляр ядра со своим соединением
//! с базой и своим пользователем. Ядро так и знает одного пользователя на экземпляр,
//! а одновременную работу разводит база (блокировки строк в ядре).
//!
//! Всё здесь синхронное и зовётся из рабочих потоков (`spawn_blocking`), не из асинхронных.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use ostov_api::{
    Engine, EngineError, EngineResult, ErrorCode, HelloInfo, PROTOCOL_VERSION, SessionInfo,
};
use ostov_core::LocalEngine;
use ostov_core::local::{Config, Rules};
use ostov_sec::Sessions;
use ostov_store::{Driver, StoreError};

/// Новое соединение с базой: для PostgreSQL — `PostgresDriver::connect`.
pub type Connect = Arc<dyn Fn() -> Result<Box<dyn Driver + Send>, StoreError> + Send + Sync>;

/// База сервера: как подключаться, какая конфигурация и какие правила документов.
pub struct Base {
    connect: Connect,
    model: ostov_model::Config,
    rules: Option<Arc<dyn Rules + Send + Sync>>,
}

impl Base {
    pub fn new(connect: Connect, model: ostov_model::Config) -> Self {
        Self {
            connect,
            model,
            rules: None,
        }
    }

    /// Правила документов не из Rust, например Python.
    pub fn with_rules(mut self, rules: Arc<dyn Rules + Send + Sync>) -> Self {
        self.rules = Some(rules);
        self
    }

    fn config(&self) -> EngineResult<Config> {
        match &self.rules {
            Some(rules) => Config::from_model_with(&self.model, rules.as_ref()),
            None => Config::from_model(&self.model),
        }
        .map_err(|e| EngineError::new(ErrorCode::Incompatible, e.to_string()))
    }

    fn connect(&self) -> EngineResult<Box<dyn Driver + Send>> {
        (self.connect)().map_err(|e| EngineError {
            code: ErrorCode::Unavailable,
            message: "Сервер не может подключиться к базе данных.".into(),
            details: e.to_string(),
        })
    }

    fn engine(&self) -> EngineResult<LocalEngine> {
        Ok(LocalEngine::new(self.connect()?, self.config()?))
    }

    /// Сброс пароля из консоли (`ostov-server --reset-password`): пустую базу не создаёт.
    pub fn reset_password(&self, login: &str, password: &str) -> EngineResult<()> {
        let mut db = self.connect()?;
        let exists = db
            .query("SELECT value FROM ostov_base WHERE key = 'config'", &[])
            .is_ok();
        if !exists {
            return Err(EngineError::new(
                ErrorCode::NotFound,
                "Базы ещё нет: сервер её создаст при первом запуске.",
            ));
        }
        LocalEngine::open(db, self.config()?)?.reset_password(login, password)
    }

    /// Один раз при запуске: пустую базу устанавливает, существующую доводит до конфигурации.
    fn prepare(&self) -> EngineResult<()> {
        let mut db = self.connect()?;
        let exists = db
            .query("SELECT value FROM ostov_base WHERE key = 'config'", &[])
            .is_ok();
        if exists {
            LocalEngine::open(db, self.config()?)?;
        } else {
            LocalEngine::install(db, self.config()?)?;
        }
        Ok(())
    }
}

pub struct Hub {
    base: Base,
    /// Соединение для вопросов без сессии: есть ли пользователи.
    lobby: Mutex<Option<Box<dyn Driver + Send>>>,
    sessions: Mutex<Sessions>,
    engines: Mutex<HashMap<String, Arc<LocalEngine>>>,
}

impl Hub {
    pub fn start(base: Base) -> EngineResult<Self> {
        base.prepare()?;
        let lobby = Mutex::new(Some(base.connect()?));
        Ok(Self {
            base,
            lobby,
            sessions: Mutex::new(Sessions::default()),
            engines: Mutex::new(HashMap::new()),
        })
    }

    /// В режиме 2 вход нужен всегда; `first_run` — пользователей ещё нет (СТ-36).
    pub fn hello(&self) -> EngineResult<HelloInfo> {
        Ok(HelloInfo {
            protocol_version: PROTOCOL_VERSION,
            config_name: self.base.model.title.clone(),
            config_version: self.base.model.version.clone(),
            config_format: 0,
            login_required: true,
            first_run: !self.has_users()?,
        })
    }

    /// Ядро для команды клиента с токеном `token`.
    pub fn session(&self, token: Option<&str>) -> EngineResult<Arc<LocalEngine>> {
        let Some(token) = token else {
            return Err(if self.has_users()? {
                EngineError::new(ErrorCode::BadCredentials, "Войдите в систему.")
            } else {
                EngineError::new(
                    ErrorCode::NeedFirstAdmin,
                    "Первый запуск сервера: создайте администратора.",
                )
            });
        };
        let alive = lock(&self.sessions).touch(token, now()).is_some();
        let engine = alive
            .then(|| lock(&self.engines).get(token).cloned())
            .flatten();
        engine.ok_or_else(|| {
            lock(&self.engines).remove(token);
            EngineError::new(
                ErrorCode::BadCredentials,
                "Сессия закончилась: войдите заново.",
            )
        })
    }

    pub fn login(&self, login: &str, password: &str) -> EngineResult<(SessionInfo, String)> {
        let engine = self.base.engine()?;
        let info = engine.login(login, password)?;
        self.open(engine, info)
    }

    pub fn create_first_admin(
        &self,
        login: &str,
        password: &str,
    ) -> EngineResult<(SessionInfo, String)> {
        let engine = self.base.engine()?;
        let info = engine.create_first_admin(login, password)?;
        self.open(engine, info)
    }

    pub fn logout(&self, token: &str) {
        lock(&self.sessions).close(token);
        lock(&self.engines).remove(token);
    }

    /// Отключённый пользователь выходит отовсюду сразу, а не через 30 минут.
    pub fn user_disabled(&self, login: &str) {
        lock(&self.sessions).close_user(login);
        lock(&self.engines).retain(|_, e| e.current_user().is_none_or(|u| u.login != login));
    }

    fn open(&self, engine: LocalEngine, info: SessionInfo) -> EngineResult<(SessionInfo, String)> {
        let user = engine.current_user().ok_or_else(|| {
            EngineError::new(ErrorCode::Internal, "вход прошёл, а пользователя нет")
        })?;
        let token = lock(&self.sessions)
            .open(user, now())
            .map_err(|e| EngineError::new(ErrorCode::Internal, e.to_string()))?;
        lock(&self.engines).insert(token.clone(), Arc::new(engine));
        Ok((info, token))
    }

    /// Закрывает все сессии и соединения. Синхронный драйвер PostgreSQL нельзя ронять
    /// в асинхронном потоке, поэтому сервер зовёт это в рабочем потоке при остановке.
    pub fn close(&self) {
        lock(&self.engines).clear();
        lock(&self.lobby).take();
    }

    fn has_users(&self) -> EngineResult<bool> {
        let mut lobby = lock(&self.lobby);
        let db = lobby
            .as_mut()
            .ok_or_else(|| EngineError::new(ErrorCode::Unavailable, "Сервер остановлен."))?;
        ostov_sec::has_users(&mut **db).map_err(|e| EngineError {
            code: ErrorCode::Unavailable,
            message: "Сервер не может прочитать базу данных.".into(),
            details: e.to_string(),
        })
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
