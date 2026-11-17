//! Окно «Свойства пользователя» (ADR-043) на экране «Пользователи и роли». Поля строятся только
//! из `Meta.user_fields`: ссылку выбирают из справочника (`ref_choice` и окно `RefPicker`),
//! остальные типы вводятся теми же редакторами, что в формах. Значения читает
//! `admin_user_fields`, пишет `admin_set_user_fields`, оба в фоне. Что можно записать (тип,
//! элемент справочника, одно значение без `many`), проверяет ядро, его ошибка видна в окне.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::engine::{
    Engine, EngineResult, FieldKind, Meta, ObjectMeta, UserFieldDef, UserFields, Value,
    call_in_background,
};
use crate::form_state::{FormField, core_value};
use crate::form_ui::editor_item;
use crate::ref_choice::{self, PickPage};
use crate::values::date_text;
use crate::{AppWindow, FormFieldItem, PickItem, UserFieldGroup, UserFieldsData};

/// Пауза после последней буквы поиска, как в выборе ссылки в форме.
const SEARCH_PAUSE: Duration = Duration::from_millis(250);

/// Подпись кнопки повтора поиска в окне выбора.
const RETRY: &str = "Повторить";

/// Свойство в окне: описание из конфигурации и значения по порядку. У свойства без `many`
/// значение ровно одно, возможно пустое.
struct Prop {
    def: UserFieldDef,
    values: Vec<FormField>,
}

impl Prop {
    fn new(def: &UserFieldDef, given: &[Value]) -> Self {
        let mut values: Vec<FormField> = given
            .iter()
            .map(|value| FormField::new(&def.field, value.clone()))
            .collect();
        if !def.many {
            values.truncate(1);
            if values.is_empty() {
                values.push(FormField::new(&def.field, Value::Null));
            }
        }
        Self {
            def: def.clone(),
            values,
        }
    }

    fn is_ref(&self) -> bool {
        self.def.field.kind == FieldKind::Ref
    }

    /// Выбор ссылки. `slot`: какое значение заменить, `None`: добавить новое (уже выбранный
    /// элемент второй раз не встаёт). `chosen` `None` очищает значение.
    fn choose(&mut self, slot: Option<usize>, chosen: Option<(i64, String)>) {
        match slot {
            Some(slot) => {
                if let Some(value) = self.values.get_mut(slot) {
                    value.choose(chosen);
                }
            }
            None => {
                if let Some((id, title)) = chosen
                    && !self.values.iter().any(|v| v.reference_id() == Some(id))
                {
                    let mut value = FormField::new(&self.def.field, Value::Null);
                    value.choose(Some((id, title)));
                    self.values.push(value);
                }
            }
        }
    }
}

/// Значения окна для `admin_set_user_fields`: все свойства конфигурации, пустые значения
/// отброшены (пустой список очищает свойство), у ссылок только `id`. Ошибка: где-то текст не
/// разобрался, записывать нельзя.
fn for_core(props: &[Prop]) -> Result<UserFields, String> {
    if props
        .iter()
        .flat_map(|prop| &prop.values)
        .any(|value| value.error().is_some())
    {
        return Err("Исправьте значения с ошибками.".to_owned());
    }
    Ok(props
        .iter()
        .map(|prop| {
            let values = prop
                .values
                .iter()
                .map(FormField::value)
                .filter(|value| **value != Value::Null)
                .map(core_value)
                .collect();
            (prop.def.field.name.clone(), values)
        })
        .collect())
}

/// Свойство для окна: подпись, значения редакторами форм и можно ли добавить ещё.
fn group(prop: &Prop, objects: &[ObjectMeta]) -> UserFieldGroup {
    let field = &prop.def.field;
    let refused = prop
        .is_ref()
        .then(|| ref_choice::target_of(objects, field.target.as_deref(), &field.title).err())
        .flatten();
    let values: Vec<FormFieldItem> = prop
        .values
        .iter()
        .map(|value| {
            let mut item =
                editor_item(value, value.accepts_text(), value.accepts_choice(), objects);
            // У нескольких значений подпись одна, над списком.
            if prop.def.many {
                item.label = SharedString::new();
            }
            item
        })
        .collect();
    UserFieldGroup {
        label: field.title.as_str().into(),
        many: prop.def.many,
        list: prop.def.many && prop.is_ref(),
        values: ModelRc::new(VecModel::from(values)),
        can_add: refused.is_none(),
        hint: refused.unwrap_or_default().into(),
    }
}

