//! Callables (`zend_is_callable_ex` and friends, `Zend/zend_API.c`):
//! resolving strings, arrays and objects to a function, with php-src's
//! error texts ("must be a valid callback, ...") and the 8.2 deprecations of
//! `self`/`parent`/`static` in callables.

use std::rc::Rc;

use crate::class::{ClassRef, Function, member_flags};
use crate::object::ObjRef;
use crate::value::{PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::Env;
use super::call::{CallableInfo, Target, calling_env};

/// `zend_fcall_info_cache` under construction.
#[derive(Default)]
struct Fcc {
    calling_scope: Option<ClassRef>,
    called_scope: Option<ClassRef>,
    object: Option<ObjRef>,
    func: Option<Rc<Function>>,
    /// `__call`/`__callStatic` trampoline: the method name.
    magic: Option<PhpStr>,
}

/// The frame a callable is resolved at (the innermost userland frame).
struct Frame {
    env: Rc<Env>,
}

impl Frame {
    /// `get_scope`.
    fn scope(&self) -> Option<ClassRef> {
        self.env.scope.clone()
    }

    /// `zend_get_this_object`.
    fn this(&self) -> Option<ObjRef> {
        self.env.this.clone()
    }

    /// `zend_get_called_scope`.
    fn called_scope(&self) -> Option<ClassRef> {
        match &self.env.this {
            Some(o) => Some(o.class().clone()),
            None => self.env.called.clone(),
        }
    }
}

fn same(a: &ClassRef, b: &ClassRef) -> bool {
    Rc::ptr_eq(a, b) || a.lc_name == b.lc_name
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

/// `zend_check_method_accessible`.
fn method_accessible(vm: &Vm, f: &Function, scope: Option<&ClassRef>) -> bool {
    if f.flags & member_flags::PRIVATE != 0 {
        return scope.is_some_and(|s| f.scope.as_ref().is_some_and(|c| c.eq_ignore_ascii_case(&s.name)));
    }
    if f.flags & member_flags::PROTECTED != 0 {
        let root = super::call::method_root_class(f, vm);
        return super::objects::protected_ok(&root, scope, vm);
    }
    true
}

/// `get_static_method_fallback`.
fn static_method_fallback(frame: &Frame, ce: &ClassRef, name: &PhpStr, fcc: &mut Fcc) -> bool {
    if ce.magic.call.is_some() {
        if let Some(this) = frame.this() {
            if this.class().instance_of(ce) {
                fcc.func = ce.magic.call.clone();
                fcc.magic = Some(name.clone());
                return true;
            }
        }
    }
    if ce.magic.call_static.is_some() {
        fcc.func = ce.magic.call_static.clone();
        fcc.magic = Some(name.clone());
        return true;
    }
    false
}

/// `zend_is_callable_check_class`: resolves the class part of a callable.
fn check_class(
    vm: &mut Vm,
    name: &[u8],
    scope: Option<&ClassRef>,
    frame: &Frame,
    fcc: &mut Fcc,
    strict_class: &mut bool,
    suppress: bool,
) -> PResult<Result<(), String>> {
    let lc = name.to_ascii_lowercase();
    *strict_class = false;
    match lc.as_slice() {
        b"self" => {
            let Some(scope) = scope else {
                return Ok(Err("cannot access \"self\" when no class scope is active".into()));
            };
            if !suppress {
                if let Err(e) = vm.engine_deprecated("Use of \"self\" in callables is deprecated") {
                    return Ok(Err(deprecation_failed(e)));
                }
            }
            fcc.called_scope = frame.called_scope().filter(|c| c.instance_of(scope)).or_else(|| Some(scope.clone()));
            fcc.calling_scope = Some(scope.clone());
            if fcc.object.is_none() {
                fcc.object = frame.this();
            }
            Ok(Ok(()))
        }
        b"parent" => {
            let Some(scope) = scope else {
                return Ok(Err("cannot access \"parent\" when no class scope is active".into()));
            };
            let Some(parent) = scope.parent.clone() else {
                return Ok(Err("cannot access \"parent\" when current class scope has no parent".into()));
            };
            if !suppress {
                if let Err(e) = vm.engine_deprecated("Use of \"parent\" in callables is deprecated") {
                    return Ok(Err(deprecation_failed(e)));
                }
            }
            fcc.called_scope = frame.called_scope().filter(|c| c.instance_of(&parent)).or_else(|| Some(parent.clone()));
            fcc.calling_scope = Some(parent);
            if fcc.object.is_none() {
                fcc.object = frame.this();
            }
            *strict_class = true;
            Ok(Ok(()))
        }
        b"static" => {
            let Some(called) = frame.called_scope() else {
                return Ok(Err("cannot access \"static\" when no class scope is active".into()));
            };
            if !suppress {
                if let Err(e) = vm.engine_deprecated("Use of \"static\" in callables is deprecated") {
                    return Ok(Err(deprecation_failed(e)));
                }
            }
            fcc.called_scope = Some(called.clone());
            fcc.calling_scope = Some(called);
            if fcc.object.is_none() {
                fcc.object = frame.this();
            }
            *strict_class = true;
            Ok(Ok(()))
        }
        _ => match vm.lookup_class(name)? {
            Some(ce) => {
                let frame_scope = frame.scope();
                fcc.calling_scope = Some(ce.clone());
                match (&frame_scope, &fcc.object) {
                    (Some(fs), None) => {
                        let this = frame.this().filter(|t| t.class().instance_of(fs) && fs.instance_of(&ce));
                        match this {
                            Some(t) => {
                                fcc.called_scope = Some(t.class().clone());
                                fcc.object = Some(t);
                            }
                            None => fcc.called_scope = Some(ce),
                        }
                    }
                    _ => {
                        fcc.called_scope = Some(fcc.object.as_ref().map_or(ce, |o| o.class().clone()));
                    }
                }
                *strict_class = true;
                Ok(Ok(()))
            }
            None => Ok(Err(format!("class \"{}\" not found", String::from_utf8_lossy(name)))),
        },
    }
}

thread_local! {
    /// An exception thrown by an error handler for a callable deprecation
    /// (php-src leaves it in `EG(exception)` and fails the check).
    static PENDING: std::cell::RefCell<Option<Flow>> = const { std::cell::RefCell::new(None) };
}

fn deprecation_failed(e: Flow) -> String {
    PENDING.with(|p| *p.borrow_mut() = Some(e));
    "cannot use \"self\"/\"parent\"/\"static\" in callables".into()
}

/// Takes the exception left by a failed callable check, if any.
pub fn take_pending() -> Option<Flow> {
    PENDING.with(|p| p.borrow_mut().take())
}

/// Like [`is_callable_ex`], but a pending exception from a deprecation
/// handler is returned as the error flow.
pub fn is_callable_or_throw(vm: &mut Vm, v: &Value, suppress: bool) -> PResult<Result<CallableInfo, String>> {
    let r = is_callable_ex(vm, v, suppress)?;
    if let Some(e) = take_pending() {
        return Err(e);
    }
    Ok(r)
}

/// `zend_is_method_callable`.
fn is_method_callable(
    vm: &mut Vm,
    callable: &PhpStr,
    frame: &Frame,
    fcc: &mut Fcc,
    mut strict_class: bool,
    suppress: bool,
) -> PResult<Result<(), String>> {
    let ce_org = fcc.calling_scope.take();
    let bytes = callable.as_bytes();
    // The last "::" separates the class (or namespace) from the method.
    let colon = bytes.iter().rposition(|&c| c == b':').filter(|&i| i > 0 && bytes[i - 1] == b':');
    let (mname, ftable) = if let Some(c) = colon {
        let class_len = c - 1;
        if class_len == 0 {
            return Ok(Err("invalid function name".into()));
        }
        let scope = ce_org.clone().or_else(|| frame.scope());
        let class_name = &bytes[..class_len];
        if let Err(e) = check_class(vm, class_name, scope.as_ref(), frame, fcc, &mut strict_class, suppress || ce_org.is_some())? {
            return Ok(Err(e));
        }
        let calling = fcc.calling_scope.clone().expect("class resolved");
        if let Some(org) = &ce_org {
            if !org.instance_of(&calling) {
                return Ok(Err(format!("class {} is not a subclass of {}", org.name, calling.name)));
            }
            if !suppress {
                let msg = format!(
                    "Callables of the form [\"{}\", \"{}\"] are deprecated",
                    org.name, callable
                );
                vm.engine_deprecated(msg)?;
            }
        }
        (PhpStr::from_bytes(&bytes[class_len + 2..]), calling)
    } else if let Some(org) = &ce_org {
        fcc.calling_scope = Some(org.clone());
        (callable.clone(), org.clone())
    } else {
        return Ok(Err(format!("function \"{callable}\" not found or invalid function name")));
    };

    let lm = mname.to_ascii_lowercase();
    let calling = fcc.calling_scope.clone();
    let mut call_via_handler = false;
    let mut retval = false;
    let mut found = false;
    if strict_class && calling.is_some() && lm.as_bytes() == b"__construct" {
        fcc.func = calling.as_ref().and_then(|c| c.magic.constructor.clone());
        retval = fcc.func.is_some();
        found = true;
    } else if let Some(m) = ftable.methods.get(&lm).cloned() {
        retval = true;
        found = true;
        let mut m = m;
        if m.flags & member_flags::CHANGED != 0 && !strict_class {
            if let Some(scope) = frame.scope() {
                let decl_in_scope = m
                    .scope
                    .as_ref()
                    .and_then(|s| vm.find_class(s))
                    .is_some_and(|d| d.instance_of(&scope));
                if decl_in_scope {
                    if let Some(p) = scope.methods.get(&lm) {
                        if p.flags & member_flags::PRIVATE != 0
                            && p.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&scope.name))
                        {
                            m = p.clone();
                        }
                    }
                }
            }
        }
        fcc.func = Some(m.clone());
        if m.flags & member_flags::PUBLIC == 0 {
            if let Some(c) = &calling {
                let magic = if fcc.object.is_some() { c.magic.call.is_some() } else { c.magic.call_static.is_some() };
                if magic && !method_accessible(vm, &m, frame.scope().as_ref()) {
                    retval = false;
                    fcc.func = None;
                    found = false;
                }
            }
        }
    }
    if !found {
        // get_function_via_handler
        let same_as_org = match (&calling, &ce_org) {
            (Some(a), Some(b)) => same(a, b),
            _ => false,
        };
        if fcc.object.is_some() && same_as_org {
            let org = ce_org.clone().unwrap();
            if strict_class && org.magic.call.is_some() {
                fcc.func = org.magic.call.clone();
                fcc.magic = Some(mname.clone());
                call_via_handler = true;
                retval = true;
            } else {
                // zend_std_get_method (visibility from the frame's scope;
                // inaccessible or missing methods go through __call).
                let obj = fcc.object.clone().unwrap();
                let oc = obj.class().clone();
                if oc.methods.contains_key(&lm) || oc.magic.call.is_some() {
                    let t = super::call::method_target(vm, frame.scope().as_ref(), &obj, &mname)?;
                    let ok = !strict_class
                        || t.func.scope.as_ref().and_then(|s| vm.find_class(s)).is_some_and(|s| org.instance_of(&s));
                    if ok {
                        call_via_handler = t.magic_name.is_some();
                        fcc.func = Some(t.func);
                        fcc.magic = t.magic_name;
                        retval = true;
                    }
                }
            }
        } else if let Some(c) = &calling {
            // zend_std_get_static_method
            let m = c.methods.get(&lm).cloned();
            let resolved = match m {
                Some(m) => {
                    if m.flags & member_flags::PUBLIC == 0 && !method_accessible(vm, &m, frame.scope().as_ref()) {
                        if static_method_fallback(frame, c, &mname, fcc) {
                            true
                        } else {
                            let scope = frame.scope();
                            let what = match &scope {
                                Some(s) => format!("scope {}", super::decl::display_class_name(&s.name)),
                                None => "global scope".to_string(),
                            };
                            return Err(vm.throw_error(
                                "Error",
                                format!(
                                    "Call to {} method {}::{}() from {}",
                                    visibility_word(m.flags),
                                    super::decl::display_class_name(&c.name),
                                    m.name,
                                    what
                                ),
                            ));
                        }
                    } else {
                        fcc.func = Some(m);
                        true
                    }
                }
                None => static_method_fallback(frame, c, &mname, fcc),
            };
            if resolved {
                retval = true;
                call_via_handler = fcc.magic.is_some();
                if call_via_handler && fcc.object.is_none() {
                    if let Some(t) = frame.this() {
                        if t.class().instance_of(c) {
                            fcc.object = Some(t);
                        }
                    }
                }
            }
        }
    }

    let mut error = None;
    if retval {
        if let (Some(c), false) = (&calling, call_via_handler) {
            let f = fcc.func.clone().expect("resolved function");
            if f.has_flag(member_flags::ABSTRACT) {
                retval = false;
                error = Some(format!("cannot call abstract method {}::{}()", c.name, f.name));
            } else if fcc.object.is_none() && !f.is_static() {
                retval = false;
                error = Some(format!("non-static method {}::{}() cannot be called statically", c.name, f.name));
            }
            if retval && f.flags & member_flags::PUBLIC == 0 && !method_accessible(vm, &f, frame.scope().as_ref()) {
                error = Some(format!(
                    "cannot access {} method {}::{}()",
                    visibility_word(f.flags),
                    c.name,
                    f.name
                ));
                retval = false;
            }
        }
    } else {
        error = Some(match &calling {
            Some(c) => format!("class {} does not have a method \"{}\"", c.name, mname),
            None => format!("function {mname}() does not exist"),
        });
    }
    if let Some(o) = fcc.object.clone() {
        fcc.called_scope = Some(o.class().clone());
        if fcc.func.as_ref().is_some_and(|f| f.is_static()) && fcc.magic.is_none() {
            fcc.object = None;
        }
    }
    Ok(match error {
        Some(e) if !retval => Err(e),
        _ => Ok(()),
    })
}

