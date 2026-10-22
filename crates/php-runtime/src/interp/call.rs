//! Calls: argument evaluation (by-value/by-reference, spread, named),
//! parameter binding and type checks of userland functions, return type
//! checks, method and static calls, `new`, and callables.

use std::rc::Rc;

use crate::class::{ClassRef, Function, FunctionKind, fn_flags, member_flags};
use crate::compile::ir::*;
use crate::errors::level;
use crate::native::CallArgs;
use crate::object::ObjRef;
use crate::types::{ClassType, CoerceMode, TypeDecl, mask};
use crate::value::array::ArrayKey;
use crate::value::{PhpArray, PhpRef, PhpStr, Value};
use crate::vm::{Flow, Frame, PResult, Vm};

use super::closure::ClosureData;
use super::eval::{self, eval};
use super::{Ctx, Env, SHORT_CIRCUIT, UserFn, place, user_fn};

/// Arguments ready to bind.
#[derive(Default)]
pub struct Bound {
    pub positional: Vec<Value>,
    pub named: Vec<(PhpStr, Value)>,
}

/// Who is calling (for "called in %s on line %d" and strictness).
#[derive(Clone)]
pub struct Caller {
    pub strict: bool,
    /// `(file, line)` when the caller is userland code.
    pub site: Option<(PhpStr, u32)>,
}

impl Caller {
    pub fn internal() -> Caller {
        Caller {
            strict: false,
            site: None,
        }
    }
}

fn param_by_ref(func: &Function, i: usize) -> (bool, bool) {
    let sig = func.signature();
    let n = sig.num_args();
    let p = if i < n { sig.params.get(i) } else { sig.variadic() };
    match p {
        Some(p) => (p.by_ref, p.prefer_ref),
        None => (false, false),
    }
}

fn param_index_by_name(func: &Function, name: &[u8]) -> Option<usize> {
    func.signature().param_index(name)
}

fn cannot_pass_by_ref(vm: &mut Vm, func: &Function, i: usize) -> Flow {
    let sig = func.signature();
    let pname = sig
        .params
        .get(i)
        .filter(|p| !p.variadic || i >= sig.num_args())
        .map(|p| format!(" (${})", p.name))
        .unwrap_or_default();
    vm.throw_error(
        "Error",
        format!(
            "{}(): Argument #{}{} could not be passed by reference",
            func.display_name(),
            i + 1,
            pname
        ),
    )
}

/// Evaluates one argument for parameter position `i`.
fn eval_arg(vm: &mut Vm, cx: &Ctx, e: &Expr, func: Option<&Rc<Function>>, i: usize, named: Option<&[u8]>) -> PResult<Value> {
    let (by_ref, prefer) = match func {
        Some(f) => {
            let idx = match named {
                Some(n) => param_index_by_name(f, n).unwrap_or(usize::MAX),
                None => i,
            };
            if idx == usize::MAX {
                // Collected by a variadic parameter.
                match f.signature().variadic() {
                    Some(p) => (p.by_ref, p.prefer_ref),
                    None => (false, false),
                }
            } else {
                param_by_ref(f, idx)
            }
        }
        None => (false, false),
    };
    if !by_ref {
        return match e {
            Expr::Place(lv) => Ok(place::read_place(vm, cx, lv, false)?.into_unref()),
            _ => Ok(eval(vm, cx, e)?.into_unref()),
        };
    }
    match e {
        Expr::Var(cv, _) => Ok(Value::Ref(cx.env.make_ref(cx.slot(*cv)))),
        Expr::Place(lv) => match &**lv {
            LVal::Temp(t) => {
                let v = eval(vm, cx, t)?;
                match v {
                    Value::Ref(r) => Ok(Value::Ref(r)),
                    v => {
                        if !prefer {
                            vm.engine_notice("Only variables should be passed by reference")?;
                        }
                        Ok(Value::Ref(PhpRef::new(v)))
                    }
                }
            }
            lv => Ok(Value::Ref(place::make_ref(vm, cx, lv)?)),
        },
        Expr::Call(_) | Expr::MethodCall(_) | Expr::StaticCall(_) => {
            let v = eval(vm, cx, e)?;
            match v {
                Value::Ref(r) => Ok(Value::Ref(r)),
                v => {
                    if !prefer {
                        vm.engine_notice("Only variables should be passed by reference")?;
                    }
                    Ok(Value::Ref(PhpRef::new(v)))
                }
            }
        }
        _ => {
            if prefer {
                let v = eval(vm, cx, e)?;
                return Ok(Value::Ref(PhpRef::new(v.into_unref())));
            }
            let f = func.expect("by-ref implies a known function");
            let idx = match named {
                Some(n) => param_index_by_name(f, n).unwrap_or(i),
                None => i,
            };
            Err(cannot_pass_by_ref(vm, f, idx))
        }
    }
}

/// Evaluates an argument list against `func` (by-ref parameters get references).
pub fn eval_args(vm: &mut Vm, cx: &Ctx, args: &ArgList, func: Option<&Rc<Function>>) -> PResult<Bound> {
    let mut b = Bound {
        positional: Vec::with_capacity(args.args.len()),
        named: Vec::new(),
    };
    for a in &args.args {
        match a {
            Arg::Pos(e) => {
                let i = b.positional.len();
                let v = eval_arg(vm, cx, e, func, i, None)?;
                b.positional.push(v);
            }
            Arg::Named(n, e) => {
                let v = eval_arg(vm, cx, e, func, 0, Some(n))?;
                b.named.push((n.clone(), v));
            }
            Arg::Spread(e) => {
                let v = eval(vm, cx, e)?;
                spread_into(vm, &mut b, v)?;
            }
        }
    }
    Ok(b)
}

