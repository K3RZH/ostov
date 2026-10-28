//! Десятичное число с фиксированным масштабом для денег и количеств (ADR-010, СТ-56, СТ-К15).

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// Максимальный масштаб — число знаков после запятой. Больше 18 знаков не помещается в `i64`.
pub const MAX_SCALE: u8 = 18;

/// Десятичное число `units / 10^scale` без плавающей запятой.
///
/// В базе хранится целым `units` в минимальных единицах поля, масштаб задаёт поле
/// (количество с 3 знаками, сумма с 2). Операции проверяют переполнение и возвращают
/// [`DecimalError::Overflow`] вместо искажённого значения.
///
/// Сравнение математическое: `1.50 == 1.5`. Поэтому `Hash` не реализован — для ключей
/// приводите числа к одному масштабу и используйте [`Decimal::units`].
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    units: i64,
    scale: u8,
}

/// Ошибки операций с [`Decimal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalError {
    /// Масштаб больше [`MAX_SCALE`].
    ScaleTooLarge(u8),
    /// Результат не помещается в 18 значащих цифр (`i64`).
    Overflow,
    /// Строка не является числом вида `-123.45` (в ошибке — её начало, не длиннее 32 символов).
    Parse(String),
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScaleTooLarge(scale) => write!(
                f,
                "масштаб {scale} больше допустимого {MAX_SCALE}: уменьшите число знаков после запятой"
            ),
            Self::Overflow => write!(
                f,
                "число слишком большое: результат не помещается в 18 цифр"
            ),
            Self::Parse(input) => {
                write!(
                    f,
                    "«{input}» не десятичное число: ожидается запись вида -123.45"
                )
            }
        }
    }
}

impl std::error::Error for DecimalError {}

impl Decimal {
    /// Создаёт число `units / 10^scale`.
    ///
    /// # Errors
    /// [`DecimalError::ScaleTooLarge`], если `scale` больше [`MAX_SCALE`].
    pub fn new(units: i64, scale: u8) -> Result<Self, DecimalError> {
        check_scale(scale)?;
        Ok(Self { units, scale })
    }

    /// Ноль с масштабом `scale`.
    ///
    /// # Errors
    /// [`DecimalError::ScaleTooLarge`], если `scale` больше [`MAX_SCALE`].
    pub fn zero(scale: u8) -> Result<Self, DecimalError> {
        Self::new(0, scale)
    }

    /// Значение в минимальных единицах — то, что хранится в базе.
    pub const fn units(self) -> i64 {
        self.units
    }

    /// Число знаков после запятой.
    pub const fn scale(self) -> u8 {
        self.scale
    }

    /// Равно ли число нулю.
    pub const fn is_zero(self) -> bool {
        self.units == 0
    }

    /// Меньше ли число нуля.
    pub const fn is_negative(self) -> bool {
        self.units < 0
    }

    /// Приводит число к масштабу `scale`; при уменьшении масштаба округляет
    /// математически: половина — от нуля (`0.125 → 0.13`, `-0.125 → -0.13`), СТ-К15.
    ///
    /// # Errors
    /// [`DecimalError::ScaleTooLarge`] или [`DecimalError::Overflow`] при увеличении масштаба.
    pub fn rescale(self, scale: u8) -> Result<Self, DecimalError> {
        check_scale(scale)?;
        let units = rescale_units(i128::from(self.units), self.scale, scale)?;
        Self::from_i128(units, scale)
    }

    /// Сумма; масштаб результата — больший из двух.
    ///
    /// # Errors
    /// [`DecimalError::Overflow`], если результат не помещается в `i64`.
    pub fn checked_add(self, other: Self) -> Result<Self, DecimalError> {
        let scale = self.scale.max(other.scale);
        let sum = self
            .aligned(scale)
            .checked_add(other.aligned(scale))
            .ok_or(DecimalError::Overflow)?;
        Self::from_i128(sum, scale)
    }

    /// Разность; масштаб результата — больший из двух.
    ///
    /// # Errors
    /// [`DecimalError::Overflow`], если результат не помещается в `i64`.
    pub fn checked_sub(self, other: Self) -> Result<Self, DecimalError> {
        let scale = self.scale.max(other.scale);
        let difference = self
            .aligned(scale)
            .checked_sub(other.aligned(scale))
            .ok_or(DecimalError::Overflow)?;
        Self::from_i128(difference, scale)
    }

    /// Число с противоположным знаком.
    ///
    /// # Errors
    /// [`DecimalError::Overflow`] для минимального `i64`, у которого нет положительной пары.
    pub fn checked_neg(self) -> Result<Self, DecimalError> {
        let units = self.units.checked_neg().ok_or(DecimalError::Overflow)?;
        Ok(Self { units, ..self })
    }

