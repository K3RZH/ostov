//! Ввод даты одной строкой «дд.мм.гггг»: маска и разбор для поля даты (`DateInput` в Slint).

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, DateInput};

/// Оставляет цифры и расставляет точки: «15» → «15», «150» → «15.0», «15032025» → «15.03.2025».
pub fn mask(raw: &str) -> String {
    let mut out = String::new();
    for (i, digit) in raw.chars().filter(char::is_ascii_digit).take(8).enumerate() {
        if i == 2 || i == 4 {
            out.push('.');
        }
        out.push(digit);
    }
    out
}

/// Нажатие клавиши при открытом календаре: цифра дописывается к дате (к полной дате начинает
/// новую), Backspace и Delete стирают последнюю цифру, остальное игнорируется.
pub fn apply_key(text: &str, key: &str) -> String {
    let mut digits: String = text.chars().filter(char::is_ascii_digit).collect();
    match key {
        "\u{8}" | "\u{7f}" => {
            digits.pop();
        }
        _ => {
            for digit in key.chars().filter(char::is_ascii_digit) {
                // После полной даты набор начинается заново (как после выделения всего поля).
                if digits.len() >= 8 {
                    digits.clear();
                }
                digits.push(digit);
            }
        }
    }
    mask(&digits)
}

fn days_in(year: i32, month: i32) -> i32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// День, месяц и год, если введена полная и существующая дата.
pub fn parts(text: &str) -> Option<[i32; 3]> {
    let digits: Vec<i32> = text
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| i32::try_from(d).unwrap_or(0))
        .collect();
    if digits.len() != 8 {
        return None;
    }
    let number = |from: usize, to: usize| digits[from..to].iter().fold(0, |acc, d| acc * 10 + d);
    let (day, month, year) = (number(0, 2), number(2, 4), number(4, 8));
    ((1..=12).contains(&month) && (1..=days_in(year, month)).contains(&day))
        .then_some([day, month, year])
}

/// Подключает маску и разбор к окну: поле даты вызывает их через `DateInput`.
pub fn install(window: &AppWindow) {
    let input = window.global::<DateInput>();
    input.on_mask(|raw| SharedString::from(mask(raw.as_str())));
    input.on_apply_key(|text, key| SharedString::from(apply_key(text.as_str(), key.as_str())));
    input.on_parts(|text| {
        let values = parts(text.as_str()).map(Vec::from).unwrap_or_default();
        ModelRc::new(VecModel::from(values))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_adds_dots_while_typing() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("15"), "15");
        assert_eq!(mask("150"), "15.0");
        assert_eq!(mask("1503"), "15.03");
        assert_eq!(mask("15032025"), "15.03.2025");
        // Лишнее отбрасывается: буквы, точки, цифры сверх даты.
        assert_eq!(mask("15.03.2025999"), "15.03.2025");
        assert_eq!(mask("1a5"), "15");
    }

    #[test]
    fn keys_extend_and_erase_the_date() {
        assert_eq!(apply_key("15", "0"), "15.0");
        assert_eq!(apply_key("15.0", "\u{8}"), "15");
        assert_eq!(apply_key("15.03.2025", "9"), "9");
        assert_eq!(apply_key("", "\u{8}"), "");
        assert_eq!(apply_key("15", "x"), "15");
    }

    #[test]
    fn only_a_real_full_date_is_parsed() {
        assert_eq!(parts("15.03.2025"), Some([15, 3, 2025]));
        assert_eq!(parts("29.02.2024"), Some([29, 2, 2024]));
        assert_eq!(parts("29.02.2025"), None);
        assert_eq!(parts("31.04.2025"), None);
        assert_eq!(parts("15.13.2025"), None);
        assert_eq!(parts("15.03.20"), None);
        assert_eq!(parts("00.01.2025"), None);
    }
}
