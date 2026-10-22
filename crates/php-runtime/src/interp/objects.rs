//! Objects: property access (`zend_std_{read,write,has,unset}_property`),
//! visibility, magic `__get/__set/__isset/__unset` with recursion guards,
//! typed and readonly properties, `ArrayAccess`, static properties,
//! `clone`, instantiation and destructors.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::class::{ClassRef, PropPurpose, PropertyInfo, class_flags, member_flags};
use crate::errors::level;
use crate::object::{ObjRef, obj_flags};
use crate::types::CoerceMode;
use crate::value::array::ArrayKey;
use crate::value::convert::mangle_property_name;
use crate::value::{PhpArray, PhpRef, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::Ctx;

// ---------------------------------------------------------------------------
// Per-object interpreter state
// ---------------------------------------------------------------------------

const IN_GET: u8 = 1;
const IN_SET: u8 = 2;
const IN_ISSET: u8 = 4;
const IN_UNSET: u8 = 8;

/// Interpreter data attached to objects (`PhpObject::extra`).
#[derive(Default)]
pub struct ObjExtra {
    /// Magic-method recursion guards per property name.
    guards: HashMap<PhpStr, u8>,
    /// Declared slots that were `unset()` (as opposed to never initialised).
    unset_slots: Vec<usize>,
}

fn with_extra<R>(obj: &ObjRef, f: impl FnOnce(&mut ObjExtra) -> R) -> R {
    let mut e = obj.obj().extra.borrow_mut();
    if e.is_none() {
        *e = Some(Box::new(ObjExtra::default()));
    }
    let x = e.as_mut().unwrap().downcast_mut::<ObjExtra>().expect("ObjExtra");
    f(x)
}

fn guard_get(obj: &ObjRef, name: &PhpStr, bit: u8) -> bool {
    if obj.obj().extra.borrow().is_none() {
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
    if obj.obj().extra.borrow().is_none() {
        return false;
    }
    with_extra(obj, |x| x.unset_slots.contains(&slot))
}

fn mark_slot_unset(obj: &ObjRef, slot: usize, on: bool) {
    if !on && obj.obj().extra.borrow().is_none() {
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

// ---------------------------------------------------------------------------
// Lookup (zend_get_property_offset)
// ---------------------------------------------------------------------------

pub(crate) enum Lookup {
    Declared(PropertyInfo),
    Dynamic,
    /// Declared but not accessible from the scope.
    Denied(PropertyInfo),
}

/// `is_protected_compatible_scope`.
pub(crate) fn protected_ok(decl_class: &[u8], scope: Option<&ClassRef>, vm: &Vm) -> bool {
    let Some(scope) = scope else { return false };
    if scope.lc_name.eq_ignore_ascii_case(decl_class) {
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

/// Root declaring class of a protected property (the topmost declaration).
fn prop_root_class(class: &ClassRef, name: &[u8]) -> PhpStr {
    let mut root = None;
    let mut c = Some(class.clone());
    while let Some(ce) = c {
        if let Some(p) = ce.properties.get(name) {
            if p.flags & member_flags::PRIVATE == 0 {
                root = Some(p.declaring_class.clone());
            }
        }
        c = ce.parent.clone();
    }
    root.unwrap_or_else(|| class.name.clone())
}

pub(crate) fn lookup_prop(vm: &Vm, class: &ClassRef, name: &[u8], scope: Option<&ClassRef>) -> Lookup {
    let Some(info) = class.properties.get(name) else {
        // A private property of the scope class, shadowed away in the object's class?
        if let Some(s) = scope {
            if !std::ptr::eq(&**s, &**class) && class.instance_of(s) {
                if let Some(p) = s.properties.get(name) {
                    if p.flags & member_flags::PRIVATE != 0
                        && p.declaring_class.eq_ignore_ascii_case(&s.name)
                        && !p.is_static()
                    {
                        return Lookup::Declared(p.clone());
                    }
                }
            }
        }
        return Lookup::Dynamic;
    };
    if info.is_static() {
        return Lookup::Dynamic;
    }
    let flags = info.flags;
    if flags & (member_flags::PRIVATE | member_flags::PROTECTED | member_flags::CHANGED) != 0 {
        let same = scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class));
        if !same {
            if flags & member_flags::CHANGED != 0 {
                if let Some(s) = scope {
                    if class.instance_of(s) {
                        if let Some(p) = s.properties.get(name) {
                            if p.flags & member_flags::PRIVATE != 0 && p.declaring_class.eq_ignore_ascii_case(&s.name) {
                                return Lookup::Declared(p.clone());
                            }
                        }
                    }
                }
                if flags & member_flags::PUBLIC != 0 {
                    return Lookup::Declared(info.clone());
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

fn denied_error(vm: &mut Vm, info: &PropertyInfo, class: &ClassRef) -> Flow {
    let _ = class;
    vm.throw_error(
        "Error",
        format!(
            "Cannot access {} property {}::${}",
            visibility_word(info.flags),
            crate::interp::decl::display_class_name(&class.name),
            info.name
        ),
    )
}

fn uninit_error(vm: &mut Vm, info: &PropertyInfo) -> Flow {
    vm.throw_error(
        "Error",
        format!(
            "Typed property {}::${} must not be accessed before initialization",
            crate::interp::decl::display_class_name(&info.declaring_class),
            info.name
        ),
    )
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

fn call_magic(vm: &mut Vm, obj: &ObjRef, method: &[u8], args: Vec<Value>) -> PResult<Value> {
    let m = obj.class().find_method(method).cloned().expect("magic method present");
    super::call::call_method_fn(vm, obj, &m, args, None)
}

/// `$obj->name` (`zend_std_read_property`).
pub fn read_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, is: bool) -> PResult<Value> {
    if let Some(h) = obj.class().handlers.read_property {
        if let Some(v) = h(vm, obj, name)? {
            return Ok(v.into_unref());
        }
    }
    let class = obj.class().clone();
    match lookup_prop(vm, &class, name, scope) {
        Lookup::Declared(info) => {
            if let Some(hooks) = super::decl::prop_hooks(&info) {
                if let Some(get) = &hooks.get {
                    if !super::decl::in_hook_of(vm, obj, &info.name) {
                        return Ok(super::call::call_method_fn(vm, obj, get, Vec::new(), None)?.into_unref());
                    }
                }
            }
            let v = obj.obj().props.borrow().get(info.slot).cloned().unwrap_or_default();
            if !v.is_undef() {
                return Ok(v.into_unref());
            }
            // Unset or uninitialised.
            let was_unset = slot_was_unset(obj, info.slot);
            if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) && (info.ty.is_none() || was_unset) {
                return magic_get(vm, obj, name, is);
            }
            if is {
                return Ok(Value::Null);
            }
            if info.ty.is_some() {
                return Err(uninit_error(vm, &info));
            }
            undefined_property(vm, &class, name)?;
            Ok(Value::Null)
        }
        Lookup::Dynamic => {
            let found = obj
                .obj()
                .dyn_props
                .borrow()
                .as_ref()
                .and_then(|d| d.get_str(name).cloned());
            if let Some(v) = found {
                return Ok(v.into_unref());
            }
            if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                return magic_get(vm, obj, name, is);
            }
            if name.first() == Some(&0) && !name.is_empty() {
                return Err(vm.throw_error("Error", "Cannot access property starting with \"\\0\""));
            }
            if !is {
                undefined_property(vm, &class, name)?;
            }
            Ok(Value::Null)
        }
        Lookup::Denied(info) => {
            if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                return magic_get(vm, obj, name, is);
            }
            if is {
                return Ok(Value::Null);
            }
            Err(denied_error(vm, &info, &class))
        }
    }
}

fn undefined_property(vm: &mut Vm, class: &ClassRef, name: &[u8]) -> PResult<()> {
    let mut m = format!(
        "Undefined property: {}::$",
        super::decl::display_class_name(&class.name)
    )
    .into_bytes();
    m.extend_from_slice(name);
    vm.engine_warning(PhpStr::from_vec(m))
}

fn magic_get(vm: &mut Vm, obj: &ObjRef, name: &PhpStr, is: bool) -> PResult<Value> {
    if is {
        // `??` / isset-like reads call __isset first.
        if obj.class().magic.isset.is_some() && !guard_get(obj, name, IN_ISSET) {
            guard_set(obj, name, IN_ISSET, true);
            let r = call_magic(vm, obj, b"__isset", vec![Value::String(name.clone())]);
            guard_set(obj, name, IN_ISSET, false);
            if !super::eval::to_bool(vm, &r?)? {
                return Ok(Value::Null);
            }
        } else if obj.class().magic.isset.is_none() {
            return Ok(Value::Null);
        }
    }
    guard_set(obj, name, IN_GET, true);
    let r = call_magic(vm, obj, b"__get", vec![Value::String(name.clone())]);
    guard_set(obj, name, IN_GET, false);
    Ok(r?.into_unref())
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

/// Set-visibility check for readonly / asymmetric properties.
fn check_set_access(vm: &mut Vm, info: &PropertyInfo, scope: Option<&ClassRef>, op: &str) -> PResult<()> {
    let mut set_flags = info.flags & (member_flags::PRIVATE_SET | member_flags::PROTECTED_SET | member_flags::PUBLIC_SET);
    if set_flags == 0 && info.is_readonly() {
        set_flags = if info.flags & member_flags::PRIVATE != 0 {
            member_flags::PRIVATE_SET
        } else {
            member_flags::PROTECTED_SET
        };
    }
    if set_flags == 0 || set_flags & member_flags::PUBLIC_SET != 0 {
        return Ok(());
    }
    let same = scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class));
    if same {
        return Ok(());
    }
    if set_flags & member_flags::PROTECTED_SET != 0 && protected_ok(&info.declaring_class, scope, vm) {
        return Ok(());
    }
    let vis = if set_flags & member_flags::PRIVATE_SET != 0 {
        "private(set)"
    } else if info.is_readonly() {
        "protected(set) readonly"
    } else {
        "protected(set)"
    };
    let from = match scope {
        Some(s) => format!("scope {}", super::decl::display_class_name(&s.name)),
        None => "global scope".to_string(),
    };
    Err(vm.throw_error(
        "Error",
        format!(
            "Cannot {op} {vis} property {}::${} from {from}",
            super::decl::display_class_name(&info.declaring_class),
            info.name
        ),
    ))
}

/// Coerces a value for a typed property (`zend_verify_property_type`).
pub(crate) fn coerce_prop(vm: &mut Vm, info: &PropertyInfo, v: Value, strict: bool) -> PResult<Value> {
    let Some(ty) = &info.ty else { return Ok(v) };
    if ty.is_mixed() {
        return Ok(v);
    }
    if super::call::type_matches(vm, ty, &v, None)? {
        return Ok(v);
    }
    if !v.is_null() {
        let mode = CoerceMode { strict, silent: false };
        if let Some(c) = vm.coerce_to_type(ty, v.clone(), mode)? {
            return Ok(c);
        }
    }
    Err(vm.throw_type_error(format!(
        "Cannot assign {} to property {}::${} of type {}",
        v.value_name(),
        super::decl::display_class_name(&info.declaring_class),
        info.name,
        ty.to_type_string()
    )))
}

/// `$obj->name = $value` (`zend_std_write_property`). Returns the assigned value.
pub fn write_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, value: Value, strict: bool) -> PResult<Value> {
    if let Some(h) = obj.class().handlers.write_property {
        if h(vm, obj, name, value.clone())? {
            return Ok(value);
        }
    }
    let class = obj.class().clone();
    match lookup_prop(vm, &class, name, scope) {
        Lookup::Declared(info) => {
            if let Some(hooks) = super::decl::prop_hooks(&info) {
                if !super::decl::in_hook_of(vm, obj, &info.name) {
                    if let Some(set) = &hooks.set {
                        check_set_access(vm, &info, scope, "modify")?;
                        super::call::call_method_fn(vm, obj, set, vec![value.clone()], None)?;
                        return Ok(value);
                    }
                    if hooks.virtual_ {
                        return Err(vm.throw_error(
                            "Error",
                            format!(
                                "Cannot write to get-only virtual property {}::${}",
                                super::decl::display_class_name(&class.name),
                                name
                            ),
                        ));
                    }
                }
            }
            let cur_undef = obj.obj().props.borrow().get(info.slot).is_none_or(Value::is_undef);
            let was_unset = cur_undef && slot_was_unset(obj, info.slot);
            if info.is_readonly() || info.flags & (member_flags::PRIVATE_SET | member_flags::PROTECTED_SET) != 0 {
                let use_magic = cur_undef && was_unset && class.magic.set.is_some() && !guard_get(obj, name, IN_SET);
                if !use_magic {
                    if info.is_readonly() && !cur_undef {
                        return Err(vm.throw_error(
                            "Error",
                            format!(
                                "Cannot modify readonly property {}::${}",
                                super::decl::display_class_name(&info.declaring_class),
                                info.name
                            ),
                        ));
                    }
                    check_set_access(vm, &info, scope, if info.is_readonly() { "modify" } else { "modify" })?;
                }
            }
            if !cur_undef || !was_unset || class.magic.set.is_none() || guard_get(obj, name, IN_SET) {
                let v = coerce_prop(vm, &info, value, strict)?;
                store_slot(obj, info.slot, v.clone());
                mark_slot_unset(obj, info.slot, false);
                return Ok(v);
            }
            magic_set(vm, obj, name, value)
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
            if class.magic.set.is_some() && !guard_get(obj, name, IN_SET) {
                return magic_set(vm, obj, name, value);
            }
            create_dynamic(vm, obj, name, value)
        }
        Lookup::Denied(info) => {
            if class.magic.set.is_some() && !guard_get(obj, name, IN_SET) {
                return magic_set(vm, obj, name, value);
            }
            Err(denied_error(vm, &info, &class))
        }
    }
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
        return Err(vm.throw_error("Error", "Cannot access property starting with \"\\0\""));
    }
    if class.has_flag(class_flags::NO_DYNAMIC_PROPERTIES) || class.has_flag(class_flags::READONLY) || class.is_enum() {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Cannot create dynamic property {}::${}",
                super::decl::display_class_name(&class.name),
                name
            ),
        ));
    }
    if !allows_dynamic(&class) {
        vm.engine_deprecated(format!(
            "Creation of dynamic property {}::${} is deprecated",
            super::decl::display_class_name(&class.name),
            name
        ))?;
    }
    let mut d = obj.obj().dyn_props.borrow_mut();
    let t = d.get_or_insert_with(PhpArray::new);
    t.insert(ArrayKey::Str(name.clone()), value.clone());
    Ok(value)
}

