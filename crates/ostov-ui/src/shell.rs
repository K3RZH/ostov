//! Состояние оболочки главного окна: дерево навигации и вкладки. Без интерфейса и без ядра:
//! дерево строится из метаданных базы (`Shell::from_meta`).

use std::collections::HashSet;

use ostov_api::{MenuItem, Meta, ObjectKind, SystemRights};

/// Значки, которые есть в интерфейсе (`assets/icons`). Имя значка приходит из конфигурации:
/// незнакомое заменяется значком по виду пункта, а не пустым местом.
const KNOWN_ICONS: [&str; 18] = [
    "bar-chart",
    "box",
    "calendar",
    "database",
    "document",
    "folder",
    "home",
    "inventory",
    "list",
    "recipe",
    "server",
    "settings",
    "stock-in",
    "stock-out",
    "transfer",
    "user",
    "users",
    "warehouse",
];

/// Узел дерева навигации.
pub struct NavNode {
    pub title: String,
    pub icon: &'static str,
    pub parent: Option<usize>,
    pub is_group: bool,
    /// Ссылка на основной узел: избранное открывает ту же вкладку, что и основной пункт.
    pub link: Option<usize>,
    pub favorite: bool,
    /// Какой экран показывать во вкладке (пусто: экран ещё не сделан).
    pub view: &'static str,
    /// Объект конфигурации, который открывает пункт (имя в ядре).
    pub object: Option<String>,
}

/// Строка дерева для показа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRowData {
    pub id: usize,
    pub title: String,
    pub icon: &'static str,
    pub level: usize,
    pub is_group: bool,
    pub expanded: bool,
    pub selected: bool,
    pub favorite: bool,
}

/// Открытая вкладка: пункт навигации или документ в нём (`doc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub title: String,
    pub icon: &'static str,
    pub node: usize,
    pub view: &'static str,
    /// Номер документа, если вкладка открывает документ, а не весь пункт.
    pub doc: Option<u32>,
    /// Вкладка предпросмотра: следующее открытие без изменений заменит её.
    pub preview: bool,
    /// В документе есть несохранённые изменения; такая вкладка закреплена.
    pub modified: bool,
}

pub struct Shell {
    nodes: Vec<NavNode>,
    expanded: HashSet<usize>,
    tabs: Vec<Tab>,
    active: usize,
}

fn node(title: &str, icon: &'static str, parent: Option<usize>, is_group: bool) -> NavNode {
    NavNode {
        title: title.to_owned(),
        icon,
        parent,
        is_group,
        link: None,
        favorite: false,
        view: "",
        object: None,
    }
}

fn known_icon(name: Option<&str>, fallback: &'static str) -> &'static str {
    name.and_then(|name| KNOWN_ICONS.iter().copied().find(|known| *known == name))
        .unwrap_or(fallback)
}

/// Экран вкладки объекта. Свой экран пока есть только у поступлений (`receipts_ui.rs`,
/// временный); остальные объекты получат универсальный список (шаг A3 плана).
fn view_for(object: &str) -> &'static str {
    if object == "receipt" { "receipts" } else { "" }
}

/// Экраны «Администрирования» (название, значок, вид вкладки), которые доступны с такими правами.
/// Ядро проверяет право на каждой команде; здесь пункт только не показывается, чтобы не вести
/// в экран, который ответит отказом.
fn admin_screens(rights: SystemRights) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut screens = Vec::new();
    if rights.admin {
        screens.push(("Пользователи и роли", "users", "users"));
        screens.push(("Журнал действий", "list", "journal"));
    }
    if rights.set_closing_date {
        screens.push(("Закрытие периода", "calendar", "period"));
    }
    screens
}

/// Добавляет пункт меню и его детей. Глубину ограничивает формат YAML (раздел → группа → объект).
fn push_menu(nodes: &mut Vec<NavNode>, meta: &Meta, item: &MenuItem, parent: Option<usize>) {
    let id = nodes.len();
    let object = item
        .object
        .as_deref()
        .and_then(|name| meta.objects.iter().find(|object| object.name == name));
    let fallback = match object.map(|object| &object.kind) {
        Some(ObjectKind::Catalog) => "list",
        Some(ObjectKind::Document) => "document",
        None => "folder",
    };
    let icon = item
        .icon
        .as_deref()
        .or_else(|| object.and_then(|object| object.icon.as_deref()));
    let title = match object {
        Some(object) if item.title.trim().is_empty() => object.title.clone(),
        _ => item.title.clone(),
    };
    nodes.push(NavNode {
        title,
        icon: known_icon(icon, fallback),
        parent,
        is_group: item.object.is_none(),
        link: None,
        favorite: false,
        view: item.object.as_deref().map_or("", view_for),
        object: item.object.clone(),
    });
    for child in &item.children {
        push_menu(nodes, meta, child, Some(id));
    }
}

