//! Показ и разбор значений полей по типу поля (`FieldKind`): общее для списка, формы и ячеек,
//! чтобы экран не знал, как записывается сумма или дата. Деньги и количества это целые единицы
//! и число знаков после запятой (ADR-010), без `f32` и `f64`.

use ostov_api::{FieldKind, Value};

/// Неразрывный пробел между разрядами: сумма не переносится посреди числа.
const GROUP_SEPARATOR: char = '\u{a0}';

/// Значение поля текстом для людей. Тип поля задаёт число знаков десятичного (лишние знаки
/// значения отбрасываются, недостающие добавляются нулями, как в колонке); всё остальное видно
/// по самому значению. Значение не того типа, что поле, показывается как есть, а не скрывается.
pub fn display(value: &Value, kind: FieldKind) -> String {
    match value {
        Value::Null => String::new(),
        Value::Int(number) => number.to_string(),
        Value::Text(text) => text.clone(),
        Value::Decimal { units, scale } => {
            let shown = match kind {
                FieldKind::Decimal { scale: field_scale } => field_scale,
                FieldKind::Int | FieldKind::Text | FieldKind::Date | FieldKind::Ref => *scale,
            };
            decimal_text(*units, *scale, shown)
        }
        Value::Bool(flag) => if *flag { "Да" } else { "Нет" }.to_owned(),
        // Дату, которую не разобрать, не показываем: в ячейке лучше пусто, чем «2026-1x-05».
        Value::Date(iso) => parse_iso_date(iso).map(date_text).unwrap_or_default(),
        // Без названия ссылка показывается номером: ядро заполняет название не везде.
        Value::Ref { id, title } => title.clone().unwrap_or_else(|| id.to_string()),
    }
}

/// Введённый текст в значение поля. Пустой текст это `Value::Null`: обязательность поля проверяет
/// ядро при записи. Числа пока без знака: отрицательных в формах нет; пробелы между разрядами
/// допустимы, их ставит сам `display`. Ссылку текстом не ввести, её выбирают из справочника.
/// Ошибка: текст для человека.
///
/// # Errors
/// Текст не разобрать как значение этого типа поля.
pub fn parse(text: &str, kind: FieldKind) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    // Разряды `display` разделяет неразрывным пробелом: без них число разбирается как есть.
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    match kind {
        FieldKind::Int => parse_decimal(&compact, 0)
            .map(Value::Int)
            .ok_or_else(|| decimal_error(0)),
        FieldKind::Text => Ok(Value::Text(text.trim().to_owned())),
        FieldKind::Decimal { scale } => parse_decimal(&compact, u32::from(scale))
            .map(|units| Value::Decimal { units, scale })
            .ok_or_else(|| decimal_error(scale)),
        FieldKind::Date => parse_date(text)
            .map(|(year, month, day)| Value::Date(format!("{year:04}-{month:02}-{day:02}")))
            .ok_or_else(|| "Введите дату в виде дд.мм.гггг.".to_owned()),
        FieldKind::Ref => Err("Ссылку выбирают из справочника, текстом её не ввести.".to_owned()),
    }
}

fn decimal_error(scale: u8) -> String {
    if scale == 0 {
        "Введите целое число без знака.".to_owned()
    } else {
        format!("Введите число без знака (после запятой не больше {scale} знаков).")
    }
}

/// Цифры числа: признак «меньше нуля», целая часть и дробная ровно из `scale` цифр. Строками,
/// без деления на степени десяти: так нет переполнения при любом `scale` и при `i64::MIN`.
fn decimal_parts(units: i64, scale: u8) -> (bool, String, String) {
    let scale = usize::from(scale);
    // Не меньше scale + 1 цифры, чтобы целая часть была хотя бы «0»: 5 при двух знаках это «0,05».
    let digits = format!("{:0>width$}", units.unsigned_abs(), width = scale + 1);
    let split = digits.len().saturating_sub(scale);
    let (whole, fraction) = digits.split_at(split);
    (units < 0, whole.to_owned(), fraction.to_owned())
}

