//! Даты и числа на границе с Excel. Деньги в файле — число Excel (double), поэтому десятичное
//! значение переводится в текст и уже текст разбирается в `f64`, без арифметики с плавающей точкой.

/// День 0 календаря Excel (система 1900, с учётом ошибки 1900 года): 30.12.1899.
const EXCEL_EPOCH: (i64, i64, i64) = (1899, 12, 30);

/// `units = -1250, scale = 2` → `-12.50`.
pub(crate) fn decimal_text(units: i64, scale: u8) -> String {
    let digits = units.unsigned_abs().to_string();
    let sign = if units < 0 { "-" } else { "" };
    let scale = usize::from(scale);
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = padded.split_at(padded.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// Число Excel для десятичного значения.
pub(crate) fn decimal_number(units: i64, scale: u8) -> Option<f64> {
    decimal_text(units, scale).parse().ok()
}

/// Формат ячейки по числу знаков: `0`, `0.00`, `0.000`.
pub(crate) fn decimal_format(scale: u8) -> String {
    if scale == 0 {
        "0".to_owned()
    } else {
        format!("0.{}", "0".repeat(usize::from(scale)))
    }
}

/// `ГГГГ-ММ-ДД` → номер дня Excel. `None`, если строка — не дата.
pub(crate) fn excel_day(date: &str) -> Option<i64> {
    let (year, month, day) = parse_iso(date)?;
    Some(
        days_from_civil(year, month, day)
            - days_from_civil(EXCEL_EPOCH.0, EXCEL_EPOCH.1, EXCEL_EPOCH.2),
    )
}

fn parse_iso(date: &str) -> Option<(i64, i64, i64)> {
    let mut parts = date.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let (year, month, day) = (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    (valid_date(year, month, day)).then_some((year, month, day))
}

pub(crate) fn valid_date(year: i64, month: i64, day: i64) -> bool {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=9999).contains(&year) && (1..=days).contains(&day)
}

/// Дни от 01.01.1970 по григорианскому календарю (алгоритм Хиннанта).
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Номер дня Excel → `ГГГГ-ММ-ДД` (время отбрасывается).
pub(crate) fn excel_day_to_iso(day: i64) -> Option<String> {
    let epoch = days_from_civil(EXCEL_EPOCH.0, EXCEL_EPOCH.1, EXCEL_EPOCH.2);
    let (year, month, day) = civil_from_days(epoch.checked_add(day)?);
    valid_date(year, month, day).then(|| format!("{year:04}-{month:02}-{day:02}"))
}

/// Обратное к `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Дата из текста: `ДД.ММ.ГГГГ` или `ГГГГ-ММ-ДД` → `ГГГГ-ММ-ДД`.
pub(crate) fn parse_date(text: &str) -> Option<String> {
    let text = text.trim();
    if parse_iso(text).is_some() {
        return Some(text.to_owned());
    }
    let mut parts = text.split('.');
    let (day, month, year) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || day.len() != 2 || month.len() != 2 || year.len() != 4 {
        return None;
    }
    let iso = format!("{year}-{month}-{day}");
    parse_iso(&iso).map(|_| iso)
}

/// Число из текста (`2,5`, `2.5`, `-3`, `1 250,00`) в целых единицах `scale`.
/// Знаков после запятой больше `scale` можно, только если лишние — нули.
pub(crate) fn parse_decimal(text: &str, scale: u8) -> Result<i64, String> {
    let shown = text.trim();
    let cleaned: String = shown
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == ',' { '.' } else { c })
        .collect();
    let (negative, body) = match cleaned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cleaned.as_str()),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if int.is_empty() || !digits(int) || !digits(frac) {
        return Err(format!("«{shown}» — не число"));
    }
    let scale = usize::from(scale);
    let (kept, extra) = frac.split_at(frac.len().min(scale));
    if extra.bytes().any(|b| b != b'0') {
        return Err(format!("в «{shown}» больше {scale} знаков после запятой"));
    }
    let units: i64 = format!("{int}{kept:0<scale$}")
        .parse()
        .map_err(|_| format!("«{shown}» — слишком большое число"))?;
    Ok(if negative { -units } else { units })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_become_text_without_float_math() {
        assert_eq!(decimal_text(1250, 2), "12.50");
        assert_eq!(decimal_text(-5, 3), "-0.005");
        assert_eq!(decimal_text(7, 0), "7");
        assert_eq!(decimal_text(i64::MIN, 2), "-92233720368547758.08");
        assert_eq!(decimal_format(3), "0.000");
    }

    #[test]
    fn excel_days_round_trip() {
        for day in [1, 61, 45351, 46032, 2_958_465] {
            let iso = excel_day_to_iso(day).unwrap();
            assert_eq!(excel_day(&iso), Some(day), "{iso}");
        }
        assert_eq!(excel_day_to_iso(46032).as_deref(), Some("2026-01-10"));
    }

    #[test]
    fn dates_and_numbers_from_text() {
        assert_eq!(parse_date("10.01.2026").as_deref(), Some("2026-01-10"));
        assert_eq!(parse_date(" 2026-01-10 ").as_deref(), Some("2026-01-10"));
        assert_eq!(parse_date("31.02.2026"), None);
        assert_eq!(parse_decimal("2,5", 3), Ok(2500));
        assert_eq!(parse_decimal("2.5", 3), Ok(2500));
        assert_eq!(parse_decimal("-0,10", 2), Ok(-10));
        assert_eq!(parse_decimal("1 250,00", 2), Ok(125_000));
        assert_eq!(parse_decimal("3.330", 2), Ok(333), "лишний ноль можно");
        assert!(
            parse_decimal("3.335", 2)
                .unwrap_err()
                .contains("больше 2 знаков")
        );
        assert!(parse_decimal("2,5кг", 3).is_err());
        assert!(parse_decimal("", 2).is_err());
    }

    #[test]
    fn excel_days_match_excel() {
        assert_eq!(excel_day("1900-03-01"), Some(61));
        assert_eq!(excel_day("2026-01-10"), Some(46032));
        assert_eq!(excel_day("2024-02-29"), Some(45351));
        for bad in ["2026-02-30", "20260110", "2026-1-10", "вчера", "2026-13-01"] {
            assert_eq!(excel_day(bad), None, "{bad}");
        }
    }
}