/// `...$args` in a call.
fn spread_into(vm: &mut Vm, b: &mut Bound, v: Value) -> PResult<()> {
    match v.into_unref() {
        Value::Array(a) => {
            for (k, v) in a.iter() {
                match k {
                    ArrayKey::Int(_) => {
                        if !b.named.is_empty() {
                            return Err(vm.throw_error(
                                "Error",
                                "Cannot use positional argument after named argument during unpacking",
                            ));
                        }
                        b.positional.push(v.unref());
                    }
                    ArrayKey::Str(s) => b.named.push((s, v.unref())),
                }
            }
            Ok(())
        }
        Value::Object(o) if o.class().instance_of_name(b"Traversable") => {
            let mut err: Option<Flow> = None;
            super::exec::iterate_object(vm, &o, false, &mut |vm, k, v| {
                match k {
                    Value::String(s) => b.named.push((s, v.unref())),
                    Value::Int(_) => {
                        if !b.named.is_empty() {
                            err = Some(vm.throw_error(
                                "Error",
                                "Cannot use positional argument after named argument during unpacking",
                            ));
                            return Ok(false);
                        }
                        b.positional.push(v.unref());
                    }
                    other => {
                        err = Some(vm.throw_error(
                            "Error",
                            format!("Keys must be of type int|string during argument unpacking, {} given", other.type_name()),
                        ));
                        return Ok(false);
                    }
                }
                Ok(true)
            })?;
            match err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        other => Err(vm.throw_type_error(format!(
            "Only arrays and Traversables can be unpacked, {} given",
            other.value_name()
        ))),
    }
}

fn caller_of(vm: &Vm, cx: &Ctx) -> Caller {
    Caller {
        strict: cx.strict(),
        site: Some((vm.current_file.clone(), vm.current_line)),
    }
}

/// Resolves a named function (`ZEND_INIT_FCALL_BY_NAME` / `_NS_`).
fn resolve_function(vm: &mut Vm, nc: &NamedCallee) -> PResult<Rc<Function>> {
    if let Some(f) = nc.cache.get() {
        return Ok(f.clone());
    }
    if let Some(f) = vm.functions.get(&nc.lc).cloned() {
        let _ = nc.cache.set(f.clone());
        return Ok(f);
    }
    if let Some((_, flc)) = &nc.fallback {
        if let Some(f) = vm.functions.get(flc).cloned() {
            let _ = nc.cache.set(f.clone());
            return Ok(f);
        }
    }
    let name = match &nc.fallback {
        Some((n, _)) if !nc.name.contains(&b'\\') => n.clone(),
        _ => nc.name.clone(),
    };
    Err(vm.throw_error("Error", format!("Call to undefined function {name}()")))
}

/// `f(...)`.
pub fn eval_call(vm: &mut Vm, cx: &Ctx, c: &CallExpr) -> PResult<Value> {
    match &c.callee {
        Callee::Named(nc) => {
            vm.current_line = c.line;
            let f = resolve_function(vm, nc)?;
            let b = eval_args(vm, cx, &c.args, Some(&f))?;
            vm.current_line = c.line;
            let caller = caller_of(vm, cx);
            invoke(vm, &f, b, None, None, None, &caller, false)
        }
        Callee::Expr(e) => {
            let callee = eval(vm, cx, e)?;
            vm.current_line = c.line;
            let target = resolve_callable_for_call(vm, cx, &callee)?;
            let b = eval_args(vm, cx, &c.args, Some(&target.func))?;
            vm.current_line = c.line;
            let caller = caller_of(vm, cx);
            invoke(
                vm,
                &target.func,
                b,
                target.this,
                target.called,
                target.closure.as_ref(),
                &caller,
                true,
            )
        }
    }
}

/// A resolved callable.
pub struct Target {
    pub func: Rc<Function>,
    pub this: Option<ObjRef>,
    pub called: Option<ClassRef>,
    pub closure: Option<Rc<ClosureData>>,
    /// `__call`/`__callStatic` trampoline: the original method name.
    pub magic_name: Option<PhpStr>,
}

/// `$f(...)` target resolution (`ZEND_INIT_DYNAMIC_CALL`).
fn resolve_callable_for_call(vm: &mut Vm, cx: &Ctx, callee: &Value) -> PResult<Target> {
    match &*callee.deref() {
        Value::String(s) => {
            if let Some(pos) = s.windows(2).position(|w| w == b"::") {
                let cls = PhpStr::from_bytes(&s[..pos]);
                let m = PhpStr::from_bytes(&s[pos + 2..]);
                let class = eval::lookup_class_or_error(vm, &cls, vm.current_line)?;
                return static_method_target(vm, cx.env, &class, &m, false);
            }
            let name = s.strip_prefix(b"\\").unwrap_or(s);
            match vm.find_function(name) {
                Some(f) => Ok(Target {
                    func: f,
                    this: None,
                    called: None,
                    closure: None,
                    magic_name: None,
                }),
                None => Err(vm.throw_error(
                    "Error",
                    format!("Call to undefined function {}()", String::from_utf8_lossy(name)),
                )),
            }
        }
        Value::Array(a) => {
            if a.len() != 2 {
                return Err(vm.throw_error("Error", "Array callback must have exactly two elements"));
            }
            let (Some(t), Some(m)) = (a.get_int(0).map(Value::unref), a.get_int(1).map(Value::unref)) else {
                return Err(vm.throw_error("Error", "Array callback has to contain indices 0 and 1"));
            };
            let Value::String(m) = m else {
                return Err(vm.throw_error("Error", "Second array member is not a valid method"));
            };
            match t {
                Value::Object(o) => method_target(vm, cx.scope(), &o, &m),
                Value::String(c) => {
                    let class = eval::lookup_class_or_error(vm, &c, vm.current_line)?;
                    static_method_target(vm, cx.env, &class, &m, false)
                }
                _ => Err(vm.throw_error("Error", "First array member is not a valid class name or object")),
            }
        }
        Value::Object(o) => {
            if let Some(c) = super::closure::closure_data(o) {
                return Ok(Target {
                    func: c.func.clone(),
                    this: c.this.clone(),
                    called: c.called.clone(),
                    closure: Some(c),
                    magic_name: None,
                });
            }
            match o.class().find_method(b"__invoke").cloned() {
                Some(m) => Ok(Target {
                    func: m,
                    this: Some(o.clone()),
                    called: Some(o.class().clone()),
                    closure: None,
                    magic_name: None,
                }),
                None => Err(vm.throw_error(
                    "Error",
                    format!("Object of type {} is not callable", o.class().name),
                )),
            }
        }
        _ => Err(vm.throw_error("Error", "Value not callable")),
    }
}

/// Invokes a function with bound arguments.
#[allow(clippy::too_many_arguments)]
pub fn invoke(
    vm: &mut Vm,
    func: &Rc<Function>,
    b: Bound,
    this: Option<ObjRef>,
    called: Option<ClassRef>,
    closure: Option<&Rc<ClosureData>>,
    caller: &Caller,
    dynamic: bool,
) -> PResult<Value> {
    match &func.kind {
        FunctionKind::Native(_) => {
            let r = vm.invoke_native(
                func,
                CallArgs {
                    positional: b.positional,
                    named: b.named,
                    this,
                    scope: called,
                    strict: caller.strict,
                    dynamic,
                },
            );
            r.map(Value::into_unref)
        }
        FunctionKind::User(_) => {
            let ufn = user_fn(func).expect("user function payload");
            call_user(vm, func, ufn, b, this, called, closure, caller)
        }
    }
}

/// The declaring class of a userland function.
fn function_scope(vm: &Vm, func: &Function, ufn: &UserFn) -> Option<ClassRef> {
    if let Some(c) = ufn.scope_class() {
        return Some(c);
    }
    func.scope.as_ref().and_then(|s| vm.find_class(s))
}

/// Calls a userland function (`i_init_func_execute_data` + RECV ops + body).
#[allow(clippy::too_many_arguments)]
pub fn call_user(
    vm: &mut Vm,
    func: &Rc<Function>,
    ufn: &UserFn,
    b: Bound,
    this: Option<ObjRef>,
    called: Option<ClassRef>,
    closure: Option<&Rc<ClosureData>>,
    caller: &Caller,
) -> PResult<Value> {
    super::check_stack(vm)?;
    let code = ufn.code.clone();
    let scope = match closure {
        Some(c) => c.scope.clone(),
        None => function_scope(vm, func, ufn),
    };
    let called = match called {
        Some(c) => Some(c),
        None => match &this {
            Some(o) => Some(o.class().clone()),
            None => scope.clone(),
        },
    };
    let this = if func.is_static() { None } else { this };
    let nparams = code.params.iter().filter(|p| !p.variadic).count();
    let mut env = Env::new(code.cvs.clone());
    env.this = this.clone();
    env.scope = scope.clone();
    env.called = called.clone();
    env.func = Some(func.clone());
    env.num_params = nparams as u32;
    env.strict = code.strict();

    // Place arguments (named-argument errors are raised in the caller's frame).
    let Bound { positional, named } = b;
    let mut variadic: Option<PhpArray> = code.params.last().filter(|p| p.variadic).map(|_| PhpArray::new());
    let mut extra = Vec::new();
    let mut num_args = positional.len();
    {
        let mut vars = env.vars.borrow_mut();
        for (i, v) in positional.into_iter().enumerate() {
            if i < nparams {
                let p = &code.params[i];
                vars[p.slot as usize] = if p.by_ref { v } else { v.into_unref() };
            } else if let Some(va) = &mut variadic {
                let by_ref = code.params.last().is_some_and(|p| p.by_ref);
                let _ = va.append(if by_ref { v } else { v.into_unref() });
            } else {
                extra.push(v);
            }
        }
    }
    for (name, v) in named {
        match code.params.iter().position(|p| !p.variadic && p.name == name) {
            Some(i) => {
                let slot = code.params[i].slot as usize;
                let taken = !env.vars.borrow()[slot].is_undef() || i < num_args.min(nparams);
                if taken {
                    return Err(vm.throw_error(
                        "Error",
                        format!("Named parameter ${name} overwrites previous argument"),
                    ));
                }
                let by_ref = code.params[i].by_ref;
                env.vars.borrow_mut()[slot] = if by_ref { v } else { v.into_unref() };
                num_args = num_args.max(i + 1);
            }
            None => match &mut variadic {
                Some(va) => {
                    if va.get_str(&name).is_some() {
                        return Err(vm.throw_error(
                            "Error",
                            format!("Named parameter ${name} overwrites previous argument"),
                        ));
                    }
                    va.insert(ArrayKey::Str(name), v.into_unref());
                }
                None => {
                    return Err(vm.throw_error("Error", format!("Unknown named parameter ${name}")));
                }
            },
        }
    }
    if num_args <= nparams {
        // `num_args` counts placed slots; extra args are counted separately.
    }
    env.num_args.set((num_args + extra.len()) as u32);
    *env.extra_args.borrow_mut() = extra;
    let env = Rc::new(env);

    // Push the frame.
    let call_site = if vm.in_native_frame() { None } else { caller.site.clone() };
    let call_type = match (&this, &scope) {
        (Some(_), _) => Some("->"),
        (None, Some(_)) => Some("::"),
        _ => None,
    };
    let frame = Frame {
        function: Some(func.clone()),
        name: func.name.clone(),
        class: if func.scope.is_some() { scope.as_ref().map(|s| s.name.clone()).or_else(|| func.scope.clone()) } else { None },
        call_type: if func.scope.is_some() { call_type } else { None },
        this: this.clone(),
        call_site,
        args: Vec::new(),
        is_internal: false,
        env: Some(env.clone()),
    };
    let saved_file = std::mem::replace(&mut vm.current_file, code.file.clone());
    let saved_line = vm.current_line;
    vm.push_frame(frame);
    vm.interp.depth += 1;

    let r = run_user_body(vm, func, ufn, &code, &env, num_args, variadic, closure, caller);

    vm.interp.depth -= 1;
    vm.pop_frame();
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    r
}

#[allow(clippy::too_many_arguments)]
fn run_user_body(
    vm: &mut Vm,
    func: &Rc<Function>,
    ufn: &UserFn,
    code: &Rc<FuncCode>,
    env: &Rc<Env>,
    num_args: usize,
    variadic: Option<PhpArray>,
    closure: Option<&Rc<ClosureData>>,
    caller: &Caller,
) -> PResult<Value> {
    let cx = Ctx {
        env,
        code,
        ufn,
        map: None,
        goto: std::cell::Cell::new(None),
        generator: None,
    };
    if func.has_flag(fn_flags::DEPRECATED) {
        vm.current_line = code.start_line;
        super::decl::emit_user_deprecation(vm, func)?;
    }
    // RECV / RECV_INIT / RECV_VARIADIC.
    let required = code
        .params
        .iter()
        .rposition(|p| !p.variadic && p.default.is_none())
        .map_or(0, |i| i + 1);
    for (i, p) in code.params.iter().enumerate() {
        vm.current_line = p.line.max(code.start_line);
        if p.variadic {
            let mut arr = variadic.clone().unwrap_or_default();
            if let Some(ty) = &p.ty {
                let keys: Vec<ArrayKey> = arr.keys().collect();
                for (j, k) in keys.into_iter().enumerate() {
                    let v = arr.get(&k).cloned().unwrap_or_default();
                    let c = check_param_type(vm, func, code, env, i + j, p, ty, v, caller)?;
                    if let Some(slot) = arr.get_mut(&k) {
                        *slot = c;
                    }
                }
            }
            env.vars.borrow_mut()[p.slot as usize] = Value::array(arr);
            continue;
        }
        let cur = env.vars.borrow()[p.slot as usize].clone();
        if cur.is_undef() {
            match &p.default {
                Some(d) => {
                    if i < num_args {
                        // A hole left by named arguments: evaluated like an unpassed default.
                    }
                    let v = match &p.default_const {
                        Some(v) => v.clone(),
                        None => {
                            let v = eval(vm, &cx, d)?;
                            if let Some(ty) = &p.ty {
                                check_param_type(vm, func, code, env, i, p, ty, v.clone(), caller)?
                            } else {
                                v
                            }
                        }
                    };
                    let v = if p.by_ref { Value::Ref(PhpRef::new(v)) } else { v };
                    env.vars.borrow_mut()[p.slot as usize] = v;
                }
                None => {
                    if i < num_args {
                        return Err(vm.throw_error(
                            "ArgumentCountError",
                            format!("{}(): Argument #{} (${}) not passed", func.display_name(), i + 1, p.name),
                        ));
                    }
                    let passed = env.num_args.get();
                    let what = if required == code.params.iter().filter(|p| !p.variadic).count() && code.params.last().is_none_or(|p| !p.variadic) {
                        "exactly"
                    } else {
                        "at least"
                    };
                    let msg = match &caller.site {
                        Some((file, line)) => format!(
                            "Too few arguments to function {}(), {} passed in {} on line {} and {} {} expected",
                            func.display_name(),
                            passed,
                            file,
                            line,
                            what,
                            required
                        ),
                        None => format!(
                            "Too few arguments to function {}(), {} passed and {} {} expected",
                            func.display_name(),
                            passed,
                            what,
                            required
                        ),
                    };
                    return Err(vm.throw_error("ArgumentCountError", msg));
                }
            }
        } else if let Some(ty) = &p.ty {
            let v = cur.unref();
            let c = check_param_type(vm, func, code, env, i, p, ty, v.clone(), caller)?;
            if !c.identical(&v) || matches!(c, Value::Float(_)) && matches!(v, Value::Int(_)) {
                let r = match &env.vars.borrow()[p.slot as usize] {
                    Value::Ref(r) => Some(r.clone()),
                    _ => None,
                };
                match r {
                    Some(r) => {
                        r.set(c);
                    }
                    None => env.vars.borrow_mut()[p.slot as usize] = c,
                }
            }
        }
    }
    // Constructor promotion.
    if code.params.iter().any(|p| p.promote.is_some()) {
        if let Some(this) = &env.this {
            for p in &code.params {
                if p.promote.is_none() {
                    continue;
                }
                vm.current_line = p.line;
                let v = env.vars.borrow()[p.slot as usize].clone();
                if p.by_ref {
                    let r = env.make_ref(p.slot);
                    super::objects::assign_prop_ref(vm, env.scope.as_ref(), this, &p.name, r)?;
                } else {
                    super::objects::write_prop(vm, env.scope.as_ref(), this, &p.name, v.into_unref(), code.strict())?;
                }
            }
        }
    }
    // Closure bindings (`use`) and static closure vars.
    if let Some(c) = closure {
        let mut vars = env.vars.borrow_mut();
        for (slot, v) in &c.bound {
            vars[*slot as usize] = v.clone();
        }
    }
    if code.is_generator() {
        return super::generator::create(vm, func.clone(), env.clone(), closure.cloned());
    }
    let r = super::exec::exec_block(vm, &cx, &code.body);
    let v = match r {
        Ok(()) => {
            vm.current_line = code.end_line;
            if let Some(rt) = &code.return_type {
                if rt.decl.mask & mask::NEVER != 0 {
                    return Err(vm.throw_type_error(format!(
                        "{}(): never-returning {} must not implicitly return",
                        func.display_name(),
                        if func.scope.is_some() { "method" } else { "function" }
                    )));
                }
                if rt.decl.mask & mask::VOID == 0 {
                    verify_return(vm, func, code, env, Value::Null)?;
                }
            }
            Value::Null
        }
        Err(Flow::Return(v)) => {
            if let Some(rt) = &code.return_type {
                if rt.decl.mask & mask::VOID == 0 {
                    match v {
                        Value::Ref(r) => {
                            let inner = r.get();
                            let c = verify_return(vm, func, code, env, inner.clone())?;
                            if !c.identical(&inner) {
                                r.set(c);
                            }
                            Value::Ref(r)
                        }
                        v => verify_return(vm, func, code, env, v)?,
                    }
                } else {
                    v
                }
            } else {
                v
            }
        }
        Err(Flow::Break(super::GOTO)) => Value::Null,
        Err(e) => return Err(e),
    };
    if code.returns_ref() {
        return Ok(v);
    }
    Ok(v.into_unref())
}

/// `zend_verify_arg_type`: checks/coerces an argument value.
#[allow(clippy::too_many_arguments)]
fn check_param_type(
    vm: &mut Vm,
    func: &Function,
    code: &FuncCode,
    env: &Env,
    i: usize,
    p: &ParamInfo,
    ty: &TypeInfo,
    v: Value,
    caller: &Caller,
) -> PResult<Value> {
    let _ = code;
    if ty.decl.is_mixed() {
        return Ok(v);
    }
    if type_matches_in(vm, &ty.decl, &v, env.scope.as_ref(), env.called.as_ref())? {
        return Ok(v);
    }
    if !v.is_null() {
        if let Some(c) = vm.coerce_to_type(
            &ty.decl,
            v.clone(),
            CoerceMode {
                strict: caller.strict,
                silent: false,
            },
        )? {
            return Ok(c);
        }
    }
    let need = ty.decl.to_type_string_in(env.scope.as_deref(), None);
    let mut msg = format!(
        "{}(): Argument #{} (${}) must be of type {}, {} given",
        func.display_name(),
        i + 1,
        p.name,
        need,
        v.value_name()
    );
    if let Some((file, line)) = &caller.site {
        msg.push_str(&format!(", called in {file} on line {line}"));
    }
    Err(vm.throw_type_error(msg))
}

/// `zend_verify_return_type`.
fn verify_return(vm: &mut Vm, func: &Function, code: &FuncCode, env: &Env, v: Value) -> PResult<Value> {
    let Some(rt) = &code.return_type else { return Ok(v) };
    if rt.decl.is_mixed() {
        return Ok(v);
    }
    if type_matches_in(vm, &rt.decl, &v, env.scope.as_ref(), env.called.as_ref())? {
        return Ok(v);
    }
    if !v.is_null() {
        if let Some(c) = vm.coerce_to_type(
            &rt.decl,
            v.clone(),
            CoerceMode {
                strict: code.strict(),
                silent: false,
            },
        )? {
            return Ok(c);
        }
    }
    let need = rt.decl.to_type_string_in(env.scope.as_deref(), None);
    Err(vm.throw_type_error(format!(
        "{}(): Return value must be of type {}, {} returned",
        func.display_name(),
        need,
        v.value_name()
    )))
}

/// Exact type match including class types resolved against `scope`.
pub fn type_matches(vm: &mut Vm, ty: &TypeDecl, v: &Value, scope: Option<&ClassRef>) -> PResult<bool> {
    type_matches_in(vm, ty, v, scope, scope)
}

pub fn type_matches_in(vm: &mut Vm, ty: &TypeDecl, v: &Value, scope: Option<&ClassRef>, called: Option<&ClassRef>) -> PResult<bool> {
    let m = ty.mask;
    match &*v.deref() {
        Value::Object(o) => {
            if m & mask::OBJECT != 0 {
                return Ok(true);
            }
            if m & mask::STATIC != 0 {
                if let Some(c) = called.or(scope) {
                    if o.class().instance_of(c) {
                        return Ok(true);
                    }
                }
            }
            for c in &ty.classes {
                let ok = match c {
                    ClassType::Name(n) => class_type_matches(vm, o, n, scope),
                    ClassType::Intersection(ns) => ns.iter().all(|n| class_type_matches(vm, o, n, scope)),
                };
                if ok {
                    return Ok(true);
                }
            }
            if m & mask::CALLABLE != 0 && check_callable(vm, v, false).is_ok() {
                return Ok(true);
            }
            Ok(false)
        }
        other => {
            if ty.matches_exact(other) {
                return Ok(true);
            }
            if m & mask::CALLABLE != 0 && matches!(other, Value::String(_) | Value::Array(_)) {
                return Ok(check_callable(vm, other, false).is_ok());
            }
            Ok(false)
        }
    }
}

fn class_type_matches(vm: &Vm, o: &ObjRef, name: &[u8], scope: Option<&ClassRef>) -> bool {
    if name.eq_ignore_ascii_case(b"self") {
        return scope.is_some_and(|s| o.class().instance_of(s));
    }
    if name.eq_ignore_ascii_case(b"parent") {
        return scope.and_then(|s| s.parent.as_ref()).is_some_and(|p| o.class().instance_of(p));
    }
    if o.class().instance_of_name(name) {
        return true;
    }
    // Class aliases.
    match vm.find_class(name) {
        Some(c) => o.class().instance_of(&c),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn scope_desc(scope: Option<&ClassRef>) -> String {
    match scope {
        Some(s) => format!("scope {}", super::decl::display_class_name(&s.name)),
        None => "global scope".to_string(),
    }
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

/// Root class of a method for protected checks (`zend_get_function_root_class`).
fn method_root_class(m: &Function, vm: &Vm) -> PhpStr {
    let decl = m.scope.clone().unwrap_or_default();
    let mut root = decl.clone();
    let mut c = vm.find_class(&decl).and_then(|c| c.parent.clone());
    let lc = m.name.to_ascii_lowercase();
    while let Some(ce) = c {
        match ce.methods.get(&lc) {
            Some(pm) if pm.flags & member_flags::PRIVATE == 0 => {
                root = pm.scope.clone().unwrap_or(root);
            }
            _ => break,
        }
        c = ce.parent.clone();
    }
    root
}

/// `zend_std_get_method`: finds an instance method callable from `scope`.
pub fn method_target(vm: &mut Vm, scope: Option<&ClassRef>, obj: &ObjRef, name: &PhpStr) -> PResult<Target> {
    let class = obj.class().clone();
    let lc = name.to_ascii_lowercase();
    if let Some(m) = class.methods.get(&lc).cloned() {
        let flags = m.flags;
        let decl_same = scope.is_some_and(|s| m.scope.as_ref().is_some_and(|ms| ms.eq_ignore_ascii_case(&s.name)));
        if flags & (member_flags::PRIVATE | member_flags::PROTECTED | member_flags::CHANGED) != 0 && !decl_same {
            // A private method of the calling scope takes precedence.
            if let Some(s) = scope {
                if class.instance_of(s) {
                    if let Some(pm) = s.methods.get(&lc) {
                        if pm.flags & member_flags::PRIVATE != 0 && pm.scope.as_ref().is_some_and(|ps| ps.eq_ignore_ascii_case(&s.name)) {
                            return Ok(Target {
                                func: pm.clone(),
                                this: Some(obj.clone()),
                                called: Some(class.clone()),
                                closure: None,
                                magic_name: None,
                            });
                        }
                    }
                }
            }
            let denied = if flags & member_flags::PRIVATE != 0 {
                true
            } else if flags & member_flags::PROTECTED != 0 {
                let root = method_root_class(&m, vm);
                !super::objects::protected_ok(&root, scope, vm)
            } else {
                false
            };
            if denied {
                if class.magic.call.is_some() {
                    return Ok(magic_call_target(&class, obj, name));
                }
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Call to {} method {}::{}() from {}",
                        visibility_word(flags),
                        super::decl::display_class_name(&class.name),
                        m.name,
                        scope_desc(scope)
                    ),
                ));
            }
        }
        return Ok(Target {
            func: m,
            this: Some(obj.clone()),
            called: Some(class.clone()),
            closure: None,
            magic_name: None,
        });
    }
    // Closures: `$closure->__invoke()` and `call`/`bindTo` are real methods.
    if class.magic.call.is_some() {
        return Ok(magic_call_target(&class, obj, name));
    }
    Err(vm.throw_error(
        "Error",
        format!(
            "Call to undefined method {}::{}()",
            super::decl::display_class_name(&class.name),
            name
        ),
    ))
}

fn magic_call_target(class: &ClassRef, obj: &ObjRef, name: &PhpStr) -> Target {
    Target {
        func: class.magic.call.clone().expect("__call"),
        this: Some(obj.clone()),
        called: Some(class.clone()),
        closure: None,
        magic_name: Some(name.clone()),
    }
}

/// `A::m()` resolution (`zend_std_get_static_method` + `$this` forwarding).
pub fn static_method_target(vm: &mut Vm, env: &Env, class: &ClassRef, name: &PhpStr, forwarding: bool) -> PResult<Target> {
    let scope = env.scope.as_ref();
    let lc = name.to_ascii_lowercase();
    let this_compat = env.this.as_ref().filter(|t| t.class().instance_of(class)).cloned();
    let found = class.methods.get(&lc).cloned();
    let m = match found {
        Some(m) => {
            let flags = m.flags;
            let decl_same = scope.is_some_and(|s| m.scope.as_ref().is_some_and(|ms| ms.eq_ignore_ascii_case(&s.name)));
            let mut m = m;
            if flags & (member_flags::PRIVATE | member_flags::PROTECTED | member_flags::CHANGED) != 0 && !decl_same {
                let mut alt = None;
                if let Some(s) = scope {
                    if class.instance_of(s) {
                        if let Some(pm) = s.methods.get(&lc) {
                            if pm.flags & member_flags::PRIVATE != 0 && pm.scope.as_ref().is_some_and(|ps| ps.eq_ignore_ascii_case(&s.name)) {
                                alt = Some(pm.clone());
                            }
                        }
                    }
                }
                match alt {
                    Some(a) => m = a,
                    None => {
                        let denied = if flags & member_flags::PRIVATE != 0 {
                            true
                        } else if flags & member_flags::PROTECTED != 0 {
                            let root = method_root_class(&m, vm);
                            !super::objects::protected_ok(&root, scope, vm)
                        } else {
                            false
                        };
                        if denied {
                            if let (Some(t), true) = (&this_compat, class.magic.call.is_some()) {
                                return Ok(magic_call_target(class, t, name));
                            }
                            if class.magic.call_static.is_some() {
                                return Ok(magic_static_target(class, name));
                            }
                            return Err(vm.throw_error(
                                "Error",
                                format!(
                                    "Call to {} method {}::{}() from {}",
                                    visibility_word(flags),
                                    super::decl::display_class_name(&class.name),
                                    m.name,
                                    scope_desc(scope)
                                ),
                            ));
                        }
                    }
                }
            }
            m
        }
        None => {
            if let (Some(t), true) = (&this_compat, class.magic.call.is_some()) {
                return Ok(magic_call_target(class, t, name));
            }
            if class.magic.call_static.is_some() {
                return Ok(magic_static_target(class, name));
            }
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Call to undefined method {}::{}()",
                    super::decl::display_class_name(&class.name),
                    name
                ),
            ));
        }
    };
    if m.has_flag(member_flags::ABSTRACT) {
        return Err(vm.throw_error(
            "Error",
            format!(
                "Cannot call abstract method {}::{}()",
                m.scope.clone().unwrap_or_default(),
                m.name
            ),
        ));
    }
    if m.is_static() {
        let called = if forwarding {
            env.called.clone().or_else(|| Some(class.clone()))
        } else {
            Some(class.clone())
        };
        return Ok(Target {
            func: m,
            this: None,
            called,
            closure: None,
            magic_name: None,
        });
    }
    match this_compat {
        Some(t) => {
            let called = Some(t.class().clone());
            Ok(Target {
                func: m,
                this: Some(t),
                called,
                closure: None,
                magic_name: None,
            })
        }
        None => Err(vm.throw_error(
            "Error",
            format!(
                "Non-static method {}::{}() cannot be called statically",
                super::decl::display_class_name(m.scope.as_deref().unwrap_or(&class.name)),
                m.name
            ),
        )),
    }
}