/// `zend_get_callable_name_ex`.
pub fn callable_name(vm: &mut Vm, v: &Value) -> PhpStr {
    match &*v.deref() {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            if a.len() != 2 {
                return PhpStr::from("Array");
            }
            let (Some(t), Some(Value::String(m))) = (a.get_int(0).map(Value::unref), a.get_int(1).map(Value::unref)) else {
                return PhpStr::from("Array");
            };
            match t {
                Value::String(c) => PhpStr::from(format!("{c}::{m}")),
                Value::Object(o) => PhpStr::from(format!("{}::{m}", o.class().name)),
                _ => PhpStr::from("Array"),
            }
        }
        Value::Object(o) => match super::closure::closure_data(o) {
            Some(c) => {
                let name = c.magic_name.clone().unwrap_or_else(|| c.func.name.clone());
                match (&c.scope, c.from_callable) {
                    (Some(s), true) => PhpStr::from(format!("{}::{name}", s.name)),
                    _ => name,
                }
            }
            None => PhpStr::from(format!("{}::__invoke", o.class().name)),
        },
        other => vm.to_php_string(other).unwrap_or_default(),
    }
}

/// `zend_is_callable_at_frame` (`IS_CALLABLE_CHECK_SYNTAX_ONLY`).
pub fn is_callable_syntax(v: &Value) -> bool {
    match &*v.deref() {
        Value::String(_) => true,
        Value::Array(a) => {
            a.len() == 2
                && matches!(a.get_int(0).map(Value::unref), Some(Value::String(_)) | Some(Value::Object(_)))
                && matches!(a.get_int(1).map(Value::unref), Some(Value::String(_)))
        }
        Value::Object(o) => super::closure::closure_data(o).is_some() || o.class().find_method(b"__invoke").is_some(),
        _ => false,
    }
}

