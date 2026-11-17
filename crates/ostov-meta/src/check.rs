//! Проверка конфигурации без базы (`ostov check`, СТ-02): ссылки, имена, типы, регистры, меню, роли.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use ostov_model::ADMIN_ROLE;
use serde_saphyr::Spanned;

use crate::error::codes;
use crate::model::USER_PREFIX;

use crate::{
    ConfigFiles, Document, Error, Field, FieldType, Located, MenuEntry, Register, RegisterCost,
    RoleRestriction, Subsystem,
};

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
/// Права роли на справочник или документ.
pub const OBJECT_RIGHTS: [&str; 6] = ["read", "create", "update", "post", "unpost", "delete"];
/// Проводить можно только документы.
const DOCUMENT_ONLY_RIGHTS: [&str; 2] = ["post", "unpost"];
/// Права роли на всю базу.
pub const SYSTEM_RIGHTS: [&str; 4] = [
    "closed_period",
    "set_closing_date",
    "admin",
    "update_config",
];
/// Так начинаются служебные таблицы платформы (`ostov_users`): объекту и роли такое имя нельзя.
pub const SERVICE_PREFIX: &str = "ostov_";
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
        check_service_prefix(file, name, &mut errors);
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
        catalogs: name_set(config.catalogs.iter().map(|c| &c.value.name)),
        documents: name_set(config.documents.iter().map(|d| &d.value.name)),
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
        if let Some(cost) = &register.value.cost {
            check_cost(&register.file, &register.value, cost, &mut errors);
        }
    }
    check_user_fields(config, &scope, &mut errors);
    check_menu(config, &scope, &mut errors);
    check_roles(config, &scope, &mut errors);

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
                codes::MENU_UNKNOWN_SECTION,
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
            errors.push(at(&subsystem.file, name, codes::MENU_NOT_LISTED, reason));
        }
    }
}

/// Роли: имя как у объектов, права на справочники и документы, системные права.
fn check_roles(config: &ConfigFiles, scope: &Scope<'_>, errors: &mut Vec<Error>) {
    let mut roles = Names::new("роль");
    for role in &config.roles {
        let (file, role) = (&role.file, &role.value);
        roles.add(file, &role.name, errors);
        check_service_prefix(file, &role.name, errors);
        if role.name.value == ADMIN_ROLE {
            let reason = "роль admin встроенная, со всеми правами: в YAML её не описывают".into();
            errors.push(at(file, &role.name, codes::ROLE_ADMIN, reason));
        }
        for (object, rights) in &role.objects.0 {
            scope.check_role_object(file, object, rights, errors);
        }
        check_rights_list(
            file,
            &role.system,
            &SYSTEM_RIGHTS,
            "системное право",
            errors,
        );
        for restriction in &role.restrictions {
            check_restriction(config, file, restriction, errors);
        }
    }
}

/// `cost:` регистра: два разных ресурса и измерения средней без повторов (ADR-042).
fn check_cost(
    file: &Path,
    register: &Register,
    cost: &Spanned<RegisterCost>,
    errors: &mut Vec<Error>,
) {
    let value = &cost.value;
    for (name, what) in [(&value.qty, "количество"), (&value.sum, "сумма")] {
        if !register
            .resources
            .iter()
            .any(|r| r.name.value == name.value)
        {
            let reason = format!(
                "себестоимость: {what} «{}» — не ресурс регистра «{}»",
                name.value, register.name.value
            );
            errors.push(at(file, name, codes::COST_NOT_RESOURCE, reason));
        }
    }
    if value.qty.value == value.sum.value {
        let reason = "себестоимость: количество и сумма должны быть разными ресурсами".into();
        errors.push(at(file, &value.sum, codes::COST_SAME_RESOURCE, reason));
    }
    if value.by.is_empty() {
        let reason = "себестоимость: в by нужно хотя бы одно измерение".into();
        errors.push(at(file, cost, codes::COST_EMPTY_BY, reason));
    }
    let mut seen = Names::new("измерение в by");
    for dimension in &value.by {
        seen.add_repeat_only(file, dimension, errors);
        if !register
            .dimensions
            .iter()
            .any(|d| d.name.value == dimension.value)
        {
            let reason = format!(
                "себестоимость: «{}» — не измерение регистра «{}»",
                dimension.value, register.name.value
            );
            errors.push(at(file, dimension, codes::COST_NOT_DIMENSION, reason));
        }
    }
}

