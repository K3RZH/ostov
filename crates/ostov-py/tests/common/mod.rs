//! Общее для тестов с Python: модель «Склада» и временная папка с `.py` документов.
//! Тесты идут только с `--features python` и `PYTHONHOME`, указывающим на сборку Python.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use ostov_core::local::Rules;
use ostov_core::{FieldValue, Fields, PostingRule};
use ostov_py::{PyLoadError, PyRules};

pub fn repo_config() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config")
}

pub fn model() -> ostov_model::Config {
    ostov_meta::load(&repo_config()).unwrap()
}

/// Временная `config/` только с `.py`; удаляется после теста.
pub struct Scripts {
    pub dir: PathBuf,
}

impl Scripts {
    pub fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ostov-py-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("documents")).unwrap();
        Self { dir }
    }

    pub fn write(&self, document: &str, text: &str) -> &Self {
        fs::write(
            self.dir.join("documents").join(format!("{document}.py")),
            text,
        )
        .unwrap();
        self
    }

    pub fn load(&self, model: &ostov_model::Config) -> Result<PyRules, Vec<PyLoadError>> {
        PyRules::load(&self.dir, model)
    }
}

impl Drop for Scripts {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub fn rule(rules: &PyRules, model: &ostov_model::Config, document: &str) -> Box<dyn PostingRule> {
    let def = model.documents.iter().find(|d| d.name == document).unwrap();
    rules.rule(def).unwrap()
}

pub fn dec(text: &str) -> FieldValue {
    FieldValue::Decimal(text.parse().unwrap())
}

pub fn fields(pairs: &[(&str, FieldValue)]) -> Fields {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

/// Расход, которому ничего не нужно: подставляется, когда тест про поступление.
pub const QUIET_ISSUE: &str = "def on_post(doc, ctx):\n    pass\n";
