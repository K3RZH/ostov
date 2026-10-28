use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

pub const MAX_SCALE: u8 = 18;

/// Число `units / 10^scale` для денег и количеств, без float (ADR-010).
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    units: i64,
    scale: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalError {
    ScaleTooLarge(u8),
    Overflow,
    Parse(String),
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScaleTooLarge(scale) => write!(f, "масштаб {scale} больше {MAX_SCALE}"),
            Self::Overflow => write!(f, "переполнение: число не помещается в 18 цифр"),
            Self::Parse(input) => write!(f, "не число: «{input}»"),
        }
    }
}

impl std::error::Error for DecimalError {}

impl Decimal {
    pub fn new(units: i64, scale: u8) -> Result<Self, DecimalError> {
        check_scale(scale)?;
        Ok(Self { units, scale })
    }

    pub fn zero(scale: u8) -> Result<Self, DecimalError> {
        Self::new(0, scale)
    }

    pub const fn units(self) -> i64 {
        self.units
    }

    pub const fn scale(self) -> u8 {
        self.scale
    }

    pub const fn is_zero(self) -> bool {
        self.units == 0
    }

    pub const fn is_negative(self) -> bool {
        self.units < 0
    }

    /// При уменьшении масштаба округляет половину от нуля: 0.125 -> 0.13, -0.125 -> -0.13.
    pub fn rescale(self, scale: u8) -> Result<Self, DecimalError> {
        check_scale(scale)?;
        let units = rescale(i128::from(self.units), self.scale, scale)?;
        Self::from_i128(units, scale)
    }

