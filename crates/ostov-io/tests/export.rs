//! Экспорт в xlsx: список и отчёт открываются calamine, заголовки и значения совпадают.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use calamine::Data;
use common::{Base, add, read_sheet, receipt, s, text};
use ostov_api::{ListQuery, PostAction, ReportQuery, Value};
use ostov_io::{export_list, export_report};

fn list(object: &str) -> ListQuery {
    ListQuery {
        object: object.into(),
        size: 50,
        ..ListQuery::default()
    }
}

fn number(cell: &Data) -> f64 {
    match cell {
        Data::Float(v) => *v,
        Data::Int(v) => v.to_string().parse().unwrap(),
        Data::DateTime(d) => d.as_f64(),
        other => panic!("не число: {other:?}"),
    }
}

#[test]
fn goods_list_has_titles_and_values() {
    let base = Base::new("export-goods");
    let engine = base.engine();
    add(
        &*engine,
        "goods",
        &[("name", text("Мука")), ("unit", text("кг"))],
    );
    add(&*engine, "goods", &[("name", text("Сахар"))]);
    let path = base.file("goods.xlsx");

    assert_eq!(export_list(&*engine, &list("goods"), &path).unwrap(), 2);
    let (sheet, rows) = read_sheet(&path);
    assert_eq!(sheet, "Товары");
    assert_eq!(
        rows[0],
        [s("Наименование"), s("Единица измерения"), s("Помечен")]
    );
    assert_eq!(rows[1], [s("Мука"), s("кг"), s("Нет")]);
    assert_eq!(
        rows[2],
        [s("Сахар"), Data::Empty, s("Нет")],
        "пустое значение — пустая ячейка"
    );
}

#[test]
fn list_keeps_search_and_reads_every_page() {
    let base = Base::new("export-pages");
    let engine = base.engine();
    for n in 0..1203 {
        add(
            &*engine,
            "goods",
            &[("name", text(&format!("Товар {n:04}")))],
        );
    }
    add(&*engine, "goods", &[("name", text("Мука"))]);
    let path = base.file("all.xlsx");
    let query = ListQuery {
        size: 7,
        ..list("goods")
    };
    assert_eq!(
        export_list(&*engine, &query, &path).unwrap(),
        1204,
        "все страницы, не одна"
    );

    let search = ListQuery {
        search: Some("мука".into()),
        ..list("goods")
    };
    assert_eq!(export_list(&*engine, &search, &path).unwrap(), 1);
    assert_eq!(read_sheet(&path).1[1][0], s("Мука"));
}

#[test]
fn documents_have_number_dates_refs_and_money() {
    let base = Base::new("export-receipts");
    let engine = base.engine();
    let flour = add(&*engine, "goods", &[("name", text("Мука"))]);
    let main = add(&*engine, "warehouses", &[("name", text("Основной"))]);
    let mill = add(&*engine, "suppliers", &[("name", text("Мельница"))]);
    engine
        .save(
            "receipt",
            &receipt("2026-01-10", main, mill, &[(flour, 2_500, 333)]),
            PostAction::Post,
        )
        .unwrap();
    let path = base.file("receipts.xlsx");

    assert_eq!(export_list(&*engine, &list("receipt"), &path).unwrap(), 1);
    let (_, rows) = read_sheet(&path);
    assert_eq!(
        rows[0],
        [
            s("Номер"),
            s("Дата"),
            s("Поставщик"),
            s("Склад"),
            s("Сумма"),
            s("Проведён"),
            s("Помечен")
        ]
    );
    let row = &rows[1];
    assert_eq!(number(&row[0]), 1.0);
    assert!(
        matches!(row[1], Data::DateTime(_)),
        "дата Excel: {:?}",
        row[1]
    );
    assert_eq!(number(&row[1]), 46032.0, "10.01.2026");
    assert_eq!(row[2], s("Мельница"));
    assert_eq!(row[3], s("Основной"));
    assert_eq!(number(&row[4]), 8.33);
    assert_eq!((row[5].clone(), row[6].clone()), (s("Да"), s("Нет")));
}

#[test]
fn stock_reports_have_register_titles() {
    let base = Base::new("export-report");
    let engine = base.engine();
    let flour = add(&*engine, "goods", &[("name", text("Мука"))]);
    let main = add(&*engine, "warehouses", &[("name", text("Основной"))]);
    let mill = add(&*engine, "suppliers", &[("name", text("Мельница"))]);
    engine
        .save(
            "receipt",
            &receipt("2026-01-10", main, mill, &[(flour, 2_500, 100)]),
            PostAction::Post,
        )
        .unwrap();

    let balance = base.file("balance.xlsx");
    let query = ReportQuery {
        report: "stock_balance".into(),
        params: Default::default(),
        after_key: None,
        size: 10,
    };
    assert_eq!(export_report(&*engine, &query, &balance).unwrap(), 1);
    let (_, rows) = read_sheet(&balance);
    assert_eq!(rows[0], [s("Товар"), s("Склад"), s("Количество")]);
    assert_eq!(rows[1][0], s("Мука"));
    assert_eq!(rows[1][1], s("Основной"));
    assert_eq!(number(&rows[1][2]), 2.5);

    let turnover = base.file("turnover.xlsx");
    let query = ReportQuery {
        report: "stock_turnover".into(),
        params: common::fields(&[
            ("from", Value::Date("2026-01-01".into())),
            ("to", Value::Date("2026-01-31".into())),
        ]),
        after_key: None,
        size: 10,
    };
    assert_eq!(export_report(&*engine, &query, &turnover).unwrap(), 1);
    let (_, rows) = read_sheet(&turnover);
    assert_eq!(
        rows[0],
        [
            s("Товар"),
            s("Склад"),
            s("Количество: начало"),
            s("Количество: приход"),
            s("Количество: расход"),
            s("Количество: конец"),
        ]
    );
    assert_eq!(number(&rows[1][3]), 2.5);
}

#[test]
fn unknown_object_and_forbidden_query_are_errors() {
    let base = Base::new("export-errors");
    let engine = base.engine();
    let path = base.file("x.xlsx");
    let error = export_list(&*engine, &list("orders"), &path).unwrap_err();
    assert_eq!(error.to_string(), "нет объекта «orders»");
    let query = ReportQuery {
        report: "sales".into(),
        params: Default::default(),
        after_key: None,
        size: 10,
    };
    assert!(matches!(
        export_report(&*engine, &query, &path),
        Err(ostov_io::IoError::Engine(_))
    ));
}
