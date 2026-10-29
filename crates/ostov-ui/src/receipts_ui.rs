//! Экран «Поступления»: состояние списка и формы, связь с Slint через `ReceiptsData`.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::receipts::{self, COLUMNS, Column, Receipt};
use crate::{AppWindow, GridColumn, GridRow, ReceiptsData};

/// Ширины колонок списка в px: гибкая только «Поставщик», её ширина здесь минимум.
const DEFAULT_WIDTHS: [f32; 6] = [70.0, 110.0, 220.0, 130.0, 140.0, 110.0];
const SUPPLIER: usize = 2;
const RIGHT_ALIGNED: [usize; 1] = [4];

struct State {
    all: Vec<Receipt>,
    /// Колонка и признак «по убыванию».
    sort: Option<(usize, bool)>,
    query: String,
    widths: [f32; 6],
    /// Номер документа, открытого в форме (`None`: новый, ещё не записан).
    editing: Option<u32>,
    next_number: u32,
}

impl State {
    fn visible(&self) -> Vec<&Receipt> {
        receipts::filter(&self.all, &self.query)
    }
}

fn grid_row(cells: Vec<String>, muted: bool) -> GridRow {
    GridRow {
        cells: ModelRc::new(VecModel::from(
            cells
                .into_iter()
                .map(slint::SharedString::from)
                .collect::<Vec<_>>(),
        )),
        muted,
    }
}

fn columns(state: &State) -> Vec<GridColumn> {
    COLUMNS
        .iter()
        .enumerate()
        .map(|(i, column)| GridColumn {
            title: column.title().into(),
            width: state.widths[i],
            right: RIGHT_ALIGNED.contains(&i),
            flex: i == SUPPLIER,
        })
        .collect()
}

fn refresh_list(app: &AppWindow, state: &State) {
    let data = app.global::<ReceiptsData>();
    let rows: Vec<GridRow> = state
        .visible()
        .iter()
        .map(|receipt| grid_row(receipt.cells(), false))
        .collect();
    data.set_columns(ModelRc::new(VecModel::from(columns(state))));
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

fn line_columns() -> Vec<GridColumn> {
    let column = |title: &str, width: f32, right: bool, flex: bool| GridColumn {
        title: title.into(),
        width,
        right,
        flex,
    };
    vec![
        column("№", 56.0, false, false),
        column("Номенклатура", 220.0, false, true),
        column("Ед.", 70.0, false, false),
        column("Количество", 120.0, true, false),
        column("Цена", 120.0, true, false),
        column("Сумма", 140.0, true, false),
    ]
}

/// Показывает документ в форме. `None`: новый черновик.
fn open_form(app: &AppWindow, state: &mut State, receipt: Option<Receipt>) {
    let data = app.global::<ReceiptsData>();
    let (title, posted, number, date, warehouse, supplier, lines) = match &receipt {
        Some(r) => (
            format!(
                "Поступление № {} от {}",
                r.number,
                receipts::date_text(r.date)
            ),
            r.posted,
            r.number.to_string(),
            receipts::date_text(r.date),
            r.warehouse.clone(),
            r.supplier.clone(),
            receipts::lines(r),
        ),
        None => (
            "Новое поступление".to_owned(),
            false,
            state.next_number.to_string(),
            receipts::date_text((2026, 10, 29)),
            "Основной".to_owned(),
            String::new(),
            Vec::new(),
        ),
    };
    state.editing = receipt.map(|r| r.number);
    let total: i64 = lines.iter().map(|l| l.amount).sum();
    let rows: Vec<GridRow> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            grid_row(
                vec![
                    (i + 1).to_string(),
                    line.name.to_owned(),
                    line.unit.to_owned(),
                    receipts::quantity(line.quantity_tenths),
                    receipts::money(line.price()),
                    receipts::money(line.amount),
                ],
                false,
            )
        })
        .collect();
    data.set_form_title(title.into());
    data.set_form_posted(posted);
    data.set_form_number(number.into());
    data.set_form_date(date.into());
    data.set_form_warehouse(warehouse.into());
    data.set_form_supplier(supplier.into());
    data.set_line_columns(ModelRc::new(VecModel::from(line_columns())));
    data.set_lines(ModelRc::new(VecModel::from(rows)));
    data.set_form_total(receipts::money(total).into());
    data.set_form_open(true);
}

/// Переносит поля формы в документ (новый добавляет в список). Возвращает номер документа.
fn apply_form(app: &AppWindow, state: &mut State, post: bool) -> u32 {
    let data = app.global::<ReceiptsData>();
    let supplier = data.get_form_supplier().to_string();
    let warehouse = data.get_form_warehouse().to_string();
    let date = receipts::parse_date(&data.get_form_date());
    let number = state.editing.unwrap_or(state.next_number);
    if let Some(receipt) = state.all.iter_mut().find(|r| r.number == number) {
        receipt.supplier = supplier;
        receipt.warehouse = warehouse;
        if let Some(date) = date {
            receipt.date = date;
        }
        receipt.posted |= post;
    } else {
        state.all.insert(
            0,
            Receipt {
                number,
                date: date.unwrap_or((2026, 10, 29)),
                supplier,
                warehouse,
                amount: 0,
                posted: post,
            },
        );
        state.next_number += 1;
        state.editing = Some(number);
    }
    number
}

/// Подключает список и форму поступлений к окну.
pub fn setup(app: &AppWindow) {
    let all = receipts::demo(2000);
    let next_number = all.iter().map(|r| r.number).max().unwrap_or(0) + 1;
    let state = Rc::new(RefCell::new(State {
        all,
        sort: None,
        query: String::new(),
        widths: DEFAULT_WIDTHS,
        editing: None,
        next_number,
    }));
    refresh_list(app, &state.borrow());
    let data = app.global::<ReceiptsData>();

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

    data.on_resize({
        let (app, state) = (app.as_weak(), state.clone());
        move |column, width| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            if let Some(slot) = usize::try_from(column)
                .ok()
                .and_then(|i| state.widths.get_mut(i))
            {
                *slot = width;
            }
            app.global::<ReceiptsData>()
                .set_columns(ModelRc::new(VecModel::from(columns(&state))));
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
        let (app, state) = (app.as_weak(), state.clone());
        move |row| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let picked = usize::try_from(row)
                .ok()
                .and_then(|i| state.visible().get(i).map(|r| (*r).clone()));
            if let Some(receipt) = picked {
                open_form(&app, &mut state, Some(receipt));
            }
        }
    });

    data.on_create({
        let (app, state) = (app.as_weak(), state.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            open_form(&app, &mut state.borrow_mut(), None);
        }
    });

    data.on_form_save({
        let (app, state) = (app.as_weak(), state.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let number = apply_form(&app, &mut state, false);
            let receipt = state.all.iter().find(|r| r.number == number).cloned();
            open_form(&app, &mut state, receipt);
            refresh_list(&app, &state);
        }
    });

    data.on_form_post({
        let (app, state) = (app.as_weak(), state.clone());
        move |close| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let number = apply_form(&app, &mut state, true);
            if close {
                app.global::<ReceiptsData>().set_form_open(false);
            } else {
                let receipt = state.all.iter().find(|r| r.number == number).cloned();
                open_form(&app, &mut state, receipt);
            }
            refresh_list(&app, &state);
        }
    });

    data.on_form_close({
        let app = app.as_weak();
        move || {
            let Some(app) = app.upgrade() else { return };
            app.global::<ReceiptsData>().set_form_open(false);
        }
    });
}
