//! Engine helpers shared by the SPL data structures (`spl_dllist.c`,
//! `spl_heap.c`, `spl_fixedarray.c`) that have no shared home yet
//! (candidates for `spl::engine` or the runtime):
//!
//! * [`zval_get_long`], [`parse_long_arg`] (`zend_parse_parameters` `"l"`
//!   for parameters the stub leaves untyped);
//! * [`object_properties_load`] (`Zend/zend_API.c`);
//! * [`release`] / [`run_destructors`]: `zval_ptr_dtor()` semantics — a
//!   released object's destructor runs before the caller continues, like in
//!   php-src, by draining [`Vm::run_pending_destructors`] right away;
//!   exceptions thrown meanwhile are chained ([`exception_set_previous`]) the
//!   way `zend_objects_destroy_object` chains them;
//! * [`check_allocation`] (`safe_emalloc` overflow / `memory_limit` errors).

use php_runtime::class::{CastKind, PropertyInfo, class_flags, member_flags};
use php_runtime::value::convert::to_int_silent;
use php_runtime::{ArrayKey, ClassRef, Flow, ObjRef, PResult, PhpArray, PhpStr, Value, Vm};

/// `zval_get_long()`: no diagnostics except PHP 8.5's out-of-range float
/// warning; objects use their `cast_object` handler (or count as 1).
pub fn zval_get_long(vm: &mut Vm, v: &Value) -> PResult<i64> {
    match v {
        Value::Float(d) => vm.dval_to_lval(*d),
        Value::Object(o) => Ok(match vm.object_cast(o, CastKind::Int)? {
            Some(Value::Int(i)) => i,
            _ => 1,
        }),
        Value::Ref(r) => {
            let inner = r.get();
            zval_get_long(vm, &inner)
        }
        v => Ok(to_int_silent(v)),
    }
}

/// `zend_parse_parameters()` `"l"` (`"l!"` when `nullable`) in weak mode for
/// argument `arg` (0-based) named `name`, for methods whose stub parameter
/// is untyped (`offsetGet($index)`). `Ok(None)` is a `null` passed to a
/// nullable parameter.
pub fn parse_long_arg(
    vm: &mut Vm,
    v: &Value,
    arg: usize,
    name: &str,
    nullable: bool,
) -> PResult<Option<i64>> {
    let v = v.unref();
    match v {
        Value::Int(i) => return Ok(Some(i)),
        Value::Null | Value::Undef if nullable => return Ok(None),
        Value::Null | Value::Undef => {
            let fname = vm.current_function_display_name();
            vm.engine_deprecated(format!(
                "{fname}(): Passing null to parameter #{} (${name}) of type int is deprecated",
                arg + 1
            ))?;
            return Ok(Some(0));
        }
        _ => {}
    }
    if let Some(i) = vm.parse_arg_long_weak(&v, false)? {
        return Ok(Some(i));
    }
    let fname = vm.current_function_display_name();
    let ty = if nullable { "?int" } else { "int" };
    Err(vm.throw_type_error(format!(
        "{fname}(): Argument #{} (${name}) must be of type {ty}, {} given",
        arg + 1,
        v.value_name()
    )))
}

// ---------------------------------------------------------------------------
// Exceptions and destructors
// ---------------------------------------------------------------------------

fn previous_of(e: &ObjRef) -> Option<ObjRef> {
    match e.get_prop(b"previous") {
        Some(Value::Object(p)) => Some(p),
        _ => None,
    }
}

/// `zend_exception_set_previous()`: appends `add_previous` at the end of
/// `exception`'s `previous` chain (unless that would create a cycle).
pub fn exception_set_previous(exception: &ObjRef, add_previous: ObjRef) {
    if ObjRef::ptr_eq(exception, &add_previous) {
        return;
    }
    let mut ancestor = Some(add_previous.clone());
    while let Some(a) = ancestor {
        if ObjRef::ptr_eq(&a, exception) {
            return;
        }
        ancestor = previous_of(&a);
    }
    let mut ancestor = exception.clone();
    loop {
        match previous_of(&ancestor) {
            Some(p) => ancestor = p,
            None => {
                ancestor.set_prop(b"previous", Value::Object(add_previous));
                return;
            }
        }
    }
}

/// Records `e` as the pending error of an operation that keeps running
/// (`EG(exception)`): a later exception gets the earlier one as `previous`;
/// `exit` and fatal errors take precedence.
pub fn merge_error(pending: &mut Option<Flow>, e: Flow) {
    *pending = match (pending.take(), e) {
        (None, e) => Some(e),
        (Some(Flow::Exception(old)), Flow::Exception(new)) => {
            exception_set_previous(&new, old);
            Some(Flow::Exception(new))
        }
        (Some(Flow::Exception(_)), other) => Some(other),
        (Some(other), _) => Some(other),
    };
}

