//! Экран «Поступления»: список и документы в своих вкладках, связь с Slint через `ReceiptsData`.
//!
//! Каждый открытый документ хранит своё состояние (`DocState`), поэтому в нескольких вкладках
//! можно править разные документы. Поля формы лежат в глобальных свойствах Slint только для
//! вкладки на экране; при переключении они сохраняются в `DocState` и загружаются из него.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use ostov_api::{Engine, EngineResult, ListQuery, Row, Value};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::engine::call_in_background;
use crate::grid;
use crate::receipts::{self, COLUMNS, Line, Receipt};
use crate::shell::{RECEIPTS_NODE, Shell};
use crate::{AppWindow, GridColumn, GridMark, GridRow, MenuAction, ReceiptsData};

/// Ширины колонок списка в px: гибкая только «Поставщик», её ширина здесь минимум.
/// Дальше ширины меняет пользователь перетаскиванием (это делает сама таблица).
const DEFAULT_WIDTHS: [f32; 6] = [64.0, 104.0, 120.0, 120.0, 130.0, 108.0];
const SUPPLIER: usize = 2;
const AMOUNT: usize = 4;
const STATUS: usize = 5;
const NEW_DOC_DATE: (u16, u8, u8) = (2026, 10, 29);

/// Объект списка в ядре и поля сортировки по колонкам (порядок как у `receipts::COLUMNS`).
/// Ссылочные колонки ядро пока сортирует по id, а не по названию.
const OBJECT: &str = "receipt";
const COLUMN_FIELDS: [&str; 6] = [
    "number",
    "date",
    "supplier",
    "warehouse",
    "amount",
    "posted",
];
/// Строк в одном запросе к ядру (оно отдаёт не больше 1000).
const PAGE_SIZE: u32 = 500;
/// Сколько строк грузим целиком, пока нет подгрузки окнами по прокрутке и прыжков `seek`.
const MAX_ROWS: usize = 20_000;

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
    /// Ядро, из которого читается список (пока не подключено: список пуст).
    engine: Option<Arc<dyn Engine>>,
    /// Номер запроса списка: ответ на устаревший запрос (успели сменить сортировку или поиск)
    /// отбрасывается.
    generation: u64,
    /// Строк в списке без поиска (для подписи «Показано N из M»).
    total_all: usize,
    next_number: u32,
    docs: HashMap<u32, DocState>,
    /// Состояние формы окна, которому сейчас служит обработчик (см. `form_state`).
    form: FormCtx,
    /// Документы, вынесенные из вкладок в отдельные окна.
    detached: HashMap<u32, Detached>,
    /// Номера документов со своей вехой пользователя (в списке они не зависят от сортировки).
    marks: HashSet<u32>,
    /// Документ, удаление которого ждёт подтверждения.
    pending_delete: Option<u32>,
    warehouses: Vec<String>,
    suppliers: Vec<String>,
    /// Номенклатура: название и единица измерения.
    goods: Vec<(String, String)>,
}

/// Что помнит форма документа между действиями: открытый в ней документ, правимая строка,
/// справочник, в который добавляют элемент. У каждого окна формы своё.
#[derive(Default)]
struct FormCtx {
    /// Документ, поля которого сейчас в форме.
    loaded: Option<u32>,
    /// Какая строка правится в окне строки (`None`: добавляется новая).
    editing_line: Option<usize>,
    /// Справочник, в который добавляют элемент (кнопка «+ Добавить» в выпадающем списке).
    catalog_kind: Option<Catalog>,
}

/// Окно формы: вкладка главного окна или отдельное окно документа.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Main,
    Doc(u32),
}

/// Документ в отдельном окне: окно (второй `AppWindow`, страница 3) и состояние его формы.
struct Detached {
    window: AppWindow,
    form: FormCtx,
}

/// Доступ к состоянию с формой нужного окна: у отдельного окна контекст формы подменяется на
/// его собственный и возвращается назад, когда охрана освобождается (в том числе при раннем
/// `return` в обработчике). Так одни и те же функции работают с любой формой.
struct FormGuard<'a> {
    state: std::cell::RefMut<'a, State>,
    /// Номер документа окна и контекст главной формы, пока тот подменён.
    swapped: Option<(u32, FormCtx)>,
}

impl std::ops::Deref for FormGuard<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        &self.state
    }
}

impl std::ops::DerefMut for FormGuard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.state
    }
}

impl Drop for FormGuard<'_> {
    fn drop(&mut self) {
        if let Some((number, main)) = self.swapped.take() {
            let doc_form = std::mem::replace(&mut self.state.form, main);
            // Окно могли закрыть внутри обработчика: тогда его контекст некуда возвращать.
            if let Some(detached) = self.state.detached.get_mut(&number) {
                detached.form = doc_form;
            }
        }
    }
}

fn form_state(state: &RefCell<State>, target: Target) -> FormGuard<'_> {
    let mut guard = state.borrow_mut();
    let swapped = match target {
        Target::Main => None,
        Target::Doc(number) => {
            let doc_form = guard
                .detached
                .get_mut(&number)
                .map(|d| std::mem::take(&mut d.form))
                .unwrap_or_default();
            Some((number, std::mem::replace(&mut guard.form, doc_form)))
        }
    };
    FormGuard {
        state: guard,
        swapped,
    }
}

/// Справочники, на которые ссылаются поля формы.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Catalog {
    Warehouse,
    Supplier,
    Goods,
}

impl Catalog {
    fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Warehouse),
            1 => Some(Self::Supplier),
            2 => Some(Self::Goods),
            _ => None,
        }
    }

    fn dialog_title(self) -> &'static str {
        match self {
            Self::Warehouse => "Новый склад",
            Self::Supplier => "Новый поставщик",
            Self::Goods => "Новая номенклатура",
        }
    }
}

