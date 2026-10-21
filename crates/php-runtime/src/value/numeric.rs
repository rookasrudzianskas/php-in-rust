//! Numeric strings and float↔int conversion (ports of `_is_numeric_string_ex`,
//! `zend_strtod`, `zend_dval_to_lval*` from `zend_operators.[ch]` / `zend_strtod.c`).

/// `MAX_LENGTH_OF_LONG` on 64-bit platforms.
pub const MAX_LENGTH_OF_LONG: usize = 20;

/// A parsed number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Numeric {
    Int(i64),
    Float(f64),
}

impl Numeric {
    pub fn to_f64(self) -> f64 {
        match self {
            Numeric::Int(i) => i as f64,
            Numeric::Float(d) => d,
        }
    }

    pub fn into_value(self) -> super::Value {
        match self {
            Numeric::Int(i) => super::Value::Int(i),
            Numeric::Float(d) => super::Value::Float(d),
        }
    }
}

/// Full result of [`is_numeric_string_ex`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericInfo {
    pub num: Numeric,
    /// Only possible with `allow_errors`: the string is "leading-numeric".
    pub trailing_data: bool,
    /// `oflow_info`: `1`/`-1` if an integer literal overflowed to float.
    pub overflow: i8,
}

/// The bytes up to the first NUL (how C's `%s` prints a `zend_string`).
pub fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(i) => &s[..i],
        None => s,
    }
}

/// Whitespace accepted around numeric strings (`" \t\n\r\v\f"`).
#[inline]
pub fn is_numeric_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

const LONG_MIN_DIGITS: &[u8] = b"9223372036854775808";

/// C `strcmp` of the NUL-terminated prefix of `a` against `b`.
fn c_strcmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let a = match a.iter().position(|&c| c == 0) {
        Some(n) => &a[..n],
        None => a,
    };
    a.cmp(b)
}

/// `_is_numeric_string_ex` (with both `lval` and `dval` requested).
///
/// Leading whitespace and (PHP 8) trailing whitespace are allowed. With
/// `allow_errors`, a numeric prefix followed by garbage is accepted and
/// `trailing_data` is set ("leading-numeric string"). Hex, octal and binary
/// notations are not numeric. Integers that do not fit in `i64` become floats.
pub fn is_numeric_string_ex(s: &[u8], allow_errors: bool) -> Option<NumericInfo> {
    let n = s.len();
    if n == 0 || s[0] > b'9' {
        return None;
    }
    let mut i = 0;
    while i < n && is_numeric_ws(s[i]) {
        i += 1;
    }
    let start = i;
    let mut p = i;
    let neg = p < n && s[p] == b'-';
    if p < n && (s[p] == b'-' || s[p] == b'+') {
        p += 1;
    }
    let sign: i8 = if neg { -1 } else { 1 };
    let mut overflow = 0i8;
    let mut is_double = false;
    let mut dval = 0.0;
    let mut tmp: u64 = 0;
    let mut digits = 0usize;

    if p < n && s[p].is_ascii_digit() {
        while p < n && s[p] == b'0' {
            p += 1;
        }
        loop {
            if digits >= MAX_LENGTH_OF_LONG {
                overflow = sign;
                is_double = true;
                break;
            }
            if p < n && s[p].is_ascii_digit() {
                tmp = tmp.wrapping_mul(10).wrapping_add((s[p] - b'0') as u64);
                digits += 1;
                p += 1;
                continue;
            }
            if p < n && s[p] == b'.' {
                is_double = true;
            } else if p < n && (s[p] == b'e' || s[p] == b'E') {
                let mut e = p + 1;
                if e < n && (s[e] == b'-' || s[e] == b'+') {
                    // php-src: `ptr = e++` leaves `ptr` on the sign, which shifts
                    // the 19-digit overflow comparison below by one byte.
                    p = e;
                    e += 1;
                }
                if e < n && s[e].is_ascii_digit() {
                    is_double = true;
                }
            }
            break;
        }
    } else if p + 1 < n && s[p] == b'.' && s[p + 1].is_ascii_digit() {
        is_double = true;
    } else {
        return None;
    }

    if is_double {
        let (d, used) = strtod(&s[start..]);
        dval = d;
        p = start + used;
    }

    let mut trailing_data = false;
    if p != n {
        let mut q = p;
        while q < n && is_numeric_ws(s[q]) {
            q += 1;
        }
        if q != n {
            if !allow_errors {
                return None;
            }
            trailing_data = true;
        }
    }

    if !is_double {
        if digits == MAX_LENGTH_OF_LONG - 1 {
            let cmp = c_strcmp(&s[p - digits..], LONG_MIN_DIGITS);
            if !(cmp.is_lt() || (cmp.is_eq() && neg)) {
                return Some(NumericInfo {
                    num: Numeric::Float(strtod(&s[start..]).0),
                    trailing_data,
                    overflow: sign,
                });
            }
        }
        let v = if neg {
            (tmp as i64).wrapping_neg()
        } else {
            tmp as i64
        };
        return Some(NumericInfo {
            num: Numeric::Int(v),
            trailing_data,
            overflow: 0,
        });
    }
    Some(NumericInfo {
        num: Numeric::Float(dval),
        trailing_data,
        overflow,
    })
}

