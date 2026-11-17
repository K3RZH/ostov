//! Выбор ссылки в форме: окно со списком элементов справочника поверх формы или окна строки.
//! Состояние выбора лежит у формы (`Form::picker`). Варианты ищет ядро в фоне, первую порцию
//! совпадений за раз (`ref_choice`). «+ Добавить» открывает форму нового элемента во вкладке
//! рядом; когда её записывают, элемент встаёт в поле, вкладка с ним закрывается и форма
//! возвращается на экран.

use std::time::Duration;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::{Base, Draw, FormUi, shown_key};
use crate::engine::{EngineResult, call_in_background};
use crate::form_state::{FormField, FormState};
use crate::ref_choice::{self, PickPage};
use crate::shell::DocKey;
use crate::workspace::WindowRef;
use crate::{AppWindow, FormData, PickItem};

/// Пауза после последней буквы поиска: запрос в ядро не уходит на каждое нажатие.
const SEARCH_PAUSE: Duration = Duration::from_millis(250);

/// Куда идёт выбранное значение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PickTarget {
    /// Поле шапки с этим номером.
    Field(usize),
    /// Ячейка открытого окна строки.
    Cell(usize),
}

/// Как перерисовать форму, чтобы фокус вернулся в поле или ячейку выбора.
pub(super) fn focus_on(target: PickTarget) -> Draw {
    match target {
        PickTarget::Field(index) => Draw::FocusField(index),
        PickTarget::Cell(cell) => Draw::FocusCell(Some(cell)),
    }
}

/// Окно выбора одной ссылки.
pub(super) struct Picker {
    target: PickTarget,
    /// Справочник, из которого выбирают (имя в ядре) и его название.
    object: String,
    title: String,
    query: String,
    /// Варианты из ядра; `None`: первая порция ещё едет.
    page: Option<PickPage>,
    /// Ошибка поиска или открытия формы и можно ли её повторить.
    error: Option<(String, bool)>,
    request: u64,
    /// Что выбрано в поле сейчас: этот вариант отмечен и выделен при открытии.
    current: Option<i64>,
    can_add: bool,
    can_clear: bool,
    pause: slint::Timer,
}

/// Форма нового элемента, открытая из выбора: после записи элемент встанет в `target` формы
/// `origin`.
pub(super) struct Adding {
    pub(super) origin: DocKey,
    pub(super) target: PickTarget,
}

/// Поле или ячейка, куда выбирают значение, если выбирать туда можно.
fn pickable(state: &FormState, target: PickTarget) -> Option<&FormField> {
    match target {
        PickTarget::Field(index) => state
            .can_pick(index)
            .then(|| state.fields().get(index))
            .flatten(),
        PickTarget::Cell(cell) => state
            .parts()
            .draft()
            .filter(|draft| draft.accepts_choice(cell))
            .and_then(|draft| draft.cells().get(cell)),
    }
}

fn apply(state: &mut FormState, target: PickTarget, chosen: Option<(i64, String)>) {
    match target {
        PickTarget::Field(index) => state.set_reference(index, chosen),
        PickTarget::Cell(cell) => state.set_cell_reference(cell, chosen),
    }
}

