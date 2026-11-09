//! Вход и права в режиме 1 (СТ-30, 31, 36). Пока в базе нет пользователей, вход не нужен и
//! разрешено всё, кроме правки закрытого периода. После первого администратора — только по входу.

use std::sync::{MutexGuard, PoisonError};
use std::time::Duration;

use ostov_api::{EngineError, EngineResult, ErrorCode, SessionInfo, UserInfo};
use ostov_model::{ADMIN_ROLE, MenuItem, Meta, Rights, SystemRights};
use ostov_sec::{Action, Password, SecError, User};

use super::LocalEngine;
use super::convert::error;
use crate::in_transaction;

#[derive(Debug, Default)]
pub(super) struct Access {
    /// Есть ли в базе пользователи: тогда без входа ничего нельзя.
    pub(super) users: bool,
    pub(super) user: Option<User>,
}

/// Что нужно сделать с объектом.
#[derive(Debug, Clone, Copy)]
pub(super) enum Need {
    Read,
    Create,
    Update,
    Post,
    Unpost,
    Delete,
}

impl Need {
    fn granted(self, r: &Rights) -> bool {
        match self {
            Self::Read => r.read,
            Self::Create => r.create,
            Self::Update => r.update,
            Self::Post => r.post,
            Self::Unpost => r.unpost,
            Self::Delete => r.delete,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Self::Read => "просмотр",
            Self::Create => "создание",
            Self::Update => "изменение",
            Self::Post => "проведение",
            Self::Unpost => "отмену проведения",
            Self::Delete => "пометку на удаление",
        }
    }
}

impl LocalEngine {
    /// Кто вошёл в этот экземпляр ядра. Сервер держит по экземпляру на сессию и ведёт их по этому.
    pub fn current_user(&self) -> Option<User> {
        self.access().user.clone()
    }

    pub(super) fn access(&self) -> MutexGuard<'_, Access> {
        self.access.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Роль того, кто работает; `None` — база без пользователей.
    fn role(&self) -> EngineResult<Option<String>> {
        let access = self.access();
        match (&access.user, access.users) {
            (Some(user), _) => Ok(Some(user.role.clone())),
            (None, false) => Ok(None),
            (None, true) => Err(EngineError::new(
                ErrorCode::Forbidden,
                "Войдите в систему: в базе есть пользователи.",
            )),
        }
    }

    pub(super) fn need(&self, object: &str, need: Need) -> EngineResult<()> {
        let Some(role) = self.role()? else {
            return Ok(());
        };
        if need.granted(&ostov_sec::rights(&self.config.roles, &role, object)) {
            return Ok(());
        }
        let title = self
            .config
            .meta
            .objects
            .iter()
            .find(|o| o.name == object)
            .map_or(object, |o| o.title.as_str());
        Err(EngineError::new(
            ErrorCode::Forbidden,
            format!("Нет права на {} «{title}».", need.what()),
        ))
    }

    pub(super) fn need_system(
        &self,
        granted: fn(&SystemRights) -> bool,
        what: &str,
    ) -> EngineResult<()> {
        let Some(role) = self.role()? else {
            return Ok(());
        };
        if granted(&ostov_sec::system_rights(&self.config.roles, &role)) {
            Ok(())
        } else {
            Err(EngineError::new(
                ErrorCode::Forbidden,
                format!("Нет права: {what}."),
            ))
        }
    }

    /// Может ли пользователь менять документы закрытого периода. Без входа — нет:
    /// дата закрытия должна работать и в базе без пользователей.
    pub(super) fn may_edit_closed(&self) -> bool {
        let access = self.access();
        access
            .user
            .as_ref()
            .is_some_and(|u| ostov_sec::system_rights(&self.config.roles, &u.role).closed_period)
    }

    pub(super) fn login_required(&self) -> bool {
        let access = self.access();
        access.users && access.user.is_none()
    }

    pub(super) fn run_login(&self, login: &str, password: &str) -> EngineResult<SessionInfo> {
        let password = Password::new(password);
        // Транзакция фиксируется и при неудаче: счётчик ошибок и запись журнала должны остаться.
        let verified = {
            let mut db = self.db();
            in_transaction(&mut **db, |db| {
                let result = ostov_sec::verify(db, login, &password);
                let (action, note) = match &result {
                    Ok(_) => (Action::Login, ""),
                    Err(SecError::WrongCredentials { .. }) => {
                        (Action::LoginFailed, "неверный логин или пароль")
                    }
                    Err(SecError::Disabled) => (Action::LoginFailed, "учётная запись отключена"),
                    Err(_) => return Ok(result),
                };
                let who = login.trim().to_lowercase();
                super::journal::record(db, &who, action, "", None, &[], note)?;
                Ok(result)
            })
            .map_err(error)?
        };
        match verified {
            Ok(user) => Ok(self.enter(user)),
            Err(e) => {
                // Ждём уже без замка базы: остальные команды не стоят.
                std::thread::sleep(Duration::from_secs(e.delay_secs()));
                Err(sec_error(e))
            }
        }
    }

