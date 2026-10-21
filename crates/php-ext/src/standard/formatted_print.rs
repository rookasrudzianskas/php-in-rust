//! ext/standard/formatted_print.c — the sprintf family, plus the
//! `php_conv_fp` helper of main/snprintf.c it relies on.

use php_runtime::value::float_fmt;
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use crate::standard::streams;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Align {
    Left,
    Right,
}

const ADJ_WIDTH: u8 = 1;
const ADJ_PRECISION: u8 = 2;
const FLOAT_PRECISION: i32 = 6;
const MAX_FLOAT_PRECISION: i32 = 53;
const INT_MAX: i64 = i32::MAX as i64;
/// `NDIG` of main/snprintf.c: the precision limit of `php_conv_fp`.
const NDIG: i32 = 320;

/// Where the arguments of a format come from. Selects the error raised when
/// the format references more arguments than were given
/// (`nb_additional_parameters` of `php_formatted_print`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormatArgs {
    /// Variadic arguments following `n` fixed parameters (1 for `sprintf`,
    /// 2 for `fprintf`): ArgumentCountError "N arguments are required, M given".
    Variadic(usize),
    /// An argument array (`vsprintf` family): ValueError
    /// "The arguments array must contain N items, M given".
    Array,
}

/// `php_formatted_print`: format `args` according to `format` exactly like
/// `sprintf`. `format_arg_num` is the 1-based position of the format
/// argument in the calling PHP function (1 for sprintf, 2 for fprintf), used
/// in error messages. The string-functions work package owns this module and
/// must keep this signature.
pub fn php_formatted_print(
    vm: &mut Vm,
    format: &PhpStr,
    args: &[Value],
    format_arg_num: usize,
) -> PResult<Vec<u8>> {
    formatted_print(vm, format, args, FormatArgs::Variadic(format_arg_num))
}

