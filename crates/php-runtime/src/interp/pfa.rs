//! Partial function application (`Zend/zend_partial.c`).
//!
//! `f($a, ?)` evaluates the callee and the bound arguments like a call, then
//! generates the code of an equivalent closure
//!
//! ```php
//! static function ($b) use ($a) { return \f($a, $b); }
//! ```
//!
//! (`zp_compile`), caches it at the call site, and binds the pre-bound
//! arguments as lexical variables (`zp_bind`). The generated code is built
//! directly as IR; it calls the original function through the ordinary call
//! nodes (`f()`, `$this->m()`, `static::m()`, `$fn()`), so argument passing,
//! by-reference parameters, defaults and diagnostics behave like a userland
//! closure written by hand.
//!
//! Also: the direct-call form of the pipe operator (`$x |> f(?)`).

use std::cell::OnceCell;
use std::rc::Rc;

use crate::class::{ClassRef, Function, FunctionKind, fn_flags, member_flags};
use crate::compile::ir::*;
use crate::native::DefaultValue;
use crate::object::ObjRef;
use crate::types::{CoerceMode, TypeDecl, mask};
use crate::value::array::ArrayKey;
use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{PResult, Vm};

use super::call::{self, Target};
use super::closure::{self, ClosureData};
use super::eval::{self, eval};
use super::{Ctx, UserFn, place, user_fn};

/// Evaluates a [`PfaExpr`].
pub fn eval_pfa(vm: &mut Vm, cx: &Ctx, p: &PfaExpr) -> PResult<Value> {
    match p {
        PfaExpr::Create(pe) => create(vm, cx, pe),
        PfaExpr::ParamDefault(code, idx) => {
            let p = &code.params[*idx as usize];
            match (&p.default_const, &p.default) {
                (Some(v), _) => Ok(v.clone()),
                (None, Some(d)) => eval(vm, cx, d),
                (None, None) => Ok(Value::Null),
            }
        }
        PfaExpr::PipeCall(operand, slot, callexpr) => {
            let v = eval(vm, cx, operand)?.into_unref();
            let s = cx.slot(*slot);
            cx.env.vars.borrow_mut()[s as usize] = v;
            let r = eval(vm, cx, callexpr);
            cx.env.vars.borrow_mut()[s as usize] = Value::Undef;
            r
        }
        PfaExpr::PipeArg(slot) => {
            let s = cx.slot(*slot);
            let v = std::mem::take(&mut cx.env.vars.borrow_mut()[s as usize]);
            Ok(if v.is_undef() { Value::Null } else { v })
        }
    }
}

// ---------------------------------------------------------------------------
// The partially applied function
// ---------------------------------------------------------------------------

/// Where the default value of a parameter comes from.
#[derive(Clone)]
enum PDefault {
    Required,
    Const(Value),
    /// A userland default expression, evaluated by the generated code.
    Expr(Rc<FuncCode>, u32),
    /// `= UNKNOWN` (internal) or magic arginfo.
    Unknown,
}

#[derive(Clone)]
struct PParam {
    name: PhpStr,
    ty: Option<TypeInfo>,
    /// `ZEND_ARG_SEND_MODE` (by-reference or prefer-reference).
    by_ref: bool,
    /// `ARG_MUST_BE_SENT_BY_REF`.
    must_ref: bool,
    default: PDefault,
    /// Inherited `#[\SensitiveParameter]`.
    attributes: Vec<Rc<AttrDef>>,
}

/// The view of the called function that `zend_partial.c` works with
/// (`zend_function.common`).
struct FnView {
    func: Rc<Function>,
    /// `get_function_or_method_name`.
    display: String,
    /// `function_name`.
    name: PhpStr,
    /// Declaring class (`common.scope`).
    scope: Option<ClassRef>,
    params: Vec<PParam>,
    variadic: Option<PParam>,
    required: usize,
    /// `ZEND_ACC_CLOSURE`.
    is_closure: bool,
    is_static: bool,
    /// `ZEND_ACC_CALL_VIA_TRAMPOLINE`.
    trampoline: bool,
    return_type: Option<TypeInfo>,
    returns_ref: bool,
    nodiscard: Option<Rc<AttrDef>>,
    /// Function used to decide by-reference passing of bound arguments.
    by_ref_func: Option<Rc<Function>>,
    /// Cache key (`function->op_array.opcodes` / the internal function).
    cache_key: Option<usize>,
}

impl FnView {
    fn num_args(&self) -> usize {
        self.params.len()
    }

    /// `arg_info[MIN(offset, num_args)]`.
    fn arg_info(&self, offset: usize) -> &PParam {
        if offset < self.params.len() {
            &self.params[offset]
        } else {
            self.variadic.as_ref().expect("variadic function")
        }
    }