fn magic_static_target(class: &ClassRef, name: &PhpStr) -> Target {
    Target {
        func: class.magic.call_static.clone().expect("__callStatic"),
        this: None,
        called: Some(class.clone()),
        closure: None,
        magic_name: Some(name.clone()),
    }
}

/// Calls a resolved target (handles `__call`/`__callStatic` trampolines).
pub fn call_target(vm: &mut Vm, t: Target, b: Bound, caller: &Caller, dynamic: bool) -> PResult<Value> {
    if let Some(name) = t.magic_name {
        let mut arr = PhpArray::with_capacity(b.positional.len());
        for v in b.positional {
            let _ = arr.append(v.into_unref());
        }
        for (k, v) in b.named {
            arr.insert(ArrayKey::Str(k), v.into_unref());
        }
        let args = Bound {
            positional: vec![Value::String(name), Value::array(arr)],
            named: Vec::new(),
        };
        return invoke(vm, &t.func, args, t.this, t.called, None, caller, false);
    }
    invoke(vm, &t.func, b, t.this, t.called, t.closure.as_ref(), caller, dynamic)
}

/// `$obj->m(...)`.
pub fn eval_method_call(vm: &mut Vm, cx: &Ctx, m: &MethodCallExpr) -> PResult<Value> {
    let obj = eval(vm, cx, &m.object)?;
    if m.nullsafe && obj.is_null() {
        return Err(Flow::Break(SHORT_CIRCUIT));
    }
    let name = match &m.name {
        Member::Name(n) => n.clone(),
        Member::Expr(e) => {
            let v = eval(vm, cx, e)?;
            match v.into_unref() {
                Value::String(s) => s,
                Value::Object(o) => {
                    // `$obj->$closure()` is not a call of the closure.
                    let _ = o;
                    vm.current_line = m.line;
                    return Err(vm.throw_error("Error", "Method name must be a string"));
                }
                _ => {
                    vm.current_line = m.line;
                    return Err(vm.throw_error("Error", "Method name must be a string"));
                }
            }
        }
    };
    vm.current_line = m.line;
    let o = match obj.into_unref() {
        Value::Object(o) => o,
        other => {
            return Err(vm.throw_error(
                "Error",
                format!("Call to a member function {name}() on {}", other.value_name()),
            ));
        }
    };
    if let Some(h) = o.class().handlers.call_method {
        let b = eval_args(vm, cx, &m.args, None)?;
        vm.current_line = m.line;
        if let Some(v) = h(vm, &o, &name, &b.positional)? {
            return Ok(v);
        }
        let target = method_target(vm, cx.scope(), &o, &name)?;
        let caller = caller_of(vm, cx);
        return call_target(vm, target, b, &caller, false);
    }
    let target = method_target(vm, cx.scope(), &o, &name)?;
    let b = eval_args(vm, cx, &m.args, if target.magic_name.is_some() { None } else { Some(&target.func) })?;
    vm.current_line = m.line;
    let caller = caller_of(vm, cx);
    call_target(vm, target, b, &caller, false)
}

