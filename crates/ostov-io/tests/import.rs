//! Импорт xlsx: read_xlsx → plan → write через Engine, права и журнал — от ядра.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::path::Path;

use common::{Base, add, model, text};
use ostov_api::{Engine, ErrorCode, ListQuery, NewUser, ReportQuery, Value};
use ostov_io::{Cell, Limits, Mapping, Target, plan, read_xlsx, read_xlsx_sheet, suggest, write};
use rust_xlsxwriter::{Format, Workbook};

/// Ячейка входного файла.
enum In {
    T(&'static str),
    N(f64),
    /// Дата Excel: номер дня с форматом даты.
    D(f64),
    E,
}

/// Лист входного файла: имя, заголовки, строки.
type SheetIn<'a> = (&'a str, &'a [&'a str], &'a [&'a [In]]);

fn write_xlsx(path: &Path, sheets: &[SheetIn<'_>]) {
    let mut book = Workbook::new();
    let date = Format::new().set_num_format("dd.mm.yyyy");
    for (name, headers, rows) in sheets {
        let sheet = book.add_worksheet();
        sheet.set_name(*name).unwrap();
        for (col, header) in (0u16..).zip(headers.iter()) {
            sheet.write_string(0, col, *header).unwrap();
        }
        for (row, cells) in (1u32..).zip(rows.iter()) {
            for (col, cell) in (0u16..).zip(cells.iter()) {
                match cell {
                    In::T(v) => drop(sheet.write_string(row, col, *v).unwrap()),
                    In::N(v) => drop(sheet.write_number(row, col, *v).unwrap()),
                    In::D(v) => drop(sheet.write_number_with_format(row, col, *v, &date).unwrap()),
                    In::E => {}
                }
            }
        }
    }
    book.save(path).unwrap();
}

fn goods_count(engine: &dyn Engine) -> u64 {
    let query = ListQuery {
        object: "goods".into(),
        size: 100,
        ..ListQuery::default()
    };
    engine.list(&query).unwrap().total.unwrap_or_default()
}

fn goods_file(base: &Base, rows: &[&[In]]) -> std::path::PathBuf {
    let path = base.file("goods.xlsx");
    write_xlsx(&path, &[("Товары", &["Наименование", "Ед. изм."], rows)]);
    path
}

fn goods_mapping(engine: &dyn Engine, path: &Path) -> (ostov_io::Table, Mapping) {
    let table = read_xlsx(path, &Limits::default()).unwrap();
    let mut mapping = suggest(&engine.get_meta().unwrap(), "goods", &table).unwrap();
    assert_eq!(
        mapping.columns,
        [Target::Field("name".into()), Target::Skip],
        "«Ед. изм.» не узнать"
    );
    mapping.columns[1] = Target::Field("unit".into());
    (table, mapping)
}

#[test]
fn bad_goods_row_blocks_write_until_fixed() {
    let base = Base::new("import-goods");
    let engine = base.engine();
    let path = goods_file(
        &base,
        &[
            &[In::T("Мука"), In::T("кг")],
            &[In::E, In::T("шт")],
            &[In::T("Сахар"), In::T("кг")],
        ],
    );
    let (table, mapping) = goods_mapping(&*engine, &path);
    assert_eq!(table.first_row, 2);

    let checked = plan(&*engine, &table, &mapping).unwrap();
    let errors: Vec<String> = checked.errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        errors,
        ["строка 3, колонка «Наименование»: не заполнено обязательное поле «Наименование»"]
    );
    assert_eq!(checked.items.len(), 2);
    assert!(
        write(&*engine, &checked).is_err(),
        "с ошибками write отказывается"
    );
    assert_eq!(goods_count(&*engine), 0, "ничего не записано");

    let path = goods_file(
        &base,
        &[
            &[In::T("Мука"), In::T("кг")],
            &[In::T("Соль"), In::T("шт")],
            &[In::T("Сахар"), In::T("кг")],
        ],
    );
    let (table, mapping) = goods_mapping(&*engine, &path);
    let checked = plan(&*engine, &table, &mapping).unwrap();
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let written = write(&*engine, &checked).unwrap();
    assert_eq!(
        (written.saved, written.ids.len(), written.failure),
        (3, 3, None)
    );
    assert_eq!(goods_count(&*engine), 3);

    // Повторный импорт тех же названий — не обновление, а ошибка (R1).
    let again = plan(&*engine, &table, &mapping).unwrap();
    assert_eq!(again.errors.len(), 3);
    assert_eq!(again.errors[0].reason, "уже есть «Мука» в «Товары»");
}

#[test]
fn same_name_twice_in_file_is_an_error() {
    let base = Base::new("import-twice");
    let engine = base.engine();
    let path = goods_file(&base, &[&[In::T("Мука"), In::E], &[In::T(" мука "), In::E]]);
    let (table, mapping) = goods_mapping(&*engine, &path);
    let checked = plan(&*engine, &table, &mapping).unwrap();
    assert_eq!(checked.errors.len(), 1);
    assert_eq!(checked.errors[0].row, 3);
    assert!(
        checked.errors[0]
            .reason
            .contains("уже есть выше в этом файле")
    );
}

