//! `ostov check`: каждая проверка из ТЗ даёт верные файл, строку и причину.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TempConfig, assert_error, repo_config, single};
use ostov_meta::load_checked;

/// Справочник с одним полем; поле на строке 4, его тип — на строке 6.
fn catalog(name: &str, field: &str, kind: &str) -> String {
    format!(
        "catalog: {name}\ntitle: Т\nfields:\n  - name: {field}\n    title: П\n    type: {kind}\n"
    )
}

/// Регистр: измерения и ресурсы — списки `(имя, тип)`.
fn register(dimensions: &[(&str, &str)], resources: &[(&str, &str)]) -> String {
    let list = |fields: &[(&str, &str)]| -> String {
        if fields.is_empty() {
            return " []\n".into();
        }
        fields
            .iter()
            .map(|(name, kind)| format!("\n  - name: {name}\n    title: П\n    type: {kind}"))
            .collect::<String>()
            + "\n"
    };
    format!(
        "register: stock\ntitle: Остатки\nkind: accumulation\ndimensions:{}resources:{}",
        list(dimensions),
        list(resources)
    )
}

#[test]
fn warehouse_config_passes_check() {
    load_checked(&repo_config()).unwrap();
}

#[test]
fn unknown_ref_target() {
    let config = TempConfig::new("check-ref");
    config.write(
        "catalogs/goods.yaml",
        &catalog("goods", "maker", "ref(makers)"),
    );
    assert_error(
        &single(config.check_errors()),
        "catalogs/goods.yaml",
        Some(6),
        "нет справочника или документа «makers» для ref(makers)",
    );
}

#[test]
fn ref_to_register_is_not_allowed() {
    let config = TempConfig::new("check-ref-register");
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "catalogs/notes.yaml",
            &catalog("notes", "about", "ref(stock)"),
        )
        .write(
            "registers/stock.yaml",
            &register(&[("goods", "ref(goods)")], &[("qty", "decimal(3)")]),
        );
    assert_error(
        &single(config.check_errors()),
        "catalogs/notes.yaml",
        Some(6),
        "«stock»",
    );
}

#[test]
fn document_registers_must_exist() {
    let config = TempConfig::new("check-doc-register");
    config.write(
        "documents/receipt.yaml",
        "document: receipt\ntitle: П\nregisters:\n  - stock\nfields:\n  - name: date\n    title: Д\n    type: date\n",
    );
    assert_error(
        &single(config.check_errors()),
        "documents/receipt.yaml",
        Some(4),
        "нет регистра «stock»",
    );
}

#[test]
fn duplicate_object_names_across_files() {
    let config = TempConfig::new("check-dup-object");
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "documents/goods.yaml",
            "document: goods\ntitle: Д\nfields:\n  - name: date\n    title: Д\n    type: date\n",
        );
    let error = single(config.check_errors());
    assert_error(
        &error,
        "documents/goods.yaml",
        Some(1),
        "объект «goods» повторяется",
    );
    assert!(error.reason.contains("goods.yaml:1"), "{error}");
}

#[test]
fn duplicate_field_names() {
    let config = TempConfig::new("check-dup-field");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: Т\nfields:\n  - name: name\n    title: А\n    type: text\n  - name: name\n    title: Б\n    type: text\n",
    );
    assert_error(
        &single(config.check_errors()),
        "catalogs/goods.yaml",
        Some(7),
        "поле «name» повторяется",
    );
}

#[test]
fn same_field_name_in_header_and_table_part_is_fine() {
    let config = TempConfig::new("check-scopes");
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "documents/receipt.yaml",
            "document: receipt\ntitle: П\nfields:\n  - name: amount\n    title: С\n    type: decimal(2)\n\
             tables:\n  - name: lines\n    title: Т\n    fields:\n      - name: amount\n        title: С\n        type: decimal(2)\n",
        );
    load_checked(&config.dir).unwrap();
}

