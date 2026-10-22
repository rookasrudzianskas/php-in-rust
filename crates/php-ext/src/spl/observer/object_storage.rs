//! `SplObjectStorage`: methods, the `Serializable` format
//! (`x:i:N;O:...,inf;...;m:a:...`), `__serialize()` / `__unserialize()` and
//! the class registration.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, fn_flags};
use php_runtime::value::convert::{proptable_to_symtable, to_int_silent};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::super::engine::{self, method_flags, this};
use super::super::functions::object_hash;
use super::SPL_OBJECT_STORAGE;
use super::storage::{
    self, addall, attach, check_not_in_get_hash, contains, detach, get_hash, len,
    object_properties_load, with_state,
};
use crate::standard::var_serialize::{
    SerializeData, UnserializeData, var_serialize, var_serialize_destroy, var_serialize_init,
    var_unserialize, var_unserialize_destroy, var_unserialize_init,
};

/// `PHP_COUNT_RECURSIVE`.
const COUNT_RECURSIVE: i64 = 1;

/// `zend_deprecated_function` for the methods marked
/// `#[\Deprecated(since: '8.5', message: "use method SplObjectStorage::<alt>() instead")]`.
fn deprecated_method(vm: &mut Vm, method: &str, alternative: &str) -> PResult<()> {
    vm.engine_deprecated(format!(
        "Method SplObjectStorage::{method}() is deprecated since 8.5, use method \
         SplObjectStorage::{alternative}() instead"
    ))
}

/// `Z_PARAM_OBJ` for the untyped `$object` parameters of the ArrayAccess
/// methods (their stubs cannot declare `object`).
fn object_arg(vm: &mut Vm, args: &Args, i: usize) -> PResult<ObjRef> {
    match args.get(i).unref() {
        Value::Object(o) => Ok(o),
        other => {
            let msg = format!("must be of type object, {} given", other.value_name());
            Err(vm.argument_type_error(i + 1, &msg))
        }
    }
}

// ---------------------------------------------------------------------------
// attach / detach / contains and their ArrayAccess aliases
// ---------------------------------------------------------------------------

fn attach_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = object_arg(vm, args, 0)?;
    let inf = args.get(1).clone();
    attach(vm, &this(args), &obj, Some(inf))?;
    Ok(Value::Null)
}

fn sos_attach(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "attach", "offsetSet")?;
    attach_impl(vm, args)
}

fn detach_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = object_arg(vm, args, 0)?;
    let this = this(args);
    detach(vm, &this, &obj)?;
    with_state(&this, |s| {
        s.reset_pos();
        s.index = 0;
    });
    Ok(Value::Null)
}

fn sos_detach(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "detach", "offsetUnset")?;
    detach_impl(vm, args)
}

fn contains_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = object_arg(vm, args, 0)?;
    Ok(Value::Bool(contains(vm, &this(args), &obj)?))
}

fn sos_contains(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "contains", "offsetExists")?;
    contains_impl(vm, args)
}

fn sos_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = object_arg(vm, args, 0)?;
    let this = this(args);
    let key = get_hash(vm, &this, &obj)?;
    let inf = with_state(&this, |s| s.table.get(&key).map(|e| e.inf.clone()));
    match inf {
        Some(inf) => Ok(inf.into_unref()),
        None => Err(vm.throw_error("UnexpectedValueException", "Object not found")),
    }
}

fn sos_get_hash(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(object_hash(args.object(0))))
}

// ---------------------------------------------------------------------------
// addAll / removeAll / removeAllExcept
// ---------------------------------------------------------------------------

fn sos_add_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let other = args.object(0).clone();
    addall(vm, &this, &other)?;
    Ok(Value::Int(len(&this) as i64))
}

