//! Fibers (`Zend/zend_fibers.c`) on stackful coroutines. See
//! [`super::generator`] for the safety discipline of switching with a
//! shared `Vm`.
//!
//! Each switch saves and restores the per-context executor state that
//! php-src keeps in `zend_fiber_vm_state`: the frames, the current
//! position, the stack limit and `error_reporting`. A fiber may suspend
//! while a generator runs inside it; its logical stack then spans the
//! generator's coroutine stack, and the suspension point restores that
//! generator's stack limit when the fiber comes back.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::class::{ClassEntry, Function, class_flags, member_flags};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::{ObjRef, WeakObjRef};
use crate::value::Value;
use crate::vm::{Flow, PResult, Vm};

use super::UNWIND;

enum FiberIn {
    Start(Vec<Value>),
    Resume(Value),
    Throw(ObjRef),
    /// Destruction of a suspended fiber (php-src's graceful exit): unwinds
    /// the fiber, running `finally` blocks but no `catch` blocks.
    Destroy,
}

type FiberCo = Coroutine<(usize, FiberIn), Value, PResult<Value>, DefaultStack>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Init,
    Running,
    Suspended,
    Terminated,
}

pub struct FiberObj {
    /// The callback (`fci`); released when the fiber finishes.
    callable: RefCell<Option<Value>>,
    co: RefCell<Option<FiberCo>>,
    status: Cell<Status>,
    ret: RefCell<Option<Value>>,
    /// `ZEND_FIBER_FLAG_THREW`.
    threw: Cell<bool>,
    /// `ZEND_FIBER_FLAG_BAILOUT`.
    bailout: Cell<bool>,
    /// `ZEND_FIBER_FLAG_DESTROYED`.
    destroying: Cell<bool>,
    stack_limit: Cell<usize>,
    /// Frames of the fiber while it is suspended.
    saved_frames: RefCell<Vec<crate::vm::Frame>>,
}

/// An entry of the active fiber stack (`EG(active_fiber)` and the fibers it
/// was resumed from).
struct Running {
    fiber: Rc<FiberObj>,
    obj: WeakObjRef,
    yielder: *const Yielder<(usize, FiberIn), Value>,
}

thread_local! {
    static CURRENT: RefCell<Vec<Running>> = const { RefCell::new(Vec::new()) };
    /// `zend_fiber_switch_blocking`.
    static SWITCH_BLOCKING: Cell<u32> = const { Cell::new(0) };
}

/// `zend_fiber_switch_block`: fibers cannot switch until the matching
/// [`unblock_switch`] (tick functions run like this).
pub fn block_switch() {
    SWITCH_BLOCKING.with(|b| b.set(b.get() + 1));
}

/// `zend_fiber_switch_unblock`.
pub fn unblock_switch() {
    SWITCH_BLOCKING.with(|b| b.set(b.get().saturating_sub(1)));
}

fn switch_blocked() -> bool {
    SWITCH_BLOCKING.with(Cell::get) != 0
}

/// Whether code is running inside a fiber (`EG(active_fiber) != NULL`).
pub fn in_fiber() -> bool {
    CURRENT.with(|c| !c.borrow().is_empty())
}

fn fiber_of(o: &ObjRef) -> Option<Rc<FiberObj>> {
    o.native::<Rc<FiberObj>>().map(|f| f.clone())
}

fn fiber_error(vm: &mut Vm, msg: &str) -> Flow {
    vm.throw_error("FiberError", msg)
}

fn this_fiber(vm: &mut Vm, args: &Args) -> PResult<(ObjRef, Rc<FiberObj>)> {
    let o = args.this().cloned().ok_or_else(|| vm.throw_error("Error", "Fiber method without $this"))?;
    let f = match fiber_of(&o) {
        Some(f) => f,
        None => {
            let f = new_fiber_obj();
            o.set_native(f.clone());
            f
        }
    };
    Ok((o, f))
}

fn new_fiber_obj() -> Rc<FiberObj> {
    Rc::new(FiberObj {
        callable: RefCell::new(None),
        co: RefCell::new(None),
        status: Cell::new(Status::Init),
        ret: RefCell::new(None),
        threw: Cell::new(false),
        bailout: Cell::new(false),
        destroying: Cell::new(false),
        stack_limit: Cell::new(0),
        saved_frames: RefCell::new(Vec::new()),
    })
}

