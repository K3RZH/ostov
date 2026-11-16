//! Состояние формы объекта: поля шапки справочника или документа, пока их правят.
//!
//! Чистая логика без интерфейса и без ядра. Форма строится из описания объекта в метаданных
//! (`ObjectMeta`) и значений из ядра (`ObjectData`), либо пустой для нового объекта. Правки идут
//! текстом: разбор по типу поля делает `values::parse`, ошибка остаётся у поля вместе с введённым
//! текстом. Перед записью обязательные поля проверяются, а значения собираются обратно в
//! `ObjectData`. Табличные части ведёт `form_table::Parts`: строки правятся в окне строки и
//! уходят в ядро вместе с шапкой.

use std::fmt;

use ostov_api::{
    FieldDef, FieldKind, ObjectData, ObjectKind, ObjectMeta, PostAction, Rights, Saved, Value,
};

use crate::form_table::{InvalidRow, Parts};
use crate::values::{display, parse, parse_iso_date};

/// Что написать у обязательного поля без значения: по виду поля.
const REQUIRED_TEXT: &str = "Заполните поле.";
const REQUIRED_DATE: &str = "Укажите дату.";
const REQUIRED_REFERENCE: &str = "Выберите значение.";

/// Одно поле шапки: описание из метаданных и то, что в нём сейчас.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub name: String,
    pub title: String,
    pub kind: FieldKind,
    /// Для ссылки: на какой объект она указывает.
    pub target: Option<String>,
    pub required: bool,
    /// Поле считает ядро: пользователь его не правит.
    pub read_only: bool,
    /// Текст в поле: то, что ввёл пользователь, даже если он не разобрался.
    text: String,
    /// Последнее принятое значение. Ссылка это `Value::Ref` с `id` и названием.
    value: Value,
    /// Значение, с которым форма открылась или которое записало ядро: от него считается «изменено».
    original: Value,
    error: Option<String>,
}

impl FormField {
    pub(crate) fn new(def: &FieldDef, value: Value) -> Self {
        Self {
            name: def.name.clone(),
            title: def.title.clone(),
            kind: def.kind,
            target: def.target.clone(),
            required: def.required,
            read_only: def.read_only,
            text: display(&value, def.kind),
            original: value.clone(),
            value,
            error: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Принятое значение поля (последнее, которое удалось разобрать).
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Ошибка разбора или проверки обязательности; текст для человека.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Поле правится текстом: не вычисляемое и не ссылка (ссылку выбирают из справочника).
    pub fn accepts_text(&self) -> bool {
        !self.read_only && self.kind != FieldKind::Ref
    }

    /// Значение поля выбирают из справочника: ссылка, которую не считает ядро.
    pub fn accepts_choice(&self) -> bool {
        !self.read_only && self.kind == FieldKind::Ref
    }

    /// `id` выбранного элемента, если поле ссылка и оно заполнено.
    pub fn reference_id(&self) -> Option<i64> {
        match self.value {
            Value::Ref { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Поле отличается от открытого: значение другое или введённое не разобралось.
    pub fn is_changed(&self) -> bool {
        self.error.is_some() || self.value != self.original
    }

    /// Дата поля (год, месяц, день), если в поле лежит дата.
    pub fn date(&self) -> Option<(u16, u8, u8)> {
        match &self.value {
            Value::Date(iso) => parse_iso_date(iso),
            _ => None,
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    fn required_message(&self) -> &'static str {
        match self.kind {
            FieldKind::Date => REQUIRED_DATE,
            FieldKind::Ref => REQUIRED_REFERENCE,
            FieldKind::Int | FieldKind::Text | FieldKind::Decimal { .. } => REQUIRED_TEXT,
        }
    }

    pub(crate) fn accept_value(&mut self, value: Value) {
        self.text = display(&value, self.kind);
        self.value = value;
        self.error = None;
    }

    /// Правка текстом: разобралось — значение принято, нет — ошибка и набранный текст остаются.
    pub(crate) fn apply_text(&mut self, text: &str) {
        match parse(text, self.kind) {
            Ok(value) => {
                self.value = value;
                self.error = None;
            }
            Err(message) => self.error = Some(message),
        }
        text.clone_into(&mut self.text);
    }

    /// Выбор ссылки: `id` и название; `None` очищает поле.
    pub(crate) fn choose(&mut self, chosen: Option<(i64, String)>) {
        self.accept_value(match chosen {
            Some((id, title)) => Value::Ref {
                id,
                title: Some(title),
            },
            None => Value::Null,
        });
    }

    /// Проверка перед записью: пустое обязательное поле получает ошибку. `true`: у поля ошибка
    /// (эта или прежняя ошибка разбора).
    pub(crate) fn check_required(&mut self) -> bool {
        if self.required && !self.read_only && self.is_empty() && self.error.is_none() {
            self.error = Some(self.required_message().to_owned());
        }
        self.error.is_some()
    }

    /// Текущее значение как его принимает ядро: у ссылки название не нужно.
    fn value_for_core(&self) -> Value {
        core_value(&self.value)
    }
}

/// Значение для ядра: у ссылки только `id`, название ядро подставит само.
pub(crate) fn core_value(value: &Value) -> Value {
    match value {
        Value::Ref { id, .. } => Value::Ref {
            id: *id,
            title: None,
        },
        other => other.clone(),
    }
}

/// Состояние объекта для подписи на форме.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Ещё не записан в ядро.
    New,
    /// Документ записан, но не проведён.
    Draft,
    Posted,
    /// Документ проведён, но в форме есть правки: чтобы они вступили в силу, его надо перепровести.
    PostedChanged,
    /// Помечен на удаление (СТ-30).
    Deleted,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "Не записан",
            Self::Draft => "Черновик",
            Self::Posted => "Проведён",
            Self::PostedChanged => "Проведён, изменён",
            Self::Deleted => "Помечен на удаление",
        }
    }
}

/// Запись не началась: в форме есть поля или строки табличных частей с ошибкой (они уже
/// отмечены).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidForm {
    /// Сколько полей шапки с ошибкой.
    pub fields: usize,
    /// Сколько строк табличных частей с незаполненными обязательными колонками.
    pub rows: usize,
}

impl fmt::Display for InvalidForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.fields, self.rows) {
            (fields, 0) => write!(f, "Не записано: исправьте отмеченные поля ({fields})."),
            (0, rows) => write!(f, "Не записано: заполните отмеченные строки ({rows})."),
            (fields, rows) => write!(
                f,
                "Не записано: исправьте отмеченные поля ({fields}) и строки ({rows})."
            ),
        }
    }
}

