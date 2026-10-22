//! Objects: property access (`zend_std_{read,write,has,unset}_property`,
//! `get_property_ptr_ptr`), visibility, asymmetric visibility, readonly and
//! typed properties, property hooks, magic `__get/__set/__isset/__unset`
//! with recursion guards, `ArrayAccess`, static properties, `clone`,
//! instantiation and destructors.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::class::{ClassRef, Function, PropPurpose, PropertyInfo, class_flags, member_flags};
use crate::errors::level;
use crate::object::{ObjRef, obj_flags};
use crate::types::CoerceMode;
use crate::value::array::ArrayKey;
use crate::value::convert::mangle_property_name;
use crate::value::{PhpArray, PhpRef, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::Ctx;
use super::decl::{HookSet, display_class_name, prop_hooks};

// ---------------------------------------------------------------------------
// Per-object interpreter state
// ---------------------------------------------------------------------------

const IN_GET: u8 = 1;
const IN_SET: u8 = 2;
const IN_ISSET: u8 = 4;
const IN_UNSET: u8 = 8;

const SET_MASK: u32 = member_flags::PUBLIC_SET | member_flags::PROTECTED_SET | member_flags::PRIVATE_SET;

/// Interpreter data attached to objects (`PhpObject::extra`).
#[derive(Default, Clone)]
pub struct ObjExtra {
    /// Magic-method recursion guards per property name.
    guards: HashMap<PhpStr, u8>,
    /// Declared slots that were `unset()` (as opposed to never initialised:
    /// the inverse of `IS_PROP_UNINIT`).
    unset_slots: Vec<usize>,
    /// Readonly slots that may be re-initialised once (`IS_PROP_REINITABLE`,
    /// set while `__clone` / clone-with runs).
    reinitable: Vec<usize>,
}

fn with_extra<R>(obj: &ObjRef, f: impl FnOnce(&mut ObjExtra) -> R) -> R {
    let mut e = obj.obj().extra.borrow_mut();
    if e.is_none() {
        *e = Some(Box::new(ObjExtra::default()));
    }
    let x = e.as_mut().unwrap().downcast_mut::<ObjExtra>().expect("ObjExtra");
    f(x)
}

fn has_extra(obj: &ObjRef) -> bool {
    obj.obj().extra.borrow().is_some()
}

fn guard_get(obj: &ObjRef, name: &PhpStr, bit: u8) -> bool {
    if !has_extra(obj) {
        return false;
    }
    with_extra(obj, |x| x.guards.get(name).is_some_and(|g| g & bit != 0))
}

fn guard_set(obj: &ObjRef, name: &PhpStr, bit: u8, on: bool) {
    with_extra(obj, |x| {
        let g = x.guards.entry(name.clone()).or_insert(0);
        if on {
            *g |= bit;
        } else {
            *g &= !bit;
        }
    });
}

fn slot_was_unset(obj: &ObjRef, slot: usize) -> bool {
    if !has_extra(obj) {
        return false;
    }
    with_extra(obj, |x| x.unset_slots.contains(&slot))
}

fn mark_slot_unset(obj: &ObjRef, slot: usize, on: bool) {
    if !on && !has_extra(obj) {
        return;
    }
    with_extra(obj, |x| {
        if on {
            if !x.unset_slots.contains(&slot) {
                x.unset_slots.push(slot);
            }
        } else {
            x.unset_slots.retain(|s| *s != slot);
        }
    });
}

fn slot_reinitable(obj: &ObjRef, slot: usize) -> bool {
    if !has_extra(obj) {
        return false;
    }
    with_extra(obj, |x| x.reinitable.contains(&slot))
}

fn clear_reinitable(obj: &ObjRef, slot: usize) {
    if !has_extra(obj) {
        return;
    }
    with_extra(obj, |x| x.reinitable.retain(|s| *s != slot));
}

/// `zend_object_set_properties_reinitable`.
fn set_properties_reinitable(obj: &ObjRef, on: bool) {
    let class = obj.class().clone();
    let has_readonly = class.properties.values().any(|p| p.is_readonly() && !p.is_static());
    if !has_readonly {
        return;
    }
    if !on {
        if has_extra(obj) {
            with_extra(obj, |x| x.reinitable.clear());
        }
        return;
    }
    let slots: Vec<usize> = {
        let props = obj.obj().props.borrow();
        (0..props.len()).filter(|&i| !props[i].is_undef()).collect()
    };
    with_extra(obj, |x| x.reinitable = slots);
}

/// Whether a declared slot is uninitialised (`IS_PROP_UNINIT`): undefined and
/// never `unset()`.
fn slot_uninit(obj: &ObjRef, slot: usize) -> bool {
    obj.obj().props.borrow().get(slot).is_none_or(Value::is_undef) && !slot_was_unset(obj, slot)
}

fn slot_value(obj: &ObjRef, slot: usize) -> Value {
    obj.obj().props.borrow().get(slot).cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Lookup (zend_get_property_offset)
// ---------------------------------------------------------------------------

pub(crate) enum Lookup {
    Declared(PropertyInfo),
    Dynamic,
    /// Declared but not accessible from the scope.
    Denied(PropertyInfo),
    /// A name starting with `"\0"`.
    BadName,
}

/// `is_protected_compatible_scope`.
pub(crate) fn protected_ok(decl_class: &[u8], scope: Option<&ClassRef>, vm: &Vm) -> bool {
    let Some(scope) = scope else { return false };
    if scope.lc_name.eq_ignore_ascii_case(decl_class) || scope.name.eq_ignore_ascii_case(decl_class) {
        return true;
    }
    if scope.instance_of_name(decl_class) {
        return true;
    }
    match vm.find_class(decl_class) {
        Some(d) => d.instance_of(scope),
        None => false,
    }
}

/// Class of the root declaration (`prop_info->prototype->ce`): the topmost
/// non-private declaration of the property in the class hierarchy.
fn prop_root_class(class: &ClassRef, name: &[u8]) -> PhpStr {
    let mut root = None;
    let mut c = Some(class.clone());
    while let Some(ce) = c {
        if let Some(p) = ce.properties.get(name) {
            if p.flags & member_flags::PRIVATE == 0 {
                root = Some(p.declaring_class.clone());
            } else {
                break;
            }
        } else {
            break;
        }
        c = ce.parent.clone();
    }
    root.unwrap_or_else(|| class.name.clone())
}

pub(crate) fn lookup_prop(vm: &Vm, class: &ClassRef, name: &[u8], scope: Option<&ClassRef>) -> Lookup {
    let Some(info) = class.properties.get(name) else {
        if name.first() == Some(&0) {
            return Lookup::BadName;
        }
        return Lookup::Dynamic;
    };
    let flags = info.flags;
    if flags & (member_flags::PRIVATE | member_flags::PROTECTED | member_flags::CHANGED) != 0 {
        let same = scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class));
        if !same {
            if flags & member_flags::CHANGED != 0 {
                // zend_get_parent_private_property
                if let Some(s) = scope {
                    if !std::ptr::eq(&**s, &**class) && class.instance_of(s) {
                        if let Some(p) = s.properties.get(name) {
                            if p.flags & member_flags::PRIVATE != 0
                                && p.declaring_class.eq_ignore_ascii_case(&s.name)
                                && (!p.is_static() || info.is_static())
                            {
                                if p.is_static() {
                                    return Lookup::Dynamic;
                                }
                                return Lookup::Declared(p.clone());
                            }
                        }
                    }
                }
                if flags & member_flags::PUBLIC != 0 {
                    return found(info);
                }
            }
            if flags & member_flags::PRIVATE != 0 {
                if !info.declaring_class.eq_ignore_ascii_case(&class.name) {
                    // A parent's private property: invisible here (dynamic).
                    return Lookup::Dynamic;
                }
                return Lookup::Denied(info.clone());
            } else if flags & member_flags::PROTECTED != 0 {
                let root = prop_root_class(class, name);
                if !protected_ok(&root, scope, vm) {
                    return Lookup::Denied(info.clone());
                }
            }
        }
    } else if let Some(s) = scope {
        // A private property of the scope class shadowed by a public one
        // redeclared in a subclass without ZEND_ACC_CHANGED.
        let _ = s;
    }
    found(info)
}

fn found(info: &PropertyInfo) -> Lookup {
    if info.is_static() {
        return Lookup::Dynamic;
    }
    Lookup::Declared(info.clone())
}

