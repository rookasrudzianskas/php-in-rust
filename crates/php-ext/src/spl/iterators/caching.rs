//! `CachingIterator` and `RecursiveCachingIterator`: iterate one element
//! ahead of the inner iterator (so `hasNext()` can be answered), optionally
//! remembering the string value of each element (`CALL_TOSTRING`,
//! `TOSTRING_USE_*`) and every element (`FULL_CACHE`, exposed through
//! `ArrayAccess`, `getCache()` and `count()`).

use std::rc::Rc;

use php_runtime::class::ClassConstant;
use php_runtime::value::convert::OffsetKind;
use php_runtime::{Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpStr, Registry, Value, Vm};

use super::dual::{self, DitType, checked, with};
use super::inner::object_init_with_constructor;
use crate::spl::engine;

pub const CIT_CALL_TOSTRING: i64 = 0x0000_0001;
pub const CIT_TOSTRING_USE_KEY: i64 = 0x0000_0002;
pub const CIT_TOSTRING_USE_CURRENT: i64 = 0x0000_0004;
pub const CIT_TOSTRING_USE_INNER: i64 = 0x0000_0008;
pub const CIT_CATCH_GET_CHILD: i64 = 0x0000_0010;
pub const CIT_FULL_CACHE: i64 = 0x0000_0100;
pub const CIT_PUBLIC: i64 = 0x0000_FFFF;
pub const CIT_VALID: i64 = 0x0001_0000;

/// Message of the `spl_cit_check_flags` ValueError.
pub const FLAGS_ERROR: &str = "must contain only one of CachingIterator::CALL_TOSTRING, \
     CachingIterator::TOSTRING_USE_KEY, CachingIterator::TOSTRING_USE_CURRENT, \
     or CachingIterator::TOSTRING_USE_INNER";

/// `spl_cit_check_flags`: at most one of the string modes.
pub fn check_flags(flags: i64) -> bool {
    [
        CIT_CALL_TOSTRING,
        CIT_TOSTRING_USE_KEY,
        CIT_TOSTRING_USE_CURRENT,
        CIT_TOSTRING_USE_INNER,
    ]
    .iter()
    .filter(|&&f| flags & f != 0)
    .count()
        <= 1
}

fn flags(obj: &ObjRef) -> i64 {
    with(obj, |s| s.caching.flags)
}

/// Runs a call that `CATCH_GET_CHILD` may silence (`zend_clear_exception`).
fn catchable<T>(obj: &ObjRef, r: PResult<T>) -> PResult<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(php_runtime::Flow::Exception(_)) if flags(obj) & CIT_CATCH_GET_CHILD != 0 => Ok(None),
        Err(e) => Err(e),
    }
}