    /// `zend_get_arg_offset_by_name`: `None` for unknown parameters.
    fn arg_offset_by_name(&self, name: &[u8]) -> Option<usize> {
        if let Some(i) = self.params.iter().position(|p| p.name.as_bytes() == name) {
            return Some(i);
        }
        self.variadic.as_ref().map(|_| self.params.len())
    }
}

fn type_info(decl: &TypeDecl) -> TypeInfo {
    TypeInfo {
        text: PhpStr::from(decl.to_type_string()),
        decl: decl.clone(),
    }
}

fn is_sensitive(a: &Rc<AttrDef>) -> bool {
    let n = a.name.as_bytes();
    let n = n.strip_prefix(b"\\").unwrap_or(n);
    n.eq_ignore_ascii_case(b"SensitiveParameter")
}

/// Builds the view of a userland or internal function.
fn view_of(vm: &mut Vm, func: &Rc<Function>, unknown_defaults: bool) -> PResult<FnView> {
    let scope = func.scope.as_ref().and_then(|s| vm.find_class(s));
    let mut v = FnView {
        func: func.clone(),
        display: func.display_name(),
        name: func.name.clone(),
        scope,
        params: Vec::new(),
        variadic: None,
        required: 0,
        is_closure: false,
        is_static: func.is_static(),
        trampoline: false,
        return_type: None,
        returns_ref: func.has_flag(fn_flags::RETURNS_REF),
        nodiscard: None,
        by_ref_func: Some(func.clone()),
        cache_key: None,
    };
    if let Some(u) = user_fn(func) {
        let code = u.code.clone();
        if let Some(s) = u.scope_class() {
            v.scope = Some(s);
        }
        let required = code
            .params
            .iter()
            .rposition(|p| !p.variadic && p.default.is_none())
            .map_or(0, |i| i + 1);
        v.required = required;
        for (i, p) in code.params.iter().enumerate() {
            let default = if unknown_defaults {
                if i < required { PDefault::Required } else { PDefault::Unknown }
            } else if i < required || p.variadic {
                PDefault::Required
            } else if let Some(c) = &p.default_const {
                PDefault::Const(c.clone())
            } else if p.default.is_some() {
                PDefault::Expr(code.clone(), i as u32)
            } else {
                PDefault::Required
            };
            let pp = PParam {
                name: p.name.clone(),
                ty: p.ty.clone(),
                by_ref: p.by_ref,
                must_ref: p.by_ref,
                default,
                attributes: p.attributes.iter().filter(|a| is_sensitive(a)).cloned().collect(),
            };
            if p.variadic {
                v.variadic = Some(pp);
            } else {
                v.params.push(pp);
            }
        }
        v.return_type = code.return_type.clone();
        v.returns_ref = code.returns_ref();
        v.nodiscard = code.attributes.iter().find(|a| a.name.eq_ignore_ascii_case(b"NoDiscard")).cloned();
        v.cache_key = Some(Rc::as_ptr(&u.code) as *const u8 as usize);
    } else {
        let sig = func.signature().clone();
        v.required = sig.required;
        for (i, p) in sig.params.iter().enumerate() {
            let default = if i < sig.required || p.variadic {
                PDefault::Required
            } else if unknown_defaults {
                PDefault::Unknown
            } else {
                match &p.default {
                    Some(DefaultValue::Expr(_)) => match vm.native_param_default(p)? {
                        Some(d) => PDefault::Const(d),
                        None => PDefault::Unknown,
                    },
                    _ => PDefault::Unknown,
                }
            };
            let pp = PParam {
                name: p.name.clone(),
                ty: p.ty.as_ref().map(type_info),
                by_ref: p.by_ref,
                must_ref: p.by_ref && !p.prefer_ref,
                default,
                attributes: Vec::new(),
            };
            if p.variadic {
                v.variadic = Some(pp);
            } else {
                v.params.push(pp);
            }
        }
        v.return_type = sig.return_type.as_ref().map(type_info);
        v.returns_ref = sig.returns_ref;
        if let FunctionKind::Native(nf) = &func.kind {
            v.cache_key = Some(*nf as *const _ as *const u8 as usize);
        }
    }
    Ok(v)
}

/// The `__call` / `__callStatic` trampoline (`zend_get_call_trampoline_func`).
fn trampoline_view(vm: &mut Vm, magic: &Rc<Function>, name: &PhpStr, class: &ClassRef) -> FnView {
    let _ = vm;
    FnView {
        func: magic.clone(),
        display: format!("{}::{}", class.name, name),
        name: name.clone(),
        scope: Some(class.clone()),
        params: Vec::new(),
        variadic: Some(PParam {
            name: PhpStr::from("arguments"),
            ty: Some(type_info(&TypeDecl::mixed())),
            by_ref: false,
            must_ref: false,
            default: PDefault::Required,
            attributes: Vec::new(),
        }),
        required: 0,
        is_closure: false,
        is_static: magic.is_static(),
        trampoline: true,
        return_type: None,
        returns_ref: magic.has_flag(fn_flags::RETURNS_REF),
        nodiscard: None,
        by_ref_func: None,
        cache_key: None,
    }
}

