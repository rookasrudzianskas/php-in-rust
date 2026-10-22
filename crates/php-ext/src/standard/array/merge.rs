//! `array_merge`, `array_merge_recursive`, `array_replace` and
//! `array_replace_recursive`.

use std::rc::Rc;

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpRef, Value, Vm};

use super::util::{HT_MAX_SIZE, append_or_throw, cannot_add_element, copy_elem};

/// The array in `v`, taking it out of the `Rc` when it is not shared
/// (`SEPARATE_ARRAY`).
fn take_array(rc: Rc<PhpArray>) -> PhpArray {
    Rc::try_unwrap(rc).unwrap_or_else(|rc| (*rc).clone())
}

fn recursion_detected(vm: &mut Vm) -> php_runtime::Flow {
    vm.throw_error("Error", "Recursion detected")
}

// ---------------------------------------------------------------------------
// array_merge / array_merge_recursive
// ---------------------------------------------------------------------------

/// `php_array_merge`: string keys overwrite, integer keys are appended.
fn merge_into(vm: &mut Vm, dest: &mut PhpArray, src: &PhpArray) -> PResult<()> {
    for (key, v) in src.iter() {
        match key {
            ArrayKey::Str(_) => {
                dest.insert(key, copy_elem(v));
            }
            ArrayKey::Int(_) => append_or_throw(vm, dest, copy_elem(v))?,
        }
    }
    Ok(())
}

/// `php_array_merge_recursive`.
fn merge_recursive(
    vm: &mut Vm,
    dest: &mut PhpArray,
    src: &PhpArray,
    guard: &mut Vec<usize>,
) -> PResult<()> {
    for (key, src_entry) in src.iter() {
        let ArrayKey::Str(_) = &key else {
            append_or_throw(vm, dest, copy_elem(src_entry))?;
            continue;
        };
        if !dest.contains_key(&key) {
            dest.insert(key, copy_elem(src_entry));
            continue;
        }
        // Take the destination entry out of its slot (SEPARATE_ZVAL).
        let dest_entry = {
            let slot = dest.get_mut(&key).expect("checked above");
            std::mem::replace(slot, Value::Null)
        };
        let src_zval = src_entry.unref();
        let dest_zval = dest_entry.into_unref();
        let thash = match &dest_zval {
            Value::Array(a) => Some(Rc::as_ptr(a) as usize),
            _ => None,
        };
        if thash.is_some_and(|p| guard.contains(&p)) {
            return Err(recursion_detected(vm));
        }

        // convert_to_array() on the destination entry.
        let mut arr: PhpArray = match dest_zval {
            Value::Array(a) => take_array(a),
            Value::Null | Value::Undef => {
                let mut a = PhpArray::new();
                let _ = a.append(Value::Null);
                a
            }
            other => vm.cast_to_array(&other)?,
        };

        let src_array: Option<Rc<PhpArray>> = match &src_zval {
            Value::Object(_) => Some(Rc::new(vm.cast_to_array(&src_zval)?)),
            Value::Array(a) => Some(a.clone()),
            _ => None,
        };
        match src_array {
            Some(sa) => {
                if let Some(p) = thash {
                    guard.push(p);
                }
                let r = merge_recursive(vm, &mut arr, &sa, guard);
                if thash.is_some() {
                    guard.pop();
                }
                r?;
            }
            None => {
                if arr.append(src_zval).is_err() {
                    return Err(cannot_add_element(vm));
                }
            }
        }
        dest.insert(key, Value::Array(Rc::new(arr)));
    }
    Ok(())
}

fn merge_wrapper(vm: &mut Vm, args: &mut Args, recursive: bool) -> PResult<Value> {
    let arrays: Vec<Rc<PhpArray>> = args
        .rest(0)
        .iter()
        .map(|v| match v {
            Value::Array(a) => a.clone(),
            _ => unreachable!("array_merge() argument after ZPP"),
        })
        .collect();
    if arrays.is_empty() {
        return Ok(Value::empty_array());
    }
    let count: u64 = arrays.iter().map(|a| a.len() as u64).sum();
    if count >= HT_MAX_SIZE as u64 {
        return Err(vm.throw_error(
            "Error",
            format!("The total number of elements must be lower than {HT_MAX_SIZE}"),
        ));
    }

    if arrays.len() == 2 {
        let ret = if arrays[0].is_empty() {
            Some(&arrays[1])
        } else if arrays[1].is_empty() {
            Some(&arrays[0])
        } else {
            None
        };
        if let Some(ret) = ret {
            let reusable = if ret.is_packed() {
                ret.is_list()
            } else {
                ret.keys().all(|k| matches!(k, ArrayKey::Str(_)))
            };
            if reusable {
                return Ok(Value::Array(ret.clone()));
            }
        }
    }

    // Copy the first array, renumbering integer keys.
    let mut dest = PhpArray::with_capacity(count as usize);
    for (key, v) in arrays[0].iter() {
        match key {
            ArrayKey::Str(_) => {
                dest.insert(key, copy_elem(v));
            }
            ArrayKey::Int(_) => {
                let _ = dest.append(copy_elem(v));
            }
        }
    }
    if recursive {
        let mut guard = Vec::new();
        for a in &arrays[1..] {
            merge_recursive(vm, &mut dest, a, &mut guard)?;
        }
    } else {
        for a in &arrays[1..] {
            merge_into(vm, &mut dest, a)?;
        }
    }
    Ok(Value::array(dest))
}

