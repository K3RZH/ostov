//! Список окнами: в памяти лежат несколько окон по `WINDOW_SIZE` строк, а не весь список.
//!
//! Два слоя. `ListWindow` — чистая логика без потоков и Slint: какие окна нужны для видимых
//! строк, чем их грузить (`list` по ключу или `seek` по позиции), что выкинуть из кэша и чьи
//! ответы устарели. `ListModel` — оболочка `slint::Model` поверх него: отдаёт строки или
//! заглушки, запрашивает недостающие окна в фоне и сообщает таблице, когда они пришли.
//!
//! Как грузим (ADR-014, ADR-022): окно N — это строки `[N × size, (N + 1) × size)`.
//! - Первое окно — `list` без ключа, он же даёт `total` и токен списка.
//! - Окно сразу за загруженным — `list` с `next_key` этого окна: так дешевле всего.
//! - Окно, до которого ключа нет (прыжок ползунком, окно выкинуто из кэша, прокрутка вверх) —
//!   один `seek(токен, позиция, размер)`: ядро идёт от ближайшей вехи, а не листает всё подряд.
//!
//! Данные считаются снимком: `total` берётся при открытии. Сменился запрос, сортировка или
//! поиск (или нужно обновить список) — `restart`: поколение растёт, ответы на старые запросы
//! отбрасываются.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use ostov_api::{Engine, EngineError, EngineResult, ListPage, ListQuery, Row};
use slint::{Model, ModelNotify, ModelTracker};

use crate::engine::call_in_background;

/// Строк в одном окне. Больше экрана (30–50 строк), чтобы одного окна хватало на несколько
/// прокруток, но ответ ядра остаётся быстрым.
pub const WINDOW_SIZE: u32 = 100;
/// Окон в памяти: вместе около 800 строк, сколько бы ни было в списке.
pub const MAX_WINDOWS: usize = 8;
/// Меньше трёх нельзя: видимые строки могут лежать на стыке двух окон, плюс упреждающее.
const MIN_WINDOWS: usize = 3;
/// Ядро отдаёт не больше 1000 строк за раз (СТ-51).
const ENGINE_MAX_SIZE: u32 = 1000;
/// Сколько запросов к ядру одновременно. При быстром перетаскивании ползунка видимая область
/// меняется каждый кадр; без предела на каждый кадр ушёл бы свой `seek` и свой поток.
const MAX_IN_FLIGHT: usize = 4;

// ---------------------------------------------------------------- настройки

/// Размер окна и сколько окон держим в памяти.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowConfig {
    size: u32,
    max_windows: usize,
}

impl WindowConfig {
    /// Размер окна приводится к `1..=1000`, число окон — не меньше трёх: видимые строки
    /// могут лежать на стыке двух окон, и ещё одно нужно под упреждающую загрузку.
    pub fn new(size: u32, max_windows: usize) -> Self {
        Self {
            size: size.clamp(1, ENGINE_MAX_SIZE),
            max_windows: max_windows.max(MIN_WINDOWS),
        }
    }

    /// Размер окна как индекс: `size` всегда от 1, поэтому делить на него безопасно.
    fn rows_per_window(self) -> usize {
        usize::try_from(self.size).unwrap_or(1)
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self::new(WINDOW_SIZE, MAX_WINDOWS)
    }
}

// ---------------------------------------------------------------- запрос окна

/// Какой командой ядра грузить окно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `list`: первое окно (без ключа) или следующее за ключом.
    List(ListQuery),
    /// `seek`: окно с позиции `position` (с нуля), токен — из первой страницы.
    Seek {
        token: String,
        position: u64,
        size: u32,
    },
}

/// Запрос одного окна. Уходит в фоновый поток, ответ возвращается в `ListWindow::accept`
/// с тем же `generation` и `window`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub generation: u64,
    pub window: usize,
    pub call: Call,
}

impl Fetch {
    /// Вызывает ядро. Синхронно: звать из фонового потока.
    ///
    /// # Errors
    /// Ошибка ядра отдаётся как есть.
    pub fn run(&self, engine: &dyn Engine) -> EngineResult<ListPage> {
        match &self.call {
            Call::List(query) => engine.list(query),
            Call::Seek {
                token,
                position,
                size,
            } => engine.seek(token, *position, *size),
        }
    }
}

/// Что случилось с ответом ядра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    /// Ответ на запрос прошлого поколения (или на окно, которое никто не ждёт): выброшен.
    Stale,
    /// Пришло первое окно: известны `total` и токен списка, число строк изменилось.
    Opened,
    /// Окно положено в кэш: изменились строки `first..first + count`.
    Rows { first: usize, count: usize },
    /// Ядро вернуло ошибку: её видно в `ListWindow::error`, строки окна остаются заглушками.
    Failed,
}

// ---------------------------------------------------------------- чистая логика

#[derive(Debug)]
struct Loaded {
    rows: Vec<Rc<Row>>,
    /// Ключ окна, которое идёт следом: по нему оно грузится без `seek`.
    next_key: Option<String>,
}

