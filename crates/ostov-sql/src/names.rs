//! Имена из запроса → таблицы и колонки базы по метаданным.

use ostov_model::{Config, FieldDef, FieldKind};

use crate::SqlError;

/// Таблица из конфигурации: как называется в базе и какие колонки можно читать.
#[derive(Debug)]
pub(crate) struct Table {
    /// Заголовок для сообщений: «Товары», «Поступление.Товары».
    pub title: String,
    pub physical: String,
    pub columns: Vec<(String, FieldKind)>,
    /// Поле-ссылка → на что ссылается.
    pub targets: Vec<(String, String)>,
    /// Объект, право на чтение которого нужно; у регистров права нет.
    pub right: Option<(String, String)>,
}

/// Отметки объектов: в базе 0 и 1.
const FLAGS: [&str; 2] = ["posted", "deleted"];

impl Table {
    pub fn column(&self, name: &str) -> Option<FieldKind> {
        self.columns
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, k)| *k)
    }

    pub fn target(&self, name: &str) -> Option<String> {
        self.targets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
    }

    /// `posted` и `deleted` справочников и документов; у регистров таких колонок нет.
    pub fn is_flag(&self, name: &str) -> bool {
        self.right.is_some() && FLAGS.contains(&name) && self.column(name).is_some()
    }
}

fn targets<'a>(fields: impl IntoIterator<Item = &'a FieldDef>) -> Vec<(String, String)> {
    fields
        .into_iter()
        .filter_map(|f| Some((f.name.clone(), f.target.clone()?)))
        .collect()
}

fn user(fields: &[FieldDef]) -> impl Iterator<Item = (String, FieldKind)> + '_ {
    fields.iter().map(|f| (f.name.clone(), f.kind))
}

fn system<'a>(names: &'a [(&'a str, FieldKind)]) -> impl Iterator<Item = (String, FieldKind)> + 'a {
    names.iter().map(|(n, k)| ((*n).to_owned(), *k))
}

/// `goods`, `receipt`, `stock` или `receipt.lines`.
pub(crate) fn resolve(config: &Config, parts: &[String]) -> Result<Table, SqlError> {
    let not_found = || SqlError::NoTable {
        name: parts.join("."),
    };
    match parts {
        [name] => {
            if let Some(c) = config.catalogs.iter().find(|c| &c.name == name) {
                return Ok(Table {
                    title: c.title.clone(),
                    physical: c.name.clone(),
                    columns: user(&c.fields)
                        .chain(system(&[
                            ("id", FieldKind::Int),
                            ("version", FieldKind::Int),
                            ("deleted", FieldKind::Int),
                        ]))
                        .collect(),
                    targets: targets(&c.fields),
                    right: Some((c.name.clone(), c.title.clone())),
                });
            }
            if let Some(d) = config.documents.iter().find(|d| &d.name == name) {
                return Ok(Table {
                    title: d.title.clone(),
                    physical: d.name.clone(),
                    columns: user(&d.fields)
                        .chain(system(&[
                            ("id", FieldKind::Int),
                            ("version", FieldKind::Int),
                            ("number", FieldKind::Int),
                            ("posted", FieldKind::Int),
                            ("deleted", FieldKind::Int),
                        ]))
                        .collect(),
                    targets: targets(&d.fields),
                    right: Some((d.name.clone(), d.title.clone())),
                });
            }
            if let Some(r) = config.registers.iter().find(|r| &r.name == name) {
                // Колонки движений — как в ostov-core/src/register/accumulation.rs.
                return Ok(Table {
                    title: r.title.clone(),
                    physical: r.name.clone(),
                    columns: user(&r.dimensions)
                        .chain(user(&r.resources))
                        .chain(system(&[
                            ("id", FieldKind::Int),
                            ("period", FieldKind::Date),
                            ("recorder", FieldKind::Text),
                            ("recorder_id", FieldKind::Int),
                            ("kind", FieldKind::Int),
                        ]))
                        .collect(),
                    targets: targets(r.dimensions.iter().chain(&r.resources)),
                    right: None,
                });
            }
            Err(not_found())
        }
        [doc, part] => {
            let d = config
                .documents
                .iter()
                .find(|d| &d.name == doc)
                .ok_or_else(not_found)?;
            let p = d
                .table_parts
                .iter()
                .find(|p| &p.name == part)
                .ok_or_else(not_found)?;
            Ok(Table {
                title: format!("{}.{}", d.title, p.title),
                physical: format!("{}__{}", d.name, p.name),
                columns: user(&p.fields)
                    .chain(system(&[
                        ("id", FieldKind::Int),
                        ("doc_id", FieldKind::Int),
                        ("line_no", FieldKind::Int),
                    ]))
                    .collect(),
                targets: targets(&p.fields),
                right: Some((d.name.clone(), d.title.clone())),
            })
        }
        _ => Err(not_found()),
    }
}
