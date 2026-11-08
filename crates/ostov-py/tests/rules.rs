//! `PyRules`: загрузка `.py`, Python API документа и движений, перевод ошибок.

#![cfg(feature = "python")]
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;

use common::{QUIET_ISSUE, Scripts, dec, fields, model, rule};
use ostov_core::{CoreError, Document, FieldValue, MovementKind, Parts};
use ostov_model::{RegisterDef, RegisterKind};

const RECEIPT: &str = r#"
from decimal import Decimal, ROUND_HALF_UP

def before_write(doc, ctx):
    total = Decimal("0.00")
    for row in doc.lines:
        if row.qty is None or row.price is None:
            row.amount = None
            continue
        row.amount = (row.qty * row.price).quantize(Decimal("0.01"), ROUND_HALF_UP)
        total += row.amount
    doc.amount = total

def on_post(doc, ctx):
    for row in doc.lines:
        ctx.receipt("stock", warehouse=doc.warehouse, goods=row.goods, qty=row.qty)
"#;

fn document(lines: Vec<ostov_core::Fields>) -> Document {
    Document {
        id: 7,
        version: 1,
        number: 3,
        posted: false,
        deleted: false,
        header: fields(&[
            ("date", FieldValue::Date("2026-02-01".into())),
            ("supplier", FieldValue::Ref(5)),
            ("warehouse", FieldValue::Ref(2)),
        ]),
        parts: BTreeMap::from([("lines".to_owned(), lines)]),
    }
}

fn line(goods: i64, qty: &str, price: &str) -> ostov_core::Fields {
    fields(&[
        ("goods", FieldValue::Ref(goods)),
        ("qty", dec(qty)),
        ("price", dec(price)),
    ])
}

/// Ошибка `movements` для поступления с телом `on_post`.
fn on_post_error(test: &str, body: &str) -> CoreError {
    let model = model();
    let scripts = Scripts::new(test);
    scripts
        .write(
            "receipt",
            &format!("import ostov\n\ndef on_post(doc, ctx):\n{body}\n"),
        )
        .write("issue", QUIET_ISSUE);
    let rules = scripts.load(&model).unwrap();
    rule(&rules, &model, "receipt")
        .movements(&document(vec![line(1, "2.500", "3.335")]))
        .unwrap_err()
}

fn script_message(error: CoreError) -> (String, String) {
    match error {
        CoreError::Script { message, details } => (message, details),
        other => panic!("ожидалась ошибка логики: {other:?}"),
    }
}

#[test]
fn before_write_computes_amounts_and_on_post_moves_stock() {
    let model = model();
    let scripts = Scripts::new("happy");
    scripts
        .write("receipt", RECEIPT)
        .write("issue", QUIET_ISSUE);
    let rules = scripts.load(&model).unwrap();
    let receipt = rule(&rules, &model, "receipt");

    let mut header = document(vec![]).header;
    let mut parts: Parts = BTreeMap::from([(
        "lines".to_owned(),
        vec![
            line(1, "2.500", "3.335"),
            fields(&[("goods", FieldValue::Ref(2)), ("qty", dec("1.000"))]),
        ],
    )]);
    receipt.prepare(&mut header, &mut parts).unwrap();
    assert_eq!(
        parts["lines"][0]["amount"],
        dec("8.34"),
        "2,5 × 3,335 = 8,3375 → 8,34"
    );
    assert_eq!(
        parts["lines"][1]["amount"],
        FieldValue::Null,
        "нет цены — нет суммы"
    );
    assert_eq!(header["amount"], dec("8.34"));
    assert_eq!(
        header["warehouse"],
        FieldValue::Ref(2),
        "остальные поля не тронуты"
    );

    let moves = receipt
        .movements(&document(vec![line(1, "2.500", "3.335")]))
        .unwrap();
    let [movement] = moves["stock"].as_slice() else {
        panic!("{moves:?}");
    };
    assert_eq!(movement.kind, MovementKind::Receipt);
    assert_eq!(movement.dims["warehouse"], FieldValue::Ref(2));
    assert_eq!(movement.dims["goods"], FieldValue::Ref(1));
    assert_eq!(movement.resources["qty"].to_string(), "2.500");
}

