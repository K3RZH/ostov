//! Экран «Поступления»: список и документы в своих вкладках, связь с Slint через `ReceiptsData`.
//!
//! Каждый открытый документ хранит своё состояние (`DocState`), поэтому в нескольких вкладках
//! можно править разные документы. Поля формы лежат в глобальных свойствах Slint только для
//! вкладки на экране; при переключении они сохраняются в `DocState` и загружаются из него.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::receipts::{self, COLUMNS, Column, Line, Receipt};
use crate::shell::{RECEIPTS_NODE, Shell};
use crate::{AppWindow, GridColumn, GridRow, ReceiptsData};

/// Ширины колонок списка в px: гибкая только «Поставщик», её ширина здесь минимум.
/// Дальше ширины меняет пользователь перетаскиванием (это делает сама таблица).
const DEFAULT_WIDTHS: [f32; 6] = [64.0, 104.0, 120.0, 120.0, 130.0, 108.0];
const SUPPLIER: usize = 2;
const AMOUNT: usize = 4;
const STATUS: usize = 5;
const NEW_DOC_DATE: (u16, u8, u8) = (2026, 10, 29);

/// Содержимое документа, которое пользователь может изменить.
#[derive(Clone, PartialEq, Eq)]
struct Content {
    supplier: String,
    warehouse: String,
    date: String,
    lines: Vec<Line>,
}

struct DocState {
    number: u32,
    posted: bool,
    /// Документ уже есть в списке (новый попадёт туда при записи).
    saved: bool,
    content: Content,
    /// Содержимое на момент открытия или последней записи: с ним сравниваем, чтобы понять,
    /// изменён ли документ.
    baseline: Content,
}

impl DocState {
    fn from_receipt(receipt: &Receipt) -> Self {
        let content = Content {
            supplier: receipt.supplier.clone(),
            warehouse: receipt.warehouse.clone(),
            date: receipts::date_text(receipt.date),
            lines: receipt
                .lines
                .clone()
                .unwrap_or_else(|| receipts::lines(receipt)),
        };
        Self {
            number: receipt.number,
            posted: receipt.posted,
            saved: true,
            baseline: content.clone(),
            content,
        }
    }

    fn new_document(number: u32) -> Self {
        let content = Content {
            supplier: String::new(),
            warehouse: receipts::WAREHOUSES[0].to_owned(),
            date: receipts::date_text(NEW_DOC_DATE),
            lines: Vec::new(),
        };
        Self {
            number,
            posted: false,
            saved: false,
            baseline: content.clone(),
            content,
        }
    }

    fn is_modified(&self) -> bool {
        self.content != self.baseline
    }

    fn title(&self) -> String {
        if self.saved {
            format!("Поступление № {} от {}", self.number, self.content.date)
        } else {
            "Новое поступление".to_owned()
        }
    }

    fn total(&self) -> i64 {
        self.content.lines.iter().map(|l| l.amount).sum()
    }
}

struct State {
    all: Vec<Receipt>,
    /// Колонка и признак «по убыванию».
    sort: Option<(usize, bool)>,
    query: String,
    next_number: u32,
    docs: HashMap<u32, DocState>,
    /// Документ, поля которого сейчас в форме на экране.
    loaded: Option<u32>,
    /// Какая строка правится в окне строки (`None`: добавляется новая).
    editing_line: Option<usize>,
}

impl State {
    fn visible(&self) -> Vec<&Receipt> {
        receipts::filter(&self.all, &self.query)
    }
}

/// Точка входа для оболочки: перед и после любого действия с вкладками.
#[derive(Clone)]
pub struct ReceiptsUi {
    state: Rc<RefCell<State>>,
}

impl ReceiptsUi {
    /// Вызывать ПЕРЕД действием с вкладками: сохраняет поля формы и закрепляет изменённую вкладку,
    /// чтобы следующее открытие не заменило её.
    pub fn before(&self, app: &AppWindow, shell: &mut Shell) {
        before(app, &mut self.state.borrow_mut(), shell);
    }

    /// Вызывать ПОСЛЕ действия с вкладками: забывает закрытые документы и загружает форму
    /// документа, чья вкладка стала активной.
    pub fn after(&self, app: &AppWindow, shell: &Shell) {
        after(app, &mut self.state.borrow_mut(), shell);
    }
}

fn texts(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|s| (*s).into())
            .collect::<Vec<SharedString>>(),
    ))
}

fn grid_row(cells: Vec<String>, muted: bool) -> GridRow {
    GridRow {
        cells: ModelRc::new(VecModel::from(
            cells
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )),
        muted,
    }
}