#[test]
fn duplicate_table_parts() {
    let config = TempConfig::new("check-dup-part");
    let part = "  - name: lines\n    title: Т\n    fields:\n      - name: qty\n        title: К\n        type: decimal(3)\n";
    config.write(
        "documents/issue.yaml",
        &format!("document: issue\ntitle: Р\nfields:\n  - name: date\n    title: Д\n    type: date\ntables:\n{part}{part}"),
    );
    assert_error(
        &single(config.check_errors()),
        "documents/issue.yaml",
        Some(14),
        "табличная часть «lines» повторяется",
    );
}

#[test]
fn names_must_be_ascii_and_short() {
    let config = TempConfig::new("check-names");
    let long = "a".repeat(64);
    config
        .write(
            "catalogs/goods.yaml",
            &catalog("goods", "Наименование", "text"),
        )
        .write("catalogs/long.yaml", &catalog(&long, "name", "text"))
        .write("catalogs/units.yaml", &catalog("units", "Name", "text"));
    let errors = config.check_errors();
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert_error(&errors[0], "catalogs/goods.yaml", Some(4), "не ASCII");
    assert_error(&errors[1], "catalogs/long.yaml", Some(1), "длиннее 63");
    assert_error(
        &errors[2],
        "catalogs/units.yaml",
        Some(4),
        "строчные латинские",
    );
}

#[test]
fn reserved_names_are_rejected() {
    for reserved in ostov_meta::check::RESERVED_NAMES {
        let config = TempConfig::new(&format!("check-reserved-{reserved}"));
        config.write("catalogs/goods.yaml", &catalog("goods", reserved, "text"));
        assert_error(
            &single(config.check_errors()),
            "catalogs/goods.yaml",
            Some(4),
            &format!("имя «{reserved}» занято ядром"),
        );
    }
}

#[test]
fn decimal_scale_is_limited() {
    let config = TempConfig::new("check-scale");
    config.write(
        "catalogs/goods.yaml",
        &catalog("goods", "weight", "decimal(19)"),
    );
    assert_error(
        &single(config.check_errors()),
        "catalogs/goods.yaml",
        Some(6),
        "decimal(19) больше 18",
    );
}

#[test]
fn register_needs_dimensions_and_resources() {
    let config = TempConfig::new("check-register-empty");
    config.write("registers/stock.yaml", &register(&[], &[]));
    let errors = config.check_errors();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_error(&errors[0], "registers/stock.yaml", Some(1), "нет измерений");
    assert_error(&errors[1], "registers/stock.yaml", Some(1), "нет ресурсов");
}

#[test]
fn register_field_types_are_limited() {
    let config = TempConfig::new("check-register-types");
    config.write(
        "registers/stock.yaml",
        &register(&[("day", "date")], &[("note", "text")]),
    );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_error(
        &errors[0],
        "registers/stock.yaml",
        Some(7),
        "измерение «day» может быть только ref, int или text",
    );
    assert_error(
        &errors[1],
        "registers/stock.yaml",
        Some(11),
        "ресурс «note» может быть только decimal",
    );
}

#[test]
fn register_dimension_and_resource_share_names() {
    let config = TempConfig::new("check-register-dup");
    config.write(
        "registers/stock.yaml",
        &register(&[("qty", "int")], &[("qty", "decimal(3)")]),
    );
    assert_error(
        &single(config.check_errors()),
        "registers/stock.yaml",
        Some(9),
        "поле «qty» повторяется",
    );
}

#[test]
fn all_errors_are_reported_sorted_by_file_and_line() {
    let config = TempConfig::new("check-many");
    config
        .write("catalogs/goods.yaml", &catalog("goods", "id", "decimal"))
        .write(
            "catalogs/units.yaml",
            &catalog("units", "Имя", "ref(nothing)"),
        );
    // Ошибка разбора в goods.yaml: проверки не запускаются, иначе ref на его объекты дали бы ложные ошибки.
    assert_error(
        &single(config.check_errors()),
        "catalogs/goods.yaml",
        Some(6),
        "не указана точность",
    );

    config.write("catalogs/goods.yaml", &catalog("goods", "id", "decimal(2)"));
    let errors = config.check_errors();
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert_error(&errors[0], "catalogs/goods.yaml", Some(4), "занято ядром");
    assert_error(&errors[1], "catalogs/units.yaml", Some(4), "не ASCII");
    assert_error(&errors[2], "catalogs/units.yaml", Some(6), "«nothing»");
}