/// Свойства пользователя: имена как у полей, ссылка — только на справочник (ADR-043).
fn check_user_fields(config: &ConfigFiles, scope: &Scope<'_>, errors: &mut Vec<Error>) {
    let file = &config.manifest.file;
    let mut names = Names::new("свойство пользователя");
    for field in &config.manifest.value.user_fields {
        names.add(file, &field.name, errors);
        match &field.kind.value {
            FieldType::Decimal { scale } if *scale > MAX_SCALE => {
                let reason = format!("точность decimal({scale}) больше {MAX_SCALE}");
                errors.push(at(file, &field.kind, codes::DECIMAL_SCALE, reason));
            }
            FieldType::Ref(target) if !scope.catalogs.contains(target.as_str()) => {
                let reason = format!(
                    "свойство пользователя ссылается только на справочник, а «{target}» — не справочник"
                );
                errors.push(at(file, &field.kind, codes::USER_FIELD_REF, reason));
            }
            FieldType::Int
            | FieldType::Text
            | FieldType::Date
            | FieldType::Decimal { .. }
            | FieldType::Ref(_) => {}
        }
    }
}

/// Ограничение роли: объект, поле (у регистра — измерение) и свойство пользователя того же типа.
fn check_restriction(
    config: &ConfigFiles,
    file: &Path,
    restriction: &RoleRestriction,
    errors: &mut Vec<Error>,
) {
    let object = &restriction.object;
    let name = object.value.as_str();
    let catalog = config.catalogs.iter().find(|c| c.value.name.value == name);
    let document = config.documents.iter().find(|d| d.value.name.value == name);
    let register = config.registers.iter().find(|r| r.value.name.value == name);
    let fields: &[Field] = match (catalog, document, register) {
        (Some(c), _, _) => &c.value.fields,
        (_, Some(d), _) => &d.value.fields,
        (_, _, Some(r)) => &r.value.dimensions,
        _ => {
            let reason = format!("ограничение: нет справочника, документа или регистра «{name}»");
            errors.push(at(file, object, codes::RESTRICTION_NO_OBJECT, reason));
            return;
        }
    };
    if restriction.conditions.is_empty() {
        let reason = format!("ограничение на «{name}»: в where нет условий");
        errors.push(at(file, object, codes::RESTRICTION_EMPTY, reason));
    }
    for condition in &restriction.conditions {
        let wanted = condition.field.value.as_str();
        let Some(field) = fields.iter().find(|f| f.name.value == wanted) else {
            let (code, reason) = if is_resource(register, wanted) {
                (
                    codes::RESTRICTION_RESOURCE,
                    format!(
                        "ограничение на регистр «{name}»: «{wanted}» — ресурс, ограничивают по измерениям"
                    ),
                )
            } else {
                (
                    codes::RESTRICTION_NO_FIELD,
                    format!("ограничение: у «{name}» нет поля «{wanted}»"),
                )
            };
            errors.push(at(file, &condition.field, code, reason));
            continue;
        };
        let source = &condition.user_field;
        let Some(user_name) = source.value.strip_prefix(USER_PREFIX) else {
            let reason = format!(
                "ограничение: «{}» — нужно свойство пользователя вида user.<имя>",
                source.value
            );
            errors.push(at(file, source, codes::RESTRICTION_NOT_USER, reason));
            continue;
        };
        let users = &config.manifest.value.user_fields;
        let Some(user) = users.iter().find(|u| u.name.value == user_name) else {
            let reason = format!(
                "ограничение: нет свойства пользователя «{user_name}» в user_fields {}",
                config.manifest.file.display()
            );
            errors.push(at(file, source, codes::RESTRICTION_NO_USER_FIELD, reason));
            continue;
        };
        if user.kind.value != field.kind.value {
            let reason =
                format!("ограничение: поле «{wanted}» и свойство «{user_name}» разного типа");
            errors.push(at(file, source, codes::RESTRICTION_TYPE, reason));
        }
    }
}