fn list_columns() -> Vec<GridColumn> {
    COLUMNS
        .iter()
        .enumerate()
        .map(|(i, column)| GridColumn {
            title: column.title().into(),
            width: DEFAULT_WIDTHS[i],
            right: i == AMOUNT,
            flex: i == SUPPLIER,
            status: i == STATUS,
        })
        .collect()
}

fn line_columns() -> Vec<GridColumn> {
    let column = |title: &str, width: f32, right: bool, flex: bool| GridColumn {
        title: title.into(),
        width,
        right,
        flex,
        status: false,
    };
    vec![
        column("№", 56.0, false, false),
        column("Номенклатура", 140.0, false, true),
        column("Ед.", 70.0, false, false),
        column("Количество", 120.0, true, false),
        column("Цена", 120.0, true, false),
        column("Сумма", 140.0, true, false),
    ]
}

/// Шаг вех для списка: около десяти отметок на «круглых» числах строк (10, 50, 100, 500, …).
fn milestone_step(rows: usize) -> i32 {
    if rows < 50 {
        return 0;
    }
    let target = rows / 10;
    let mut step = 10;
    loop {
        for factor in [1, 2, 5] {
            if step * factor >= target {
                return i32::try_from(step * factor).unwrap_or(i32::MAX);
            }
        }
        step *= 10;
    }
}

fn refresh_list(app: &AppWindow, state: &State) {
    let data = app.global::<ReceiptsData>();
    let rows: Vec<GridRow> = state
        .visible()
        .iter()
        .map(|receipt| grid_row(receipt.cells(), false))
        .collect();
    data.set_milestone_step(milestone_step(rows.len()));
    data.set_rows(ModelRc::new(VecModel::from(rows)));
    data.set_total(i32::try_from(state.all.len()).unwrap_or(i32::MAX));
    match state.sort {
        Some((column, descending)) => {
            data.set_sort_column(i32::try_from(column).unwrap_or(-1));
            data.set_sort_desc(descending);
        }
        None => data.set_sort_column(-1),
    }
}

/// Показывает строки и итоги документа, открытого в форме.
fn render_lines(app: &AppWindow, doc: &DocState) {
    let data = app.global::<ReceiptsData>();
    let rows: Vec<GridRow> = doc
        .content
        .lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            grid_row(
                vec![
                    (i + 1).to_string(),
                    line.name.clone(),
                    line.unit.clone(),
                    receipts::quantity(line.quantity_tenths),
                    receipts::money(line.price),
                    receipts::money(line.amount),
                ],
                false,
            )
        })
        .collect();
    data.set_lines(ModelRc::new(VecModel::from(rows)));
    data.set_form_count(receipts::positions(doc.content.lines.len()).into());
    data.set_form_total(receipts::money(doc.total()).into());
}

/// Сохраняет поля формы на экране в состояние её документа.
fn store_form(app: &AppWindow, state: &mut State) {
    let Some(number) = state.loaded else { return };
    let data = app.global::<ReceiptsData>();
    if let Some(doc) = state.docs.get_mut(&number) {
        doc.content.supplier = data.get_form_supplier().to_string();
        doc.content.warehouse = data.get_form_warehouse().to_string();
        doc.content.date = data.get_form_date().to_string();
    }
}

/// Загружает документ в форму на экране.
fn load_doc(app: &AppWindow, state: &mut State, number: u32) {
    let Some(doc) = state.docs.get(&number) else {
        return;
    };
    let data = app.global::<ReceiptsData>();
    data.set_form_title(doc.title().into());
    data.set_form_posted(doc.posted);
    data.set_form_number(doc.number.to_string().into());
    data.set_form_date(doc.content.date.as_str().into());
    data.set_form_warehouse(doc.content.warehouse.as_str().into());
    data.set_form_supplier(doc.content.supplier.as_str().into());
    data.set_line_columns(ModelRc::new(VecModel::from(line_columns())));
    data.set_line_selected(-1);
    data.set_line_open(false);
    render_lines(app, doc);
    state.loaded = Some(number);
}

fn before(app: &AppWindow, state: &mut State, shell: &mut Shell) {
    store_form(app, state);
    if let Some(number) = state.loaded {
        let modified = state.docs.get(&number).is_some_and(DocState::is_modified);
        if let Some(tab) = shell.doc_tab(number).filter(|_| modified) {
            shell.set_modified(tab, true);
        }
    }
}

fn after(app: &AppWindow, state: &mut State, shell: &Shell) {
    state
        .docs
        .retain(|number, _| shell.doc_tab(*number).is_some());
    match shell.active_doc().filter(|n| state.docs.contains_key(n)) {
        Some(number) if state.loaded != Some(number) => load_doc(app, state, number),
        Some(_) => {}
        None => state.loaded = None,
    }
}