/// Номер из колбэка Slint; отрицательный: нет такого.
fn index(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

impl FormUi {
    /// События выбора ссылки в окне `window`.
    pub(super) fn attach_picker(&self, window: &AppWindow) {
        let data = window.global::<FormData>();
        let bound = || (self.clone(), window.as_weak());
        data.on_pick({
            let (ui, window) = bound();
            move |field| {
                if let (Some(window), Some(field)) = (window.upgrade(), index(field)) {
                    ui.open_picker(&window, PickTarget::Field(field));
                }
            }
        });
        data.on_row_pick({
            let (ui, window) = bound();
            move |cell| {
                if let (Some(window), Some(cell)) = (window.upgrade(), index(cell)) {
                    ui.open_picker(&window, PickTarget::Cell(cell));
                }
            }
        });
        data.on_clear_reference({
            let (ui, window) = bound();
            move |field| {
                if let (Some(window), Some(field)) = (window.upgrade(), index(field)) {
                    ui.clear_reference(&window, PickTarget::Field(field));
                }
            }
        });
        data.on_row_clear({
            let (ui, window) = bound();
            move |cell| {
                if let (Some(window), Some(cell)) = (window.upgrade(), index(cell)) {
                    ui.clear_reference(&window, PickTarget::Cell(cell));
                }
            }
        });
        data.on_picker_query_changed({
            let (ui, window) = bound();
            move |text| {
                if let Some(window) = window.upgrade() {
                    ui.picker_query(&window, &text);
                }
            }
        });
        data.on_picker_choose({
            let (ui, window) = bound();
            move |option| {
                if let (Some(window), Some(option)) = (window.upgrade(), index(option)) {
                    ui.choose(&window, option);
                }
            }
        });
        data.on_picker_clear({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.close_picker(&window, Some(None));
                }
            }
        });
        data.on_picker_close({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.close_picker(&window, None);
                }
            }
        });
        data.on_picker_retry({
            let (ui, window) = bound();
            move || {
                if let Some(key) = window.upgrade().as_ref().and_then(shown_key) {
                    ui.search(&key);
                }
            }
        });
        data.on_picker_add({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.add_from_picker(&window);
                }
            }
        });
    }

    /// Открывает выбор для поля или ячейки и просит у ядра первую порцию вариантов.
    fn open_picker(&self, window: &AppWindow, target: PickTarget) {
        let Some(key) = shown_key(window) else {
            return;
        };
        {
            let mut base = self.base.borrow_mut();
            let Base { objects, forms, .. } = &mut *base;
            let Some(form) = forms
                .get_mut(&key)
                .filter(|form| !form.busy && form.picker.is_none())
            else {
                return;
            };
            let Some(field) = form.state().and_then(|state| pickable(state, target)) else {
                return;
            };
            let Ok(meta) = ref_choice::target_of(objects, field.target.as_deref(), &field.title)
            else {
                return;
            };
            let current = field.reference_id();
            form.picker = Some(Picker {
                target,
                object: meta.name.clone(),
                title: format!("Выбор: {}", meta.title),
                query: String::new(),
                page: None,
                error: None,
                request: 0,
                current,
                can_add: ref_choice::can_add(meta),
                can_clear: current.is_some(),
                pause: slint::Timer::default(),
            });
        }
        self.show_everywhere(&key, Draw::Refresh);
        self.search(&key);
    }

    /// Набран текст поиска: запрос уйдёт после паузы.
    fn picker_query(&self, window: &AppWindow, text: &str) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let mut base = self.base.borrow_mut();
        let Some(picker) = base
            .forms
            .get_mut(&key)
            .and_then(|form| form.picker.as_mut())
        else {
            return;
        };
        text.clone_into(&mut picker.query);
        // Таймер живёт в выборе: закрытый выбор останавливает его вместе с запросом.
        let ui = self.clone();
        picker
            .pause
            .start(slint::TimerMode::SingleShot, SEARCH_PAUSE, move || {
                ui.search(&key);
            });
    }

    /// Ищет варианты в фоне по текущему тексту поиска.
    fn search(&self, key: &DocKey) {
        let request = self.next_request();
        let job = {
            let mut base = self.base.borrow_mut();
            let Base {
                engine,
                objects,
                forms,
                ..
            } = &mut *base;
            let Some(picker) = forms.get_mut(key).and_then(|form| form.picker.as_mut()) else {
                return;
            };
            let Some(meta) = objects.iter().find(|meta| meta.name == picker.object) else {
                return;
            };
            picker.request = request;
            picker.error = None;
            engine
                .clone()
                .map(|engine| (engine, meta.clone(), picker.query.clone()))
        };
        let Some((engine, meta, query)) = job else {
            return;
        };
        self.show_picker_everywhere(key);
        let (ui, key) = (self.clone(), key.clone());
        call_in_background(
            move || ref_choice::find(&*engine, &meta, &query),
            move |result| ui.found(&key, request, result),
        );
    }

    /// Ответ ядра на поиск. Устаревший ответ (пока шёл, набрали ещё) выбрасывается.
    fn found(&self, key: &DocKey, request: u64, result: EngineResult<PickPage>) {
        let highlight = {
            let mut base = self.base.borrow_mut();
            let Some(picker) = base
                .forms
                .get_mut(key)
                .and_then(|form| form.picker.as_mut())
                .filter(|picker| picker.request == request)
            else {
                return;
            };
            match result {
                Ok(page) => {
                    // Выделен вариант, который сейчас в поле, иначе первый.
                    let at = picker
                        .current
                        .and_then(|id| page.options.iter().position(|o| o.id == id));
                    let highlight = match (at, page.options.is_empty()) {
                        (Some(at), _) => i32::try_from(at).unwrap_or(0),
                        (None, false) => 0,
                        (None, true) => -1,
                    };
                    picker.page = Some(page);
                    highlight
                }
                Err(error) => {
                    picker.error = Some((error.message, true));
                    -1
                }
            }
        };
        self.each_showing(key, |window| {
            window.global::<FormData>().set_picker_current(highlight);
        });
        self.show_picker_everywhere(key);
    }

    /// Выбран вариант с номером `option`.
    fn choose(&self, window: &AppWindow, option: usize) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let chosen = self
            .base
            .borrow()
            .forms
            .get(&key)
            .and_then(|form| form.picker.as_ref())
            .and_then(|picker| picker.page.as_ref())
            .and_then(|page| page.options.get(option))
            .map(|option| (option.id, option.title.clone()));
        if chosen.is_some() {
            self.close_picker(window, Some(chosen));
        }
    }

    /// Закрывает выбор. `Some(значение)`: поставить его в поле (`Some(None)` очищает),
    /// `None`: отмена, поле не меняется. Фокус возвращается в поле.
    fn close_picker(&self, window: &AppWindow, chosen: Option<Option<(i64, String)>>) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let (target, was, now) = {
            let mut base = self.base.borrow_mut();
            let Some(form) = base.forms.get_mut(&key) else {
                return;
            };
            let Some(picker) = form.picker.take() else {
                return;
            };
            let Some(state) = form.state_mut() else {
                return;
            };
            let was = state.is_modified();
            if let Some(chosen) = chosen {
                apply(state, picker.target, chosen);
            }
            (picker.target, was, state.is_modified())
        };
        self.show_everywhere(&key, focus_on(target));
        if was != now {
            self.mark_tab(&key, now);
        }
    }

    /// Крестик или Delete в самом поле: значение снимается без окна выбора.
    fn clear_reference(&self, window: &AppWindow, target: PickTarget) {
        self.change(window, |state| {
            pickable(state, target)?;
            apply(state, target, None);
            Some(focus_on(target))
        });
    }

    /// «+ Добавить»: форма нового элемента справочника во вкладке рядом. Вкладка формы
    /// закрепляется, чтобы новая её не заменила, пока элемент не подставлен.
    fn add_from_picker(&self, window: &AppWindow) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let Some((object, target)) = self
            .base
            .borrow()
            .forms
            .get(&key)
            .and_then(|form| form.picker.as_ref())
            .filter(|picker| picker.can_add)
            .map(|picker| (picker.object.clone(), picker.target))
        else {
            return;
        };
        self.with_tabs(&key, |tabs| {
            if let Some(index) = tabs.tab_of_doc(&key) {
                tabs.pin_index(index);
            }
        });
        let place = self
            .windows
            .id_of(window)
            .map_or(WindowRef::Main, WindowRef::Extra);
        self.windows.set_active(place);
        match self.open_new_key(&object) {
            Ok(added) => {
                let mut base = self.base.borrow_mut();
                if let Some(form) = base.forms.get_mut(&key) {
                    form.picker = None;
                    form.refocus = Some(target);
                }
                base.adding.insert(
                    added,
                    Adding {
                        origin: key.clone(),
                        target,
                    },
                );
            }
            Err(error) => {
                if let Some(picker) = self
                    .base
                    .borrow_mut()
                    .forms
                    .get_mut(&key)
                    .and_then(|form| form.picker.as_mut())
                {
                    picker.error = Some((error.to_string(), false));
                }
                self.show_picker_everywhere(&key);
            }
        }
    }

    /// Новый элемент записан в форме `key`, теперь его ключ `saved`: если форму открыли из
    /// выбора, элемент встаёт в поле той формы, вкладка элемента закрывается, а та форма
    /// снова на экране.
    pub(super) fn deliver_added(&self, key: &DocKey, saved: &DocKey) {
        let delivered = {
            let mut base = self.base.borrow_mut();
            let Some(adding) = base.adding.remove(key) else {
                return;
            };
            let title = base
                .forms
                .get(saved)
                .and_then(|form| form.state())
                .map(FormState::presentation);
            let Some(form) = base.forms.get_mut(&adding.origin) else {
                return;
            };
            let Some(state) = form.state_mut() else {
                return;
            };
            let was = state.is_modified();
            let title = title.unwrap_or_else(|| saved.id.to_string());
            apply(state, adding.target, Some((saved.id, title)));
            let now = state.is_modified();
            form.refocus = Some(adding.target);
            (adding, was, now)
        };
        let (adding, was, now) = delivered;
        self.close_key(saved);
        if let (Some(main), Some(place)) = (
            self.main.upgrade(),
            self.windows.window_of_doc(&adding.origin),
        ) {
            self.windows
                .show_doc(&main, &self.shell, place, &adding.origin);
        }
        if was != now {
            self.mark_tab(&adding.origin, now);
        }
        self.show_everywhere(&adding.origin, focus_on(adding.target));
    }

    /// Перерисовывает только окно выбора формы `key`: поля формы при этом не трогаются.
    fn show_picker_everywhere(&self, key: &DocKey) {
        let base = self.base.borrow();
        let Some(form) = base.forms.get(key) else {
            return;
        };
        self.each_showing(key, |window| {
            show(&window.global::<FormData>(), form.picker.as_ref());
        });
    }
}

