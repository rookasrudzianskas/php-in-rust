//! `serialize()` / `unserialize()`: the serializer of ext/standard/var.c, the
//! unserializer of ext/standard/var_unserializer.re and the
//! `__PHP_Incomplete_Class` of ext/standard/incomplete_class.c.
//!
//! * [`serializer`] — `php_var_serialize` (back-reference numbering, `__serialize`,
//!   `Serializable`, `__sleep`, enums, incomplete classes).
//! * [`unserializer`] — the re2c grammar of `php_var_unserialize`, delayed
//!   `__wakeup()`/`__unserialize()` calls, `allowed_classes`, `max_depth`,
//!   `unserialize_callback_func`, typed-property checks.
//! * [`incomplete`] — `__PHP_Incomplete_Class` and its object handlers.
//!
//! Nested `serialize()`/`unserialize()` calls made from `Serializable`
//! methods share the outer call's back-reference table, exactly like
//! `BG(serialize)` / `BG(unserialize)`; calls made while `BG(serialize_lock)`
//! is held (magic methods, autoloading, callbacks) start a fresh one.
//!
//! # Interpreter contract
//! User classes reach this module only through the runtime's object model:
//! * methods are invoked with [`Vm::call_method`] (`__serialize`, `__sleep`,
//!   `__wakeup`, `__unserialize`, `Serializable::serialize/unserialize`); the
//!   host must call them regardless of visibility, like `zend_call_method`;
//! * objects are created without running a constructor through
//!   [`Vm::instantiate`] (which must evaluate default property values);
//! * classes are found with [`Vm::find_class`] / [`Vm::lookup_class`]
//!   (autoloading) and enum cases with [`Vm::class_constant`];
//! * properties are read and written in the object's slot table and
//!   dynamic property table directly (visibility-free, by mangled name);
//! * the "implements the Serializable interface" deprecation is emitted at
//!   class linking time: call [`implement_serializable`] there.

// Let-chains are avoided (see php-runtime's MSRV note).
#![allow(clippy::collapsible_if)]

mod incomplete;
mod serializer;
mod unserializer;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, PropertyInfo, member_flags};
use php_runtime::ini::{IniDef, modifiable};
use php_runtime::value::convert::mangle_property_name;
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpStr, Registry, Value, Vm};

pub use incomplete::{
    INCOMPLETE_CLASS, MAGIC_MEMBER, MAGIC_MEMBER_ENUM_CASE, MAGIC_MEMBER_SERIALIZED,
    incomplete_class_error, incomplete_class_message, is_incomplete_class, lookup_class_name,
    store_class_name,
};
pub use serializer::{
    SerializeData, var_serialize, var_serialize_destroy, var_serialize_init,
    var_serialize_set_incomplete_classes_only,
};
pub use unserializer::{
    UnserializeData, unserialize_with_options, var_unserialize, var_unserialize_destroy,
    var_unserialize_init, var_unserialize_set_incomplete_classes_only,
};

/// `BG(serialize)`, `BG(unserialize)` and `BG(serialize_lock)`.
#[derive(Default)]
struct Globals {
    /// `BG(serialize_lock)`: held while user code runs on behalf of the
    /// (un)serializer, so that nested calls get a fresh context.
    lock: u32,
    ser_level: u32,
    ser_data: Option<Rc<RefCell<SerializeData>>>,
    unser_level: u32,
    unser_data: Option<Rc<RefCell<UnserializeData>>>,
}

fn globals(vm: &mut Vm) -> &mut Globals {
    vm.ext_data::<Globals>()
}

/// Runs `f` with `BG(serialize_lock)` held.
fn with_lock<T>(vm: &mut Vm, f: impl FnOnce(&mut Vm) -> T) -> T {
    globals(vm).lock += 1;
    let r = f(vm);
    globals(vm).lock -= 1;
    r
}

/// `serialize(mixed $value): string`.
fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let value = args.get(0).clone();
    let ctx = var_serialize_init(vm);
    let mut buf = Vec::new();
    let r = var_serialize(vm, &mut buf, &value, &ctx);
    var_serialize_destroy(vm, ctx);
    r?;
    Ok(Value::String(PhpStr::from_vec(buf)))
}

