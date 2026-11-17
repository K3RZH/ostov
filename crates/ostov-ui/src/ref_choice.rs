//! Выбор значения ссылки из справочника: можно ли выбирать, что спросить у ядра и как назвать
//! варианты. Ядро зовётся одним списком (`Engine::list`) с поиском по подстроке, и в память
//! попадает только первая порция совпадений: большой справочник целиком не грузится.

use ostov_api::{
    Engine, EngineResult, FieldDef, FieldKind, ListQuery, ObjectKind, ObjectMeta, Row, Value,
};

use crate::object_list::NUMBER_FIELD;
use crate::values::display;

/// Сколько вариантов показывает выбор за раз: больше человек не просмотрит, дальше он уточняет
/// поиск.
pub const PICK_LIMIT: u32 = 50;

/// Вариант выбора: элемент справочника и его название.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickOption {
    pub id: i64,
    pub title: String,
}

/// Первая порция совпадений и сколько их всего (если ядро сказало).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickPage {
    pub options: Vec<PickOption>,
    pub total: Option<u64>,
}

/// Справочник, из которого выбирают значение поля, если выбирать можно. Ошибка: почему нельзя,
/// текстом для человека. Объект, который пользователю читать нельзя, ядро в метаданные не отдаёт,
/// поэтому его название неизвестно: говорим о поле.
///
/// # Errors
/// У поля нет цели, цели нет в метаданных базы или на неё нет права чтения.
pub fn target_of<'a>(
    objects: &'a [ObjectMeta],
    target: Option<&str>,
    field_title: &str,
) -> Result<&'a ObjectMeta, String> {
    let meta = target.and_then(|target| objects.iter().find(|meta| meta.name == target));
    match meta {
        Some(meta) if meta.rights.read => Ok(meta),
        Some(meta) => Err(format!(
            "Нет права читать «{}»: выбор недоступен.",
            meta.title
        )),
        None => Err(format!(
            "Нет доступа к данным для поля «{field_title}»: выбор недоступен."
        )),
    }
}

/// Из выбора можно создать новый элемент: это справочник и есть право создавать.
pub fn can_add(meta: &ObjectMeta) -> bool {
    meta.kind == ObjectKind::Catalog && meta.rights.create
}

/// Поле, которым называют элемент справочника: первое текстовое (так же ссылку называет ядро).
fn title_field(meta: &ObjectMeta) -> Option<&FieldDef> {
    meta.fields.iter().find(|def| def.kind == FieldKind::Text)
}

/// Запрос первой порции: без помеченных на удаление, по названию, с поиском по подстроке.
pub fn query(meta: &ObjectMeta, search: &str) -> ListQuery {
    ListQuery {
        object: meta.name.clone(),
        order: title_field(meta).map(|def| def.name.clone()),
        search: Some(search.trim())
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
        deleted: Some(false),
        size: PICK_LIMIT,
        ..ListQuery::default()
    }
}

/// Название варианта: у элемента справочника его текстовое поле, у документа «Расход №5».
/// Пусто: название объекта и номер.
pub fn option_title(meta: &ObjectMeta, row: &Row) -> String {
    let named = match meta.kind {
        ObjectKind::Catalog => title_field(meta)
            .and_then(|def| row.values.get(&def.name))
            .map(|value| display(value, FieldKind::Text))
            .filter(|title| !title.trim().is_empty()),
        ObjectKind::Document => match row.values.get(NUMBER_FIELD) {
            Some(Value::Int(number)) => Some(format!("{} №{number}", meta.title)),
            _ => None,
        },
    };
    named.unwrap_or_else(|| format!("{} №{}", meta.title, row.id))
}

/// Первая порция вариантов из ядра. Синхронно: звать из фонового потока.
///
/// # Errors
/// Ошибка ядра отдаётся как есть.
pub fn find(engine: &dyn Engine, meta: &ObjectMeta, search: &str) -> EngineResult<PickPage> {
    let page = engine.list(&query(meta, search))?;
    Ok(PickPage {
        options: page
            .rows
            .iter()
            .map(|row| PickOption {
                id: row.id,
                title: option_title(meta, row),
            })
            .collect(),
        total: page.total,
    })
}