/// Окно выбора в `FormData`; `None`: закрыто. Текст поиска ставится, только если он другой:
/// иначе курсор в поле поиска прыгал бы на каждом ответе ядра.
pub(super) fn show(data: &FormData<'_>, picker: Option<&Picker>) {
    data.set_picker_open(picker.is_some());
    let Some(picker) = picker else {
        data.set_picker_items(ModelRc::default());
        data.set_picker_query("".into());
        return;
    };
    if data.get_picker_query() != picker.query.as_str() {
        data.set_picker_query(picker.query.as_str().into());
    }
    data.set_picker_title(picker.title.as_str().into());
    let items: Vec<PickItem> = picker
        .page
        .iter()
        .flat_map(|page| &page.options)
        .map(|option| PickItem {
            title: option.title.as_str().into(),
            current: Some(option.id) == picker.current,
        })
        .collect();
    data.set_picker_items(ModelRc::new(VecModel::from(items)));
    let note = match &picker.page {
        Some(page) => ref_choice::note(page, !picker.query.trim().is_empty()),
        None => "Загрузка…".to_owned(),
    };
    data.set_picker_note(note.into());
    let (error, retry) = picker
        .error
        .as_ref()
        .map_or(("", false), |(text, retry)| (text.as_str(), *retry));
    data.set_picker_error(error.into());
    data.set_picker_error_action(if retry { super::RETRY } else { "" }.into());
    data.set_picker_can_add(picker.can_add);
    data.set_picker_can_clear(picker.can_clear);
}
