//! Object handlers of `ArrayObject` / `ArrayIterator`
//! (`spl_handler_ArrayObject`) and the `*_ex` helpers shared with the
//! `ArrayAccess` methods. `check_inherited` selects whether user overrides
//! of `offsetGet()`/`offsetSet()`/`offsetExists()`/`offsetUnset()` are
//! called (handlers) or bypassed (the methods themselves).

use php_runtime::class::{ClassEntry, DimFetch, PropPurpose, PropertyInfo, member_flags};
use php_runtime::value::KeyRef;
use php_runtime::value::convert::{OffsetKind, to_int_silent};
use php_runtime::{ArrayKey, ClassRef, Flow, ObjRef, PResult, PhpArray, PhpRef, PhpStr, Value, Vm};

use super::table::{Deleted, Found, HKey, is_object, resolve, state};
use super::{
    ARRAY_AS_PROPS, Overrides, STD_PROP_LIST, debug_info, find_declared, is_spl_array,
    move_forward, overrides_debug_info, skip_protected,
};

pub(crate) const SORTING: &str = "Modification of ArrayObject during sorting is prohibited";

fn fptr(obj: &ObjRef) -> Overrides {
    state(obj).map(|s| s.fptr).unwrap_or_default()
}

pub(crate) fn sorting(obj: &ObjRef) -> bool {
    state(obj).is_some_and(|s| s.apply_count > 0)
}

pub(crate) fn sorting_error(vm: &mut Vm) -> Flow {
    vm.throw_error("Error", SORTING)
}

/// `get_hash_key()`: the table key of an offset, `None` for illegal offsets
/// (arrays, objects, `IS_UNDEF`). Property tables use string keys only.
pub(crate) fn get_hash_key(vm: &mut Vm, obj: &ObjRef, offset: &Value) -> PResult<Option<HKey>> {
    let h = match offset {
        Value::Ref(r) => {
            let v = r.get();
            return get_hash_key(vm, obj, &v);
        }
        Value::Null => {
            vm.engine_deprecated(
                "Using null as an array offset is deprecated, use an empty string instead",
            )?;
            return Ok(Some(HKey::Str(PhpStr::new())));
        }
        Value::String(s) => match ArrayKey::parse_numeric(s) {
            Some(h) => h,
            None => return Ok(Some(HKey::Str(s.clone()))),
        },
        Value::Resource(r) => {
            let id = r.id();
            vm.engine_warning(format!(
                "Resource ID#{id} used as offset, casting to integer ({id})"
            ))?;
            id
        }
        Value::Float(d) => vm.dval_to_lval_safe(*d)?,
        Value::Bool(b) => *b as i64,
        Value::Int(i) => *i,
        Value::Undef | Value::Array(_) | Value::Object(_) => return Ok(None),
    };
    if is_object(obj) {
        Ok(Some(HKey::Str(PhpStr::from(h.to_string()))))
    } else {
        Ok(Some(HKey::Int(h)))
    }
}

/// `zend_illegal_container_offset(ce->name, offset, type)`.
pub(crate) fn illegal_offset(vm: &mut Vm, obj: &ObjRef, offset: &Value, kind: OffsetKind) -> Flow {
    let ty = offset.type_name();
    let class = &obj.class().name;
    let msg = match kind {
        OffsetKind::IsSet => format!("Cannot access offset of type {ty} in isset or empty"),
        OffsetKind::Unset => format!("Cannot unset offset of type {ty} on {class}"),
        _ => format!("Cannot access offset of type {ty} on {class}"),
    };
    vm.throw_type_error(msg)
}

fn offset_kind(fetch: DimFetch) -> OffsetKind {
    match fetch {
        DimFetch::Read => OffsetKind::Read,
        DimFetch::IsSet => OffsetKind::IsSet,
        DimFetch::Write | DimFetch::ReadWrite => OffsetKind::Write,
    }
}

fn is_write(fetch: DimFetch) -> bool {
    matches!(fetch, DimFetch::Write | DimFetch::ReadWrite)
}

/// What `spl_array_get_dimension_ptr` returns.
pub(crate) enum Ptr {
    /// `&EG(uninitialized_zval)`.
    Uninit,
    /// The element (read fetches).
    Value(Value),
    /// The element, turned into a reference (write fetches).
    Ref(PhpRef),
}

