//! The `Closure` class (`Zend/zend_closures.c`): closure creation, bound
//! variables, `bind`/`bindTo`/`call`/`fromCallable`, first-class callables.

use std::rc::Rc;

use crate::class::{ClassEntry, ClassRef, Function, FunctionKind, class_flags, fn_flags, member_flags};
use crate::compile::ir::*;
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::value::{PhpRef, PhpStr, Value};
use crate::vm::{PResult, Vm};

use super::call::{Bound, Caller, Target};
use super::{Ctx, UserFn, user_fn};

/// Native payload of a `Closure` object.
pub struct ClosureData {
    pub func: Rc<Function>,
    pub this: Option<ObjRef>,
    /// Class scope (visibility, `self::`).
    pub scope: Option<ClassRef>,
    /// Called scope (`static::`).
    pub called: Option<ClassRef>,
    /// Captured variables: (slot in the closure, value or reference).
    pub bound: Vec<(Slot, Value)>,
    /// `__call` / `__callStatic` trampoline: the method name.
    pub magic_name: Option<PhpStr>,
    /// Created from a function or method (`fromCallable`, `f(...)`).
    pub from_callable: bool,
    /// Partial application flags ([`PFA`], [`PFA_OF_CLOSURE`]).
    pub pfa: u8,
}

/// The closure is a partial application (`ZEND_PARTIAL`).
pub const PFA: u8 = 1;
/// The closure is a partial application of a closure, which is its first
/// bound variable (`ZEND_PARTIAL_OF_CLOSURE`).
pub const PFA_OF_CLOSURE: u8 = 3;

/// The closure payload of an object, if it is a `Closure`.
pub fn closure_data(o: &ObjRef) -> Option<Rc<ClosureData>> {
    o.native::<Rc<ClosureData>>().map(|r| r.clone())
}

fn closure_class(vm: &Vm) -> ClassRef {
    vm.find_class(b"Closure").expect("Closure is registered")
}

/// Wraps closure data into a new `Closure` object.
pub fn new_closure(vm: &mut Vm, data: ClosureData) -> Value {
    new_closure_object(vm, data)
}

fn new_closure_object(vm: &mut Vm, data: ClosureData) -> Value {
    let obj = ObjRef::new(closure_class(vm));
    obj.set_native(Rc::new(data));
    Value::Object(obj)
}

/// `ZEND_DECLARE_LAMBDA_FUNCTION` + `ZEND_BIND_LEXICAL`.
pub fn create_closure(vm: &mut Vm, cx: &Ctx, c: &ClosureExpr) -> PResult<Value> {
    let code = &c.code;
    let mut bound = Vec::with_capacity(code.uses.len() + c.auto_capture.len());
    for u in &code.uses {
        let outer = cx.slot(u.outer);
        if u.by_ref {
            bound.push((u.inner, Value::Ref(cx.env.make_ref(outer))));
        } else {
            let v = cx.env.get(outer);
            let v = if v.is_undef() {
                vm.current_line = u.line;
                super::eval::undefined_var(vm, &u.name)?;
                Value::Null
            } else {
                v
            };
            bound.push((u.inner, v));
        }
    }
    for (outer, inner) in &c.auto_capture {
        let v = cx.env.get(cx.slot(*outer));
        if !v.is_undef() {
            bound.push((*inner, v));
        }
    }
    let scope = cx.env.scope.clone();
    let this = if c.is_static { None } else { cx.env.this.clone() };
    let mut flags = member_flags::PUBLIC | fn_flags::CLOSURE;
    if c.is_static {
        flags |= member_flags::STATIC;
    }
    let ufn = UserFn::new(code.clone());
    let f = super::decl::make_function_from(ufn, code.name.clone(), flags, scope.as_ref().map(|s| s.name.clone()));
    if let Some(s) = &scope {
        if let Some(u) = user_fn(&f) {
            let _ = u.scope.set(Rc::downgrade(s));
        }
    }
    Ok(new_closure_object(
        vm,
        ClosureData {
            func: Rc::new(f),
            this,
            called: cx.env.called.clone().or_else(|| scope.clone()),
            scope,
            bound,
            magic_name: None,
            from_callable: false,
            pfa: 0,
        },
    ))
}

