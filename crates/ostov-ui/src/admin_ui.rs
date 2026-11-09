//! Экраны «Журнал действий» и «Закрытие периода» на командах ядра `admin_journal`,
//! `closing_date` и `set_closing_date`. Вызовы идут в фоне, результат ложится в окно в потоке
//! интерфейса. Данные одни на все окна с вкладками. Права проверяет ядро, интерфейс только не
//! показывает пункты меню без права (`Shell::from_meta`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::admin::{Closing, JOURNAL_PAGE, Journal, format_time, iso_date, user_label};
use crate::engine::{Engine, EngineResult, SystemRights, call_in_background};
use crate::windows::Windows;
use crate::{AdminData, AppWindow, JournalRow};

/// Откуда взять открытую базу: сессия живёт в `app.rs`, здесь нужен только движок.
type EngineSource = Rc<dyn Fn() -> Option<Arc<dyn Engine>>>;

#[derive(Default)]
struct State {
    journal: Journal,
    journal_loading: bool,
    journal_error: String,
    closing: Closing,
    closing_busy: bool,
    closing_error: String,
}

#[derive(Clone)]
pub struct AdminUi {
    main: slint::Weak<AppWindow>,
    windows: Windows,
    engine: EngineSource,
    state: Rc<RefCell<State>>,
    /// Растёт, когда меняется база или пользователь: ответ на запрос, отправленный раньше, уже
    /// не про текущие данные и выбрасывается.
    epoch: Rc<Cell<u32>>,
}

impl AdminUi {
    pub fn new(
        main: slint::Weak<AppWindow>,
        windows: Windows,
        engine: impl Fn() -> Option<Arc<dyn Engine>> + 'static,
    ) -> Self {
        Self {
            main,
            windows,
            engine: Rc::new(engine),
            state: Rc::default(),
            epoch: Rc::default(),
        }
    }

    /// Забывает данные прошлой базы или пользователя; ответы на уже отправленные запросы пропадают.
    pub fn clear(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        *self.state.borrow_mut() = State::default();
        self.show_rows();
        self.show_flags();
    }

    /// Вход в главное окно: данные нужны только тем экранам, на которые есть право.
    pub fn start(&self, rights: SystemRights) {
        self.clear();
        if rights.admin {
            self.refresh_journal();
        }
        if rights.set_closing_date {
            self.reload_closing();
        }
    }