impl Shell {
    /// Навигация из метаданных базы: «Главная», меню конфигурации (ядро уже убрало пункты, на
    /// которые у пользователя нет прав) и «Администрирование» с теми экранами, на которые есть
    /// системное право. Разделы верхнего уровня раскрыты.
    pub fn from_meta(meta: &Meta, rights: SystemRights) -> Self {
        let mut nodes = vec![node("Главная", "home", None, false)];
        let mut expanded = HashSet::new();
        for item in &meta.menu {
            if item.object.is_none() {
                expanded.insert(nodes.len());
            }
            push_menu(&mut nodes, meta, item, None);
        }
        let screens = admin_screens(rights);
        if !screens.is_empty() {
            let group = nodes.len();
            expanded.insert(group);
            nodes.push(node("Администрирование", "settings", None, true));
            for (title, icon, view) in screens {
                let mut item = node(title, icon, Some(group), false);
                item.view = view;
                nodes.push(item);
            }
        }
        Self::new(nodes, expanded)
    }

    /// Только «Главная»: до открытия базы.
    pub fn empty() -> Self {
        Self::new(vec![node("Главная", "home", None, false)], HashSet::new())
    }
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

    /// Примерная навигация из макета «Главное окно»: только для тестов и стендов
    /// (`examples/`), приложение строит дерево из метаданных.
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
        nodes[3].view = "receipts";
        nodes[3].object = Some("receipt".to_owned());
        nodes[4].object = Some("issue".to_owned());
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
        self.nav_rows_for(self.selected_node())
    }

    /// То же для окна, у которого выбран другой узел (дополнительные окна показывают дерево
    /// навигации с выбором по своей активной вкладке).
    pub fn nav_rows_for(&self, selected: Option<usize>) -> Vec<NavRowData> {
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
                    title: node.title.clone(),
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

    /// Пункт навигации, где живут документы этого вида (имя объекта в ядре).
    pub fn node_for_object(&self, object: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| node.object.as_deref() == Some(object))
    }

    pub fn is_group(&self, id: usize) -> bool {
        self.nodes.get(id).is_some_and(|node| node.is_group)
    }

    /// Раскрывает или сворачивает группу дерева (общее для всех окон).
    pub fn toggle_group(&mut self, id: usize) {
        if self.is_group(id) && !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
    }

    /// Вкладка для пункта дерева (избранное ведёт на основной пункт); у группы вкладки нет.
    pub fn node_tab(&self, id: usize) -> Option<Tab> {
        let node = self.nodes.get(id).filter(|node| !node.is_group)?;
        let target = node.link.unwrap_or(id);
        let node = &self.nodes[target];
        Some(Tab {
            title: node.title.to_owned(),
            icon: node.icon,
            node: target,
            view: node.view,
            doc: None,
            preview: false,
            modified: false,
        })
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
        let (title, view) = (self.nodes[id].title.clone(), self.nodes[id].view);
        self.open_tab(id, None, title, view, None);
    }

    /// Открывает вкладку. Уже открытая просто выбирается. Иначе: если есть вкладка
    /// предпросмотра без изменений, новая занимает её место (число вкладок не растёт);
    /// если все вкладки закреплены или изменены, добавляется новая вкладка предпросмотра.
    ///
    /// `after`: вкладка, рядом с которой встанет новая (родитель, из которого её открыли).
    fn open_tab(
        &mut self,
        node: usize,
        doc: Option<u32>,
        title: String,
        view: &'static str,
        after: Option<usize>,
    ) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.node == node && tab.doc == doc)
        {
            self.active = index;
            return;
        }
        let tab = Tab {
            title,
            icon: self.nodes[node].icon,
            node,
            view,
            doc,
            // «Главная» всегда остаётся.
            preview: node != 0,
            modified: false,
        };
        match self.tabs.iter().position(|t| t.preview && !t.modified) {
            Some(index) => {
                self.tabs[index] = tab;
                self.active = index;
            }
            None => {
                let at = after.map_or(self.tabs.len(), |parent| parent + 1);
                self.tabs.insert(at, tab);
                self.active = at;
            }
        }
    }

    /// Открывает документ пункта `node` в своей вкладке. Если документ открыт из списка этого
    /// пункта, вкладка списка закрепляется (документ её не заменяет) и документ встаёт рядом.
    pub fn open_doc(&mut self, node: usize, doc: u32, title: String, view: &'static str) {
        let parent = self
            .tabs
            .iter()
            .position(|tab| tab.node == node && tab.doc.is_none());
        if let Some(parent) = parent {
            self.tabs[parent].preview = false;
        }
        self.open_tab(node, Some(doc), title, view, parent);
    }

    /// Отмечает вкладку изменённой: она закрепляется и больше не заменяется.
    pub fn set_modified(&mut self, index: usize, modified: bool) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.modified = modified;
            if modified {
                tab.preview = false;
            }
        }
    }

    /// Закрепляет вкладку: следующее открытие её не заменит.
    pub fn pin_tab(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.preview = false;
        }
    }

    /// Ядро назначило документу другой номер: вкладка следует за ним.
    pub fn renumber_doc(&mut self, old: u32, new: u32) {
        for tab in &mut self.tabs {
            if tab.doc == Some(old) {
                tab.doc = Some(new);
            }
        }
    }

    pub fn doc_tab(&self, doc: u32) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.doc == Some(doc))
    }

    pub fn active_doc(&self) -> Option<u32> {
        self.tabs.get(self.active).and_then(|tab| tab.doc)
    }

    pub fn set_doc_title(&mut self, doc: u32, title: String) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.doc == Some(doc)) {
            tab.title = title;
        }
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    /// Забирает вкладку из окна (для переноса в другое окно); активная вкладка выбирается как при закрытии.
    pub fn take_tab(&mut self, index: usize) -> Option<Tab> {
        let tab = self.tabs.get(index)?.clone();
        self.close_tab(index);
        Some(tab)
    }

    /// Вставляет перенесённую вкладку и делает её активной. Перенос это осознанное действие,
    /// поэтому вкладка закреплена: следующее открытие её не заменит.
    pub fn insert_tab(&mut self, at: usize, mut tab: Tab) -> usize {
        tab.preview = false;
        let at = at.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
        at
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let closed = self.tabs.remove(index);
        if self.active > index {
            self.active -= 1;
        } else if self.active == index {
            // Закрыли документ: возвращаемся к списку, из которого его открыли. Иначе встаёт
            // соседняя вкладка.
            let parent = closed.doc.and_then(|_| {
                self.tabs
                    .iter()
                    .position(|t| t.node == closed.node && t.doc.is_none())
            });
            self.active = parent.unwrap_or_else(|| index.min(self.tabs.len().saturating_sub(1)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(shell: &Shell) -> Vec<usize> {
        shell.nav_rows().iter().map(|row| row.id).collect()
    }

    fn warehouse_meta() -> Meta {
        let config = ostov_meta::demo();
        assert!(config.is_ok(), "{:?}", config.as_ref().err());
        Meta::new(&config.unwrap_or_else(|_| unreachable!()))
    }

    fn titles(shell: &Shell) -> Vec<String> {
        shell.nav_rows().into_iter().map(|row| row.title).collect()
    }

    #[test]
    fn menu_comes_from_configuration() {
        let meta = warehouse_meta();
        let shell = Shell::from_meta(&meta, SystemRights::default());
        let titles = titles(&shell);
        assert_eq!(titles.first().map(String::as_str), Some("Главная"));
        for object in &meta.objects {
            assert!(
                titles.contains(&object.title),
                "нет пункта «{}»",
                object.title
            );
        }
        assert!(!titles.iter().any(|title| title == "Администрирование"));
    }

    #[test]
    fn admin_sees_administration() {
        let shell = Shell::from_meta(&warehouse_meta(), SystemRights::ALL);
        let titles = titles(&shell);
        for title in [
            "Администрирование",
            "Пользователи и роли",
            "Журнал действий",
            "Закрытие периода",
        ] {
            assert!(titles.iter().any(|t| t == title), "нет пункта «{title}»");
        }
    }

    #[test]
    fn administration_items_follow_rights() {
        let only = |rights: SystemRights| titles(&Shell::from_meta(&warehouse_meta(), rights));
        // Журнал и пользователи идут с правом администратора, закрытие периода с собственным.
        let admin = only(SystemRights {
            admin: true,
            ..SystemRights::default()
        });
        assert!(admin.iter().any(|t| t == "Журнал действий"));
        assert!(!admin.iter().any(|t| t == "Закрытие периода"));
        let accountant = only(SystemRights {
            set_closing_date: true,
            ..SystemRights::default()
        });
        assert!(accountant.iter().any(|t| t == "Администрирование"));
        assert!(accountant.iter().any(|t| t == "Закрытие периода"));
        assert!(!accountant.iter().any(|t| t == "Журнал действий"));
        assert!(!accountant.iter().any(|t| t == "Пользователи и роли"));
        // Права, не связанные с этими экранами, раздела не открывают.
        let editor = only(SystemRights {
            closed_period: true,
            update_config: true,
            ..SystemRights::default()
        });
        assert!(!editor.iter().any(|t| t == "Администрирование"));
    }

    #[test]
    fn administration_items_open_their_screens() {
        let mut shell = Shell::from_meta(&warehouse_meta(), SystemRights::ALL);
        for (title, view) in [
            ("Пользователи и роли", "users"),
            ("Журнал действий", "journal"),
            ("Закрытие периода", "period"),
        ] {
            let id = shell
                .nav_rows()
                .iter()
                .find(|row| row.title == title)
                .map(|row| row.id);
            assert!(id.is_some(), "нет пункта «{title}»");
            if let Some(id) = id {
                shell.click(id);
                assert_eq!(shell.tabs().last().map(|tab| tab.view), Some(view));
            }
        }
    }

    #[test]
    fn administration_icons_are_known() {
        let all = SystemRights::ALL;
        for (title, icon, _) in admin_screens(all) {
            assert!(KNOWN_ICONS.contains(&icon), "значок «{icon}» у «{title}»");
        }
    }

    #[test]
    fn object_finds_its_menu_item() {
        let mut shell = Shell::from_meta(&warehouse_meta(), SystemRights::default());
        let receipts = shell.node_for_object("receipt");
        assert!(receipts.is_some());
        assert_eq!(shell.node_for_object("нет такого"), None);
        if let Some(id) = receipts {
            shell.click(id);
            assert_eq!(shell.tabs().last().map(|tab| tab.view), Some("receipts"));
        }
    }

    #[test]
    fn unknown_icon_falls_back_to_kind() {
        assert_eq!(known_icon(Some("rm -rf"), "document"), "document");
        assert_eq!(known_icon(Some("warehouse"), "folder"), "warehouse");
        assert_eq!(known_icon(None, "list"), "list");
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
    fn opening_without_changes_replaces_preview_tab() {
        let mut shell = Shell::demo();
        shell.click(3);
        shell.click(4);
        shell.click(3);
        // «Главная» и одна вкладка предпросмотра, которую заменяют.
        assert_eq!(shell.tabs().len(), 2);
        assert_eq!(shell.tabs()[shell.active_tab()].node, 3);
    }

    #[test]
    fn modified_tab_is_kept_and_next_opens_new_one() {
        let mut shell = Shell::demo();
        shell.click(3);
        shell.set_modified(1, true);
        shell.click(4);
        assert_eq!(shell.tabs().len(), 3);
        assert!(shell.tabs()[1].modified && !shell.tabs()[1].preview);
        assert!(shell.tabs()[2].preview);
        // Новая вкладка без изменений снова заменяется.
        shell.click(5);
        assert_eq!(shell.tabs().len(), 3);
    }

    #[test]
    fn document_opened_from_list_keeps_the_list_tab() {
        let mut shell = Shell::demo();
        shell.click(3);
        shell.open_doc(3, 1998, "Поступление № 1998".into(), "receipt");
        // «Главная», список (закреплён) и документ рядом с ним.
        assert_eq!(shell.tabs().len(), 3);
        assert!(!shell.tabs()[1].preview);
        assert_eq!(shell.active_doc(), Some(1998));
        // Другой документ без изменений заменяет первый, а не добавляет вкладку.
        shell.open_doc(3, 1997, "Поступление № 1997".into(), "receipt");
        assert_eq!(shell.tabs().len(), 3);
        // Закрытие документа возвращает к списку.
        shell.close_tab(2);
        assert_eq!(shell.tabs().len(), 2);
        assert_eq!(shell.tabs()[shell.active_tab()].node, 3);
        assert_eq!(shell.active_doc(), None);
    }

    #[test]
    fn same_document_is_selected_not_duplicated() {
        let mut shell = Shell::demo();
        shell.open_doc(3, 1998, "Поступление № 1998".into(), "receipt");
        shell.open_doc(3, 1998, "Поступление № 1998".into(), "receipt");
        assert_eq!(shell.tabs().len(), 2);
        assert_eq!(shell.active_doc(), Some(1998));
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
        shell.set_modified(1, true);
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