/// The resolved callee of a partial application.
struct PTarget {
    view: FnView,
    /// `call->This` object.
    this: Option<ObjRef>,
    /// `Z_CE(call->This)` / the object's class.
    called: Option<ClassRef>,
    /// The callee is a closure object (bound as `$fn`).
    closure: Option<(ObjRef, Rc<ClosureData>)>,
}

fn target_from(vm: &mut Vm, t: Target, closure_obj: Option<ObjRef>) -> PResult<PTarget> {
    if let Some(name) = &t.magic_name {
        let class = match (&t.this, &t.called) {
            (Some(o), _) => o.class().clone(),
            (None, Some(c)) => c.clone(),
            _ => vm.find_class(t.func.scope.as_deref().unwrap_or_default()).expect("magic method class"),
        };
        let view = trampoline_view(vm, &t.func, name, &class);
        return Ok(PTarget {
            view,
            this: t.this,
            called: t.called,
            closure: None,
        });
    }
    if let (Some(c), Some(o)) = (&t.closure, closure_obj) {
        let mut view = view_of(vm, &c.func, false)?;
        view.is_closure = true;
        view.scope = c.scope.clone();
        return Ok(PTarget {
            view,
            this: c.this.clone(),
            called: c.called.clone(),
            closure: Some((o, c.clone())),
        });
    }
    let view = view_of(vm, &t.func, false)?;
    let this = if t.func.is_static() { None } else { t.this };
    Ok(PTarget {
        view,
        this,
        called: t.called,
        closure: None,
    })
}

/// `$closure->__invoke(...)` (`zend_get_closure_invoke_method`).
fn closure_invoke_target(vm: &mut Vm, o: &ObjRef, c: &Rc<ClosureData>) -> PResult<PTarget> {
    let mut view = view_of(vm, &c.func, true)?;
    let closure_class = o.class().clone();
    view.display = "Closure::__invoke".to_string();
    view.name = PhpStr::from("__invoke");
    view.scope = Some(closure_class.clone());
    view.is_static = false;
    view.cache_key = view.cache_key.map(|k| k ^ 1);
    Ok(PTarget {
        view,
        this: Some(o.clone()),
        called: Some(closure_class),
        closure: None,
    })
}

fn resolve_target(vm: &mut Vm, cx: &Ctx, target: &CallableConvert, line: u32) -> PResult<PTarget> {
    vm.current_line = line;
    match target {
        CallableConvert::Func(Callee::Named(nc)) => {
            let f = call::resolve_function(vm, nc)?;
            let view = view_of(vm, &f, false)?;
            Ok(PTarget {
                view,
                this: None,
                called: None,
                closure: None,
            })
        }
        CallableConvert::Func(Callee::Expr(e)) => {
            let v = eval(vm, cx, e)?.into_unref();
            vm.current_line = line;
            let closure_obj = match &v {
                Value::Object(o) if closure::closure_data(o).is_some() => Some(o.clone()),
                _ => None,
            };
            let t = call::resolve_callable_for_call(vm, cx, &v)?;
            target_from(vm, t, closure_obj)
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
            if name.eq_ignore_ascii_case(b"__invoke") {
                if let Some(c) = closure::closure_data(&o) {
                    return closure_invoke_target(vm, &o, &c);
                }
            }
            let t = call::method_target(vm, cx.scope(), &o, &name)?;
            target_from(vm, t, None)
        }
        CallableConvert::Static(cs, m) => {
            let class = eval::resolve_class(vm, cx, cs, line)?;
            let name = place::member_name(vm, cx, m)?;
            vm.current_line = line;
            let forwarding = matches!(cs, ClassSpec::Self_ | ClassSpec::Parent | ClassSpec::Static);
            let mut t = call::static_method_target(vm, cx.env, &class, &name, forwarding)?;
            if matches!(cs, ClassSpec::Static) && t.this.is_none() && t.func.is_static() {
                t.called = Some(class.clone());
            }
            target_from(vm, t, None)
        }
    }
}

// ---------------------------------------------------------------------------
// Argument evaluation (SEND_* / SEND_PLACEHOLDER)
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum ArgSlot {
    Undef,
    Placeholder,
    Val(Value),
}

impl ArgSlot {
    fn is_placeholder(&self) -> bool {
        matches!(self, ArgSlot::Placeholder)
    }

    fn is_undef(&self) -> bool {
        matches!(self, ArgSlot::Undef)
    }
}

struct SentArgs {
    argv: Vec<ArgSlot>,
    extra_named: Vec<(PhpStr, ArgSlot)>,
}

