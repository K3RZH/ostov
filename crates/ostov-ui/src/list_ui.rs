//! Экран универсального списка (`ObjectListView`): список любого объекта по метаданным.
//! Колонки и ячейки даёт `object_list`, строки окнами — `list_window::ListModel` (ядро зовётся
//! только в фоне). Список объекта один на все окна: сортировка, поиск и ширины колонок общие,
//! а какой объект показан, у каждого окна свой (по активной вкладке).
//!
//! Меню строки (правая кнопка): пункты по виду объекта, правам и состоянию строки считает
//! `row_actions`. Действие идёт в ядро в фоне, строка меняется на месте. Подтверждение и
//! выбранная строка живут у окна, а не у списка: два окна одного списка не мешают друг другу.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, MapModel, ModelRc, SharedString, VecModel};

use crate::engine::{
    Engine, EngineResult, ErrorCode, ListQuery, ObjectData, ObjectKind, ObjectMeta, Rights, Saved,
    call_in_background,
};
use crate::form_ui::FormUi;
use crate::list_window::{LazyRow, ListModel, WindowConfig};
use crate::object_list::{ColumnSpec, columns_from_meta, row_cells, row_label};
use crate::receipts_ui::{resized_columns, snapshot_of, write_columns};
use crate::row_actions::{
    MenuEntry, RowAction, RowCommand, RowState, RowTarget, confirmation, failure_text, perform,
    row_from_object, row_menu, updated_row,
};
use crate::shell::DocKey;
use crate::sorting::Sorting;
use crate::windows::Windows;
use crate::workspace::WindowRef;
use crate::{AppWindow, GridColumn, GridRow, ListData, MenuAction};

/// Пауза после последней буквы поиска: запрос в ядро не уходит на каждое нажатие.
const SEARCH_PAUSE: Duration = Duration::from_millis(250);

/// Список одного объекта: модель строк и то, что выбрал пользователь.
struct ObjectList {
    object: String,
    title: String,
    kind: ObjectKind,
    /// Права вошедшего пользователя на объект: от них зависят пункты меню строки.
    rights: Rights,
    columns: Rc<[ColumnSpec]>,
    /// Колонки таблицы: одна модель на все окна, ширины меняются на месте.
    grid_columns: ModelRc<GridColumn>,
    model: Rc<ListModel>,
    rows: ModelRc<GridRow>,
    sorting: RefCell<Sorting>,
    search: RefCell<String>,
    /// Сколько строк в списке без поиска: «из M» в подписи.
    full_total: Cell<Option<u64>>,
    pause: slint::Timer,
    /// Колонки на момент нажатия на ручку: от них считается каждое движение.
    resize_start: RefCell<Option<Vec<GridColumn>>>,
    /// Ошибка последнего действия над строкой; пусто: ошибки нет.
    notice: RefCell<String>,
    /// Действие над строкой выполняется в ядре: пока оно не кончилось, меню не открывается
    /// (версия строки уже устарела бы).
    busy: Cell<bool>,
}

impl ObjectList {
    fn new(engine: Arc<dyn Engine>, meta: &ObjectMeta) -> Rc<Self> {
        let columns: Rc<[ColumnSpec]> = columns_from_meta(meta).into();
        let grid: Vec<GridColumn> = columns.iter().map(ColumnSpec::grid_column).collect();
        let model = ListModel::new(engine, list_query(&meta.name), WindowConfig::default());
        let rows = MapModel::new(ModelRc::from(Rc::clone(&model)), {
            let columns = Rc::clone(&columns);
            move |row: LazyRow| grid_row(&row, &columns)
        });
        Rc::new(Self {
            object: meta.name.clone(),
            title: meta.title.clone(),
            kind: meta.kind,
            rights: meta.rights,
            columns,
            grid_columns: ModelRc::new(VecModel::from(grid)),
            model,
            rows: ModelRc::new(rows),
            sorting: RefCell::default(),
            search: RefCell::default(),
            full_total: Cell::new(None),
            pause: slint::Timer::default(),
            resize_start: RefCell::new(None),
            notice: RefCell::default(),
            busy: Cell::new(false),
        })
    }

