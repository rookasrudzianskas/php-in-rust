//! Float formatting: ports of `zend_gcvt` (`zend_strtod.c`), `zend_double_to_str`
//! (`zend_operators.c`), `smart_str_append_double` (`zend_smart_str.c`) and the
//! `%H` conversion of `main/snprintf.c`.
//!
//! * `precision` ini (default 14) drives `echo`/string conversion: `%.14G` rules.
//! * `serialize_precision` (default -1) drives `var_dump`/`var_export`/`json_encode`:
//!   `-1` selects the shortest representation that round-trips (`zend_dtoa` mode 0).

/// Decimal digits (no leading/trailing zeros, except `"0"` for zero) and the
/// decimal point position (`value = 0.d1d2d3... × 10^decpt`), plus the sign.
struct Digits {
    digits: Vec<u8>,
    decpt: i32,
    negative: bool,
}

/// `zend_dtoa(value, mode, ndigits)` for finite values: mode 0 when
/// `ndigits` is `None` (shortest round-trip), mode 2 otherwise (correctly
/// rounded to `ndigits` significant digits, trailing zeros suppressed).
fn dtoa(value: f64, ndigits: Option<usize>) -> Digits {
    let negative = value.is_sign_negative();
    let a = value.abs();
    if a == 0.0 {
        return Digits {
            digits: vec![b'0'],
            decpt: 1,
            negative,
        };
    }
    let s = match ndigits {
        None => format!("{a:e}"),
        Some(n) => format!("{:.*e}", n.max(1) - 1, a),
    };
    let (mant, exp) = s.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent value");
    let mut digits: Vec<u8> = mant.bytes().filter(|c| *c != b'.').collect();
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    Digits {
        digits,
        decpt: exp + 1,
        negative,
    }
}

/// `zend_gcvt(value, ndigit, '.', exp_char, buf)`. A negative `ndigit`
/// selects mode 0 (shortest) with a 17-digit threshold for exponential notation.
pub fn gcvt(value: f64, ndigit: i32, exp_char: u8) -> String {
    let (mode0, ndigit) = if ndigit < 0 {
        (true, 17)
    } else {
        (false, ndigit)
    };
    if !value.is_finite() {
        // snprintf(buf, ndigit + 1, ...) truncates to `ndigit` characters.
        let s = if value.is_nan() {
            "NAN"
        } else if value < 0.0 {
            "-INF"
        } else {
            "INF"
        };
        let keep = (ndigit.max(0) as usize).min(s.len());
        return s[..keep].to_string();
    }
    let d = dtoa(
        value,
        if mode0 {
            None
        } else {
            Some(ndigit.max(1) as usize)
        },
    );
    let mut out = String::with_capacity(24);
    if d.negative {
        out.push('-');
    }
    let digits = &d.digits;
    let mut decpt = d.decpt;
    if (decpt >= 0 && decpt > ndigit) || decpt < -3 {
        decpt -= 1;
        let (esign, eabs) = if decpt < 0 {
            ('-', -decpt)
        } else {
            ('+', decpt)
        };
        out.push(digits[0] as char);
        out.push('.');
        if digits.len() == 1 {
            out.push('0');
        } else {
            for &c in &digits[1..] {
                out.push(c as char);
            }
        }
        out.push(exp_char as char);
        out.push(esign);
        out.push_str(&eabs.to_string());
    } else if decpt < 0 {
        out.push_str("0.");
        for _ in decpt..0 {
            out.push('0');
        }
        for &c in digits {
            out.push(c as char);
        }
    } else {
        let decpt = decpt as usize;
        for i in 0..decpt {
            out.push(*digits.get(i).unwrap_or(&b'0') as char);
        }
        if digits.len() > decpt {
            if decpt == 0 {
                out.push('0');
            }
            out.push('.');
            for &c in &digits[decpt..] {
                out.push(c as char);
            }
        }
    }
    out
}

/// `zend_double_to_str` / `smart_str_append_double` precision handling:
/// `0` behaves like `1`, `-1` selects the shortest round-trip form.
fn effective_precision(precision: i64) -> i32 {
    match precision {
        0 => 1,
        p if p < 0 => -1,
        p => p.min(i32::MAX as i64) as i32,
    }
}

/// Float to string as done by `(string)$f` and `echo` with the given
/// `precision` ini value (`zend_double_to_str`). Does not emit the NaN warning.
pub fn double_to_string(value: f64, precision: i64) -> String {
    gcvt(value, effective_precision(precision), b'E')
}

/// `smart_str_append_double(dest, num, precision, zero_fraction)`: like
/// [`double_to_string`], appending `.0` to finite integral results when
/// `zero_fraction` is set (`var_export`, `json_encode` with `JSON_PRESERVE_ZERO_FRACTION`).
pub fn append_double(out: &mut Vec<u8>, value: f64, precision: i64, zero_fraction: bool) {
    let s = gcvt(value, effective_precision(precision), b'E');
    out.extend_from_slice(s.as_bytes());
    if zero_fraction && value.is_finite() && !s.contains('.') {
        out.extend_from_slice(b".0");
    }
}

