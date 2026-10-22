//! The methods of `ArrayObject`, `ArrayIterator` and
//! `RecursiveArrayIterator` (`PHP_METHOD`s of `spl_array.c`).

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{DimFetch, fn_flags, member_flags};
use php_runtime::types::CoerceMode;
use php_runtime::value::convert::proptable_to_symtable;
use php_runtime::{
    Args, ArrayKey, CallArgs, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpRef,
    PhpStr, Value, Vm,
};

use super::handlers::{
    count_helper, has_dimension_ex, iterator_append, read_dimension_ex, sorting, sorting_error,
    unset_dimension_ex, write_dimension_ex,
};
use super::table::{Found, resolve, state};
use super::{
    CHILD_ARRAYS_ONLY, CLONE_MASK, INT_MASK, IS_SELF, SplArray, USE_OTHER, current_entry,
    debug_info, find_declared, is_spl_array, iterator_key, next, rewind, valid,
};
use crate::spl::engine::this;
use crate::standard::var_serialize::{
    SerializeData, UnserializeData, var_serialize, var_serialize_destroy, var_serialize_init,
    var_unserialize, var_unserialize_destroy, var_unserialize_init,
};

/// User flags given to constructors (`ar_flags &= ~SPL_ARRAY_INT_MASK`).
fn user_flags(flags: i64) -> u32 {
    (flags as u32) & !INT_MASK
}

fn with_state<R>(obj: &ObjRef, f: impl FnOnce(&mut SplArray) -> R) -> Option<R> {
    obj.native_mut::<SplArray>().map(|mut st| f(&mut st))
}

/// `zend_deprecated_function` for the `#[\Deprecated]` methods of
/// `ArrayIterator` (they share their handlers with `ArrayObject`).
fn deprecated(vm: &mut Vm, args: &Args) -> PResult<()> {
    let f = args.function().clone();
    if f.has_flag(fn_flags::DEPRECATED) {
        let scope = f.scope.clone().unwrap_or_default();
        vm.engine_deprecated(format!(
            "Method {scope}::{}() is deprecated since 8.6",
            f.name
        ))?;
    }
    Ok(())
}

/// `Z_PARAM_CLASS` with base class `ArrayIterator`.
fn iterator_class_arg(vm: &mut Vm, name: &PhpStr, arg: usize) -> PResult<ClassRef> {
    match vm.lookup_class(name)? {
        Some(ce) if ce.instance_of_name(b"ArrayIterator") => Ok(ce),
        _ => Err(vm.argument_type_error(
            arg,
            &format!("must be a class name derived from ArrayIterator, \"{name}\" given"),
        )),
    }
}

/// Classes whose property table is not the standard one (`get_properties`
/// or `get_properties_for` overridden). The runtime has no `get_properties`
/// hook; `DateInterval` is the class of the default build overriding it.
fn overloaded_properties(obj: &ObjRef) -> bool {
    let ce = obj.class();
    ce.handlers.get_properties_for.is_some() || ce.instance_of_name(b"DateInterval")
}

