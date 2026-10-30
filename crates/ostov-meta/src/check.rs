//! Проверка конфигурации без базы (`ostov check`, СТ-02): ссылки, имена, типы, регистры.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde_saphyr::Spanned;

use crate::{ConfigFiles, Document, Error, Field, FieldType, MenuEntry, Register, Subsystem};

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
/// Значки интерфейса: файлы `ostov-ui/assets/icons/*.svg` без расширения; тест сверяет с папкой.
pub const ICONS: [&str; 34] = [
    "bar-chart",
    "bell",
    "box",
    "calendar",
    "check",
    "chevron-down",
    "chevron-right",
    "chevrons-left",
    "chevrons-right",
    "close",
    "database",
    "document",
    "error",
    "folder",
    "help",
    "home",
    "inventory",
    "list",
    "lock",
    "minus",
    "pin",
    "plus",
    "recipe",
    "search",
    "settings",
    "star",
    "star-filled",
    "stock-in",
    "stock-out",
    "success",
    "transfer",
    "user",
    "users",
    "warehouse",
];

/// Проверяет прочитанную конфигурацию и возвращает все ошибки, по файлам и строкам.
pub fn check(config: &ConfigFiles) -> Vec<Error> {
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
        check_icon(&catalog.file, catalog.value.icon.as_ref(), &mut errors);
        scope.check_fields(&catalog.file, &catalog.value.fields, &mut errors);
    }
    for document in &config.documents {
        check_icon(&document.file, document.value.icon.as_ref(), &mut errors);
        scope.check_document(&document.file, &document.value, &mut errors);
    }
    for register in &config.registers {
        scope.check_register(&register.file, &register.value, &mut errors);
    }
    check_menu(config, &scope, &mut errors);

    errors.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    errors
}

/// Меню: каждый раздел из `subsystems/` перечислен в `menu:` манифеста и наоборот.
fn check_menu(config: &ConfigFiles, scope: &Scope<'_>, errors: &mut Vec<Error>) {
    let mut sections = Names::new("раздел меню");
    for subsystem in &config.subsystems {
        sections.add(&subsystem.file, &subsystem.value.name, errors);
        scope.check_subsystem(&subsystem.file, &subsystem.value, errors);
    }
    let manifest = &config.manifest;
    let mut listed = Names::new("раздел в menu");
    for name in &manifest.value.menu {
        listed.add_repeat_only(&manifest.file, name, errors);
        if !sections.contains(&name.value) {
            errors.push(at(
                &manifest.file,
                name,
                format!("нет раздела меню «{}»", name.value),
            ));
        }
    }
    for subsystem in &config.subsystems {
        let name = &subsystem.value.name;
        if !listed.contains(&name.value) {
            let reason = format!(
                "раздела «{}» нет в списке menu в {}",
                name.value,
                manifest.file.display()
            );
            errors.push(at(&subsystem.file, name, reason));
        }
    }
}

fn check_icon(file: &Path, icon: Option<&Spanned<String>>, errors: &mut Vec<Error>) {
    if let Some(icon) = icon
        && !ICONS.contains(&icon.value.as_str())
    {
        let reason = format!(
            "нет значка «{}»: допустимы имена файлов ostov-ui/assets/icons без .svg",
            icon.value
        );
        errors.push(at(file, icon, reason));
    }
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

    /// Раздел → группа → объекты; объект встречается в разделе один раз, даже в разных группах.
    fn check_subsystem(&self, file: &Path, subsystem: &Subsystem, errors: &mut Vec<Error>) {
        check_icon(file, subsystem.icon.as_ref(), errors);
        if subsystem.objects.is_empty() {
            let reason = format!("в разделе «{}» нет объектов", subsystem.name.value);
            errors.push(at(file, &subsystem.name, reason));
        }
        let mut items = Names::new("пункт меню");
        self.check_entries(file, &subsystem.objects, true, &mut items, errors);
    }

    fn check_entries<'a>(
        &self,
        file: &'a Path,
        entries: &[Spanned<MenuEntry>],
        groups_allowed: bool,
        items: &mut Names<'a>,
        errors: &mut Vec<Error>,
    ) {
        for entry in entries {
            match &entry.value {
                MenuEntry::Object(name) => {
                    items.repeat(file, name, entry.referenced.line(), errors);
                    if self.registers.contains(name.as_str()) {
                        let reason = format!(
                            "регистр «{name}» нельзя открыть из меню: там справочники и документы"
                        );
                        errors.push(at(file, entry, reason));
                    } else if !self.ref_targets.contains(name.as_str()) {
                        let reason = format!("нет справочника или документа «{name}»");
                        errors.push(at(file, entry, reason));
                    }
                }
                MenuEntry::Group(group) if groups_allowed => {
                    check_icon(file, group.icon.as_ref(), errors);
                    if group.objects.is_empty() {
                        let reason = format!("в группе «{}» нет объектов", group.title);
                        errors.push(at(file, entry, reason));
                    }
                    self.check_entries(file, &group.objects, false, items, errors);
                }
                MenuEntry::Group(group) => {
                    let reason = format!(
                        "группа «{}» внутри группы: в меню допустим один уровень групп",
                        group.title
                    );
                    errors.push(at(file, entry, reason));
                }
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
        self.repeat(file, &name.value, name.referenced.line(), errors);
    }

    fn repeat(&mut self, file: &'a Path, name: &str, line: u64, errors: &mut Vec<Error>) {
        if let Some((first_file, first_line)) = self.first.get(name) {
            let reason = format!(
                "{} «{name}» повторяется, первое объявление: {}:{first_line}",
                self.what,
                first_file.display()
            );
            errors.push(Error::new(file, Some(line), reason));
        } else {
            self.first.insert(name.to_owned(), (file, line));
        }
    }

    fn contains(&self, name: &str) -> bool {
        self.first.contains_key(name)
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
