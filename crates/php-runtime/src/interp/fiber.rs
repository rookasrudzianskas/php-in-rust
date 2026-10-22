//! Fibers (`Zend/zend_fibers.c`) on stackful coroutines. See
//! [`super::generator`] for the safety discipline of switching with a
//! shared `Vm`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::class::{ClassEntry, Function, class_flags, member_flags};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::value::Value;
use crate::vm::{Flow, PResult, Vm};

enum FiberIn {
    Start(Vec<Value>),
    Resume(Value),
    Throw(ObjRef),
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
    callable: Value,
    co: RefCell<Option<FiberCo>>,
    status: Cell<Status>,
    ret: RefCell<Option<Value>>,
    threw: Cell<bool>,
    destroying: Cell<bool>,
    stack_limit: Cell<usize>,
    /// Frames of the fiber while it is suspended.
    saved_frames: RefCell<Vec<crate::vm::Frame>>,
}

struct Running {
    fiber: Rc<FiberObj>,
    obj: ObjRef,
    yielder: *const Yielder<(usize, FiberIn), Value>,
    /// Generator nesting level when the fiber body started.
    depth: usize,
}

thread_local! {
    static CURRENT: RefCell<Vec<Running>> = const { RefCell::new(Vec::new()) };
    /// Nesting of generator coroutines (fibers cannot switch from inside one).
    pub(crate) static GEN_DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn fiber_of(o: &ObjRef) -> Option<Rc<FiberObj>> {
    o.native::<Rc<FiberObj>>().map(|f| f.clone())
}

fn fiber_error(vm: &mut Vm, msg: &str) -> Flow {
    vm.throw_error("FiberError", msg)
}

fn this_fiber(vm: &mut Vm, args: &Args) -> PResult<(ObjRef, Rc<FiberObj>)> {
    let o = args.this().cloned().ok_or_else(|| vm.throw_error("Error", "Fiber method without $this"))?;
    match fiber_of(&o) {
        Some(f) => Ok((o, f)),
        None => Err(vm.throw_error("Error", "Fiber has not been constructed")),
    }
}

fn f_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = args.this().cloned().expect("this");
    if fiber_of(&o).is_some() {
        return Err(vm.throw_error("Error", "Cannot call constructor twice"));
    }
    o.set_native(Rc::new(FiberObj {
        callable: args.get(0).clone(),
        co: RefCell::new(None),
        status: Cell::new(Status::Init),
        ret: RefCell::new(None),
        threw: Cell::new(false),
        destroying: Cell::new(false),
        stack_limit: Cell::new(0),
        saved_frames: RefCell::new(Vec::new()),
    }));
    Ok(Value::Null)
}

fn switch_to(vm: &mut Vm, obj: &ObjRef, f: &Rc<FiberObj>, input: FiberIn) -> PResult<Value> {
    let Some(mut co) = f.co.borrow_mut().take() else {
        return Ok(Value::Null);
    };
    f.status.set(Status::Running);
    let saved_limit = vm.interp.stack_limit.get();
    vm.interp.stack_limit.set(f.stack_limit.get());
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
    let _ = obj;
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
            match res {
                Ok(v) => {
                    *f.ret.borrow_mut() = Some(v);
                    Ok(Value::Null)
                }
                Err(e) => {
                    if matches!(e, Flow::Exception(_)) {
                        f.threw.set(true);
                    }
                    Err(e)
                }
            }
        }
    }
}

fn f_start(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, f) = this_fiber(vm, args)?;
    if f.status.get() != Status::Init {
        return Err(fiber_error(vm, "Cannot start a fiber that has already been started"));
    }
    let start_args: Vec<Value> = args.rest(0).to_vec();
    let callable = f.callable.clone();
    let fiber_rc = f.clone();
    let obj2 = obj.clone();
    let size = {
        let s = vm.ini_int("fiber.stack_size");
        if s > 0 { (s as usize).max(64 * 1024) } else { 16 << 20 }
    };
    let stack = DefaultStack::new(size.max(1 << 20) * 4).map_err(|_| fiber_error(vm, "Fiber stack allocate failed"))?;
    {
        use corosensei::stack::Stack;
        f.stack_limit.set(stack.limit().get() + 256 * 1024);
    }
    let co: FiberCo = Coroutine::with_stack(stack, move |y: &Yielder<(usize, FiberIn), Value>, input: (usize, FiberIn)| {
        // SAFETY: see generator.rs; the pointer is the resumer's live `Vm`.
        let vm: &mut Vm = unsafe { &mut *(input.0 as *mut Vm) };
        let FiberIn::Start(args) = input.1 else {
            return Ok(Value::Null);
        };
        CURRENT.with(|c| {
            c.borrow_mut().push(Running {
                fiber: fiber_rc.clone(),
                obj: obj2.clone(),
                yielder: y as *const _,
                depth: GEN_DEPTH.with(Cell::get),
            })
        });
        let r = vm.call(&callable, args);
        CURRENT.with(|c| {
            c.borrow_mut().pop();
        });
        r
    });
    *f.co.borrow_mut() = Some(co);
    let r = switch_to(vm, &obj, &f, FiberIn::Start(start_args));
    r
}

fn f_resume(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, f) = this_fiber(vm, args)?;
    if f.status.get() != Status::Suspended {
        return Err(fiber_error(vm, "Cannot resume a fiber that is not suspended"));
    }
    let v = args.get(0).clone();
    switch_to(vm, &obj, &f, FiberIn::Resume(v))
}

fn f_throw(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, f) = this_fiber(vm, args)?;
    if f.status.get() != Status::Suspended {
        return Err(fiber_error(vm, "Cannot resume a fiber that is not suspended"));
    }
    let e = args.object(0).clone();
    switch_to(vm, &obj, &f, FiberIn::Throw(e))
}