/// `zend_is_callable_ex`: resolves a callable at the innermost userland
/// frame. The inner `Err` is the text used in "must be a valid callback, ..."
/// messages; deprecations are emitted unless `suppress`.
pub fn is_callable_ex(vm: &mut Vm, v: &Value, suppress: bool) -> PResult<Result<CallableInfo, String>> {
    let frame = Frame { env: calling_env(vm) };
    let mut fcc = Fcc::default();
    let name = callable_name(vm, v);
    let r = match &*v.deref() {
        Value::String(s) => {
            let fname = s.strip_prefix(b"\\").unwrap_or(s);
            if let Some(f) = vm.find_function(fname) {
                return Ok(Ok(CallableInfo {
                    name,
                    target: Target {
                        func: f,
                        this: None,
                        called: None,
                        closure: None,
                        magic_name: None,
                    },
                }));
            }
            is_method_callable(vm, s, &frame, &mut fcc, false, suppress)?
        }
        Value::Array(a) => {
            if a.len() != 2 {
                return Ok(Err("array callback must have exactly two members".into()));
            }
            let (Some(obj), Some(method)) = (a.get_int(0).map(Value::unref), a.get_int(1).map(Value::unref)) else {
                return Ok(Err("array callback has to contain indices 0 and 1".into()));
            };
            if !matches!(obj, Value::String(_) | Value::Object(_)) {
                return Ok(Err("first array member is not a valid class name or object".into()));
            }
            let Value::String(method) = method else {
                return Ok(Err("second array member is not a valid method".into()));
            };
            let mut strict_class = false;
            match obj {
                Value::String(c) => {
                    let scope = frame.scope();
                    if let Err(e) = check_class(vm, &c, scope.as_ref(), &frame, &mut fcc, &mut strict_class, suppress)? {
                        return Ok(Err(e));
                    }
                }
                Value::Object(o) => {
                    fcc.calling_scope = Some(o.class().clone());
                    fcc.object = Some(o);
                }
                _ => unreachable!(),
            }
            is_method_callable(vm, &method, &frame, &mut fcc, strict_class, suppress)?
        }
        Value::Object(o) => {
            if let Some(c) = super::closure::closure_data(o) {
                let magic_name = c.magic_name.clone();
                return Ok(Ok(CallableInfo {
                    name,
                    target: Target {
                        func: c.func.clone(),
                        this: c.this.clone(),
                        called: c.called.clone(),
                        closure: Some(c),
                        magic_name,
                    },
                }));
            }
            match o.class().find_method(b"__invoke").cloned() {
                Some(m) => {
                    let this = if m.is_static() { None } else { Some(o.clone()) };
                    return Ok(Ok(CallableInfo {
                        name,
                        target: Target {
                            func: m,
                            this,
                            called: Some(o.class().clone()),
                            closure: None,
                            magic_name: None,
                        },
                    }));
                }
                None => return Ok(Err("no array or string given".into())),
            }
        }
        _ => return Ok(Err("no array or string given".into())),
    };
    // `[$closure, "__invoke"]`: the trampoline carries the closure's arginfo.
    if let (Some(f), Some(o)) = (&fcc.func, &fcc.object) {
        if super::closure::is_closure_invoke(f) {
            if let Some(c) = super::closure::closure_data(o) {
                fcc.func = Some(super::closure::invoke_method_for(&c));
            }
        }
    }
    Ok(r.map(|()| CallableInfo {
        name,
        target: Target {
            func: fcc.func.expect("resolved function"),
            this: fcc.object,
            called: fcc.called_scope,
            closure: None,
            magic_name: fcc.magic,
        },
    }))
}
