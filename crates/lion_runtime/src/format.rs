//! How values are written as text, by `show`, interpolation and `as Text`.

/// Writes a Float as the shortest decimal that reads back as the same value, always
/// with a fractional part so it cannot be mistaken for an Int: `3.0`, `0.1`,
/// `0.30000000000000004`. Values below 1e-4 or from 1e16 use an exponent, written
/// as a valid Lion literal (`1.0e16`, `2.5e-7`). The special values are `NaN`,
/// `Infinity` and `-Infinity`.
pub fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's `Debug` output is the shortest round-trip representation, with the
    // same notation thresholds; only its exponent form lacks the `.0`.
    let mut text = format!("{value:?}");
    if let Some(exponent) = text.find('e')
        && !text[..exponent].contains('.')
    {
        text.insert_str(exponent, ".0");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::format_float;

    #[test]
    fn floats_keep_a_fractional_part() {
        assert_eq!(format_float(3.0), "3.0");
        assert_eq!(format_float(3.5), "3.5");
        assert_eq!(format_float(-0.0), "-0.0");
        assert_eq!(format_float(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(format_float(1e15), "1000000000000000.0");
        assert_eq!(format_float(0.0001), "0.0001");
    }

    #[test]
    fn large_and_small_floats_use_an_exponent() {
        assert_eq!(format_float(1e16), "1.0e16");
        assert_eq!(format_float(1.5e300), "1.5e300");
        assert_eq!(format_float(1e-5), "1.0e-5");
        assert_eq!(format_float(-2.5e-7), "-2.5e-7");
        assert_eq!(format_float(f64::MAX), "1.7976931348623157e308");
    }

    #[test]
    fn special_values() {
        assert_eq!(format_float(f64::NAN), "NaN");
        assert_eq!(format_float(f64::INFINITY), "Infinity");
        assert_eq!(format_float(f64::NEG_INFINITY), "-Infinity");
    }
}