fn sos_remove_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let other = args.object(0).clone();
    // Walks `other` with its internal pointer; a successful detach from
    // `this` retries the same element (it moves on once it is gone).
    with_state(&other, |s| s.internal_pointer = s.table.first_pos());
    loop {
        let current = with_state(&other, |s| {
            s.table.current(s.internal_pointer).map(|e| e.obj.clone())
        });
        let Some(obj) = current else { break };
        if !detach(vm, &this, &obj)? {
            with_state(&other, |s| {
                let mut ptr = s.internal_pointer;
                s.table.move_forward(&mut ptr);
                s.internal_pointer = ptr;
            });
        }
    }
    Ok(with_state(&this, |s| {
        s.reset_pos();
        s.index = 0;
        Value::Int(s.table.len() as i64)
    }))
}

fn sos_remove_all_except(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let other = args.object(0).clone();
    let mut cursor = php_runtime::Pos::MIN;
    while let Some((pos, elem)) = with_state(&this, |s| s.table.next_from(cursor)) {
        if !contains(vm, &other, &elem.obj)? {
            detach(vm, &this, &elem.obj)?;
        }
        cursor = pos + 1;
    }
    Ok(with_state(&this, |s| {
        s.reset_pos();
        s.index = 0;
        Value::Int(s.table.len() as i64)
    }))
}

// ---------------------------------------------------------------------------
// Info, count and iteration
// ---------------------------------------------------------------------------

fn sos_get_info(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(with_state(&this(args), |s| {
        s.current().map_or(Value::Null, |e| e.inf.clone())
    }))
}

fn sos_set_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let inf = args.get(0).clone().into_unref();
    check_not_in_get_hash(vm)?;
    let old = with_state(&this(args), |s| {
        let pos = s.pos;
        s.table
            .current_mut(pos)
            .map(|e| std::mem::replace(&mut e.inf, inf))
    });
    drop(old);
    Ok(Value::Null)
}

fn sos_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(0);
    let this = this(args);
    if mode == COUNT_RECURSIVE && with_state(&this, |s| s.compare_guard) {
        // php_count_recursive() on a table that is being compared.
        vm.warning("Recursion detected")?;
        return Ok(Value::Int(0));
    }
    // The elements are not arrays, so COUNT_RECURSIVE counts the same.
    Ok(Value::Int(len(&this) as i64))
}

/// `count_elements`: `count($storage)`. php-src has no handler and calls
/// `Countable::count()`; this is the same, minus the call when `count()`
/// is not overridden.
fn count_elements(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<i64>> {
    if engine::is_overridden(obj.class(), b"count", SPL_OBJECT_STORAGE) {
        let r = vm.call_method(obj, b"count", Vec::new())?;
        return Ok(Some(to_int_silent(&r)));
    }
    Ok(Some(len(obj) as i64))
}

fn sos_rewind(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    with_state(&this(args), |s| {
        s.reset_pos();
        s.index = 0;
    });
    Ok(Value::Null)
}

fn sos_valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(with_state(&this(args), |s| {
        s.current().is_some()
    })))
}

fn sos_key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(with_state(&this(args), |s| s.index)))
}

fn sos_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match with_state(&this(args), |s| s.current().map(|e| e.obj.clone())) {
        Some(obj) => Ok(Value::Object(obj)),
        None => Err(vm.throw_error("RuntimeException", "Called current() on invalid iterator")),
    }
}

fn sos_next(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    with_state(&this(args), |s| {
        s.move_forward();
        s.index += 1;
    });
    Ok(Value::Null)
}

fn sos_seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let position = args.int(0);
    let this = this(args);
    if position < 0 || position >= len(&this) as i64 {
        return Err(vm.throw_error(
            "OutOfBoundsException",
            format!("Seek position {position} is out of range"),
        ));
    }
    with_state(&this, |s| {
        if position == 0 {
            s.reset_pos();
            s.index = 0;
        } else if position > s.index {
            // Unlike going backwards, going to the end is not cheap.
            while position > s.index {
                s.move_forward();
                s.index += 1;
            }
        } else if position < s.index {
            // Resetting is cheap: seek forwards from the start when closer.
            if s.index - position > position {
                s.reset_pos();
                s.index = 0;
                while position > s.index {
                    s.move_forward();
                    s.index += 1;
                }
            } else {
                while position < s.index {
                    s.move_backward();
                    s.index -= 1;
                }
            }
        }
    });
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// Serializable
// ---------------------------------------------------------------------------

