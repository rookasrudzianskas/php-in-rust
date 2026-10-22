//! ext/spl/spl_array.c — `ArrayObject`, `ArrayIterator` and
//! `RecursiveArrayIterator`.
//!
//! # State
//! Every instance (user subclasses included, through the inherited
//! `create_object` handler) carries a [`SplArray`] in
//! [`php_runtime::ObjRef::native`]: the storage (`intern->array`: an array,
//! another SPL array object for `SPL_ARRAY_USE_OTHER`, a plain object whose
//! property table is used, or nothing for `SPL_ARRAY_IS_SELF`), the flags,
//! the iterator position, the sort guard (`nApplyCount`), the iterator class
//! of `getIterator()` and which `ArrayAccess`/`Countable` methods a user
//! subclass overrides (php-src's `fptr_*`). The backing table is resolved by
//! `table::resolve` (`spl_array_get_hash_table`); property tables are built on
//! `PhpObject::props` / `PhpObject::dyn_props` with mangled keys for
//! non-public declared properties.
//!
//! # Object handlers (return-value semantics)
//! * `read_dimension(obj, offset, fetch)` (`$o[k]`, `$o[]` = `None`):
//!   * `Read` / `IsSet`: the element value (never a reference); `null` when
//!     missing (after the "Undefined array key" warning for `Read`).
//!   * `Write` / `ReadWrite`: `Value::Ref(r)` for a writable element (the
//!     element itself becomes a reference, php-src's `ZVAL_NEW_REF`; write
//!     through `r`); `Value::Undef` stands for `&EG(uninitialized_zval)` (no
//!     element; the interpreter uses a temporary `null` without notice). Any
//!     other value (returned by a user `offsetGet()`) is not writable: the
//!     interpreter emits "Indirect modification of overloaded element of %s
//!     has no effect" unless it is an object, exactly as for
//!     `zend_fetch_dimension_address_W` on objects.
//! * `write_dimension(obj, offset, value)` (`None` = `$o[] = v`),
//!   `has_dimension(obj, offset, check_empty)` (`isset` = `false`, `empty` =
//!   `true`), `unset_dimension(obj, offset)`: php-src's handlers, including
//!   the calls to user `offsetGet/offsetSet/offsetExists/offsetUnset`
//!   overrides.
//! * `read_property(obj, name)`: `Ok(None)` = use the standard handler;
//!   `Some(v)` = the value read through `ArrayObject::ARRAY_AS_PROPS`
//!   (`Read` fetch; use [`read_property_fetch`] for `isset`-style fetches
//!   such as `??`).
//! * `write_property(obj, name, value)`: `Ok(true)` = handled (stored as an
//!   element), `Ok(false)` = use the standard handler.
//! * `count_elements`: `Some(n)`, or `None` when a user `count()` returned
//!   nothing (FAILURE: `count()` then calls `Countable::count()` itself).
//! * `compare`: `spl_array_compare_objects`.
//! * `get_properties_for`: `None` = standard table (also for every purpose
//!   when `STD_PROP_LIST` is set); the storage for `ArrayCast`, `VarExport`
//!   and `Json`.
//! * `get_debug_info`: the table `__debugInfo()` returns (a user override of
//!   `__debugInfo()` is called instead).
//! * `clone_obj`: copies the properties and the state; the interpreter runs
//!   `__clone()` afterwards.
//!
//! # Hooks the interpreter must call (no runtime hook exists)
//! * [`has_property`] — `has_property` handler (`isset($o->p)`,
//!   `empty($o->p)`, `property_exists()`); `Ok(None)` = standard handler.
//! * [`unset_property`] — `unset($o->p)`; `Ok(false)` = standard handler.
//! * [`read_property_fetch`] — `read_property` with the fetch mode
//!   (`DimFetch::IsSet` for `??` / `isset` fetches).
//! * [`property_ptr`] — `get_property_ptr_ptr` (`$o->p[] = 1`, `$o->p .= x`,
//!   `$r = &$o->p`).
//! * [`current_ref`] — by-reference `foreach` over an `ArrayIterator` (or
//!   the iterator returned by `ArrayObject::getIterator()`): when the
//!   iterator class does not override any `Iterator` method, php-src uses
//!   `spl_array_get_iterator`, whose `get_current_data` returns the element
//!   itself; the interpreter must drive `rewind()/valid()/key()/next()` as
//!   usual and bind the loop variable to the reference returned here
//!   instead of calling `current()`. (A class overriding an `Iterator`
//!   method cannot be iterated by reference: "An iterator cannot be used
//!   with foreach by reference".)
//! * [`append`] / [`iterator_key`] — `spl_array_iterator_append()` /
//!   `spl_array_iterator_key()` exported to other SPL code.
//!
//! # Deprecations
//! `ArrayIterator`'s sort, flag and `Serializable` methods carry
//! `fn_flags::DEPRECATED`; their handlers emit "Method ArrayIterator::%s()
//! is deprecated since 8.6" (the runtime does not do it for native
//! functions).
//!
//! # Known gaps
//! * php-src parses the arguments of `__construct()`/`exchangeArray()`
//!   (`array|object`, `Z_PARAM_ARRAY_OR_OBJECT`: "must be of type array") and
//!   `uasort()`/`uksort()` (`callable` in the stub, but `"z"` in ZPP, so the
//!   callback is checked by `uasort()`, "uasort(): Argument #2 ($callback)
//!   must be a valid callback") differently from their arginfo. The stubs are
//!   kept verbatim (inheritance checks depend on them), so the runtime's ZPP
//!   messages are reported for those mistakes.
//! * Typed-property reference sources (`ZEND_REF_ADD_TYPE_SOURCE`) are not
//!   tracked; readonly properties still refuse by-reference iteration.

