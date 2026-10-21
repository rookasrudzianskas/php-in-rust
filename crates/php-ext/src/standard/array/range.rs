//! `range()` (PHP 8.3+ semantics).

use php_runtime::value::numeric::{
    Numeric, dval_to_lval_silent, is_long_compatible, is_numeric_str,
};
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Value, Vm};

use super::util::HT_MAX_SIZE;

// `php_range_process_input` result: ordered like `IS_LONG < IS_DOUBLE <
// IS_STRING < IS_ARRAY` (the last one stands for a single-byte numeric
// string, which is ambiguous between a digit character and a number).
const T_LONG: u8 = 4;
const T_DOUBLE: u8 = 5;
const T_STRING: u8 = 6;
const T_AMBIGUOUS: u8 = 7;

struct Input {
    ty: u8,
    lval: i64,
    dval: f64,
}

/// `php_range_process_input`.
fn process_input(vm: &mut Vm, input: &Value, arg_num: usize) -> PResult<Input> {
    let arg_name = if arg_num == 1 { "start" } else { "end" };
    let check_dval = |vm: &mut Vm, d: f64, lval: i64| -> PResult<Input> {
        if d.is_infinite() {
            return Err(vm.argument_value_error(arg_num, "must be a finite number, INF provided"));
        }
        if d.is_nan() {
            return Err(vm.argument_value_error(arg_num, "must be a finite number, NAN provided"));
        }
        Ok(Input {
            ty: T_DOUBLE,
            lval,
            dval: d,
        })
    };
    match input {
        Value::Int(i) => Ok(Input {
            ty: T_LONG,
            lval: *i,
            dval: *i as f64,
        }),
        Value::Float(d) => check_dval(vm, *d, 0),
        Value::String(s) => {
            if s.is_empty() {
                vm.warning(format!(
                    "Argument #{arg_num} (${arg_name}) must not be empty, casted to 0"
                ))?;
                return Ok(Input {
                    ty: T_LONG,
                    lval: 0,
                    dval: 0.0,
                });
            }
            match is_numeric_str(s) {
                Some(Numeric::Float(d)) => return check_dval(vm, d, 0),
                Some(Numeric::Int(l)) => {
                    return Ok(Input {
                        ty: if s.len() == 1 { T_AMBIGUOUS } else { T_LONG },
                        lval: l,
                        dval: l as f64,
                    });
                }
                None => {}
            }
            if s.len() != 1 {
                vm.warning(format!(
                    "Argument #{arg_num} (${arg_name}) must be a single byte, subsequent bytes are ignored"
                ))?;
            }
            // Fall back to 0 in case the other argument is not a string.
            Ok(Input {
                ty: T_STRING,
                lval: 0,
                dval: 0.0,
            })
        }
        other => unreachable!("range() argument after ZPP: {other:?}"),
    }
}

/// `_php_math_round(value, 0, PHP_ROUND_HALF_UP)` for a positive value.
fn round_half_up(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 || v == v.trunc() {
        return v;
    }
    let integral = v.floor();
    let edge = (integral + 0.5).abs();
    if v.abs() >= edge {
        integral + 1.0
    } else {
        integral
    }
}

enum Fail {
    NegativeStep,
    Boundary,
}

fn fail(vm: &mut Vm, f: Fail) -> php_runtime::Flow {
    match f {
        Fail::NegativeStep => {
            vm.argument_value_error(3, "must be greater than 0 for increasing ranges")
        }
        Fail::Boundary => vm.argument_value_error(
            3,
            "must be less than the range spanned by argument #1 ($start) and argument #2 ($end)",
        ),
    }
}

fn single_byte(b: u8) -> Value {
    Value::String(PhpStr::from_bytes(&[b]))
}

