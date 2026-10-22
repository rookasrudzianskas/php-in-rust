//! ext/spl/spl_fixedarray.c — `SplFixedArray`.
//!
//! The elements live in a [`FixedArray`] stored as the object's native
//! state (created by the class's `create_object` handler, so user
//! subclasses get it too). Releasing elements (`setSize()` shrinking,
//! overwriting, unsetting) runs the destructors of freed objects before the
//! operation continues, like `zval_ptr_dtor()` does; a destructor that
//! resizes the array while it is being resized has its size remembered and
//! applied afterwards (`cached_resize`, GH-81992 / GH-23811).
//!
//! Sub-modules: [`internal_iterator`] (`ObjectIterator`, the
//! `InternalIterator` class returned by `getIterator()`) and [`support`]
//! (engine helpers shared with `dllist` and `heap`).
//!
//! # Interpreter contract
//! Object handlers (inherited by user subclasses):
//! * `read_dimension(obj, offset, fetch)` — `$a[$i]` in every fetch mode;
//!   `offset` is `None` for `$a[]`. Returns the element (a copy: writes
//!   through it never reach the array; the interpreter emits php-src's
//!   "Indirect modification of overloaded element of %s has no effect"
//!   notice for non-object results of `Write`/`ReadWrite` fetches). For
//!   `DimFetch::IsSet` it returns `null` when the offset does not exist
//!   (`??`). User `offsetGet()`/`offsetExists()` overrides are called.
//! * `write_dimension(obj, offset, value)` — `$a[$i] = $v` (`None` for
//!   `$a[] = $v`, an Error unless `offsetSet()` is overridden).
//! * `has_dimension(obj, offset, check_empty)` — `isset($a[$i])` is
//!   `has_dimension(.., false)`, `empty($a[$i])` is `!has_dimension(.., true)`.
//! * `unset_dimension(obj, offset)` — `unset($a[$i])`.
//! * `count_elements` — `count($a)` (calls a user `count()` override);
//!   never `Ok(None)`.
//! * `get_properties_for(obj, purpose)` — every purpose including
//!   `Debug`: `var_dump()`/`print_r()` must ask `get_properties_for(Debug)`
//!   before falling back to `get_debug_info`/`__debugInfo()`
//!   (`zend_get_properties_for`), so a `__debugInfo()` of a subclass is
//!   ignored like in php-src. Elements come first (integer keys), then the
//!   object's properties (mangled keys); `GetObjectVars` omits the elements.
//! * `clone_obj` copies the elements and the properties; the interpreter
//!   then calls `__clone()`.
//!
//! `foreach ($fixedArray as ...)` goes through `IteratorAggregate::getIterator()`
//! (an `InternalIterator` over [`get_iterator`]), which is what php-src's
//! `get_iterator` handler iterates too. `foreach` by reference must throw
//! "An iterator cannot be used with foreach by reference" (php-src's
//! `spl_fixedarray_get_iterator`): the interpreter can call
//! [`get_iterator`] with `by_ref = true` to get that error.

pub use crate::internal_iterator;
pub mod support;

#[cfg(test)]
pub(crate) mod test_helpers;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, DimFetch, Function, PropPurpose, fn_flags};
use php_runtime::value::array::ArrayKey;
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, Registry, Value, Vm,
};

use self::internal_iterator::{ObjectIterator, by_ref_error, create_internal_iterator};
use self::support::{
    check_allocation, finish, object_properties_load, release, release_value, zval_get_long,
};
use super::engine::{self, is_overridden};

pub const SPL_FIXED_ARRAY: &str = "SplFixedArray";

/// `sizeof(zval)`, for allocation-size messages.
const ZVAL_SIZE: u64 = 16;

/// `spl_fixedarray` plus the `fptr_count` of `spl_fixedarray_object`.
pub struct FixedArray {
    elements: Vec<Value>,
    /// The size requested by a re-entrant resize while a resize runs; `-1`
    /// when no resize is in progress.
    cached_resize: i64,
    /// A user subclass overrides `count()`.
    fptr_count: bool,
}

