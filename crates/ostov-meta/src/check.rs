//! Проверка конфигурации без базы (`ostov check`, СТ-02): ссылки, имена, типы, регистры.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde_saphyr::Spanned;

use crate::{Config, Document, Error, Field, FieldType, Register};

/// Лимит идентификатора PostgreSQL: из имён делаются таблицы и колонки (ADR-025).
pub const MAX_NAME_LEN: usize = 63;
/// Как `ostov_store::decimal::MAX_SCALE`: больший масштаб ядро не создаст.
pub const MAX_SCALE: u8 = 18;
/// Колонки, которые ядро добавляет в таблицы само.
pub const RESERVED_NAMES: [&str; 11] = [
    "id",
    "version",
    "deleted",
    "posted",
    "number",
    "doc_id",
    "line_no",
    "recorder",
    "recorder_id",
    "kind",
    "period",
];

/// Проверяет прочитанную конфигурацию и возвращает все ошибки, по файлам и строкам.
pub fn check(config: &Config) -> Vec<Error> {
    let mut errors = Vec::new();
    let mut objects = Names::new("объект");
    let catalogs = config.catalogs.iter().map(|c| (&c.file, &c.value.name));
    let documents = config.documents.iter().map(|d| (&d.file, &d.value.name));
    let registers = config.registers.iter().map(|r| (&r.file, &r.value.name));
    for (file, name) in catalogs.chain(documents).chain(registers) {
        objects.add(file, name, &mut errors);
    }

    let scope = Scope {
        // ref(X) ведёт на справочник или документ; регистр — не запись, на него не ссылаются.
        ref_targets: name_set(
            config
                .catalogs
                .iter()
                .map(|c| &c.value.name)
                .chain(config.documents.iter().map(|d| &d.value.name)),
        ),
        registers: name_set(config.registers.iter().map(|r| &r.value.name)),
    };

    for catalog in &config.catalogs {
        scope.check_fields(&catalog.file, &catalog.value.fields, &mut errors);
    }
    for document in &config.documents {
        scope.check_document(&document.file, &document.value, &mut errors);
    }
    for register in &config.registers {
        scope.check_register(&register.file, &register.value, &mut errors);
    }

    errors.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    errors
}

/// Имена объектов, на которые можно сослаться.
struct Scope<'a> {
    ref_targets: BTreeSet<&'a str>,
    registers: BTreeSet<&'a str>,
}

impl Scope<'_> {
    fn check_document(&self, file: &Path, document: &Document, errors: &mut Vec<Error>) {
        self.check_fields(file, &document.fields, errors);
        let mut parts = Names::new("табличная часть");
        for part in &document.tables {
            parts.add(file, &part.name, errors);
            self.check_fields(file, &part.fields, errors);
        }
        let mut used = Names::new("регистр");
        for register in &document.registers {
            used.add_repeat_only(file, register, errors);
            if !self.registers.contains(register.value.as_str()) {
                errors.push(at(
                    file,
                    register,
                    format!("нет регистра «{}»", register.value),
                ));
            }
        }
    }

    fn check_register(&self, file: &Path, register: &Register, errors: &mut Vec<Error>) {
        let name = &register.name;
        if register.dimensions.is_empty() {
            errors.push(at(
                file,
                name,
                format!("у регистра «{}» нет измерений", name.value),
            ));
        }
        if register.resources.is_empty() {
            errors.push(at(
                file,
                name,
                format!("у регистра «{}» нет ресурсов", name.value),
            ));
        }
        for dimension in &register.dimensions {
            if !matches!(
                dimension.kind.value,
                FieldType::Ref(_) | FieldType::Int | FieldType::Text
            ) {
                let reason = format!(
                    "измерение «{}» может быть только ref, int или text",
                    dimension.name.value
                );
                errors.push(at(file, &dimension.kind, reason));
            }
        }
        for resource in &register.resources {
            if !matches!(resource.kind.value, FieldType::Decimal { .. }) {
                let reason = format!("ресурс «{}» может быть только decimal", resource.name.value);
                errors.push(at(file, &resource.kind, reason));
            }
        }
        // Измерения и ресурсы — колонки одной таблицы движений, имена у них общие.
        self.check_fields(
            file,
            register.dimensions.iter().chain(&register.resources),
            errors,
        );
    }

    /// Поля одной таблицы: имена, повторы, занятые ядром имена, точность и ссылки.
    fn check_fields<'f>(
        &self,
        file: &Path,
        fields: impl IntoIterator<Item = &'f Field>,
        errors: &mut Vec<Error>,
    ) {
        let mut names = Names::new("поле");
        for field in fields {
            names.add(file, &field.name, errors);
            if RESERVED_NAMES.contains(&field.name.value.as_str()) {
                let reason = format!("имя «{}» занято ядром", field.name.value);
                errors.push(at(file, &field.name, reason));
            }
            match &field.kind.value {
                FieldType::Decimal { scale } if *scale > MAX_SCALE => {
                    let reason = format!("точность decimal({scale}) больше {MAX_SCALE}");
                    errors.push(at(file, &field.kind, reason));
                }
                FieldType::Ref(target) if !self.ref_targets.contains(target.as_str()) => {
                    let reason =
                        format!("нет справочника или документа «{target}» для ref({target})");
                    errors.push(at(file, &field.kind, reason));
                }
                FieldType::Int
                | FieldType::Text
                | FieldType::Date
                | FieldType::Decimal { .. }
                | FieldType::Ref(_) => {}
            }
        }
    }
}

