//! ext/standard/scanf.c — the scanning engine behind `sscanf` (and `fscanf`).
//!
//! Like php-src, the input and the format are treated as C strings: both end
//! at their first NUL byte.

use php_runtime::value::numeric::{c_str, strtod_value};
use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

#[cfg(test)]
mod tests;

// Flag values used internally by [f|s]canf.
const SCAN_NOSKIP: u32 = 0x1; // Don't skip blanks.
const SCAN_SUPPRESS: u32 = 0x2; // Suppress assignment.
const SCAN_UNSIGNED: u32 = 0x4; // Read an unsigned value.
const SCAN_WIDTH: u32 = 0x8; // A width value was supplied.
const SCAN_SIGNOK: u32 = 0x10; // A +/- character is allowed.
const SCAN_NODIGITS: u32 = 0x20; // No digits have been scanned.
const SCAN_NOZERO: u32 = 0x40; // No zero digits have been scanned.
const SCAN_XOK: u32 = 0x80; // An 'x' is allowed.
const SCAN_PTOK: u32 = 0x100; // Decimal point is allowed.
const SCAN_EXPOK: u32 = 0x200; // An exponent is allowed.

/// `SCAN_MAX_ARGS`: the highest `%n$` index accepted when no variables are passed.
pub const SCAN_MAX_ARGS: i32 = 0xFF;
/// `SCAN_ERROR_EOF`: returned (in variable mode) when the input ended before
/// the first conversion.
pub const SCAN_ERROR_EOF: i64 = -1;

/// The size of the C code's temporary number buffer (`char buf[64]`).
const NUM_BUF_SIZE: usize = 64;

/// C `isspace` in the "C" locale.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `ZEND_STRTOUL(s + pos, &end, 10)` where `s[pos]` is a digit: the value
/// (saturated at `u64::MAX`) and the index after the digits.
fn strtoul_digits(s: &[u8], pos: usize) -> (u64, usize) {
    let mut value: u64 = 0;
    let mut i = pos;
    while let Some(&c) = s.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(c - b'0')))
            .unwrap_or(u64::MAX);
        i += 1;
    }
    (value, i)
}