/// `php_formatted_print` with an explicit argument source; use
/// [`FormatArgs::Array`] for `vsprintf`-style callers.
pub fn formatted_print(
    vm: &mut Vm,
    format: &[u8],
    args: &[Value],
    source: FormatArgs,
) -> PResult<Vec<u8>> {
    let len = format.len();
    // The C code reads the NUL terminator past the end of the format.
    let at = |i: usize| format.get(i).copied().unwrap_or(0);
    let argc = args.len();
    let mut out = Vec::with_capacity(240);
    let mut currarg = 0usize;
    let mut max_missing: Option<usize> = None;
    let mut pos = 0usize;

    while pos < len {
        let rest = &format[pos..];
        match rest.iter().position(|&c| c == b'%') {
            None => {
                out.extend_from_slice(rest);
                break;
            }
            Some(off) => {
                out.extend_from_slice(&rest[..off]);
                pos += off;
            }
        }
        pos += 1; // skip the '%'

        if at(pos) == b'%' {
            out.push(b'%');
            pos += 1;
            continue;
        }

        // Starting a new format specifier.
        let mut alignment = Align::Right;
        let mut adjusting = 0u8;
        let mut padding = b' ';
        let mut always_sign = false;
        let mut expprec = false;
        let mut width = 0usize;
        let mut precision = 0i32;
        let argnum;

        if at(pos).is_ascii_alphabetic() {
            argnum = None;
        } else {
            argnum = get_argnum(vm, format, &mut pos)?;

            // Modifiers.
            loop {
                match at(pos) {
                    c @ (b' ' | b'0') => padding = c,
                    b'-' => alignment = Align::Left,
                    b'+' => always_sign = true,
                    b'\'' => {
                        if len - pos > 1 {
                            pos += 1;
                            padding = at(pos);
                        } else {
                            return Err(vm.throw_value_error("Missing padding character"));
                        }
                    }
                    _ => break,
                }
                pos += 1;
            }

            // Width.
            if at(pos) == b'*' {
                pos += 1;
                let width_argnum = match get_argnum(vm, format, &mut pos)? {
                    Some(n) => n,
                    None => next_arg(&mut currarg),
                };
                if width_argnum >= argc {
                    // Like C, resume scanning right here: the rest of the
                    // format is still parsed (and may still throw) before the
                    // missing argument is reported.
                    max_missing = max_missing.max(Some(width_argnum));
                    continue;
                }
                let w = match &*args[width_argnum].deref() {
                    Value::Int(w) => *w,
                    _ => return Err(vm.throw_value_error("Width must be an integer")),
                };
                if !(0..=INT_MAX).contains(&w) {
                    return Err(
                        vm.throw_value_error(format!("Width must be between 0 and {INT_MAX}"))
                    );
                }
                width = w as usize;
                adjusting |= ADJ_WIDTH;
            } else if at(pos).is_ascii_digit() {
                match get_number(format, &mut pos) {
                    Some(w) => width = w,
                    None => {
                        return Err(
                            vm.throw_value_error(format!("Width must be between 0 and {INT_MAX}"))
                        );
                    }
                }
                adjusting |= ADJ_WIDTH;
            }

            // Precision.
            if at(pos) == b'.' {
                pos += 1;
                if at(pos) == b'*' {
                    pos += 1;
                    let prec_argnum = match get_argnum(vm, format, &mut pos)? {
                        Some(n) => n,
                        None => next_arg(&mut currarg),
                    };
                    if prec_argnum >= argc {
                        max_missing = max_missing.max(Some(prec_argnum));
                        continue;
                    }
                    let p = match &*args[prec_argnum].deref() {
                        Value::Int(p) => *p,
                        _ => return Err(vm.throw_value_error("Precision must be an integer")),
                    };
                    if !(-1..=INT_MAX).contains(&p) {
                        return Err(vm.throw_value_error(format!(
                            "Precision must be between -1 and {INT_MAX}"
                        )));
                    }
                    precision = p as i32;
                    adjusting |= ADJ_PRECISION;
                    expprec = true;
                } else if at(pos).is_ascii_digit() {
                    match get_number(format, &mut pos) {
                        Some(p) => precision = p as i32,
                        None => {
                            return Err(vm.throw_value_error(format!(
                                "Precision must be between 0 and {INT_MAX}"
                            )));
                        }
                    }
                    adjusting |= ADJ_PRECISION;
                    expprec = true;
                } else {
                    adjusting |= ADJ_PRECISION;
                }
            }
        }

        if at(pos) == b'l' {
            pos += 1;
        }

        let argnum = match argnum {
            Some(n) => n,
            None => next_arg(&mut currarg),
        };
        if argnum >= argc {
            max_missing = max_missing.max(Some(argnum));
            continue;
        }

        let spec = at(pos);
        if expprec && precision == -1 && !matches!(spec, b'g' | b'G' | b'h' | b'H') {
            return Err(
                vm.throw_value_error("Precision -1 is only supported for %g, %G, %h and %H")
            );
        }

        let arg = &args[argnum];
        match spec {
            b's' => {
                let s = vm.to_php_string(arg)?;
                append_string(
                    vm,
                    &mut out,
                    &s,
                    width,
                    precision.max(0) as usize,
                    padding,
                    alignment,
                    false,
                    expprec,
                    false,
                )?;
            }
            b'd' => {
                let n = vm.to_int(arg)?;
                append_int(vm, &mut out, n, width, padding, alignment, always_sign)?;
            }
            b'u' => {
                let n = vm.to_int(arg)?;
                append_uint(vm, &mut out, n as u64, width, padding, alignment)?;
            }
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'h' | b'H' => {
                let d = vm.to_float(arg)?;
                append_double(
                    vm,
                    &mut out,
                    d,
                    width,
                    padding,
                    alignment,
                    precision,
                    adjusting,
                    spec,
                    always_sign,
                )?;
            }
            b'c' => {
                // Truncated to a byte; width and padding are ignored.
                let n = vm.to_int(arg)?;
                out.push(n as u8);
            }
            b'o' | b'x' | b'X' | b'b' => {
                let n = vm.to_int(arg)?;
                let nbits = match spec {
                    b'o' => 3,
                    b'b' => 1,
                    _ => 4,
                };
                append_2n(
                    vm,
                    &mut out,
                    n,
                    width,
                    padding,
                    alignment,
                    nbits,
                    spec == b'X',
                    expprec,
                )?;
            }
            b'%' => out.push(b'%'),
            0 if pos >= len => {
                return Err(vm.throw_value_error("Missing format specifier at end of string"));
            }
            c => {
                let mut msg = b"Unknown format specifier \"".to_vec();
                msg.push(c);
                msg.push(b'"');
                return Err(vm.throw_value_error(PhpStr::from_vec(msg)));
            }
        }
        pos += 1;
    }

    if let Some(missing) = max_missing {
        return Err(match source {
            FormatArgs::Array => vm.throw_value_error(format!(
                "The arguments array must contain {} items, {argc} given",
                missing + 1
            )),
            FormatArgs::Variadic(nb) => {
                // C computes these in `int`.
                let required = (missing as i64 + nb as i64 + 1) as i32;
                let given = (argc as i64 + nb as i64) as i32;
                vm.argument_count_error(format!("{required} arguments are required, {given} given"))
            }
        });
    }
    Ok(out)
}

