//! Конфигуратор: данные для окна «Ostov Конфигуратор» из метаданных конфигурации.
//!
//! Чистая логика без интерфейса: дерево по видам объектов, выбранный элемент, разделы редактора,
//! таблицы полей и результат проверки. Окно берёт готовые значения у `Browser`
//! (src/configurator_ui.rs). Конфигурация читается из папки и здесь не меняется: правят её
//! в редакторе (решение на R1). Что задумано в конфигураторе: документ «Состав конфигурации»,
//! раздел 11.

use std::collections::HashSet;
use std::path::Path;

use ostov_api::{
    FieldDef, FieldKind, MenuItem, Meta, ObjectKind, ObjectMeta, RegisterMeta, RoleInfo,
};

use crate::EditorSection;

/// Виды элементов дерева: только те, что есть в метаданных (`Meta`). Отчёты, перечисления и
/// константы появятся в дереве вместе с их поддержкой в ядре.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementKind {
    Catalog,
    Document,
    Register,
    Role,
    Subsystem,
}

/// Все разделы редактора: у справочника и документа есть всё.
const ALL_SECTIONS: [EditorSection; 9] = [
    EditorSection::General,
    EditorSection::Fields,
    EditorSection::TableParts,
    EditorSection::Forms,
    EditorSection::Print,
    EditorSection::Rights,
    EditorSection::Events,
    EditorSection::Numbering,
    EditorSection::ImportExport,
];

impl ElementKind {
    /// Порядок групп в дереве.
    pub const ALL: [Self; 5] = [
        Self::Catalog,
        Self::Document,
        Self::Register,
        Self::Role,
        Self::Subsystem,
    ];

    /// Название группы в дереве.
    pub fn group_title(self) -> &'static str {
        match self {
            Self::Catalog => "Справочники",
            Self::Document => "Документы",
            Self::Register => "Регистры",
            Self::Role => "Роли",
            Self::Subsystem => "Подсистемы",
        }
    }

    /// Вид элемента для строки «Вид» и метки в заголовке.
    pub fn title(self) -> &'static str {
        match self {
            Self::Catalog => "Справочник",
            Self::Document => "Документ",
            Self::Register => "Регистр",
            Self::Role => "Роль",
            Self::Subsystem => "Подсистема",
        }
    }

    /// Значок по умолчанию, если в метаданных своего нет.
    fn icon(self) -> &'static str {
        match self {
            Self::Catalog => "list",
            Self::Document => "document",
            Self::Register => "bar-chart",
            Self::Role => "users",
            Self::Subsystem => "home",
        }
    }

    /// Разделы редактора, которые имеют смысл для вида. Табличные части у роли или формы у
    /// регистра были бы пустым обещанием, поэтому их в списке нет.
    pub fn sections(self) -> &'static [EditorSection] {
        match self {
            Self::Catalog | Self::Document => &ALL_SECTIONS,
            Self::Register => &[
                EditorSection::General,
                EditorSection::Fields,
                EditorSection::Rights,
                EditorSection::Events,
            ],
            Self::Role => &[EditorSection::General, EditorSection::Rights],
            Self::Subsystem => &[EditorSection::General],
        }
    }
}

/// Элемент конфигурации: вид и номер среди элементов этого вида (в порядке метаданных).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element {
    pub kind: ElementKind,
    pub index: usize,
}