/// `spl_array_set_array`.
pub(crate) fn set_array(
    vm: &mut Vm,
    obj: &ObjRef,
    array: &Value,
    mut ar_flags: u32,
    just_array: bool,
) -> PResult<()> {
    if sorting(obj) {
        return Err(sorting_error(vm));
    }
    let storage = match array.unref() {
        Value::Array(a) => Value::Array(a),
        Value::Object(o) => {
            let which = if obj.class().instance_of_name(b"ArrayIterator") {
                "ArrayIterator"
            } else {
                "ArrayObject"
            };
            vm.deprecated(format!(
                "Using an object as a backing array for {which} is deprecated, as it allows violating class constraints and invariants"
            ))?;
            if is_spl_array(&o) {
                if just_array {
                    ar_flags = state(&o).map_or(0, |s| s.ar_flags & !INT_MASK);
                }
                if ObjRef::ptr_eq(obj, &o) {
                    ar_flags |= IS_SELF;
                    Value::Undef
                } else {
                    ar_flags |= USE_OTHER;
                    Value::Object(o)
                }
            } else {
                if overloaded_properties(&o) {
                    return Err(vm.throw_error(
                        "InvalidArgumentException",
                        format!(
                            "Overloaded object of type {} is not compatible with {}",
                            o.class().name,
                            obj.class().name
                        ),
                    ));
                }
                if o.class().is_enum() {
                    return Err(vm.throw_error(
                        "InvalidArgumentException",
                        format!("Enums are not compatible with {}", obj.class().name),
                    ));
                }
                Value::Object(o)
            }
        }
        _ => return Ok(()),
    };
    let garbage = with_state(obj, |st| {
        let old = std::mem::replace(&mut st.array, storage);
        st.ar_flags &= !(IS_SELF | USE_OTHER);
        st.ar_flags |= ar_flags;
        st.pos = None;
        old
    });
    drop(garbage);
    Ok(())
}

// ---------------------------------------------------------------------------
// ArrayObject / ArrayIterator (shared)
// ---------------------------------------------------------------------------

fn ao_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let n = args.len();
    if n == 0 {
        return Ok(Value::Null);
    }
    let array = args.get(0).clone();
    let flags = user_flags(args.int(1));
    if n > 2 {
        let name = args.str(2).clone();
        let ce = iterator_class_arg(vm, &name, 3)?;
        with_state(&this, |st| st.ce_get_iterator = ce);
    }
    set_array(vm, &this, &array, flags, n == 1)?;
    Ok(Value::Null)
}

fn ai_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let n = args.len();
    if n == 0 {
        return Ok(Value::Null);
    }
    let array = args.get(0).clone();
    let flags = user_flags(args.int(1));
    set_array(vm, &this, &array, flags, n == 1)?;
    Ok(Value::Null)
}

fn offset_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let index = args.get(0).clone();
    Ok(Value::Bool(has_dimension_ex(vm, false, &this, &index, 2)?))
}

fn offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let index = args.get(0).clone();
    read_dimension_ex(vm, false, &this, Some(&index), DimFetch::Read)
}

fn offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let index = args.get(0).clone();
    let value = args.get(1).clone();
    write_dimension_ex(vm, false, &this, Some(&index), value)?;
    Ok(Value::Null)
}

fn offset_unset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let index = args.get(0).clone();
    unset_dimension_ex(vm, false, &this, &index)?;
    Ok(Value::Null)
}

fn append(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let value = args.get(0).clone();
    iterator_append(vm, &this, value)?;
    Ok(Value::Null)
}

fn get_array_copy(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(resolve(&this(args)).to_array()))
}

fn count(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(count_helper(&this(args))))
}

fn get_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let flags = state(&this(args)).map_or(0, |s| s.ar_flags & !INT_MASK);
    Ok(Value::Int(flags as i64))
}

fn set_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let flags = user_flags(args.int(0));
    with_state(&this(args), |st| {
        st.ar_flags = (st.ar_flags & INT_MASK) | flags;
    });
    Ok(Value::Null)
}

/// `spl_array_method`: calls the ext/standard sort function `fname` on the
/// table by reference while modifications are prohibited, then stores the
/// sorted table.
fn array_method(vm: &mut Vm, this: &ObjRef, fname: &str, extra: Option<Value>) -> PResult<Value> {
    let ht = resolve(this);
    let Some(func) = vm.find_function(fname) else {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot call method {fname} when function {fname} is disabled"),
        ));
    };
    let cell = PhpRef::new(Value::Array(ht.shared_array()));
    let mut params = vec![Value::Ref(cell.clone())];
    params.extend(extra);
    with_state(this, |st| st.apply_count += 1);
    let result = vm.call_function(&func, CallArgs::new(params));
    with_state(this, |st| st.apply_count -= 1);
    if let Value::Array(sorted) = cell.set(Value::Null) {
        ht.replace(sorted);
    }
    result
}