/// Открывает документ в своей вкладке (создаёт его состояние, если вкладки ещё нет).
fn open_doc(app: &AppWindow, state: &mut State, shell: &mut Shell, number: u32) {
    before(app, state, shell);
    if !state.docs.contains_key(&number) {
        let doc = match state.all.iter().find(|r| r.number == number) {
            Some(receipt) => DocState::from_receipt(receipt),
            None => DocState::new_document(number),
        };
        state.docs.insert(number, doc);
    }
    let title = state
        .docs
        .get(&number)
        .map(DocState::title)
        .unwrap_or_default();
    shell.open_doc(RECEIPTS_NODE, number, title, "receipt");
    after(app, state, shell);
    crate::app::refresh(app, shell);
}

/// Записывает открытый документ в список (новый добавляет); `post` ещё и проводит.
fn save_loaded(app: &AppWindow, state: &mut State, shell: &mut Shell, post: bool) {
    store_form(app, state);
    let Some(number) = state.loaded else { return };
    let State { all, docs, .. } = &mut *state;
    let Some(doc) = docs.get_mut(&number) else {
        return;
    };
    let date = receipts::parse_date(&doc.content.date);
    let lines = doc.content.lines.clone();
    let amount = doc.total();
    doc.posted |= post;
    if let Some(receipt) = all.iter_mut().find(|r| r.number == number) {
        receipt.supplier = doc.content.supplier.clone();
        receipt.warehouse = doc.content.warehouse.clone();
        if let Some(date) = date {
            receipt.date = date;
        }
        receipt.amount = amount;
        receipt.lines = Some(lines);
        receipt.posted = doc.posted;
    } else {
        all.insert(
            0,
            Receipt {
                number,
                date: date.unwrap_or(NEW_DOC_DATE),
                supplier: doc.content.supplier.clone(),
                warehouse: doc.content.warehouse.clone(),
                amount,
                posted: doc.posted,
                lines: Some(lines),
            },
        );
    }
    doc.saved = true;
    doc.baseline = doc.content.clone();
    let title = doc.title();
    if let Some(tab) = shell.doc_tab(number) {
        shell.set_modified(tab, false);
    }
    shell.set_doc_title(number, title);
}

/// Закрывает вкладку активного документа.
fn close_active(app: &AppWindow, state: &mut State, shell: &mut Shell) {
    let active = shell.active_tab();
    shell.close_tab(active);
    after(app, state, shell);
}

/// Открывает окно строки: пустое для новой или с данными для правки.
fn open_line_dialog(app: &AppWindow, state: &mut State, index: Option<usize>) {
    let data = app.global::<ReceiptsData>();
    let line = state
        .loaded
        .and_then(|n| state.docs.get(&n))
        .zip(index)
        .and_then(|(doc, i)| doc.content.lines.get(i).cloned());
    state.editing_line = index.filter(|_| line.is_some());
    data.set_line_title(
        if line.is_some() {
            "Изменить товар"
        } else {
            "Добавить товар"
        }
        .into(),
    );
    data.set_line_name(line.as_ref().map_or("", |l| l.name.as_str()).into());
    data.set_line_quantity(
        line.as_ref()
            .map_or(String::new(), |l| receipts::quantity(l.quantity_tenths))
            .into(),
    );
    data.set_line_price(
        line.as_ref()
            .map_or(String::new(), |l| receipts::money(l.price))
            .into(),
    );
    data.set_line_error("".into());
    data.set_line_open(true);
}

