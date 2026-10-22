//! `min()` and `max()`.
//!
//! `min`/`max` mirror php-src's int/float fast paths exactly (they differ from
//! a plain `zend_compare` loop for NaN and for ties between an int and a
//! float). Calls with exactly two arguments follow the "frameless" variants
//! php-src's compiler uses for direct calls.

use php_runtime::value::numeric::dval_to_lval_silent;
use php_runtime::{Args, NativeFunction, PResult, Value, Vm};

/// `zend_dval_to_lval_silent((double) l) == l`: the int converts to a float
/// without loss.
fn exact_double(l: i64) -> bool {
    dval_to_lval_silent(l as f64) == l
}

/// `php_array_data_minmax`.
fn array_minmax(vm: &mut Vm, arr: &php_runtime::PhpArray, is_max: bool) -> PResult<Option<Value>> {
    let mut result: Option<&Value> = None;
    for v in arr.values() {
        match result {
            None => result = Some(v),
            Some(cur) => {
                let cmp = vm.compare(cur, v)?;
                if if is_max { cmp < 0 } else { cmp > 0 } {
                    result = Some(v);
                }
            }
        }
    }
    Ok(result.map(Value::unref))
}

/// `ZEND_FRAMELESS_FUNCTION(min|max, 2)`.
fn minmax2(vm: &mut Vm, lhs: &Value, rhs: &Value, is_max: bool) -> PResult<Value> {
    // `pick_lhs(lhs_before_rhs)` for min: lhs < rhs; for max: lhs >= rhs.
    let pick_d = |a: f64, b: f64| if is_max { a >= b } else { a < b };
    let pick_i = |a: i64, b: i64| if is_max { a >= b } else { a < b };
    let double_compare = |l: f64, r: f64| pick_d(l, r);
    let chosen_lhs = match (lhs, rhs) {
        (Value::Int(l), Value::Int(r)) => pick_i(*l, *r),
        (Value::Int(l), Value::Float(r)) if exact_double(*l) => double_compare(*l as f64, *r),
        (Value::Float(l), Value::Float(r)) => double_compare(*l, *r),
        (Value::Float(l), Value::Int(r)) if exact_double(*r) => double_compare(*l, *r as f64),
        _ => {
            let c = vm.compare(lhs, rhs)?;
            if is_max { c >= 0 } else { c < 0 }
        }
    };
    Ok(if chosen_lhs { lhs.clone() } else { rhs.clone() })
}

enum Phase {
    Long(i64),
    Double(f64),
    Generic,
}

/// `PHP_FUNCTION(min|max)` for three or more arguments.
fn minmax_n(vm: &mut Vm, vals: &[Value], is_max: bool) -> PResult<Value> {
    let mut cur = 0usize;
    let mut phase = match &vals[0] {
        Value::Int(l) => Phase::Long(*l),
        Value::Float(d) => Phase::Double(*d),
        _ => Phase::Generic,
    };
    let better_i = |cur: i64, x: i64| if is_max { cur < x } else { cur > x };
    let better_d = |cur: f64, x: f64| if is_max { cur < x } else { cur > x };
    let mut i = 1;
    while i < vals.len() {
        match phase {
            Phase::Long(lv) => match &vals[i] {
                Value::Int(x) => {
                    if better_i(lv, *x) {
                        phase = Phase::Long(*x);
                        cur = i;
                    }
                    i += 1;
                }
                Value::Float(_) if exact_double(lv) => {
                    // goto double_compare
                    phase = Phase::Double(lv as f64);
                }
                _ => {
                    // goto generic_compare
                    phase = Phase::Generic;
                }
            },
            Phase::Double(dv) => match &vals[i] {
                Value::Float(x) => {
                    if better_d(dv, *x) {
                        phase = Phase::Double(*x);
                        cur = i;
                    }
                    i += 1;
                }
                Value::Int(x) if exact_double(*x) => {
                    if better_d(dv, *x as f64) {
                        phase = Phase::Double(*x as f64);
                        cur = i;
                    }
                    i += 1;
                }
                _ => phase = Phase::Generic,
            },
            Phase::Generic => {
                let c = vm.compare(&vals[i], &vals[cur])?;
                if if is_max { c > 0 } else { c < 0 } {
                    cur = i;
                }
                i += 1;
            }
        }
    }
    Ok(vals[cur].clone())
}

fn minmax(vm: &mut Vm, args: &mut Args, is_max: bool) -> PResult<Value> {
    let mut vals: Vec<Value> = vec![args.get(0).clone()];
    vals.extend(args.rest(1).iter().cloned());
    match vals.len() {
        1 => {
            let Value::Array(a) = &vals[0] else {
                let msg = format!("must be of type array, {} given", vals[0].value_name());
                return Err(vm.argument_type_error(1, &msg));
            };
            match array_minmax(vm, a, is_max)? {
                Some(v) => Ok(v),
                None => Err(vm.argument_value_error(1, "must contain at least one element")),
            }
        }
        2 => minmax2(vm, &vals[0], &vals[1], is_max),
        _ => minmax_n(vm, &vals, is_max),
    }
}

fn min(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    minmax(vm, args, false)
}

fn max(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    minmax(vm, args, true)
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "min(mixed $value, mixed ...$values): mixed",
        handler: min,
    },
    NativeFunction {
        sig: "max(mixed $value, mixed ...$values): mixed",
        handler: max,
    },
];