    pub fn checked_add(self, other: Self) -> Result<Self, DecimalError> {
        let scale = self.scale.max(other.scale);
        let sum = self.at(scale).checked_add(other.at(scale));
        Self::from_i128(sum.ok_or(DecimalError::Overflow)?, scale)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, DecimalError> {
        let scale = self.scale.max(other.scale);
        let diff = self.at(scale).checked_sub(other.at(scale));
        Self::from_i128(diff.ok_or(DecimalError::Overflow)?, scale)
    }

    pub fn checked_neg(self) -> Result<Self, DecimalError> {
        let units = self.units.checked_neg().ok_or(DecimalError::Overflow)?;
        Ok(Self { units, ..self })
    }

    /// Произведение с округлением до `scale` — так считается сумма строки (СТ-К15).
    pub fn checked_mul(self, other: Self, scale: u8) -> Result<Self, DecimalError> {
        check_scale(scale)?;
        let product = i128::from(self.units) * i128::from(other.units);
        let units = rescale(product, self.scale + other.scale, scale)?;
        Self::from_i128(units, scale)
    }

    // В i128 не переполняется: |i64| * 10^18 < 2^127.
    fn at(self, scale: u8) -> i128 {
        i128::from(self.units) * pow10(scale - self.scale)
    }

    fn from_i128(units: i128, scale: u8) -> Result<Self, DecimalError> {
        let units = i64::try_from(units).map_err(|_| DecimalError::Overflow)?;
        Ok(Self { units, scale })
    }
}

fn check_scale(scale: u8) -> Result<(), DecimalError> {
    if scale > MAX_SCALE {
        return Err(DecimalError::ScaleTooLarge(scale));
    }
    Ok(())
}

fn pow10(exp: u8) -> i128 {
    10_i128.pow(u32::from(exp))
}

fn rescale(units: i128, from: u8, to: u8) -> Result<i128, DecimalError> {
    match to.cmp(&from) {
        Ordering::Equal => Ok(units),
        Ordering::Greater => units
            .checked_mul(pow10(to - from))
            .ok_or(DecimalError::Overflow),
        Ordering::Less => {
            let divisor = pow10(from - to);
            let (q, r) = (units / divisor, units % divisor);
            Ok(if r.abs() * 2 >= divisor {
                q + units.signum()
            } else {
                q
            })
        }
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
        self.at(scale).cmp(&other.at(scale))
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
        let (int, frac) = padded.split_at(padded.len() - scale);
        write!(f, "{sign}{int}.{frac}")
    }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    /// Только запись вида `-123.45`: без пробелов, `+`, экспоненты и запятой.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || DecimalError::Parse(s.chars().take(32).collect());
        let (negative, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (int, frac) = match body.split_once('.') {
            Some((_, "")) => return Err(bad()),
            Some(parts) => parts,
            None => (body, ""),
        };
        let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty() || !digits(int) || !digits(frac) {
            return Err(bad());
        }
        let scale = u8::try_from(frac.len()).unwrap_or(u8::MAX);
        check_scale(scale)?;

        let mut units: i128 = 0;
        for b in int.bytes().chain(frac.bytes()) {
            units = units
                .checked_mul(10)
                .and_then(|u| u.checked_add(i128::from(b - b'0')))
                .ok_or(DecimalError::Overflow)?;
        }
        Self::from_i128(if negative { -units } else { units }, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn parse_and_display() {
        for s in [
            "0",
            "-0.5",
            "12.30",
            "0.001",
            "-123.45",
            "9223372036854775807",
        ] {
            assert_eq!(dec(s).to_string(), s);
        }
        assert_eq!(dec("12.30").units(), 1230);
        assert_eq!(dec("12.30").scale(), 2);
        assert_eq!(dec("-0.00").to_string(), "0.00");
    }

    #[test]
    fn parse_rejects_garbage() {
        for s in [
            "", "-", "+1", " 1", "1.", ".5", "1,5", "1e3", "1.2.3", "--1",
        ] {
            assert!(
                matches!(s.parse::<Decimal>(), Err(DecimalError::Parse(_))),
                "{s:?}"
            );
        }
        assert_eq!(
            "0.1234567890123456789".parse::<Decimal>(),
            Err(DecimalError::ScaleTooLarge(19))
        );
        assert_eq!(
            "9223372036854775808".parse::<Decimal>(),
            Err(DecimalError::Overflow)
        );
    }

    #[test]
    fn compare_across_scales() {
        assert_eq!(dec("1.50"), dec("1.5"));
        assert!(dec("1.05") < dec("1.5"));
        assert!(dec("-2") < dec("-1.999"));
    }

    #[test]
    fn add_sub() {
        assert_eq!(
            dec("1.5").checked_add(dec("0.25")).unwrap().to_string(),
            "1.75"
        );
        assert_eq!(
            dec("1").checked_sub(dec("0.001")).unwrap().to_string(),
            "0.999"
        );

        let max = Decimal::new(i64::MAX, 0).unwrap();
        assert_eq!(max.checked_add(dec("1")), Err(DecimalError::Overflow));
        let big = Decimal::new(9_000_000_000_000_000_000, 0).unwrap();
        assert_eq!(big.checked_add(dec("0.01")), Err(DecimalError::Overflow));
    }

    #[test]
    fn neg() {
        assert_eq!(dec("1.5").checked_neg().unwrap(), dec("-1.5"));
        let min = Decimal::new(i64::MIN, 2).unwrap();
        assert_eq!(min.checked_neg(), Err(DecimalError::Overflow));
    }

    #[test]
    fn rounding() {
        assert_eq!(dec("0.125").rescale(2).unwrap().to_string(), "0.13");
        assert_eq!(dec("0.124").rescale(2).unwrap().to_string(), "0.12");
        assert_eq!(dec("-0.125").rescale(2).unwrap().to_string(), "-0.13");
        assert_eq!(dec("-2.5").rescale(0).unwrap().to_string(), "-3");
        assert_eq!(dec("1.5").rescale(3).unwrap().to_string(), "1.500");
    }

    #[test]
    fn mul() {
        // 2.5 кг * 3.333 = 8.3325
        assert_eq!(
            dec("2.500")
                .checked_mul(dec("3.333"), 2)
                .unwrap()
                .to_string(),
            "8.33"
        );
        assert_eq!(
            dec("3").checked_mul(dec("0.335"), 2).unwrap().to_string(),
            "1.01"
        );

        let max = Decimal::new(i64::MAX, 0).unwrap();
        assert_eq!(max.checked_mul(dec("2"), 0), Err(DecimalError::Overflow));
    }
}