impl FixedArray {
    fn new(fptr_count: bool) -> FixedArray {
        FixedArray {
            elements: Vec::new(),
            cached_resize: -1,
            fptr_count,
        }
    }

    fn resize_in_progress(&self) -> bool {
        self.cached_resize >= 0
    }
}

fn state(obj: &ObjRef) -> std::cell::RefMut<'_, FixedArray> {
    obj.native_mut::<FixedArray>()
        .unwrap_or_else(|| panic!("{} without SplFixedArray state", obj.class().name))
}

/// The number of elements of an `SplFixedArray` (`intern->array.size`).
pub fn size(obj: &ObjRef) -> i64 {
    state(obj).elements.len() as i64
}

/// A copy of the elements of an `SplFixedArray`.
pub fn elements(obj: &ObjRef) -> Vec<Value> {
    state(obj).elements.clone()
}

// ---------------------------------------------------------------------------
// Object handlers
// ---------------------------------------------------------------------------

fn create_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    let fptr_count = is_overridden(ce, b"count", SPL_FIXED_ARRAY);
    obj.set_native(FixedArray::new(fptr_count));
    Ok(obj)
}

/// `spl_fixedarray_object_clone`.
fn clone_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = vm.instantiate(&ce)?;
    let elements = state(old).elements.clone();
    check_allocation(vm, elements.len() as u64, ZVAL_SIZE)?;
    state(&new).elements = elements;
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    Ok(new)
}

/// `spl_offset_convert_to_ulong`: the index of an offset; negative indexes
/// wrap to huge values (always out of range).
fn offset_to_index(vm: &mut Vm, offset: &Value) -> PResult<u64> {
    match offset {
        Value::Int(i) => Ok(*i as u64),
        Value::String(s) => match ArrayKey::parse_numeric(s.as_bytes()) {
            Some(i) => Ok(i as u64),
            None => Err(illegal_offset(vm, offset)),
        },
        Value::Float(d) => Ok(vm.dval_to_lval_safe(*d)? as u64),
        Value::Bool(b) => Ok(*b as u64),
        Value::Ref(r) => {
            let inner = r.get();
            offset_to_index(vm, &inner)
        }
        Value::Resource(r) => {
            let id = r.id();
            vm.engine_warning(format!(
                "Resource ID#{id} used as offset, casting to integer ({id})"
            ))?;
            Ok(id as u64)
        }
        _ => Err(illegal_offset(vm, offset)),
    }
}

/// `zend_illegal_container_offset(spl_ce_SplFixedArray->name, offset, BP_VAR_R)`.
fn illegal_offset(vm: &mut Vm, offset: &Value) -> Flow {
    vm.throw_type_error(format!(
        "Cannot access offset of type {} on SplFixedArray",
        offset.type_name()
    ))
}

fn out_of_range(vm: &mut Vm) -> Flow {
    vm.throw_error("OutOfBoundsException", "Index invalid or out of range")
}

fn no_append(vm: &mut Vm) -> Flow {
    vm.throw_error("Error", "[] operator not supported for SplFixedArray")
}

/// `spl_fixedarray_object_read_dimension_helper`.
fn read_helper(vm: &mut Vm, obj: &ObjRef, offset: Option<&Value>) -> PResult<Value> {
    let Some(offset) = offset else {
        return Err(no_append(vm));
    };
    let index = offset_to_index(vm, offset)?;
    let st = state(obj);
    match st.elements.get(index as usize) {
        Some(v) => Ok(v.clone()),
        None => {
            drop(st);
            Err(out_of_range(vm))
        }
    }
}

/// `spl_fixedarray_object_write_dimension_helper`.
fn write_helper(vm: &mut Vm, obj: &ObjRef, offset: Option<&Value>, value: Value) -> PResult<()> {
    let Some(offset) = offset else {
        return Err(no_append(vm));
    };
    let index = offset_to_index(vm, offset)?;
    let old = {
        let mut st = state(obj);
        match st.elements.get_mut(index as usize) {
            Some(slot) => std::mem::replace(slot, value.into_unref()),
            None => {
                drop(st);
                return Err(out_of_range(vm));
            }
        }
    };
    // zend_safe_assign_to_variable_noref(): the old value is released last.
    release_value(vm, old)
}

