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
}

/// The closure payload of an object, if it is a `Closure`.
pub fn closure_data(o: &ObjRef) -> Option<Rc<ClosureData>> {
    o.native::<Rc<ClosureData>>().map(|r| r.clone())
}

fn closure_class(vm: &Vm) -> ClassRef {
    vm.find_class(b"Closure").expect("Closure is registered")
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

/// Rebinds a closure; `Ok(None)` after a warning.
fn rebind(vm: &mut Vm, c: &Rc<ClosureData>, new_this: Option<ObjRef>, new_scope: Option<Option<ClassRef>>, is_call: bool) -> PResult<Option<ClosureData>> {
    let is_static_fn = c.func.is_static();
    if new_this.is_some() && is_static_fn {
        vm.warning("Cannot bind an instance to a static closure")?;
        return Ok(None);
    }
    let scope = match new_scope {
        Some(s) => s,
        None => c.scope.clone(),
    };
    if c.from_callable {
        let same_scope = match (&scope, &c.scope) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same_scope {
            vm.warning("Cannot rebind scope of closure created from method")?;
            return Ok(None);
        }
        if c.func.scope.is_some() && !c.func.is_static() {
            match (&new_this, &c.this) {
                (None, Some(_)) => {
                    vm.warning("Cannot unbind $this of method")?;
                    return Ok(None);
                }
                (Some(nt), _) => {
                    if let Some(sc) = &c.scope {
                        if !nt.class().instance_of(sc) {
                            vm.warning(format!(
                                "Cannot bind method {}::{}() to object of class {}",
                                sc.name,
                                c.func.name,
                                nt.class().name
                            ))?;
                            return Ok(None);
                        }
                    }
                }
                _ => {}
            }
        }
        if matches!(c.func.kind, FunctionKind::Native(_)) && new_this.is_some() && c.this.is_none() && c.func.scope.is_none() {
            vm.warning("Cannot bind an instance to a static closure")?;
            return Ok(None);
        }
    } else if new_this.is_none() && c.this.is_some() && uses_this(c) && !is_static_fn {
        vm.warning("Cannot unbind $this of closure using $this")?;
        return Ok(None);
    }
    if let Some(s) = &scope {
        if s.is_internal() && !c.scope.as_ref().is_some_and(|cs| Rc::ptr_eq(cs, s)) {
            if !s.name.eq_ignore_ascii_case(b"Closure") || true {
                vm.warning(format!("Cannot bind closure to scope of internal class {}", s.name))?;
                return Ok(None);
            }
        }
    }
    let _ = is_call;
    let called = match &new_this {
        Some(t) => Some(t.class().clone()),
        None => scope.clone(),
    };
    // Statics are duplicated per closure object.
    let func = match user_fn(&c.func) {
        Some(u) => {
            let nu = u.fork();
            let mut f = super::decl::make_function_from(nu, c.func.name.clone(), c.func.flags, scope.as_ref().map(|s| s.name.clone()));
            f.location = c.func.location.clone();
            if let (Some(s), Some(u)) = (&scope, user_fn(&f)) {
                let _ = u.scope.set(Rc::downgrade(s));
            }
            Rc::new(f)
        }
        None => c.func.clone(),
    };
    Ok(Some(ClosureData {
        func,
        this: new_this,
        scope,
        called,
        bound: c.bound.iter().map(|(s, v)| (*s, v.clone())).collect(),
        magic_name: c.magic_name.clone(),
        from_callable: c.from_callable,
    }))
}

fn scope_arg(vm: &mut Vm, v: &Value, current: &Option<ClassRef>) -> PResult<Option<Option<ClassRef>>> {
    match v {
        Value::Undef => Ok(None),
        Value::Object(o) => Ok(Some(Some(o.class().clone()))),
        Value::Null => Ok(Some(None)),
        other => {
            let s = vm.to_php_string(other)?;
            if s.as_bytes() == b"static" {
                return Ok(Some(current.clone()));
            }
            match vm.lookup_class(&s)? {
                Some(c) => Ok(Some(Some(c))),
                None => {
                    vm.warning(format!("Class \"{s}\" not found"))?;
                    Err(crate::vm::Flow::Return(Value::Null))
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

fn do_bind(vm: &mut Vm, c: &Rc<ClosureData>, this: Option<&Value>, scope: &Value) -> PResult<Value> {
    let new_this = match this {
        Some(Value::Object(o)) => Some(o.clone()),
        _ => None,
    };
    let new_scope = match scope_arg(vm, scope, &c.scope) {
        Ok(s) => s,
        Err(crate::vm::Flow::Return(_)) => return Ok(Value::Null),
        Err(e) => return Err(e),
    };
    match rebind(vm, c, new_this, new_scope, false)? {
        Some(d) => Ok(new_closure_object(vm, d)),
        None => Ok(Value::Null),
    }
}

fn closure_bind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(c) = closure_data(args.object(0)) else {
        return Err(vm.throw_type_error("Closure::bind(): Argument #1 ($closure) must be of type Closure"));
    };
    let this = args.get(1).clone();
    let scope = if args.is_passed(2) { args.get(2).clone() } else { Value::Undef };
    do_bind(vm, &c, Some(&this), &scope)
}

fn closure_bind_to(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let c = this_closure(vm, args)?;
    let this = args.get(0).clone();
    let scope = if args.is_passed(1) { args.get(1).clone() } else { Value::Undef };
    do_bind(vm, &c, Some(&this), &scope)
}

fn closure_call(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let c = this_closure(vm, args)?;
    let new_this = args.object(0).clone();
    if c.func.is_static() {
        vm.warning("Cannot bind an instance to a static closure")?;
        return Ok(Value::Null);
    }
    let new_scope = Some(Some(new_this.class().clone()));
    if new_this.class().is_internal() && !c.from_callable {
        // Binding to internal classes is allowed for call() only when the
        // scope is unchanged.
    }
    let d = match rebind(vm, &c, Some(new_this.clone()), new_scope, true)? {
        Some(d) => Rc::new(d),
        None => return Ok(Value::Null),
    };
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
