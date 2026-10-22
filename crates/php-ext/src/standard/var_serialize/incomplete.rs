//! ext/standard/incomplete_class.c — `__PHP_Incomplete_Class`, the class of
//! objects whose class is unknown (or not allowed) when unserializing.
//!
//! The original class name is kept in the dynamic property
//! `__PHP_Incomplete_Class_Name`; serializing the object writes it back.
//! Reading a property warns and writing one throws. The interpreter must
//! also route method calls, `isset()` and `unset()` on such objects to
//! [`incomplete_class_error`] / [`incomplete_class_message`] (php-src's
//! `get_method`, `has_property` and `unset_property` handlers).

use php_runtime::class::{ClassEntry, ObjectHandlers, class_flags};
use php_runtime::{ArrayKey, Flow, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// `INCOMPLETE_CLASS`.
pub const INCOMPLETE_CLASS: &str = "__PHP_Incomplete_Class";
/// `MAGIC_MEMBER`: the property holding the original class name.
pub const MAGIC_MEMBER: &[u8] = b"__PHP_Incomplete_Class_Name";
/// `MAGIC_MEMBER_SERIALIZED`: raw `C:` payload (incomplete-classes-only mode).
pub const MAGIC_MEMBER_SERIALIZED: &[u8] = b"__PHP_Incomplete_Class_Serialized";
/// `MAGIC_MEMBER_ENUM_CASE`: `E:` case name (incomplete-classes-only mode).
pub const MAGIC_MEMBER_ENUM_CASE: &[u8] = b"__PHP_Incomplete_Enum_Case";

/// `ce == PHP_IC_ENTRY` (the class is final, so no subclass can match).
pub fn is_incomplete_class(ce: &ClassEntry) -> bool {
    ce.lc_name.as_bytes() == b"__php_incomplete_class"
}

/// A dynamic property's raw value (no dereferencing, like `zend_hash_find`).
fn dyn_prop(obj: &ObjRef, name: &[u8]) -> Option<Value> {
    obj.obj()
        .dyn_props
        .borrow()
        .as_ref()
        .and_then(|d| d.get_str(name).cloned())
}

/// `php_lookup_class_name`: the stored class name, if it is a string.
pub fn lookup_class_name(obj: &ObjRef) -> Option<PhpStr> {
    match dyn_prop(obj, MAGIC_MEMBER) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// A dynamic property as `zend_hash_str_find` sees it, if it is a string.
pub(super) fn string_member(obj: &ObjRef, name: &[u8]) -> Option<PhpStr> {
    match dyn_prop(obj, name) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// `zend_hash_str_update(Z_OBJPROP_P(obj), name, value)`.
pub(super) fn set_member(obj: &ObjRef, name: &[u8], value: Value) {
    let mut d = obj.obj().dyn_props.borrow_mut();
    d.get_or_insert_with(PhpArray::new)
        .insert(ArrayKey::Str(PhpStr::from_bytes(name)), value);
}

/// `php_store_class_name`.
pub fn store_class_name(obj: &ObjRef, name: &PhpStr) {
    set_member(obj, MAGIC_MEMBER, Value::String(name.clone()));
}

fn message(obj: &ObjRef, what: &str) -> String {
    let name = lookup_class_name(obj);
    let name = name
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |n| super::c_text(n));
    format!(
        "The script tried to {what} on an incomplete object. Please ensure that the class \
         definition \"{name}\" of the object you are trying to operate on was loaded _before_ \
         unserialize() gets called or provide an autoloader to load the class definition"
    )
}

/// `incomplete_class_message`: the warning for reading (or testing) a property.
pub fn incomplete_class_message(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let msg = message(obj, "access a property");
    vm.warning(msg)
}

/// `throw_incomplete_class_error`: `what` is `"modify a property"` or
/// `"call a method"`.
pub fn incomplete_class_error(vm: &mut Vm, obj: &ObjRef, what: &str) -> Flow {
    let msg = message(obj, what);
    vm.throw_error("Error", msg)
}

/// `incomplete_class_get_property`: warns and reads `null`.
fn read_property(vm: &mut Vm, obj: &ObjRef, _name: &PhpStr) -> PResult<Option<Value>> {
    incomplete_class_message(vm, obj)?;
    Ok(Some(Value::Null))
}

/// `incomplete_class_write_property`.
fn write_property(vm: &mut Vm, obj: &ObjRef, _name: &PhpStr, _value: Value) -> PResult<bool> {
    Err(incomplete_class_error(vm, obj, "modify a property"))
}

/// Registers `#[AllowDynamicProperties] final class __PHP_Incomplete_Class {}`
/// with its object handlers (`php_register_incomplete_class_handlers`).
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(
        INCOMPLETE_CLASS,
        class_flags::FINAL | class_flags::ALLOW_DYNAMIC_PROPERTIES,
    );
    ce.handlers = ObjectHandlers {
        read_property: Some(read_property),
        write_property: Some(write_property),
        ..ObjectHandlers::default()
    };
    r.class(ce);
}