/// `zend_handle_named_arg`: the slot for a named argument.
fn named_slot<'a>(vm: &mut Vm, view: &FnView, sent: &'a mut SentArgs, name: &PhpStr) -> PResult<&'a mut ArgSlot> {
    let Some(offset) = view.arg_offset_by_name(name) else {
        return Err(vm.throw_error("Error", format!("Unknown named parameter ${name}")));
    };
    if offset == view.num_args() {
        if sent.extra_named.iter().any(|(n, _)| n == name) {
            return Err(vm.throw_error("Error", format!("Named parameter ${name} overwrites previous argument")));
        }
        sent.extra_named.push((name.clone(), ArgSlot::Undef));
        return Ok(&mut sent.extra_named.last_mut().unwrap().1);
    }
    if offset >= sent.argv.len() {
        sent.argv.resize(offset + 1, ArgSlot::Undef);
    } else if !sent.argv[offset].is_undef() {
        let what = if sent.argv[offset].is_placeholder() { "placeholder" } else { "argument" };
        return Err(vm.throw_error("Error", format!("Named parameter ${name} overwrites previous {what}")));
    }
    Ok(&mut sent.argv[offset])
}

fn send_args(vm: &mut Vm, cx: &Ctx, view: &FnView, pe: &PartialExpr) -> PResult<SentArgs> {
    let mut sent = SentArgs {
        argv: Vec::with_capacity(pe.args.len()),
        extra_named: Vec::new(),
    };
    let by_ref_func = view.by_ref_func.clone();
    for a in &pe.args {
        match a {
            PartialArg::Pos(e) => {
                let i = sent.argv.len();
                let v = call::eval_arg(vm, cx, e, by_ref_func.as_ref(), i, None)?;
                sent.argv.push(ArgSlot::Val(v));
            }
            PartialArg::Placeholder => sent.argv.push(ArgSlot::Placeholder),
            PartialArg::Named(n, e) => {
                let v = call::eval_arg(vm, cx, e, by_ref_func.as_ref(), 0, Some(n))?;
                vm.current_line = pe.line;
                let slot = named_slot(vm, view, &mut sent, n)?;
                *slot = ArgSlot::Val(v);
            }
            PartialArg::NamedPlaceholder(n) => {
                vm.current_line = pe.line;
                let slot = named_slot(vm, view, &mut sent, n)?;
                *slot = ArgSlot::Placeholder;
            }
        }
    }
    Ok(sent)
}

// ---------------------------------------------------------------------------
// zp_compile
// ---------------------------------------------------------------------------

/// Functions with `ZEND_ACC2_FORBID_DYN_CALLS`.
pub(crate) fn forbids_dynamic_calls(f: &Function) -> bool {
    f.is_native()
        && f.scope.is_none()
        && matches!(
            f.name.to_ascii_lowercase().as_bytes(),
            b"func_num_args" | b"func_get_arg" | b"func_get_args" | b"get_defined_vars" | b"extract" | b"compact"
        )
}

/// `zp_args_check`.
fn args_check(vm: &mut Vm, view: &FnView, argc: usize, sent: &SentArgs, variadic: bool) -> PResult<()> {
    for (k, v) in &sent.extra_named {
        if v.is_placeholder() {
            return Err(vm.throw_error(
                "Error",
                format!("Cannot use named placeholder for unknown or variadic parameter ${k}"),
            ));
        }
    }
    if argc < view.required {
        if variadic {
            return Ok(());
        }
        let limit = if view.num_args() <= view.required { "exactly" } else { "at least" };
        return Err(vm.throw_error(
            "ArgumentCountError",
            format!(
                "Partial application of {}() expects {limit} {} arguments, {argc} given",
                view.display, view.required
            ),
        ));
    } else if argc > view.num_args() && view.variadic.is_none() {
        return Err(vm.throw_error(
            "ArgumentCountError",
            format!(
                "Partial application of {}() expects at most {} arguments, {argc} given",
                view.display,
                view.num_args()
            ),
        ));
    }
    Ok(())
}

/// `zend_argument_error_ex(zend_ce_argument_count_error, function, arg_num, ...)`.
fn argument_count_error(vm: &mut Vm, view: &FnView, offset: usize, what: &str) -> crate::vm::Flow {
    let name = if offset < view.num_args() {
        format!(" (${})", view.params[offset].name)
    } else {
        String::new()
    };
    vm.throw_error(
        "ArgumentCountError",
        format!("{}(): Argument #{}{name} {what}", view.display, offset + 1),
    )
}

/// Variable names of the generated closure (`zp_assign_names`).
struct Names {
    params: Vec<Option<PhpStr>>,
    variadic_param: Option<PhpStr>,
    extra_named_params: Option<PhpStr>,
    inner_closure: Option<PhpStr>,
}

impl Names {
    fn exists(&self, name: &[u8]) -> bool {
        self.params.iter().flatten().any(|p| p.as_bytes() == name)
            || self.variadic_param.as_ref().is_some_and(|p| p.as_bytes() == name)
            || self.extra_named_params.as_ref().is_some_and(|p| p.as_bytes() == name)
            || self.inner_closure.as_ref().is_some_and(|p| p.as_bytes() == name)
    }