/// `$obj->name =& $ref`.
pub fn assign_prop_ref(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, r: PhpRef) -> PResult<()> {
    let class = obj.class().clone();
    match lookup_prop(vm, &class, name, scope) {
        Lookup::Declared(info) => {
            if info.is_readonly() {
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Cannot modify readonly property {}::${}",
                        super::decl::display_class_name(&info.declaring_class),
                        info.name
                    ),
                ));
            }
            if info.ty.is_some() {
                let v = r.get();
                let c = coerce_prop(vm, &info, v.clone(), false)?;
                if !c.identical(&v) {
                    r.set(c);
                }
            }
            let old = std::mem::replace(&mut obj.obj().props.borrow_mut()[info.slot], Value::Ref(r));
            drop(old);
            mark_slot_unset(obj, info.slot, false);
            Ok(())
        }
        Lookup::Dynamic => {
            let exists = obj.obj().dyn_props.borrow().as_ref().is_some_and(|d| d.get_str(name).is_some());
            if !exists {
                if class.magic.get.is_some() || class.magic.set.is_some() {
                    if class.magic.set.is_some() && !guard_get(obj, name, IN_SET) {
                        return Err(vm.throw_error("Error", "Cannot assign by reference to overloaded object"));
                    }
                }
                create_dynamic(vm, obj, name, Value::Null)?;
            }
            let mut d = obj.obj().dyn_props.borrow_mut();
            let t = d.get_or_insert_with(PhpArray::new);
            let old = t.insert(ArrayKey::Str(name.clone()), Value::Ref(r));
            drop(d);
            drop(old);
            Ok(())
        }
        Lookup::Denied(info) => Err(denied_error(vm, &info, &class)),
    }
}