/// `spl_array_get_dimension_ptr`.
pub(crate) fn get_dimension_ptr(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    fetch: DimFetch,
) -> PResult<Ptr> {
    let Some(offset) = offset.filter(|o| !o.is_undef()) else {
        return Ok(Ptr::Uninit);
    };
    if is_write(fetch) && sorting(obj) {
        return Err(sorting_error(vm));
    }
    let Some(key) = get_hash_key(vm, obj, offset)? else {
        return Err(illegal_offset(vm, obj, offset, offset_kind(fetch)));
    };
    let ht = resolve(obj);
    match ht.find(&key) {
        Found::Value(v) if !is_write(fetch) => Ok(Ptr::Value(v)),
        Found::Value(_) => Ok(ht.element_ref(&key, false).map_or(Ptr::Uninit, Ptr::Ref)),
        Found::Missing | Found::Unset => match fetch {
            DimFetch::Read => {
                vm.engine_warning(key.undefined_message())?;
                Ok(Ptr::Uninit)
            }
            DimFetch::IsSet => Ok(Ptr::Uninit),
            DimFetch::ReadWrite | DimFetch::Write => {
                if fetch == DimFetch::ReadWrite {
                    vm.engine_warning(key.undefined_message())?;
                }
                Ok(resolve(obj)
                    .element_ref(&key, true)
                    .map_or(Ptr::Uninit, Ptr::Ref))
            }
        },
    }
}

/// `&EG(uninitialized_zval)` as returned to the interpreter.
fn uninit(fetch: DimFetch) -> Value {
    if is_write(fetch) {
        Value::Undef
    } else {
        Value::Null
    }
}

/// `spl_array_read_dimension_ex`.
pub(crate) fn read_dimension_ex(
    vm: &mut Vm,
    check_inherited: bool,
    obj: &ObjRef,
    offset: Option<&Value>,
    fetch: DimFetch,
) -> PResult<Value> {
    let fptr = fptr(obj);
    if check_inherited && (fptr.offset_get || (fetch == DimFetch::IsSet && fptr.offset_has)) {
        if fetch == DimFetch::IsSet {
            let off = offset.cloned().unwrap_or(Value::Null);
            if !has_dimension_ex(vm, true, obj, &off, 0)? {
                return Ok(Value::Null);
            }
        }
        if fptr.offset_get {
            let arg = offset.cloned().unwrap_or(Value::Null);
            let rv = vm.call_method(obj, b"offsetGet", vec![arg])?;
            return Ok(match rv {
                Value::Undef => uninit(fetch),
                rv if is_write(fetch) => rv,
                rv => rv.into_unref(),
            });
        }
    }
    Ok(match get_dimension_ptr(vm, obj, offset, fetch)? {
        Ptr::Uninit => uninit(fetch),
        Ptr::Value(v) => v.into_unref(),
        Ptr::Ref(r) => Value::Ref(r),
    })
}

/// `read_dimension` handler.
pub(crate) fn read_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    fetch: DimFetch,
) -> PResult<Value> {
    read_dimension_ex(vm, true, obj, offset, fetch)
}

/// `spl_array_write_dimension_ex`; `None` or a `null` offset appends.
pub(crate) fn write_dimension_ex(
    vm: &mut Vm,
    check_inherited: bool,
    obj: &ObjRef,
    offset: Option<&Value>,
    value: Value,
) -> PResult<()> {
    if check_inherited && fptr(obj).offset_set {
        let off = offset.cloned().unwrap_or(Value::Null);
        vm.call_method(obj, b"offsetSet", vec![off, value])?;
        return Ok(());
    }
    if sorting(obj) {
        return Err(sorting_error(vm));
    }
    let value = value.into_unref();
    let offset = match offset {
        None | Some(Value::Null) => {
            resolve(obj).append(value);
            return Ok(());
        }
        Some(o) => o,
    };
    let Some(key) = get_hash_key(vm, obj, offset)? else {
        return Err(illegal_offset(vm, obj, offset, OffsetKind::Write));
    };
    resolve(obj).update(&key, value);
    Ok(())
}

