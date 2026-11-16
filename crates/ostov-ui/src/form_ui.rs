//! Форма объекта во вкладке (`ObjectFormView`): справочник или документ по метаданным.
//! Состояние каждой открытой формы (`FormState`) лежит здесь, а не в окне: вкладка может переехать
//! в другое окно, а окно показывает форму своей активной вкладки (глобальный `FormData` у каждого
//! окна, как `ListData` у списка). Ядро зовётся только в фоне: чтение, запись, проведение.
//! Правки идут в `FormState` текстом, в модель полей не пишутся на каждую букву (курсор).
//!
//! Входы для списка: `FormUi::open_form` (открыть записанный объект) и `FormUi::open_new`
//! (новый). Выбор ссылки из справочника — в `picker`, табличные части и окно строки — в `parts`.

mod parts;
mod picker;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::engine::{
    Engine, EngineError, EngineResult, ErrorCode, ObjectData, ObjectMeta, PostAction, Saved,
    call_in_background,
};
use crate::form_state::{FormField, FormState, Status};
use crate::form_table::{RowDraft, TablePart};
use crate::ref_choice;
use crate::shell::{DocKey, FORM_VIEW, Shell};
use crate::values::date_text;
use crate::windows::Windows;
use crate::workspace::{DocTabs, WindowRef};
use crate::{AppWindow, EditorKind, FormData, FormFieldItem, LineColumn, LineRow, TablePartItem};
use picker::{Adding, PickTarget, Picker};

/// Подпись кнопки в полосе ошибки: взять объект из ядра заново.
const RELOAD: &str = "Перечитать";
const RETRY: &str = "Повторить";

/// Границы лет у поля даты (`DateField`): календарь открывается на сегодняшнем дне, если даты
/// нет, и системные часы с другим годом его бы сломали.
const YEAR_MIN: u16 = 1990;
const YEAR_MAX: u16 = 2060;

/// Почему форму не удалось открыть.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// База не открыта.
    NoBase,
    /// Такого объекта нет в метаданных базы или его читать нельзя.
    UnknownObject,
    /// Нового объекта пользователю создавать нельзя.
    NotAllowed,
    /// У объекта нет пункта в меню: вкладка живёт в пункте навигации, форме негде открыться.
    NoMenuItem,
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoBase => "База не открыта.",
            Self::UnknownObject => "Такого объекта нет в конфигурации базы.",
            Self::NotAllowed => "Нет права создавать такие объекты.",
            Self::NoMenuItem => "Объект не вынесен в меню, открыть его форму негде.",
        })
    }
}

impl std::error::Error for OpenError {}

/// Что делает кнопка в полосе над формой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeAction {
    /// «Перечитать»: объект из ядра заново, правки в форме пропадают.
    Reload,
    /// «Повторить» чтение, которое не удалось.
    RetryLoad,
    /// «Повторить» запись тем же действием: данные были заняты или пропала связь.
    RetrySubmit(PostAction),
}

impl NoticeAction {
    fn label(self) -> &'static str {
        match self {
            Self::Reload => RELOAD,
            Self::RetryLoad | Self::RetrySubmit(_) => RETRY,
        }
    }
}

/// Что стоит над формой: ошибка или предупреждение и, если есть, кнопка.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    text: String,
    action: Option<NoticeAction>,
}

impl Notice {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            action: None,
        }
    }

    /// Ошибка записи или проведения `action`. Конфликт версий лечится перечитыванием, занятые
    /// данные и обрыв связи — повтором того же действия (ядро уже повторило само несколько раз),
    /// остальное сообщение ядра говорит само.
    fn from_save_error(error: &EngineError, action: PostAction) -> Self {
        match error.code {
            ErrorCode::VersionConflict => Self {
                text: "Объект изменён другим пользователем. Нажмите «Перечитать»: правки в форме пропадут."
                    .to_owned(),
                action: Some(NoticeAction::Reload),
            },
            ErrorCode::Unavailable => Self {
                text: error.message.clone(),
                action: Some(NoticeAction::RetrySubmit(action)),
            },
            _ => Self::plain(error.message.clone()),
        }
    }
}

/// Что в форме сейчас.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    /// Объект читается из ядра.
    Loading,
    /// Прочитать не удалось: над формой ошибка и «Повторить».
    Failed,
    Ready(FormState),
}

/// Одна открытая форма: состояние и то, что вокруг него (заголовок, ошибка, запись идёт).
struct Form {
    content: Content,
    notice: Option<Notice>,
    /// Идёт вызов ядра (запись или перечитывание): поля и кнопки закрыты.
    busy: bool,
    /// Номер последнего запроса к ядру: ответ с другим номером устарел, его выбрасываем.
    request: u64,
    /// Название формы: у объекта в ядре его представление, пока оно не прочитано, имя типа.
    title: String,
    /// Открытое окно выбора ссылки.
    picker: Option<Picker>,
    /// Куда вернуть фокус при следующем показе: в поле, куда подставили элемент из другой вкладки.
    refocus: Option<PickTarget>,
}

impl Form {
    fn loading(title: &str, request: u64) -> Self {
        Self {
            content: Content::Loading,
            notice: None,
            busy: false,
            request,
            title: title.to_owned(),
            picker: None,
            refocus: None,
        }
    }

    fn ready(state: FormState) -> Self {
        Self {
            title: state.presentation(),
            content: Content::Ready(state),
            notice: None,
            busy: false,
            request: 0,
            picker: None,
            refocus: None,
        }
    }

    fn state(&self) -> Option<&FormState> {
        match &self.content {
            Content::Ready(state) => Some(state),
            Content::Loading | Content::Failed => None,
        }
    }

    fn state_mut(&mut self) -> Option<&mut FormState> {
        match &mut self.content {
            Content::Ready(state) => Some(state),
            Content::Loading | Content::Failed => None,
        }
    }
}

#[derive(Default)]
struct Base {
    engine: Option<Arc<dyn Engine>>,
    objects: Vec<ObjectMeta>,
    /// Открытые формы по ключу вкладки. Закрытые вкладки попадают сюда не сразу, а при следующем
    /// открытии формы (`FormUi::sweep`); открытие же всегда читает объект заново, если вкладки
    /// у него нет, поэтому лишняя запись правок не воскресит.
    forms: HashMap<DocKey, Form>,
    /// Формы новых элементов, открытые из выбора ссылки, по их ключу: куда подставить элемент.
    adding: HashMap<DocKey, Adding>,
}

impl Base {
    fn meta(&self, object: &str) -> Option<&ObjectMeta> {
        self.objects.iter().find(|meta| meta.name == object)
    }
}

/// Кого известить о записи объекта: имя объекта, что записало ядро (`None`: прочитать заново не
/// удалось) и новый ли это объект.
type SavedListener = Rc<dyn Fn(&str, Option<&ObjectData>, bool)>;

/// Формы объектов открытой базы во всех окнах с вкладками.
#[derive(Clone)]
pub struct FormUi {
    main: slint::Weak<AppWindow>,
    windows: Windows,
    shell: Rc<RefCell<Shell>>,
    base: Rc<RefCell<Base>>,
    /// Форма, у которой закрытие вкладки ждёт ответа «Закрыть без записи?».
    closing: Rc<RefCell<Option<DocKey>>>,
    saved: Rc<RefCell<Option<SavedListener>>>,
    /// Счётчики общие на все копии и переживают смену базы: ответ ядра прошлой базы не должен
    /// совпасть с номером запроса новой.
    requests: Rc<Cell<u64>>,
    temporary: Rc<Cell<i64>>,
}

