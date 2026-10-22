//! ext/spl/spl_iterators.c — the iterator interfaces (`RecursiveIterator`,
//! `OuterIterator`, `SeekableIterator`), the iterator classes and
//! `iterator_to_array()`, `iterator_count()`, `iterator_apply()`.

use php_runtime::value::convert::OffsetKind;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, Registry, Value, Vm};

use super::engine::{abstract_method, interface, iterator_apply};

pub const RECURSIVE_ITERATOR: &str = "RecursiveIterator";
pub const OUTER_ITERATOR: &str = "OuterIterator";
pub const SEEKABLE_ITERATOR: &str = "SeekableIterator";

// ---------------------------------------------------------------------------
// Interfaces
// ---------------------------------------------------------------------------

static RECURSIVE_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool;",
    handler: abstract_method,
};
static RECURSIVE_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): ?RecursiveIterator;",
    handler: abstract_method,
};
static OUTER_GET_INNER_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getInnerIterator(): ?Iterator;",
    handler: abstract_method,
};
static SEEKABLE_SEEK: NativeFunction = NativeFunction {
    sig: "public function seek(int $offset): void;",
    handler: abstract_method,
};

fn register_interfaces(r: &mut Registry) {
    interface(
        r,
        RECURSIVE_ITERATOR,
        &["Iterator"],
        &[&RECURSIVE_HAS_CHILDREN, &RECURSIVE_GET_CHILDREN],
    );
    interface(
        r,
        OUTER_ITERATOR,
        &["Iterator"],
        &[&OUTER_GET_INNER_ITERATOR],
    );
    interface(r, SEEKABLE_ITERATOR, &["Iterator"], &[&SEEKABLE_SEEK]);
}

// ---------------------------------------------------------------------------
// iterator_to_array / iterator_count / iterator_apply
// ---------------------------------------------------------------------------

fn iterator_to_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let use_keys = args.bool(1);
    let obj = match args.get(0) {
        Value::Array(a) => {
            if use_keys {
                return Ok(Value::Array(a.clone()));
            }
            // zend_array_to_list()
            let values: Vec<Value> = a.values().cloned().collect();
            return Ok(Value::array(PhpArray::from_values(values)));
        }
        Value::Object(o) => o.clone(),
        _ => unreachable!("iterator_to_array(): iterable after ZPP"),
    };
    let mut out = PhpArray::new();
    iterator_apply(vm, &obj, |vm, iter| {
        let data = iter.current(vm)?;
        if use_keys {
            // array_set_zval_key()
            let key = iter.key(vm)?;
            let key = vm.to_array_key(&key, OffsetKind::Write)?;
            out.insert(key, data);
        } else {
            let _ = out.append(data);
        }
        Ok(true)
    })?;
    Ok(Value::array(out))
}

fn iterator_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = match args.get(0) {
        Value::Array(a) => return Ok(Value::Int(a.len() as i64)),
        Value::Object(o) => o.clone(),
        _ => unreachable!("iterator_count(): iterable after ZPP"),
    };
    let mut count: i64 = 0;
    iterator_apply(vm, &obj, |_, _| {
        if count == i64::MAX {
            return Ok(false);
        }
        count += 1;
        Ok(true)
    })?;
    Ok(Value::Int(count))
}

fn iterator_apply_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    let callback = args.get(1).clone();
    // The array supplies the callback's arguments (`zend_call_known_fcc`
    // with `named_params`); string keys would be named arguments, which
    // `Vm::call` cannot pass, so only the values are used.
    let params: Vec<Value> = args
        .array_opt(2)
        .map(|a| a.values().cloned().collect())
        .unwrap_or_default();
    let mut count: i64 = 0;
    iterator_apply(vm, &obj, |vm, _| {
        count += 1;
        let ret = vm.call(&callback, params.clone())?;
        vm.to_bool(&ret.into_unref())
    })?;
    Ok(Value::Int(count))
}

pub static ITERATOR_APPLY: NativeFunction = NativeFunction {
    sig: "function iterator_apply(Traversable $iterator, callable $callback, ?array $args = null): int {}",
    handler: iterator_apply_fn,
};
pub static ITERATOR_COUNT: NativeFunction = NativeFunction {
    sig: "function iterator_count(iterable $iterator): int {}",
    handler: iterator_count,
};
pub static ITERATOR_TO_ARRAY: NativeFunction = NativeFunction {
    sig: "function iterator_to_array(iterable $iterator, bool $preserve_keys = true): array {}",
    handler: iterator_to_array,
};

/// `PHP_MINIT_FUNCTION(spl_iterators)` plus the functions of the file.
pub fn register(r: &mut Registry) {
    register_interfaces(r);
    r.functions(&[&ITERATOR_APPLY, &ITERATOR_COUNT, &ITERATOR_TO_ARRAY]);
}