    /// Подключает данные и кнопки обоих экранов к окну `window` (главному или дополнительному).
    pub fn attach_window(&self, window: &AppWindow) {
        self.show_window(window);
        let data = window.global::<AdminData>();
        data.on_journal_refresh({
            let admin = self.clone();
            move || admin.refresh_journal()
        });
        data.on_journal_load_more({
            let admin = self.clone();
            move || admin.load_more()
        });
        data.on_closing_reload({
            let admin = self.clone();
            move || admin.reload_closing()
        });
        data.on_closing_close({
            let (admin, window) = (self.clone(), window.as_weak());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let data = window.global::<AdminData>();
                let date = iso_date(
                    data.get_closing_year(),
                    data.get_closing_month(),
                    data.get_closing_day(),
                );
                admin.set_closing(Some(date));
            }
        });
        data.on_closing_open({
            let admin = self.clone();
            move || admin.set_closing(None)
        });
    }

    // ------------------------------------------------------------------ журнал

    /// Первая страница заново («Обновить», открытие вкладки).
    fn refresh_journal(&self) {
        self.load_journal(None);
    }

    /// Следующая страница; без ключа (записей больше нет) ничего не делает.
    fn load_more(&self) {
        let key = self.state.borrow().journal.next_key();
        if key.is_some() {
            self.load_journal(key);
        }
    }

    fn load_journal(&self, after_key: Option<String>) {
        let Some(engine) = (self.engine)() else {
            return;
        };
        // Один запрос за раз: «Ещё» поверх «Обновить» перемешало бы страницы.
        if std::mem::replace(&mut self.state.borrow_mut().journal_loading, true) {
            return;
        }
        self.show_flags();
        let (admin, epoch, first) = (self.clone(), self.epoch.get(), after_key.is_none());
        call_in_background(
            move || engine.admin_journal(after_key.as_deref(), JOURNAL_PAGE),
            move |result| {
                if admin.epoch.get() != epoch {
                    return;
                }
                {
                    let mut state = admin.state.borrow_mut();
                    state.journal_loading = false;
                    match result {
                        Ok(page) if first => {
                            state.journal.replace(page);
                            state.journal_error.clear();
                        }
                        Ok(page) => {
                            state.journal.append(page);
                            state.journal_error.clear();
                        }
                        Err(error) => state.journal_error = error.message,
                    }
                }
                admin.show_rows();
                admin.show_flags();
            },
        );
    }

    // ------------------------------------------------------------------ закрытие периода

    fn reload_closing(&self) {
        self.run_closing(|engine| engine.closing_date());
    }

    /// Закрывает период по дату или (`None`) открывает; потом читает дату заново, чтобы на экране
    /// было то, что записало ядро.
    fn set_closing(&self, date: Option<String>) {
        self.run_closing(move |engine| {
            engine.set_closing_date(date.as_deref())?;
            engine.closing_date()
        });
    }

    fn run_closing(
        &self,
        command: impl FnOnce(&dyn Engine) -> EngineResult<Option<String>> + Send + 'static,
    ) {
        let Some(engine) = (self.engine)() else {
            return;
        };
        if std::mem::replace(&mut self.state.borrow_mut().closing_busy, true) {
            return;
        }
        self.show_flags();
        let (admin, epoch) = (self.clone(), self.epoch.get());
        call_in_background(
            move || command(&*engine),
            move |result| {
                if admin.epoch.get() != epoch {
                    return;
                }
                let known = {
                    let mut state = admin.state.borrow_mut();
                    state.closing_busy = false;
                    match result {
                        Ok(date) => {
                            state.closing = Closing::new(date);
                            state.closing_error.clear();
                            true
                        }
                        // Ошибка ядра как есть: нет права, дата не распознана.
                        Err(error) => {
                            state.closing_error = error.message;
                            false
                        }
                    }
                };
                admin.show_flags();
                if known {
                    admin.show_date();
                }
            },
        );
    }

    // ------------------------------------------------------------------ показ в окнах

    fn each_window(&self, mut show: impl FnMut(&AppWindow)) {
        if let Some(main) = self.main.upgrade() {
            show(&main);
        }
        self.windows.each(show);
    }

    fn show_window(&self, window: &AppWindow) {
        self.show_rows_in(window);
        self.show_flags_in(window);
        self.show_date_in(window);
    }

    /// Строки журнала: модель собирается заново, только когда изменились сами записи.
    fn show_rows(&self) {
        self.each_window(|window| self.show_rows_in(window));
    }

    /// Всё остальное (загрузка, ошибки, надписи): дешёвые свойства, строки не трогаем.
    fn show_flags(&self) {
        self.each_window(|window| self.show_flags_in(window));
    }

    /// Дата закрытия в поле даты. Не вызывается при каждом показе: иначе недописанная дата
    /// пропадала бы при обновлении журнала.
    fn show_date(&self) {
        self.each_window(|window| self.show_date_in(window));
    }

    fn show_rows_in(&self, window: &AppWindow) {
        let rows: Vec<JournalRow> = self
            .state
            .borrow()
            .journal
            .entries()
            .iter()
            .map(|entry| JournalRow {
                when: format_time(&entry.when).into(),
                user: user_label(&entry.user).into(),
                action: entry.action.as_str().into(),
                object: entry.object.as_str().into(),
                details: entry.details.as_str().into(),
            })
            .collect();
        window
            .global::<AdminData>()
            .set_journal(ModelRc::new(VecModel::from(rows)));
    }

    fn show_flags_in(&self, window: &AppWindow) {
        let state = self.state.borrow();
        let data = window.global::<AdminData>();
        data.set_journal_loading(state.journal_loading);
        data.set_journal_more(state.journal.has_more());
        data.set_journal_error(state.journal_error.as_str().into());
        data.set_closing_text(state.closing.text().into());
        data.set_closing_loaded(state.closing.is_known());
        data.set_closing_set(state.closing.is_closed());
        data.set_closing_busy(state.closing_busy);
        data.set_closing_error(state.closing_error.as_str().into());
    }

    fn show_date_in(&self, window: &AppWindow) {
        let Some((year, month, day)) = self.state.borrow().closing.date() else {
            return;
        };
        let data = window.global::<AdminData>();
        data.set_closing_year(year);
        data.set_closing_month(month);
        data.set_closing_day(day);
    }
}
