//! Окно «Ostov Конфигуратор»: связывает `Browser` (src/configurator.rs) с окном Slint.
//! Только просмотр: обработчики ничего не пишут ни в конфигурацию, ни на диск.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::configurator::{Browser, CheckReport, Configuration, Table, TreeRow};
use crate::receipts_ui::{resized_columns, snapshot_of, write_columns};
use crate::settings::Settings;
use crate::{ConfiguratorWindow, GridColumn, GridRow, InfoRow, NavRow, Shape, Theme};

/// Создаёт окно конфигуратора для конфигурации `config` и подключает обработчики.
///
/// # Errors
/// Окно не создалось: нет графической подсистемы.
pub fn create_window(config: Configuration) -> Result<ConfiguratorWindow, slint::PlatformError> {
    let window = ConfiguratorWindow::new()?;
    window.set_config_title(config.title.as_str().into());
    window.set_config_version(config.version.as_str().into());
    let browser = Rc::new(RefCell::new(Browser::new(config)));
    show_browser(&window, &browser.borrow());
    show_check(&window, &CheckReport::NotRun);
    connect_browser(&window, &browser);
    connect_check(&window);
    Ok(window)
}

/// Тема, скругление, размер шрифта и плотность строк из настроек пользователя: те же, что у приложения.
pub fn apply_settings(window: &ConfiguratorWindow, settings: Settings) {
    window.global::<Theme>().set_dark(settings.dark);
    window
        .global::<Shape>()
        .set_control_radius(settings.radius_px());
    window
        .global::<Theme>()
        .set_font_scale(settings.font_scale());
    window.global::<Theme>().set_row_height(settings.row_px());
}

fn connect_browser(window: &ConfiguratorWindow, browser: &Rc<RefCell<Browser>>) {
    window.on_tree_clicked({
        let (weak, browser) = (window.as_weak(), browser.clone());
        move |row| {
            let Some(window) = weak.upgrade() else { return };
            if let Ok(row) = usize::try_from(row) {
                browser.borrow_mut().click(row);
            }
            show_browser(&window, &browser.borrow());
        }
    });
    window.on_section_chosen({
        let (weak, browser) = (window.as_weak(), browser.clone());
        move |section| {
            let Some(window) = weak.upgrade() else { return };
            browser.borrow_mut().choose_section(section);
            show_browser(&window, &browser.borrow());
        }
    });
    window.on_part_chosen({
        let (weak, browser) = (window.as_weak(), browser.clone());
        move |part| {
            let Some(window) = weak.upgrade() else { return };
            if let Ok(part) = usize::try_from(part) {
                browser.borrow_mut().choose_part(part);
            }
            show_browser(&window, &browser.borrow());
        }
    });
    let resize = Rc::new(Resize::default());
    window.on_field_resize_begin({
        let (weak, resize) = (window.as_weak(), resize.clone());
        move || {
            if let Some(window) = weak.upgrade() {
                resize.begin(&window.get_field_columns());
            }
        }
    });
    window.on_field_resize_column({
        let (weak, resize) = (window.as_weak(), resize.clone());
        move |index, delta, width| {
            if let Some(window) = weak.upgrade() {
                resize.drag(&window.get_field_columns(), index, delta, width);
            }
        }
    });
    window.on_field_resize_end(move || resize.end());
}

fn connect_check(window: &ConfiguratorWindow) {
    // Проверка читает файлы из памяти и занимает миллисекунды, поэтому идёт прямо в обработчике.
    window.on_check_requested({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                show_check(&window, &CheckReport::run());
            }
        }
    });
    let resize = Rc::new(Resize::default());
    window.on_check_resize_begin({
        let (weak, resize) = (window.as_weak(), resize.clone());
        move || {
            if let Some(window) = weak.upgrade() {
                resize.begin(&window.get_check_columns());
            }
        }
    });
    window.on_check_resize_column({
        let (weak, resize) = (window.as_weak(), resize.clone());
        move |index, delta, width| {
            if let Some(window) = weak.upgrade() {
                resize.drag(&window.get_check_columns(), index, delta, width);
            }
        }
    });
    window.on_check_resize_end(move || resize.end());
}

/// Перетаскивание границы колонки: ширины на начало запоминаются, дальше считаются от них,
/// поэтому ошибки движения мыши не накапливаются.
#[derive(Default)]
struct Resize {
    start: RefCell<Option<Vec<GridColumn>>>,
}

impl Resize {
    fn begin(&self, columns: &ModelRc<GridColumn>) {
        *self.start.borrow_mut() = Some(snapshot_of(columns));
    }

    fn drag(&self, columns: &ModelRc<GridColumn>, index: i32, delta: f32, width: f32) {
        if let Some(start) = self.start.borrow().as_ref() {
            write_columns(columns, resized_columns(start, index, delta, width));
        }
    }

    fn end(&self) {
        *self.start.borrow_mut() = None;
    }
}

/// Переносит состояние `Browser` в окно: дерево, заголовок, разделы и таблицу полей.
fn show_browser(window: &ConfiguratorWindow, browser: &Browser) {
    window.set_tree_rows(model(
        browser.tree_rows().into_iter().enumerate().map(nav_row),
    ));
    let heading = browser.heading();
    window.set_has_element(heading.is_some());
    let heading = heading.unwrap_or_default();
    window.set_element_title(heading.title.into());
    window.set_element_name(heading.name.into());
    window.set_element_kind(heading.kind.into());
    window.set_sections(model(browser.sections().iter().copied()));
    window.set_section(browser.section());
    window.set_general_rows(model(browser.general().into_iter().map(|detail| InfoRow {
        label: detail.label.into(),
        value: detail.value.into(),
    })));
    window.set_part_titles(model(
        browser.part_titles().into_iter().map(SharedString::from),
    ));
    window.set_part_index(i32::try_from(browser.part()).unwrap_or_default());
    let table = browser.grid().unwrap_or_default();
    window.set_field_columns(grid_columns(&table));
    window.set_field_rows(grid_rows(&table));
    window.set_field_empty_text(browser.grid_empty_text().into());
}

fn show_check(window: &ConfiguratorWindow, report: &CheckReport) {
    let table = report.table();
    window.set_check_summary(report.summary().into());
    window.set_check_passed(report.is_passed());
    window.set_check_failed(report.is_failed());
    window.set_check_columns(grid_columns(&table));
    window.set_check_rows(grid_rows(&table));
    window.set_check_empty_text(report.empty_text().into());
}

fn nav_row((id, row): (usize, TreeRow)) -> NavRow {
    NavRow {
        id: i32::try_from(id).unwrap_or_default(),
        title: row.title.into(),
        icon: row.icon.into(),
        level: row.level,
        is_group: row.is_group,
        expanded: row.expanded,
        selected: row.selected,
        favorite: false,
    }
}

fn model<T: Clone + 'static>(items: impl Iterator<Item = T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items.collect::<Vec<T>>()))
}

fn grid_columns(table: &Table) -> ModelRc<GridColumn> {
    model(table.columns.iter().map(|column| GridColumn {
        title: column.title.into(),
        width: column.width,
        right: column.right,
        flex: column.flex,
        status: false,
    }))
}

fn grid_rows(table: &Table) -> ModelRc<GridRow> {
    model(table.rows.iter().map(|row| GridRow {
        cells: model(row.iter().map(|cell| SharedString::from(cell.as_str()))),
        muted: false,
        milestone: false,
    }))
}