/// Число «12 345,60»: разряды по три через неразрывный пробел, дробная часть из `shown` знаков.
/// Минус только у числа, которое после отбрасывания лишних знаков не стало нулём.
fn decimal_text(units: i64, scale: u8, shown: u8) -> String {
    let (negative, whole, fraction) = decimal_parts(units, scale);
    let fraction: String = fraction
        .chars()
        .chain(std::iter::repeat('0'))
        .take(usize::from(shown))
        .collect();
    let is_zero = whole.chars().chain(fraction.chars()).all(|c| c == '0');
    let mut text = String::new();
    if negative && !is_zero {
        text.push('-');
    }
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            text.push(GROUP_SEPARATOR);
        }
        text.push(digit);
    }
    if shown > 0 {
        text.push(',');
        text.push_str(&fraction);
    }
    text
}

/// Сумма «12 345,60» из копеек: неразрывные пробелы между разрядами.
pub fn money(kopecks: i64) -> String {
    decimal_text(kopecks, 2, 2)
}

/// Количество «12,5» или «12,125» из тысячных долей: лишние нули в конце не показываем, один
/// знак оставляем. Без пробелов между разрядами, как в строках документа.
pub fn quantity(milli: i64) -> String {
    let (negative, whole, fraction) = decimal_parts(milli, 3);
    let fraction = fraction.trim_end_matches('0');
    let is_zero = fraction.is_empty() && whole.chars().all(|c| c == '0');
    let sign = if negative && !is_zero { "-" } else { "" };
    format!(
        "{sign}{whole},{}",
        if fraction.is_empty() { "0" } else { fraction }
    )
}

/// Дата «дд.мм.гггг» из (год, месяц, день).
pub fn date_text(date: (u16, u8, u8)) -> String {
    format!("{:02}.{:02}.{}", date.2, date.1, date.0)
}

/// Разбирает дату «дд.мм.гггг»; неверный текст: `None`.
pub fn parse_date(text: &str) -> Option<(u16, u8, u8)> {
    let mut parts = text.trim().split('.');
    let day: u8 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let year: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month, day))
}

/// Дата ядра `ГГГГ-ММ-ДД` в (год, месяц, день).
pub fn parse_iso_date(text: &str) -> Option<(u16, u8, u8)> {
    let mut parts = text.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((year, month, day))
}

