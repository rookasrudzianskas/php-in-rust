//! Functions that build or reshape arrays: `array_fill`, `array_fill_keys`,
//! `array_pad`, `array_combine`, `array_flip`, `array_chunk`, `array_slice`,
//! `array_splice`, `array_reverse`, `array_change_key_case` and the stack
//! functions `array_push`, `array_pop`, `array_shift`, `array_unshift`.

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpRef, Value, Vm};

use super::util::{HT_MAX_SIZE, append_or_throw, cannot_add_element, copy_elem, set_array};
use super::util::{ref_array, with_array_mut};

// ---------------------------------------------------------------------------
// array_fill / array_fill_keys
// ---------------------------------------------------------------------------

fn array_fill(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let start_key = args.int(0);
    let num = args.int(1);
    let val = args.get(2).clone();
    if num > 0 {
        if num > i32::MAX as i64 {
            return Err(vm.argument_value_error(2, "is too large"));
        }
        if start_key > i64::MAX - num + 1 {
            return Err(cannot_add_element(vm));
        }
        let mut out = PhpArray::with_capacity(num as usize);
        for i in 0..num {
            out.insert(ArrayKey::Int(start_key + i), val.clone());
        }
        Ok(Value::array(out))
    } else if num == 0 {
        Ok(Value::empty_array())
    } else {
        Err(vm.argument_value_error(2, "must be greater than or equal to 0"))
    }
}

fn array_fill_keys(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let keys = args.array(0).clone();
    let val = args.get(1).clone();
    let mut out = PhpArray::with_capacity(keys.len());
    for entry in keys.values() {
        let entry = entry.unref();
        // `zend_hash_index_add` / `zend_symtable_add`: the first one wins.
        let key = match &entry {
            Value::Int(i) => ArrayKey::Int(*i),
            other => ArrayKey::from_php_str(vm.to_php_string(other)?),
        };
        if !out.contains_key(&key) {
            out.insert(key, val.clone());
        }
    }
    Ok(Value::array(out))
}

// ---------------------------------------------------------------------------
// array_pad / array_combine / array_flip / array_change_key_case
// ---------------------------------------------------------------------------

fn array_pad(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let pad_size = args.int(1);
    let pad_value = args.get(2).clone();

    if !(-HT_MAX_SIZE..=HT_MAX_SIZE).contains(&pad_size) {
        return Err(vm.argument_value_error(2, "must not exceed the maximum allowed array size"));
    }
    let input_size = input.len() as i64;
    let pad_size_abs = pad_size.abs();
    if input_size >= pad_size_abs {
        return Ok(Value::Array(input));
    }
    let num_pads = (pad_size_abs - input_size) as usize;

    let mut out = PhpArray::with_capacity(pad_size_abs as usize);
    if pad_size < 0 {
        for _ in 0..num_pads {
            append_or_throw(vm, &mut out, pad_value.clone())?;
        }
    }
    for (key, v) in input.iter() {
        match key {
            ArrayKey::Str(_) => {
                out.insert(key, v.clone());
            }
            ArrayKey::Int(_) => append_or_throw(vm, &mut out, v.clone())?,
        }
    }
    if pad_size > 0 {
        for _ in 0..num_pads {
            append_or_throw(vm, &mut out, pad_value.clone())?;
        }
    }
    Ok(Value::array(out))
}

fn array_combine(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let keys = args.array(0).clone();
    let values = args.array(1).clone();
    if keys.len() != values.len() {
        return Err(vm.argument_value_error(
            1,
            "and argument #2 ($values) must have the same number of elements",
        ));
    }
    if keys.is_empty() {
        return Ok(Value::empty_array());
    }
    let mut out = PhpArray::with_capacity(keys.len());
    for (k, v) in keys.values().zip(values.values()) {
        let key = match k {
            Value::Int(i) => ArrayKey::Int(*i),
            other => ArrayKey::from_php_str(vm.to_php_string(other)?),
        };
        out.insert(key, copy_elem(v));
    }
    Ok(Value::array(out))
}

fn array_flip(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let mut out = PhpArray::with_capacity(input.len());
    for (key, entry) in input.iter() {
        let new_key = match &*entry.deref() {
            Value::Int(i) => ArrayKey::Int(*i),
            Value::String(s) => ArrayKey::from_php_str(s.clone()),
            _ => {
                vm.warning("Can only flip string and integer values, entry skipped")?;
                continue;
            }
        };
        out.insert(new_key, key.into_value());
    }
    Ok(Value::array(out))
}

