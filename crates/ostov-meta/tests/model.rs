//! Модель из `config/` совпадает с конфигурацией «Склад» из `ostov-core/src/local/config.rs`,
//! а встроенная `demo()` — с папкой на диске.

#![allow(clippy::unwrap_used)]

mod common;

use common::repo_config;
use ostov_core::local::warehouse;
use ostov_meta::{demo, load};
use ostov_model::{FieldDef, FieldKind, RegisterKind};

/// Поле без заголовка: в config.rs заголовков нет, сравниваем имя, тип и цель ссылки.
fn shape(fields: &[FieldDef]) -> Vec<(String, FieldKind, Option<String>)> {
    fields
        .iter()
        .map(|f| (f.name.clone(), f.kind, f.target.clone()))
        .collect()
}

#[test]
fn yaml_model_matches_config_rs() {
    let model = load(&repo_config()).unwrap();
    let code = warehouse().unwrap();

    assert_eq!(model.catalogs.len(), code.catalogs.len());
    for catalog in &model.catalogs {
        let expected = &code.catalogs[&catalog.name];
        assert_eq!(
            shape(&catalog.fields),
            shape(expected.fields()),
            "справочник {}",
            catalog.name
        );
    }

    assert_eq!(model.documents.len(), code.documents.len());
    for document in &model.documents {
        let expected = &code.documents[&document.name].kind;
        assert_eq!(
            shape(&document.fields),
            shape(expected.header()),
            "документ {}",
            document.name
        );
        assert_eq!(document.table_parts.len(), expected.parts().len());
        for (part, expected) in document.table_parts.iter().zip(expected.parts()) {
            assert_eq!(part.name, expected.name());
            assert_eq!(
                shape(&part.fields),
                shape(expected.fields()),
                "{}.{}",
                document.name,
                part.name
            );
        }
        assert_eq!(document.registers, vec!["stock".to_owned()]);
    }

    let [stock] = model.registers.as_slice() else {
        panic!("ожидался один регистр: {:?}", model.registers);
    };
    assert_eq!(stock.name, code.stock.name());
    assert_eq!(stock.kind, RegisterKind::Accumulation);
    assert_eq!(shape(&stock.dimensions), shape(code.stock.dims()));
    // Ресурсы ядро наружу не отдаёт; в config.rs это qty с 3 знаками.
    assert_eq!(
        shape(&stock.resources),
        vec![("qty".to_owned(), FieldKind::Decimal { scale: 3 }, None)]
    );
}

#[test]
fn titles_come_from_yaml() {
    let model = load(&repo_config()).unwrap();
    assert_eq!(model.title, "Склад");
    let receipt = model
        .documents
        .iter()
        .find(|d| d.name == "receipt")
        .unwrap();
    assert_eq!(receipt.title, "Поступление");
    assert_eq!(receipt.fields[1].title, "Поставщик");
    assert_eq!(receipt.table_parts[0].title, "Товары");
}

#[test]
fn demo_matches_config_folder() {
    assert_eq!(demo().unwrap(), load(&repo_config()).unwrap());
}
