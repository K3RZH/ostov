//! Модель метаданных: что описано в конфигурации, без SQL и без разбора файлов.
//!
//! Общий язык `ostov-core` и `ostov-meta`: meta строит модель из YAML, ядро — таблицы из модели.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Int,
    Text,
    Decimal {
        scale: u8,
    },
    /// Дата `ГГГГ-ММ-ДД`; хранится текстом, такая строка сортируется как дата.
    Date,
    /// Ссылка на элемент справочника или документ: хранится id, цель — в `FieldDef::target`.
    Ref,
}

impl fmt::Display for FieldKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => write!(f, "целое число"),
            Self::Text => write!(f, "строка"),
            Self::Decimal { scale } => write!(f, "число с {scale} знаками после запятой"),
            Self::Date => write!(f, "дата ГГГГ-ММ-ДД"),
            Self::Ref => write!(f, "ссылка"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    /// Название для интерфейса; без него — имя поля.
    pub title: String,
    pub kind: FieldKind,
    /// Для ссылки: на какой справочник или документ.
    pub target: Option<String>,
    /// Без значения нельзя провести документ; проверяет ядро при записи.
    pub required: bool,
    /// Подсказка интерфейсу: поле не редактируют (его считает ядро). Ядро значению от клиента
    /// всё равно не верит и пересчитывает вычисляемые поля само.
    pub read_only: bool,
}

impl FieldDef {
    pub fn new(name: &str, kind: FieldKind) -> Self {
        Self {
            name: name.to_owned(),
            title: name.to_owned(),
            kind,
            target: None,
            required: false,
            read_only: false,
        }
    }

    pub fn reference(name: &str, target: &str) -> Self {
        Self {
            target: Some(target.to_owned()),
            ..Self::new(name, FieldKind::Ref)
        }
    }

    pub fn with_title(mut self, title: &str) -> Self {
        title.clone_into(&mut self.title);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDef {
    pub name: String,
    pub title: String,
    /// Значок из набора интерфейса (`ostov-ui/assets/icons`).
    pub icon: Option<String>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePartDef {
    pub name: String,
    pub title: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentDef {
    pub name: String,
    pub title: String,
    pub icon: Option<String>,
    pub fields: Vec<FieldDef>,
    pub table_parts: Vec<TablePartDef>,
    /// Регистры, по которым документ делает движения. Сами движения считает правило проведения.
    pub registers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterKind {
    Accumulation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterDef {
    pub name: String,
    pub title: String,
    pub kind: RegisterKind,
    pub dimensions: Vec<FieldDef>,
    pub resources: Vec<FieldDef>,
}

/// Конфигурация целиком: манифест, все объекты и меню.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: String,
    pub title: String,
    /// Версия самой конфигурации из манифеста.
    pub version: String,
    pub catalogs: Vec<CatalogDef>,
    pub documents: Vec<DocumentDef>,
    pub registers: Vec<RegisterDef>,
    /// Разделы меню в порядке `menu:` манифеста.
    pub menu: Vec<MenuItem>,
}

// ---------------------------------------------------------------- ответ GetMeta
// Единственное место этих типов: ostov-api их переэкспортирует, своих не заводит.

/// Пункт меню. Раздел и группа — пункты без объекта, с детьми; в YAML глубина ограничена:
/// раздел → группа → объекты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub title: String,
    /// Значок из набора интерфейса (`ostov-ui/assets/icons`); `None` — интерфейс выбирает сам.
    pub icon: Option<String>,
    /// Объект, который открывает пункт; `None` — раздел или группа.
    pub object: Option<String>,
    pub children: Vec<MenuItem>,
}

/// Что интерфейсу нужно, чтобы построить меню, колонки списков и формы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub menu: Vec<MenuItem>,
    pub objects: Vec<ObjectMeta>,
    /// Роли для экрана «Пользователи и роли»; заполнит ostov-sec, пока пусто.
    pub roles: Vec<String>,
}

/// Объект, который открывается в интерфейсе: справочник или документ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    pub name: String,
    pub title: String,
    pub kind: ObjectKind,
    pub icon: Option<String>,
    /// Поля шапки: имя, заголовок, тип, цель ссылки, required, read_only.
    pub fields: Vec<FieldDef>,
    /// У справочника пусто.
    pub table_parts: Vec<TablePartDef>,
    pub rights: Rights,
}

/// Виды объектов в интерфейсе. Регистры, отчёты и перечисления добавятся вместе с их поддержкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Catalog,
    Document,
}

/// Что текущий пользователь может делать с объектом.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rights {
    pub read: bool,
    pub create: bool,
    pub update: bool,
    pub post: bool,
    pub unpost: bool,
    pub delete: bool,
}

impl Rights {
    /// Всё разрешено: так сейчас и есть, пока нет ostov-sec (один пользователь без входа).
    pub const ALL: Self = Self {
        read: true,
        create: true,
        update: true,
        post: true,
        unpost: true,
        delete: true,
    };
}

impl Meta {
    /// Метаданные для интерфейса из конфигурации. Права — `Rights::ALL`, роли пусты:
    /// их будет выдавать ostov-sec по пользователю.
    pub fn new(config: &Config) -> Self {
        let catalogs = config.catalogs.iter().map(|c| ObjectMeta {
            name: c.name.clone(),
            title: c.title.clone(),
            kind: ObjectKind::Catalog,
            icon: c.icon.clone(),
            fields: c.fields.clone(),
            table_parts: Vec::new(),
            rights: Rights::ALL,
        });
        let documents = config.documents.iter().map(|d| ObjectMeta {
            name: d.name.clone(),
            title: d.title.clone(),
            kind: ObjectKind::Document,
            icon: d.icon.clone(),
            fields: d.fields.clone(),
            table_parts: d.table_parts.clone(),
            rights: Rights::ALL,
        });
        Self {
            menu: config.menu.clone(),
            objects: catalogs.chain(documents).collect(),
            roles: Vec::new(),
        }
    }
}