fn range(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let user_start = args.get(0).clone();
    let user_end = args.get(1).clone();

    let mut is_step_double = false;
    let mut is_step_negative = false;
    let mut step_double: f64;
    let mut step: i64;

    match args.get(2) {
        Value::Float(d) => {
            step_double = *d;
            if step_double.is_infinite() {
                return Err(vm.argument_value_error(3, "must be a finite number, INF provided"));
            }
            if step_double.is_nan() {
                return Err(vm.argument_value_error(3, "must be a finite number, NAN provided"));
            }
            // We only want positive step values.
            if step_double < 0.0 {
                is_step_negative = true;
                step_double *= -1.0;
            }
            step = dval_to_lval_silent(step_double);
            if !is_long_compatible(step_double, step) {
                is_step_double = true;
            }
        }
        other => {
            step = match other {
                Value::Int(i) => *i,
                _ => 1,
            };
            // We only want positive step values.
            if step < 0 {
                if step == i64::MIN {
                    return Err(vm.argument_value_error(3, &format!("must be greater than {step}")));
                }
                is_step_negative = true;
                step = -step;
            }
            step_double = step as f64;
        }
    }
    if step_double == 0.0 {
        return Err(vm.argument_value_error(3, "cannot be 0"));
    }

    let mut start = process_input(vm, &user_start, 1)?;
    let mut end = process_input(vm, &user_end, 2)?;

    // If the range is given as strings, generate an array of characters.
    if start.ty >= T_STRING || end.ty >= T_STRING {
        // If one of the inputs is NOT a string nor a single-byte string.
        if start.ty < T_STRING || end.ty < T_STRING {
            if start.ty < T_STRING {
                if end.ty != T_AMBIGUOUS {
                    vm.warning("Argument #1 ($start) must be a single byte string if argument #2 ($end) is a single byte string, argument #2 ($end) converted to 0")?;
                }
                end.ty = T_LONG;
            } else if end.ty < T_STRING {
                if start.ty != T_AMBIGUOUS {
                    vm.warning("Argument #2 ($end) must be a single byte string if argument #1 ($start) is a single byte string, argument #1 ($start) converted to 0")?;
                }
                start.ty = T_LONG;
            }
        } else if is_step_double {
            // Only emit a warning if one of the inputs is not a numeric digit.
            if start.ty == T_STRING || end.ty == T_STRING {
                vm.warning("Argument #3 ($step) must be of type int when generating an array of characters, inputs converted to 0")?;
            }
            end.ty = T_LONG;
            start.ty = T_LONG;
        } else {
            return char_range(vm, &user_start, &user_end, step, is_step_negative);
        }
    }

    numeric_range(
        vm,
        &start,
        &end,
        step,
        step_double,
        is_step_double,
        is_step_negative,
    )
}

/// The character branch of `range()`: bounds are the first bytes of the inputs.
fn char_range(
    vm: &mut Vm,
    user_start: &Value,
    user_end: &Value,
    step: i64,
    is_step_negative: bool,
) -> PResult<Value> {
    let first_byte = |v: &Value| match v {
        Value::String(s) => s.first().copied().unwrap_or(0),
        _ => 0,
    };
    let mut low = first_byte(user_start) as i64;
    let high = first_byte(user_end) as i64;
    let mut out = Vec::new();
    if low > high {
        // Decreasing char range.
        if low - high < step {
            return Err(fail(vm, Fail::Boundary));
        }
        while low >= high {
            out.push(single_byte(low as u8));
            low -= step;
        }
    } else if high > low {
        // Increasing char range.
        if is_step_negative {
            return Err(fail(vm, Fail::NegativeStep));
        }
        if high - low < step {
            return Err(fail(vm, Fail::Boundary));
        }
        while low <= high {
            out.push(single_byte(low as u8));
            low += step;
        }
    } else {
        out.push(single_byte(low as u8));
    }
    Ok(Value::array(PhpArray::from_values(out)))
}

