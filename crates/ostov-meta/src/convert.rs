//! Проверенные файлы → модель для ядра (`ostov-model`). Номера строк здесь уже не нужны.

use ostov_model::{
    CatalogDef, Config, DocumentDef, FieldDef, FieldKind, MenuItem, RegisterDef, RegisterKind,
    TablePartDef,
};
use serde_saphyr::Spanned;

use crate::{ConfigFiles, Field, FieldType, MenuEntry, Subsystem};

pub(crate) fn into_model(files: ConfigFiles) -> Config {
    let manifest = files.manifest.value;
    let catalogs: Vec<CatalogDef> = files
        .catalogs
        .into_iter()
        .map(|catalog| {
            let catalog = catalog.value;
            CatalogDef {
                name: catalog.name.value,
                title: catalog.title,
                icon: catalog.icon.map(|icon| icon.value),
                fields: fields(catalog.fields),
            }
        })
        .collect();
    let documents: Vec<DocumentDef> = files
        .documents
        .into_iter()
        .map(|document| document_def(document.value))
        .collect();
    let registers = files
        .registers
        .into_iter()
        .map(|register| {
            let register = register.value;
            RegisterDef {
                name: register.name.value,
                title: register.title,
                kind: match register.kind {
                    crate::RegisterKind::Accumulation => RegisterKind::Accumulation,
                },
                dimensions: fields(register.dimensions),
                resources: fields(register.resources),
            }
        })
        .collect();
    let objects = Objects {
        catalogs: &catalogs,
        documents: &documents,
    };
    let subsystems: Vec<Subsystem> = files.subsystems.into_iter().map(|s| s.value).collect();
    // Порядок разделов — из `menu:` манифеста; check гарантирует, что там все разделы.
    let menu = manifest
        .menu
        .iter()
        .filter_map(|name| subsystems.iter().find(|s| s.name.value == name.value))
        .map(|subsystem| MenuItem {
            title: subsystem.title.clone(),
            icon: subsystem.icon.as_ref().map(|icon| icon.value.clone()),
            object: None,
            children: objects.items(&subsystem.objects),
        })
        .collect();
    Config {
        name: manifest.name.value,
        title: manifest.title,
        version: manifest.version,
        catalogs,
        documents,
        registers,
        menu,
    }
}

fn document_def(document: crate::Document) -> DocumentDef {
    DocumentDef {
        name: document.name.value,
        title: document.title,
        icon: document.icon.map(|icon| icon.value),
        fields: fields(document.fields),
        table_parts: document
            .tables
            .into_iter()
            .map(|part| TablePartDef {
                name: part.name.value,
                title: part.title,
                fields: fields(part.fields),
            })
            .collect(),
        registers: document.registers.into_iter().map(|r| r.value).collect(),
    }
}

/// Объекты, которые открываются из меню: пункт меню берёт у них заголовок и значок.
struct Objects<'a> {
    catalogs: &'a [CatalogDef],
    documents: &'a [DocumentDef],
}

impl Objects<'_> {
    fn items(&self, entries: &[Spanned<MenuEntry>]) -> Vec<MenuItem> {
        entries
            .iter()
            .filter_map(|entry| match &entry.value {
                MenuEntry::Object(name) => self.item(name),
                MenuEntry::Group(group) => Some(MenuItem {
                    title: group.title.clone(),
                    icon: group.icon.as_ref().map(|icon| icon.value.clone()),
                    object: None,
                    children: self.items(&group.objects),
                }),
            })
            .collect()
    }

    /// Пункт объекта; неизвестный объект пропускается — после `check` таких нет.
    fn item(&self, name: &str) -> Option<MenuItem> {
        let catalog = self.catalogs.iter().find(|c| c.name == name);
        let (title, icon) = match catalog {
            Some(catalog) => (&catalog.title, &catalog.icon),
            None => {
                let document = self.documents.iter().find(|d| d.name == name)?;
                (&document.title, &document.icon)
            }
        };
        Some(MenuItem {
            title: title.clone(),
            icon: icon.clone(),
            object: Some(name.to_owned()),
            children: Vec::new(),
        })
    }
}

fn fields(fields: Vec<Field>) -> Vec<FieldDef> {
    fields.into_iter().map(field).collect()
}

fn field(field: Field) -> FieldDef {
    let (kind, target) = match field.kind.value {
        FieldType::Int => (FieldKind::Int, None),
        FieldType::Text => (FieldKind::Text, None),
        FieldType::Date => (FieldKind::Date, None),
        FieldType::Decimal { scale } => (FieldKind::Decimal { scale }, None),
        FieldType::Ref(target) => (FieldKind::Ref, Some(target)),
    };
    FieldDef {
        name: field.name.value,
        title: field.title,
        kind,
        target,
        required: field.required,
        read_only: field.read_only,
    }
}
