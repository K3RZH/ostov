use super::*;
use crate::testing::*;
use crate::{FieldDef, FieldValue, in_transaction};
use ostov_store::Decimal;
use ostov_store::drivers::sqlite::SqliteDriver;

fn goods() -> Catalog {
    Catalog::new(
        "goods",
        vec![
            FieldDef::new("name", FieldKind::Text),
            FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
        ],
    )
    .unwrap()
}

/// Справочник из `n` товаров «item 001», «item 002»…; у каждого пятого цены нет.
fn db_with(n: usize) -> SqliteDriver {
    let mut db = memory_db();
    let cat = goods();
    cat.create_table(&mut db).unwrap();
    in_transaction(&mut db, |db| {
        for i in 1..=n {
            let price = if i % 5 == 0 {
                FieldValue::Null
            } else {
                dec(&format!("{}.{:02}", i % 7, i % 100))
            };
            cat.insert(
                db,
                &fields(&[("name", text(&format!("item {i:03}"))), ("price", price)]),
            )?;
        }
        Ok(())
    })
    .unwrap();
    db
}

fn ids(page: &ListPage) -> Vec<i64> {
    page.rows.iter().map(|r| r.id).collect()
}

/// Все строки списка, страница за страницей.
fn walk(db: &mut SqliteDriver, query: &ListQuery, size: usize) -> Vec<ListRow> {
    let mut rows = Vec::new();
    let mut key = None;
    loop {
        let page = list(db, &goods(), query, key.as_deref(), size).unwrap();
        rows.extend(page.rows);
        match page.next_key {
            Some(next) => key = Some(next),
            None => return rows,
        }
    }
}

fn by(field: &str, descending: bool) -> ListQuery {
    ListQuery {
        order: Some(field.into()),
        descending,
        search: None,
    }
}

#[test]
fn pages_cover_all_rows_once() {
    let mut db = db_with(250);
    let first = list(&mut db, &goods(), &ListQuery::default(), None, 40).unwrap();
    assert_eq!(first.total, Some(250));
    assert_eq!(ids(&first), (1..=40).collect::<Vec<_>>());

    let all: Vec<i64> = walk(&mut db, &ListQuery::default(), 40)
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(all, (1..=250).collect::<Vec<_>>());
}

#[test]
fn sorted_by_field_with_empty_values_last() {
    let mut db = db_with(60);
    for descending in [false, true] {
        let rows = walk(&mut db, &by("price", descending), 7);
        assert_eq!(rows.len(), 60);

        let prices: Vec<_> = rows.iter().map(|r| r.fields["price"].clone()).collect();
        let filled: Vec<Decimal> = prices
            .iter()
            .filter_map(|p| match p {
                FieldValue::Decimal(d) => Some(*d),
                _ => None,
            })
            .collect();
        let mut expected = filled.clone();
        expected.sort();
        if descending {
            expected.reverse();
        }
        assert_eq!(filled, expected);
        // 12 пустых цен — все в конце.
        assert!(prices[48..].iter().all(|p| *p == FieldValue::Null));

        let mut seen: Vec<i64> = rows.iter().map(|r| r.id).collect();
        seen.sort();
        assert_eq!(seen, (1..=60).collect::<Vec<_>>());
    }
}

#[test]
fn search_is_literal() {
    let mut db = memory_db();
    let cat = goods();
    cat.create_table(&mut db).unwrap();
    for name in ["скидка 50%", "a_b", "abc", "50 кг"] {
        cat.insert(&mut db, &fields(&[("name", text(name))]))
            .unwrap();
    }
    let found = |db: &mut SqliteDriver, s: &str| -> Vec<FieldValue> {
        let query = ListQuery {
            search: Some(s.into()),
            ..ListQuery::default()
        };
        let page = list(db, &goods(), &query, None, 10).unwrap();
        page.rows.iter().map(|r| r.fields["name"].clone()).collect()
    };
    assert_eq!(found(&mut db, "%"), vec![text("скидка 50%")]);
    assert_eq!(found(&mut db, "_"), vec![text("a_b")]);
    assert_eq!(
        found(&mut db, "50"),
        vec![text("скидка 50%"), text("50 кг")]
    );
    assert_eq!(found(&mut db, "нет такого"), Vec::<FieldValue>::new());
}