    /// Строка, на которую нажали правой кнопкой (номер в таблице), если она уже загружена.
    fn target_at(&self, row: i32) -> Option<RowTarget> {
        let row = self.model.loaded_row(usize::try_from(row).ok()?)?;
        let label = row_label(&self.title, self.kind, &self.columns, &row);
        Some(RowTarget::new(&self.object, label, &row))
    }

    /// Запрос к ядру по текущим сортировке и поиску. Ключ сортировки один: ядро сортирует
    /// по одному полю.
    fn query(&self) -> ListQuery {
        let sorting = self.sorting.borrow();
        let key = sorting
            .keys()
            .first()
            .and_then(|(column, descending)| Some((self.columns.get(*column)?, *descending)));
        let search = self.search.borrow();
        ListQuery {
            order: key.map(|(column, _)| column.field.clone()),
            descending: key.is_some_and(|(_, descending)| descending),
            search: Some(search.trim())
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            ..list_query(&self.object)
        }
    }

    fn is_searching(&self) -> bool {
        !self.search.borrow().trim().is_empty()
    }

    /// Щелчок по заголовку: нет → по возрастанию → по убыванию → нет. Ключ один, поэтому
    /// сортировка по другой колонке снимается.
    fn click_sort(&self, column: usize) {
        let mut sorting = self.sorting.borrow_mut();
        let direction = sorting.dir(column);
        sorting.clear_all();
        match direction {
            0 => sorting.set(column, false),
            1 => sorting.set(column, true),
            _ => {}
        }
    }

    fn summary(&self) -> String {
        match self.model.total() {
            Some(shown) => {
                let all = if self.is_searching() {
                    self.full_total.get().unwrap_or(shown)
                } else {
                    shown
                };
                format!("Показано {shown} из {all}")
            }
            None if self.model.error().is_some() => String::new(),
            None => "Загрузка…".to_owned(),
        }
    }

    /// Что написать в пустой таблице.
    fn empty_text(&self) -> &'static str {
        match self.model.total() {
            None if self.model.error().is_some() => "Список не загружен",
            None => "Загрузка…",
            Some(_) if self.is_searching() => "Ничего не найдено",
            Some(_) => "Нет данных",
        }
    }
}

fn list_query(object: &str) -> ListQuery {
    ListQuery {
        object: object.to_owned(),
        ..ListQuery::default()
    }
}

/// Строка таблицы: у строки, которая ещё едет из ядра, ячейки пустые. Помеченная на удаление
/// приглушена.
fn grid_row(row: &LazyRow, columns: &[ColumnSpec]) -> GridRow {
    let cells: Vec<SharedString> = row.as_row().map_or_else(Vec::new, |row| {
        row_cells(row, columns)
            .into_iter()
            .map(SharedString::from)
            .collect()
    });
    let deleted = row.as_row().is_some_and(|row| RowState::of(row).deleted);
    GridRow {
        cells: ModelRc::new(VecModel::from(cells)),
        muted: deleted,
        milestone: false,
    }
}

#[derive(Default)]
struct Base {
    engine: Option<Arc<dyn Engine>>,
    objects: Vec<ObjectMeta>,
    /// Списки открываются по первому показу и живут, пока открыта база: вкладка, к которой
    /// вернулись, помнит сортировку и поиск.
    lists: HashMap<String, Rc<ObjectList>>,
}

/// Универсальные списки открытой базы во всех окнах с вкладками.
#[derive(Clone)]
pub struct ListUi {
    main: slint::Weak<AppWindow>,
    windows: Windows,
    /// Формы объектов: строка списка открывается в форме, а записанная форма обновляет строку.
    forms: FormUi,
    base: Rc<RefCell<Base>>,
}