/// `spl_caching_it_next`: fetches the inner element and moves the inner
/// iterator one step ahead.
fn caching_next(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    match dual::fetch(vm, obj, true) {
        Ok(true) => {}
        Ok(false) => {
            with(obj, |s| s.caching.flags &= !CIT_VALID);
            return Ok(());
        }
        Err(e) => {
            with(obj, |s| s.caching.flags &= !CIT_VALID);
            return Err(e);
        }
    }
    let (fl, key, data) = with(obj, |s| {
        s.caching.flags |= CIT_VALID;
        (s.caching.flags, s.key.clone(), s.data.clone())
    });
    // Full cache?
    if fl & CIT_FULL_CACHE != 0 {
        let key = key.unwrap_or(Value::Null);
        let data = data.as_ref().map_or(Value::Null, Value::unref);
        // array_set_zval_key()
        let k = vm.to_array_key(&key, OffsetKind::Write)?;
        with(obj, |s| {
            Rc::make_mut(&mut s.caching.zcache).insert(k, data);
        });
    }
    // Recursion?
    if dual::dit_type(obj) == Some(DitType::RecursiveCaching) {
        let inner = dual::inner_obj(obj).expect("constructed caching iterator");
        let r = vm.call_method(&inner, b"hasChildren", Vec::new());
        if let Some(ret) = catchable(obj, r)? {
            if vm.to_bool(&ret.into_unref())? {
                // Store the children in the first constructor argument.
                let r = vm.call_method(&inner, b"getChildren", Vec::new());
                if let Some(children) = catchable(obj, r)? {
                    let fl = flags(obj) & CIT_PUBLIC;
                    let ce = vm
                        .find_class(b"RecursiveCachingIterator")
                        .expect("RecursiveCachingIterator is registered");
                    let r = object_init_with_constructor(
                        vm,
                        &ce,
                        vec![children.into_unref(), Value::Int(fl)],
                    );
                    if let Some(child) = catchable(obj, r)? {
                        with(obj, |s| s.caching.zchildren = Some(child));
                    }
                }
            }
        }
    }
    let fl = flags(obj);
    if fl & (CIT_TOSTRING_USE_INNER | CIT_CALL_TOSTRING) != 0 {
        let src = if fl & CIT_TOSTRING_USE_INNER != 0 {
            dual::inner_obj(obj).map_or(Value::Null, Value::Object)
        } else {
            dual::current_value(obj)
        };
        match vm.to_php_string(&src) {
            Ok(s) => with(obj, |st| st.caching.zstr = Some(s)),
            Err(e) => {
                // zval_get_string() yields "" and the step still happens.
                with(obj, |st| {
                    st.caching.zstr = Some(PhpStr::new());
                    st.pos += 1;
                });
                if let Some(it) = dual::inner_iter(obj) {
                    it.invalidate_current();
                }
                return Err(e);
            }
        }
    }
    dual::next(vm, obj, false)
}

/// `spl_caching_it_rewind`.
fn caching_rewind(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    dual::rewind(vm, obj)?;
    with(obj, |s| Rc::make_mut(&mut s.caching.zcache).clear());
    caching_next(vm, obj)
}

/// `BadMethodCallException` for cache accessors without `FULL_CACHE`.
fn require_full_cache(vm: &mut Vm, this: &ObjRef) -> PResult<()> {
    if flags(this) & CIT_FULL_CACHE == 0 {
        return Err(vm.throw_error(
            "BadMethodCallException",
            format!(
                "{} does not use a full cache (see CachingIterator::__construct)",
                this.class().name
            ),
        ));
    }
    Ok(())
}

/// ZPP `"S"` for the untyped `$key` parameter of the `offset*()` methods
/// (weak mode): `null` is deprecated, objects need `__toString()`.
fn key_arg(vm: &mut Vm, args: &Args) -> PResult<PhpStr> {
    let v = args.get(0).unref();
    if v.is_null() {
        let fname = args.function().display_name();
        vm.engine_deprecated(format!(
            "{fname}(): Passing null to parameter #1 ($key) of type string is deprecated"
        ))?;
        return Ok(PhpStr::new());
    }
    match vm.parse_arg_str_weak(&v)? {
        Some(s) => Ok(s),
        None => {
            let msg = format!("must be of type string, {} given", v.value_name());
            Err(vm.argument_type_error(1, &msg))
        }
    }
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn ci_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    dual::construct(vm, args, "CachingIterator", "Iterator", DitType::Caching)?;
    Ok(Value::Null)
}

fn ci_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    caching_rewind(vm, &this)?;
    Ok(Value::Null)
}

fn ci_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Bool(flags(&this) & CIT_VALID != 0))
}

fn ci_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    caching_next(vm, &this)?;
    Ok(Value::Null)
}

/// Whether the inner iterator has a further element.
fn ci_has_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Bool(dual::valid(vm, &this)?))
}

