//! Логика входа без ядра: растущая задержка после неудач и временная проверка данных.
//! Настоящую проверку пароля сделает ядро (Engine API, СТ-32), эти функции его заменят.

use crate::app::LoginKind;

/// Сколько неудач подряд допускается без задержки.
const FREE_ATTEMPTS: u32 = 5;
/// Потолок задержки, секунд.
const MAX_DELAY: u32 = 60;

/// Задержка перед следующей попыткой: после 5 неудач растёт вдвое (4, 8, 16, 32, 60 с),
/// учётную запись не блокируем (СТ-32).
pub fn delay_secs(failures: u32) -> u32 {
    if failures < FREE_ATTEMPTS {
        return 0;
    }
    let steps = (failures - FREE_ATTEMPTS + 2).min(6);
    (1u32 << steps).min(MAX_DELAY)
}

/// Причина отказа во входе. `counts`: считать ли это неудачей для задержки
/// (несовпавшие пароли при создании администратора неудачей не считаются).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub message: &'static str,
    pub counts: bool,
}

/// Временная проверка: пустые данные отклоняются, остальное пропускается.
/// Так весь путь (ошибка, задержка, вход) виден без ядра.
pub fn verify(kind: LoginKind, login: &str, password: &str, repeat: &str) -> Result<(), Rejection> {
    match kind {
        LoginKind::ServerFirstRun => {
            if login.trim().is_empty() || password.is_empty() {
                Err(Rejection {
                    message: "Введите логин и пароль администратора.",
                    counts: false,
                })
            } else if password != repeat {
                Err(Rejection {
                    message: "Пароли не совпадают.",
                    counts: false,
                })
            } else {
                Ok(())
            }
        }
        LoginKind::Server | LoginKind::LocalUser => {
            if login.trim().is_empty() || password.is_empty() {
                Err(Rejection {
                    message: "Неверный логин или пароль.",
                    counts: true,
                })
            } else {
                Ok(())
            }
        }
        LoginKind::LocalPassword => {
            if password.is_empty() {
                Err(Rejection {
                    message: "Неверный пароль.",
                    counts: true,
                })
            } else {
                Ok(())
            }
        }
        LoginKind::None => Ok(()),
    }
}