fn array_merge(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    merge_wrapper(vm, args, false)
}

fn array_merge_recursive(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    merge_wrapper(vm, args, true)
}

// ---------------------------------------------------------------------------
// array_replace / array_replace_recursive
// ---------------------------------------------------------------------------

/// `php_array_replace_recursive`.
fn replace_recursive(
    vm: &mut Vm,
    dest: &mut PhpArray,
    src: &PhpArray,
    guard: &mut Vec<usize>,
) -> PResult<()> {
    for (key, src_entry) in src.iter() {
        let src_zval = src_entry.unref();
        let dest_entry = dest.get(&key).cloned();
        let recurse = matches!(src_zval, Value::Array(_))
            && dest_entry
                .as_ref()
                .is_some_and(|d| matches!(d.unref(), Value::Array(_)));
        if !recurse {
            dest.insert(key, copy_elem(src_entry));
            continue;
        }
        let dest_entry = dest_entry.expect("checked above");
        let same_ref = match (src_entry, &dest_entry) {
            (Value::Ref(a), Value::Ref(b)) => PhpRef::ptr_eq(a, b) && (b.refcount() - 1) % 2 == 1,
            _ => false,
        };
        let (Value::Array(dest_arr), Value::Array(src_arr)) = (dest_entry.unref(), src_zval) else {
            unreachable!()
        };
        let (dp, sp) = (
            Rc::as_ptr(&dest_arr) as usize,
            Rc::as_ptr(&src_arr) as usize,
        );
        if guard.contains(&dp) || guard.contains(&sp) || same_ref {
            return Err(recursion_detected(vm));
        }
        // SEPARATE_ZVAL(dest_entry): detach the entry from the slot first.
        drop(dest_entry);
        drop(dest_arr);
        let taken = {
            let slot = dest.get_mut(&key).expect("present");
            std::mem::replace(slot, Value::Null)
        };
        let Value::Array(dest_arr) = taken.into_unref() else {
            unreachable!()
        };
        let mut arr = take_array(dest_arr);
        guard.push(dp);
        guard.push(sp);
        let r = replace_recursive(vm, &mut arr, &src_arr, guard);
        guard.pop();
        guard.pop();
        r?;
        dest.insert(key, Value::Array(Rc::new(arr)));
    }
    Ok(())
}

fn replace_wrapper(vm: &mut Vm, args: &mut Args, recursive: bool) -> PResult<Value> {
    let first = args.array(0).clone();
    let mut dest: PhpArray = (*first).clone();
    drop(first);
    let mut guard = Vec::new();
    for v in args.rest(1) {
        let Value::Array(a) = v else {
            unreachable!("array_replace() argument after ZPP")
        };
        if recursive {
            replace_recursive(vm, &mut dest, a, &mut guard)?;
        } else {
            // zend_hash_merge(dest, src, zval_add_ref, overwrite = 1)
            for (key, v) in a.iter() {
                dest.insert(key, copy_elem(v));
            }
        }
    }
    Ok(Value::array(dest))
}

fn array_replace(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    replace_wrapper(vm, args, false)
}

fn array_replace_recursive(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    replace_wrapper(vm, args, true)
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "array_merge(array ...$arrays): array",
        handler: array_merge,
    },
    NativeFunction {
        sig: "array_merge_recursive(array ...$arrays): array",
        handler: array_merge_recursive,
    },
    NativeFunction {
        sig: "array_replace(array $array, array ...$replacements): array",
        handler: array_replace,
    },
    NativeFunction {
        sig: "array_replace_recursive(array $array, array ...$replacements): array",
        handler: array_replace_recursive,
    },
];