impl FormUi {
    pub fn new(main: slint::Weak<AppWindow>, windows: Windows, shell: Rc<RefCell<Shell>>) -> Self {
        let ui = Self {
            main,
            windows,
            shell,
            base: Rc::default(),
            closing: Rc::default(),
            saved: Rc::default(),
            requests: Rc::default(),
            temporary: Rc::default(),
        };
        // Окна и форма живут, пока работает приложение: цикл ссылок между ними не утечка.
        let guard = ui.clone();
        ui.windows
            .on_close_guard(move |window, index| guard.guard_close(window, index));
        ui
    }

    /// Подписывается на запись объекта (список обновляет строку). Подписчик один, новый заменяет.
    pub fn on_saved(&self, listener: impl Fn(&str, Option<&ObjectData>, bool) + 'static) {
        *self.saved.borrow_mut() = Some(Rc::new(listener));
    }

    /// База открыта: формы прошлой базы забыты.
    pub fn start(&self, engine: Arc<dyn Engine>, objects: &[ObjectMeta]) {
        *self.base.borrow_mut() = Base {
            engine: Some(engine),
            objects: objects.to_vec(),
            forms: HashMap::new(),
            adding: HashMap::new(),
        };
    }

    /// База закрыта: формы и их запросы к ядру больше не нужны.
    pub fn clear(&self) {
        *self.base.borrow_mut() = Base::default();
        self.each_window(|window| show_nothing(window, ""));
    }

    /// Открывает форму записанного объекта в активном окне. Уже открытая выбирается там, где
    /// открыта: одна правка не должна идти в двух формах сразу.
    ///
    /// # Errors
    /// База не открыта, объекта нет в метаданных или в меню.
    pub fn open_form(&self, key: DocKey) -> Result<(), OpenError> {
        let main = self.main.upgrade().ok_or(OpenError::NoBase)?;
        if let Some(window) = self.windows.window_of_doc(&key) {
            self.windows.show_doc(&main, &self.shell, window, &key);
            return Ok(());
        }
        self.sweep();
        let title = {
            let mut base = self.base.borrow_mut();
            base.engine.as_ref().ok_or(OpenError::NoBase)?;
            let title = base
                .meta(&key.object)
                .ok_or(OpenError::UnknownObject)?
                .title
                .clone();
            let form = Form::loading(&title, self.next_request());
            base.forms.insert(key.clone(), form);
            title
        };
        self.open_tab(&key, &title)?;
        self.start_load(&key);
        Ok(())
    }

    /// Открывает форму нового объекта в активном окне. Тип `object` — имя объекта в ядре.
    ///
    /// # Errors
    /// База не открыта, объекта нет в метаданных или в меню, создавать его нельзя.
    pub fn open_new(&self, object: &str) -> Result<(), OpenError> {
        self.open_new_key(object).map(|_| ())
    }

    /// Как `open_new`, но отдаёт временный ключ новой формы.
    fn open_new_key(&self, object: &str) -> Result<DocKey, OpenError> {
        self.sweep();
        let key = DocKey::new(object, self.next_temporary());
        let title = {
            let mut base = self.base.borrow_mut();
            base.engine.as_ref().ok_or(OpenError::NoBase)?;
            let meta = base.meta(object).ok_or(OpenError::UnknownObject)?;
            if !meta.rights.create {
                return Err(OpenError::NotAllowed);
            }
            let form = Form::ready(FormState::new_object(meta));
            let title = form.title.clone();
            base.forms.insert(key.clone(), form);
            title
        };
        if let Err(error) = self.open_tab(&key, &title) {
            self.base.borrow_mut().forms.remove(&key);
            return Err(error);
        }
        Ok(key)
    }