/// `spl_fixedarray_object_unset_dimension_helper`.
fn unset_helper(vm: &mut Vm, obj: &ObjRef, offset: &Value) -> PResult<()> {
    let index = offset_to_index(vm, offset)?;
    let old = {
        let mut st = state(obj);
        match st.elements.get_mut(index as usize) {
            Some(slot) => std::mem::replace(slot, Value::Null),
            None => {
                drop(st);
                return Err(out_of_range(vm));
            }
        }
    };
    release_value(vm, old)
}

/// `spl_fixedarray_object_has_dimension_helper`.
fn has_helper(vm: &mut Vm, obj: &ObjRef, offset: &Value, check_empty: bool) -> PResult<bool> {
    let index = offset_to_index(vm, offset)?;
    let v = match state(obj).elements.get(index as usize) {
        Some(v) => v.clone(),
        None => return Ok(false),
    };
    if check_empty {
        return vm.to_bool(&v);
    }
    Ok(!v.unref().is_null())
}

/// `HAS_FIXEDARRAY_ARRAYACCESS_OVERRIDE`.
fn overrides(obj: &ObjRef, method: &[u8]) -> bool {
    is_overridden(obj.class(), method, SPL_FIXED_ARRAY)
}

/// `spl_fixedarray_object_read_dimension`.
fn read_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    fetch: DimFetch,
) -> PResult<Value> {
    if fetch == DimFetch::IsSet {
        let probe = offset.cloned().unwrap_or(Value::Null);
        if !has_dimension(vm, obj, &probe, false)? {
            return Ok(Value::Null);
        }
    }
    if overrides(obj, b"offsetget") {
        let offset = offset.cloned().unwrap_or(Value::Null);
        return vm.call_method(obj, b"offsetGet", vec![offset]);
    }
    read_helper(vm, obj, offset)
}

/// `spl_fixedarray_object_write_dimension`.
fn write_dimension(vm: &mut Vm, obj: &ObjRef, offset: Option<&Value>, value: Value) -> PResult<()> {
    if overrides(obj, b"offsetset") {
        let offset = offset.cloned().unwrap_or(Value::Null);
        vm.call_method(obj, b"offsetSet", vec![offset, value])?;
        return Ok(());
    }
    write_helper(vm, obj, offset, value)
}

/// `spl_fixedarray_object_unset_dimension`.
fn unset_dimension(vm: &mut Vm, obj: &ObjRef, offset: &Value) -> PResult<()> {
    if overrides(obj, b"offsetunset") {
        vm.call_method(obj, b"offsetUnset", vec![offset.clone()])?;
        return Ok(());
    }
    unset_helper(vm, obj, offset)
}

/// `spl_fixedarray_object_has_dimension`.
fn has_dimension(vm: &mut Vm, obj: &ObjRef, offset: &Value, check_empty: bool) -> PResult<bool> {
    if overrides(obj, b"offsetexists") {
        let r = vm.call_method(obj, b"offsetExists", vec![offset.clone()])?;
        return vm.to_bool(&r);
    }
    has_helper(vm, obj, offset, check_empty)
}