/// `A::m(...)`.
pub fn eval_static_call(vm: &mut Vm, cx: &Ctx, s: &StaticCallExpr) -> PResult<Value> {
    let class = eval::resolve_class(vm, cx, &s.class, s.line)?;
    let name = match &s.name {
        Member::Name(n) => n.clone(),
        Member::Expr(e) => {
            let v = eval(vm, cx, e)?;
            match v.into_unref() {
                Value::String(s) => s,
                _ => {
                    vm.current_line = s.line;
                    return Err(vm.throw_error("Error", "Method name must be a string"));
                }
            }
        }
    };
    vm.current_line = s.line;
    let forwarding = matches!(s.class, ClassSpec::Self_ | ClassSpec::Parent | ClassSpec::Static);
    let mut target = static_method_target(vm, cx.env, &class, &name, forwarding)?;
    if matches!(s.class, ClassSpec::Static) {
        if target.this.is_none() && target.func.is_static() {
            target.called = Some(class.clone());
        }
    }
    let b = eval_args(vm, cx, &s.args, if target.magic_name.is_some() { None } else { Some(&target.func) })?;
    vm.current_line = s.line;
    let caller = caller_of(vm, cx);
    call_target(vm, target, b, &caller, false)
}

/// `new A(...)`.
pub fn eval_new(vm: &mut Vm, cx: &Ctx, n: &NewExpr) -> PResult<Value> {
    let class = match &n.class {
        NewTarget::Class(cs) => eval::resolve_class(vm, cx, cs, n.line)?,
        NewTarget::Anonymous(def) => {
            vm.current_line = n.line;
            super::decl::declare_anon_class(vm, def)?
        }
    };
    vm.current_line = n.line;
    let obj = new_object(vm, &class)?;
    if let Some(ctor) = class.magic.constructor.clone() {
        check_ctor_access(vm, cx.scope(), &class, &ctor)?;
        let b = eval_args(vm, cx, &n.args, Some(&ctor))?;
        vm.current_line = n.line;
        let caller = caller_of(vm, cx);
        invoke(vm, &ctor, b, Some(obj.clone()), Some(class.clone()), None, &caller, false)?;
    }
    Ok(Value::Object(obj))
}

