//! Property writes as performed by `zend_update_property_ex()` on behalf of
//! PDO's object fetch modes.
//!
//! The runtime has no scoped `write_property` operation yet (the interpreter
//! owns it), so this is a faithful subset of `zend_std_write_property()`:
//! the class's `write_property` handler, visibility relative to `scope`,
//! readonly and typed properties (weak coercion), `__set` for inaccessible
//! or undeclared properties, and the dynamic-property deprecation.

use php_runtime::class::{class_flags, member_flags};
use php_runtime::types::CoerceMode;
use php_runtime::{ClassRef, ObjRef, PResult, PhpStr, Value, Vm};

/// `zend_check_protected`: `a` and `b` are related by inheritance.
fn related(a: &ClassRef, b_name: &[u8], vm: &Vm) -> bool {
    if a.instance_of_name(b_name) {
        return true;
    }
    vm.find_class(b_name)
        .is_some_and(|b| b.instance_of_name(&a.name))
}

/// `zend_update_property_ex(scope, obj, name, value)`.
pub fn update_property(
    vm: &mut Vm,
    scope: Option<&ClassRef>,
    obj: &ObjRef,
    name: &PhpStr,
    value: Value,
) -> PResult<()> {
    let class = obj.class().clone();
    if let Some(write) = class.handlers.write_property {
        if write(vm, obj, name, value.clone())? {
            return Ok(());
        }
    }
    let info = class
        .find_property(name)
        .filter(|i| !i.is_static())
        .cloned();
    let mut dynamic = info.is_none();
    if let Some(info) = info {
        let vis = info.visibility();
        let accessible = if vis & member_flags::PRIVATE != 0 {
            scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class))
        } else if vis & member_flags::PROTECTED != 0 {
            scope.is_some_and(|s| related(s, &info.declaring_class, vm))
        } else {
            true
        };
        if accessible {
            if info.is_readonly() {
                if obj.get_prop(name).is_some() {
                    return Err(vm.throw_error(
                        "Error",
                        format!("Cannot modify readonly property {}::${}", class.name, name),
                    ));
                }
                let in_scope =
                    scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class));
                if !in_scope {
                    let from =
                        scope.map_or("global scope".to_string(), |s| format!("scope {}", s.name));
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Cannot initialize readonly property {}::${} from {from}",
                            class.name, name
                        ),
                    ));
                }
            }
            let value = match &info.ty {
                Some(ty) => {
                    let mode = CoerceMode {
                        strict: false,
                        silent: false,
                    };
                    match vm.coerce_to_type(ty, value.clone(), mode)? {
                        Some(v) => v,
                        None => {
                            return Err(vm.throw_type_error(format!(
                                "Cannot assign {} to property {}::${} of type {}",
                                value.value_name(),
                                info.declaring_class,
                                name,
                                ty.to_type_string()
                            )));
                        }
                    }
                }
                None => value,
            };
            obj.set_prop(name, value);
            return Ok(());
        }
        if class.magic.set.is_none() {
            // A private property of a parent class is invisible to other
            // scopes: the write creates a dynamic property instead.
            if vis & member_flags::PRIVATE != 0
                && !class.name.eq_ignore_ascii_case(&info.declaring_class)
            {
                dynamic = true;
            } else {
                let kind = if vis & member_flags::PRIVATE != 0 {
                    "private"
                } else {
                    "protected"
                };
                return Err(vm.throw_error(
                    "Error",
                    format!("Cannot access {kind} property {}::${}", class.name, name),
                ));
            }
        }
    }
    if class.magic.set.is_some() {
        vm.call_method(obj, b"__set", vec![Value::String(name.clone()), value])?;
        return Ok(());
    }
    if dynamic {
        if class.has_flag(class_flags::READONLY) {
            return Err(vm.throw_error(
                "Error",
                format!("Cannot create dynamic property {}::${}", class.name, name),
            ));
        }
        if !class.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
            vm.engine_deprecated(format!(
                "Creation of dynamic property {}::${} is deprecated",
                class.name, name
            ))?;
        }
    }
    obj.set_prop(name, value);
    Ok(())
}