/// `spl_fixedarray_object_count_elements`.
fn count_elements(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<i64>> {
    let (fptr_count, size) = {
        let st = state(obj);
        (st.fptr_count, st.elements.len() as i64)
    };
    if fptr_count {
        let rv = vm.call_method(obj, b"count", Vec::new())?;
        return Ok(Some(zval_get_long(vm, &rv)?));
    }
    Ok(Some(size))
}

/// `spl_fixedarray_object_get_properties_for`.
fn get_properties_for(
    _vm: &mut Vm,
    obj: &ObjRef,
    purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    let props = engine::std_properties(obj);
    let elements = state(obj).elements.clone();
    let mut ht = PhpArray::with_capacity(elements.len() + props.len());
    if purpose != PropPurpose::GetObjectVars {
        for v in elements {
            let _ = ht.append(v);
        }
    }
    for (k, v) in props.iter() {
        ht.insert(k, v.clone());
    }
    Ok(Some(ht))
}

// ---------------------------------------------------------------------------
// Sizing
// ---------------------------------------------------------------------------

/// `spl_fixedarray_init` on an empty array.
fn init(vm: &mut Vm, obj: &ObjRef, size: i64) -> PResult<()> {
    if size > 0 {
        check_allocation(vm, size as u64, ZVAL_SIZE)?;
    }
    let mut st = state(obj);
    st.elements = vec![Value::Null; size.max(0) as usize];
    st.cached_resize = -1;
    Ok(())
}

/// `spl_fixedarray_resize`.
fn resize(vm: &mut Vm, obj: &ObjRef, size: i64) -> PResult<()> {
    let (cur, in_progress) = {
        let st = state(obj);
        (st.elements.len() as i64, st.resize_in_progress())
    };
    if size == cur {
        return Ok(());
    }
    if in_progress {
        // Already resizing: remember the desired size, applied later.
        state(obj).cached_resize = size;
        return Ok(());
    }
    if cur == 0 {
        return init(vm, obj, size);
    }
    if size > cur {
        check_allocation(vm, size as u64, ZVAL_SIZE)?;
    }
    let mut pending = None;
    let removed = {
        let mut st = state(obj);
        st.cached_resize = size;
        if size == 0 {
            // spl_fixedarray_dtor(): cleared first, destroyed last to first.
            let mut all = std::mem::take(&mut st.elements);
            all.reverse();
            all
        } else if size > cur {
            st.elements.resize(size as usize, Value::Null);
            Vec::new()
        } else {
            // spl_fixedarray_dtor_range(): the size shrinks first.
            st.elements.split_off(size as usize)
        }
    };
    release(vm, removed, &mut pending);
    let cached = {
        let mut st = state(obj);
        let cached = st.cached_resize;
        st.cached_resize = -1;
        cached
    };
    if cached != size
        && let Err(e) = resize(vm, obj, cached)
    {
        support::merge_error(&mut pending, e);
    }
    finish(pending)
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

/// `spl_fixedarray_it`.
struct FixedArrayIterator {
    obj: ObjRef,
    current: Cell<i64>,
}

impl ObjectIterator for FixedArrayIterator {
    fn valid(&self, _vm: &mut Vm) -> PResult<bool> {
        let c = self.current.get();
        Ok(c >= 0 && c < size(&self.obj))
    }

    fn current(&self, vm: &mut Vm) -> PResult<Option<Value>> {
        read_helper(vm, &self.obj, Some(&Value::Int(self.current.get()))).map(Some)
    }

    fn key(&self, _vm: &mut Vm) -> PResult<Option<Value>> {
        Ok(Some(Value::Int(self.current.get())))
    }

    fn move_forward(&self, _vm: &mut Vm) -> PResult<()> {
        self.current.set(self.current.get() + 1);
        Ok(())
    }

    fn rewind(&self, _vm: &mut Vm) -> PResult<()> {
        self.current.set(0);
        Ok(())
    }
}

/// `spl_fixedarray_get_iterator`: the internal iterator over an
/// `SplFixedArray` (what `foreach` walks).
pub fn get_iterator(vm: &mut Vm, obj: &ObjRef, by_ref: bool) -> PResult<Rc<dyn ObjectIterator>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    Ok(Rc::new(FixedArrayIterator {
        obj: obj.clone(),
        current: Cell::new(0),
    }))
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn this(args: &Args) -> ObjRef {
    engine::this(args)
}

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let size = args.int(0);
    if size < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    let this = this(args);
    let (empty, in_progress) = {
        let st = state(&this);
        (st.elements.is_empty(), st.resize_in_progress())
    };
    if !empty || in_progress {
        // __construct() called twice: bail out.
        return Ok(Value::Null);
    }
    init(vm, &this, size)?;
    Ok(Value::Null)
}

fn wakeup(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // #[\Deprecated(since: '8.4', message: '...')] (zend_deprecated_function).
    vm.engine_deprecated(
        "Method SplFixedArray::__wakeup() is deprecated since 8.4, this method is obsolete, \
         as serialization hooks are provided by __unserialize() and __serialize()",
    )?;
    let this = this(args);
    let (empty, in_progress) = {
        let st = state(&this);
        (st.elements.is_empty(), st.resize_in_progress())
    };
    if empty && !in_progress {
        let props: Vec<Value> = engine::std_properties(&this).values().cloned().collect();
        init(vm, &this, props.len() as i64)?;
        state(&this).elements = props;
        // Remove the unserialised properties: they are the elements now.
        let old = this.obj().dyn_props.borrow_mut().take();
        drop(old);
        let mut pending = None;
        support::run_destructors(vm, &mut pending);
        finish(pending)?;
    }
    Ok(Value::Null)
}

fn serialize(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let elements = state(&this).elements.clone();
    let props = engine::std_properties(&this);
    let mut out = PhpArray::with_capacity(elements.len() + props.len());
    for v in elements {
        let _ = out.append(v);
    }
    for (k, v) in props.iter() {
        // Only string keys: integer keys would be array elements.
        if matches!(k, ArrayKey::Str(_)) && !out.contains_key(&k) {
            out.insert(k, v.clone());
        }
    }
    Ok(Value::array(out))
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let data = args.array(0).clone();
    let (empty, in_progress) = {
        let st = state(&this);
        (st.elements.is_empty(), st.resize_in_progress())
    };
    if !empty || in_progress {
        return Ok(Value::Null);
    }
    let size = data.len();
    check_allocation(vm, size as u64, ZVAL_SIZE)?;
    if size == 0 {
        return Ok(Value::Null);
    }
    let mut elements = Vec::with_capacity(size);
    let mut members = PhpArray::new();
    for (k, v) in data.iter() {
        match k {
            ArrayKey::Int(_) => elements.push(v.unref()),
            ArrayKey::Str(_) => {
                members.insert(k, v.clone());
            }
        }
    }
    {
        let mut st = state(&this);
        st.elements = elements;
        st.cached_resize = -1;
    }
    object_properties_load(vm, &this, &members, SPL_FIXED_ARRAY)?;
    Ok(Value::Null)
}

fn count(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(size(&this(args))))
}

