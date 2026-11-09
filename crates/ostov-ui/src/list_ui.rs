//! Экран универсального списка (`ObjectListView`): список любого объекта по метаданным.
//! Колонки и ячейки даёт `object_list`, строки окнами — `list_window::ListModel` (ядро зовётся
//! только в фоне). Список объекта один на все окна: сортировка, поиск и ширины колонок общие,
//! а какой объект показан, у каждого окна свой (по активной вкладке).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, MapModel, ModelRc, SharedString, VecModel};

use crate::engine::{Engine, ListQuery, ObjectMeta, Value};
use crate::list_window::{LazyRow, ListModel, WindowConfig};
use crate::object_list::{ColumnSpec, columns_from_meta, row_cells};
use crate::receipts_ui::{resized_columns, snapshot_of, write_columns};
use crate::sorting::Sorting;
use crate::windows::Windows;
use crate::{AppWindow, GridColumn, GridRow, ListData};

/// Пауза после последней буквы поиска: запрос в ядро не уходит на каждое нажатие.
const SEARCH_PAUSE: Duration = Duration::from_millis(250);

/// Список одного объекта: модель строк и то, что выбрал пользователь.
struct ObjectList {
    object: String,
    title: String,
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
            columns,
            grid_columns: ModelRc::new(VecModel::from(grid)),
            model,
            rows: ModelRc::new(rows),
            sorting: RefCell::default(),
            search: RefCell::default(),
            full_total: Cell::new(None),
            pause: slint::Timer::default(),
            resize_start: RefCell::new(None),
        })
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
    let deleted = row
        .as_row()
        .is_some_and(|row| matches!(row.values.get("deleted"), Some(Value::Bool(true))));
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
    base: Rc<RefCell<Base>>,
}

impl ListUi {
    pub fn new(main: slint::Weak<AppWindow>, windows: Windows) -> Self {
        Self {
            main,
            windows,
            base: Rc::default(),
        }
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

/// `ListUi` без владения базой: для подписки модели, которая сама лежит в базе.
struct WeakUi {
    main: slint::Weak<AppWindow>,
    windows: Windows,
    base: Weak<RefCell<Base>>,
}

impl WeakUi {
    fn upgrade(&self) -> Option<ListUi> {
        Some(ListUi {
            main: self.main.clone(),
            windows: self.windows.clone(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostov_api::{FieldDef, FieldKind, ObjectKind, Rights, Row};
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