/// Что именно найдено в метаданных по `Element`.
enum Found<'a> {
    Object(&'a ObjectMeta),
    Register(&'a RegisterMeta),
    Role(&'a RoleInfo),
    Subsystem(&'a MenuItem),
}

/// Конфигурация, которую показывает окно: название и версия из манифеста и метаданные.
pub struct Configuration {
    pub title: String,
    pub version: String,
    pub meta: Meta,
}

impl Configuration {
    /// Конфигурация из папки `dir`: те же чтение и проверка, что у `ostov check`.
    ///
    /// # Errors
    /// Ошибки разбора или проверки с файлом и строкой; их показывает панель «Проверка».
    pub fn load(dir: &Path) -> Result<Self, Vec<ostov_meta::Error>> {
        let config = ostov_meta::load(dir)?;
        Ok(Self {
            meta: Meta::new(&config),
            title: config.title,
            version: config.version,
        })
    }

    /// Пустая конфигурация: папка не открыта или не прочиталась.
    pub fn empty() -> Self {
        Self {
            title: String::new(),
            version: String::new(),
            meta: Meta {
                menu: Vec::new(),
                objects: Vec::new(),
                roles: Vec::new(),
                registers: Vec::new(),
                user_fields: Vec::new(),
            },
        }
    }

    /// Сколько элементов этого вида в метаданных.
    pub fn count(&self, kind: ElementKind) -> usize {
        match kind {
            ElementKind::Catalog => self.objects(ObjectKind::Catalog).count(),
            ElementKind::Document => self.objects(ObjectKind::Document).count(),
            ElementKind::Register => self.meta.registers.len(),
            ElementKind::Role => self.meta.roles.len(),
            ElementKind::Subsystem => self.meta.menu.len(),
        }
    }

    fn objects(&self, kind: ObjectKind) -> impl Iterator<Item = &ObjectMeta> {
        self.meta.objects.iter().filter(move |o| o.kind == kind)
    }

    fn find(&self, element: Element) -> Option<Found<'_>> {
        let index = element.index;
        match element.kind {
            ElementKind::Catalog => self
                .objects(ObjectKind::Catalog)
                .nth(index)
                .map(Found::Object),
            ElementKind::Document => self
                .objects(ObjectKind::Document)
                .nth(index)
                .map(Found::Object),
            ElementKind::Register => self.meta.registers.get(index).map(Found::Register),
            ElementKind::Role => self.meta.roles.get(index).map(Found::Role),
            ElementKind::Subsystem => self.meta.menu.get(index).map(Found::Subsystem),
        }
    }

    /// Заголовок элемента для дерева и редактора.
    pub fn title_of(&self, element: Element) -> Option<&str> {
        Some(match self.find(element)? {
            Found::Object(object) => &object.title,
            Found::Register(register) => &register.title,
            Found::Role(role) => &role.title,
            Found::Subsystem(menu) => &menu.title,
        })
    }

    /// Имя-идентификатор. У подсистемы в метаданных его нет: пункт меню знает только заголовок.
    pub fn name_of(&self, element: Element) -> Option<&str> {
        match self.find(element)? {
            Found::Object(object) => Some(&object.name),
            Found::Register(register) => Some(&register.name),
            Found::Role(role) => Some(&role.name),
            Found::Subsystem(_) => None,
        }
    }

    /// Значок, который задан в метаданных (без значка по умолчанию).
    fn explicit_icon(&self, element: Element) -> Option<&str> {
        match self.find(element)? {
            Found::Object(object) => object.icon.as_deref(),
            Found::Subsystem(menu) => menu.icon.as_deref(),
            Found::Register(_) | Found::Role(_) => None,
        }
    }

    /// Значок для дерева: свой из метаданных или по умолчанию для вида.
    pub fn icon_of(&self, element: Element) -> &str {
        self.explicit_icon(element)
            .unwrap_or_else(|| element.kind.icon())
    }

    /// Строки дерева: группы по видам со счётчиками (пустые тоже, с нулём) и под раскрытой
    /// группой её элементы.
    pub fn tree_rows(
        &self,
        expanded: &HashSet<ElementKind>,
        selected: Option<Element>,
    ) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for kind in ElementKind::ALL {
            let count = self.count(kind);
            let open = expanded.contains(&kind);
            rows.push(TreeRow {
                title: format!("{} ({count})", kind.group_title()),
                icon: "folder".to_owned(),
                level: 0,
                is_group: true,
                expanded: open,
                selected: false,
                target: Target::Group(kind),
            });
            if !open {
                continue;
            }
            for index in 0..count {
                let element = Element { kind, index };
                rows.push(TreeRow {
                    title: self.title_of(element).unwrap_or_default().to_owned(),
                    icon: self.icon_of(element).to_owned(),
                    level: 1,
                    is_group: false,
                    expanded: false,
                    selected: selected == Some(element),
                    target: Target::Element(element),
                });
            }
        }
        rows
    }

    /// Раздел «Общие»: свойства, которые есть у элемента в метаданных.
    pub fn general(&self, element: Element) -> Vec<Detail> {
        let Some(title) = self.title_of(element) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if let Some(name) = self.name_of(element) {
            rows.push(Detail::new("Имя (идентификатор)", name));
        }
        rows.push(Detail::new("Заголовок", title));
        rows.push(Detail::new("Вид", element.kind.title()));
        if let Some(icon) = self.explicit_icon(element) {
            rows.push(Detail::new("Значок", icon));
        }
        if let Some(Found::Subsystem(menu)) = self.find(element) {
            let mut titles = Vec::new();
            leaf_titles(menu, &mut titles);
            let composition = if titles.is_empty() {
                "нет пунктов".to_owned()
            } else {
                titles.join(", ")
            };
            rows.push(Detail::new("Состав", &composition));
        }
        rows
    }

    /// Таблица полей элемента. У роли и подсистемы полей нет.
    pub fn fields_table(&self, element: Element) -> Option<Table> {
        match self.find(element)? {
            Found::Object(object) => Some(fields_table(&object.fields)),
            Found::Register(register) => Some(register_table(register)),
            Found::Role(_) | Found::Subsystem(_) => None,
        }
    }

    /// Названия табличных частей элемента по порядку.
    pub fn part_titles(&self, element: Element) -> Vec<String> {
        match self.find(element) {
            Some(Found::Object(object)) => object
                .table_parts
                .iter()
                .map(|part| part.title.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Таблица полей табличной части `part` (номер по порядку).
    pub fn part_table(&self, element: Element, part: usize) -> Option<Table> {
        match self.find(element)? {
            Found::Object(object) => object
                .table_parts
                .get(part)
                .map(|part| fields_table(&part.fields)),
            _ => None,
        }
    }
}

/// Заголовки пунктов меню, которые открывают объекты; группы пунктов раскрываются.
fn leaf_titles<'a>(item: &'a MenuItem, titles: &mut Vec<&'a str>) {
    for child in &item.children {
        if child.object.is_some() {
            titles.push(&child.title);
        } else {
            leaf_titles(child, titles);
        }
    }
}

/// Тип поля так, как он записан в файле конфигурации: `int`, `decimal(2)`, `ref(goods)`.
fn type_text(field: &FieldDef) -> String {
    match field.kind {
        FieldKind::Int => "int".to_owned(),
        FieldKind::Text => "text".to_owned(),
        FieldKind::Date => "date".to_owned(),
        FieldKind::Decimal { scale } => format!("decimal({scale})"),
        FieldKind::Ref => format!("ref({})", field.target.as_deref().unwrap_or_default()),
    }
}

// Начальные ширины колонок, px. Их сумма с полосой прокрутки (456 + 14) помещается в самое
// узкое окно: при 960 px на таблицу остаётся 484 px после дерева, списка разделов и отступов.
const NAME_WIDTH: f32 = 104.0;
const TITLE_WIDTH: f32 = 100.0;
const TYPE_WIDTH: f32 = 140.0;
const FLAG_WIDTH: f32 = 112.0;

fn fields_table(fields: &[FieldDef]) -> Table {
    Table {
        columns: vec![
            Column::new("Имя", NAME_WIDTH),
            Column::flex("Заголовок", TITLE_WIDTH),
            Column::new("Тип", TYPE_WIDTH),
            Column::new("Обязательное", FLAG_WIDTH),
        ],
        rows: fields
            .iter()
            .map(|field| {
                vec![
                    field.name.clone(),
                    field.title.clone(),
                    type_text(field),
                    if field.required { "Да" } else { "Нет" }.to_owned(),
                ]
            })
            .collect(),
    }
}

/// Измерения и ресурсы регистра одной таблицей, роль поля отдельной колонкой.
fn register_table(register: &RegisterMeta) -> Table {
    let rows = register
        .dimensions
        .iter()
        .map(|field| (field, "Измерение"))
        .chain(register.resources.iter().map(|field| (field, "Ресурс")))
        .map(|(field, role)| {
            vec![
                field.name.clone(),
                field.title.clone(),
                role.to_owned(),
                type_text(field),
            ]
        })
        .collect();
    Table {
        columns: vec![
            Column::new("Имя", NAME_WIDTH),
            Column::flex("Заголовок", TITLE_WIDTH),
            Column::new("Роль", FLAG_WIDTH),
            Column::new("Тип", TYPE_WIDTH),
        ],
        rows,
    }
}

/// Строка дерева для показа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub title: String,
    pub icon: String,
    pub level: i32,
    pub is_group: bool,
    pub expanded: bool,
    pub selected: bool,
    pub target: Target,
}

/// Что открывает щелчок по строке дерева.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Group(ElementKind),
    Element(Element),
}

/// Свойство элемента в разделе «Общие».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    pub label: String,
    pub value: String,
}

