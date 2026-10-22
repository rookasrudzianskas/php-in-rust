//! `object_properties_load()` (`Zend/zend_API.c`), as used by the
//! `__unserialize()` methods of the random engines and `Random\Randomizer`.

use php_runtime::{ArrayKey, ObjRef, PResult, PhpArray, PhpStr, Value, Vm};

/// `zend_unmangle_property_name_ex`: `"\0Class\0name"` / `"\0*\0name"` → `(class, name)`.
fn unmangle(key: &PhpStr) -> Option<(Option<PhpStr>, PhpStr)> {
    let b = key.as_bytes();
    if b.first() != Some(&0) {
        return Some((None, key.clone()));
    }
    let rest = &b[1..];
    let end = rest.iter().position(|c| *c == 0)?;
    let class = &rest[..end];
    let name = &rest[end + 1..];
    if name.is_empty() {
        return None;
    }
    let class = (class != b"*").then(|| PhpStr::from_bytes(class));
    Some((class, PhpStr::from_bytes(name)))
}

/// Loads `props` into the declared properties of `obj`.
///
/// Mirrors the hardened php-src behaviour: typed properties are verified
/// (strict mode), initialised readonly properties cannot be overwritten, and
/// dynamic properties are rejected (every class using this function is
/// declared `@strict-properties`).
pub fn properties_load(vm: &mut Vm, obj: &ObjRef, props: &PhpArray) -> PResult<()> {
    let class = obj.class().clone();
    for (key, prop) in props.iter() {
        let ArrayKey::Str(key) = key else {
            let h = key.as_int().unwrap_or(0);
            return Err(vm.throw_error(
                "Error",
                format!("Cannot create dynamic property {}::${h}", class.name),
            ));
        };
        let unmangled = unmangle(&key);
        let info = unmangled.as_ref().and_then(|(scope, name)| {
            class
                .find_property(name)
                .filter(|i| !i.is_static())
                .filter(|i| {
                    scope
                        .as_ref()
                        .is_none_or(|c| i.declaring_class.eq_ignore_ascii_case(c))
                })
        });
        let Some(info) = info else {
            let shown = unmangled.map_or_else(String::new, |(_, n)| n.to_string());
            return Err(vm.throw_error(
                "Error",
                format!("Cannot create dynamic property {}::${shown}", class.name),
            ));
        };
        if info.flags & php_runtime::class::member_flags::VIRTUAL != 0 {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Cannot unserialize value for virtual property {}::${}",
                    class.name, info.name
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
                val = val.unref();
            } else if info.is_readonly() && obj.get_prop(&info.name).is_some() {
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Cannot modify readonly property {}::${}",
                        info.declaring_class, info.name
                    ),
                ));
            }
            // Strict verification: only int -> float widening is allowed.
            if !ty.matches_exact(&val) {
                match (&val, ty.mask & php_runtime::types::mask::FLOAT != 0) {
                    (Value::Int(i), true) => val = Value::Float(*i as f64),
                    _ => {
                        return Err(vm.throw_type_error(format!(
                            "Cannot assign {} to property {}::${} of type {}",
                            val.value_name(),
                            info.declaring_class,
                            info.name,
                            ty.to_type_string()
                        )));
                    }
                }
            }
        }
        obj.set_prop(&info.name, val);
    }
    Ok(())
}