/// `strtol(buf, NULL, base)` / `strtoul(buf, NULL, base)` on a buffer
/// collected by the integer scanner (an optional sign, an optional `0x`
/// prefix for base 16 and digits), with C's overflow saturation and
/// `strtoul`'s negation of negative input.
fn c_strtol(buf: &[u8], base: u32, unsigned: bool) -> i64 {
    // The scanner always settles the base before a digit is collected.
    let base = if (2..=36).contains(&base) { base } else { 10 };
    let mut i = 0;
    let neg = match buf.first() {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    if base == 16
        && matches!(buf.get(i..i + 2), Some(b"0x" | b"0X"))
        && buf.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
    {
        i += 2;
    }
    let mut magnitude: u64 = 0;
    let mut overflow = false;
    for &c in &buf[i..] {
        let digit = match (c as char).to_digit(base) {
            Some(d) => u64::from(d),
            None => break,
        };
        match magnitude
            .checked_mul(u64::from(base))
            .and_then(|m| m.checked_add(digit))
        {
            Some(m) => magnitude = m,
            None => overflow = true,
        }
    }
    if unsigned {
        if overflow {
            u64::MAX as i64
        } else if neg {
            magnitude.wrapping_neg() as i64
        } else {
            magnitude as i64
        }
    } else if neg {
        if overflow || magnitude > i64::MIN.unsigned_abs() {
            i64::MIN
        } else {
            (magnitude as i64).wrapping_neg()
        }
    } else if overflow || magnitude > i64::MAX as u64 {
        i64::MAX
    } else {
        magnitude as i64
    }
}

/// A `%[...]` character set (`CharSet`).
#[derive(Default, Debug)]
struct CharSet {
    exclude: bool,
    chars: Vec<u8>,
    /// Ranges compare as (signed) C `char`s, as on x86-64 builds of php-src.
    ranges: Vec<(i8, i8)>,
}

impl CharSet {
    /// `CharInSet`.
    fn contains(&self, c: u8) -> bool {
        let ch = c as i8;
        let found = self.chars.contains(&c)
            || self
                .ranges
                .iter()
                .any(|&(start, end)| start <= ch && ch <= end);
        found != self.exclude
    }
}

/// `BuildCharSet`: parses the set whose text starts at `pos` (just after the
/// `[`); returns it and the position after the closing `]`. The format must
/// have passed [`validate_format`].
fn build_char_set(format: &[u8], pos: usize) -> (CharSet, usize) {
    let at = |i: usize| format.get(i).copied().unwrap_or(0);
    let mut cset = CharSet::default();
    let mut f = pos;
    if at(f) == b'^' {
        cset.exclude = true;
        f += 1;
    }
    let mut ch = f;
    f += 1;
    let mut start = at(ch);
    if at(ch) == b']' || at(ch) == b'-' {
        cset.chars.push(at(ch));
        ch = f;
        f += 1;
    }
    while at(ch) != b']' && ch < format.len() {
        if at(f) == b'-' {
            // This may be the first character of a range, so don't add it yet.
            start = at(ch);
        } else if at(ch) == b'-' {
            if at(f) == b']' {
                // A trailing dash is not a range: add the previous character
                // as well as the dash.
                cset.chars.push(start);
                cset.chars.push(b'-');
            } else {
                ch = f;
                f += 1;
                let (s, e) = (start as i8, at(ch) as i8);
                cset.ranges.push(if s < e { (s, e) } else { (e, s) });
            }
        } else {
            cset.chars.push(at(ch));
        }
        ch = f;
        f += 1;
    }
    (cset, f)
}

/// `ValidateFormat`: checks that `format` is well formed and assigns each of
/// `num_vars` variables exactly once (any number of slots when `num_vars` is
/// 0). Returns the number of result slots (`totalSubs`); errors are thrown
/// as ValueErrors with php-src's messages.
pub fn validate_format(vm: &mut Vm, format: &[u8], num_vars: usize) -> PResult<usize> {
    let format = c_str(format);
    let at = |i: usize| format.get(i).copied().unwrap_or(0);
    // How many times each variable is assigned by the format.
    let mut nassign: Vec<u32> = vec![0; num_vars];
    let mut xpg_size: usize = 0;
    let mut obj_index: usize = 0;
    let mut got_xpg = false;
    let mut got_sequential = false;
    let bad_index = |vm: &mut Vm, got_xpg: bool| {
        Err(vm.throw_value_error(if got_xpg {
            "\"%n$\" argument index out of range"
        } else {
            "Different numbers of variable names and field specifiers"
        }))
    };
    let mixed = |vm: &mut Vm| {
        Err(vm.throw_value_error("cannot mix \"%\" and \"%n$\" conversion specifiers"))
    };

    let mut i = 0usize;
    while at(i) != 0 {
        let mut ch = i;
        i += 1;
        let mut flags = 0u32;
        if at(ch) != b'%' {
            continue;
        }
        ch = i;
        i += 1;
        if at(ch) == b'%' {
            continue;
        }

        'xpg_check: {
            if at(ch) == b'*' {
                flags |= SCAN_SUPPRESS;
                ch = i;
                i += 1;
                break 'xpg_check;
            }
            if at(ch).is_ascii_digit() {
                // An XPG3-style %n$ specification; XPG3 and sequential
                // specifiers must not be mixed.
                let (value, end) = strtoul_digits(format, ch);
                if at(end) == b'$' {
                    i = end + 1;
                    ch = i;
                    i += 1;
                    got_xpg = true;
                    if got_sequential {
                        return mixed(vm);
                    }
                    // C stores the strtoul() result in an `int`.
                    let value = value as i32;
                    if value < 1 || (num_vars > 0 && value as usize > num_vars) {
                        return bad_index(vm, got_xpg);
                    } else if num_vars == 0 {
                        if value > SCAN_MAX_ARGS {
                            return bad_index(vm, got_xpg);
                        }
                        xpg_size = xpg_size.max(value as usize);
                    }
                    obj_index = value as usize - 1;
                    break 'xpg_check;
                }
            }
            got_sequential = true;
            if got_xpg {
                return mixed(vm);
            }
        }

        // Width.
        if at(ch).is_ascii_digit() {
            let (_, end) = strtoul_digits(format, ch);
            i = end;
            flags |= SCAN_WIDTH;
            ch = i;
            i += 1;
        }
        // Size specifiers are ignored.
        if matches!(at(ch), b'l' | b'L' | b'h') {
            ch = i;
            i += 1;
        }

        if flags & SCAN_SUPPRESS == 0 && num_vars > 0 && obj_index >= num_vars {
            return bad_index(vm, got_xpg);
        }

        match at(ch) {
            b'n' | b'd' | b'D' | b'i' | b'o' | b'x' | b'X' | b'u' | b'f' | b'e' | b'E' | b'g'
            | b's' | b'c' => {}
            b'[' => {
                let unmatched =
                    |vm: &mut Vm| Err(vm.throw_value_error("Unmatched [ in format string"));
                if at(i) == 0 {
                    return unmatched(vm);
                }
                ch = i;
                i += 1;
                if at(ch) == b'^' {
                    if at(i) == 0 {
                        return unmatched(vm);
                    }
                    ch = i;
                    i += 1;
                }
                if at(ch) == b']' {
                    if at(i) == 0 {
                        return unmatched(vm);
                    }
                    ch = i;
                    i += 1;
                }
                while at(ch) != b']' {
                    if at(i) == 0 {
                        return unmatched(vm);
                    }
                    ch = i;
                    i += 1;
                }
            }
            c => {
                let mut msg = b"Bad scan conversion character \"".to_vec();
                msg.push(c);
                msg.push(b'"');
                return Err(vm.throw_value_error(PhpStr::from_vec(msg)));
            }
        }

        if flags & SCAN_SUPPRESS == 0 {
            if obj_index >= nassign.len() {
                nassign.resize(obj_index + 1, 0);
            }
            nassign[obj_index] += 1;
            obj_index += 1;
        }
    }

    // Verify that all of the variables were assigned exactly once.
    let total = match num_vars {
        0 if xpg_size > 0 => xpg_size,
        0 => obj_index,
        n => n,
    };
    for k in 0..total {
        let n = nassign.get(k).copied().unwrap_or(0);
        if n > 1 {
            return Err(vm.throw_value_error(
                "Variable is assigned by multiple \"%n$\" conversion specifiers",
            ));
        } else if xpg_size == 0 && n == 0 {
            // Without XPG specifiers (or with variables passed), an
            // unassigned slot means too many variables were given.
            return Err(
                vm.throw_value_error("Variable is not assigned by any conversion specifiers")
            );
        }
    }
    Ok(total)
}