/// Instantiation checks + object creation (`object_init_ex`).
pub fn new_object(vm: &mut Vm, class: &ClassRef) -> PResult<ObjRef> {
    if class.is_interface() {
        return Err(vm.throw_error("Error", format!("Cannot instantiate interface {}", class.name)));
    }
    if class.is_trait() {
        return Err(vm.throw_error("Error", format!("Cannot instantiate trait {}", class.name)));
    }
    if class.is_enum() {
        return Err(vm.throw_error("Error", format!("Cannot instantiate enum {}", class.name)));
    }
    if class.is_abstract() {
        return Err(vm.throw_error("Error", format!("Cannot instantiate abstract class {}", class.name)));
    }
    if class.has_flag(crate::class::class_flags::NO_DYNAMIC_INSTANTIATION) {
        if class.lc_name.as_bytes() == b"generator" {
            return Err(vm.throw_error(
                "Error",
                "The \"Generator\" class is reserved for internal use and cannot be manually instantiated",
            ));
        }
        return Err(vm.throw_error(
            "Error",
            format!("Instantiation of class {} is not allowed", class.name),
        ));
    }
    super::objects::instantiate(vm, class)
}

fn check_ctor_access(vm: &mut Vm, scope: Option<&ClassRef>, class: &ClassRef, ctor: &Function) -> PResult<()> {
    let flags = ctor.flags;
    if flags & (member_flags::PRIVATE | member_flags::PROTECTED) == 0 {
        return Ok(());
    }
    let same = scope.is_some_and(|s| ctor.scope.as_ref().is_some_and(|c| c.eq_ignore_ascii_case(&s.name)));
    if same {
        return Ok(());
    }
    if flags & member_flags::PROTECTED != 0 {
        let root = method_root_class(ctor, vm);
        if super::objects::protected_ok(&root, scope, vm) {
            return Ok(());
        }
    }
    Err(vm.throw_error(
        "Error",
        format!(
            "Call to {} {}::{}() from {}",
            visibility_word(flags),
            super::decl::display_class_name(&class.name),
            ctor.name,
            scope_desc(scope)
        ),
    ))
}

