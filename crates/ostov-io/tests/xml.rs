//! XML: чтение в тот же `Table`, выгрузка и загрузка туда и обратно, запрет DTD и сущностей.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::path::Path;

use common::{Base, add, text};
use ostov_api::{Engine, ListQuery, Value};
use ostov_io::{Cell, IoError, Limits, export_list_xml, plan, read_xml, suggest, write};

fn read(base: &Base, xml: &str) -> Result<ostov_io::Table, IoError> {
    let path = base.file("in.xml");
    std::fs::write(&path, xml).unwrap();
    read_xml(&path, &Limits::default())
}

fn goods(engine: &dyn Engine) -> Vec<(String, Option<String>)> {
    let query = ListQuery {
        object: "goods".into(),
        size: 100,
        order: Some("name".into()),
        ..ListQuery::default()
    };
    engine
        .list(&query)
        .unwrap()
        .rows
        .into_iter()
        .map(|row| {
            let field = |name: &str| match row.values.get(name) {
                Some(Value::Text(t)) => Some(t.clone()),
                _ => None,
            };
            (field("name").unwrap_or_default(), field("unit"))
        })
        .collect()
}

#[test]
fn records_become_table_rows() {
    let base = Base::new("xml-read");
    let table = read(
        &base,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<goods>
  <item><name>Мука</name><unit>кг</unit></item>
  <item><unit>шт</unit><name>Соль &amp; перец &#8470;1</name><note/></item>
</goods>"#,
    )
    .unwrap();
    assert_eq!(table.sheet, "goods");
    assert_eq!(table.headers, ["name", "unit", "note"]);
    assert_eq!(
        table.rows[0][..2],
        [Cell::Text("Мука".into()), Cell::Text("кг".into())]
    );
    assert_eq!(table.rows[1][0], Cell::Text("Соль & перец №1".into()));
    assert_eq!(table.rows[1][2], Cell::Empty);
    assert_eq!(table.row_number(1), 2, "строка — номер записи");
}

#[test]
fn goods_go_to_xml_and_back_into_another_base() {
    let source = Base::new("xml-source");
    let engine = source.engine();
    add(
        &*engine,
        "goods",
        &[("name", text("Мука")), ("unit", text("кг"))],
    );
    add(&*engine, "goods", &[("name", text("Соль <йодированная>"))]);
    let path = source.file("goods.xml");
    let query = ListQuery {
        object: "goods".into(),
        size: 10,
        ..ListQuery::default()
    };
    assert_eq!(export_list_xml(&*engine, &query, &path).unwrap(), 2);
    let xml = std::fs::read_to_string(&path).unwrap();
    assert!(
        xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
        "{xml}"
    );
    assert!(
        xml.contains("<name>Соль &lt;йодированная&gt;</name>"),
        "{xml}"
    );

    let target = Base::new("xml-target");
    let other = target.engine();
    let table = read_xml(&path, &Limits::default()).unwrap();
    let mapping = suggest(&other.get_meta().unwrap(), "goods", &table).unwrap();
    let checked = plan(&*other, &table, &mapping).unwrap();
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    assert_eq!(write(&*other, &checked).unwrap().saved, 2);
    assert_eq!(goods(&*other), goods(&*engine));
}

#[test]
fn receipts_export_dates_refs_and_money_as_text() {
    let base = Base::new("xml-receipts");
    let engine = base.engine();
    let flour = add(&*engine, "goods", &[("name", text("Мука"))]);
    let main = add(&*engine, "warehouses", &[("name", text("Основной"))]);
    let mill = add(&*engine, "suppliers", &[("name", text("Мельница"))]);
    engine
        .save(
            "receipt",
            &common::receipt("2026-01-10", main, mill, &[(flour, 2_500, 333)]),
            ostov_api::PostAction::Post,
        )
        .unwrap();
    let path = base.file("receipts.xml");
    let query = ListQuery {
        object: "receipt".into(),
        size: 10,
        ..ListQuery::default()
    };
    export_list_xml(&*engine, &query, &path).unwrap();
    let xml = std::fs::read_to_string(&path).unwrap();
    for part in [
        "<receipt>",
        "<date>10.01.2026</date>",
        "<supplier>Мельница</supplier>",
        "<amount>8.33</amount>",
    ] {
        assert!(xml.contains(part), "нет {part} в\n{xml}");
    }
}

fn rejected(base: &Base, xml: &str) -> String {
    read(base, xml).unwrap_err().to_string()
}

#[test]
fn dtd_entities_and_nesting_are_rejected() {
    let base = Base::new("xml-rejects");
    assert_eq!(
        rejected(
            &base,
            "<!DOCTYPE goods []><goods><item><name>x</name></item></goods>"
        ),
        "XML с DTD не принимается"
    );
    assert!(
        rejected(&base, "<goods><item><name>&nbsp;</name></item></goods>")
            .starts_with("сущность &nbsp; не принимается")
    );
    assert_eq!(
        rejected(
            &base,
            "<goods><item><name><b>Мука</b></name></item></goods>"
        ),
        "запись 1: в поле вложенный элемент <b>"
    );
    assert_eq!(rejected(&base, "<!-- пусто -->"), "в XML нет записей");
    assert!(
        rejected(&base, "<goods><item><name>Мука</item></goods>").starts_with("XML не разобран")
    );
    let missing = read_xml(Path::new("нет-такого-файла.xml"), &Limits::default()).unwrap_err();
    assert!(matches!(missing, IoError::File(_)), "{missing:?}");
}