/// `currarg++`.
fn next_arg(currarg: &mut usize) -> usize {
    let n = *currarg;
    *currarg += 1;
    n
}

/// `php_sprintf_getnumber`: parses the decimal digits at `pos` (with
/// `strtol` saturation) and advances past them. `None` when the value is not
/// below `INT_MAX`.
fn get_number(format: &[u8], pos: &mut usize) -> Option<usize> {
    let mut num: i64 = 0;
    while let Some(&c) = format.get(*pos) {
        if !c.is_ascii_digit() {
            break;
        }
        num = num
            .checked_mul(10)
            .and_then(|n| n.checked_add(i64::from(c - b'0')))
            .unwrap_or(i64::MAX);
        *pos += 1;
    }
    if num >= INT_MAX {
        None
    } else {
        Some(num as usize)
    }
}

/// `php_sprintf_get_argnum`: an `N$` argument number at `pos` (0-based) or
/// `None` for "the next argument" (`ARG_NUM_NEXT`).
fn get_argnum(vm: &mut Vm, format: &[u8], pos: &mut usize) -> PResult<Option<usize>> {
    let tail = format.get(*pos..).unwrap_or(&[]);
    let digits = tail.iter().take_while(|c| c.is_ascii_digit()).count();
    if tail.get(digits) != Some(&b'$') {
        return Ok(None);
    }
    if digits == 0 {
        return Err(vm.throw_value_error("Argument number specifier must not be empty"));
    }
    match get_number(format, pos) {
        Some(n) if n > 0 => {
            *pos += 1; // skip the '$'
            Ok(Some(n - 1))
        }
        _ => Err(vm.throw_value_error(format!(
            "Argument number specifier must be greater than zero and less than {INT_MAX}"
        ))),
    }
}