/// Подпись под вариантами: пусто, ничего не нашлось или показана только первая порция.
pub fn note(page: &PickPage, searching: bool) -> String {
    let shown = page.options.len();
    if shown == 0 {
        return if searching {
            "Ничего не найдено. Измените поиск.".to_owned()
        } else {
            "Здесь пока пусто.".to_owned()
        };
    }
    match page.total {
        Some(total) if total > u64::try_from(shown).unwrap_or(u64::MAX) => {
            format!("Показаны первые {shown} из {total}: уточните поиск.")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use ostov_api::{Meta, Rights};

    use super::*;

    fn demo_objects() -> Vec<ObjectMeta> {
        let config = ostov_meta::demo().unwrap_or_else(|errors| unreachable!("{errors:?}"));
        Meta::new(&config).objects
    }

    fn object(name: &str) -> ObjectMeta {
        demo_objects()
            .into_iter()
            .find(|object| object.name == name)
            .unwrap_or_else(|| unreachable!("в демо-конфигурации нет {name}"))
    }

    fn row(id: i64, values: &[(&str, Value)]) -> Row {
        Row {
            id,
            values: values
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        }
    }

    #[test]
    fn readable_target_is_chosen_from() {
        let objects = demo_objects();
        let target = target_of(&objects, Some("warehouses"), "Склад").unwrap();
        assert_eq!(target.title, "Склады");
        assert!(can_add(target));
    }

    #[test]
    fn missing_or_closed_target_explains_why() {
        let mut objects = demo_objects();
        assert_eq!(
            target_of(&objects, Some("nowhere"), "Склад").unwrap_err(),
            "Нет доступа к данным для поля «Склад»: выбор недоступен."
        );
        assert!(target_of(&objects, None, "Склад").is_err());
        if let Some(meta) = objects.iter_mut().find(|meta| meta.name == "warehouses") {
            meta.rights = Rights::default();
        }
        assert_eq!(
            target_of(&objects, Some("warehouses"), "Склад").unwrap_err(),
            "Нет права читать «Склады»: выбор недоступен."
        );
    }

    #[test]
    fn adding_needs_a_catalog_and_the_right_to_create() {
        let mut goods = object("goods");
        goods.rights.create = false;
        assert!(!can_add(&goods));
        assert!(!can_add(&object("issue")));
    }

    #[test]
    fn query_takes_the_first_portion_by_title_without_deleted() {
        let query = query(&object("goods"), "  мук ");
        assert_eq!(query.object, "goods");
        assert_eq!(query.order.as_deref(), Some("name"));
        assert_eq!(query.search.as_deref(), Some("мук"));
        assert_eq!(query.deleted, Some(false));
        assert_eq!(query.size, PICK_LIMIT);
        assert_eq!(super::query(&object("goods"), "   ").search, None);
    }

    #[test]
    fn options_are_named_by_title_field_or_number() {
        let goods = object("goods");
        let named = row(3, &[("name", Value::Text("Мука".to_owned()))]);
        assert_eq!(option_title(&goods, &named), "Мука");
        assert_eq!(option_title(&goods, &row(4, &[])), "Товары №4");
        let issue = object("issue");
        assert_eq!(
            option_title(&issue, &row(9, &[("number", Value::Int(12))])),
            "Расход №12"
        );
        assert_eq!(option_title(&issue, &row(9, &[])), "Расход №9");
    }

    #[test]
    fn note_tells_about_empty_result_and_the_limit() {
        let option = PickOption {
            id: 1,
            title: "Мука".to_owned(),
        };
        let empty = PickPage::default();
        assert_eq!(note(&empty, true), "Ничего не найдено. Измените поиск.");
        assert_eq!(note(&empty, false), "Здесь пока пусто.");
        let all = PickPage {
            options: vec![option.clone()],
            total: Some(1),
        };
        assert_eq!(note(&all, false), "");
        let first = PickPage {
            options: vec![option],
            total: Some(120),
        };
        assert_eq!(
            note(&first, true),
            "Показаны первые 1 из 120: уточните поиск."
        );
    }

    /// Поиск идёт в ядре: на демо-базе находятся товары по подстроке, порция не больше предела.
    #[test]
    fn search_reaches_the_core_and_stays_within_the_limit() {
        let path =
            std::env::temp_dir().join(format!("ostov-ui-ref-choice-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let engine = ostov_core::LocalConnector::default()
            .create_demo(path.to_str().unwrap_or_default(), 20)
            .unwrap();
        let goods = object("goods");
        let all = find(&*engine, &goods, "").unwrap();
        assert!(!all.options.is_empty());
        assert!(all.options.len() <= usize::try_from(PICK_LIMIT).unwrap());
        let first = all.options[0].title.clone();
        let piece: String = first.chars().take(3).collect();
        let found = find(&*engine, &goods, &piece.to_lowercase()).unwrap();
        assert!(found.options.iter().any(|option| option.title == first));
        let none = find(&*engine, &goods, "щщщщщ").unwrap();
        assert!(none.options.is_empty());
        drop(engine);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite.lock"));
    }
}
