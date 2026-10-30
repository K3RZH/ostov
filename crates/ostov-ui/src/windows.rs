//! Дополнительные окна с вкладками: создание, обновление, закрытие. Состав вкладок хранит
//! `Workspace`, здесь только окна Slint и обработчики их панели вкладок.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::AppWindow;
use crate::app::{current_settings, prepare_host_window, refresh, tab_items};
use crate::shell::{Shell, Tab};
use crate::workspace::{Change, WindowId, WindowRef, Workspace};

struct Inner {
    workspace: Workspace,
    hosts: HashMap<WindowId, AppWindow>,
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