impl State {
    /// Показанные строки: порядок задало ядро, поиск делаем здесь по загруженным строкам
    /// (поиск ядра работает только по текстовым полям самого объекта, а у поступления их нет).
    fn visible(&self) -> Vec<&Receipt> {
        receipts::filter(&self.all, &self.query)
    }

    /// Запрос списка по текущей сортировке.
    fn request(&self) -> ListRequest {
        ListRequest {
            order: self
                .sort
                .map(|(column, _)| COLUMN_FIELDS[column].to_owned()),
            descending: self.sort.is_some_and(|(_, descending)| descending),
        }
    }
}

/// Параметры запроса списка (то, что уходит в `ListQuery`).
#[derive(Clone, Default)]
struct ListRequest {
    order: Option<String>,
    descending: bool,
}

/// Число из десятичного значения ядра в копейках (деньги в ядре: 2 знака; другое
/// приводится, лишние знаки отбрасываются).
fn kopecks(units: i64, scale: u8) -> i64 {
    let factor = |digits: u8| 10i64.checked_pow(u32::from(digits)).unwrap_or(i64::MAX);
    match scale.cmp(&2) {
        std::cmp::Ordering::Equal => units,
        std::cmp::Ordering::Less => units.saturating_mul(factor(2 - scale)),
        std::cmp::Ordering::Greater => units / factor(scale - 2),
    }
}

/// Дата ядра `ГГГГ-ММ-ДД` в (год, месяц, день).
fn parse_iso_date(text: &str) -> Option<(u16, u8, u8)> {
    let mut parts = text.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((year, month, day))
}

/// Строка списка ядра в строку списка интерфейса; без номера строка не показывается.
fn receipt_from_row(row: &Row) -> Option<Receipt> {
    let int = |name: &str| match row.values.get(name) {
        Some(Value::Int(value)) => Some(*value),
        _ => None,
    };
    let title = |name: &str| match row.values.get(name) {
        Some(Value::Ref {
            title: Some(title), ..
        }) => title.clone(),
        Some(Value::Ref { id, .. }) => id.to_string(),
        _ => String::new(),
    };
    Some(Receipt {
        id: row.id,
        number: u32::try_from(int("number")?).ok()?,
        date: match row.values.get("date") {
            Some(Value::Date(text)) => parse_iso_date(text).unwrap_or_default(),
            _ => (0, 0, 0),
        },
        supplier: title("supplier"),
        warehouse: title("warehouse"),
        amount: match row.values.get("amount") {
            Some(Value::Decimal { units, scale }) => kopecks(*units, *scale),
            _ => 0,
        },
        posted: int("posted").unwrap_or(0) != 0,
        deleted: int("deleted").unwrap_or(0) != 0,
        lines: None,
    })
}

/// Читает список из ядра страницами по `PAGE_SIZE`; возвращает строки и общее число.
fn fetch_receipts(engine: &dyn Engine, request: &ListRequest) -> EngineResult<(Vec<Receipt>, u64)> {
    let mut receipts = Vec::new();
    let mut total = 0;
    let mut after_key = None;
    loop {
        let page = engine.list(&ListQuery {
            object: OBJECT.to_owned(),
            order: request.order.clone(),
            descending: request.descending,
            search: None,
            after_key: after_key.take(),
            size: PAGE_SIZE,
        })?;
        if let Some(count) = page.total {
            total = count;
        }
        receipts.extend(page.rows.iter().filter_map(receipt_from_row));
        match page.next_key {
            Some(key) if receipts.len() < MAX_ROWS => after_key = Some(key),
            _ => break,
        }
    }
    Ok((receipts, total))
}

/// Точка входа для оболочки: перед и после любого действия с вкладками.
#[derive(Clone)]
pub struct ReceiptsUi {
    state: Rc<RefCell<State>>,
    shell: Rc<RefCell<Shell>>,
}

impl ReceiptsUi {
    /// Вызывать ПЕРЕД действием с вкладками: сохраняет поля формы и закрепляет изменённую вкладку,
    /// чтобы следующее открытие не заменило её.
    pub fn before(&self, app: &AppWindow, shell: &mut Shell) {
        before(app, &mut self.state.borrow_mut(), shell);
    }

    /// Выносит документ вкладки `tab` в отдельное окно (вкладки без документа не трогает).
    pub fn detach(&self, app: &AppWindow, shell: &mut Shell, tab: usize) {
        detach(app, &self.state, &self.shell, shell, tab);
        crate::app::refresh(app, shell);
    }

    /// Вызывает `f` для каждого отдельного окна документа (например, применить настройки).
    pub fn each_detached(&self, f: impl Fn(&AppWindow)) {
        for detached in self.state.borrow().detached.values() {
            f(&detached.window);
        }
    }

    /// Отдельное окно документа (снимки экранов).
    pub fn detached_window(&self, number: u32) -> Option<AppWindow> {
        let state = self.state.borrow();
        state.detached.get(&number).map(|d| d.window.clone_strong())
    }

    /// Подключено ли ядро.
    pub fn is_attached(&self) -> bool {
        self.state.borrow().engine.is_some()
    }

    /// Показывает ошибку подключения вместо списка.
    pub fn show_error(&self, app: &AppWindow, message: &str) {
        let data = app.global::<ReceiptsData>();
        data.set_list_loading(false);
        data.set_list_error(message.into());
    }

    /// Подключает ядро и читает список в фоне.
    pub fn attach(&self, app: &AppWindow, engine: Arc<dyn Engine>) {
        self.state.borrow_mut().engine = Some(engine);
        reload(app, &self.state, false);
    }

    /// То же, но ждёт ответа на месте (снимки экранов и проверки).
    pub fn attach_blocking(&self, app: &AppWindow, engine: Arc<dyn Engine>) {
        self.state.borrow_mut().engine = Some(engine);
        reload(app, &self.state, true);
    }

    /// Вызывать ПОСЛЕ действия с вкладками: забывает закрытые документы и загружает форму
    /// документа, чья вкладка стала активной.
    pub fn after(&self, app: &AppWindow, shell: &Shell) {
        after(app, &mut self.state.borrow_mut(), shell);
    }
}