fn numeric_range(
    vm: &mut Vm,
    start: &Input,
    end: &Input,
    step: i64,
    step_double: f64,
    is_step_double: bool,
    is_step_negative: bool,
) -> PResult<Value> {
    let mut out: Vec<Value> = Vec::new();
    if start.ty == T_DOUBLE || end.ty == T_DOUBLE || is_step_double {
        let (sd, ed) = (start.dval, end.dval);
        if sd > ed {
            // Decreasing float range.
            if sd - ed < step_double {
                return Err(fail(vm, Fail::Boundary));
            }
            // RANGE_CHECK_DOUBLE_INIT_ARRAY(start_double, end_double, step):
            // the message prints the macro's `end` and `start` in that order.
            let size = double_range_size(vm, sd, ed, step_double, ed, sd)?;
            let mut i = 0u64;
            let mut element = sd;
            while i < size && element >= ed {
                out.push(Value::Float(element));
                i += 1;
                element = sd - (i as f64 * step_double);
            }
        } else if ed > sd {
            // Increasing float range.
            if is_step_negative {
                return Err(fail(vm, Fail::NegativeStep));
            }
            if ed - sd < step_double {
                return Err(fail(vm, Fail::Boundary));
            }
            let size = double_range_size(vm, ed, sd, step_double, sd, ed)?;
            let mut i = 0u64;
            let mut element = sd;
            while i < size && element <= ed {
                out.push(Value::Float(element));
                i += 1;
                element = sd + (i as f64 * step_double);
            }
        } else {
            out.push(Value::Float(sd));
        }
    } else {
        let (sl, el) = (start.lval, end.lval);
        let unsigned_step = step as u64;
        if sl > el {
            // Decreasing int range.
            if (sl as u64).wrapping_sub(el as u64) < unsigned_step {
                return Err(fail(vm, Fail::Boundary));
            }
            let size = long_range_size(vm, sl, el, unsigned_step, el, sl)?;
            for i in 0..size {
                out.push(Value::Int(
                    sl.wrapping_sub((i.wrapping_mul(unsigned_step)) as i64),
                ));
            }
        } else if el > sl {
            // Increasing int range.
            if is_step_negative {
                return Err(fail(vm, Fail::NegativeStep));
            }
            if (el as u64).wrapping_sub(sl as u64) < unsigned_step {
                return Err(fail(vm, Fail::Boundary));
            }
            let size = long_range_size(vm, el, sl, unsigned_step, sl, el)?;
            for i in 0..size {
                out.push(Value::Int(
                    sl.wrapping_add((i.wrapping_mul(unsigned_step)) as i64),
                ));
            }
        } else {
            out.push(Value::Int(sl));
        }
    }
    Ok(Value::array(PhpArray::from_values(out)))
}

/// `RANGE_CHECK_DOUBLE_INIT_ARRAY(hi, lo, step)`; `shown_start` / `shown_end`
/// are what the error message prints for `start=` and `end=`.
fn double_range_size(
    vm: &mut Vm,
    hi: f64,
    lo: f64,
    step: f64,
    shown_start: f64,
    shown_end: f64,
) -> PResult<u64> {
    let calc = ((hi - lo) / step) + 1.0;
    let max = HT_MAX_SIZE as f64;
    if calc >= max {
        let exceed = calc - max;
        let msg = format!(
            "The supplied range exceeds the maximum array size by {exceed:.1} elements: start={shown_start:.1}, end={shown_end:.1}, step={step:.1}. Max size: {max:.0}"
        );
        return Err(vm.throw_value_error(msg));
    }
    Ok(round_half_up(calc) as u32 as u64)
}

/// `RANGE_CHECK_LONG_INIT_ARRAY(hi, lo, step)`.
fn long_range_size(
    vm: &mut Vm,
    hi: i64,
    lo: i64,
    step: u64,
    shown_start: i64,
    shown_end: i64,
) -> PResult<u64> {
    let calc = (hi as u64).wrapping_sub(lo as u64) / step;
    let max = HT_MAX_SIZE as u64;
    if calc >= max - 1 {
        let excess = calc - (max - 1);
        let msg = format!(
            "The supplied range exceeds the maximum array size by {excess} elements: start={shown_start}, end={shown_end}, step={step}. Calculated size: {calc}. Maximum size: {max}."
        );
        return Err(vm.throw_value_error(msg));
    }
    Ok(calc + 1)
}

pub static FUNCTIONS: &[NativeFunction] = &[NativeFunction {
    sig: "range(string|int|float $start, string|int|float $end, int|float $step = 1): array",
    handler: range,
}];
