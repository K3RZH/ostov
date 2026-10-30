//! `ostov check`: каждая проверка из ТЗ даёт верные файл, строку и причину.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TempConfig, assert_error, repo_config, single};
use ostov_meta::load;

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
    load(&repo_config()).unwrap();
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
    load(&config.dir).unwrap();
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

/// Раздел меню; пункты начинаются со строки 4.
fn section(name: &str, objects: &[&str]) -> String {
    let items: String = objects.iter().map(|o| format!("\n  - {o}")).collect();
    let items = if items.is_empty() {
        " []".to_owned()
    } else {
        items
    };
    format!("subsystem: {name}\ntitle: Раздел\nobjects:{items}\n")
}

/// Манифест со списком `menu:`; пункты списка начинаются со строки 6.
fn with_menu(config: &TempConfig, sections: &[&str]) {
    let list: String = sections.iter().map(|s| format!("\n  - {s}")).collect();
    config.write("ostov.yaml", &format!("{}menu:{list}\n", common::MANIFEST));
}

#[test]
fn menu_objects_must_exist() {
    let config = TempConfig::new("check-menu-missing");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "subsystems/warehouse.yaml",
            &section("warehouse", &["goods", "receipt"]),
        );
    assert_error(
        &single(config.check_errors()),
        "subsystems/warehouse.yaml",
        Some(5),
        "нет справочника или документа «receipt»",
    );
}

#[test]
fn registers_are_not_menu_items() {
    let config = TempConfig::new("check-menu-register");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "registers/stock.yaml",
            &register(&[("goods", "ref(goods)")], &[("qty", "decimal(3)")]),
        )
        .write(
            "subsystems/warehouse.yaml",
            &section("warehouse", &["stock"]),
        );
    assert_error(
        &single(config.check_errors()),
        "subsystems/warehouse.yaml",
        Some(4),
        "регистр «stock» нельзя открыть из меню",
    );
}

#[test]
fn menu_item_repeats_in_section() {
    let config = TempConfig::new("check-menu-dup-item");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "subsystems/warehouse.yaml",
            &section("warehouse", &["goods", "goods"]),
        );
    assert_error(
        &single(config.check_errors()),
        "subsystems/warehouse.yaml",
        Some(5),
        "пункт меню «goods» повторяется",
    );
}

