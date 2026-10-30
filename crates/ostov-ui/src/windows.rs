//! Дополнительные окна с вкладками: создание, обновление, закрытие. Состав вкладок хранит
//! `Workspace`, здесь только окна Slint и обработчики их панели вкладок.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, LogicalSize, Model, PhysicalPosition};

use crate::AppWindow;
use crate::TabItem;
use crate::app::{
    HOST_SIZE, current_settings, nav_items, prepare_host_window, refresh, set_tabs, tab_items,
};
use crate::receipts_ui::ReceiptsUi;
use crate::shell::{Shell, Tab};
use crate::workspace::{Change, TabList, WindowId, WindowRef, Workspace};

/// Перетаскивание вкладки между окнами на общих координатах экрана.
struct DragState {
    /// Единственная вкладка дополнительного окна: двигается само окно, копия не нужна.
    whole: bool,
    /// Окно-копия, идущее за указателем (вкладка вышла за панель).
    follower: Option<WindowId>,
    /// Размер окна, которое тащат целиком: на время перетаскивания оно уменьшается.
    restore: Option<LogicalSize>,
}

/// Что подключить к каждому новому окну.
type HostSetup = Rc<dyn Fn(&AppWindow)>;

struct Inner {
    workspace: Workspace,
    hosts: HashMap<WindowId, AppWindow>,
    drag: Option<DragState>,
    /// Что подключить к каждому новому окну, кроме форм (например, пользователей).
    host_setup: Option<HostSetup>,
}

/// Журнал перетаскивания для разбора странностей в живых окнах: включается переменной
/// `OSTOV_DEBUG_DRAG=1`, пишет в `~/.local/state/ostov/drag.log`.
fn drag_log(text: &str) {
    use std::io::Write;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("OSTOV_DEBUG_DRAG").is_some()) {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let dir = std::path::Path::new(&home).join(".local/state/ostov");
    let _ = std::fs::create_dir_all(&dir);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() % 1_000_000);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("drag.log"))
    {
        let _ = writeln!(file, "{millis:>6} {text}");
    }
}

/// Высота панели вкладок (логические пиксели).
const STRIP_HEIGHT: f32 = 40.0;
/// Где в окне вкладка, которую тащат: указатель встаёт на её середину (логические пиксели).
const FOLLOWER_GRAB: (f32, f32) = (128.0, 56.0);
/// Размер окна на время перетаскивания. Оконный менеджер не даёт программно вывести окно за край
/// экрана (mutter держит его целиком на экране): большое окно упёрлось бы и отстало от указателя.
const DRAG_SIZE: LogicalSize = LogicalSize {
    width: 640.0,
    height: 420.0,
};

/// Положения окон и указателя в общих координатах экрана известны на Windows, macOS и X11.
/// На чистом Wayland приложение их не знает.
pub fn global_coords() -> bool {
    cfg!(any(windows, target_os = "macos"))
        || (cfg!(target_os = "linux")
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var_os("DISPLAY").is_some())
}

/// Указатель в общих координатах экрана (физические пиксели) по положению внутри окна.
fn pointer_global(window: &AppWindow, x: f32, y: f32) -> (f32, f32) {
    let position = window.window().position();
    let scale = window.window().scale_factor();
    (position.x as f32 + x * scale, position.y as f32 + y * scale)
}

/// Место вставки, если точка экрана попадает в панель вкладок окна.
fn strip_hit(window: &AppWindow, point: (f32, f32)) -> Option<usize> {
    let position = window.window().position();
    let scale = window.window().scale_factor();
    let local = (
        (point.0 - position.x as f32) / scale,
        (point.1 - position.y as f32) / scale,
    );
    let (left, top) = (window.get_strip_left(), window.get_strip_top());
    if local.0 < left
        || local.0 > left + window.get_strip_width()
        || local.1 < top
        || local.1 > top + STRIP_HEIGHT
    {
        return None;
    }
    let count = window.get_tabs().row_count();
    let tab_width = window.get_strip_tab_width().max(1.0);
    let at = ((local.0 - left - 8.0) / tab_width).round().max(0.0);
    Some((at as usize).min(count))
}

/// Дополнительные окна и активное окно.
#[derive(Clone)]
pub struct Windows {
    inner: Rc<RefCell<Inner>>,
    receipts: ReceiptsUi,
    /// Дерево навигации общее; у каждого окна своя подсветка выбранного пункта.
    shell: Rc<RefCell<Shell>>,
}

