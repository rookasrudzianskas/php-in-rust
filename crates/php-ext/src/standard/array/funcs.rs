//! `count`, `array_map`, `array_filter`, `array_reduce`, `array_find*`,
//! `array_any`, `array_all`, `array_sum` and `array_product`.

use std::rc::Rc;

use php_runtime::class::CastKind;
use php_runtime::{Args, ArrayKey, Flow, NativeFunction, PResult, PhpArray, Value, Vm};

use super::util::{call, copy_elem, php_get_long};

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

/// `php_count_recursive`. `None` means an exception is pending.
fn count_recursive(vm: &mut Vm, ht: &Rc<PhpArray>, guard: &mut Vec<usize>) -> PResult<Option<i64>> {
    let ptr = Rc::as_ptr(ht) as usize;
    if guard.contains(&ptr) {
        vm.warning("Recursion detected")?;
        return Ok(Some(0));
    }
    guard.push(ptr);
    let mut cnt = ht.len() as i64;
    let subs: Vec<Rc<PhpArray>> = ht
        .values()
        .filter_map(|v| match &*v.deref() {
            Value::Array(a) => Some(a.clone()),
            _ => None,
        })
        .collect();
    for sub in subs {
        match count_recursive(vm, &sub, guard)? {
            Some(n) => cnt += n,
            None => {
                guard.pop();
                return Ok(None);
            }
        }
    }
    guard.pop();
    Ok(Some(cnt))
}

fn count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(1);
    if mode != 0 && mode != 1 {
        return Err(vm.argument_value_error(2, "must be either COUNT_NORMAL or COUNT_RECURSIVE"));
    }
    let value = args.get(0).clone();
    match &value {
        Value::Array(a) => {
            if mode == 0 {
                Ok(Value::Int(a.len() as i64))
            } else {
                let mut guard = Vec::new();
                match count_recursive(vm, a, &mut guard)? {
                    Some(n) => Ok(Value::Int(n)),
                    None => Ok(Value::Null),
                }
            }
        }
        Value::Object(o) => {
            // First, check whether the class has a count handler.
            if let Some(h) = o.class().handlers.count_elements {
                if let Some(n) = h(vm, o)? {
                    return Ok(Value::Int(n));
                }
            }
            // Otherwise call count() if the object implements Countable.
            if o.class().instance_of_name(b"Countable") {
                let ret = vm.call_method(o, b"count", Vec::new())?;
                let ret = ret.into_unref();
                return Ok(Value::Int(php_get_long(vm, &ret)?));
            }
            let msg = format!(
                "must be of type Countable|array, {} given",
                value.value_name()
            );
            Err(vm.argument_type_error(1, &msg))
        }
        _ => {
            let msg = format!(
                "must be of type Countable|array, {} given",
                value.value_name()
            );
            Err(vm.argument_type_error(1, &msg))
        }
    }
}

// ---------------------------------------------------------------------------
// array_map
// ---------------------------------------------------------------------------

fn array_map(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let callback: Option<Value> = args.opt(0).cloned();
    let mut arrays: Vec<Rc<PhpArray>> = vec![args.array(1).clone()];
    for v in args.rest(2) {
        match v {
            Value::Array(a) => arrays.push(a.clone()),
            _ => unreachable!("array_map() argument after ZPP"),
        }
    }

    if arrays.len() == 1 {
        let input = &arrays[0];
        // Short-circuit: no callback (or nothing to map) returns the input.
        let Some(cb) = callback else {
            return Ok(Value::Array(input.clone()));
        };
        if input.is_empty() {
            return Ok(Value::Array(input.clone()));
        }
        // The keys (including string keys and holes) are preserved.
        let mut out = PhpArray::with_capacity(input.len());
        for (key, v) in input.iter() {
            let r = call(vm, &cb, vec![v.clone()])?;
            out.insert(key, r);
        }
        // A packed input yields an array whose next free index follows the
        // last element.
        return Ok(Value::array(out));
    }

    let maxlen = arrays.iter().map(|a| a.len()).max().unwrap_or(0);
    if maxlen == 0 {
        return Ok(Value::empty_array());
    }
    let columns: Vec<Vec<Value>> = arrays
        .iter()
        .map(|a| a.values().cloned().collect())
        .collect();
    let mut out = Vec::with_capacity(maxlen);
    for k in 0..maxlen {
        let row: Vec<Value> = columns
            .iter()
            .map(|c| c.get(k).cloned().unwrap_or(Value::Null))
            .collect();
        match &callback {
            None => {
                // No callback: zip the arrays.
                out.push(Value::array(PhpArray::from_values(row)));
            }
            Some(cb) => out.push(call(vm, cb, row)?),
        }
    }
    Ok(Value::array(PhpArray::from_values(out)))
}

// ---------------------------------------------------------------------------
// array_filter / array_reduce / array_find*
// ---------------------------------------------------------------------------

