//! Generators (`Zend/zend_generators.c`) on stackful coroutines.
//!
//! A generator's body is interpreted by the ordinary evaluator on its own
//! coroutine stack (`corosensei`); `yield` suspends the coroutine. Values
//! sent in (`send`, `throw`, destruction) come back as the result of the
//! suspended `yield`.
//!
//! # Safety
//!
//! The evaluator passes `&mut Vm` down its call chain. While a generator
//! is suspended, the frames on its coroutine stack still hold that
//! reference, and the resumer uses the same `Vm` through its own `&mut`.
//! The two never run concurrently (the coroutine only runs between
//! `resume` and `suspend`), the `Vm` pointer is passed across every switch,
//! and no reference derived from it is cached across a suspension point.
//! This is the same discipline as any interpreter built on stackful
//! coroutines; it is confined to this module and [`super::fiber`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::class::{ClassEntry, ClassRef, Function, class_flags, member_flags};
use crate::compile::ir::*;
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::value::{PhpRef, Value};
use crate::vm::{Flow, Frame, PResult, Vm};

use super::closure::ClosureData;
use super::eval::eval;
use super::{Ctx, Env, user_fn};

/// What a resumed `yield` receives.
pub enum Resume {
    Start,
    Send(Value),
    Throw(ObjRef),
    Destroy,
}

/// What a suspended body hands out.
pub struct Yielded {
    pub key: Value,
    pub value: Value,
}

type GenCoroutine = Coroutine<(usize, Resume), Yielded, PResult<Value>, DefaultStack>;

/// State shared between the generator object and its running body.
pub struct GenCore {
    pub key: Value,
    pub value: Value,
    pub largest_int_key: i64,
    pub finished: bool,
    pub running: bool,
    pub started: bool,
    /// Still at the first yield (rewind allowed).
    pub at_first_yield: bool,
    pub destroying: bool,
    pub return_value: Option<Value>,
    /// The body threw (getReturn() is then an error).
    pub threw: bool,
    pub returns_ref: bool,
}

/// Per-body context available to `yield` (via [`Ctx::generator`]).
pub struct GenCtx {
    yielder: *const Yielder<(usize, Resume), Yielded>,
    pub core: Rc<RefCell<GenCore>>,
    /// The VM pointer of the latest resume.
    vm: Cell<usize>,
}

/// Native payload of a `Generator` object.
pub struct GenObj {
    pub core: Rc<RefCell<GenCore>>,
    co: RefCell<Option<GenCoroutine>>,
    func: Rc<Function>,
    env: Rc<Env>,
    stack_limit: usize,
}

thread_local! {
    static STACK_POOL: RefCell<Vec<DefaultStack>> = const { RefCell::new(Vec::new()) };
}

const GEN_STACK_SIZE: usize = 16 << 20;

fn take_stack() -> DefaultStack {
    STACK_POOL
        .with(|p| p.borrow_mut().pop())
        .unwrap_or_else(|| DefaultStack::new(GEN_STACK_SIZE).expect("coroutine stack"))
}

fn return_stack(s: DefaultStack) {
    STACK_POOL.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() < 64 {
            p.push(s);
        }
    });
}

pub fn gen_obj(o: &ObjRef) -> Option<Rc<GenObj>> {
    o.native::<Rc<GenObj>>().map(|g| g.clone())
}

pub fn is_generator(o: &ObjRef) -> bool {
    o.native::<Rc<GenObj>>().is_some()
}

