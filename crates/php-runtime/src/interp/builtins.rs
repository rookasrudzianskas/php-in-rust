//! Zend built-in functions (`Zend/zend_builtin_functions.c`), the
//! engine-coupled `ext/standard` functions (callbacks, `compact`/`extract`,
//! `constant`, shutdown/tick functions, `assert`), and registration of the
//! engine's internal classes.

use std::rc::Rc;

use crate::class::{ClassEntry, ClassRef, Function, PropPurpose, class_flags, fn_flags, member_flags};
use crate::compile::ir::*;
use crate::errors::level;
use crate::native::{Args, CallArgs, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::value::array::ArrayKey;
use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::call::{self, Bound, Caller};
use super::{Ctx, Env};

macro_rules! native {
    ($name:ident, $sig:expr, $h:expr) => {
        pub static $name: NativeFunction = NativeFunction { sig: $sig, handler: $h };
    };
}

/// The environment of the userland code that called the running native.
fn caller_env(vm: &mut Vm) -> Rc<Env> {
    call::calling_env(vm)
}

/// The frame of the userland function that called the running native.
fn caller_func_frame(vm: &Vm) -> Option<&crate::vm::Frame> {
    // frames[last] is the native itself.
    let n = vm.frames.len();
    if n < 2 {
        return None;
    }
    let f = &vm.frames[n - 2];
    if f.env.is_some() && f.function.is_some() && !f.is_internal {
        Some(f)
    } else {
        None
    }
}

fn forbid_dynamic(vm: &mut Vm, name: &str) -> PResult<()> {
    // Called from an internal function (callback): php-src forbids it.
    let n = vm.frames.len();
    if n >= 2 && vm.frames[n - 2].is_internal {
        return Err(vm.throw_error("Error", format!("Cannot call {name}() dynamically")));
    }
    Ok(())
}

// ----- func_* -------------------------------------------------------------------

fn func_num_args(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    forbid_dynamic(vm, "func_num_args")?;
    match caller_func_frame(vm) {
        Some(f) => Ok(Value::Int(f.env.as_ref().unwrap().num_args.get() as i64)),
        None => Err(vm.throw_error("Error", "func_num_args() must be called from a function context")),
    }
}

fn func_get_args(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    forbid_dynamic(vm, "func_get_args")?;
    match caller_func_frame(vm) {
        Some(f) => {
            let env = f.env.clone().unwrap();
            let func = f.function.clone();
            let args: Vec<Value> = env
                .trace_args(func.as_deref())
                .into_iter()
                .filter(|(n, _)| n.is_none())
                .map(|(_, v)| v)
                .collect();
            Ok(Value::array(PhpArray::from_values(args)))
        }
        None => Err(vm.throw_error("Error", "func_get_args() cannot be called from the global scope")),
    }
}

fn func_get_arg(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    forbid_dynamic(vm, "func_get_arg")?;
    let pos = a.int(0);
    let Some(f) = caller_func_frame(vm) else {
        return Err(vm.throw_error("Error", "func_get_arg() cannot be called from the global scope"));
    };
    let env = f.env.clone().unwrap();
    let func = f.function.clone();
    if pos < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    let args: Vec<Value> = env.trace_args(func.as_deref()).into_iter().filter(|(n, _)| n.is_none()).map(|(_, v)| v).collect();
    match args.get(pos as usize) {
        Some(v) => Ok(v.clone()),
        None => Err(vm.argument_value_error(1, "must be less than the number of the arguments passed to the currently executed function")),
    }
}

// ----- strings ------------------------------------------------------------------------

fn strlen(_vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    Ok(Value::Int(a.str(0).len() as i64))
}

fn cmp_result(o: std::cmp::Ordering) -> i64 {
    match o {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn byte_cmp(a: &[u8], b: &[u8]) -> i64 {
    // zend_binary_strcmp: memcmp then length difference (normalised to -1/0/1 in PHP 8.2+).
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return if a[i] < b[i] { -1 } else { 1 };
        }
    }
    cmp_result(a.len().cmp(&b.len()))
}

fn strcmp(_vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    Ok(Value::Int(byte_cmp(a.str(0), a.str(1))))
}

fn strncmp(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let n = a.int(2);
    if n < 0 {
        return Err(vm.argument_value_error(3, "must be greater than or equal to 0"));
    }
    let n = n as usize;
    let x = a.str(0);
    let y = a.str(1);
    Ok(Value::Int(byte_cmp(&x[..x.len().min(n)], &y[..y.len().min(n)])))
}

fn lower_cmp(a: &[u8], b: &[u8]) -> i64 {
    let n = a.len().min(b.len());
    for i in 0..n {
        let (c, d) = (a[i].to_ascii_lowercase(), b[i].to_ascii_lowercase());
        if c != d {
            return if c < d { -1 } else { 1 };
        }
    }
    cmp_result(a.len().cmp(&b.len()))
}

fn strcasecmp(_vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    Ok(Value::Int(lower_cmp(a.str(0), a.str(1))))
}

fn strncasecmp(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let n = a.int(2);
    if n < 0 {
        return Err(vm.argument_value_error(3, "must be greater than or equal to 0"));
    }
    let n = n as usize;
    let x = a.str(0);
    let y = a.str(1);
    Ok(Value::Int(lower_cmp(&x[..x.len().min(n)], &y[..y.len().min(n)])))
}

// ----- errors ----------------------------------------------------------------------------

fn error_reporting(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let old = vm.errors.reporting;
    if let Some(new) = a.int_opt(0) {
        vm.ini_set("error_reporting", PhpStr::from(new.to_string()));
        vm.errors.reporting = new;
    }
    Ok(Value::Int(old))
}

fn trigger_error(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let msg = a.str(0).clone();
    let ty = a.int(1);
    match ty {
        level::E_USER_ERROR => {
            vm.engine_deprecated(
                "Passing E_USER_ERROR to trigger_error() is deprecated since 8.4, throw an exception or call exit with a string message instead",
            )?;
        }
        level::E_USER_WARNING | level::E_USER_NOTICE | level::E_USER_DEPRECATED => {}
        _ => {
            return Err(vm.argument_value_error(
                2,
                "must be one of E_USER_ERROR, E_USER_WARNING, E_USER_NOTICE, or E_USER_DEPRECATED",
            ));
        }
    }
    vm.zend_error(ty, msg)?;
    Ok(Value::Bool(true))
}

fn set_error_handler(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let cb = a.get(0).clone();
    let mask = a.int(1);
    let cb = if cb.is_null() { None } else { Some(cb) };
    let prev = vm.set_error_handler(cb, mask);
    Ok(prev.unwrap_or(Value::Null))
}

fn restore_error_handler(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    vm.restore_error_handler();
    Ok(Value::Bool(true))
}

fn get_error_handler(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(vm.errors.user_handler.as_ref().map(|h| h.callback.clone()).unwrap_or(Value::Null))
}

fn set_exception_handler(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let cb = a.get(0).clone();
    let cb = if cb.is_null() { None } else { Some(cb) };
    Ok(vm.set_exception_handler(cb).unwrap_or(Value::Null))
}

fn restore_exception_handler(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    vm.restore_exception_handler();
    Ok(Value::Bool(true))
}

fn get_exception_handler(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(vm.errors.exception_handler.clone().unwrap_or(Value::Null))
}

// ----- constants ----------------------------------------------------------------------------

fn define(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let name = a.str(0).clone();
    let value = a.get(1).unref();
    if a.bool(2) {
        vm.warning("Argument #3 ($case_insensitive) is ignored since declaration of case-insensitive constants is no longer supported")?;
    }
    if let Some(pos) = name.windows(2).position(|w| w == b"::") {
        let _ = pos;
        return Err(vm.argument_value_error(1, "cannot be a class constant"));
    }
    let value = match value {
        Value::Object(o) if !o.class().is_enum() && false => Value::Object(o),
        v => v,
    };
    if vm.constants.contains(&name) || matches!(name.to_ascii_lowercase().as_bytes(), b"true" | b"false" | b"null") && vm.constants.get(&name).is_some() {
        vm.warning(format!("Constant {name} already defined"))?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(vm.define_constant(name, value)))
}

fn defined(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let name = a.str(0).clone();
    if let Some(pos) = name.windows(2).position(|w| w == b"::") {
        let (c, n) = (&name[..pos], &name[pos + 2..]);
        let Some(class) = vm.lookup_class(c)? else { return Ok(Value::Bool(false)) };
        return Ok(Value::Bool(class.find_constant(n).is_some()));
    }
    Ok(Value::Bool(vm.constants.contains(&name)))
}

fn constant(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let name = a.str(0).clone();
    if let Some(pos) = name.windows(2).position(|w| w == b"::") {
        let (c, n) = (PhpStr::from_bytes(&name[..pos]), PhpStr::from_bytes(&name[pos + 2..]));
        let class = super::eval::lookup_class_or_error(vm, &c, vm.current_line)?;
        let scope = call::calling_scope(vm);
        return super::decl::class_constant_value(vm, &class, &n, scope.as_ref());
    }
    match vm.fetch_constant(&name)? {
        Some(v) => Ok(v),
        None => Err(vm.throw_error("Error", format!("Undefined constant \"{name}\""))),
    }
}

// ----- classes & objects -----------------------------------------------------------------

fn get_class(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    if !a.is_passed(0) {
        let scope = call::calling_scope(vm);
        return match scope {
            Some(s) => Ok(Value::String(s.name.clone())),
            None => Err(vm.throw_error("Error", "get_class() without arguments must be called from within a class")),
        };
    }
    Ok(Value::String(a.object(0).class().name.clone()))
}

fn get_called_class(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    let env = caller_env(vm);
    match env.called.clone().or_else(|| env.scope.clone()) {
        Some(c) => Ok(Value::String(c.name.clone())),
        None => Err(vm.throw_error("Error", "get_called_class() must be called from within a class")),
    }
}

fn class_of_arg(vm: &mut Vm, v: &Value, autoload: bool) -> PResult<Option<ClassRef>> {
    match v {
        Value::Object(o) => Ok(Some(o.class().clone())),
        Value::String(s) => {
            if autoload {
                vm.lookup_class(s)
            } else {
                Ok(vm.find_class(s))
            }
        }
        _ => Ok(None),
    }
}

fn get_parent_class(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let class = if a.is_passed(0) {
        class_of_arg(vm, &a.get(0).clone(), true)?
    } else {
        call::calling_scope(vm)
    };
    Ok(match class.and_then(|c| c.parent.clone()) {
        Some(p) => Value::String(p.name.clone()),
        None => Value::Bool(false),
    })
}

fn is_a_impl(vm: &mut Vm, a: &mut Args, subclass_only: bool) -> PResult<Value> {
    let v = a.get(0).clone();
    let target = a.str(1).clone();
    let allow_string = a.bool(2);
    let class = match &v {
        Value::Object(o) => o.class().clone(),
        Value::String(s) if allow_string => match vm.lookup_class(s)? {
            Some(c) => c,
            None => return Ok(Value::Bool(false)),
        },
        _ => return Ok(Value::Bool(false)),
    };
    let Some(t) = vm.lookup_class(&target)? else {
        return Ok(Value::Bool(false));
    };
    if subclass_only && Rc::ptr_eq(&class, &t) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(class.instance_of(&t)))
}

