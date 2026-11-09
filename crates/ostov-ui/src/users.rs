//! Пользователи базы для экрана «Пользователи и роли». Список приходит из ядра
//! (`admin_users`); проверки (логин, пароль, роль, последний администратор) тоже в ядре.

use ostov_api::UserInfo;

#[derive(Debug, Default)]
pub struct Directory {
    users: Vec<UserInfo>,
}

impl Directory {
    pub fn new(users: Vec<UserInfo>) -> Self {
        Self { users }
    }

    /// Примерные пользователи: только для стендов (`examples/snapshot.rs`).
    pub fn demo() -> Self {
        let user = |login: &str, name: &str, role: &str, active: bool| UserInfo {
            login: login.into(),
            display_name: name.into(),
            role: role.into(),
            active,
        };
        Self::new(vec![
            user("admin", "Артём К.", "admin", true),
            user("kladovshik", "Иван Петров", "storekeeper", true),
            user("old", "Сергей В.", "storekeeper", false),
        ])
    }

    pub fn users(&self) -> &[UserInfo] {
        &self.users
    }

    pub fn get(&self, index: usize) -> Option<&UserInfo> {
        self.users.get(index)
    }
}