/// Переносит строку модели вкладок окна с `from` на `to` (индекс после переноса).
fn move_row(window: &AppWindow, from: usize, to: usize) {
    let model = window.get_tabs();
    if let Some(rows) = model.as_any().downcast_ref::<slint::VecModel<TabItem>>()
        && from < rows.row_count()
    {
        let row = rows.remove(from);
        rows.insert(to.min(rows.row_count()), row);
    }
}

/// Убирает окно: прячет и освобождает чуть позже, обработчик, из которого вызвали, может быть
/// событием этого же окна.
fn retire(window: AppWindow) {
    let _ = window.hide();
    slint::Timer::single_shot(Duration::from_millis(100), move || drop(window));
}

impl Windows {
    pub fn new(receipts: &ReceiptsUi) -> Self {
        let windows = Self {
            inner: Rc::new(RefCell::new(Inner {
                workspace: Workspace::new(),
                hosts: HashMap::new(),
                drag: None,
                host_setup: None,
            })),
            receipts: receipts.clone(),
            shell: receipts.shell(),
        };
        receipts.bind_windows(&windows);
        windows
    }

    /// Что подключить к каждому новому окну, кроме списка и форм документов.
    pub fn on_host_created(&self, setup: impl Fn(&AppWindow) + 'static) {
        self.inner.borrow_mut().host_setup = Some(Rc::new(setup));
    }

    /// Доступ к вкладкам дополнительного окна.
    pub fn with_list<R>(&self, id: WindowId, f: impl FnOnce(&mut TabList) -> R) -> Option<R> {
        self.inner.borrow_mut().workspace.list_mut(id).map(f)
    }

    /// Для тестового стенда: окна по модели, окна на экране и число вкладок в каждом окне модели.
    /// Две первые части обязаны совпадать.
    #[doc(hidden)]
    pub fn debug_state(&self) -> (Vec<WindowId>, Vec<WindowId>, Vec<usize>) {
        let inner = self.inner.borrow();
        let mut model = inner.workspace.extra_ids();
        model.sort_unstable();
        let mut screen: Vec<WindowId> = inner.hosts.keys().copied().collect();
        screen.sort_unstable();
        let tabs = model
            .iter()
            .map(|id| inner.workspace.list(*id).map_or(0, |l| l.tabs().len()))
            .collect();
        (model, screen, tabs)
    }

    /// Окно дополнительное по номеру (для стенда).
    #[doc(hidden)]
    pub fn host(&self, id: WindowId) -> Option<AppWindow> {
        self.inner.borrow().hosts.get(&id).map(|w| w.clone_strong())
    }

    /// Активное окно: в нём открываются новые вкладки и документы.
    pub fn active(&self) -> WindowRef {
        self.inner.borrow().workspace.active()
    }

    pub fn set_active(&self, window: WindowRef) {
        self.inner.borrow_mut().workspace.set_active(window);
    }

    /// Вызывает `f` для каждого дополнительного окна (настройки темы и вида).
    pub fn each(&self, mut f: impl FnMut(&AppWindow)) {
        for window in self.inner.borrow().hosts.values() {
            f(window);
        }
    }

    /// Выносит вкладку главного окна в новое окно.
    pub fn detach_from_main(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, index: usize) {
        self.detach(main, shell, WindowRef::Main, index);
    }

    fn detach(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, from: WindowRef, index: usize) {
        let change = {
            let mut inner = self.inner.borrow_mut();
            inner.workspace.detach(&mut shell.borrow_mut(), from, index)
        };
        if let Some(change) = change {
            self.apply(main, shell, change);
        }
    }

    fn all_windows(&self, main: &AppWindow) -> Vec<(WindowRef, AppWindow)> {
        let mut windows = vec![(WindowRef::Main, main.clone_strong())];
        windows.extend(
            self.inner
                .borrow()
                .hosts
                .iter()
                .map(|(id, window)| (WindowRef::Extra(*id), window.clone_strong())),
        );
        windows
    }

    /// Подсвечивает панель окна, в которую упадёт вкладка (`None`: никакую).
    fn highlight(&self, main: &AppWindow, target: Option<(WindowRef, usize)>) {
        for (window_ref, window) in self.all_windows(main) {
            match target {
                Some((t, at)) if t == window_ref => {
                    window.set_drop_target(true);
                    window.set_insert_at(i32::try_from(at).unwrap_or(0));
                }
                _ => {
                    window.set_drop_target(false);
                    window.set_insert_at(-1);
                }
            }
        }
    }