fn fiber_create(_vm: &mut Vm, class: &crate::class::ClassRef) -> PResult<ObjRef> {
    let o = ObjRef::new(class.clone());
    o.set_native(new_fiber_obj());
    Ok(o)
}

fn f_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    if f.status.get() != Status::Init || f.callable.borrow().is_some() {
        return Err(fiber_error(vm, "Cannot call constructor twice"));
    }
    *f.callable.borrow_mut() = Some(args.get(0).clone());
    Ok(Value::Null)
}

/// The `error_reporting` ini value a new fiber starts with (the silence
/// operator only changes `EG(error_reporting)`).
fn ini_error_reporting(vm: &Vm) -> i64 {
    match vm.ini.value(b"error_reporting") {
        None => crate::errors::level::E_ALL,
        Some(v) => crate::ini::atoi(&v),
    }
}

/// `zend_fiber_switch_to` from the resumer's side.
fn switch_to(vm: &mut Vm, f: &Rc<FiberObj>, input: FiberIn) -> PResult<Value> {
    let Some(mut co) = f.co.borrow_mut().take() else {
        return Ok(Value::Null);
    };
    f.status.set(Status::Running);
    let saved_limit = vm.interp.stack_limit.get();
    vm.interp.stack_limit.set(f.stack_limit.get());
    let saved_reporting = vm.errors.reporting;
    let base = vm.frames.len();
    // Restore the fiber's own frames on top of the resumer's.
    let frames = std::mem::take(&mut *f.saved_frames.borrow_mut());
    vm.frames.extend(frames);
    let saved_file = vm.current_file.clone();
    let saved_line = vm.current_line;
    let r = co.resume((vm as *mut Vm as usize, input));
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    vm.interp.stack_limit.set(saved_limit);
    vm.errors.reporting = saved_reporting;
    match r {
        CoroutineResult::Yield(v) => {
            let fr: Vec<_> = vm.frames.drain(base..).collect();
            *f.saved_frames.borrow_mut() = fr;
            f.status.set(Status::Suspended);
            *f.co.borrow_mut() = Some(co);
            Ok(v)
        }
        CoroutineResult::Return(res) => {
            vm.frames.truncate(base);
            f.status.set(Status::Terminated);
            drop(co);
            match res {
                Ok(v) => {
                    *f.ret.borrow_mut() = Some(v);
                    Ok(Value::Null)
                }
                // The graceful exit of a destroyed fiber is not an error.
                Err(Flow::Break(UNWIND)) if f.destroying.get() => Ok(Value::Null),
                Err(e) => {
                    match e {
                        Flow::Exception(_) => f.threw.set(true),
                        Flow::Exit(_) | Flow::Fatal => f.bailout.set(true),
                        _ => {}
                    }
                    Err(e)
                }
            }
        }
    }
}

/// Page size used for php-src's minimum fiber stack size.
const PAGE_SIZE: usize = if cfg!(all(target_os = "macos", target_arch = "aarch64")) { 16384 } else { 4096 };
/// `ZEND_FIBER_DEFAULT_C_STACK_SIZE`.
const DEFAULT_STACK_SIZE: usize = 4096 * 512;