/// Кэш окон одного списка. Не знает ни про потоки, ни про Slint.
#[derive(Debug)]
pub struct ListWindow {
    /// Запрос без `after_key` и `size`: их подставляет каждое окно.
    query: ListQuery,
    config: WindowConfig,
    generation: u64,
    /// Маркер списка для `seek`; приходит вместе с `total` в первой странице.
    token: Option<String>,
    total: Option<u64>,
    loaded: HashMap<usize, Loaded>,
    /// Окна, на которые запрос уже ушёл: чтобы не слать его на каждую отрисовку строки.
    pending: HashSet<usize>,
    /// Окна, которые ядро не отдало: автоматически не повторяем, иначе ошибка крутилась бы
    /// по кругу на каждую отрисовку. Повтор — только явный, через `retry`.
    failed: HashSet<usize>,
    error: Option<EngineError>,
    /// Последние видимые строки: по ним `replan` ищет, что догрузить после ответа, а кэш —
    /// что выкинуть.
    visible: (usize, usize),
}

impl ListWindow {
    /// Новый список; первое окно запросит первый же `wanted`.
    pub fn new(mut query: ListQuery, config: WindowConfig) -> Self {
        query.after_key = None;
        Self {
            query,
            config,
            generation: 0,
            token: None,
            total: None,
            loaded: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            error: None,
            visible: (0, 0),
        }
    }