fn asort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let flags = args.int(0);
    array_method(vm, &this(args), "asort", Some(Value::Int(flags)))
}

fn ksort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let flags = args.int(0);
    array_method(vm, &this(args), "ksort", Some(Value::Int(flags)))
}

fn uasort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let callback = args.get(0).clone();
    array_method(vm, &this(args), "uasort", Some(callback))
}

fn uksort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let callback = args.get(0).clone();
    array_method(vm, &this(args), "uksort", Some(callback))
}

fn natsort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    array_method(vm, &this(args), "natsort", None)
}

fn natcasesort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    array_method(vm, &this(args), "natcasesort", None)
}

// ----- Serializable ---------------------------------------------------------

fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let this = this(args);
    let (flags, storage) = match state(&this) {
        Some(st) => (
            st.ar_flags & CLONE_MASK,
            (st.ar_flags & IS_SELF == 0).then(|| st.array.clone()),
        ),
        None => (0, None),
    };
    let data = var_serialize_init(vm);
    let mut buf = b"x:".to_vec();
    let result = serialize_parts(vm, &this, &mut buf, flags, storage, &data);
    var_serialize_destroy(vm, data);
    result?;
    Ok(Value::String(PhpStr::from_vec(buf)))
}

fn serialize_parts(
    vm: &mut Vm,
    this: &ObjRef,
    buf: &mut Vec<u8>,
    flags: u32,
    storage: Option<Value>,
    data: &Rc<RefCell<SerializeData>>,
) -> PResult<()> {
    var_serialize(vm, buf, &Value::Int(flags as i64), data)?;
    if let Some(storage) = storage {
        var_serialize(vm, buf, &storage, data)?;
        buf.push(b';');
    }
    buf.extend_from_slice(b"m:");
    let members = Value::array(crate::spl::engine::std_properties(this));
    var_serialize(vm, buf, &members, data)
}

/// Outcome of parsing a `Serializable` payload.
enum Parsed {
    /// Parsed; an exception raised while installing the storage (php-src
    /// keeps parsing with `EG(exception)` set) is reported at the end.
    Done(Option<Flow>),
    /// Syntax error at this offset.
    Error(usize),
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated(vm, args)?;
    let this = this(args);
    let buf = args.str(0).clone();
    if buf.is_empty() {
        return Ok(Value::Null);
    }
    if sorting(&this) {
        return Err(sorting_error(vm));
    }
    let data = var_unserialize_init(vm);
    let parsed = match unserialize_parts(vm, &this, &buf, &data) {
        Ok(p) => p,
        Err(e) => {
            let _ = var_unserialize_destroy(vm, data);
            return Err(e);
        }
    };
    var_unserialize_destroy(vm, data)?;
    match parsed {
        Parsed::Done(None) => Ok(Value::Null),
        Parsed::Done(Some(e)) => Err(e),
        Parsed::Error(p) => Err(vm.throw_error(
            "UnexpectedValueException",
            format!("Error at offset {p} of {} bytes", buf.len()),
        )),
    }
}