/// A closure for a resolved callable (`Closure::fromCallable`, `f(...)`).
pub fn closure_from_target(vm: &mut Vm, t: Target) -> PResult<Value> {
    if let Some(c) = &t.closure {
        // Already a closure: return an equivalent one.
        return Ok(new_closure_object(
            vm,
            ClosureData {
                func: c.func.clone(),
                this: c.this.clone(),
                scope: c.scope.clone(),
                called: c.called.clone(),
                bound: c.bound.clone(),
                magic_name: c.magic_name.clone(),
                from_callable: c.from_callable,
                pfa: c.pfa,
            },
        ));
    }
    let scope = t.func.scope.as_ref().and_then(|s| vm.find_class(s));
    Ok(new_closure_object(
        vm,
        ClosureData {
            func: t.func,
            this: t.this,
            scope,
            called: t.called,
            bound: Vec::new(),
            magic_name: t.magic_name,
            from_callable: true,
            pfa: 0,
        },
    ))
}

/// Calls a closure object with positional arguments.
pub fn call_closure(vm: &mut Vm, c: &Rc<ClosureData>, b: Bound, caller: &Caller) -> PResult<Value> {
    if let Some(name) = &c.magic_name {
        let t = Target {
            func: c.func.clone(),
            this: c.this.clone(),
            called: c.called.clone(),
            closure: None,
            magic_name: Some(name.clone()),
        };
        return super::call::call_target(vm, t, b, caller, false);
    }
    super::call::invoke(vm, &c.func, b, c.this.clone(), c.called.clone(), Some(c), caller, false)
}

// ---------------------------------------------------------------------------
// Binding (zend_valid_closure_binding / zend_create_closure_ex)
// ---------------------------------------------------------------------------

fn uses_this(c: &ClosureData) -> bool {
    user_fn(&c.func).is_some_and(|u| u.code.has(code_flags::USES_THIS))
}