fn ci_to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let fl = flags(&this);
    if fl
        & (CIT_CALL_TOSTRING
            | CIT_TOSTRING_USE_KEY
            | CIT_TOSTRING_USE_CURRENT
            | CIT_TOSTRING_USE_INNER)
        == 0
    {
        return Err(vm.throw_error(
            "BadMethodCallException",
            format!(
                "{} does not fetch string value (see CachingIterator::__construct)",
                this.class().name
            ),
        ));
    }
    if fl & CIT_TOSTRING_USE_KEY != 0 {
        let key = with(&this, |s| s.key.clone()).unwrap_or(Value::Undef);
        return Ok(Value::String(vm.to_php_string(&key.unref())?));
    }
    if fl & CIT_TOSTRING_USE_CURRENT != 0 {
        let data = with(&this, |s| s.data.clone()).unwrap_or(Value::Undef);
        return Ok(Value::String(vm.to_php_string(&data.unref())?));
    }
    Ok(Value::String(
        with(&this, |s| s.caching.zstr.clone()).unwrap_or_default(),
    ))
}

fn ci_offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let key = key_arg(vm, args)?;
    let value = args.get(1).unref();
    let this = checked(vm, args)?;
    require_full_cache(vm, &this)?;
    with(&this, |s| {
        Rc::make_mut(&mut s.caching.zcache).insert(ArrayKey::from_bytes(&key), value);
    });
    Ok(Value::Null)
}

fn ci_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let key = key_arg(vm, args)?;
    let this = checked(vm, args)?;
    require_full_cache(vm, &this)?;
    let found = with(&this, |s| {
        s.caching
            .zcache
            .get(&ArrayKey::from_bytes(&key))
            .map(Value::unref)
    });
    match found {
        Some(v) => Ok(v),
        None => {
            vm.engine_warning(format!("Undefined array key \"{key}\""))?;
            Ok(Value::Null)
        }
    }
}

fn ci_offset_unset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let key = key_arg(vm, args)?;
    require_full_cache(vm, &this)?;
    with(&this, |s| {
        Rc::make_mut(&mut s.caching.zcache).remove(&ArrayKey::from_bytes(&key));
    });
    Ok(Value::Null)
}

fn ci_offset_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let key = key_arg(vm, args)?;
    let this = checked(vm, args)?;
    require_full_cache(vm, &this)?;
    Ok(Value::Bool(with(&this, |s| {
        s.caching.zcache.contains_key(&ArrayKey::from_bytes(&key))
    })))
}

fn ci_get_cache(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    require_full_cache(vm, &this)?;
    Ok(Value::Array(with(&this, |s| s.caching.zcache.clone())))
}

fn ci_get_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Int(flags(&this)))
}

fn ci_set_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let new = args.int(0);
    let this = checked(vm, args)?;
    if !check_flags(new) {
        return Err(vm.argument_value_error(1, FLAGS_ERROR));
    }
    let old = flags(&this);
    if old & CIT_CALL_TOSTRING != 0 && new & CIT_CALL_TOSTRING == 0 {
        return Err(vm.throw_error(
            "InvalidArgumentException",
            "Unsetting flag CALL_TO_STRING is not possible",
        ));
    }
    if old & CIT_TOSTRING_USE_INNER != 0 && new & CIT_TOSTRING_USE_INNER == 0 {
        return Err(vm.throw_error(
            "InvalidArgumentException",
            "Unsetting flag TOSTRING_USE_INNER is not possible",
        ));
    }
    with(&this, |s| {
        if new & CIT_FULL_CACHE != 0 && old & CIT_FULL_CACHE == 0 {
            // Clear on (re)enable.
            Rc::make_mut(&mut s.caching.zcache).clear();
        }
        s.caching.flags = (old & !CIT_PUBLIC) | (new & CIT_PUBLIC);
    });
    Ok(Value::Null)
}

fn ci_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    require_full_cache(vm, &this)?;
    Ok(Value::Int(with(&this, |s| s.caching.zcache.len() as i64)))
}

