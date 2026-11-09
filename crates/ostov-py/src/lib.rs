//! Логика конфигурации на Python (ADR-003, ADR-018): `config/documents/<документ>.py`
//! с функциями `before_write(doc, ctx)` и `on_post(doc, ctx)`. `PyRules` встаёт в ядро
//! на место встроенных правил: `LocalConnector::new(model).with_rules(Arc::new(rules))`.
//!
//! PyO3 подключается фичей `python`, по умолчанию выключенной: без неё крейт собирается
//! без установленного Python, а `PyRules::load` возвращает ошибку.

mod error;
#[cfg(feature = "python")]
mod python;

use std::path::{Path, PathBuf};

pub use error::PyLoadError;

/// Собран ли крейт с Python.
pub const PYTHON_ENABLED: bool = cfg!(feature = "python");

/// Модули документов `ostov_cfg_<документ>` — так их видит Python.
pub const MODULE_PREFIX: &str = "ostov_cfg_";

/// Файл логики документа: `config/documents/<документ>.py`.
pub fn script_path(config_dir: &Path, document: &str) -> PathBuf {
    config_dir.join("documents").join(format!("{document}.py"))
}

#[cfg(feature = "python")]
pub use python::PyRules;

/// Заглушка сборки без Python: загрузка всегда с ошибкой, правил нет.
#[cfg(not(feature = "python"))]
#[derive(Debug)]
pub struct PyRules;

#[cfg(not(feature = "python"))]
impl PyRules {
    /// # Errors
    /// Всегда: крейт собран без фичи `python`.
    pub fn load(config_dir: &Path, _model: &ostov_model::Config) -> Result<Self, Vec<PyLoadError>> {
        Err(vec![PyLoadError::new(
            config_dir,
            None,
            "сборка без Python: логика документов на Python недоступна (нужна фича python)",
        )])
    }
}

#[cfg(not(feature = "python"))]
impl ostov_core::local::Rules for PyRules {
    fn rule(
        &self,
        _document: &ostov_model::DocumentDef,
    ) -> Option<Box<dyn ostov_core::PostingRule>> {
        None
    }
}
