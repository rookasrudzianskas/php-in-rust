//! `MultipleIterator`: iterates over all attached iterators at once. Its
//! storage is an `SplObjectStorage` storage (iterator => info) whose flags
//! word holds the `MIT_*` flags.

use php_runtime::class::{ClassConstant, ClassEntry};
use php_runtime::types::TypeDecl;
use php_runtime::{Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, Registry, Value, Vm};

use super::super::engine::{self, this};
use super::object_storage::sos_debug_info;
use super::storage::{self, attach, contains, detach, len, throw_chained, with_state};

pub const MULTIPLE_ITERATOR: &str = "MultipleIterator";

/// `MIT_NEED_ANY`: `valid()` if any sub-iterator is valid.
pub const MIT_NEED_ANY: i64 = 0;
/// `MIT_NEED_ALL`: `valid()` only if all sub-iterators are valid.
pub const MIT_NEED_ALL: i64 = 1;
/// `MIT_KEYS_NUMERIC`: `key()`/`current()` are lists.
pub const MIT_KEYS_NUMERIC: i64 = 0;
/// `MIT_KEYS_ASSOC`: `key()`/`current()` are keyed by the attached infos.
pub const MIT_KEYS_ASSOC: i64 = 2;

fn mi_construct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flags = args.int(0);
    with_state(&this(args), |s| s.flags = flags);
    Ok(Value::Null)
}

fn mi_get_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(with_state(&this(args), |s| s.flags)))
}

fn mi_set_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flags = args.int(0);
    with_state(&this(args), |s| s.flags = flags);
    Ok(Value::Null)
}

fn mi_attach_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let iterator = args.object(0).clone();
    let info = match args.get(1).unref() {
        Value::Null | Value::Undef => None,
        v => Some(v),
    };
    if let Some(info) = &info {
        // The duplicate scan uses (and leaves at the end) the cursor.
        let duplicate = with_state(&this, |s| {
            s.reset_pos();
            while let Some(e) = s.current() {
                if info.identical(&e.inf) {
                    return true;
                }
                s.move_forward();
            }
            false
        });
        if duplicate {
            return Err(vm.throw_error("InvalidArgumentException", "Key duplication error"));
        }
    }
    attach(vm, &this, &iterator, info)?;
    Ok(Value::Null)
}

fn mi_detach_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let iterator = args.object(0).clone();
    detach(vm, &this, &iterator)?;
    with_state(&this, |s| {
        s.reset_pos();
        s.index = 0;
    });
    Ok(Value::Null)
}

fn mi_contains_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let iterator = args.object(0).clone();
    Ok(Value::Bool(contains(vm, &this(args), &iterator)?))
}

fn mi_count_iterators(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(len(&this(args)) as i64))
}

/// The sub-iterator at the cursor and its info. The cursor is re-read on
/// every step: a sub-iterator may detach itself (which resets it).
fn current_entry(this: &ObjRef) -> Option<(ObjRef, Value)> {
    with_state(this, |s| {
        s.current().map(|e| (e.obj.clone(), e.inf.clone()))
    })
}

fn advance(this: &ObjRef) {
    with_state(this, |s| {
        s.move_forward();
    });
}

/// Calls `method` on every attached iterator (`rewind()` / `next()`).
fn call_all(vm: &mut Vm, this: &ObjRef, method: &[u8]) -> PResult<()> {
    with_state(this, |s| s.reset_pos());
    while let Some((it, _)) = current_entry(this) {
        let r = vm.call_method(&it, method, Vec::new());
        advance(this);
        r?;
    }
    Ok(())
}

fn mi_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    call_all(vm, &this(args), b"rewind")?;
    Ok(Value::Null)
}

fn mi_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    call_all(vm, &this(args), b"next")?;
    Ok(Value::Null)
}

/// A sub-iterator's `valid()`: only a literal `true` counts.
fn sub_valid(vm: &mut Vm, it: &ObjRef) -> PResult<bool> {
    let r = vm.call_method(it, b"valid", Vec::new())?;
    Ok(matches!(r.unref(), Value::Bool(true)))
}

fn mi_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (empty, flags) = with_state(&this, |s| (s.table.is_empty(), s.flags));
    if empty {
        return Ok(Value::Bool(false));
    }
    let expect = flags & MIT_NEED_ALL != 0;
    with_state(&this, |s| s.reset_pos());
    while let Some((it, _)) = current_entry(&this) {
        if sub_valid(vm, &it)? != expect {
            return Ok(Value::Bool(!expect));
        }
        advance(&this);
    }
    Ok(Value::Bool(expect))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GetAll {
    Current,
    Key,
}

impl GetAll {
    fn name(self) -> &'static str {
        match self {
            GetAll::Current => "current",
            GetAll::Key => "key",
        }
    }
}

