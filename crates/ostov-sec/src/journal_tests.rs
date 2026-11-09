//! Журнал: порядок, страницы, отборы, текст изменений, секреты. Хранение — на SQLite и,
//! если задан `OSTOV_PG_URL`, на PostgreSQL.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;

use ostov_store::{Driver, Value};

use crate::journal::{Action, Change, Entry, JournalQuery, details, list, record};
use crate::tests::{GOOD_PASSWORD, add, bases};
use crate::{Password, SecError, set_password, verify};

fn entry<'a>(login: &'a str, action: Action, object: &'a str, object_id: Option<i64>) -> Entry<'a> {
    Entry {
        at: "2026-11-08T12:34:56Z",
        login,
        action,
        object,
        object_id,
        changes: &[],
        note: "",
    }
}

fn change(field: &str, old: Option<&str>, new: Option<&str>) -> Change {
    Change {
        field: field.into(),
        old: old.map(str::to_owned),
        new: new.map(str::to_owned),
    }
}

fn page(size: u32) -> JournalQuery {
    JournalQuery {
        size,
        ..JournalQuery::default()
    }
}

fn ids(db: &mut dyn Driver, query: &JournalQuery) -> Vec<i64> {
    list(db, query).unwrap().rows.iter().map(|r| r.id).collect()
}

#[test]
fn pages_go_from_newest_without_gaps_or_repeats() {
    for mut db in bases("journal_pages") {
        let db = db.as_mut();
        for n in 1..=25 {
            record(db, &entry("ivan", Action::Update, "receipt", Some(n))).unwrap();
        }
        let mut seen = Vec::new();
        let mut query = page(10);
        let mut sizes = Vec::new();
        loop {
            let page = list(db, &query).unwrap();
            sizes.push(page.rows.len());
            seen.extend(page.rows.iter().map(|r| r.id));
            match page.next_after_id {
                Some(after_id) => query.after_id = Some(after_id),
                None => break,
            }
        }
        assert_eq!(sizes, [10, 10, 5]);
        assert!(
            seen.windows(2).all(|w| w[0] > w[1]),
            "от новых к старым: {seen:?}"
        );
        assert_eq!(seen.iter().collect::<BTreeSet<_>>().len(), 25);
        // Ровно полная последняя страница: следующей нет.
        let full = list(
            db,
            &JournalQuery {
                after_id: Some(seen[4]),
                ..page(20)
            },
        )
        .unwrap();
        assert_eq!(full.rows.len(), 20);
        assert_eq!(full.next_after_id, None);
    }
}

#[test]
fn rows_keep_every_column() {
    for mut db in bases("journal_columns") {
        let db = db.as_mut();
        record(db, &entry("ivan", Action::Post, "receipt", Some(7))).unwrap();
        record(db, &entry("", Action::LoginFailed, "", None)).unwrap();
        let rows = list(db, &page(10)).unwrap().rows;
        let failed = &rows[0];
        assert_eq!(
            (
                failed.login.as_str(),
                failed.action.as_str(),
                failed.action_title.as_str()
            ),
            ("", "login_failed", "Неудачный вход")
        );
        assert_eq!((failed.object.as_str(), failed.object_id), ("", None));
        let post = &rows[1];
        assert_eq!(post.at, "2026-11-08T12:34:56Z");
        assert_eq!(
            (
                post.action.as_str(),
                post.action_title.as_str(),
                post.object_id
            ),
            ("post", "Проведение", Some(7))
        );
    }
}

#[test]
fn filters_by_object_record_and_login() {
    for mut db in bases("journal_filters") {
        let db = db.as_mut();
        record(db, &entry("ivan", Action::Create, "receipt", Some(1))).unwrap();
        record(db, &entry("anna", Action::Update, "receipt", Some(1))).unwrap();
        record(db, &entry("ivan", Action::Create, "receipt", Some(2))).unwrap();
        record(db, &entry("ivan", Action::Create, "goods", Some(1))).unwrap();
        record(db, &entry("ivan", Action::Login, "", None)).unwrap();
        let by = |object: Option<&str>, object_id, login: Option<&str>| JournalQuery {
            object: object.map(str::to_owned),
            object_id,
            login: login.map(str::to_owned),
            ..page(100)
        };
        assert_eq!(ids(db, &by(Some("receipt"), None, None)), [3, 2, 1]);
        assert_eq!(ids(db, &by(Some("receipt"), Some(1), None)), [2, 1]);
        assert_eq!(ids(db, &by(None, None, Some("anna"))), [2]);
        assert_eq!(ids(db, &by(Some("receipt"), Some(1), Some("ivan"))), [1]);
        assert_eq!(ids(db, &by(None, Some(1), None)), [4, 2, 1]);
    }
}