/// Окно выбора ссылки для свойства `prop`: заменить значение `slot` или, если `None`, добавить.
struct Picker {
    prop: usize,
    slot: Option<usize>,
    meta: ObjectMeta,
    query: String,
    /// Варианты из ядра; `None`: первая порция ещё едет.
    page: Option<PickPage>,
    error: Option<String>,
    request: u64,
    pause: slint::Timer,
}

/// Открытое окно: в каком окне приложения, чьи свойства и что в них сейчас.
struct Open {
    window: slint::Weak<AppWindow>,
    engine: Arc<dyn Engine>,
    login: String,
    defs: Vec<UserFieldDef>,
    objects: Vec<ObjectMeta>,
    /// `None`: значения ещё не прочитаны или прочитать не удалось, записывать нечего.
    props: Option<Vec<Prop>>,
    busy: bool,
    error: String,
    /// Запрос чтения или записи, ответа на который ждёт окно.
    request: u64,
    picker: Option<Picker>,
}

impl Open {
    fn prop_mut(&mut self, index: usize) -> Option<&mut Prop> {
        self.props.as_mut()?.get_mut(index)
    }

    /// Элементы, которые выбор отмечает: значение, которое меняют, или все уже добавленные.
    fn marked(&self) -> Vec<i64> {
        let Some(picker) = &self.picker else {
            return Vec::new();
        };
        let Some(prop) = self.props.as_ref().and_then(|props| props.get(picker.prop)) else {
            return Vec::new();
        };
        let values = match picker.slot {
            Some(slot) => prop.values.get(slot..=slot).unwrap_or_default(),
            None => prop.values.as_slice(),
        };
        values.iter().filter_map(FormField::reference_id).collect()
    }
}

/// Окно «Свойства пользователя»: одно на все окна приложения, открыто в том, где нажали
/// «Свойства».
#[derive(Clone)]
pub struct UserFieldsUi {
    open: Rc<RefCell<Option<Open>>>,
    /// Номера запросов: ответ на запрос прошлого окна или прошлого поиска выбрасывается.
    requests: Rc<Cell<u64>>,
    /// Свойства записаны: логин пользователя.
    saved: Rc<dyn Fn(&str)>,
}