/// `php_sprintf_appendstring`: appends `add` padded to `min_width` (and cut
/// to `max_width` when a precision was given, `expprec`). With `'0'` padding
/// and right alignment a leading sign of numbers (`neg` / `always_sign`) is
/// moved in front of the padding; like C this drops the first byte of `add`
/// even when it is not a sign (`sprintf("%+05f", NAN)` is `"+aN"`).
#[allow(clippy::too_many_arguments)]
fn append_string(
    vm: &mut Vm,
    out: &mut Vec<u8>,
    add: &[u8],
    min_width: usize,
    max_width: usize,
    padding: u8,
    alignment: Align,
    neg: bool,
    expprec: bool,
    always_sign: bool,
) -> PResult<()> {
    let mut add = add;
    let mut copy_len = if expprec {
        max_width.min(add.len())
    } else {
        add.len()
    };
    let npad = min_width.saturating_sub(copy_len);
    let m_width = min_width.max(copy_len);

    // `INT_MAX - *pos - 1` in size_t arithmetic.
    if m_width > (INT_MAX as usize).wrapping_sub(out.len()).wrapping_sub(1) {
        return Err(vm.fatal_error(format!("Field width {m_width} is too long")));
    }
    out.reserve(m_width);

    if alignment == Align::Right {
        if (neg || always_sign) && padding == b'0' {
            out.push(if neg { b'-' } else { b'+' });
            add = add.get(1..).unwrap_or(&[]);
            copy_len = copy_len.saturating_sub(1);
        }
        out.resize(out.len() + npad, padding);
    }
    out.extend_from_slice(&add[..copy_len]);
    if alignment == Align::Left {
        out.resize(out.len() + npad, padding);
    }
    Ok(())
}

/// `php_sprintf_appendint` (`%d`).
fn append_int(
    vm: &mut Vm,
    out: &mut Vec<u8>,
    number: i64,
    width: usize,
    padding: u8,
    alignment: Align,
    always_sign: bool,
) -> PResult<()> {
    let neg = number < 0;
    // Can't right-pad 0's on integers.
    let padding = if alignment == Align::Left && padding == b'0' {
        b' '
    } else {
        padding
    };
    let mut buf = Vec::with_capacity(21);
    if neg {
        buf.push(b'-');
    } else if always_sign {
        buf.push(b'+');
    }
    buf.extend_from_slice(number.unsigned_abs().to_string().as_bytes());
    append_string(
        vm,
        out,
        &buf,
        width,
        0,
        padding,
        alignment,
        neg,
        false,
        always_sign,
    )
}

/// `php_sprintf_appenduint` (`%u`).
fn append_uint(
    vm: &mut Vm,
    out: &mut Vec<u8>,
    number: u64,
    width: usize,
    padding: u8,
    alignment: Align,
) -> PResult<()> {
    let padding = if alignment == Align::Left && padding == b'0' {
        b' '
    } else {
        padding
    };
    let buf = number.to_string();
    append_string(
        vm,
        out,
        buf.as_bytes(),
        width,
        0,
        padding,
        alignment,
        false,
        false,
        false,
    )
}

/// `php_sprintf_append2n` (`%b`, `%o`, `%x`, `%X`): the two's complement bits
/// of `number` in base `2^nbits`. The C code passes a maximum width of 0, so
/// an explicit precision (`%.3x`) leaves only the padding.
#[allow(clippy::too_many_arguments)]
fn append_2n(
    vm: &mut Vm,
    out: &mut Vec<u8>,
    number: i64,
    width: usize,
    padding: u8,
    alignment: Align,
    nbits: u32,
    upper: bool,
    expprec: bool,
) -> PResult<()> {
    let chartable: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mask = (1u64 << nbits) - 1;
    let mut num = number as u64;
    let mut buf = Vec::with_capacity(64);
    loop {
        buf.push(chartable[(num & mask) as usize]);
        num >>= nbits;
        if num == 0 {
            break;
        }
    }
    buf.reverse();
    append_string(
        vm, out, &buf, width, 0, padding, alignment, false, expprec, false,
    )
}

