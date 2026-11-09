use std::fmt;

/// Место в тексте запроса: строка и столбец с 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: u64,
    pub column: u64,
}

impl Pos {
    pub(crate) fn from_span(span: sqlparser::tokenizer::Span) -> Option<Self> {
        // У синтетических узлов sqlparser строка 0: места нет.
        (span.start.line > 0).then_some(Self {
            line: span.start.line,
            column: span.start.column,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlError {
    /// Запрос длиннее `MAX_QUERY_LEN`.
    TooLong {
        len: usize,
    },
    /// Текст не разобрался как SQL; сообщение разборщика на английском.
    Parse(String),
    /// Пусто или больше одного оператора через `;`.
    NotOneStatement {
        count: usize,
    },
    /// Не `SELECT`: изменение данных, DDL, `PRAGMA` и прочее.
    OnlySelect,
    /// Конструкция вне белого списка.
    Forbidden {
        what: String,
        at: Option<Pos>,
    },
    /// Функция вне списка.
    Function {
        name: String,
        at: Option<Pos>,
    },
    /// Ошибка в аргументах разрешённой функции.
    FunctionArgs {
        name: String,
        need: &'static str,
    },
    /// Имя в кавычках: имена метаданных пишутся без кавычек.
    QuotedName {
        name: String,
        at: Option<Pos>,
    },
    NoTable {
        name: String,
    },
    NoField {
        field: String,
        table: String,
    },
    /// Поле без таблицы, а таблиц с таким полем несколько.
    Ambiguous {
        field: String,
    },
    /// В запросе нет таблицы или псевдонима с таким именем.
    NoSource {
        name: String,
    },
    /// Одно имя у двух таблиц в FROM: нужны разные псевдонимы.
    DuplicateSource {
        name: String,
    },
    NoRight {
        title: String,
    },
    MissingParam {
        name: String,
    },
    ExtraParam {
        name: String,
    },
    /// Плейсхолдер не вида `:имя` (`?`, `$1`, `@x`).
    BadParam {
        text: String,
    },
    /// Число не целое или не влезает в 64 бита.
    BadNumber {
        text: String,
    },
    /// Сложение или вычитание сумм с разным числом знаков после запятой.
    ScaleMismatch {
        left: u8,
        right: u8,
    },
    BadLimit,
    /// Слишком много токенов или слишком глубокое выражение.
    TooComplex,
    /// Сумма рядом с целым полем или целым числом, кроме 0: единицы разного смысла.
    MoneyWithInt {
        scale: u8,
    },
    /// SUM от текста или даты.
    SumNotNumber {
        kind: String,
    },
    /// Две колонки результата с одним именем.
    DuplicateColumn {
        name: String,
    },
    /// Символ с кодом 0 в тексте: PostgreSQL такой текст не принимает.
    NulInText,
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { len } => write!(
                f,
                "Запрос слишком длинный: {len} байт, можно до {}",
                crate::MAX_QUERY_LEN
            ),
            Self::Parse(e) => write!(f, "Не удалось разобрать запрос: {e}"),
            Self::NotOneStatement { count: 0 } => write!(f, "Запрос пустой"),
            Self::NotOneStatement { count } => {
                write!(f, "В запросе должен быть один оператор, а их {count}")
            }
            Self::OnlySelect => write!(f, "В запросе можно только SELECT"),
            Self::Forbidden { what, at } => {
                write!(f, "В запросе нельзя: {what}")?;
                write_pos(f, *at)
            }
            Self::Function { name, at } => {
                write!(
                    f,
                    "Нельзя функцию «{name}»: можно COUNT, SUM, MIN, MAX, COALESCE, LOWER"
                )?;
                write_pos(f, *at)
            }
            Self::FunctionArgs { name, need } => write!(f, "У {name} {need}"),
            Self::QuotedName { name, at } => {
                write!(f, "Имя «{name}» в кавычках: имена пишутся без кавычек")?;
                write_pos(f, *at)
            }
            Self::NoTable { name } => write!(f, "Нет таблицы «{name}» в конфигурации"),
            Self::NoField { field, table } => write!(f, "Нет поля «{field}» у «{table}»"),
            Self::Ambiguous { field } => write!(
                f,
                "Поле «{field}» есть у нескольких таблиц запроса: укажите таблицу"
            ),
            Self::NoSource { name } => write!(f, "В запросе нет таблицы «{name}»"),
            Self::DuplicateSource { name } => write!(
                f,
                "Таблица «{name}» в запросе дважды: дайте им разные псевдонимы (AS)"
            ),
            Self::NoRight { title } => write!(f, "Нет права на просмотр «{title}»"),
            Self::MissingParam { name } => write!(f, "Не передан параметр «{name}»"),
            Self::ExtraParam { name } => {
                write!(f, "Параметр «{name}» передан, но в запросе его нет")
            }
            Self::BadParam { text } => write!(
                f,
                "Параметр «{text}» не подходит: пишите параметры как :имя"
            ),
            Self::BadNumber { text } => write!(
                f,
                "Число «{text}» не подходит: можно только целые; суммы хранятся в копейках \
                 (единицах), дробное значение передайте параметром"
            ),
            Self::ScaleMismatch { left, right } => write!(
                f,
                "Нельзя складывать или вычитать числа с разным числом знаков после запятой \
                 ({left} и {right})"
            ),
            Self::BadLimit => write!(f, "LIMIT должен быть целым числом от 0"),
            Self::TooComplex => write!(
                f,
                "Запрос слишком сложный: больше {} частей или выражение глубже {}; \
                 длинные цепочки OR замените на IN (…)",
                crate::MAX_TOKENS,
                crate::MAX_DEPTH
            ),
            Self::MoneyWithInt { scale } => write!(
                f,
                "Сумма с {scale} знаками после запятой хранится в единицах: сравнивать и \
                 складывать её с целым числом нельзя (кроме 0), передайте значение параметром"
            ),
            Self::SumNotNumber { kind } => write!(f, "SUM можно только от чисел, а тут {kind}"),
            Self::DuplicateColumn { name } => write!(
                f,
                "Две колонки результата называются «{name}»: дайте им разные имена (AS)"
            ),
            Self::NulInText => write!(f, "В тексте нельзя символ с кодом 0"),
        }
    }
}

fn write_pos(f: &mut fmt::Formatter<'_>, at: Option<Pos>) -> fmt::Result {
    match at {
        Some(p) => write!(f, " (строка {}, столбец {})", p.line, p.column),
        None => Ok(()),
    }
}

impl std::error::Error for SqlError {}