    pub fn query(&self) -> &ListQuery {
        &self.query
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Сколько строк в списке; `None`, пока не пришло первое окно.
    pub fn total(&self) -> Option<u64> {
        self.total
    }

    /// Число строк для модели: до открытия ноль.
    pub fn row_count(&self) -> usize {
        self.total
            .map_or(0, |total| usize::try_from(total).unwrap_or(usize::MAX))
    }

    /// Сколько окон сейчас лежит в памяти.
    pub fn cached_windows(&self) -> usize {
        self.loaded.len()
    }

    /// Строка `index` (с нуля), если её окно в кэше.
    pub fn row(&self, index: usize) -> Option<Rc<Row>> {
        let per_window = self.config.rows_per_window();
        let window = self.loaded.get(&(index / per_window))?;
        window.rows.get(index % per_window).cloned()
    }

    pub fn is_loaded(&self, index: usize) -> bool {
        self.row(index).is_some()
    }

    /// Последняя ошибка ядра; сбрасывается в `retry` и `restart`.
    pub fn error(&self) -> Option<&EngineError> {
        self.error.as_ref()
    }

    /// Какие окна загрузить, чтобы показать строки `first..=last`. Помечает их запрошенными,
    /// поэтому повторный вызов с тем же диапазоном вернёт пусто, пока ответ не пришёл.
    ///
    /// Окно за запрошенным в этом же диапазоне ждёт ответа: из него придёт ключ, и оно
    /// догрузится дешёвым `list`, а не вторым `seek`. Когда видна вторая половина окна,
    /// следующее за ним грузится заранее (только по ключу, прыжка вниз ради этого не делаем).
    pub fn wanted(&mut self, first: usize, last: usize) -> Vec<Fetch> {
        let (first, last) = if first <= last {
            (first, last)
        } else {
            (last, first)
        };
        self.visible = (first, last);
        let per_window = self.config.rows_per_window();
        let (low, high) = (first / per_window, last / per_window);

        let mut fetches = Vec::new();
        let Some(count) = self.window_count() else {
            // До первой страницы нет ни total, ни токена: можно только открыть список.
            fetches.extend(self.first_page());
            return fetches;
        };
        // Пустой список и прокрутка за конец: окон для этих строк нет.
        let Some(last_window) = count
            .checked_sub(1)
            .filter(|last_window| low <= *last_window)
        else {
            return fetches;
        };
        let high = high.min(last_window);
        for window in low..=high {
            fetches.extend(self.plan(window, false));
        }
        // Упреждающее окно не должно вытеснять видимые: иначе оно грузилось бы по кругу.
        let room = high - low + 2 <= self.config.max_windows;
        let second_half = last % per_window >= per_window - per_window / 2;
        if room && second_half && high < last_window {
            fetches.extend(self.plan(high + 1, true));
        }
        fetches
    }

    /// То же для последних видимых строк: что догрузить после очередного ответа.
    pub fn replan(&mut self) -> Vec<Fetch> {
        let (first, last) = self.visible;
        self.wanted(first, last)
    }

    /// Принимает ответ ядра на `Fetch` с этим `generation` и `window`.
    ///
    /// Ответ не из текущего поколения, как и ответ на окно, которое не ждали, выбрасывается:
    /// ни кэш, ни ошибка от него не меняются.
    pub fn accept(
        &mut self,
        generation: u64,
        window: usize,
        result: EngineResult<ListPage>,
    ) -> Accepted {
        if generation != self.generation || !self.pending.remove(&window) {
            return Accepted::Stale;
        }
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                self.failed.insert(window);
                self.error = Some(error);
                return Accepted::Failed;
            }
        };
        let ListPage {
            rows,
            next_key,
            total,
            token,
        } = page;
        let opened = self.total.is_none();
        if opened {
            let known = u64::try_from(rows.len()).unwrap_or(u64::MAX);
            self.total = Some(total.unwrap_or(known));
            self.token = Some(token);
        }
        let count = rows.len();
        self.loaded.insert(
            window,
            Loaded {
                rows: rows.into_iter().map(Rc::new).collect(),
                next_key,
            },
        );
        self.evict();
        if opened {
            Accepted::Opened
        } else {
            Accepted::Rows {
                first: window.saturating_mul(self.config.rows_per_window()),
                count,
            }
        }
    }

    /// Снимает ошибку и разрешает повторить упавшие окна: вернёт запросы для видимых строк.
    pub fn retry(&mut self) -> Vec<Fetch> {
        self.error = None;
        self.failed.clear();
        self.replan()
    }

    /// Новый запрос (сортировка, поиск, отбор) или обновление тем же запросом. Кэш, токен и
    /// ошибка сбрасываются, поколение растёт: ответы на прежние запросы больше не принимаются.
    /// Первое окно запросит следующий `wanted`.
    pub fn restart(&mut self, mut query: ListQuery) -> u64 {
        query.after_key = None;
        self.query = query;
        self.generation += 1;
        self.token = None;
        self.total = None;
        self.loaded.clear();
        self.pending.clear();
        self.failed.clear();
        self.error = None;
        self.visible = (0, 0);
        self.generation
    }

    /// Сколько окон в списке; `None`, пока не известен `total`.
    fn window_count(&self) -> Option<usize> {
        let windows = self.total?.div_ceil(u64::from(self.config.size));
        Some(usize::try_from(windows).unwrap_or(usize::MAX))
    }

    fn page_query(&self, after_key: Option<&str>) -> ListQuery {
        ListQuery {
            after_key: after_key.map(str::to_owned),
            size: self.config.size,
            ..self.query.clone()
        }
    }

    fn first_page(&mut self) -> Option<Fetch> {
        if self.pending.contains(&0) || self.failed.contains(&0) {
            return None;
        }
        self.pending.insert(0);
        Some(Fetch {
            generation: self.generation,
            window: 0,
            call: Call::List(self.page_query(None)),
        })
    }

    /// Запрос окна, если его ещё нет, не ждём и оно не упало. `prefetch`: только если окно
    /// можно взять по ключу предыдущего, иначе упреждающая загрузка стоила бы лишнего `seek`.
    fn plan(&mut self, window: usize, prefetch: bool) -> Option<Fetch> {
        let known = self.loaded.contains_key(&window)
            || self.pending.contains(&window)
            || self.failed.contains(&window);
        if known || self.pending.len() >= MAX_IN_FLIGHT {
            return None;
        }
        let before = window.checked_sub(1);
        if before.is_some_and(|b| self.pending.contains(&b)) {
            return None;
        }
        let key = before
            .and_then(|b| self.loaded.get(&b))
            .and_then(|loaded| loaded.next_key.as_deref());
        let call = match key {
            Some(key) => Call::List(self.page_query(Some(key))),
            None if prefetch => return None,
            None => Call::Seek {
                token: self.token.clone()?,
                position: u64::try_from(window)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(u64::from(self.config.size)),
                size: self.config.size,
            },
        };
        self.pending.insert(window);
        Some(Fetch {
            generation: self.generation,
            window,
            call,
        })
    }

    /// Выкидывает окна дальше всех от видимой области; видимые не трогает, пока есть другие.
    /// При равном расстоянии уходит то, что выше по списку: чаще листают вниз.
    fn evict(&mut self) {
        let per_window = self.config.rows_per_window();
        let (low, high) = (self.visible.0 / per_window, self.visible.1 / per_window);
        let distance = |window: usize| low.saturating_sub(window).max(window.saturating_sub(high));
        while self.loaded.len() > self.config.max_windows {
            let Some(farthest) = self
                .loaded
                .keys()
                .copied()
                .max_by_key(|window| (distance(*window), Reverse(*window)))
            else {
                break;
            };
            self.loaded.remove(&farthest);
        }
    }
}

// ---------------------------------------------------------------- модель для Slint

/// Строка модели: настоящая или заглушка, пока окно с ней едет из ядра.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LazyRow {
    Placeholder,
    /// `Rc`: Slint зовёт `row_data` на каждую отрисовку, копировать строку каждый раз дорого.
    Loaded(Rc<Row>),
}

impl LazyRow {
    pub fn as_row(&self) -> Option<&Row> {
        match self {
            Self::Placeholder => None,
            Self::Loaded(row) => Some(row),
        }
    }
}

/// Работа для фонового потока.
pub type Job = Box<dyn FnOnce() -> EngineResult<ListPage> + Send>;
/// Приём результата в потоке интерфейса.
pub type Done = Box<dyn FnOnce(EngineResult<ListPage>)>;
/// Как выполнить `Job` в фоне и отдать результат в `Done`. В приложении это
/// `call_in_background`; в проверках без окна его заменяет ручная очередь.
pub type Runner = Rc<dyn Fn(Job, Done)>;

fn background(work: Job, apply: Done) {
    call_in_background(work, apply);
}

