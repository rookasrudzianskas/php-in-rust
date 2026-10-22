//! `number_format()` (`_php_math_number_format_ex` and
//! `_php_math_number_format_long` in `ext/standard/math.c`).

use super::round::{PHP_ROUND_HALF_UP, php_math_round};

/// `NDIG` of `main/snprintf.c`: `%F` never prints more than `NDIG - 2` decimals.
const NDIG: usize = 320;

/// `strpprintf("%.*F", dec, d)` for a non-negative `d` (`php_conv_fp`):
/// `nan`/`inf` spelled in lowercase, at most `NDIG - 2` decimals.
fn format_f(d: f64, dec: usize) -> Vec<u8> {
    if d.is_nan() {
        return b"nan".to_vec();
    }
    if d.is_infinite() {
        return b"inf".to_vec();
    }
    let dec = dec.min(NDIG - 2);
    format!("{d:.dec$}").into_bytes()
}

/// Digits grouped by threes with `thousand_sep` (inserted between groups).
fn group_thousands(digits: &[u8], thousand_sep: &[u8], out: &mut Vec<u8>) {
    let len = digits.len();
    for (i, d) in digits.iter().enumerate() {
        out.push(*d);
        let remaining = len - i - 1;
        if remaining > 0 && remaining.is_multiple_of(3) {
            out.extend_from_slice(thousand_sep);
        }
    }
}

/// `_php_math_number_format_ex`.
pub fn number_format_ex(d: f64, dec: i32, dec_point: &[u8], thousand_sep: &[u8]) -> Vec<u8> {
    let mut d = d;
    let mut is_negative = false;
    if d < 0.0 {
        is_negative = true;
        d = -d;
    }

    d = php_math_round(d, dec, PHP_ROUND_HALF_UP);
    let dec = dec.max(0) as usize;
    let tmpbuf = format_f(d, dec);
    if !tmpbuf[0].is_ascii_digit() {
        return tmpbuf;
    }

    // Check if the number is no longer negative after rounding.
    if is_negative && d == 0.0 {
        is_negative = false;
    }

    // Find the decimal point, if expected.
    let dp = if dec > 0 {
        tmpbuf.iter().position(|c| *c == b'.' || *c == b',')
    } else {
        None
    };
    let (integral, fraction): (&[u8], &[u8]) = match dp {
        Some(i) => (&tmpbuf[..i], &tmpbuf[i + 1..]),
        None => (&tmpbuf[..], &[]),
    };

    let mut out: Vec<u8> = Vec::with_capacity(tmpbuf.len() + dec_point.len() + 8);
    if is_negative {
        out.push(b'-');
    }
    group_thousands(integral, thousand_sep, &mut out);
    if dec > 0 {
        out.extend_from_slice(dec_point);
        out.extend_from_slice(fraction);
        // The conversion may return fewer places than requested.
        let topad = dec.saturating_sub(fraction.len());
        out.resize(out.len() + topad, b'0');
    }
    out
}

/// `_php_math_number_format_long`: formats an integer (exactly, without going
/// through a double), rounding to negative `dec` places.
pub fn number_format_long(num: i64, dec: i64, dec_point: &[u8], thousand_sep: &[u8]) -> Vec<u8> {
    const POWERS: [u64; 20] = [
        1,
        10,
        100,
        1000,
        10000,
        100000,
        1000000,
        10000000,
        100000000,
        1000000000,
        10000000000,
        100000000000,
        1000000000000,
        10000000000000,
        100000000000000,
        1000000000000000,
        10000000000000000,
        100000000000000000,
        1000000000000000000,
        10000000000000000000,
    ];

    let mut is_negative = false;
    // Unsigned absolute value (also correct for i64::MIN).
    let mut tmpnum: u64 = if num < 0 {
        is_negative = true;
        ((-(num + 1)) as u64) + 1
    } else {
        num as u64
    };

    // Rounding the number.
    if dec < 0 {
        // Rounding to more negative places than possible gives 0.
        if dec < -(POWERS.len() as i64 - 1) {
            tmpnum = 0;
        } else {
            let power = POWERS[(-dec) as usize];
            let power_half = power / 2;
            let rest = tmpnum % power;
            tmpnum /= power;
            tmpnum = if rest >= power_half {
                tmpnum.wrapping_mul(power).wrapping_add(power)
            } else {
                tmpnum.wrapping_mul(power)
            };
        }
        // Prevent resulting in negative zero.
        if tmpnum == 0 {
            is_negative = false;
        }
    }

    let digits = tmpnum.to_string().into_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(digits.len() * 2 + 4);
    if is_negative {
        out.push(b'-');
    }
    group_thousands(&digits, thousand_sep, &mut out);
    if dec > 0 {
        out.extend_from_slice(dec_point);
        out.resize(out.len() + dec as usize, b'0');
    }
    out
}
