//! Shared SPL infrastructure.
//!
//! * Class building: [`add_methods`] (member flags parsed from the stub
//!   signature), [`implement`] (`zend_do_implement_interface` ordering),
//!   [`interface`], [`abstract_method`].
//! * Debug info: [`std_properties`] (`zend_std_get_properties`) and
//!   [`set_private_debug_info_property`] (`spl_functions.c`).
//! * Override detection ([`is_overridden`]): php-src caches `fptr_*` for
//!   methods a user subclass overrides and calls them instead of the
//!   internal implementation.
//! * Iteration ([`ObjIterator`], [`get_iterator`], [`iterator_apply`]): the
//!   `zend_user_iterator` of `zend_interfaces.c` and `spl_iterator_apply()`.
//!   Every step goes through the object's `Iterator` methods with
//!   [`Vm::call_method`], so user overrides (and generators, once the
//!   interpreter provides them) work; `current()` is cached per position
//!   exactly like `zend_user_it_get_current_data`.

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::value::convert::{mangle_property_name, object_property_table};
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value,
    Vm,
};

// ---------------------------------------------------------------------------
// Class building
// ---------------------------------------------------------------------------

/// Member flags from the modifiers of a stub signature (`public static
/// function fromArray(...)`, `abstract public function accept()`, ...).
/// Methods without a visibility keyword are public.
pub fn method_flags(sig: &str) -> u32 {
    let head = sig
        .find("function")
        .map_or("", |i| &sig[..i])
        .split_whitespace();
    let mut flags = 0;
    for word in head {
        flags |= match word {
            "public" => member_flags::PUBLIC,
            "protected" => member_flags::PROTECTED,
            "private" => member_flags::PRIVATE,
            "static" => member_flags::STATIC,
            "final" => member_flags::FINAL,
            "abstract" => member_flags::ABSTRACT,
            _ => 0,
        };
    }
    if flags & member_flags::PPP_MASK == 0 {
        flags |= member_flags::PUBLIC;
    }
    flags
}

/// Adds native methods declared by `ce` (the scope is `ce.name`); the
/// member flags come from each signature's modifiers.
pub fn add_methods(ce: &mut ClassEntry, methods: &[&'static NativeFunction]) {
    let scope = ce.name.to_string_lossy().into_owned();
    for nf in methods {
        ce.add_method(Function::native_method(nf, &scope, method_flags(nf.sig)));
    }
}

/// `zend_do_implement_interface`: appends `iface` (unless inherited
/// already), then the interfaces `iface` extends that are still missing,
/// in reverse order (`zend_do_inherit_interfaces`). This reproduces the
/// order `class_implements()` reports for internal classes.
pub fn implement(ce: &mut ClassEntry, iface: &ClassRef) {
    if ce.interfaces.iter().any(|i| i.lc_name == iface.lc_name) {
        return;
    }
    ce.interfaces.push(iface.clone());
    let known = ce.interfaces.len();
    for parent in iface.interfaces.iter().rev() {
        if !ce.interfaces[..known]
            .iter()
            .any(|i| i.lc_name == parent.lc_name)
        {
            ce.interfaces.push(parent.clone());
        }
    }
}

/// Looks up an interface or class registered earlier (core or SPL).
pub fn class(r: &Registry, name: &str) -> ClassRef {
    r.find_class(name)
        .unwrap_or_else(|| panic!("{name} must be registered before it is used"))
}

/// Registers an interface extending `parents`, with abstract `methods`
/// (signatures copied from the stub; handlers normally [`abstract_method`]).
pub fn interface(
    r: &mut Registry,
    name: &str,
    parents: &[&str],
    methods: &[&'static NativeFunction],
) -> ClassRef {
    let mut ce = ClassEntry::new(name, class_flags::INTERFACE);
    for p in parents {
        let p = class(r, p);
        implement(&mut ce, &p);
    }
    for nf in methods {
        ce.add_method(Function::native_method(
            nf,
            name,
            method_flags(nf.sig) | member_flags::ABSTRACT,
        ));
    }
    r.class(ce)
}

/// Handler of abstract (interface) methods: calling one directly is an error.
pub fn abstract_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.function().display_name();
    Err(vm.throw_error("Error", format!("Cannot call abstract method {name}()")))
}

/// `$this` of a native method.
pub fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .unwrap_or_else(|| panic!("{} called without $this", args.function().display_name()))
}

/// Whether `class`'s method `name` (lowercase) is declared by a class other
/// than the internal class `base` — the `fptr->common.scope != base_ce`
/// test php-src uses to decide whether to call a user override.
pub fn is_overridden(class: &ClassEntry, name: &[u8], base: &str) -> bool {
    class.find_method(name).is_some_and(|m| {
        m.scope
            .as_ref()
            .is_some_and(|s| !s.eq_ignore_ascii_case(base.as_bytes()))
    })
}