fn f_start(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, f) = this_fiber(vm, args)?;
    let start_args: Vec<Value> = args.rest(0).to_vec();
    if switch_blocked() {
        return Err(fiber_error(vm, "Cannot switch fibers in current execution context"));
    }
    if f.status.get() != Status::Init {
        return Err(fiber_error(vm, "Cannot start a fiber that has already been started"));
    }
    // EG(fiber_stack_size): the ini value, or the default when unset.
    let size = match vm.ini.get(b"fiber.stack_size").and_then(|e| e.value.clone()) {
        None => DEFAULT_STACK_SIZE,
        Some(v) => crate::ini::atol_quantity(&v).max(0) as usize,
    };
    // One usable page plus the guard page (ZEND_FIBER_GUARD_PAGES).
    let minimum = 2 * PAGE_SIZE;
    if size < minimum {
        return Err(vm.throw_error(
            "Exception",
            format!("Fiber stack size is too small, it needs to be at least {minimum} bytes"),
        ));
    }
    // The interpreter needs more native stack per PHP call than php-src.
    let stack = DefaultStack::new(size.max(16 << 20) * 4).map_err(|_| vm.throw_error("Exception", "Fiber stack allocate failed"))?;
    {
        use corosensei::stack::Stack;
        f.stack_limit.set(stack.limit().get() + 256 * 1024);
    }
    let Some(callable) = f.callable.borrow().clone() else {
        return Err(vm.throw_error("Error", "Fiber has not been constructed"));
    };
    let fiber_rc = f.clone();
    let weak_obj = obj.downgrade();
    drop(obj);
    let co: FiberCo = Coroutine::with_stack(stack, move |y: &Yielder<(usize, FiberIn), Value>, input: (usize, FiberIn)| {
        // SAFETY: see generator.rs; the pointer is the resumer's live `Vm`.
        let vm: &mut Vm = unsafe { &mut *(input.0 as *mut Vm) };
        let FiberIn::Start(args) = input.1 else {
            return Ok(Value::Null);
        };
        // zend_fiber_execute: a fiber starts with the ini error_reporting.
        vm.errors.reporting = ini_error_reporting(vm);
        CURRENT.with(|c| {
            c.borrow_mut().push(Running {
                fiber: fiber_rc.clone(),
                obj: weak_obj,
                yielder: y as *const _,
            })
        });
        let r = vm.call(&callable, args);
        // zend_fiber_release_callable.
        drop(callable);
        *fiber_rc.callable.borrow_mut() = None;
        let r = super::generator::run_queued_destructors(vm, r);
        CURRENT.with(|c| {
            c.borrow_mut().pop();
        });
        r
    });
    *f.co.borrow_mut() = Some(co);
    switch_to(vm, &f, FiberIn::Start(start_args))
}

fn f_resume(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    let v = args.get(0).clone();
    if switch_blocked() {
        return Err(fiber_error(vm, "Cannot switch fibers in current execution context"));
    }
    if f.status.get() != Status::Suspended {
        return Err(fiber_error(vm, "Cannot resume a fiber that is not suspended"));
    }
    switch_to(vm, &f, FiberIn::Resume(v))
}

fn f_throw(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    let e = args.object(0).clone();
    if switch_blocked() {
        return Err(fiber_error(vm, "Cannot switch fibers in current execution context"));
    }
    if f.status.get() != Status::Suspended {
        return Err(fiber_error(vm, "Cannot resume a fiber that is not suspended"));
    }
    switch_to(vm, &f, FiberIn::Throw(e))
}

fn f_get_return(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    let message = match f.status.get() {
        Status::Terminated => {
            if f.threw.get() {
                "The fiber threw an exception"
            } else if f.bailout.get() {
                "The fiber exited with a fatal error"
            } else {
                return Ok(f.ret.borrow().clone().map(|v| v.unref()).unwrap_or(Value::Null));
            }
        }
        Status::Init => "The fiber has not been started",
        _ => "The fiber has not returned",
    };
    Err(fiber_error(vm, &format!("Cannot get fiber return value: {message}")))
}

fn f_is_started(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    Ok(Value::Bool(f.status.get() != Status::Init))
}

fn f_is_suspended(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    Ok(Value::Bool(f.status.get() == Status::Suspended))
}

fn f_is_running(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    Ok(Value::Bool(f.status.get() == Status::Running))
}

fn f_is_terminated(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    Ok(Value::Bool(f.status.get() == Status::Terminated))
}

fn f_suspend(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).clone();
    let cur = CURRENT.with(|c| c.borrow().last().map(|r| (r.fiber.clone(), r.yielder)));
    let Some((fiber, yielder)) = cur else {
        return Err(fiber_error(vm, "Cannot suspend outside of a fiber"));
    };
    if fiber.destroying.get() {
        return Err(fiber_error(vm, "Cannot suspend in a force-closed fiber"));
    }
    if switch_blocked() {
        return Err(fiber_error(vm, "Cannot switch fibers in current execution context"));
    }
    // The current fiber is no longer "current" while suspended.
    let running = CURRENT.with(|c| c.borrow_mut().pop()).expect("running fiber");
    // The fiber may be suspended from a generator running inside it: its
    // stack (and stack limit) is the one the fiber comes back to.
    let saved_limit = vm.interp.stack_limit.get();
    let saved_file = vm.current_file.clone();
    let saved_line = vm.current_line;
    let saved_reporting = vm.errors.reporting;
    drop(fiber);
    // SAFETY: the yielder belongs to the fiber coroutine whose logical stack
    // we are running on (possibly through nested generator stacks).
    let y = unsafe { &*yielder };
    let (ptr, input) = y.suspend(v);
    CURRENT.with(|c| c.borrow_mut().push(running));
    let vm: &mut Vm = unsafe { &mut *(ptr as *mut Vm) };
    vm.interp.stack_limit.set(saved_limit);
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    vm.errors.reporting = saved_reporting;
    match input {
        FiberIn::Resume(v) => Ok(v),
        FiberIn::Throw(e) => Err(Flow::Exception(e)),
        FiberIn::Start(_) => Ok(Value::Null),
        FiberIn::Destroy => Err(Flow::Break(UNWIND)),
    }
}