/// `slint::Model` над `ListWindow`: `row_count` равен `total`, строки из кэша или заглушки,
/// недостающие окна грузятся в фоне. Живёт в потоке интерфейса.
///
/// Не перебирай такую модель целиком (`iter`): каждая строка запросит своё окно.
pub struct ListModel {
    state: RefCell<ListWindow>,
    notify: ModelNotify,
    engine: Arc<dyn Engine>,
    runner: Runner,
    /// Ответ ядра приходит позже: модель нужна ему живой, но не удерживаемой им.
    this: Weak<ListModel>,
}

impl ListModel {
    /// Открывает список: первое окно сразу уходит в фон.
    pub fn new(engine: Arc<dyn Engine>, query: ListQuery, config: WindowConfig) -> Rc<Self> {
        Self::with_runner(engine, query, config, Rc::new(background))
    }

    /// То же со своим способом запуска фоновой работы (для проверок без окна и таймеров).
    pub fn with_runner(
        engine: Arc<dyn Engine>,
        query: ListQuery,
        config: WindowConfig,
        runner: Runner,
    ) -> Rc<Self> {
        let model = Rc::new_cyclic(|this| Self {
            state: RefCell::new(ListWindow::new(query, config)),
            notify: ModelNotify::default(),
            engine,
            runner,
            this: Weak::clone(this),
        });
        model.ensure_visible(0, 0);
        model
    }

    /// Сколько строк в списке; `None`, пока список не открылся.
    pub fn total(&self) -> Option<u64> {
        self.state.borrow().total()
    }

    /// Последняя ошибка ядра, если окно не загрузилось. Строки такого окна остаются заглушками.
    pub fn error(&self) -> Option<EngineError> {
        self.state.borrow().error().cloned()
    }

    /// Повторяет упавшие окна для видимых строк.
    pub fn retry(&self) {
        let fetches = self.state.borrow_mut().retry();
        self.dispatch(fetches);
    }

    /// Показывает строки `first..=last`: недостающие окна грузятся в фоне. Таблица может звать
    /// это по прокрутке; `row_data` делает то же для каждой строки, которую у неё просят.
    pub fn ensure_visible(&self, first: usize, last: usize) {
        let fetches = self.state.borrow_mut().wanted(first, last);
        self.dispatch(fetches);
    }

    /// Другая сортировка, поиск или отбор: список открывается заново.
    pub fn set_query(&self, query: ListQuery) {
        self.state.borrow_mut().restart(query);
        self.notify.reset();
        self.ensure_visible(0, 0);
    }

    /// Обновляет список тем же запросом.
    pub fn reload(&self) {
        let query = self.state.borrow().query().clone();
        self.set_query(query);
    }

    fn dispatch(&self, fetches: Vec<Fetch>) {
        for fetch in fetches {
            let (generation, window) = (fetch.generation, fetch.window);
            let engine = Arc::clone(&self.engine);
            let this = Weak::clone(&self.this);
            (self.runner)(
                Box::new(move || fetch.run(&*engine)),
                Box::new(move |result| {
                    if let Some(model) = this.upgrade() {
                        model.accept(generation, window, result);
                    }
                }),
            );
        }
    }

    fn accept(&self, generation: u64, window: usize, result: EngineResult<ListPage>) {
        // Заём снимаем до оповещения: таблица в ответ сразу зовёт `row_data`.
        let (accepted, fetches) = {
            let mut state = self.state.borrow_mut();
            let accepted = state.accept(generation, window, result);
            let fetches = match accepted {
                Accepted::Stale => Vec::new(),
                _ => state.replan(),
            };
            (accepted, fetches)
        };
        match accepted {
            Accepted::Opened => self.notify.reset(),
            Accepted::Rows { first, count } => {
                for row in first..first.saturating_add(count) {
                    self.notify.row_changed(row);
                }
            }
            Accepted::Stale | Accepted::Failed => {}
        }
        self.dispatch(fetches);
    }
}

impl Model for ListModel {
    type Data = LazyRow;

    fn row_count(&self) -> usize {
        self.state.borrow().row_count()
    }