fn same_class(a: Option<&ClassRef>, b: Option<&ClassRef>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn bind_warning(vm: &mut Vm, msg: impl std::fmt::Display) -> PResult<bool> {
    vm.engine_warning(format!("{msg}, this will be an error in PHP 9"))?;
    Ok(false)
}

/// `zend_valid_closure_binding`.
fn valid_binding(vm: &mut Vm, c: &ClosureData, new_this: Option<&ObjRef>, scope: Option<&ClassRef>) -> PResult<bool> {
    let func_scope = c.scope.as_ref();
    let is_fake = c.from_callable || c.pfa != 0;
    let is_static = c.func.is_static();
    if let Some(nt) = new_this {
        if is_static {
            return bind_warning(vm, "Cannot bind an instance to a static closure");
        }
        if is_fake {
            if let Some(fs) = func_scope {
                if !nt.class().instance_of(fs) {
                    let fname = c.magic_name.clone().unwrap_or_else(|| c.func.name.clone());
                    return bind_warning(
                        vm,
                        format!(
                            "Cannot bind method {}::{}() to object of class {}",
                            fs.name,
                            fname,
                            nt.class().name
                        ),
                    );
                }
            }
        }
    } else if is_fake && func_scope.is_some() && !is_static {
        return bind_warning(vm, "Cannot unbind $this of method");
    } else if !is_fake && c.this.is_some() && uses_this(c) {
        return bind_warning(vm, "Cannot unbind $this of closure using $this");
    }
    if let Some(s) = scope {
        if !same_class(Some(s), func_scope) && s.is_internal() {
            return bind_warning(vm, format!("Cannot bind closure to scope of internal class {}", s.name));
        }
    }
    if is_fake && !same_class(scope, func_scope) {
        if func_scope.is_none() {
            return bind_warning(vm, "Cannot rebind scope of closure created from function");
        }
        return bind_warning(vm, "Cannot rebind scope of closure created from method");
    }
    Ok(true)
}

/// A copy of a closure's function bound to `scope`; static variables are
/// duplicated unless `share_statics` (fake closures, `Closure::call`).
fn func_with_scope(c: &ClosureData, scope: Option<&ClassRef>, share_statics: bool) -> Rc<Function> {
    let same = same_class(scope, c.scope.as_ref());
    match (&c.func.kind, user_fn(&c.func)) {
        (FunctionKind::User(body), Some(u)) => {
            if share_statics {
                if same {
                    return c.func.clone();
                }
                let mut f = Function::user(c.func.name.clone(), body.clone(), c.func.signature().clone(), c.func.flags);
                f.scope = scope.map(|s| s.name.clone());
                f.doc_comment = c.func.doc_comment.clone();
                f.location = c.func.location.clone();
                f.attributes = c.func.attributes.clone();
                return Rc::new(f);
            }
            let nu = u.fork();
            let mut f = super::decl::make_function_from(nu, c.func.name.clone(), c.func.flags, scope.map(|s| s.name.clone()));
            f.location = c.func.location.clone();
            if let (Some(s), Some(u)) = (scope, user_fn(&f)) {
                let _ = u.scope.set(Rc::downgrade(s));
            }
            Rc::new(f)
        }
        _ => c.func.clone(),
    }
}

/// `zend_create_closure_ex` for a rebound closure.
fn create_bound(vm: &mut Vm, c: &ClosureData, scope: Option<ClassRef>, called: Option<ClassRef>, new_this: Option<ObjRef>) -> ClosureData {
    let mut scope = scope;
    if scope.is_none() && new_this.is_some() {
        // A dummy scope when binding an object without a scope.
        scope = Some(closure_class(vm));
    }
    let mut new_this = new_this;
    if c.func.is_native() && c.scope.is_none() {
        // Free internal functions: scope and $this are meaningless.
        new_this = None;
        scope = None;
    }
    if scope.is_none() || c.func.is_static() {
        new_this = None;
    }
    let func = func_with_scope(c, scope.as_ref(), c.from_callable);
    ClosureData {
        func,
        this: new_this,
        scope,
        called,
        bound: c.bound.clone(),
        magic_name: c.magic_name.clone(),
        from_callable: c.from_callable,
        pfa: c.pfa,
    }
}

enum ScopeArg {
    Static,
    Class(Option<ClassRef>),
}

fn scope_arg(vm: &mut Vm, v: &Value) -> PResult<Option<ScopeArg>> {
    match v {
        Value::Undef => Ok(Some(ScopeArg::Static)),
        Value::Object(o) => Ok(Some(ScopeArg::Class(Some(o.class().clone())))),
        Value::Null => Ok(Some(ScopeArg::Class(None))),
        other => {
            let s = vm.to_php_string(other)?;
            if s.as_bytes() == b"static" {
                return Ok(Some(ScopeArg::Static));
            }
            match vm.lookup_class(&s)? {
                Some(c) => Ok(Some(ScopeArg::Class(Some(c)))),
                None => {
                    vm.engine_warning(format!("Class \"{s}\" not found"))?;
                    Ok(None)
                }
            }
        }
    }
}

fn this_closure(vm: &mut Vm, args: &Args) -> PResult<Rc<ClosureData>> {
    match args.this().and_then(closure_data) {
        Some(c) => Ok(c),
        None => Err(vm.throw_error("Error", "Closure method called on a non-closure")),
    }
}

/// `do_closure_bind`.
fn do_bind(vm: &mut Vm, c: &Rc<ClosureData>, this: &Value, scope: &Value) -> PResult<Value> {
    let new_this = match this {
        Value::Object(o) => Some(o.clone()),
        _ => None,
    };
    let ce = match scope_arg(vm, scope)? {
        None => return Ok(Value::Null),
        Some(ScopeArg::Static) => c.scope.clone(),
        Some(ScopeArg::Class(ce)) => ce,
    };
    if !valid_binding(vm, c, new_this.as_ref(), ce.as_ref())? {
        return Ok(Value::Null);
    }
    let called = match &new_this {
        Some(t) => Some(t.class().clone()),
        None => ce.clone(),
    };
    let mut d = create_bound(vm, c, ce, called, new_this.clone());
    if c.pfa == PFA_OF_CLOSURE {
        // Re-bind the inner closure (the first bound variable).
        if let Some((slot, Value::Object(inner))) = d.bound.first().cloned() {
            if let Some(ic) = closure_data(&inner) {
                let nv = do_bind(vm, &ic, this, scope)?;
                d.bound[0] = (slot, nv);
            }
        }
    }
    Ok(new_closure_object(vm, d))
}

fn closure_bind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(c) = closure_data(args.object(0)) else {
        return Err(vm.throw_type_error("Closure::bind(): Argument #1 ($closure) must be of type Closure"));
    };
    let this = args.get(1).clone();
    let scope = if args.is_passed(2) { args.get(2).clone() } else { Value::Undef };
    do_bind(vm, &c, &this, &scope)
}

fn closure_bind_to(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let c = this_closure(vm, args)?;
    let this = args.get(0).clone();
    let scope = if args.is_passed(1) { args.get(1).clone() } else { Value::Undef };
    do_bind(vm, &c, &this, &scope)
}

