//! Табличные части в форме: таблица строк и окно строки. Правки идут в `FormState`, ядро здесь
//! не зовётся: строки уходят в ядро вместе с записью или проведением формы.

use slint::{ComponentHandle, Model, VecModel};

use super::{Draw, Form, FormUi, cell_item, date_args, shown_key};
use crate::values::date_text;
use crate::{AppWindow, FormData, FormFieldItem};

/// `(часть, строка)` из колбэка таблицы; `None`, если номера отрицательные.
fn place(part: i32, row: i32) -> Option<(usize, usize)> {
    Some((usize::try_from(part).ok()?, usize::try_from(row).ok()?))
}

impl FormUi {
    /// События таблиц и окна строки в окне `window`.
    pub(super) fn attach_parts(&self, window: &AppWindow) {
        let data = window.global::<FormData>();
        let bound = || (self.clone(), window.as_weak());
        data.on_row_add({
            let (ui, window) = bound();
            move |part| {
                let (Some(window), Ok(part)) = (window.upgrade(), usize::try_from(part)) else {
                    return;
                };
                ui.change(&window, |state| {
                    state.add_row(part).then_some(Draw::FocusCell(None))
                });
            }
        });
        data.on_row_select({
            let (ui, window) = bound();
            move |part, row| {
                let (Some(window), Some((part, row))) = (window.upgrade(), place(part, row)) else {
                    return;
                };
                ui.change(&window, |state| {
                    state.select_row(part, row);
                    Some(Draw::Refresh)
                });
            }
        });
        data.on_open_row({
            let (ui, window) = bound();
            move |part, row| {
                let (Some(window), Some((part, row))) = (window.upgrade(), place(part, row)) else {
                    return;
                };
                ui.change(&window, |state| {
                    state.open_row(part, row).then_some(Draw::FocusCell(None))
                });
            }
        });
        data.on_row_remove({
            let (ui, window) = bound();
            move |part, row| {
                let (Some(window), Some((part, row))) = (window.upgrade(), place(part, row)) else {
                    return;
                };
                ui.change(&window, |state| {
                    state.remove_row(part, row).then_some(Draw::Refresh)
                });
            }
        });
        data.on_row_edited({
            let (ui, window) = bound();
            move |cell, text| {
                if let (Some(window), Ok(cell)) = (window.upgrade(), usize::try_from(cell)) {
                    ui.edit_cell(&window, cell, &text);
                }
            }
        });
        data.on_row_date_edited({
            let (ui, window) = bound();
            move |cell, day, month, year| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                if let Some((cell, date)) = date_args(cell, day, month, year) {
                    ui.edit_cell(&window, cell, &date_text(date));
                }
            }
        });
        data.on_row_done({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    // Не готово: ошибки встают у ячеек на месте, окно остаётся.
                    ui.change(&window, |state| {
                        let _ = state.commit_row();
                        Some(Draw::Refresh)
                    });
                }
            }
        });
        data.on_row_cancel({
            let (ui, window) = bound();
            move || {
                if let Some(window) = window.upgrade() {
                    ui.change(&window, |state| {
                        state.cancel_row();
                        Some(Draw::Refresh)
                    });
                }
            }
        });
    }

    /// Правка ячейки окна строки текстом: редактор остаётся, набранное в нём не теряется.
    fn edit_cell(&self, window: &AppWindow, cell: usize, text: &str) {
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
            state.edit_cell(cell, text);
            (was, state.is_modified())
        };
        {
            let base = self.base.borrow();
            let draft = base
                .forms
                .get(&key)
                .and_then(Form::state)
                .and_then(|state| state.parts().draft());
            if let Some(item) = draft.and_then(|draft| cell_item(draft, cell, &base.objects)) {
                let data = window.global::<FormData>();
                let fields = data.get_row_fields();
                if let Some(model) = fields.as_any().downcast_ref::<VecModel<FormFieldItem>>()
                    && cell < model.row_count()
                {
                    model.set_row_data(cell, item);
                }
            }
        }
        if was != now {
            self.mark_tab(&key, now);
        }
    }
}