fn strings(items: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.iter().map(SharedString::from).collect::<Vec<_>>(),
    ))
}

/// Пункт меню: по умолчанию доступен, без черты и без красного.
fn item(id: &str, label: &str, icon: &str) -> MenuAction {
    MenuAction {
        id: id.into(),
        label: label.into(),
        icon: icon.into(),
        danger: false,
        enabled: true,
        divider: false,
    }
}

/// Меню строки списка: что доступно, зависит от состояния документа.
fn list_menu(receipt: &Receipt, milestone: bool) -> Vec<MenuAction> {
    let (post_id, post_label, post_icon) = if receipt.posted {
        ("unpost", "Отменить проведение", "undo")
    } else {
        ("post", "Провести", "check")
    };
    vec![
        item("edit", "Редактировать", "edit"),
        MenuAction {
            enabled: !receipt.deleted,
            ..item(post_id, post_label, post_icon)
        },
        MenuAction {
            divider: true,
            ..item(
                "mark",
                if receipt.deleted {
                    "Снять пометку удаления"
                } else {
                    "Пометить на удаление"
                },
                "ban",
            )
        },
        // Проведённый документ сначала проводят обратно: удалить его напрямую нельзя.
        MenuAction {
            danger: true,
            enabled: !receipt.posted,
            ..item("delete", "Удалить…", "trash")
        },
        MenuAction {
            divider: true,
            ..item(
                "milestone",
                if milestone {
                    "Убрать веху"
                } else {
                    "Добавить веху"
                },
                "minus",
            )
        },
    ]
}

