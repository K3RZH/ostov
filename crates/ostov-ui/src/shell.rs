//! Состояние оболочки главного окна: дерево навигации и вкладки. Без интерфейса и без ядра.

use std::collections::HashSet;

/// Узел дерева навигации. Состав сейчас примерный (из макета), позже его даст конфигурация.
pub struct NavNode {
    pub title: &'static str,
    pub icon: &'static str,
    pub parent: Option<usize>,
    pub is_group: bool,
    /// Ссылка на основной узел: избранное открывает ту же вкладку, что и основной пункт.
    pub link: Option<usize>,
    pub favorite: bool,
    /// Какой экран показывать во вкладке (пусто: заглушка «появится после ядра»).
    pub view: &'static str,
}

/// Строка дерева для показа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRowData {
    pub id: usize,
    pub title: &'static str,
    pub icon: &'static str,
    pub level: usize,
    pub is_group: bool,
    pub expanded: bool,
    pub selected: bool,
    pub favorite: bool,
}

/// Открытая вкладка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub title: &'static str,
    pub icon: &'static str,
    pub node: usize,
    pub view: &'static str,
}

pub struct Shell {
    nodes: Vec<NavNode>,
    expanded: HashSet<usize>,
    tabs: Vec<Tab>,
    active: usize,
}

fn node(title: &'static str, icon: &'static str, parent: Option<usize>, is_group: bool) -> NavNode {
    NavNode {
        title,
        icon,
        parent,
        is_group,
        link: None,
        favorite: false,
        view: "",
    }
}

impl Shell {
    pub fn new(nodes: Vec<NavNode>, expanded: HashSet<usize>) -> Self {
        let mut shell = Self {
            nodes,
            expanded,
            tabs: Vec::new(),
            active: 0,
        };
        shell.open(0);
        shell
    }

    /// Примерная навигация из макета «Главное окно».
    pub fn demo() -> Self {
        let mut nodes = vec![
            node("Главная", "home", None, false),                    // 0
            node("Избранное", "star", None, true),                   // 1
            node("Склад", "warehouse", None, true),                  // 2
            node("Поступления товаров", "stock-in", Some(2), false), // 3
            node("Расход и списание", "stock-out", Some(2), false),  // 4
            node("Перемещения", "transfer", Some(2), false),         // 5
            node("Инвентаризация", "inventory", Some(2), true),      // 6
            node("Акты инвентаризации", "document", Some(6), false), // 7
            node("Расхождения", "document", Some(6), false),         // 8
            node("Справочники", "folder", None, true),               // 9
            node("Номенклатура", "box", Some(9), false),             // 10
            node("Контрагенты", "users", Some(9), false),            // 11
            node("Склады", "warehouse", Some(9), false),             // 12
            node("Рецептуры", "recipe", None, false),                // 13
            node("Отчёты", "bar-chart", None, true),                 // 14
            node("Остатки и обороты", "bar-chart", Some(14), false), // 15
            node("Администрирование", "settings", None, true),       // 16
            node("Пользователи и роли", "users", Some(16), false),   // 17
        ];
        nodes[17].view = "users";
        for (title, icon, target) in [
            ("Поступления товаров", "stock-in", 3),
            ("Остатки и обороты", "bar-chart", 15),
        ] {
            let mut favorite = node(title, icon, Some(1), false);
            favorite.link = Some(target);
            favorite.favorite = true;
            nodes.push(favorite);
        }
        Self::new(nodes, HashSet::from([1, 2]))
    }

    fn depth(&self, id: usize) -> usize {
        let mut level = 0;
        let mut current = self.nodes[id].parent;
        while let Some(parent) = current {
            level += 1;
            current = self.nodes[parent].parent;
        }
        level
    }

    fn is_visible(&self, id: usize) -> bool {
        let mut current = self.nodes[id].parent;
        while let Some(parent) = current {
            if !self.expanded.contains(&parent) {
                return false;
            }
            current = self.nodes[parent].parent;
        }
        true
    }

    fn selected_node(&self) -> Option<usize> {
        self.tabs.get(self.active).map(|tab| tab.node)
    }