/// `write_dimension` handler.
pub(crate) fn write_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    value: Value,
) -> PResult<()> {
    write_dimension_ex(vm, true, obj, offset, value)
}

/// `spl_array_unset_dimension_ex`.
pub(crate) fn unset_dimension_ex(
    vm: &mut Vm,
    check_inherited: bool,
    obj: &ObjRef,
    offset: &Value,
) -> PResult<()> {
    if check_inherited && fptr(obj).offset_del {
        vm.call_method(obj, b"offsetUnset", vec![offset.clone()])?;
        return Ok(());
    }
    if sorting(obj) {
        return Err(sorting_error(vm));
    }
    let Some(key) = get_hash_key(vm, obj, offset)? else {
        return Err(illegal_offset(vm, obj, offset, OffsetKind::Unset));
    };
    let ht = resolve(obj);
    if let Deleted::DeclaredSlot = ht.delete(&key) {
        // php-src moves the object's own iterator past the unset property.
        move_forward(obj, &ht);
        skip_protected(obj, &ht);
    }
    Ok(())
}

/// `unset_dimension` handler.
pub(crate) fn unset_dimension(vm: &mut Vm, obj: &ObjRef, offset: &Value) -> PResult<()> {
    unset_dimension_ex(vm, true, obj, offset)
}

/// `spl_array_has_dimension_ex`: `check_empty` is 0 (`isset`), 1 (`!empty`)
/// or 2 (`offsetExists()`: a `null` element exists).
pub(crate) fn has_dimension_ex(
    vm: &mut Vm,
    check_inherited: bool,
    obj: &ObjRef,
    offset: &Value,
    check_empty: u8,
) -> PResult<bool> {
    let fptr = fptr(obj);
    let mut value = None;
    if check_inherited && fptr.offset_has {
        let rv = vm.call_method(obj, b"offsetExists", vec![offset.clone()])?;
        if !vm.to_bool(&rv)? {
            return Ok(false);
        }
        if check_empty == 0 {
            return Ok(true);
        } else if fptr.offset_get {
            value = Some(read_dimension_ex(
                vm,
                true,
                obj,
                Some(offset),
                DimFetch::Read,
            )?);
        }
    }
    let value = match value {
        Some(v) => v,
        None => {
            let Some(key) = get_hash_key(vm, obj, offset)? else {
                return Err(illegal_offset(vm, obj, offset, OffsetKind::IsSet));
            };
            let found = resolve(obj).find(&key);
            if let Found::Missing = found {
                return Ok(false);
            }
            if check_empty == 2 {
                return Ok(true);
            }
            if check_empty != 0 && check_inherited && fptr.offset_get {
                read_dimension_ex(vm, true, obj, Some(offset), DimFetch::Read)?
            } else {
                match found {
                    Found::Value(v) => v,
                    // php-src sees the `IS_INDIRECT` bucket of an unset
                    // property: not null for isset(), not truthy for empty().
                    _ => return Ok(check_empty == 0),
                }
            }
        }
    };
    if check_empty != 0 {
        vm.to_bool(&value)
    } else {
        Ok(!matches!(value, Value::Null | Value::Undef))
    }
}

/// `has_dimension` handler (`check_empty`: `empty()`).
pub(crate) fn has_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: &Value,
    check_empty: bool,
) -> PResult<bool> {
    has_dimension_ex(vm, true, obj, offset, check_empty as u8)
}

/// `spl_array_iterator_append`.
pub(crate) fn iterator_append(vm: &mut Vm, obj: &ObjRef, value: Value) -> PResult<()> {
    if is_object(obj) {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Cannot append properties to objects, use {}::offsetSet() instead",
                obj.class().name
            ),
        ));
    }
    write_dimension_ex(vm, true, obj, None, value)
}

// ---------------------------------------------------------------------------
// count / compare / property tables
// ---------------------------------------------------------------------------

/// `spl_array_object_count_elements_helper`.
pub(crate) fn count_helper(obj: &ObjRef) -> i64 {
    resolve(obj).visible_count()
}

