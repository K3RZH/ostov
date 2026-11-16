//! Окно «Ostov Конфигуратор»: связывает `Browser` (src/configurator.rs) с окном Slint.
//! Конфигурацию правят в редакторе; окно её показывает и проверяет, в файлы не пишет.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::configurator::{Browser, CheckReport, Configuration, Table, TreeRow};
use crate::engine::{ErrorCode, call_in_background};
use crate::receipts_ui::{resized_columns, snapshot_of, write_columns};
use crate::settings::Settings;
use crate::test_run::{Prepared, Preview, RunError};
use crate::{ConfiguratorWindow, GridColumn, GridRow, InfoRow, NavRow, Shape, Theme};

/// Открытая папка конфигурации; `None`, пока её не выбрали.
type Folder = Rc<RefCell<Option<PathBuf>>>;
/// Запущенный клиент предпросмотра; новый запуск сначала останавливает прошлый.
type Running = Rc<RefCell<Option<Preview>>>;

/// Создаёт окно конфигуратора и открывает папку `folder`, если она задана.
///
/// # Errors
/// Окно не создалось: нет графической подсистемы.
pub fn create_window(folder: Option<PathBuf>) -> Result<ConfiguratorWindow, slint::PlatformError> {
    let window = ConfiguratorWindow::new()?;
    let browser = Rc::new(RefCell::new(Browser::new(Configuration::empty())));
    let opened: Folder = Rc::default();
    connect_browser(&window, &browser);
    connect_check(&window, &browser, &opened);
    connect_folder(&window, &browser, &opened);
    connect_run(&window, &browser, &opened);
    match folder {
        Some(dir) => open_folder(&window, &browser, &opened, dir),
        None => {
            window.set_editor_empty_text(NO_FOLDER_TEXT.into());
            show_browser(&window, &browser.borrow());
            show_check(&window, &CheckReport::NotRun);
            window.set_check_empty_text("Папка не открыта: проверять пока нечего.".into());
        }
    }
    Ok(window)
}

const NO_FOLDER_TEXT: &str = "Откройте папку конфигурации кнопкой «Открыть папку» на панели сверху. \
                              Конфигурацию правят в редакторе кода, здесь её смотрят, проверяют \
                              и запускают.";

/// «Открыть папку»: системное окно выбора; выбранная папка открывается и запоминается.
fn connect_folder(window: &ConfiguratorWindow, browser: &Rc<RefCell<Browser>>, opened: &Folder) {
    window.on_open_folder({
        let (weak, browser, opened) = (window.as_weak(), browser.clone(), opened.clone());
        move || {
            let (weak, browser, opened) = (weak.clone(), browser.clone(), opened.clone());
            let start = opened.borrow().clone();
            // Ждём ответ окна выбора, не блокируя интерфейс.
            let _ = slint::spawn_local(async move {
                let mut dialog = rfd::AsyncFileDialog::new().set_title("Папка конфигурации");
                if let Some(start) = start {
                    dialog = dialog.set_directory(start);
                }
                let Some(folder) = dialog.pick_folder().await else {
                    return;
                };
                let Some(window) = weak.upgrade() else { return };
                let dir = folder.path().to_path_buf();
                crate::settings::remember_config_folder(&dir);
                open_folder(&window, &browser, &opened, dir);
            });
        }
    });
}

/// Читает папку: дерево, название и версия из манифеста и сразу проверка. Если конфигурация
/// не прочиталась, дерево пустое, а причины в панели «Проверка».
fn open_folder(
    window: &ConfiguratorWindow,
    browser: &Rc<RefCell<Browser>>,
    opened: &Folder,
    dir: PathBuf,
) {
    let (config, empty_text) = match Configuration::load(&dir) {
        Ok(config) => (config, "В конфигурации нет объектов."),
        Err(_) => (
            Configuration::empty(),
            "Конфигурация не прочиталась: ошибки с файлом и строкой в панели «Проверка» внизу.",
        ),
    };
    window.set_editor_empty_text(empty_text.into());
    window.set_config_title(config.title.as_str().into());
    window.set_config_version(config.version.as_str().into());
    window.set_config_folder(crate::app::short_home(&dir.to_string_lossy()).into());
    window.set_run_error("".into());
    show_python_warning(window, &dir);
    *browser.borrow_mut() = Browser::new(config);
    show_browser(window, &browser.borrow());
    show_check(window, &CheckReport::run(&dir));
    *opened.borrow_mut() = Some(dir);
}

/// Правила на Python эта сборка не читает (`ostov-py` не подключён): говорим сразу, а не
/// когда проведение в предпросмотре поведёт себя не так, как написано в `.py`.
fn show_python_warning(window: &ConfiguratorWindow, dir: &Path) {
    let warning = if crate::test_run::has_python(dir) {
        crate::test_run::NO_PYTHON
    } else {
        ""
    };
    window.set_python_warning(warning.into());
}