    fn unique(&self, base: &[u8], mut n: u32) -> PhpStr {
        let mut name = PhpStr::from_bytes(base);
        while self.exists(&name) {
            let mut b = base.to_vec();
            b.extend_from_slice(n.to_string().as_bytes());
            name = PhpStr::from_vec(b);
            n += 1;
        }
        name
    }
}

fn assign_names(view: &FnView, argv: &[ArgSlot], variadic_partial: bool, has_extra_named: bool) -> Names {
    let argc = argv.len();
    let num_args = view.num_args();
    let mut names = Names {
        params: vec![None; argc],
        variadic_param: None,
        extra_named_params: None,
        inner_closure: None,
    };
    for offset in 0..argc.min(num_args) {
        if argv[offset].is_placeholder() {
            names.params[offset] = Some(view.params[offset].name.clone());
        }
    }
    if variadic_partial {
        if let Some(v) = &view.variadic {
            names.variadic_param = Some(v.name.clone());
        }
    }
    for offset in num_args..argc {
        if !argv[offset].is_placeholder() {
            continue;
        }
        let orig = view.arg_info(offset).name.clone();
        let mut n = 0u32;
        let new_name = loop {
            let mut b = orig.to_vec();
            b.extend_from_slice(n.to_string().as_bytes());
            if !names.exists(&b) {
                break PhpStr::from_vec(b);
            }
            n += 1;
        };
        names.params[offset] = Some(new_name);
    }
    for offset in 0..argc {
        if argv[offset].is_placeholder() || argv[offset].is_undef() {
            continue;
        }
        let orig = view.arg_info(offset).name.clone();
        let new_name = names.unique(&orig, 2);
        names.params[offset] = Some(new_name);
    }
    if has_extra_named {
        names.extra_named_params = Some(names.unique(b"extra_named_params", 2));
    }
    if view.is_closure {
        names.inner_closure = Some(names.unique(b"fn", 2));
    }
    names
}

/// A compiled-variable table under construction.
#[derive(Default)]
struct Cvs {
    table: CvTable,
}

impl Cvs {
    fn add(&mut self, name: &PhpStr) -> Slot {
        if let Some(s) = self.table.lookup(name) {
            return s;
        }
        let s = self.table.names.len() as Slot;
        self.table.names.push(name.clone());
        self.table.index.insert(name.clone(), s);
        s
    }
}

fn named_call(name: &str, args: Vec<Arg>, line: u32) -> Expr {
    Expr::Call(Box::new(CallExpr {
        callee: Callee::Named(Box::new(NamedCallee {
            name: PhpStr::from(name),
            lc: PhpStr::from(name),
            fallback: None,
            cache: OnceCell::new(),
        })),
        args: ArgList {
            args,
            has_spread: false,
            has_named: false,
        },
        line,
    }))
}

struct CompileInput<'a> {
    view: &'a FnView,
    pe: &'a PartialExpr,
    this_is_object: bool,
    has_called_scope: bool,
    strict: bool,
    file: PhpStr,
}

