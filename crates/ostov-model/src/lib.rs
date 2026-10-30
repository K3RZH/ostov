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
}

impl FieldDef {
    pub fn new(name: &str, kind: FieldKind) -> Self {
        Self {
            name: name.to_owned(),
            title: name.to_owned(),
            kind,
            target: None,
        }
    }

    pub fn reference(name: &str, target: &str) -> Self {
        Self {
            name: name.to_owned(),
            title: name.to_owned(),
            kind: FieldKind::Ref,
            target: Some(target.to_owned()),
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

/// Конфигурация целиком: манифест и все объекты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: String,
    pub title: String,
    /// Версия самой конфигурации из манифеста.
    pub version: String,
    pub catalogs: Vec<CatalogDef>,
    pub documents: Vec<DocumentDef>,
    pub registers: Vec<RegisterDef>,
}