/// «Запустить» и «План миграции»: проверка, план в окне, по «Запустить» клиент предпросмотра.
fn connect_run(window: &ConfiguratorWindow, browser: &Rc<RefCell<Browser>>, opened: &Folder) {
    let running: Running = Rc::default();
    window.on_run_requested({
        let (weak, browser, opened) = (window.as_weak(), browser.clone(), opened.clone());
        let running = running.clone();
        move || {
            let Some(window) = weak.upgrade() else { return };
            let Some(dir) = opened.borrow().clone() else {
                return;
            };
            // Запускается то, что сейчас в файлах: папка перечитывается и проверяется заново.
            recheck(&window, &browser, &dir);
            if window.get_check_failed() {
                show_run_error(
                    &window,
                    "В конфигурации ошибки: исправьте их (панель «Проверка» внизу) и запустите снова.",
                    false,
                );
                return;
            }
            let previous = running.borrow_mut().take();
            prepare_in_background(&window, dir, previous, false);
        }
    });
    window.on_plan_reset({
        let (weak, opened, running) = (window.as_weak(), opened.clone(), running.clone());
        move || {
            let Some(window) = weak.upgrade() else { return };
            let Some(dir) = opened.borrow().clone() else {
                return;
            };
            window.set_plan_open(false);
            let previous = running.borrow_mut().take();
            prepare_in_background(&window, dir, previous, true);
        }
    });
    window.on_plan_launch({
        let (weak, opened) = (window.as_weak(), opened.clone());
        move || {
            let Some(window) = weak.upgrade() else { return };
            let Some(dir) = opened.borrow().clone() else {
                return;
            };
            window.set_plan_open(false);
            let base = crate::test_run::base_path(&dir);
            match Preview::spawn(&dir, &base) {
                Ok(preview) => *running.borrow_mut() = Some(preview),
                Err(error) => show_run_error(
                    &window,
                    &format!("Клиент предпросмотра не запустился: {error}."),
                    false,
                ),
            }
        }
    });
}

/// Останавливает прошлый предпросмотр (до 3 с), готовит тестовую базу и считает план; всё
/// в фоне, окно в это время отвечает. `reset`: базу сначала удалить и создать заново.
fn prepare_in_background(
    window: &ConfiguratorWindow,
    dir: PathBuf,
    previous: Option<Preview>,
    reset: bool,
) {
    window.set_run_busy(true);
    window.set_run_error("".into());
    let weak = window.as_weak();
    let folder = dir.clone();
    call_in_background(
        move || {
            if let Some(previous) = previous {
                previous.stop();
            }
            Ok(if reset {
                crate::test_run::reset(&dir)
            } else {
                crate::test_run::prepare(&dir)
            })
        },
        move |result| {
            let Some(window) = weak.upgrade() else { return };
            window.set_run_busy(false);
            match result {
                Ok(Ok(prepared)) => show_plan(&window, &prepared),
                Ok(Err(RunError::Config(errors))) => {
                    let errors = crate::configurator::relative_to(&folder, errors);
                    show_check(&window, &CheckReport::from_errors(&errors));
                    show_run_error(
                        &window,
                        "В конфигурации ошибки: исправьте их (панель «Проверка» внизу) и запустите снова.",
                        false,
                    );
                }
                Ok(Err(RunError::Engine(error))) | Err(error) => {
                    // Занятую базу сбросом не лечат: её держит другая программа.
                    let resettable = error.code != ErrorCode::AlreadyOpen;
                    show_run_error(&window, &error.message, resettable);
                }
            }
        },
    );
}

fn show_run_error(window: &ConfiguratorWindow, message: &str, resettable: bool) {
    window.set_run_error(message.into());
    window.set_run_error_resettable(resettable);
}

fn show_plan(window: &ConfiguratorWindow, prepared: &Prepared) {
    let base = crate::app::short_home(&prepared.base.to_string_lossy());
    let note = if prepared.created {
        format!("Тестовая база {base} создана сейчас.")
    } else {
        format!("Тестовая база {base}.")
    };
    window.set_plan_note(note.into());
    window.set_plan_lines(model(
        crate::test_run::plan_lines(&prepared.plan)
            .into_iter()
            .map(SharedString::from),
    ));
    window.set_plan_refused(prepared.plan.refused.clone().unwrap_or_default().into());
    window.set_plan_open(true);
}

/// «Проверить»: файлы могли поменяться в редакторе, поэтому папка читается заново; открытый
/// элемент остаётся, если он ещё есть.
fn recheck(window: &ConfiguratorWindow, browser: &Rc<RefCell<Browser>>, dir: &Path) {
    let report = CheckReport::run(dir);
    show_python_warning(window, dir);
    if let Ok(config) = Configuration::load(dir) {
        window.set_editor_empty_text("В конфигурации нет объектов.".into());
        window.set_config_title(config.title.as_str().into());
        window.set_config_version(config.version.as_str().into());
        browser.borrow_mut().reload(config);
        show_browser(window, &browser.borrow());
    }
    show_check(window, &report);
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
    crate::x11_background::set_theme_color(window.window(), window.global::<Theme>().get_bg());
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

fn connect_check(window: &ConfiguratorWindow, browser: &Rc<RefCell<Browser>>, opened: &Folder) {
    // Проверка читает десяток маленьких файлов за миллисекунды, поэтому идёт прямо в обработчике.
    window.on_check_requested({
        let (weak, browser, opened) = (window.as_weak(), browser.clone(), opened.clone());
        move || {
            let Some(window) = weak.upgrade() else { return };
            if let Some(dir) = opened.borrow().as_deref() {
                recheck(&window, &browser, dir);
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