impl Detail {
    fn new(label: &str, value: &str) -> Self {
        Self {
            label: label.to_owned(),
            value: value.to_owned(),
        }
    }
}

/// Колонка таблицы: ширина в px; гибкая забирает свободное место.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub title: &'static str,
    pub width: f32,
    pub right: bool,
    pub flex: bool,
}

impl Column {
    fn new(title: &'static str, width: f32) -> Self {
        Self {
            title,
            width,
            right: false,
            flex: false,
        }
    }

    fn flex(title: &'static str, width: f32) -> Self {
        Self {
            flex: true,
            ..Self::new(title, width)
        }
    }
}

/// Таблица для окна: колонки и строки из ячеек-строк.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
}

/// Заголовок редактора выбранного элемента.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Heading {
    pub title: String,
    /// Имя-идентификатор; пусто, если у элемента его нет.
    pub name: String,
    pub kind: &'static str,
}

/// Состояние окна: что раскрыто в дереве, какой элемент и раздел открыты.
pub struct Browser {
    config: Configuration,
    expanded: HashSet<ElementKind>,
    selected: Option<Element>,
    section: EditorSection,
    part: usize,
}

impl Browser {
    /// Все группы раскрыты, открыт первый элемент первой непустой группы.
    pub fn new(config: Configuration) -> Self {
        let selected = ElementKind::ALL
            .into_iter()
            .find(|kind| config.count(*kind) > 0)
            .map(|kind| Element { kind, index: 0 });
        Self {
            config,
            expanded: ElementKind::ALL.into_iter().collect(),
            selected,
            section: EditorSection::General,
            part: 0,
        }
    }

    /// Новая версия той же папки (файлы правили в редакторе): раскрытые группы, элемент и раздел
    /// остаются, если элемент ещё есть; иначе как у нового окна.
    pub fn reload(&mut self, config: Configuration) {
        let kept = self
            .selected
            .filter(|element| element.index < config.count(element.kind));
        if kept.is_none() {
            let expanded = std::mem::take(&mut self.expanded);
            *self = Self::new(config);
            self.expanded = expanded;
            return;
        }
        self.config = config;
        if self.part >= self.part_titles().len() {
            self.part = 0;
        }
    }

