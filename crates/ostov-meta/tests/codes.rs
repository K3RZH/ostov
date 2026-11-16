//! Постоянные коды `ostov check`: список в README, коды у ошибок, позиции элементов.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;

use common::{TempConfig, repo_config};
use ostov_meta::error::codes;
use ostov_meta::{load, load_with_warnings};

#[test]
fn codes_are_unique_and_listed_in_readme() {
    let unique: BTreeSet<&str> = codes::ALL.iter().copied().collect();
    assert_eq!(unique.len(), codes::ALL.len(), "коды повторяются");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = std::fs::read_to_string(path).unwrap();
    let listed: BTreeSet<&str> = readme
        .lines()
        .filter_map(|line| line.strip_prefix("| "))
        .filter_map(|line| line.split(' ').next())
        .filter(|code| code.len() == 4 && (code.starts_with('E') || code.starts_with('W')))
        .collect();
    assert_eq!(listed, unique, "README и codes::ALL расходятся");
}

#[test]
fn errors_carry_their_codes() {
    let config = TempConfig::new("codes");
    config
        .write(
            "catalogs/goods.yaml",
            "catalog: goods\ntitle: Т\nfields:\n  - name: id\n    title: П\n    type: ref(nowhere)\n",
        )
        .write(
            "registers/stock.yaml",
            "register: stock\ntitle: О\nkind: accumulation\ndimensions: []\nresources: []\n",
        );
    let errors = config.check_errors();
    let got: BTreeSet<&str> = errors.iter().map(|e| e.code).collect();
    for code in [
        codes::RESERVED_NAME,
        codes::REF_UNKNOWN,
        codes::REGISTER_NO_DIMENSIONS,
        codes::REGISTER_NO_RESOURCES,
    ] {
        assert!(got.contains(code), "нет {code}: {errors:#?}");
    }
    assert!(errors.iter().all(|e| !e.is_warning()));
    let shown = errors[0].to_string();
    assert!(
        shown.contains(".yaml:") && shown.contains(": E0"),
        "{shown}"
    );

    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: Т\ncolour: red\nfields: []\n",
    );
    let errors = config.errors();
    assert_eq!(errors[0].code, codes::UNKNOWN_KEY, "{errors:#?}");
}

#[test]
fn warning_has_w_code() {
    let config = TempConfig::new("codes-warning");
    config
        .write("catalogs/goods.yaml", "catalog: goods\ntitle: Т\nfields: []\n")
        .write(
            "documents/issue.yaml",
            "document: issue\ntitle: Р\nfields:\n  - name: goods\n    title: Т\n    type: ref(goods)\n    required: true\n",
        )
        .write(
            "roles/clerk.yaml",
            "role: clerk\ntitle: К\nobjects:\n  issue: [read, create]\n",
        );
    let (_, warnings) = load_with_warnings(&config.dir).unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(warnings[0].code, codes::READ_FOR_REQUIRED);
    assert!(warnings[0].is_warning());
    assert!(
        warnings[0].to_string().contains(": W001 роль «clerk»"),
        "{}",
        warnings[0]
    );
}

#[test]
fn positions_point_to_files_and_lines() {
    let model = load(&repo_config()).unwrap();
    let at = |key: &str| {
        let p = model
            .positions
            .get(key)
            .unwrap_or_else(|| panic!("нет {key}"));
        (p.file.as_str(), p.line, p.column)
    };
    // `name: warehouse` во 2-й строке манифеста, значение с 7-го столбца.
    assert_eq!(at("config"), ("ostov.yaml", 2, 7));
    assert_eq!(at("catalog:goods"), ("catalogs/goods.yaml", 1, 10));
    assert_eq!(at("catalog:goods/field:name").0, "catalogs/goods.yaml");
    assert_eq!(
        at("document:receipt/field:date"),
        ("documents/receipt.yaml", 7, 11)
    );
    let (file, line, _) = at("document:receipt/table:lines/field:qty");
    assert_eq!(file, "documents/receipt.yaml");
    assert!(line > 7);
    assert_eq!(at("register:stock").0, "registers/stock.yaml");
    assert_eq!(at("register:stock/field:qty").0, "registers/stock.yaml");
    assert_eq!(at("role:storekeeper").0, "roles/storekeeper.yaml");
    assert_eq!(at("subsystem:warehouse").0, "subsystems/warehouse.yaml");
    assert_eq!(at("user_field:warehouses").0, "ostov.yaml");
    // Каждое поле каждого объекта есть в карте.
    for catalog in &model.catalogs {
        for field in &catalog.fields {
            at(&format!("catalog:{}/field:{}", catalog.name, field.name));
        }
    }
    for document in &model.documents {
        for field in &document.fields {
            at(&format!("document:{}/field:{}", document.name, field.name));
        }
        for part in &document.table_parts {
            for field in &part.fields {
                at(&format!(
                    "document:{}/table:{}/field:{}",
                    document.name, part.name, field.name
                ));
            }
        }
    }
    // У demo() пути те же, относительно встроенной папки.
    assert_eq!(ostov_meta::demo().unwrap().positions, model.positions);
}