fn is_resource(register: Option<&Located<Register>>, field: &str) -> bool {
    register.is_some_and(|r| r.value.resources.iter().any(|f| f.name.value == field))
}

/// Предупреждения: конфигурация рабочая, но пользователь упрётся в права.
///
/// ADR-044: роль может создать документ, но не читает справочник или документ, на который
/// ссылается обязательное поле (в шапке или табличной части), — такой документ ей не заполнить.
pub fn warnings(config: &ConfigFiles) -> Vec<Error> {
    let mut out = Vec::new();
    for role in &config.roles {
        let (file, role) = (&role.file, &role.value);
        let has = |object: &str, right: &str| {
            role.objects
                .0
                .iter()
                .filter(|(name, _)| name.value == object)
                .any(|(_, list)| list.iter().any(|r| r.value == right))
        };
        for (object, _) in &role.objects.0 {
            if !has(&object.value, "create") {
                continue;
            }
            let Some(document) = config
                .documents
                .iter()
                .find(|d| d.value.name.value == object.value)
            else {
                continue;
            };
            let document = &document.value;
            let fields = document
                .fields
                .iter()
                .chain(document.tables.iter().flat_map(|t| &t.fields));
            let mut reported = BTreeSet::new();
            for field in fields.filter(|f| f.required) {
                let FieldType::Ref(target) = &field.kind.value else {
                    continue;
                };
                if !has(target, "read") && reported.insert(target.as_str()) {
                    let reason = format!(
                        "роль «{}» создаёт «{}», но не читает «{target}» \
                         (обязательное поле «{}»): пользователь не сможет заполнить документ",
                        role.name.value, document.name.value, field.name.value
                    );
                    out.push(at(file, object, codes::READ_FOR_REQUIRED, reason));
                }
            }
        }
    }
    out
}

/// Каждое право из списка `allowed` и не больше одного раза.
fn check_rights_list(
    file: &Path,
    rights: &[Spanned<String>],
    allowed: &[&str],
    what: &'static str,
    errors: &mut Vec<Error>,
) {
    let mut seen = Names::new(what);
    for right in rights {
        seen.add_repeat_only(file, right, errors);
        if !allowed.contains(&right.value.as_str()) {
            let reason = format!(
                "неизвестное {what} «{}»: допустимы {}",
                right.value,
                allowed.join(", ")
            );
            errors.push(at(file, right, codes::UNKNOWN_RIGHT, reason));
        }
    }
}

fn check_service_prefix(file: &Path, name: &Spanned<String>, errors: &mut Vec<Error>) {
    if name.value.starts_with(SERVICE_PREFIX) {
        let reason = format!(
            "имя «{}» начинается с {SERVICE_PREFIX}: так называются служебные таблицы",
            name.value
        );
        errors.push(at(file, name, codes::SERVICE_PREFIX, reason));
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
        errors.push(at(file, icon, codes::UNKNOWN_ICON, reason));
    }
}

/// Имена объектов, на которые можно сослаться.
struct Scope<'a> {
    ref_targets: BTreeSet<&'a str>,
    catalogs: BTreeSet<&'a str>,
    documents: BTreeSet<&'a str>,
    registers: BTreeSet<&'a str>,
}

