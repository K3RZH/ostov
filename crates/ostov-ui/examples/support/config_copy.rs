//! Копия `config/` репозитория для стендов: стенд меняет файлы, не трогая репозиторий.

use std::path::{Path, PathBuf};

/// Копирует `config/` в `dir` (папка создаётся) и возвращает её.
pub fn config_copy(dir: &Path) -> std::io::Result<PathBuf> {
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config"),
        dir,
    )?;
    Ok(dir.to_path_buf())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        // Тестовую базу из папки разработчика не тащим: у копии своя.
        if entry.file_name() == ".ostov-test" {
            continue;
        }
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Заменяет в файле первое вхождение `from` на `to`; текста нет: ошибка.
pub fn replace_in(file: &Path, from: &str, to: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(file)?;
    if !text.contains(from) {
        return Err(std::io::Error::other(format!(
            "в {} нет «{from}»",
            file.display()
        )));
    }
    std::fs::write(file, text.replacen(from, to, 1))
}