fn is_subclass_of(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    is_a_impl(vm, a, true)
}

fn is_a(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    is_a_impl(vm, a, false)
}

fn get_class_vars(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let name = a.str(0).clone();
    let Some(class) = vm.lookup_class(&name)? else {
        return Ok(Value::Bool(false));
    };
    super::decl::update_class_constants(vm, &class)?;
    let scope = call::calling_scope(vm);
    let defaults = super::decl::resolved_defaults(vm, &class)?;
    let mut out = PhpArray::new();
    for statics in [false, true] {
        for info in class.properties.values() {
            if info.is_static() != statics {
                continue;
            }
            if !prop_visible(vm, info, scope.as_ref()) {
                continue;
            }
            let v = if statics {
                class.static_members.borrow().get(info.slot).map(|r| r.get()).unwrap_or_default()
            } else {
                match &defaults {
                    Some(d) => d.get(info.slot).cloned().unwrap_or_default(),
                    None => class.default_properties.get(info.slot).cloned().unwrap_or_default(),
                }
            };
            if v.is_undef() {
                if info.ty.is_some() {
                    continue;
                }
            }
            out.insert(ArrayKey::Str(info.name.clone()), v.or_null());
        }
    }
    Ok(Value::array(out))
}

fn prop_visible(vm: &Vm, info: &crate::class::PropertyInfo, scope: Option<&ClassRef>) -> bool {
    if info.flags & member_flags::PRIVATE != 0 {
        return scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&info.declaring_class));
    }
    if info.flags & member_flags::PROTECTED != 0 {
        return super::objects::protected_ok(&info.declaring_class, scope, vm);
    }
    true
}

fn get_object_vars(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.object(0).clone();
    let scope = call::calling_scope(vm);
    if let Some(f) = o.class().handlers.get_properties_for {
        if let Some(t) = f(vm, &o, PropPurpose::GetObjectVars)? {
            return Ok(Value::array(crate::value::convert::proptable_to_symtable(&t)));
        }
    }
    let props = vm.visible_properties(&o, scope.as_ref());
    let mut out = PhpArray::new();
    for (k, v) in props {
        out.insert(ArrayKey::from_php_str(k), match v {
            Value::Ref(r) if r.refcount() == 1 => r.get(),
            v => v,
        });
    }
    Ok(Value::array(out))
}

fn get_mangled_object_vars(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.object(0).clone();
    let t = vm.properties_for(&o, PropPurpose::ArrayCast)?;
    Ok(Value::array(t))
}

fn get_class_methods(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let Some(class) = class_of_arg(vm, &v, true)? else {
        return Err(vm.argument_type_error(1, &format!("must be an object or a valid class name, {} given", v.value_name())));
    };
    let scope = call::calling_scope(vm);
    let mut out = PhpArray::new();
    for m in class.methods.values() {
        let visible = if m.flags & member_flags::PRIVATE != 0 {
            scope.as_ref().is_some_and(|s| m.scope.as_ref().is_some_and(|c| c.eq_ignore_ascii_case(&s.name)))
        } else if m.flags & member_flags::PROTECTED != 0 {
            super::objects::protected_ok(m.scope.as_deref().unwrap_or(b""), scope.as_ref(), vm)
        } else {
            true
        };
        if visible {
            let _ = out.append(Value::String(m.name.clone()));
        }
    }
    Ok(Value::array(out))
}