/// Меню строки табличной части: правка, вставка копии, порядок, удаление.
fn line_menu(index: usize, count: usize) -> Vec<MenuAction> {
    vec![
        item("edit", "Изменить", "edit"),
        item("add", "Добавить", "plus"),
        item("copy", "Дублировать", "copy"),
        MenuAction {
            divider: true,
            enabled: index > 0,
            ..item("up", "Переместить выше", "arrow-up")
        },
        MenuAction {
            enabled: index + 1 < count,
            ..item("down", "Переместить ниже", "arrow-down")
        },
        MenuAction {
            divider: true,
            danger: true,
            ..item("delete", "Удалить", "trash")
        },
    ]
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
        milestone: false,
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

/// Колонки на момент нажатия на ручку: от них считается каждое движение, а не от прошлого.
type Snapshot = Rc<RefCell<Option<Vec<GridColumn>>>>;

fn snapshot_of(model: &ModelRc<GridColumn>) -> Vec<GridColumn> {
    use slint::Model;
    model.iter().collect()
}

/// Пишет колонки в ту же модель на месте: заголовки и строки остаются теми же элементами,
/// поэтому перетаскивание не прерывается (подмена модели целиком пересоздала бы заголовки).
fn write_columns(model: &ModelRc<GridColumn>, columns: Vec<GridColumn>) {
    use slint::Model;
    for (i, column) in columns.into_iter().enumerate() {
        model.set_row_data(i, column);
    }
}

/// Пересчитывает колонки от снимка: `delta` смещение от точки нажатия, `table_width` ширина
/// области колонок.
fn resized_columns(
    start: &[GridColumn],
    index: i32,
    delta: f32,
    table_width: f32,
) -> Vec<GridColumn> {
    let mut columns: Vec<GridColumn> = start.to_vec();
    let Some(index) = usize::try_from(index).ok().filter(|i| *i < columns.len()) else {
        return columns;
    };
    // Фактическая ширина гибкой колонки: остаток места после обычных колонок.
    let flex_width = |columns: &[GridColumn]| {
        let fixed: f32 = columns.iter().filter(|c| !c.flex).map(|c| c.width).sum();
        (table_width - fixed).max(0.0)
    };
    // Границу гибкой колонки тянут: колонка получает собственную ширину (ту, что видна на
    // экране) и дальше меняется сама, а гибкой становится последняя колонка.
    if columns[index].flex {
        let actual = flex_width(&columns).max(columns[index].width);
        columns[index].width = actual;
        columns[index].flex = false;
        if let Some(last) = columns.last_mut() {
            last.flex = true;
        }
    }
    let flex = columns.iter().position(|c| c.flex);
    let mut widths: Vec<f32> = columns.iter().map(|c| c.width).collect();
    let flex_actual = flex.map_or(0.0, |f| flex_width(&columns).max(widths[f]));
    grid::resize(&mut widths, flex, flex_actual, index, delta);
    for (column, width) in columns.iter_mut().zip(widths) {
        column.width = width;
    }
    columns
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

/// Вехи показанного списка: номер строки (с нуля) и поставщик как наименование.
fn mark_positions(visible: &[&Receipt], marks: &HashSet<u32>) -> Vec<GridMark> {
    visible
        .iter()
        .enumerate()
        .filter(|(_, receipt)| marks.contains(&receipt.number))
        .filter_map(|(i, receipt)| {
            Some(GridMark {
                row: i32::try_from(i).ok()?,
                label: receipt.supplier.as_str().into(),
            })
        })
        .collect()
}

/// Перечитывает список из ядра по текущей сортировке и поиску. Устаревший ответ (параметры
/// успели сменить) отбрасывается. `blocking`: ждать ответа на месте.
fn reload(app: &AppWindow, state: &Rc<RefCell<State>>, blocking: bool) {
    let (engine, request, generation) = {
        let mut state = state.borrow_mut();
        let Some(engine) = state.engine.clone() else {
            return;
        };
        state.generation += 1;
        (engine, state.request(), state.generation)
    };
    app.global::<ReceiptsData>().set_list_loading(true);
    let apply = {
        let (app, state) = (app.as_weak(), state.clone());
        move |result: EngineResult<(Vec<Receipt>, u64)>| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            if state.generation == generation {
                apply_list(&app, &mut state, result);
            }
        }
    };
    if blocking {
        apply(fetch_receipts(&*engine, &request));
    } else {
        call_in_background(move || fetch_receipts(&*engine, &request), apply);
    }
}

/// Кладёт ответ ядра в список или показывает ошибку.
fn apply_list(app: &AppWindow, state: &mut State, result: EngineResult<(Vec<Receipt>, u64)>) {
    let data = app.global::<ReceiptsData>();
    data.set_list_loading(false);
    match result {
        Ok((rows, total)) => {
            state.total_all = usize::try_from(total).unwrap_or(usize::MAX);
            let next = rows.iter().map(|r| r.number).max().map_or(1, |n| n + 1);
            state.next_number = state.next_number.max(next);
            state.all = rows;
            data.set_list_error("".into());
        }
        Err(error) => {
            state.all.clear();
            data.set_list_error(error.message.as_str().into());
        }
    }
    data.set_selected_row(-1);
    refresh_list(app, state);
}

fn refresh_list(app: &AppWindow, state: &State) {
    let data = app.global::<ReceiptsData>();
    let visible = state.visible();
    let rows: Vec<GridRow> = visible
        .iter()
        .map(|receipt| GridRow {
            milestone: state.marks.contains(&receipt.number),
            ..grid_row(receipt.cells(), receipt.deleted)
        })
        .collect();
    let marks = mark_positions(&visible, &state.marks);
    data.set_marks(ModelRc::new(VecModel::from(marks)));
    data.set_milestone_step(milestone_step(rows.len()));
    data.set_rows(ModelRc::new(VecModel::from(rows)));
    data.set_total(i32::try_from(state.total_all).unwrap_or(i32::MAX));
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

/// Дата из полей формы (день, месяц, год).
fn form_date(data: &ReceiptsData<'_>) -> String {
    let part = |value: i32| u16::try_from(value).unwrap_or(1);
    receipts::date_text((
        part(data.get_form_year()),
        u8::try_from(data.get_form_month()).unwrap_or(1),
        u8::try_from(data.get_form_day()).unwrap_or(1),
    ))
}

/// Сохраняет поля формы на экране в состояние её документа.
fn store_form(app: &AppWindow, state: &mut State) {
    let Some(number) = state.form.loaded else {
        return;
    };
    let data = app.global::<ReceiptsData>();
    if let Some(doc) = state.docs.get_mut(&number) {
        doc.content.supplier = data.get_form_supplier().to_string();
        doc.content.warehouse = data.get_form_warehouse().to_string();
        doc.content.date = form_date(&data);
    }
}

/// Показывает документ в форме окна `app` (поля, строки, заголовок окна).
fn fill_form(app: &AppWindow, doc: &DocState) {
    let data = app.global::<ReceiptsData>();
    data.set_form_title(doc.title().into());
    data.set_form_posted(doc.posted);
    data.set_form_number(doc.number.to_string().into());
    let (year, month, day) = receipts::parse_date(&doc.content.date).unwrap_or(NEW_DOC_DATE);
    data.set_form_year(i32::from(year));
    data.set_form_month(i32::from(month));
    data.set_form_day(i32::from(day));
    data.set_form_warehouse(doc.content.warehouse.as_str().into());
    data.set_form_supplier(doc.content.supplier.as_str().into());
    data.set_line_columns(ModelRc::new(VecModel::from(line_columns())));
    data.set_line_selected(-1);
    data.set_line_open(false);
    app.set_doc_title(doc.title().into());
    render_lines(app, doc);
}

/// Загружает документ в форму на экране.
fn load_doc(app: &AppWindow, state: &mut State, number: u32) {
    let Some(doc) = state.docs.get(&number) else {
        return;
    };
    fill_form(app, doc);
    state.form.loaded = Some(number);
}

/// Значения выпадающих списков (справочники) в окне `app`.
fn publish_catalogs(app: &AppWindow, state: &State) {
    let data = app.global::<ReceiptsData>();
    data.set_warehouses(strings(&state.warehouses));
    data.set_suppliers(strings(&state.suppliers));
    let goods: Vec<String> = state.goods.iter().map(|(name, _)| name.clone()).collect();
    data.set_goods(strings(&goods));
}

/// То же во всех окнах: главном и отдельных.
fn publish_all(main: &AppWindow, state: &State) {
    publish_catalogs(main, state);
    for detached in state.detached.values() {
        publish_catalogs(&detached.window, state);
    }
}

/// Убирает окно: прячет (если оно ещё на экране) и освобождает чуть позже. Освобождать сразу
/// нельзя: обработчик, из которого вызвали, может быть событием этого же окна.
fn retire(window: AppWindow, hide: bool) {
    if hide {
        let _ = window.hide();
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(100), move || drop(window));
}

fn before(app: &AppWindow, state: &mut State, shell: &mut Shell) {
    store_form(app, state);
    if let Some(number) = state.form.loaded {
        let modified = state.docs.get(&number).is_some_and(DocState::is_modified);
        if let Some(tab) = shell.doc_tab(number).filter(|_| modified) {
            shell.set_modified(tab, true);
        }
    }
}

fn after(app: &AppWindow, state: &mut State, shell: &Shell) {
    let State { docs, detached, .. } = &mut *state;
    docs.retain(|number, _| shell.doc_tab(*number).is_some() || detached.contains_key(number));
    match shell.active_doc().filter(|n| state.docs.contains_key(n)) {
        Some(number) if state.form.loaded != Some(number) => load_doc(app, state, number),
        Some(_) => {}
        None => state.form.loaded = None,
    }
}

/// Обновляет одну строку списка на месте (подписи, статус, приглушение, веху).
fn update_row(app: &AppWindow, state: &State, index: usize) {
    use slint::Model;
    let data = app.global::<ReceiptsData>();
    let visible = state.visible();
    let Some(receipt) = visible.get(index) else {
        return;
    };
    let row = GridRow {
        milestone: state.marks.contains(&receipt.number),
        ..grid_row(receipt.cells(), receipt.deleted)
    };
    data.get_rows().set_row_data(index, row);
}

/// Проводит документ из списка или отменяет проведение; открытый документ получает тот же статус.
fn set_posted(app: &AppWindow, state: &mut State, number: u32, posted: bool) {
    if let Some(receipt) = state.all.iter_mut().find(|r| r.number == number) {
        receipt.posted = posted;
    }
    if let Some(doc) = state.docs.get_mut(&number) {
        doc.posted = posted;
    }
    if state.form.loaded == Some(number) {
        app.global::<ReceiptsData>().set_form_posted(posted);
    }
    if let Some(detached) = state.detached.get(&number) {
        detached
            .window
            .global::<ReceiptsData>()
            .set_form_posted(posted);
    }
}

/// Открывает документ в своей вкладке (создаёт его состояние, если вкладки ещё нет).
fn open_doc(app: &AppWindow, state: &mut State, shell: &mut Shell, number: u32) {
    // Документ уже в своём окне: показываем его, вкладку не открываем.
    if let Some(detached) = state.detached.get(&number) {
        let _ = detached.window.show();
        return;
    }
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
    let Some(number) = state.form.loaded else {
        return;
    };
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
                id: 0,
                number,
                date: date.unwrap_or(NEW_DOC_DATE),
                supplier: doc.content.supplier.clone(),
                warehouse: doc.content.warehouse.clone(),
                amount,
                posted: doc.posted,
                deleted: false,
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

/// Закрывает документ в отдельном окне (как закрытие вкладки). `hide`: окно ещё на экране
/// (закрытие крестиком прячет его само).
fn close_detached(main: &AppWindow, state: &mut State, shell: &Shell, number: u32, hide: bool) {
    if let Some(detached) = state.detached.remove(&number) {
        retire(detached.window, hide);
    }
    after(main, state, shell);
}

/// Возвращает документ из отдельного окна в закреплённую вкладку главного окна.
fn dock(main: &AppWindow, state: &mut State, shell: &mut Shell, number: u32) {
    if let Some(detached) = state.detached.remove(&number) {
        retire(detached.window, true);
    }
    let (title, modified) = state
        .docs
        .get(&number)
        .map(|doc| (doc.title(), doc.is_modified()))
        .unwrap_or_default();
    shell.open_doc(RECEIPTS_NODE, number, title, "receipt");
    if let Some(tab) = shell.doc_tab(number) {
        shell.pin_tab(tab);
        shell.set_modified(tab, modified);
    }
    after(main, state, shell);
}

/// Выносит документ вкладки `tab` в отдельное окно. Окно это второй `AppWindow` на странице 3
/// (только форма) со своим `ReceiptsData`; состояние документов общее.
fn detach(
    main: &AppWindow,
    state_rc: &Rc<RefCell<State>>,
    shell_rc: &Rc<RefCell<Shell>>,
    shell: &mut Shell,
    tab: usize,
) {
    let Some(number) = shell.tabs().get(tab).and_then(|t| t.doc) else {
        return;
    };
    let mut state = state_rc.borrow_mut();
    // Поля формы главного окна сначала попадают в документ.
    before(main, &mut state, shell);
    if !state.docs.contains_key(&number) || state.detached.contains_key(&number) {
        return;
    }
    let Ok(window) = AppWindow::new() else { return };
    crate::app::prepare_doc_window(&window);
    window.global::<ReceiptsData>().set_detached(true);
    publish_catalogs(&window, &state);
    if let Some(doc) = state.docs.get(&number) {
        fill_form(&window, doc);
    }
    wire_form(
        &window,
        main.as_weak(),
        state_rc,
        shell_rc,
        Target::Doc(number),
    );
    // Закрытие окна крестиком закрывает документ, как закрытие вкладки.
    window.window().on_close_requested({
        let (main, state, shell) = (main.as_weak(), state_rc.clone(), shell_rc.clone());
        move || {
            if let Some(main) = main.upgrade() {
                close_detached(
                    &main,
                    &mut state.borrow_mut(),
                    &shell.borrow(),
                    number,
                    false,
                );
            }
            slint::CloseRequestResponse::HideWindow
        }
    });
    let _ = window.show();
    state.detached.insert(
        number,
        Detached {
            window,
            form: FormCtx {
                loaded: Some(number),
                ..FormCtx::default()
            },
        },
    );
    shell.close_tab(tab);
    after(main, &mut state, shell);
}

/// Меняет состав табличной части открытого документа. `edit` возвращает `None`, если менять
/// нечего, иначе `Some(строка для выделения)`. После изменения вкладка закрепляется сразу.
fn change_lines(
    app: &AppWindow,
    state: &mut State,
    shell: &mut Shell,
    edit: impl FnOnce(&mut Vec<Line>) -> Option<Option<usize>>,
) {
    let data = app.global::<ReceiptsData>();
    let Some(doc) = state.form.loaded.and_then(|n| state.docs.get_mut(&n)) else {
        return;
    };
    let Some(select) = edit(&mut doc.content.lines) else {
        return;
    };
    render_lines(app, doc);
    data.set_line_selected(select.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1));
    before(app, state, shell);
    crate::app::refresh(app, shell);
}

/// Открывает окно строки: пустое для новой или с данными для правки.
fn open_line_dialog(app: &AppWindow, state: &mut State, index: Option<usize>) {
    let data = app.global::<ReceiptsData>();
    let line = state
        .form
        .loaded
        .and_then(|n| state.docs.get(&n))
        .zip(index)
        .and_then(|(doc, i)| doc.content.lines.get(i).cloned());
    state.form.editing_line = index.filter(|_| line.is_some());
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
    let state = Rc::new(RefCell::new(State {
        all: Vec::new(),
        sort: None,
        query: String::new(),
        engine: None,
        generation: 0,
        total_all: 0,
        next_number: 1,
        docs: HashMap::new(),
        form: FormCtx::default(),
        detached: HashMap::new(),
        marks: HashSet::new(),
        pending_delete: None,
        warehouses: receipts::WAREHOUSES
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        suppliers: receipts::SUPPLIERS
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        goods: receipts::GOODS
            .iter()
            .map(|(name, unit)| ((*name).to_owned(), (*unit).to_owned()))
            .collect(),
    }));
    let data = app.global::<ReceiptsData>();
    data.set_columns(ModelRc::new(VecModel::from(list_columns())));
    publish_catalogs(app, &state.borrow());
    refresh_list(app, &state.borrow());

    data.on_sort({
        let (app, state) = (app.as_weak(), state.clone());
        move |column| {
            let Some(app) = app.upgrade() else { return };
            let Some(column) = usize::try_from(column).ok().filter(|c| *c < COLUMNS.len()) else {
                return;
            };
            {
                let mut state = state.borrow_mut();
                let descending = matches!(state.sort, Some((current, false)) if current == column);
                state.sort = Some((column, descending));
            }
            // Сортирует ядро: значок в шапке обновится вместе с ответом.
            reload(&app, &state, false);
        }
    });

    // Ширины колонок меняются на каждом движении ручки (живое перетаскивание).
    let list_snapshot: Snapshot = Rc::default();

    data.on_resize_begin_list({
        let (app, snapshot) = (app.as_weak(), list_snapshot.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            *snapshot.borrow_mut() = Some(snapshot_of(&app.global::<ReceiptsData>().get_columns()));
        }
    });
    data.on_resize_list({
        let (app, snapshot) = (app.as_weak(), list_snapshot.clone());
        move |index, delta, table_width| {
            let Some(app) = app.upgrade() else { return };
            let model = app.global::<ReceiptsData>().get_columns();
            if let Some(start) = snapshot.borrow().as_ref() {
                write_columns(&model, resized_columns(start, index, delta, table_width));
            }
        }
    });
    data.on_resize_end_list({
        let snapshot = list_snapshot;
        move || *snapshot.borrow_mut() = None
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

    // Меню строки списка: пункты зависят от состояния документа.
    data.on_menu_opening({
        let (app, state) = (app.as_weak(), state.clone());
        move |row| {
            let Some(app) = app.upgrade() else { return };
            let state = state.borrow();
            let actions = usize::try_from(row)
                .ok()
                .and_then(|i| state.visible().get(i).copied())
                .map(|receipt| list_menu(receipt, state.marks.contains(&receipt.number)))
                .unwrap_or_default();
            app.global::<ReceiptsData>()
                .set_menu_actions(ModelRc::new(VecModel::from(actions)));
        }
    });

    // Действия меню строки. Строка меняется на месте: прокрутка и выбор остаются как были.
    data.on_action({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move |row, id| {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            let Some(index) = usize::try_from(row).ok() else {
                return;
            };
            let Some(number) = state.visible().get(index).map(|r| r.number) else {
                return;
            };
            match id.as_str() {
                "edit" => open_doc(&app, &mut state, &mut shell.borrow_mut(), number),
                "post" | "unpost" => set_posted(&app, &mut state, number, id == "post"),
                "mark" => {
                    if let Some(receipt) = state.all.iter_mut().find(|r| r.number == number) {
                        receipt.deleted = !receipt.deleted;
                    }
                }
                "delete" => {
                    state.pending_delete = Some(number);
                    let data = app.global::<ReceiptsData>();
                    data.set_confirm_text(
                        format!("Удалить поступление № {number} без возможности восстановления?")
                            .into(),
                    );
                    data.set_confirm_open(true);
                    return;
                }
                "milestone" => {
                    if !state.marks.remove(&number) {
                        state.marks.insert(number);
                    }
                    let marks = mark_positions(&state.visible(), &state.marks);
                    app.global::<ReceiptsData>()
                        .set_marks(ModelRc::new(VecModel::from(marks)));
                }
                _ => return,
            }
            update_row(&app, &state, index);
        }
    });

    data.on_confirm_accepted({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let mut state = state.borrow_mut();
            app.global::<ReceiptsData>().set_confirm_open(false);
            let Some(number) = state.pending_delete.take() else {
                return;
            };
            state.all.retain(|r| r.number != number);
            state.marks.remove(&number);
            let mut shell = shell.borrow_mut();
            if let Some(tab) = shell.doc_tab(number) {
                shell.close_tab(tab);
            }
            if let Some(detached) = state.detached.remove(&number) {
                retire(detached.window, true);
            }
            after(&app, &mut state, &shell);
            app.global::<ReceiptsData>().set_selected_row(-1);
            refresh_list(&app, &state);
            crate::app::refresh(&app, &shell);
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

    wire_form(app, app.as_weak(), &state, &shell, Target::Main);

    ReceiptsUi { state, shell }
}

/// Закрывает форму: вкладку главного окна или документ в отдельном окне.
fn close_form(main: &AppWindow, state: &mut State, shell: &mut Shell, target: Target) {
    match target {
        Target::Main => close_active(main, state, shell),
        Target::Doc(number) => close_detached(main, state, shell, number, true),
    }
}

/// Подключает действия формы документа (кнопки, строки, справочники) к окну `app`: вкладке
/// главного окна (`Target::Main`) или отдельному окну документа. `main` это главное окно,
/// где живут список и вкладки.
fn wire_form(
    app: &AppWindow,
    main: slint::Weak<AppWindow>,
    state: &Rc<RefCell<State>>,
    shell: &Rc<RefCell<Shell>>,
    target: Target,
) {
    let data = app.global::<ReceiptsData>();
    let lines_snapshot: Snapshot = Rc::default();

    data.on_resize_begin_lines({
        let (app, snapshot) = (app.as_weak(), lines_snapshot.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            *snapshot.borrow_mut() = Some(snapshot_of(
                &app.global::<ReceiptsData>().get_line_columns(),
            ));
        }
    });
    data.on_resize_lines({
        let (app, snapshot) = (app.as_weak(), lines_snapshot.clone());
        move |index, delta, table_width| {
            let Some(app) = app.upgrade() else { return };
            let model = app.global::<ReceiptsData>().get_line_columns();
            if let Some(start) = snapshot.borrow().as_ref() {
                write_columns(&model, resized_columns(start, index, delta, table_width));
            }
        }
    });
    data.on_resize_end_lines({
        let snapshot = lines_snapshot;
        move || *snapshot.borrow_mut() = None
    });

    // Действия формы. Список и вкладки обновляются в главном окне (`main`), поэтому у отдельного
    // окна документа обработчики работают с двумя окнами сразу.
    data.on_form_save({
        let (app, main, state, shell) = (app.as_weak(), main.clone(), state.clone(), shell.clone());
        move || {
            let (Some(app), Some(main)) = (app.upgrade(), main.upgrade()) else {
                return;
            };
            let mut shell = shell.borrow_mut();
            {
                let mut state = form_state(&state, target);
                save_loaded(&app, &mut state, &mut shell, false);
                if let Some(number) = state.form.loaded {
                    load_doc(&app, &mut state, number);
                }
            }
            refresh_list(&main, &state.borrow());
            crate::app::refresh(&main, &shell);
        }
    });

    data.on_form_post({
        let (app, main, state, shell) = (app.as_weak(), main.clone(), state.clone(), shell.clone());
        move |close| {
            let (Some(app), Some(main)) = (app.upgrade(), main.upgrade()) else {
                return;
            };
            let mut shell = shell.borrow_mut();
            {
                let mut state = form_state(&state, target);
                save_loaded(&app, &mut state, &mut shell, true);
                if !close && let Some(number) = state.form.loaded {
                    load_doc(&app, &mut state, number);
                }
            }
            if close {
                close_form(&main, &mut state.borrow_mut(), &mut shell, target);
            }
            refresh_list(&main, &state.borrow());
            crate::app::refresh(&main, &shell);
        }
    });

    data.on_form_close({
        let (app, main, state, shell) = (app.as_weak(), main.clone(), state.clone(), shell.clone());
        move || {
            let (Some(app), Some(main)) = (app.upgrade(), main.upgrade()) else {
                return;
            };
            let mut shell = shell.borrow_mut();
            before(&app, &mut form_state(&state, target), &mut shell);
            close_form(&main, &mut state.borrow_mut(), &mut shell, target);
            crate::app::refresh(&main, &shell);
        }
    });

    // Из вкладки в отдельное окно (кнопка на форме).
    data.on_form_detach({
        let (main, state_rc, shell_rc) = (main.clone(), state.clone(), shell.clone());
        move || {
            let Some(main) = main.upgrade() else { return };
            let mut shell = shell_rc.borrow_mut();
            let tab = shell.active_tab();
            detach(&main, &state_rc, &shell_rc, &mut shell, tab);
            crate::app::refresh(&main, &shell);
        }
    });

    // Из отдельного окна обратно в панель вкладок.
    data.on_form_dock({
        let (app, main, state, shell) = (app.as_weak(), main.clone(), state.clone(), shell.clone());
        move || {
            let (Some(app), Some(main)) = (app.upgrade(), main.upgrade()) else {
                return;
            };
            let Target::Doc(number) = target else { return };
            let mut shell = shell.borrow_mut();
            store_form(&app, &mut form_state(&state, target));
            dock(&main, &mut state.borrow_mut(), &mut shell, number);
            crate::app::refresh(&main, &shell);
        }
    });

    data.on_line_add({
        let (app, state) = (app.as_weak(), state.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            open_line_dialog(&app, &mut form_state(&state, target), None);
        }
    });

    data.on_line_edit({
        let (app, state) = (app.as_weak(), state.clone());
        move |row| {
            let Some(app) = app.upgrade() else { return };
            open_line_dialog(
                &app,
                &mut form_state(&state, target),
                usize::try_from(row).ok(),
            );
        }
    });

    data.on_line_delete({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let selected = app.global::<ReceiptsData>().get_line_selected();
            if let Ok(index) = usize::try_from(selected) {
                change_lines(
                    &app,
                    &mut form_state(&state, target),
                    &mut shell.borrow_mut(),
                    |lines| {
                        (index < lines.len()).then(|| {
                            lines.remove(index);
                            None
                        })
                    },
                );
            }
        }
    });

    // Меню строки табличной части.
    data.on_line_menu_opening({
        let (app, state) = (app.as_weak(), state.clone());
        move |row| {
            let Some(app) = app.upgrade() else { return };
            let state = form_state(&state, target);
            let count = state
                .form
                .loaded
                .and_then(|n| state.docs.get(&n))
                .map_or(0, |doc| doc.content.lines.len());
            let actions = usize::try_from(row)
                .ok()
                .filter(|i| *i < count)
                .map(|i| line_menu(i, count))
                .unwrap_or_default();
            app.global::<ReceiptsData>()
                .set_line_menu_actions(ModelRc::new(VecModel::from(actions)));
        }
    });

    data.on_line_action({
        let (app, state, shell) = (app.as_weak(), state.clone(), shell.clone());
        move |row, id| {
            let Some(app) = app.upgrade() else { return };
            let Some(index) = usize::try_from(row).ok() else {
                return;
            };
            let (mut state, mut shell) = (form_state(&state, target), shell.borrow_mut());
            match id.as_str() {
                "edit" => open_line_dialog(&app, &mut state, Some(index)),
                "add" => open_line_dialog(&app, &mut state, None),
                // Каждое изменение состава возвращает, какую строку выделить после него.
                "copy" => change_lines(&app, &mut state, &mut shell, |lines| {
                    let line = lines.get(index)?.clone();
                    lines.insert(index + 1, line);
                    Some(Some(index + 1))
                }),
                "up" => change_lines(&app, &mut state, &mut shell, |lines| {
                    (index > 0 && index < lines.len()).then(|| {
                        lines.swap(index, index - 1);
                        Some(index - 1)
                    })
                }),
                "down" => change_lines(&app, &mut state, &mut shell, |lines| {
                    (index + 1 < lines.len()).then(|| {
                        lines.swap(index, index + 1);
                        Some(index + 1)
                    })
                }),
                "delete" => change_lines(&app, &mut state, &mut shell, |lines| {
                    (index < lines.len()).then(|| {
                        lines.remove(index);
                        None
                    })
                }),
                _ => {}
            }
        }
    });

    // «+ Добавить» в выпадающем списке: окно нового элемента справочника.
    data.on_catalog_add({
        let (app, state) = (app.as_weak(), state.clone());
        move |kind| {
            let Some(app) = app.upgrade() else { return };
            let Some(catalog) = Catalog::from_index(kind) else {
                return;
            };
            form_state(&state, target).form.catalog_kind = Some(catalog);
            let data = app.global::<ReceiptsData>();
            data.set_catalog_title(catalog.dialog_title().into());
            data.set_catalog_has_unit(catalog == Catalog::Goods);
            data.set_catalog_name("".into());
            data.set_catalog_unit("шт".into());
            data.set_catalog_error("".into());
            data.set_catalog_open(true);
        }
    });

    data.on_catalog_confirm({
        let (app, main, state) = (app.as_weak(), main.clone(), state.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let mut state = form_state(&state, target);
            let Some(catalog) = state.form.catalog_kind else {
                return;
            };
            let data = app.global::<ReceiptsData>();
            let name = data.get_catalog_name().trim().to_owned();
            let unit = data.get_catalog_unit().trim().to_owned();
            let existing: Vec<&String> = match catalog {
                Catalog::Warehouse => state.warehouses.iter().collect(),
                Catalog::Supplier => state.suppliers.iter().collect(),
                Catalog::Goods => state.goods.iter().map(|(n, _)| n).collect(),
            };
            let error = if name.is_empty() {
                "Введите наименование."
            } else if catalog == Catalog::Goods && unit.is_empty() {
                "Укажите единицу измерения."
            } else if existing
                .iter()
                .any(|n| n.to_lowercase() == name.to_lowercase())
            {
                "Такой элемент уже есть в справочнике."
            } else {
                ""
            };
            if !error.is_empty() {
                data.set_catalog_error(error.into());
                return;
            }
            // Новый элемент появляется в списке и сразу выбирается в поле, из которого его добавляли.
            match catalog {
                Catalog::Warehouse => {
                    state.warehouses.push(name.clone());
                    data.set_form_warehouse(name.into());
                }
                Catalog::Supplier => {
                    state.suppliers.push(name.clone());
                    data.set_form_supplier(name.into());
                }
                Catalog::Goods => {
                    state.goods.push((name.clone(), unit));
                    data.set_line_name(name.into());
                }
            }
            if let Some(main) = main.upgrade() {
                publish_all(&main, &state);
            }
            state.form.catalog_kind = None;
            data.set_catalog_open(false);
        }
    });

    // Количество и цена: только цифры и запятая (буквы и лишние знаки отбрасываются сразу).
    data.on_line_quantity_edited({
        let app = app.as_weak();
        move |text| {
            let Some(app) = app.upgrade() else { return };
            let clean = receipts::clean_decimal(&text, 1);
            if clean != text.as_str() {
                app.global::<ReceiptsData>().set_line_quantity(clean.into());
            }
        }
    });
    data.on_line_price_edited({
        let app = app.as_weak();
        move |text| {
            let Some(app) = app.upgrade() else { return };
            let clean = receipts::clean_decimal(&text, 2);
            if clean != text.as_str() {
                app.global::<ReceiptsData>().set_line_price(clean.into());
            }
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
            let mut state = form_state(&state, target);
            let unit = state
                .goods
                .iter()
                .find(|(good, _)| *good == name)
                .map_or("шт", |(_, unit)| unit.as_str())
                .to_owned();
            let line = Line::new(&name, &unit, quantity, price);
            let editing = state.form.editing_line;
            data.set_line_open(false);
            change_lines(&app, &mut state, &mut shell.borrow_mut(), |lines| {
                match editing.filter(|i| *i < lines.len()) {
                    Some(index) => lines[index] = line,
                    None => lines.push(line),
                }
                Some(None)
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_is_converted_to_kopecks() {
        assert_eq!(kopecks(1250, 2), 1250);
        assert_eq!(kopecks(12, 0), 1200);
        assert_eq!(kopecks(12_345, 3), 1234);
    }

    #[test]
    fn iso_date_is_parsed() {
        assert_eq!(parse_iso_date("2026-10-18"), Some((2026, 10, 18)));
        assert_eq!(parse_iso_date("18.10.2026"), None);
    }

    /// Список из настоящего ядра: все строки пришли, порядок и поиск делает ядро.
    #[test]
    fn list_comes_from_the_core() {
        let path =
            std::env::temp_dir().join(format!("ostov-ui-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let text = path.to_str().unwrap_or_default();
        let engine = ostov_core::LocalConnector.create_demo(text, 50).unwrap();

        let (all, total) = fetch_receipts(&*engine, &ListRequest::default()).unwrap();
        assert_eq!((all.len(), total), (50, 50));
        assert!(
            all.iter()
                .all(|r| r.id > 0 && !r.supplier.is_empty() && r.amount > 0)
        );

        let descending = ListRequest {
            order: Some("number".to_owned()),
            descending: true,
        };
        let (rows, _) = fetch_receipts(&*engine, &descending).unwrap();
        assert!(rows.windows(2).all(|pair| pair[0].number > pair[1].number));

        drop(engine);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite.lock"));
    }
}