fn visibility_word(flags: u32) -> &'static str {
    if flags & member_flags::PRIVATE != 0 {
        "private"
    } else if flags & member_flags::PROTECTED != 0 {
        "protected"
    } else {
        "public"
    }
}

/// `zend_bad_property_access`.
fn denied_error(vm: &mut Vm, info: &PropertyInfo, class: &ClassRef) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Cannot access {} property {}::${}",
            visibility_word(info.flags),
            display_class_name(&class.name),
            info.name
        ),
    )
}

fn bad_name_error(vm: &mut Vm) -> Flow {
    vm.throw_error("Error", "Cannot access property starting with \"\\0\"")
}

/// The error of a wrong (inaccessible) property offset (`zend_wrong_offset`).
fn wrong_offset_error(vm: &mut Vm, lookup: &Lookup, class: &ClassRef) -> Flow {
    match lookup {
        Lookup::Denied(info) => denied_error(vm, info, class),
        _ => bad_name_error(vm),
    }
}

fn uninit_error(vm: &mut Vm, info: &PropertyInfo) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Typed property {}::${} must not be accessed before initialization",
            display_class_name(&info.declaring_class),
            info.name
        ),
    )
}

fn readonly_modification_error(vm: &mut Vm, info: &PropertyInfo) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Cannot modify readonly property {}::${}",
            display_class_name(&info.declaring_class),
            info.name
        ),
    )
}

fn readonly_indirect_modification_error(vm: &mut Vm, info: &PropertyInfo) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Cannot indirectly modify readonly property {}::${}",
            display_class_name(&info.declaring_class),
            info.name
        ),
    )
}

/// The called scope of the running code (`zend_get_called_scope`).
fn called_scope(vm: &Vm) -> Option<ClassRef> {
    for f in vm.frames.iter().rev() {
        if let Some(env) = &f.env {
            return env.called.clone().or_else(|| env.scope.clone());
        }
    }
    None
}

/// `zend_asymmetric_visibility_property_modification_error`.
fn asymmetric_error(vm: &mut Vm, info: &PropertyInfo, op: &str) -> Flow {
    let vis = if info.flags & member_flags::PRIVATE_SET != 0 {
        "private(set)"
    } else if info.is_readonly() {
        "protected(set) readonly"
    } else {
        "protected(set)"
    };
    let from = match called_scope(vm) {
        Some(s) => format!("scope {}", display_class_name(&s.name)),
        None => "global scope".to_string(),
    };
    vm.throw_error(
        "Error",
        format!(
            "Cannot {op} {vis} property {}::${} from {from}",
            display_class_name(&info.declaring_class),
            info.name
        ),
    )
}

/// `zend_asymmetric_property_has_set_access` (for properties with a
/// restricted set visibility).
fn has_set_access(vm: &Vm, class: &ClassRef, info: &PropertyInfo, scope: Option<&ClassRef>) -> bool {
    if scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class)) {
        return true;
    }
    info.flags & member_flags::PROTECTED_SET != 0 && protected_ok(&prop_root_class(class, &info.name), scope, vm)
}

/// Whether a write to `info` needs the readonly / set-visibility checks and
/// they fail.
fn set_restricted(vm: &Vm, class: &ClassRef, info: &PropertyInfo, scope: Option<&ClassRef>) -> bool {
    info.is_readonly() || (info.flags & SET_MASK != 0 && !has_set_access(vm, class, info, scope))
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

fn is_virtual(info: &PropertyInfo) -> bool {
    info.flags & member_flags::VIRTUAL != 0
}

/// `zend_should_call_hook`: hooks are bypassed inside the property's own
/// hooks on the same object (backing value access).
fn should_call_hook(vm: &Vm, info: &PropertyInfo, obj: &ObjRef) -> bool {
    !super::decl::in_hook_of(vm, obj, &info.name)
}

fn hook_caller(vm: &Vm, strict: bool) -> super::call::Caller {
    super::call::Caller {
        strict,
        site: Some((vm.current_file.clone(), vm.current_line)),
    }
}

fn caller_strict(vm: &mut Vm) -> bool {
    super::call::calling_env(vm).strict
}

/// Calls a hook or magic method of `obj` from the current location.
fn call_on(vm: &mut Vm, obj: &ObjRef, f: &Rc<Function>, args: Vec<Value>, strict: bool) -> PResult<Value> {
    let caller = hook_caller(vm, strict);
    let b = super::call::Bound {
        positional: args,
        named: Vec::new(),
    };
    super::call::invoke(vm, f, b, Some(obj.clone()), Some(obj.class().clone()), None, &caller, false)
}

fn no_backing_error(vm: &mut Vm, class: &ClassRef, name: &[u8], read: bool) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Must not {} virtual property {}::${}",
            if read { "read from" } else { "write to" },
            display_class_name(&class.name),
            String::from_utf8_lossy(name)
        ),
    )
}

fn set_only_error(vm: &mut Vm, class: &ClassRef, name: &[u8]) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Cannot read from set-only virtual property {}::${}",
            display_class_name(&class.name),
            String::from_utf8_lossy(name)
        ),
    )
}

fn indirect_hook_error(vm: &mut Vm, class: &ClassRef, name: &[u8]) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Indirect modification of {}::${} is not allowed",
            display_class_name(&class.name),
            String::from_utf8_lossy(name)
        ),
    )
}

