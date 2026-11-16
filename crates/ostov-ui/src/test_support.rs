//! Общее для тестов: папка `config/` репозитория и её временные копии, которые тест может менять.

use std::path::{Path, PathBuf};

/// Конфигурация «Склад» в файлах: та же, что встроена в `ostov_meta::demo`.
pub fn config_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config")
}

/// Пустая временная папка теста; имя с номером процесса, чтобы параллельные запуски не мешали.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ostov-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("временная папка создаётся");
    dir
}

/// Копия `config/` во временной папке: тест правит файлы, не трогая репозиторий.
pub fn config_copy(name: &str) -> PathBuf {
    let dir = temp_dir(name).join("config");
    copy_dir(&config_dir(), &dir);
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("папка копии создаётся");
    for entry in std::fs::read_dir(from).expect("папка читается").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("файл копируется");
        }
    }
}

/// Заменяет в файле копии текст `from` на `to`; текста нет: тест устарел. Концы строк
/// приводятся к `\n`: на Windows git отдаёт файлы с `\r\n`, а правки в тестах записаны с `\n`.
pub fn replace_in(file: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(file)
        .expect("файл читается")
        .replace("\r\n", "\n");
    assert!(text.contains(from), "в {} нет «{from}»", file.display());
    std::fs::write(file, text.replacen(from, to, 1)).expect("файл пишется");
}