fn method_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let name = a.str(1).clone();
    let class = match &v {
        Value::Object(o) => o.class().clone(),
        Value::String(s) => match vm.lookup_class(s)? {
            Some(c) => c,
            None => return Ok(Value::Bool(false)),
        },
        other => {
            return Err(vm.argument_type_error(1, &format!("must be of type object|string, {} given", other.value_name())));
        }
    };
    if class.find_method(&name).is_some() {
        return Ok(Value::Bool(true));
    }
    if let Value::Object(o) = &v {
        if super::closure::closure_data(o).is_some() && name.eq_ignore_ascii_case(b"__invoke") {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn property_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let name = a.str(1).clone();
    let class = match &v {
        Value::Object(o) => o.class().clone(),
        Value::String(s) => match vm.lookup_class(s)? {
            Some(c) => c,
            None => return Ok(Value::Bool(false)),
        },
        other => {
            return Err(vm.argument_type_error(1, &format!("must be of type object|string, {} given", other.value_name())));
        }
    };
    if class.find_property(&name).is_some() {
        return Ok(Value::Bool(true));
    }
    if let Value::Object(o) = &v {
        if o.obj().dyn_props.borrow().as_ref().is_some_and(|d| d.get_str(&name).is_some()) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn class_exists_impl(vm: &mut Vm, a: &mut Args, pred: fn(&ClassEntry) -> bool) -> PResult<Value> {
    let name = a.str(0).clone();
    let autoload = a.bool(1);
    let c = if autoload { vm.lookup_class(&name)? } else { vm.find_class(&name) };
    Ok(Value::Bool(c.is_some_and(|c| pred(&c))))
}

fn class_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    class_exists_impl(vm, a, |c| !c.is_interface() && !c.is_trait() && !c.is_enum())
}

fn interface_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    class_exists_impl(vm, a, ClassEntry::is_interface)
}

fn trait_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    class_exists_impl(vm, a, ClassEntry::is_trait)
}

fn enum_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    class_exists_impl(vm, a, ClassEntry::is_enum)
}

fn function_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let name = a.str(0).clone();
    let n = name.strip_prefix(b"\\").unwrap_or(&name);
    Ok(Value::Bool(vm.find_function(n).is_some()))
}

fn class_alias(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let orig = a.str(0).clone();
    let alias = a.str(1).clone();
    let autoload = a.bool(2);
    let c = if autoload { vm.lookup_class(&orig)? } else { vm.find_class(&orig) };
    let Some(c) = c else {
        vm.warning(format!("Class \"{orig}\" not found"))?;
        return Ok(Value::Bool(false));
    };
    if c.is_internal() {
        return Err(vm.argument_value_error(1, "must be a user-defined class name, internal class name given"));
    }
    let lc = alias.strip_prefix(b"\\").unwrap_or(&alias).to_ascii_lowercase();
    if vm.classes.contains_key(&lc) {
        vm.warning(format!(
            "Cannot declare class {alias}, because the name is already in use"
        ))?;
        return Ok(Value::Bool(false));
    }
    vm.classes.insert(PhpStr::from_vec(lc), c);
    Ok(Value::Bool(true))
}

fn get_included_files(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    let mut out = PhpArray::new();
    if !vm.interp.main_file.is_empty() && !vm.interp.is_included(&vm.interp.main_file.clone()) {
        let _ = out.append(Value::String(vm.interp.main_file.clone()));
    }
    for f in &vm.interp.included_files {
        let _ = out.append(Value::String(f.clone()));
    }
    Ok(Value::array(out))
}

fn declared(vm: &mut Vm, pred: fn(&ClassEntry) -> bool) -> PResult<Value> {
    let mut out = PhpArray::new();
    let mut seen: Vec<*const ClassEntry> = Vec::new();
    for (k, c) in vm.classes.iter() {
        if !pred(c) || c.has_flag(class_flags::ANON) {
            continue;
        }
        // Aliases are listed under their alias name.
        let ptr = Rc::as_ptr(c);
        if seen.contains(&ptr) && k.eq_ignore_ascii_case(&c.name) {
            continue;
        }
        seen.push(ptr);
        let name = if k.eq_ignore_ascii_case(&c.name) { c.name.clone() } else { k.clone() };
        let _ = out.append(Value::String(name));
    }
    Ok(Value::array(out))
}

fn get_declared_classes(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    declared(vm, |c| !c.is_interface() && !c.is_trait())
}

fn get_declared_traits(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    declared(vm, ClassEntry::is_trait)
}

fn get_declared_interfaces(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    declared(vm, ClassEntry::is_interface)
}

fn get_defined_functions(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    let mut internal = PhpArray::new();
    let mut user = PhpArray::new();
    for (k, f) in vm.functions.iter() {
        if f.is_native() {
            let _ = internal.append(Value::String(k.clone()));
        } else {
            let _ = user.append(Value::String(k.clone()));
        }
    }
    let mut out = PhpArray::new();
    out.insert_str("internal", Value::array(internal));
    out.insert_str("user", Value::array(user));
    Ok(Value::array(out))
}

fn get_defined_vars(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    let env = caller_env(vm);
    let mut out = PhpArray::new();
    for (k, v) in env.defined_vars() {
        out.insert(ArrayKey::from_php_str(k), match v {
            Value::Ref(r) => r.get(),
            v => v,
        });
    }
    Ok(Value::array(out))
}

fn get_resource_type(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    match a.get(0) {
        Value::Resource(r) => Ok(Value::str(r.type_name())),
        other => {
            let msg = format!("must be of type resource, {} given", other.value_name());
            Err(vm.argument_type_error(1, &msg))
        }
    }
}

fn get_resource_id(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    match a.get(0) {
        Value::Resource(r) => Ok(Value::Int(r.id())),
        other => {
            let msg = format!("must be of type resource, {} given", other.value_name());
            Err(vm.argument_type_error(1, &msg))
        }
    }
}

fn get_resources(_vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(Value::empty_array())
}

fn get_loaded_extensions(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    if a.bool(0) {
        return Ok(Value::empty_array());
    }
    Ok(Value::array(PhpArray::from_values(
        vm.modules.iter().map(|m| Value::str(m)).collect(),
    )))
}

fn extension_loaded(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let n = a.str(0).to_ascii_lowercase();
    Ok(Value::Bool(vm.modules.iter().any(|m| m.eq_ignore_ascii_case(&n.to_string_lossy()))))
}

fn get_extension_funcs(_vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(false))
}

fn get_defined_constants(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let categorize = a.bool(0);
    if !categorize {
        let mut out = PhpArray::new();
        for c in vm.constants.iter() {
            out.insert(ArrayKey::Str(c.name.clone()), c.value.clone());
        }
        return Ok(Value::array(out));
    }
    let mut groups: Vec<(String, PhpArray)> = Vec::new();
    for c in vm.constants.iter() {
        let m = c.module.unwrap_or("user").to_string();
        let idx = match groups.iter().position(|(n, _)| *n == m) {
            Some(i) => i,
            None => {
                groups.push((m, PhpArray::new()));
                groups.len() - 1
            }
        };
        groups[idx].1.insert(ArrayKey::Str(c.name.clone()), c.value.clone());
    }
    let mut out = PhpArray::new();
    for (n, a) in groups {
        out.insert(ArrayKey::from(n.as_str()), Value::array(a));
    }
    Ok(Value::array(out))
}