    fn row_data(&self, row: usize) -> Option<LazyRow> {
        let (data, fetches) = {
            let mut state = self.state.borrow_mut();
            if row >= state.row_count() {
                return None;
            }
            let fetches = state.wanted(row, row);
            (state.row(row), fetches)
        };
        self.dispatch(fetches);
        Some(data.map_or(LazyRow::Placeholder, LazyRow::Loaded))
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use ostov_api::{ErrorCode, Fields, Value};

    use super::*;

    const TOKEN: &str = "fake-token";

    /// Ядро-двойник: список из `total` строк с id `1..=total`; считает вызовы `list` и `seek`.
    struct FakeEngine {
        total: u64,
        list_keys: Mutex<Vec<Option<String>>>,
        seek_positions: Mutex<Vec<u64>>,
        fail_list: AtomicBool,
        fail_seek: AtomicBool,
    }

    impl FakeEngine {
        fn new(total: u64) -> Arc<Self> {
            Arc::new(Self {
                total,
                list_keys: Mutex::new(Vec::new()),
                seek_positions: Mutex::new(Vec::new()),
                fail_list: AtomicBool::new(false),
                fail_seek: AtomicBool::new(false),
            })
        }

        fn lists(&self) -> usize {
            self.list_keys.lock().unwrap().len()
        }

        fn seeks(&self) -> usize {
            self.seek_positions.lock().unwrap().len()
        }

        /// Страница с id `start + 1..` не больше `size`; `next_key` — id последней строки.
        fn page(&self, start: u64, size: u32, with_total: bool) -> ListPage {
            let end = (start + u64::from(size)).min(self.total);
            let rows = (start..end)
                .map(|index| {
                    let id = i64::try_from(index + 1).unwrap();
                    Row {
                        id,
                        values: Fields::from([("n".to_owned(), Value::Int(id))]),
                    }
                })
                .collect();
            ListPage {
                rows,
                next_key: (end < self.total).then(|| end.to_string()),
                total: with_total.then_some(self.total),
                token: TOKEN.to_owned(),
            }
        }
    }

    fn unavailable() -> EngineError {
        EngineError::new(ErrorCode::Unavailable, "нет связи")
    }

    impl Engine for FakeEngine {
        fn list(&self, query: &ListQuery) -> EngineResult<ListPage> {
            self.list_keys.lock().unwrap().push(query.after_key.clone());
            if self.fail_list.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            let start = match &query.after_key {
                Some(key) => key.parse().unwrap(),
                None => 0,
            };
            Ok(self.page(start, query.size, query.after_key.is_none()))
        }

        fn seek(&self, token: &str, position: u64, size: u32) -> EngineResult<ListPage> {
            self.seek_positions.lock().unwrap().push(position);
            if self.fail_seek.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            assert_eq!(token, TOKEN);
            Ok(self.page(position, size, true))
        }
    }

    fn query() -> ListQuery {
        ListQuery {
            object: "receipt".to_owned(),
            ..ListQuery::default()
        }
    }

    /// Выполняет запросы синхронно и повторяет план, пока есть что догружать: так таблица
    /// «дожидается» окон для видимых строк.
    fn settle(window: &mut ListWindow, engine: &FakeEngine, first: usize, last: usize) {
        let mut fetches = window.wanted(first, last);
        while !fetches.is_empty() {
            for fetch in fetches {
                let result = fetch.run(engine);
                window.accept(fetch.generation, fetch.window, result);
            }
            fetches = window.replan();
        }
    }

    fn id_of(window: &ListWindow, index: usize) -> Option<i64> {
        window.row(index).map(|row| row.id)
    }

    #[test]
    fn first_open_loads_one_window_and_takes_total() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        assert_eq!((window.row_count(), window.total()), (0, None));

        settle(&mut window, &engine, 0, 29);

        assert_eq!(window.total(), Some(20_000));
        assert_eq!(window.row_count(), 20_000);
        assert_eq!(window.cached_windows(), 1);
        assert_eq!((engine.lists(), engine.seeks()), (1, 0));
        assert_eq!(id_of(&window, 0), Some(1));
        assert_eq!(id_of(&window, 99), Some(100));
        assert_eq!(id_of(&window, 100), None);
    }

    #[test]
    fn repeated_wanted_does_not_ask_twice_while_waiting() {
        let mut window = ListWindow::new(query(), WindowConfig::default());
        assert_eq!(window.wanted(0, 29).len(), 1);
        assert!(window.wanted(0, 29).is_empty());
        assert!(window.wanted(5, 6).is_empty());
    }