    /// Окно, в панель вкладок которого попадает точка экрана, кроме исключённых.
    fn find_target(
        &self,
        main: &AppWindow,
        point: (f32, f32),
        exclude: &[WindowRef],
    ) -> Option<(WindowRef, usize)> {
        // Сначала дополнительные окна: обычно они лежат поверх главного.
        self.all_windows(main)
            .into_iter()
            .rev()
            .filter(|(window_ref, _)| !exclude.contains(window_ref))
            .find_map(|(window_ref, window)| strip_hit(&window, point).map(|at| (window_ref, at)))
    }

    fn tab_at(&self, shell: &Rc<RefCell<Shell>>, window: WindowRef, index: usize) -> Option<Tab> {
        match window {
            WindowRef::Main => shell.borrow().tabs().get(index).cloned(),
            WindowRef::Extra(id) => self
                .inner
                .borrow()
                .workspace
                .list(id)
                .and_then(|list| list.tabs().get(index).cloned()),
        }
    }

    /// Вкладку тащат: окно с вкладкой идёт за указателем, панель под указателем подсвечивается.
    /// Работает на общих координатах экрана; на чистом Wayland ничего не делает.
    #[allow(clippy::too_many_arguments)]
    pub fn drag_moved(
        &self,
        main: &AppWindow,
        shell: &Rc<RefCell<Shell>>,
        source: WindowRef,
        index: usize,
        source_window: &AppWindow,
        x: f32,
        y: f32,
        out: bool,
    ) {
        if !global_coords() {
            return;
        }
        let Some(tab) = self.tab_at(shell, source, index) else {
            return;
        };
        let scale = source_window.window().scale_factor();
        let point = pointer_global(source_window, x, y);
        drag_log(&format!(
            "moved {source:?} tab={index} local=({x:.0},{y:.0}) out={out} окно={:?} масштаб={scale} точка=({:.0},{:.0})",
            source_window.window().position(),
            point.0,
            point.1
        ));
        let (whole, follower) = {
            let mut inner = self.inner.borrow_mut();
            if inner.drag.is_none() {
                let whole = matches!(source, WindowRef::Extra(id)
                    if inner.workspace.list(id).is_some_and(|l| l.tabs().len() == 1));
                inner.drag = Some(DragState {
                    whole,
                    follower: None,
                    restore: whole.then(|| {
                        source_window
                            .window()
                            .size()
                            .to_logical(source_window.window().scale_factor())
                    }),
                });
                if whole {
                    source_window.set_keep_on_top(true);
                    source_window.window().set_maximized(false);
                    source_window.window().set_size(DRAG_SIZE);
                }
            }
            match inner.drag.as_ref() {
                Some(drag) => (drag.whole, drag.follower),
                None => return,
            }
        };
        if whole {
            let (x, y) = (
                point.0 - FOLLOWER_GRAB.0 * scale,
                point.1 - FOLLOWER_GRAB.1 * scale,
            );
            drag_log(&format!("  целое окно -> ({x:.0},{y:.0})"));
            source_window
                .window()
                .set_position(PhysicalPosition::new(x as i32, y as i32));
            self.highlight(main, self.find_target(main, point, &[source]));
            return;
        }
        let mut follower = follower;
        if out && follower.is_none() {
            let id = self.inner.borrow_mut().workspace.open_extra(tab);
            drag_log(&format!("  создано окно-копия {id}"));
            self.open_host(main, shell, id, true);
            source_window.set_follower_active(true);
            if let Some(drag) = self.inner.borrow_mut().drag.as_mut() {
                drag.follower = Some(id);
            }
            follower = Some(id);
            self.apply(main, shell, Change::default());
        } else if !out && let Some(id) = follower {
            // Вернулись в панель исходного окна: окно-копия не нужно.
            let change = self.inner.borrow_mut().workspace.cancel_detach(id);
            source_window.set_follower_active(false);
            if let Some(drag) = self.inner.borrow_mut().drag.as_mut() {
                drag.follower = None;
            }
            self.apply(main, shell, change);
            follower = None;
        }
        let mut exclude = vec![source];
        if let Some(id) = follower {
            exclude.push(WindowRef::Extra(id));
            if let Some(window) = self.inner.borrow().hosts.get(&id) {
                let s = window.window().scale_factor();
                window.window().set_position(PhysicalPosition::new(
                    (point.0 - FOLLOWER_GRAB.0 * s) as i32,
                    (point.1 - FOLLOWER_GRAB.1 * s) as i32,
                ));
            }
        }
        self.highlight(main, self.find_target(main, point, &exclude));
    }

