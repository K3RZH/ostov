//! Загрузка папки конфигурации: верная загружается, у каждой ошибки верные файл, строка и причина.

#![allow(clippy::unwrap_used)]

mod common;

use std::fs;

use common::{MANIFEST, TempConfig, assert_error, repo_config, single};
use ostov_meta::{Error, FieldType, RegisterKind, load};

#[test]
fn warehouse_config_loads() {
    let config = load(&repo_config()).unwrap();
    assert_eq!(config.manifest.value.name.value, "warehouse");
    assert_eq!(
        config.manifest.value.format.value,
        ostov_meta::FORMAT_VERSION
    );

    let names = |items: Vec<&str>| items.into_iter().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(
        config
            .catalogs
            .iter()
            .map(|c| c.value.name.value.clone())
            .collect::<Vec<_>>(),
        names(vec!["goods", "suppliers", "warehouses"])
    );
    assert_eq!(
        config
            .documents
            .iter()
            .map(|d| d.value.name.value.clone())
            .collect::<Vec<_>>(),
        names(vec!["issue", "receipt"])
    );

    let receipt = &config.documents[1];
    assert!(receipt.file.ends_with("documents/receipt.yaml"));
    let supplier = &receipt.value.fields[1];
    assert_eq!(supplier.kind.value, FieldType::Ref("suppliers".into()));
    assert_eq!(supplier.kind.referenced.line(), 11, "строка ref(suppliers)");
    let qty = &receipt.value.tables[0].fields[1];
    assert_eq!(qty.kind.value, FieldType::Decimal { scale: 3 });

    let stock = &config.registers[0].value;
    assert_eq!(stock.kind, RegisterKind::Accumulation);
    assert_eq!(stock.dimensions.len(), 2);
    assert_eq!(stock.resources.len(), 1);
}

#[test]
fn missing_object_folders_mean_no_objects() {
    let config = TempConfig::new("empty");
    let loaded = load(&config.dir).unwrap();
    assert!(
        loaded.catalogs.is_empty() && loaded.documents.is_empty() && loaded.registers.is_empty()
    );
}

#[test]
fn missing_manifest_is_reported() {
    let config = TempConfig::new("no-manifest");
    fs::remove_file(config.dir.join("ostov.yaml")).unwrap();
    assert_error(
        &single(config.errors()),
        "ostov.yaml",
        None,
        "нет манифеста",
    );
}

#[test]
fn other_format_version_is_rejected_with_clear_reason() {
    let config = TempConfig::new("format");
    // Новый ключ формата 2 не должен заслонять ошибку версии.
    config.write(
        "ostov.yaml",
        "name: test\ntitle: Тест\nversion: 0.1.0\nformat: 2\nmodules: []\n",
    );
    assert_error(
        &single(config.errors()),
        "ostov.yaml",
        Some(4),
        "формат конфигурации 2 не поддерживается",
    );
}

#[test]
fn unknown_key_is_rejected() {
    let config = TempConfig::new("unknown-key");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: Товары\ncolor: red\nfields: []\n",
    );
    assert_error(
        &single(config.errors()),
        "catalogs/goods.yaml",
        Some(3),
        "color",
    );
}

#[test]
fn missing_key_is_rejected() {
    let config = TempConfig::new("missing-key");
    config.write("catalogs/goods.yaml", "catalog: goods\nfields: []\n");
    let error = single(config.errors());
    assert_error(&error, "catalogs/goods.yaml", error.line, "title");
    assert!(error.line.is_some(), "{error}");
}

#[test]
fn decimal_without_scale_points_to_its_line() {
    let config = TempConfig::new("decimal");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: Товары\nfields:\n  - name: weight\n    title: Вес\n    type: decimal\n",
    );
    assert_error(
        &single(config.errors()),
        "catalogs/goods.yaml",
        Some(6),
        "не указана точность",
    );
}

#[test]
fn unknown_register_kind_is_rejected() {
    let config = TempConfig::new("register-kind");
    config.write(
        "registers/prices.yaml",
        "register: prices\ntitle: Цены\nkind: info\ndimensions: []\nresources: []\n",
    );
    assert_error(
        &single(config.errors()),
        "registers/prices.yaml",
        Some(3),
        "info",
    );
}

#[test]
fn anchors_and_aliases_are_forbidden() {
    let config = TempConfig::new("anchors");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: &t Товары\nfields:\n  - name: name\n    title: *t\n    type: text\n",
    );
    assert_error(
        &single(config.errors()),
        "catalogs/goods.yaml",
        Some(2),
        "якоря и алиасы",
    );
}

#[test]
fn alias_without_anchor_is_rejected() {
    let config = TempConfig::new("alias");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: *t\nfields: []\n",
    );
    let error = single(config.errors());
    assert_error(&error, "catalogs/goods.yaml", error.line, "");
    assert_eq!(error.line, Some(2), "{error}");
}

#[test]
fn duplicate_keys_are_rejected() {
    let config = TempConfig::new("duplicate-key");
    config.write(
        "catalogs/goods.yaml",
        "catalog: goods\ntitle: Товары\ntitle: Ещё\nfields: []\n",
    );
    assert_error(
        &single(config.errors()),
        "catalogs/goods.yaml",
        Some(3),
        "title",
    );
}

#[test]
fn too_deep_nesting_is_rejected() {
    // Лишние ключи манифеста версия формата пропускает не глядя — здесь и нужен лимит глубины.
    let config = TempConfig::new("depth");
    let deep = format!("{MANIFEST}extra: {}{}\n", "[".repeat(40), "]".repeat(40));
    config.write("ostov.yaml", &deep);
    assert_error(
        &single(config.errors()),
        "ostov.yaml",
        Some(5),
        "вложенность глубже",
    );
}

#[test]
fn file_over_size_limit_is_rejected() {
    let config = TempConfig::new("size");
    let big = format!(
        "catalog: goods\ntitle: Т\nfields: []\n#{}\n",
        "x".repeat(1024 * 1024)
    );
    config.write("catalogs/goods.yaml", &big);
    assert_error(
        &single(config.errors()),
        "catalogs/goods.yaml",
        None,
        "файл больше 1 МБ",
    );
}

#[test]
fn all_broken_files_are_reported_at_once() {
    let config = TempConfig::new("many");
    config
        .write("catalogs/goods.yaml", "catalog: goods\ntitle: Товары\ncolor: red\nfields: []\n")
        .write(
            "documents/receipt.yaml",
            "document: receipt\ntitle: Поступление\nfields:\n  - name: qty\n    title: Кол-во\n    type: decimal\n",
        )
        .write("registers/stock.yaml", "register: stock\ntitle: Остатки\nkind: info\ndimensions: []\nresources: []\n");
    let errors = config.errors();
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert_error(&errors[0], "catalogs/goods.yaml", Some(3), "color");
    assert_error(
        &errors[1],
        "documents/receipt.yaml",
        Some(6),
        "не указана точность",
    );
    assert_error(&errors[2], "registers/stock.yaml", Some(3), "info");
}

#[test]
fn error_display_is_file_line_reason() {
    let error = Error::new("config/catalogs/goods.yaml", Some(3), "неизвестный ключ");
    assert_eq!(
        error.to_string(),
        format!("{} — неизвестный ключ", "config/catalogs/goods.yaml:3")
    );
    let error = Error::new("config/ostov.yaml", None, "нет манифеста конфигурации");
    assert_eq!(
        error.to_string(),
        "config/ostov.yaml — нет манифеста конфигурации"
    );
}