/// `php_sprintf_appenddouble` (`%e %E %f %F %g %G %h %H`).
#[allow(clippy::too_many_arguments)]
fn append_double(
    vm: &mut Vm,
    out: &mut Vec<u8>,
    number: f64,
    width: usize,
    padding: u8,
    alignment: Align,
    precision: i32,
    adjust: u8,
    fmt: u8,
    always_sign: bool,
) -> PResult<()> {
    let mut precision = precision;
    if adjust & ADJ_PRECISION == 0 {
        precision = FLOAT_PRECISION;
    } else if precision > MAX_FLOAT_PRECISION {
        vm.notice(format!(
            "Requested precision of {precision} digits was truncated to PHP maximum of \
             {MAX_FLOAT_PRECISION} digits"
        ))?;
        precision = MAX_FLOAT_PRECISION;
    }

    // NaN and infinity ignore the requested width.
    if number.is_nan() {
        return append_string(
            vm,
            out,
            b"NaN",
            3,
            0,
            padding,
            alignment,
            false,
            false,
            always_sign,
        );
    }
    if number.is_infinite() {
        let neg = number < 0.0;
        let s: &[u8] = if neg { b"-INF" } else { b"INF" };
        return append_string(
            vm,
            out,
            s,
            s.len(),
            0,
            padding,
            alignment,
            neg,
            false,
            always_sign,
        );
    }

    let mut buf = Vec::with_capacity(32);
    let is_negative;
    match fmt {
        b'e' | b'E' | b'f' | b'F' => {
            // The locale is always "C", so `LCONV_DECIMAL_POINT` is '.'.
            let conv = if fmt == b'f' { b'F' } else { fmt };
            let (digits, neg) = php_conv_fp(conv, number, false, precision, b'.');
            is_negative = neg;
            if neg {
                buf.push(b'-');
            } else if always_sign {
                buf.push(b'+');
            }
            buf.extend_from_slice(&digits);
        }
        _ => {
            // 'g', 'G', 'h', 'H' go through zend_gcvt.
            if precision == 0 {
                precision = 1;
            }
            let exp_char = if fmt == b'G' || fmt == b'H' {
                b'E'
            } else {
                b'e'
            };
            let s = float_fmt::gcvt(number, precision, exp_char);
            is_negative = s.starts_with('-');
            if !is_negative && always_sign {
                buf.push(b'+');
            }
            buf.extend_from_slice(s.as_bytes());
        }
    }

    append_string(
        vm,
        out,
        &buf,
        width,
        0,
        padding,
        alignment,
        is_negative,
        false,
        always_sign,
    )
}

/// `php_conv_fp` (main/snprintf.c): formats `num` in the `'F'` (fixed), `'e'`
/// or `'E'` style with `precision` digits after the decimal point, without a
/// sign. Returns the text and whether `num` is negative (never for zero, so
/// `-0.0` prints as `0.000000`). The exponent has no leading zeros
/// (`1.0e+1`, `0.0e+0`); `add_dp` forces a decimal point at precision 0.
/// Infinity and NaN render as `"INF"` / `"NAN"` (not negative).
pub fn php_conv_fp(
    format: u8,
    num: f64,
    add_dp: bool,
    precision: i32,
    dec_point: u8,
) -> (Vec<u8>, bool) {
    let precision = precision.clamp(0, NDIG - 2) as usize;
    if !num.is_finite() {
        let s: &[u8] = if num.is_nan() { b"NAN" } else { b"INF" };
        return (s.to_vec(), false);
    }
    // zend_dtoa (modes 2 and 3) rounds the exact binary value half-to-even,
    // as Rust's exact float formatting does, so the digits are identical.
    let is_negative = num != 0.0 && num.is_sign_negative();
    let a = num.abs();
    let mut out = Vec::with_capacity(precision + 24);
    if format == b'F' {
        let s = format!("{a:.precision$}");
        out.extend(s.bytes().map(|b| if b == b'.' { dec_point } else { b }));
        if precision == 0 && add_dp && num != 0.0 {
            out.push(dec_point);
        }
    } else {
        let s = format!("{a:.precision$e}");
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        out.extend_from_slice(mantissa.as_bytes());
        if precision == 0 && add_dp {
            out.push(b'.');
        }
        out.push(format);
        let exp: i32 = exp.parse().unwrap_or(0);
        if exp == 0 {
            out.extend_from_slice(b"+0");
        } else {
            out.push(if exp < 0 { b'-' } else { b'+' });
            out.extend_from_slice(exp.unsigned_abs().to_string().as_bytes());
        }
    }
    (out, is_negative)
}