mod handlers;
mod methods;
mod table;
#[cfg(test)]
mod tests;

use php_runtime::class::{
    ClassConstant, ClassEntry, Function, ObjectHandlers, PropertyInfo, fn_flags,
};
use php_runtime::types::{TypeDecl, mask};
use php_runtime::{
    ArrayKey, ClassRef, ObjRef, PResult, PhpArray, PhpRef, Pos, Registry, Value, Vm,
};

use super::engine;
use table::{Found, Ht, resolve, state};

pub use handlers::{PropertyPtr, has_property, property_ptr, read_property_fetch, unset_property};

/// `ArrayObject::STD_PROP_LIST`.
pub const STD_PROP_LIST: u32 = 0x0000_0001;
/// `ArrayObject::ARRAY_AS_PROPS`.
pub const ARRAY_AS_PROPS: u32 = 0x0000_0002;
/// `RecursiveArrayIterator::CHILD_ARRAYS_ONLY`.
pub const CHILD_ARRAYS_ONLY: u32 = 0x0000_0004;
/// The object uses its own property table.
pub(crate) const IS_SELF: u32 = 0x0100_0000;
/// The storage is another SPL array object.
pub(crate) const USE_OTHER: u32 = 0x0200_0000;
/// Internal flags (`SPL_ARRAY_INT_MASK`); user flags are the low 16 bits.
pub(crate) const INT_MASK: u32 = 0xFFFF_0000;
/// Flags kept by clones and serialization (`SPL_ARRAY_CLONE_MASK`).
pub(crate) const CLONE_MASK: u32 = 0x0100_FFFF;

/// Which methods a user subclass overrides (php-src's cached `fptr_*`).
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Overrides {
    pub offset_get: bool,
    pub offset_set: bool,
    pub offset_has: bool,
    pub offset_del: bool,
    pub count: bool,
}

/// `spl_array_object`.
pub struct SplArray {
    /// `intern->array`: `Array`, `Object` (wrapped object, or the other SPL
    /// array object with [`USE_OTHER`]) or `Undef` ([`IS_SELF`]).
    pub(crate) array: Value,
    pub(crate) ar_flags: u32,
    /// `nApplyCount`: sorts in progress.
    pub(crate) apply_count: u32,
    /// `ht_iter`: `None` until the iterator is first used.
    pub(crate) pos: Option<Pos>,
    /// `ce_get_iterator`.
    pub(crate) ce_get_iterator: ClassRef,
    pub(crate) fptr: Overrides,
}