/// `count_elements` handler (`spl_array_object_count_elements`).
pub(crate) fn count_elements(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<i64>> {
    if fptr(obj).count {
        let rv = vm.call_method(obj, b"count", Vec::new())?;
        if rv.is_undef() {
            return Ok(None);
        }
        return Ok(Some(to_int_silent(&rv)));
    }
    Ok(Some(count_helper(obj)))
}

/// `spl_array_compare_objects`.
pub(crate) fn compare(vm: &mut Vm, a: &Value, b: &Value) -> PResult<i32> {
    let (Value::Object(o1), Value::Object(o2)) = (a, b) else {
        return vm.std_compare_objects(a, b);
    };
    if !is_spl_array(o1) || !is_spl_array(o2) {
        return vm.std_compare_objects(a, b);
    }
    let (ht1, ht2) = (resolve(o1), resolve(o2));
    let (t1, t2) = (ht1.shared_array(), ht2.shared_array());
    let result = vm.compare_arrays(&t1, &t2)?;
    // If both tables are the objects' own properties they were compared already.
    if result == 0 && !(ht1.is_props_of(o1) && ht2.is_props_of(o2)) {
        return vm.std_compare_objects(a, b);
    }
    Ok(result)
}

/// `spl_array_get_properties_for`.
pub(crate) fn get_properties_for(
    _vm: &mut Vm,
    obj: &ObjRef,
    purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    if state(obj).is_none_or(|s| s.ar_flags & STD_PROP_LIST != 0) {
        return Ok(None);
    }
    match purpose {
        PropPurpose::ArrayCast | PropPurpose::VarExport | PropPurpose::Json => {
            Ok(Some(resolve(obj).to_array()))
        }
        _ => Ok(None),
    }
}

/// `get_debug_info`: `zend_std_get_debug_info` calling `__debugInfo()`,
/// which is `spl_array_get_debug_info` unless a subclass overrides it.
pub(crate) fn get_debug_info(
    vm: &mut Vm,
    obj: &ObjRef,
    _purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    if overrides_debug_info(obj.class()) {
        return match vm
            .call_method(obj, b"__debugInfo", Vec::new())?
            .into_unref()
        {
            Value::Array(a) => Ok(Some((*a).clone())),
            Value::Null => Ok(Some(PhpArray::new())),
            _ => Err(vm.fatal_error("__debuginfo() must return an array")),
        };
    }
    Ok(Some(debug_info(obj)))
}

// ---------------------------------------------------------------------------
// Properties (ArrayObject::ARRAY_AS_PROPS)
// ---------------------------------------------------------------------------

fn array_as_props(obj: &ObjRef) -> bool {
    state(obj).is_some_and(|s| s.ar_flags & ARRAY_AS_PROPS != 0)
}

/// The class of the code accessing the property (`EG(fake_scope)` /
/// the executing function's scope).
fn current_scope(vm: &Vm) -> Option<ClassRef> {
    let f = vm.frames.last()?;
    let scope = f.function.as_ref()?.scope.as_ref()?;
    vm.find_class(scope)
}

enum Access {
    Granted,
    Denied,
    /// An inaccessible private property of a parent class: looked up as a
    /// dynamic property.
    Dynamic,
}

/// `zend_get_property_offset` visibility rules.
fn access(vm: &Vm, ce: &ClassEntry, info: &PropertyInfo) -> Access {
    let vis = info.flags & member_flags::PPP_MASK;
    if vis == 0 || vis & member_flags::PUBLIC != 0 {
        return Access::Granted;
    }
    let scope = current_scope(vm);
    if vis & member_flags::PRIVATE != 0 {
        if scope
            .as_ref()
            .is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class))
        {
            return Access::Granted;
        }
        if !ce.name.eq_ignore_ascii_case(&info.declaring_class) {
            return Access::Dynamic;
        }
        return Access::Denied;
    }
    let Some(scope) = scope else {
        return Access::Denied;
    };
    match vm.find_class(&info.declaring_class) {
        Some(decl) if scope.instance_of(&decl) || decl.instance_of(&scope) => Access::Granted,
        _ => Access::Denied,
    }
}