fn to_array(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let elements = state(&this(args)).elements.clone();
    Ok(Value::array(PhpArray::from_values(elements)))
}

fn invalid_argument(vm: &mut Vm, msg: &str) -> Flow {
    vm.throw_error("InvalidArgumentException", msg)
}

fn from_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = args.array(0).clone();
    let save_indexes = args.bool(1);
    let num = data.len();
    let elements = if num > 0 && save_indexes {
        let mut max_index: u64 = 0;
        for k in data.keys() {
            match k {
                ArrayKey::Int(i) if i >= 0 => max_index = max_index.max(i as u64),
                _ => {
                    return Err(invalid_argument(
                        vm,
                        "array must contain only positive integer keys",
                    ));
                }
            }
        }
        let size = (max_index as i64).wrapping_add(1);
        if size <= 0 {
            return Err(invalid_argument(vm, "integer overflow detected"));
        }
        check_allocation(vm, size as u64, ZVAL_SIZE)?;
        let mut elements = vec![Value::Null; size as usize];
        for (k, v) in data.iter() {
            if let ArrayKey::Int(i) = k {
                elements[i as usize] = v.unref();
            }
        }
        elements
    } else if num > 0 {
        check_allocation(vm, num as u64, ZVAL_SIZE)?;
        data.values().map(Value::unref).collect()
    } else {
        Vec::new()
    };
    let ce = fixed_array_class(vm);
    let obj = vm.instantiate(&ce)?;
    state(&obj).elements = elements;
    Ok(Value::Object(obj))
}

fn fixed_array_class(vm: &Vm) -> ClassRef {
    vm.find_class(SPL_FIXED_ARRAY.as_bytes())
        .expect("SplFixedArray is registered")
}

fn get_size(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(size(&this(args))))
}

