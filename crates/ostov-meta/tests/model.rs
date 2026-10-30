//! Модель из `config/` совпадает с конфигурацией «Склад» из `ostov-core/src/local/config.rs`,
//! а встроенная `demo()` — с папкой на диске.

#![allow(clippy::unwrap_used)]

mod common;

use common::repo_config;
use ostov_core::local::warehouse;
use ostov_meta::{demo, load};
use ostov_model::{FieldDef, FieldKind, Meta, ObjectKind, RegisterKind, Rights};

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

#[test]
fn get_meta_for_warehouse() {
    let meta = Meta::new(&load(&repo_config()).unwrap());
    assert!(meta.roles.is_empty(), "роли заполнит ostov-sec");

    let [section] = meta.menu.as_slice() else {
        panic!("ожидался один раздел меню: {:?}", meta.menu);
    };
    assert_eq!(
        (
            section.title.as_str(),
            section.icon.as_deref(),
            section.object.as_deref()
        ),
        ("Склад", Some("warehouse"), None)
    );
    let items: Vec<(&str, Option<&str>, Option<&str>)> = section
        .children
        .iter()
        .map(|i| (i.title.as_str(), i.icon.as_deref(), i.object.as_deref()))
        .collect();
    assert_eq!(
        items,
        vec![
            ("Товары", Some("box"), Some("goods")),
            ("Склады", Some("warehouse"), Some("warehouses")),
            ("Поставщики", Some("users"), Some("suppliers")),
            ("Поступление", Some("stock-in"), Some("receipt")),
            ("Расход", Some("stock-out"), Some("issue")),
        ]
    );

    assert_eq!(meta.objects.len(), 5, "регистры в GetMeta не попадают");
    let receipt = meta.objects.iter().find(|o| o.name == "receipt").unwrap();
    assert_eq!(receipt.kind, ObjectKind::Document);
    assert_eq!(receipt.rights, Rights::ALL, "пока нет ostov-sec");
    let supplier = &receipt.fields[1];
    assert_eq!(
        (
            supplier.name.as_str(),
            supplier.title.as_str(),
            supplier.kind,
            supplier.target.as_deref()
        ),
        ("supplier", "Поставщик", FieldKind::Ref, Some("suppliers"))
    );
    let amount = &receipt.fields[3];
    assert_eq!(
        (amount.name.as_str(), amount.read_only, amount.required),
        ("amount", true, false)
    );
    let lines = &receipt.table_parts[0];
    assert_eq!(
        (lines.name.as_str(), lines.title.as_str()),
        ("lines", "Товары")
    );
    assert_eq!(lines.fields[1].kind, FieldKind::Decimal { scale: 3 });
    assert!(lines.fields[3].read_only, "сумму строки считает ядро");

    let goods = meta.objects.iter().find(|o| o.name == "goods").unwrap();
    assert_eq!(goods.kind, ObjectKind::Catalog);
    assert!(goods.table_parts.is_empty());
}