    /// Подключает данные и события формы к окну `window` (главному или дополнительному).
    pub fn attach_window(&self, window: &AppWindow) {
        let data = window.global::<FormData>();
        let bound = || (self.clone(), window.as_weak());
        data.on_show({
            let (ui, window) = bound();
            move |object, id| {
                if let (Some(window), Ok(id)) = (window.upgrade(), id.parse()) {
                    ui.show(&window, &DocKey::new(&object, id));
                }
            }
        });
        data.on_edited({
            let (ui, window) = bound();
            move |index, text| {
                if let (Some(window), Ok(index)) = (window.upgrade(), usize::try_from(index)) {
                    ui.edit(&window, index, &text);
                }
            }
        });
        data.on_date_edited({
            let (ui, window) = bound();
            move |index, day, month, year| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                if let Some((index, date)) = date_args(index, day, month, year) {
                    ui.edit(&window, index, &date_text(date));
                }
            }
        });
        data.on_save({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.submit(&window, PostAction::Write);
                }
            }
        });
        data.on_post({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.submit(&window, PostAction::Post);
                }
            }
        });
        data.on_unpost({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.submit(&window, PostAction::Unpost);
                }
            }
        });
        data.on_reload({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.reload(&window);
                }
            }
        });
        data.on_notice_action({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.notice_action(&window);
                }
            }
        });
        data.on_close({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.close(&window);
                }
            }
        });
        data.on_ask_cancelled({
            let ui = self.clone();
            move || *ui.closing.borrow_mut() = None
        });
        self.attach_picker(window);
        self.attach_parts(window);
    }

    fn next_request(&self) -> u64 {
        self.requests.set(self.requests.get().saturating_add(1));
        self.requests.get()
    }

    /// Ключ для нового объекта, пока ядро не дало ему `id`: отрицательный, ядро таких не выдаёт.
    fn next_temporary(&self) -> i64 {
        self.temporary.set(self.temporary.get().saturating_sub(1));
        self.temporary.get()
    }

    /// Забывает формы, чьих вкладок уже нет ни в одном окне.
    fn sweep(&self) {
        if let Some(open) = self.windows.open_documents() {
            let mut base = self.base.borrow_mut();
            let Base { forms, adding, .. } = &mut *base;
            forms.retain(|key, _| open.contains(key));
            adding.retain(|key, _| forms.contains_key(key));
        }
    }

    fn each_window(&self, mut show: impl FnMut(&AppWindow)) {
        if let Some(main) = self.main.upgrade() {
            show(&main);
        }
        self.windows.each(show);
    }

    /// Окна, в которых сейчас показана форма `key`.
    fn each_showing(&self, key: &DocKey, mut show: impl FnMut(&AppWindow)) {
        self.each_window(|window| {
            if shown_key(window).as_ref() == Some(key) {
                show(window);
            }
        });
    }

    /// Рисует форму `key` во всех окнах, где она показана.
    fn show_everywhere(&self, key: &DocKey, draw: Draw) {
        let base = self.base.borrow();
        let Some(form) = base.forms.get(key) else {
            return;
        };
        self.each_showing(key, |window| render(window, key, form, &base.objects, draw));
    }

    /// Меняет состояние формы, показанной в окне `window`, и перерисовывает её так, как скажет
    /// `change` (`None`: ничего не поменялось). Пока идёт запись, форма закрыта для правок.
    fn change(&self, window: &AppWindow, change: impl FnOnce(&mut FormState) -> Option<Draw>) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let (was, now, draw) = {
            let mut base = self.base.borrow_mut();
            let Some(state) = base
                .forms
                .get_mut(&key)
                .filter(|form| !form.busy)
                .and_then(Form::state_mut)
            else {
                return;
            };
            let was = state.is_modified();
            let draw = change(state);
            (was, state.is_modified(), draw)
        };
        if let Some(draw) = draw {
            self.show_everywhere(&key, draw);
        }
        if was != now {
            self.mark_tab(&key, now);
        }
    }

    /// Вкладки окна, в котором открыт документ `key`.
    fn with_tabs<R>(&self, key: &DocKey, f: impl FnOnce(&mut dyn DocTabs) -> R) -> Option<R> {
        match self.windows.window_of_doc(key)? {
            WindowRef::Main => Some(f(&mut *self.shell.borrow_mut())),
            WindowRef::Extra(id) => self.windows.with_list(id, |list| f(list)),
        }
    }

    /// Показывает изменения вкладок окна, в котором открыт документ `key`.
    fn refresh_tabs(&self, key: &DocKey) {
        match self.windows.window_of_doc(key) {
            Some(WindowRef::Main) => {
                if let Some(main) = self.main.upgrade() {
                    crate::app::refresh(&main, &self.shell.borrow());
                }
            }
            Some(WindowRef::Extra(id)) => self.windows.refresh_host(id),
            None => {}
        }
    }

    /// Открывает вкладку формы в активном окне и показывает её.
    fn open_tab(&self, key: &DocKey, title: &str) -> Result<(), OpenError> {
        let node = self
            .shell
            .borrow()
            .node_for_object(&key.object)
            .ok_or(OpenError::NoMenuItem)?;
        match self.windows.active() {
            WindowRef::Main => {
                self.shell.borrow_mut().open_document(
                    node,
                    key.clone(),
                    title.to_owned(),
                    FORM_VIEW,
                );
                if let Some(main) = self.main.upgrade() {
                    crate::app::refresh(&main, &self.shell.borrow());
                }
            }
            WindowRef::Extra(id) => {
                self.windows.with_list(id, |list| {
                    list.open_document(node, key.clone(), title.to_owned(), FORM_VIEW);
                });
                self.windows.refresh_host(id);
            }
        }
        Ok(())
    }

    /// Заголовок и признак «изменено» у вкладки документа.
    fn sync_tab(&self, key: &DocKey, title: &str, modified: bool) {
        self.with_tabs(key, |tabs| {
            tabs.set_title_of_doc(key, title.to_owned());
            if let Some(index) = tabs.tab_of_doc(key) {
                tabs.set_tab_modified(index, modified);
            }
        });
        self.refresh_tabs(key);
    }

    /// Окно показывает форму: из состояния, которое лежит у `FormUi`.
    fn show(&self, window: &AppWindow, key: &DocKey) {
        let refocus = self
            .base
            .borrow_mut()
            .forms
            .get_mut(key)
            .and_then(|form| form.refocus.take());
        let draw = refocus.map_or(Draw::Rebuild { focus: true }, picker::focus_on);
        let base = self.base.borrow();
        match base.forms.get(key) {
            Some(form) => render(window, key, form, &base.objects, draw),
            None => show_nothing(
                window,
                "Форма не открыта. Закройте вкладку и откройте объект заново.",
            ),
        }
        // Вопрос о закрытии относится к одной форме: на соседней вкладке его не видно.
        let asking = self.closing.borrow().as_ref() == Some(key);
        window.global::<FormData>().set_ask_close(asking);
    }

    /// Вкладку `index` окна `window` просят закрыть. Форма с несохранёнными правками остаётся:
    /// вкладка выбирается и спрашивает, закрывать ли без записи. Пока идёт запись, вкладка
    /// тоже остаётся. `true`: закрывать не надо.
    fn guard_close(&self, window: WindowRef, index: usize) -> bool {
        let key = match window {
            WindowRef::Main => self
                .shell
                .borrow()
                .tabs()
                .get(index)
                .and_then(|t| t.doc.clone()),
            WindowRef::Extra(id) => self
                .windows
                .with_list(id, |list| {
                    list.tabs().get(index).and_then(|t| t.doc.clone())
                })
                .flatten(),
        };
        let Some(key) = key else {
            return false;
        };
        let (busy, modified) = {
            let base = self.base.borrow();
            let Some(form) = base.forms.get(&key) else {
                return false;
            };
            (form.busy, form.state().is_some_and(FormState::is_modified))
        };
        if !busy && !modified {
            return false;
        }
        if !busy {
            *self.closing.borrow_mut() = Some(key.clone());
            match window {
                WindowRef::Main => self.shell.borrow_mut().select_tab(index),
                WindowRef::Extra(id) => {
                    self.windows.with_list(id, |list| list.select(index));
                }
            }
            self.windows.set_active(window);
            self.refresh_tabs(&key);
            self.each_showing(&key, |w| w.global::<FormData>().set_ask_close(true));
        }
        true
    }

    /// Правка поля `index` текстом.
    fn edit(&self, window: &AppWindow, index: usize, text: &str) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let (was, now) = {
            let mut base = self.base.borrow_mut();
            let Some(state) = base
                .forms
                .get_mut(&key)
                .filter(|form| !form.busy)
                .and_then(Form::state_mut)
            else {
                return;
            };
            let was = state.is_modified();
            state.edit(index, text);
            (was, state.is_modified())
        };
        {
            let base = self.base.borrow();
            if let Some(state) = base.forms.get(&key).and_then(Form::state) {
                let data = window.global::<FormData>();
                show_flags(&data, Some(state));
                update_row(&data, state, index, &base.objects);
            }
        }
        if was != now {
            self.mark_tab(&key, now);
        }
    }

    /// Вкладке отмечается «изменено»: она закрепляется и показывает точку.
    fn mark_tab(&self, key: &DocKey, modified: bool) {
        self.with_tabs(key, |tabs| {
            if let Some(index) = tabs.tab_of_doc(key) {
                tabs.set_tab_modified(index, modified);
            }
        });
        self.refresh_tabs(key);
    }

    /// Кнопка в полосе над формой: перечитать или повторить то, что не удалось.
    fn notice_action(&self, window: &AppWindow) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let action = self
            .base
            .borrow()
            .forms
            .get(&key)
            .and_then(|form| form.notice.as_ref())
            .and_then(|notice| notice.action);
        match action {
            Some(NoticeAction::Reload | NoticeAction::RetryLoad) => self.reload(window),
            Some(NoticeAction::RetrySubmit(action)) => self.submit(window, action),
            None => {}
        }
    }

    /// «Записать», «Провести», «Отменить проведение»: проверка, потом запись в фоне.
    fn submit(&self, window: &AppWindow, action: PostAction) {
        let Some(key) = shown_key(window) else {
            return;
        };
        let request = self.next_request();
        let job = {
            let mut base = self.base.borrow_mut();
            let Some(engine) = base.engine.clone() else {
                return;
            };
            let Some(form) = base.forms.get_mut(&key).filter(|form| !form.busy) else {
                return;
            };
            let Content::Ready(state) = &mut form.content else {
                return;
            };
            match state.to_data() {
                Ok(data) => {
                    let object = state.object().to_owned();
                    form.notice = None;
                    form.busy = true;
                    form.request = request;
                    Some((engine, object, data))
                }
                Err(invalid) => {
                    form.notice = Some(Notice::plain(invalid.to_string()));
                    None
                }
            }
        };
        self.show_everywhere(&key, Draw::Refresh);
        let Some((engine, object, data)) = job else {
            return;
        };
        let ui = self.clone();
        call_in_background(
            move || save_and_read(&*engine, &object, &data, action),
            move |result| ui.saved(&key, request, action, result),
        );
    }

    /// Ответ ядра на запись: форма берёт то, что записало ядро (номер, суммы, версию).
    fn saved(
        &self,
        key: &DocKey,
        request: u64,
        action: PostAction,
        result: EngineResult<SaveOutcome>,
    ) {
        let saved_ok = result.is_ok();
        // Что узнает список: записанное ядром (или `None`, если прочитать не удалось) и новизна.
        let announce = result
            .as_ref()
            .ok()
            .map(|outcome| (outcome.fresh.as_ref().ok().cloned(), key.is_temporary()));
        let (new_key, title) = {
            let mut base = self.base.borrow_mut();
            let Base {
                objects,
                forms,
                adding,
                ..
            } = &mut *base;
            if forms.get(key).is_none_or(|form| form.request != request) {
                return;
            }
            let Some(mut form) = forms.remove(key) else {
                return;
            };
            form.busy = false;
            let new_key = match result {
                Ok(outcome) => {
                    let new_key = DocKey::new(&key.object, outcome.saved.id);
                    apply_outcome(&mut form, objects, &key.object, outcome, action);
                    new_key
                }
                Err(error) => {
                    form.notice = Some(Notice::from_save_error(&error, action));
                    key.clone()
                }
            };
            let title = form.title.clone();
            forms.insert(new_key.clone(), form);
            if new_key != *key {
                // Форма, из которой открыт выбор с «+ Добавить», тоже могла получить свой `id`.
                for adding in adding.values_mut() {
                    if adding.origin == *key {
                        adding.origin = new_key.clone();
                    }
                }
            }
            (new_key, title)
        };
        if new_key != *key {
            // Новый объект получил настоящий `id`: вкладка и окна, где он показан, следуют за ним.
            self.each_showing(key, |window| {
                window
                    .global::<FormData>()
                    .set_doc_id(new_key.id.to_string().into());
            });
            self.with_tabs(key, |tabs| tabs.rekey(key, &new_key));
        }
        let modified = self
            .base
            .borrow()
            .forms
            .get(&new_key)
            .and_then(Form::state)
            .is_some_and(FormState::is_modified);
        self.sync_tab(&new_key, &title, modified);
        // Записано: форма взяла, что вернуло ядро, поля строятся заново. Отказ ядра поля не
        // трогает, набранное остаётся на месте.
        self.show_everywhere(
            &new_key,
            if saved_ok {
                Draw::Rebuild { focus: false }
            } else {
                Draw::Refresh
            },
        );
        if let Some((fresh, created)) = announce {
            // Заём снимаем до вызова: список в ответ читает окна и сам может открывать формы.
            let listener = self.saved.borrow().clone();
            if let Some(listener) = listener {
                listener(&key.object, fresh.as_ref(), created);
            }
        }
        if saved_ok && new_key != *key {
            self.deliver_added(key, &new_key);
        }
    }

    /// «Перечитать» и «Повторить»: объект из ядра заново.
    fn reload(&self, window: &AppWindow) {
        let Some(key) = shown_key(window) else {
            return;
        };
        if key.is_temporary() {
            return;
        }
        self.start_load(&key);
    }

    /// Читает объект из ядра в фоне. Форма, которая уже на экране, остаётся на нём закрытой, пока
    /// нет ответа; пустая показывает «Загрузка…».
    fn start_load(&self, key: &DocKey) {
        let request = self.next_request();
        let engine = {
            let mut base = self.base.borrow_mut();
            let Some(engine) = base.engine.clone() else {
                return;
            };
            let Some(form) = base.forms.get_mut(key) else {
                return;
            };
            form.request = request;
            form.notice = None;
            match form.content {
                Content::Ready(_) => form.busy = true,
                Content::Loading | Content::Failed => form.content = Content::Loading,
            }
            engine
        };
        self.show_everywhere(key, Draw::Refresh);
        let (ui, object, id) = (self.clone(), key.object.clone(), key.id);
        let key = key.clone();
        call_in_background(
            move || engine.get(&object, id),
            move |result| ui.loaded(&key, request, result),
        );
    }

    fn loaded(&self, key: &DocKey, request: u64, result: EngineResult<ObjectData>) {
        let title = {
            let mut base = self.base.borrow_mut();
            let Base { objects, forms, .. } = &mut *base;
            let Some(form) = forms.get_mut(key).filter(|form| form.request == request) else {
                return;
            };
            form.busy = false;
            match result {
                Ok(data) => {
                    let Some(meta) = objects.iter().find(|meta| meta.name == key.object) else {
                        return;
                    };
                    let state = FormState::from_data(meta, &data);
                    form.title = state.presentation();
                    form.content = Content::Ready(state);
                    form.notice = None;
                }
                Err(error) => {
                    // Перечитывание открытой формы не удалось: правки остаются, можно повторить.
                    form.notice = Some(Notice {
                        text: error.message,
                        action: Some(NoticeAction::RetryLoad),
                    });
                    if !matches!(form.content, Content::Ready(_)) {
                        form.content = Content::Failed;
                    }
                }
            }
            form.title.clone()
        };
        self.sync_tab(key, &title, false);
        self.show_everywhere(key, Draw::Rebuild { focus: true });
    }

    /// «Закрыть»: форма забывается, вкладка закрывается. Спрашивать о несохранённом успела
    /// сама форма.
    fn close(&self, window: &AppWindow) {
        if let Some(key) = shown_key(window) {
            self.close_key(&key);
        }
    }

    /// Закрывает форму `key` и её вкладку, где бы она ни стояла.
    fn close_key(&self, key: &DocKey) {
        if self
            .base
            .borrow()
            .forms
            .get(key)
            .is_some_and(|form| form.busy)
        {
            return;
        }
        let place = self.windows.window_of_doc(key);
        self.base.borrow_mut().forms.remove(key);
        *self.closing.borrow_mut() = None;
        match place {
            Some(WindowRef::Main) => {
                let Some(main) = self.main.upgrade() else {
                    return;
                };
                let mut shell = self.shell.borrow_mut();
                if let Some(index) = shell.doc_tab(key) {
                    shell.close_tab(index);
                }
                crate::app::refresh(&main, &shell);
            }
            Some(WindowRef::Extra(id)) => {
                // Закрывается активная вкладка окна: сначала выбираем вкладку этой формы.
                self.windows.with_list(id, |list| {
                    if let Some(index) = list.tab_of_doc(key) {
                        list.select(index);
                    }
                });
                if let Some(main) = self.main.upgrade() {
                    self.windows.close_active_tab(&main, &self.shell, id);
                }
            }
            None => {}
        }
    }
}