/// `is_numeric_string(str, len, &l, &d, allow_errors)`.
pub fn is_numeric_string(s: &[u8], allow_errors: bool) -> Option<Numeric> {
    is_numeric_string_ex(s, allow_errors).map(|i| i.num)
}

/// `is_numeric_str_function`: whole-string numeric check (whitespace allowed).
pub fn is_numeric_str(s: &[u8]) -> Option<Numeric> {
    is_numeric_string(s, false)
}

/// `zend_strtod`: parses a decimal float prefix (after optional whitespace).
/// Returns the value and the number of bytes consumed (`0` if no number).
/// Hexadecimal floats, `inf` and `nan` are not recognised.
pub fn strtod(s: &[u8]) -> (f64, usize) {
    let n = s.len();
    let mut i = 0;
    while i < n && is_numeric_ws(s[i]) {
        i += 1;
    }
    let num_start = i;
    if i < n && (s[i] == b'-' || s[i] == b'+') {
        i += 1;
    }
    let mant = i;
    while i < n && s[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - mant;
    if i < n && s[i] == b'.' {
        let mut k = i + 1;
        while k < n && s[k].is_ascii_digit() {
            k += 1;
        }
        if int_digits > 0 || k > i + 1 {
            i = k;
        }
    }
    if i == mant || (int_digits == 0 && i == mant + 1) {
        return (0.0, 0);
    }
    if i < n && (s[i] == b'e' || s[i] == b'E') {
        let mut k = i + 1;
        if k < n && (s[k] == b'+' || s[k] == b'-') {
            k += 1;
        }
        let ds = k;
        while k < n && s[k].is_ascii_digit() {
            k += 1;
        }
        if k > ds {
            i = k;
        }
    }
    // The validated slice is pure ASCII in Rust's float grammar.
    let text = std::str::from_utf8(&s[num_start..i]).expect("ascii");
    let v = text.parse::<f64>().unwrap_or(0.0);
    (v, i)
}

/// `zend_strtod(str, NULL)`: the float value of a string prefix (0.0 if none).
pub fn strtod_value(s: &[u8]) -> f64 {
    strtod(s).0
}

/// `zend_hex_strtod` (accumulates in floating point exactly like php-src).
pub fn hex_strtod(s: &[u8]) -> (f64, usize) {
    let mut i = if s.len() >= 2 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
        2
    } else {
        0
    };
    let start = i;
    let mut v = 0.0f64;
    while i < s.len() {
        let d = match s[i] {
            c @ b'0'..=b'9' => c - b'0',
            c @ b'a'..=b'f' => c - b'a' + 10,
            c @ b'A'..=b'F' => c - b'A' + 10,
            _ => break,
        };
        v = v * 16.0 + d as f64;
        i += 1;
    }
    (v, if i > start { i } else { 0 })
}