const RECEIPT_HEADERS: [&str; 7] = [
    "Накладная",
    "Дата",
    "Поставщик",
    "Склад",
    "Товар",
    "Количество",
    "Цена",
];

fn receipts_mapping(engine: &dyn Engine, table: &ostov_io::Table) -> Mapping {
    let mut mapping = suggest(&engine.get_meta().unwrap(), "receipt", table).unwrap();
    let line = |field: &str| Target::Line {
        part: "lines".into(),
        field: field.into(),
    };
    assert_eq!(
        mapping.columns,
        [
            Target::Skip,
            Target::Field("date".into()),
            Target::Field("supplier".into()),
            Target::Field("warehouse".into()),
            line("goods"),
            line("qty"),
            line("price"),
        ]
    );
    mapping.group_by = Some(0);
    mapping.post = true;
    mapping
}

fn stock_directory(engine: &dyn Engine) {
    for name in ["Мука", "Сахар"] {
        add(engine, "goods", &[("name", text(name))]);
    }
    add(engine, "warehouses", &[("name", text("Основной"))]);
    add(engine, "suppliers", &[("name", text("Мельница"))]);
}

#[test]
fn receipts_are_grouped_by_number_with_refs_by_name_and_posted() {
    let base = Base::new("import-receipts");
    let engine = base.engine();
    stock_directory(&*engine);
    let path = base.file("receipts.xlsx");
    let rows: &[&[In]] = &[
        &[
            In::T("Н-1"),
            In::T("10.01.2026"),
            In::T("Мельница"),
            In::T("основной"),
            In::T("Мука"),
            In::T("2,5"),
            In::N(3.33),
        ],
        &[
            In::T("Н-1"),
            In::E,
            In::E,
            In::E,
            In::T("Сахар"),
            In::N(1.0),
            In::T("10"),
        ],
        &[
            In::T("Н-2"),
            In::D(46033.0),
            In::T("Мельница"),
            In::T("Основной"),
            In::T("МУКА"),
            In::N(3.0),
            In::N(4.0),
        ],
        &[
            In::T("Н-2"),
            In::E,
            In::E,
            In::E,
            In::T("Сахар"),
            In::T("2.000"),
            In::N(5.0),
        ],
    ];
    write_xlsx(&path, &[("Накладные", &RECEIPT_HEADERS, rows)]);
    let table = read_xlsx(&path, &Limits::default()).unwrap();
    assert_eq!(
        table.rows[2][1],
        Cell::Date("2026-01-11".into()),
        "дата Excel"
    );

    let checked = plan(&*engine, &table, &receipts_mapping(&*engine, &table)).unwrap();
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    assert_eq!(checked.items.len(), 2);
    assert_eq!(checked.items[0].rows, [2, 3]);
    let first = &checked.items[0].data;
    assert_eq!(first.fields["date"], Value::Date("2026-01-10".into()));
    assert_eq!(first.tables["lines"].len(), 2);
    assert_eq!(
        first.tables["lines"][0]["qty"],
        Value::Decimal {
            units: 2500,
            scale: 3
        }
    );
    assert_eq!(
        first.tables["lines"][0]["price"],
        Value::Decimal {
            units: 333,
            scale: 2
        }
    );

    let written = write(&*engine, &checked).unwrap();
    assert_eq!((written.saved, written.failure), (2, None));
    let receipt = engine.get("receipt", written.ids[0]).unwrap();
    assert!(receipt.posted);
    assert_eq!(
        receipt.fields["amount"],
        Value::Decimal {
            units: 1833,
            scale: 2
        },
        "8,33 + 10,00"
    );
    let balance = engine
        .run_report(&ReportQuery {
            report: "stock_balance".into(),
            params: Default::default(),
            after_key: None,
            size: 10,
        })
        .unwrap();
    let flour = balance
        .rows
        .iter()
        .find(|r| matches!(&r.values["goods"], Value::Ref { title: Some(t), .. } if t == "Мука"))
        .unwrap();
    assert_eq!(
        flour.values["qty"],
        Value::Decimal {
            units: 5500,
            scale: 3
        }
    );
}