/// Результат записи: что записало ядро и что оно отдало при чтении после записи.
struct SaveOutcome {
    saved: Saved,
    fresh: EngineResult<ObjectData>,
}

/// Записывает объект и сразу читает его: номер, суммы и версию считает ядро. Запись прошла, а
/// чтение нет, это не ошибка записи: повторная запись нового объекта создала бы второй.
fn save_and_read(
    engine: &dyn Engine,
    object: &str,
    data: &ObjectData,
    action: PostAction,
) -> EngineResult<SaveOutcome> {
    let saved = engine.save(object, data, action)?;
    let fresh = engine.get(object, saved.id);
    Ok(SaveOutcome { saved, fresh })
}

/// Кладёт ответ ядра на запись в форму.
fn apply_outcome(
    form: &mut Form,
    objects: &[ObjectMeta],
    object: &str,
    outcome: SaveOutcome,
    action: PostAction,
) {
    let SaveOutcome { saved, fresh } = outcome;
    match fresh {
        Ok(data) => {
            if let Some(meta) = objects.iter().find(|meta| meta.name == object) {
                form.content = Content::Ready(FormState::from_data(meta, &data));
                form.notice = None;
            }
        }
        Err(error) => {
            if let Some(state) = form.state_mut() {
                state.apply_saved(&saved, action);
            }
            form.notice = Some(Notice {
                text: format!(
                    "Записано, но заново прочитать не удалось: {}",
                    error.message
                ),
                action: Some(NoticeAction::Reload),
            });
        }
    }
    if let Some(state) = form.state() {
        form.title = state.presentation();
    }
}

