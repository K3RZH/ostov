//! Меню строки универсального списка и действия над строкой. Чистая логика без интерфейса:
//! какие пункты показать (вид объекта из метаданных, права пользователя, состояние строки),
//! какой командой ядра выполнить пункт и как поменять строку в списке после ответа ядра.
//! Строка опознаётся по `id`, а не по номеру или названию: номер документа не ключ.

use ostov_api::{Engine, EngineResult, ObjectData, ObjectKind, Rights, Row, Saved, Value};

use crate::object_list::{DELETED_FIELD, NUMBER_FIELD, POSTED_FIELD, VERSION_FIELD};

/// Что ядро делает со строкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowCommand {
    Post,
    Unpost,
    MarkDeleted,
    /// Снять пометку на удаление.
    Restore,
}

/// Пункт меню строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    /// Открыть форму объекта; ядро не зовётся, форма читает объект сама.
    Open,
    Command(RowCommand),
    /// Перечитать список; ядро не зовётся, строки загружаются заново.
    Reload,
}

/// Группа пунктов: между группами в меню стоит черта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Open,
    Posting,
    Deletion,
    List,
}

impl RowAction {
    /// Идентификатор пункта: по нему интерфейс сообщает, что выбрали.
    pub fn id(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Command(RowCommand::Post) => "post",
            Self::Command(RowCommand::Unpost) => "unpost",
            Self::Command(RowCommand::MarkDeleted) => "delete",
            Self::Command(RowCommand::Restore) => "restore",
            Self::Reload => "reload",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            Self::Open,
            Self::Command(RowCommand::Post),
            Self::Command(RowCommand::Unpost),
            Self::Command(RowCommand::MarkDeleted),
            Self::Command(RowCommand::Restore),
            Self::Reload,
        ]
        .into_iter()
        .find(|action| action.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Открыть",
            Self::Command(RowCommand::Post) => "Провести",
            Self::Command(RowCommand::Unpost) => "Отменить проведение",
            Self::Command(RowCommand::MarkDeleted) => "Пометить на удаление",
            Self::Command(RowCommand::Restore) => "Снять пометку",
            Self::Reload => "Обновить список",
        }
    }

    /// Название значка из набора интерфейса.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Open => "edit",
            Self::Command(RowCommand::Post) => "check",
            Self::Command(RowCommand::Unpost | RowCommand::Restore) => "undo",
            Self::Command(RowCommand::MarkDeleted) => "trash",
            Self::Reload => "list",
        }
    }

    /// Опасное действие: пункт красный и перед выполнением нужно подтверждение.
    pub fn is_danger(self) -> bool {
        self == Self::Command(RowCommand::MarkDeleted)
    }

    fn group(self) -> Group {
        match self {
            Self::Open => Group::Open,
            Self::Command(RowCommand::Post | RowCommand::Unpost) => Group::Posting,
            Self::Command(RowCommand::MarkDeleted | RowCommand::Restore) => Group::Deletion,
            Self::Reload => Group::List,
        }
    }
}

/// Состояние строки, от которого зависят пункты меню.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowState {
    pub posted: bool,
    pub deleted: bool,
}

impl RowState {
    pub fn of(row: &Row) -> Self {
        let flag = |name: &str| matches!(row.values.get(name), Some(Value::Bool(true)));
        Self {
            posted: flag(POSTED_FIELD),
            deleted: flag(DELETED_FIELD),
        }
    }
}

/// Пункт меню в том виде, как его показывает таблица.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuEntry {
    pub action: RowAction,
    /// Пункт показан, но недоступен: так ядро отказало бы в действии (проведённый документ
    /// не помечается, помеченный не проводится).
    pub enabled: bool,
    /// Черта над пунктом: начало новой группы.
    pub divider: bool,
}

/// Меню строки. Действие, на которое у пользователя нет права, в меню не показывается;
/// «Провести» и «Отменить проведение» есть только у документов; «Обновить список» всегда.
pub fn row_menu(kind: ObjectKind, rights: Rights, state: RowState) -> Vec<MenuEntry> {
    let mut wanted: Vec<(RowAction, bool)> = Vec::new();
    if rights.read {
        wanted.push((RowAction::Open, true));
    }
    if kind == ObjectKind::Document {
        if state.posted {
            if rights.unpost {
                wanted.push((RowAction::Command(RowCommand::Unpost), true));
            }
        } else if rights.post {
            wanted.push((RowAction::Command(RowCommand::Post), !state.deleted));
        }
    }
    if rights.delete {
        if state.deleted {
            wanted.push((RowAction::Command(RowCommand::Restore), true));
        } else {
            wanted.push((RowAction::Command(RowCommand::MarkDeleted), !state.posted));
        }
    }
    wanted.push((RowAction::Reload, true));

    let mut previous: Option<Group> = None;
    wanted
        .into_iter()
        .map(|(action, enabled)| {
            let divider = previous.is_some_and(|group| group != action.group());
            previous = Some(action.group());
            MenuEntry {
                action,
                enabled,
                divider,
            }
        })
        .collect()
}

