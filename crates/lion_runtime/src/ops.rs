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
            result = result.checked_mul(square).ok_or(fail)?;
        }
        remaining >>= 1;
        if remaining == 0 {
            return Ok(result);
        }
        // With |base| >= 2, a square that overflows would make the result overflow too.
        square = square.checked_mul(square).ok_or(fail)?;
    }
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
    fn int_to_float_reports_precision_loss() {
        assert_eq!(int_to_float(5), (5.0, true));
        assert_eq!(int_to_float(1 << 60), ((1u64 << 60) as f64, true));
        assert!(!int_to_float((1 << 53) + 1).1);
        assert!(!int_to_float(i64::MAX).1);
        assert!(int_to_float(i64::MIN).1);
    }
}