// ---------------------------------------------------------------- рисование в окне

/// Ключ формы, которая показана в окне: объект и `id` лежат в `FormData`.
fn shown_key(window: &AppWindow) -> Option<DocKey> {
    let data = window.global::<FormData>();
    let id = data.get_doc_id().parse().ok()?;
    let object = data.get_object();
    (!object.is_empty()).then(|| DocKey::new(&object, id))
}

/// `(индекс поля, день, месяц, год)` из колбэка поля даты; `None`, если числа не те.
fn date_args(index: i32, day: i32, month: i32, year: i32) -> Option<(usize, (u16, u8, u8))> {
    Some((
        usize::try_from(index).ok()?,
        (
            u16::try_from(year).ok()?,
            u8::try_from(month).ok()?,
            u8::try_from(day).ok()?,
        ),
    ))
}

/// Как перерисовать форму в окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Draw {
    /// Поля шапки и окна строки строятся заново и берут значения из состояния; `focus`: первое
    /// поле для ввода берёт фокус. Так форма открывается, читается из ядра и показывает
    /// записанное.
    Rebuild { focus: bool },
    /// Подписи, кнопки, ошибки и значения полей меняются на месте: редакторы остаются, фокус
    /// и положение курсора тоже. Так форма закрывается на время записи и показывает отказ ядра.
    Refresh,
    /// Поля шапки строятся заново, фокус берёт поле с этим номером: так форма возвращается из
    /// выбора ссылки.
    FocusField(usize),
    /// Окно строки строится заново, фокус у ячейки с этим номером (`None`: первая для ввода):
    /// так окно строки открывается и возвращается из выбора ссылки.
    FocusCell(Option<usize>),
}

/// Показывает форму в окне: заголовок, кнопки, ошибку, поля, табличные части, окно строки и
/// выбор ссылки.
fn render(window: &AppWindow, key: &DocKey, form: &Form, objects: &[ObjectMeta], draw: Draw) {
    let data = window.global::<FormData>();
    data.set_object(key.object.as_str().into());
    data.set_doc_id(key.id.to_string().into());
    data.set_title(form.title.as_str().into());
    data.set_loading(matches!(form.content, Content::Loading));
    data.set_busy(form.busy);
    let state = form.state();
    // Пока открыто окно поверх формы, поле шапки фокус не берёт: он нужен окну.
    let covered = form.picker.is_some() || state.is_some_and(|s| s.parts().draft().is_some());
    let head_focus = match draw {
        Draw::Rebuild { focus } => Some(focus.then(|| first_input(state)).flatten()),
        Draw::FocusField(index) => Some(Some(index)),
        Draw::Refresh | Draw::FocusCell(_) => None,
    };
    match head_focus {
        Some(focus) => {
            let items: Vec<FormFieldItem> = state.map_or_else(Vec::new, |state| {
                (0..state.fields().len())
                    .filter_map(|index| field_item(state, index, objects))
                    .collect()
            });
            // Новая модель пересоздаёт редакторы: набранное в старых не переживёт замену
            // значений, а нужное поле успеет взять фокус.
            data.set_fields(ModelRc::new(VecModel::from(items)));
            data.set_autofocus_field(
                focus
                    .filter(|_| !form.busy && !covered)
                    .and_then(|index| i32::try_from(index).ok())
                    .unwrap_or(-1),
            );
        }
        None => {
            if let Some(state) = state {
                for index in 0..state.fields().len() {
                    update_row(&data, state, index, objects);
                }
            }
        }
    }
    show_parts(&data, state);
    show_row(&data, state, objects, draw, form.picker.is_some());
    picker::show(&data, form.picker.as_ref());
    data.set_error(form.notice.as_ref().map_or("", |n| n.text.as_str()).into());
    data.set_error_action(
        form.notice
            .as_ref()
            .and_then(|notice| notice.action)
            .map_or("", NoticeAction::label)
            .into(),
    );
    show_flags(&data, state);
}

/// Первое поле шапки, куда можно ввести или выбрать значение.
fn first_input(state: Option<&FormState>) -> Option<usize> {
    let state = state?;
    (0..state.fields().len()).find(|index| state.is_editable(*index) || state.can_pick(*index))
}

/// Подписи и кнопки, которые зависят от состояния формы и меняются при правке.
fn show_flags(data: &FormData<'_>, state: Option<&FormState>) {
    let status = state.and_then(FormState::status);
    data.set_status(status.map_or("", Status::label).into());
    data.set_status_kind(status.map_or(0, status_kind));
    data.set_modified(state.is_some_and(FormState::is_modified));
    data.set_can_save(state.is_some_and(FormState::can_save));
    data.set_can_post(state.is_some_and(FormState::can_post));
    data.set_can_unpost(state.is_some_and(FormState::can_unpost));
    data.set_posted(state.is_some_and(FormState::is_posted));
}

/// Обновляет одно поле на месте: редактор остаётся, набранное в нём не теряется.
fn update_row(data: &FormData<'_>, state: &FormState, index: usize, objects: &[ObjectMeta]) {
    let rows = data.get_fields();
    let Some(model) = rows.as_any().downcast_ref::<VecModel<FormFieldItem>>() else {
        return;
    };
    if index < model.row_count()
        && let Some(item) = field_item(state, index, objects)
    {
        model.set_row_data(index, item);
    }
}

/// Табличные части. Модель с тем же числом частей обновляется на месте: таблицы не
/// пересоздаются и фокус клавиатуры у таблицы не теряется.
fn show_parts(data: &FormData<'_>, state: Option<&FormState>) {
    let items: Vec<TablePartItem> = state.map_or_else(Vec::new, |state| {
        state
            .parts()
            .iter()
            .map(|part| part_item(part, state.can_write()))
            .collect()
    });
    let current = data.get_parts();
    match current.as_any().downcast_ref::<VecModel<TablePartItem>>() {
        Some(model) if model.row_count() == items.len() => {
            for (index, item) in items.into_iter().enumerate() {
                model.set_row_data(index, item);
            }
        }
        _ => data.set_parts(ModelRc::new(VecModel::from(items))),
    }
}