impl ListUi {
    pub fn new(main: slint::Weak<AppWindow>, windows: Windows, forms: FormUi) -> Self {
        let ui = Self {
            main,
            windows,
            forms,
            base: Rc::default(),
        };
        // Подписка держит список слабой ссылкой на базу; форма и список живут, пока работает
        // приложение, цикл между ними не утечка.
        let weak = ui.downgrade();
        ui.forms.on_saved(move |object, data, created| {
            if let Some(ui) = weak.upgrade() {
                ui.form_saved(object, data, created);
            }
        });
        ui
    }

    /// База открыта: списки прошлой базы забыты, экраны, которые уже стоят в окнах,
    /// показывают свои объекты заново.
    pub fn start(&self, engine: Arc<dyn Engine>, objects: &[ObjectMeta]) {
        *self.base.borrow_mut() = Base {
            engine: Some(engine),
            objects: objects.to_vec(),
            lists: HashMap::new(),
        };
        self.each_window(|window| {
            let object = window.global::<ListData>().get_object();
            if !object.is_empty() {
                self.show(window, &object);
            }
        });
    }

    /// База закрыта: списки и их запросы к ядру больше не нужны.
    pub fn clear(&self) {
        *self.base.borrow_mut() = Base::default();
        self.each_window(|window| show_nothing(window, "", ""));
    }

    /// Подключает данные и события списка к окну `window` (главному или дополнительному).
    pub fn attach_window(&self, window: &AppWindow) {
        let data = window.global::<ListData>();
        let bound = || (self.clone(), window.as_weak());
        data.on_show({
            let (ui, window) = bound();
            move |object| {
                if let Some(window) = window.upgrade() {
                    ui.show(&window, &object);
                }
            }
        });
        data.on_sort({
            let (ui, window) = bound();
            move |column| {
                let Some((window, list)) = ui.shown(&window) else {
                    return;
                };
                let Some(column) = usize::try_from(column)
                    .ok()
                    .filter(|column| *column < list.columns.len())
                else {
                    return;
                };
                list.click_sort(column);
                window.global::<ListData>().set_selected_row(-1);
                list.model.set_query(list.query());
            }
        });
        data.on_query_changed({
            let (ui, window) = bound();
            move |text| {
                let Some((window, list)) = ui.shown(&window) else {
                    return;
                };
                text.as_str().clone_into(&mut list.search.borrow_mut());
                window.global::<ListData>().set_selected_row(-1);
                // Текст поиска у списка один: в других окнах с ним в поле то же самое.
                ui.each_showing(&list.object, |other| {
                    other.global::<ListData>().set_query(text.clone());
                });
                // Таймер принадлежит списку: держать список сильной ссылкой ему нельзя.
                let waiting = Rc::downgrade(&list);
                list.pause
                    .start(slint::TimerMode::SingleShot, SEARCH_PAUSE, move || {
                        if let Some(list) = waiting.upgrade() {
                            list.model.set_query(list.query());
                        }
                    });
            }
        });
        data.on_retry({
            let (ui, window) = bound();
            move || {
                if let Some((_, list)) = ui.shown(&window) {
                    list.model.retry();
                }
            }
        });
        data.on_visible_changed({
            let (ui, window) = bound();
            move |first, last| {
                let Some((_, list)) = ui.shown(&window) else {
                    return;
                };
                if let (Ok(first), Ok(last)) = (usize::try_from(first), usize::try_from(last)) {
                    list.model.ensure_visible(first, last.max(first));
                }
            }
        });
        data.on_resize_begin({
            let (ui, window) = bound();
            move || {
                if let Some((_, list)) = ui.shown(&window) {
                    *list.resize_start.borrow_mut() = Some(snapshot_of(&list.grid_columns));
                }
            }
        });
        data.on_resize({
            let (ui, window) = bound();
            move |index, delta, width| {
                let Some((_, list)) = ui.shown(&window) else {
                    return;
                };
                if let Some(start) = list.resize_start.borrow().as_ref() {
                    write_columns(
                        &list.grid_columns,
                        resized_columns(start, index, delta, width),
                    );
                }
            }
        });
        data.on_resize_end({
            let (ui, window) = bound();
            move || {
                if let Some((_, list)) = ui.shown(&window) {
                    *list.resize_start.borrow_mut() = None;
                }
            }
        });
        self.attach_row_menu(window);
    }