    /// Произведение, округлённое до масштаба `scale`, как в [`Decimal::rescale`].
    /// Так считается сумма строки: округление(количество × цена), СТ-К15.
    ///
    /// # Errors
    /// [`DecimalError::ScaleTooLarge`] или [`DecimalError::Overflow`].
    pub fn checked_mul(self, other: Self, scale: u8) -> Result<Self, DecimalError> {
        check_scale(scale)?;
        // i64 × i64 всегда помещается в i128, а сумма масштабов не больше 36.
        let product = i128::from(self.units) * i128::from(other.units);
        let units = rescale_units(product, self.scale + other.scale, scale)?;
        Self::from_i128(units, scale)
    }

    /// Значение в масштабе `scale` не меньше собственного; в i128 переполнения нет:
    /// `|i64| × 10^18 < 2^127`.
    fn aligned(self, scale: u8) -> i128 {
        i128::from(self.units) * pow10(scale - self.scale)
    }

    fn from_i128(units: i128, scale: u8) -> Result<Self, DecimalError> {
        let units = i64::try_from(units).map_err(|_| DecimalError::Overflow)?;
        Ok(Self { units, scale })
    }
}

fn check_scale(scale: u8) -> Result<(), DecimalError> {
    if scale > MAX_SCALE {
        Err(DecimalError::ScaleTooLarge(scale))
    } else {
        Ok(())
    }
}

/// 10^exp; вызывается только с `exp <= 36`, поэтому помещается в i128.
fn pow10(exp: u8) -> i128 {
    10_i128.pow(u32::from(exp))
}

fn rescale_units(units: i128, from: u8, to: u8) -> Result<i128, DecimalError> {
    match to.cmp(&from) {
        Ordering::Equal => Ok(units),
        Ordering::Greater => units
            .checked_mul(pow10(to - from))
            .ok_or(DecimalError::Overflow),
        Ordering::Less => Ok(div_round_half_away(units, pow10(from - to))),
    }
}