fn part_item(part: &TablePart, editable: bool) -> TablePartItem {
    let columns: Vec<LineColumn> = part.columns().iter().map(line_column).collect();
    let rows: Vec<LineRow> = (0..part.len())
        .map(|row| {
            let cells: Vec<slint::SharedString> = (0..part.columns().len())
                .map(|column| part.cell_text(row, column).into())
                .collect();
            LineRow {
                cells: ModelRc::new(VecModel::from(cells)),
                error: part.row_error(row).unwrap_or_default().into(),
            }
        })
        .collect();
    TablePartItem {
        title: part.title.as_str().into(),
        columns: ModelRc::new(VecModel::from(columns)),
        rows: ModelRc::new(VecModel::from(rows)),
        selected: part
            .selected()
            .and_then(|row| i32::try_from(row).ok())
            .unwrap_or(-1),
        editable,
    }
}

// Ширины колонок табличной части, px: число и дата постоянные, текст и ссылка делят остаток.
const LINE_TEXT_WIDTH: f32 = 160.0;
const LINE_NUMBER_WIDTH: f32 = 120.0;
const LINE_DATE_WIDTH: f32 = 110.0;

fn line_column(def: &ostov_api::FieldDef) -> LineColumn {
    use ostov_api::FieldKind;
    let (width, right, flex) = match def.kind {
        FieldKind::Text | FieldKind::Ref => (LINE_TEXT_WIDTH, false, true),
        FieldKind::Int | FieldKind::Decimal { .. } => (LINE_NUMBER_WIDTH, true, false),
        FieldKind::Date => (LINE_DATE_WIDTH, false, false),
    };
    LineColumn {
        title: def.title.as_str().into(),
        width,
        right,
        flex,
    }
}

/// Окно строки: открыто, пока в состоянии есть черновик строки.
fn show_row(
    data: &FormData<'_>,
    state: Option<&FormState>,
    objects: &[ObjectMeta],
    draw: Draw,
    covered: bool,
) {
    let draft = state.and_then(|state| state.parts().draft().map(|draft| (state, draft)));
    data.set_row_open(draft.is_some());
    let Some((state, draft)) = draft else {
        data.set_row_fields(ModelRc::default());
        data.set_row_autofocus(-1);
        return;
    };
    data.set_row_title(row_title(state, draft).into());
    data.set_row_error(draft.error().unwrap_or_default().into());
    data.set_row_editable(draft.is_editable());
    let items: Vec<FormFieldItem> = (0..draft.cells().len())
        .filter_map(|cell| cell_item(draft, cell, objects))
        .collect();
    let current = data.get_row_fields();
    let reusable = current
        .as_any()
        .downcast_ref::<VecModel<FormFieldItem>>()
        .filter(|model| model.row_count() == items.len());
    let focus = match (draw, reusable) {
        (Draw::Refresh | Draw::FocusField(_), Some(model)) => {
            for (index, item) in items.into_iter().enumerate() {
                model.set_row_data(index, item);
            }
            return;
        }
        (Draw::FocusCell(cell), _) => cell.or_else(|| first_cell(draft)),
        (Draw::Rebuild { focus: true }, _) => first_cell(draft),
        (Draw::Rebuild { focus: false } | Draw::Refresh | Draw::FocusField(_), _) => None,
    };
    data.set_row_fields(ModelRc::new(VecModel::from(items)));
    data.set_row_autofocus(
        focus
            .filter(|_| !covered)
            .and_then(|cell| i32::try_from(cell).ok())
            .unwrap_or(-1),
    );
}

fn first_cell(draft: &RowDraft) -> Option<usize> {
    (0..draft.cells().len()).find(|cell| draft.accepts_text(*cell) || draft.accepts_choice(*cell))
}

/// «Товары: новая строка» или «Товары: строка 3».
fn row_title(state: &FormState, draft: &RowDraft) -> String {
    let part = state
        .parts()
        .get(draft.part())
        .map_or("", |part| part.title.as_str());
    match draft.row() {
        Some(row) => format!("{part}: строка {}", row + 1),
        None => format!("{part}: новая строка"),
    }
}

/// Ячейка окна строки для редактора; `None`, если такой ячейки нет.
fn cell_item(draft: &RowDraft, cell: usize, objects: &[ObjectMeta]) -> Option<FormFieldItem> {
    let field = draft.cells().get(cell)?;
    Some(editor_item(
        field,
        draft.accepts_text(cell),
        draft.accepts_choice(cell),
        objects,
    ))
}

/// Окно без формы: база закрыта или формы нет. Пустое `error`: просто пустой экран.
fn show_nothing(window: &AppWindow, error: &str) {
    let data = window.global::<FormData>();
    data.set_object("".into());
    data.set_doc_id("".into());
    data.set_title("".into());
    data.set_loading(false);
    data.set_busy(false);
    data.set_fields(ModelRc::default());
    data.set_autofocus_field(-1);
    data.set_parts(ModelRc::default());
    data.set_row_open(false);
    data.set_row_fields(ModelRc::default());
    data.set_row_autofocus(-1);
    picker::show(&data, None);
    data.set_error(error.into());
    data.set_error_action("".into());
    data.set_ask_close(false);
    show_flags(&data, None);
}

/// Вид чипа состояния в `FormData.status-kind`.
fn status_kind(status: Status) -> i32 {
    match status {
        Status::New => 0,
        Status::Posted => 1,
        Status::Draft => 2,
        Status::PostedChanged => 4,
        Status::Deleted => 5,
    }
}

fn editor_kind(kind: ostov_api::FieldKind) -> EditorKind {
    use ostov_api::FieldKind;
    match kind {
        FieldKind::Text => EditorKind::Text,
        FieldKind::Int | FieldKind::Decimal { .. } => EditorKind::Number,
        FieldKind::Date => EditorKind::Date,
        FieldKind::Ref => EditorKind::Reference,
    }
}

/// Подпись поля; обязательное помечается звёздочкой.
fn field_label(field: &FormField) -> String {
    if field.required && !field.read_only {
        format!("{} *", field.title)
    } else {
        field.title.clone()
    }
}

/// Поле шапки для редактора; `None`, если поля с таким номером нет.
fn field_item(state: &FormState, index: usize, objects: &[ObjectMeta]) -> Option<FormFieldItem> {
    let field = state.fields().get(index)?;
    Some(editor_item(
        field,
        state.is_editable(index),
        state.can_pick(index),
        objects,
    ))
}

/// Поле или ячейка для редактора. `can_type`: правится текстом, `can_pick`: значение выбирают
/// из справочника. Если выбирать можно, а справочник недоступен, поле закрыто и под ним
/// сказано почему.
fn editor_item(
    field: &FormField,
    can_type: bool,
    can_pick: bool,
    objects: &[ObjectMeta],
) -> FormFieldItem {
    let kind = editor_kind(field.kind);
    let date = field.date();
    let (year, month, day) = date.unwrap_or_else(calendar_start);
    let refused = can_pick
        .then(|| ref_choice::target_of(objects, field.target.as_deref(), &field.title).err())
        .flatten();
    FormFieldItem {
        label: field_label(field).into(),
        kind,
        text: field.text().into(),
        error: field.error().unwrap_or_default().into(),
        hint: refused.as_deref().unwrap_or_default().into(),
        read_only: !(can_type || can_pick && refused.is_none()),
        day: i32::from(day),
        month: i32::from(month),
        year: i32::from(year),
        empty_date: kind == EditorKind::Date && date.is_none(),
    }
}

/// День, на котором открывается календарь пустого поля даты: сегодня по UTC (часового пояса в
/// стандартной библиотеке нет), в пределах лет поля. На сам объект это не влияет: даты в нём
/// нет, пока её не выбрали.
fn calendar_start() -> (u16, u8, u8) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    (year.clamp(YEAR_MIN, YEAR_MAX), month, day)
}