impl std::error::Error for InvalidForm {}

/// Форма одного объекта: шапка, версия для проверки конфликта, признаки документа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormState {
    object: String,
    object_title: String,
    kind: ObjectKind,
    rights: Rights,
    id: Option<i64>,
    version: i64,
    number: Option<i64>,
    posted: bool,
    deleted: bool,
    fields: Vec<FormField>,
    parts: Parts,
}

impl FormState {
    /// Форма нового объекта: поля пусты, значений по умолчанию ядро пока не отдаёт.
    pub fn new_object(meta: &ObjectMeta) -> Self {
        Self::build(meta, None)
    }

    /// Форма записанного объекта по ответу ядра. Поля, которых в ответе нет, пусты; лишние
    /// значения, которых нет в метаданных, форма не показывает.
    pub fn from_data(meta: &ObjectMeta, data: &ObjectData) -> Self {
        Self::build(meta, Some(data))
    }

    fn build(meta: &ObjectMeta, data: Option<&ObjectData>) -> Self {
        let fields = meta
            .fields
            .iter()
            .map(|def| {
                let value = data
                    .and_then(|data| data.fields.get(&def.name))
                    .cloned()
                    .unwrap_or(Value::Null);
                FormField::new(def, value)
            })
            .collect();
        Self {
            object: meta.name.clone(),
            object_title: meta.title.clone(),
            kind: meta.kind,
            rights: meta.rights,
            id: data.and_then(|data| data.id),
            version: data.map_or(0, |data| data.version),
            number: data.and_then(|data| data.number),
            posted: data.is_some_and(|data| data.posted),
            deleted: data.is_some_and(|data| data.deleted),
            fields,
            parts: Parts::new(&meta.table_parts, data.map(|data| &data.tables)),
        }
    }

    /// Имя объекта в ядре.
    pub fn object(&self) -> &str {
        &self.object
    }

    pub fn kind(&self) -> ObjectKind {
        self.kind
    }

    /// `None`: объект ещё не записан.
    pub fn id(&self) -> Option<i64> {
        self.id
    }

    /// Версия, с которой форма открыта: уходит в ядро при записи, по ней ядро находит
    /// конфликт (объект изменили после чтения).
    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn number(&self) -> Option<i64> {
        self.number
    }

    pub fn is_new(&self) -> bool {
        self.id.is_none()
    }

    pub fn is_posted(&self) -> bool {
        self.posted
    }

    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    /// Табличные части объекта и открытое окно строки.
    pub fn parts(&self) -> &Parts {
        &self.parts
    }

    /// Как объект называется во вкладке и в заголовке: у документа «Расход №5», у элемента
    /// справочника его первое текстовое поле (ядро представления объекта пока не отдаёт).
    /// Считается по записанному, а не по тому, что сейчас набрано.
    pub fn presentation(&self) -> String {
        let unnamed = || match self.id {
            Some(id) => format!("{} №{id}", self.object_title),
            None => format!("{} (новый)", self.object_title),
        };
        match self.kind {
            ObjectKind::Document => match self.number {
                Some(number) => format!("{} №{number}", self.object_title),
                None => format!("{} (новый)", self.object_title),
            },
            ObjectKind::Catalog => self
                .fields
                .iter()
                .find(|field| field.kind == FieldKind::Text)
                .map(|field| display(&field.original, field.kind))
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(unnamed),
        }
    }

