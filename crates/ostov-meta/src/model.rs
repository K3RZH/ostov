//! Содержимое файлов конфигурации как есть, до проверки ссылок и имён.
//!
//! Имена и типы обёрнуты в `Spanned`: так `ostov check` укажет строку конкретного значения.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde_saphyr::Spanned;

use crate::FieldType;

/// Вся конфигурация, как она записана в файлах.
#[derive(Debug)]
pub struct ConfigFiles {
    pub manifest: Located<Manifest>,
    pub catalogs: Vec<Located<Catalog>>,
    pub documents: Vec<Located<Document>>,
    pub registers: Vec<Located<Register>>,
    pub subsystems: Vec<Located<Subsystem>>,
}

/// Раздел меню: заголовок, значок и пункты — объекты и группы объектов.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subsystem {
    #[serde(rename = "subsystem")]
    pub name: Spanned<String>,
    pub title: String,
    #[serde(default)]
    pub icon: Option<Spanned<String>>,
    pub objects: Vec<Spanned<MenuEntry>>,
}

/// Пункт раздела: имя объекта (`- goods`) или группа (`- title: ...`, `objects: [...]`).
#[derive(Debug)]
pub enum MenuEntry {
    Object(String),
    Group(Group),
}

/// Группа внутри раздела. Пункты у неё того же вида, но вложенную группу запрещает `ostov check`:
/// так ошибка получит понятную причину, а не «ожидалась строка».
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub title: String,
    #[serde(default)]
    pub icon: Option<Spanned<String>>,
    pub objects: Vec<Spanned<MenuEntry>>,
}

impl<'de> Deserialize<'de> for MenuEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = MenuEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("имя объекта или группу (title, icon, objects)")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<MenuEntry, E> {
                Ok(MenuEntry::Object(name.to_owned()))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<MenuEntry, A::Error> {
                Group::deserialize(de::value::MapAccessDeserializer::new(map)).map(MenuEntry::Group)
            }
        }

        deserializer.deserialize_any(EntryVisitor)
    }
}

/// Объект и файл, из которого он прочитан.
#[derive(Debug)]
pub struct Located<T> {
    pub file: PathBuf,
    pub value: T,
}

/// `ostov.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: Spanned<String>,
    pub title: String,
    /// Версия самой конфигурации, её ведёт автор.
    pub version: String,
    /// Версия формата файлов, её проверяет загрузчик.
    pub format: Spanned<u32>,
    /// Разделы меню по порядку; каждый раздел из `subsystems/` должен быть здесь.
    #[serde(default)]
    pub menu: Vec<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: Spanned<String>,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: Spanned<FieldType>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(rename = "catalog")]
    pub name: Spanned<String>,
    pub title: String,
    #[serde(default)]
    pub icon: Option<Spanned<String>>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    #[serde(rename = "document")]
    pub name: Spanned<String>,
    pub title: String,
    #[serde(default)]
    pub icon: Option<Spanned<String>>,
    /// Регистры, по которым документ делает движения.
    #[serde(default)]
    pub registers: Vec<Spanned<String>>,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub tables: Vec<TablePart>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TablePart {
    pub name: Spanned<String>,
    pub title: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    #[serde(rename = "register")]
    pub name: Spanned<String>,
    pub title: String,
    pub kind: RegisterKind,
    pub dimensions: Vec<Field>,
    pub resources: Vec<Field>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterKind {
    Accumulation,
}