fn sos_serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let ctx = var_serialize_init(vm);
    let r = serialize_storage(vm, &this, &ctx);
    var_serialize_destroy(vm, ctx);
    Ok(Value::String(PhpStr::from_vec(r?)))
}

/// `SplObjectStorage::serialize()`: `x:i:N;` then `obj,inf;` per element
/// and `m:` with the property table, sharing the back-reference table of
/// an outer `serialize()`.
fn serialize_storage(
    vm: &mut Vm,
    this: &ObjRef,
    ctx: &Rc<RefCell<SerializeData>>,
) -> PResult<Vec<u8>> {
    let mut buf = b"x:".to_vec();
    var_serialize(vm, &mut buf, &Value::Int(len(this) as i64), ctx)?;
    let mut pos = with_state(this, |s| s.table.first_pos());
    while let Some(elem) = with_state(this, |s| s.table.current(pos).cloned()) {
        // The info is copied first: serializing the object may run user
        // code that modifies the storage.
        var_serialize(vm, &mut buf, &Value::Object(elem.obj), ctx)?;
        buf.push(b',');
        var_serialize(vm, &mut buf, &elem.inf, ctx)?;
        buf.push(b';');
        with_state(this, |s| s.table.move_forward(&mut pos));
    }
    buf.extend_from_slice(b"m:");
    let members = Value::array(engine::std_properties(this));
    var_serialize(vm, &mut buf, &members, ctx)?;
    Ok(buf)
}

fn sos_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let data = args.str(0).clone();
    if data.is_empty() {
        return Ok(Value::Null);
    }
    let ctx = var_unserialize_init(vm);
    match unserialize_storage(vm, &this, &ctx, data.as_bytes()) {
        Ok(Ok(())) => {
            var_unserialize_destroy(vm, ctx)?;
            Ok(Value::Null)
        }
        Ok(Err(offset)) => {
            var_unserialize_destroy(vm, ctx)?;
            Err(vm.throw_error(
                "UnexpectedValueException",
                format!("Error at offset {offset} of {} bytes", data.len()),
            ))
        }
        Err(e) => {
            let _ = var_unserialize_destroy(vm, ctx);
            Err(e)
        }
    }
}