/// The SPL base class (`ArrayObject`, `ArrayIterator` or
/// `RecursiveArrayIterator`) of `ce`, and whether `ce` is a subclass of it.
fn spl_base(ce: &ClassEntry) -> Option<(String, bool)> {
    let mut cur = Some(ce);
    let mut inherited = false;
    while let Some(c) = cur {
        if matches!(
            c.lc_name.as_bytes(),
            b"arrayobject" | b"arrayiterator" | b"recursivearrayiterator"
        ) {
            return Some((c.name.to_string_lossy().into_owned(), inherited));
        }
        cur = c.parent.as_deref();
        inherited = true;
    }
    None
}

/// `spl_array_object_new_ex`: the `fptr_*` of a (sub)class.
pub(crate) fn overrides(ce: &ClassEntry) -> Overrides {
    match spl_base(ce) {
        Some((base, true)) => Overrides {
            offset_get: engine::is_overridden(ce, b"offsetget", &base),
            offset_set: engine::is_overridden(ce, b"offsetset", &base),
            offset_has: engine::is_overridden(ce, b"offsetexists", &base),
            offset_del: engine::is_overridden(ce, b"offsetunset", &base),
            count: engine::is_overridden(ce, b"count", &base),
        },
        _ => Overrides::default(),
    }
}

/// Whether `__debugInfo()` of `ce` is a user override.
pub(crate) fn overrides_debug_info(ce: &ClassEntry) -> bool {
    match spl_base(ce) {
        Some((base, true)) => engine::is_overridden(ce, b"__debuginfo", &base),
        _ => false,
    }
}

impl SplArray {
    fn new(vm: &Vm, ce: &ClassRef) -> SplArray {
        SplArray {
            array: Value::empty_array(),
            ar_flags: 0,
            apply_count: 0,
            pos: None,
            ce_get_iterator: vm
                .find_class(b"ArrayIterator")
                .unwrap_or_else(|| ce.clone()),
            fptr: overrides(ce),
        }
    }
}

/// Whether `obj` is an `ArrayObject`/`ArrayIterator` (uses
/// `spl_handler_ArrayObject`).
pub fn is_spl_array(obj: &ObjRef) -> bool {
    state(obj).is_some()
}

/// `spl_array_object_new`.
fn create_object(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(SplArray::new(vm, ce));
    Ok(obj)
}

/// `zval_add_ref` as used by `zend_objects_clone_members`.
fn copy_member(v: &Value) -> Value {
    match v {
        Value::Ref(r) if r.refcount() == 1 => r.get(),
        v => v.clone(),
    }
}

/// `spl_array_object_clone`: `ArrayObject` clones copy the table,
/// `ArrayIterator` clones share the original's storage (`USE_OTHER`).
fn clone_obj(_vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let (flags, ce_get_iterator) = match state(old) {
        Some(st) => (st.ar_flags & CLONE_MASK, Some(st.ce_get_iterator.clone())),
        None => (0, None),
    };
    let (array, flags) = if flags & IS_SELF != 0 {
        (Value::Undef, flags)
    } else if ce.instance_of_name(b"ArrayObject") {
        (Value::array(resolve(old).to_array()), flags)
    } else {
        (Value::Object(old.clone()), flags | USE_OTHER)
    };
    let props: Vec<Value> = old.obj().props.borrow().iter().map(copy_member).collect();
    let new = ObjRef::with_props(ce.clone(), props);
    let dyn_props = old.obj().dyn_props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = dyn_props;
    new.set_native(SplArray {
        array,
        ar_flags: flags,
        apply_count: 0,
        pos: None,
        ce_get_iterator: match ce_get_iterator {
            Some(c) => c,
            None => ce.clone(),
        },
        fptr: overrides(&ce),
    });
    Ok(new)
}

// ---------------------------------------------------------------------------
// Iterator position (`spl_array_get_pos_ptr` and friends)
// ---------------------------------------------------------------------------

fn set_pos(obj: &ObjRef, pos: Pos) {
    if let Some(mut st) = obj.native_mut::<SplArray>() {
        st.pos = Some(pos);
    }
}

