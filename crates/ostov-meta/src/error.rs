use std::fmt;
use std::path::PathBuf;

/// Ошибка или предупреждение в файле конфигурации: где, какой код и почему.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub file: PathBuf,
    /// Строка, считая с 1. `None`, когда место неизвестно: файл не открылся или слишком большой.
    pub line: Option<u64>,
    /// Постоянный код из [`codes`]: `E…` — ошибка, `W…` — предупреждение.
    pub code: &'static str,
    pub reason: String,
}

impl Error {
    pub fn new(
        file: impl Into<PathBuf>,
        line: Option<u64>,
        code: &'static str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            line,
            code,
            reason: reason.into(),
        }
    }

    /// Предупреждение: с ним конфигурация работает.
    pub fn is_warning(&self) -> bool {
        self.code.starts_with('W')
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = self.file.display();
        match self.line {
            Some(line) => write!(f, "{file}:{line}: {} {}", self.code, self.reason),
            None => write!(f, "{file}: {} {}", self.code, self.reason),
        }
    }
}

impl std::error::Error for Error {}

/// Коды `ostov check`. Код не меняется, когда меняется текст; новый — только в конец группы.
/// Список с описаниями — в README крейта, тест сверяет его с этими константами.
pub mod codes {
    // Чтение файлов и разбор YAML.
    pub const NO_MANIFEST: &str = "E001";
    pub const FORMAT_VERSION: &str = "E002";
    pub const READ_DIR: &str = "E003";
    pub const OPEN_FILE: &str = "E004";
    pub const YAML_SYNTAX: &str = "E005";
    pub const YAML_ANCHORS: &str = "E006";
    pub const YAML_DEPTH: &str = "E007";
    pub const FILE_TOO_LARGE: &str = "E008";
    pub const UNKNOWN_KEY: &str = "E009";
    pub const MISSING_KEY: &str = "E010";
    pub const UNKNOWN_VALUE: &str = "E011";
    pub const DUPLICATE_KEY: &str = "E012";

    // Имена.
    pub const NAME_FORM: &str = "E020";
    pub const DUPLICATE_NAME: &str = "E021";
    pub const SERVICE_PREFIX: &str = "E022";
    pub const RESERVED_NAME: &str = "E023";

    // Поля и значки.
    pub const DECIMAL_SCALE: &str = "E030";
    pub const REF_UNKNOWN: &str = "E031";
    pub const UNKNOWN_ICON: &str = "E032";

    // Регистры.
    pub const REGISTER_NO_DIMENSIONS: &str = "E040";
    pub const REGISTER_NO_RESOURCES: &str = "E041";
    pub const DIMENSION_TYPE: &str = "E042";
    pub const RESOURCE_TYPE: &str = "E043";
    pub const DOCUMENT_NO_REGISTER: &str = "E044";

    // Меню.
    pub const MENU_UNKNOWN_SECTION: &str = "E050";
    pub const MENU_NOT_LISTED: &str = "E051";
    pub const MENU_EMPTY_SECTION: &str = "E052";
    pub const MENU_EMPTY_GROUP: &str = "E053";
    pub const MENU_NESTED_GROUP: &str = "E054";
    pub const MENU_REGISTER: &str = "E055";
    pub const MENU_NO_OBJECT: &str = "E056";

    // Роли.
    pub const ROLE_ADMIN: &str = "E060";
    pub const UNKNOWN_RIGHT: &str = "E061";
    pub const ROLE_REGISTER: &str = "E062";
    pub const ROLE_NO_OBJECT: &str = "E063";
    pub const DOCUMENT_ONLY_RIGHT: &str = "E064";

    // Себестоимость регистра (ADR-042).
    pub const COST_NOT_RESOURCE: &str = "E070";
    pub const COST_SAME_RESOURCE: &str = "E071";
    pub const COST_EMPTY_BY: &str = "E072";
    pub const COST_NOT_DIMENSION: &str = "E073";

    // Свойства пользователя (ADR-043).
    pub const USER_FIELD_REF: &str = "E080";

    // Ограничения по записям (ADR-043).
    pub const RESTRICTION_NO_OBJECT: &str = "E090";
    pub const RESTRICTION_EMPTY: &str = "E091";
    pub const RESTRICTION_NO_FIELD: &str = "E092";
    pub const RESTRICTION_RESOURCE: &str = "E093";
    pub const RESTRICTION_NOT_USER: &str = "E094";
    pub const RESTRICTION_NO_USER_FIELD: &str = "E095";
    pub const RESTRICTION_TYPE: &str = "E096";

    // Предупреждения.
    pub const READ_FOR_REQUIRED: &str = "W001";

    /// Все коды по порядку: для проверки README и уникальности.
    pub const ALL: [&str; 49] = [
        NO_MANIFEST,
        FORMAT_VERSION,
        READ_DIR,
        OPEN_FILE,
        YAML_SYNTAX,
        YAML_ANCHORS,
        YAML_DEPTH,
        FILE_TOO_LARGE,
        UNKNOWN_KEY,
        MISSING_KEY,
        UNKNOWN_VALUE,
        DUPLICATE_KEY,
        NAME_FORM,
        DUPLICATE_NAME,
        SERVICE_PREFIX,
        RESERVED_NAME,
        DECIMAL_SCALE,
        REF_UNKNOWN,
        UNKNOWN_ICON,
        REGISTER_NO_DIMENSIONS,
        REGISTER_NO_RESOURCES,
        DIMENSION_TYPE,
        RESOURCE_TYPE,
        DOCUMENT_NO_REGISTER,
        MENU_UNKNOWN_SECTION,
        MENU_NOT_LISTED,
        MENU_EMPTY_SECTION,
        MENU_EMPTY_GROUP,
        MENU_NESTED_GROUP,
        MENU_REGISTER,
        MENU_NO_OBJECT,
        ROLE_ADMIN,
        UNKNOWN_RIGHT,
        ROLE_REGISTER,
        ROLE_NO_OBJECT,
        DOCUMENT_ONLY_RIGHT,
        COST_NOT_RESOURCE,
        COST_SAME_RESOURCE,
        COST_EMPTY_BY,
        COST_NOT_DIMENSION,
        USER_FIELD_REF,
        RESTRICTION_NO_OBJECT,
        RESTRICTION_EMPTY,
        RESTRICTION_NO_FIELD,
        RESTRICTION_RESOURCE,
        RESTRICTION_NOT_USER,
        RESTRICTION_NO_USER_FIELD,
        RESTRICTION_TYPE,
        READ_FOR_REQUIRED,
    ];
}