impl Scope<'_> {
    /// В роли права задаются на справочники и документы; `post`/`unpost` — только документам.
    fn check_role_object(
        &self,
        file: &Path,
        object: &Spanned<String>,
        rights: &[Spanned<String>],
        errors: &mut Vec<Error>,
    ) {
        let name = object.value.as_str();
        if self.registers.contains(name) {
            let reason =
                format!("права на регистр «{name}» не задаются: в роли справочники и документы");
            errors.push(at(file, object, codes::ROLE_REGISTER, reason));
        } else if !self.ref_targets.contains(name) {
            errors.push(at(
                file,
                object,
                codes::ROLE_NO_OBJECT,
                format!("нет справочника или документа «{name}»"),
            ));
        }
        check_rights_list(file, rights, &OBJECT_RIGHTS, "право", errors);
        if self.ref_targets.contains(name) && !self.documents.contains(name) {
            for right in rights {
                if DOCUMENT_ONLY_RIGHTS.contains(&right.value.as_str()) {
                    let reason = format!(
                        "право {} только у документов, а «{name}» — справочник",
                        right.value
                    );
                    errors.push(at(file, right, codes::DOCUMENT_ONLY_RIGHT, reason));
                }
            }
        }
    }

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
                    codes::DOCUMENT_NO_REGISTER,
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
            errors.push(at(file, &subsystem.name, codes::MENU_EMPTY_SECTION, reason));
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
                        errors.push(at(file, entry, codes::MENU_REGISTER, reason));
                    } else if !self.ref_targets.contains(name.as_str()) {
                        let reason = format!("нет справочника или документа «{name}»");
                        errors.push(at(file, entry, codes::MENU_NO_OBJECT, reason));
                    }
                }
                MenuEntry::Group(group) if groups_allowed => {
                    check_icon(file, group.icon.as_ref(), errors);
                    if group.objects.is_empty() {
                        let reason = format!("в группе «{}» нет объектов", group.title);
                        errors.push(at(file, entry, codes::MENU_EMPTY_GROUP, reason));
                    }
                    self.check_entries(file, &group.objects, false, items, errors);
                }
                MenuEntry::Group(group) => {
                    let reason = format!(
                        "группа «{}» внутри группы: в меню допустим один уровень групп",
                        group.title
                    );
                    errors.push(at(file, entry, codes::MENU_NESTED_GROUP, reason));
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
                codes::REGISTER_NO_DIMENSIONS,
                format!("у регистра «{}» нет измерений", name.value),
            ));
        }
        if register.resources.is_empty() {
            errors.push(at(
                file,
                name,
                codes::REGISTER_NO_RESOURCES,
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
                errors.push(at(file, &dimension.kind, codes::DIMENSION_TYPE, reason));
            }
        }
        for resource in &register.resources {
            if !matches!(resource.kind.value, FieldType::Decimal { .. }) {
                let reason = format!("ресурс «{}» может быть только decimal", resource.name.value);
                errors.push(at(file, &resource.kind, codes::RESOURCE_TYPE, reason));
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
                errors.push(at(file, &field.name, codes::RESERVED_NAME, reason));
            }
            match &field.kind.value {
                FieldType::Decimal { scale } if *scale > MAX_SCALE => {
                    let reason = format!("точность decimal({scale}) больше {MAX_SCALE}");
                    errors.push(at(file, &field.kind, codes::DECIMAL_SCALE, reason));
                }
                FieldType::Ref(target) if !self.ref_targets.contains(target.as_str()) => {
                    let reason =
                        format!("нет справочника или документа «{target}» для ref({target})");
                    errors.push(at(file, &field.kind, codes::REF_UNKNOWN, reason));
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
            errors.push(at(file, name, codes::NAME_FORM, problem));
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
            errors.push(Error::new(file, Some(line), codes::DUPLICATE_NAME, reason));
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

fn at<T>(file: &Path, value: &Spanned<T>, code: &'static str, reason: String) -> Error {
    Error::new(file, Some(value.referenced.line()), code, reason)
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