    /// Меню строки, подтверждение опасного действия и полоса ошибки действия.
    fn attach_row_menu(&self, window: &AppWindow) {
        let data = window.global::<ListData>();
        // Что выбрано в меню этого окна: строка, на которую нажали, и действие, ждущее ответа
        // в окне подтверждения.
        let menu = Rc::new(RefCell::new(MenuState::default()));
        let bound = || (self.clone(), window.as_weak(), Rc::clone(&menu));
        data.on_menu_opening({
            let (ui, window, menu) = bound();
            move |row| {
                let Some((window, list)) = ui.shown(&window) else {
                    return;
                };
                let target = list.target_at(row).filter(|_| !list.busy.get());
                let entries = target.as_ref().map_or_else(Vec::new, |target| {
                    row_menu(list.kind, list.rights, target.state)
                });
                window
                    .global::<ListData>()
                    .set_menu_actions(menu_actions(&entries));
                menu.borrow_mut().target = target;
            }
        });
        data.on_action({
            let (ui, window, menu) = bound();
            move |_, id| {
                let (Some(action), Some((window, list))) =
                    (RowAction::from_id(&id), ui.shown(&window))
                else {
                    return;
                };
                match action {
                    RowAction::Reload => ui.reload(&list),
                    RowAction::Open => {
                        let target = menu.borrow_mut().target.take();
                        if let Some(target) = target.filter(|target| target.object == list.object) {
                            ui.open_row(&window, &list, &target);
                        }
                    }
                    RowAction::Command(command) => {
                        let target = menu.borrow_mut().target.take();
                        // Окно успело показать другой объект: выбранная строка уже не тут.
                        let Some(target) = target.filter(|target| target.object == list.object)
                        else {
                            return;
                        };
                        match confirmation(action, &target.label) {
                            Some(ask) => {
                                let data = window.global::<ListData>();
                                data.set_confirm_title(ask.title.into());
                                data.set_confirm_text(ask.text.into());
                                data.set_confirm_accept(ask.accept.into());
                                data.set_confirm_open(true);
                                menu.borrow_mut().pending = Some((target, command));
                            }
                            None => ui.run(target, command),
                        }
                    }
                }
            }
        });
        data.on_confirm_accepted({
            let (ui, window, menu) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    window.global::<ListData>().set_confirm_open(false);
                }
                let pending = menu.borrow_mut().pending.take();
                if let Some((target, command)) = pending {
                    ui.run(target, command);
                }
            }
        });
        data.on_confirm_cancelled({
            let (_, window, menu) = bound();
            move || {
                menu.borrow_mut().pending = None;
                if let Some(window) = window.upgrade() {
                    window.global::<ListData>().set_confirm_open(false);
                }
            }
        });
        data.on_dismiss_notice({
            let (ui, window, _) = bound();
            move || {
                if let Some((_, list)) = ui.shown(&window) {
                    list.notice.borrow_mut().clear();
                    ui.show_state(&list);
                }
            }
        });
        // Двойной щелчок и кнопка «Открыть»: форма выбранной строки.
        data.on_open({
            let (ui, window, _) = bound();
            move |row| {
                let Some((window, list)) = ui.shown(&window) else {
                    return;
                };
                if let Some(target) = list.target_at(row) {
                    ui.open_row(&window, &list, &target);
                }
            }
        });
        data.on_create({
            let (ui, window, _) = bound();
            move || {
                if let Some((window, list)) = ui.shown(&window) {
                    ui.create(&window, &list);
                }
            }
        });
    }

    /// В каком окне открыть форму: в том, где нажали. Активное окно оболочка меняет только
    /// щелчками по вкладкам, а список в другом окне мог не получить ни одного.
    fn activate(&self, window: &AppWindow) {
        let place = self
            .windows
            .id_of(window)
            .map_or(WindowRef::Main, WindowRef::Extra);
        self.windows.set_active(place);
    }

    /// Открывает форму строки; отказ ложится полосой над таблицей, как ошибка действия.
    fn open_row(&self, window: &AppWindow, list: &ObjectList, target: &RowTarget) {
        self.activate(window);
        let key = DocKey::new(&target.object, target.id);
        if let Err(error) = self.forms.open_form(key) {
            self.fail(list, failure_text(&target.label, &error.to_string()));
        }
    }

    /// «Создать»: форма нового объекта этого списка.
    fn create(&self, window: &AppWindow, list: &ObjectList) {
        self.activate(window);
        if let Err(error) = self.forms.open_new(&list.object) {
            self.fail(list, failure_text(&list.title, &error.to_string()));
        }
    }

    fn fail(&self, list: &ObjectList, text: String) {
        *list.notice.borrow_mut() = text;
        self.show_state(list);
    }

    /// Объект записан в форме: строка списка меняется на месте, новый объект и случай, когда
    /// записанное прочитать не удалось, читаются списком заново.
    fn form_saved(&self, object: &str, data: Option<&ObjectData>, created: bool) {
        let Some(list) = self.base.borrow().lists.get(object).cloned() else {
            return;
        };
        match data.filter(|_| !created) {
            Some(data) => {
                if let Some(id) = data.id {
                    list.model
                        .update_row(id, |old| row_from_object(old, list.kind, data));
                }
            }
            None => self.reload(&list),
        }
        self.show_state(&list);
    }

    /// «Обновить список»: строки читаются заново с первой, выбор и прошлая ошибка снимаются.
    fn reload(&self, list: &ObjectList) {
        list.notice.borrow_mut().clear();
        self.each_showing(&list.object, |window| {
            window.global::<ListData>().set_selected_row(-1);
        });
        // Подпись и полосы обновит подписка модели: она зовётся при перезапуске.
        list.model.reload();
    }

    /// Выполняет команду над строкой: ядро зовётся в фоне, в ответ строка меняется на месте или
    /// над таблицей появляется ошибка. Строку ищем по `id` из `target`.
    fn run(&self, target: RowTarget, command: RowCommand) {
        let (engine, list) = {
            let base = self.base.borrow();
            match (base.engine.clone(), base.lists.get(&target.object)) {
                (Some(engine), Some(list)) => (engine, Rc::clone(list)),
                _ => return,
            }
        };
        if list.busy.replace(true) {
            return;
        }
        list.notice.borrow_mut().clear();
        self.show_state(&list);
        // Ответ может прийти, когда база уже закрыта: тогда список, а с ним и ответ, не нужны.
        let (ui, waiting) = (self.downgrade(), Rc::downgrade(&list));
        let asked = target.clone();
        call_in_background(
            move || perform(&*engine, &asked, command),
            move |result| {
                if let (Some(ui), Some(list)) = (ui.upgrade(), waiting.upgrade()) {
                    list.busy.set(false);
                    ui.finish(&list, &target, command, result);
                }
            },
        );
    }

    /// Ответ ядра на команду: успех меняет строку в памяти, отказ ложится полосой над таблицей.
    fn finish(
        &self,
        list: &ObjectList,
        target: &RowTarget,
        command: RowCommand,
        result: EngineResult<Saved>,
    ) {
        match result {
            Ok(saved) => {
                list.model
                    .update_row(target.id, |row| updated_row(row, command, &saved));
            }
            Err(error) => {
                *list.notice.borrow_mut() = failure_text(&target.label, &error.message);
                // Строку изменил кто-то другой: в списке она устарела, читаем заново.
                if error.code == ErrorCode::VersionConflict {
                    self.each_showing(&list.object, |window| {
                        window.global::<ListData>().set_selected_row(-1);
                    });
                    list.model.reload();
                }
            }
        }
        self.show_state(list);
    }

    /// Окно и список, который в нём показан; `None`, если окна уже нет или список не открыт.
    fn shown(&self, window: &slint::Weak<AppWindow>) -> Option<(AppWindow, Rc<ObjectList>)> {
        let window = window.upgrade()?;
        let object = window.global::<ListData>().get_object();
        // Заём базы снимается здесь же: дальше обработчик зовёт модель, а та оповещает экран.
        let list = self.base.borrow().lists.get(object.as_str()).cloned()?;
        Some((window, list))
    }

    /// Показывает в окне список объекта: открывает его при первом показе.
    fn show(&self, window: &AppWindow, object: &str) {
        match self.list(object) {
            Ok(list) => {
                let data = window.global::<ListData>();
                data.set_object(object.into());
                data.set_title(list.title.as_str().into());
                data.set_columns(list.grid_columns.clone());
                data.set_rows(list.rows.clone());
                data.set_query(list.search.borrow().as_str().into());
                data.set_selected_row(-1);
                data.set_can_create(list.rights.create);
                show_state_in(window, &list);
            }
            Err(message) => show_nothing(window, object, message),
        }
    }

    /// Список объекта: уже открытый или новый. Ошибка: текст для полосы над таблицей.
    fn list(&self, object: &str) -> Result<Rc<ObjectList>, &'static str> {
        let mut base = self.base.borrow_mut();
        if let Some(list) = base.lists.get(object) {
            return Ok(Rc::clone(list));
        }
        let engine = base.engine.clone().ok_or("База не открыта.")?;
        let meta = base
            .objects
            .iter()
            .find(|meta| meta.name == object)
            .ok_or("Такого объекта нет в конфигурации базы.")?;
        let list = ObjectList::new(engine, meta);
        list.model.on_changed({
            // Модель принадлежит списку, а список базе: сильные ссылки замкнули бы круг.
            let (ui, list) = (self.downgrade(), Rc::downgrade(&list));
            move || {
                if let (Some(ui), Some(list)) = (ui.upgrade(), list.upgrade()) {
                    ui.show_state(&list);
                }
            }
        });
        base.lists.insert(object.to_owned(), Rc::clone(&list));
        Ok(list)
    }

    fn downgrade(&self) -> WeakUi {
        WeakUi {
            main: self.main.clone(),
            windows: self.windows.clone(),
            forms: self.forms.clone(),
            base: Rc::downgrade(&self.base),
        }
    }

    /// Подпись, ошибка и сортировка списка во всех окнах, где он показан.
    fn show_state(&self, list: &ObjectList) {
        if !list.is_searching()
            && let Some(total) = list.model.total()
        {
            list.full_total.set(Some(total));
        }
        self.each_showing(&list.object, |window| show_state_in(window, list));
    }

    fn each_window(&self, mut show: impl FnMut(&AppWindow)) {
        if let Some(main) = self.main.upgrade() {
            show(&main);
        }
        self.windows.each(show);
    }

    fn each_showing(&self, object: &str, mut show: impl FnMut(&AppWindow)) {
        self.each_window(|window| {
            if window.global::<ListData>().get_object() == object {
                show(window);
            }
        });
    }
}