/// The parser of `SplObjectStorage::unserialize()`: `Ok(Err(offset))` is a
/// format error detected at `offset` (php-src's `outexcept`).
fn unserialize_storage(
    vm: &mut Vm,
    this: &ObjRef,
    ctx: &Rc<RefCell<UnserializeData>>,
    buf: &[u8],
) -> PResult<Result<(), usize>> {
    // `*p` of the NUL-terminated buffer.
    let at = |p: usize| buf.get(p).copied().unwrap_or(0);
    let mut p = 0usize;
    if at(p) != b'x' {
        return Ok(Err(p));
    }
    p += 1;
    if at(p) != b':' {
        return Ok(Err(p));
    }
    p += 1;

    let count = match var_unserialize(vm, ctx, buf, &mut p)?.map(Value::into_unref) {
        Some(Value::Int(n)) => n,
        _ => return Ok(Err(p)),
    };
    p -= 1; // back to the ';'
    if count < 0 {
        return Ok(Err(p));
    }

    for _ in 0..count {
        if at(p) != b';' {
            return Ok(Err(p));
        }
        p += 1;
        if !matches!(at(p), b'O' | b'C' | b'r') {
            return Ok(Err(p));
        }
        // Entries stay in the shared table, so later elements can refer
        // back to earlier objects and infos.
        let Some(entry) = var_unserialize(vm, ctx, buf, &mut p)? else {
            return Ok(Err(p));
        };
        let mut inf = None;
        if at(p) == b',' {
            // The current format carries the info.
            p += 1;
            match var_unserialize(vm, ctx, buf, &mut p)? {
                Some(v) => inf = Some(v),
                None => return Ok(Err(p)),
            }
        }
        let Value::Object(obj) = entry.into_unref() else {
            return Ok(Err(p));
        };
        // php-src hashes once to keep a replaced element alive, then
        // attaches (which hashes again).
        get_hash(vm, this, &obj)?;
        attach(vm, this, &obj, inf)?;
    }

    if at(p) != b';' {
        return Ok(Err(p));
    }
    p += 1;
    if at(p) != b'm' {
        return Ok(Err(p));
    }
    p += 1;
    if at(p) != b':' {
        return Ok(Err(p));
    }
    p += 1;
    let members = match var_unserialize(vm, ctx, buf, &mut p)?.map(Value::into_unref) {
        Some(Value::Array(a)) => a,
        _ => return Ok(Err(p)),
    };
    object_properties_load(vm, this, &members)?;
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------
// __serialize / __unserialize / __debugInfo
// ---------------------------------------------------------------------------

fn sos_magic_serialize(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let flat: Vec<Value> = with_state(&this, |s| {
        s.table
            .elements()
            .flat_map(|e| [Value::Object(e.obj.clone()), e.inf.clone()])
            .collect()
    });
    let members = proptable_to_symtable(&engine::std_properties(&this));
    Ok(Value::array(PhpArray::from_values(vec![
        Value::array(PhpArray::from_values(flat)),
        Value::array(members),
    ])))
}

fn sos_magic_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let data = args.array(0).clone();
    // The raw zvals are checked: a reference is not an array.
    let (Some(Value::Array(storage)), Some(Value::Array(members))) =
        (data.get_int(0).cloned(), data.get_int(1).cloned())
    else {
        return Err(vm.throw_error(
            "UnexpectedValueException",
            "Incomplete or ill-typed serialization data",
        ));
    };
    if storage.len() % 2 != 0 {
        return Err(vm.throw_error("UnexpectedValueException", "Odd number of elements"));
    }
    let mut key: Option<Value> = None;
    for val in storage.values() {
        match key.take() {
            Some(Value::Object(obj)) => attach(vm, &this, &obj, Some(val.unref()))?,
            Some(_) => {
                return Err(vm.throw_error("UnexpectedValueException", "Non-object key"));
            }
            None => key = Some(val.clone()),
        }
    }
    object_properties_load(vm, &this, &members)?;
    Ok(Value::Null)
}