fn unserialize_parts(
    vm: &mut Vm,
    this: &ObjRef,
    buf: &[u8],
    data: &Rc<RefCell<UnserializeData>>,
) -> PResult<Parsed> {
    let at = |p: usize| buf.get(p).copied().unwrap_or(0);
    if at(0) != b'x' {
        return Ok(Parsed::Error(0));
    }
    if at(1) != b':' {
        return Ok(Parsed::Error(1));
    }
    let mut p = 2;
    let flags = match var_unserialize(vm, data, buf, &mut p)?.map(Value::into_unref) {
        Some(Value::Int(f)) => f,
        _ => return Ok(Parsed::Error(p)),
    };
    // php_var_unserialize consumed the ';' of the flags.
    p -= 1;
    if at(p) != b';' {
        return Ok(Parsed::Error(p));
    }
    p += 1;
    let clone_flags = (flags & CLONE_MASK as i64) as u32;
    let mut pending = None;
    if flags & IS_SELF as i64 != 0 {
        let garbage = with_state(this, |st| {
            st.ar_flags = (st.ar_flags & !CLONE_MASK) | clone_flags;
            std::mem::replace(&mut st.array, Value::Undef)
        });
        drop(garbage);
    } else {
        if !matches!(at(p), b'a' | b'O' | b'C' | b'r') {
            return Ok(Parsed::Error(p));
        }
        let array = match var_unserialize(vm, data, buf, &mut p)?.map(Value::into_unref) {
            Some(v @ (Value::Array(_) | Value::Object(_))) => v,
            _ => return Ok(Parsed::Error(p)),
        };
        with_state(this, |st| {
            st.ar_flags = (st.ar_flags & !CLONE_MASK) | clone_flags;
        });
        if let Value::Array(a) = array {
            let garbage = with_state(this, |st| {
                st.ar_flags &= !USE_OTHER;
                std::mem::replace(&mut st.array, Value::Array(a))
            });
            drop(garbage);
        } else if let Err(e) = set_array(vm, this, &array, 0, true) {
            pending = Some(e);
        }
        if at(p) != b';' {
            return Ok(pending.map_or(Parsed::Error(p), |e| Parsed::Done(Some(e))));
        }
        p += 1;
    }
    if at(p) != b'm' {
        return Ok(pending.map_or(Parsed::Error(p), |e| Parsed::Done(Some(e))));
    }
    p += 1;
    if at(p) != b':' {
        return Ok(pending.map_or(Parsed::Error(p), |e| Parsed::Done(Some(e))));
    }
    p += 1;
    let members = match var_unserialize(vm, data, buf, &mut p)?.map(Value::into_unref) {
        Some(Value::Array(m)) => m,
        _ => return Ok(pending.map_or(Parsed::Error(p), |e| Parsed::Done(Some(e)))),
    };
    properties_load(vm, this, &members)?;
    Ok(Parsed::Done(pending))
}

/// `zend_unmangle_property_name_ex`: `(class, name)`; `class` is `None` for
/// public names and `Some("*")` for protected ones.
fn unmangle(key: &[u8]) -> Option<(Option<&[u8]>, &[u8])> {
    if key.first() != Some(&0) {
        return Some((None, key));
    }
    let rest = &key[1..];
    let end = rest.iter().position(|c| *c == 0)?;
    let name = &rest[end + 1..];
    if name.is_empty() {
        return None;
    }
    Some((Some(&rest[..end]), name))
}