fn set_size(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let size = args.int(0);
    if size < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    resize(vm, &this(args), size)?;
    Ok(Value::Bool(true))
}

fn offset_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = args.get(0).clone();
    Ok(Value::Bool(has_helper(vm, &this(args), &index, false)?))
}

fn offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = args.get(0).clone();
    read_helper(vm, &this(args), Some(&index))
}

fn offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = args.get(0).clone();
    let value = args.get(1).clone();
    write_helper(vm, &this(args), Some(&index), value)?;
    Ok(Value::Null)
}

fn offset_unset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = args.get(0).clone();
    unset_helper(vm, &this(args), &index)?;
    Ok(Value::Null)
}

fn get_iterator_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // zend_create_internal_iterator_zval(): the scope's get_iterator.
    let iter = get_iterator(vm, &this(args), false)?;
    Ok(Value::Object(create_internal_iterator(vm, iter)?))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

static CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(int $size = 0) {}",
    handler: construct,
};
static WAKEUP: NativeFunction = NativeFunction {
    sig: "public function __wakeup(): void {}",
    handler: wakeup,
};
static SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: serialize,
};
static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __unserialize(array $data): void {}",
    handler: unserialize,
};
static COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: count,
};
static TO_ARRAY: NativeFunction = NativeFunction {
    sig: "public function toArray(): array {}",
    handler: to_array,
};
static FROM_ARRAY: NativeFunction = NativeFunction {
    sig: "public static function fromArray(array $array, bool $preserveKeys = true): SplFixedArray {}",
    handler: from_array,
};
static GET_SIZE: NativeFunction = NativeFunction {
    sig: "public function getSize(): int {}",
    handler: get_size,
};
static SET_SIZE: NativeFunction = NativeFunction {
    sig: "public function setSize(int $size): true {}",
    handler: set_size,
};
static OFFSET_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists($index): bool {}",
    handler: offset_exists,
};
static OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($index): mixed {}",
    handler: offset_get,
};
static OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet($index, mixed $value): void {}",
    handler: offset_set,
};
static OFFSET_UNSET: NativeFunction = NativeFunction {
    sig: "public function offsetUnset($index): void {}",
    handler: offset_unset,
};
static GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getIterator(): Iterator {}",
    handler: get_iterator_method,
};
/// `@implementation-alias SplFixedArray::toArray`.
static JSON_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function jsonSerialize(): array {}",
    handler: to_array,
};

/// `PHP_MINIT_FUNCTION(spl_fixedarray)` (plus `InternalIterator` when the
/// runtime lacks it).
pub fn register(r: &mut Registry) {
    internal_iterator::register(r);

    let mut ce = ClassEntry::new(SPL_FIXED_ARRAY, 0);
    for iface in [
        "IteratorAggregate",
        "ArrayAccess",
        "Countable",
        "JsonSerializable",
    ] {
        let iface = engine::class(r, iface);
        engine::implement(&mut ce, &iface);
    }
    engine::add_methods(
        &mut ce,
        &[
            &CONSTRUCT,
            &SERIALIZE,
            &UNSERIALIZE,
            &COUNT,
            &TO_ARRAY,
            &FROM_ARRAY,
            &GET_SIZE,
            &SET_SIZE,
            &OFFSET_EXISTS,
            &OFFSET_GET,
            &OFFSET_SET,
            &OFFSET_UNSET,
            &GET_ITERATOR,
            &JSON_SERIALIZE,
        ],
    );
    // #[\Deprecated] __wakeup()
    ce.add_method(Function::native_method(
        &WAKEUP,
        SPL_FIXED_ARRAY,
        engine::method_flags(WAKEUP.sig) | fn_flags::DEPRECATED,
    ));
    let h = &mut ce.handlers;
    h.create_object = Some(create_object);
    h.clone_obj = Some(clone_object);
    h.read_dimension = Some(read_dimension);
    h.write_dimension = Some(write_dimension);
    h.unset_dimension = Some(unset_dimension);
    h.has_dimension = Some(has_dimension);
    h.count_elements = Some(count_elements);
    h.get_properties_for = Some(get_properties_for);
    r.class(ce);
}