/// `ZEND_GENERATOR_CREATE`: wraps a bound function activation in a Generator.
pub fn create(vm: &mut Vm, func: Rc<Function>, env: Rc<Env>, closure: Option<Rc<ClosureData>>) -> PResult<Value> {
    let class = vm.find_class(b"Generator").expect("Generator registered");
    let ufn = user_fn(&func).expect("user function");
    let code = ufn.code.clone();
    let core = Rc::new(RefCell::new(GenCore {
        key: Value::Null,
        value: Value::Null,
        largest_int_key: -1,
        finished: false,
        running: false,
        started: false,
        at_first_yield: true,
        destroying: false,
        return_value: None,
        threw: false,
        returns_ref: code.returns_ref(),
    }));
    let body_core = core.clone();
    let body_env = env.clone();
    let body_func = func.clone();
    let _keep_closure = closure;
    let stack = take_stack();
    let limit = {
        use corosensei::stack::Stack;
        stack.limit().get() + 256 * 1024
    };
    let co: GenCoroutine = Coroutine::with_stack(stack, move |y: &Yielder<(usize, Resume), Yielded>, input: (usize, Resume)| {
        let _closure = _keep_closure;
        // SAFETY: see the module documentation; the pointer is the live `Vm`
        // of the resumer for the duration of this activation.
        let vm: &mut Vm = unsafe { &mut *(input.0 as *mut Vm) };
        if let Resume::Destroy = input.1 {
            return Ok(Value::Null);
        }
        let ufn = user_fn(&body_func).expect("user function");
        let gctx = GenCtx {
            yielder: y as *const _,
            core: body_core.clone(),
            vm: Cell::new(input.0),
        };
        let cx = Ctx {
            env: &body_env,
            code: &ufn.code,
            ufn,
            map: None,
            goto: Cell::new(None),
            generator: Some(&gctx),
        };
        let r = super::exec::exec_block(vm, &cx, &ufn.code.body);
        let vm: &mut Vm = unsafe { &mut *(gctx.vm.get() as *mut Vm) };
        let _ = &vm;
        match r {
            Ok(()) => Ok(Value::Null),
            Err(Flow::Return(v)) => Ok(if body_core.borrow().returns_ref { v } else { v.into_unref() }),
            Err(e) => Err(e),
        }
    });
    let obj = ObjRef::new(class);
    obj.set_native(Rc::new(GenObj {
        core,
        co: RefCell::new(Some(co)),
        func,
        env,
        stack_limit: limit,
    }));
    Ok(Value::Object(obj))
}

/// Resumes the generator body with `input`; updates key/value on yield.
fn resume(vm: &mut Vm, g: &Rc<GenObj>, input: Resume) -> PResult<()> {
    {
        let c = g.core.borrow();
        if c.finished {
            return Ok(());
        }
        if c.running {
            drop(c);
            return Err(vm.throw_error("Error", "Cannot resume an already running generator"));
        }
    }
    let Some(mut co) = g.co.borrow_mut().take() else {
        return Ok(());
    };
    {
        let mut c = g.core.borrow_mut();
        c.running = true;
        if c.started && !matches!(input, Resume::Destroy) {
            c.at_first_yield = false;
        }
        c.started = true;
    }
    // Frame for backtraces: the generator function, called from here.
    let ufn = user_fn(&g.func).expect("user function");
    let frame = Frame {
        function: Some(g.func.clone()),
        name: g.func.name.clone(),
        class: g.func.scope.clone(),
        call_type: if g.func.scope.is_some() {
            Some(if g.env.this.is_some() { "->" } else { "::" })
        } else {
            None
        },
        this: g.env.this.clone(),
        call_site: if vm.in_native_frame() {
            None
        } else {
            Some((vm.current_file.clone(), vm.current_line))
        },
        args: Vec::new(),
        is_internal: false,
        env: Some(g.env.clone()),
    };
    let saved_file = std::mem::replace(&mut vm.current_file, ufn.code.file.clone());
    let saved_line = vm.current_line;
    let saved_limit = vm.interp.stack_limit.get();
    vm.interp.stack_limit.set(g.stack_limit);
    vm.push_frame(frame);
    super::fiber::GEN_DEPTH.with(|d| d.set(d.get() + 1));
    let r = co.resume((vm as *mut Vm as usize, input));
    super::fiber::GEN_DEPTH.with(|d| d.set(d.get() - 1));
    vm.pop_frame();
    vm.interp.stack_limit.set(saved_limit);
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    let out = match r {
        CoroutineResult::Yield(y) => {
            let mut c = g.core.borrow_mut();
            c.running = false;
            c.key = y.key;
            c.value = y.value;
            drop(c);
            *g.co.borrow_mut() = Some(co);
            Ok(())
        }
        CoroutineResult::Return(res) => {
            return_stack(co.into_stack());
            let mut c = g.core.borrow_mut();
            c.running = false;
            c.finished = true;
            c.key = Value::Null;
            c.value = Value::Null;
            match res {
                Ok(v) => {
                    c.return_value = Some(v);
                    Ok(())
                }
                Err(e) => {
                    c.threw = true;
                    Err(e)
                }
            }
        }
    };
    out
}

