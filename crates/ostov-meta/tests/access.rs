//! Себестоимость регистра, свойства пользователя и ограничения ролей (ADR-042…044):
//! разбор в модель и проверки `ostov check` с файлом и строкой.

#![allow(clippy::unwrap_used)]

mod common;

use common::{MANIFEST, TempConfig, assert_error, repo_config, single};
use ostov_meta::{load, load_with_warnings};
use ostov_model::{Cost, FieldDef, FieldKind, Restriction, UserFieldDef};

const WAREHOUSES: &str =
    "catalog: warehouses\ntitle: Склады\nfields:\n  - name: name\n    title: Н\n    type: text\n";
const GOODS: &str =
    "catalog: goods\ntitle: Товары\nfields:\n  - name: name\n    title: Н\n    type: text\n";

/// Измерения на строках 5 и 8, ресурсы на 12 и 15; `cost:` дописывается с 18-й.
const STOCK: &str = "register: stock
title: Остатки
kind: accumulation
dimensions:
  - name: warehouse
    title: Склад
    type: ref(warehouses)
  - name: goods
    title: Товар
    type: ref(goods)
resources:
  - name: qty
    title: Количество
    type: decimal(3)
  - name: amount
    title: Сумма
    type: decimal(2)
";

/// Обязательные ссылки: склад в шапке (строка 6), товар в строке (строка 14).
const ISSUE: &str = "document: issue
title: Расход
fields:
  - name: warehouse
    title: Склад
    type: ref(warehouses)
    required: true
tables:
  - name: lines
    title: Товары
    fields:
      - name: goods
        title: Товар
        type: ref(goods)
        required: true
";

/// Свойство «Склады» на строках 6–9 манифеста.
const USER_FIELDS: &str = "user_fields:
  - name: warehouses
    title: Склады
    type: ref(warehouses)
    many: true
";

/// Права роли на строках 3–6, ограничения — с 7-й.
const ROLE: &str = "role: storekeeper
title: Кладовщик
objects:
  warehouses: [read]
  goods: [read]
  issue: [read, create]
";

fn base(test: &str) -> TempConfig {
    let config = TempConfig::new(test);
    config
        .write("ostov.yaml", &format!("{MANIFEST}{USER_FIELDS}"))
        .write("catalogs/warehouses.yaml", WAREHOUSES)
        .write("catalogs/goods.yaml", GOODS)
        .write("documents/issue.yaml", ISSUE)
        .write("registers/stock.yaml", STOCK)
        .write("roles/storekeeper.yaml", ROLE);
    config
}

fn cost(text: &str) -> String {
    format!("{STOCK}cost:\n{text}")
}

fn restrictions(text: &str) -> String {
    format!("{ROLE}restrictions:\n{text}")
}

#[test]
fn full_example_goes_into_model() {
    let config = base("access-model");
    config
        .write(
            "registers/stock.yaml",
            &cost("  qty: qty\n  sum: amount\n  by: [warehouse, goods]\n"),
        )
        .write(
            "roles/storekeeper.yaml",
            &restrictions(
                "  - object: issue
    where:
      - field: warehouse
        in: user.warehouses
  - object: stock
    where:
      - field: warehouse
        in: user.warehouses
      - field: goods
        in: user.goods
",
            ),
        )
        .write(
            "ostov.yaml",
            &format!(
                "{MANIFEST}{USER_FIELDS}  - name: goods\n    title: Товары\n    type: ref(goods)\n"
            ),
        );
    let (model, warnings) = load_with_warnings(&config.dir).unwrap();
    assert!(warnings.is_empty(), "{warnings:#?}");

    assert_eq!(
        model.registers[0].cost,
        Some(Cost {
            qty: "qty".into(),
            sum: "amount".into(),
            by: vec!["warehouse".into(), "goods".into()],
        })
    );
    assert_eq!(model.user_fields.len(), 2);
    assert_eq!(
        model.user_fields[0],
        UserFieldDef {
            field: FieldDef {
                name: "warehouses".into(),
                title: "Склады".into(),
                kind: FieldKind::Ref,
                target: Some("warehouses".into()),
                required: false,
                read_only: false,
            },
            many: true,
        }
    );
    assert!(!model.user_fields[1].many);
    let restriction = |object: &str, field: &str, user_field: &str| Restriction {
        object: object.into(),
        field: field.into(),
        user_field: user_field.into(),
    };
    assert_eq!(
        model.roles[0].restrictions,
        vec![
            restriction("issue", "warehouse", "warehouses"),
            restriction("stock", "warehouse", "warehouses"),
            restriction("stock", "goods", "goods"),
        ]
    );
}