static CI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator, int $flags = CachingIterator::CALL_TOSTRING) {}",
    handler: ci_construct,
};
static CI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: ci_rewind,
};
static CI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: ci_valid,
};
static CI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: ci_next,
};
static CI_HAS_NEXT: NativeFunction = NativeFunction {
    sig: "public function hasNext(): bool {}",
    handler: ci_has_next,
};
static CI_TO_STRING: NativeFunction = NativeFunction {
    sig: "public function __toString(): string {}",
    handler: ci_to_string,
};
static CI_GET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function getFlags(): int {}",
    handler: ci_get_flags,
};
static CI_SET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function setFlags(int $flags): void {}",
    handler: ci_set_flags,
};
static CI_OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($key): mixed {}",
    handler: ci_offset_get,
};
static CI_OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet($key, mixed $value): void {}",
    handler: ci_offset_set,
};
static CI_OFFSET_UNSET: NativeFunction = NativeFunction {
    sig: "public function offsetUnset($key): void {}",
    handler: ci_offset_unset,
};
static CI_OFFSET_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists($key): bool {}",
    handler: ci_offset_exists,
};
static CI_GET_CACHE: NativeFunction = NativeFunction {
    sig: "public function getCache(): array {}",
    handler: ci_get_cache,
};
static CI_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: ci_count,
};

// ---------------------------------------------------------------------------
// RecursiveCachingIterator
// ---------------------------------------------------------------------------

fn rci_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    dual::construct(
        vm,
        args,
        "RecursiveCachingIterator",
        "RecursiveIterator",
        DitType::RecursiveCaching,
    )?;
    Ok(Value::Null)
}

fn rci_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Bool(with(&this, |s| s.caching.zchildren.is_some())))
}

fn rci_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(with(&this, |s| s.caching.zchildren.clone()).map_or(Value::Null, Value::Object))
}

static RCI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator, int $flags = RecursiveCachingIterator::CALL_TOSTRING) {}",
    handler: rci_construct,
};
static RCI_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: rci_has_children,
};
static RCI_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): ?RecursiveCachingIterator {}",
    handler: rci_get_children,
};

pub fn register(r: &mut Registry) {
    let parent = engine::class(r, "IteratorIterator");
    let mut ce = php_runtime::class::ClassEntry::new_subclass("CachingIterator", 0, &parent);
    for iface in ["ArrayAccess", "Countable", "Stringable"] {
        let iface = engine::class(r, iface);
        engine::implement(&mut ce, &iface);
    }
    for (name, value) in [
        ("CALL_TOSTRING", CIT_CALL_TOSTRING),
        ("CATCH_GET_CHILD", CIT_CATCH_GET_CHILD),
        ("TOSTRING_USE_KEY", CIT_TOSTRING_USE_KEY),
        ("TOSTRING_USE_CURRENT", CIT_TOSTRING_USE_CURRENT),
        ("TOSTRING_USE_INNER", CIT_TOSTRING_USE_INNER),
        ("FULL_CACHE", CIT_FULL_CACHE),
    ] {
        ce.add_constant(ClassConstant::new(
            name,
            Value::Int(value),
            "CachingIterator",
        ));
    }
    engine::add_methods(
        &mut ce,
        &[
            &CI_CONSTRUCT,
            &CI_REWIND,
            &CI_VALID,
            &CI_NEXT,
            &CI_HAS_NEXT,
            &CI_TO_STRING,
            &CI_GET_FLAGS,
            &CI_SET_FLAGS,
            &CI_OFFSET_GET,
            &CI_OFFSET_SET,
            &CI_OFFSET_UNSET,
            &CI_OFFSET_EXISTS,
            &CI_GET_CACHE,
            &CI_COUNT,
        ],
    );
    r.class(ce);

    dual::subclass(
        r,
        "RecursiveCachingIterator",
        0,
        "CachingIterator",
        &[super::RECURSIVE_ITERATOR],
        &[&RCI_CONSTRUCT, &RCI_HAS_CHILDREN, &RCI_GET_CHILDREN],
    );
}

/// The `FULL_CACHE` array of a caching iterator (tests).
#[cfg(test)]
pub fn cache_of(obj: &ObjRef) -> php_runtime::PhpArray {
    with(obj, |s| (*s.caching.zcache).clone())
}