/// Подтверждение опасного действия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub title: String,
    pub text: String,
    pub accept: String,
}

/// Что спросить перед действием; `None`: подтверждение не нужно. Спрашиваем только пометку на
/// удаление, остальное безопасно повторить обратным действием.
pub fn confirmation(action: RowAction, label: &str) -> Option<Confirmation> {
    action.is_danger().then(|| Confirmation {
        title: "Пометить на удаление?".to_owned(),
        text: format!("{label}. Запись не стирается: пометку можно снять в любой момент."),
        accept: "Пометить".to_owned(),
    })
}

/// Строка, над которой выбрано действие: всё, что нужно ядру и тексту ошибки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowTarget {
    /// Имя объекта в ядре.
    pub object: String,
    pub id: i64,
    /// Версия строки в списке: по ней ядро ловит чужую правку. `None`, если её нет в строке.
    pub version: Option<i64>,
    /// Как назвать строку человеку: «Расход № 7», название элемента справочника.
    pub label: String,
    pub state: RowState,
}

impl RowTarget {
    pub fn new(object: &str, label: String, row: &Row) -> Self {
        let version = match row.values.get(VERSION_FIELD) {
            Some(Value::Int(version)) => Some(*version),
            _ => None,
        };
        Self {
            object: object.to_owned(),
            id: row.id,
            version,
            label,
            state: RowState::of(row),
        }
    }
}

/// Выполняет команду ядра над строкой. Синхронно: звать из фонового потока.
///
/// # Errors
/// Ошибка ядра отдаётся как есть.
pub fn perform(
    engine: &dyn Engine,
    target: &RowTarget,
    command: RowCommand,
) -> EngineResult<Saved> {
    match command {
        RowCommand::Post => engine.post(&target.object, target.id),
        RowCommand::Unpost => engine.unpost(&target.object, target.id),
        RowCommand::MarkDeleted | RowCommand::Restore => {
            // Версия обычно уже в строке списка; без неё спрашиваем ядро.
            let version = match target.version {
                Some(version) => version,
                None => engine.get(&target.object, target.id)?.version,
            };
            engine.mark_deleted(
                &target.object,
                target.id,
                version,
                command == RowCommand::MarkDeleted,
            )
        }
    }
}

/// Строка списка после того, как ядро выполнило команду: признак и версия меняются на месте,
/// без перечитывания окна.
pub fn updated_row(row: &Row, command: RowCommand, saved: &Saved) -> Row {
    let mut row = row.clone();
    let (field, value) = match command {
        RowCommand::Post => (POSTED_FIELD, true),
        RowCommand::Unpost => (POSTED_FIELD, false),
        RowCommand::MarkDeleted => (DELETED_FIELD, true),
        RowCommand::Restore => (DELETED_FIELD, false),
    };
    row.values.insert(field.to_owned(), Value::Bool(value));
    row.values
        .insert(VERSION_FIELD.to_owned(), Value::Int(saved.version));
    row
}

