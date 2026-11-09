//! Права по роли (СТ-31). Встроенная `admin` может всё; объекта нет в роли — прав нет.

use ostov_model::{ADMIN_ROLE, Rights, RoleDef, SystemRights};

/// Права роли `role` на справочник или документ `object`.
pub fn rights(roles: &[RoleDef], role: &str, object: &str) -> Rights {
    if role == ADMIN_ROLE {
        return Rights::ALL;
    }
    find(roles, role)
        .and_then(|role| role.objects.iter().find(|(name, _)| name == object))
        .map(|(_, rights)| *rights)
        .unwrap_or_default()
}

pub fn system_rights(roles: &[RoleDef], role: &str) -> SystemRights {
    if role == ADMIN_ROLE {
        return SystemRights::ALL;
    }
    find(roles, role)
        .map(|role| role.system)
        .unwrap_or_default()
}

pub(crate) fn role_exists(roles: &[RoleDef], role: &str) -> bool {
    role == ADMIN_ROLE || find(roles, role).is_some()
}

fn find<'a>(roles: &'a [RoleDef], name: &str) -> Option<&'a RoleDef> {
    roles.iter().find(|role| role.name == name)
}