/// `&$obj->name`.
pub fn make_prop_ref(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr) -> PResult<PhpRef> {
    match prop_target(vm, scope, obj, name, FetchMode::Write, true)? {
        PropTarget::Ref(r) => Ok(r),
        PropTarget::Value(v) => Ok(PhpRef::new(v)),
        PropTarget::Missing => Ok(PhpRef::new(Value::Null)),
    }
}

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
    /// A temporary (from `__get` or a virtual property): writes have no effect.
    Value(Value),
    /// Nothing to unset.
    Missing,
}

/// Fetches a property for a nested write (`$obj->p[...] = ...`).
pub(crate) fn prop_for_write(vm: &mut Vm, cx: &Ctx, obj: &ObjRef, name: &PhpStr, mode: FetchMode, next_is_dim: bool) -> PResult<PropTarget> {
    let _ = next_is_dim;
    prop_target(vm, cx.scope(), obj, name, mode, false)
}

fn prop_target(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, mode: FetchMode, for_ref: bool) -> PResult<PropTarget> {
    let class = obj.class().clone();
    match lookup_prop(vm, &class, name, scope) {
        Lookup::Declared(info) => {
            if let Some(hooks) = super::decl::prop_hooks(&info) {
                if hooks.get.is_some() && !super::decl::in_hook_of(vm, obj, &info.name) {
                    let v = super::call::call_method_fn(vm, obj, hooks.get.as_ref().unwrap(), Vec::new(), None)?;
                    if let Value::Ref(r) = v {
                        return Ok(PropTarget::Ref(r));
                    }
                    if !matches!(v, Value::Object(_)) {
                        vm.engine_notice(format!(
                            "Indirect modification of {}::${} is not allowed",
                            super::decl::display_class_name(&class.name),
                            name
                        ))?;
                    }
                    return Ok(PropTarget::Value(v));
                }
            }
            let cur = obj.obj().props.borrow()[info.slot].clone();
            if cur.is_undef() {
                if mode == FetchMode::Unset {
                    return Ok(PropTarget::Missing);
                }
                let was_unset = slot_was_unset(obj, info.slot);
                if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) && (info.ty.is_none() || was_unset) {
                    let v = magic_get_raw(vm, obj, name)?;
                    return magic_result_target(vm, &class, name, v);
                }
                if let Some(ty) = &info.ty {
                    if info.is_readonly() {
                        return Err(uninit_error(vm, &info));
                    }
                    // Auto-initialise an array in a typed property if allowed.
                    if ty.mask & crate::types::mask::ARRAY == 0 && !ty.is_mixed() {
                        if mode == FetchMode::ReadWrite || for_ref {
                            return Err(uninit_error(vm, &info));
                        }
                        return Err(vm.throw_error(
                            "Error",
                            format!(
                                "Cannot auto-initialize an array inside property {}::${} of type {}",
                                super::decl::display_class_name(&info.declaring_class),
                                info.name,
                                ty.to_type_string()
                            ),
                        ));
                    }
                    if mode == FetchMode::ReadWrite {
                        return Err(uninit_error(vm, &info));
                    }
                } else if mode == FetchMode::ReadWrite {
                    undefined_property(vm, &class, name)?;
                }
                if for_ref && info.ty.is_some() {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Cannot access uninitialized non-nullable property {}::${} by reference",
                            super::decl::display_class_name(&info.declaring_class),
                            info.name
                        ),
                    ));
                }
            }
            if info.is_readonly() && !cur.is_undef() {
                if matches!(cur, Value::Object(_)) && !for_ref {
                    return Ok(PropTarget::Value(cur));
                }
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Cannot modify readonly property {}::${}",
                        super::decl::display_class_name(&info.declaring_class),
                        info.name
                    ),
                ));
            }
            if let Some(ty) = &info.ty {
                if !cur.is_undef() && !matches!(cur.deref().type_code(), crate::value::TypeCode::Array | crate::value::TypeCode::Object | crate::value::TypeCode::Null) && !for_ref {
                    let _ = ty;
                }
            }
            let mut props = obj.obj().props.borrow_mut();
            let slot = &mut props[info.slot];
            mark_slot_unset_later(obj, info.slot);
            Ok(PropTarget::Ref(match slot {
                Value::Ref(r) => r.clone(),
                other => {
                    let v = std::mem::take(other);
                    let r = PhpRef::new(if v.is_undef() { Value::Null } else { v });
                    *other = Value::Ref(r.clone());
                    r
                }
            }))
        }
        Lookup::Dynamic => {
            let exists = obj.obj().dyn_props.borrow().as_ref().is_some_and(|d| d.get_str(name).is_some());
            if !exists {
                if mode == FetchMode::Unset {
                    return Ok(PropTarget::Missing);
                }
                if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                    let v = magic_get_raw(vm, obj, name)?;
                    return magic_result_target(vm, &class, name, v);
                }
                if mode == FetchMode::ReadWrite {
                    undefined_property(vm, &class, name)?;
                }
                create_dynamic(vm, obj, name, Value::Null)?;
            }
            let mut d = obj.obj().dyn_props.borrow_mut();
            let t = d.get_or_insert_with(PhpArray::new);
            let r = crate::value::array::make_element_ref(t, ArrayKey::Str(name.clone()));
            Ok(PropTarget::Ref(r))
        }
        Lookup::Denied(info) => {
            if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                let v = magic_get_raw(vm, obj, name)?;
                return magic_result_target(vm, &class, name, v);
            }
            Err(denied_error(vm, &info, &class))
        }
    }
}