/// Номер из колбэка Slint; отрицательный: нет такого.
fn index(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

impl UserFieldsUi {
    pub fn new(saved: impl Fn(&str) + 'static) -> Self {
        Self {
            open: Rc::default(),
            requests: Rc::default(),
            saved: Rc::new(saved),
        }
    }

    fn next_request(&self) -> u64 {
        let next = self.requests.get().wrapping_add(1);
        self.requests.set(next);
        next
    }

    /// Подключает события окна к `window` (главному или дополнительному). «Свойства» у строки
    /// списка подключает `app.rs`: логины знает он.
    pub fn attach(&self, window: &AppWindow) {
        let data = window.global::<UserFieldsData>();
        data.on_edited({
            let ui = self.clone();
            move |prop, value, text| {
                if let (Some(prop), Some(value)) = (index(prop), index(value)) {
                    ui.edit(prop, value, &text);
                }
            }
        });
        data.on_date_edited({
            let ui = self.clone();
            move |prop, value, day, month, year| {
                let date = (u16::try_from(year), u8::try_from(month), u8::try_from(day));
                if let (Some(prop), Some(value), (Ok(year), Ok(month), Ok(day))) =
                    (index(prop), index(value), date)
                {
                    ui.edit(prop, value, &date_text((year, month, day)));
                }
            }
        });
        data.on_pick({
            let ui = self.clone();
            move |prop, value| {
                if let (Some(prop), Some(value)) = (index(prop), index(value)) {
                    ui.open_picker(prop, Some(value));
                }
            }
        });
        data.on_clear({
            let ui = self.clone();
            move |prop, value| {
                if let (Some(prop), Some(value)) = (index(prop), index(value)) {
                    ui.change(prop, |p| p.choose(Some(value), None));
                }
            }
        });
        data.on_remove({
            let ui = self.clone();
            move |prop, value| {
                if let (Some(prop), Some(value)) = (index(prop), index(value)) {
                    ui.change(prop, |p| {
                        if p.def.many && value < p.values.len() {
                            p.values.remove(value);
                        }
                    });
                }
            }
        });
        data.on_add({
            let ui = self.clone();
            move |prop| {
                if let Some(prop) = index(prop) {
                    ui.add(prop);
                }
            }
        });
        data.on_save({
            let ui = self.clone();
            move || ui.save()
        });
        data.on_cancel({
            let ui = self.clone();
            move || ui.close()
        });
        data.on_picker_query_changed({
            let ui = self.clone();
            move |text| ui.picker_query(&text)
        });
        data.on_picker_choose({
            let ui = self.clone();
            move |option| {
                if let Some(option) = index(option) {
                    ui.choose(option);
                }
            }
        });
        data.on_picker_clear({
            let ui = self.clone();
            move || ui.close_picker(true)
        });
        data.on_picker_close({
            let ui = self.clone();
            move || ui.close_picker(false)
        });
        data.on_picker_retry({
            let ui = self.clone();
            move || ui.search()
        });
    }

    /// Открывает свойства пользователя `login` в окне `window`, значения читаются в фоне.
    pub fn open(&self, window: &AppWindow, engine: Arc<dyn Engine>, login: &str, meta: &Meta) {
        self.close();
        let request = self.next_request();
        *self.open.borrow_mut() = Some(Open {
            window: window.as_weak(),
            engine: engine.clone(),
            login: login.to_owned(),
            defs: meta.user_fields.clone(),
            objects: meta.objects.clone(),
            props: None,
            busy: false,
            error: String::new(),
            request,
            picker: None,
        });
        self.show(true);
        let (ui, login) = (self.clone(), login.to_owned());
        call_in_background(
            move || engine.admin_user_fields(&login),
            move |result| ui.loaded(request, result),
        );
    }

    /// Закрывает окно (отмена, база закрыта, сменился пользователь); ответы на отправленные
    /// запросы пропадают.
    pub fn close(&self) {
        self.next_request();
        let Some(open) = self.open.borrow_mut().take() else {
            return;
        };
        if let Some(window) = open.window.upgrade() {
            let data = window.global::<UserFieldsData>();
            data.set_shown(false);
            data.set_groups(ModelRc::default());
            data.set_error("".into());
            show_picker(&data, None, &[]);
        }
    }

    fn loaded(&self, request: u64, result: EngineResult<UserFields>) {
        {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard.as_mut().filter(|open| open.request == request) else {
                return;
            };
            match result {
                Ok(values) => {
                    let props = open
                        .defs
                        .iter()
                        .map(|def| {
                            let given = values.get(&def.field.name).map_or(&[][..], Vec::as_slice);
                            Prop::new(def, given)
                        })
                        .collect();
                    open.props = Some(props);
                }
                Err(error) => open.error = error.message,
            }
        }
        self.show(true);
    }

    /// Правка значения текстом: поле меняется на месте, курсор не теряется.
    fn edit(&self, prop: usize, value: usize, text: &str) {
        {
            let mut guard = self.open.borrow_mut();
            let Some(field) = guard
                .as_mut()
                .filter(|open| !open.busy)
                .and_then(|open| open.prop_mut(prop))
                .and_then(|p| p.values.get_mut(value))
                .filter(|field| field.accepts_text())
            else {
                return;
            };
            field.apply_text(text);
        }
        self.show(false);
    }

    /// Правка, после которой значений другое число или другие ссылки: окно строится заново.
    fn change(&self, prop: usize, apply: impl FnOnce(&mut Prop)) {
        {
            let mut guard = self.open.borrow_mut();
            let Some(p) = guard
                .as_mut()
                .filter(|open| !open.busy && open.picker.is_none())
                .and_then(|open| open.prop_mut(prop))
            else {
                return;
            };
            apply(p);
        }
        self.show(true);
    }

    /// «+ Добавить»: ссылку выбирают из справочника, остальное вводят в новое пустое поле.
    fn add(&self, prop: usize) {
        let is_ref = self
            .open
            .borrow()
            .as_ref()
            .and_then(|open| open.props.as_ref()?.get(prop))
            .is_some_and(|p| p.def.many && p.is_ref());
        if is_ref {
            self.open_picker(prop, None);
            return;
        }
        self.change(prop, |p| {
            if p.def.many {
                p.values.push(FormField::new(&p.def.field, Value::Null));
            }
        });
    }

    /// Открывает выбор из справочника и просит у ядра первую порцию вариантов.
    fn open_picker(&self, prop: usize, slot: Option<usize>) {
        {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard
                .as_mut()
                .filter(|open| !open.busy && open.picker.is_none())
            else {
                return;
            };
            let Some(p) = open
                .props
                .as_ref()
                .and_then(|props| props.get(prop))
                .filter(|p| p.is_ref() && slot.is_none_or(|slot| slot < p.values.len()))
            else {
                return;
            };
            let field = &p.def.field;
            let Ok(meta) =
                ref_choice::target_of(&open.objects, field.target.as_deref(), &field.title)
            else {
                return;
            };
            open.picker = Some(Picker {
                prop,
                slot,
                meta: meta.clone(),
                query: String::new(),
                page: None,
                error: None,
                request: 0,
                pause: slint::Timer::default(),
            });
        }
        self.show(false);
        self.search();
    }

    /// Набран текст поиска: запрос уйдёт после паузы.
    fn picker_query(&self, text: &str) {
        let mut guard = self.open.borrow_mut();
        let Some(picker) = guard.as_mut().and_then(|open| open.picker.as_mut()) else {
            return;
        };
        text.clone_into(&mut picker.query);
        // Таймер живёт в выборе: закрытый выбор останавливает его вместе с запросом.
        let ui = self.clone();
        picker
            .pause
            .start(slint::TimerMode::SingleShot, SEARCH_PAUSE, move || {
                ui.search()
            });
    }

    /// Ищет варианты в фоне по текущему тексту поиска.
    fn search(&self) {
        let request = self.next_request();
        let job = {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard.as_mut() else {
                return;
            };
            let engine = open.engine.clone();
            let Some(picker) = open.picker.as_mut() else {
                return;
            };
            picker.request = request;
            picker.error = None;
            (engine, picker.meta.clone(), picker.query.clone())
        };
        self.show_picker_only();
        let ui = self.clone();
        let (engine, meta, query) = job;
        call_in_background(
            move || ref_choice::find(&*engine, &meta, &query),
            move |result| ui.found(request, result),
        );
    }

    /// Ответ ядра на поиск. Устаревший ответ (пока шёл, набрали ещё) выбрасывается.
    fn found(&self, request: u64, result: EngineResult<PickPage>) {
        let (highlight, window) = {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard.as_mut() else {
                return;
            };
            let marked = open.marked();
            let window = open.window.clone();
            let Some(picker) = open
                .picker
                .as_mut()
                .filter(|picker| picker.request == request)
            else {
                return;
            };
            let highlight = match result {
                Ok(page) => {
                    // Выделен вариант, который меняют, иначе первый.
                    let at = picker
                        .slot
                        .and_then(|_| marked.first())
                        .and_then(|id| page.options.iter().position(|o| o.id == *id));
                    let highlight = match (at, page.options.is_empty()) {
                        (Some(at), _) => i32::try_from(at).unwrap_or(0),
                        (None, false) => 0,
                        (None, true) => -1,
                    };
                    picker.page = Some(page);
                    highlight
                }
                Err(error) => {
                    picker.error = Some(error.message);
                    -1
                }
            };
            (highlight, window)
        };
        if let Some(window) = window.upgrade() {
            window
                .global::<UserFieldsData>()
                .set_picker_current(highlight);
        }
        self.show_picker_only();
    }

    /// Выбран вариант с номером `option`.
    fn choose(&self, option: usize) {
        {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard.as_mut() else {
                return;
            };
            let Some(picker) = open.picker.take() else {
                return;
            };
            let chosen = picker
                .page
                .as_ref()
                .and_then(|page| page.options.get(option))
                .map(|option| (option.id, option.title.clone()));
            let Some(chosen) = chosen else {
                open.picker = Some(picker);
                return;
            };
            if let Some(p) = open.prop_mut(picker.prop) {
                p.choose(picker.slot, Some(chosen));
            }
        }
        self.show(true);
    }

    /// Закрывает выбор; `clear`: «Очистить», значение, которое меняли, снимается.
    fn close_picker(&self, clear: bool) {
        let cleared = {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard.as_mut() else {
                return;
            };
            let Some(picker) = open.picker.take() else {
                return;
            };
            match (clear, picker.slot, open.prop_mut(picker.prop)) {
                (true, Some(slot), Some(p)) => {
                    p.choose(Some(slot), None);
                    true
                }
                _ => false,
            }
        };
        self.show(cleared);
    }

    /// «Сохранить»: все свойства одной командой, при успехе окно закрывается.
    fn save(&self) {
        let job = {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard
                .as_mut()
                .filter(|open| !open.busy && open.picker.is_none())
            else {
                return;
            };
            let Some(props) = open.props.as_deref() else {
                return;
            };
            match for_core(props) {
                Ok(fields) => {
                    let request = self.next_request();
                    open.request = request;
                    open.busy = true;
                    open.error.clear();
                    Some((open.engine.clone(), open.login.clone(), fields, request))
                }
                Err(message) => {
                    open.error = message;
                    None
                }
            }
        };
        self.show(false);
        let Some((engine, login, fields, request)) = job else {
            return;
        };
        let ui = self.clone();
        call_in_background(
            move || engine.admin_set_user_fields(&login, &fields),
            move |result| ui.written(request, result),
        );
    }

    fn written(&self, request: u64, result: EngineResult<()>) {
        let login = {
            let mut guard = self.open.borrow_mut();
            let Some(open) = guard.as_mut().filter(|open| open.request == request) else {
                return;
            };
            open.busy = false;
            match result {
                Ok(()) => Some(open.login.clone()),
                Err(error) => {
                    open.error = error.message;
                    None
                }
            }
        };
        match login {
            Some(login) => {
                self.close();
                (self.saved)(&login);
            }
            None => self.show(false),
        }
    }

    /// Переносит состояние в окно. `rebuild`: поля строятся заново (открытие, «Добавить»,
    /// «Убрать», выбор); иначе меняются на месте, и набранный текст с курсором остаются.
    fn show(&self, rebuild: bool) {
        let guard = self.open.borrow();
        let Some(open) = guard.as_ref() else {
            return;
        };
        let Some(window) = open.window.upgrade() else {
            return;
        };
        let data = window.global::<UserFieldsData>();
        data.set_shown(true);
        data.set_login(open.login.as_str().into());
        data.set_loading(open.props.is_none() && open.error.is_empty());
        data.set_loaded(open.props.is_some());
        data.set_busy(open.busy);
        data.set_error(open.error.as_str().into());
        let props = open.props.as_deref().unwrap_or_default();
        let groups: Vec<UserFieldGroup> = props.iter().map(|p| group(p, &open.objects)).collect();
        if rebuild || !update_in_place(&data, &groups) {
            data.set_groups(ModelRc::new(VecModel::from(groups)));
        }
        show_picker(&data, open.picker.as_ref(), &open.marked());
    }

    /// Перерисовывает только окно выбора: поля свойств при этом не трогаются.
    fn show_picker_only(&self) {
        let guard = self.open.borrow();
        let Some(open) = guard.as_ref() else {
            return;
        };
        if let Some(window) = open.window.upgrade() {
            let data = window.global::<UserFieldsData>();
            show_picker(&data, open.picker.as_ref(), &open.marked());
        }
    }
}

/// Значения на месте: редакторы остаются. `false`: свойств или значений другое число, окно надо
/// строить заново.
fn update_in_place(data: &UserFieldsData<'_>, groups: &[UserFieldGroup]) -> bool {
    let current = data.get_groups();
    if current.row_count() != groups.len() {
        return false;
    }
    for (row, group) in groups.iter().enumerate() {
        let shown = current.row_data(row).map(|old| old.values);
        let Some(shown) = shown else {
            return false;
        };
        let Some(model) = shown.as_any().downcast_ref::<VecModel<FormFieldItem>>() else {
            return false;
        };
        if model.row_count() != group.values.row_count() {
            return false;
        }
        for (value, item) in group.values.iter().enumerate() {
            model.set_row_data(value, item);
        }
    }
    true
}

/// Окно выбора в `UserFieldsData`; `None`: закрыто. `marked`: элементы, отмеченные галочкой.
/// Текст поиска ставится, только если он другой: иначе курсор прыгал бы на каждом ответе ядра.
fn show_picker(data: &UserFieldsData<'_>, picker: Option<&Picker>, marked: &[i64]) {
    data.set_picker_open(picker.is_some());
    let Some(picker) = picker else {
        data.set_picker_items(ModelRc::default());
        data.set_picker_query("".into());
        return;
    };
    if data.get_picker_query() != picker.query.as_str() {
        data.set_picker_query(picker.query.as_str().into());
    }
    data.set_picker_title(format!("Выбор: {}", picker.meta.title).into());
    let items: Vec<PickItem> = picker
        .page
        .iter()
        .flat_map(|page| &page.options)
        .map(|option| PickItem {
            title: option.title.as_str().into(),
            current: marked.contains(&option.id),
        })
        .collect();
    data.set_picker_items(ModelRc::new(VecModel::from(items)));
    let note = match &picker.page {
        Some(page) => ref_choice::note(page, !picker.query.trim().is_empty()),
        None => "Загрузка…".to_owned(),
    };
    data.set_picker_note(note.into());
    let error = picker.error.as_deref().unwrap_or_default();
    data.set_picker_error(error.into());
    data.set_picker_error_action(if error.is_empty() { "" } else { RETRY }.into());
    // Очистить можно только значение, которое меняют; новое добавляют выбором.
    data.set_picker_can_clear(picker.slot.is_some() && !marked.is_empty());
}

#[cfg(test)]
mod tests {
    use ostov_api::FieldDef;

    use super::*;

    fn def(name: &str, kind: FieldKind, many: bool) -> UserFieldDef {
        UserFieldDef {
            field: FieldDef::new(name, kind),
            many,
        }
    }

    #[test]
    fn values_go_to_the_core_without_titles_and_blanks() {
        let refs = def("stores", FieldKind::Ref, true);
        let titled = |id| Value::Ref {
            id,
            title: Some(format!("Склад {id}")),
        };
        let mut stores = Prop::new(&refs, &[titled(1), titled(2)]);
        // Уже выбранный второй раз не встаёт, убранный уходит из списка.
        stores.choose(None, Some((2, "Склад 2".into())));
        stores.choose(None, Some((3, "Склад 3".into())));
        stores.values.remove(0);
        let badge = Prop::new(&def("badge", FieldKind::Int, false), &[]);
        assert_eq!(badge.values.len(), 1);
        let props = [stores, badge];
        let bare = |id| Value::Ref { id, title: None };
        let fields = for_core(&props).unwrap();
        assert_eq!(fields["stores"], [bare(2), bare(3)]);
        // Пустое поле без `many`: пустой список, ядро очистит свойство.
        assert!(fields["badge"].is_empty());

        let [stores, mut badge] = props;
        badge.values[0].apply_text("12x");
        assert!(for_core(&[stores, badge]).is_err());
    }
}