    /// Вкладку отпустили. `true`: перетаскивание обработано здесь; `false`: вызывающий код
    /// сам решает, что делать (чистый Wayland, документы, вкладки со своим состоянием).
    #[allow(clippy::too_many_arguments)]
    pub fn drag_ended(
        &self,
        main: &AppWindow,
        shell: &Rc<RefCell<Shell>>,
        source: WindowRef,
        index: usize,
        source_window: &AppWindow,
        x: f32,
        y: f32,
        out: bool,
    ) -> bool {
        self.highlight(main, None);
        source_window.set_follower_active(false);
        drag_log(&format!(
            "ended {source:?} tab={index} local=({x:.0},{y:.0}) out={out}"
        ));
        let state = self.inner.borrow_mut().drag.take();
        let Some(state) = state.filter(|_| global_coords()) else {
            return false;
        };
        let point = pointer_global(source_window, x, y);
        let mut exclude = vec![source];
        if let Some(id) = state.follower {
            exclude.push(WindowRef::Extra(id));
        }
        let target = self.find_target(main, point, &exclude);
        drag_log(&format!(
            "  состояние={:?} цель={target:?}",
            state.follower.map(|f| (f, state.whole))
        ));
        // Окно, которое остаётся жить, снова обычное и своего размера.
        if let Some(size) = state.restore {
            source_window.set_keep_on_top(false);
            source_window.window().set_size(size);
        }
        if let (Some(id), None) = (state.follower, target) {
            let window = self.inner.borrow().hosts.get(&id).map(|w| w.clone_strong());
            if let Some(window) = window {
                window.set_keep_on_top(false);
                window.window().set_size(HOST_SIZE);
            }
        }
        let change = {
            let mut inner = self.inner.borrow_mut();
            let mut shell = shell.borrow_mut();
            match (state.follower, target) {
                (Some(id), Some((to, at))) => inner
                    .workspace
                    .drop_on(&mut shell, id, source, index, to, at),
                (Some(id), None) => {
                    Some(inner.workspace.finish_detach(&mut shell, source, index, id))
                }
                // Отпустили над панелью другого окна: вкладка переезжает, даже если указатель не
                // выходил за свою панель (окна стоят рядом на одной высоте).
                (None, Some((to, at))) => {
                    inner.workspace.move_tab(&mut shell, source, index, to, at)
                }
                _ => None,
            }
        };
        if let Some(change) = change {
            self.apply(main, shell, change);
        }
        true
    }

    /// Закрывает активную вкладку дополнительного окна.
    pub fn close_active_tab(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, id: WindowId) {
        let change = {
            let mut inner = self.inner.borrow_mut();
            let active = inner.workspace.list(id).map(TabList::active);
            active.map(|active| inner.workspace.close_tab(id, active))
        };
        if let Some(change) = change {
            self.apply(main, shell, change);
        }
    }

    /// Возвращает вкладку дополнительного окна в главное.
    fn return_tab(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, id: WindowId, tab: usize) {
        let change = self.inner.borrow_mut().workspace.move_tab(
            &mut shell.borrow_mut(),
            WindowRef::Extra(id),
            tab,
            WindowRef::Main,
            usize::MAX,
        );
        if let Some(change) = change {
            self.apply(main, shell, change);
        }
    }

    /// Показывает вкладку документа в окне, где он уже открыт.
    pub fn show_doc(
        &self,
        main: &AppWindow,
        shell: &Rc<RefCell<Shell>>,
        window: WindowRef,
        doc: u32,
    ) {
        match window {
            WindowRef::Main => {
                let mut shell = shell.borrow_mut();
                if let Some(tab) = shell.doc_tab(doc) {
                    shell.select_tab(tab);
                }
            }
            WindowRef::Extra(id) => {
                self.with_list(id, |list| {
                    if let Some(tab) = list.doc_tab(doc) {
                        list.select(tab);
                    }
                });
            }
        }
        self.set_active(window);
        self.apply(main, shell, Change::default());
    }