#[test]
fn document_attributes_and_types_reach_python() {
    let body = "    assert doc.id == 7 and doc.number == 3 and doc.posted is False\n    \
                import datetime, decimal\n    \
                assert doc.date == datetime.date(2026, 2, 1)\n    \
                assert doc.warehouse == 2 and doc.amount is None\n    \
                assert isinstance(doc.lines[0].qty, decimal.Decimal)\n    \
                raise ostov.UserError('всё дошло')";
    match on_post_error("types", body) {
        CoreError::Invalid(text) => assert_eq!(text, "всё дошло"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn user_error_text_goes_to_user_as_is() {
    match on_post_error(
        "user-error",
        "    raise ostov.UserError('Нельзя провести без склада')",
    ) {
        CoreError::Invalid(text) => assert_eq!(text, "Нельзя провести без склада"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn other_exception_becomes_script_error_with_traceback() {
    let (message, details) = script_message(on_post_error("exception", "    int('x')"));
    assert!(
        message.starts_with("Ошибка в логике документа «Поступление»: ValueError: invalid literal"),
        "{message}"
    );
    assert!(details.contains("Traceback"), "{details}");
    assert!(details.contains("receipt.py\", line 4"), "{details}");
}

#[test]
fn on_post_is_read_only() {
    let (message, _) = script_message(on_post_error("read-only", "    doc.warehouse = 1"));
    assert!(
        message.contains("AttributeError: receipt: в on_post поля только для чтения"),
        "{message}"
    );
    let (message, _) = script_message(on_post_error("read-only-row", "    doc.lines[0].qty = 1"));
    assert!(message.contains("только для чтения"), "{message}");
}

#[test]
fn rows_cannot_be_added() {
    let (message, _) = script_message(on_post_error("rows", "    doc.lines.append(None)"));
    assert!(
        message.contains("AttributeError: 'tuple' object has no attribute 'append'"),
        "{message}"
    );
}

#[test]
fn float_is_rejected_in_fields_and_movements() {
    let model = model();
    let scripts = Scripts::new("float");
    scripts
        .write("receipt", "def before_write(doc, ctx):\n    doc.amount = 1.5\n\ndef on_post(doc, ctx):\n    pass\n")
        .write("issue", QUIET_ISSUE);
    let rules = scripts.load(&model).unwrap();
    let error = rule(&rules, &model, "receipt")
        .prepare(&mut document(vec![]).header, &mut Parts::new())
        .unwrap_err();
    let (message, _) = script_message(error);
    assert!(
        message.contains("TypeError: receipt.amount: числа только Decimal или int"),
        "{message}"
    );

    let (message, _) = script_message(on_post_error(
        "float-move",
        "    ctx.receipt('stock', warehouse=1, goods=1, qty=2.5)",
    ));
    assert!(
        message.contains("TypeError: stock.qty: числа только Decimal или int"),
        "{message}"
    );
}

#[test]
fn wrong_type_is_rejected() {
    let (message, _) = script_message(on_post_error(
        "wrong-type",
        "    ctx.receipt('stock', warehouse='Основной', goods=1, qty=1)",
    ));
    assert!(
        message.contains("stock.warehouse: ожидается int (id), а не str"),
        "{message}"
    );
}

#[test]
fn movement_keys_must_match_register() {
    let (message, _) = script_message(on_post_error(
        "extra-key",
        "    ctx.receipt('stock', warehouse=1, goods=1, qty=1, price=2)",
    ));
    assert!(
        message.contains("TypeError: stock: лишние ключи price"),
        "{message}"
    );
    let (message, _) = script_message(on_post_error(
        "missing-key",
        "    ctx.issue('stock', warehouse=1, qty=1)",
    ));
    assert!(
        message.contains("stock: не хватает ключей goods"),
        "{message}"
    );
    let (message, _) = script_message(on_post_error(
        "no-qty",
        "    ctx.issue('stock', warehouse=1, goods=1, qty=None)",
    ));
    assert!(message.contains("stock: ресурс qty не указан"), "{message}");
}

#[test]
fn only_document_registers_can_move() {
    let (message, _) = script_message(on_post_error(
        "unknown-register",
        "    ctx.receipt('sales', goods=1, qty=1)",
    ));
    assert!(
        message.contains("ValueError: нет регистра «sales»"),
        "{message}"
    );

    let mut model = model();
    model.registers.push(RegisterDef {
        name: "sales".into(),
        title: "Продажи".into(),
        kind: RegisterKind::Accumulation,
        dimensions: vec![ostov_model::FieldDef::reference("goods", "goods")],
        resources: vec![ostov_model::FieldDef::new(
            "qty",
            ostov_model::FieldKind::Decimal { scale: 3 },
        )],
    });
    let scripts = Scripts::new("foreign-register");
    scripts
        .write(
            "receipt",
            "def on_post(doc, ctx):\n    ctx.receipt('sales', goods=1, qty=1)\n",
        )
        .write("issue", QUIET_ISSUE);
    let rules = scripts.load(&model).unwrap();
    let error = rule(&rules, &model, "receipt")
        .movements(&document(vec![]))
        .unwrap_err();
    let (message, _) = script_message(error);
    assert!(
        message.contains("регистра «sales» нет в registers документа receipt"),
        "{message}"
    );
}

#[test]
fn load_reports_every_broken_file() {
    let model = model();
    let scripts = Scripts::new("load-errors");
    scripts.write("receipt", "def before_write(doc, ctx):\n    pass\n");
    let errors = scripts.load(&model).unwrap_err();
    let texts: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(errors.len(), 2, "{texts:#?}");
    assert!(
        texts[0].contains("issue.py — нет issue.py: у документа есть регистры"),
        "{texts:#?}"
    );
    assert!(
        texts[1].ends_with("receipt.py — в receipt.py нет on_post"),
        "{texts:#?}"
    );
}

#[test]
fn syntax_error_has_file_and_line() {
    let model = model();
    let scripts = Scripts::new("syntax");
    scripts
        .write(
            "receipt",
            "def on_post(doc, ctx):\n    pass\n\ndef broken(:\n    pass\n",
        )
        .write("issue", QUIET_ISSUE);
    let [error] = scripts.load(&model).unwrap_err().try_into().unwrap();
    assert!(error.file.ends_with("receipt.py"), "{error}");
    assert_eq!(error.line, Some(4), "{error}");
    assert!(error.reason.starts_with("синтаксическая ошибка"), "{error}");
}

#[test]
fn exception_while_loading_points_to_its_line() {
    let model = model();
    let scripts = Scripts::new("import-error");
    scripts
        .write(
            "receipt",
            "import ostov\n\nraise RuntimeError('нет настройки')\n",
        )
        .write("issue", QUIET_ISSUE);
    let [error] = scripts.load(&model).unwrap_err().try_into().unwrap();
    assert_eq!(error.line, Some(3), "{error}");
    assert_eq!(error.reason, "RuntimeError: нет настройки");
}

#[test]
fn document_without_registers_needs_no_script() {
    let mut model = model();
    for document in &mut model.documents {
        document.registers.clear();
    }
    let scripts = Scripts::new("no-registers");
    scripts.write(
        "receipt",
        "def before_write(doc, ctx):\n    doc.amount = 0\n",
    );
    let rules = scripts.load(&model).unwrap();
    let issue = model.documents.iter().find(|d| d.name == "issue").unwrap();
    assert!(ostov_core::local::Rules::rule(&rules, issue).is_none());
    let receipt = rule(&rules, &model, "receipt");
    let mut header = document(vec![]).header;
    receipt.prepare(&mut header, &mut Parts::new()).unwrap();
    assert_eq!(header["amount"], dec("0"));
    assert!(receipt.movements(&document(vec![])).unwrap().is_empty());
}

#[test]
fn reload_forgets_functions_of_previous_version() {
    let model = model();
    let scripts = Scripts::new("reload");
    scripts
        .write("receipt", RECEIPT)
        .write("issue", QUIET_ISSUE);
    scripts.load(&model).unwrap();
    scripts.write("receipt", "def before_write(doc, ctx):\n    pass\n");
    let [error] = scripts.load(&model).unwrap_err().try_into().unwrap();
    assert_eq!(error.reason, "в receipt.py нет on_post");
}

#[test]
fn parallel_loads_of_one_document_do_not_mix_functions() {
    let model = model();
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let model = model.clone();
            std::thread::spawn(move || {
                let scripts = Scripts::new(&format!("parallel-{n}"));
                let body = format!("import ostov\n\ndef on_post(doc, ctx):\n    raise ostov.UserError('версия {n}')\n");
                scripts.write("receipt", &body).write("issue", QUIET_ISSUE);
                let rules = scripts.load(&model).unwrap();
                match rule(&rules, &model, "receipt").movements(&document(vec![])) {
                    Err(CoreError::Invalid(text)) => (n, text),
                    other => panic!("{other:?}"),
                }
            })
        })
        .collect();
    for handle in handles {
        let (n, text) = handle.join().unwrap();
        assert_eq!(text, format!("версия {n}"));
    }
}
