//! Дополнительные окна с вкладками: создание, обновление, закрытие. Состав вкладок хранит
//! `Workspace`, здесь только окна Slint и обработчики их панели вкладок.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, VecModel};

use crate::AppWindow;
use crate::app::{current_settings, prepare_host_window, refresh, tab_items};
use crate::shell::{Shell, Tab};
use crate::workspace::{Change, WindowId, WindowRef, Workspace};

/// Перетаскивание вкладки между окнами на общих координатах экрана.
struct DragState {
    /// Единственная вкладка дополнительного окна: двигается само окно, копия не нужна.
    whole: bool,
    /// Окно-копия, идущее за указателем (вкладка вышла за панель).
    follower: Option<WindowId>,
    /// Где указатель был внутри окна в начале перетаскивания (физические пиксели).
    grab: (f32, f32),
}

struct Inner {
    workspace: Workspace,
    hosts: HashMap<WindowId, AppWindow>,
    drag: Option<DragState>,
}

/// Высота панели вкладок (логические пиксели).
const STRIP_HEIGHT: f32 = 40.0;
/// Где в новом окне вкладка: указатель встаёт на её середину (логические пиксели).
const FOLLOWER_GRAB: (f32, f32) = (128.0, 56.0);

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
}

impl Default for Windows {
    fn default() -> Self {
        Self::new()
    }
}

/// Убирает окно: прячет и освобождает чуть позже, обработчик, из которого вызвали, может быть
/// событием этого же окна.
fn retire(window: AppWindow) {
    let _ = window.hide();
    slint::Timer::single_shot(Duration::from_millis(100), move || drop(window));
}

impl Windows {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                workspace: Workspace::new(),
                hosts: HashMap::new(),
                drag: None,
            })),
        }
    }

    /// Активное окно: в нём открываются новые вкладки и документы.
    pub fn active(&self) -> WindowRef {
        self.inner.borrow().workspace.active()
    }

    pub fn set_active(&self, window: WindowRef) {
        self.inner.borrow_mut().workspace.set_active(window);
    }

    /// Вкладку можно вынести в окно: список и пользователи пока привязаны к главному окну.
    pub fn can_detach(tab: &Tab) -> bool {
        !matches!(tab.view, "users" | "receipts")
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
        self.all_windows(main)
            .into_iter()
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
        // Документы и вкладки со своим состоянием двигаются другим путём.
        let Some(tab) = self.tab_at(shell, source, index) else {
            return;
        };
        if tab.doc.is_some() || !Self::can_detach(&tab) {
            return;
        }
        let scale = source_window.window().scale_factor();
        let point = pointer_global(source_window, x, y);
        let (whole, follower, grab) = {
            let mut inner = self.inner.borrow_mut();
            if inner.drag.is_none() {
                let whole = matches!(source, WindowRef::Extra(id)
                    if inner.workspace.list(id).is_some_and(|l| l.tabs().len() == 1));
                inner.drag = Some(DragState {
                    whole,
                    follower: None,
                    grab: (x * scale, y * scale),
                });
            }
            match inner.drag.as_ref() {
                Some(drag) => (drag.whole, drag.follower, drag.grab),
                None => return,
            }
        };
        if whole {
            source_window.window().set_position(PhysicalPosition::new(
                (point.0 - grab.0) as i32,
                (point.1 - grab.1) as i32,
            ));
            self.highlight(main, self.find_target(main, point, &[source]));
            return;
        }
        let mut follower = follower;
        if out && follower.is_none() {
            let id = self.inner.borrow_mut().workspace.open_extra(tab);
            self.open_host(main, shell, id);
            self.refresh_host(id);
            if let Some(drag) = self.inner.borrow_mut().drag.as_mut() {
                drag.follower = Some(id);
            }
            follower = Some(id);
        } else if !out && let Some(id) = follower {
            // Вернулись в панель исходного окна: окно-копия не нужно.
            let change = self.inner.borrow_mut().workspace.cancel_detach(id);
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
                (None, Some((to, at))) if state.whole || out => {
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

    /// Меняет порядок вкладок внутри окна: `to` это новый индекс вкладки после переноса.
    pub fn reorder(
        &self,
        main: &AppWindow,
        shell: &Rc<RefCell<Shell>>,
        window: WindowRef,
        from: usize,
        to: usize,
    ) {
        if from == to {
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
        if let Some(change) = change {
            self.apply(main, shell, change);
        }
    }

    /// Применяет изменение набора окон и обновляет все окна.
    fn apply(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, change: Change) {
        for id in change.closed {
            let window = self.inner.borrow_mut().hosts.remove(&id);
            if let Some(window) = window {
                retire(window);
            }
        }
        if let Some(id) = change.opened {
            self.open_host(main, shell, id);
        }
        refresh(main, &shell.borrow());
        let ids: Vec<WindowId> = self.inner.borrow().hosts.keys().copied().collect();
        for id in ids {
            self.refresh_host(id);
        }
    }

    fn refresh_host(&self, id: WindowId) {
        let inner = self.inner.borrow();
        let (Some(window), Some(list)) = (inner.hosts.get(&id), inner.workspace.list(id)) else {
            return;
        };
        window.set_tabs(ModelRc::new(VecModel::from(tab_items(list.tabs()))));
        window.set_active_tab(i32::try_from(list.active()).unwrap_or_default());
        let title = list
            .tabs()
            .get(list.active())
            .map(|tab| tab.title.clone())
            .unwrap_or_default();
        window.set_doc_title(title.into());
    }

    fn open_host(&self, main: &AppWindow, shell: &Rc<RefCell<Shell>>, id: WindowId) {
        let Ok(window) = AppWindow::new() else { return };
        prepare_host_window(&window, current_settings(main));

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
                        list.select(tab);
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
                let change = {
                    let mut inner = this.inner.borrow_mut();
                    inner.workspace.move_tab(
                        &mut shell.borrow_mut(),
                        WindowRef::Extra(id),
                        tab,
                        WindowRef::Main,
                        usize::MAX,
                    )
                };
                if let Some(change) = change {
                    this.apply(&main, &shell, change);
                }
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