/// Строка списка по объекту, который записали в форме: поля шапки и служебные (`posted` и номер
/// только у документа). Ключи, которых в объекте нет, остаются от прежней строки.
pub fn row_from_object(old: &Row, kind: ObjectKind, data: &ObjectData) -> Row {
    let mut row = old.clone();
    row.values.extend(
        data.fields
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    row.values
        .insert(DELETED_FIELD.to_owned(), Value::Bool(data.deleted));
    row.values
        .insert(VERSION_FIELD.to_owned(), Value::Int(data.version));
    if kind == ObjectKind::Document {
        row.values
            .insert(POSTED_FIELD.to_owned(), Value::Bool(data.posted));
        if let Some(number) = data.number {
            row.values
                .insert(NUMBER_FIELD.to_owned(), Value::Int(number));
        }
    }
    row
}

/// Текст ошибки действия над строкой: с названием строки, чтобы было ясно, к чему она.
pub fn failure_text(label: &str, message: &str) -> String {
    format!("{label}: {message}")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ostov_api::{EngineError, ErrorCode, Fields};

    use super::*;

    fn rights(post: bool, unpost: bool, delete: bool) -> Rights {
        Rights {
            read: true,
            create: true,
            update: true,
            post,
            unpost,
            delete,
        }
    }

    fn state(posted: bool, deleted: bool) -> RowState {
        RowState { posted, deleted }
    }

    fn actions(entries: &[MenuEntry]) -> Vec<RowAction> {
        entries.iter().map(|entry| entry.action).collect()
    }

    fn open() -> RowAction {
        RowAction::Open
    }
    fn post() -> RowAction {
        RowAction::Command(RowCommand::Post)
    }
    fn unpost() -> RowAction {
        RowAction::Command(RowCommand::Unpost)
    }
    fn mark() -> RowAction {
        RowAction::Command(RowCommand::MarkDeleted)
    }
    fn restore() -> RowAction {
        RowAction::Command(RowCommand::Restore)
    }

    #[test]
    fn draft_document_can_be_posted_or_marked_for_deletion() {
        let menu = row_menu(ObjectKind::Document, Rights::ALL, state(false, false));
        assert_eq!(actions(&menu), [open(), post(), mark(), RowAction::Reload]);
        assert!(menu.iter().all(|entry| entry.enabled));
    }

    #[test]
    fn posted_document_offers_unpost_and_cannot_be_marked() {
        let menu = row_menu(ObjectKind::Document, Rights::ALL, state(true, false));
        assert_eq!(
            actions(&menu),
            [open(), unpost(), mark(), RowAction::Reload]
        );
        // Ядро не помечает проведённый документ: пункт виден, но недоступен.
        let enabled: Vec<bool> = menu.iter().map(|entry| entry.enabled).collect();
        assert_eq!(enabled, [true, true, false, true]);
    }

    #[test]
    fn deleted_document_offers_restore_and_cannot_be_posted() {
        let menu = row_menu(ObjectKind::Document, Rights::ALL, state(false, true));
        assert_eq!(
            actions(&menu),
            [open(), post(), restore(), RowAction::Reload]
        );
        let enabled: Vec<bool> = menu.iter().map(|entry| entry.enabled).collect();
        assert_eq!(enabled, [true, false, true, true]);
    }

    #[test]
    fn catalog_has_no_posting_actions() {
        let menu = row_menu(ObjectKind::Catalog, Rights::ALL, state(false, false));
        assert_eq!(actions(&menu), [open(), mark(), RowAction::Reload]);
        let menu = row_menu(ObjectKind::Catalog, Rights::ALL, state(false, true));
        assert_eq!(actions(&menu), [open(), restore(), RowAction::Reload]);
    }

    #[test]
    fn action_without_the_right_is_not_shown() {
        let menu = row_menu(
            ObjectKind::Document,
            rights(false, true, false),
            state(false, false),
        );
        assert_eq!(actions(&menu), [open(), RowAction::Reload]);
        let menu = row_menu(
            ObjectKind::Document,
            rights(false, true, false),
            state(true, false),
        );
        assert_eq!(actions(&menu), [open(), unpost(), RowAction::Reload]);
        let menu = row_menu(
            ObjectKind::Document,
            rights(true, false, true),
            state(true, false),
        );
        assert_eq!(actions(&menu), [open(), mark(), RowAction::Reload]);
        let menu = row_menu(
            ObjectKind::Catalog,
            rights(true, true, false),
            state(false, true),
        );
        assert_eq!(actions(&menu), [open(), RowAction::Reload]);
    }

    #[test]
    fn open_needs_the_read_right() {
        let no_read = Rights {
            read: false,
            ..Rights::ALL
        };
        let menu = row_menu(ObjectKind::Document, no_read, state(false, false));
        assert!(!actions(&menu).contains(&open()));
        let menu = row_menu(ObjectKind::Document, Rights::ALL, state(false, false));
        assert_eq!(actions(&menu).first(), Some(&open()));
        assert!(!open().is_danger());
        assert!(confirmation(open(), "Расход № 7").is_none());
    }

    #[test]
    fn reload_is_always_there_even_without_any_right() {
        let none = Rights::default();
        for kind in [ObjectKind::Catalog, ObjectKind::Document] {
            let menu = row_menu(kind, none, state(false, false));
            assert_eq!(actions(&menu), [RowAction::Reload]);
            assert!(!menu[0].divider);
        }
    }

    #[test]
    fn divider_stands_between_groups_only() {
        let menu = row_menu(ObjectKind::Document, Rights::ALL, state(false, false));
        let dividers: Vec<bool> = menu.iter().map(|entry| entry.divider).collect();
        assert_eq!(dividers, [false, true, true, true]);
        let menu = row_menu(ObjectKind::Catalog, Rights::ALL, state(false, false));
        let dividers: Vec<bool> = menu.iter().map(|entry| entry.divider).collect();
        assert_eq!(dividers, [false, true, true]);
    }

    #[test]
    fn only_marking_for_deletion_is_dangerous_and_asks_to_confirm() {
        for action in [open(), post(), unpost(), restore(), RowAction::Reload] {
            assert!(!action.is_danger());
            assert!(confirmation(action, "Расход № 7").is_none());
        }
        assert!(mark().is_danger());
        let ask = confirmation(mark(), "Расход № 7").expect("пометку нужно подтвердить");
        assert!(ask.text.starts_with("Расход № 7"));
    }

    #[test]
    fn action_ids_round_trip_and_unknown_id_is_ignored() {
        for action in [
            open(),
            post(),
            unpost(),
            mark(),
            restore(),
            RowAction::Reload,
        ] {
            assert_eq!(RowAction::from_id(action.id()), Some(action));
        }
        assert_eq!(RowAction::from_id("purge"), None);
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
    fn target_takes_id_version_and_state_from_the_row() {
        let target = RowTarget::new(
            "issue",
            "Расход № 7".to_owned(),
            &row(
                42,
                &[
                    ("number", Value::Int(7)),
                    ("posted", Value::Bool(true)),
                    ("deleted", Value::Bool(false)),
                    ("version", Value::Int(3)),
                ],
            ),
        );
        assert_eq!((target.id, target.version), (42, Some(3)));
        assert_eq!(target.state, state(true, false));
        let bare = RowTarget::new("goods", String::new(), &row(1, &[]));
        assert_eq!((bare.version, bare.state), (None, RowState::default()));
    }

    /// Ядро-двойник: запоминает вызовы.
    #[derive(Default)]
    struct FakeEngine {
        calls: Mutex<Vec<String>>,
        stored_version: i64,
    }

    impl FakeEngine {
        fn saved(&self, call: String) -> EngineResult<Saved> {
            self.calls.lock().unwrap().push(call);
            Ok(Saved {
                id: 42,
                version: 9,
                number: None,
            })
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Engine for FakeEngine {
        fn post(&self, document: &str, id: i64) -> EngineResult<Saved> {
            self.saved(format!("post {document} {id}"))
        }
        fn unpost(&self, document: &str, id: i64) -> EngineResult<Saved> {
            self.saved(format!("unpost {document} {id}"))
        }
        fn mark_deleted(
            &self,
            object: &str,
            id: i64,
            version: i64,
            deleted: bool,
        ) -> EngineResult<Saved> {
            self.saved(format!("mark_deleted {object} {id} v{version} {deleted}"))
        }
        fn get(&self, object: &str, id: i64) -> EngineResult<ostov_api::ObjectData> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("get {object} {id}"));
            Ok(ostov_api::ObjectData {
                id: Some(id),
                version: self.stored_version,
                fields: Fields::new(),
                ..Default::default()
            })
        }
    }

    fn target(version: Option<i64>) -> RowTarget {
        RowTarget {
            object: "issue".to_owned(),
            id: 42,
            version,
            label: "Расход № 7".to_owned(),
            state: RowState::default(),
        }
    }

    #[test]
    fn commands_reach_the_core_by_row_id() {
        let engine = FakeEngine::default();
        perform(&engine, &target(Some(3)), RowCommand::Post).unwrap();
        perform(&engine, &target(Some(3)), RowCommand::Unpost).unwrap();
        assert_eq!(engine.calls(), ["post issue 42", "unpost issue 42"]);
    }

    #[test]
    fn marking_uses_the_version_from_the_list_row() {
        let engine = FakeEngine::default();
        perform(&engine, &target(Some(3)), RowCommand::MarkDeleted).unwrap();
        perform(&engine, &target(Some(3)), RowCommand::Restore).unwrap();
        assert_eq!(
            engine.calls(),
            [
                "mark_deleted issue 42 v3 true",
                "mark_deleted issue 42 v3 false"
            ]
        );
    }

    #[test]
    fn marking_asks_the_core_for_the_version_when_the_row_has_none() {
        let engine = FakeEngine {
            stored_version: 5,
            ..FakeEngine::default()
        };
        perform(&engine, &target(None), RowCommand::MarkDeleted).unwrap();
        assert_eq!(
            engine.calls(),
            ["get issue 42", "mark_deleted issue 42 v5 true"]
        );
    }

    #[test]
    fn core_refusal_is_passed_on_as_is() {
        struct Refusing;
        impl Engine for Refusing {
            fn post(&self, _: &str, _: i64) -> EngineResult<Saved> {
                Err(EngineError::new(
                    ErrorCode::Validation,
                    "не хватает остатка",
                ))
            }
        }
        let error = perform(&Refusing, &target(Some(1)), RowCommand::Post).unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(
            failure_text("Расход № 7", &error.message),
            "Расход № 7: не хватает остатка"
        );
    }

    #[test]
    fn updated_row_changes_the_flag_and_version_and_keeps_the_rest() {
        let before = row(
            42,
            &[
                ("number", Value::Int(7)),
                ("posted", Value::Bool(false)),
                ("deleted", Value::Bool(false)),
                ("version", Value::Int(3)),
            ],
        );
        let saved = Saved {
            id: 42,
            version: 4,
            number: None,
        };
        let posted = updated_row(&before, RowCommand::Post, &saved);
        assert_eq!(posted.values["posted"], Value::Bool(true));
        assert_eq!(posted.values["deleted"], Value::Bool(false));
        assert_eq!(posted.values["version"], Value::Int(4));
        assert_eq!(posted.values["number"], Value::Int(7));
        assert_eq!(posted.id, 42);

        let unposted = updated_row(&posted, RowCommand::Unpost, &saved);
        assert_eq!(unposted.values["posted"], Value::Bool(false));
        let marked = updated_row(&before, RowCommand::MarkDeleted, &saved);
        assert_eq!(marked.values["deleted"], Value::Bool(true));
        assert_eq!(
            updated_row(&marked, RowCommand::Restore, &saved).values["deleted"],
            Value::Bool(false)
        );
    }

    #[test]
    fn updated_row_adds_missing_flag_and_version() {
        let saved = Saved {
            id: 1,
            version: 2,
            number: None,
        };
        let updated = updated_row(&row(1, &[]), RowCommand::MarkDeleted, &saved);
        assert_eq!(updated.values["deleted"], Value::Bool(true));
        assert_eq!(updated.values["version"], Value::Int(2));
        assert_eq!(RowState::of(&updated), state(false, true));
    }

    #[test]
    fn row_from_object_takes_fields_flags_and_version_and_keeps_other_keys() {
        let old = row(
            7,
            &[
                ("number", Value::Int(3)),
                ("posted", Value::Bool(false)),
                ("deleted", Value::Bool(false)),
                ("version", Value::Int(1)),
                ("extra", Value::Text("осталось".to_owned())),
                ("date", Value::Date("2026-01-01".to_owned())),
            ],
        );
        let data = ObjectData {
            id: Some(7),
            version: 5,
            number: Some(3),
            posted: true,
            deleted: false,
            fields: [("date".to_owned(), Value::Date("2026-02-10".to_owned()))].into(),
            ..ObjectData::default()
        };
        let fresh = row_from_object(&old, ObjectKind::Document, &data);
        assert_eq!(fresh.id, 7);
        assert_eq!(fresh.values["date"], Value::Date("2026-02-10".to_owned()));
        assert_eq!(fresh.values["posted"], Value::Bool(true));
        assert_eq!(fresh.values["version"], Value::Int(5));
        assert_eq!(fresh.values["number"], Value::Int(3));
        assert_eq!(fresh.values["extra"], Value::Text("осталось".to_owned()));
    }

    #[test]
    fn row_of_catalog_gets_no_posting_keys() {
        let data = ObjectData {
            id: Some(1),
            version: 2,
            posted: true,
            fields: [("name".to_owned(), Value::Text("Мука".to_owned()))].into(),
            ..ObjectData::default()
        };
        let fresh = row_from_object(&row(1, &[]), ObjectKind::Catalog, &data);
        assert!(!fresh.values.contains_key("posted"));
        assert!(!fresh.values.contains_key("number"));
        assert_eq!(fresh.values["deleted"], Value::Bool(false));
        assert_eq!(fresh.values["name"], Value::Text("Мука".to_owned()));
    }
}
