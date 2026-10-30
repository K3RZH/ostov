//! Что GetMeta отдаст интерфейсу для конфигурации: `cargo run -p ostov-meta --example meta [папка]`.
//! Без папки — встроенный «Склад» (`ostov_meta::demo()`).

use std::path::Path;
use std::process::ExitCode;

use ostov_model::{FieldDef, FieldKind, Meta, ObjectKind};

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
    for section in &meta.menu {
        out += &format!("  {} [{}]\n", section.title, section.name);
        for item in &section.items {
            out += &format!(
                "    {} [{}], {}\n",
                item.title,
                item.object,
                kind(item.kind)
            );
        }
    }
    out += "\nОбъекты\n";
    for object in &meta.objects {
        out += &format!(
            "  {} [{}], {}\n",
            object.title,
            object.name,
            kind(object.kind)
        );
        out += &fields(&object.fields, "    ");
        for part in &object.table_parts {
            out += &format!("    табличная часть {} [{}]\n", part.title, part.name);
            out += &fields(&part.fields, "      ");
        }
    }
    out
}

fn fields(fields: &[FieldDef], indent: &str) -> String {
    fields
        .iter()
        .map(|field| {
            let target = match (&field.kind, &field.target) {
                (FieldKind::Ref, Some(target)) => format!(" → {target}"),
                _ => String::new(),
            };
            format!(
                "{indent}{} [{}]: {}{target}\n",
                field.title, field.name, field.kind
            )
        })
        .collect()
}

fn kind(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Catalog => "справочник",
        ObjectKind::Document => "документ",
    }
}