fn debug_backtrace(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let options = a.int(0);
    let limit = a.int(1).max(0) as usize;
    let bt = vm.backtrace_ex(1, options & 2 != 0, options & 1 != 0, limit);
    Ok(Value::array(bt))
}

fn debug_print_backtrace(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let options = a.int(0);
    let limit = a.int(1).max(0) as usize;
    let bt = vm.backtrace_ex(1, options & 2 != 0, false, limit);
    let max_len = vm.ini_int("zend.exception_string_param_max_len").max(0) as usize;
    let s = crate::errors::trace_to_string(&bt, false, max_len, vm.precision);
    vm.echo(&s)?;
    Ok(Value::Null)
}

fn zend_version(_vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(Value::str(crate::vm::version::ZEND_VERSION))
}

fn zend_thread_id(_vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(Value::Int(1))
}

fn gc_mem_caches(_vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(Value::Int(0))
}

fn gc_collect_cycles(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    vm.run_pending_destructors()?;
    Ok(Value::Int(0))
}

fn gc_enabled(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(vm.ini_bool("zend.enable_gc")))
}

fn gc_enable(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    vm.ini_set("zend.enable_gc", "1");
    Ok(Value::Null)
}

fn gc_disable(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    vm.ini_set("zend.enable_gc", "0");
    Ok(Value::Null)
}

fn gc_status(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    let mut a = PhpArray::new();
    a.insert_str("running", false);
    a.insert_str("protected", false);
    a.insert_str("full", false);
    a.insert_str("runs", 0i64);
    a.insert_str("collected", 0i64);
    a.insert_str("threshold", 10001i64);
    a.insert_str("buffer_size", 16384i64);
    a.insert_str("roots", 0i64);
    a.insert_str("application_time", 0.0);
    a.insert_str("collector_time", 0.0);
    a.insert_str("destructor_time", 0.0);
    a.insert_str("free_time", 0.0);
    let _ = vm;
    Ok(Value::array(a))
}

fn exit_fn(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = if a.is_passed(0) { Some(a.get(0).clone()) } else { None };
    super::eval::do_exit(vm, v)
}

fn clone_fn(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.get(0).clone();
    let with = if a.is_passed(1) { Some(a.get(1).clone()) } else { None };
    let scope = call::calling_scope(vm);
    super::objects::clone_value(vm, scope, o, with)
}

// ----- callbacks (ext/standard/basic_functions.c) -----------------------------------------

fn call_user_func(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let args: Vec<Value> = a.rest(1).iter().map(Value::unref).collect();
    let named: Vec<(PhpStr, Value)> = a.named_rest().to_vec();
    call::call_value(vm, &f, args, named, true)
}

fn call_user_func_array(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let arr = a.array(1).clone();
    let mut pos = Vec::new();
    let mut named = Vec::new();
    for (k, v) in arr.iter() {
        match k {
            ArrayKey::Int(_) => {
                if !named.is_empty() {
                    return Err(vm.throw_error("Error", "Cannot use positional argument after named argument during unpacking"));
                }
                pos.push(v.clone());
            }
            ArrayKey::Str(s) => named.push((s, v.clone())),
        }
    }
    call::call_value(vm, &f, pos, named, true)
}

fn forward_static_call(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let args: Vec<Value> = a.rest(1).iter().map(Value::unref).collect();
    forward_static(vm, f, args)
}

fn forward_static_call_array(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let args: Vec<Value> = a.array(1).values().map(Value::unref).collect();
    forward_static(vm, f, args)
}

fn forward_static(vm: &mut Vm, f: Value, args: Vec<Value>) -> PResult<Value> {
    let env = caller_env(vm);
    if env.scope.is_none() {
        return Err(vm.throw_error("Error", "Cannot call forward_static_call() when no class scope is active"));
    }
    let info = match call::check_callable(vm, &f, false) {
        Ok(i) => i,
        Err(msg) => return Err(vm.throw_type_error(format!("forward_static_call(): Argument #1 ($callback) must be a valid callback, {msg}"))),
    };
    let mut t = info.target;
    if t.this.is_none() {
        if let (Some(called), Some(fs)) = (&env.called, &t.func.scope) {
            if let Some(fsc) = vm.find_class(fs) {
                if called.instance_of(&fsc) {
                    t.called = Some(called.clone());
                }
            }
        }
    }
    call::call_target(vm, t, Bound { positional: args, named: Vec::new() }, &Caller::internal(), true)
}

fn is_callable(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let syntax_only = a.bool(1);
    let ok = if syntax_only {
        match &v {
            Value::String(_) => true,
            Value::Array(arr) => {
                arr.len() == 2
                    && matches!(arr.get_int(0).map(Value::unref), Some(Value::String(_)) | Some(Value::Object(_)))
                    && matches!(arr.get_int(1).map(Value::unref), Some(Value::String(_)))
            }
            Value::Object(o) => super::closure::closure_data(o).is_some() || o.class().find_method(b"__invoke").is_some(),
            _ => false,
        }
    } else {
        call::check_callable(vm, &v, false).is_ok()
    };
    if a.is_passed(2) {
        let name = callable_name(vm, &v);
        a.by_ref(2).set(Value::String(name));
    }
    Ok(Value::Bool(ok))
}

/// `zend_get_callable_name`.
pub fn callable_name(_vm: &mut Vm, v: &Value) -> PhpStr {
    match &*v.deref() {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            let t = a.get_int(0).map(Value::unref);
            let m = a.get_int(1).map(Value::unref);
            let cls = match &t {
                Some(Value::Object(o)) => o.class().name.to_string(),
                Some(Value::String(s)) => s.to_string(),
                _ => return PhpStr::from("Array"),
            };
            match m {
                Some(Value::String(m)) => PhpStr::from(format!("{cls}::{m}")),
                _ => PhpStr::from("Array"),
            }
        }
        Value::Object(o) => PhpStr::from(format!("{}::__invoke", o.class().name)),
        other => PhpStr::from(format!("{}", other.type_name())),
    }
}

// ----- compact / extract -------------------------------------------------------------------

fn compact(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    forbid_dynamic(vm, "compact")?;
    let env = caller_env(vm);
    let mut out = PhpArray::new();
    let args: Vec<Value> = a.values().to_vec();
    fn add(vm: &mut Vm, env: &Env, v: &Value, out: &mut PhpArray, depth: usize) -> PResult<()> {
        match &*v.deref() {
            Value::String(name) => {
                match env.lookup(name).filter(|s| env.is_set(*s)) {
                    Some(s) => {
                        out.insert(ArrayKey::from_php_str(name.clone()), env.get(s));
                    }
                    None => {
                        if name.as_bytes() == b"this" {
                            if let Some(t) = &env.this {
                                out.insert(ArrayKey::from_php_str(name.clone()), Value::Object(t.clone()));
                                return Ok(());
                            }
                        }
                        let mut m = b"compact(): Undefined variable $".to_vec();
                        m.extend_from_slice(name);
                        vm.engine_warning(PhpStr::from_vec(m))?;
                    }
                }
            }
            Value::Array(arr) => {
                if depth > 0 && false {
                    return Ok(());
                }
                for x in arr.values() {
                    add(vm, env, x, out, depth + 1)?;
                }
            }
            other => {
                vm.warning(format!(
                    "Argument #{} must be string or array of strings, {} given",
                    1,
                    other.type_name()
                ))?;
            }
        }
        Ok(())
    }
    for (i, v) in args.iter().enumerate() {
        if let Value::Array(_) | Value::String(_) = &*v.deref() {
            add(vm, &env, v, &mut out, 0)?;
        } else {
            vm.warning(format!(
                "Argument #{} must be string or array of strings, {} given",
                i + 1,
                v.type_name()
            ))?;
        }
    }
    Ok(Value::array(out))
}