    pub(super) fn run_create_first_admin(
        &self,
        login: &str,
        password: &str,
    ) -> EngineResult<SessionInfo> {
        if self.access().users {
            return Err(EngineError::new(
                ErrorCode::Forbidden,
                "Администратор уже есть: войдите под ним.",
            ));
        }
        let user = self.add_user(&ostov_sec::NewUser {
            login: login.to_owned(),
            display_name: login.to_owned(),
            role: ADMIN_ROLE.to_owned(),
            password: Password::new(password),
        })?;
        Ok(self.enter(user))
    }

    pub(super) fn run_admin_users(&self) -> EngineResult<Vec<UserInfo>> {
        self.need_system(|s| s.admin, "администрирование")?;
        let users = ostov_sec::list_users(&mut **self.db()).map_err(sec_error)?;
        Ok(users
            .into_iter()
            .map(|u| UserInfo {
                login: u.login,
                display_name: u.display_name,
                role: u.role,
                active: u.active,
            })
            .collect())
    }

    pub(super) fn run_admin_add_user(&self, user: &ostov_api::NewUser) -> EngineResult<()> {
        self.need_system(|s| s.admin, "администрирование")?;
        // Иначе после первого пользователя войти можно, а управлять пользователями — никому.
        let first = !self.access().users;
        if first && !ostov_sec::system_rights(&self.config.roles, &user.role).admin {
            return Err(EngineError::new(
                ErrorCode::Validation,
                "Первым пользователем должен быть администратор: выберите роль с правом администрирования.",
            ));
        }
        self.add_user(&ostov_sec::NewUser {
            login: user.login.clone(),
            display_name: user.display_name.clone(),
            role: user.role.clone(),
            password: Password::new(user.password.as_str()),
        })?;
        Ok(())
    }

    pub(super) fn run_admin_set_user_active(&self, login: &str, active: bool) -> EngineResult<()> {
        self.need_system(|s| s.admin, "администрирование")?;
        let user = {
            let mut db = self.db();
            in_transaction(&mut **db, |db| {
                let user = ostov_sec::set_active(db, login, active, &self.config.roles)?;
                let action = if active {
                    Action::UserEnabled
                } else {
                    Action::UserDisabled
                };
                let note = format!("пользователь {}", user.login);
                self.journal(db, action, "", Some(user.id), &[], &note)?;
                Ok(user)
            })
            .map_err(error)?
        };
        let mut access = self.access();
        if !user.active && access.user.as_ref().is_some_and(|u| u.login == user.login) {
            access.user = None;
        }
        Ok(())
    }

    /// Метаданные с правами того, кто вошёл: объекты без права просмотра скрыты, и в меню тоже.
    pub(super) fn run_get_meta(&self) -> EngineResult<Meta> {
        let mut meta = self.config.meta.clone();
        let Some(role) = self.role()? else {
            return Ok(meta);
        };
        for object in &mut meta.objects {
            object.rights = ostov_sec::rights(&self.config.roles, &role, &object.name);
        }
        meta.objects.retain(|o| o.rights.read);
        let visible: Vec<String> = meta.objects.iter().map(|o| o.name.clone()).collect();
        meta.menu = prune(meta.menu, &visible);
        Ok(meta)
    }

    fn add_user(&self, user: &ostov_sec::NewUser) -> EngineResult<User> {
        let added = {
            let mut db = self.db();
            in_transaction(&mut **db, |db| {
                let added = ostov_sec::add_user(db, user, &self.config.roles)?;
                let note = format!("пользователь {}, роль {}", added.login, added.role);
                // Первого администратора записываем от его же имени: до него входить было некому.
                let by = match &self.access().user {
                    Some(u) => u.login.clone(),
                    None => added.login.clone(),
                };
                super::journal::record(db, &by, Action::UserAdded, "", Some(added.id), &[], &note)?;
                Ok(added)
            })
            .map_err(error)?
        };
        self.access().users = true;
        Ok(added)
    }

    fn enter(&self, user: User) -> SessionInfo {
        let session = SessionInfo {
            login: user.login.clone(),
            display_name: user.display_name.clone(),
            roles: vec![user.role.clone()],
            system: ostov_sec::system_rights(&self.config.roles, &user.role),
        };
        self.access().user = Some(user);
        self.forget_lists();
        session
    }
}

/// Пункты меню на скрытые объекты убираются, опустевшие группы тоже.
fn prune(items: Vec<MenuItem>, visible: &[String]) -> Vec<MenuItem> {
    items
        .into_iter()
        .filter_map(|mut item| match &item.object {
            Some(object) => visible.contains(object).then_some(item),
            None => {
                item.children = prune(item.children, visible);
                (!item.children.is_empty()).then_some(item)
            }
        })
        .collect()
}

pub(super) fn sec_error(e: SecError) -> EngineError {
    let code = match &e {
        SecError::WrongCredentials { .. } => ErrorCode::BadCredentials,
        SecError::Disabled => ErrorCode::Forbidden,
        SecError::Invalid(_) | SecError::LastAdmin => ErrorCode::Validation,
        SecError::NotFound { .. } => ErrorCode::NotFound,
        SecError::VersionConflict { .. } => ErrorCode::VersionConflict,
        SecError::Store(_) | SecError::Crypto(_) => {
            return EngineError {
                code: ErrorCode::Internal,
                message: "Ошибка при работе с пользователями. Повторите; если не поможет — сообщите администратору.".into(),
                details: e.to_string(),
            };
        }
    };
    EngineError::new(code, e.to_string())
}