fn mark_slot_unset_later(_obj: &ObjRef, _slot: usize) {}

fn magic_get_raw(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<Value> {
    guard_set(obj, name, IN_GET, true);
    let r = call_magic(vm, obj, b"__get", vec![Value::String(name.clone())]);
    guard_set(obj, name, IN_GET, false);
    r
}

fn magic_result_target(vm: &mut Vm, class: &ClassRef, name: &PhpStr, v: Value) -> PResult<PropTarget> {
    match v {
        Value::Ref(r) => Ok(PropTarget::Ref(r)),
        Value::Object(o) => Ok(PropTarget::Value(Value::Object(o))),
        v => {
            vm.engine_notice(format!(
                "Indirect modification of overloaded property {}::${} has no effect",
                super::decl::display_class_name(&class.name),
                name
            ))?;
            Ok(PropTarget::Value(v))
        }
    }
}

/// `++$obj->p` / `$obj->p--`.
pub(crate) fn incdec_prop(vm: &mut Vm, cx: &Ctx, obj: &ObjRef, name: &PhpStr, inc: bool, post: bool) -> PResult<Value> {
    let scope = cx.scope().cloned();
    let old = read_prop(vm, scope.as_ref(), obj, name, false)?;
    let new = if inc { vm.increment(&old)? } else { vm.decrement(&old)? };
    // Typed int properties must not overflow into float.
    if let (Value::Int(_), Value::Float(_)) = (&old, &new) {
        if let Lookup::Declared(info) = lookup_prop(vm, obj.class(), name, scope.as_ref()) {
            if let Some(ty) = &info.ty {
                if ty.mask & crate::types::mask::FLOAT == 0 && !ty.is_mixed() {
                    return Err(vm.throw_type_error(format!(
                        "Cannot {} property {}::${} of type {} past its {} value",
                        if inc { "increment" } else { "decrement" },
                        super::decl::display_class_name(&info.declaring_class),
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
    let value = match lookup_prop(vm, &class, name, scope) {
        Lookup::Declared(info) => {
            if let Some(hooks) = super::decl::prop_hooks(&info) {
                if let Some(get) = &hooks.get {
                    if !super::decl::in_hook_of(vm, obj, &info.name) {
                        let v = super::call::call_method_fn(vm, obj, get, Vec::new(), None)?.into_unref();
                        return if empty { Ok(!v.is_null() && super::eval::to_bool(vm, &v)?) } else { Ok(!v.is_null()) };
                    }
                }
            }
            let v = obj.obj().props.borrow()[info.slot].clone();
            if v.is_undef() {
                let was_unset = slot_was_unset(obj, info.slot);
                if info.ty.is_some() && !was_unset {
                    return Ok(false);
                }
                None
            } else {
                Some(v.into_unref())
            }
        }
        Lookup::Dynamic => obj
            .obj()
            .dyn_props
            .borrow()
            .as_ref()
            .and_then(|d| d.get_str(name).map(Value::unref)),
        Lookup::Denied(_) => None,
    };
    match value {
        Some(v) => {
            if empty {
                Ok(!v.is_null() && super::eval::to_bool(vm, &v)?)
            } else {
                Ok(!v.is_null())
            }
        }
        None => {
            if class.magic.isset.is_some() && !guard_get(obj, name, IN_ISSET) {
                guard_set(obj, name, IN_ISSET, true);
                let r = call_magic(vm, obj, b"__isset", vec![Value::String(name.clone())]);
                guard_set(obj, name, IN_ISSET, false);
                let has = super::eval::to_bool(vm, &r?)?;
                if !has {
                    return Ok(false);
                }
                if empty {
                    if class.magic.get.is_some() && !guard_get(obj, name, IN_GET) {
                        let v = magic_get_raw(vm, obj, name)?.into_unref();
                        return super::eval::to_bool(vm, &v);
                    }
                    return Ok(false);
                }
                return Ok(true);
            }
            Ok(false)
        }
    }
}

/// `unset($obj->name)` (`zend_std_unset_property`).
pub fn unset_prop(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr) -> PResult<()> {
    if let Some(h) = obj.class().handlers.unset_property {
        if h(vm, obj, name)? {
            return Ok(());
        }
    }
    let class = obj.class().clone();
    match lookup_prop(vm, &class, name, scope) {
        Lookup::Declared(info) => {
            if super::decl::prop_hooks(&info).is_some() {
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Cannot unset hooked property {}::${}",
                        super::decl::display_class_name(&class.name),
                        name
                    ),
                ));
            }
            let cur_undef = obj.obj().props.borrow()[info.slot].is_undef();
            if info.is_readonly() {
                if !cur_undef {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Cannot unset readonly property {}::${}",
                            super::decl::display_class_name(&info.declaring_class),
                            info.name
                        ),
                    ));
                }
                check_set_access(vm, &info, scope, "unset")?;
            }
            if cur_undef {
                if class.magic.unset.is_some() && !guard_get(obj, name, IN_UNSET) && slot_was_unset(obj, info.slot) {
                    return magic_unset(vm, obj, name);
                }
                mark_slot_unset(obj, info.slot, true);
                return Ok(());
            }
            let old = std::mem::take(&mut obj.obj().props.borrow_mut()[info.slot]);
            mark_slot_unset(obj, info.slot, true);
            drop(old);
            Ok(())
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
            if class.magic.unset.is_some() && !guard_get(obj, name, IN_UNSET) {
                return magic_unset(vm, obj, name);
            }
            Ok(())
        }
        Lookup::Denied(info) => {
            if class.magic.unset.is_some() && !guard_get(obj, name, IN_UNSET) {
                return magic_unset(vm, obj, name);
            }
            Err(denied_error(vm, &info, &class))
        }
    }
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
    if let Some(with) = with {
        let Value::Array(a) = with.into_unref() else {
            return Err(vm.throw_type_error("clone(): Argument #2 ($withProperties) must be of type array"));
        };
        for (k, v) in a.iter() {
            let name = match k {
                ArrayKey::Str(s) => s,
                ArrayKey::Int(_) => {
                    return Err(vm.throw_type_error("Property name must be of type string, int given"));
                }
            };
            write_prop_clone_with(vm, scope.as_ref(), &new, &name, v.unref())?;
        }
    }
    if class.magic.clone.is_some() {
        let m = class.magic.clone.clone().unwrap();
        super::call::call_method_fn(vm, &new, &m, Vec::new(), None)?;
    }
    Ok(Value::Object(new))
}

fn write_prop_clone_with(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr, v: Value) -> PResult<()> {
    // Readonly properties may be reinitialised once during clone-with.
    if let Lookup::Declared(info) = lookup_prop(vm, obj.class(), name, scope) {
        if info.is_readonly() {
            check_set_access(vm, &info, scope, "modify")?;
            let v = coerce_prop(vm, &info, v, false)?;
            store_slot(obj, info.slot, v);
            return Ok(());
        }
    }
    write_prop(vm, scope, obj, name, v, false)?;
    Ok(())
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