const EXTR_OVERWRITE: i64 = 0;
const EXTR_SKIP: i64 = 1;
const EXTR_PREFIX_SAME: i64 = 2;
const EXTR_PREFIX_ALL: i64 = 3;
const EXTR_PREFIX_INVALID: i64 = 4;
const EXTR_PREFIX_IF_EXISTS: i64 = 5;
const EXTR_IF_EXISTS: i64 = 6;
const EXTR_REFS: i64 = 0x100;

fn valid_var_name(n: &[u8]) -> bool {
    match n.first() {
        None => false,
        Some(&c) if !(c.is_ascii_alphabetic() || c == b'_' || c >= 0x80) => false,
        _ => n.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80),
    }
}

fn extract(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    forbid_dynamic(vm, "extract")?;
    let arr_ref = a.get(0).clone();
    let flags = a.int(1);
    let prefix = a.str_opt(2).cloned();
    let refs = flags & EXTR_REFS != 0;
    let ty = flags & 0xff;
    if !(EXTR_OVERWRITE..=EXTR_IF_EXISTS).contains(&ty) {
        return Err(vm.argument_value_error(2, "must be a valid extract type"));
    }
    if ty > EXTR_SKIP && ty <= EXTR_PREFIX_IF_EXISTS && !a.is_passed(2) {
        return Err(vm.argument_count_error("extract(): Argument #3 ($prefix) is required when using this extract type"));
    }
    if let Some(p) = &prefix {
        if !p.is_empty() && !valid_var_name(p) {
            return Err(vm.argument_value_error(3, "must be a valid identifier"));
        }
    }
    let env = caller_env(vm);
    let arr = match &arr_ref {
        Value::Ref(r) => match r.get() {
            Value::Array(a) => a,
            _ => return Ok(Value::Int(0)),
        },
        Value::Array(a) => a.clone(),
        _ => return Ok(Value::Int(0)),
    };
    let mut count = 0;
    let keys: Vec<ArrayKey> = arr.keys().collect();
    for k in keys {
        let name: PhpStr = match &k {
            ArrayKey::Str(s) => s.clone(),
            ArrayKey::Int(i) => {
                if ty == EXTR_PREFIX_ALL || ty == EXTR_PREFIX_INVALID {
                    PhpStr::from(i.to_string())
                } else {
                    continue;
                }
            }
        };
        let exists = env.lookup(&name).is_some_and(|s| env.is_set(s));
        let mut target = name.clone();
        let with_prefix = |p: &Option<PhpStr>, n: &PhpStr| -> PhpStr {
            let mut v = p.as_ref().map(|p| p.to_vec()).unwrap_or_default();
            v.push(b'_');
            v.extend_from_slice(n);
            PhpStr::from_vec(v)
        };
        match ty {
            EXTR_OVERWRITE => {}
            EXTR_SKIP => {
                if exists {
                    continue;
                }
            }
            EXTR_PREFIX_SAME => {
                if exists {
                    target = with_prefix(&prefix, &name);
                }
            }
            EXTR_PREFIX_ALL => target = with_prefix(&prefix, &name),
            EXTR_PREFIX_INVALID => {
                if !valid_var_name(&name) || matches!(k, ArrayKey::Int(_)) {
                    target = with_prefix(&prefix, &name);
                }
            }
            EXTR_PREFIX_IF_EXISTS => {
                if !exists {
                    continue;
                }
                target = with_prefix(&prefix, &name);
            }
            EXTR_IF_EXISTS => {
                if !exists {
                    continue;
                }
            }
            _ => {}
        }
        if !valid_var_name(&target) {
            continue;
        }
        if target.as_bytes() == b"this" {
            if ty == EXTR_SKIP || ty == EXTR_IF_EXISTS && !exists {
                continue;
            }
            return Err(vm.throw_error("Error", "Cannot re-assign $this"));
        }
        if target.as_bytes() == b"GLOBALS" {
            continue;
        }
        let slot = env.slot_for(&target);
        if refs {
            let r = match &arr_ref {
                Value::Ref(src) => {
                    let mut b = src.borrow_mut();
                    match &mut *b {
                        Value::Array(a) => {
                            let a = Rc::make_mut(a);
                            crate::value::array::make_element_ref(a, k.clone())
                        }
                        _ => continue,
                    }
                }
                _ => crate::value::PhpRef::new(arr.get(&k).cloned().unwrap_or_default().unref()),
            };
            env.bind_ref(slot, r);
        } else {
            let v = arr.get(&k).cloned().unwrap_or_default().unref();
            env.set(slot, v);
        }
        count += 1;
    }
    Ok(Value::Int(count))
}

// ----- shutdown / ticks ----------------------------------------------------------------------

fn register_shutdown_function(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let args: Vec<Value> = a.rest(1).iter().map(Value::unref).collect();
    vm.interp.shutdown_functions.push((f, args));
    Ok(Value::Null)
}

fn register_tick_function(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let args: Vec<Value> = a.rest(1).iter().map(Value::unref).collect();
    vm.interp.tick_functions.push((f, args));
    Ok(Value::Bool(true))
}

/// What `zend_fcc_equals` compares: the function (or the closure object of
/// an anonymous function), the object and a trampoline's method name.
fn tick_callable_key(vm: &mut Vm, v: &Value) -> Option<(String, u32, Option<Vec<u8>>)> {
    let info = call::check_callable(vm, v, false).ok()?;
    let t = info.target;
    if let Some(c) = &t.closure {
        if !c.from_callable {
            if let Value::Object(o) = &*v.deref() {
                return Some((String::new(), o.id(), None));
            }
        }
    }
    let name = t.func.display_name().to_ascii_lowercase();
    let this = t.this.as_ref().map_or(0, |o| o.id());
    let magic = t.magic_name.as_ref().map(|m| m.as_bytes().to_ascii_lowercase());
    Some((name, this, magic))
}

fn unregister_tick_function(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let f = a.get(0).clone();
    let Some(key) = tick_callable_key(vm, &f) else { return Ok(Value::Null) };
    let entries: Vec<Value> = vm.interp.tick_functions.iter().map(|(g, _)| g.clone()).collect();
    let calling = vm.interp.ticks_calling.clone();
    for (i, g) in entries.iter().enumerate() {
        if tick_callable_key(vm, g).as_ref() != Some(&key) {
            continue;
        }
        if calling.iter().any(|c| crate::value::ops::is_identical(c, g)) {
            return Err(vm.throw_error("Error", "Registered tick function cannot be unregistered while it is being executed"));
        }
        vm.interp.tick_functions.remove(i);
        break;
    }
    Ok(Value::Null)
}

/// Runs the registered tick functions (`declare(ticks=N)`).
/// `ZEND_TICKS`: every `n`th tick runs the tick functions.
pub fn tick(vm: &mut Vm, n: u32) -> PResult<()> {
    vm.interp.ticks_count += 1;
    if vm.interp.ticks_count >= n {
        vm.interp.ticks_count = 0;
        run_ticks(vm)?;
    }
    Ok(())
}