/// `zp_compile_forwarding_call`.
fn forwarding_call(
    vm: &mut Vm,
    inp: &CompileInput,
    argv: &[ArgSlot],
    slots: &[Option<Slot>],
    extra_slot: Option<Slot>,
    variadic_slot: Option<Slot>,
    fn_slot: Option<Slot>,
    num_params: usize,
    forward_superfluous: bool,
) -> PResult<Stmt> {
    let view = inp.view;
    let line = inp.pe.line;
    let is_assert = view.name.eq_ignore_ascii_case(b"assert") && view.scope.is_none() && view.func.is_native();
    let mut args = Vec::with_capacity(argv.len() + 3);
    if is_assert {
        args.push(Arg::Pos(Expr::Lit(Value::str("assert"))));
    }
    let mut has_spread = false;
    for (offset, a) in argv.iter().enumerate() {
        match a {
            ArgSlot::Undef => {
                if offset < view.required {
                    return Err(argument_count_error(vm, view, offset, "not passed"));
                }
                let e = match &view.params[offset].default {
                    PDefault::Const(v) => Expr::Lit(v.clone()),
                    PDefault::Expr(code, i) => Expr::Pfa(Box::new(PfaExpr::ParamDefault(code.clone(), *i))),
                    PDefault::Unknown | PDefault::Required => {
                        return Err(argument_count_error(
                            vm,
                            view,
                            offset,
                            "must be passed explicitly, because the default value is not known",
                        ));
                    }
                };
                args.push(Arg::Pos(e));
            }
            _ => args.push(Arg::Pos(Expr::Var(slots[offset].expect("named var"), line))),
        }
    }
    if let Some(s) = extra_slot {
        has_spread = true;
        args.push(Arg::Spread(Expr::Var(s, line)));
    }
    if inp.pe.variadic {
        if let Some(s) = variadic_slot {
            has_spread = true;
            args.push(Arg::Spread(Expr::Var(s, line)));
        } else if forward_superfluous {
            has_spread = true;
            let fga = named_call("func_get_args", Vec::new(), line);
            let slice = named_call(
                "array_slice",
                vec![Arg::Pos(fga), Arg::Pos(Expr::Lit(Value::Int(num_params as i64)))],
                line,
            );
            args.push(Arg::Spread(slice));
        }
    }
    let arglist = ArgList {
        args,
        has_spread,
        has_named: false,
    };
    let call = if is_assert {
        let mut e = named_call("call_user_func", Vec::new(), line);
        if let Expr::Call(c) = &mut e {
            c.args = arglist;
        }
        e
    } else if view.is_closure {
        Expr::Call(Box::new(CallExpr {
            callee: Callee::Expr(Box::new(Expr::Var(fn_slot.expect("$fn"), line))),
            args: arglist,
            line,
        }))
    } else if inp.this_is_object {
        Expr::MethodCall(Box::new(MethodCallExpr {
            object: Expr::This(line),
            name: Member::Name(view.name.clone()),
            args: arglist,
            nullsafe: false,
            line,
        }))
    } else if inp.has_called_scope {
        Expr::StaticCall(Box::new(StaticCallExpr {
            class: ClassSpec::Static,
            name: Member::Name(view.name.clone()),
            args: arglist,
            line,
        }))
    } else {
        let cache = OnceCell::new();
        let _ = cache.set(view.func.clone());
        Expr::Call(Box::new(CallExpr {
            callee: Callee::Named(Box::new(NamedCallee {
                name: view.name.clone(),
                lc: view.name.to_ascii_lowercase(),
                fallback: None,
                cache,
            })),
            args: arglist,
            line,
        }))
    };
    let is_void = view
        .return_type
        .as_ref()
        .is_some_and(|t| t.decl.mask & (mask::VOID | mask::NEVER) != 0);
    Ok(Stmt {
        kind: if is_void { StmtKind::Expr(call) } else { StmtKind::Return(Some(call)) },
        line,
    })
}

/// `zp_compute_num_required`.
fn compute_num_required(view: &FnView, orig_offset: usize, new_offset: usize, num_required: usize) -> usize {
    if orig_offset < view.num_args() {
        if orig_offset < view.required {
            return num_required.max(new_offset + 1);
        }
        num_required
    } else {
        new_offset + 1
    }
}