/// `zend_oct_strtod`.
pub fn oct_strtod(s: &[u8]) -> (f64, usize) {
    let mut i = 0;
    let mut v = 0.0f64;
    while i < s.len() && (b'0'..=b'7').contains(&s[i]) {
        v = v * 8.0 + (s[i] - b'0') as f64;
        i += 1;
    }
    (v, i)
}

/// `zend_bin_strtod`.
pub fn bin_strtod(s: &[u8]) -> (f64, usize) {
    let mut i = if s.len() >= 2 && s[0] == b'0' && (s[1] == b'b' || s[1] == b'B') {
        2
    } else {
        0
    };
    let start = i;
    let mut v = 0.0f64;
    while i < s.len() && (s[i] == b'0' || s[i] == b'1') {
        v = v * 2.0 + (s[i] - b'0') as f64;
        i += 1;
    }
    (v, if i > start { i } else { 0 })
}

/// `ZEND_DOUBLE_FITS_LONG` (64-bit). NaN "fits" (callers check finiteness first).
#[inline]
pub fn double_fits_long(d: f64) -> bool {
    !(d >= 9_223_372_036_854_775_808.0 || d < -9_223_372_036_854_775_808.0)
}

/// `zend_dval_to_lval_slow`: modular conversion of an out-of-range float.
pub fn dval_to_lval_modular(d: f64) -> i64 {
    const TWO_POW_64: f64 = 18_446_744_073_709_551_616.0;
    let mut dmod = d % TWO_POW_64;
    if dmod < 0.0 {
        dmod += TWO_POW_64;
    }
    (dmod as u64) as i64
}

/// `zend_dval_to_lval` without the warning: NaN/±Inf → 0, out of range →
/// modular arithmetic, otherwise truncation toward zero.
#[inline]
pub fn dval_to_lval_silent(d: f64) -> i64 {
    if !d.is_finite() {
        0
    } else if !double_fits_long(d) {
        dval_to_lval_modular(d)
    } else {
        d as i64
    }
}

/// `zend_dval_to_lval_cap`: saturating conversion (used for numeric strings).
#[inline]
pub fn dval_to_lval_cap(d: f64) -> i64 {
    if !d.is_finite() {
        0
    } else if !double_fits_long(d) {
        if d > 0.0 { i64::MAX } else { i64::MIN }
    } else {
        d as i64
    }
}

