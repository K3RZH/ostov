//! Конфигурация «Склад» из `config/`, встроенная в программу: для демо-базы, где папки рядом нет.

use std::path::PathBuf;

use crate::Error;
use crate::load::{
    CATALOGS_DIR, DOCUMENTS_DIR, MANIFEST_FILE, REGISTERS_DIR, SUBSYSTEMS_DIR, Source, Sources,
    checked, parse_sources,
};

/// Встроенные файлы: (путь внутри `config/`, текст). Тест сверяет их с папкой на диске.
const MANIFEST: &str = include_str!("../../../config/ostov.yaml");
const CATALOGS: [(&str, &str); 3] = [
    (
        "goods.yaml",
        include_str!("../../../config/catalogs/goods.yaml"),
    ),
    (
        "suppliers.yaml",
        include_str!("../../../config/catalogs/suppliers.yaml"),
    ),
    (
        "warehouses.yaml",
        include_str!("../../../config/catalogs/warehouses.yaml"),
    ),
];
const DOCUMENTS: [(&str, &str); 2] = [
    (
        "issue.yaml",
        include_str!("../../../config/documents/issue.yaml"),
    ),
    (
        "receipt.yaml",
        include_str!("../../../config/documents/receipt.yaml"),
    ),
];
const REGISTERS: [(&str, &str); 1] = [(
    "stock.yaml",
    include_str!("../../../config/registers/stock.yaml"),
)];
const SUBSYSTEMS: [(&str, &str); 1] = [(
    "warehouse.yaml",
    include_str!("../../../config/subsystems/warehouse.yaml"),
)];

/// Модель встроенной конфигурации «Склад».
///
/// # Errors
/// Только если встроенные файлы испорчены; тест `demo_matches_config_folder` это ловит до сборки.
pub fn demo() -> Result<ostov_model::Config, Vec<Error>> {
    let embedded = |dir: &str, files: &[(&str, &'static str)]| -> Vec<Source> {
        files
            .iter()
            .map(|(name, text)| Source {
                path: PathBuf::from("config").join(dir).join(name),
                text: Some(text),
            })
            .collect()
    };
    let sources = Sources {
        manifest: Source {
            path: PathBuf::from("config").join(MANIFEST_FILE),
            text: Some(MANIFEST),
        },
        catalogs: embedded(CATALOGS_DIR, &CATALOGS),
        documents: embedded(DOCUMENTS_DIR, &DOCUMENTS),
        registers: embedded(REGISTERS_DIR, &REGISTERS),
        subsystems: embedded(SUBSYSTEMS_DIR, &SUBSYSTEMS),
    };
    checked(parse_sources(&sources, Vec::new())?).map(crate::convert::into_model)
}