/// `SplObjectStorage::__debugInfo()` (also `MultipleIterator::__debugInfo()`).
pub fn sos_debug_info(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(storage::debug_info(&this(args))))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub static ATTACH: NativeFunction = NativeFunction {
    sig: "public function attach(object $object, mixed $info = null): void {}",
    handler: sos_attach,
};
pub static DETACH: NativeFunction = NativeFunction {
    sig: "public function detach(object $object): void {}",
    handler: sos_detach,
};
pub static CONTAINS: NativeFunction = NativeFunction {
    sig: "public function contains(object $object): bool {}",
    handler: sos_contains,
};
pub static ADD_ALL: NativeFunction = NativeFunction {
    sig: "public function addAll(SplObjectStorage $storage): int {}",
    handler: sos_add_all,
};
pub static REMOVE_ALL: NativeFunction = NativeFunction {
    sig: "public function removeAll(SplObjectStorage $storage): int {}",
    handler: sos_remove_all,
};
pub static REMOVE_ALL_EXCEPT: NativeFunction = NativeFunction {
    sig: "public function removeAllExcept(SplObjectStorage $storage): int {}",
    handler: sos_remove_all_except,
};
pub static GET_INFO: NativeFunction = NativeFunction {
    sig: "public function getInfo(): mixed {}",
    handler: sos_get_info,
};
pub static SET_INFO: NativeFunction = NativeFunction {
    sig: "public function setInfo(mixed $info): void {}",
    handler: sos_set_info,
};
pub static COUNT: NativeFunction = NativeFunction {
    sig: "public function count(int $mode = COUNT_NORMAL): int {}",
    handler: sos_count,
};
pub static REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: sos_rewind,
};
pub static VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: sos_valid,
};
pub static KEY: NativeFunction = NativeFunction {
    sig: "public function key(): int {}",
    handler: sos_key,
};
pub static CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): object {}",
    handler: sos_current,
};
pub static NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: sos_next,
};
pub static SEEK: NativeFunction = NativeFunction {
    sig: "public function seek(int $offset): void {}",
    handler: sos_seek,
};
pub static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function unserialize(string $data): void {}",
    handler: sos_unserialize,
};
pub static SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function serialize(): string {}",
    handler: sos_serialize,
};
pub static OFFSET_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists($object): bool {}",
    handler: contains_impl,
};
pub static OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($object): mixed {}",
    handler: sos_offset_get,
};
pub static OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet($object, mixed $info = null): void {}",
    handler: attach_impl,
};
pub static OFFSET_UNSET: NativeFunction = NativeFunction {
    sig: "public function offsetUnset($object): void {}",
    handler: detach_impl,
};
pub static GET_HASH: NativeFunction = NativeFunction {
    sig: "public function getHash(object $object): string {}",
    handler: sos_get_hash,
};
pub static MAGIC_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: sos_magic_serialize,
};
pub static MAGIC_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __unserialize(array $data): void {}",
    handler: sos_magic_unserialize,
};
pub static DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: sos_debug_info,
};

/// `register_class_SplObjectStorage` + the handlers of `PHP_MINIT_FUNCTION(spl_observer)`.
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(SPL_OBJECT_STORAGE, 0);
    for iface in [
        "Countable",
        "SeekableIterator",
        "Serializable",
        "ArrayAccess",
    ] {
        let iface = engine::class(r, iface);
        engine::implement(&mut ce, &iface);
    }
    // Stub order; attach/detach/contains carry #[\Deprecated].
    let methods: [(&'static NativeFunction, bool); 25] = [
        (&ATTACH, true),
        (&DETACH, true),
        (&CONTAINS, true),
        (&ADD_ALL, false),
        (&REMOVE_ALL, false),
        (&REMOVE_ALL_EXCEPT, false),
        (&GET_INFO, false),
        (&SET_INFO, false),
        (&COUNT, false),
        (&REWIND, false),
        (&VALID, false),
        (&KEY, false),
        (&CURRENT, false),
        (&NEXT, false),
        (&SEEK, false),
        (&UNSERIALIZE, false),
        (&SERIALIZE, false),
        (&OFFSET_EXISTS, false),
        (&OFFSET_GET, false),
        (&OFFSET_SET, false),
        (&OFFSET_UNSET, false),
        (&GET_HASH, false),
        (&MAGIC_SERIALIZE, false),
        (&MAGIC_UNSERIALIZE, false),
        (&DEBUG_INFO, false),
    ];
    for (nf, deprecated) in methods {
        let mut flags = method_flags(nf.sig);
        if deprecated {
            flags |= fn_flags::DEPRECATED;
        }
        ce.add_method(Function::native_method(nf, SPL_OBJECT_STORAGE, flags));
    }
    ce.handlers.create_object = Some(storage::create_object);
    ce.handlers.clone_obj = Some(storage::clone_obj);
    ce.handlers.compare = Some(storage::compare);
    ce.handlers.count_elements = Some(count_elements);
    ce.handlers.read_dimension = Some(storage::read_dimension);
    ce.handlers.write_dimension = Some(storage::write_dimension);
    ce.handlers.has_dimension = Some(storage::has_dimension);
    ce.handlers.unset_dimension = Some(storage::unset_dimension);
    r.class(ce);
}
