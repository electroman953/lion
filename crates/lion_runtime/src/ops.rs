//! Primitive operations on numbers (spec §8).
//!
//! Int arithmetic is checked: a result that does not fit in 64 bits is a bug in both
//! modes (§8.1). Float arithmetic follows IEEE 754 and never fails (§8.2).

use crate::bug::{BugKind, IntOp};

pub fn int_add(lhs: i64, rhs: i64) -> Result<i64, BugKind> {
    lhs.checked_add(rhs).ok_or(overflow(IntOp::Add, lhs, rhs))
}

pub fn int_sub(lhs: i64, rhs: i64) -> Result<i64, BugKind> {
    lhs.checked_sub(rhs).ok_or(overflow(IntOp::Subtract, lhs, rhs))
}

pub fn int_mul(lhs: i64, rhs: i64) -> Result<i64, BugKind> {
    lhs.checked_mul(rhs).ok_or(overflow(IntOp::Multiply, lhs, rhs))
}

pub fn int_neg(value: i64) -> Result<i64, BugKind> {
    value.checked_neg().ok_or(BugKind::IntOverflow { op: IntOp::Negate, lhs: value, rhs: None })
}

/// Euclidean division: `lhs = q * rhs + r` with `0 <= r < |rhs|` (§8.1, D62).
pub fn int_div(lhs: i64, rhs: i64) -> Result<i64, BugKind> {
    if rhs == 0 {
        return Err(BugKind::DivisionByZero { op: IntOp::Divide, lhs });
    }
    lhs.checked_div_euclid(rhs).ok_or(overflow(IntOp::Divide, lhs, rhs))
}

/// Euclidean remainder: always between 0 and `|rhs| - 1` (§8.1, D62).
pub fn int_mod(lhs: i64, rhs: i64) -> Result<i64, BugKind> {
    if rhs == 0 {
        return Err(BugKind::DivisionByZero { op: IntOp::Modulo, lhs });
    }
    // `Int.min mod -1` is 0 and fits; only the intermediate quotient would overflow.
    Ok(lhs.checked_rem_euclid(rhs).unwrap_or(0))
}

/// `base ^ exponent` on Ints; a negative exponent is a bug (§8.4).
pub fn int_pow(base: i64, exponent: i64) -> Result<i64, BugKind> {
    if exponent < 0 {
        return Err(BugKind::NegativeExponent { base, exponent });
    }
    let fail = overflow(IntOp::Power, base, exponent);
    let (mut result, mut square, mut remaining) = (1i64, base, exponent);
    loop {
        if remaining & 1 == 1 {
            result = result.checked_mul(square).ok_or_else(|| fail.clone())?;
        }
        remaining >>= 1;
        if remaining == 0 {
            return Ok(result);
        }
        // With |base| >= 2, a square that overflows would make the result overflow too.
        square = square.checked_mul(square).ok_or_else(|| fail.clone())?;
    }
}

/// `isqrt(n)`: the largest `r` such that `r * r <= n` (§23, C59).
pub fn isqrt(n: i64) -> Result<i64, BugKind> {
    if n < 0 {
        return Err(BugKind::NegativeSquareRoot { value: n });
    }
    // The Float estimate is off by at most one either way; the checks correct it.
    let mut root = (n as f64).sqrt() as i64;
    while root.checked_mul(root).is_none_or(|square| square > n) {
        root -= 1;
    }
    while (root + 1).checked_mul(root + 1).is_some_and(|square| square <= n) {
        root += 1;
    }
    Ok(root)
}

/// `floor(x)`, `ceil(x)`, `round(x)` of a Float: an Int, or a bug when there is none
/// (§23, C59). `round` goes away from zero at the half: `round(2.5)` is 3.
pub fn float_floor(value: f64) -> Result<i64, BugKind> {
    float_to_int(value.floor())
}

pub fn float_ceil(value: f64) -> Result<i64, BugKind> {
    float_to_int(value.ceil())
}

pub fn float_round(value: f64) -> Result<i64, BugKind> {
    float_to_int(value.round())
}