/// First-class callable syntax (`f(...)`).
pub fn callable_convert(vm: &mut Vm, cx: &Ctx, c: &CallableConvert, line: u32) -> PResult<Value> {
    vm.current_line = line;
    let target = match c {
        CallableConvert::Func(Callee::Named(nc)) => {
            let f = resolve_function(vm, nc)?;
            Target {
                func: f,
                this: None,
                called: None,
                closure: None,
                magic_name: None,
            }
        }
        CallableConvert::Func(Callee::Expr(e)) => {
            let v = eval(vm, cx, e)?;
            vm.current_line = line;
            if let Value::Object(o) = &*v.deref() {
                if super::closure::closure_data(o).is_some() {
                    return Ok(Value::Object(o.clone()));
                }
            }
            resolve_callable_for_call(vm, cx, &v)?
        }
        CallableConvert::Method(obj, m, _) => {
            let o = eval(vm, cx, obj)?;
            let name = place::member_name(vm, cx, m)?;
            vm.current_line = line;
            let o = match o.into_unref() {
                Value::Object(o) => o,
                other => {
                    return Err(vm.throw_error(
                        "Error",
                        format!("Call to a member function {name}() on {}", other.value_name()),
                    ));
                }
            };
            method_target(vm, cx.scope(), &o, &name)?
        }
        CallableConvert::Static(cs, m) => {
            let class = eval::resolve_class(vm, cx, cs, line)?;
            let name = place::member_name(vm, cx, m)?;
            vm.current_line = line;
            let forwarding = matches!(cs, ClassSpec::Self_ | ClassSpec::Parent | ClassSpec::Static);
            static_method_target(vm, cx.env, &class, &name, forwarding)?
        }
    };
    super::closure::closure_from_target(vm, target)
}