/// Подключает список и документы поступлений к окну.
pub fn setup(app: &AppWindow, shell: Rc<RefCell<Shell>>) -> ReceiptsUi {
    let all = receipts::demo(2000);
    let next_number = all.iter().map(|r| r.number).max().unwrap_or(0) + 1;
    let state = Rc::new(RefCell::new(State {
        all,
        sort: None,
        query: String::new(),
        next_number,
        docs: HashMap::new(),
        loaded: None,
        editing_line: None,
    }));
    let data = app.global::<ReceiptsData>();
    data.set_columns(ModelRc::new(VecModel::from(list_columns())));
    data.set_warehouses(texts(&receipts::WAREHOUSES));
    data.set_suppliers(texts(&receipts::SUPPLIERS));
    data.set_goods(texts(
        &receipts::GOODS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
    ));
    refresh_list(app, &state.borrow());

    data.on_sort({
        let (app, state) = (app.as_weak(), state.clone());
        move |column| {
            let Some(app) = app.upgrade() else { return };
            let Some(column) = usize::try_from(column).ok().filter(|c| *c < COLUMNS.len()) else {
                return;
            };
            let mut state = state.borrow_mut();
            let descending = matches!(state.sort, Some((current, false)) if current == column);
            state.sort = Some((column, descending));
            let kind: Column = COLUMNS[column];
            receipts::sort(&mut state.all, kind, descending);
            app.global::<ReceiptsData>().set_selected_row(-1);
            refresh_list(&app, &state);
        }
    });

    data.on_query_changed({
        let (app, state) = (app.as_weak(), state.clone());
        move |query| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            state.query = query.to_string();
            app.global::<ReceiptsData>().set_selected_row(-1);
            refresh_list(&app, &state);
        }
    });

    data.on_open({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move |row| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let picked = usize::try_from(row)
                .ok()
                .and_then(|i| state.visible().get(i).map(|r| r.number));
            if let Some(number) = picked {
                open_doc(&app, &mut state, &mut shell.borrow_mut(), number);
            }
        }
    });

    data.on_create({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let number = state.next_number;
            state.next_number += 1;
            open_doc(&app, &mut state, &mut shell.borrow_mut(), number);
        }
    });

    data.on_form_save({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let (mut state, mut shell) = (state.borrow_mut(), shell.borrow_mut());
            save_loaded(&app, &mut state, &mut shell, false);
            if let Some(number) = state.loaded {
                load_doc(&app, &mut state, number);
            }
            refresh_list(&app, &state);
            crate::app::refresh(&app, &shell);
        }
    });

    data.on_form_post({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move |close| {
            let Some(app) = app.upgrade() else { return };
            let (mut state, mut shell) = (state.borrow_mut(), shell.borrow_mut());
            save_loaded(&app, &mut state, &mut shell, true);
            if close {
                close_active(&app, &mut state, &mut shell);
            } else if let Some(number) = state.loaded {
                load_doc(&app, &mut state, number);
            }
            refresh_list(&app, &state);
            crate::app::refresh(&app, &shell);
        }
    });

    data.on_form_close({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let (mut state, mut shell) = (state.borrow_mut(), shell.borrow_mut());
            before(&app, &mut state, &mut shell);
            close_active(&app, &mut state, &mut shell);
            crate::app::refresh(&app, &shell);
        }
    });

    data.on_line_add({
        let (app, state) = (app.as_weak(), state.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            open_line_dialog(&app, &mut state.borrow_mut(), None);
        }
    });

    data.on_line_edit({
        let (app, state) = (app.as_weak(), state.clone());
        move |row| {
            let Some(app) = app.upgrade() else { return };
            open_line_dialog(&app, &mut state.borrow_mut(), usize::try_from(row).ok());
        }
    });

    data.on_line_delete({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let selected = app.global::<ReceiptsData>().get_line_selected();
            if let Some(doc) = state.loaded.and_then(|n| state.docs.get_mut(&n)) {
                if let Some(index) = usize::try_from(selected)
                    .ok()
                    .filter(|i| *i < doc.content.lines.len())
                {
                    doc.content.lines.remove(index);
                }
                render_lines(&app, doc);
            }
            app.global::<ReceiptsData>().set_line_selected(-1);
            // Состав документа изменился: вкладка закрепляется сразу, а не при следующем открытии.
            before(&app, &mut state, &mut shell.borrow_mut());
            crate::app::refresh(&app, &shell.borrow());
        }
    });

    data.on_line_confirm({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let data = app.global::<ReceiptsData>();
            let name = data.get_line_name().to_string();
            let quantity = receipts::parse_decimal(&data.get_line_quantity(), 1).filter(|q| *q > 0);
            let price = receipts::parse_decimal(&data.get_line_price(), 2);
            let error = match (name.is_empty(), quantity, price) {
                (true, _, _) => "Выберите товар из справочника.",
                (_, None, _) => {
                    "Количество: число больше нуля, не больше одного знака после запятой."
                }
                (_, _, None) => "Цена: число, не больше двух знаков после запятой.",
                _ => "",
            };
            if !error.is_empty() {
                data.set_line_error(error.into());
                return;
            }
            let (Some(quantity), Some(price)) = (quantity, price) else {
                return;
            };
            let line = Line::new(&name, quantity, price);
            let mut state = state.borrow_mut();
            let editing = state.editing_line;
            if let Some(doc) = state.loaded.and_then(|n| state.docs.get_mut(&n)) {
                match editing.filter(|i| *i < doc.content.lines.len()) {
                    Some(index) => doc.content.lines[index] = line,
                    None => doc.content.lines.push(line),
                }
                render_lines(&app, doc);
            }
            data.set_line_open(false);
            before(&app, &mut state, &mut shell.borrow_mut());
            crate::app::refresh(&app, &shell.borrow());
        }
    });

    ReceiptsUi { state }
}