fn f_get_return(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, f) = this_fiber(vm, args)?;
    match f.status.get() {
        Status::Terminated => {
            if f.threw.get() {
                return Err(fiber_error(vm, "Cannot get fiber return value: The fiber threw an exception"));
            }
            Ok(f.ret.borrow().clone().unwrap_or(Value::Null))
        }
        Status::Init => Err(fiber_error(vm, "Cannot get fiber return value: The fiber has not been started")),
        _ => Err(fiber_error(vm, "Cannot get fiber return value: The fiber has not returned")),
    }
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
    let cur = CURRENT.with(|c| c.borrow().last().map(|r| (r.fiber.clone(), r.yielder, r.depth)));
    let Some((fiber, yielder, depth)) = cur else {
        return Err(fiber_error(vm, "Cannot suspend outside of fiber"));
    };
    if fiber.destroying.get() {
        return Err(fiber_error(vm, "Cannot suspend in a force-closed fiber"));
    }
    if GEN_DEPTH.with(Cell::get) != depth {
        return Err(fiber_error(vm, "Cannot switch fibers in current execution context"));
    }
    // The current fiber is no longer "current" while suspended.
    let running = CURRENT.with(|c| c.borrow_mut().pop()).expect("running fiber");
    // SAFETY: the yielder belongs to the fiber coroutine we are running on.
    let y = unsafe { &*yielder };
    let (ptr, input) = y.suspend(v);
    CURRENT.with(|c| c.borrow_mut().push(running));
    let vm: &mut Vm = unsafe { &mut *(ptr as *mut Vm) };
    match input {
        FiberIn::Resume(v) => Ok(v),
        FiberIn::Throw(e) => Err(Flow::Exception(e)),
        FiberIn::Start(_) => Ok(Value::Null),
        FiberIn::Destroy => {
            fiber.destroying.set(true);
            let _ = vm;
            Err(Flow::Break(super::UNWIND))
        }
    }
}

fn f_get_current(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(CURRENT.with(|c| c.borrow().last().map(|r| Value::Object(r.obj.clone())).unwrap_or(Value::Null)))
}

fn fiber_error_construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "Error",
        "The \"FiberError\" class is reserved for internal use and cannot be manually instantiated",
    ))
}

fn fiber_dtor(vm: &mut Vm, o: &ObjRef) -> PResult<()> {
    let Some(f) = fiber_of(o) else { return Ok(()) };
    if f.status.get() == Status::Suspended {
        // Unwind the suspended fiber; `finally` blocks run (exit-like unwind).
        let r = switch_to(vm, o, &f, FiberIn::Destroy);
        if let Some(co) = f.co.borrow_mut().take() {
            drop(co);
        }
        f.status.set(Status::Terminated);
        match r {
            Err(Flow::Break(super::UNWIND)) => return Ok(()),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
    }
    Ok(())
}

static CONSTRUCT: NativeFunction = NativeFunction { sig: "public function __construct(callable $callback) {}", handler: f_construct };
static START: NativeFunction = NativeFunction { sig: "public function start(mixed ...$args): mixed {}", handler: f_start };
static RESUME: NativeFunction = NativeFunction { sig: "public function resume(mixed $value = null): mixed {}", handler: f_resume };
static THROW: NativeFunction = NativeFunction { sig: "public function throw(Throwable $exception): mixed {}", handler: f_throw };
static GET_RETURN: NativeFunction = NativeFunction { sig: "public function getReturn(): mixed {}", handler: f_get_return };
static IS_STARTED: NativeFunction = NativeFunction { sig: "public function isStarted(): bool {}", handler: f_is_started };
static IS_SUSPENDED: NativeFunction = NativeFunction { sig: "public function isSuspended(): bool {}", handler: f_is_suspended };
static IS_RUNNING: NativeFunction = NativeFunction { sig: "public function isRunning(): bool {}", handler: f_is_running };
static IS_TERMINATED: NativeFunction = NativeFunction { sig: "public function isTerminated(): bool {}", handler: f_is_terminated };
static SUSPEND: NativeFunction = NativeFunction { sig: "public static function suspend(mixed $value = null): mixed {}", handler: f_suspend };
static GET_CURRENT: NativeFunction = NativeFunction { sig: "public static function getCurrent(): ?Fiber {}", handler: f_get_current };
static FE_CONSTRUCT: NativeFunction = NativeFunction { sig: "public function __construct() {}", handler: fiber_error_construct };

/// Registers `Fiber` and `FiberError`.
pub fn register(r: &mut Registry<'_>) {
    let mut ce = ClassEntry::new("Fiber", class_flags::FINAL | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES);
    for nf in [&CONSTRUCT, &START, &RESUME, &THROW, &GET_RETURN, &IS_STARTED, &IS_SUSPENDED, &IS_RUNNING, &IS_TERMINATED] {
        ce.add_method(Function::native_method(nf, "Fiber", member_flags::PUBLIC));
    }
    for nf in [&SUSPEND, &GET_CURRENT] {
        ce.add_method(Function::native_method(nf, "Fiber", member_flags::PUBLIC | member_flags::STATIC));
    }
    ce.handlers.dtor_obj = Some(fiber_dtor);
    r.class(ce);
    let error = r.find_class("Error").expect("Error registered");
    let mut fe = ClassEntry::new_subclass("FiberError", class_flags::FINAL, &error);
    fe.add_method(Function::native_method(&FE_CONSTRUCT, "FiberError", member_flags::PUBLIC));
    r.class(fe);
}