/// Where scanned values go: the by-reference variables, or the result array.
enum Sink<'a> {
    Vars(&'a [Value]),
    Array(PhpArray),
}

impl Sink<'_> {
    /// `ZEND_TRY_ASSIGN_REF_*(args[index])` / `add_index_*(return_value, index)`.
    fn assign(&mut self, index: usize, value: Value) {
        match self {
            Sink::Vars(vars) => {
                if let Some(Value::Ref(r)) = vars.get(index) {
                    r.set(value);
                }
            }
            Sink::Array(a) => {
                a.insert(ArrayKey::Int(index as i64), value);
            }
        }
    }
}

/// `php_sscanf_internal`: scans `string` according to `format`. With no
/// `vars` the result is an array with one element per conversion (`null` for
/// those not reached) or `null` when the input ended before the first
/// conversion; otherwise the scanned values are assigned to the references
/// in `vars` and the result is the number of conversions, or
/// [`SCAN_ERROR_EOF`]. Format errors throw ValueErrors.
pub fn php_sscanf_internal(
    vm: &mut Vm,
    string: &[u8],
    format: &[u8],
    vars: &[Value],
) -> PResult<Value> {
    let string = c_str(string);
    let format = c_str(format);
    let num_vars = vars.len();
    let total_vars = validate_format(vm, format, num_vars)?;

    let mut sink = if num_vars == 0 {
        let mut a = PhpArray::new();
        for k in 0..total_vars {
            a.insert(ArrayKey::Int(k as i64), Value::Null);
        }
        Sink::Array(a)
    } else {
        Sink::Vars(vars)
    };

    let s_at = |i: usize| string.get(i).copied().unwrap_or(0);
    let f_at = |i: usize| format.get(i).copied().unwrap_or(0);
    let mut sp = 0usize; // position in `string`
    let mut fp = 0usize; // position in `format`
    let mut obj_index = 0usize;
    let mut nconversions: i64 = 0;
    let mut underflow = false;
    // Like the C locals, `op` and `base` keep their values across specifiers.
    let mut op = 0u8;
    let mut base = 0u32;

    // Fill in the results until the end of the input or of the format, or
    // until a mismatch.
    'scan: while f_at(fp) != 0 {
        let mut ch = fp;
        fp += 1;
        let mut flags = 0u32;

        // Whitespace in the format skips whitespace in the input.
        if is_space(f_at(ch)) {
            while is_space(s_at(sp)) {
                sp += 1;
            }
            continue;
        }

        let literal = if f_at(ch) != b'%' {
            true
        } else {
            ch = fp;
            fp += 1;
            f_at(ch) == b'%'
        };
        if literal {
            if s_at(sp) == 0 {
                underflow = true;
                break 'scan;
            }
            let sch = s_at(sp);
            sp += 1;
            if f_at(ch) != sch {
                break 'scan;
            }
            continue;
        }

        // Assignment suppression ('*') or an XPG3-style assignment ('%n$').
        if f_at(ch) == b'*' {
            flags |= SCAN_SUPPRESS;
            ch = fp;
            fp += 1;
        } else if f_at(ch).is_ascii_digit() {
            let (value, end) = strtoul_digits(format, ch);
            if f_at(end) == b'$' {
                fp = end + 1;
                ch = fp;
                fp += 1;
                // `objIndex = varStart + value - 1` in `int` arithmetic;
                // validate_format() bounded it.
                obj_index = (value as i32).saturating_sub(1).max(0) as usize;
            }
        }

        // Width.
        let mut width = if f_at(ch).is_ascii_digit() {
            let (w, end) = strtoul_digits(format, ch);
            fp = end;
            ch = fp;
            fp += 1;
            w as usize
        } else {
            0
        };

        // Size specifiers are ignored.
        if matches!(f_at(ch), b'l' | b'L' | b'h') {
            ch = fp;
            fp += 1;
        }

        match f_at(ch) {
            b'n' => {
                if flags & SCAN_SUPPRESS == 0 {
                    sink.assign(obj_index, Value::Int(sp as i64));
                    obj_index += 1;
                }
                nconversions += 1;
                continue;
            }
            b'd' | b'D' => {
                op = b'i';
                base = 10;
            }
            b'i' => {
                op = b'i';
                base = 0;
            }
            b'o' => {
                op = b'i';
                base = 8;
            }
            b'x' | b'X' => {
                op = b'i';
                base = 16;
            }
            b'u' => {
                op = b'i';
                base = 10;
                flags |= SCAN_UNSIGNED;
            }
            b'f' | b'e' | b'E' | b'g' => op = b'f',
            b's' => op = b's',
            b'c' => {
                // Unlike Tcl, a width is allowed; but like %s the scan stops
                // at whitespace.
                op = b's';
                flags |= SCAN_NOSKIP;
                if width == 0 {
                    width = 1;
                }
            }
            b'[' => {
                op = b'[';
                flags |= SCAN_NOSKIP;
            }
            _ => {}
        }

        // More input is needed from here on.
        if s_at(sp) == 0 {
            underflow = true;
            break 'scan;
        }

        // Skip leading whitespace unless the conversion suppresses that.
        if flags & SCAN_NOSKIP == 0 {
            while s_at(sp) != 0 && is_space(s_at(sp)) {
                sp += 1;
            }
            if s_at(sp) == 0 {
                underflow = true;
                break 'scan;
            }
        }

        match op {
            b's' => {
                // A string up to `width` characters or whitespace.
                if width == 0 {
                    width = usize::MAX;
                }
                let mut end = sp;
                while s_at(end) != 0 {
                    if is_space(s_at(end)) {
                        break;
                    }
                    end += 1;
                    width -= 1;
                    if width == 0 {
                        break;
                    }
                }
                if flags & SCAN_SUPPRESS == 0 {
                    sink.assign(obj_index, Value::bytes(&string[sp..end]));
                    obj_index += 1;
                }
                sp = end;
            }
            b'[' => {
                if width == 0 {
                    width = usize::MAX;
                }
                let (cset, next) = build_char_set(format, fp);
                fp = next;
                let mut end = sp;
                while s_at(end) != 0 {
                    if !cset.contains(s_at(end)) {
                        break;
                    }
                    end += 1;
                    width -= 1;
                    if width == 0 {
                        break;
                    }
                }
                if sp == end {
                    // Nothing matched the set: stop processing.
                    break 'scan;
                }
                if flags & SCAN_SUPPRESS == 0 {
                    sink.assign(obj_index, Value::bytes(&string[sp..end]));
                    obj_index += 1;
                }
                sp = end;
            }
            b'i' => {
                // A signed or unsigned integer.
                let mut buf: Vec<u8> = Vec::with_capacity(NUM_BUF_SIZE);
                if width == 0 || width > NUM_BUF_SIZE - 1 {
                    width = NUM_BUF_SIZE - 1;
                }
                flags |= SCAN_SIGNOK | SCAN_NODIGITS | SCAN_NOZERO;
                while width > 0 {
                    let c = s_at(sp);
                    let accept = match c {
                        // A leading 0 means base 8, or base 16 with an 'x'.
                        b'0' => {
                            if base == 16 {
                                flags |= SCAN_XOK;
                            }
                            if base == 0 {
                                base = 8;
                                flags |= SCAN_XOK;
                            }
                            if flags & SCAN_NOZERO != 0 {
                                flags &= !(SCAN_SIGNOK | SCAN_NODIGITS | SCAN_NOZERO);
                            } else {
                                flags &= !(SCAN_SIGNOK | SCAN_XOK | SCAN_NODIGITS);
                            }
                            true
                        }
                        b'1'..=b'7' => {
                            if base == 0 {
                                base = 10;
                            }
                            flags &= !(SCAN_SIGNOK | SCAN_XOK | SCAN_NODIGITS);
                            true
                        }
                        b'8' | b'9' => {
                            if base == 0 {
                                base = 10;
                            }
                            if base <= 8 {
                                false
                            } else {
                                flags &= !(SCAN_SIGNOK | SCAN_XOK | SCAN_NODIGITS);
                                true
                            }
                        }
                        b'A'..=b'F' | b'a'..=b'f' if base > 10 => {
                            flags &= !(SCAN_SIGNOK | SCAN_XOK | SCAN_NODIGITS);
                            true
                        }
                        b'+' | b'-' if flags & SCAN_SIGNOK != 0 => {
                            flags &= !SCAN_SIGNOK;
                            true
                        }
                        b'x' | b'X' if flags & SCAN_XOK != 0 && buf.len() == 1 => {
                            base = 16;
                            flags &= !SCAN_XOK;
                            true
                        }
                        // An illegal character ends the number.
                        _ => false,
                    };
                    if !accept {
                        break;
                    }
                    buf.push(c);
                    sp += 1;
                    if s_at(sp) == 0 {
                        break;
                    }
                    width -= 1;
                }

                // Back up if we only got a sign, or a trailing x after a 0.
                if flags & SCAN_NODIGITS != 0 {
                    if s_at(sp) == 0 {
                        underflow = true;
                    }
                    break 'scan;
                } else if matches!(buf.last(), Some(b'x' | b'X')) {
                    buf.pop();
                    sp -= 1;
                }

                if flags & SCAN_SUPPRESS == 0 {
                    let unsigned = flags & SCAN_UNSIGNED != 0;
                    let value = c_strtol(&buf, base, unsigned);
                    // PHP has no unsigned ints: large unsigned values become
                    // decimal strings.
                    let v = if unsigned && value < 0 {
                        Value::from((value as u64).to_string())
                    } else {
                        Value::Int(value)
                    };
                    sink.assign(obj_index, v);
                    obj_index += 1;
                }
            }
            b'f' => {
                // A floating point number.
                let mut buf: Vec<u8> = Vec::with_capacity(NUM_BUF_SIZE);
                if width == 0 || width > NUM_BUF_SIZE - 1 {
                    width = NUM_BUF_SIZE - 1;
                }
                flags |= SCAN_SIGNOK | SCAN_NODIGITS | SCAN_PTOK | SCAN_EXPOK;
                while width > 0 {
                    let c = s_at(sp);
                    let accept = match c {
                        b'0'..=b'9' => {
                            flags &= !(SCAN_SIGNOK | SCAN_NODIGITS);
                            true
                        }
                        b'+' | b'-' if flags & SCAN_SIGNOK != 0 => {
                            flags &= !SCAN_SIGNOK;
                            true
                        }
                        b'.' if flags & SCAN_PTOK != 0 => {
                            flags &= !(SCAN_SIGNOK | SCAN_PTOK);
                            true
                        }
                        // An exponent is not allowed until there has been at
                        // least one digit.
                        b'e' | b'E' if flags & (SCAN_NODIGITS | SCAN_EXPOK) == SCAN_EXPOK => {
                            flags =
                                (flags & !(SCAN_EXPOK | SCAN_PTOK)) | SCAN_SIGNOK | SCAN_NODIGITS;
                            true
                        }
                        // An illegal character ends the number.
                        _ => false,
                    };
                    if !accept {
                        break;
                    }
                    buf.push(c);
                    sp += 1;
                    if s_at(sp) == 0 {
                        break;
                    }
                    width -= 1;
                }

                // Back up over a trailing 'e' or 'e' plus sign.
                if flags & SCAN_NODIGITS != 0 {
                    if flags & SCAN_EXPOK != 0 {
                        // No digits at all: the scan failed.
                        if s_at(sp) == 0 {
                            underflow = true;
                        }
                        break 'scan;
                    }
                    let last = buf.pop();
                    sp -= 1;
                    if !matches!(last, Some(b'e' | b'E')) {
                        buf.pop();
                        sp -= 1;
                    }
                }

                if flags & SCAN_SUPPRESS == 0 {
                    sink.assign(obj_index, Value::Float(strtod_value(&buf)));
                    obj_index += 1;
                }
            }
            _ => {}
        }
        nconversions += 1;
    }

    if underflow && nconversions == 0 {
        // `scan_set_error_return`
        return Ok(if num_vars > 0 {
            Value::Int(SCAN_ERROR_EOF)
        } else {
            Value::Null
        });
    }
    Ok(match sink {
        Sink::Vars(_) => Value::Int(nconversions),
        Sink::Array(a) => Value::array(a),
    })
}

fn sscanf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sscanf_internal(vm, args.str(0), args.str(1), args.rest(2))
}

pub static SSCANF: NativeFunction = NativeFunction {
    sig: "sscanf(string $string, string $format, mixed &...$vars): array|int|null",
    handler: sscanf,
};

pub fn register(r: &mut Registry) {
    r.function(&SSCANF);
}
