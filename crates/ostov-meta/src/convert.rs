//! Проверенные файлы → модель для ядра (`ostov-model`). Номера строк здесь уже не нужны.

use std::collections::BTreeMap;
use std::path::Path;

use ostov_model::{
    CatalogDef, Config, Cost, DocumentDef, FieldDef, FieldKind, MenuItem, Position, RegisterDef,
    RegisterKind, Restriction, Rights, RoleDef, SystemRights, TablePartDef, UserFieldDef,
};
use serde_saphyr::Spanned;

use crate::model::USER_PREFIX;

use crate::{ConfigFiles, Field, FieldType, MenuEntry, Subsystem, UserField};

/// `base` — папка конфигурации: пути в `Config.positions` считаются от неё.
pub(crate) fn into_model(files: ConfigFiles, base: &Path) -> Config {
    let positions = positions(&files, base);
    let mut manifest = files.manifest.value;
    let user_fields = std::mem::take(&mut manifest.user_fields)
        .into_iter()
        .map(user_field)
        .collect();
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
                cost: register.cost.map(|cost| {
                    let cost = cost.value;
                    Cost {
                        qty: cost.qty.value,
                        sum: cost.sum.value,
                        by: cost.by.into_iter().map(|d| d.value).collect(),
                    }
                }),
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
        roles: files
            .roles
            .into_iter()
            .map(|role| role_def(role.value))
            .collect(),
        user_fields,
        positions,
    }
}

/// Карта «элемент → файл и строка»: у объекта — строка имени, у поля — строка `name:`.
fn positions(files: &ConfigFiles, base: &Path) -> BTreeMap<String, Position> {
    let mut out = BTreeMap::new();
    let mut put = |key: String, file: &Path, name: &Spanned<String>| {
        let relative = file.strip_prefix(base).unwrap_or(file);
        let file = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let at = &name.referenced;
        let line = u32::try_from(at.line()).unwrap_or(u32::MAX);
        let column = u32::try_from(at.column()).unwrap_or(u32::MAX);
        out.insert(key, Position { file, line, column });
    };
    let manifest = &files.manifest;
    put("config".into(), &manifest.file, &manifest.value.name);
    for field in &manifest.value.user_fields {
        let key = format!("user_field:{}", field.name.value);
        put(key, &manifest.file, &field.name);
    }
    for catalog in &files.catalogs {
        let key = format!("catalog:{}", catalog.value.name.value);
        put(key.clone(), &catalog.file, &catalog.value.name);
        for field in &catalog.value.fields {
            put(
                format!("{key}/field:{}", field.name.value),
                &catalog.file,
                &field.name,
            );
        }
    }
    for document in &files.documents {
        let key = format!("document:{}", document.value.name.value);
        put(key.clone(), &document.file, &document.value.name);
        for field in &document.value.fields {
            put(
                format!("{key}/field:{}", field.name.value),
                &document.file,
                &field.name,
            );
        }
        for part in &document.value.tables {
            let part_key = format!("{key}/table:{}", part.name.value);
            put(part_key.clone(), &document.file, &part.name);
            for field in &part.fields {
                let field_key = format!("{part_key}/field:{}", field.name.value);
                put(field_key, &document.file, &field.name);
            }
        }
    }
    for register in &files.registers {
        let key = format!("register:{}", register.value.name.value);
        put(key.clone(), &register.file, &register.value.name);
        // Измерения и ресурсы — колонки одной таблицы, имена у них общие.
        for field in register
            .value
            .dimensions
            .iter()
            .chain(&register.value.resources)
        {
            put(
                format!("{key}/field:{}", field.name.value),
                &register.file,
                &field.name,
            );
        }
    }
    for subsystem in &files.subsystems {
        let key = format!("subsystem:{}", subsystem.value.name.value);
        put(key, &subsystem.file, &subsystem.value.name);
    }
    for role in &files.roles {
        let key = format!("role:{}", role.value.name.value);
        put(key, &role.file, &role.value.name);
    }
    out
}

fn user_field(field: UserField) -> UserFieldDef {
    let (kind, target) = kind(field.kind.value);
    UserFieldDef {
        field: FieldDef {
            name: field.name.value,
            title: field.title,
            kind,
            target,
            required: false,
            read_only: false,
        },
        many: field.many,
    }
}

/// Строки прав → флаги. Неизвестных прав здесь нет: их отсеял `check`.
fn role_def(role: crate::Role) -> RoleDef {
    let has = |list: &[Spanned<String>], right: &str| list.iter().any(|r| r.value == right);
    RoleDef {
        name: role.name.value,
        title: role.title,
        objects: role
            .objects
            .0
            .into_iter()
            .map(|(object, list)| {
                let rights = Rights {
                    read: has(&list, "read"),
                    create: has(&list, "create"),
                    update: has(&list, "update"),
                    post: has(&list, "post"),
                    unpost: has(&list, "unpost"),
                    delete: has(&list, "delete"),
                };
                (object.value, rights)
            })
            .collect(),
        system: SystemRights {
            closed_period: has(&role.system, "closed_period"),
            set_closing_date: has(&role.system, "set_closing_date"),
            admin: has(&role.system, "admin"),
            update_config: has(&role.system, "update_config"),
        },
        restrictions: role
            .restrictions
            .into_iter()
            .flat_map(|r| {
                let object = r.object.value;
                r.conditions.into_iter().map(move |c| Restriction {
                    object: object.clone(),
                    field: c.field.value,
                    // `user.` проверил check.
                    user_field: c
                        .user_field
                        .value
                        .strip_prefix(USER_PREFIX)
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .collect(),
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

fn kind(kind: FieldType) -> (FieldKind, Option<String>) {
    match kind {
        FieldType::Int => (FieldKind::Int, None),
        FieldType::Text => (FieldKind::Text, None),
        FieldType::Date => (FieldKind::Date, None),
        FieldType::Decimal { scale } => (FieldKind::Decimal { scale }, None),
        FieldType::Ref(target) => (FieldKind::Ref, Some(target)),
    }
}

fn field(field: Field) -> FieldDef {
    let (kind, target) = kind(field.kind.value);
    FieldDef {
        name: field.name.value,
        title: field.title,
        kind,
        target,
        required: field.required,
        read_only: field.read_only,
    }
}