/// Разбирает неотрицательное число с запятой или точкой в целые доли: `scale` знаков после
/// запятой (для количества 3, для денег 2). Лишние знаки отклоняются, а не округляются.
/// Число, которое не помещается в `i64`, тоже `None`.
pub fn parse_decimal(text: &str, scale: u32) -> Option<i64> {
    let text = text.trim().replace(',', ".");
    let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    let scale_len = usize::try_from(scale).ok()?;
    if (whole.is_empty() && fraction.is_empty())
        || !digits(whole)
        || !digits(fraction)
        || fraction.len() > scale_len
    {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction_value: i64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    let missing = u32::try_from(scale_len.checked_sub(fraction.len())?).ok()?;
    whole
        .checked_mul(10_i64.checked_pow(scale)?)?
        .checked_add(fraction_value.checked_mul(10_i64.checked_pow(missing)?)?)
}

/// Оставляет в числе только допустимое: цифры и один разделитель («,» вместо «.»), после
/// запятой не больше `scale` знаков. Нужна, чтобы в поля количества и цены нельзя было
/// ввести буквы и лишние знаки.
pub fn clean_decimal(text: &str, scale: usize) -> String {
    let mut out = String::new();
    let mut separator = false;
    let mut decimals = 0;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !separator {
                out.push(c);
            } else if decimals < scale {
                out.push(c);
                decimals += 1;
            }
        } else if (c == ',' || c == '.') && !separator && scale > 0 {
            separator = true;
            out.push(',');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NBSP: &str = "\u{a0}";

    #[test]
    fn money_groups_digits_by_three() {
        assert_eq!(money(0), "0,00");
        assert_eq!(money(5), "0,05");
        assert_eq!(money(100), "1,00");
        assert_eq!(money(123_456), format!("1{NBSP}234,56"));
        assert_eq!(money(1_234_560), format!("12{NBSP}345,60"));
        assert_eq!(money(123_456_789), format!("1{NBSP}234{NBSP}567,89"));
    }

    #[test]
    fn negative_money_has_one_minus() {
        assert_eq!(money(-5), "-0,05");
        assert_eq!(money(-1_234_560), format!("-12{NBSP}345,60"));
        // Самое маленькое число не должно переполняться при смене знака.
        assert_eq!(
            money(i64::MIN),
            format!("-92{NBSP}233{NBSP}720{NBSP}368{NBSP}547{NBSP}758,08")
        );
    }

    #[test]
    fn quantity_drops_trailing_zeros_but_keeps_one_digit() {
        assert_eq!(quantity(0), "0,0");
        assert_eq!(quantity(5), "0,005");
        assert_eq!(quantity(12_000), "12,0");
        assert_eq!(quantity(12_500), "12,5");
        assert_eq!(quantity(12_125), "12,125");
        assert_eq!(quantity(1_234_500), "1234,5");
        assert_eq!(quantity(-2_500), "-2,5");
    }

    #[test]
    fn decimal_is_parsed_into_units() {
        assert_eq!(parse_decimal("12,5", 2), Some(1250));
        assert_eq!(parse_decimal(" 12.50 ", 2), Some(1250));
        assert_eq!(parse_decimal("0,05", 2), Some(5));
        assert_eq!(parse_decimal(",5", 2), Some(50));
        assert_eq!(parse_decimal("5,", 2), Some(500));
        assert_eq!(parse_decimal("7", 3), Some(7000));
        assert_eq!(parse_decimal("12,125", 3), Some(12_125));
    }

    #[test]
    fn bad_decimal_is_rejected_not_rounded() {
        for text in ["", " ", ",", "abc", "1,234", "-1", "1,2,3", "1 2"] {
            assert_eq!(parse_decimal(text, 2), None, "{text:?}");
        }
        assert_eq!(parse_decimal("1,5", 0), None);
    }

    #[test]
    fn decimal_overflow_is_an_error() {
        assert_eq!(parse_decimal("92233720368547758,07", 2), Some(i64::MAX));
        assert_eq!(parse_decimal("92233720368547758,08", 2), None);
        assert_eq!(parse_decimal("99999999999999999999", 0), None);
        // Степень десяти для такого числа знаков не помещается в i64.
        assert_eq!(parse_decimal("1", 20), None);
    }

    #[test]
    fn clean_keeps_digits_and_one_separator() {
        assert_eq!(clean_decimal("12,345", 2), "12,34");
        assert_eq!(clean_decimal("a1b2.5", 3), "12,5");
        assert_eq!(clean_decimal("1,2,3", 3), "1,23");
        assert_eq!(clean_decimal("1,5", 0), "15");
        assert_eq!(clean_decimal("-7", 2), "7");
    }

    #[test]
    fn dates_go_both_ways() {
        assert_eq!(date_text((2026, 3, 5)), "05.03.2026");
        assert_eq!(parse_date(" 05.03.2026 "), Some((2026, 3, 5)));
        assert_eq!(parse_date("32.01.2026"), None);
        assert_eq!(parse_date("01.13.2026"), None);
        assert_eq!(parse_date("01.02"), None);
        assert_eq!(parse_date("01.02.2026.5"), None);
        assert_eq!(parse_iso_date("2026-10-18"), Some((2026, 10, 18)));
        assert_eq!(parse_iso_date("18.10.2026"), None);
    }

    #[test]
    fn display_follows_the_field_kind() {
        let decimal = |units, scale| Value::Decimal { units, scale };
        let two = FieldKind::Decimal { scale: 2 };
        assert_eq!(display(&Value::Null, FieldKind::Text), "");
        assert_eq!(display(&Value::Int(42), FieldKind::Int), "42");
        assert_eq!(
            display(&Value::Text("Мука".into()), FieldKind::Text),
            "Мука"
        );
        assert_eq!(display(&decimal(1_234_560, 2), two), money(1_234_560));
        // Тип поля задаёт знаки: лишние отбрасываются, недостающие добавляются нулями.
        assert_eq!(display(&decimal(12_345, 3), two), "12,34");
        assert_eq!(display(&decimal(12, 0), two), "12,00");
        assert_eq!(display(&decimal(-1, 3), two), "0,00");
        assert_eq!(
            display(&decimal(12_500, 3), FieldKind::Decimal { scale: 0 }),
            "12"
        );
        assert_eq!(
            display(&Value::Date("2026-10-18".into()), FieldKind::Date),
            "18.10.2026"
        );
        assert_eq!(display(&Value::Date("мусор".into()), FieldKind::Date), "");
        assert_eq!(display(&Value::Bool(true), FieldKind::Int), "Да");
        let titled = Value::Ref {
            id: 7,
            title: Some("Мельница".into()),
        };
        assert_eq!(display(&titled, FieldKind::Ref), "Мельница");
        let bare = Value::Ref { id: 7, title: None };
        assert_eq!(display(&bare, FieldKind::Ref), "7");
    }

    #[test]
    fn parse_builds_values_of_the_field_kind() {
        let two = FieldKind::Decimal { scale: 2 };
        assert_eq!(parse("42", FieldKind::Int), Ok(Value::Int(42)));
        assert_eq!(
            parse(" Мука ", FieldKind::Text),
            Ok(Value::Text("Мука".into()))
        );
        assert_eq!(
            parse("12,5", two),
            Ok(Value::Decimal {
                units: 1250,
                scale: 2
            })
        );
        assert_eq!(
            parse("18.10.2026", FieldKind::Date),
            Ok(Value::Date("2026-10-18".into()))
        );
        // Пустое поле это Null для любого типа; обязательность проверяет ядро.
        for kind in [
            FieldKind::Int,
            FieldKind::Text,
            two,
            FieldKind::Date,
            FieldKind::Ref,
        ] {
            assert_eq!(parse("  ", kind), Ok(Value::Null));
        }
    }

    #[test]
    fn parse_explains_what_is_wrong() {
        let two = FieldKind::Decimal { scale: 2 };
        assert!(parse("1,234", two).is_err_and(|m| m.contains("не больше 2")));
        assert!(parse("1,5", FieldKind::Int).is_err_and(|m| m.contains("целое")));
        assert!(parse("31.02", FieldKind::Date).is_err_and(|m| m.contains("дд.мм.гггг")));
        assert!(parse("Мельница", FieldKind::Ref).is_err());
    }

    #[test]
    fn display_and_parse_round_trip() {
        // Большое число показано с неразрывными пробелами, и разбор должен их принять.
        for (units, scale) in [(12_125, 3), (1_234_567_890, 2), (0, 2), (7, 0)] {
            let kind = FieldKind::Decimal { scale };
            let value = Value::Decimal { units, scale };
            assert_eq!(parse(&display(&value, kind), kind), Ok(value));
        }
    }

    // Окно строки подставляет цену так; она должна разбираться обратно без правки.
    #[test]
    fn price_in_line_dialog_parses_back() {
        for kopecks in [0, 5, 100_000, 123_456_789] {
            let text = clean_decimal(&money(kopecks), 2);
            assert_eq!(parse_decimal(&text, 2), Some(kopecks), "{text:?}");
        }
    }
}
