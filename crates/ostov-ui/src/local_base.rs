//! Открытие локальной базы (режим 1) через ядро. Пока одна демо-база «Кафе-2026»: файл
//! `~/.local/share/ostov/demo-cafe-2026.sqlite`, при первом запуске создаётся с примерными
//! данными (`LocalConnector::create_demo`). Дальше база выбирается на стартовой странице.

use std::path::PathBuf;
use std::sync::Arc;

use ostov_api::{Connector, Engine, EngineError, EngineResult, ErrorCode};
use ostov_core::LocalConnector;

/// Сколько поступлений в демо-базе (как в прежних примерных данных).
const DEMO_RECEIPTS: usize = 2000;

/// Файл демо-базы: `$XDG_DATA_HOME/ostov/` или `~/.local/share/ostov/`.
fn demo_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(base.join("ostov").join("demo-cafe-2026.sqlite"))
}

/// Открывает демо-базу, а если файла нет, создаёт её (создание занимает время: один раз).
pub fn open_demo() -> EngineResult<Arc<dyn Engine>> {
    let unavailable = |what: &str| EngineError::new(ErrorCode::Unavailable, what.to_owned());
    let path = demo_path().ok_or_else(|| unavailable("Не найден каталог данных пользователя."))?;
    let text = path
        .to_str()
        .ok_or_else(|| unavailable("Путь к базе не в UTF-8."))?;
    if path.exists() {
        return LocalConnector.open_local(text, None);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| unavailable(&format!("Не удалось создать каталог базы: {e}.")))?;
    }
    LocalConnector.create_demo(text, DEMO_RECEIPTS)
}