/// Дата по числу дней от 1970-01-01 (алгоритм Говарда Хиннанта, григорианский календарь).
fn civil_from_days(days: i64) -> (u16, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        u16::try_from(year).unwrap_or(0),
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use ostov_api::{FieldDef, FieldKind, Meta, Rights, Value};

    use super::*;

    fn object_meta(name: &str) -> ObjectMeta {
        let config = ostov_meta::demo().unwrap_or_else(|errors| unreachable!("{errors:?}"));
        Meta::new(&config)
            .objects
            .into_iter()
            .find(|object| object.name == name)
            .unwrap_or_else(|| unreachable!("в демо-конфигурации нет {name}"))
    }

    /// Демо-база в своём файле: параллельные тесты не делят файл.
    fn demo_engine(name: &str) -> (Arc<dyn Engine>, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "ostov-ui-form-{name}-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let engine = ostov_core::LocalConnector::default()
            .create_demo(path.to_str().unwrap_or_default(), 20)
            .unwrap();
        (engine, path)
    }

    fn remove(engine: Arc<dyn Engine>, path: &std::path::Path) {
        drop(engine);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite.lock"));
    }

    fn edit(state: &mut FormState, name: &str, text: &str) {
        let index = state
            .fields()
            .iter()
            .position(|field| field.name == name)
            .unwrap();
        state.edit(index, text);
    }

    #[test]
    fn civil_date_from_days_since_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_473), (2026, 1, 20));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn calendar_start_stays_inside_year_range() {
        let (year, month, day) = calendar_start();
        assert!((YEAR_MIN..=YEAR_MAX).contains(&year));
        assert!((1..=12).contains(&month) && (1..=31).contains(&day));
    }

    #[test]
    fn date_arguments_are_checked() {
        assert_eq!(date_args(2, 5, 3, 2026), Some((2, (2026, 3, 5))));
        assert_eq!(date_args(-1, 5, 3, 2026), None);
        assert_eq!(date_args(0, 300, 3, 2026), None);
        assert_eq!(date_args(0, 5, 3, -1), None);
    }

    #[test]
    fn field_items_follow_field_kinds() {
        let meta = ObjectMeta {
            fields: vec![
                FieldDef::new("name", FieldKind::Text).with_title("Наименование"),
                FieldDef::new("count", FieldKind::Int),
                FieldDef::new("price", FieldKind::Decimal { scale: 2 }),
                FieldDef::new("day", FieldKind::Date),
                FieldDef::reference("owner", "users"),
            ],
            rights: Rights::ALL,
            ..object_meta("goods")
        };
        let state = FormState::new_object(&meta);
        let items: Vec<FormFieldItem> = (0..state.fields().len())
            .filter_map(|index| field_item(&state, index, &[]))
            .collect();
        let kinds: Vec<EditorKind> = items.iter().map(|item| item.kind).collect();
        assert_eq!(
            kinds,
            [
                EditorKind::Text,
                EditorKind::Number,
                EditorKind::Number,
                EditorKind::Date,
                EditorKind::Reference
            ]
        );
        // Ссылка на объект, которого нет в метаданных, закрыта, и под ней сказано почему; у
        // пустой даты календарь открывается на сегодняшнем дне.
        let read_only: Vec<bool> = items.iter().map(|item| item.read_only).collect();
        assert_eq!(read_only, [false, false, false, false, true]);
        assert!(items[4].hint.contains("выбор недоступен"));
        assert!(items[0].hint.is_empty());
        assert!(items[3].empty_date);
        assert!(items[3].year >= i32::from(YEAR_MIN));
        assert!(!items[0].empty_date);
    }

    #[test]
    fn date_field_item_carries_its_date() {
        let mut state = FormState::new_object(&object_meta("receipt"));
        edit(&mut state, "date", "15.03.2026");
        let item = field_item(&state, 0, &[]).unwrap();
        assert_eq!((item.day, item.month, item.year), (15, 3, 2026));
        assert!(!item.empty_date);
        assert_eq!(item.text, "15.03.2026");
    }

    #[test]
    fn required_fields_are_marked_with_an_asterisk() {
        let state = FormState::new_object(&object_meta("receipt"));
        let labels: Vec<String> = state.fields().iter().map(field_label).collect();
        // Дата, поставщик и склад обязательные; сумма вычисляется, звёздочка ей не нужна.
        assert_eq!(labels, ["Дата *", "Поставщик *", "Склад *", "Сумма"]);
    }

    fn demo_objects() -> Vec<ObjectMeta> {
        let config = ostov_meta::demo().unwrap_or_else(|errors| unreachable!("{errors:?}"));
        Meta::new(&config).objects
    }

    #[test]
    fn reference_to_a_readable_catalog_is_open_for_choice() {
        let mut meta = object_meta("issue");
        meta.rights = Rights::ALL;
        let state = FormState::new_object(&meta);
        let warehouse = state
            .fields()
            .iter()
            .position(|field| field.name == "warehouse")
            .unwrap();
        let item = field_item(&state, warehouse, &demo_objects()).unwrap();
        assert_eq!(item.kind, EditorKind::Reference);
        assert!(!item.read_only);
        assert!(item.hint.is_empty());
        // Без права на справочник поле закрыто и объясняет почему.
        let mut objects = demo_objects();
        objects.retain(|object| object.name != "warehouses");
        let item = field_item(&state, warehouse, &objects).unwrap();
        assert!(item.read_only);
        assert_eq!(
            item.hint,
            "Нет доступа к данным для поля «Склад»: выбор недоступен."
        );
    }

    #[test]
    fn table_part_item_has_columns_rows_and_errors() {
        let mut meta = object_meta("issue");
        meta.rights = Rights::ALL;
        let mut data = ObjectData::default();
        data.tables.insert(
            "lines".to_owned(),
            vec![
                [(
                    "goods".to_owned(),
                    Value::Ref {
                        id: 5,
                        title: Some("Мука".to_owned()),
                    },
                )]
                .into(),
            ],
        );
        let mut state = FormState::from_data(&meta, &data);
        assert!(state.to_data().is_err());
        let part = state.parts().get(0).unwrap();
        let item = part_item(part, true);
        assert_eq!(item.title, "Товары");
        let columns: Vec<(String, bool, bool)> = item
            .columns
            .iter()
            .map(|column| (column.title.to_string(), column.right, column.flex))
            .collect();
        assert_eq!(
            columns,
            [
                ("Товар".to_owned(), false, true),
                ("Количество".to_owned(), true, false)
            ]
        );
        let row = item.rows.row_data(0).unwrap();
        let cells: Vec<String> = row.cells.iter().map(|cell| cell.to_string()).collect();
        assert_eq!(cells, ["Мука", ""]);
        assert_eq!(row.error, "Заполните: Количество.");
        assert_eq!(item.selected, -1);
        assert!(item.editable);
    }

    #[test]
    fn row_window_title_names_the_part_and_the_row() {
        let mut meta = object_meta("issue");
        meta.rights = Rights::ALL;
        let mut state = FormState::new_object(&meta);
        state.add_row(0);
        let draft = state.parts().draft().unwrap();
        assert_eq!(row_title(&state, draft), "Товары: новая строка");
        assert_eq!(first_cell(draft), Some(0));
        let item = cell_item(draft, 0, &demo_objects()).unwrap();
        assert_eq!(item.kind, EditorKind::Reference);
        assert!(!item.read_only);
    }

    #[test]
    fn status_kinds_match_the_chip_kinds_of_the_screen() {
        assert_eq!(status_kind(Status::New), 0);
        assert_eq!(status_kind(Status::Posted), 1);
        assert_eq!(status_kind(Status::Draft), 2);
        assert_eq!(status_kind(Status::PostedChanged), 4);
        assert_eq!(status_kind(Status::Deleted), 5);
    }

    #[test]
    fn version_conflict_offers_to_reread_other_errors_do_not() {
        let conflict = EngineError::new(ErrorCode::VersionConflict, "версия");
        let notice = Notice::from_save_error(&conflict, PostAction::Write);
        assert_eq!(notice.action, Some(NoticeAction::Reload));
        assert_eq!(NoticeAction::Reload.label(), RELOAD);
        assert!(notice.text.contains("Перечитать"));
        let other = EngineError::new(ErrorCode::PeriodClosed, "Период закрыт.");
        assert_eq!(
            Notice::from_save_error(&other, PostAction::Post),
            Notice::plain("Период закрыт.")
        );
    }

    #[test]
    fn busy_data_offers_to_repeat_the_same_action() {
        let busy = EngineError::new(
            ErrorCode::Unavailable,
            "Эти данные сейчас меняет другой пользователь. Повторите действие.",
        );
        for action in [PostAction::Write, PostAction::Post, PostAction::Unpost] {
            let notice = Notice::from_save_error(&busy, action);
            assert_eq!(notice.text, busy.message);
            assert_eq!(notice.action, Some(NoticeAction::RetrySubmit(action)));
        }
        assert_eq!(NoticeAction::RetrySubmit(PostAction::Post).label(), RETRY);
        assert_eq!(NoticeAction::RetryLoad.label(), RETRY);
    }

    #[test]
    fn open_errors_have_human_messages() {
        for error in [
            OpenError::NoBase,
            OpenError::UnknownObject,
            OpenError::NotAllowed,
            OpenError::NoMenuItem,
        ] {
            assert!(error.to_string().ends_with('.'), "{error:?}");
        }
    }

    /// Новый элемент справочника из формы: запись, чтение назад, повторная запись как правка
    /// и конфликт версий. Всё через настоящее ядро.
    #[test]
    fn catalog_element_goes_through_the_core() {
        let (engine, path) = demo_engine("catalog");
        let meta = object_meta("goods");
        let mut form = Form::ready(FormState::new_object(&meta));
        let state = form.state_mut().unwrap();
        // Без обязательного наименования форма до ядра не доходит.
        assert!(state.to_data().is_err());
        edit(state, "name", "Мука ржаная");
        edit(state, "unit", "кг");
        let data = state.to_data().unwrap();

        let outcome = save_and_read(&*engine, "goods", &data, PostAction::Write).unwrap();
        let id = outcome.saved.id;
        apply_outcome(
            &mut form,
            std::slice::from_ref(&meta),
            "goods",
            outcome,
            PostAction::Write,
        );
        let state = form.state().unwrap();
        assert_eq!(state.id(), Some(id));
        assert!(!state.is_new() && !state.is_modified());
        assert_eq!(form.title, "Мука ржаная");
        assert_eq!(form.notice, None);

        // Правка записанного: версия из ответа ядра, новая версия больше.
        let before = state.version();
        let state = form.state_mut().unwrap();
        edit(state, "unit", "г");
        let data = state.to_data().unwrap();
        let again = save_and_read(&*engine, "goods", &data, PostAction::Write).unwrap();
        assert!(again.saved.version > before);

        // Повторная запись со старой версией: конфликт, форма предлагает «Перечитать».
        let error = save_and_read(&*engine, "goods", &data, PostAction::Write)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::VersionConflict);
        assert_eq!(
            Notice::from_save_error(&error, PostAction::Write).action,
            Some(NoticeAction::Reload)
        );
        remove(engine, &path);
    }

    /// Документ из ядра пишется назад с теми же строками табличной части, а проведение и отмена
    /// идут одной записью.
    #[test]
    fn document_round_trip_keeps_table_lines_and_posts() {
        let (engine, path) = demo_engine("document");
        let meta = object_meta("receipt");
        let page = engine
            .list(&ostov_api::ListQuery {
                object: "receipt".to_owned(),
                posted: Some(false),
                deleted: Some(false),
                size: 1,
                ..ostov_api::ListQuery::default()
            })
            .unwrap();
        let id = page.rows[0].id;
        let loaded = engine.get("receipt", id).unwrap();
        assert!(!loaded.tables["lines"].is_empty());

        let mut form = Form::ready(FormState::from_data(&meta, &loaded));
        let state = form.state_mut().unwrap();
        edit(state, "date", "11.02.2026");
        let data = state.to_data().unwrap();
        // Строки уходят как пришли, ссылки одним `id`.
        assert_eq!(data.tables["lines"].len(), loaded.tables["lines"].len());
        assert!(
            data.tables["lines"]
                .iter()
                .flat_map(|line| line.values())
                .all(|value| !matches!(value, Value::Ref { title: Some(_), .. }))
        );

        let outcome = save_and_read(&*engine, "receipt", &data, PostAction::Post).unwrap();
        apply_outcome(
            &mut form,
            std::slice::from_ref(&meta),
            "receipt",
            outcome,
            PostAction::Post,
        );
        let state = form.state().unwrap();
        assert!(state.is_posted() && !state.is_modified());
        assert_eq!(state.status(), Some(Status::Posted));
        let stored = engine.get("receipt", id).unwrap();
        assert_eq!(stored.tables, loaded.tables);
        assert_eq!(stored.fields["date"], Value::Date("2026-02-11".to_owned()));

        // Проведённый документ отменяется той же записью.
        let state = form.state_mut().unwrap();
        let data = state.to_data().unwrap();
        let outcome = save_and_read(&*engine, "receipt", &data, PostAction::Unpost).unwrap();
        apply_outcome(&mut form, &[meta], "receipt", outcome, PostAction::Unpost);
        assert!(!form.state().unwrap().is_posted());
        remove(engine, &path);
    }

    /// Отказ ядра (период закрыт) доходит до формы как есть, а набранное остаётся.
    #[test]
    fn core_refusal_keeps_typed_values() {
        let (engine, path) = demo_engine("refusal");
        engine.set_closing_date(Some("2026-12-31")).unwrap();
        let meta = object_meta("receipt");
        let mut state = FormState::new_object(&meta);
        edit(&mut state, "date", "11.02.2026");
        // Ссылки пока не выбрать руками: ставим, как это сделает выбор из справочника.
        for name in ["supplier", "warehouse"] {
            let index = state.fields().iter().position(|f| f.name == name).unwrap();
            state.set_reference(index, Some((1, "Первый".to_owned())));
        }
        let data = state.to_data().unwrap();
        let error = save_and_read(&*engine, "receipt", &data, PostAction::Write)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::PeriodClosed);
        let notice = Notice::from_save_error(&error, PostAction::Write);
        assert_eq!(notice.text, error.message);
        assert_eq!(notice.action, None);
        // Форма та же: набранное на месте, можно исправить дату и записать снова.
        assert!(state.is_modified());
        assert_eq!(state.fields()[0].text(), "11.02.2026");
        remove(engine, &path);
    }
}