/// `zend_std_has_property(obj, name, ZEND_PROPERTY_EXISTS)`.
fn std_has_property(vm: &Vm, obj: &ObjRef, name: &[u8]) -> bool {
    let ce = obj.class();
    if let Some(info) = find_declared(ce, name) {
        if info.flags & member_flags::VIRTUAL != 0 {
            return true;
        }
        match access(vm, ce, info) {
            Access::Granted => {
                return obj
                    .obj()
                    .props
                    .borrow()
                    .get(info.slot)
                    .is_some_and(|v| !v.is_undef());
            }
            Access::Denied => return false,
            Access::Dynamic => {}
        }
    }
    obj.obj()
        .dyn_props
        .borrow()
        .as_ref()
        .is_some_and(|t| t.get_by(KeyRef::Str(name)).is_some())
}

/// Whether property `name` is handled as an element.
fn as_element(vm: &Vm, obj: &ObjRef, name: &[u8]) -> bool {
    array_as_props(obj) && !std_has_property(vm, obj, name)
}

/// `read_property` handler (`Read` fetch).
pub(crate) fn read_property(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<Option<Value>> {
    read_property_fetch(vm, obj, name, DimFetch::Read)
}

/// `spl_array_read_property` with the fetch type: `Ok(None)` = standard
/// handler; otherwise the element read through `ARRAY_AS_PROPS`.
pub fn read_property_fetch(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &PhpStr,
    fetch: DimFetch,
) -> PResult<Option<Value>> {
    if as_element(vm, obj, name) {
        let member = Value::String(name.clone());
        return Ok(Some(read_dimension_ex(
            vm,
            true,
            obj,
            Some(&member),
            fetch,
        )?));
    }
    Ok(None)
}

/// `write_property` handler: `Ok(true)` when stored as an element.
pub(crate) fn write_property(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &PhpStr,
    value: Value,
) -> PResult<bool> {
    if as_element(vm, obj, name) {
        let member = Value::String(name.clone());
        write_dimension_ex(vm, true, obj, Some(&member), value)?;
        return Ok(true);
    }
    Ok(false)
}

/// Result of [`property_ptr`] (`get_property_ptr_ptr`).
pub enum PropertyPtr {
    /// Not an element: use the standard property handling.
    Standard,
    /// `NULL`: fall back to `read_property`/`write_property` (a user
    /// `offsetGet()` must see the access).
    ReadWrite,
    /// The element: `Value::Ref` for write fetches (write through it),
    /// `Value::Undef` for `&EG(uninitialized_zval)`, else the value.
    Element(Value),
}

/// `spl_array_get_property_ptr_ptr`.
pub fn property_ptr(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &PhpStr,
    fetch: DimFetch,
) -> PResult<PropertyPtr> {
    if !as_element(vm, obj, name) {
        return Ok(PropertyPtr::Standard);
    }
    if fptr(obj).offset_get {
        return Ok(PropertyPtr::ReadWrite);
    }
    let member = Value::String(name.clone());
    Ok(PropertyPtr::Element(
        match get_dimension_ptr(vm, obj, Some(&member), fetch)? {
            Ptr::Uninit => Value::Undef,
            Ptr::Value(v) => v,
            Ptr::Ref(r) => Value::Ref(r),
        },
    ))
}

/// `spl_array_has_property`: `has_set_exists` is `ZEND_PROPERTY_ISSET` (0),
/// `ZEND_PROPERTY_NOT_EMPTY` (1) or `ZEND_PROPERTY_EXISTS` (2). `Ok(None)` =
/// standard handler.
pub fn has_property(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &PhpStr,
    has_set_exists: u8,
) -> PResult<Option<bool>> {
    if as_element(vm, obj, name) {
        let member = Value::String(name.clone());
        return Ok(Some(has_dimension_ex(
            vm,
            true,
            obj,
            &member,
            has_set_exists,
        )?));
    }
    Ok(None)
}

/// `spl_array_unset_property`: `Ok(true)` when the element was unset,
/// `Ok(false)` = standard handler.
pub fn unset_property(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<bool> {
    if as_element(vm, obj, name) {
        let member = Value::String(name.clone());
        unset_dimension_ex(vm, true, obj, &member)?;
        return Ok(true);
    }
    Ok(false)
}