/// The `PHP_Z_PARAM_STREAM` part of argument parsing, which php-src performs
/// before formatting: a non-resource is a TypeError, and so is a closed
/// resource. Open resources of other types are rejected by
/// [`streams::write_resource`].
fn check_stream_arg(vm: &mut Vm, stream: &Value) -> PResult<()> {
    match stream {
        Value::Resource(r) => {
            if r.is_closed() {
                let fname = vm.current_function_name();
                return Err(vm.throw_type_error(format!(
                    "{fname}(): supplied resource is not a valid stream resource"
                )));
            }
            Ok(())
        }
        other => Err(vm.argument_type_error(
            1,
            &format!("must be of type resource, {} given", other.value_name()),
        )),
    }
}

/// `php_formatted_print_get_array`: the array's values in order.
fn array_args(args: &Args, i: usize) -> Vec<Value> {
    args.array(i).iter().map(|(_, v)| v.clone()).collect()
}

fn sprintf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let out = formatted_print(vm, args.str(0), args.rest(1), FormatArgs::Variadic(1))?;
    Ok(Value::String(PhpStr::from_vec(out)))
}

fn vsprintf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let values = array_args(args, 1);
    let out = formatted_print(vm, args.str(0), &values, FormatArgs::Array)?;
    Ok(Value::String(PhpStr::from_vec(out)))
}

fn printf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let out = formatted_print(vm, args.str(0), args.rest(1), FormatArgs::Variadic(1))?;
    vm.echo(&out)?;
    Ok(Value::Int(out.len() as i64))
}

fn vprintf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let values = array_args(args, 1);
    let out = formatted_print(vm, args.str(0), &values, FormatArgs::Array)?;
    vm.echo(&out)?;
    Ok(Value::Int(out.len() as i64))
}

fn fprintf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let stream = args.get(0).clone();
    check_stream_arg(vm, &stream)?;
    let out = formatted_print(vm, args.str(1), args.rest(2), FormatArgs::Variadic(2))?;
    // php-src ignores the result of php_stream_write and always returns the
    // length of the formatted string.
    streams::write_resource(vm, &stream, &out)?;
    Ok(Value::Int(out.len() as i64))
}

fn vfprintf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let stream = args.get(0).clone();
    check_stream_arg(vm, &stream)?;
    let values = array_args(args, 2);
    let out = formatted_print(vm, args.str(1), &values, FormatArgs::Array)?;
    streams::write_resource(vm, &stream, &out)?;
    Ok(Value::Int(out.len() as i64))
}

pub static SPRINTF: NativeFunction = NativeFunction {
    sig: "sprintf(string $format, mixed ...$values): string",
    handler: sprintf,
};
pub static PRINTF: NativeFunction = NativeFunction {
    sig: "printf(string $format, mixed ...$values): int",
    handler: printf,
};
pub static VPRINTF: NativeFunction = NativeFunction {
    sig: "vprintf(string $format, array $values): int",
    handler: vprintf,
};
pub static VSPRINTF: NativeFunction = NativeFunction {
    sig: "vsprintf(string $format, array $values): string",
    handler: vsprintf,
};
pub static FPRINTF: NativeFunction = NativeFunction {
    sig: "fprintf($stream, string $format, mixed ...$values): int",
    handler: fprintf,
};
pub static VFPRINTF: NativeFunction = NativeFunction {
    sig: "vfprintf($stream, string $format, array $values): int",
    handler: vfprintf,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&SPRINTF, &PRINTF, &VPRINTF, &VSPRINTF, &FPRINTF, &VFPRINTF]);
}
