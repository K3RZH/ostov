//! `ostov check` как программа: коды выхода и вывод «файл:строка — причина».

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn ostov(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ostov"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn warehouse_config_has_no_errors() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let output = ostov(&["check", config.to_str().unwrap()]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("ошибок нет"), "{stdout}");
}

#[test]
fn errors_are_printed_as_file_line_reason() {
    let dir = std::env::temp_dir().join(format!("ostov-cli-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("catalogs")).unwrap();
    fs::write(
        dir.join("ostov.yaml"),
        "name: t\ntitle: Т\nversion: 0.1.0\nformat: 1\n",
    )
    .unwrap();
    fs::write(
        dir.join("catalogs/goods.yaml"),
        "catalog: goods\ntitle: Т\nfields:\n  - name: id\n    title: П\n    type: ref(makers)\n",
    )
    .unwrap();

    let output = ostov(&["check", dir.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 3, "{stderr}");
    assert!(
        lines[0].ends_with("goods.yaml:4 — имя «id» занято ядром"),
        "{stderr}"
    );
    assert!(
        lines[1].ends_with("goods.yaml:6 — нет справочника или документа «makers» для ref(makers)"),
        "{stderr}"
    );
    assert_eq!(lines[2], "Ошибок: 2.");
}

#[test]
fn wrong_arguments_print_usage() {
    let output = ostov(&["chek"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("использование")
    );
}
