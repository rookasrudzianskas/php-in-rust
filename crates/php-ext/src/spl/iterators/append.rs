//! `AppendIterator`: iterates several iterators one after the other.
//!
//! The appended iterators live in an internal `ArrayIterator`
//! (`u.append.zarrayit`), driven only through its methods (`append()`,
//! `rewind()`, `valid()`, `current()`, `key()`, `next()`) like php-src's
//! `spl_array_iterator_append()` and `spl_array` object iterator. The
//! iterator over it does not cache `current()` (the `spl_array` iterator
//! reads the storage on every access).

use std::rc::Rc;

use php_runtime::{Args, NativeFunction, ObjRef, PResult, Registry, Value, Vm};

use super::dual::{self, DitType, checked, with};
use super::inner::{InnerIter, object_init_with_constructor};
use crate::spl::engine;

fn list_iter(obj: &ObjRef) -> Option<Rc<InnerIter>> {
    with(obj, |s| s.append.iter.clone())
}

/// `spl_append_it_next_iterator`: switches to the iterator at the list
/// position (rewinding it); `Ok(false)` when the list is exhausted.
fn next_iterator(vm: &mut Vm, obj: &ObjRef) -> PResult<bool> {
    dual::free(obj);
    with(obj, |s| {
        if s.inner_obj.is_some() {
            s.inner_obj = None;
            s.inner_ce = None;
            s.inner_iter = None;
        }
    });
    let Some(list) = list_iter(obj) else {
        return Ok(false);
    };
    if !list.valid(vm)? {
        return Ok(false);
    }
    let it = list.current(vm)?.into_unref();
    let Value::Object(it) = it else {
        return Ok(false);
    };
    let ce = it.class().clone();
    with(obj, |s| {
        s.inner_obj = Some(it.clone());
        s.inner_ce = Some(ce);
    });
    let iter = InnerIter::new(vm, &it)?;
    with(obj, |s| s.inner_iter = Some(iter));
    dual::rewind(vm, obj)?;
    Ok(true)
}

/// `spl_append_it_fetch`: skips exhausted iterators, then fetches.
fn append_fetch(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    while !dual::valid(vm, obj)? {
        if let Some(list) = list_iter(obj) {
            list.next(vm)?;
        }
        if !next_iterator(vm, obj)? {
            return Ok(());
        }
    }
    dual::fetch(vm, obj, false)?;
    Ok(())
}

/// `spl_append_it_next`.
fn append_next(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    if dual::valid(vm, obj)? {
        dual::next(vm, obj, true)?;
    }
    append_fetch(vm, obj)
}

fn ai_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if with(&this, |s| s.dit_type.is_some()) {
        return Err(vm.throw_error(
            "BadMethodCallException",
            "AppendIterator::getIterator() must be called exactly once per instance",
        ));
    }
    with(&this, |s| s.dit_type = Some(DitType::Append));
    let ce = vm
        .find_class(b"ArrayIterator")
        .ok_or_else(|| vm.throw_error("Error", "Class \"ArrayIterator\" not found"))?;
    let list = object_init_with_constructor(vm, &ce, Vec::new())?;
    let iter = InnerIter::uncached(list.clone());
    with(&this, |s| {
        s.append.zarrayit = Some(list);
        s.append.iter = Some(iter);
    });
    Ok(Value::Null)
}

fn ai_append(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let it = args.object(0).clone();
    let this = checked(vm, args)?;
    let (Some(list), Some(zarrayit)) =
        (list_iter(&this), with(&this, |s| s.append.zarrayit.clone()))
    else {
        return Err(vm.throw_error("Error", dual::INVALID_STATE));
    };
    if list.valid(vm)? && !dual::valid(vm, &this)? {
        vm.call_method(&zarrayit, b"append", vec![Value::Object(it.clone())])?;
        list.next(vm)?;
    } else {
        vm.call_method(&zarrayit, b"append", vec![Value::Object(it.clone())])?;
    }
    if dual::inner_iter(&this).is_none() || !dual::valid(vm, &this)? {
        if !list.valid(vm)? {
            list.rewind(vm)?;
        }
        loop {
            if !next_iterator(vm, &this)? {
                break;
            }
            if dual::inner_obj(&this).is_some_and(|o| ObjRef::ptr_eq(&o, &it)) {
                break;
            }
        }
        append_fetch(vm, &this)?;
    }
    Ok(Value::Null)
}

fn ai_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    dual::fetch(vm, &this, true)?;
    Ok(dual::current_value(&this))
}

/// Rewinds to the first iterator and rewinds it, too.
fn ai_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    if let Some(list) = list_iter(&this) {
        list.rewind(vm)?;
    }
    if next_iterator(vm, &this)? {
        append_fetch(vm, &this)?;
    }
    Ok(Value::Null)
}

fn ai_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Bool(with(&this, |s| s.data.is_some())))
}

fn ai_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    append_next(vm, &this)?;
    Ok(Value::Null)
}

/// The key of the current iterator in the internal `ArrayIterator`.
fn ai_get_iterator_index(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let Some(zarrayit) = with(&this, |s| s.append.zarrayit.clone()) else {
        return Ok(Value::Null);
    };
    Ok(vm.call_method(&zarrayit, b"key", Vec::new())?.into_unref())
}

fn ai_get_array_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(with(&this, |s| s.append.zarrayit.clone()).map_or(Value::Null, Value::Object))
}

static AI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct() {}",
    handler: ai_construct,
};
static AI_APPEND: NativeFunction = NativeFunction {
    sig: "public function append(Iterator $iterator): void {}",
    handler: ai_append,
};
static AI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: ai_rewind,
};
static AI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: ai_valid,
};
static AI_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: ai_current,
};
static AI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: ai_next,
};
static AI_GET_ITERATOR_INDEX: NativeFunction = NativeFunction {
    sig: "public function getIteratorIndex(): ?int {}",
    handler: ai_get_iterator_index,
};
static AI_GET_ARRAY_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getArrayIterator(): ArrayIterator {}",
    handler: ai_get_array_iterator,
};

pub fn register(r: &mut Registry) {
    dual::subclass(
        r,
        "AppendIterator",
        0,
        "IteratorIterator",
        &[],
        &[
            &AI_CONSTRUCT,
            &AI_APPEND,
            &AI_REWIND,
            &AI_VALID,
            &AI_CURRENT,
            &AI_NEXT,
            &AI_GET_ITERATOR_INDEX,
            &AI_GET_ARRAY_ITERATOR,
        ],
    );
}