/// `Closure::call`: calls the closure bound to `$newThis` and its class.
fn closure_call(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let c = this_closure(vm, args)?;
    let new_this = args.object(0).clone();
    let newclass = new_this.class().clone();
    if !valid_binding(vm, &c, Some(&new_this), Some(&newclass))? {
        return Ok(Value::Null);
    }
    let share = !c.func.has_flag(fn_flags::GENERATOR);
    let d = Rc::new(ClosureData {
        func: func_with_scope(&c, Some(&newclass), share),
        this: Some(new_this.clone()),
        scope: Some(newclass.clone()),
        called: Some(newclass),
        bound: c.bound.clone(),
        magic_name: c.magic_name.clone(),
        from_callable: c.from_callable,
        pfa: c.pfa,
    });
    let rest: Vec<Value> = args.rest(1).to_vec();
    let named: Vec<(PhpStr, Value)> = args.named_rest().to_vec();
    let b = Bound {
        positional: rest,
        named,
    };
    call_closure(vm, &d, b, &Caller::internal())
}

fn closure_from_callable(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).clone();
    if let Value::Object(o) = &v {
        if closure_data(o).is_some() {
            return Ok(v);
        }
    }
    match super::call::check_callable(vm, &v, false) {
        Ok(info) => closure_from_target(vm, info.target),
        Err(msg) => Err(vm.throw_type_error(format!("Failed to create closure from callable: {msg}"))),
    }
}

fn closure_invoke(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let c = this_closure(vm, args)?;
    let b = Bound {
        positional: args.rest(0).to_vec(),
        named: args.named_rest().to_vec(),
    };
    call_closure(vm, &c, b, &Caller::internal())
}

static BIND: NativeFunction = NativeFunction {
    sig: "public static function bind(Closure $closure, ?object $newThis, object|string|null $newScope = \"static\"): ?Closure {}",
    handler: closure_bind,
};
static BIND_TO: NativeFunction = NativeFunction {
    sig: "public function bindTo(?object $newThis, object|string|null $newScope = \"static\"): ?Closure {}",
    handler: closure_bind_to,
};
static CALL: NativeFunction = NativeFunction {
    sig: "public function call(object $newThis, mixed ...$args): mixed {}",
    handler: closure_call,
};
static FROM_CALLABLE: NativeFunction = NativeFunction {
    sig: "public static function fromCallable(callable $callback): Closure {}",
    handler: closure_from_callable,
};
static INVOKE: NativeFunction = NativeFunction {
    sig: "public function __invoke(mixed ...$args): mixed {}",
    handler: closure_invoke,
};

fn closure_get_closure(_vm: &mut Vm, o: &ObjRef) -> PResult<Option<Value>> {
    Ok(Some(Value::Object(o.clone())))
}

fn closure_clone(vm: &mut Vm, o: &ObjRef) -> PResult<ObjRef> {
    let c = closure_data(o).expect("closure");
    let d = ClosureData {
        func: c.func.clone(),
        this: c.this.clone(),
        scope: c.scope.clone(),
        called: c.called.clone(),
        bound: c.bound.clone(),
        magic_name: c.magic_name.clone(),
        from_callable: c.from_callable,
        pfa: c.pfa,
    };
    let Value::Object(n) = new_closure_object(vm, d) else { unreachable!() };
    Ok(n)
}

/// Registers the `Closure` class.
pub fn register(r: &mut Registry<'_>) {
    let mut ce = ClassEntry::new(
        "Closure",
        class_flags::FINAL | class_flags::NO_DYNAMIC_INSTANTIATION | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES,
    );
    let pub_ = member_flags::PUBLIC;
    ce.add_method(Function::native_method(&BIND, "Closure", pub_ | member_flags::STATIC));
    ce.add_method(Function::native_method(&BIND_TO, "Closure", pub_));
    ce.add_method(Function::native_method(&CALL, "Closure", pub_));
    ce.add_method(Function::native_method(&FROM_CALLABLE, "Closure", pub_ | member_flags::STATIC));
    let mut inv = Function::native_method(&INVOKE, "Closure", pub_);
    inv.flags |= fn_flags::NAMED_VARIADIC;
    ce.add_method(inv);
    ce.handlers.get_closure = Some(closure_get_closure);
    ce.handlers.clone_obj = Some(closure_clone);
    r.class(ce);
}

/// Makes a reference wrapper (used by bound by-ref variables).
#[allow(dead_code)]
pub(crate) fn ref_of(v: Value) -> PhpRef {
    match v {
        Value::Ref(r) => r,
        v => PhpRef::new(v),
    }
}