/// `unserialize(string $data, array $options = []): mixed`.
fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = args.str(0).clone();
    let options = if args.is_passed(1) {
        Some(args.array(1).clone())
    } else {
        None
    };
    unserialize_with_options(vm, &data, options.as_deref(), "unserialize")
}

pub static SERIALIZE: NativeFunction = NativeFunction {
    sig: "function serialize(mixed $value): string {}",
    handler: serialize,
};
pub static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "function unserialize(string $data, array $options = []): mixed {}",
    handler: unserialize,
};

/// `zend_implement_serializable` (Zend/zend_interfaces.c): the deprecation a
/// class implementing `Serializable` gets at link time unless it is
/// abstract or also has both `__serialize()` and `__unserialize()`.
pub fn implement_serializable(vm: &mut Vm, ce: &ClassEntry) -> PResult<()> {
    let has = |m: &[u8]| ce.find_method(m).is_some();
    if !ce.is_abstract() && !(has(b"__serialize") && has(b"__unserialize")) {
        vm.engine_deprecated(format!(
            "{} implements the Serializable interface, which is deprecated. Implement \
             __serialize() and __unserialize() instead (or in addition, if support for old PHP \
             versions is necessary)",
            ce.name
        ))?;
    }
    Ok(())
}

/// Whether instances use the `Serializable` interface (`ce->serialize` /
/// `ce->unserialize` are set by `zend_implement_serializable`).
fn is_serializable(ce: &ClassEntry) -> bool {
    ce.instance_of_name(b"Serializable")
}

/// A declared, backed instance property slot of an object's class.
#[derive(Clone)]
struct DeclaredProp {
    /// Property-table key (`zend_property_info.name`, mangled).
    key: PhpStr,
    slot: usize,
    info: PropertyInfo,
}

/// The declared property slots in slot order with their mangled names
/// (`ce->properties_info_table`), including private slots of parents that a
/// subclass shadows.
fn declared_props(obj: &ObjRef) -> Vec<DeclaredProp> {
    let mut out: Vec<DeclaredProp> = Vec::new();
    let mut ce = Some(obj.class().clone());
    while let Some(c) = ce {
        for info in c.properties.values() {
            if info.is_static()
                || info.flags & member_flags::VIRTUAL != 0
                || out.iter().any(|d| d.slot == info.slot)
            {
                continue;
            }
            out.push(DeclaredProp {
                key: mangle_property_name(&info.name, info.flags, &info.declaring_class),
                slot: info.slot,
                info: info.clone(),
            });
        }
        ce = c.parent.clone();
    }
    out.sort_by_key(|d| d.slot);
    out
}

/// `zend_mangle_property_name(prefix, name)`: `"\0prefix\0name"`.
fn mangle(prefix: &[u8], name: &[u8]) -> PhpStr {
    let mut v = Vec::with_capacity(prefix.len() + name.len() + 2);
    v.push(0);
    v.extend_from_slice(prefix);
    v.push(0);
    v.extend_from_slice(name);
    PhpStr::from_vec(v)
}

/// A byte string as printed by C's `%s` (cut at the first NUL).
fn c_str(s: &[u8]) -> &[u8] {
    php_runtime::value::numeric::c_str(s)
}

/// Lossy text of [`c_str`].
fn c_text(s: &[u8]) -> String {
    String::from_utf8_lossy(c_str(s)).into_owned()
}

/// `zend_is_valid_class_name`: `[0-9a-zA-Z_\\\x80-\xff]*`.
fn is_valid_class_name(name: &[u8]) -> bool {
    name.iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80)
}

pub fn register(r: &mut Registry) {
    r.functions(&[&SERIALIZE, &UNSERIALIZE]);
    r.ini(IniDef::new(
        "unserialize_max_depth",
        Some("4096"),
        modifiable::ALL,
    ));
    incomplete::register(r);
}