/// Whether `pending` stops PHP code from running (`exit`, fatal errors).
fn is_abort(pending: &Option<Flow>) -> bool {
    matches!(pending, Some(f) if !matches!(f, Flow::Exception(_)))
}

/// Runs the destructors of the objects released so far (php-src runs them
/// inside `zval_ptr_dtor()`), chaining their exceptions into `pending`.
pub fn run_destructors(vm: &mut Vm, pending: &mut Option<Flow>) {
    while !is_abort(pending) {
        match vm.run_pending_destructors() {
            Ok(()) => return,
            Err(e) => merge_error(pending, e),
        }
    }
}

/// `zval_ptr_dtor()` of each value in turn (destructors included).
pub fn release(vm: &mut Vm, values: impl IntoIterator<Item = Value>, pending: &mut Option<Flow>) {
    for v in values {
        drop(v);
        run_destructors(vm, pending);
    }
}

/// [`release`] of a single value, returning the error it caused.
pub fn release_value(vm: &mut Vm, v: Value) -> PResult<()> {
    let mut pending = None;
    release(vm, [v], &mut pending);
    pending.map_or(Ok(()), Err)
}

/// `pending` as the result of an operation.
pub fn finish(pending: Option<Flow>) -> PResult<()> {
    pending.map_or(Ok(()), Err)
}

// ---------------------------------------------------------------------------
// Allocation
// ---------------------------------------------------------------------------

/// `safe_emalloc(nmemb, size, 0)`: "Possible integer overflow in memory
/// allocation" when the size overflows, else the `memory_limit` check.
pub fn check_allocation(vm: &mut Vm, nmemb: u64, size: u64) -> PResult<()> {
    match nmemb.checked_mul(size) {
        Some(bytes) if bytes <= isize::MAX as u64 => {
            crate::random::check_allocation(vm, bytes as usize)
        }
        _ => Err(vm.fatal_error(format!(
            "Possible integer overflow in memory allocation ({nmemb} * {size} + 0)"
        ))),
    }
}

// ---------------------------------------------------------------------------
// object_properties_load()
// ---------------------------------------------------------------------------

/// `zend_unmangle_property_name_ex`: `(class, name)`; `None` (with php-src's
/// notice) for malformed mangled names.
/// `(class or "*", property name)` of a property-table key.
type Unmangled = (Option<Vec<u8>>, Vec<u8>);

fn unmangle(vm: &mut Vm, key: &[u8]) -> PResult<Option<Unmangled>> {
    if key.first() != Some(&0) {
        return Ok(Some((None, key.to_vec())));
    }
    if key.len() < 3 || key[1] == 0 {
        vm.engine_notice("Illegal member variable name")?;
        return Ok(None);
    }
    let rest = &key[1..];
    match rest[..rest.len() - 1].iter().position(|c| *c == 0) {
        Some(end) => Ok(Some((Some(rest[..end].to_vec()), rest[end + 1..].to_vec()))),
        None => {
            vm.engine_notice("Corrupt member variable name")?;
            Ok(None)
        }
    }
}

/// The unmangled name of a property-table key (`zend_get_unmangled_property_name`).
fn unmangled_name(key: &[u8]) -> &[u8] {
    if key.first() != Some(&0) {
        return key;
    }
    match key[1..].iter().position(|c| *c == 0) {
        Some(end) => &key[end + 2..],
        None => key,
    }
}

enum PropLookup {
    Declared(PropertyInfo),
    Dynamic,
    /// `ZEND_WRONG_PROPERTY_INFO`.
    Wrong,
}

/// `zend_get_property_info(ce, name, silent=1)` from `scope`.
fn property_info(vm: &mut Vm, ce: &ClassRef, name: &[u8], scope: Option<&ClassRef>) -> PropLookup {
    let Some(info) = ce.find_property(name).cloned() else {
        return if name.first() == Some(&0) {
            PropLookup::Wrong
        } else {
            PropLookup::Dynamic
        };
    };
    let flags = info.flags;
    if flags & (member_flags::PRIVATE | member_flags::PROTECTED) != 0 {
        let same = scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class));
        if !same {
            if flags & member_flags::PRIVATE != 0 {
                return if info.declaring_class.eq_ignore_ascii_case(&ce.name) {
                    PropLookup::Wrong
                } else {
                    PropLookup::Dynamic
                };
            }
            // is_protected_compatible_scope()
            let declaring = vm.find_class(info.declaring_class.as_bytes());
            let compatible = match (scope, declaring) {
                (Some(s), Some(d)) => s.instance_of(&d) || d.instance_of(s),
                _ => false,
            };
            if !compatible {
                return PropLookup::Wrong;
            }
        }
    }
    PropLookup::Declared(info)
}