/// `object_properties_load`: declared properties are written into their
/// slots (typed ones verified in strict mode), the others become dynamic
/// properties (with the dynamic-property deprecation).
pub(crate) fn properties_load(vm: &mut Vm, obj: &ObjRef, props: &PhpArray) -> PResult<()> {
    let ce = obj.class().clone();
    for (key, prop) in props.iter() {
        let key = match key {
            ArrayKey::Str(s) => s,
            ArrayKey::Int(h) => {
                if !ce.has_flag(php_runtime::class::class_flags::ALLOW_DYNAMIC_PROPERTIES) {
                    vm.engine_deprecated(format!(
                        "Creation of dynamic property {}::${h} is deprecated",
                        ce.name
                    ))?;
                }
                let mut d = obj.obj().dyn_props.borrow_mut();
                d.get_or_insert_with(PhpArray::new)
                    .insert(ArrayKey::Str(PhpStr::from(h.to_string())), prop.clone());
                continue;
            }
        };
        let unmangled = unmangle(&key);
        let info = unmangled.and_then(|(class, name)| {
            let info = find_declared(&ce, name)?;
            let private = info.flags & member_flags::PRIVATE != 0;
            match class {
                Some(c) if c != b"*" => info
                    .declaring_class
                    .eq_ignore_ascii_case(c)
                    .then(|| info.clone()),
                // The scope is the SPL class: private properties of user
                // classes are not accessible.
                _ if private => None,
                _ => Some(info.clone()),
            }
        });
        let Some(info) = info else {
            if !ce.has_flag(php_runtime::class::class_flags::ALLOW_DYNAMIC_PROPERTIES) {
                let shown = unmangled.map_or(&b""[..], |(_, n)| n);
                vm.engine_deprecated(format!(
                    "Creation of dynamic property {}::${} is deprecated",
                    ce.name,
                    String::from_utf8_lossy(shown)
                ))?;
            }
            let mut d = obj.obj().dyn_props.borrow_mut();
            d.get_or_insert_with(PhpArray::new)
                .insert(ArrayKey::Str(key.clone()), prop.clone());
            continue;
        };
        if info.flags & member_flags::VIRTUAL != 0 {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Cannot unserialize value for virtual property {}::${}",
                    ce.name, info.name
                ),
            ));
        }
        let mut val = prop.clone();
        if let Some(ty) = &info.ty {
            if val.is_ref() {
                if info.is_readonly() {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Cannot indirectly modify readonly property {}::${}",
                            info.declaring_class, info.name
                        ),
                    ));
                }
                let inner = val.unref();
                if vm
                    .coerce_to_type(ty, inner.clone(), strict())?
                    .is_none_or(|c| !c.identical(&inner))
                {
                    return Err(type_error(vm, &inner, &info));
                }
            } else {
                let initialised = obj
                    .obj()
                    .props
                    .borrow()
                    .get(info.slot)
                    .is_some_and(|v| !v.is_undef());
                if info.is_readonly() && initialised {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Cannot modify readonly property {}::${}",
                            info.declaring_class, info.name
                        ),
                    ));
                }
                match vm.coerce_to_type(ty, val.clone(), strict())? {
                    Some(c) => val = c,
                    None => return Err(type_error(vm, &val, &info)),
                }
            }
        }
        let old = {
            let mut slots = obj.obj().props.borrow_mut();
            slots.get_mut(info.slot).map(|s| std::mem::replace(s, val))
        };
        drop(old);
    }
    Ok(())
}

fn strict() -> CoerceMode {
    CoerceMode {
        strict: true,
        silent: false,
    }
}

/// `zend_verify_property_type_error`.
fn type_error(vm: &mut Vm, v: &Value, info: &php_runtime::class::PropertyInfo) -> Flow {
    let ty = info
        .ty
        .as_ref()
        .map(|t| t.to_type_string())
        .unwrap_or_default();
    vm.throw_type_error(format!(
        "Cannot assign {} to property {}::${} of type {ty}",
        v.value_name(),
        info.declaring_class,
        info.name
    ))
}

fn serialize_magic(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (flags, storage, iterator_class) = match state(&this) {
        Some(st) => (
            st.ar_flags & CLONE_MASK,
            if st.ar_flags & IS_SELF != 0 {
                Value::Null
            } else {
                st.array.clone()
            },
            if st.ce_get_iterator.lc_name.as_bytes() == b"arrayiterator" {
                Value::Null
            } else {
                Value::String(st.ce_get_iterator.name.clone())
            },
        ),
        None => (0, Value::Null, Value::Null),
    };
    let members = proptable_to_symtable(&crate::spl::engine::std_properties(&this));
    Ok(Value::array(PhpArray::from_values(vec![
        Value::Int(flags as i64),
        storage,
        Value::array(members),
        iterator_class,
    ])))
}