#[test]
fn page_size_is_limited() {
    for mut db in bases("journal_size") {
        let db = db.as_mut();
        for size in [0, 1001] {
            match list(db, &page(size)) {
                Err(SecError::Invalid(reason)) => {
                    assert_eq!(reason, "размер страницы журнала: от 1 до 1000");
                }
                other => panic!("{size}: {other:?}"),
            }
        }
        assert!(list(db, &page(1000)).unwrap().rows.is_empty());
    }
}

#[test]
fn details_show_changes_empty_values_and_note() {
    let changes = [
        change("Количество", Some("2,500"), Some("3,000")),
        change("Цена", None, Some("3,33")),
        change("Склад", Some("Основной"), None),
    ];
    assert_eq!(
        details(&changes, ""),
        "Количество: 2,500 → 3,000; Цена: — → 3,33; Склад: Основной → —"
    );
    assert_eq!(
        details(&changes[..1], "(по заявке)"),
        "Количество: 2,500 → 3,000 (по заявке)"
    );
    assert_eq!(
        details(&[], "дата закрытия 2026-02-28"),
        "дата закрытия 2026-02-28"
    );
    assert_eq!(details(&[], ""), "");
}

#[test]
fn long_details_are_cut_to_4000_chars() {
    let long = [change("Комментарий", None, Some(&"я".repeat(5000)))];
    let text = details(&long, "");
    assert_eq!(text.chars().count(), 4000);
    assert!(text.ends_with('…'));
    assert!(text.starts_with("Комментарий: — → яяя"));
    let exact = details(&[], &"я".repeat(4000));
    assert_eq!(exact.chars().count(), 4000);
    assert!(!exact.ends_with('…'), "ровно 4000 не обрезается");
}

#[test]
fn changes_are_written_only_for_create_and_update() {
    for mut db in bases("journal_changes") {
        let db = db.as_mut();
        let changes = [change("Количество", Some("1"), Some("2"))];
        for action in [Action::Create, Action::Update, Action::Post] {
            let entry = Entry {
                changes: &changes,
                note: "заметка",
                ..entry("ivan", action, "receipt", Some(1))
            };
            record(db, &entry).unwrap();
        }
        let details: Vec<String> = list(db, &page(10))
            .unwrap()
            .rows
            .into_iter()
            .map(|r| r.details)
            .collect();
        assert_eq!(
            details,
            [
                "заметка",
                "Количество: 1 → 2 заметка",
                "Количество: 1 → 2 заметка"
            ]
        );
    }
}

#[test]
fn action_codes_are_unique_and_round_trip() {
    let codes: BTreeSet<&str> = Action::ALL.iter().map(|a| a.code()).collect();
    let titles: BTreeSet<&str> = Action::ALL.iter().map(|a| a.title()).collect();
    assert_eq!(
        (codes.len(), titles.len()),
        (Action::ALL.len(), Action::ALL.len())
    );
    for action in Action::ALL {
        assert_eq!(Action::from_code(action.code()), Some(action));
    }
    assert_eq!(Action::from_code("logout"), None);
}

#[test]
fn passwords_never_reach_the_journal() {
    for mut db in bases("journal_secrets") {
        let db = db.as_mut();
        add(db, "ivan", "storekeeper");
        let typed = "неверный-пароль-777";
        let new = "новый-секретный-пароль";
        // Как это сделает ядро: неудачный вход и смена пароля, каждый со своей записью.
        let _ = verify(db, "ivan", &Password::new(typed));
        record(db, &entry("ivan", Action::LoginFailed, "", None)).unwrap();
        set_password(db, "ivan", &Password::new(new)).unwrap();
        // Даже если вызывающий по ошибке передаст пароль в изменениях, он не запишется.
        let leaked = [change("Пароль", Some(GOOD_PASSWORD), Some(new))];
        let password_changed = Entry {
            changes: &leaked,
            ..entry("boss", Action::PasswordChanged, "", None)
        };
        record(db, &password_changed).unwrap();

        let mut stored = String::new();
        for table in ["ostov_journal", "ostov_users"] {
            for row in db.query(&format!("SELECT * FROM {table}"), &[]).unwrap() {
                for value in row {
                    if let Value::Text(text) = value {
                        stored.push_str(&text);
                        stored.push('\n');
                    }
                }
            }
        }
        for secret in [typed, new, GOOD_PASSWORD] {
            assert!(
                !stored.contains(secret),
                "в базе открытый пароль «{secret}»"
            );
        }
        let rows = list(db, &page(10)).unwrap().rows;
        assert_eq!(rows[0].details, "", "смена пароля без значений");
        assert_eq!(rows[1].login, "ivan", "в неудачном входе — введённый логин");
    }
}