    /// Меняет порядок вкладок внутри окна: `to` это новый индекс вкладки после переноса.
    pub fn reorder(
        &self,
        main: &AppWindow,
        shell: &Rc<RefCell<Shell>>,
        window: WindowRef,
        from: usize,
        to: usize,
    ) {
        // Над чужой панелью вкладку не переставляют: её сейчас перенесёт `drag_ended`.
        let over_other = self
            .all_windows(main)
            .iter()
            .any(|(_, window)| window.get_drop_target());
        if from == to || over_other {
            return;
        }
        // Модель принимает место вставки до изъятия вкладки.
        let at = if to > from { to + 1 } else { to };
        let change = {
            let mut inner = self.inner.borrow_mut();
            inner
                .workspace
                .move_tab(&mut shell.borrow_mut(), window, from, window, at)
        };
        if change.is_some() {
            // Строка переезжает вместе с вкладкой: соседи остаются на своих местах, а не
            // меняются содержимым (иначе при отпускании видно рывок и повторную анимацию).
            let window = match window {
                WindowRef::Main => Some(main.clone_strong()),
                WindowRef::Extra(id) => self.host(id),
            };
            if let Some(window) = window {
                move_row(&window, from, to);
            }
        }
        if let Some(change) = change {
            self.apply(main, shell, change);
        }
    }

    /// Применяет изменение набора окон и обновляет все окна.
    fn apply(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, change: Change) {
        // Поля форм попадают в документы, пока окна ещё на экране.
        self.receipts.store_forms(main);
        for id in change.closed {
            self.receipts.host_closed(id);
            let window = self.inner.borrow_mut().hosts.remove(&id);
            if let Some(window) = window {
                retire(window);
            }
        }
        if let Some(id) = change.opened {
            self.open_host(main, shell, id, false);
        }
        self.receipts.sync(
            main,
            &mut shell.borrow_mut(),
            &mut self.inner.borrow_mut().workspace,
        );
        refresh(main, &shell.borrow());
        let ids: Vec<WindowId> = self.inner.borrow().hosts.keys().copied().collect();
        for id in ids {
            self.refresh_host(id);
        }
    }

    /// Обновляет дополнительные окна (например, раскрыли группу навигации).
    pub fn refresh_hosts(&self) {
        let ids: Vec<WindowId> = self.inner.borrow().hosts.keys().copied().collect();
        for id in ids {
            self.refresh_host(id);
        }
    }

    /// Пункт навигации в дополнительном окне: группа раскрывается, остальное открывается вкладкой
    /// в этом окне.
    fn host_nav(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, id: WindowId, node: usize) {
        if shell.borrow().is_group(node) {
            shell.borrow_mut().toggle_group(node);
        } else if let Some(tab) = shell.borrow().node_tab(node) {
            self.with_list(id, |list| list.open_node(tab));
            self.set_active(WindowRef::Extra(id));
        }
        self.apply(main, shell, Change::default());
    }

    /// Показывает вкладки дополнительного окна.
    pub fn refresh_host(&self, id: WindowId) {
        let inner = self.inner.borrow();
        let (Some(window), Some(list)) = (inner.hosts.get(&id), inner.workspace.list(id)) else {
            return;
        };
        set_tabs(window, tab_items(list.tabs()));
        // Дерево навигации общее, выбранный пункт свой: по активной вкладке этого окна.
        if let Ok(shell) = self.shell.try_borrow() {
            let selected = list.tabs().get(list.active()).map(|tab| tab.node);
            window.set_nav(slint::ModelRc::new(slint::VecModel::from(nav_items(
                shell.nav_rows_for(selected),
            ))));
        }
        window.set_active_tab(i32::try_from(list.active()).unwrap_or_default());
        let title = list
            .tabs()
            .get(list.active())
            .map(|tab| tab.title.clone())
            .unwrap_or_default();
        window.set_doc_title(title.into());
    }

