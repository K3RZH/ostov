//! Программа командной строки `ostov`: `ostov check [папка]` и `ostov stubs [папка]`.

use std::fmt::Display;
use std::path::Path;
use std::process::ExitCode;

const DEFAULT_CONFIG_DIR: &str = "config";
const USAGE: &str = "использование: ostov check|stubs [папка конфигурации, по умолчанию config]";
/// Код выхода при неверных аргументах, отличный от «в конфигурации ошибки».
const USAGE_EXIT_CODE: u8 = 2;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check"] => check(Path::new(DEFAULT_CONFIG_DIR)),
        ["check", dir] => check(Path::new(dir)),
        ["stubs"] => stubs(Path::new(DEFAULT_CONFIG_DIR)),
        ["stubs", dir] => stubs(Path::new(dir)),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(USAGE_EXIT_CODE)
        }
    }
}

/// Проверка без базы: печатает все ошибки как «файл:строка — причина».
fn check(dir: &Path) -> ExitCode {
    let config = match ostov_meta::load(dir) {
        Ok(config) => config,
        Err(errors) => return failure(&errors),
    };
    if let Err(errors) = check_scripts(dir, &config) {
        return failure(&errors);
    }
    println!(
        "{}: ошибок нет. Справочников: {}, документов: {}, регистров: {}, разделов меню: {}, ролей: {}.",
        dir.display(),
        config.catalogs.len(),
        config.documents.len(),
        config.registers.len(),
        config.menu.len(),
        config.roles.len()
    );
    ExitCode::SUCCESS
}

/// `.py` документов компилирует только сборка с Python; без неё — предупреждение.
fn check_scripts(
    dir: &Path,
    config: &ostov_model::Config,
) -> Result<(), Vec<ostov_py::PyLoadError>> {
    let has_scripts = config
        .documents
        .iter()
        .any(|d| ostov_py::script_path(dir, &d.name).is_file());
    if !has_scripts {
        return Ok(());
    }
    if !ostov_py::PYTHON_ENABLED {
        eprintln!("предупреждение: .py документов не проверены — сборка без Python");
        return Ok(());
    }
    ostov_py::PyRules::load(dir, config).map(|_| ())
}

/// Пишет `config/_stubs/<документ>.pyi` для подсказок в IDE.
fn stubs(dir: &Path) -> ExitCode {
    let config = match ostov_meta::load(dir) {
        Ok(config) => config,
        Err(errors) => return failure(&errors),
    };
    let target = dir.join(ostov_meta::STUBS_DIR);
    if let Err(e) = std::fs::create_dir_all(&target) {
        return failure(&[format!("{}: {e}", target.display())]);
    }
    for (name, text) in ostov_meta::stubs(&config) {
        let path = target.join(name);
        if let Err(e) = std::fs::write(&path, text) {
            return failure(&[format!("{}: {e}", path.display())]);
        }
        println!("{}", path.display());
    }
    ExitCode::SUCCESS
}

fn failure(errors: &[impl Display]) -> ExitCode {
    for error in errors {
        eprintln!("{error}");
    }
    eprintln!("Ошибок: {}.", errors.len());
    ExitCode::FAILURE
}
