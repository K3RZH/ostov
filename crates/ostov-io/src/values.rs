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
    fn excel_days_match_excel() {
        assert_eq!(excel_day("1900-03-01"), Some(61));
        assert_eq!(excel_day("2026-01-10"), Some(46032));
        assert_eq!(excel_day("2024-02-29"), Some(45351));
        for bad in ["2026-02-30", "20260110", "2026-1-10", "вчера", "2026-13-01"] {
            assert_eq!(excel_day(bad), None, "{bad}");
        }
    }
}