#[test]
fn without_new_keys_everything_is_empty() {
    let config = TempConfig::new("access-empty");
    config.write("catalogs/goods.yaml", GOODS);
    let model = load(&config.dir).unwrap();
    assert!(model.user_fields.is_empty());
    assert!(model.registers.iter().all(|r| r.cost.is_none()));
}

#[test]
fn repo_config_has_storekeeper_restrictions_and_core_accepts_them() {
    let model = load(&repo_config()).unwrap();
    let warehouses = &model.user_fields[0];
    assert_eq!(warehouses.field.name, "warehouses");
    assert_eq!(warehouses.field.target.as_deref(), Some("warehouses"));
    assert!(warehouses.many);
    let storekeeper = model
        .roles
        .iter()
        .find(|r| r.name == "storekeeper")
        .unwrap();
    let objects: Vec<_> = storekeeper
        .restrictions
        .iter()
        .map(|r| (r.object.as_str(), r.field.as_str(), r.user_field.as_str()))
        .collect();
    assert_eq!(
        objects,
        vec![
            ("issue", "warehouse", "warehouses"),
            ("stock", "warehouse", "warehouses"),
        ]
    );
    // Ядро проверяет то же при загрузке.
    ostov_core::local::Config::from_model(&model).unwrap();
}

#[test]
fn cost_resources_must_differ_and_exist() {
    let config = base("access-cost-res");
    config.write(
        "registers/stock.yaml",
        &cost("  qty: qty\n  sum: price\n  by: [warehouse]\n"),
    );
    let error = single(config.check_errors());
    assert_error(
        &error,
        "stock.yaml",
        Some(20),
        "«price» — не ресурс регистра",
    );

    config.write(
        "registers/stock.yaml",
        &cost("  qty: qty\n  sum: qty\n  by: [warehouse]\n"),
    );
    let error = single(config.check_errors());
    assert_error(&error, "stock.yaml", Some(20), "разными ресурсами");

    config.write(
        "registers/stock.yaml",
        &cost("  qty: warehouse\n  sum: amount\n  by: [goods]\n"),
    );
    let error = single(config.check_errors());
    assert_error(&error, "stock.yaml", Some(19), "«warehouse» — не ресурс");
}

#[test]
fn cost_by_dimensions_only_without_repeats() {
    let config = base("access-cost-by");
    config.write(
        "registers/stock.yaml",
        &cost("  qty: qty\n  sum: amount\n  by: [warehouse, amount, warehouse]\n"),
    );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(errors.iter().all(|e| e.line == Some(21)));
    assert!(
        errors
            .iter()
            .any(|e| e.reason.contains("«amount» — не измерение"))
    );
    assert!(errors.iter().any(|e| e.reason.contains("повторяется")));

    config.write(
        "registers/stock.yaml",
        &cost("  qty: qty\n  sum: amount\n  by: []\n"),
    );
    let error = single(config.check_errors());
    assert_error(&error, "stock.yaml", Some(19), "хотя бы одно измерение");
}

#[test]
fn unknown_key_in_cost_is_a_parse_error() {
    let config = base("access-cost-key");
    config.write(
        "registers/stock.yaml",
        &cost("  qty: qty\n  sum: amount\n  by: [goods]\n  method: fifo\n"),
    );
    let error = single(config.errors());
    assert_error(&error, "stock.yaml", Some(22), "method");
}

