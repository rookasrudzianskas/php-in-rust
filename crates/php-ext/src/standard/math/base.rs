//! Number base conversions (`_php_math_basetozval`, `_php_math_longtobase`,
//! `_php_math_zvaltobase`, `_php_math_longtobase_pwr2` in `ext/standard/math.c`).

use php_runtime::errors::level::{E_DEPRECATED, E_NOTICE};
use php_runtime::{PResult, PhpStr, Value, Vm};

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// `ZEND_DOUBLE_MAX_LENGTH`: `3 + DBL_MANT_DIG - DBL_MIN_EXP`.
const DOUBLE_MAX_LENGTH: usize = 3 + 53 + 1021;

/// C `isspace()` in the "C" locale.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `_php_math_basetozval`: parses `s` as a base-`base` number, ignoring
/// invalid characters (with a deprecation) and overflowing into a float (with a
/// notice).
pub fn basetozval(vm: &mut Vm, s: &[u8], base: u32) -> PResult<Value> {
    let mut start = 0;
    let mut end = s.len();
    while start < end && is_space(s[start]) {
        start += 1;
    }
    while start < end && is_space(s[end - 1]) {
        end -= 1;
    }
    if end - start >= 2 && s[start] == b'0' {
        let p = s[start + 1];
        if (base == 16 && (p == b'x' || p == b'X'))
            || (base == 8 && (p == b'o' || p == b'O'))
            || (base == 2 && (p == b'b' || p == b'B'))
        {
            start += 2;
        }
    }

    let base_i = base as i64;
    let cutoff = i64::MAX / base_i;
    let cutlim = i64::MAX % base_i;
    let mut num: i64 = 0;
    let mut fnum: f64 = 0.0;
    let mut float_mode = false;
    let mut invalid_chars = 0usize;

    for &ch in &s[start..end] {
        let c: i64 = match ch {
            b'0'..=b'9' => (ch - b'0') as i64,
            b'A'..=b'Z' => (ch - b'A') as i64 + 10,
            b'a'..=b'z' => (ch - b'a') as i64 + 10,
            _ => {
                invalid_chars += 1;
                continue;
            }
        };
        if c >= base_i {
            invalid_chars += 1;
            continue;
        }
        if !float_mode {
            if num < cutoff || (num == cutoff && c <= cutlim) {
                num = num * base_i + c;
                continue;
            }
            vm.zend_error(
                E_NOTICE,
                "Input number is larger than PHP_INT_MAX, precision has been lost in conversion",
            )?;
            fnum = num as f64;
            float_mode = true;
        }
        fnum = fnum * base as f64 + c as f64;
    }

    if invalid_chars > 0 {
        vm.zend_error(
            E_DEPRECATED,
            "Invalid characters passed for attempted conversion, these have been ignored",
        )?;
    }

    Ok(if float_mode {
        Value::Float(fnum)
    } else {
        Value::Int(num)
    })
}

/// `_php_math_longtobase`: `arg` as an unsigned number in base 2..=36.
pub fn longtobase(arg: i64, base: u32) -> Vec<u8> {
    if !(2..=36).contains(&base) {
        return Vec::new();
    }
    let mut value = arg as u64;
    let base = base as u64;
    let mut out = Vec::with_capacity(64);
    loop {
        out.push(DIGITS[(value % base) as usize]);
        value /= base;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    out
}

/// `_php_math_longtobase_pwr2` for `base_log2` of 1 (binary), 3 (octal) or 4 (hex).
pub fn longtobase_pwr2(arg: i64, base_log2: u32) -> Vec<u8> {
    let mut value = arg as u64;
    let mask = (1u64 << base_log2) - 1;
    let mut out = Vec::with_capacity(64);
    loop {
        out.push(DIGITS[(value & mask) as usize]);
        value >>= base_log2;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    out
}

/// `_php_math_zvaltobase` for the values `basetozval` produces: an int, or a
/// float digit-by-digit via `fmod`. `Err(())` for an infinite float (the
/// caller throws the ValueError).
pub fn zvaltobase(v: &Value, base: u32) -> Result<Vec<u8>, ()> {
    match v {
        Value::Int(i) => Ok(longtobase(*i, base)),
        Value::Float(d) => {
            let mut fvalue = d.floor(); // floor it just in case
            if fvalue.is_infinite() {
                return Err(());
            }
            let mut out: Vec<u8> = Vec::new();
            let b = base as f64;
            loop {
                out.push(DIGITS[(fvalue % b) as i32 as usize]);
                fvalue /= b;
                if out.len() >= DOUBLE_MAX_LENGTH - 1 || fvalue.abs() < 1.0 {
                    break;
                }
            }
            out.reverse();
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

/// A byte string as a `PhpStr` value.
pub fn bytes_value(bytes: Vec<u8>) -> Value {
    Value::String(PhpStr::from_vec(bytes))
}