/// Деление на положительное `divisor` с округлением: половина и больше — от нуля.
fn div_round_half_away(dividend: i128, divisor: i128) -> i128 {
    let quotient = dividend / divisor;
    let remainder = dividend % divisor;
    if remainder.abs() * 2 >= divisor {
        quotient + dividend.signum()
    } else {
        quotient
    }
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Decimal {}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let scale = self.scale.max(other.scale);
        self.aligned(scale).cmp(&other.aligned(scale))
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.units < 0 { "-" } else { "" };
        let digits = self.units.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        if scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let (integer, fraction) = padded.split_at(padded.len() - scale);
        write!(f, "{sign}{integer}.{fraction}")
    }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    /// Строгий разбор записи `-123.45`: без пробелов, знака `+`, экспоненты, запятой
    /// и пустой целой или дробной части. Масштаб равен числу цифр после точки.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let error = || DecimalError::Parse(input.chars().take(32).collect());
        let (negative, body) = match input.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, input),
        };
        let (integer, fraction) = match body.split_once('.') {
            Some((integer, fraction)) if !fraction.is_empty() => (integer, fraction),
            Some(_) => return Err(error()),
            None => (body, ""),
        };
        let is_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if integer.is_empty() || !is_digits(integer) || !is_digits(fraction) {
            return Err(error());
        }
        let scale =
            u8::try_from(fraction.len()).map_err(|_| DecimalError::ScaleTooLarge(u8::MAX))?;
        check_scale(scale)?;
        // Цифры копим в i128, чтобы поймать выход за i64 ошибкой, а не паникой.
        let mut units: i128 = 0;
        for byte in integer.bytes().chain(fraction.bytes()) {
            units = units
                .checked_mul(10)
                .and_then(|value| value.checked_add(i128::from(byte - b'0')))
                .ok_or(DecimalError::Overflow)?;
        }
        if negative {
            units = -units;
        }
        Self::from_i128(units, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(input: &str) -> Decimal {
        input.parse().unwrap()
    }

    #[test]
    fn parses_and_displays_round_trip() {
        for input in [
            "0",
            "-0.5",
            "12.30",
            "0.001",
            "-123.45",
            "9223372036854775807",
        ] {
            let value = d(input);
            let expected = if input == "-0.5" { "-0.5" } else { input };
            assert_eq!(value.to_string(), expected);
        }
        assert_eq!(d("12.30").scale(), 2);
        assert_eq!(d("12.30").units(), 1230);
    }

    #[test]
    fn negative_zero_is_plain_zero() {
        let zero = d("-0.00");
        assert!(zero.is_zero());
        assert!(!zero.is_negative());
        assert_eq!(zero.to_string(), "0.00");
    }

    #[test]
    fn parse_rejects_loose_formats() {
        for input in [
            "", "-", "+1", " 1", "1 ", "1.", ".5", "1,5", "1e3", "1.2.3", "abc", "--1", "١",
        ] {
            assert!(
                matches!(input.parse::<Decimal>(), Err(DecimalError::Parse(_))),
                "{input:?}"
            );
        }
    }

    #[test]
    fn parse_rejects_too_many_fraction_digits() {
        assert_eq!(
            "0.1234567890123456789".parse::<Decimal>(),
            Err(DecimalError::ScaleTooLarge(19))
        );
    }

    #[test]
    fn parse_reports_overflow_instead_of_panicking() {
        assert_eq!(
            "9223372036854775808".parse::<Decimal>(),
            Err(DecimalError::Overflow)
        );
        assert_eq!(
            "99999999999999999999999999999999999999999".parse::<Decimal>(),
            Err(DecimalError::Overflow)
        );
    }

    #[test]
    fn parse_error_truncates_long_input() {
        let long = "x".repeat(1000);
        let Err(DecimalError::Parse(shown)) = long.parse::<Decimal>() else {
            panic!("ожидалась ошибка разбора");
        };
        assert_eq!(shown.chars().count(), 32);
    }

    #[test]
    fn new_rejects_scale_above_max() {
        assert_eq!(Decimal::new(1, 19), Err(DecimalError::ScaleTooLarge(19)));
        assert!(Decimal::new(1, MAX_SCALE).is_ok());
    }

    #[test]
    fn compares_mathematically_across_scales() {
        assert_eq!(d("1.50"), d("1.5"));
        assert!(d("1.05") < d("1.5"));
        assert!(d("-2") < d("-1.999"));
    }

    #[test]
    fn adds_and_subtracts_with_larger_scale() {
        let sum = d("1.5").checked_add(d("0.25")).unwrap();
        assert_eq!(sum.to_string(), "1.75");
        assert_eq!(sum.scale(), 2);
        assert_eq!(d("1").checked_sub(d("0.001")).unwrap().to_string(), "0.999");
    }

    #[test]
    fn addition_overflow_is_an_error() {
        let max = Decimal::new(i64::MAX, 0).unwrap();
        assert_eq!(max.checked_add(d("1")), Err(DecimalError::Overflow));
        let min = Decimal::new(i64::MIN, 0).unwrap();
        assert_eq!(min.checked_sub(d("1")), Err(DecimalError::Overflow));
    }

    #[test]
    fn aligning_scales_can_overflow() {
        // 9e18 с масштабом 0 нельзя представить с двумя знаками после запятой в i64.
        let big = Decimal::new(9_000_000_000_000_000_000, 0).unwrap();
        assert_eq!(big.checked_add(d("0.01")), Err(DecimalError::Overflow));
    }

    #[test]
    fn negation_of_min_is_an_error() {
        let min = Decimal::new(i64::MIN, 2).unwrap();
        assert_eq!(min.checked_neg(), Err(DecimalError::Overflow));
        assert_eq!(d("1.5").checked_neg().unwrap(), d("-1.5"));
    }

    #[test]
    fn rescale_rounds_half_away_from_zero() {
        assert_eq!(d("0.125").rescale(2).unwrap().to_string(), "0.13");
        assert_eq!(d("0.124").rescale(2).unwrap().to_string(), "0.12");
        assert_eq!(d("-0.125").rescale(2).unwrap().to_string(), "-0.13");
        assert_eq!(d("2.5").rescale(0).unwrap().to_string(), "3");
        assert_eq!(d("-2.5").rescale(0).unwrap().to_string(), "-3");
        assert_eq!(d("1.5").rescale(3).unwrap().to_string(), "1.500");
    }

    #[test]
    fn line_sum_is_rounded_product() {
        // 2.5 кг × 3.333 за кг = 8.3325 → 8.33
        let quantity = d("2.500");
        let price = d("3.333");
        assert_eq!(quantity.checked_mul(price, 2).unwrap().to_string(), "8.33");
        // 3 × 0.335 = 1.005 → 1.01 (половина — вверх)
        assert_eq!(
            d("3").checked_mul(d("0.335"), 2).unwrap().to_string(),
            "1.01"
        );
    }

    #[test]
    fn multiplication_overflow_is_an_error() {
        let big = Decimal::new(i64::MAX, 0).unwrap();
        assert_eq!(big.checked_mul(d("2"), 0), Err(DecimalError::Overflow));
        assert_eq!(
            d("1").checked_mul(d("1"), 19),
            Err(DecimalError::ScaleTooLarge(19))
        );
    }
}
