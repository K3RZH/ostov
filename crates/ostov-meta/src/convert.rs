//! Проверенные файлы → модель для ядра (`ostov-model`). Номера строк здесь уже не нужны.

use ostov_model::{
    CatalogDef, Config, DocumentDef, FieldDef, FieldKind, RegisterDef, RegisterKind, SubsystemDef,
    TablePartDef,
};

use crate::{ConfigFiles, Field, FieldType};

pub(crate) fn into_model(files: ConfigFiles) -> Config {
    let manifest = files.manifest.value;
    Config {
        name: manifest.name.value,
        title: manifest.title,
        version: manifest.version,
        catalogs: files
            .catalogs
            .into_iter()
            .map(|catalog| {
                let catalog = catalog.value;
                CatalogDef {
                    name: catalog.name.value,
                    title: catalog.title,
                    fields: fields(catalog.fields),
                }
            })
            .collect(),
        documents: files
            .documents
            .into_iter()
            .map(|document| {
                let document = document.value;
                DocumentDef {
                    name: document.name.value,
                    title: document.title,
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
            })
            .collect(),
        registers: files
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
            .collect(),
        subsystems: files
            .subsystems
            .into_iter()
            .map(|subsystem| {
                let subsystem = subsystem.value;
                SubsystemDef {
                    name: subsystem.name.value,
                    title: subsystem.title,
                    objects: subsystem.objects.into_iter().map(|o| o.value).collect(),
                }
            })
            .collect(),
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
    }
}
