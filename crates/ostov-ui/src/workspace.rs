//! Окна и вкладки: какая вкладка в каком окне, какое окно активно.
//!
//! Главное окно держит свои вкладки в `Shell` (там же дерево навигации), дополнительные окна
//! (вынесенные вкладки) в `TabList`. Модель не знает про Slint: Slint-окна создаёт и закрывает
//! код интерфейса по результату операций (`Change`).

use crate::shell::{Shell, Tab};

pub type WindowId = u32;

/// Окно: главное или дополнительное.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRef {
    Main,
    Extra(WindowId),
}

/// Что изменилось в наборе окон: интерфейс создаёт и закрывает окна по этому списку.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Change {
    pub opened: Option<WindowId>,
    pub closed: Vec<WindowId>,
}

/// Вкладки дополнительного окна.
#[derive(Debug, Default)]
pub struct TabList {
    tabs: Vec<Tab>,
    active: usize,
}

impl TabList {
    fn single(mut tab: Tab) -> Self {
        tab.preview = false;
        Self {
            tabs: vec![tab],
            active: 0,
        }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn insert(&mut self, at: usize, mut tab: Tab) -> usize {
        tab.preview = false;
        let at = at.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
        at
    }

    /// Забирает вкладку. Если закрыли документ, активной становится вкладка списка, из которого
    /// его открыли, иначе соседняя (так же ведёт себя главное окно).
    pub fn take(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let closed = self.tabs.remove(index);
        if self.active > index {
            self.active -= 1;
        } else if self.active == index {
            let parent = closed.doc.and_then(|_| {
                self.tabs
                    .iter()
                    .position(|t| t.node == closed.node && t.doc.is_none())
            });
            self.active = parent.unwrap_or_else(|| index.min(self.tabs.len().saturating_sub(1)));
        }
        Some(closed)
    }

    /// Открывает документ: уже открытый выбирается, новый встаёт рядом с вкладкой своего списка.
    pub fn open_doc(&mut self, node: usize, doc: u32, tab: Tab) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.node == node && t.doc == Some(doc))
        {
            self.active = index;
            return;
        }
        let parent = self
            .tabs
            .iter()
            .position(|t| t.node == node && t.doc.is_none());
        self.insert(parent.map_or(self.tabs.len(), |p| p + 1), tab);
    }

    pub fn set_modified(&mut self, index: usize, modified: bool) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.modified = modified;
        }
    }

    pub fn doc_tab(&self, doc: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.doc == Some(doc))
    }

    pub fn active_doc(&self) -> Option<u32> {
        self.tabs.get(self.active).and_then(|t| t.doc)
    }

    pub fn set_doc_title(&mut self, doc: u32, title: String) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.doc == Some(doc)) {
            tab.title = title;
        }
    }

    pub fn renumber_doc(&mut self, old: u32, new: u32) {
        for tab in &mut self.tabs {
            if tab.doc == Some(old) {
                tab.doc = Some(new);
            }
        }
    }
}