/// Имена одной области: проверка вида имени и повторов.
struct Names<'a> {
    what: &'static str,
    first: HashMap<String, (&'a Path, u64)>,
}

impl<'a> Names<'a> {
    fn new(what: &'static str) -> Self {
        Self {
            what,
            first: HashMap::new(),
        }
    }

    fn add(&mut self, file: &'a Path, name: &Spanned<String>, errors: &mut Vec<Error>) {
        if let Some(problem) = name_problem(&name.value) {
            errors.push(at(file, name, problem));
        }
        self.add_repeat_only(file, name, errors);
    }

    /// Только повтор, без проверки вида: для ссылок на уже объявленные имена.
    fn add_repeat_only(&mut self, file: &'a Path, name: &Spanned<String>, errors: &mut Vec<Error>) {
        if let Some((first_file, first_line)) = self.first.get(&name.value) {
            let reason = format!(
                "{} «{}» повторяется, первое объявление: {}:{first_line}",
                self.what,
                name.value,
                first_file.display()
            );
            errors.push(at(file, name, reason));
        } else {
            self.first
                .insert(name.value.clone(), (file, name.referenced.line()));
        }
    }
}

/// Имя станет идентификатором в базе: строчная латиница, цифры и `_`, первой — буква.
fn name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("пустое имя".into());
    }
    if !name.is_ascii() {
        return Some(format!(
            "имя «{name}» не ASCII: пиши латиницей, русское название — в title"
        ));
    }
    if name.len() > MAX_NAME_LEN {
        return Some(format!("имя «{name}» длиннее {MAX_NAME_LEN} символов"));
    }
    let mut chars = name.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_allowed = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !starts_with_letter || !rest_allowed {
        return Some(format!(
            "имя «{name}»: допустимы строчные латинские буквы, цифры и _, первой — буква"
        ));
    }
    None
}

fn name_set<'a>(names: impl Iterator<Item = &'a Spanned<String>>) -> BTreeSet<&'a str> {
    names.map(|name| name.value.as_str()).collect()
}

fn at<T>(file: &Path, value: &Spanned<T>, reason: String) -> Error {
    Error::new(file, Some(value.referenced.line()), reason)
}

#[cfg(test)]
mod tests {
    use super::name_problem;

    #[test]
    fn name_rules() {
        assert_eq!(name_problem("goods"), None);
        assert_eq!(name_problem("line_2"), None);
        assert_eq!(name_problem(&"a".repeat(63)), None);
        for bad in [
            "",
            "Товары",
            "Goods",
            "2goods",
            "_goods",
            "goods-list",
            "goods list",
        ] {
            assert!(name_problem(bad).is_some(), "{bad:?} должно быть ошибкой");
        }
        assert!(name_problem(&"a".repeat(64)).is_some());
    }
}