/// `spl_array_get_pos_ptr`: the position, created (and rewound) on first use.
pub(crate) fn get_pos(obj: &ObjRef, ht: &Ht) -> Pos {
    if let Some(p) = state(obj).and_then(|s| s.pos) {
        return p;
    }
    let first = ht.first_pos();
    set_pos(obj, first);
    skip_protected(obj, ht);
    state(obj).and_then(|s| s.pos).unwrap_or(first)
}

/// `spl_array_skip_protected`: in property tables, skips unset declared
/// properties and mangled (non-public) names. `true` = SUCCESS.
pub(crate) fn skip_protected(obj: &ObjRef, ht: &Ht) -> bool {
    if !matches!(ht, Ht::Props(_)) {
        return false;
    }
    let mut pos = get_pos(obj, ht);
    loop {
        let skip = match ht.valid_pos(pos).and_then(|p| ht.entry_at(p)) {
            Some((ArrayKey::Str(k), found)) => {
                matches!(found, Found::Unset) || k.as_bytes().first() == Some(&0)
            }
            _ => return true,
        };
        if !skip {
            return true;
        }
        match ht.move_forward(pos) {
            Some(p) => {
                pos = p;
                set_pos(obj, p);
            }
            None => return false,
        }
    }
}

/// `spl_array_rewind`.
pub(crate) fn rewind(obj: &ObjRef) {
    let ht = resolve(obj);
    let first = ht.first_pos();
    set_pos(obj, first);
    skip_protected(obj, &ht);
}

/// `zend_hash_has_more_elements_ex` at the object's position.
pub(crate) fn valid(obj: &ObjRef, ht: &Ht) -> bool {
    let pos = get_pos(obj, ht);
    ht.valid_pos(pos).is_some()
}

/// `zend_hash_move_forward_ex` on the object's position.
pub(crate) fn move_forward(obj: &ObjRef, ht: &Ht) {
    let pos = get_pos(obj, ht);
    if let Some(p) = ht.move_forward(pos) {
        set_pos(obj, p);
    }
}

/// `spl_array_next_ex`: `true` = SUCCESS.
pub(crate) fn next(obj: &ObjRef, ht: &Ht) -> bool {
    move_forward(obj, ht);
    if matches!(ht, Ht::Props(_)) {
        skip_protected(obj, ht)
    } else {
        valid(obj, ht)
    }
}

/// The current element (`zend_hash_get_current_data_ex`).
pub(crate) fn current_entry(obj: &ObjRef, ht: &Ht) -> Option<(ArrayKey, Found)> {
    let pos = get_pos(obj, ht);
    ht.valid_pos(pos).and_then(|p| ht.entry_at(p))
}

// ---------------------------------------------------------------------------
// Functions exported to the interpreter and other SPL code
// ---------------------------------------------------------------------------

/// `spl_array_iterator_append()`: `$obj->append($value)` without dispatch.
pub fn append(vm: &mut Vm, obj: &ObjRef, value: Value) -> PResult<()> {
    handlers::iterator_append(vm, obj, value)
}

/// `spl_array_iterator_key()`: the key at the iterator position.
pub fn iterator_key(obj: &ObjRef) -> Value {
    let ht = resolve(obj);
    match current_entry(obj, &ht) {
        Some((k, _)) => k.into_value(),
        None => Value::Null,
    }
}

/// `spl_array_it_get_current_data()` with `by_ref`: the current element as
/// a reference (the element becomes a reference, like `ZEND_FE_FETCH_RW`),
/// or `None` past the end. Readonly typed properties of a wrapped object
/// cannot be iterated by reference.
pub fn current_ref(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<PhpRef>> {
    let ht = resolve(obj);
    let pos = get_pos(obj, &ht);
    let Some(p) = ht.valid_pos(pos) else {
        return Ok(None);
    };
    let wraps_plain_object = state(obj).is_some_and(|st| {
        matches!(st.array, Value::Object(_)) && st.ar_flags & (IS_SELF | USE_OTHER) == 0
    });
    if wraps_plain_object
        && let (Ht::Props(target), Some((ArrayKey::Str(key), found))) = (&ht, ht.entry_at(p))
        && !matches!(found, Found::Value(Value::Ref(_)))
        && let Some(d) = table::declared_props(target)
            .into_iter()
            .find(|d| d.key == key)
        && d.info.ty.is_some()
        && d.info.is_readonly()
    {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Cannot acquire reference to readonly property {}::${}",
                d.info.declaring_class, d.info.name
            ),
        ));
    }
    Ok(ht.ref_at(p))
}