#[test]
fn every_wrong_cell_is_reported_with_row_and_column() {
    let base = Base::new("import-errors");
    let engine = base.engine();
    stock_directory(&*engine);
    add(&*engine, "goods", &[("name", text("Соль"))]);
    add(&*engine, "goods", &[("name", text("соль"))]);
    let path = base.file("bad.xlsx");
    let rows: &[&[In]] = &[
        &[
            In::T("1"),
            In::T("31.02.2026"),
            In::T("Мельница"),
            In::T("Основной"),
            In::T("Мука в/с"),
            In::T("1"),
            In::T("3,335"),
        ],
        &[
            In::T("2"),
            In::T("2026-01-10"),
            In::T("Мельница"),
            In::T("Основной"),
            In::T("Соль"),
            In::T("два"),
            In::N(1.0),
        ],
        &[
            In::T("3"),
            In::T("2026-01-10"),
            In::E,
            In::T("Основной"),
            In::T("Мука"),
            In::N(1.0),
            In::N(1.0),
        ],
    ];
    write_xlsx(&path, &[("Лист1", &RECEIPT_HEADERS, rows)]);
    let table = read_xlsx(&path, &Limits::default()).unwrap();
    let checked = plan(&*engine, &table, &receipts_mapping(&*engine, &table)).unwrap();
    let errors: Vec<String> = checked.errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        errors,
        [
            "строка 2, колонка «Дата»: «31.02.2026» — не дата: нужно ДД.ММ.ГГГГ или ГГГГ-ММ-ДД",
            "строка 2, колонка «Товар»: нет «Мука в/с» в «Товары»",
            "строка 2, колонка «Цена»: в «3,335» больше 2 знаков после запятой",
            "строка 3, колонка «Товар»: «Соль» в «Товары» неоднозначно: найдено 2",
            "строка 3, колонка «Количество»: «два» — не число",
            "строка 4, колонка «Поставщик»: не заполнено обязательное поле «Поставщик»",
        ]
    );
    assert!(checked.items.is_empty());
}

fn viewer_model() -> ostov_model::Config {
    let mut model = model();
    model.roles.push(ostov_model::RoleDef {
        name: "viewer".into(),
        title: "Наблюдатель".into(),
        objects: vec![(
            "goods".into(),
            ostov_model::Rights {
                read: true,
                ..Default::default()
            },
        )],
        system: Default::default(),
    });
    model
}

#[test]
fn import_without_create_right_is_forbidden_by_core() {
    let base = Base::new("import-rights");
    let engine = base.engine_with(&ostov_core::LocalConnector::new(viewer_model()));
    engine
        .create_first_admin("boss", "admin-password-1")
        .unwrap();
    engine
        .admin_add_user(&NewUser {
            login: "anna".into(),
            display_name: "Анна".into(),
            role: "viewer".into(),
            password: "viewer-password-1".into(),
        })
        .unwrap();
    engine.logout().unwrap();
    engine.login("anna", "viewer-password-1").unwrap();

    let path = goods_file(&base, &[&[In::T("Мука"), In::T("кг")]]);
    let (table, mapping) = goods_mapping(&*engine, &path);
    let checked = plan(&*engine, &table, &mapping).unwrap();
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let written = write(&*engine, &checked).unwrap();
    let failure = written.failure.unwrap();
    assert_eq!(failure.error.code, ErrorCode::Forbidden);
    assert_eq!((written.saved, failure.rows), (0, vec![2]));
    assert_eq!(goods_count(&*engine), 0, "ничего не записано");
}

#[test]
fn journal_names_who_imported() {
    let base = Base::new("import-journal");
    let engine = base.engine();
    engine
        .create_first_admin("boss", "admin-password-1")
        .unwrap();
    let path = goods_file(
        &base,
        &[&[In::T("Мука"), In::T("кг")], &[In::T("Сахар"), In::E]],
    );
    let (table, mapping) = goods_mapping(&*engine, &path);
    write(&*engine, &plan(&*engine, &table, &mapping).unwrap()).unwrap();
    let journal = engine.admin_journal(None, 50).unwrap();
    let created: Vec<_> = journal
        .iter()
        .filter(|e| e.object.contains("goods") || e.object.contains("Товар"))
        .collect();
    assert_eq!(created.len(), 2, "{journal:#?}");
    assert!(created.iter().all(|e| e.user == "boss"), "{journal:#?}");
}

#[test]
fn sheets_are_listed_and_one_is_read() {
    let base = Base::new("import-sheets");
    let path = base.file("book.xlsx");
    write_xlsx(
        &path,
        &[
            ("Справка", &["Что"], &[&[In::T("не то")]]),
            ("Товары", &["Наименование"], &[&[In::T("Мука")]]),
        ],
    );
    let first = read_xlsx(&path, &Limits::default()).unwrap();
    assert_eq!(first.sheets, ["Справка", "Товары"]);
    assert_eq!(first.sheet, "Справка");
    let goods = read_xlsx_sheet(&path, Some("Товары"), &Limits::default()).unwrap();
    assert_eq!(
        (goods.headers.clone(), goods.rows.clone()),
        (
            vec!["Наименование".to_owned()],
            vec![vec![Cell::Text("Мука".into())]]
        )
    );
    let error = read_xlsx_sheet(&path, Some("Нет"), &Limits::default()).unwrap_err();
    assert_eq!(error.to_string(), "в книге нет листа «Нет»");
}