#[test]
fn bad_requests() {
    let mut db = db_with(3);
    let src = goods();
    assert!(list(&mut db, &src, &by("color", false), None, 10).is_err());
    assert!(list(&mut db, &src, &ListQuery::default(), None, 0).is_err());
    assert!(list(&mut db, &src, &ListQuery::default(), None, MAX_WINDOW + 1).is_err());
    assert!(list(&mut db, &src, &ListQuery::default(), Some("мусор"), 10).is_err());
    // Ключ от сортировки по id не подходит к сортировке по цене.
    let page = list(&mut db, &src, &ListQuery::default(), None, 1).unwrap();
    let key = page.next_key.unwrap();
    assert!(list(&mut db, &src, &by("price", false), Some(&key), 10).is_err());
}

#[test]
fn seek_lands_on_exact_row() {
    let mut db = db_with(2500);
    let src = goods();
    for query in [ListQuery::default(), by("name", true), by("price", false)] {
        let all = walk(&mut db, &query, MAX_WINDOW);
        let marks = Milestones::build(&mut db, &src, &query, 100).unwrap();
        assert_eq!(marks.total(), 2500);

        for position in [0u64, 1, 99, 100, 1234, 2490] {
            let page = marks.seek(&mut db, &src, position, 10).unwrap();
            let start = usize::try_from(position).unwrap();
            let expected: Vec<i64> = all[start..(start + 10).min(2500)]
                .iter()
                .map(|r| r.id)
                .collect();
            assert_eq!(ids(&page), expected, "{query:?} @ {position}");
            assert_eq!(page.total, Some(2500));
        }
        assert!(marks.seek(&mut db, &src, 2500, 10).unwrap().rows.is_empty());
    }
}

#[test]
fn seek_then_next_page_continues() {
    let mut db = db_with(500);
    let src = goods();
    let query = by("name", false);
    let marks = Milestones::build(&mut db, &src, &query, 100).unwrap();
    let page = marks.seek(&mut db, &src, 250, 20).unwrap();
    let next = list(&mut db, &src, &query, page.next_key.as_deref(), 20).unwrap();
    assert_eq!(page.rows.last().unwrap().fields["name"], text("item 270"));
    assert_eq!(next.rows.first().unwrap().fields["name"], text("item 271"));
}

#[test]
fn documents_are_listed_with_posted_flag() {
    use crate::object::document::TablePart;
    let mut db = memory_db();
    let kind = DocumentKind::new(
        "receipt",
        vec![FieldDef::new("store", FieldKind::Text)],
        vec![TablePart::new("lines", vec![FieldDef::new("item", FieldKind::Int)]).unwrap()],
    )
    .unwrap();
    kind.create_tables(&mut db).unwrap();
    let parts = std::collections::BTreeMap::new();
    kind.insert(&mut db, &fields(&[("store", text("main"))]), &parts)
        .unwrap();

    let page = list(&mut db, &kind, &ListQuery::default(), None, 10).unwrap();
    assert_eq!(page.rows[0].fields["posted"], FieldValue::Int(0));
    assert_eq!(page.rows[0].fields["store"], text("main"));
    assert_eq!(page.rows[0].fields["number"], FieldValue::Int(1));
    assert_eq!(page.rows[0].fields["deleted"], FieldValue::Int(0));

    let goods_page = list(&mut db_with(1), &goods(), &ListQuery::default(), None, 1).unwrap();
    assert_eq!(goods_page.rows[0].fields["deleted"], FieldValue::Int(0));
}