// ---------------------------------------------------------------------------
// Callables from Rust
// ---------------------------------------------------------------------------

/// Result of `zend_is_callable_ex`.
pub struct CallableInfo {
    pub name: PhpStr,
    pub target: Target,
}

/// The class scope of the innermost userland frame (for callable visibility).
pub fn calling_scope(vm: &mut Vm) -> Option<ClassRef> {
    for f in vm.frames.iter().rev() {
        if let Some(env) = &f.env {
            return env.scope.clone();
        }
    }
    None
}

/// The environment of the innermost userland frame (or the global scope).
pub fn calling_env(vm: &mut Vm) -> Rc<Env> {
    for f in vm.frames.iter().rev() {
        if let Some(env) = &f.env {
            return env.clone();
        }
    }
    vm.global_env()
}

/// `zend_is_callable_ex`: resolves a callable value or returns the error text
/// used in "must be a valid callback, ..." messages.
pub fn check_callable(vm: &mut Vm, v: &Value, _syntax_only: bool) -> Result<CallableInfo, String> {
    let scope = calling_scope(vm);
    let env = calling_env(vm);
    match &*v.deref() {
        Value::String(s) => {
            if let Some(pos) = s.windows(2).position(|w| w == b"::") {
                let cls = &s[..pos];
                let m = PhpStr::from_bytes(&s[pos + 2..]);
                let class = match vm.lookup_class(cls) {
                    Ok(Some(c)) => c,
                    _ => return Err(format!("class \"{}\" not found", String::from_utf8_lossy(cls))),
                };
                return static_callable(vm, &env, scope.as_ref(), &class, &m, s.clone());
            }
            let name = s.strip_prefix(b"\\").unwrap_or(s);
            match vm.find_function(name) {
                Some(f) => Ok(CallableInfo {
                    name: f.name.clone(),
                    target: Target {
                        func: f,
                        this: None,
                        called: None,
                        closure: None,
                        magic_name: None,
                    },
                }),
                None => Err(format!("function \"{s}\" not found or invalid function name")),
            }
        }
        Value::Array(a) => {
            if a.len() != 2 {
                return Err("array callback must have exactly two members".into());
            }
            let (Some(t), Some(m)) = (a.get_int(0).map(Value::unref), a.get_int(1).map(Value::unref)) else {
                return Err("array callback must have exactly two members".into());
            };
            let Value::String(m) = m else {
                return Err("second array member is not a valid method".into());
            };
            match t {
                Value::Object(o) => {
                    let class = o.class().clone();
                    let (m2, cls_override) = split_parent_method(&m);
                    let lookup_class = match &cls_override {
                        Some(c) if c.eq_ignore_ascii_case(b"parent") => class.parent.clone().unwrap_or(class.clone()),
                        _ => class.clone(),
                    };
                    let lc = m2.to_ascii_lowercase();
                    match lookup_class.methods.get(&lc).cloned() {
                        Some(f) => {
                            if !method_visible(vm, &f, scope.as_ref()) {
                                if class.magic.call.is_some() {
                                    return Ok(CallableInfo {
                                        name: PhpStr::from(format!("{}::{}", class.name, m2)),
                                        target: magic_call_target(&class, &o, &m2),
                                    });
                                }
                                return Err(format!(
                                    "cannot access {} method {}::{}()",
                                    visibility_word(f.flags),
                                    class.name,
                                    f.name
                                ));
                            }
                            let this = if f.is_static() { None } else { Some(o.clone()) };
                            Ok(CallableInfo {
                                name: PhpStr::from(format!("{}::{}", class.name, f.name)),
                                target: Target {
                                    func: f,
                                    this,
                                    called: Some(class.clone()),
                                    closure: None,
                                    magic_name: None,
                                },
                            })
                        }
                        None => {
                            if class.magic.call.is_some() {
                                return Ok(CallableInfo {
                                    name: PhpStr::from(format!("{}::{}", class.name, m2)),
                                    target: magic_call_target(&class, &o, &m2),
                                });
                            }
                            Err(format!("class {} does not have a method \"{}\"", class.name, m2))
                        }
                    }
                }
                Value::String(c) => {
                    let class = match vm.lookup_class(&c) {
                        Ok(Some(c)) => c,
                        _ => return Err(format!("class \"{c}\" not found")),
                    };
                    let full = PhpStr::from(format!("{}::{}", class.name, m));
                    static_callable(vm, &env, scope.as_ref(), &class, &m, full)
                }
                _ => Err("first array member is not a valid class name or object".into()),
            }
        }
        Value::Object(o) => {
            if let Some(c) = super::closure::closure_data(o) {
                return Ok(CallableInfo {
                    name: PhpStr::from("Closure::__invoke"),
                    target: Target {
                        func: c.func.clone(),
                        this: c.this.clone(),
                        called: c.called.clone(),
                        closure: Some(c),
                        magic_name: None,
                    },
                });
            }
            match o.class().find_method(b"__invoke").cloned() {
                Some(m) => Ok(CallableInfo {
                    name: PhpStr::from(format!("{}::__invoke", o.class().name)),
                    target: Target {
                        func: m,
                        this: Some(o.clone()),
                        called: Some(o.class().clone()),
                        closure: None,
                        magic_name: None,
                    },
                }),
                None => Err("no array or string given".into()),
            }
        }
        _ => Err("no array or string given".into()),
    }
}