fn array_change_key_case(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let to_upper = args.int(1);
    if to_upper != 0 && to_upper != 1 {
        return Err(vm.argument_value_error(2, "must be either CASE_LOWER or CASE_UPPER"));
    }
    let mut out = PhpArray::with_capacity(input.len());
    for (key, v) in input.iter() {
        match key {
            ArrayKey::Int(_) => {
                out.insert(key, copy_elem(v));
            }
            ArrayKey::Str(s) => {
                let new = if to_upper == 1 {
                    s.to_ascii_uppercase()
                } else {
                    s.to_ascii_lowercase()
                };
                out.insert(ArrayKey::Str(new), copy_elem(v));
            }
        }
    }
    Ok(Value::array(out))
}

// ---------------------------------------------------------------------------
// array_chunk / array_slice / array_reverse
// ---------------------------------------------------------------------------

fn array_chunk(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let mut size = args.int(1);
    let preserve_keys = args.bool(2);

    if size < 1 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    let num_in = input.len() as i64;
    if size > num_in {
        if num_in == 0 {
            return Ok(Value::empty_array());
        }
        size = num_in;
    }
    let size = size as usize;

    let mut chunks: Vec<Value> = Vec::with_capacity(input.len() / size + 1);
    if !preserve_keys {
        let vals: Vec<&Value> = input.values().collect();
        for chunk in vals.chunks(size) {
            let list: Vec<Value> = chunk.iter().map(|v| copy_elem(v)).collect();
            chunks.push(Value::array(PhpArray::from_values(list)));
        }
        return Ok(Value::array(PhpArray::from_values(chunks)));
    }

    let mut current = PhpArray::new();
    let mut n = 0usize;
    for (key, v) in input.iter() {
        current.insert(key, copy_elem(v));
        n += 1;
        if n == size {
            chunks.push(Value::array(std::mem::take(&mut current)));
            n = 0;
        }
    }
    if n > 0 {
        chunks.push(Value::array(current));
    }
    Ok(Value::array(PhpArray::from_values(chunks)))
}

fn array_slice(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let mut offset = args.int(1);
    let length_arg = args.int_opt(2);
    let preserve_keys = args.bool(3);

    let num_in = input.len() as i64;
    // We want all entries from offset to the end if length is not passed or is null.
    let mut length = length_arg.unwrap_or(num_in);

    // Clamp the offset..
    if offset > num_in {
        return Ok(Value::empty_array());
    } else if offset < 0 {
        offset += num_in;
        if offset < 0 {
            offset = 0;
        }
    }
    // ..and the length.
    if length < 0 {
        length += num_in - offset;
    } else if (offset as u64).wrapping_add(length as u64) > num_in as u64 {
        length = num_in - offset;
    }
    if length <= 0 {
        return Ok(Value::empty_array());
    }

    let mut out = PhpArray::with_capacity(length as usize);
    for (key, v) in input.iter().skip(offset as usize).take(length as usize) {
        match key {
            ArrayKey::Str(_) => {
                out.insert(key, copy_elem(v));
            }
            ArrayKey::Int(_) => {
                if preserve_keys {
                    out.insert(key, copy_elem(v));
                } else {
                    let _ = out.append(copy_elem(v));
                }
            }
        }
    }
    Ok(Value::array(out))
}

fn array_reverse(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let preserve_keys = args.bool(1);
    let entries: Vec<(ArrayKey, &Value)> = input.iter().collect();
    let mut out = PhpArray::with_capacity(entries.len());
    for (key, v) in entries.into_iter().rev() {
        match key {
            ArrayKey::Str(_) => {
                out.insert(key, copy_elem(v));
            }
            ArrayKey::Int(_) => {
                if preserve_keys {
                    out.insert(key, copy_elem(v));
                } else {
                    let _ = out.append(copy_elem(v));
                }
            }
        }
    }
    Ok(Value::array(out))
}

// ---------------------------------------------------------------------------
// array_push / array_pop / array_shift / array_unshift / array_splice
// ---------------------------------------------------------------------------

fn array_push(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let vals: Vec<Value> = args.rest(1).to_vec();
    let result = with_array_mut(vm, args, 0, |a| {
        for v in vals {
            if a.append(v).is_err() {
                return Err(());
            }
        }
        Ok(a.len())
    });
    match result {
        Ok(n) => Ok(Value::Int(n as i64)),
        Err(()) => Err(cannot_add_element(vm)),
    }
}

/// Whether the element `pick` selects from the array in `var` is a reference
/// to `var` itself (`$a = [&$a, ...]`): `array_pop`/`array_shift` then return
/// the variable's value as it is *after* the removal.
fn is_self_reference(var: &PhpRef, pick: impl Fn(&PhpArray) -> Option<Value>) -> bool {
    let slot = match &*var.borrow() {
        Value::Array(a) => pick(a.as_ref()),
        _ => None,
    };
    matches!(slot, Some(Value::Ref(r)) if PhpRef::ptr_eq(&r, var))
}