/// How a property is resolved after looking at its hooks.
enum Resolved {
    /// The property slot; the info is kept only for typed properties
    /// (php-src's `prop_info`).
    Slot(usize, Option<PropertyInfo>),
    /// A hook call produced this value.
    Value(Value),
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Fetch type (`BP_VAR_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fetch {
    R,
    Is,
    W,
    Rw,
    Unset,
}

impl Fetch {
    fn is_write(self) -> bool {
        matches!(self, Fetch::W | Fetch::Rw | Fetch::Unset)
    }
}

fn typed(info: &PropertyInfo) -> Option<PropertyInfo> {
    info.ty.is_some().then(|| info.clone())
}

/// `$obj->name` (`zend_std_read_property`).
pub fn read_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, is: bool) -> PResult<Value> {
    if let Some(h) = obj.class().handlers.read_property {
        if let Some(v) = h(vm, obj, name)? {
            return Ok(v.into_unref());
        }
    }
    let v = read_property(vm, scope, obj, name, if is { Fetch::Is } else { Fetch::R })?;
    Ok(v.into_unref())
}

pub(crate) fn read_property(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, ty: Fetch) -> PResult<Value> {
    let class = obj.class().clone();
    let lookup = lookup_prop(vm, &class, name, scope);
    let silent = ty == Fetch::Is || class.magic.get.is_some();
    // The property slot (valid offset) and its typed info.
    let mut slot: Option<(usize, Option<PropertyInfo>)> = None;
    match &lookup {
        Lookup::Declared(info) => match prop_hooks(info) {
            Some(hooks) => match read_hooked(vm, obj, &class, info, hooks, name, ty)? {
                Resolved::Value(v) => return Ok(v),
                Resolved::Slot(s, ti) => slot = Some((s, ti)),
            },
            None => slot = Some((info.slot, typed(info))),
        },
        Lookup::Dynamic => {
            let found = obj.obj().dyn_props.borrow().as_ref().and_then(|d| d.get_str(name).cloned());
            if let Some(v) = found {
                return Ok(v);
            }
        }
        Lookup::Denied(_) | Lookup::BadName => {
            if !silent {
                return Err(wrong_offset_error(vm, &lookup, &class));
            }
        }
    }
    let mut prop_info: Option<PropertyInfo> = None;
    if let Some((s, ti)) = slot {
        let v = slot_value(obj, s);
        if let Some(info) = &ti {
            if ty.is_write() && set_restricted(vm, &class, info, scope) {
                if matches!(v, Value::Object(_)) {
                    return Ok(v);
                }
                if v.is_undef() && ty == Fetch::Unset {
                    return Ok(Value::Null);
                }
                if info.is_readonly() {
                    return Err(readonly_indirect_modification_error(vm, info));
                }
                return Err(asymmetric_error(vm, info, "indirectly modify"));
            }
        }
        if !v.is_undef() {
            return Ok(v);
        }
        if ti.is_some() && slot_uninit(obj, s) {
            // Skip __get() for uninitialized typed properties.
            return uninit_read(vm, &class, ti.as_ref(), name, ty);
        }
        prop_info = ti;
    }

    // Magic isset (`??`, `isset()`-like reads).
    if ty == Fetch::Is && class.magic.isset.is_some() {
        if !guard_get(obj, name, IN_ISSET) {
            guard_set(obj, name, IN_ISSET, true);
            let r = call_magic(vm, obj, b"__isset", vec![Value::String(name.clone())]);
            let r = match r {
                Ok(v) => v,
                Err(e) => {
                    guard_set(obj, name, IN_ISSET, false);
                    return Err(e);
                }
            };
            let r = super::eval::to_bool(vm, &r);
            let truthy = match r {
                Ok(b) => b,
                Err(e) => {
                    guard_set(obj, name, IN_ISSET, false);
                    return Err(e);
                }
            };
            if !truthy {
                guard_set(obj, name, IN_ISSET, false);
                return Ok(Value::Null);
            }
            // __isset() may have materialised the property (GH-12695).
            let now = match &lookup {
                Lookup::Declared(info) if prop_hooks(info).is_none() => {
                    let v = slot_value(obj, info.slot);
                    (!v.is_undef()).then_some(v)
                }
                Lookup::Dynamic => obj.obj().dyn_props.borrow().as_ref().and_then(|d| d.get_str(name).cloned()),
                _ => None,
            };
            if let Some(v) = now {
                guard_set(obj, name, IN_ISSET, false);
                return Ok(v);
            }
            if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                let r = call_getter(vm, obj, &class, name, prop_info.as_ref(), ty);
                guard_set(obj, name, IN_ISSET, false);
                return r;
            }
            guard_set(obj, name, IN_ISSET, false);
            return uninit_read(vm, &class, prop_info.as_ref(), name, ty);
        } else if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
            return call_getter(vm, obj, &class, name, prop_info.as_ref(), ty);
        }
    } else if class.magic.get.is_some() {
        if !guard_get(obj, name, IN_GET) {
            return call_getter(vm, obj, &class, name, prop_info.as_ref(), ty);
        } else if matches!(lookup, Lookup::Denied(_) | Lookup::BadName) {
            return Err(wrong_offset_error(vm, &lookup, &class));
        }
    }
    uninit_read(vm, &class, prop_info.as_ref(), name, ty)
}

/// The `uninit_error` tail of `zend_std_read_property`.
fn uninit_read(vm: &mut Vm, class: &ClassRef, prop_info: Option<&PropertyInfo>, name: &[u8], ty: Fetch) -> PResult<Value> {
    if ty != Fetch::Is {
        match prop_info {
            Some(info) => return Err(uninit_error(vm, info)),
            None => undefined_property(vm, class, name)?,
        }
    }
    Ok(Value::Null)
}

/// Hooked property read: calls the get hook or resolves to the backing slot.
fn read_hooked(vm: &mut Vm, obj: &ObjRef, class: &ClassRef, info: &PropertyInfo, hooks: &HookSet, name: &PhpStr, ty: Fetch) -> PResult<Resolved> {
    let Some(get) = hooks.get.clone() else {
        if is_virtual(info) {
            return Err(set_only_error(vm, class, name));
        }
        let v = slot_value(obj, info.slot);
        if v.is_undef() {
            return Err(uninit_error(vm, info));
        }
        if ty.is_write() {
            if !matches!(v.deref().type_code(), crate::value::TypeCode::Object) {
                return Err(indirect_hook_error(vm, class, name));
            }
            return Ok(Resolved::Value(v.into_unref()));
        }
        return Ok(Resolved::Value(v));
    };
    if !should_call_hook(vm, info, obj) {
        if is_virtual(info) {
            return Err(no_backing_error(vm, class, name, true));
        }
        return Ok(Resolved::Slot(info.slot, typed(info)));
    }
    let strict = caller_strict(vm);
    let rv = call_on(vm, obj, &get, Vec::new(), strict)?;
    if !rv.is_ref() && ty.is_write() && !matches!(rv, Value::Object(_)) {
        return Err(indirect_hook_error(vm, class, name));
    }
    Ok(Resolved::Value(rv))
}

/// Calls `__get` (`zend_std_call_getter` with the guard and the checks of
/// `zend_std_read_property`).
fn call_getter(vm: &mut Vm, obj: &ObjRef, class: &ClassRef, name: &PhpStr, prop_info: Option<&PropertyInfo>, ty: Fetch) -> PResult<Value> {
    guard_set(obj, name, IN_GET, true);
    let r = call_magic(vm, obj, b"__get", vec![Value::String(name.clone())]);
    guard_set(obj, name, IN_GET, false);
    let mut rv = r?;
    if !rv.is_ref() && ty.is_write() && !matches!(rv, Value::Object(_)) {
        vm.engine_notice(format!(
            "Indirect modification of overloaded property {}::${} has no effect",
            display_class_name(&class.name),
            name
        ))?;
    }
    if let Some(info) = prop_info {
        // zend_verify_prop_assignable_by_ref_ex(..., MAGIC_GET)
        let strict = class.magic.get.as_ref().and_then(|f| super::user_fn(f)).is_some_and(|u| u.code.strict());
        let inner = rv.deref().clone();
        match coerce_prop_silent(vm, info, inner.clone(), strict)? {
            Some(c) => {
                if !c.identical(&inner) {
                    match &rv {
                        Value::Ref(r) => {
                            r.set(c);
                        }
                        _ => rv = c,
                    }
                }
            }
            None => {
                return Err(vm.throw_type_error(format!(
                    "Value of type {} returned from {}::__get() must be compatible with unset property {}::${} of type {}",
                    inner.type_name(),
                    display_class_name(&info.declaring_class),
                    display_class_name(&info.declaring_class),
                    info.name,
                    info.ty.as_ref().map(|t| t.to_type_string()).unwrap_or_default()
                )));
            }
        }
    }
    Ok(rv)
}

fn undefined_property(vm: &mut Vm, class: &ClassRef, name: &[u8]) -> PResult<()> {
    let mut m = format!("Undefined property: {}::$", display_class_name(&class.name)).into_bytes();
    m.extend_from_slice(name);
    vm.engine_warning(PhpStr::from_vec(m))
}

fn call_magic(vm: &mut Vm, obj: &ObjRef, method: &[u8], args: Vec<Value>) -> PResult<Value> {
    let m = obj.class().find_method(method).cloned().expect("magic method present");
    let strict = caller_strict(vm);
    call_on(vm, obj, &m, args, strict)
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

/// Coerces a value for a typed property (`zend_verify_property_type`).
pub(crate) fn coerce_prop(vm: &mut Vm, info: &PropertyInfo, v: Value, strict: bool) -> PResult<Value> {
    match coerce_prop_silent(vm, info, v.clone(), strict)? {
        Some(c) => Ok(c),
        None => Err(vm.throw_type_error(format!(
            "Cannot assign {} to property {}::${} of type {}",
            v.value_name(),
            display_class_name(&info.declaring_class),
            info.name,
            info.ty.as_ref().map(|t| t.to_type_string()).unwrap_or_default()
        ))),
    }
}

/// Type check/coercion without the error (`i_zend_verify_property_type`).
fn coerce_prop_silent(vm: &mut Vm, info: &PropertyInfo, v: Value, strict: bool) -> PResult<Option<Value>> {
    let Some(ty) = &info.ty else { return Ok(Some(v)) };
    if ty.is_mixed() {
        return Ok(Some(v));
    }
    let scope = vm.find_class(&info.declaring_class);
    if super::call::type_matches(vm, ty, &v, scope.as_ref())? {
        return Ok(Some(v));
    }
    if !v.is_null() {
        let mode = CoerceMode { strict, silent: false };
        if let Some(c) = vm.coerce_to_type(ty, v.clone(), mode)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// `$obj->name = $value` (`zend_std_write_property`). Returns the assigned value.
pub fn write_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, value: Value, strict: bool) -> PResult<Value> {
    if let Some(h) = obj.class().handlers.write_property {
        if h(vm, obj, name, value.clone())? {
            return Ok(value);
        }
    }
    let value = value.into_unref();
    let class = obj.class().clone();
    let lookup = lookup_prop(vm, &class, name, scope);
    let mut guard_checked = false;
    // (slot, typed info)
    let slot: Option<(usize, Option<PropertyInfo>)>;
    match &lookup {
        Lookup::Declared(info) => {
            let mut target = (info.slot, typed(info));
            if let Some(hooks) = prop_hooks(info) {
                match &hooks.set {
                    None => {
                        if is_virtual(info) {
                            return Err(vm.throw_error(
                                "Error",
                                format!(
                                    "Cannot write to get-only virtual property {}::${}",
                                    display_class_name(&class.name),
                                    name
                                ),
                            ));
                        }
                    }
                    Some(set) => {
                        if !should_call_hook(vm, info, obj) {
                            if is_virtual(info) {
                                return Err(no_backing_error(vm, &class, name, false));
                            }
                        } else {
                            if info.flags & SET_MASK != 0 && !has_set_access(vm, &class, info, scope) {
                                return Err(asymmetric_error(vm, info, "modify"));
                            }
                            let set = set.clone();
                            call_on(vm, obj, &set, vec![value.clone()], strict)?;
                            return Ok(value);
                        }
                    }
                }
                target = (info.slot, typed(info));
            }
            let (s, ti) = target;
            let cur = slot_value(obj, s);
            if let Some(info) = &ti {
                if info.is_readonly() || info.flags & SET_MASK != 0 {
                    let error = if !cur.is_undef() || slot_uninit(obj, s) || class.magic.set.is_none() {
                        true
                    } else {
                        guard_checked = true;
                        guard_get(obj, name, IN_SET)
                    };
                    if error {
                        if info.is_readonly() && !cur.is_undef() && !slot_reinitable(obj, s) {
                            return Err(readonly_modification_error(vm, info));
                        }
                        if info.flags & SET_MASK != 0 && !has_set_access(vm, &class, info, scope) {
                            return Err(asymmetric_error(vm, info, "modify"));
                        }
                    }
                }
            }
            if !cur.is_undef() {
                return assign_slot(vm, obj, s, ti.as_ref(), value, strict);
            }
            if slot_uninit(obj, s) {
                // Writes to uninitialized typed properties bypass __set().
                return assign_slot(vm, obj, s, ti.as_ref(), value, strict);
            }
            slot = Some((s, ti));
        }
        Lookup::Dynamic => {
            {
                let mut d = obj.obj().dyn_props.borrow_mut();
                if let Some(t) = d.as_mut() {
                    if let Some(slot) = t.get_mut(&ArrayKey::Str(name.clone())) {
                        let old = match slot {
                            Value::Ref(r) => r.set(value.clone()),
                            s => std::mem::replace(s, value.clone()),
                        };
                        drop(d);
                        drop(old);
                        return Ok(value);
                    }
                }
            }
            slot = None;
        }
        Lookup::Denied(_) | Lookup::BadName => {
            if class.magic.set.is_none() {
                return Err(wrong_offset_error(vm, &lookup, &class));
            }
            slot = None;
        }
    }
    let _ = guard_checked;
    // Magic set.
    if class.magic.set.is_some() {
        if !guard_get(obj, name, IN_SET) {
            return magic_set(vm, obj, name, value);
        }
        if matches!(lookup, Lookup::Denied(_) | Lookup::BadName) {
            return Err(wrong_offset_error(vm, &lookup, &class));
        }
    }
    // write_std_property
    match slot {
        Some((s, ti)) => assign_slot(vm, obj, s, ti.as_ref(), value, strict),
        None => create_dynamic(vm, obj, name, value),
    }
}

/// Assigns to a declared slot with the property type check
/// (`typed_property:` / `found:` in `zend_std_write_property`).
fn assign_slot(vm: &mut Vm, obj: &ObjRef, slot: usize, info: Option<&PropertyInfo>, value: Value, strict: bool) -> PResult<Value> {
    let v = match info {
        Some(info) => {
            let v = coerce_prop(vm, info, value, strict)?;
            clear_reinitable(obj, slot);
            v
        }
        None => value,
    };
    store_slot(obj, slot, v.clone());
    mark_slot_unset(obj, slot, false);
    Ok(v)
}

fn store_slot(obj: &ObjRef, slot: usize, v: Value) {
    let old = {
        let mut props = obj.obj().props.borrow_mut();
        match &mut props[slot] {
            Value::Ref(r) => r.set(v),
            s => std::mem::replace(s, v),
        }
    };
    drop(old);
}

fn magic_set(vm: &mut Vm, obj: &ObjRef, name: &PhpStr, value: Value) -> PResult<Value> {
    guard_set(obj, name, IN_SET, true);
    let r = call_magic(vm, obj, b"__set", vec![Value::String(name.clone()), value.clone()]);
    guard_set(obj, name, IN_SET, false);
    r?;
    Ok(value)
}

/// Whether dynamic properties may be created on `class` without deprecation.
fn allows_dynamic(class: &ClassRef) -> bool {
    let mut c = Some(class);
    while let Some(ce) = c {
        if ce.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
            return true;
        }
        c = ce.parent.as_ref();
    }
    false
}

fn create_dynamic(vm: &mut Vm, obj: &ObjRef, name: &PhpStr, value: Value) -> PResult<Value> {
    let class = obj.class().clone();
    if name.first() == Some(&0) {
        return Err(bad_name_error(vm));
    }
    if class.has_flag(class_flags::NO_DYNAMIC_PROPERTIES) || class.has_flag(class_flags::READONLY) || class.is_enum() {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot create dynamic property {}::${}", display_class_name(&class.name), name),
        ));
    }
    if !allows_dynamic(&class) {
        vm.engine_deprecated(format!(
            "Creation of dynamic property {}::${} is deprecated",
            display_class_name(&class.name),
            name
        ))?;
    }
    let mut d = obj.obj().dyn_props.borrow_mut();
    let t = d.get_or_insert_with(PhpArray::new);
    t.insert(ArrayKey::Str(name.clone()), value.clone());
    Ok(value)
}

// ---------------------------------------------------------------------------
// Fetch for write (zend_fetch_property_address / get_property_ptr_ptr)
// ---------------------------------------------------------------------------

/// How a property is fetched for a nested write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FetchMode {
    Write,
    ReadWrite,
    Unset,
}

/// A property fetched for writing into it (`ZEND_FETCH_OBJ_W`).
pub enum PropTarget {
    /// The property slot, made a reference.
    Ref(PhpRef),
    /// A temporary (from `__get` or a hook): writes have no effect.
    Value(Value),
    /// Nothing to unset.
    Missing,
}

/// `ZEND_FETCH_OBJ_FLAGS`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjFlags {
    None,
    DimWrite,
    Ref,
}

/// The result of `get_property_ptr_ptr`.
enum Ptr {
    /// A declared slot (with its typed info) or a dynamic property.
    Slot(usize, Option<PropertyInfo>),
    Dyn,
    /// No direct pointer: use `read_property`.
    Null,
}

/// `zend_std_get_property_ptr_ptr`.
fn get_property_ptr(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, ty: Fetch) -> PResult<Ptr> {
    let class = obj.class().clone();
    let lookup = lookup_prop(vm, &class, name, scope);
    let (s, ti) = match &lookup {
        Lookup::Declared(info) => match prop_hooks(info) {
            Some(_) => {
                if !is_virtual(info) && !should_call_hook(vm, info, obj) {
                    (info.slot, typed(info))
                } else {
                    return Ok(Ptr::Null);
                }
            }
            None => (info.slot, typed(info)),
        },
        Lookup::Dynamic => {
            let exists = obj.obj().dyn_props.borrow().as_ref().is_some_and(|d| d.get_str(name).is_some());
            if exists {
                return Ok(Ptr::Dyn);
            }
            if class.magic.get.is_none() || guard_get(obj, name, IN_GET) {
                if class.has_flag(class_flags::NO_DYNAMIC_PROPERTIES) || class.has_flag(class_flags::READONLY) || class.is_enum() {
                    return Err(vm.throw_error(
                        "Error",
                        format!("Cannot create dynamic property {}::${}", display_class_name(&class.name), name),
                    ));
                }
                if !allows_dynamic(&class) {
                    vm.engine_deprecated(format!(
                        "Creation of dynamic property {}::${} is deprecated",
                        display_class_name(&class.name),
                        name
                    ))?;
                }
                if ty == Fetch::Rw {
                    undefined_property(vm, &class, name)?;
                }
                let mut d = obj.obj().dyn_props.borrow_mut();
                d.get_or_insert_with(PhpArray::new).insert(ArrayKey::Str(name.clone()), Value::Null);
                return Ok(Ptr::Dyn);
            }
            return Ok(Ptr::Null);
        }
        Lookup::Denied(_) | Lookup::BadName => {
            if class.magic.get.is_none() {
                return Err(wrong_offset_error(vm, &lookup, &class));
            }
            return Ok(Ptr::Null);
        }
    };
    let cur = slot_value(obj, s);
    if cur.is_undef() {
        let uninit = ti.is_some() && slot_uninit(obj, s);
        if class.magic.get.is_none() || guard_get(obj, name, IN_GET) || uninit {
            if ty == Fetch::Rw {
                match &ti {
                    Some(info) => return Err(uninit_error(vm, info)),
                    None => {
                        undefined_property(vm, &class, name)?;
                        if slot_value(obj, s).is_undef() {
                            store_slot(obj, s, Value::Null);
                            mark_slot_unset(obj, s, false);
                        }
                    }
                }
            } else if let Some(info) = ti.as_ref().filter(|i| i.is_readonly() || i.flags & SET_MASK != 0) {
                if info.is_readonly() || !has_set_access(vm, &class, info, scope) {
                    return Ok(Ptr::Null);
                }
            } else if ti.is_none() {
                store_slot(obj, s, Value::Null);
                mark_slot_unset(obj, s, false);
            }
        } else {
            return Ok(Ptr::Null);
        }
    } else if let Some(info) = ti.as_ref().filter(|i| i.is_readonly() || i.flags & SET_MASK != 0) {
        if info.is_readonly() || !has_set_access(vm, &class, info, scope) {
            return Ok(Ptr::Null);
        }
    }
    Ok(Ptr::Slot(s, ti))
}

/// `zend_fetch_property_address` for W/RW/UNSET fetches.
fn fetch_property_address(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, ty: Fetch, flags: ObjFlags) -> PResult<PropTarget> {
    match get_property_ptr(vm, scope, obj, name, ty)? {
        Ptr::Null => {
            let v = read_property(vm, scope, obj, name, ty)?;
            Ok(match v {
                Value::Ref(r) => PropTarget::Ref(r),
                v => PropTarget::Value(v),
            })
        }
        Ptr::Dyn => {
            let mut d = obj.obj().dyn_props.borrow_mut();
            let t = d.get_or_insert_with(PhpArray::new);
            Ok(PropTarget::Ref(crate::value::array::make_element_ref(t, ArrayKey::Str(name.clone()))))
        }
        Ptr::Slot(s, ti) => {
            let cur = slot_value(obj, s);
            if ty == Fetch::Unset && cur.is_undef() {
                return Ok(PropTarget::Missing);
            }
            if let Some(info) = &ti {
                match flags {
                    ObjFlags::DimWrite => {
                        // promotes_to_array: undef/null/false
                        let promotes = matches!(cur.deref().type_code(), crate::value::TypeCode::Undef | crate::value::TypeCode::Null)
                            || matches!(&*cur.deref(), Value::Bool(false));
                        if promotes {
                            let ty = info.ty.as_ref().expect("typed");
                            if ty.mask & crate::types::mask::ARRAY == 0 && !ty.is_mixed() {
                                return Err(vm.throw_type_error(format!(
                                    "Cannot auto-initialize an array inside property {}::${} of type {}",
                                    display_class_name(&info.declaring_class),
                                    info.name,
                                    ty.to_type_string()
                                )));
                            }
                        }
                    }
                    ObjFlags::Ref => {
                        if cur.is_undef() {
                            let ty = info.ty.as_ref().expect("typed");
                            if ty.mask & crate::types::mask::NULL == 0 && !ty.is_mixed() {
                                return Err(vm.throw_error(
                                    "Error",
                                    format!(
                                        "Cannot access uninitialized non-nullable property {}::${} by reference",
                                        display_class_name(&info.declaring_class),
                                        info.name
                                    ),
                                ));
                            }
                        }
                    }
                    ObjFlags::None => {}
                }
            }
            let mut props = obj.obj().props.borrow_mut();
            let slot = &mut props[s];
            let r = match slot {
                Value::Ref(r) => r.clone(),
                other => {
                    let v = std::mem::take(other);
                    let r = PhpRef::new(if v.is_undef() { Value::Null } else { v });
                    *other = Value::Ref(r.clone());
                    r
                }
            };
            drop(props);
            mark_slot_unset(obj, s, false);
            Ok(PropTarget::Ref(r))
        }
    }
}

/// `$obj->name =& $ref` (`zend_assign_to_property_reference`).
pub fn assign_prop_ref(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, r: PhpRef) -> PResult<()> {
    match get_property_ptr(vm, scope, obj, name, Fetch::W)? {
        Ptr::Null => {
            // Errors of the fallback read (readonly, hooks) take precedence.
            let _ = read_property(vm, scope, obj, name, Fetch::W)?;
            Err(vm.throw_error("Error", "Cannot assign by reference to overloaded object"))
        }
        Ptr::Dyn => {
            let mut d = obj.obj().dyn_props.borrow_mut();
            let t = d.get_or_insert_with(PhpArray::new);
            let old = t.insert(ArrayKey::Str(name.clone()), Value::Ref(r));
            drop(d);
            drop(old);
            Ok(())
        }
        Ptr::Slot(s, ti) => {
            if let Some(info) = &ti {
                let v = r.get();
                let strict = caller_strict(vm);
                let c = coerce_prop(vm, info, v.clone(), strict)?;
                if !c.identical(&v) {
                    r.set(c);
                }
            }
            let old = std::mem::replace(&mut obj.obj().props.borrow_mut()[s], Value::Ref(r));
            drop(old);
            mark_slot_unset(obj, s, false);
            Ok(())
        }
    }
}

/// `&$obj->name`.
pub fn make_prop_ref(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr) -> PResult<PhpRef> {
    match fetch_property_address(vm, scope, obj, name, Fetch::W, ObjFlags::Ref)? {
        PropTarget::Ref(r) => Ok(r),
        PropTarget::Value(v) => Ok(PhpRef::new(v)),
        PropTarget::Missing => Ok(PhpRef::new(Value::Null)),
    }
}

/// Fetches a property for a nested write (`$obj->p[...] = ...`).
pub(crate) fn prop_for_write(vm: &mut Vm, cx: &Ctx, obj: &ObjRef, name: &PhpStr, mode: FetchMode, next_is_dim: bool) -> PResult<PropTarget> {
    let ty = match mode {
        FetchMode::Write => Fetch::W,
        FetchMode::ReadWrite => Fetch::Rw,
        FetchMode::Unset => Fetch::Unset,
    };
    let flags = if next_is_dim && mode == FetchMode::Write { ObjFlags::DimWrite } else { ObjFlags::None };
    fetch_property_address(vm, cx.scope(), obj, name, ty, flags)
}

/// `++$obj->p` / `$obj->p--` (`zend_pre_incdec_property_zval` /
/// `zend_pre_incdec_overloaded_property`).
pub(crate) fn incdec_prop(vm: &mut Vm, cx: &Ctx, obj: &ObjRef, name: &PhpStr, inc: bool, post: bool) -> PResult<Value> {
    let scope = cx.scope().cloned();
    let old = read_prop(vm, scope.as_ref(), obj, name, false)?;
    let new = if inc { vm.increment(&old)? } else { vm.decrement(&old)? };
    // Typed int properties must not overflow into float.
    if let (Value::Int(_), Value::Float(_)) = (&old, &new) {
        if let Lookup::Declared(info) = lookup_prop(vm, obj.class(), name, scope.as_ref()) {
            if let Some(ty) = &info.ty {
                if ty.mask & crate::types::mask::FLOAT == 0 && !ty.is_mixed() && prop_hooks(&info).is_none() {
                    if info.is_readonly() {
                        return Err(readonly_modification_error(vm, &info));
                    }
                    return Err(vm.throw_type_error(format!(
                        "Cannot {} property {}::${} of type {} past its {} value",
                        if inc { "increment" } else { "decrement" },
                        display_class_name(&info.declaring_class),
                        info.name,
                        ty.to_type_string(),
                        if inc { "maximal" } else { "minimal" }
                    )));
                }
            }
        }
    }
    write_prop(vm, scope.as_ref(), obj, name, new.clone(), cx.strict())?;
    Ok(if post { old } else { new })
}

/// `parent::$prop::get()` / `parent::$prop::set()`
/// (`ZEND_INIT_PARENT_PROPERTY_HOOK_CALL`): calls the parent's hook, or the
/// trampoline accessing the backing value (`zend_parent_hook_*_trampoline`).
pub(crate) fn parent_hook_call(vm: &mut Vm, cx: &Ctx, s: &crate::compile::ir::StaticCallExpr, pseudo: &PhpStr) -> PResult<Value> {
    vm.current_line = s.line;
    let is_get = pseudo.ends_with(b"::get");
    let prop = PhpStr::from_bytes(&pseudo[1..pseudo.len() - 5]);
    let scope = cx.scope().cloned();
    let Some(parent) = scope.as_ref().and_then(|c| c.parent.clone()) else {
        return Err(vm.throw_error("Error", "Cannot use \"parent\" when current class scope has no parent"));
    };
    let Some(info) = parent.properties.get(&prop).cloned() else {
        return Err(vm.throw_error(
            "Error",
            format!("Undefined property {}::${}", display_class_name(&parent.name), prop),
        ));
    };
    if info.flags & member_flags::PRIVATE != 0 {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot access private property {}::${}", display_class_name(&parent.name), prop),
        ));
    }
    let Some(this) = cx.env.this.clone() else {
        return Err(vm.throw_error("Error", "Using $this when not in object context"));
    };
    let hook = prop_hooks(&info).and_then(|h| if is_get { h.get.clone() } else { h.set.clone() });
    let caller = super::call::Caller {
        strict: cx.strict(),
        site: Some((vm.current_file.clone(), s.line)),
    };
    if let Some(f) = hook {
        let b = super::call::eval_args(vm, cx, &s.args, Some(&f))?;
        vm.current_line = s.line;
        return super::call::invoke(vm, &f, b, Some(this.clone()), Some(this.class().clone()), None, &caller, false);
    }
    // The trampoline: an internal function `P::$prop::get` / `::set` whose
    // scope is the declaring class of the parent property.
    let b = super::call::eval_args(vm, cx, &s.args, None)?;
    vm.current_line = s.line;
    let decl = vm.find_class(&info.declaring_class).unwrap_or_else(|| parent.clone());
    let fname = format!("{}::{}", display_class_name(&info.declaring_class), pseudo);
    let mut args = b.positional;
    for (n, v) in b.named {
        if !is_get && n.as_bytes() == b"value" {
            if !args.is_empty() {
                return Err(vm.throw_error("Error", "Named parameter $value overwrites previous argument"));
            }
            args.push(v);
        } else {
            return Err(vm.throw_error("Error", format!("Unknown named parameter ${n}")));
        }
    }
    let want = if is_get { 0 } else { 1 };
    if args.len() != want {
        return Err(vm.throw_error(
            "ArgumentCountError",
            format!(
                "{fname}() expects exactly {want} argument{}, {} given",
                if want == 1 { "" } else { "s" },
                args.len()
            ),
        ));
    }
    if is_get {
        Ok(read_property(vm, Some(&decl), &this, &prop, Fetch::R)?.into_unref())
    } else {
        let v = args.pop().unwrap_or_default().into_unref();
        write_prop(vm, Some(&decl), &this, &prop, v, cx.strict())
    }
}

// ---------------------------------------------------------------------------
// isset / unset
// ---------------------------------------------------------------------------

/// `isset($obj->name)` / `empty($obj->name)` (`zend_std_has_property`).
pub fn isset_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, empty: bool) -> PResult<bool> {
    if let Some(h) = obj.class().handlers.has_property {
        if let Some(b) = h(vm, obj, name, empty)? {
            return Ok(b);
        }
    }
    let class = obj.class().clone();
    let lookup = lookup_prop(vm, &class, name, scope);
    let check = |vm: &mut Vm, v: &Value| -> PResult<bool> {
        if empty {
            super::eval::to_bool(vm, &v.unref())
        } else {
            Ok(!v.unref().is_null())
        }
    };
    let mut slot: Option<usize> = None;
    match &lookup {
        Lookup::Declared(info) => {
            let mut s = info.slot;
            if let Some(hooks) = prop_hooks(info) {
                match &hooks.get {
                    None => {
                        if is_virtual(info) {
                            return Err(set_only_error(vm, &class, name));
                        }
                    }
                    Some(get) => {
                        if should_call_hook(vm, info, obj) {
                            let get = get.clone();
                            let strict = caller_strict(vm);
                            let rv = call_on(vm, obj, &get, Vec::new(), strict)?;
                            return check(vm, &rv);
                        }
                        if is_virtual(info) {
                            return Err(no_backing_error(vm, &class, name, true));
                        }
                    }
                }
                s = info.slot;
            }
            let v = slot_value(obj, s);
            if !v.is_undef() {
                return check(vm, &v);
            }
            if info.ty.is_some() && slot_uninit(obj, s) {
                // Skip __isset() for uninitialized typed properties.
                return Ok(false);
            }
            slot = Some(s);
        }
        Lookup::Dynamic => {
            let v = obj.obj().dyn_props.borrow().as_ref().and_then(|d| d.get_str(name).cloned());
            if let Some(v) = v {
                return check(vm, &v);
            }
        }
        Lookup::Denied(_) | Lookup::BadName => {}
    }
    if class.magic.isset.is_none() || guard_get(obj, name, IN_ISSET) {
        return Ok(false);
    }
    guard_set(obj, name, IN_ISSET, true);
    let r = (|| -> PResult<bool> {
        let rv = call_magic(vm, obj, b"__isset", vec![Value::String(name.clone())])?;
        let mut result = super::eval::to_bool(vm, &rv.unref())?;
        if empty && result {
            // GH-12695: __isset() may have materialised the property.
            let prop = match slot {
                Some(s) => {
                    let v = slot_value(obj, s);
                    (!v.is_undef()).then_some(v)
                }
                None if matches!(lookup, Lookup::Dynamic) => {
                    obj.obj().dyn_props.borrow().as_ref().and_then(|d| d.get_str(name).cloned())
                }
                None => None,
            };
            if let Some(p) = prop {
                result = super::eval::to_bool(vm, &p.unref())?;
            } else if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                guard_set(obj, name, IN_GET, true);
                let g = call_magic(vm, obj, b"__get", vec![Value::String(name.clone())]);
                guard_set(obj, name, IN_GET, false);
                result = super::eval::to_bool(vm, &g?.unref())?;
            } else {
                result = false;
            }
        }
        Ok(result)
    })();
    guard_set(obj, name, IN_ISSET, false);
    r
}

/// `unset($obj->name)` (`zend_std_unset_property`).
pub fn unset_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr) -> PResult<()> {
    if let Some(h) = obj.class().handlers.unset_property {
        if h(vm, obj, name)? {
            return Ok(());
        }
    }
    let class = obj.class().clone();
    let lookup = lookup_prop(vm, &class, name, scope);
    let mut guard_used = false;
    match &lookup {
        Lookup::Declared(info) => {
            if prop_hooks(info).is_some() {
                return Err(vm.throw_error(
                    "Error",
                    format!("Cannot unset hooked property {}::${}", display_class_name(&class.name), name),
                ));
            }
            let s = info.slot;
            let cur = slot_value(obj, s);
            let ti = typed(info);
            if let Some(info) = ti.as_ref().filter(|i| i.is_readonly() || i.flags & SET_MASK != 0) {
                let error = if !cur.is_undef() || slot_uninit(obj, s) || class.magic.unset.is_none() {
                    true
                } else {
                    guard_used = true;
                    guard_get(obj, name, IN_UNSET)
                };
                if error {
                    if info.is_readonly() && !cur.is_undef() && !slot_reinitable(obj, s) {
                        return Err(vm.throw_error(
                            "Error",
                            format!(
                                "Cannot unset readonly property {}::${}",
                                display_class_name(&info.declaring_class),
                                name
                            ),
                        ));
                    }
                    if info.flags & SET_MASK != 0 && !has_set_access(vm, &class, info, scope) {
                        return Err(asymmetric_error(vm, info, "unset"));
                    }
                }
            }
            if !cur.is_undef() {
                let old = std::mem::take(&mut obj.obj().props.borrow_mut()[s]);
                mark_slot_unset(obj, s, true);
                drop(old);
                return Ok(());
            }
            if ti.is_some() && slot_uninit(obj, s) {
                // Reset the IS_PROP_UNINIT flag and bypass __unset().
                mark_slot_unset(obj, s, true);
                return Ok(());
            }
        }
        Lookup::Dynamic => {
            let removed = {
                let mut d = obj.obj().dyn_props.borrow_mut();
                d.as_mut().and_then(|t| t.remove(&ArrayKey::Str(name.clone())))
            };
            if removed.is_some() {
                drop(removed);
                return Ok(());
            }
        }
        Lookup::Denied(_) | Lookup::BadName => {
            if class.magic.unset.is_none() {
                return Err(wrong_offset_error(vm, &lookup, &class));
            }
        }
    }
    let _ = guard_used;
    if class.magic.unset.is_some() {
        if !guard_get(obj, name, IN_UNSET) {
            return magic_unset(vm, obj, name);
        }
        if matches!(lookup, Lookup::Denied(_) | Lookup::BadName) {
            return Err(wrong_offset_error(vm, &lookup, &class));
        }
    }
    Ok(())
}

fn magic_unset(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<()> {
    guard_set(obj, name, IN_UNSET, true);
    let r = call_magic(vm, obj, b"__unset", vec![Value::String(name.clone())]);
    guard_set(obj, name, IN_UNSET, false);
    r.map(|_| ())
}

// ---------------------------------------------------------------------------
// ArrayAccess
// ---------------------------------------------------------------------------

fn is_array_access(obj: &ObjRef) -> bool {
    obj.class().instance_of_name(b"ArrayAccess")
}

fn cannot_use_as_array(vm: &mut Vm, obj: &ObjRef) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Cannot use object of type {} as array",
            super::decl::display_class_name(&obj.class().name)
        ),
    )
}

/// `$obj[$key]` read (`zend_std_read_dimension`).
pub fn dim_read_obj(vm: &mut Vm, obj: &ObjRef, key: &Value, is: bool) -> PResult<Value> {
    if let Some(h) = obj.class().handlers.read_dimension {
        let mode = if is { crate::class::DimFetch::IsSet } else { crate::class::DimFetch::Read };
        return Ok(h(vm, obj, Some(key), mode)?.into_unref());
    }
    if !is_array_access(obj) {
        return Err(cannot_use_as_array(vm, obj));
    }
    let k = key.unref();
    if is {
        let has = vm.call_method(obj, b"offsetExists", vec![k.clone()])?;
        if !super::eval::to_bool(vm, &has)? {
            return Ok(Value::Null);
        }
    }
    Ok(vm.call_method(obj, b"offsetGet", vec![k])?.into_unref())
}

/// `$obj[$key] = $value` (`zend_std_write_dimension`).
pub fn dim_write_obj(vm: &mut Vm, obj: &ObjRef, key: Option<&Value>, value: Value) -> PResult<()> {
    if let Some(h) = obj.class().handlers.write_dimension {
        return h(vm, obj, key, value);
    }
    if !is_array_access(obj) {
        return Err(cannot_use_as_array(vm, obj));
    }
    let k = key.map(Value::unref).unwrap_or(Value::Null);
    vm.call_method(obj, b"offsetSet", vec![k, value])?;
    Ok(())
}

/// `isset($obj[$key])` / `empty($obj[$key])`.
pub fn dim_isset_obj(vm: &mut Vm, obj: &ObjRef, key: &Value, empty: bool) -> PResult<bool> {
    if let Some(h) = obj.class().handlers.has_dimension {
        return h(vm, obj, key, empty);
    }
    if !is_array_access(obj) {
        return Err(cannot_use_as_array(vm, obj));
    }
    let k = key.unref();
    let has = vm.call_method(obj, b"offsetExists", vec![k.clone()])?;
    let has = super::eval::to_bool(vm, &has)?;
    if !has || !empty {
        return Ok(has);
    }
    let v = vm.call_method(obj, b"offsetGet", vec![k])?;
    super::eval::to_bool(vm, &v.into_unref())
}

/// `unset($obj[$key])`.
pub fn dim_unset_obj(vm: &mut Vm, obj: &ObjRef, key: &Value) -> PResult<()> {
    if let Some(h) = obj.class().handlers.unset_dimension {
        return h(vm, obj, key);
    }
    if !is_array_access(obj) {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Cannot use object of type {} as array",
                super::decl::display_class_name(&obj.class().name)
            ),
        ));
    }
    vm.call_method(obj, b"offsetUnset", vec![key.unref()])?;
    Ok(())
}

/// `$obj[$key][...] = ...`: `offsetGet` for a nested write (with the
/// "Indirect modification" notice when the result is a plain value).
pub fn dim_fetch_obj_for_write(vm: &mut Vm, obj: &ObjRef, key: Option<Value>, unset_mode: bool) -> PResult<Value> {
    if let Some(h) = obj.class().handlers.read_dimension {
        let mode = if unset_mode { crate::class::DimFetch::Read } else { crate::class::DimFetch::Write };
        return h(vm, obj, key.as_ref(), mode);
    }
    if !is_array_access(obj) {
        return Err(cannot_use_as_array(vm, obj));
    }
    let k = key.map(|k| k.into_unref()).unwrap_or(Value::Null);
    let v = vm.call_method(obj, b"offsetGet", vec![k])?;
    match &v {
        Value::Ref(_) | Value::Object(_) => Ok(v),
        _ => {
            if !unset_mode {
                vm.engine_notice(format!(
                    "Indirect modification of overloaded element of {} has no effect",
                    super::decl::display_class_name(&obj.class().name)
                ))?;
            }
            Ok(v)
        }
    }
}

// ---------------------------------------------------------------------------
// Static properties
// ---------------------------------------------------------------------------

/// Finds a static property (with visibility check), `None` if undeclared.
pub fn static_prop_ref_opt(vm: &mut Vm, scope: Option<&ClassRef>, class: &ClassRef, name: &[u8]) -> PResult<Option<PhpRef>> {
    super::decl::init_statics(vm, class)?;
    let Some(info) = class.properties.get(name).filter(|p| p.is_static()).cloned() else {
        return Ok(None);
    };
    if info.flags & member_flags::PRIVATE != 0 {
        if !scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class)) {
            return Ok(None);
        }
    } else if info.flags & member_flags::PROTECTED != 0 && !protected_ok(&info.declaring_class, scope, vm) {
        return Ok(None);
    }
    Ok(class.static_members.borrow().get(info.slot).cloned())
}

/// `A::$name` for read/write (`zend_fetch_static_property_address`).
pub fn static_prop_ref(vm: &mut Vm, scope: Option<&ClassRef>, class: &ClassRef, name: &[u8]) -> PResult<PhpRef> {
    super::decl::init_statics(vm, class)?;
    let info = class.properties.get(name).filter(|p| p.is_static()).cloned();
    let Some(info) = info else {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Access to undeclared static property {}::${}",
                super::decl::display_class_name(&class.name),
                String::from_utf8_lossy(name)
            ),
        ));
    };
    if info.flags & member_flags::PRIVATE != 0 {
        if !scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class)) {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Cannot access private property {}::${}",
                    super::decl::display_class_name(&class.name),
                    info.name
                ),
            ));
        }
    } else if info.flags & member_flags::PROTECTED != 0 && !protected_ok(&info.declaring_class, scope, vm) {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Cannot access protected property {}::${}",
                super::decl::display_class_name(&class.name),
                info.name
            ),
        ));
    }
    let r = class.static_members.borrow().get(info.slot).cloned();
    match r {
        Some(r) => Ok(r),
        None => Err(vm.throw_error("Error", "Static property storage missing")),
    }
}

pub fn uninit_static_error(vm: &mut Vm, class: &ClassRef, name: &[u8]) -> Flow {
    let decl = class
        .properties
        .get(name)
        .map(|p| p.declaring_class.clone())
        .unwrap_or_else(|| class.name.clone());
    vm.throw_error(
        "Error",
        format!(
            "Typed static property {}::${} must not be accessed before initialization",
            super::decl::display_class_name(&decl),
            String::from_utf8_lossy(name)
        ),
    )
}

// ---------------------------------------------------------------------------
// Instantiation, clone, destruction
// ---------------------------------------------------------------------------

/// `object_init_ex` for userland and internal classes, with constant-expression
/// property defaults evaluated.
pub fn instantiate(vm: &mut Vm, class: &ClassRef) -> PResult<ObjRef> {
    let defaults = super::decl::resolved_defaults(vm, class)?;
    let obj = match class.handlers.create_object {
        Some(f) => {
            let o = f(vm, class)?;
            if let Some(d) = &defaults {
                let mut props = o.obj().props.borrow_mut();
                for (slot, v) in d.iter().enumerate() {
                    if slot < props.len() && super::decl::slot_is_user(class, slot) {
                        props[slot] = v.clone();
                    }
                }
            }
            o
        }
        None => match defaults {
            Some(d) => ObjRef::with_props(class.clone(), (*d).clone()),
            None => ObjRef::new(class.clone()),
        },
    };
    Ok(obj)
}

/// `clone $v` (`ZEND_CLONE`) with optional `clone($v, [...])` property overrides.
pub fn clone_value(vm: &mut Vm, scope: Option<ClassRef>, v: Value, with: Option<Value>) -> PResult<Value> {
    let Value::Object(obj) = v.into_unref() else {
        return Err(vm.throw_error("Error", "__clone method called on non-object"));
    };
    let class = obj.class().clone();
    if class.is_enum() {
        return Err(vm.throw_error(
            "Error",
            format!("Trying to clone an uncloneable object of class {}", class.name),
        ));
    }
    if let Some(m) = &class.magic.clone {
        if m.flags & member_flags::PRIVATE != 0 && !scope.as_ref().is_some_and(|s| m.scope.as_ref().is_some_and(|ms| ms.eq_ignore_ascii_case(&s.name))) {
            let from = match &scope {
                Some(s) => format!("scope {}", s.name),
                None => "global scope".into(),
            };
            return Err(vm.throw_error(
                "Error",
                format!("Call to private {}::__clone() from {from}", class.name),
            ));
        }
        if m.flags & member_flags::PROTECTED != 0 && !protected_ok(m.scope.as_deref().unwrap_or(b""), scope.as_ref(), vm) {
            let from = match &scope {
                Some(s) => format!("scope {}", s.name),
                None => "global scope".into(),
            };
            return Err(vm.throw_error(
                "Error",
                format!("Call to protected {}::__clone() from {from}", class.name),
            ));
        }
    }
    let new = match class.handlers.clone_obj {
        Some(f) => f(vm, &obj)?,
        None => {
            if class.handlers.create_object.is_some() && class.is_internal() && class.handlers.clone_obj.is_none() && class.has_flag(class_flags::NO_DYNAMIC_INSTANTIATION) {
                return Err(vm.throw_error(
                    "Error",
                    format!("Trying to clone an uncloneable object of class {}", class.name),
                ));
            }
            std_clone(&obj)
        }
    };
    if let Some(m) = class.magic.clone.clone() {
        set_properties_reinitable(&new, true);
        let r = super::call::call_method_fn(vm, &new, &m, Vec::new(), None);
        set_properties_reinitable(&new, false);
        r?;
    }
    if let Some(with) = with {
        let Value::Array(a) = with.into_unref() else {
            return Err(vm.throw_type_error("clone(): Argument #2 ($withProperties) must be of type array"));
        };
        set_properties_reinitable(&new, true);
        let r = (|| -> PResult<()> {
            for (k, v) in a.iter() {
                let name = match k {
                    ArrayKey::Str(s) => s,
                    ArrayKey::Int(i) => PhpStr::from(i.to_string()),
                };
                write_prop(vm, scope.as_ref(), &new, &name, v.unref(), false)?;
            }
            Ok(())
        })();
        set_properties_reinitable(&new, false);
        r?;
    }
    Ok(Value::Object(new))
}

/// The standard clone handler (`zend_objects_clone_obj`).
pub fn std_clone(obj: &ObjRef) -> ObjRef {
    let props: Vec<Value> = obj
        .obj()
        .props
        .borrow()
        .iter()
        .map(|v| match v {
            Value::Ref(r) if r.refcount() == 1 => r.get(),
            v => v.clone(),
        })
        .collect();
    let new = ObjRef::with_props(obj.class().clone(), props);
    if let Some(d) = obj.obj().dyn_props.borrow().as_ref() {
        *new.obj().dyn_props.borrow_mut() = Some(d.clone());
    }
    // Slot flags (unset vs. uninitialised) are copied with the slots.
    if has_extra(obj) {
        let unset = with_extra(obj, |x| x.unset_slots.clone());
        if !unset.is_empty() {
            with_extra(&new, |x| x.unset_slots = unset);
        }
    }
    new
}

/// Runs `__destruct` for `obj` (`zend_objects_destroy_object`).
pub fn run_destructor(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    if let Some(dtor) = obj.class().handlers.dtor_obj {
        dtor(vm, obj)?;
    }
    let Some(m) = obj.class().magic.destructor.clone() else {
        return Ok(());
    };
    // A pending exception is saved while the destructor runs.
    let r = super::call::call_method_fn(vm, obj, &m, Vec::new(), None);
    match r {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `(array)$obj` (mangled keys for non-public properties).
pub fn object_to_array(vm: &mut Vm, o: &ObjRef) -> PResult<PhpArray> {
    if o.class().lc_name.as_bytes() == b"closure" {
        return Ok(PhpArray::from_values(vec![Value::Object(o.clone())]));
    }
    if let Some(f) = o.class().handlers.get_properties_for {
        if let Some(t) = f(vm, o, PropPurpose::ArrayCast)? {
            return Ok(crate::value::convert::proptable_to_symtable(&t));
        }
    }
    Ok(crate::value::convert::proptable_to_symtable(&property_table(o)))
}

/// The object's property table with mangled keys (declared slots in
/// declaration order, then dynamic properties).
pub fn property_table(o: &ObjRef) -> PhpArray {
    crate::value::convert::object_property_table(o)
}

// ---------------------------------------------------------------------------
// Property listings for extensions
// ---------------------------------------------------------------------------

impl Vm {
    /// Properties visible from `scope` (`get_object_vars`): unmangled names,
    /// uninitialised typed properties skipped, references kept.
    pub fn visible_properties(&mut self, obj: &ObjRef, scope: Option<&ClassRef>) -> Vec<(PhpStr, Value)> {
        let mut out = Vec::new();
        let table = property_table(obj);
        for (k, v) in table.iter() {
            let ArrayKey::Str(key) = k else { continue };
            let (cls, name) = unmangle(&key);
            let visible = match cls {
                None => true,
                Some(c) if c.as_slice() == b"*" => protected_ok(&obj.class().name, scope, self) || scope.is_some_and(|s| {
                    obj.class().properties.get(&name).is_some_and(|p| protected_ok(&p.declaring_class, Some(s), self))
                }),
                Some(c) => scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&c)),
            };
            if visible {
                out.push((PhpStr::from_vec(name), v.clone()));
            }
        }
        out
    }

    /// The property table for `purpose` (`zend_get_properties_for`): the
    /// class handler's table if it has one, else the mangled standard table.
    pub fn properties_for(&mut self, obj: &ObjRef, purpose: PropPurpose) -> PResult<PhpArray> {
        if let Some(f) = obj.class().handlers.get_properties_for {
            if let Some(t) = f(self, obj, purpose)? {
                return Ok(t);
            }
        }
        if purpose == PropPurpose::Debug {
            if let Some(f) = obj.class().handlers.get_debug_info {
                if let Some(t) = f(self, obj, purpose)? {
                    return Ok(t);
                }
            }
            if obj.class().magic.debug_info.is_some() {
                let v = self.call_method(obj, b"__debuginfo", Vec::new())?;
                return match v.into_unref() {
                    Value::Array(a) => Ok((*a).clone()),
                    Value::Null => Ok(PhpArray::new()),
                    _ => {
                        let msg = "__debuginfo() must return an array";
                        Err(self.fatal_error(msg))
                    }
                };
            }
        }
        Ok(property_table(obj))
    }

    /// Instantiates a class without calling its constructor
    /// (`object_init_ex`; property defaults are evaluated).
    pub fn instantiate_class(&mut self, class: &ClassRef) -> PResult<ObjRef> {
        instantiate(self, class)
    }

    /// Reads `$obj->name` with php-src semantics from `scope` (magic
    /// `__get`, visibility, uninitialised typed property errors).
    pub fn read_property(&mut self, obj: &ObjRef, name: &[u8], scope: Option<&ClassRef>) -> PResult<Value> {
        read_prop(self, scope, obj, &PhpStr::from_bytes(name), false)
    }

    /// Writes `$obj->name = $value` with php-src semantics from `scope`.
    pub fn write_property(&mut self, obj: &ObjRef, name: &[u8], value: Value, scope: Option<&ClassRef>) -> PResult<()> {
        write_prop(self, scope, obj, &PhpStr::from_bytes(name), value, false).map(|_| ())
    }
}

/// Splits a mangled property name into (class or `*`, name).
pub fn unmangle(key: &[u8]) -> (Option<Vec<u8>>, Vec<u8>) {
    if key.first() == Some(&0) {
        if let Some(end) = key[1..].iter().position(|&c| c == 0) {
            let cls = key[1..1 + end].to_vec();
            let name = key[2 + end..].to_vec();
            return (Some(cls), name);
        }
    }
    (None, key.to_vec())
}

/// Mangles a property name for its declaration.
pub fn mangled_name(info: &PropertyInfo) -> PhpStr {
    mangle_property_name(&info.name, info.flags, &info.declaring_class)
}

#[allow(dead_code)]
fn _keep(_: RefCell<()>, _: Rc<()>, _: i64) {
    let _ = level::E_ALL;
    let _ = obj_flags::USER_1;
}