    #[test]
    fn scrolling_down_loads_next_windows_by_key() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());

        for first in (0..=1_000).step_by(20) {
            settle(&mut window, &engine, first, first + 39);
        }

        assert_eq!(engine.seeks(), 0);
        // Окна 0..=10, каждое по ключу предыдущего.
        let keys: Vec<Option<String>> = std::iter::once(None)
            .chain((1..=10).map(|n| Some((n * 100).to_string())))
            .collect();
        assert_eq!(*engine.list_keys.lock().unwrap(), keys);
        assert_eq!(id_of(&window, 1_000), Some(1_001));
        assert_eq!(id_of(&window, 1_039), Some(1_040));
    }

    #[test]
    fn far_jump_is_one_seek_not_dozens_of_lists() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        settle(&mut window, &engine, 0, 29);

        settle(&mut window, &engine, 15_000, 15_030);

        assert_eq!(*engine.seek_positions.lock().unwrap(), vec![15_000]);
        assert_eq!(engine.lists(), 1);
        assert_eq!(id_of(&window, 15_000), Some(15_001));
        assert_eq!(id_of(&window, 15_099), Some(15_100));
        // Окна рядом не грузились: в них никто не смотрит.
        assert_eq!(id_of(&window, 14_999), None);
        assert_eq!(id_of(&window, 15_100), None);
    }

    #[test]
    fn jump_onto_window_border_is_one_seek_and_one_list() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        settle(&mut window, &engine, 0, 29);

        // Видны строки двух окон: второе берётся по ключу первого, а не вторым seek.
        settle(&mut window, &engine, 15_090, 15_110);

        assert_eq!(*engine.seek_positions.lock().unwrap(), vec![15_000]);
        assert_eq!(engine.lists(), 2);
        assert_eq!(id_of(&window, 15_090), Some(15_091));
        assert_eq!(id_of(&window, 15_110), Some(15_111));
    }

    #[test]
    fn scrolling_back_after_eviction_uses_seek() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::new(100, 3));
        settle(&mut window, &engine, 0, 29);
        for first in [5_000, 9_000] {
            settle(&mut window, &engine, first, first + 30);
        }
        assert!(window.is_loaded(0));

        // Окон три, четвёртое вытесняет самое дальнее от видимой области: окно 0.
        settle(&mut window, &engine, 13_000, 13_030);
        assert!(!window.is_loaded(0));
        settle(&mut window, &engine, 0, 29);

        let seeks = engine.seek_positions.lock().unwrap().clone();
        assert_eq!(seeks, vec![5_000, 9_000, 13_000, 0]);
        assert_eq!(id_of(&window, 0), Some(1));
    }

    #[test]
    fn stale_answer_after_restart_is_dropped() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        settle(&mut window, &engine, 0, 29);
        let old = window.wanted(15_000, 15_030);
        assert_eq!(old.len(), 1);

        let sorted = ListQuery {
            order: Some("n".to_owned()),
            descending: true,
            ..query()
        };
        assert_eq!(window.restart(sorted), 1);
        let stale = &old[0];
        let (ok, failed) = (stale.run(&*engine), Err(unavailable()));
        for result in [ok, failed] {
            let accepted = window.accept(stale.generation, stale.window, result);
            assert_eq!(accepted, Accepted::Stale);
        }

        // Ни строк, ни ошибки от прошлого поколения нет, список ждёт нового первого окна.
        assert_eq!((window.row_count(), window.cached_windows()), (0, 0));
        assert!(window.error().is_none() && !window.is_loaded(15_000));
        let fresh = window.wanted(0, 29);
        let [fetch] = fresh.as_slice() else {
            panic!("ждали один запрос первого окна");
        };
        assert_eq!(fetch.generation, 1);
        let Call::List(first_page) = &fetch.call else {
            panic!("первое окно грузится через list");
        };
        assert_eq!(first_page.after_key, None);
        assert_eq!(first_page.order.as_deref(), Some("n"));
        assert!(first_page.descending);
    }

    #[test]
    fn answer_for_a_window_nobody_waits_for_is_dropped() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        let fetches = window.wanted(0, 29);
        let result = fetches[0].run(&*engine);
        assert_eq!(window.accept(0, 0, result.clone()), Accepted::Opened);
        // Тот же ответ второй раз: окно уже принято, а не ждёт.
        assert_eq!(window.accept(0, 0, result), Accepted::Stale);
    }

    #[test]
    fn cache_never_exceeds_the_limit_and_keeps_the_visible_area() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::new(100, 3));

        for first in (0..=1_950).step_by(25) {
            settle(&mut window, &engine, first, first + 39);
            assert!(window.cached_windows() <= 3, "на строке {first}");
            assert!(window.is_loaded(first) && window.is_loaded(first + 39));
        }
        assert_eq!(engine.seeks(), 0);

        // Прыжки тоже не раздувают кэш.
        for first in [4_000, 12_000, 700, 19_960] {
            settle(&mut window, &engine, first, first + 39);
            assert!(window.cached_windows() <= 3, "на строке {first}");
            assert!(window.is_loaded(first));
        }
        assert_eq!(id_of(&window, 19_999), Some(20_000));
        assert_eq!(id_of(&window, 20_000), None);
    }

    #[test]
    fn last_window_is_short_and_ends_the_list() {
        let engine = FakeEngine::new(250);
        let mut window = ListWindow::new(query(), WindowConfig::default());

        settle(&mut window, &engine, 0, 29);
        settle(&mut window, &engine, 220, 249);
        // Прокрутка за конец списка ничего не просит.
        assert!(window.wanted(250, 290).is_empty());

        assert_eq!(id_of(&window, 249), Some(250));
        assert_eq!(id_of(&window, 250), None);
        assert_eq!(engine.seeks(), 1);
    }

    #[test]
    fn scrolling_far_past_the_end_asks_nothing() {
        let engine = FakeEngine::new(250);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        settle(&mut window, &engine, 0, 29);

        assert!(window.wanted(5_000, 5_030).is_empty());
        assert!(window.wanted(usize::MAX - 10, usize::MAX).is_empty());
        assert_eq!((engine.lists(), engine.seeks()), (1, 0));
    }

    #[test]
    fn empty_list_opens_without_windows() {
        let engine = FakeEngine::new(0);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        settle(&mut window, &engine, 0, 29);
        assert_eq!((window.total(), window.row_count()), (Some(0), 0));
        assert!(window.wanted(0, 29).is_empty());
        assert_eq!((engine.lists(), engine.seeks()), (1, 0));
    }

    #[test]
    fn engine_error_is_remembered_and_not_retried_by_itself() {
        let engine = FakeEngine::new(20_000);
        let mut window = ListWindow::new(query(), WindowConfig::default());
        settle(&mut window, &engine, 0, 29);

        engine.fail_seek.store(true, Ordering::SeqCst);
        settle(&mut window, &engine, 15_000, 15_030);
        assert_eq!(window.error().map(|e| e.code), Some(ErrorCode::Unavailable));
        assert!(!window.is_loaded(15_000));
        assert_eq!(window.row_count(), 20_000);
        assert_eq!(id_of(&window, 0), Some(1));

        // Сколько ни просить, упавшее окно само не повторяется.
        assert!(window.wanted(15_000, 15_030).is_empty());
        assert_eq!(engine.seeks(), 1);

        engine.fail_seek.store(false, Ordering::SeqCst);
        let again = window.retry();
        assert_eq!(again.len(), 1);
        assert!(window.error().is_none());
        let result = again[0].run(&*engine);
        window.accept(again[0].generation, again[0].window, result);
        assert_eq!(id_of(&window, 15_000), Some(15_001));
    }

    #[test]
    fn error_of_the_first_page_leaves_the_list_closed() {
        let engine = FakeEngine::new(20_000);
        engine.fail_list.store(true, Ordering::SeqCst);
        let mut window = ListWindow::new(query(), WindowConfig::default());

        settle(&mut window, &engine, 0, 29);
        assert!(window.error().is_some());
        assert_eq!((window.row_count(), window.cached_windows()), (0, 0));
        assert!(window.wanted(0, 29).is_empty());
        assert_eq!(engine.lists(), 1);

        engine.fail_list.store(false, Ordering::SeqCst);
        let again = window.retry();
        assert_eq!(again.len(), 1);
        let result = again[0].run(&*engine);
        assert_eq!(window.accept(0, 0, result), Accepted::Opened);
        assert_eq!(window.total(), Some(20_000));
    }

    #[test]
    fn config_is_clamped_to_sane_values() {
        let config = WindowConfig::new(0, 0);
        assert_eq!((config.size, config.max_windows), (1, MIN_WINDOWS));
        assert_eq!(WindowConfig::new(5_000, 10).size, ENGINE_MAX_SIZE);
        assert_eq!(WindowConfig::default(), WindowConfig::new(100, 8));
    }

    // ------------------------------------------------------------ модель

    type Queue = Rc<RefCell<VecDeque<(Job, Done)>>>;

    /// Фоновая работа не запускается сама, а встаёт в очередь: порядок ответов задаёт тест.
    fn manual_runner(queue: &Queue) -> Runner {
        let queue = Rc::clone(queue);
        Rc::new(move |job, done| queue.borrow_mut().push_back((job, done)))
    }

    fn run_next(queue: &Queue) -> bool {
        let next = queue.borrow_mut().pop_front();
        match next {
            Some((job, done)) => {
                done(job());
                true
            }
            None => false,
        }
    }

    fn run_all(queue: &Queue) {
        while run_next(queue) {}
    }

    fn model_over(engine: &Arc<FakeEngine>, queue: &Queue) -> Rc<ListModel> {
        ListModel::with_runner(
            Arc::clone(engine) as Arc<dyn Engine>,
            query(),
            WindowConfig::default(),
            manual_runner(queue),
        )
    }

    fn model_id(model: &ListModel, row: usize) -> Option<i64> {
        model.row_data(row)?.as_row().map(|row| row.id)
    }

    #[test]
    fn model_gives_placeholders_until_the_window_arrives() {
        let engine = FakeEngine::new(20_000);
        let queue = Queue::default();
        let model = model_over(&engine, &queue);

        // Пока список открывается, строк нет, первое окно уже в пути.
        assert_eq!((model.row_count(), queue.borrow().len()), (0, 1));
        run_all(&queue);
        assert_eq!((model.row_count(), model.total()), (20_000, Some(20_000)));
        assert_eq!(model_id(&model, 0), Some(1));
        assert_eq!(model.row_data(20_000), None);

        // Дальняя строка: заглушка, и ровно один запрос на всё окно.
        assert_eq!(model.row_data(15_000), Some(LazyRow::Placeholder));
        assert_eq!(model.row_data(15_001), Some(LazyRow::Placeholder));
        assert_eq!(queue.borrow().len(), 1);
        run_all(&queue);
        assert_eq!(model_id(&model, 15_000), Some(15_001));
        assert_eq!(*engine.seek_positions.lock().unwrap(), vec![15_000]);
    }

    #[test]
    fn model_prefetches_the_next_window_in_the_second_half() {
        let engine = FakeEngine::new(20_000);
        let queue = Queue::default();
        let model = model_over(&engine, &queue);
        run_all(&queue);

        assert_eq!(model_id(&model, 10), Some(11));
        assert!(queue.borrow().is_empty());
        assert_eq!(model_id(&model, 60), Some(61));
        run_all(&queue);
        assert_eq!(engine.lists(), 2);
        assert_eq!(model_id(&model, 150), Some(151));
        assert_eq!(engine.seeks(), 0);
    }

    #[test]
    fn model_drops_answers_of_the_old_query() {
        let engine = FakeEngine::new(20_000);
        let queue = Queue::default();
        let model = model_over(&engine, &queue);
        run_all(&queue);
        assert_eq!(model.row_data(15_000), Some(LazyRow::Placeholder));

        // Сортировку сменили, пока seek ещё в пути: его ответ придёт уже никому не нужным.
        model.set_query(ListQuery {
            descending: true,
            ..query()
        });
        assert_eq!(model.row_count(), 0);
        assert_eq!(queue.borrow().len(), 2);
        run_all(&queue);

        assert_eq!(model.row_count(), 20_000);
        assert_eq!(model.row_data(15_000), Some(LazyRow::Placeholder));
        assert_eq!(model_id(&model, 0), Some(1));
    }

    #[test]
    fn model_survives_engine_errors_and_can_retry() {
        let engine = FakeEngine::new(20_000);
        let queue = Queue::default();
        let model = model_over(&engine, &queue);
        run_all(&queue);

        engine.fail_seek.store(true, Ordering::SeqCst);
        assert_eq!(model.row_data(15_000), Some(LazyRow::Placeholder));
        run_all(&queue);

        assert_eq!(model.error().map(|e| e.code), Some(ErrorCode::Unavailable));
        assert_eq!(model.row_count(), 20_000);
        assert_eq!(model_id(&model, 0), Some(1));
        assert_eq!(model.row_data(15_000), Some(LazyRow::Placeholder));
        assert!(queue.borrow().is_empty());

        engine.fail_seek.store(false, Ordering::SeqCst);
        model.retry();
        run_all(&queue);
        assert!(model.error().is_none());
        assert_eq!(model_id(&model, 15_000), Some(15_001));
    }

    #[test]
    fn dropped_model_ignores_a_late_answer() {
        let engine = FakeEngine::new(20_000);
        let queue = Queue::default();
        drop(model_over(&engine, &queue));
        run_all(&queue);
        assert_eq!(engine.lists(), 1);
    }

    // ------------------------------------------------------------ настоящее ядро

    /// Модель над настоящим ядром: прыжок даёт те же строки, что прямой `seek` ядра, при
    /// сортировке по id и по номеру в обратном порядке (токен несёт сортировку в `seek`).
    fn check_jump_over_the_real_core(name: &str, receipts: usize, jump: usize) {
        let path = std::env::temp_dir().join(format!(
            "ostov-ui-list-window-{name}-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let engine = ostov_core::LocalConnector::default()
            .create_demo(path.to_str().unwrap_or_default(), receipts)
            .unwrap();

        for (order, descending) in [(None, false), (Some("number"), true)] {
            let query = ListQuery {
                object: "receipt".to_owned(),
                order: order.map(str::to_owned),
                descending,
                ..ListQuery::default()
            };
            let queue = Queue::default();
            let model = ListModel::with_runner(
                Arc::clone(&engine),
                query.clone(),
                WindowConfig::default(),
                manual_runner(&queue),
            );
            run_all(&queue);
            assert_eq!(model.total(), Some(u64::try_from(receipts).unwrap()));
            assert_eq!(model.row_count(), receipts);

            // Прямой ответ ядра на те же окна: первая страница даёт токен.
            let first = engine
                .list(&ListQuery {
                    size: 1,
                    ..query.clone()
                })
                .unwrap();
            let direct = engine
                .seek(&first.token, u64::try_from(jump).unwrap(), 100)
                .unwrap();
            assert_eq!(direct.rows.len(), 100);

            assert_eq!(model.row_data(jump), Some(LazyRow::Placeholder));
            run_all(&queue);
            for (offset, expected) in direct.rows.iter().enumerate() {
                let row = model.row_data(jump + offset).unwrap();
                assert_eq!(row.as_row(), Some(expected), "строка {}", jump + offset);
            }
            // Окно за прыжком догружается по ключу и продолжает тот же порядок.
            run_all(&queue);
            let next = engine
                .seek(&first.token, u64::try_from(jump + 100).unwrap(), 100)
                .unwrap();
            let after = model.row_data(jump + 100).unwrap();
            assert_eq!(after.as_row(), next.rows.first());
            assert!(model.row_data(0).unwrap().as_row().is_some());
            assert!(model.error().is_none());
        }

        drop(engine);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite.lock"));
    }

    #[test]
    fn jump_over_the_real_core_matches_direct_seek() {
        // Больше шага вех ядра (1000): окно берётся от вехи с пропуском, а не от начала.
        check_jump_over_the_real_core("small", 1_500, 1_200);
    }

    /// Целевой размер списка. Демо-база на 20 000 документов в debug собирается минуты,
    /// поэтому по умолчанию пропущен: `cargo test -p ostov-ui -- --ignored list_window`.
    #[test]
    #[ignore = "создание 20 000 демо-документов в debug занимает минуты"]
    fn jump_over_the_real_core_with_20000_rows() {
        check_jump_over_the_real_core("big", 20_000, 15_000);
    }
}