pub fn run_ticks(vm: &mut Vm) -> PResult<()> {
    let ticks = vm.interp.tick_functions.clone();
    // Fibers cannot switch while tick functions run (ZEND_TICKS).
    super::fiber::block_switch();
    let r = ticks.into_iter().try_for_each(|(f, args)| {
        // A tick function is not re-entered (user_tick_function_call).
        if vm.interp.ticks_calling.iter().any(|g| crate::value::ops::is_identical(g, &f)) {
            return Ok(());
        }
        vm.interp.ticks_calling.push(f.clone());
        let r = vm.call(&f, args).map(drop);
        if let Some(i) = vm.interp.ticks_calling.iter().rposition(|g| crate::value::ops::is_identical(g, &f)) {
            vm.interp.ticks_calling.remove(i);
        }
        r
    });
    super::fiber::unblock_switch();
    r
}

// ----- assert -------------------------------------------------------------------------------------

/// `assert()` compiled specially (`zend_compile_assert`).
pub fn eval_assert(vm: &mut Vm, cx: &Ctx, args: &ArgList, text: &PhpStr, line: u32) -> PResult<Value> {
    let mode = vm.ini_int("zend.assertions");
    if mode <= 0 {
        return Ok(Value::Bool(true));
    }
    let f = vm.find_function(b"assert");
    let b = call::eval_args(vm, cx, args, f.as_ref())?;
    vm.current_line = line;
    let mut pos = b.positional;
    if pos.len() == 1 && b.named.is_empty() {
        pos.push(Value::String(text.clone()));
    }
    let named = b.named;
    match f {
        Some(f) => vm.invoke_native(
            &f,
            CallArgs {
                positional: pos,
                named,
                strict: cx.strict(),
                ..CallArgs::default()
            },
        ),
        None => do_assert(vm, pos.first().cloned().unwrap_or_default(), pos.get(1).cloned()),
    }
}

fn do_assert(vm: &mut Vm, v: Value, desc: Option<Value>) -> PResult<Value> {
    if vm.ini_int("zend.assertions") <= 0 || !vm.ini_bool("assert.active") {
        return Ok(Value::Bool(true));
    }
    if super::eval::to_bool(vm, &v.unref())? {
        return Ok(Value::Bool(true));
    }
    let cb = vm.ini_get("assert.callback").filter(|s| !s.is_empty());
    if let Some(cb) = cb {
        let args = vec![
            Value::String(vm.current_file.clone()),
            Value::Int(vm.current_line as i64),
            Value::Null,
        ];
        let mut args = args;
        if let Some(d) = &desc {
            args.push(d.clone());
        }
        vm.call(&Value::String(cb), args)?;
    }
    if let Some(Value::Object(o)) = desc.as_ref().map(Value::unref) {
        if o.class().instance_of_name(b"Throwable") {
            return Err(Flow::Exception(o));
        }
    }
    let msg = match &desc {
        Some(Value::Null) | None => PhpStr::from("assert(false)"),
        Some(d) => vm.to_php_string(&d.unref())?,
    };
    if vm.ini_bool("assert.exception") {
        return Err(vm.throw_error("AssertionError", msg));
    }
    if vm.ini_bool("assert.warning") {
        vm.warning(msg)?;
    }
    if vm.ini_bool("assert.bail") {
        vm.exit_status = 255;
        return Err(Flow::Exit(255));
    }
    Ok(Value::Bool(false))
}

fn assert_fn(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let v = a.get(0).clone();
    let desc = if a.is_passed(1) { Some(a.get(1).clone()) } else { None };
    do_assert(vm, v, desc)
}

// ----- registration --------------------------------------------------------------------------------

native!(FUNC_NUM_ARGS, "function func_num_args(): int {}", func_num_args);
native!(FUNC_GET_ARG, "function func_get_arg(int $position): mixed {}", func_get_arg);
native!(FUNC_GET_ARGS, "function func_get_args(): array {}", func_get_args);
native!(STRLEN, "function strlen(string $string): int {}", strlen);
native!(STRCMP, "function strcmp(string $string1, string $string2): int {}", strcmp);
native!(STRNCMP, "function strncmp(string $string1, string $string2, int $length): int {}", strncmp);
native!(STRCASECMP, "function strcasecmp(string $string1, string $string2): int {}", strcasecmp);
native!(STRNCASECMP, "function strncasecmp(string $string1, string $string2, int $length): int {}", strncasecmp);
native!(ERROR_REPORTING, "function error_reporting(?int $error_level = null): int {}", error_reporting);
native!(DEFINE, "function define(string $constant_name, mixed $value, bool $case_insensitive = false): bool {}", define);
native!(DEFINED, "function defined(string $constant_name): bool {}", defined);
native!(GET_CLASS, "function get_class(object $object = UNKNOWN): string {}", get_class);
native!(GET_CALLED_CLASS, "function get_called_class(): string {}", get_called_class);
native!(GET_PARENT_CLASS, "function get_parent_class(object|string $object_or_class = UNKNOWN): string|false {}", get_parent_class);
native!(IS_SUBCLASS_OF, "function is_subclass_of(mixed $object_or_class, string $class, bool $allow_string = true): bool {}", is_subclass_of);
native!(IS_A, "function is_a(mixed $object_or_class, string $class, bool $allow_string = false): bool {}", is_a);
native!(GET_CLASS_VARS, "function get_class_vars(string $class): array {}", get_class_vars);
native!(GET_OBJECT_VARS, "function get_object_vars(object $object): array {}", get_object_vars);
native!(GET_MANGLED_OBJECT_VARS, "function get_mangled_object_vars(object $object): array {}", get_mangled_object_vars);
native!(GET_CLASS_METHODS, "function get_class_methods(object|string $object_or_class): array {}", get_class_methods);
native!(METHOD_EXISTS, "function method_exists($object_or_class, string $method): bool {}", method_exists);
native!(PROPERTY_EXISTS, "function property_exists($object_or_class, string $property): bool {}", property_exists);
native!(CLASS_EXISTS, "function class_exists(string $class, bool $autoload = true): bool {}", class_exists);
native!(INTERFACE_EXISTS, "function interface_exists(string $interface, bool $autoload = true): bool {}", interface_exists);
native!(TRAIT_EXISTS, "function trait_exists(string $trait, bool $autoload = true): bool {}", trait_exists);
native!(ENUM_EXISTS, "function enum_exists(string $enum, bool $autoload = true): bool {}", enum_exists);
native!(FUNCTION_EXISTS, "function function_exists(string $function): bool {}", function_exists);
native!(CLASS_ALIAS, "function class_alias(string $class, string $alias, bool $autoload = true): bool {}", class_alias);
native!(GET_INCLUDED_FILES, "function get_included_files(): array {}", get_included_files);
native!(GET_REQUIRED_FILES, "function get_required_files(): array {}", get_included_files);
native!(TRIGGER_ERROR, "function trigger_error(string $message, int $error_level = E_USER_NOTICE): true {}", trigger_error);
native!(USER_ERROR, "function user_error(string $message, int $error_level = E_USER_NOTICE): true {}", trigger_error);
native!(SET_ERROR_HANDLER, "function set_error_handler(?callable $callback, int $error_levels = E_ALL) {}", set_error_handler);
native!(RESTORE_ERROR_HANDLER, "function restore_error_handler(): true {}", restore_error_handler);
native!(GET_ERROR_HANDLER, "function get_error_handler(): ?callable {}", get_error_handler);
native!(SET_EXCEPTION_HANDLER, "function set_exception_handler(?callable $callback) {}", set_exception_handler);
native!(RESTORE_EXCEPTION_HANDLER, "function restore_exception_handler(): true {}", restore_exception_handler);
native!(GET_EXCEPTION_HANDLER, "function get_exception_handler(): ?callable {}", get_exception_handler);
native!(GET_DECLARED_CLASSES, "function get_declared_classes(): array {}", get_declared_classes);
native!(GET_DECLARED_TRAITS, "function get_declared_traits(): array {}", get_declared_traits);
native!(GET_DECLARED_INTERFACES, "function get_declared_interfaces(): array {}", get_declared_interfaces);
native!(GET_DEFINED_FUNCTIONS, "function get_defined_functions(bool $exclude_disabled = true): array {}", get_defined_functions);
native!(GET_DEFINED_VARS, "function get_defined_vars(): array {}", get_defined_vars);
native!(GET_RESOURCE_TYPE, "function get_resource_type($resource): string {}", get_resource_type);
native!(GET_RESOURCE_ID, "function get_resource_id($resource): int {}", get_resource_id);
native!(GET_RESOURCES, "function get_resources(?string $type = null): array {}", get_resources);
native!(GET_LOADED_EXTENSIONS, "function get_loaded_extensions(bool $zend_extensions = false): array {}", get_loaded_extensions);
native!(GET_DEFINED_CONSTANTS, "function get_defined_constants(bool $categorize = false): array {}", get_defined_constants);
native!(DEBUG_BACKTRACE, "function debug_backtrace(int $options = DEBUG_BACKTRACE_PROVIDE_OBJECT, int $limit = 0): array {}", debug_backtrace);
native!(DEBUG_PRINT_BACKTRACE, "function debug_print_backtrace(int $options = 0, int $limit = 0): void {}", debug_print_backtrace);
native!(EXTENSION_LOADED, "function extension_loaded(string $extension): bool {}", extension_loaded);
native!(GET_EXTENSION_FUNCS, "function get_extension_funcs(string $extension): array|false {}", get_extension_funcs);
native!(ZEND_VERSION_FN, "function zend_version(): string {}", zend_version);
native!(ZEND_THREAD_ID, "function zend_thread_id(): int {}", zend_thread_id);
native!(GC_MEM_CACHES, "function gc_mem_caches(): int {}", gc_mem_caches);
native!(GC_COLLECT_CYCLES, "function gc_collect_cycles(): int {}", gc_collect_cycles);
native!(GC_ENABLED, "function gc_enabled(): bool {}", gc_enabled);
native!(GC_ENABLE, "function gc_enable(): void {}", gc_enable);
native!(GC_DISABLE, "function gc_disable(): void {}", gc_disable);
native!(GC_STATUS, "function gc_status(): array {}", gc_status);
native!(EXIT, "function exit(string|int $status = 0): never {}", exit_fn);
native!(DIE, "function die(string|int $status = 0): never {}", exit_fn);
native!(CLONE, "function clone(object $object, array $withProperties = []): object {}", clone_fn);