/// `zend_generator_ensure_initialized`.
fn ensure_started(vm: &mut Vm, g: &Rc<GenObj>) -> PResult<()> {
    let (started, finished) = {
        let c = g.core.borrow();
        (c.started, c.finished)
    };
    if !started && !finished {
        resume(vm, g, Resume::Start)?;
        g.core.borrow_mut().at_first_yield = true;
    }
    Ok(())
}

/// `yield` (`ZEND_YIELD`).
pub fn eval_yield(vm: &mut Vm, cx: &Ctx, key: Option<&Expr>, value: Option<&Expr>, line: u32) -> PResult<Value> {
    let Some(g) = cx.generator else {
        return Err(vm.throw_error("Error", "Cannot yield outside a generator"));
    };
    if g.core.borrow().destroying {
        vm.current_line = line;
        return Err(vm.throw_error("Error", "Cannot yield from finally in a force-closed generator"));
    }
    let v = match value {
        Some(e) => {
            let v = eval(vm, cx, e)?;
            if g.core.borrow().returns_ref {
                match v {
                    Value::Ref(r) => Value::Ref(r),
                    v => {
                        if !matches!(e, Expr::MakeRef(..)) {
                            vm.current_line = line;
                            vm.engine_notice("Only variable references should be yielded by reference")?;
                        }
                        Value::Ref(PhpRef::new(v))
                    }
                }
            } else {
                v.into_unref()
            }
        }
        None => Value::Null,
    };
    let k = match key {
        Some(e) => {
            let k = eval(vm, cx, e)?.into_unref();
            if let Value::Int(i) = k {
                let mut c = g.core.borrow_mut();
                if i > c.largest_int_key {
                    c.largest_int_key = i;
                }
            }
            k
        }
        None => {
            let mut c = g.core.borrow_mut();
            c.largest_int_key += 1;
            Value::Int(c.largest_int_key)
        }
    };
    suspend(vm, g, k, v)
}

fn suspend(vm: &mut Vm, g: &GenCtx, key: Value, value: Value) -> PResult<Value> {
    let _ = vm;
    // SAFETY: the yielder outlives the body activation that owns `g`.
    let y = unsafe { &*g.yielder };
    let (ptr, input) = y.suspend(Yielded { key, value });
    g.vm.set(ptr);
    match input {
        Resume::Start | Resume::Send(_) if false => unreachable!(),
        Resume::Send(v) => Ok(v),
        Resume::Start => Ok(Value::Null),
        Resume::Throw(e) => Err(Flow::Exception(e)),
        Resume::Destroy => {
            g.core.borrow_mut().destroying = true;
            Err(Flow::Return(Value::Null))
        }
    }
}

/// `yield from` (`ZEND_YIELD_FROM`).
pub fn eval_yield_from(vm: &mut Vm, cx: &Ctx, e: &Expr, line: u32) -> PResult<Value> {
    let Some(g) = cx.generator else {
        return Err(vm.throw_error("Error", "Cannot use \"yield from\" outside a generator"));
    };
    let v = eval(vm, cx, e)?;
    vm.current_line = line;
    match v.into_unref() {
        Value::Array(a) => {
            for (k, v) in a.iter() {
                if let crate::value::ArrayKey::Int(i) = k {
                    let _ = i;
                }
                let r = suspend(vm, g, k.to_value(), v.unref())?;
                drop(r);
            }
            Ok(Value::Null)
        }
        Value::Object(o) => {
            if let Some(inner) = gen_obj(&o) {
                return delegate(vm, g, &o, &inner);
            }
            if !o.class().instance_of_name(b"Traversable") {
                return Err(vm.throw_error("Error", "Can use \"yield from\" only with arrays and Traversables"));
            }
            let mut items: Vec<(Value, Value)> = Vec::new();
            // Iterate lazily: yield each element as it is produced.
            let mut err = None;
            super::exec::iterate_object(vm, &o, false, &mut |vm, k, v| {
                match suspend(vm, g, k, v) {
                    Ok(_) => Ok(true),
                    Err(e) => {
                        err = Some(e);
                        Ok(false)
                    }
                }
            })?;
            items.clear();
            match err {
                Some(e) => Err(e),
                None => Ok(Value::Null),
            }
        }
        _ => Err(vm.throw_error("Error", "Can use \"yield from\" only with arrays and Traversables")),
    }
}

