//! Numeric literal conversion (the number rules of the scanner) and the
//! `zend_gcvt()` float formatting used by the AST exporter.

/// The value of a numeric literal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Float(f64),
}

fn strip_underscores(s: &[u8]) -> Vec<u8> {
    s.iter().copied().filter(|&c| c != b'_').collect()
}

fn skip_leading_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| c == b'0' || c == b'_').count();
    &s[n..]
}

fn digit_value(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => u32::from(c - b'0'),
        b'a'..=b'f' => u32::from(c - b'a' + 10),
        b'A'..=b'F' => u32::from(c - b'A' + 10),
        _ => 0,
    }
}

/// Accumulate digits in a double, like `zend_hex_strtod()` and friends.
fn accumulate_float(digits: &[u8], base: f64) -> f64 {
    digits
        .iter()
        .fold(0.0, |v, &c| v * base + f64::from(digit_value(c)))
}

fn parse_int(digits: &[u8], base: u32) -> Option<i64> {
    let mut v: i64 = 0;
    for &c in digits {
        v = v
            .checked_mul(i64::from(base))?
            .checked_add(i64::from(digit_value(c)))?;
    }
    Some(v)
}

/// Digits after `0x`.
pub fn hex_literal(digits: &[u8]) -> Number {
    let d = strip_underscores(skip_leading_zeros(digits));
    if d.len() < 16 || (d.len() == 16 && d[0] <= b'7') {
        Number::Int(parse_int(&d, 16).unwrap_or(0))
    } else {
        Number::Float(accumulate_float(&d, 16.0))
    }
}

/// Digits after `0b`.
pub fn bin_literal(digits: &[u8]) -> Number {
    let d = strip_underscores(skip_leading_zeros(digits));
    if d.len() < 64 {
        Number::Int(parse_int(&d, 2).unwrap_or(0))
    } else {
        Number::Float(accumulate_float(&d, 2.0))
    }
}

/// Digits after `0o`.
pub fn oct_literal(digits: &[u8]) -> Number {
    let d = strip_underscores(skip_leading_zeros(digits));
    match parse_int(&d, 8) {
        Some(v) => Number::Int(v),
        None => Number::Float(accumulate_float(&d, 8.0)),
    }
}

/// A decimal (or legacy octal) integer literal. Returns `Err` with the value
/// of the valid prefix when a legacy octal literal contains `8` or `9`.
pub fn decimal_literal(text: &[u8]) -> Result<Number, Number> {
    let d = strip_underscores(text);
    let is_octal = text.first() == Some(&b'0');
    if is_octal && let Some(i) = d.iter().position(|&c| c == b'8' || c == b'9') {
        return Err(integer_value(&d[..i], true));
    }
    Ok(integer_value(&d, is_octal))
}

fn integer_value(d: &[u8], is_octal: bool) -> Number {
    let base = if is_octal { 8 } else { 10 };
    match parse_int(d, base) {
        Some(v) => Number::Int(v),
        None if is_octal => Number::Float(accumulate_float(d, 8.0)),
        None => Number::Float(parse_float_digits(d)),
    }
}

fn parse_float_digits(d: &[u8]) -> f64 {
    std::str::from_utf8(d)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(f64::INFINITY)
}

/// A `DNUM` / `EXPONENT_DNUM` literal.
pub fn float_literal(text: &[u8]) -> f64 {
    let d = strip_underscores(text);
    // Normalise `1.`, `.5` and `1.e5` so the standard parser accepts them.
    let mut s = String::with_capacity(d.len() + 2);
    let mut prev = 0u8;
    for (i, &c) in d.iter().enumerate() {
        if c == b'.' && i == 0 {
            s.push('0');
        }
        if (c == b'e' || c == b'E') && prev == b'.' {
            s.push('0');
        }
        s.push(c as char);
        prev = c;
    }
    if prev == b'.' {
        s.push('0');
    }
    s.parse::<f64>().unwrap_or(0.0)
}

/// A numeric string offset in `"$a[123]"`: `Some` when it is stored as an
/// integer.
pub fn offset_literal(digits: &[u8]) -> Option<i64> {
    if digits.len() < 19 || (digits.len() == 19 && digits < &b"9223372036854775808"[..]) {
        parse_int(digits, 10)
    } else {
        None
    }
}