native!(CALL_USER_FUNC, "function call_user_func(callable $callback, mixed ...$args): mixed {}", call_user_func);
native!(CALL_USER_FUNC_ARRAY, "function call_user_func_array(callable $callback, array $args): mixed {}", call_user_func_array);
native!(FORWARD_STATIC_CALL, "function forward_static_call(callable $callback, mixed ...$args): mixed {}", forward_static_call);
native!(FORWARD_STATIC_CALL_ARRAY, "function forward_static_call_array(callable $callback, array $args): mixed {}", forward_static_call_array);
native!(IS_CALLABLE, "function is_callable(mixed $value, bool $syntax_only = false, &$callable_name = null): bool {}", is_callable);
native!(CONSTANT, "function constant(string $name): mixed {}", constant);
native!(COMPACT, "function compact($var_name, ...$var_names): array {}", compact);
native!(EXTRACT, "/** @prefer-ref $array */ function extract(array &$array, int $flags = EXTR_OVERWRITE, string $prefix = \"\"): int {}", extract);
native!(REGISTER_SHUTDOWN_FUNCTION, "function register_shutdown_function(callable $callback, mixed ...$args): void {}", register_shutdown_function);
native!(REGISTER_TICK_FUNCTION, "function register_tick_function(callable $callback, mixed ...$args): bool {}", register_tick_function);
native!(UNREGISTER_TICK_FUNCTION, "function unregister_tick_function(callable $callback): void {}", unregister_tick_function);
native!(ASSERT, "function assert(mixed $assertion, Throwable|string|null $description = null): bool {}", assert_fn);

/// Core additions: built-in functions and engine classes.
pub fn register_core_extras(r: &mut Registry<'_>) {
    r.functions(&[
        &FUNC_NUM_ARGS, &FUNC_GET_ARG, &FUNC_GET_ARGS, &STRLEN, &STRCMP, &STRNCMP, &STRCASECMP, &STRNCASECMP,
        &ERROR_REPORTING, &DEFINE, &DEFINED, &GET_CLASS, &GET_CALLED_CLASS, &GET_PARENT_CLASS, &IS_SUBCLASS_OF,
        &IS_A, &GET_CLASS_VARS, &GET_OBJECT_VARS, &GET_MANGLED_OBJECT_VARS, &GET_CLASS_METHODS, &METHOD_EXISTS,
        &PROPERTY_EXISTS, &CLASS_EXISTS, &INTERFACE_EXISTS, &TRAIT_EXISTS, &ENUM_EXISTS, &FUNCTION_EXISTS,
        &CLASS_ALIAS, &GET_INCLUDED_FILES, &GET_REQUIRED_FILES, &TRIGGER_ERROR, &USER_ERROR, &SET_ERROR_HANDLER,
        &RESTORE_ERROR_HANDLER, &GET_ERROR_HANDLER, &SET_EXCEPTION_HANDLER, &RESTORE_EXCEPTION_HANDLER,
        &GET_EXCEPTION_HANDLER, &GET_DECLARED_CLASSES, &GET_DECLARED_TRAITS, &GET_DECLARED_INTERFACES,
        &GET_DEFINED_FUNCTIONS, &GET_DEFINED_VARS, &GET_RESOURCE_TYPE, &GET_RESOURCE_ID, &GET_RESOURCES,
        &GET_LOADED_EXTENSIONS, &GET_DEFINED_CONSTANTS, &DEBUG_BACKTRACE, &DEBUG_PRINT_BACKTRACE,
        &EXTENSION_LOADED, &GET_EXTENSION_FUNCS, &ZEND_VERSION_FN, &GC_MEM_CACHES, &GC_COLLECT_CYCLES,
        &GC_ENABLED, &GC_ENABLE, &GC_DISABLE, &GC_STATUS, &EXIT, &DIE, &CLONE,
    ]);
    let _ = &ZEND_THREAD_ID;
    super::closure::register(r);
    super::generator::register(r);
    super::fiber::register(r);
    register_attributes(r);
    super::weak::register(r);
    crate::output::register_functions(r);
}