fn array_filter(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let callback: Option<Value> = args.opt(1).cloned();
    let use_type = args.int(2);

    // ARRAY_FILTER_USE_VALUE = 0, ARRAY_FILTER_USE_BOTH = 1, ARRAY_FILTER_USE_KEY = 2
    if !(0..=2).contains(&use_type) {
        return Err(vm.argument_value_error(
            3,
            "must be one of ARRAY_FILTER_USE_VALUE, ARRAY_FILTER_USE_KEY, or ARRAY_FILTER_USE_BOTH",
        ));
    }
    if input.is_empty() {
        return Ok(Value::empty_array());
    }
    let mut out = PhpArray::new();
    for (key, operand) in input.iter() {
        let keep = match &callback {
            Some(cb) => {
                let cargs = match use_type {
                    2 => vec![key.to_value()],
                    1 => vec![operand.clone(), key.to_value()],
                    _ => vec![operand.clone()],
                };
                let r = call(vm, cb, cargs)?;
                vm.to_bool(&r)?
            }
            None => vm.to_bool(operand)?,
        };
        if keep {
            out.insert(key, copy_elem(operand));
        }
    }
    Ok(Value::array(out))
}

fn array_reduce(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let callback = args.get(1).clone();
    let mut acc = args.get(2).clone();
    for operand in input.values() {
        acc = call(vm, &callback, vec![acc, operand.clone()])?;
    }
    Ok(acc)
}

/// `php_array_find`: the first element for which the callback's truthiness
/// differs from `negate_condition`.
fn find_first(
    vm: &mut Vm,
    args: &mut Args,
    negate_condition: bool,
) -> PResult<Option<(ArrayKey, Value)>> {
    let input = args.array(0).clone();
    let callback = args.get(1).clone();
    for (key, operand) in input.iter() {
        let r = call(vm, &callback, vec![operand.clone(), key.to_value()])?;
        if vm.to_bool(&r)? ^ negate_condition {
            return Ok(Some((key, operand.unref())));
        }
    }
    Ok(None)
}

fn array_find(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(find_first(vm, args, false)?.map_or(Value::Null, |(_, v)| v))
}

fn array_find_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(find_first(vm, args, false)?.map_or(Value::Null, |(k, _)| k.into_value()))
}

fn array_any(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(find_first(vm, args, false)?.is_some()))
}

fn array_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(find_first(vm, args, true)?.is_none()))
}

// ---------------------------------------------------------------------------
// array_sum / array_product
// ---------------------------------------------------------------------------

fn binop(vm: &mut Vm, is_sum: bool, a: &Value, b: &Value) -> PResult<Value> {
    if is_sum { vm.add(a, b) } else { vm.mul(a, b) }
}

/// `php_array_binop_apply`.
fn binop_apply(vm: &mut Vm, acc: &mut Value, entry: &Value, is_sum: bool) -> PResult<()> {
    let op_name = if is_sum { "Addition" } else { "Multiplication" };
    // For objects we try to cast them to a numeric type.
    if let Value::Object(o) = entry {
        let dst = vm.object_cast(o, CastKind::Number)?;
        match dst {
            Some(d @ (Value::Int(_) | Value::Float(_))) => {
                if let Ok(v) = binop(vm, is_sum, acc, &d) {
                    *acc = v;
                }
            }
            // Do not type error for BC.
            _ => vm.warning(format!(
                "{op_name} is not supported on type {}",
                entry.type_name()
            ))?,
        }
        return Ok(());
    }

    match binop(vm, is_sum, acc, entry) {
        Ok(v) => *acc = v,
        Err(Flow::Exception(_)) => {
            // The exception is cleared; BC for resources and non-numeric strings.
            if let Value::Resource(r) = entry {
                if let Ok(v) = binop(vm, is_sum, acc, &Value::Int(r.id())) {
                    *acc = v;
                }
            } else if let Value::String(_) = entry {
                if let Ok(v) = binop(vm, is_sum, acc, &Value::Int(0)) {
                    *acc = v;
                }
            }
            vm.warning(format!(
                "{op_name} is not supported on type {}",
                entry.type_name()
            ))?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

fn array_binop(vm: &mut Vm, args: &mut Args, is_sum: bool) -> PResult<Value> {
    let input = args.array(0).clone();
    let initial = if is_sum { 0 } else { 1 };
    let mut acc = Value::Int(initial);
    for entry in input.values() {
        binop_apply(vm, &mut acc, entry, is_sum)?;
    }
    Ok(acc)
}

fn array_sum(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    array_binop(vm, args, true)
}

fn array_product(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    array_binop(vm, args, false)
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "count(Countable|array $value, int $mode = COUNT_NORMAL): int",
        handler: count,
    },
    NativeFunction {
        sig: "sizeof(Countable|array $value, int $mode = COUNT_NORMAL): int",
        handler: count,
    },
    NativeFunction {
        sig: "array_sum(array $array): int|float",
        handler: array_sum,
    },
    NativeFunction {
        sig: "array_product(array $array): int|float",
        handler: array_product,
    },
    NativeFunction {
        sig: "array_reduce(array $array, callable $callback, mixed $initial = null): mixed",
        handler: array_reduce,
    },
    NativeFunction {
        sig: "array_filter(array $array, ?callable $callback = null, int $mode = ARRAY_FILTER_USE_VALUE): array",
        handler: array_filter,
    },
    NativeFunction {
        sig: "array_find(array $array, callable $callback): mixed",
        handler: array_find,
    },
    NativeFunction {
        sig: "array_find_key(array $array, callable $callback): mixed",
        handler: array_find_key,
    },
    NativeFunction {
        sig: "array_any(array $array, callable $callback): bool",
        handler: array_any,
    },
    NativeFunction {
        sig: "array_all(array $array, callable $callback): bool",
        handler: array_all,
    },
    NativeFunction {
        sig: "array_map(?callable $callback, array $array, array ...$arrays): array",
        handler: array_map,
    },
];
