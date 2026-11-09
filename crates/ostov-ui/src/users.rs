//! Пользователи базы для экрана «Пользователи и роли». Список приходит из ядра
//! (`admin_users`); проверки (логин, пароль, роль, последний администратор) тоже в ядре.

use ostov_api::{RoleInfo, UserInfo};

#[derive(Debug, Default)]
pub struct Directory {
    users: Vec<UserInfo>,
    /// Роли базы: в `UserInfo.role` имя роли, людям показываем заголовок.
    roles: Vec<RoleInfo>,
}

impl Directory {
    pub fn new(users: Vec<UserInfo>, roles: Vec<RoleInfo>) -> Self {
        Self { users, roles }
    }

    /// Примерные пользователи: только для стендов (`examples/snapshot.rs`).
    pub fn demo() -> Self {
        let user = |login: &str, name: &str, role: &str, active: bool| UserInfo {
            login: login.into(),
            display_name: name.into(),
            role: role.into(),
            active,
        };
        let role = |name: &str, title: &str| RoleInfo {
            name: name.into(),
            title: title.into(),
        };
        Self::new(
            vec![
                user("admin", "Артём К.", "admin", true),
                user("kladovshik", "Иван Петров", "storekeeper", true),
                user("old", "Сергей В.", "storekeeper", false),
            ],
            vec![
                role("storekeeper", "Кладовщик"),
                role("admin", "Администратор"),
            ],
        )
    }

    pub fn users(&self) -> &[UserInfo] {
        &self.users
    }

    pub fn get(&self, index: usize) -> Option<&UserInfo> {
        self.users.get(index)
    }

    /// Заголовок роли по имени; незнакомая роль (удалена из конфигурации) показывается именем.
    pub fn role_title<'a>(&'a self, name: &'a str) -> &'a str {
        self.roles
            .iter()
            .find(|role| role.name == name)
            .map_or(name, |role| role.title.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_shown_by_title() {
        let directory = Directory::demo();
        assert_eq!(directory.role_title("storekeeper"), "Кладовщик");
        assert_eq!(directory.role_title("admin"), "Администратор");
        assert_eq!(directory.role_title("удалённая"), "удалённая");
    }
}