fn unserialize_magic(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let data = args.array(0).clone();
    let get = |i: i64| data.get_int(i).map(Value::unref);
    let (flags, storage, members, iterator_class) = (get(0), get(1), get(2), get(3));
    let (Some(Value::Int(flags)), Some(storage), Some(Value::Array(members))) =
        (flags, storage, members)
    else {
        return Err(incomplete(vm));
    };
    if matches!(&iterator_class, Some(v) if !matches!(v, Value::Null | Value::String(_))) {
        return Err(incomplete(vm));
    }
    let clone_flags = (flags & CLONE_MASK as i64) as u32;
    with_state(&this, |st| {
        st.ar_flags = (st.ar_flags & !CLONE_MASK) | clone_flags;
    });
    if flags & IS_SELF as i64 != 0 {
        let garbage = with_state(&this, |st| std::mem::replace(&mut st.array, Value::Undef));
        drop(garbage);
    } else {
        if !matches!(storage, Value::Array(_) | Value::Object(_)) {
            return Err(vm.throw_error(
                "InvalidArgumentException",
                "Passed variable is not an array or object",
            ));
        }
        set_array(vm, &this, &storage, 0, true)?;
    }
    properties_load(vm, &this, &members)?;
    if let Some(Value::String(name)) = iterator_class {
        let Some(ce) = vm.lookup_class(&name)? else {
            return Err(vm.throw_error(
                "UnexpectedValueException",
                format!(
                    "Cannot deserialize ArrayObject with iterator class '{name}'; no such class exists"
                ),
            ));
        };
        if !ce.instance_of_name(b"ArrayIterator") {
            return Err(vm.throw_error(
                "UnexpectedValueException",
                format!(
                    "Cannot deserialize ArrayObject with iterator class '{name}'; this class is not derived from ArrayIterator"
                ),
            ));
        }
        with_state(&this, |st| st.ce_get_iterator = ce);
    }
    Ok(Value::Null)
}

fn incomplete(vm: &mut Vm) -> Flow {
    vm.throw_error(
        "UnexpectedValueException",
        "Incomplete or ill-typed serialization data",
    )
}

fn debug_info_method(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(debug_info(&this(args))))
}

// ----- ArrayObject only ----------------------------------------------------

fn get_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let Some((ce, flags)) = state(&this).map(|s| (s.ce_get_iterator.clone(), s.ar_flags)) else {
        return Ok(Value::Null);
    };
    let it = vm.instantiate(&ce)?;
    with_state(&it, |st| {
        st.ar_flags = (flags & CLONE_MASK) | USE_OTHER;
        st.ce_get_iterator = ce.clone();
        st.array = Value::Object(this.clone());
        st.pos = None;
    });
    Ok(Value::Object(it))
}

fn exchange_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    if sorting(&this) {
        return Err(sorting_error(vm));
    }
    let old = resolve(&this).to_array();
    let array = args.get(0).clone();
    set_array(vm, &this, &array, 0, true)?;
    Ok(Value::array(old))
}

fn set_iterator_class(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let name = args.str(0).clone();
    let ce = iterator_class_arg(vm, &name, 1)?;
    with_state(&this, |st| st.ce_get_iterator = ce);
    Ok(Value::Null)
}

fn get_iterator_class(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(state(&this(args)).map_or(Value::Null, |s| {
        Value::String(s.ce_get_iterator.name.clone())
    }))
}

// ----- ArrayIterator ---------------------------------------------------------

fn it_rewind(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    rewind(&this(args));
    Ok(Value::Null)
}

fn it_seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let opos = args.int(0);
    let mut position = opos;
    if position >= 0 {
        rewind(&this);
        let mut result = true;
        while position > 0 {
            position -= 1;
            let ht = resolve(&this);
            result = next(&this, &ht);
            if !result {
                break;
            }
        }
        let ht = resolve(&this);
        if result && valid(&this, &ht) {
            return Ok(Value::Null);
        }
    }
    Err(vm.throw_error(
        "OutOfBoundsException",
        format!("Seek position {opos} is out of range"),
    ))
}

fn it_current(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let ht = resolve(&this);
    Ok(match current_entry(&this, &ht) {
        Some((_, Found::Value(v))) => v.into_unref(),
        _ => Value::Null,
    })
}

fn it_key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(iterator_key(&this(args)))
}