    /// Подпись состояния; у записанного и не помеченного элемента справочника её нет.
    pub fn status(&self) -> Option<Status> {
        if self.deleted {
            return Some(Status::Deleted);
        }
        if self.is_new() {
            return Some(Status::New);
        }
        match (self.kind, self.posted) {
            (ObjectKind::Catalog, _) => None,
            (ObjectKind::Document, false) => Some(Status::Draft),
            (ObjectKind::Document, true) if self.is_modified() => Some(Status::PostedChanged),
            (ObjectKind::Document, true) => Some(Status::Posted),
        }
    }

    /// Пользователь может записывать: новый объект нужно создавать, записанный менять.
    pub fn can_write(&self) -> bool {
        if self.is_new() {
            self.rights.create
        } else {
            self.rights.update
        }
    }

    /// «Записать». Проведённый документ ядро без перепроведения не принимает.
    pub fn can_save(&self) -> bool {
        self.can_write() && !self.posted
    }

    /// «Провести» (у проведённого это «Перепровести»): только документ и только с правом.
    pub fn can_post(&self) -> bool {
        self.kind == ObjectKind::Document && self.rights.post && self.can_write()
    }

    /// «Отменить проведение»: проведённый записанный документ и право на отмену.
    pub fn can_unpost(&self) -> bool {
        self.kind == ObjectKind::Document
            && self.posted
            && !self.is_new()
            && self.rights.unpost
            && self.rights.update
    }

    /// Поле с номером `index` можно править руками: есть право на запись и поле не вычисляемое
    /// и не ссылка.
    pub fn is_editable(&self, index: usize) -> bool {
        self.can_write() && self.fields.get(index).is_some_and(FormField::accepts_text)
    }

    /// Значение поля `index` можно выбрать из справочника: есть право на запись, поле ссылка и
    /// не вычисляемое.
    pub fn can_pick(&self, index: usize) -> bool {
        self.can_write()
            && self
                .fields
                .get(index)
                .is_some_and(FormField::accepts_choice)
    }

    /// В форме есть правки относительно открытого или записанного (в шапке, в строках или в
    /// открытом окне строки).
    pub fn is_modified(&self) -> bool {
        self.fields.iter().any(FormField::is_changed) || self.parts.is_changed()
    }

    /// Правка поля `index` текстом. Разобралось: значение принято, ошибка снята. Не разобралось:
    /// у поля остаётся введённый текст и ошибка, принятое значение прежнее. Поле, которое нельзя
    /// править (вычисляемое, ссылка, нет права или нет такого поля), правка не меняет.
    pub fn edit(&mut self, index: usize, text: &str) {
        if !self.is_editable(index) {
            return;
        }
        if let Some(field) = self.fields.get_mut(index) {
            field.apply_text(text);
        }
    }

    /// Выбор значения в поле-ссылке: `id` и название пары. `None`: поле очищено. Поле, которое
    /// не ссылка, вычисляемое или закрыто правами, не меняется.
    pub fn set_reference(&mut self, index: usize, chosen: Option<(i64, String)>) {
        if !self.can_write() {
            return;
        }
        if let Some(field) = self
            .fields
            .get_mut(index)
            .filter(|field| field.accepts_choice())
        {
            field.choose(chosen);
        }
    }

    /// Выбирает строку `row` табличной части `part` (для «Изменить» и «Удалить»).
    pub fn select_row(&mut self, part: usize, row: usize) {
        self.parts.select(part, row);
    }

    /// «Добавить» строку в табличную часть `part`: открывается пустое окно строки. `false`:
    /// записывать нельзя, такой части нет или окно строки уже открыто.
    pub fn add_row(&mut self, part: usize) -> bool {
        self.can_write() && self.parts.open_new(part)
    }

    /// Открывает строку в окне строки; без права на запись она только показывается.
    pub fn open_row(&mut self, part: usize, row: usize) -> bool {
        let editable = self.can_write();
        self.parts.open_row(part, row, editable)
    }

    /// Правка ячейки `cell` открытой строки текстом.
    pub fn edit_cell(&mut self, cell: usize, text: &str) {
        self.parts.edit_cell(cell, text);
    }

    /// Выбор ссылки в ячейке `cell` открытой строки; `None` очищает ячейку.
    pub fn set_cell_reference(&mut self, cell: usize, chosen: Option<(i64, String)>) {
        self.parts.choose_cell(cell, chosen);
    }

    /// «Готово» в окне строки: строка встаёт в таблицу, если обязательные ячейки заполнены.
    ///
    /// # Errors
    /// В строке есть ячейки с ошибкой; они отмечены, окно остаётся открытым.
    pub fn commit_row(&mut self) -> Result<(), InvalidRow> {
        self.parts.commit()
    }

    /// «Отмена» в окне строки: правки строки пропадают.
    pub fn cancel_row(&mut self) {
        self.parts.cancel();
    }

    /// «Удалить» строку `row` табличной части `part`.
    pub fn remove_row(&mut self, part: usize, row: usize) -> bool {
        self.can_write() && self.parts.remove(part, row)
    }