/// `zend_get_type_by_const`-style name of the class of `obj` (for messages).
pub fn class_name(obj: &ObjRef) -> String {
    obj.class().name.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Debug info
// ---------------------------------------------------------------------------

/// `zend_std_get_properties()`: the object's property table with mangled
/// keys (what `get_debug_info` handlers start from).
pub fn std_properties(obj: &ObjRef) -> PhpArray {
    object_property_table(obj)
}

/// `spl_set_private_debug_info_property()`: stores `value` under the
/// mangled name of a private property `property` of class `class`
/// (`"\0Class\0property"`), shown by `var_dump()` as
/// `["property":"Class":private]`.
pub fn set_private_debug_info_property(
    table: &mut PhpArray,
    class: &str,
    property: &str,
    value: Value,
) {
    let key = mangle_property_name(property.as_bytes(), member_flags::PRIVATE, class.as_bytes());
    table.insert(ArrayKey::Str(key), value);
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

/// `ce->get_iterator(ce, obj, 0)` for a `Traversable`: resolves
/// `IteratorAggregate::getIterator()` chains (`zend_user_it_get_new_iterator`)
/// down to an object implementing `Iterator`.
pub fn get_iterator(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    let mut cur = obj.clone();
    loop {
        let ce = cur.class().clone();
        if ce.instance_of_name(b"Iterator") || !ce.instance_of_name(b"IteratorAggregate") {
            return Ok(cur);
        }
        let it = vm
            .call_method(&cur, b"getIterator", Vec::new())?
            .into_unref();
        match it {
            Value::Object(o)
                if o.class().instance_of_name(b"Traversable") && !ObjRef::ptr_eq(&o, &cur) =>
            {
                cur = o;
            }
            _ => {
                return Err(vm.throw_error(
                    "Exception",
                    format!(
                        "Objects returned by {}::getIterator() must be traversable or implement interface Iterator",
                        ce.name
                    ),
                ));
            }
        }
    }
}

/// A `zend_object_iterator` over an `Iterator` object (`zend_user_iterator`).
pub struct ObjIterator {
    obj: ObjRef,
    /// Cached `current()` of the current position (`iter->value`).
    value: Option<Value>,
    /// `iter->index`: number of `move_forward` steps since the last rewind,
    /// maintained by the users of the iterator (like php-src).
    pub index: i64,
}

impl ObjIterator {
    /// An iterator over a `Traversable` ([`get_iterator`] is applied).
    pub fn new(vm: &mut Vm, traversable: &ObjRef) -> PResult<ObjIterator> {
        let obj = get_iterator(vm, traversable)?;
        Ok(ObjIterator::over(obj))
    }

    /// An iterator over an object already known to implement `Iterator`.
    pub fn over(obj: ObjRef) -> ObjIterator {
        ObjIterator {
            obj,
            value: None,
            index: 0,
        }
    }

    /// The `Iterator` object being walked.
    pub fn object(&self) -> &ObjRef {
        &self.obj
    }

    /// `zend_user_it_invalidate_current`.
    pub fn invalidate_current(&mut self) {
        self.value = None;
    }

    /// `zend_user_it_rewind`.
    pub fn rewind(&mut self, vm: &mut Vm) -> PResult<()> {
        self.value = None;
        vm.call_method(&self.obj, b"rewind", Vec::new())?;
        Ok(())
    }

    /// `zend_user_it_valid` (`i_zend_is_true` of `valid()`).
    pub fn valid(&mut self, vm: &mut Vm) -> PResult<bool> {
        let more = vm.call_method(&self.obj, b"valid", Vec::new())?;
        vm.to_bool(&more.into_unref())
    }

    /// `zend_user_it_get_current_data`: `current()`, called once per position.
    pub fn current(&mut self, vm: &mut Vm) -> PResult<Value> {
        if let Some(v) = &self.value {
            return Ok(v.clone());
        }
        let v = vm.call_method(&self.obj, b"current", Vec::new())?;
        self.value = Some(v.clone());
        Ok(v)
    }

    /// `zend_user_it_get_current_key` (references are unwrapped).
    pub fn key(&mut self, vm: &mut Vm) -> PResult<Value> {
        Ok(vm.call_method(&self.obj, b"key", Vec::new())?.into_unref())
    }

    /// `zend_user_it_move_forward`.
    pub fn next(&mut self, vm: &mut Vm) -> PResult<()> {
        self.value = None;
        vm.call_method(&self.obj, b"next", Vec::new())?;
        Ok(())
    }
}

/// `spl_iterator_apply()`: rewinds `traversable` and calls `apply` for every
/// valid position until it returns `Ok(false)` (`ZEND_HASH_APPLY_STOP`).
pub fn iterator_apply(
    vm: &mut Vm,
    traversable: &ObjRef,
    mut apply: impl FnMut(&mut Vm, &mut ObjIterator) -> PResult<bool>,
) -> PResult<()> {
    let mut iter = ObjIterator::new(vm, traversable)?;
    iter.index = 0;
    iter.rewind(vm)?;
    while iter.valid(vm)? {
        if !apply(vm, &mut iter)? {
            break;
        }
        iter.index += 1;
        iter.next(vm)?;
    }
    Ok(())
}

/// Lowercase copy of a name (class-table / method-table key).
pub fn lc(name: &[u8]) -> PhpStr {
    PhpStr::from_vec(name.to_ascii_lowercase())
}