/// Delegation to an inner generator.
fn delegate(vm: &mut Vm, g: &GenCtx, inner_obj: &ObjRef, inner: &Rc<GenObj>) -> PResult<Value> {
    if Rc::ptr_eq(&inner.core, &g.core) {
        return Err(vm.throw_error("Error", "Impossible to yield from the Generator being currently run"));
    }
    if inner.core.borrow().finished {
        if inner.core.borrow().threw {
            return Err(vm.throw_error("Exception", "Generator yielded from aborted, no return value available"));
        }
        return Ok(inner.core.borrow().return_value.clone().unwrap_or(Value::Null));
    }
    ensure_started(vm, inner)?;
    let _ = inner_obj;
    loop {
        if inner.core.borrow().finished {
            break;
        }
        let (k, v) = {
            let c = inner.core.borrow();
            (c.key.clone(), c.value.clone())
        };
        // Pass the inner yield through; the outer key mirrors the inner one.
        let y = unsafe { &*g.yielder };
        let (ptr, input) = y.suspend(Yielded { key: k, value: v });
        g.vm.set(ptr);
        let vm: &mut Vm = unsafe { &mut *(ptr as *mut Vm) };
        match input {
            Resume::Send(v) => resume(vm, inner, Resume::Send(v))?,
            Resume::Start => resume(vm, inner, Resume::Send(Value::Null))?,
            Resume::Throw(e) => resume(vm, inner, Resume::Throw(e))?,
            Resume::Destroy => {
                g.core.borrow_mut().destroying = true;
                return Err(Flow::Return(Value::Null));
            }
        }
    }
    Ok(inner.core.borrow().return_value.clone().unwrap_or(Value::Null))
}

/// `foreach` over a generator.
pub fn iterate(vm: &mut Vm, o: &ObjRef, by_ref: bool, f: &mut dyn FnMut(&mut Vm, Value, Value) -> PResult<bool>) -> PResult<()> {
    let g = gen_obj(o).expect("generator");
    {
        let c = g.core.borrow();
        if c.finished && c.started {
            drop(c);
            return Err(vm.throw_error("Exception", "Cannot traverse an already closed generator"));
        }
        if by_ref && !c.returns_ref {
            drop(c);
            return Err(vm.throw_error(
                "Exception",
                "You can only iterate a generator by-reference if it declared that it yields by-reference",
            ));
        }
    }
    rewind(vm, &g)?;
    loop {
        let (finished, k, v) = {
            let c = g.core.borrow();
            (c.finished, c.key.clone(), c.value.clone())
        };
        if finished {
            break;
        }
        let v = if by_ref { v } else { v.into_unref() };
        if !f(vm, k, v)? {
            return Ok(());
        }
        resume(vm, &g, Resume::Send(Value::Null))?;
    }
    Ok(())
}

fn rewind(vm: &mut Vm, g: &Rc<GenObj>) -> PResult<()> {
    ensure_started(vm, g)?;
    let c = g.core.borrow();
    if !c.at_first_yield {
        drop(c);
        return Err(vm.throw_error("Exception", "Cannot rewind a generator that was already run"));
    }
    Ok(())
}

/// Destroys a suspended generator, running its `finally` blocks.
fn destroy(vm: &mut Vm, o: &ObjRef) -> PResult<()> {
    let Some(g) = gen_obj(o) else { return Ok(()) };
    let (started, finished, running) = {
        let c = g.core.borrow();
        (c.started, c.finished, c.running)
    };
    if finished || running {
        return Ok(());
    }
    if !started {
        // Never ran: nothing to clean up.
        if let Some(co) = g.co.borrow_mut().take() {
            drop(co);
        }
        g.core.borrow_mut().finished = true;
        return Ok(());
    }
    let r = resume(vm, &g, Resume::Destroy);
    if let Some(co) = g.co.borrow_mut().take() {
        // Yielded again inside finally (error already reported): unwind.
        drop(co);
    }
    g.core.borrow_mut().finished = true;
    r
}

// ---------------------------------------------------------------------------
// Generator methods
// ---------------------------------------------------------------------------