/// Debug view of the storage (`spl_array_get_debug_info`).
pub(crate) fn debug_info(obj: &ObjRef) -> PhpArray {
    let mut table = engine::std_properties(obj);
    let storage = match state(obj) {
        Some(st) if st.ar_flags & IS_SELF == 0 => Some(st.array.clone()),
        _ => None,
    };
    if let Some(storage) = storage {
        let base = if obj.class().instance_of_name(b"ArrayIterator") {
            "ArrayIterator"
        } else {
            "ArrayObject"
        };
        engine::set_private_debug_info_property(&mut table, base, "storage", storage);
    }
    table
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

fn handlers() -> ObjectHandlers {
    ObjectHandlers {
        create_object: Some(create_object),
        clone_obj: Some(clone_obj),
        compare: Some(handlers::compare),
        count_elements: Some(handlers::count_elements),
        get_debug_info: Some(handlers::get_debug_info),
        get_properties_for: Some(handlers::get_properties_for),
        read_dimension: Some(handlers::read_dimension),
        write_dimension: Some(handlers::write_dimension),
        has_dimension: Some(handlers::has_dimension),
        unset_dimension: Some(handlers::unset_dimension),
        read_property: Some(handlers::read_property),
        write_property: Some(handlers::write_property),
        ..ObjectHandlers::default()
    }
}

fn int_constant(ce: &mut ClassEntry, name: &str, value: u32) {
    let mut c = ClassConstant::new(name, Value::Int(value as i64), ce.name.clone());
    c.ty = Some(TypeDecl::from_mask(mask::INT));
    ce.add_constant(c);
}

/// `PHP_MINIT_FUNCTION(spl_array)`.
pub fn register(r: &mut Registry) {
    let mut ao = ClassEntry::new("ArrayObject", 0);
    for iface in [
        "IteratorAggregate",
        "ArrayAccess",
        "Serializable",
        "Countable",
    ] {
        let iface = engine::class(r, iface);
        engine::implement(&mut ao, &iface);
    }
    int_constant(&mut ao, "STD_PROP_LIST", STD_PROP_LIST);
    int_constant(&mut ao, "ARRAY_AS_PROPS", ARRAY_AS_PROPS);
    engine::add_methods(&mut ao, methods::ARRAY_OBJECT_METHODS);
    ao.handlers = handlers();
    r.class(ao);

    let mut ai = ClassEntry::new("ArrayIterator", 0);
    for iface in [
        "SeekableIterator",
        "ArrayAccess",
        "Serializable",
        "Countable",
    ] {
        let iface = engine::class(r, iface);
        engine::implement(&mut ai, &iface);
    }
    int_constant(&mut ai, "STD_PROP_LIST", STD_PROP_LIST);
    int_constant(&mut ai, "ARRAY_AS_PROPS", ARRAY_AS_PROPS);
    for (nf, deprecated) in methods::ARRAY_ITERATOR_METHODS {
        let mut flags = engine::method_flags(nf.sig);
        if *deprecated {
            flags |= fn_flags::DEPRECATED;
        }
        ai.add_method(Function::native_method(nf, "ArrayIterator", flags));
    }
    ai.handlers = handlers();
    let ai = r.class(ai);

    let mut rai = ClassEntry::new_subclass("RecursiveArrayIterator", 0, &ai);
    let iface = engine::class(r, super::iterators::RECURSIVE_ITERATOR);
    engine::implement(&mut rai, &iface);
    int_constant(&mut rai, "CHILD_ARRAYS_ONLY", CHILD_ARRAYS_ONLY);
    engine::add_methods(&mut rai, methods::RECURSIVE_ARRAY_ITERATOR_METHODS);
    r.class(rai);
}

/// Declared-property info lookup used by the property hooks.
pub(crate) fn find_declared<'a>(ce: &'a ClassEntry, name: &[u8]) -> Option<&'a PropertyInfo> {
    ce.find_property(name).filter(|i| !i.is_static())
}