    /// Проверка перед записью: обязательные поля должны быть заполнены, разбор не должен
    /// падать. Неверные поля помечаются ошибкой; возвращается, сколько их.
    fn check(&mut self) -> usize {
        self.fields
            .iter_mut()
            .map(FormField::check_required)
            .filter(|invalid| *invalid)
            .count()
    }

    /// Проверяет форму и собирает объект для записи в ядро. Версия, номер и признаки документа
    /// берутся с открытия: по версии ядро найдёт конфликт.
    ///
    /// # Errors
    /// В форме есть поле с ошибкой разбора, пустое обязательное поле или строка табличной части
    /// без обязательного значения. Они помечены.
    pub fn to_data(&mut self) -> Result<ObjectData, InvalidForm> {
        let fields = self.check();
        let rows = self.parts.check();
        if fields > 0 || rows > 0 {
            return Err(InvalidForm { fields, rows });
        }
        Ok(ObjectData {
            id: self.id,
            version: self.version,
            number: self.number,
            posted: self.posted,
            deleted: self.deleted,
            fields: self
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.value_for_core()))
                .collect(),
            tables: self.parts.to_core(),
        })
    }

    /// Ядро записало объект, но заново прочитать его не удалось: форма берёт из ответа `id`,
    /// версию и номер, а набранное считает записанным. Без этого повторная запись нового
    /// объекта создала бы второй.
    pub fn apply_saved(&mut self, saved: &Saved, action: PostAction) {
        self.id = Some(saved.id);
        self.version = saved.version;
        self.number = saved.number.or(self.number);
        match action {
            PostAction::Write => {}
            PostAction::Post => self.posted = true,
            PostAction::Unpost => self.posted = false,
        }
        for field in &mut self.fields {
            field.original = field.value.clone();
            field.error = None;
        }
        self.parts.mark_saved();
    }
}

#[cfg(test)]
mod tests {
    use ostov_api::{Fields, Meta};

    use super::*;

    fn warehouse_meta() -> Meta {
        let config = ostov_meta::demo().unwrap_or_else(|errors| unreachable!("{errors:?}"));
        Meta::new(&config)
    }

    fn object_meta(name: &str) -> ObjectMeta {
        warehouse_meta()
            .objects
            .into_iter()
            .find(|object| object.name == name)
            .unwrap_or_else(|| unreachable!("в демо-конфигурации нет {name}"))
    }