/// `zp_compile`: generates the closure code.
fn compile(vm: &mut Vm, inp: &CompileInput, sent: &SentArgs) -> PResult<Rc<FuncCode>> {
    let view = inp.view;
    let pe = inp.pe;
    if forbids_dynamic_calls(&view.func) && !view.trampoline {
        return Err(vm.throw_error("Error", format!("Cannot call {}() dynamically", view.name)));
    }
    let argc0 = sent.argv.len();
    args_check(vm, view, argc0, sent, pe.variadic)?;

    let mut argv = sent.argv.clone();
    let new_argc = if pe.variadic { argc0.max(view.num_args()) } else { argc0 };
    argv.resize(new_argc, ArgSlot::Undef);

    // Parameter positions: positional placeholders, named placeholders in
    // syntax order, then implicit placeholders added by '...'.
    let mut num_params = 0usize;
    let mut map = vec![0usize; new_argc];
    let mut num_positional = 0usize;
    for (offset, a) in argv.iter().enumerate().take(argc0) {
        if !a.is_placeholder() {
            continue;
        }
        num_params += 1;
        let name = &view.arg_info(offset).name;
        let named_pos = pe.named_positions.iter().position(|n| n == name);
        map[offset] = match named_pos {
            Some(p) => num_positional + p,
            None => {
                num_positional += 1;
                num_positional - 1
            }
        };
    }
    let mut num_required = num_params;
    if pe.variadic {
        for offset in 0..new_argc {
            if offset < argc0 && !argv[offset].is_undef() {
                continue;
            }
            argv[offset] = ArgSlot::Placeholder;
            num_params += 1;
            let param_offset = num_params - 1;
            map[offset] = param_offset;
            num_required = compute_num_required(view, offset, param_offset, num_required);
        }
    }

    let names = assign_names(view, &argv, pe.variadic, !sent.extra_named.is_empty());

    // Parameters first (slots 0..num_params), then the variadic parameter.
    let mut cvs = Cvs::default();
    let mut param_names: Vec<Option<(usize, PhpStr)>> = vec![None; num_params];
    for (offset, a) in argv.iter().enumerate() {
        if a.is_placeholder() {
            param_names[map[offset]] = Some((offset, names.params[offset].clone().unwrap()));
        }
    }
    let line = pe.line;
    let mut params = Vec::with_capacity(num_params + 1);
    let mut slots: Vec<Option<Slot>> = vec![None; new_argc];
    for (param_offset, entry) in param_names.iter().enumerate() {
        let (offset, name) = entry.clone().expect("every parameter position is filled");
        let info = view.arg_info(offset).clone();
        let slot = cvs.add(&name);
        slots[offset] = Some(slot);
        let (default, default_const) = if param_offset >= num_required {
            match &info.default {
                PDefault::Const(v) => (Some(Expr::Lit(v.clone())), Some(v.clone())),
                PDefault::Expr(code, i) => (
                    Some(Expr::Pfa(Box::new(PfaExpr::ParamDefault(code.clone(), *i)))),
                    None,
                ),
                PDefault::Unknown | PDefault::Required => {
                    return Err(argument_count_error(
                        vm,
                        view,
                        offset,
                        "must be passed explicitly, because the default value is not known",
                    ));
                }
            }
        } else {
            (None, None)
        };
        params.push(ParamInfo {
            name,
            slot,
            by_ref: info.must_ref,
            variadic: false,
            ty: info.ty.clone(),
            default,
            default_const,
            default_text: None,
            promote: None,
            attributes: info.attributes.clone(),
            doc_comment: None,
            line,
        });
    }
    let mut variadic_slot = None;
    if pe.variadic {
        if let Some(v) = &view.variadic {
            let name = names.variadic_param.clone().unwrap();
            let slot = cvs.add(&name);
            variadic_slot = Some(slot);
            params.push(ParamInfo {
                name,
                slot,
                by_ref: v.must_ref,
                variadic: true,
                ty: v.ty.clone(),
                default: None,
                default_const: None,
                default_text: None,
                promote: None,
                attributes: v.attributes.clone(),
                doc_comment: None,
                line,
            });
        }
    }

    // Lexical variables: the inner closure, bound arguments, extra named args.
    let mut uses = Vec::new();
    let mut fn_slot = None;
    if let Some(n) = &names.inner_closure {
        let s = cvs.add(n);
        fn_slot = Some(s);
        uses.push(UseBinding {
            name: n.clone(),
            outer: 0,
            inner: s,
            by_ref: false,
            line,
        });
    }
    for (offset, a) in argv.iter().enumerate() {
        if let ArgSlot::Val(_) = a {
            let n = names.params[offset].clone().unwrap();
            let s = cvs.add(&n);
            slots[offset] = Some(s);
            uses.push(UseBinding {
                name: n,
                outer: 0,
                inner: s,
                by_ref: view.arg_info(offset).must_ref,
                line,
            });
        }
    }
    let mut extra_slot = None;
    if let Some(n) = &names.extra_named_params {
        let s = cvs.add(n);
        extra_slot = Some(s);
        uses.push(UseBinding {
            name: n.clone(),
            outer: 0,
            inner: s,
            by_ref: false,
            line,
        });
    }

    let body = if pe.variadic && view.variadic.is_none() {
        let no_fwd = forwarding_call(vm, inp, &argv, &slots, extra_slot, variadic_slot, fn_slot, num_params, false)?;
        let fwd = forwarding_call(vm, inp, &argv, &slots, extra_slot, variadic_slot, fn_slot, num_params, true)?;
        let cond = Expr::Bin(
            BinOp::LessEqual,
            Box::new(named_call("func_num_args", Vec::new(), line)),
            Box::new(Expr::Lit(Value::Int(num_params as i64))),
            line,
        );
        vec![Stmt {
            kind: StmtKind::If(vec![(cond, vec![no_fwd])], Some(vec![fwd])),
            line,
        }]
    } else {
        vec![forwarding_call(vm, inp, &argv, &slots, extra_slot, variadic_slot, fn_slot, num_params, false)?]
    };

    let is_static = !inp.this_is_object && !(view.is_closure && !view.is_static);
    let mut flags = 0;
    if inp.strict {
        flags |= code_flags::STRICT;
    }
    if view.returns_ref {
        flags |= code_flags::RETURNS_REF;
    }
    if is_static {
        flags |= code_flags::STATIC_CLOSURE;
    }
    if inp.this_is_object && !view.is_closure {
        flags |= code_flags::USES_THIS;
    }
    let mut attributes = Vec::new();
    if let Some(a) = &view.nodiscard {
        flags |= code_flags::NODISCARD;
        attributes.push(a.clone());
    }
    Ok(Rc::new(FuncCode {
        name: pe.name.clone(),
        kind: FuncKind::Closure,
        flags,
        cvs: Rc::new(cvs.table),
        params,
        body,
        return_type: view.return_type.clone(),
        file: inp.file.clone(),
        start_line: line,
        end_line: line,
        uses,
        num_statics: 0,
        class_name: view.scope.as_ref().map(|s| s.name.clone()),
        in_trait: false,
        doc_comment: None,
        attributes,
        labels: Vec::new(),
        namespace: PhpStr::new(),
    }))
}

/// Cached generated code of a call site (`cache_slot`).
struct CacheEntry {
    key: (usize, bool, bool),
    code: Rc<FuncCode>,
}

// ---------------------------------------------------------------------------
// zend_partial_create
// ---------------------------------------------------------------------------

