//! Что GetMeta отдаст интерфейсу для конфигурации: `cargo run -p ostov-meta --example meta [папка]`.
//! Без папки — встроенный «Склад» (`ostov_meta::demo()`).

use std::path::Path;
use std::process::ExitCode;

use ostov_model::{FieldDef, FieldKind, MenuItem, Meta, ObjectKind};

fn main() -> ExitCode {
    let config = match std::env::args().nth(1) {
        Some(dir) => ostov_meta::load(Path::new(&dir)),
        None => ostov_meta::demo(),
    };
    match config {
        Ok(config) => {
            print!("{}", render(&Meta::new(&config)));
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}

fn render(meta: &Meta) -> String {
    let mut out = String::from("Меню\n");
    for item in &meta.menu {
        out += &menu_item(item, "  ");
    }
    out += "\nОбъекты\n";
    for object in &meta.objects {
        let kind = match object.kind {
            ObjectKind::Catalog => "справочник",
            ObjectKind::Document => "документ",
        };
        out += &format!(
            "  {} [{}], {kind}{}\n",
            object.title,
            object.name,
            icon(object.icon.as_deref())
        );
        out += &fields(&object.fields, "    ");
        for part in &object.table_parts {
            out += &format!("    табличная часть {} [{}]\n", part.title, part.name);
            out += &fields(&part.fields, "      ");
        }
    }
    out += &format!("\nРоли: {:?}\n", meta.roles);
    out
}

fn menu_item(item: &MenuItem, indent: &str) -> String {
    let what = match &item.object {
        Some(object) => format!(" → {object}"),
        None => " (группа)".to_owned(),
    };
    let mut out = format!(
        "{indent}{}{what}{}\n",
        item.title,
        icon(item.icon.as_deref())
    );
    for child in &item.children {
        out += &menu_item(child, &format!("{indent}  "));
    }
    out
}

fn icon(icon: Option<&str>) -> String {
    icon.map(|icon| format!(", значок {icon}"))
        .unwrap_or_default()
}

fn fields(fields: &[FieldDef], indent: &str) -> String {
    fields
        .iter()
        .map(|field| {
            let target = match (&field.kind, &field.target) {
                (FieldKind::Ref, Some(target)) => format!(" → {target}"),
                _ => String::new(),
            };
            let required = if field.required {
                ", обязательное"
            } else {
                ""
            };
            let read_only = if field.read_only {
                ", только чтение"
            } else {
                ""
            };
            format!(
                "{indent}{} [{}]: {}{target}{required}{read_only}\n",
                field.title, field.name, field.kind
            )
        })
        .collect()
}