    /// Строки дерева с учётом раскрытых групп и выбранного элемента.
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        self.config.tree_rows(&self.expanded, self.selected)
    }

    /// Открытый элемент; `None`, если в конфигурации нет ни одного.
    pub fn selected(&self) -> Option<Element> {
        self.selected
    }

    /// Щелчок по строке дерева: группа раскрывается или сворачивается, элемент открывается.
    pub fn click(&mut self, row: usize) {
        let Some(target) = self.tree_rows().get(row).map(|row| row.target) else {
            return;
        };
        match target {
            Target::Group(kind) => {
                if !self.expanded.remove(&kind) {
                    self.expanded.insert(kind);
                }
            }
            Target::Element(element) => {
                self.selected = Some(element);
                self.part = 0;
                // Раздел остаётся, если он есть и у нового вида: так удобно листать однотипные объекты.
                if !element.kind.sections().contains(&self.section) {
                    self.section = EditorSection::General;
                }
            }
        }
    }

    /// Разделы выбранного элемента; без выбора их нет.
    pub fn sections(&self) -> &'static [EditorSection] {
        self.selected.map_or(&[], |element| element.kind.sections())
    }

    /// Открытый раздел редактора.
    pub fn section(&self) -> EditorSection {
        self.section
    }

    /// Открывает раздел, если он есть у выбранного элемента.
    pub fn choose_section(&mut self, section: EditorSection) {
        if self.sections().contains(&section) {
            self.section = section;
        }
    }

    /// Табличная часть, открытая в разделе «Табличные части».
    pub fn part(&self) -> usize {
        self.part
    }

    /// Открывает табличную часть, если она есть.
    pub fn choose_part(&mut self, part: usize) {
        if part < self.part_titles().len() {
            self.part = part;
        }
    }

    /// Названия табличных частей для выбора; только в разделе «Табличные части».
    pub fn part_titles(&self) -> Vec<String> {
        match self.selected {
            Some(element) if self.section == EditorSection::TableParts => {
                self.config.part_titles(element)
            }
            _ => Vec::new(),
        }
    }

    /// Заголовок выбранного элемента.
    pub fn heading(&self) -> Option<Heading> {
        let element = self.selected?;
        Some(Heading {
            title: self.config.title_of(element)?.to_owned(),
            name: self
                .config
                .name_of(element)
                .map(str::to_owned)
                .unwrap_or_default(),
            kind: element.kind.title(),
        })
    }

    /// Строки раздела «Общие» выбранного элемента.
    pub fn general(&self) -> Vec<Detail> {
        self.selected
            .map_or_else(Vec::new, |element| self.config.general(element))
    }

    /// Таблица полей для разделов «Поля» и «Табличные части»; в остальных разделах её нет.
    pub fn grid(&self) -> Option<Table> {
        let element = self.selected?;
        match self.section {
            EditorSection::Fields => self.config.fields_table(element),
            EditorSection::TableParts => self.config.part_table(element, self.part),
            _ => None,
        }
    }

    /// Что написать вместо пустой таблицы.
    pub fn grid_empty_text(&self) -> &'static str {
        if self.section == EditorSection::TableParts {
            "У объекта нет табличных частей."
        } else {
            "У объекта нет полей."
        }
    }
}

/// Одна найденная проверкой ошибка: где и почему.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckIssue {
    pub file: String,
    /// Номер строки; пусто, если место неизвестно.
    pub line: String,
    pub reason: String,
}

/// Результат проверки для панели «Проверка».
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CheckReport {
    #[default]
    NotRun,
    Passed,
    Failed(Vec<CheckIssue>),
}

impl CheckReport {
    /// Проверяет папку конфигурации теми же функциями, что `ostov check`: сначала разбор,
    /// затем ссылки и имена. Без базы и без записи на диск. Файлы в отчёте указаны от папки.
    pub fn run(dir: &Path) -> Self {
        let errors = match ostov_meta::parse(dir) {
            Ok(files) => ostov_meta::check(&files),
            Err(errors) => errors,
        };
        Self::from_errors(&relative_to(dir, errors))
    }

    /// Результат по списку ошибок `ostov-meta`; пустой список значит «ошибок нет».
    pub fn from_errors(errors: &[ostov_meta::Error]) -> Self {
        if errors.is_empty() {
            return Self::Passed;
        }
        Self::Failed(
            errors
                .iter()
                .map(|error| CheckIssue {
                    file: error.file.display().to_string(),
                    line: error.line.map(|line| line.to_string()).unwrap_or_default(),
                    reason: error.reason.clone(),
                })
                .collect(),
        )
    }

    /// Короткая сводка для метки панели.
    pub fn summary(&self) -> String {
        match self {
            Self::NotRun => "Не запускалась".to_owned(),
            Self::Passed => "Ошибок нет".to_owned(),
            Self::Failed(issues) => format!("Ошибок: {}", issues.len()),
        }
    }