fn create(vm: &mut Vm, cx: &Ctx, pe: &PartialExpr) -> PResult<Value> {
    let t = resolve_target(vm, cx, &pe.target, pe.line)?;
    let sent = send_args(vm, cx, &t.view, pe)?;
    vm.current_line = pe.line;
    let view = &t.view;

    let this_is_object = t.this.is_some();
    let called = match &t.this {
        Some(o) => Some(o.class().clone()),
        None => t.called.clone(),
    };
    let key = view.cache_key.map(|k| (k, this_is_object, called.is_some()));
    let cached = match (&key, pe.cache.borrow().as_ref()) {
        (Some(k), Some(c)) => c.downcast_ref::<CacheEntry>().filter(|e| e.key == *k).map(|e| e.code.clone()),
        _ => None,
    };
    let code = match cached {
        Some(c) => c,
        None => {
            let inp = CompileInput {
                view,
                pe,
                this_is_object,
                has_called_scope: called.is_some(),
                strict: cx.strict(),
                file: cx.code.file.clone(),
            };
            let code = compile(vm, &inp, &sent)?;
            if let Some(k) = key {
                *pe.cache.borrow_mut() = Some(Rc::new(CacheEntry { key: k, code: code.clone() }));
            }
            code
        }
    };

    // zend_create_partial_closure: scope of the function (magic methods: the
    // scope the PFA is created in).
    let scope = if view.trampoline { cx.env.scope.clone() } else { view.scope.clone() };
    let is_static_closure = code.has(code_flags::STATIC_CLOSURE);
    let object = if scope.is_some() && !is_static_closure { t.this.clone() } else { None };

    // zp_bind: type-check and bind the pre-bound arguments.
    let mut bound = Vec::with_capacity(code.uses.len());
    let mut uses = code.uses.iter();
    if let Some((o, _)) = &t.closure {
        let u = uses.next().expect("$fn binding");
        bound.push((u.inner, Value::Object(o.clone())));
    }
    for (offset, a) in sent.argv.iter().enumerate() {
        let ArgSlot::Val(v) = a else { continue };
        let info = if offset < view.num_args() {
            Some(&view.params[offset])
        } else {
            view.variadic.as_ref()
        };
        let mut v = v.clone();
        if let Some(ty) = info.and_then(|i| i.ty.as_ref()) {
            v = check_bound_type(vm, view, offset, ty, v, cx.strict())?;
        }
        let u = uses.next().expect("bound argument");
        bound.push((u.inner, v));
    }
    if !sent.extra_named.is_empty() {
        let mut arr = PhpArray::new();
        for (k, v) in &sent.extra_named {
            if let ArgSlot::Val(v) = v {
                arr.insert(ArrayKey::Str(k.clone()), v.clone().into_unref());
            }
        }
        let u = uses.next().expect("extra named params");
        bound.push((u.inner, Value::array(arr)));
    }

    let mut flags = member_flags::PUBLIC | fn_flags::CLOSURE;
    if is_static_closure {
        flags |= member_flags::STATIC;
    }
    let ufn = UserFn::new(code.clone());
    let f = super::decl::make_function_from(ufn, code.name.clone(), flags, scope.as_ref().map(|s| s.name.clone()));
    if let (Some(s), Some(u)) = (&scope, user_fn(&f)) {
        let _ = u.scope.set(Rc::downgrade(s));
    }
    let pfa = if view.is_closure { closure::PFA_OF_CLOSURE } else { closure::PFA };
    Ok(closure::new_closure(
        vm,
        ClosureData {
            func: Rc::new(f),
            this: object,
            scope,
            called,
            bound,
            magic_name: None,
            from_callable: false,
            pfa,
        },
    ))
}

/// The type check of `zp_bind` (`zend_check_type_ex` in the creating frame).
fn check_bound_type(vm: &mut Vm, view: &FnView, offset: usize, ty: &TypeInfo, v: Value, strict: bool) -> PResult<Value> {
    if ty.decl.is_mixed() {
        return Ok(v);
    }
    let inner = v.unref();
    if call::type_matches_in(vm, &ty.decl, &inner, view.scope.as_ref(), view.scope.as_ref())? {
        return Ok(v);
    }
    if !inner.is_null() {
        if let Some(c) = vm.coerce_to_type(&ty.decl, inner.clone(), CoerceMode { strict, silent: false })? {
            return Ok(match v {
                Value::Ref(r) => {
                    r.set(c);
                    Value::Ref(r)
                }
                _ => c,
            });
        }
    }
    let need = ty.decl.to_type_string_in(view.scope.as_deref(), None);
    let name = if offset < view.num_args() {
        format!(" (${})", view.params[offset].name)
    } else {
        String::new()
    };
    Err(vm.throw_type_error(format!(
        "{}(): Argument #{}{name} must be of type {need}, {} given",
        view.display,
        offset + 1,
        inner.value_name()
    )))
}
