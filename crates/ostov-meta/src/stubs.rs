//! Подсказки для IDE (СТ-03): `config/_stubs/<документ>.pyi` с классом документа и классами
//! строк табличных частей. Генерируются командой `ostov stubs`, в git не хранятся.

use ostov_model::{Config, DocumentDef, FieldDef, FieldKind};

/// Папка заглушек внутри `config/`.
pub const STUBS_DIR: &str = "_stubs";

const HEADER: &str =
    "# Сгенерировано командой ostov stubs по config/. Не править: файл перезапишется.\n";

/// Файлы заглушек: (имя файла, текст). Первый — `__init__.pyi` пакета `_stubs`.
pub fn stubs(config: &Config) -> Vec<(String, String)> {
    let mut files = vec![("__init__.pyi".to_owned(), HEADER.to_owned())];
    files.extend(
        config
            .documents
            .iter()
            .map(|document| (format!("{}.pyi", document.name), document_stub(document))),
    );
    files
}

fn document_stub(document: &DocumentDef) -> String {
    let class = class_name(&document.name);
    let mut text = format!("{HEADER}\nimport datetime\nfrom decimal import Decimal\n");
    for part in &document.table_parts {
        text.push_str(&format!(
            "\n\nclass {class}{}:\n    \"\"\"Строка табличной части «{}».\"\"\"\n\n",
            class_name(&part.name),
            part.title
        ));
        text.push_str(&fields(&part.fields));
    }
    text.push_str(&format!(
        "\n\nclass {class}:\n    \"\"\"Документ «{}».\"\"\"\n\n    id: int | None\n    \
         number: int | None\n    posted: bool\n",
        document.title
    ));
    text.push_str(&fields(&document.fields));
    for part in &document.table_parts {
        text.push_str(&format!(
            "    {}: tuple[{class}{}, ...]\n",
            part.name,
            class_name(&part.name)
        ));
    }
    text
}

fn fields(fields: &[FieldDef]) -> String {
    fields
        .iter()
        .map(|field| {
            let python = match field.kind {
                FieldKind::Int | FieldKind::Ref => "int",
                FieldKind::Text => "str",
                FieldKind::Decimal { .. } => "Decimal",
                FieldKind::Date => "datetime.date",
            };
            let comment = match (&field.kind, &field.target) {
                (FieldKind::Ref, Some(target)) => {
                    format!("  # {}, ссылка на {target}", field.title)
                }
                _ => format!("  # {}", field.title),
            };
            format!("    {}: {python} | None{comment}\n", field.name)
        })
        .collect()
}

/// `receipt` → `Receipt`, `price_list` → `PriceList`.
fn class_name(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::class_name;

    #[test]
    fn class_names_are_camel_case() {
        assert_eq!(class_name("receipt"), "Receipt");
        assert_eq!(class_name("price_list"), "PriceList");
        assert_eq!(class_name("doc_2"), "Doc2");
    }
}