/// `spl_multiple_iterator_get_all`.
fn get_all(vm: &mut Vm, this: &ObjRef, which: GetAll) -> PResult<Value> {
    if len(this) < 1 {
        return Err(vm.throw_error(
            "RuntimeException",
            format!("Called {}() on an invalid iterator", which.name()),
        ));
    }
    let mut out = PhpArray::new();
    with_state(this, |s| s.reset_pos());
    while let Some((it, inf)) = current_entry(this) {
        let value = if sub_valid(vm, &it)? {
            let method: &[u8] = match which {
                GetAll::Current => b"current",
                GetAll::Key => b"key",
            };
            match vm.call_method(&it, method, Vec::new()) {
                Ok(v) => v.into_unref(),
                Err(e) => {
                    return Err(throw_chained(
                        vm,
                        e,
                        "RuntimeException",
                        "Failed to call sub iterator method",
                    ));
                }
            }
        } else if with_state(this, |s| s.flags) & MIT_NEED_ALL != 0 {
            return Err(vm.throw_error(
                "RuntimeException",
                format!("Called {}() with non valid sub iterator", which.name()),
            ));
        } else {
            Value::Null
        };
        if with_state(this, |s| s.flags) & MIT_KEYS_ASSOC != 0 {
            match inf {
                Value::Int(i) => {
                    out.insert(ArrayKey::Int(i), value);
                }
                Value::String(s) => {
                    // zend_symtable_update: numeric strings become int keys.
                    out.insert(ArrayKey::from_php_str(s), value);
                }
                _ => {
                    return Err(vm.throw_error(
                        "InvalidArgumentException",
                        "Sub-Iterator is associated with NULL",
                    ));
                }
            }
        } else {
            let _ = out.append(value);
        }
        advance(this);
    }
    Ok(Value::array(out))
}

fn mi_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    get_all(vm, &this(args), GetAll::Current)
}

fn mi_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    get_all(vm, &this(args), GetAll::Key)
}

pub static CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(int $flags = MultipleIterator::MIT_NEED_ALL|MultipleIterator::MIT_KEYS_NUMERIC) {}",
    handler: mi_construct,
};
pub static GET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function getFlags(): int {}",
    handler: mi_get_flags,
};
pub static SET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function setFlags(int $flags): void {}",
    handler: mi_set_flags,
};
pub static ATTACH_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function attachIterator(Iterator $iterator, string|int|null $info = null): void {}",
    handler: mi_attach_iterator,
};
pub static DETACH_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function detachIterator(Iterator $iterator): void {}",
    handler: mi_detach_iterator,
};
pub static CONTAINS_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function containsIterator(Iterator $iterator): bool {}",
    handler: mi_contains_iterator,
};
pub static COUNT_ITERATORS: NativeFunction = NativeFunction {
    sig: "public function countIterators(): int {}",
    handler: mi_count_iterators,
};
pub static REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: mi_rewind,
};
pub static VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: mi_valid,
};
pub static KEY: NativeFunction = NativeFunction {
    sig: "public function key(): array {}",
    handler: mi_key,
};
pub static CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): array {}",
    handler: mi_current,
};
pub static NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: mi_next,
};
pub static DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: sos_debug_info,
};

/// `register_class_MultipleIterator` + its handlers (those of
/// `SplObjectStorage` except `write_dimension`; no `count_elements`, as
/// `MultipleIterator` is not `Countable`).
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(MULTIPLE_ITERATOR, 0);
    let iterator = engine::class(r, "Iterator");
    engine::implement(&mut ce, &iterator);
    for (name, value) in [
        ("MIT_NEED_ANY", MIT_NEED_ANY),
        ("MIT_NEED_ALL", MIT_NEED_ALL),
        ("MIT_KEYS_NUMERIC", MIT_KEYS_NUMERIC),
        ("MIT_KEYS_ASSOC", MIT_KEYS_ASSOC),
    ] {
        let mut c = ClassConstant::new(name, Value::Int(value), MULTIPLE_ITERATOR);
        c.ty = TypeDecl::parse("int").ok();
        ce.add_constant(c);
    }
    engine::add_methods(
        &mut ce,
        &[
            &CONSTRUCT,
            &GET_FLAGS,
            &SET_FLAGS,
            &ATTACH_ITERATOR,
            &DETACH_ITERATOR,
            &CONTAINS_ITERATOR,
            &COUNT_ITERATORS,
            &REWIND,
            &VALID,
            &KEY,
            &CURRENT,
            &NEXT,
            &DEBUG_INFO,
        ],
    );
    ce.handlers.create_object = Some(storage::create_object);
    ce.handlers.clone_obj = Some(storage::clone_obj);
    ce.handlers.compare = Some(storage::compare);
    ce.handlers.read_dimension = Some(storage::read_dimension);
    ce.handlers.write_dimension = Some(storage::multiple_iterator_write_dimension);
    ce.handlers.has_dimension = Some(storage::has_dimension);
    ce.handlers.unset_dimension = Some(storage::unset_dimension);
    r.class(ce);
}