/// Что выбрано в меню строки одного окна.
#[derive(Default)]
struct MenuState {
    /// Строка, на которую нажали правой кнопкой: для неё построено открытое меню.
    target: Option<RowTarget>,
    /// Действие, ждущее ответа в окне подтверждения.
    pending: Option<(RowTarget, RowCommand)>,
}

/// Пункты меню для таблицы.
fn menu_actions(entries: &[MenuEntry]) -> ModelRc<MenuAction> {
    let actions: Vec<MenuAction> = entries
        .iter()
        .map(|entry| MenuAction {
            id: entry.action.id().into(),
            label: entry.action.label().into(),
            icon: entry.action.icon().into(),
            danger: entry.action.is_danger(),
            enabled: entry.enabled,
            divider: entry.divider,
        })
        .collect();
    ModelRc::new(VecModel::from(actions))
}

/// `ListUi` без владения базой: для подписки модели, которая сама лежит в базе.
struct WeakUi {
    main: slint::Weak<AppWindow>,
    windows: Windows,
    forms: FormUi,
    base: Weak<RefCell<Base>>,
}

impl WeakUi {
    fn upgrade(&self) -> Option<ListUi> {
        Some(ListUi {
            main: self.main.clone(),
            windows: self.windows.clone(),
            forms: self.forms.clone(),
            base: self.base.upgrade()?,
        })
    }
}