fn it_next(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let ht = resolve(&this);
    next(&this, &ht);
    Ok(Value::Null)
}

fn it_valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let ht = resolve(&this);
    Ok(Value::Bool(valid(&this, &ht)))
}

// ----- RecursiveArrayIterator ------------------------------------------------

/// The dereferenced current element, `None` past the end.
fn current_child(this: &ObjRef) -> Option<Value> {
    let ht = resolve(this);
    let (_, found) = current_entry(this, &ht)?;
    Some(match found {
        Found::Value(v) => v.into_unref(),
        _ => Value::Undef,
    })
}

fn rai_has_children(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let Some(entry) = current_child(&this) else {
        return Ok(Value::Bool(false));
    };
    let flags = state(&this).map_or(0, |s| s.ar_flags);
    Ok(Value::Bool(match entry {
        Value::Array(_) => true,
        Value::Object(_) => flags & CHILD_ARRAYS_ONLY == 0,
        _ => false,
    }))
}

fn rai_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let Some(entry) = current_child(&this) else {
        return Ok(Value::Null);
    };
    let flags = state(&this).map_or(0, |s| s.ar_flags);
    if let Value::Object(o) = &entry {
        if flags & CHILD_ARRAYS_ONLY != 0 {
            return Ok(Value::Null);
        }
        if o.class().instance_of(this.class()) {
            return Ok(entry);
        }
    }
    // object_init_with_constructor(return_value, Z_OBJCE_P(ZEND_THIS), 2, params)
    let ce = this.class().clone();
    let child = vm.instantiate(&ce)?;
    let entry = if entry.is_undef() { Value::Null } else { entry };
    vm.call_method(
        &child,
        b"__construct",
        vec![entry, Value::Int(flags as i64)],
    )?;
    Ok(Value::Object(child))
}

// ---------------------------------------------------------------------------
// Method tables (signatures verbatim from spl_array.stub.php)
// ---------------------------------------------------------------------------

static AO_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(array|object $array = [], int $flags = 0, string $iteratorClass = ArrayIterator::class) {}",
    handler: ao_construct,
};
static AI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(array|object $array = [], int $flags = 0) {}",
    handler: ai_construct,
};
static OFFSET_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists(mixed $key): bool {}",
    handler: offset_exists,
};
static OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet(mixed $key): mixed {}",
    handler: offset_get,
};
static OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet(mixed $key, mixed $value): void {}",
    handler: offset_set,
};
static OFFSET_UNSET: NativeFunction = NativeFunction {
    sig: "public function offsetUnset(mixed $key): void {}",
    handler: offset_unset,
};
static APPEND: NativeFunction = NativeFunction {
    sig: "public function append(mixed $value): void {}",
    handler: append,
};
static GET_ARRAY_COPY: NativeFunction = NativeFunction {
    sig: "public function getArrayCopy(): array {}",
    handler: get_array_copy,
};
static COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: count,
};
static GET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function getFlags(): int {}",
    handler: get_flags,
};
static SET_FLAGS: NativeFunction = NativeFunction {
    sig: "public function setFlags(int $flags): void {}",
    handler: set_flags,
};
static ASORT: NativeFunction = NativeFunction {
    sig: "public function asort(int $flags = SORT_REGULAR): true {}",
    handler: asort,
};
static KSORT: NativeFunction = NativeFunction {
    sig: "public function ksort(int $flags = SORT_REGULAR): true {}",
    handler: ksort,
};
static UASORT: NativeFunction = NativeFunction {
    sig: "public function uasort(callable $callback): true {}",
    handler: uasort,
};
static UKSORT: NativeFunction = NativeFunction {
    sig: "public function uksort(callable $callback): true {}",
    handler: uksort,
};
static NATSORT: NativeFunction = NativeFunction {
    sig: "public function natsort(): true {}",
    handler: natsort,
};
static NATCASESORT: NativeFunction = NativeFunction {
    sig: "public function natcasesort(): true {}",
    handler: natcasesort,
};
static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function unserialize(string $data): void {}",
    handler: unserialize,
};
static SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function serialize(): string {}",
    handler: serialize,
};
static SERIALIZE_MAGIC: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: serialize_magic,
};
static UNSERIALIZE_MAGIC: NativeFunction = NativeFunction {
    sig: "public function __unserialize(array $data): void {}",
    handler: unserialize_magic,
};
static GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getIterator(): Iterator {}",
    handler: get_iterator,
};
static EXCHANGE_ARRAY: NativeFunction = NativeFunction {
    sig: "public function exchangeArray(array|object $array): array {}",
    handler: exchange_array,
};
static SET_ITERATOR_CLASS: NativeFunction = NativeFunction {
    sig: "public function setIteratorClass(string $iteratorClass): void {}",
    handler: set_iterator_class,
};
static GET_ITERATOR_CLASS: NativeFunction = NativeFunction {
    sig: "public function getIteratorClass(): string {}",
    handler: get_iterator_class,
};
static DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: debug_info_method,
};
static REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: it_rewind,
};
static CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: it_current,
};
static KEY: NativeFunction = NativeFunction {
    sig: "public function key(): string|int|null {}",
    handler: it_key,
};
static NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: it_next,
};
static VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: it_valid,
};
static SEEK: NativeFunction = NativeFunction {
    sig: "public function seek(int $offset): void {}",
    handler: it_seek,
};
static HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: rai_has_children,
};
static GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): ?RecursiveArrayIterator {}",
    handler: rai_get_children,
};