fn split_parent_method(m: &PhpStr) -> (PhpStr, Option<PhpStr>) {
    if let Some(pos) = m.windows(2).position(|w| w == b"::") {
        return (PhpStr::from_bytes(&m[pos + 2..]), Some(PhpStr::from_bytes(&m[..pos])));
    }
    (m.clone(), None)
}

fn method_visible(vm: &Vm, f: &Function, scope: Option<&ClassRef>) -> bool {
    if f.flags & member_flags::PRIVATE != 0 {
        return scope.is_some_and(|s| f.scope.as_ref().is_some_and(|c| c.eq_ignore_ascii_case(&s.name)));
    }
    if f.flags & member_flags::PROTECTED != 0 {
        let root = method_root_class(f, vm);
        return super::objects::protected_ok(&root, scope, vm);
    }
    true
}

fn static_callable(vm: &mut Vm, env: &Env, scope: Option<&ClassRef>, class: &ClassRef, m: &PhpStr, name: PhpStr) -> Result<CallableInfo, String> {
    let lc = m.to_ascii_lowercase();
    let this_compat = env.this.as_ref().filter(|t| t.class().instance_of(class)).cloned();
    match class.methods.get(&lc).cloned() {
        Some(f) => {
            if !method_visible(vm, &f, scope) {
                if class.magic.call_static.is_some() {
                    return Ok(CallableInfo {
                        name,
                        target: magic_static_target(class, m),
                    });
                }
                return Err(format!(
                    "cannot access {} method {}::{}()",
                    visibility_word(f.flags),
                    class.name,
                    f.name
                ));
            }
            if !f.is_static() {
                match this_compat {
                    Some(t) => {
                        let called = Some(t.class().clone());
                        return Ok(CallableInfo {
                            name,
                            target: Target {
                                func: f,
                                this: Some(t),
                                called,
                                closure: None,
                                magic_name: None,
                            },
                        });
                    }
                    None => {
                        return Err(format!(
                            "non-static method {}::{}() cannot be called statically",
                            class.name, f.name
                        ));
                    }
                }
            }
            Ok(CallableInfo {
                name,
                target: Target {
                    func: f,
                    this: None,
                    called: Some(class.clone()),
                    closure: None,
                    magic_name: None,
                },
            })
        }
        None => {
            if let (Some(t), true) = (&this_compat, class.magic.call.is_some()) {
                return Ok(CallableInfo {
                    name,
                    target: magic_call_target(class, t, m),
                });
            }
            if class.magic.call_static.is_some() {
                return Ok(CallableInfo {
                    name,
                    target: magic_static_target(class, m),
                });
            }
            Err(format!("class {} does not have a method \"{}\"", class.name, m))
        }
    }
}

/// Calls a callable value (`zend_call_function`): arguments that are
/// `Value::Ref` bind to by-reference parameters.
pub fn call_value(vm: &mut Vm, callable: &Value, positional: Vec<Value>, named: Vec<(PhpStr, Value)>, dynamic: bool) -> PResult<Value> {
    let info = match check_callable(vm, callable, false) {
        Ok(i) => i,
        Err(msg) => {
            // Mirror the runtime errors of a dynamic call.
            let _ = msg;
            let env = calling_env(vm);
            let cx_scope = env.scope.clone();
            let _ = cx_scope;
            return Err(match &*callable.deref() {
                Value::String(s) if !s.windows(2).any(|w| w == b"::") => {
                    vm.throw_error("Error", format!("Call to undefined function {s}()"))
                }
                _ => vm.throw_error("Error", format!("Invalid callback, {msg}")),
            });
        }
    };
    let caller = Caller::internal();
    // By-value arguments for by-reference parameters warn in dynamic calls.
    let t = info.target;
    let mut positional = positional;
    if t.magic_name.is_none() {
        let sig = t.func.signature().clone();
        for (i, v) in positional.iter_mut().enumerate() {
            let p = if i < sig.num_args() { sig.params.get(i) } else { sig.variadic() };
            if let Some(p) = p {
                if p.by_ref && !p.prefer_ref && !v.is_ref() {
                    if dynamic && t.func.is_native() {
                        // Native functions warn in ZPP (place_native_args).
                    } else if dynamic {
                        let mut msg = format!("{}(): Argument #{}", t.func.display_name(), i + 1);
                        if !p.variadic {
                            msg.push_str(&format!(" (${})", p.name));
                        }
                        msg.push_str(" must be passed by reference, value given");
                        vm.engine_warning(msg)?;
                    }
                    *v = Value::Ref(PhpRef::new(std::mem::take(v)));
                }
            }
        }
    }
    let b = Bound { positional, named };
    call_target(vm, t, b, &caller, dynamic)
}

/// Host entry: calls a function with `CallArgs` (native → user callbacks).
pub fn call_function_args(vm: &mut Vm, func: &Rc<Function>, call: CallArgs) -> PResult<Value> {
    let caller = Caller {
        strict: call.strict,
        site: None,
    };
    let b = Bound {
        positional: call.positional,
        named: call.named,
    };
    invoke(vm, func, b, call.this, call.scope, None, &caller, call.dynamic)
}

/// Host entry: `zend_call_method` (no visibility checks; `__call` fallback).
pub fn call_method_internal(vm: &mut Vm, obj: &ObjRef, name: &[u8], args: Vec<Value>) -> PResult<Value> {
    let class = obj.class().clone();
    let lc = name.to_ascii_lowercase();
    if let Some(c) = super::closure::closure_data(obj) {
        if lc.as_slice() == b"__invoke" {
            let b = Bound {
                positional: args,
                named: Vec::new(),
            };
            return invoke(vm, &c.func.clone(), b, c.this.clone(), c.called.clone(), Some(&c), &Caller::internal(), false);
        }
    }
    match class.methods.get(&lc[..]).cloned() {
        Some(m) => call_method_fn(vm, obj, &m, args, None),
        None => {
            if class.magic.call.is_some() {
                let t = magic_call_target(&class, obj, &PhpStr::from_bytes(name));
                let b = Bound {
                    positional: args,
                    named: Vec::new(),
                };
                return call_target(vm, t, b, &Caller::internal(), false);
            }
            Err(vm.throw_error(
                "Error",
                format!(
                    "Call to undefined method {}::{}()",
                    class.name,
                    String::from_utf8_lossy(name)
                ),
            ))
        }
    }
}

/// Calls method `m` on `obj` with positional arguments.
pub fn call_method_fn(vm: &mut Vm, obj: &ObjRef, m: &Rc<Function>, args: Vec<Value>, called: Option<ClassRef>) -> PResult<Value> {
    let b = Bound {
        positional: args,
        named: Vec::new(),
    };
    let this = if m.is_static() { None } else { Some(obj.clone()) };
    let called = called.or_else(|| Some(obj.class().clone()));
    invoke(vm, m, b, this, called, None, &Caller::internal(), false)
}