/// The engine-coupled `standard` functions.
pub fn register_standard(r: &mut Registry<'_>) {
    r.functions(&[
        &CALL_USER_FUNC_ARRAY, &FORWARD_STATIC_CALL_ARRAY, &IS_CALLABLE, &CONSTANT, &EXTRACT,
        &REGISTER_TICK_FUNCTION, &UNREGISTER_TICK_FUNCTION, &ASSERT, &REGISTER_SHUTDOWN_FUNCTION,
    ]);
    r.function_with_flags(&CALL_USER_FUNC, fn_flags::NAMED_VARIADIC);
    r.function_with_flags(&FORWARD_STATIC_CALL, fn_flags::NAMED_VARIADIC);
    r.function(&COMPACT);
    for (n, v) in [
        ("EXTR_OVERWRITE", EXTR_OVERWRITE),
        ("EXTR_SKIP", EXTR_SKIP),
        ("EXTR_PREFIX_SAME", EXTR_PREFIX_SAME),
        ("EXTR_PREFIX_ALL", EXTR_PREFIX_ALL),
        ("EXTR_PREFIX_INVALID", EXTR_PREFIX_INVALID),
        ("EXTR_PREFIX_IF_EXISTS", EXTR_PREFIX_IF_EXISTS),
        ("EXTR_IF_EXISTS", EXTR_IF_EXISTS),
        ("EXTR_REFS", EXTR_REFS),
    ] {
        r.constant(n, v);
    }
}

// ----- interfaces with methods ----------------------------------------------------------------

fn abstract_stub(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Cannot call abstract method"))
}

macro_rules! iface_methods {
    ($($name:ident = $sig:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: abstract_stub };)*
    };
}

iface_methods! {
    IT_CURRENT = "/** @tentative-return-type */ public function current(): mixed {}";
    IT_NEXT = "/** @tentative-return-type */ public function next(): void {}";
    IT_KEY = "/** @tentative-return-type */ public function key(): mixed {}";
    IT_VALID = "/** @tentative-return-type */ public function valid(): bool {}";
    IT_REWIND = "/** @tentative-return-type */ public function rewind(): void {}";
    IA_GET_ITERATOR = "/** @tentative-return-type */ public function getIterator(): Traversable {}";
    AA_EXISTS = "/** @tentative-return-type */ public function offsetExists(mixed $offset): bool {}";
    AA_GET = "/** @tentative-return-type */ public function offsetGet(mixed $offset): mixed {}";
    AA_SET = "/** @tentative-return-type */ public function offsetSet(mixed $offset, mixed $value): void {}";
    AA_UNSET = "/** @tentative-return-type */ public function offsetUnset(mixed $offset): void {}";
    CO_COUNT = "/** @tentative-return-type */ public function count(): int {}";
    ST_TOSTRING = "public function __toString(): string {}";
    SE_SERIALIZE = "public function serialize() {}";
    SE_UNSERIALIZE = "public function unserialize(string $data) {}";
}

/// Abstract method signatures of the engine's interfaces.
pub fn interface_methods(name: &str) -> Vec<&'static NativeFunction> {
    match name {
        "Iterator" => vec![&IT_CURRENT, &IT_NEXT, &IT_KEY, &IT_VALID, &IT_REWIND],
        "IteratorAggregate" => vec![&IA_GET_ITERATOR],
        "ArrayAccess" => vec![&AA_EXISTS, &AA_GET, &AA_SET, &AA_UNSET],
        "Countable" => vec![&CO_COUNT],
        "Stringable" => vec![&ST_TOSTRING],
        "Serializable" => vec![&SE_SERIALIZE, &SE_UNSERIALIZE],
        _ => Vec::new(),
    }
}

// ----- attribute classes -----------------------------------------------------------------------

fn attr_construct(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let Some(o) = a.this().cloned() else { return Ok(Value::Null) };
    let sig = a.function().signature().clone();
    for (i, p) in sig.params.iter().enumerate() {
        if p.variadic {
            break;
        }
        let v = a.get(i).clone();
        if !v.is_undef() {
            o.set_prop(&p.name, v.unref());
        }
    }
    let _ = vm;
    Ok(Value::Null)
}

static ATTRIBUTE_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct(int $flags = Attribute::TARGET_ALL) {}",
    handler: attr_construct,
};
static DEPRECATED_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct(?string $message = null, ?string $since = null) {}",
    handler: attr_construct,
};
static NODISCARD_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct(?string $message = null) {}",
    handler: attr_construct,
};
static EMPTY_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct() {}",
    handler: attr_construct,
};

fn register_attributes(r: &mut Registry<'_>) {
    use crate::class::{ClassConstant, PropertyInfo};
    let mut attr = ClassEntry::new("Attribute", class_flags::FINAL);
    for (n, v) in [
        ("TARGET_CLASS", 1i64),
        ("TARGET_FUNCTION", 2),
        ("TARGET_METHOD", 4),
        ("TARGET_PROPERTY", 8),
        ("TARGET_CLASS_CONSTANT", 16),
        ("TARGET_PARAMETER", 32),
        ("TARGET_CONSTANT", 64),
        ("TARGET_ALL", 127),
        ("IS_REPEATABLE", 128),
    ] {
        attr.add_constant(ClassConstant::new(n, Value::Int(v), "Attribute"));
    }
    let mut flags = PropertyInfo::new("flags", "Attribute", member_flags::PUBLIC);
    flags.ty = Some(crate::types::TypeDecl::parse("int").expect("int"));
    flags.default = Value::Undef;
    attr.declare_property(flags);
    attr.add_method(Function::native_method(&ATTRIBUTE_CTOR, "Attribute", member_flags::PUBLIC));
    r.class(attr);
    for name in ["ReturnTypeWillChange", "AllowDynamicProperties", "SensitiveParameter", "Override"] {
        let mut ce = ClassEntry::new(name, class_flags::FINAL);
        ce.add_method(Function::native_method(&EMPTY_CTOR, name, member_flags::PUBLIC));
        r.class(ce);
    }
    let mut dep = ClassEntry::new("Deprecated", class_flags::FINAL);
    for p in ["message", "since"] {
        let mut info = PropertyInfo::new(p, "Deprecated", member_flags::PUBLIC | member_flags::READONLY);
        info.ty = Some(crate::types::TypeDecl::parse("?string").expect("type"));
        info.default = Value::Undef;
        dep.declare_property(info);
    }
    dep.add_method(Function::native_method(&DEPRECATED_CTOR, "Deprecated", member_flags::PUBLIC));
    r.class(dep);
    let mut nd = ClassEntry::new("NoDiscard", class_flags::FINAL);
    let mut info = PropertyInfo::new("message", "NoDiscard", member_flags::PUBLIC | member_flags::READONLY);
    info.ty = Some(crate::types::TypeDecl::parse("?string").expect("type"));
    info.default = Value::Undef;
    nd.declare_property(info);
    nd.add_method(Function::native_method(&NODISCARD_CTOR, "NoDiscard", member_flags::PUBLIC));
    r.class(nd);
    // SensitiveParameterValue.
    let mut spv = ClassEntry::new("SensitiveParameterValue", class_flags::FINAL | class_flags::NOT_SERIALIZABLE);
    let mut v = PropertyInfo::new("value", "SensitiveParameterValue", member_flags::PRIVATE | member_flags::READONLY);
    v.ty = Some(crate::types::TypeDecl::mixed());
    v.default = Value::Undef;
    spv.declare_property(v);
    r.class(spv);
    let _ = ObjRef::ptr_eq;
}