/// Дополнительные окна и активное окно.
#[derive(Debug)]
pub struct Workspace {
    extra: Vec<(WindowId, TabList)>,
    active: WindowRef,
    next: WindowId,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            extra: Vec::new(),
            active: WindowRef::Main,
            next: 0,
        }
    }

    /// Активное окно: в нём открываются новые вкладки и документы.
    pub fn active(&self) -> WindowRef {
        if let WindowRef::Extra(id) = self.active
            && self.list(id).is_none()
        {
            return WindowRef::Main;
        }
        self.active
    }

    pub fn set_active(&mut self, window: WindowRef) {
        self.active = window;
    }

    pub fn extra_ids(&self) -> Vec<WindowId> {
        self.extra.iter().map(|(id, _)| *id).collect()
    }

    pub fn list(&self, id: WindowId) -> Option<&TabList> {
        self.extra.iter().find(|(i, _)| *i == id).map(|(_, l)| l)
    }

    pub fn list_mut(&mut self, id: WindowId) -> Option<&mut TabList> {
        self.extra
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, l)| l)
    }

    /// Окно, в котором открыта вкладка документа.
    pub fn window_of_doc(&self, shell: &Shell, doc: u32) -> Option<WindowRef> {
        if shell.doc_tab(doc).is_some() {
            return Some(WindowRef::Main);
        }
        self.extra
            .iter()
            .find(|(_, l)| l.doc_tab(doc).is_some())
            .map(|(id, _)| WindowRef::Extra(*id))
    }

    fn take(&mut self, shell: &mut Shell, from: WindowRef, index: usize) -> Option<Tab> {
        match from {
            WindowRef::Main => shell.take_tab(index),
            WindowRef::Extra(id) => self.list_mut(id)?.take(index),
        }
    }

    /// Убирает пустые дополнительные окна, возвращает их номера.
    fn sweep(&mut self) -> Vec<WindowId> {
        let closed: Vec<WindowId> = self
            .extra
            .iter()
            .filter(|(_, l)| l.is_empty())
            .map(|(id, _)| *id)
            .collect();
        self.extra.retain(|(_, l)| !l.is_empty());
        closed
    }

    /// Выносит вкладку в новое окно. Единственную вкладку дополнительного окна выносить не
    /// нужно: окно тащится само (`None`).
    pub fn detach(&mut self, shell: &mut Shell, from: WindowRef, index: usize) -> Option<Change> {
        if let WindowRef::Extra(id) = from
            && self.list(id).is_some_and(|l| l.tabs().len() == 1)
        {
            return None;
        }
        let tab = self.take(shell, from, index)?;
        self.next += 1;
        let id = self.next;
        self.extra.push((id, TabList::single(tab)));
        self.active = WindowRef::Extra(id);
        Some(Change {
            opened: Some(id),
            closed: self.sweep(),
        })
    }

    /// Переносит вкладку в другое окно на место `at`; внутри одного окна меняет порядок.
    pub fn move_tab(
        &mut self,
        shell: &mut Shell,
        from: WindowRef,
        index: usize,
        to: WindowRef,
        at: usize,
    ) -> Option<Change> {
        if let WindowRef::Extra(id) = to
            && self.list(id).is_none()
        {
            return None;
        }
        let same = from == to;
        let tab = self.take(shell, from, index)?;
        // После изъятия места правее исходного сдвигаются на единицу.
        let at = if same && at > index { at - 1 } else { at };
        match to {
            WindowRef::Main => {
                shell.insert_tab(at, tab);
            }
            WindowRef::Extra(id) => {
                self.list_mut(id)?.insert(at, tab);
            }
        }
        self.active = to;
        Some(Change {
            opened: None,
            closed: self.sweep(),
        })
    }

    /// Закрывает вкладку дополнительного окна; пустое окно закрывается.
    pub fn close_tab(&mut self, id: WindowId, index: usize) -> Change {
        if let Some(list) = self.list_mut(id) {
            list.take(index);
        }
        Change {
            opened: None,
            closed: self.sweep(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(shell: &Shell) -> Vec<String> {
        shell.tabs().iter().map(|t| t.title.clone()).collect()
    }

    fn extra_titles(ws: &Workspace, id: WindowId) -> Vec<String> {
        ws.list(id)
            .map(|l| l.tabs().iter().map(|t| t.title.clone()).collect())
            .unwrap_or_default()
    }

    /// Оболочка с тремя вкладками: «Главная», список поступлений и документ.
    fn shell_with_tabs() -> Shell {
        let mut shell = Shell::demo();
        shell.click(3);
        shell.open_doc(3, 7, "Поступление № 7".into(), "receipt");
        shell
    }

    #[test]
    fn detach_moves_tab_to_new_window() {
        let mut shell = shell_with_tabs();
        let mut ws = Workspace::new();
        let before = shell.tabs().len();
        let change = ws.detach(&mut shell, WindowRef::Main, 2).unwrap();
        let id = change.opened.unwrap();
        assert_eq!(shell.tabs().len(), before - 1);
        assert_eq!(extra_titles(&ws, id), ["Поступление № 7"]);
        assert_eq!(ws.active(), WindowRef::Extra(id));
        assert!(change.closed.is_empty());
    }

    #[test]
    fn single_tab_window_is_dragged_whole() {
        let mut shell = shell_with_tabs();
        let mut ws = Workspace::new();
        let id = ws
            .detach(&mut shell, WindowRef::Main, 2)
            .unwrap()
            .opened
            .unwrap();
        // Единственную вкладку окна выносить не нужно.
        assert!(ws.detach(&mut shell, WindowRef::Extra(id), 0).is_none());
        assert_eq!(ws.extra_ids(), [id]);
    }

    #[test]
    fn move_back_to_main_closes_empty_window_and_pins_tab() {
        let mut shell = shell_with_tabs();
        let mut ws = Workspace::new();
        let id = ws
            .detach(&mut shell, WindowRef::Main, 2)
            .unwrap()
            .opened
            .unwrap();
        let change = ws
            .move_tab(&mut shell, WindowRef::Extra(id), 0, WindowRef::Main, 1)
            .unwrap();
        assert_eq!(change.closed, [id]);
        assert!(ws.extra_ids().is_empty());
        assert_eq!(titles(&shell)[1], "Поступление № 7");
        assert_eq!(shell.active_tab(), 1);
        assert!(!shell.tabs()[1].preview);
        assert_eq!(ws.active(), WindowRef::Main);
    }

    #[test]
    fn move_between_extra_windows() {
        let mut shell = shell_with_tabs();
        shell.click(5);
        let mut ws = Workspace::new();
        let a = ws
            .detach(&mut shell, WindowRef::Main, 2)
            .unwrap()
            .opened
            .unwrap();
        let b = ws
            .detach(&mut shell, WindowRef::Main, 1)
            .unwrap()
            .opened
            .unwrap();
        ws.move_tab(&mut shell, WindowRef::Extra(b), 0, WindowRef::Extra(a), 0)
            .unwrap();
        assert_eq!(extra_titles(&ws, a).len(), 2);
        assert_eq!(ws.extra_ids(), [a]);
        assert_eq!(ws.active(), WindowRef::Extra(a));
    }

    #[test]
    fn reorder_inside_one_window() {
        let mut shell = shell_with_tabs();
        let before = titles(&shell);
        let mut ws = Workspace::new();
        ws.move_tab(&mut shell, WindowRef::Main, 0, WindowRef::Main, 3)
            .unwrap();
        let after = titles(&shell);
        assert_eq!(after.len(), before.len());
        assert_eq!(after[2], before[0]);
        assert_eq!(shell.active_tab(), 2);
    }

    #[test]
    fn closing_last_tab_removes_extra_window() {
        let mut shell = shell_with_tabs();
        let mut ws = Workspace::new();
        let id = ws
            .detach(&mut shell, WindowRef::Main, 2)
            .unwrap()
            .opened
            .unwrap();
        let change = ws.close_tab(id, 0);
        assert_eq!(change.closed, [id]);
        assert_eq!(ws.active(), WindowRef::Main);
    }

    #[test]
    fn document_opens_next_to_its_list_in_extra_window() {
        let mut shell = shell_with_tabs();
        let mut ws = Workspace::new();
        let id = ws
            .detach(&mut shell, WindowRef::Main, 1)
            .unwrap()
            .opened
            .unwrap();
        let doc = Tab {
            title: "Поступление № 9".into(),
            icon: "doc",
            node: 3,
            view: "receipt",
            doc: Some(9),
            preview: false,
            modified: false,
        };
        let list = ws.list_mut(id).unwrap();
        list.open_doc(3, 9, doc.clone());
        list.open_doc(3, 9, doc);
        assert_eq!(list.tabs().len(), 2);
        assert_eq!(list.tabs()[1].doc, Some(9));
        assert_eq!(list.active(), 1);
        assert_eq!(ws.window_of_doc(&shell, 9), Some(WindowRef::Extra(id)));
    }
}