pub fn float_pow(base: f64, exponent: f64) -> f64 {
    base.powf(exponent)
}

/// `x as Int`: truncation toward zero; NaN, infinities and out-of-range values are
/// bugs (§8.5, D74).
pub fn float_to_int(value: f64) -> Result<i64, BugKind> {
    // -2^63 is exactly representable; 2^63 is the first Float above the Int range.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if value.is_finite() && (-LIMIT..LIMIT).contains(&value.trunc()) {
        Ok(value.trunc() as i64)
    } else {
        Err(BugKind::InvalidFloatToInt { value })
    }
}

/// Int to Float, and whether the conversion was exact; beyond 2^53 some Ints have no
/// exact Float, which the interpreter reports as an alert (§8.5, §22.3).
pub fn int_to_float(value: i64) -> (f64, bool) {
    let converted = value as f64;
    // Comparing in i128 avoids the saturation of `f64 as i64` at 2^63.
    (converted, converted as i128 == value as i128)
}

/// `text as Int` (§8.5): spaces around are ignored; the error says why (D14).
pub fn parse_int(text: &str) -> Result<i64, String> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Err(format!("{} is not an Int: it is empty", crate::format::quote_text(text)));
    }
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("{} is not an Int: it is not a whole number", crate::format::quote_text(text)));
    }
    trimmed.parse().map_err(|_| format!("{} is too large for an Int", crate::format::quote_text(text)))
}

/// `text as Float` (§8.5): decimal notation with an optional exponent, spaces around
/// ignored; the error says why (D14).
pub fn parse_float(text: &str) -> Result<f64, String> {
    let trimmed = text.trim();
    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    let exponent_valid = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    if trimmed.is_empty() {
        return Err(format!("{} is not a Float: it is empty", crate::format::quote_text(text)));
    }
    if (whole.is_empty() && fraction.is_empty()) || !digits(whole) || !digits(fraction) || !exponent_valid {
        return Err(format!("{} is not a Float: it is not a number", crate::format::quote_text(text)));
    }
    let value: f64 =
        trimmed.parse().map_err(|_| format!("{} is not a Float", crate::format::quote_text(text)))?;
    if value.is_infinite() {
        return Err(format!("{} is too large for a Float", crate::format::quote_text(text)));
    }
    Ok(value)
}