fn show_state_in(window: &AppWindow, list: &ObjectList) {
    let data = window.global::<ListData>();
    data.set_summary(list.summary().into());
    data.set_empty_text(list.empty_text().into());
    data.set_error(
        list.model
            .error()
            .map(|error| error.message)
            .unwrap_or_default()
            .into(),
    );
    let sorting = list.sorting.borrow();
    let directions: Vec<i32> = (0..list.columns.len())
        .map(|column| sorting.dir(column))
        .collect();
    data.set_sort_dirs(ModelRc::new(VecModel::from(directions)));
    data.set_sort_count(i32::try_from(sorting.keys().len()).unwrap_or_default());
    data.set_notice(list.notice.borrow().as_str().into());
}

/// Окно без списка: база закрыта или объекта нет. `error` пустой: просто пустой экран.
fn show_nothing(window: &AppWindow, object: &str, error: &str) {
    let data = window.global::<ListData>();
    data.set_object(object.into());
    data.set_title(object.into());
    data.set_columns(ModelRc::default());
    data.set_rows(ModelRc::default());
    data.set_query("".into());
    data.set_selected_row(-1);
    data.set_summary("".into());
    data.set_empty_text("Список не загружен".into());
    data.set_error(error.into());
    data.set_sort_dirs(ModelRc::default());
    data.set_sort_count(0);
    data.set_notice("".into());
    data.set_menu_actions(ModelRc::default());
    data.set_confirm_open(false);
    data.set_can_create(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostov_api::{FieldDef, FieldKind, Row, Value};
    use slint::Model;

    fn columns() -> Vec<ColumnSpec> {
        columns_from_meta(&ObjectMeta {
            name: "issue".to_owned(),
            title: "Расход".to_owned(),
            kind: ObjectKind::Document,
            icon: None,
            fields: vec![
                FieldDef::new("date", FieldKind::Date).with_title("Дата"),
                FieldDef::reference("warehouse", "warehouses").with_title("Склад"),
            ],
            table_parts: Vec::new(),
            rights: Rights::ALL,
        })
    }

    fn loaded(values: &[(&str, Value)]) -> LazyRow {
        LazyRow::Loaded(Rc::new(Row {
            id: 1,
            values: values
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        }))
    }

    fn cells(row: &GridRow) -> Vec<String> {
        row.cells.iter().map(|cell| cell.to_string()).collect()
    }

    #[test]
    fn row_on_its_way_from_the_core_has_no_cells() {
        let row = grid_row(&LazyRow::Placeholder, &columns());
        assert!(cells(&row).is_empty());
        assert!(!row.muted);
    }

    #[test]
    fn loaded_row_shows_cells_and_deleted_one_is_muted() {
        let columns = columns();
        let row = grid_row(
            &loaded(&[
                ("number", Value::Int(7)),
                ("date", Value::Date("2026-02-10".to_owned())),
                ("posted", Value::Bool(true)),
                ("deleted", Value::Bool(false)),
            ]),
            &columns,
        );
        assert_eq!(cells(&row), ["7", "10.02.2026", "", "Проведён"]);
        assert!(!row.muted);
        let deleted = grid_row(&loaded(&[("deleted", Value::Bool(true))]), &columns);
        assert!(deleted.muted);
    }
}