#[test]
fn same_object_in_two_sections_is_fine() {
    let config = TempConfig::new("check-menu-two-sections");
    with_menu(&config, &["warehouse", "sales"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write("subsystems/sales.yaml", &section("sales", &["goods"]))
        .write(
            "subsystems/warehouse.yaml",
            &section("warehouse", &["goods"]),
        );
    let model = load(&config.dir).unwrap();
    let titles: Vec<_> = model.menu.iter().map(|s| s.children.len()).collect();
    assert_eq!(titles, vec![1, 1]);
}

#[test]
fn menu_sections_are_unique_and_not_empty() {
    let config = TempConfig::new("check-menu-sections");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write("subsystems/a.yaml", &section("warehouse", &["goods"]))
        .write("subsystems/b.yaml", &section("warehouse", &[]));
    let errors = config.check_errors();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_error(
        &errors[0],
        "subsystems/b.yaml",
        Some(1),
        "раздел меню «warehouse» повторяется",
    );
    assert_error(
        &errors[1],
        "subsystems/b.yaml",
        Some(1),
        "в разделе «warehouse» нет объектов",
    );
}

#[test]
fn every_section_is_listed_in_manifest_menu() {
    let config = TempConfig::new("check-menu-order");
    with_menu(&config, &["warehouse", "nothing", "warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write("subsystems/sales.yaml", &section("sales", &["goods"]))
        .write(
            "subsystems/warehouse.yaml",
            &section("warehouse", &["goods"]),
        );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert_error(
        &errors[0],
        "ostov.yaml",
        Some(7),
        "нет раздела меню «nothing»",
    );
    assert_error(
        &errors[1],
        "ostov.yaml",
        Some(8),
        "раздел в menu «warehouse» повторяется",
    );
    assert_error(
        &errors[2],
        "subsystems/sales.yaml",
        Some(1),
        "раздела «sales» нет в списке menu",
    );
}

#[test]
fn menu_order_comes_from_manifest() {
    let config = TempConfig::new("check-menu-sorted");
    with_menu(&config, &["warehouse", "sales"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write("subsystems/sales.yaml", &section("sales", &["goods"]))
        .write(
            "subsystems/warehouse.yaml",
            "subsystem: warehouse\ntitle: Склад\nobjects:\n  - goods\n",
        );
    let model = load(&config.dir).unwrap();
    let titles: Vec<&str> = model.menu.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["Склад", "Раздел"]);
}

/// Раздел «Склад»: объект, группа «Справочники» со значком и документ после неё.
const SECTION_WITH_GROUP: &str = "subsystem: warehouse
title: Склад
icon: warehouse
objects:
  - receipt
  - title: Справочники
    icon: folder
    objects:
      - goods
      - units
";

#[test]
fn one_level_of_groups_builds_a_tree() {
    let config = TempConfig::new("check-menu-group");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write("catalogs/units.yaml", &catalog("units", "name", "text"))
        .write(
            "documents/receipt.yaml",
            "document: receipt\ntitle: Поступление\nicon: stock-in\nfields:\n  - name: date\n    title: Д\n    type: date\n",
        )
        .write("subsystems/warehouse.yaml", SECTION_WITH_GROUP);
    let model = load(&config.dir).unwrap();
    let [section] = model.menu.as_slice() else {
        panic!("{:?}", model.menu);
    };
    assert_eq!(
        (section.title.as_str(), section.icon.as_deref()),
        ("Склад", Some("warehouse"))
    );
    let [receipt, group] = section.children.as_slice() else {
        panic!("{:?}", section.children);
    };
    assert_eq!(receipt.object.as_deref(), Some("receipt"));
    assert_eq!(
        receipt.icon.as_deref(),
        Some("stock-in"),
        "значок пункта — значок объекта"
    );
    assert_eq!(
        (
            group.title.as_str(),
            group.icon.as_deref(),
            group.object.as_deref()
        ),
        ("Справочники", Some("folder"), None)
    );
    let objects: Vec<_> = group.children.iter().map(|i| i.object.as_deref()).collect();
    assert_eq!(objects, vec![Some("goods"), Some("units")]);
}

#[test]
fn group_inside_group_is_rejected() {
    let config = TempConfig::new("check-menu-nested");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "subsystems/warehouse.yaml",
            "subsystem: warehouse\ntitle: Склад\nobjects:\n  - title: Внешняя\n    objects:\n      - title: Внутренняя\n        objects:\n          - goods\n",
        );
    assert_error(
        &single(config.check_errors()),
        "subsystems/warehouse.yaml",
        Some(6),
        "группа «Внутренняя» внутри группы",
    );
}

#[test]
fn group_problems_point_to_the_group() {
    let config = TempConfig::new("check-menu-group-errors");
    with_menu(&config, &["warehouse"]);
    config
        .write("catalogs/goods.yaml", &catalog("goods", "name", "text"))
        .write(
            "subsystems/warehouse.yaml",
            "subsystem: warehouse\ntitle: Склад\nobjects:\n  - goods\n  - title: Пустая\n    objects: []\n  - title: Ещё\n    objects:\n      - goods\n",
        );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_error(
        &errors[0],
        "subsystems/warehouse.yaml",
        Some(5),
        "в группе «Пустая» нет объектов",
    );
    assert_error(
        &errors[1],
        "subsystems/warehouse.yaml",
        Some(9),
        "пункт меню «goods» повторяется",
    );
}

#[test]
fn unknown_icons_are_rejected() {
    let config = TempConfig::new("check-icons");
    with_menu(&config, &["warehouse"]);
    config
        .write(
            "catalogs/goods.yaml",
            "catalog: goods\ntitle: Т\nicon: rocket\nfields: []\n",
        )
        .write(
            "subsystems/warehouse.yaml",
            "subsystem: warehouse\ntitle: Склад\nicon: Box\nobjects:\n  - goods\n",
        );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_error(
        &errors[0],
        "catalogs/goods.yaml",
        Some(3),
        "нет значка «rocket»",
    );
    assert_error(
        &errors[1],
        "subsystems/warehouse.yaml",
        Some(3),
        "нет значка «Box»",
    );
}

#[test]
fn icon_list_matches_ui_assets() {
    let icons = repo_config().join("../crates/ostov-ui/assets/icons");
    for icon in ostov_meta::check::ICONS {
        assert!(
            icons.join(format!("{icon}.svg")).is_file(),
            "нет файла значка {icon}.svg"
        );
    }
}

#[test]
fn menu_entry_of_wrong_shape_is_a_parse_error() {
    let config = TempConfig::new("check-menu-shape");
    config.write(
        "subsystems/warehouse.yaml",
        "subsystem: warehouse\ntitle: Склад\nobjects:\n  - [goods]\n",
    );
    let error = single(config.errors());
    assert_error(
        &error,
        "subsystems/warehouse.yaml",
        Some(4),
        "имя объекта или группу",
    );
}

#[test]
fn required_and_read_only_default_to_false() {
    let config = TempConfig::new("check-flags");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: Т\nfields:\n  - name: name\n    title: Н\n    type: text\n    required: true\n  - name: code\n    title: К\n    type: text\n    read_only: true\n  - name: note\n    title: П\n    type: text\n",
    );
    let model = load(&config.dir).unwrap();
    let flags: Vec<(bool, bool)> = model.catalogs[0]
        .fields
        .iter()
        .map(|f| (f.required, f.read_only))
        .collect();
    assert_eq!(flags, vec![(true, false), (false, true), (false, false)]);
}
