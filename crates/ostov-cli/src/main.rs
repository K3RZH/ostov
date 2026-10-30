//! Программа командной строки `ostov`. Пока одна команда: `ostov check [папка]`.

use std::path::Path;
use std::process::ExitCode;

const DEFAULT_CONFIG_DIR: &str = "config";
const USAGE: &str = "использование: ostov check [папка конфигурации, по умолчанию config]";
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
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(USAGE_EXIT_CODE)
        }
    }
}

/// Проверка без базы: печатает все ошибки как «файл:строка — причина».
fn check(dir: &Path) -> ExitCode {
    match ostov_meta::load_checked(dir) {
        Ok(config) => {
            println!(
                "{}: ошибок нет. Справочников: {}, документов: {}, регистров: {}.",
                dir.display(),
                config.catalogs.len(),
                config.documents.len(),
                config.registers.len()
            );
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                eprintln!("{error}");
            }
            eprintln!("Ошибок: {}.", errors.len());
            ExitCode::FAILURE
        }
    }
}
