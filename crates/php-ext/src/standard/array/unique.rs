//! `array_unique()`.

use std::collections::HashSet;

use php_runtime::{Args, ArrayKey, Flow, NativeFunction, PResult, PhpArray, PhpStr, Value, Vm};

use super::sort::{Kind, data_cmp, flags, zend_sort};
use super::util::copy_elem;

/// A bucket of the temporary table `array_unique` sorts (`struct bucketindex`).
struct Item {
    key: ArrayKey,
    val: Value,
    /// Original position.
    i: usize,
}

#[allow(clippy::mutable_key_type)] // PhpStr hashes by content; the cache is not part of identity
fn array_unique(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let array = args.array(0).clone();
    let sort_type = args.int(1);

    if array.len() <= 1 {
        // Nothing to do.
        return Ok(Value::Array(array));
    }

    if sort_type == flags::SORT_STRING {
        let mut seen: HashSet<PhpStr> = HashSet::with_capacity(array.len());
        let mut out = PhpArray::new();
        for (key, val) in array.iter() {
            let s = vm.to_php_string(val)?;
            if seen.insert(s) {
                // First occurrence of the value.
                out.insert(key, copy_elem(val));
            }
        }
        return Ok(Value::array(out));
    }

    let kind = Kind::from_flags(sort_type);
    let mut result: PhpArray = (*array).clone();

    // Create and sort an array with the buckets (`cmp` is the *unstable*
    // comparison: ties keep whatever order `zend_sort` produces).
    let mut tmp: Vec<Item> = array
        .iter()
        .enumerate()
        .map(|(i, (key, val))| Item {
            key,
            val: val.clone(),
            i,
        })
        .collect();

    let mut err: Option<Flow> = None;
    {
        let mut cmp = |a: &Item, b: &Item| -> i32 {
            if err.is_some() {
                return 0;
            }
            match data_cmp(vm, kind, &a.val, &b.val) {
                Ok(r) => r,
                Err(e) => {
                    err = Some(e);
                    0
                }
            }
        };
        zend_sort(&mut tmp, &mut cmp);
    }
    if let Some(e) = err {
        return Err(e);
    }

    // Go through the sorted array and delete duplicates from the copy.
    let mut lastkept = 0usize;
    for cur in 1..tmp.len() {
        let c = data_cmp(vm, kind, &tmp[lastkept].val, &tmp[cur].val)?;
        if c != 0 {
            lastkept = cur;
        } else {
            let victim = if tmp[lastkept].i > tmp[cur].i {
                let v = lastkept;
                lastkept = cur;
                v
            } else {
                cur
            };
            result.remove(&tmp[victim].key);
        }
    }
    Ok(Value::array(result))
}

pub static FUNCTIONS: &[NativeFunction] = &[NativeFunction {
    sig: "array_unique(array $array, int $flags = SORT_STRING): array",
    handler: array_unique,
}];
