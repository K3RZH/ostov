//! Содержимое файлов конфигурации как есть, до проверки ссылок и имён.
//!
//! Имена и типы обёрнуты в `Spanned`: так `ostov check` укажет строку конкретного значения.

use std::path::PathBuf;

use serde::Deserialize;
use serde_saphyr::Spanned;

use crate::FieldType;

/// Вся конфигурация, прочитанная из папки.
#[derive(Debug)]
pub struct Config {
    pub manifest: Located<Manifest>,
    pub catalogs: Vec<Located<Catalog>>,
    pub documents: Vec<Located<Document>>,
    pub registers: Vec<Located<Register>>,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: Spanned<String>,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: Spanned<FieldType>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(rename = "catalog")]
    pub name: Spanned<String>,
    pub title: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    #[serde(rename = "document")]
    pub name: Spanned<String>,
    pub title: String,
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