    /// Узлы в порядке показа: каждый узел сразу перед своими потомками.
    fn tree_order(&self, parent: Option<usize>, out: &mut Vec<usize>) {
        for (id, node) in self.nodes.iter().enumerate() {
            if node.parent == parent {
                out.push(id);
                self.tree_order(Some(id), out);
            }
        }
    }

    /// Видимые строки дерева (свёрнутые группы скрывают потомков).
    pub fn nav_rows(&self) -> Vec<NavRowData> {
        let selected = self.selected_node();
        let mut order = Vec::with_capacity(self.nodes.len());
        self.tree_order(None, &mut order);
        order
            .into_iter()
            .filter(|&id| self.is_visible(id))
            .map(|id| {
                let node = &self.nodes[id];
                let target = node.link.unwrap_or(id);
                NavRowData {
                    id,
                    title: node.title,
                    icon: node.icon,
                    level: self.depth(id),
                    is_group: node.is_group,
                    expanded: self.expanded.contains(&id),
                    selected: !node.is_group && selected == Some(target),
                    favorite: node.favorite,
                }
            })
            .collect()
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_tab(&self) -> usize {
        self.active
    }

    /// Клик по строке дерева: группа раскрывается или сворачивается, пункт открывает вкладку.
    pub fn click(&mut self, id: usize) {
        let Some(node) = self.nodes.get(id) else {
            return;
        };
        if node.is_group {
            if !self.expanded.remove(&id) {
                self.expanded.insert(id);
            }
        } else {
            self.open(node.link.unwrap_or(id));
        }
    }

    fn open(&mut self, id: usize) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.node == id) {
            self.active = index;
            return;
        }
        let node = &self.nodes[id];
        self.tabs.push(Tab {
            title: node.title,
            icon: node.icon,
            node: id,
            view: node.view,
        });
        self.active = self.tabs.len() - 1;
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.active > index || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(shell: &Shell) -> Vec<usize> {
        shell.nav_rows().iter().map(|row| row.id).collect()
    }

    #[test]
    fn collapsed_group_hides_children() {
        let mut shell = Shell::demo();
        assert!(ids(&shell).contains(&3));
        shell.click(2);
        assert!(!ids(&shell).contains(&3));
        shell.click(2);
        assert!(ids(&shell).contains(&3));
    }

    #[test]
    fn children_follow_their_parent() {
        let shell = Shell::demo();
        let order = ids(&shell);
        let group = order.iter().position(|&id| id == 1);
        assert_eq!(group.map(|i| order[i + 1]), Some(18));
        assert_eq!(group.map(|i| order[i + 2]), Some(19));
    }

    #[test]
    fn nested_group_needs_all_ancestors_expanded() {
        let mut shell = Shell::demo();
        assert!(!ids(&shell).contains(&7));
        shell.click(6);
        assert!(ids(&shell).contains(&7));
        shell.click(2);
        assert!(!ids(&shell).contains(&7));
    }

    #[test]
    fn opening_same_item_does_not_duplicate_tab() {
        let mut shell = Shell::demo();
        shell.click(3);
        shell.click(4);
        shell.click(3);
        assert_eq!(shell.tabs().len(), 3);
        assert_eq!(shell.tabs()[shell.active_tab()].node, 3);
    }

    #[test]
    fn favorite_opens_the_main_item_tab() {
        let mut shell = Shell::demo();
        shell.click(3);
        let tabs = shell.tabs().len();
        shell.click(18);
        assert_eq!(shell.tabs().len(), tabs);
        assert!(
            shell
                .nav_rows()
                .iter()
                .any(|row| row.id == 18 && row.selected)
        );
    }

    #[test]
    fn closing_tabs_keeps_active_valid() {
        let mut shell = Shell::demo();
        shell.click(3);
        shell.click(4);
        shell.close_tab(2);
        assert_eq!(shell.active_tab(), 1);
        shell.close_tab(0);
        assert_eq!(shell.active_tab(), 0);
        shell.close_tab(0);
        assert!(shell.tabs().is_empty());
        assert_eq!(shell.active_tab(), 0);
    }
}
