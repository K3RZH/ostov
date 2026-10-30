//! Пользователи базы: список, добавление, отключение. Без интерфейса и без ядра:
//! настоящий справочник пользователей и роли придут из ядра и конфигурации (СТ-30, СТ-36).

/// Роль администратора: с ней можно управлять пользователями.
pub const ADMIN_ROLE: &str = "Администратор";

/// Роли примерные: их задаёт конфигурация, не платформа.
pub const ROLES: [&str; 5] = [
    ADMIN_ROLE,
    "Кладовщик",
    "Повар/технолог",
    "Бухгалтер",
    "Руководитель",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub login: String,
    pub name: String,
    pub role: String,
    pub active: bool,
}

#[derive(Debug, Default)]
pub struct Directory {
    users: Vec<User>,
}

fn user(login: &str, name: &str, role: &str, active: bool) -> User {
    User {
        login: login.into(),
        name: name.into(),
        role: role.into(),
        active,
    }
}

impl Directory {
    /// Примерные пользователи для макета.
    pub fn demo() -> Self {
        Self {
            users: vec![
                user("admin", "Артём К.", ADMIN_ROLE, true),
                user("kladovshik", "Иван Петров", "Кладовщик", true),
                user("povar", "Мария Соколова", "Повар/технолог", true),
                user("buh", "Ольга Н.", "Бухгалтер", true),
                user("old", "Сергей В.", "Кладовщик", false),
            ],
        }
    }

    pub fn users(&self) -> &[User] {
        &self.users
    }

    fn active_admins(&self) -> usize {
        self.users
            .iter()
            .filter(|u| u.active && u.role == ADMIN_ROLE)
            .count()
    }

    /// Добавляет пользователя. Логин не пустой и не повторяется (без учёта регистра).
    pub fn add(&mut self, login: &str, name: &str, role: &str) -> Result<(), &'static str> {
        let login = login.trim();
        if login.is_empty() {
            return Err("Введите логин.");
        }
        if self
            .users
            .iter()
            .any(|u| u.login.to_lowercase() == login.to_lowercase())
        {
            return Err("Пользователь с таким логином уже есть.");
        }
        let name = name.trim();
        self.users.push(user(
            login,
            if name.is_empty() { login } else { name },
            role,
            true,
        ));
        Ok(())
    }

    /// Включает или отключает пользователя. Последнего активного администратора отключить
    /// нельзя: иначе некому будет управлять базой.
    pub fn toggle(&mut self, index: usize) -> Result<(), &'static str> {
        let admins = self.active_admins();
        let Some(user) = self.users.get_mut(index) else {
            return Ok(());
        };
        if user.active && user.role == ADMIN_ROLE && admins <= 1 {
            return Err("Нельзя отключить последнего администратора.");
        }
        user.active = !user.active;
        Ok(())
    }
}