/// `zend_gcvt(value, precision, '.', 'E')`.
pub fn gcvt(value: f64, precision: usize) -> String {
    if value.is_nan() {
        return "NAN".into();
    }
    if value.is_infinite() {
        return if value < 0.0 {
            "-INF".into()
        } else {
            "INF".into()
        };
    }
    let ndigit = precision.max(1);
    let mut out = String::new();
    if value.is_sign_negative() {
        out.push('-');
    }
    let v = value.abs();
    let (digits, decpt) = if v == 0.0 {
        ("0".to_string(), 1i32)
    } else {
        let s = format!("{:.*e}", ndigit - 1, v);
        let (mant, exp) = s.split_once('e').expect("exponent");
        let exp: i32 = exp.parse().expect("exponent value");
        let mut digits: String = mant.chars().filter(|c| *c != '.').collect();
        while digits.len() > 1 && digits.ends_with('0') {
            digits.pop();
        }
        (digits, exp + 1)
    };
    let ndigit = ndigit as i32;
    if if decpt < 0 {
        decpt < -3
    } else {
        decpt > ndigit
    } {
        let e = decpt - 1;
        let mut chars = digits.chars();
        out.push(chars.next().unwrap_or('0'));
        out.push('.');
        let rest: String = chars.collect();
        if rest.is_empty() {
            out.push('0');
        } else {
            out.push_str(&rest);
        }
        out.push('E');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.abs().to_string());
    } else if decpt < 0 {
        out.push_str("0.");
        for _ in 0..-decpt {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let d = digits.as_bytes();
        for i in 0..decpt as usize {
            out.push(d.get(i).map(|&c| c as char).unwrap_or('0'));
        }
        if (decpt as usize) < d.len() {
            if decpt == 0 {
                out.push('0');
            }
            out.push('.');
            out.push_str(&digits[decpt as usize..]);
        }
    }
    out
}

/// `smart_str_append_double(value, precision, zero_fraction = true)`.
pub fn format_double_zero_fraction(value: f64, precision: usize) -> String {
    let mut s = gcvt(value, precision);
    if value.is_finite() && !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        assert_eq!(decimal_literal(b"123"), Ok(Number::Int(123)));
        assert_eq!(decimal_literal(b"0777"), Ok(Number::Int(511)));
        assert_eq!(decimal_literal(b"1_000"), Ok(Number::Int(1000)));
        assert_eq!(
            decimal_literal(b"9223372036854775807"),
            Ok(Number::Int(i64::MAX))
        );
        assert_eq!(
            decimal_literal(b"9223372036854775808"),
            Ok(Number::Float(9223372036854775808.0))
        );
        assert_eq!(decimal_literal(b"078"), Err(Number::Int(7)));
        assert_eq!(hex_literal(b"7FFFFFFFFFFFFFFF"), Number::Int(i64::MAX));
        assert_eq!(
            hex_literal(b"8000000000000000"),
            Number::Float(9223372036854775808.0)
        );
        assert_eq!(hex_literal(b"0000ff"), Number::Int(255));
        assert_eq!(bin_literal(b"1_01"), Number::Int(5));
        assert_eq!(oct_literal(b"17"), Number::Int(15));
        assert_eq!(float_literal(b"1."), 1.0);
        assert_eq!(float_literal(b".5"), 0.5);
        assert_eq!(float_literal(b"1.e3"), 1000.0);
        assert_eq!(float_literal(b"1_0.5e1"), 105.0);
        assert_eq!(offset_literal(b"9223372036854775807"), Some(i64::MAX));
        assert_eq!(offset_literal(b"9223372036854775808"), None);
    }

    #[test]
    fn gcvt_formats() {
        assert_eq!(gcvt(1.5, 14), "1.5");
        assert_eq!(gcvt(1.0, 14), "1");
        assert_eq!(gcvt(0.1 + 0.2, 14), "0.3");
        assert_eq!(gcvt(1e15, 14), "1.0E+15");
        assert_eq!(gcvt(1e14, 14), "1.0E+14");
        assert_eq!(gcvt(123456789012.0, 14), "123456789012");
        assert_eq!(gcvt(0.0001, 14), "0.0001");
        assert_eq!(gcvt(0.00001, 14), "1.0E-5");
        assert_eq!(gcvt(-2.5, 14), "-2.5");
        assert_eq!(gcvt(1.25e-10, 14), "1.25E-10");
        assert_eq!(format_double_zero_fraction(1.0, 14), "1.0");
        assert_eq!(format_double_zero_fraction(0.0, 14), "0.0");
        assert_eq!(format_double_zero_fraction(-0.0, 14), "-0.0");
        assert_eq!(format_double_zero_fraction(1e100, 14), "1.0E+100");
        assert_eq!(format_double_zero_fraction(f64::INFINITY, 14), "INF");
    }
}