fn overflow(op: IntOp, lhs: i64, rhs: i64) -> BugKind {
    BugKind::IntOverflow { op, lhs, rhs: Some(rhs) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euclidean_division_from_the_spec() {
        assert_eq!(int_div(7, 2), Ok(3));
        assert_eq!(int_div(-7, 3), Ok(-3));
        assert_eq!(int_mod(-7, 3), Ok(2));
        assert_eq!(int_div(7, -2), Ok(-3));
        assert_eq!(int_mod(7, -2), Ok(1));
        assert_eq!(int_mod(-7, -3), Ok(2));
    }

    #[test]
    fn division_edge_cases() {
        assert!(matches!(int_div(1, 0), Err(BugKind::DivisionByZero { .. })));
        assert!(matches!(int_mod(1, 0), Err(BugKind::DivisionByZero { .. })));
        assert!(matches!(int_div(i64::MIN, -1), Err(BugKind::IntOverflow { .. })));
        assert_eq!(int_mod(i64::MIN, -1), Ok(0));
    }

    #[test]
    fn checked_arithmetic() {
        assert_eq!(int_add(i64::MAX - 1, 1), Ok(i64::MAX));
        assert!(int_add(i64::MAX, 1).is_err());
        assert!(int_sub(i64::MIN, 1).is_err());
        assert!(int_mul(i64::MAX, 2).is_err());
        assert!(int_neg(i64::MIN).is_err());
        assert_eq!(int_neg(i64::MAX), Ok(-i64::MAX));
    }

    #[test]
    fn int_powers() {
        assert_eq!(int_pow(2, 10), Ok(1024));
        assert_eq!(int_pow(-2, 3), Ok(-8));
        assert_eq!(int_pow(0, 0), Ok(1));
        assert_eq!(int_pow(-1, i64::MAX), Ok(-1));
        assert_eq!(int_pow(2, 62), Ok(1 << 62));
        assert!(int_pow(2, 63).is_err());
        assert_eq!(int_pow(-2, 63), Ok(i64::MIN));
        assert!(int_pow(3, 50).is_err());
        assert!(matches!(int_pow(2, -1), Err(BugKind::NegativeExponent { .. })));
    }

    #[test]
    fn float_to_int_truncates() {
        assert_eq!(float_to_int(3.7), Ok(3));
        assert_eq!(float_to_int(-3.7), Ok(-3));
        assert_eq!(float_to_int(-9_223_372_036_854_775_808.0), Ok(i64::MIN));
        assert!(float_to_int(9_223_372_036_854_775_808.0).is_err());
        assert!(float_to_int(f64::NAN).is_err());
        assert!(float_to_int(f64::INFINITY).is_err());
    }

    #[test]
    fn parsing_texts() {
        assert_eq!(parse_int("12"), Ok(12));
        assert_eq!(parse_int(" -7 "), Ok(-7));
        assert!(parse_int("12a").unwrap_err().contains("not a whole number"));
        assert!(parse_int("").unwrap_err().contains("empty"));
        assert!(parse_int("99999999999999999999").unwrap_err().contains("too large"));
        assert!(parse_int("1.5").is_err());
        assert_eq!(parse_float("2.5"), Ok(2.5));
        assert_eq!(parse_float("12"), Ok(12.0));
        assert_eq!(parse_float("-1e3"), Ok(-1000.0));
        assert_eq!(parse_float(".5"), Ok(0.5));
        assert!(parse_float("inf").is_err());
        assert!(parse_float("NaN").is_err());
        assert!(parse_float("1e").is_err());
        assert!(parse_float("abc").unwrap_err().contains("not a number"));
        assert!(parse_float("1e999").unwrap_err().contains("too large"));
    }

    #[test]
    fn int_to_float_reports_precision_loss() {
        assert_eq!(int_to_float(5), (5.0, true));
        assert_eq!(int_to_float(1 << 60), ((1u64 << 60) as f64, true));
        assert!(!int_to_float((1 << 53) + 1).1);
        assert!(!int_to_float(i64::MAX).1);
        assert!(int_to_float(i64::MIN).1);
    }

    #[test]
    fn integer_square_roots() {
        assert_eq!(isqrt(0), Ok(0));
        assert_eq!(isqrt(15), Ok(3));
        assert_eq!(isqrt(16), Ok(4));
        assert_eq!(isqrt(i64::MAX), Ok(3_037_000_499));
        assert_eq!(isqrt(-4), Err(BugKind::NegativeSquareRoot { value: -4 }));
    }

    #[test]
    fn rounding() {
        assert_eq!(float_floor(-2.5), Ok(-3));
        assert_eq!(float_ceil(-2.5), Ok(-2));
        assert_eq!(float_round(2.5), Ok(3));
        assert_eq!(float_round(-2.5), Ok(-3));
        assert!(float_round(f64::NAN).is_err());
    }
}

/// A Rational: numerator and denominator, simplified, the denominator positive (§8.3).
pub type Rational = [i64; 2];

/// `n over d` (§8.3): simplified; `d = 0` is a bug, and so is a part out of the Int range.
pub fn rational(numerator: i64, denominator: i64) -> Result<Rational, BugKind> {
    reduce(i128::from(numerator), i128::from(denominator))
}

fn reduce(mut numerator: i128, mut denominator: i128) -> Result<Rational, BugKind> {
    if denominator == 0 {
        return Err(BugKind::RationalDivisionByZero);
    }
    if denominator < 0 {
        numerator = -numerator;
        denominator = -denominator;
    }
    let divisor = gcd(numerator.unsigned_abs(), denominator.unsigned_abs()).max(1) as i128;
    let (numerator, denominator) = (numerator / divisor, denominator / divisor);
    match (i64::try_from(numerator), i64::try_from(denominator)) {
        (Ok(numerator), Ok(denominator)) => Ok([numerator, denominator]),
        _ => Err(BugKind::RationalOverflow),
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The operations on Rationals, exact (§8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RationalOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

pub fn rational_op(op: RationalOp, a: Rational, b: Rational) -> Result<Rational, BugKind> {
    let ([n1, d1], [n2, d2]) = (a.map(i128::from), b.map(i128::from));
    match op {
        RationalOp::Add => reduce(n1 * d2 + n2 * d1, d1 * d2),
        RationalOp::Subtract => reduce(n1 * d2 - n2 * d1, d1 * d2),
        RationalOp::Multiply => reduce(n1 * n2, d1 * d2),
        RationalOp::Divide => reduce(n1 * d2, d1 * n2),
    }
}

/// `r ^ n` with an Int exponent; a negative one inverts (§8.3, §8.4).
pub fn rational_pow(base: Rational, exponent: i64) -> Result<Rational, BugKind> {
    let [numerator, denominator] = base;
    let (numerator, denominator) =
        if exponent < 0 { (denominator, numerator) } else { (numerator, denominator) };
    let power = exponent.unsigned_abs();
    // 0, 1 and -1 have a power for any exponent: only its parity counts.
    let small = numerator.abs() <= 1 && denominator.abs() <= 1;
    let power = match u32::try_from(power) {
        Ok(power) => power,
        Err(_) if small => 2 + (power % 2) as u32,
        Err(_) => return Err(BugKind::RationalOverflow),
    };
    let raise = |value: i64| i128::from(value).checked_pow(power).ok_or(BugKind::RationalOverflow);
    reduce(raise(numerator)?, raise(denominator)?)
}

pub fn rational_compare(a: Rational, b: Rational) -> std::cmp::Ordering {
    (i128::from(a[0]) * i128::from(b[1])).cmp(&(i128::from(b[0]) * i128::from(a[1])))
}

pub fn rational_to_float(value: Rational) -> f64 {
    value[0] as f64 / value[1] as f64
}

#[cfg(test)]
mod rational_tests {
    use super::*;

    #[test]
    fn fractions_are_simplified_with_a_positive_denominator() {
        assert_eq!(rational(2, 4), Ok([1, 2]));
        assert_eq!(rational(4, -6), Ok([-2, 3]));
        assert_eq!(rational(0, -5), Ok([0, 1]));
        assert_eq!(rational(1, 0), Err(BugKind::RationalDivisionByZero));
        // -i64::MIN does not fit in an Int.
        assert_eq!(rational(i64::MIN, -1), Err(BugKind::RationalOverflow));
        assert_eq!(rational(i64::MIN, 2), Ok([i64::MIN / 2, 1]));
    }

    #[test]
    fn operations_are_exact() {
        let (third, half) = ([1, 3], [1, 2]);
        assert_eq!(rational_op(RationalOp::Add, third, half), Ok([5, 6]));
        assert_eq!(rational_op(RationalOp::Subtract, third, half), Ok([-1, 6]));
        assert_eq!(rational_op(RationalOp::Multiply, third, [3, 1]), Ok([1, 1]));
        assert_eq!(rational_op(RationalOp::Divide, third, half), Ok([2, 3]));
        assert_eq!(rational_op(RationalOp::Divide, third, [0, 1]), Err(BugKind::RationalDivisionByZero));
        assert_eq!(rational_pow([2, 3], -2), Ok([9, 4]));
        assert_eq!(rational_pow([0, 1], -1), Err(BugKind::RationalDivisionByZero));
        assert_eq!(rational_pow([2, 1], 63), Err(BugKind::RationalOverflow));
        assert_eq!(rational_pow([-1, 1], i64::MAX), Ok([-1, 1]));
        assert_eq!(rational_pow([0, 1], i64::MIN), Err(BugKind::RationalDivisionByZero));
        assert_eq!(rational_compare(third, half), std::cmp::Ordering::Less);
        assert_eq!(rational_compare([-1, 2], [-2, 4]), std::cmp::Ordering::Equal);
    }
}
