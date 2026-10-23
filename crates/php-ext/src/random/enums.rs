//! Internal pure enums (`zend_register_internal_enum`, `zend_enum_add_case_cstr`),
//! used for `Random\IntervalBoundary` and `RoundingMode`.
//!
//! The runtime's [`Registry::enum_class`] creates the case objects lazily on
//! first access, like php-src, so registering enums does not consume object
//! handles.

use php_runtime::class::EnumBacking;
use php_runtime::{ClassRef, ObjRef, Registry, Value};

/// Registers the pure enum `name` with the given cases.
pub fn register_pure_enum(
    r: &mut Registry<'_>,
    name: &'static str,
    cases: &[&'static str],
) -> ClassRef {
    let cases: Vec<(&str, Option<Value>)> = cases.iter().map(|c| (*c, None)).collect();
    r.enum_class(name, EnumBacking::Pure, &cases, &[])
}

/// The case name of an enum case object (`$case->name`).
pub fn case_name(obj: &ObjRef) -> Option<String> {
    match obj.get_prop(b"name") {
        Some(Value::String(s)) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }
}