/// `class_ArrayObject_methods`.
pub(crate) static ARRAY_OBJECT_METHODS: &[&NativeFunction] = &[
    &AO_CONSTRUCT,
    &OFFSET_EXISTS,
    &OFFSET_GET,
    &OFFSET_SET,
    &OFFSET_UNSET,
    &APPEND,
    &GET_ARRAY_COPY,
    &COUNT,
    &GET_FLAGS,
    &SET_FLAGS,
    &ASORT,
    &KSORT,
    &UASORT,
    &UKSORT,
    &NATSORT,
    &NATCASESORT,
    &UNSERIALIZE,
    &SERIALIZE,
    &SERIALIZE_MAGIC,
    &UNSERIALIZE_MAGIC,
    &GET_ITERATOR,
    &EXCHANGE_ARRAY,
    &SET_ITERATOR_CLASS,
    &GET_ITERATOR_CLASS,
    &DEBUG_INFO,
];

/// `class_ArrayIterator_methods`, with the `#[\Deprecated]` marker.
pub(crate) static ARRAY_ITERATOR_METHODS: &[(&NativeFunction, bool)] = &[
    (&AI_CONSTRUCT, false),
    (&OFFSET_EXISTS, false),
    (&OFFSET_GET, false),
    (&OFFSET_SET, false),
    (&OFFSET_UNSET, false),
    (&APPEND, false),
    (&GET_ARRAY_COPY, false),
    (&COUNT, false),
    (&GET_FLAGS, true),
    (&SET_FLAGS, true),
    (&ASORT, true),
    (&KSORT, true),
    (&UASORT, true),
    (&UKSORT, true),
    (&NATSORT, true),
    (&NATCASESORT, true),
    (&UNSERIALIZE, true),
    (&SERIALIZE, true),
    (&SERIALIZE_MAGIC, false),
    (&UNSERIALIZE_MAGIC, false),
    (&REWIND, false),
    (&CURRENT, false),
    (&KEY, false),
    (&NEXT, false),
    (&VALID, false),
    (&SEEK, false),
    (&DEBUG_INFO, false),
];

/// `class_RecursiveArrayIterator_methods`.
pub(crate) static RECURSIVE_ARRAY_ITERATOR_METHODS: &[&NativeFunction] =
    &[&HAS_CHILDREN, &GET_CHILDREN];