/// The `%.*H` conversion of php's `spprintf` (used by `var_dump`, `debug_zval_dump`
/// and warnings that print floats): `NAN`, `INF`, `-INF` are spelled out; precision
/// `-1` selects mode 0 and `0` behaves like `1`.
pub fn format_h(value: f64, precision: i64) -> String {
    if value.is_nan() {
        return "NAN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "INF".into()
        } else {
            "-INF".into()
        };
    }
    gcvt(value, effective_precision(precision), b'E')
}

/// `var_dump`/`var_export` style float rendering with `serialize_precision`.
pub fn repr(value: f64, serialize_precision: i64) -> String {
    format_h(value, serialize_precision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(d: f64) -> String {
        double_to_string(d, 14)
    }

    fn dump(d: f64) -> String {
        format_h(d, -1)
    }

    #[test]
    fn echo_precision_14() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (0.1 + 0.2, "0.3"),
            (1.0 / 3.0, "0.33333333333333"),
            (100000.0, "100000"),
            (1e14, "1.0E+14"),
            (1e13, "10000000000000"),
            (123456789012345.0, "1.2345678901234E+14"),
            (1e15, "1.0E+15"),
            (1e25, "1.0E+25"),
            (1.5e-7, "1.5E-7"),
            (0.0001, "0.0001"),
            (0.00001, "1.0E-5"),
            (-0.00001234, "-1.234E-5"),
            (3.0e100, "3.0E+100"),
            (f64::INFINITY, "INF"),
            (f64::NEG_INFINITY, "-INF"),
            (f64::NAN, "NAN"),
            (2.5, "2.5"),
            (7.0E-10, "7.0E-10"),
            (9007199254740993.0, "9.007199254741E+15"),
            (1.2345678901234568, "1.2345678901235"),
            (-1.0E-10, "-1.0E-10"),
            (99999999999999.0, "99999999999999"),
            (999999999999999.0, "1.0E+15"),
            (0.5, "0.5"),
            (12345.678, "12345.678"),
        ];
        for (d, want) in cases {
            assert_eq!(echo(*d), *want, "echo {d:e}");
        }
    }

    #[test]
    fn serialize_precision_minus_one() {
        let cases: &[(f64, &str)] = &[
            (0.1, "0.1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.0, "1"),
            (-0.0, "-0"),
            (1e15, "1000000000000000"),
            (1e16, "10000000000000000"),
            (123456789012345678.0, "1.2345678901234568E+17"),
            (12345678901234567.0, "12345678901234568"),
            (9.223372036854776E+18, "9.223372036854776E+18"),
            (1e17, "1.0E+17"),
            (1.5e300, "1.5E+300"),
            (1e-5, "1.0E-5"),
            (0.0001, "0.0001"),
            (1.0 / 3.0, "0.3333333333333333"),
            (5e-324, "5.0E-324"),
            (f64::MAX, "1.7976931348623157E+308"),
            (f64::INFINITY, "INF"),
            (f64::NEG_INFINITY, "-INF"),
            (f64::NAN, "NAN"),
            (100000.0, "100000"),
            (3.0910586430935376E+39, "3.0910586430935376E+39"),
            (0.8947368421052632, "0.8947368421052632"),
        ];
        for (d, want) in cases {
            assert_eq!(dump(*d), *want, "dump {d:e}");
        }
    }

    #[test]
    fn mode0_threshold_is_17_digits() {
        assert_eq!(double_to_string(1e15, -1), "1000000000000000");
        assert_eq!(double_to_string(1e17, -1), "1.0E+17");
        assert_eq!(double_to_string(123456789012345.0, -1), "123456789012345");
        assert_eq!(double_to_string(4503599627370496.0, -1), "4503599627370496");
    }

    #[test]
    fn zero_fraction_and_small_precision() {
        let mut v = Vec::new();
        append_double(&mut v, 1.0, -1, true);
        assert_eq!(v, b"1.0");
        let mut v = Vec::new();
        append_double(&mut v, 1.5, -1, true);
        assert_eq!(v, b"1.5");
        let mut v = Vec::new();
        append_double(&mut v, f64::INFINITY, -1, true);
        assert_eq!(v, b"INF");
        let mut v = Vec::new();
        append_double(&mut v, 1e25, 17, true);
        assert_eq!(v, b"1.0000000000000001E+25");
        assert_eq!(double_to_string(0.1, 17), "0.10000000000000001");
        assert_eq!(double_to_string(1234.5, 1), "1.0E+3");
        assert_eq!(double_to_string(1234.5, 0), "1.0E+3");
        assert_eq!(double_to_string(0.125, 2), "0.12");
        assert_eq!(double_to_string(0.375, 2), "0.38");
        assert_eq!(double_to_string(f64::INFINITY, 1), "I");
        assert_eq!(double_to_string(f64::NEG_INFINITY, 3), "-IN");
        assert_eq!(format_h(f64::NEG_INFINITY, 1), "-INF");
    }
}
