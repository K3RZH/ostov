//! Общее для тестов: временная папка конфигурации и проверка «файл, строка, причина».

// Каждый тестовый файл берёт свою часть помощников.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use ostov_meta::{Error, load, load_checked};

pub const MANIFEST: &str = "name: test\ntitle: Тест\nversion: 0.1.0\nformat: 1\n";

/// Временная папка конфигурации; удаляется после теста.
pub struct TempConfig {
    pub dir: PathBuf,
}

impl TempConfig {
    pub fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ostov-meta-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let config = Self { dir };
        config.write("ostov.yaml", MANIFEST);
        config
    }

    pub fn write(&self, path: &str, text: &str) -> &Self {
        let path = self.dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        self
    }

    /// Ошибки разбора.
    pub fn errors(&self) -> Vec<Error> {
        load(&self.dir).unwrap_err()
    }

    /// Ошибки разбора или проверки — то, что покажет `ostov check`.
    pub fn check_errors(&self) -> Vec<Error> {
        load_checked(&self.dir).unwrap_err()
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub fn single(errors: Vec<Error>) -> Error {
    assert_eq!(errors.len(), 1, "ожидалась одна ошибка: {errors:#?}");
    errors.into_iter().next().unwrap()
}

pub fn assert_error(error: &Error, file: &str, line: Option<u64>, reason_part: &str) {
    assert!(error.file.ends_with(file), "файл: {error}");
    assert_eq!(error.line, line, "строка: {error}");
    assert!(error.reason.contains(reason_part), "причина: {error}");
}

pub fn repo_config() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config")
}