fn array_pop(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let var = args.by_ref(0);
    let self_ref = is_self_reference(&var, |a| a.last().map(|(_, v)| v.clone()));
    let popped = with_array_mut(vm, args, 0, |a| a.pop());
    Ok(match popped {
        Some(_) if self_ref => var.get(),
        Some(v) => v,
        None => Value::Null,
    })
}

fn array_shift(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let var = args.by_ref(0);
    let self_ref = is_self_reference(&var, |a| a.first().map(|(_, v)| v.clone()));
    let shifted = with_array_mut(vm, args, 0, |a| a.shift());
    Ok(match shifted {
        Some(_) if self_ref => var.get(),
        Some(v) => v,
        None => Value::Null,
    })
}

fn array_unshift(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let vals: Vec<Value> = args.rest(1).to_vec();
    let n = with_array_mut(vm, args, 0, |a| {
        a.unshift(vals);
        a.len()
    });
    Ok(Value::Int(n as i64))
}

fn array_splice(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(input) = ref_array(args, 0) else {
        return Ok(Value::empty_array());
    };
    let mut offset = args.int(1);
    let num_in = input.len() as i64;
    let mut length = args.int_opt(2).unwrap_or(num_in);

    let replacement: Option<PhpArray> = if args.is_passed(3) {
        // convert_to_array() on the last argument
        let v = args.get(3).clone();
        Some(vm.cast_to_array(&v)?)
    } else {
        None
    };

    // Clamp the offset..
    if offset > num_in {
        offset = num_in;
    } else if offset < 0 {
        offset += num_in;
        if offset < 0 {
            offset = 0;
        }
    }
    // ..and the length (a non-positive length removes nothing).
    if length < 0 {
        length += num_in - offset;
    } else if length > num_in - offset {
        length = num_in - offset;
    }
    let length = length.max(0);

    let mut removed = PhpArray::new();
    let mut out = PhpArray::with_capacity(input.len());
    let insert_replacement = |out: &mut PhpArray| {
        if let Some(r) = &replacement {
            for v in r.values() {
                let _ = out.append(v.clone());
            }
        }
    };
    let (offset_u, end) = (offset as usize, (offset + length) as usize);
    let mut replaced = false;
    for (i, (key, v)) in input.iter().enumerate() {
        if i == offset_u {
            insert_replacement(&mut out);
            replaced = true;
        }
        let v = v.clone();
        let removing = i >= offset_u && i < end;
        let target = if removing { &mut removed } else { &mut out };
        match key {
            ArrayKey::Int(_) => {
                let _ = target.append(v);
            }
            s => {
                target.insert(s, v);
            }
        }
    }
    if !replaced {
        insert_replacement(&mut out);
    }
    set_array(&args.by_ref(0), out);
    Ok(Value::array(removed))
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "array_push(array &$array, mixed ...$values): int",
        handler: array_push,
    },
    NativeFunction {
        sig: "array_fill(int $start_index, int $count, mixed $value): array",
        handler: array_fill,
    },
    NativeFunction {
        sig: "array_fill_keys(array $keys, mixed $value): array",
        handler: array_fill_keys,
    },
    NativeFunction {
        sig: "array_pop(array &$array): mixed",
        handler: array_pop,
    },
    NativeFunction {
        sig: "array_shift(array &$array): mixed",
        handler: array_shift,
    },
    NativeFunction {
        sig: "array_unshift(array &$array, mixed ...$values): int",
        handler: array_unshift,
    },
    NativeFunction {
        sig: "array_splice(array &$array, int $offset, ?int $length = null, mixed $replacement = []): array",
        handler: array_splice,
    },
    NativeFunction {
        sig: "array_slice(array $array, int $offset, ?int $length = null, bool $preserve_keys = false): array",
        handler: array_slice,
    },
    NativeFunction {
        sig: "array_reverse(array $array, bool $preserve_keys = false): array",
        handler: array_reverse,
    },
    NativeFunction {
        sig: "array_pad(array $array, int $length, mixed $value): array",
        handler: array_pad,
    },
    NativeFunction {
        sig: "array_flip(array $array): array",
        handler: array_flip,
    },
    NativeFunction {
        sig: "array_change_key_case(array $array, int $case = CASE_LOWER): array",
        handler: array_change_key_case,
    },
    NativeFunction {
        sig: "array_chunk(array $array, int $length, bool $preserve_keys = false): array",
        handler: array_chunk,
    },
    NativeFunction {
        sig: "array_combine(array $keys, array $values): array",
        handler: array_combine,
    },
];
