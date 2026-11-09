#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use ostov_api as api;
use ostov_api::{EngineError, ErrorCode};
use prost::Message;

use crate::{from_status, pb, to_status};

/// Туда, в байты, из байтов и обратно — как по сети.
fn wire<P: Message + Default>(p: P) -> P {
    P::decode(p.encode_to_vec().as_slice()).unwrap()
}

fn all_values() -> api::Fields {
    [
        ("null", api::Value::Null),
        ("int", api::Value::Int(-42)),
        ("text", api::Value::Text("Мука «в/с»".into())),
        (
            "decimal",
            api::Value::Decimal {
                units: -123_456,
                scale: 3,
            },
        ),
        ("flag", api::Value::Bool(true)),
        ("date", api::Value::Date("2026-02-28".into())),
        (
            "ref",
            api::Value::Ref {
                id: 7,
                title: Some("Основной".into()),
            },
        ),
        ("bare_ref", api::Value::Ref { id: 8, title: None }),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

#[test]
fn object_data_survives_the_wire() {
    let data = api::ObjectData {
        id: Some(5),
        version: 3,
        number: Some(12),
        posted: true,
        deleted: false,
        fields: all_values(),
        tables: BTreeMap::from([("lines".to_owned(), vec![all_values(), api::Fields::new()])]),
    };
    let back: api::ObjectData = wire(pb::ObjectData::from(data.clone())).try_into().unwrap();
    assert_eq!(back, data);

    let new = api::ObjectData::default();
    let back: api::ObjectData = wire(pb::ObjectData::from(new.clone())).try_into().unwrap();
    assert_eq!(back, new);
}

#[test]
fn lists_and_reports_survive_the_wire() {
    let query = api::ListQuery {
        object: "receipt".into(),
        order: Some("supplier".into()),
        descending: true,
        search: Some("мельн".into()),
        posted: Some(false),
        deleted: None,
        filter: [
            (
                "warehouse".to_owned(),
                api::Value::Ref { id: 3, title: None },
            ),
            ("name".to_owned(), api::Value::Text("Мука".into())),
        ]
        .into(),
        after_key: Some("ключ".into()),
        size: 50,
    };
    let back: api::ListQuery = wire(pb::ListQuery::from(query.clone())).try_into().unwrap();
    assert_eq!(back, query);

    let page = api::ListPage {
        rows: vec![api::Row {
            id: 1,
            values: all_values(),
        }],
        next_key: None,
        total: Some(0),
        token: "t".into(),
    };
    let back: api::ListPage = wire(pb::ListPage::from(page.clone())).try_into().unwrap();
    assert_eq!(back, page);

    let report = api::ReportQuery {
        report: "stock_balance".into(),
        params: all_values(),
        after_key: None,
        size: 10,
    };
    let back: api::ReportQuery = wire(pb::ReportQuery::from(report.clone()))
        .try_into()
        .unwrap();
    assert_eq!(back, report);
}

#[test]
fn meta_survives_the_wire() {
    let field = |kind| api::FieldDef {
        name: "f".into(),
        title: "Поле".into(),
        kind,
        target: None,
        required: true,
        read_only: false,
    };
    let meta = api::Meta {
        registers: vec![api::RegisterMeta {
            name: "stock".into(),
            title: "Остатки".into(),
            dimensions: vec![field(api::FieldKind::Ref)],
            resources: vec![field(api::FieldKind::Decimal { scale: 3 })],
        }],
        menu: vec![api::MenuItem {
            title: "Склад".into(),
            icon: Some("warehouse".into()),
            object: None,
            children: vec![api::MenuItem {
                title: "Товары".into(),
                icon: None,
                object: Some("goods".into()),
                children: Vec::new(),
            }],
        }],
        objects: vec![api::ObjectMeta {
            name: "receipt".into(),
            title: "Поступление".into(),
            kind: api::ObjectKind::Document,
            icon: Some("stock-in".into()),
            fields: vec![
                field(api::FieldKind::Int),
                field(api::FieldKind::Text),
                field(api::FieldKind::Decimal { scale: 2 }),
                field(api::FieldKind::Date),
                api::FieldDef {
                    target: Some("suppliers".into()),
                    read_only: true,
                    ..field(api::FieldKind::Ref)
                },
            ],
            table_parts: vec![api::TablePartDef {
                name: "lines".into(),
                title: "Товары".into(),
                fields: vec![field(api::FieldKind::Decimal { scale: 3 })],
            }],
            rights: api::Rights {
                read: true,
                post: true,
                ..api::Rights::default()
            },
        }],
        roles: vec![
            api::RoleInfo {
                name: "admin".into(),
                title: "Администратор".into(),
            },
            api::RoleInfo {
                name: "storekeeper".into(),
                title: "Кладовщик".into(),
            },
        ],
    };
    let back: api::Meta = wire(pb::Meta::from(meta.clone())).try_into().unwrap();
    assert_eq!(back, meta);
}

#[test]
fn small_types_survive_the_wire() {
    let hello = api::HelloInfo {
        protocol_version: 1,
        config_name: "Склад".into(),
        config_version: "0.1.0".into(),
        config_format: 1,
        login_required: true,
        first_run: false,
    };
    assert_eq!(
        api::HelloInfo::from(wire(pb::HelloInfo::from(hello.clone()))),
        hello
    );

    let session = api::SessionInfo {
        login: "boss".into(),
        display_name: "Начальник".into(),
        roles: vec!["admin".into()],
        system: api::SystemRights {
            admin: true,
            ..api::SystemRights::default()
        },
    };
    let reply = wire(pb::LoginReply::new(session.clone(), "токен".into()));
    assert_eq!(crate::session_from_pb(reply), (session, "токен".to_owned()));

    for action in [
        api::PostAction::Write,
        api::PostAction::Post,
        api::PostAction::Unpost,
    ] {
        let raw: i32 = pb::PostAction::from(action).into();
        assert_eq!(crate::post_action_from_pb(raw).unwrap(), action);
    }
    assert!(crate::post_action_from_pb(99).is_err());

    let saved = api::Saved {
        id: 1,
        version: 2,
        number: None,
    };
    assert_eq!(
        api::Saved::from(wire(pb::Saved::from(saved.clone()))),
        saved
    );

    let user = api::NewUser {
        login: "keeper".into(),
        display_name: "Кладовщик".into(),
        role: "storekeeper".into(),
        password: "секрет-пароль-1".into(),
    };
    assert_eq!(
        api::NewUser::from(wire(pb::NewUser::from(user.clone()))),
        user
    );

    let entry = api::JournalEntry {
        id: 9,
        when: "2026-11-09T10:00:00Z".into(),
        user: "boss".into(),
        object: "Товары #5".into(),
        action: "Изменение".into(),
        details: "Наименование: Мука → Мука в/с".into(),
    };
    assert_eq!(
        api::JournalEntry::from(wire(pb::JournalEntry::from(entry.clone()))),
        entry
    );
}

#[test]
fn every_error_code_survives_the_wire() {
    let codes = [
        ErrorCode::NotImplemented,
        ErrorCode::Unavailable,
        ErrorCode::BadCredentials,
        ErrorCode::NeedFirstAdmin,
        ErrorCode::NeedFilePassword,
        ErrorCode::AlreadyOpen,
        ErrorCode::Forbidden,
        ErrorCode::VersionConflict,
        ErrorCode::PeriodClosed,
        ErrorCode::Validation,
        ErrorCode::Incompatible,
        ErrorCode::NotFound,
        ErrorCode::Internal,
    ];
    for code in codes {
        let e = EngineError {
            code,
            message: "Не хватает остатка на 10.02.2026: Товар «Мука»".into(),
            details: "Traceback ...".into(),
        };
        assert_eq!(from_status(&to_status(&e)), e);
    }
}

#[test]
fn foreign_status_becomes_understandable_error() {
    let down = from_status(&tonic::Status::unavailable("connection refused"));
    assert_eq!(down.code, ErrorCode::Unavailable);
    assert!(down.message.starts_with("Нет связи с сервером"));
    assert!(down.details.contains("connection refused"));
    assert_eq!(
        from_status(&tonic::Status::internal("boom")).code,
        ErrorCode::Internal
    );
}

#[test]
fn broken_input_is_rejected_not_panicking() {
    let bad_scale = pb::Value {
        kind: Some(pb::value::Kind::Decimal(pb::Decimal {
            units: 1,
            scale: 300,
        })),
    };
    let err = api::Value::try_from(bad_scale).unwrap_err();
    assert_eq!(err.code, ErrorCode::Incompatible);
    let bad_kind = pb::FieldDef {
        name: "x".into(),
        r#type: 77,
        ..pb::FieldDef::default()
    };
    assert!(api::FieldDef::try_from(bad_kind).is_err());
}