    /// Создаёт окно `id`. `dragging`: окно идёт за указателем, поверх остальных и уменьшенное.
    fn open_host(
        &self,
        main: &AppWindow,
        shell: &Rc<RefCell<Shell>>,
        id: WindowId,
        dragging: bool,
    ) {
        let Ok(window) = AppWindow::new() else { return };
        prepare_host_window(&window, current_settings(main));
        if dragging {
            window.set_keep_on_top(true);
            window.window().set_size(DRAG_SIZE);
        }
        let setup = self.inner.borrow().host_setup.clone();
        if let Some(setup) = setup {
            setup(&window);
        }
        self.receipts.host_opened(&window, id);

        window.on_nav_clicked({
            let (this, shell, main) = (self.clone(), shell.clone(), main.as_weak());
            move |node| {
                let (Ok(node), Some(main)) = (usize::try_from(node), main.upgrade()) else {
                    return;
                };
                this.host_nav(&main, &shell, id, node);
            }
        });
        window.on_tab_selected({
            let this = self.clone();
            move |tab| {
                let Ok(tab) = usize::try_from(tab) else {
                    return;
                };
                {
                    let mut inner = this.inner.borrow_mut();
                    inner.workspace.set_active(WindowRef::Extra(id));
                    if let Some(list) = inner.workspace.list_mut(id) {
                        this.receipts.before_host(id, list);
                        list.select(tab);
                        this.receipts.after_host(id, list);
                    }
                }
                this.refresh_host(id);
            }
        });
        window.on_tab_closed({
            let (this, shell, main) = (self.clone(), shell.clone(), main.as_weak());
            move |tab| {
                let (Ok(tab), Some(main)) = (usize::try_from(tab), main.upgrade()) else {
                    return;
                };
                let change = this.inner.borrow_mut().workspace.close_tab(id, tab);
                this.apply(&main, &shell, change);
            }
        });
        window.on_tab_detached({
            let (this, shell, main) = (self.clone(), shell.clone(), main.as_weak());
            move |tab| {
                let (Ok(tab), Some(main)) = (usize::try_from(tab), main.upgrade()) else {
                    return;
                };
                this.detach(&main, &shell, WindowRef::Extra(id), tab);
            }
        });
        window.on_tab_reordered({
            let (this, shell, main) = (self.clone(), shell.clone(), main.as_weak());
            move |from, to| {
                let (Ok(from), Ok(to), Some(main)) =
                    (usize::try_from(from), usize::try_from(to), main.upgrade())
                else {
                    return;
                };
                this.reorder(&main, &shell, WindowRef::Extra(id), from, to);
            }
        });
        window.on_tab_drag_moved({
            let (this, shell, main, me) = (
                self.clone(),
                shell.clone(),
                main.as_weak(),
                window.as_weak(),
            );
            move |tab, x, y, out| {
                let (Ok(tab), Some(main), Some(me)) =
                    (usize::try_from(tab), main.upgrade(), me.upgrade())
                else {
                    return;
                };
                this.drag_moved(&main, &shell, WindowRef::Extra(id), tab, &me, x, y, out);
            }
        });
        window.on_tab_drag_ended({
            let (this, shell, main, me) = (
                self.clone(),
                shell.clone(),
                main.as_weak(),
                window.as_weak(),
            );
            move |tab, x, y, out| {
                let (Ok(tab), Some(main), Some(me)) =
                    (usize::try_from(tab), main.upgrade(), me.upgrade())
                else {
                    return;
                };
                let source = WindowRef::Extra(id);
                if !this.drag_ended(&main, &shell, source, tab, &me, x, y, out) && out {
                    // Запасной путь без общих координат: окно создаётся при отпускании.
                    this.detach(&main, &shell, source, tab);
                }
            }
        });
        window.on_tab_returned({
            let (this, shell, main) = (self.clone(), shell.clone(), main.as_weak());
            move |tab| {
                let (Ok(tab), Some(main)) = (usize::try_from(tab), main.upgrade()) else {
                    return;
                };
                this.return_tab(&main, &shell, id, tab);
            }
        });
        // Закрытие окна крестиком закрывает его вкладки.
        window.window().on_close_requested({
            let this = self.clone();
            move || {
                let removed = {
                    let mut inner = this.inner.borrow_mut();
                    inner.workspace.close_window(id);
                    inner.hosts.remove(&id)
                };
                this.receipts.host_closed(id);
                this.receipts.forget_closed_docs();
                if let Some(window) = removed {
                    slint::Timer::single_shot(Duration::from_millis(100), move || drop(window));
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        let _ = window.show();
        self.inner.borrow_mut().hosts.insert(id, window);
    }
}