fn this_gen(vm: &mut Vm, args: &Args) -> PResult<Rc<GenObj>> {
    match args.this().and_then(gen_obj) {
        Some(g) => Ok(g),
        None => Err(vm.throw_error("Error", "Generator method called on a non-generator")),
    }
}

fn g_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_started(vm, &g)?;
    let c = g.core.borrow();
    Ok(if c.finished { Value::Null } else { c.value.unref() })
}

fn g_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_started(vm, &g)?;
    let c = g.core.borrow();
    Ok(if c.finished { Value::Null } else { c.key.clone() })
}

fn g_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    let started = g.core.borrow().started;
    ensure_started(vm, &g)?;
    if started {
        resume(vm, &g, Resume::Send(Value::Null))?;
    } else {
        resume(vm, &g, Resume::Send(Value::Null))?;
    }
    Ok(Value::Null)
}

fn g_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_started(vm, &g)?;
    Ok(Value::Bool(!g.core.borrow().finished))
}

fn g_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    rewind(vm, &g)?;
    Ok(Value::Null)
}

fn g_send(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    let v = args.get(0).unref();
    // Run to the first yield; the sent value becomes its result.
    if !g.core.borrow().started {
        ensure_started(vm, &g)?;
    }
    if g.core.borrow().finished {
        return Ok(Value::Null);
    }
    resume(vm, &g, Resume::Send(v))?;
    let c = g.core.borrow();
    Ok(if c.finished { Value::Null } else { c.value.unref() })
}

fn g_throw(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    let ex = args.object(0).clone();
    ensure_started(vm, &g)?;
    if g.core.borrow().finished {
        return Err(Flow::Exception(ex));
    }
    resume(vm, &g, Resume::Throw(ex))?;
    let c = g.core.borrow();
    Ok(if c.finished { Value::Null } else { c.value.unref() })
}

fn g_get_return(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_started(vm, &g)?;
    let c = g.core.borrow();
    if c.finished && !c.threw {
        if let Some(v) = &c.return_value {
            return Ok(v.unref());
        }
    }
    let threw = c.threw;
    drop(c);
    if threw {
        return Err(vm.throw_error("Exception", "Cannot get return value of a generator that hasn't returned"));
    }
    Err(vm.throw_error("Exception", "Cannot get return value of a generator that hasn't returned"))
}

fn g_wakeup(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "Unserialization of 'Generator' is not allowed"))
}

static CURRENT: NativeFunction = NativeFunction { sig: "public function current(): mixed {}", handler: g_current };
static KEY: NativeFunction = NativeFunction { sig: "public function key(): mixed {}", handler: g_key };
static NEXT: NativeFunction = NativeFunction { sig: "public function next(): void {}", handler: g_next };
static VALID: NativeFunction = NativeFunction { sig: "public function valid(): bool {}", handler: g_valid };
static REWIND: NativeFunction = NativeFunction { sig: "public function rewind(): void {}", handler: g_rewind };
static SEND: NativeFunction = NativeFunction { sig: "public function send(mixed $value): mixed {}", handler: g_send };
static THROW: NativeFunction = NativeFunction { sig: "public function throw(Throwable $exception): mixed {}", handler: g_throw };
static GET_RETURN: NativeFunction = NativeFunction { sig: "public function getReturn(): mixed {}", handler: g_get_return };
static WAKEUP: NativeFunction = NativeFunction { sig: "public function __wakeup(): void {}", handler: g_wakeup };

fn gen_dtor(vm: &mut Vm, o: &ObjRef) -> PResult<()> {
    destroy(vm, o)
}

/// Registers the `Generator` class.
pub fn register(r: &mut Registry<'_>) {
    let iterator: ClassRef = r.find_class("Iterator").expect("Iterator registered");
    let mut ce = ClassEntry::new(
        "Generator",
        class_flags::FINAL | class_flags::NO_DYNAMIC_INSTANTIATION | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES,
    );
    ce.interfaces = iterator.interfaces.clone();
    ce.interfaces.push(iterator);
    for nf in [&CURRENT, &KEY, &NEXT, &VALID, &REWIND, &SEND, &THROW, &GET_RETURN, &WAKEUP] {
        ce.add_method(Function::native_method(nf, "Generator", member_flags::PUBLIC));
    }
    ce.handlers.dtor_obj = Some(gen_dtor);
    r.class(ce);
}