    fn field<'a>(form: &'a FormState, name: &str) -> &'a FormField {
        form.fields()
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| unreachable!("нет поля {name}"))
    }

    fn index(form: &FormState, name: &str) -> usize {
        form.fields()
            .iter()
            .position(|field| field.name == name)
            .unwrap_or_else(|| unreachable!("нет поля {name}"))
    }

    fn edit(form: &mut FormState, name: &str, text: &str) {
        let index = index(form, name);
        form.edit(index, text);
    }

    fn reference(id: i64, title: &str) -> Value {
        Value::Ref {
            id,
            title: Some(title.to_owned()),
        }
    }

    /// Поступление, как его отдаёт ядро: шапка и одна строка табличной части.
    fn receipt_data() -> ObjectData {
        let line: Fields = [
            ("goods".to_owned(), reference(5, "Мука")),
            (
                "qty".to_owned(),
                Value::Decimal {
                    units: 2500,
                    scale: 3,
                },
            ),
        ]
        .into();
        ObjectData {
            id: Some(12),
            version: 4,
            number: Some(7),
            posted: false,
            deleted: false,
            fields: [
                ("date".to_owned(), Value::Date("2026-02-10".to_owned())),
                ("supplier".to_owned(), reference(3, "Мельница")),
                ("warehouse".to_owned(), reference(1, "Основной")),
                (
                    "amount".to_owned(),
                    Value::Decimal {
                        units: 123_456,
                        scale: 2,
                    },
                ),
            ]
            .into(),
            tables: [("lines".to_owned(), vec![line])].into(),
        }
    }

    fn loaded_receipt() -> FormState {
        FormState::from_data(&object_meta("receipt"), &receipt_data())
    }

    fn goods_meta() -> ObjectMeta {
        object_meta("goods")
    }

    #[test]
    fn form_follows_meta_fields_in_order() {
        let meta = object_meta("receipt");
        let form = FormState::from_data(&meta, &receipt_data());
        let names: Vec<&str> = form.fields().iter().map(|f| f.name.as_str()).collect();
        let expected: Vec<&str> = meta.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, expected);
        let date = field(&form, "date");
        assert_eq!(date.title, "Дата");
        assert_eq!(date.kind, FieldKind::Date);
        assert!(date.required);
        assert!(!date.read_only);
        let supplier = field(&form, "supplier");
        assert_eq!(supplier.kind, FieldKind::Ref);
        assert_eq!(supplier.target.as_deref(), Some("suppliers"));
        assert!(field(&form, "amount").read_only);
    }

    #[test]
    fn loaded_values_are_shown_as_text() {
        let form = loaded_receipt();
        assert_eq!(field(&form, "date").text(), "10.02.2026");
        assert_eq!(field(&form, "supplier").text(), "Мельница");
        assert_eq!(field(&form, "amount").text(), "1\u{a0}234,56");
        assert_eq!(form.id(), Some(12));
        assert_eq!(form.version(), 4);
        assert_eq!(form.number(), Some(7));
        assert!(!form.is_new());
        assert!(!form.is_modified());
    }

    #[test]
    fn new_object_has_empty_fields_and_no_identity() {
        let form = FormState::new_object(&object_meta("receipt"));
        assert!(form.is_new());
        assert_eq!(form.id(), None);
        assert_eq!(form.version(), 0);
        assert!(form.fields().iter().all(|f| f.text().is_empty()));
        assert!(form.fields().iter().all(|f| *f.value() == Value::Null));
        assert!(!form.is_modified());
    }

    #[test]
    fn data_without_a_field_leaves_it_empty() {
        let mut data = receipt_data();
        data.fields.remove("supplier");
        let form = FormState::from_data(&object_meta("receipt"), &data);
        assert_eq!(*field(&form, "supplier").value(), Value::Null);
        assert_eq!(field(&form, "supplier").text(), "");
    }

    #[test]
    fn editing_text_is_parsed_by_field_kind() {
        let mut form = FormState::from_data(&goods_meta(), &ObjectData::default());
        edit(&mut form, "name", "  Мука  ");
        assert_eq!(
            *field(&form, "name").value(),
            Value::Text("Мука".to_owned())
        );
        // Введённый текст остаётся как набран, обрезает его только значение.
        assert_eq!(field(&form, "name").text(), "  Мука  ");

        let mut form = loaded_receipt();
        edit(&mut form, "date", "15.03.2026");
        assert_eq!(
            *field(&form, "date").value(),
            Value::Date("2026-03-15".to_owned())
        );
        assert_eq!(field(&form, "date").date(), Some((2026, 3, 15)));
        assert_eq!(field(&form, "date").error(), None);
    }

    #[test]
    fn numeric_edit_uses_the_scale_of_the_field() {
        let meta = ObjectMeta {
            fields: vec![
                FieldDef::new("count", FieldKind::Int),
                FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
            ],
            ..goods_meta()
        };
        let mut form = FormState::new_object(&meta);
        edit(&mut form, "count", "12");
        edit(&mut form, "price", "1 234,5");
        assert_eq!(*field(&form, "count").value(), Value::Int(12));
        assert_eq!(
            *field(&form, "price").value(),
            Value::Decimal {
                units: 123_450,
                scale: 2
            }
        );
    }

    #[test]
    fn parse_error_stays_with_the_field_and_keeps_typed_text() {
        let mut form = loaded_receipt();
        edit(&mut form, "date", "31.02");
        let date = field(&form, "date");
        assert_eq!(date.text(), "31.02");
        assert_eq!(date.error(), Some("Введите дату в виде дд.мм.гггг."));
        // Принятое значение прежнее: плохой ввод его не затирает.
        assert_eq!(*date.value(), Value::Date("2026-02-10".to_owned()));
        // Другие поля ошибка не задевает.
        assert_eq!(field(&form, "supplier").error(), None);
    }

    #[test]
    fn correct_input_clears_the_error() {
        let mut form = loaded_receipt();
        edit(&mut form, "date", "31.02");
        edit(&mut form, "date", "01.03.2026");
        assert_eq!(field(&form, "date").error(), None);
        assert_eq!(field(&form, "date").text(), "01.03.2026");
    }

    #[test]
    fn bad_number_is_reported_with_its_scale() {
        let meta = ObjectMeta {
            fields: vec![FieldDef::new("price", FieldKind::Decimal { scale: 2 })],
            ..goods_meta()
        };
        let mut form = FormState::new_object(&meta);
        edit(&mut form, "price", "1,234");
        assert_eq!(
            field(&form, "price").error(),
            Some("Введите число без знака (после запятой не больше 2 знаков).")
        );
        assert_eq!(field(&form, "price").text(), "1,234");
    }

    #[test]
    fn read_only_field_and_reference_ignore_text_edits() {
        let mut form = loaded_receipt();
        edit(&mut form, "amount", "5");
        edit(&mut form, "supplier", "Другой");
        assert_eq!(field(&form, "amount").text(), "1\u{a0}234,56");
        assert_eq!(field(&form, "supplier").text(), "Мельница");
        assert!(!form.is_modified());
        // Номера за пределами формы тоже ничего не ломают.
        form.edit(99, "x");
        assert!(!form.is_modified());
    }

    #[test]
    fn modified_follows_the_value_not_the_keystrokes() {
        let mut form = loaded_receipt();
        assert!(!form.is_modified());
        edit(&mut form, "date", "11.02.2026");
        assert!(form.is_modified());
        assert!(field(&form, "date").is_changed());
        assert!(!field(&form, "supplier").is_changed());
        // Вернули прежнюю дату: изменений нет.
        edit(&mut form, "date", "10.02.2026");
        assert!(!form.is_modified());
    }

    #[test]
    fn text_that_does_not_parse_counts_as_modified() {
        let mut form = loaded_receipt();
        edit(&mut form, "date", "31.02");
        assert!(form.is_modified());
    }

    #[test]
    fn required_fields_are_checked_before_saving() {
        let mut form = FormState::new_object(&object_meta("receipt"));
        let invalid = form.to_data().unwrap_err();
        // Дата, поставщик и склад обязательные, сумма вычисляемая.
        assert_eq!(invalid, InvalidForm { fields: 3, rows: 0 });
        assert_eq!(field(&form, "date").error(), Some("Укажите дату."));
        assert_eq!(field(&form, "supplier").error(), Some("Выберите значение."));
        assert_eq!(field(&form, "amount").error(), None);

        let mut goods = FormState::new_object(&goods_meta());
        assert!(goods.to_data().is_err());
        assert_eq!(field(&goods, "name").error(), Some("Заполните поле."));
        // Единица измерения не обязательная.
        assert_eq!(field(&goods, "unit").error(), None);
    }

    #[test]
    fn filling_a_required_field_removes_its_error() {
        let mut form = FormState::new_object(&goods_meta());
        assert!(form.to_data().is_err());
        edit(&mut form, "name", "Мука");
        assert_eq!(field(&form, "name").error(), None);
        assert!(form.to_data().is_ok());
    }

    #[test]
    fn saving_is_refused_while_a_field_has_a_parse_error() {
        let mut form = loaded_receipt();
        edit(&mut form, "date", "31.02");
        assert_eq!(form.to_data(), Err(InvalidForm { fields: 1, rows: 0 }));
        // Ошибка разбора не заменяется сообщением об обязательности.
        assert_eq!(
            field(&form, "date").error(),
            Some("Введите дату в виде дд.мм.гггг.")
        );
    }

    #[test]
    fn data_for_the_core_carries_identity_values_and_tables() {
        let mut form = loaded_receipt();
        edit(&mut form, "date", "11.02.2026");
        let data = form.to_data().unwrap();
        assert_eq!(data.id, Some(12));
        assert_eq!(data.version, 4);
        assert_eq!(data.number, Some(7));
        assert!(!data.posted);
        assert_eq!(data.fields["date"], Value::Date("2026-02-11".to_owned()));
        assert_eq!(
            data.fields["amount"],
            Value::Decimal {
                units: 123_456,
                scale: 2
            }
        );
        // Строки табличной части уходят назад как пришли, ссылки одним `id`.
        let mut expected = receipt_data().tables;
        for line in expected.values_mut().flatten() {
            for value in line.values_mut() {
                *value = core_value(value);
            }
        }
        assert_eq!(data.tables, expected);
        assert_eq!(data.fields.len(), 4);
    }

    #[test]
    fn reference_goes_to_the_core_as_id_without_title() {
        let mut form = loaded_receipt();
        let data = form.to_data().unwrap();
        assert_eq!(data.fields["supplier"], Value::Ref { id: 3, title: None });
    }

    #[test]
    fn new_object_goes_to_the_core_without_id() {
        let mut form = FormState::new_object(&goods_meta());
        edit(&mut form, "name", "Мука");
        let data = form.to_data().unwrap();
        assert_eq!(data.id, None);
        assert_eq!(data.version, 0);
        assert_eq!(data.fields["name"], Value::Text("Мука".to_owned()));
        assert_eq!(data.fields["unit"], Value::Null);
        assert!(data.tables.is_empty());
    }

    #[test]
    fn reference_is_kept_as_id_and_title() {
        let mut form = FormState::new_object(&object_meta("receipt"));
        let supplier = index(&form, "supplier");
        form.set_reference(supplier, Some((3, "Мельница".to_owned())));
        let chosen = field(&form, "supplier");
        assert_eq!(*chosen.value(), reference(3, "Мельница"));
        assert_eq!(chosen.text(), "Мельница");
        assert_eq!(chosen.error(), None);
        assert!(form.is_modified());
        // Очистка снимает значение.
        form.set_reference(supplier, None);
        assert_eq!(*field(&form, "supplier").value(), Value::Null);
        assert_eq!(field(&form, "supplier").text(), "");
    }

    #[test]
    fn choosing_a_reference_clears_the_required_error() {
        let mut form = FormState::new_object(&object_meta("receipt"));
        assert!(form.to_data().is_err());
        let supplier = index(&form, "supplier");
        form.set_reference(supplier, Some((3, "Мельница".to_owned())));
        assert_eq!(field(&form, "supplier").error(), None);
    }

    #[test]
    fn only_editable_reference_fields_accept_a_choice() {
        let mut form = loaded_receipt();
        // Не ссылка и вычисляемое поле выбор не принимают.
        let date = index(&form, "date");
        form.set_reference(date, Some((1, "x".to_owned())));
        assert_eq!(field(&form, "date").text(), "10.02.2026");
        assert!(!form.is_modified());
    }

    #[test]
    fn reference_without_title_is_shown_by_id() {
        let mut data = receipt_data();
        data.fields
            .insert("supplier".to_owned(), Value::Ref { id: 3, title: None });
        let form = FormState::from_data(&object_meta("receipt"), &data);
        assert_eq!(field(&form, "supplier").text(), "3");
    }

    #[test]
    fn presentation_of_documents_and_elements() {
        assert_eq!(loaded_receipt().presentation(), "Поступление №7");
        assert_eq!(
            FormState::new_object(&object_meta("receipt")).presentation(),
            "Поступление (новый)"
        );
        let mut data = ObjectData {
            id: Some(5),
            ..ObjectData::default()
        };
        data.fields
            .insert("name".to_owned(), Value::Text("Мука".to_owned()));
        let element = FormState::from_data(&goods_meta(), &data);
        assert_eq!(element.presentation(), "Мука");
        data.fields.remove("name");
        assert_eq!(
            FormState::from_data(&goods_meta(), &data).presentation(),
            "Товары №5"
        );
        assert_eq!(
            FormState::new_object(&goods_meta()).presentation(),
            "Товары (новый)"
        );
    }

    #[test]
    fn presentation_ignores_unsaved_typing() {
        let mut data = ObjectData {
            id: Some(5),
            ..ObjectData::default()
        };
        data.fields
            .insert("name".to_owned(), Value::Text("Мука".to_owned()));
        let mut form = FormState::from_data(&goods_meta(), &data);
        edit(&mut form, "name", "Сахар");
        assert_eq!(form.presentation(), "Мука");
    }

    #[test]
    fn status_of_document_and_element() {
        let mut receipt = loaded_receipt();
        assert_eq!(receipt.status(), Some(Status::Draft));
        assert_eq!(
            FormState::new_object(&object_meta("receipt")).status(),
            Some(Status::New)
        );
        let mut posted_data = receipt_data();
        posted_data.posted = true;
        let mut posted = FormState::from_data(&object_meta("receipt"), &posted_data);
        assert_eq!(posted.status(), Some(Status::Posted));
        edit(&mut posted, "date", "11.02.2026");
        assert_eq!(posted.status(), Some(Status::PostedChanged));
        edit(&mut receipt, "date", "11.02.2026");
        assert_eq!(receipt.status(), Some(Status::Draft));

        let element = FormState::from_data(
            &goods_meta(),
            &ObjectData {
                id: Some(1),
                ..ObjectData::default()
            },
        );
        assert_eq!(element.status(), None);
        let mut deleted_data = receipt_data();
        deleted_data.deleted = true;
        assert_eq!(
            FormState::from_data(&object_meta("receipt"), &deleted_data).status(),
            Some(Status::Deleted)
        );
        assert_eq!(Status::PostedChanged.label(), "Проведён, изменён");
    }

    #[test]
    fn document_actions_follow_kind_state_and_rights() {
        let mut meta = object_meta("receipt");
        meta.rights = Rights::ALL;
        let mut data = receipt_data();
        let draft = FormState::from_data(&meta, &data);
        assert!(draft.can_save() && draft.can_post() && !draft.can_unpost());
        // Проведённый документ ядро без перепроведения не принимает.
        data.posted = true;
        let posted = FormState::from_data(&meta, &data);
        assert!(!posted.can_save() && posted.can_post() && posted.can_unpost());
        // Новый документ отменять нечего.
        let new = FormState::new_object(&meta);
        assert!(new.can_save() && new.can_post() && !new.can_unpost());
    }

    #[test]
    fn element_of_catalog_is_never_posted() {
        let mut meta = goods_meta();
        meta.rights = Rights::ALL;
        let element = FormState::new_object(&meta);
        assert!(element.can_save());
        assert!(!element.can_post());
        assert!(!element.can_unpost());
    }

    #[test]
    fn rights_close_what_the_user_may_not_do() {
        let mut meta = object_meta("receipt");
        meta.rights = Rights {
            read: true,
            create: true,
            ..Rights::default()
        };
        // Создавать можно, проводить и менять записанное нельзя.
        let new = FormState::new_object(&meta);
        assert!(new.can_save() && !new.can_post());
        let loaded = FormState::from_data(&meta, &receipt_data());
        assert!(!loaded.can_save() && !loaded.can_post());
        assert!(!loaded.is_editable(index(&loaded, "date")));
        let mut readonly = loaded;
        edit(&mut readonly, "date", "11.02.2026");
        assert!(!readonly.is_modified());
    }

    #[test]
    fn only_free_text_fields_are_editable() {
        let mut meta = object_meta("receipt");
        meta.rights = Rights::ALL;
        let form = FormState::from_data(&meta, &receipt_data());
        assert!(form.is_editable(index(&form, "date")));
        assert!(!form.is_editable(index(&form, "supplier")));
        assert!(!form.is_editable(index(&form, "amount")));
        assert!(!form.is_editable(99));
    }

    #[test]
    fn saved_response_makes_the_form_clean_and_keeps_the_new_version() {
        let mut meta = object_meta("receipt");
        meta.rights = Rights::ALL;
        let mut form = FormState::new_object(&meta);
        edit(&mut form, "date", "10.02.2026");
        form.apply_saved(
            &Saved {
                id: 9,
                version: 1,
                number: Some(3),
            },
            PostAction::Post,
        );
        assert_eq!(form.id(), Some(9));
        assert_eq!(form.version(), 1);
        assert_eq!(form.number(), Some(3));
        assert!(form.is_posted());
        assert!(!form.is_modified());
        // Повторная запись идёт уже как запись существующего объекта.
        assert!(!form.is_new());
        assert_eq!(form.presentation(), "Поступление №3");
        form.apply_saved(
            &Saved {
                id: 9,
                version: 2,
                number: Some(3),
            },
            PostAction::Unpost,
        );
        assert!(!form.is_posted());
        assert_eq!(form.version(), 2);
    }

    #[test]
    fn table_parts_follow_meta() {
        let receipt = loaded_receipt();
        let titles: Vec<&str> = receipt.parts().iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Товары"]);
        assert_eq!(receipt.parts().iter().next().map(|p| p.len()), Some(1));
        assert!(FormState::new_object(&goods_meta()).parts().is_empty());
    }

    #[test]
    fn invalid_form_names_fields_and_rows() {
        let only_fields = InvalidForm { fields: 2, rows: 0 };
        assert_eq!(
            only_fields.to_string(),
            "Не записано: исправьте отмеченные поля (2)."
        );
        let only_rows = InvalidForm { fields: 0, rows: 1 };
        assert_eq!(
            only_rows.to_string(),
            "Не записано: заполните отмеченные строки (1)."
        );
        assert!(
            InvalidForm { fields: 1, rows: 3 }
                .to_string()
                .contains("поля (1) и строки (3)")
        );
    }

    fn issue_meta() -> ObjectMeta {
        let mut meta = object_meta("issue");
        meta.rights = Rights::ALL;
        meta
    }

    #[test]
    fn issue_is_filled_with_a_line_and_goes_to_the_core() {
        let mut form = FormState::new_object(&issue_meta());
        edit(&mut form, "date", "12.03.2026");
        let warehouse = index(&form, "warehouse");
        assert!(form.can_pick(warehouse));
        form.set_reference(warehouse, Some((2, "Склад №2".to_owned())));
        assert!(form.add_row(0));
        form.set_cell_reference(0, Some((5, "Мука".to_owned())));
        form.edit_cell(1, "2,5");
        assert!(form.commit_row().is_ok());
        assert!(form.is_modified());
        let data = form.to_data().unwrap();
        assert_eq!(data.fields["warehouse"], Value::Ref { id: 2, title: None });
        let lines = &data.tables["lines"];
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["goods"], Value::Ref { id: 5, title: None });
        assert_eq!(
            lines[0]["qty"],
            Value::Decimal {
                units: 2500,
                scale: 3
            }
        );
    }

    #[test]
    fn empty_line_stops_saving_with_an_error_at_the_row() {
        let mut data = ObjectData::default();
        data.fields
            .insert("date".to_owned(), Value::Date("2026-03-12".to_owned()));
        data.fields
            .insert("warehouse".to_owned(), reference(1, "Основной"));
        data.tables.insert(
            "lines".to_owned(),
            vec![[("goods".to_owned(), reference(5, "Мука"))].into()],
        );
        let mut form = FormState::from_data(&issue_meta(), &data);
        let invalid = form.to_data().unwrap_err();
        assert_eq!(invalid, InvalidForm { fields: 0, rows: 1 });
        let part = form.parts().iter().next().unwrap();
        assert_eq!(part.row_error(0), Some("Заполните: Количество."));
    }

    #[test]
    fn rows_are_not_touched_without_the_right_to_write() {
        let mut meta = object_meta("issue");
        meta.rights = Rights {
            read: true,
            ..Rights::default()
        };
        let mut data = ObjectData {
            id: Some(3),
            ..ObjectData::default()
        };
        data.tables.insert(
            "lines".to_owned(),
            vec![[("goods".to_owned(), reference(5, "Мука"))].into()],
        );
        let mut form = FormState::from_data(&meta, &data);
        assert!(!form.add_row(0));
        assert!(!form.remove_row(0, 0));
        assert!(!form.can_pick(index(&form, "warehouse")));
        // Строку можно открыть посмотреть, но не править.
        assert!(form.open_row(0, 0));
        form.edit_cell(1, "3");
        form.set_cell_reference(0, None);
        assert!(form.commit_row().is_ok());
        assert!(!form.is_modified());
    }

    #[test]
    fn saved_response_makes_rows_clean() {
        let mut form = FormState::new_object(&issue_meta());
        assert!(form.add_row(0));
        form.set_cell_reference(0, Some((5, "Мука".to_owned())));
        form.edit_cell(1, "1");
        form.commit_row().unwrap();
        assert!(form.is_modified());
        form.apply_saved(
            &Saved {
                id: 4,
                version: 1,
                number: Some(2),
            },
            PostAction::Write,
        );
        assert!(!form.is_modified());
    }
}