#[test]
fn user_fields_names_and_targets() {
    let config = base("access-user-fields");
    config.write(
        "ostov.yaml",
        &format!(
            "{MANIFEST}{USER_FIELDS}  - name: warehouses\n    title: Ещё\n    type: text\n  - name: Docs\n    title: Д\n    type: ref(issue)\n"
        ),
    );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert_error(&errors[0], "ostov.yaml", Some(10), "повторяется");
    assert_error(&errors[1], "ostov.yaml", Some(13), "строчные латинские");
    assert_error(&errors[2], "ostov.yaml", Some(15), "только на справочник");
}

#[test]
fn restriction_object_and_field_must_exist() {
    let config = base("access-restrict-names");
    config.write(
        "roles/storekeeper.yaml",
        &restrictions(
            "  - object: orders
    where:
      - field: warehouse
        in: user.warehouses
  - object: issue
    where:
      - field: shop
        in: user.warehouses
  - object: stock
    where:
      - field: qty
        in: user.warehouses
",
        ),
    );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert_error(
        &errors[0],
        "storekeeper.yaml",
        Some(8),
        "нет справочника, документа или регистра «orders»",
    );
    assert_error(
        &errors[1],
        "storekeeper.yaml",
        Some(14),
        "у «issue» нет поля «shop»",
    );
    assert_error(&errors[2], "storekeeper.yaml", Some(18), "«qty» — ресурс");
}

#[test]
fn restriction_user_field_must_exist_and_match_type() {
    let config = base("access-restrict-user");
    config
        .write(
            "ostov.yaml",
            &format!("{MANIFEST}{USER_FIELDS}  - name: code\n    title: Код\n    type: text\n"),
        )
        .write(
            "roles/storekeeper.yaml",
            &restrictions(
                "  - object: issue
    where:
      - field: warehouse
        in: warehouses
      - field: warehouse
        in: user.shops
      - field: warehouse
        in: user.code
  - object: stock
    where:
      - field: goods
        in: user.warehouses
  - object: goods
    where: []
",
            ),
        );
    let errors = config.check_errors();
    assert_eq!(errors.len(), 5, "{errors:#?}");
    assert_error(&errors[0], "storekeeper.yaml", Some(11), "вида user.<имя>");
    assert_error(
        &errors[1],
        "storekeeper.yaml",
        Some(13),
        "нет свойства пользователя «shops»",
    );
    assert_error(&errors[2], "storekeeper.yaml", Some(15), "разного типа");
    // Ссылки на разные справочники — тоже разный тип.
    assert_error(&errors[3], "storekeeper.yaml", Some(19), "разного типа");
    assert_error(&errors[4], "storekeeper.yaml", Some(20), "нет условий");
}

#[test]
fn creator_without_read_on_required_ref_gets_warning() {
    let config = base("access-warning");
    config.write(
        "roles/storekeeper.yaml",
        "role: storekeeper\ntitle: Кладовщик\nobjects:\n  warehouses: [read]\n  issue: [read, create]\n",
    );
    let (_, warnings) = load_with_warnings(&config.dir).unwrap();
    let warning = single(warnings);
    assert_error(&warning, "storekeeper.yaml", Some(5), "не читает «goods»");
    assert!(warning.reason.contains("не сможет заполнить"), "{warning}");

    // Без права create предупреждения нет: документ только читают.
    config.write(
        "roles/storekeeper.yaml",
        "role: storekeeper\ntitle: Кладовщик\nobjects:\n  issue: [read]\n",
    );
    let (_, warnings) = load_with_warnings(&config.dir).unwrap();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

#[test]
fn repo_config_has_no_warnings() {
    let (_, warnings) = load_with_warnings(&repo_config()).unwrap();
    assert!(warnings.is_empty(), "{warnings:#?}");
}