/// `zend_is_long_compatible`.
#[inline]
pub fn is_long_compatible(d: f64, l: i64) -> bool {
    (l as f64) == d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str) -> Option<Numeric> {
        is_numeric_str(s.as_bytes())
    }

    fn lead(s: &str) -> Option<(Numeric, bool)> {
        is_numeric_string_ex(s.as_bytes(), true).map(|i| (i.num, i.trailing_data))
    }

    #[test]
    fn plain_numbers() {
        assert_eq!(num("123"), Some(Numeric::Int(123)));
        assert_eq!(num("-123"), Some(Numeric::Int(-123)));
        assert_eq!(num("+123"), Some(Numeric::Int(123)));
        assert_eq!(num("0"), Some(Numeric::Int(0)));
        assert_eq!(num("000123"), Some(Numeric::Int(123)));
        assert_eq!(num("1.5"), Some(Numeric::Float(1.5)));
        assert_eq!(num(".5"), Some(Numeric::Float(0.5)));
        assert_eq!(num("-.5"), Some(Numeric::Float(-0.5)));
        assert_eq!(num("1."), Some(Numeric::Float(1.0)));
        assert_eq!(num("1e3"), Some(Numeric::Float(1000.0)));
        assert_eq!(num("1E-3"), Some(Numeric::Float(0.001)));
        assert_eq!(num("1e+3"), Some(Numeric::Float(1000.0)));
    }

    #[test]
    fn whitespace_rules() {
        for s in [
            "123",
            "123   ",
            "123 \t\n\r\x0b\x0c",
            "   123",
            " \t\n\r\x0b\x0c123",
            "   123   ",
        ] {
            assert_eq!(num(s), Some(Numeric::Int(123)), "{s:?}");
        }
        assert_eq!(num(" 1.5 "), Some(Numeric::Float(1.5)));
        assert_eq!(num(""), None);
        assert_eq!(num(" "), None);
        assert_eq!(num("- 1"), None);
        assert_eq!(num("1 2"), None);
    }

    #[test]
    fn non_numeric() {
        for s in [
            "abc", "0x1A", "0b11", "1e", "1e+", ".", "-", "+", "1_000", "INF", "NAN", "1.5.5", "e5",
        ] {
            assert_eq!(num(s), None, "{s:?}");
        }
    }

    #[test]
    fn leading_numeric() {
        assert_eq!(lead("123abc"), Some((Numeric::Int(123), true)));
        assert_eq!(lead("1.5x"), Some((Numeric::Float(1.5), true)));
        assert_eq!(lead("1e5x"), Some((Numeric::Float(1e5), true)));
        assert_eq!(lead("1ex"), Some((Numeric::Int(1), true)));
        assert_eq!(lead("0x1A"), Some((Numeric::Int(0), true)));
        assert_eq!(lead("12 "), Some((Numeric::Int(12), false)));
        assert_eq!(lead("12 x"), Some((Numeric::Int(12), true)));
        assert_eq!(lead("abc"), None);
        assert_eq!(lead("1\0"), Some((Numeric::Int(1), true)));
    }

    #[test]
    fn integer_overflow_to_float() {
        assert_eq!(num("9223372036854775807"), Some(Numeric::Int(i64::MAX)));
        assert_eq!(num("-9223372036854775808"), Some(Numeric::Int(i64::MIN)));
        assert_eq!(
            num("9223372036854775808"),
            Some(Numeric::Float(9223372036854775808.0))
        );
        assert_eq!(
            num("-9223372036854775809"),
            Some(Numeric::Float(-9223372036854775809.0))
        );
        assert_eq!(num("99999999999999999999"), Some(Numeric::Float(1e20)));
        let info = is_numeric_string_ex(b"9223372036854775808", false).unwrap();
        assert_eq!(info.overflow, 1);
        let info = is_numeric_string_ex(b"-99999999999999999999", false).unwrap();
        assert_eq!(info.overflow, -1);
        assert_eq!(num("00000000000000000000000001"), Some(Numeric::Int(1)));
    }

    #[test]
    fn strtod_prefixes() {
        assert_eq!(strtod(b"1.5abc"), (1.5, 3));
        assert_eq!(strtod(b"  -2e3x"), (-2000.0, 6));
        assert_eq!(strtod(b"abc"), (0.0, 0));
        assert_eq!(strtod(b"1e"), (1.0, 1));
        assert_eq!(strtod(b"5."), (5.0, 2));
        assert_eq!(strtod(b"."), (0.0, 0));
        assert_eq!(strtod(b"1e999").0, f64::INFINITY);
        assert_eq!(strtod(b"0x10"), (0.0, 1));
    }

    #[test]
    fn float_to_int() {
        assert_eq!(dval_to_lval_silent(f64::NAN), 0);
        assert_eq!(dval_to_lval_silent(f64::INFINITY), 0);
        assert_eq!(dval_to_lval_silent(-1.9), -1);
        assert_eq!(dval_to_lval_silent(1e19), 1e19 as u64 as i64);
        assert_eq!(dval_to_lval_silent(9223372036854775808.0), i64::MIN);
        assert_eq!(dval_to_lval_silent(18446744073709551616.0), 0);
        assert_eq!(
            dval_to_lval_silent(-9223372036854777856.0),
            9223372036854773760
        );
        assert_eq!(dval_to_lval_cap(1e30), i64::MAX);
        assert_eq!(dval_to_lval_cap(-1e30), i64::MIN);
        assert!(double_fits_long(-9223372036854775808.0));
        assert!(!double_fits_long(9223372036854775808.0));
    }

    #[test]
    fn hex_oct_bin() {
        assert_eq!(hex_strtod(b"0xFF"), (255.0, 4));
        assert_eq!(
            hex_strtod(b"0x7FFFFFFFFFFFFFFFF").0,
            147573952589676412927.0
        );
        assert_eq!(oct_strtod(b"777"), (511.0, 3));
        assert_eq!(bin_strtod(b"0b101"), (5.0, 5));
    }
}
