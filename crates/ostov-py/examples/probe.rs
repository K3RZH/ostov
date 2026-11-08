//! Проба ADR-018: встроенный интерпретатор читает `.py` из файла и вызывает из него функцию.
//! `cargo run -p ostov-py --features python --example probe -- [файл.py]`

use std::ffi::CString;
use std::process::ExitCode;

use pyo3::prelude::*;
use pyo3::types::PyModule;

const DEFAULT_SCRIPT: &str = "probe.py";

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_SCRIPT.to_owned());
    match run(&path) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("проба не удалась: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: &str) -> Result<String, String> {
    let code = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let code = CString::new(code).map_err(|_| format!("{path}: в файле нулевой байт"))?;
    let file = CString::new(path).map_err(|_| "в пути нулевой байт".to_owned())?;
    Python::initialize();
    Python::attach(|py| {
        let module = PyModule::from_code(py, &code, &file, c"ostov_cfg_probe")?;
        let amount: String = module
            .getattr("amount")?
            .call1(("2.500", "3.335"))?
            .extract()?;
        let place: String = module.getattr("where_am_i")?.call0()?.extract()?;
        Ok::<_, PyErr>(format!("amount(2.500, 3.335) = {amount}\n{place}"))
    })
    .map_err(|e| e.to_string())
}