fn f_get_current(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(CURRENT.with(|c| {
        c.borrow()
            .last()
            .and_then(|r| r.obj.upgrade())
            .map(Value::Object)
            .unwrap_or(Value::Null)
    }))
}

fn fiber_error_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.this().map(|o| o.class().name.to_string()).unwrap_or_else(|| "FiberError".into());
    Err(vm.throw_error(
        "Error",
        format!("The \"{name}\" class is reserved for internal use and cannot be manually instantiated"),
    ))
}

/// `zend_fiber_object_destroy`: a suspended fiber is resumed with a
/// graceful exit, unwinding it (`finally` blocks run).
fn fiber_dtor(vm: &mut Vm, o: &ObjRef) -> PResult<()> {
    let Some(f) = fiber_of(o) else { return Ok(()) };
    if f.status.get() != Status::Suspended {
        return Ok(());
    }
    f.destroying.set(true);
    let r = switch_to(vm, &f, FiberIn::Destroy);
    if let Some(co) = f.co.borrow_mut().take() {
        // Suspended again although force-closed (cannot happen: suspending
        // fails); the coroutine is unwound when dropped.
        drop(co);
    }
    f.status.set(Status::Terminated);
    r.map(|_| ())
}

static CONSTRUCT: NativeFunction = NativeFunction { sig: "public function __construct(callable $callback) {}", handler: f_construct };
static START: NativeFunction = NativeFunction { sig: "public function start(mixed ...$args): mixed {}", handler: f_start };
static RESUME: NativeFunction = NativeFunction { sig: "public function resume(mixed $value = null): mixed {}", handler: f_resume };
static THROW: NativeFunction = NativeFunction { sig: "public function throw(Throwable $exception): mixed {}", handler: f_throw };
static IS_STARTED: NativeFunction = NativeFunction { sig: "public function isStarted(): bool {}", handler: f_is_started };
static IS_SUSPENDED: NativeFunction = NativeFunction { sig: "public function isSuspended(): bool {}", handler: f_is_suspended };
static IS_RUNNING: NativeFunction = NativeFunction { sig: "public function isRunning(): bool {}", handler: f_is_running };
static IS_TERMINATED: NativeFunction = NativeFunction { sig: "public function isTerminated(): bool {}", handler: f_is_terminated };
static GET_RETURN: NativeFunction = NativeFunction { sig: "public function getReturn(): mixed {}", handler: f_get_return };
static GET_CURRENT: NativeFunction = NativeFunction { sig: "public static function getCurrent(): ?Fiber {}", handler: f_get_current };
static SUSPEND: NativeFunction = NativeFunction { sig: "public static function suspend(mixed $value = null): mixed {}", handler: f_suspend };
static FE_CONSTRUCT: NativeFunction = NativeFunction { sig: "public function __construct() {}", handler: fiber_error_construct };

/// Registers `Fiber` and `FiberError`.
pub fn register(r: &mut Registry<'_>) {
    let mut ce = ClassEntry::new("Fiber", class_flags::FINAL | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES);
    for nf in [&CONSTRUCT, &START, &RESUME, &THROW, &IS_STARTED, &IS_SUSPENDED, &IS_RUNNING, &IS_TERMINATED, &GET_RETURN] {
        ce.add_method(Function::native_method(nf, "Fiber", member_flags::PUBLIC));
    }
    for nf in [&GET_CURRENT, &SUSPEND] {
        ce.add_method(Function::native_method(nf, "Fiber", member_flags::PUBLIC | member_flags::STATIC));
    }
    ce.handlers.create_object = Some(fiber_create);
    ce.handlers.dtor_obj = Some(fiber_dtor);
    r.class(ce);
    let error = r.find_class("Error").expect("Error registered");
    let mut fe = ClassEntry::new_subclass("FiberError", class_flags::FINAL, &error);
    fe.add_method(Function::native_method(&FE_CONSTRUCT, "FiberError", member_flags::PUBLIC));
    r.class(fe);
}