    /// Что написать вместо пустого списка.
    pub fn empty_text(&self) -> &'static str {
        match self {
            Self::NotRun => "Проверка не запускалась. Нажмите «Проверить».",
            Self::Passed => "Файлы конфигурации прочитаны и проверены, ошибок нет.",
            // Со списком ошибок пустого места нет.
            Self::Failed(_) => "",
        }
    }

    pub fn is_passed(&self) -> bool {
        matches!(self, Self::Passed)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    /// Список для таблицы: файл, строка, причина.
    pub fn table(&self) -> Table {
        let issues = match self {
            Self::Failed(issues) => issues.as_slice(),
            Self::NotRun | Self::Passed => &[],
        };
        Table {
            columns: vec![
                Column::new("Файл", 260.0),
                Column {
                    right: true,
                    ..Column::new("Строка", 80.0)
                },
                Column::flex("Причина", 200.0),
            ],
            rows: issues
                .iter()
                .map(|issue| vec![issue.file.clone(), issue.line.clone(), issue.reason.clone()])
                .collect(),
        }
    }
}

/// Пути файлов от папки конфигурации: `catalogs/goods.yaml` читается легче полного пути.
pub(crate) fn relative_to(dir: &Path, errors: Vec<ostov_meta::Error>) -> Vec<ostov_meta::Error> {
    errors
        .into_iter()
        .map(|error| match error.file.strip_prefix(dir) {
            Ok(file) => ostov_meta::Error::new(file, error.line, error.code, error.reason),
            Err(_) => error,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{config_copy, config_dir, replace_in};
    use ostov_api::{ObjectMeta, Rights, TablePartDef};

    /// Встроенная «Склад»: та же, что в `config/`, но без чтения диска.
    fn embedded() -> Configuration {
        let config = ostov_meta::demo().expect("встроенная конфигурация проходит проверку");
        Configuration {
            meta: Meta::new(&config),
            title: config.title,
            version: config.version,
        }
    }

    fn object(name: &str, kind: ObjectKind) -> ObjectMeta {
        ObjectMeta {
            name: name.to_owned(),
            title: format!("Заголовок {name}"),
            kind,
            icon: None,
            fields: vec![
                FieldDef::new("code", FieldKind::Text).with_title("Код"),
                FieldDef::reference("owner", "other").with_title("Владелец"),
            ],
            table_parts: Vec::new(),
            rights: Rights::ALL,
        }
    }

    fn config_of(meta: Meta) -> Configuration {
        Configuration {
            title: "Проба".to_owned(),
            version: "1.0.0".to_owned(),
            meta,
        }
    }

    fn empty_meta() -> Meta {
        Meta {
            menu: Vec::new(),
            objects: Vec::new(),
            roles: Vec::new(),
            registers: Vec::new(),
            user_fields: Vec::new(),
        }
    }

    fn groups(rows: &[TreeRow]) -> Vec<(&str, bool)> {
        rows.iter()
            .filter(|row| row.is_group)
            .map(|row| (row.title.as_str(), row.expanded))
            .collect()
    }

    #[test]
    fn tree_groups_and_counts_match_meta() {
        let config = embedded();
        let all = ElementKind::ALL.into_iter().collect();
        let rows = config.tree_rows(&all, None);
        let meta = &config.meta;
        let count_of = |kind: ObjectKind| meta.objects.iter().filter(|o| o.kind == kind).count();
        let expected = [
            format!("Справочники ({})", count_of(ObjectKind::Catalog)),
            format!("Документы ({})", count_of(ObjectKind::Document)),
            format!("Регистры ({})", meta.registers.len()),
            format!("Роли ({})", meta.roles.len()),
            format!("Подсистемы ({})", meta.menu.len()),
        ];
        let titles: Vec<&str> = groups(&rows).into_iter().map(|(title, _)| title).collect();
        assert_eq!(titles, expected);
        // Под раскрытыми группами по одной строке на элемент: всего групп и элементов Meta.
        let elements = rows.iter().filter(|row| !row.is_group).count();
        assert_eq!(
            elements,
            meta.objects.len() + meta.registers.len() + meta.roles.len() + meta.menu.len()
        );
    }

    #[test]
    fn collapsed_group_hides_its_elements_but_keeps_the_count() {
        let config = embedded();
        let rows = config.tree_rows(&HashSet::new(), None);
        assert_eq!(rows.len(), ElementKind::ALL.len());
        assert!(rows.iter().all(|row| row.is_group && !row.expanded));
    }

    #[test]
    fn empty_groups_are_shown_with_zero() {
        let mut meta = empty_meta();
        meta.objects.push(object("things", ObjectKind::Catalog));
        let config = config_of(meta);
        let rows = config.tree_rows(&ElementKind::ALL.into_iter().collect(), None);
        let titles: Vec<&str> = groups(&rows).into_iter().map(|(title, _)| title).collect();
        assert_eq!(
            titles,
            [
                "Справочники (1)",
                "Документы (0)",
                "Регистры (0)",
                "Роли (0)",
                "Подсистемы (0)"
            ]
        );
    }

    #[test]
    fn catalogs_and_documents_are_listed_apart_in_meta_order() {
        let mut meta = empty_meta();
        meta.objects.push(object("first_doc", ObjectKind::Document));
        meta.objects.push(object("first_cat", ObjectKind::Catalog));
        meta.objects.push(object("second_cat", ObjectKind::Catalog));
        let config = config_of(meta);
        let name = |kind, index| config.name_of(Element { kind, index });
        assert_eq!(name(ElementKind::Catalog, 0), Some("first_cat"));
        assert_eq!(name(ElementKind::Catalog, 1), Some("second_cat"));
        assert_eq!(name(ElementKind::Document, 0), Some("first_doc"));
        assert_eq!(name(ElementKind::Catalog, 2), None);
    }

    #[test]
    fn element_icon_falls_back_to_the_kind_icon() {
        let mut meta = empty_meta();
        let mut with_icon = object("a", ObjectKind::Catalog);
        with_icon.icon = Some("warehouse".to_owned());
        meta.objects.push(with_icon);
        meta.objects.push(object("b", ObjectKind::Catalog));
        let config = config_of(meta);
        let icon = |index| {
            config
                .icon_of(Element {
                    kind: ElementKind::Catalog,
                    index,
                })
                .to_owned()
        };
        assert_eq!(icon(0), "warehouse");
        assert_eq!(icon(1), "list");
    }

    #[test]
    fn fields_table_follows_the_fields_of_each_object() {
        let config = embedded();
        let kinds = [
            (ElementKind::Catalog, ObjectKind::Catalog),
            (ElementKind::Document, ObjectKind::Document),
        ];
        for (element_kind, object_kind) in kinds {
            for (index, object) in config.objects(object_kind).enumerate() {
                let table = config
                    .fields_table(Element {
                        kind: element_kind,
                        index,
                    })
                    .expect("у объекта есть таблица полей");
                assert_eq!(table.rows.len(), object.fields.len());
                for (row, field) in table.rows.iter().zip(&object.fields) {
                    assert_eq!(row[0], field.name);
                    assert_eq!(row[1], field.title);
                    assert_eq!(row[3], if field.required { "Да" } else { "Нет" });
                }
            }
        }
    }

    #[test]
    fn field_types_are_written_as_in_the_config_files() {
        let decimal = FieldDef::new("qty", FieldKind::Decimal { scale: 3 });
        assert_eq!(type_text(&decimal), "decimal(3)");
        assert_eq!(
            type_text(&FieldDef::reference("goods", "goods")),
            "ref(goods)"
        );
        assert_eq!(type_text(&FieldDef::new("n", FieldKind::Int)), "int");
        assert_eq!(type_text(&FieldDef::new("t", FieldKind::Text)), "text");
        assert_eq!(type_text(&FieldDef::new("d", FieldKind::Date)), "date");
    }

    #[test]
    fn register_table_lists_dimensions_then_resources_with_their_role() {
        let mut meta = empty_meta();
        meta.registers.push(RegisterMeta {
            name: "stock".to_owned(),
            title: "Остатки".to_owned(),
            dimensions: vec![FieldDef::reference("place", "places")],
            resources: vec![FieldDef::new("qty", FieldKind::Decimal { scale: 3 })],
        });
        let config = config_of(meta);
        let table = config
            .fields_table(Element {
                kind: ElementKind::Register,
                index: 0,
            })
            .expect("у регистра есть таблица");
        let roles: Vec<&str> = table.rows.iter().map(|row| row[2].as_str()).collect();
        assert_eq!(roles, ["Измерение", "Ресурс"]);
        assert_eq!(table.rows[0][0], "place");
        assert_eq!(table.rows[1][3], "decimal(3)");
    }

    #[test]
    fn role_and_subsystem_have_no_fields() {
        let config = embedded();
        for kind in [ElementKind::Role, ElementKind::Subsystem] {
            let element = Element { kind, index: 0 };
            assert_eq!(config.fields_table(element), None);
        }
    }

    #[test]
    fn table_parts_come_from_the_object() {
        let mut meta = empty_meta();
        let mut document = object("order", ObjectKind::Document);
        document.table_parts = vec![
            TablePartDef {
                name: "lines".to_owned(),
                title: "Строки".to_owned(),
                fields: vec![FieldDef::new("qty", FieldKind::Int).with_title("Число")],
            },
            TablePartDef {
                name: "pays".to_owned(),
                title: "Оплаты".to_owned(),
                fields: Vec::new(),
            },
        ];
        meta.objects.push(document);
        let config = config_of(meta);
        let element = Element {
            kind: ElementKind::Document,
            index: 0,
        };
        assert_eq!(config.part_titles(element), ["Строки", "Оплаты"]);
        let first = config.part_table(element, 0).expect("первая часть");
        assert_eq!(first.rows, [["qty", "Число", "int", "Нет"]]);
        let second = config.part_table(element, 1).expect("вторая часть");
        assert!(second.rows.is_empty());
        assert_eq!(config.part_table(element, 2), None);
    }

    #[test]
    fn sections_depend_on_the_kind_of_element() {
        use EditorSection::*;
        assert_eq!(ElementKind::Catalog.sections(), ALL_SECTIONS);
        assert_eq!(ElementKind::Document.sections(), ALL_SECTIONS);
        assert_eq!(ElementKind::Role.sections(), [General, Rights]);
        assert_eq!(ElementKind::Subsystem.sections(), [General]);
        assert!(!ElementKind::Role.sections().contains(&TableParts));
        assert!(!ElementKind::Register.sections().contains(&Forms));
    }

    #[test]
    fn browser_opens_the_first_element_and_expands_all_groups() {
        let browser = Browser::new(embedded());
        let first = browser.selected().expect("в «Складе» есть элементы");
        assert_eq!(first.index, 0);
        let rows = browser.tree_rows();
        assert!(
            rows.iter()
                .filter(|row| row.is_group)
                .all(|row| row.expanded)
        );
        assert_eq!(rows.iter().filter(|row| row.selected).count(), 1);
        assert_eq!(browser.section(), EditorSection::General);
    }

    #[test]
    fn browser_without_elements_has_nothing_selected() {
        let browser = Browser::new(config_of(empty_meta()));
        assert_eq!(browser.selected(), None);
        assert_eq!(browser.heading(), None);
        assert!(browser.sections().is_empty());
        assert!(browser.general().is_empty());
        assert_eq!(browser.grid(), None);
    }

    #[test]
    fn click_on_group_toggles_it_and_on_element_selects_it() {
        let mut browser = Browser::new(embedded());
        let before = browser.tree_rows().len();
        browser.click(0);
        let collapsed = browser.tree_rows();
        assert!(collapsed.len() < before);
        assert!(!collapsed[0].expanded);
        browser.click(0);
        assert_eq!(browser.tree_rows().len(), before);

        let rows = browser.tree_rows();
        let (row, target) = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.is_group && !row.selected)
            .map(|(i, row)| (i, row.target))
            .next_back()
            .expect("есть невыбранный элемент");
        browser.click(row);
        assert_eq!(Target::Element(browser.selected().expect("выбран")), target);
        // Щелчок за пределами списка ничего не меняет.
        browser.click(10_000);
        assert_eq!(Target::Element(browser.selected().expect("выбран")), target);
    }

    #[test]
    fn reload_keeps_the_open_element_while_it_exists() {
        let mut browser = Browser::new(embedded());
        browser.click(0);
        let rows = browser.tree_rows();
        let row = rows
            .iter()
            .position(|row| !row.is_group)
            .expect("есть элемент");
        browser.click(row);
        browser.choose_section(EditorSection::Fields);
        let selected = browser.selected();

        browser.reload(embedded());
        assert_eq!(browser.selected(), selected);
        assert_eq!(browser.section(), EditorSection::Fields);
        assert_eq!(browser.tree_rows().len(), rows.len());

        // Элемента больше нет: открыт первый, раздел «Общие», свёрнутое осталось свёрнутым.
        browser.reload(config_of(empty_meta()));
        assert_eq!(browser.selected(), None);
        assert_eq!(browser.section(), EditorSection::General);
        assert!(!browser.tree_rows()[0].expanded);
    }

    #[test]
    fn section_survives_a_move_between_kinds_that_share_it_and_resets_otherwise() {
        let mut meta = empty_meta();
        meta.objects.push(object("cat", ObjectKind::Catalog));
        meta.objects.push(object("doc", ObjectKind::Document));
        meta.roles.push(RoleInfo {
            name: "boss".to_owned(),
            title: "Начальник".to_owned(),
        });
        let mut browser = Browser::new(config_of(meta));
        let click_title = |browser: &mut Browser, title: &str| {
            let row = browser
                .tree_rows()
                .iter()
                .position(|row| !row.is_group && row.title == title)
                .expect("элемент в дереве");
            browser.click(row);
        };

        browser.choose_section(EditorSection::Forms);
        click_title(&mut browser, "Заголовок doc");
        assert_eq!(browser.section(), EditorSection::Forms);

        click_title(&mut browser, "Начальник");
        assert_eq!(browser.section(), EditorSection::General);
        // Раздела, которого у роли нет, выбрать нельзя.
        browser.choose_section(EditorSection::TableParts);
        assert_eq!(browser.section(), EditorSection::General);
    }

    #[test]
    fn grid_is_shown_only_in_fields_and_table_parts() {
        let mut meta = empty_meta();
        let mut document = object("doc", ObjectKind::Document);
        document.table_parts = vec![TablePartDef {
            name: "lines".to_owned(),
            title: "Строки".to_owned(),
            fields: vec![FieldDef::new("qty", FieldKind::Int)],
        }];
        meta.objects.push(document);
        let mut browser = Browser::new(config_of(meta));

        assert_eq!(browser.grid(), None);
        browser.choose_section(EditorSection::Fields);
        assert_eq!(browser.grid().expect("поля").rows.len(), 2);
        assert!(browser.part_titles().is_empty());

        browser.choose_section(EditorSection::TableParts);
        assert_eq!(browser.part_titles(), ["Строки"]);
        assert_eq!(browser.grid().expect("часть").rows[0][0], "qty");
        browser.choose_part(5);
        assert_eq!(browser.part(), 0);

        browser.choose_section(EditorSection::Print);
        assert_eq!(browser.grid(), None);
    }

    #[test]
    fn heading_and_general_rows_describe_the_selected_element() {
        let mut meta = empty_meta();
        let mut catalog = object("things", ObjectKind::Catalog);
        catalog.icon = Some("box".to_owned());
        meta.objects.push(catalog);
        let browser = Browser::new(config_of(meta));
        let heading = browser.heading().expect("элемент выбран");
        assert_eq!(heading.title, "Заголовок things");
        assert_eq!(heading.name, "things");
        assert_eq!(heading.kind, "Справочник");
        let labels: Vec<String> = browser.general().into_iter().map(|d| d.label).collect();
        assert_eq!(
            labels,
            ["Имя (идентификатор)", "Заголовок", "Вид", "Значок"]
        );
    }

    #[test]
    fn subsystem_general_rows_list_what_the_section_contains() {
        let mut meta = empty_meta();
        let item = |title: &str, object: Option<&str>, children| MenuItem {
            title: title.to_owned(),
            icon: None,
            object: object.map(str::to_owned),
            children,
        };
        meta.menu.push(item(
            "Раздел",
            None,
            vec![
                item("Первый", Some("a"), Vec::new()),
                item("Группа", None, vec![item("Второй", Some("b"), Vec::new())]),
            ],
        ));
        let config = config_of(meta);
        let rows = config.general(Element {
            kind: ElementKind::Subsystem,
            index: 0,
        });
        let composition = rows.iter().find(|row| row.label == "Состав");
        assert_eq!(
            composition.map(|row| row.value.as_str()),
            Some("Первый, Второй")
        );
        assert!(rows.iter().all(|row| row.label != "Имя (идентификатор)"));
    }

    #[test]
    fn repository_config_folder_loads_and_passes_the_check() {
        let config = Configuration::load(&config_dir()).expect("config/ читается");
        assert_eq!(config.title, embedded().title);
        assert_eq!(config.meta, embedded().meta);
        assert_eq!(CheckReport::run(&config_dir()), CheckReport::Passed);
    }

    #[test]
    fn broken_yaml_is_reported_with_its_file_and_line() {
        let dir = config_copy("configurator-broken");
        let goods = dir.join("catalogs").join("goods.yaml");
        replace_in(&goods, "title: Товары", "title: Товары\ncolour: red");
        let report = CheckReport::run(&dir);
        let CheckReport::Failed(issues) = &report else {
            panic!("ожидались ошибки: {report:?}");
        };
        let issue = issues
            .iter()
            .find(|issue| issue.file.ends_with("goods.yaml"))
            .expect("ошибка в goods.yaml");
        assert_eq!(
            issue.file,
            Path::new("catalogs")
                .join("goods.yaml")
                .display()
                .to_string()
        );
        assert_eq!(issue.line, "3");
        assert!(Configuration::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap_or(&dir));
    }

    #[test]
    fn missing_folder_is_reported_as_missing_manifest() {
        let dir = config_dir().join("нет-такой-папки");
        let report = CheckReport::run(&dir);
        assert!(report.is_failed(), "{report:?}");
        assert_eq!(report.table().rows[0][0], "ostov.yaml");
    }

    #[test]
    fn check_errors_become_rows_with_file_line_and_reason() {
        let errors = [
            ostov_meta::Error::new(
                "config/goods.yaml",
                Some(7),
                ostov_meta::error::codes::UNKNOWN_KEY,
                "нет поля",
            ),
            ostov_meta::Error::new(
                "config/ostov.yaml",
                None,
                ostov_meta::error::codes::NO_MANIFEST,
                "нет манифеста",
            ),
        ];
        let report = CheckReport::from_errors(&errors);
        assert!(report.is_failed());
        assert_eq!(report.summary(), "Ошибок: 2");
        let table = report.table();
        assert_eq!(
            table.rows,
            [
                ["config/goods.yaml", "7", "нет поля"],
                ["config/ostov.yaml", "", "нет манифеста"]
            ]
        );
    }

    #[test]
    fn check_report_states_have_their_own_texts() {
        assert_eq!(CheckReport::default(), CheckReport::NotRun);
        assert_eq!(CheckReport::NotRun.summary(), "Не запускалась");
        assert_eq!(CheckReport::Passed.summary(), "Ошибок нет");
        assert!(CheckReport::Passed.is_passed());
        assert!(CheckReport::NotRun.table().rows.is_empty());
        assert_eq!(CheckReport::from_errors(&[]), CheckReport::Passed);
    }
}