/// `object_properties_load()`: stores `properties` (a property table as
/// produced by `__serialize()`, mangled keys allowed) into `obj`. `scope` is
/// the class of the calling internal method (the executed scope).
pub fn object_properties_load(
    vm: &mut Vm,
    obj: &ObjRef,
    properties: &PhpArray,
    scope: &str,
) -> PResult<()> {
    let ce = obj.class().clone();
    let executed_scope = vm.find_class(scope.as_bytes());
    for (key, prop) in properties.iter() {
        let key = match key {
            ArrayKey::Str(s) => s,
            ArrayKey::Int(h) => {
                if ce.has_flag(class_flags::READONLY) {
                    return Err(vm.throw_error(
                        "Error",
                        format!("Cannot create dynamic property {}::${h}", ce.name),
                    ));
                }
                if !ce.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
                    vm.engine_deprecated(format!(
                        "Creation of dynamic property {}::${h} is deprecated",
                        ce.name
                    ))?;
                }
                // Property tables are keyed by strings in this runtime.
                set_dynamic(obj, PhpStr::from(h.to_string()), prop.clone());
                continue;
            }
        };
        let lookup = if key.as_bytes().first() == Some(&0) {
            match unmangle(vm, key.as_bytes())? {
                Some((class, name)) => {
                    let fake_scope = match class {
                        Some(c) if c != b"*" => vm.lookup_class(&c)?,
                        _ => None,
                    };
                    let scope = fake_scope.or_else(|| executed_scope.clone());
                    property_info(vm, &ce, &name, scope.as_ref())
                }
                None => PropLookup::Wrong,
            }
        } else {
            property_info(vm, &ce, key.as_bytes(), executed_scope.as_ref())
        };
        match lookup {
            PropLookup::Declared(info) if !info.is_static() => {
                load_declared(vm, obj, &info, prop)?;
            }
            lookup => {
                let shown = match lookup {
                    PropLookup::Wrong => Vec::new(),
                    _ => unmangled_name(key.as_bytes()).to_vec(),
                };
                if ce.has_flag(class_flags::READONLY) {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Cannot create dynamic property {}::${}",
                            ce.name,
                            String::from_utf8_lossy(&shown)
                        ),
                    ));
                }
                if !ce.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
                    let mut msg =
                        format!("Creation of dynamic property {}::$", ce.name).into_bytes();
                    msg.extend_from_slice(&shown);
                    msg.extend_from_slice(b" is deprecated");
                    vm.engine_deprecated(PhpStr::from_vec(msg))?;
                }
                set_dynamic(obj, key.clone(), prop.clone());
            }
        }
    }
    Ok(())
}

fn set_dynamic(obj: &ObjRef, key: PhpStr, v: Value) {
    let mut d = obj.obj().dyn_props.borrow_mut();
    d.get_or_insert_with(PhpArray::new)
        .insert(ArrayKey::Str(key), v);
}

/// The declared-property branch of `object_properties_load()`.
fn load_declared(vm: &mut Vm, obj: &ObjRef, info: &PropertyInfo, prop: &Value) -> PResult<()> {
    let ce = obj.class().clone();
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
        } else if info.is_readonly() && obj.get_prop(&info.name).is_some() {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Cannot modify readonly property {}::${}",
                    info.declaring_class, info.name
                ),
            ));
        }
        // zend_verify_property_type(strict): only int -> float widening.
        let inner = val.unref();
        if !ty.matches_exact(&inner) {
            match (&inner, ty.mask & php_runtime::types::mask::FLOAT != 0) {
                (Value::Int(i), true) if !val.is_ref() => val = Value::Float(*i as f64),
                _ => {
                    return Err(vm.throw_type_error(format!(
                        "Cannot assign {} to property {}::${} of type {}",
                        inner.value_name(),
                        info.declaring_class,
                        info.name,
                        ty.to_type_string()
                    )));
                }
            }
        }
    }
    let old = {
        let mut props = obj.obj().props.borrow_mut();
        match props.get_mut(info.slot) {
            Some(slot) => std::mem::replace(slot, val),
            None => Value::Undef,
        }
    };
    release_value(vm, old)
}
