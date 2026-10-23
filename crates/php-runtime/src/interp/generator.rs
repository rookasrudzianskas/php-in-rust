//! Generators (`Zend/zend_generators.c`) on stackful coroutines.
//!
//! A generator's body is interpreted by the ordinary evaluator on its own
//! coroutine stack (`corosensei`); `yield` suspends the coroutine. Values
//! sent in (`send`, `throw`, destruction) come back as the result of the
//! suspended `yield`.
//!
//! `yield from <generator>` follows php-src's delegation tree: the
//! delegating generator records the inner generator as its `parent` and
//! suspends; whoever resumes the outer generator (the "leaf") then resumes
//! the root of the tree directly ([`resume_gen`], a port of
//! `zend_generator_resume`). Several generators may delegate to the same
//! inner generator. When the root finishes, the next generator on the
//! leaf's path receives its return value (`zend_generator_update_current`).
//! Arrays and other Traversables are iterated by the delegating body itself.
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
//!
//! A fiber may suspend while a generator runs inside it (the fiber's
//! logical stack then spans the generator's coroutine stack). The generator
//! stays marked as running until the fiber comes back to it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::class::{ClassEntry, ClassRef, Function, class_flags, member_flags};
use crate::compile::ir::*;
use crate::native::{Args, NativeFunction, Registry};
use crate::object::{self, ObjRef};
use crate::value::{PhpArray, PhpRef, PhpStr, Value};
use crate::vm::{Flow, Frame, PResult, Vm};

use super::closure::ClosureData;
use super::eval::eval;
use super::{Ctx, Env, UNWIND, user_fn};

/// What a suspended `yield` (or `yield from`) receives when resumed.
pub enum Resume {
    /// Continue; the value is the result of the `yield` / `yield from`.
    Send(Value),
    /// Raise a flow (exception, unwinding) at the suspension point.
    Throw(Flow),
    /// Forced close: unwind, running `finally` blocks.
    Destroy,
}

/// Why a generator body suspended.
pub enum Suspend {
    /// A value was yielded (key and value are in the core).
    Yield,
    /// `yield from <generator>`: the core's `parent` is the delegate.
    Delegate,
}

type GenCoroutine = Coroutine<(usize, Resume), Suspend, PResult<Value>, DefaultStack>;

/// Mutable state of a generator (`zend_generator` fields and flags).
pub struct GenCore {
    /// Current key/value (`IS_UNDEF` before the first yield).
    pub key: Option<Value>,
    pub value: Option<Value>,
    pub largest_int_key: i64,
    /// The generator can still run (`execute_data != NULL`).
    pub alive: bool,
    /// The body has been entered.
    pub started: bool,
    /// `ZEND_GENERATOR_CURRENTLY_RUNNING`.
    pub running: bool,
    /// `ZEND_GENERATOR_AT_FIRST_YIELD`.
    pub at_first_yield: bool,
    /// `ZEND_GENERATOR_DO_INIT`.
    pub do_init: bool,
    /// `ZEND_GENERATOR_FORCED_CLOSE`.
    pub forced_close: bool,
    /// `IS_OBJ_DESTRUCTOR_CALLED` of the generator object.
    pub dtor_called: bool,
    /// Suspended at a `yield` whose result receives sent values.
    pub send_target: bool,
    /// Suspended in `yield from <generator>` (waiting for the delegate).
    pub at_delegate: bool,
    /// Number of active resumptions this generator takes part in (as the
    /// resumed generator or on the delegation path to the running root).
    pub busy: u32,
    pub retval: Option<Value>,
    pub returns_ref: bool,
    /// `node.parent`: the generator this one delegates to.
    pub parent: Option<ObjRef>,
    /// Input for the next resumption of the body.
    pending: Option<Resume>,
    /// Line of the current suspension point (for traces).
    pub line: u32,
}

/// Per-body context available to `yield` (via [`Ctx::generator`]).
pub struct GenCtx {
    yielder: *const Yielder<(usize, Resume), Suspend>,
    gn: Weak<GenObj>,
    /// The VM pointer of the latest resume.
    vm: Cell<usize>,
}

impl GenCtx {
    fn me(&self) -> Rc<GenObj> {
        self.gn.upgrade().expect("running generator")
    }

    /// Suspends the body; returns the resumption input.
    fn suspend(&self, why: Suspend) -> Resume {
        // SAFETY: the yielder outlives the body activation that owns `self`.
        let y = unsafe { &*self.yielder };
        let (ptr, input) = y.suspend(why);
        self.vm.set(ptr);
        input
    }
}

/// Native payload of a `Generator` object.
pub struct GenObj {
    pub core: RefCell<GenCore>,
    co: RefCell<Option<GenCoroutine>>,
    func: Rc<Function>,
    /// The activation's variables (released when the generator closes).
    env: RefCell<Option<Rc<Env>>>,
    this: Option<ObjRef>,
    file: PhpStr,
    stack_limit: usize,
}

impl GenObj {
    fn alive(&self) -> bool {
        self.core.borrow().alive
    }

    fn parent(&self) -> Option<Rc<GenObj>> {
        self.core.borrow().parent.as_ref().and_then(gen_obj)
    }
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
    let stack = take_stack();
    let limit = {
        use corosensei::stack::Stack;
        stack.limit().get() + 256 * 1024
    };
    let this = env.this.clone();
    let gn = Rc::new_cyclic(|weak: &Weak<GenObj>| {
        let body_env = env.clone();
        let body_func = func.clone();
        let body_gen = weak.clone();
        let keep_closure = closure;
        let co: GenCoroutine = Coroutine::with_stack(stack, move |y: &Yielder<(usize, Resume), Suspend>, input: (usize, Resume)| {
            let _closure = keep_closure;
            // SAFETY: see the module documentation; the pointer is the live `Vm`
            // of the resumer for the duration of this activation.
            let vm: &mut Vm = unsafe { &mut *(input.0 as *mut Vm) };
            match input.1 {
                Resume::Send(_) => {}
                Resume::Throw(f) => return Err(f),
                Resume::Destroy => return Err(Flow::Break(UNWIND)),
            }
            let ufn = user_fn(&body_func).expect("user function");
            let gctx = GenCtx {
                yielder: y as *const _,
                gn: body_gen,
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
            match r {
                Ok(()) => Ok(Value::Null),
                // ZEND_GENERATOR_RETURN copies the value dereferenced.
                Err(Flow::Return(v)) => Ok(v.into_unref()),
                Err(e) => Err(e),
            }
        });
        GenObj {
            core: RefCell::new(GenCore {
                key: None,
                value: None,
                largest_int_key: -1,
                alive: true,
                started: false,
                running: false,
                at_first_yield: false,
                do_init: false,
                forced_close: false,
                dtor_called: false,
                send_target: false,
                at_delegate: false,
                busy: 0,
                retval: None,
                returns_ref: code.returns_ref(),
                parent: None,
                pending: None,
                line: code.start_line,
            }),
            co: RefCell::new(Some(co)),
            func: func.clone(),
            env: RefCell::new(Some(env.clone())),
            this,
            file: code.file.clone(),
            stack_limit: limit,
        }
    });
    let obj = ObjRef::new(class);
    obj.set_native(gn);
    Ok(Value::Object(obj))
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// Where a generator resumed by the current code is "called" from: the
/// current userland position, or `[internal function]`.
fn caller_site(vm: &Vm) -> Option<(PhpStr, u32)> {
    if vm.in_native_frame() || (vm.frames.is_empty() && vm.interp.in_shutdown) {
        None
    } else {
        Some((vm.current_file.clone(), vm.current_line))
    }
}

fn gen_frame(g: &GenObj, call_site: Option<(PhpStr, u32)>) -> Frame {
    Frame {
        function: Some(g.func.clone()),
        name: g.func.name.clone(),
        class: g.func.scope.clone(),
        call_type: if g.func.scope.is_some() {
            Some(if g.this.is_some() { "->" } else { "::" })
        } else {
            None
        },
        this: g.this.clone(),
        call_site,
        args: Vec::new(),
        is_internal: false,
        env: g.env.borrow().clone(),
    }
}

/// The delegation path from `leaf` up to `root` (both included).
fn path(leaf: &Rc<GenObj>, root: &Rc<GenObj>) -> Vec<Rc<GenObj>> {
    let mut out = vec![leaf.clone()];
    let mut cur = leaf.clone();
    while !Rc::ptr_eq(&cur, root) {
        match cur.parent() {
            Some(p) => {
                out.push(p.clone());
                cur = p;
            }
            None => break,
        }
    }
    out
}

/// The frames of a delegation path while it runs: the leaf is called from
/// the code resuming it, each delegate from its child's `yield from` line
/// (`zend_generator_check_placeholder_frame`). Kept pushed across the
/// steps of one resumption so that deep delegation chains stay linear.
#[derive(Default)]
struct PathFrames {
    gens: Vec<Rc<GenObj>>,
}

impl PathFrames {
    fn push(&mut self, vm: &mut Vm, g: &Rc<GenObj>) {
        let site = match self.gens.last() {
            None => caller_site(vm),
            Some(c) => Some((c.file.clone(), c.core.borrow().line)),
        };
        vm.push_frame(gen_frame(g, site));
        g.core.borrow_mut().busy += 1;
        self.gens.push(g.clone());
    }

    fn pop(&mut self, vm: &mut Vm) {
        if let Some(g) = self.gens.pop() {
            vm.pop_frame();
            g.core.borrow_mut().busy -= 1;
        }
    }

    fn truncate(&mut self, vm: &mut Vm, n: usize) {
        while self.gens.len() > n {
            self.pop(vm);
        }
    }

    /// Makes the pushed path run from `orig` to `root`.
    fn sync(&mut self, vm: &mut Vm, orig: &Rc<GenObj>, root: &Rc<GenObj>) {
        if let Some(i) = self.gens.iter().rposition(|g| Rc::ptr_eq(g, root)) {
            self.truncate(vm, i + 1);
            return;
        }
        if let Some(last) = self.gens.last().cloned() {
            let mut chain = Vec::new();
            let mut cur = last.parent();
            while let Some(p) = cur {
                let done = Rc::ptr_eq(&p, root);
                cur = if done { None } else { p.parent() };
                chain.push(p);
                if done {
                    for g in &chain {
                        self.push(vm, g);
                    }
                    return;
                }
            }
        }
        self.truncate(vm, 0);
        for g in path(orig, root) {
            self.push(vm, &g);
        }
    }
}

/// Pushes the frames of `path` for a moment (exceptions created outside a
/// resumption); returns the number pushed.
fn push_path_frames(vm: &mut Vm, path: &[Rc<GenObj>]) -> usize {
    let mut site = caller_site(vm);
    for g in path {
        vm.push_frame(gen_frame(g, site));
        site = Some((g.file.clone(), g.core.borrow().line));
    }
    path.len()
}

fn pop_frames(vm: &mut Vm, n: usize) {
    for _ in 0..n {
        vm.pop_frame();
    }
}

/// Runs queued destructors now (php-src destroys values as soon as they are
/// released); an exception they throw is chained onto a pending one.
pub(crate) fn run_queued_destructors<T>(vm: &mut Vm, r: PResult<T>) -> PResult<T> {
    if !object::has_pending_destructors() {
        return r;
    }
    match vm.run_pending_destructors() {
        Ok(()) => r,
        Err(Flow::Exception(new)) => {
            if let Err(Flow::Exception(old)) = r {
                super::exec::chain_previous(&new, old);
            }
            Err(Flow::Exception(new))
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Resumption (zend_generator_resume and friends)
// ---------------------------------------------------------------------------

enum Step {
    Yielded,
    Delegated,
    Returned,
    Threw(Flow),
}

/// Runs `gn` (the root of the tree, whose frame is the last one in
/// `frames`) until it suspends or finishes.
fn run_step(vm: &mut Vm, gn: &Rc<GenObj>, frames: &mut PathFrames) -> Step {
    let Some(mut co) = gn.co.borrow_mut().take() else {
        gn.core.borrow_mut().alive = false;
        return Step::Returned;
    };
    let input = {
        let mut c = gn.core.borrow_mut();
        c.running = true;
        c.started = true;
        c.at_delegate = false;
        c.send_target = false;
        c.pending.take().unwrap_or(Resume::Send(Value::Null))
    };
    vm.current_file = gn.file.clone();
    vm.current_line = gn.core.borrow().line;
    let saved_limit = vm.interp.stack_limit.get();
    vm.interp.stack_limit.set(gn.stack_limit);
    let r = co.resume((vm as *mut Vm as usize, input));
    vm.interp.stack_limit.set(saved_limit);
    let step = match r {
        CoroutineResult::Yield(why) => {
            *gn.co.borrow_mut() = Some(co);
            match why {
                Suspend::Yield => Step::Yielded,
                Suspend::Delegate => {
                    gn.core.borrow_mut().at_delegate = true;
                    Step::Delegated
                }
            }
        }
        CoroutineResult::Return(res) => {
            return_stack(co.into_stack());
            {
                let mut c = gn.core.borrow_mut();
                c.alive = false;
                if let Ok(v) = &res {
                    c.retval = Some(v.clone());
                }
            }
            // zend_generator_close: the variables are released in the
            // context of the code that resumed the generator.
            frames.pop(vm);
            let env = gn.env.borrow_mut().take();
            drop(env);
            let res = run_queued_destructors(vm, res);
            match res {
                Ok(_) => Step::Returned,
                Err(f) => Step::Threw(f),
            }
        }
    };
    gn.core.borrow_mut().running = false;
    step
}

/// `zend_generator_resume`.
fn resume_gen(vm: &mut Vm, orig: &Rc<GenObj>) -> PResult<()> {
    let gn = get_current(vm, orig, false)?;
    if !gn.alive() {
        return Ok(());
    }
    let mut frames = PathFrames::default();
    let saved_file = vm.current_file.clone();
    let saved_line = vm.current_line;
    let r = resume_loop(vm, orig, gn, &mut frames);
    frames.truncate(vm, 0);
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    r
}

fn resume_loop(vm: &mut Vm, orig: &Rc<GenObj>, mut gn: Rc<GenObj>, frames: &mut PathFrames) -> PResult<()> {
    let mut delegator = orig.clone();
    loop {
        if gn.core.borrow().running {
            return Err(vm.throw_error("Error", "Cannot resume an already running generator"));
        }
        let do_init = std::mem::take(&mut delegator.core.borrow_mut().do_init);
        if do_init && gn.core.borrow().value.is_some() {
            // An already initialized delegate is not advanced on first resumption.
            return Ok(());
        }
        orig.core.borrow_mut().at_first_yield = false;
        frames.sync(vm, orig, &gn);
        match run_step(vm, &gn, frames) {
            Step::Yielded | Step::Returned => {}
            Step::Delegated => {
                // yield from was used: run the delegate.
                let mut root = gn.clone();
                while let Some(p) = root.parent() {
                    root = p;
                }
                delegator = gn;
                gn = if root.alive() { root } else { update_current(vm, orig, &root, false, Some(frames))? };
                continue;
            }
            Step::Threw(flow) => {
                if Rc::ptr_eq(&gn, orig) {
                    return Err(flow);
                }
                // Rethrow into the next generator on the delegation path.
                gn = update_current(vm, orig, &gn, true, Some(frames))?;
                if gn.core.borrow().running {
                    return Err(vm.throw_error("Error", "Cannot resume an already running generator"));
                }
                gn.core.borrow_mut().pending = Some(Resume::Throw(flow));
                delegator = orig.clone();
                continue;
            }
        }
        if !Rc::ptr_eq(&gn, orig) && gn.core.borrow().retval.is_some() {
            delegator = orig.clone();
            gn = update_current(vm, orig, &gn, false, Some(frames))?;
            continue;
        }
        return Ok(());
    }
}

/// `zend_generator_get_current`: the generator currently executing for
/// `gn` (the root of its delegation tree).
fn get_current(vm: &mut Vm, gn: &Rc<GenObj>, exception_pending: bool) -> PResult<Rc<GenObj>> {
    let Some(mut root) = gn.parent() else {
        return Ok(gn.clone());
    };
    while let Some(p) = root.parent() {
        root = p;
    }
    if root.alive() {
        return Ok(root);
    }
    update_current(vm, gn, &root, exception_pending, None)
}

/// `zend_generator_update_current`: the root `old_root` of the leaf `gn`'s
/// tree has finished; the generator below it on `gn`'s path takes over and
/// receives the return value as the result of its `yield from`. Inside a
/// resumption, `frames` holds the path being run.
fn update_current(
    vm: &mut Vm,
    gn: &Rc<GenObj>,
    old_root: &Rc<GenObj>,
    exception_pending: bool,
    frames: Option<&mut PathFrames>,
) -> PResult<Rc<GenObj>> {
    // get_new_root: the first generator on the leaf's path whose delegate is
    // dead (normally the last one of the path being run).
    let hinted = frames.as_ref().and_then(|f| {
        let last = f.gens.last()?;
        let ok = last.alive() && last.parent().is_some_and(|p| Rc::ptr_eq(&p, old_root));
        ok.then(|| last.clone())
    });
    let new_root = match hinted {
        Some(r) => r,
        None => {
            let mut r = gn.clone();
            loop {
                match r.parent() {
                    Some(p) if p.alive() => r = p,
                    _ => break,
                }
            }
            r
        }
    };
    let Some(parent) = new_root.parent() else {
        return Ok(new_root);
    };
    let at_delegate = new_root.core.borrow().at_delegate;
    if !exception_pending && !gn.core.borrow().dtor_called && at_delegate {
        let retval = parent.core.borrow().retval.clone();
        match retval {
            None => {
                // Thrown in the context of the new root.
                let in_resume = frames.is_some();
                let pushed = match frames {
                    Some(f) => {
                        f.sync(vm, gn, &new_root);
                        0
                    }
                    None => {
                        let p = path(gn, &new_root);
                        push_path_frames(vm, &p)
                    }
                };
                let saved_file = std::mem::replace(&mut vm.current_file, new_root.file.clone());
                let saved_line = vm.current_line;
                vm.current_line = new_root.core.borrow().line;
                let ex = vm.throw_error("ClosedGeneratorException", "Generator yielded from aborted, no return value available");
                vm.current_file = saved_file;
                vm.current_line = saved_line;
                pop_frames(vm, pushed);
                let parent_obj = new_root.core.borrow_mut().parent.take();
                drop(parent_obj);
                if old_root.core.borrow().running {
                    return Err(ex);
                }
                new_root.core.borrow_mut().pending = Some(Resume::Throw(ex));
                if in_resume {
                    // The running resumption delivers it.
                    return Ok(new_root);
                }
                resume_gen(vm, gn)?;
                return get_current(vm, gn, false);
            }
            Some(rv) => {
                let pv = parent.core.borrow().value.clone();
                let mut c = new_root.core.borrow_mut();
                c.value = pv;
                c.pending = Some(Resume::Send(rv));
            }
        }
    }
    let parent_obj = new_root.core.borrow_mut().parent.take();
    drop(parent_obj);
    Ok(new_root)
}

/// `zend_generator_ensure_initialized`.
fn ensure_initialized(vm: &mut Vm, g: &Rc<GenObj>) -> PResult<()> {
    let need = {
        let c = g.core.borrow();
        c.value.is_none() && c.alive && c.parent.is_none()
    };
    if need {
        let r = resume_gen(vm, g);
        g.core.borrow_mut().at_first_yield = true;
        r?;
    }
    Ok(())
}

/// `zend_generator_rewind`.
fn rewind(vm: &mut Vm, g: &Rc<GenObj>) -> PResult<()> {
    ensure_initialized(vm, g)?;
    if !g.core.borrow().at_first_yield {
        return Err(vm.throw_error("Exception", "Cannot rewind a generator that was already run"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// yield / yield from
// ---------------------------------------------------------------------------

/// `yield` (`ZEND_YIELD`).
pub fn eval_yield(vm: &mut Vm, cx: &Ctx, key: Option<&Expr>, value: Option<&Expr>, line: u32) -> PResult<Value> {
    let Some(g) = cx.generator else {
        return Err(vm.throw_error("Error", "Cannot yield outside a generator"));
    };
    let me = g.me();
    let returns_ref = me.core.borrow().returns_ref;
    let v = match value {
        Some(e) => Some((eval(vm, cx, e)?, matches!(e, Expr::MakeRef(..)))),
        None => None,
    };
    let k = match key {
        Some(e) => Some(eval(vm, cx, e)?.into_unref()),
        None => None,
    };
    vm.current_line = line;
    if me.core.borrow().forced_close {
        return Err(vm.throw_error("Error", "Cannot yield from finally in a force-closed generator"));
    }
    let v = match v {
        None => Value::Null,
        Some((v, is_ref_expr)) => {
            if returns_ref {
                match v {
                    Value::Ref(r) => Value::Ref(r),
                    v => {
                        if !is_ref_expr {
                            vm.engine_notice("Only variable references should be yielded by reference")?;
                        }
                        Value::Ref(PhpRef::new(v))
                    }
                }
            } else {
                v.into_unref()
            }
        }
    };
    {
        let mut c = me.core.borrow_mut();
        let k = match k {
            Some(k) => {
                if let Value::Int(i) = k {
                    if i > c.largest_int_key {
                        c.largest_int_key = i;
                    }
                }
                k
            }
            None => {
                c.largest_int_key += 1;
                Value::Int(c.largest_int_key)
            }
        };
        // The previous key and value are released here.
        c.value = Some(v);
        c.key = Some(k);
        c.send_target = true;
        c.line = line;
    }
    drop(me);
    match g.suspend(Suspend::Yield) {
        Resume::Send(v) => Ok(v),
        Resume::Throw(f) => Err(f),
        Resume::Destroy => Err(Flow::Break(UNWIND)),
    }
}

/// Publishes a delegated value (array or iterator element) and suspends.
fn yield_delegated(g: &GenCtx, key: Value, value: Value, line: u32) -> Result<(), Flow> {
    {
        let me = g.me();
        let mut c = me.core.borrow_mut();
        c.value = Some(value);
        c.key = Some(key);
        c.line = line;
    }
    match g.suspend(Suspend::Yield) {
        Resume::Send(_) => Ok(()),
        Resume::Throw(f) => Err(f),
        Resume::Destroy => Err(Flow::Break(UNWIND)),
    }
}

/// `yield from` (`ZEND_YIELD_FROM`).
pub fn eval_yield_from(vm: &mut Vm, cx: &Ctx, e: &Expr, line: u32) -> PResult<Value> {
    let Some(g) = cx.generator else {
        return Err(vm.throw_error("Error", "Cannot use \"yield from\" outside a generator"));
    };
    let v = eval(vm, cx, e)?;
    vm.current_line = line;
    if g.me().core.borrow().forced_close {
        return Err(vm.throw_error("Error", "Cannot use \"yield from\" in a force-closed generator"));
    }
    match v.into_unref() {
        Value::Array(a) => {
            let mut err = None;
            {
                let mut it = a.iter();
                while let Some((k, v)) = it.next() {
                    if let Err(f) = yield_delegated(g, k.to_value(), v.clone(), line) {
                        err = Some(f);
                        break;
                    }
                }
            }
            drop(a);
            match err {
                None => Ok(Value::Null),
                // The remaining values are released before the exception
                // reaches the generator.
                Some(f) => run_queued_destructors(vm, Err(f)),
            }
        }
        Value::Object(o) => {
            if let Some(inner) = gen_obj(&o) {
                return delegate(vm, g, o, inner, line);
            }
            if !o.class().instance_of_name(b"Traversable") {
                return Err(vm.throw_error("Error", "Can use \"yield from\" only with arrays and Traversables"));
            }
            let mut err = None;
            let r = super::exec::iterate_object(vm, &o, false, &mut |vm, k, v| {
                vm.current_line = line;
                match yield_delegated(g, k, v, line) {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        err = Some(e);
                        Ok(false)
                    }
                }
            });
            drop(o);
            match (r, err) {
                (Err(e), _) | (Ok(()), Some(e)) => run_queued_destructors(vm, Err(e)),
                (Ok(()), None) => Ok(Value::Null),
            }
        }
        _ => Err(vm.throw_error("Error", "Can use \"yield from\" only with arrays and Traversables")),
    }
}

/// `yield from <generator>`: delegation (`zend_generator_yield_from`).
fn delegate(vm: &mut Vm, g: &GenCtx, inner_obj: ObjRef, inner: Rc<GenObj>, line: u32) -> PResult<Value> {
    if !inner.alive() {
        return Err(vm.throw_error(
            "Error",
            "Generator passed to yield from was aborted without proper return and is unable to continue",
        ));
    }
    if let Some(rv) = inner.core.borrow().retval.clone() {
        return Ok(rv);
    }
    let me = g.me();
    let cur = get_current(vm, &inner, false)?;
    if Rc::ptr_eq(&cur, &me) {
        return Err(vm.throw_error("Error", "Impossible to yield from the Generator being currently run"));
    }
    {
        let mut c = me.core.borrow_mut();
        c.parent = Some(inner_obj);
        c.do_init = true;
        c.line = line;
    }
    // No strong references to the delegate are kept across the suspension.
    drop((cur, inner, me));
    match g.suspend(Suspend::Delegate) {
        Resume::Send(v) => Ok(v),
        Resume::Throw(f) => Err(f),
        Resume::Destroy => Err(Flow::Break(UNWIND)),
    }
}

/// `foreach` over a generator (the generator's `get_iterator`).
pub fn iterate(vm: &mut Vm, o: &ObjRef, by_ref: bool, f: &mut dyn FnMut(&mut Vm, Value, Value) -> PResult<bool>) -> PResult<()> {
    let g = gen_obj(o).expect("generator");
    if !g.alive() {
        return Err(vm.throw_error("Exception", "Cannot traverse an already closed generator"));
    }
    if by_ref && !g.core.borrow().returns_ref {
        return Err(vm.throw_error(
            "Exception",
            "You can only iterate a generator by-reference if it declared that it yields by-reference",
        ));
    }
    rewind(vm, &g)?;
    loop {
        // valid
        ensure_initialized(vm, &g)?;
        let root = get_current(vm, &g, false)?;
        if !g.alive() {
            break;
        }
        // current / key
        let (k, v) = {
            let c = root.core.borrow();
            (c.key.clone().unwrap_or(Value::Null).unref(), c.value.clone().unwrap_or(Value::Null))
        };
        drop(root);
        let v = if by_ref { v } else { v.into_unref() };
        if !f(vm, k, v)? {
            return Ok(());
        }
        // move_forward
        ensure_initialized(vm, &g)?;
        resume_gen(vm, &g)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Destruction
// ---------------------------------------------------------------------------

/// `zend_generator_dtor_storage`: destroys a generator, running the
/// `finally` blocks around its suspension point.
fn gen_dtor(vm: &mut Vm, o: &ObjRef) -> PResult<()> {
    let Some(g) = gen_obj(o) else { return Ok(()) };
    g.core.borrow_mut().dtor_called = true;
    {
        let mut c = g.core.borrow_mut();
        if c.running || c.busy > 0 {
            // Executing inside a suspended fiber: it is unwound with the
            // fiber; finally blocks must not yield.
            c.forced_close = true;
            return Ok(());
        }
    }
    // Leave yield from mode (the delegate may be destroyed right away).
    let parent = g.core.borrow_mut().parent.take();
    let mut r = Ok(());
    if parent.is_some() {
        drop(parent);
        r = run_queued_destructors(vm, Ok(()));
    }
    let (alive, started) = {
        let c = g.core.borrow();
        (c.alive, c.started)
    };
    if !alive {
        return r;
    }
    if !started {
        close(&g);
        return run_queued_destructors(vm, r);
    }
    g.core.borrow_mut().forced_close = true;
    let Some(mut co) = g.co.borrow_mut().take() else {
        close(&g);
        return run_queued_destructors(vm, r);
    };
    {
        let mut c = g.core.borrow_mut();
        c.running = true;
        c.at_delegate = false;
        c.pending = None;
    }
    let site = caller_site(vm);
    vm.push_frame(gen_frame(&g, site));
    let saved_file = std::mem::replace(&mut vm.current_file, g.file.clone());
    let saved_line = vm.current_line;
    vm.current_line = g.core.borrow().line;
    let saved_limit = vm.interp.stack_limit.get();
    vm.interp.stack_limit.set(g.stack_limit);
    let res = co.resume((vm as *mut Vm as usize, Resume::Destroy));
    vm.interp.stack_limit.set(saved_limit);
    let out = match res {
        CoroutineResult::Return(res) => {
            return_stack(co.into_stack());
            match res {
                Ok(v) => {
                    g.core.borrow_mut().retval = Some(v);
                    Ok(())
                }
                Err(Flow::Break(UNWIND)) => Ok(()),
                Err(e) => Err(e),
            }
        }
        CoroutineResult::Yield(_) => {
            // Cannot happen (yields fail in a force-closed generator); the
            // coroutine is unwound when dropped.
            drop(co);
            Ok(())
        }
    };
    g.core.borrow_mut().running = false;
    vm.pop_frame();
    close(&g);
    let out = run_queued_destructors(vm, out);
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    match (out, r) {
        (Err(Flow::Exception(new)), Err(Flow::Exception(old))) => {
            super::exec::chain_previous(&new, old);
            Err(Flow::Exception(new))
        }
        (Err(e), _) => Err(e),
        (Ok(()), r) => r,
    }
}

/// `zend_generator_close`.
fn close(g: &GenObj) {
    let co = g.co.borrow_mut().take();
    drop(co);
    g.core.borrow_mut().alive = false;
    let env = g.env.borrow_mut().take();
    drop(env);
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

fn root_value(vm: &mut Vm, g: &Rc<GenObj>) -> PResult<Value> {
    let root = get_current(vm, g, false)?;
    if !g.alive() {
        return Ok(Value::Null);
    }
    let v = root.core.borrow().value.clone();
    Ok(v.map(|v| v.unref()).unwrap_or(Value::Null))
}

fn g_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    rewind(vm, &g)?;
    Ok(Value::Null)
}

fn g_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_initialized(vm, &g)?;
    get_current(vm, &g, false)?;
    Ok(Value::Bool(g.alive()))
}

fn g_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_initialized(vm, &g)?;
    root_value(vm, &g)
}

fn g_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_initialized(vm, &g)?;
    let root = get_current(vm, &g, false)?;
    if !g.alive() {
        return Ok(Value::Null);
    }
    let k = root.core.borrow().key.clone();
    Ok(k.map(|k| k.unref()).unwrap_or(Value::Null))
}

fn g_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_initialized(vm, &g)?;
    resume_gen(vm, &g)?;
    Ok(Value::Null)
}

fn g_send(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    let v = args.get(0).unref();
    ensure_initialized(vm, &g)?;
    if !g.alive() {
        return Ok(Value::Null);
    }
    let root = get_current(vm, &g, false)?;
    {
        let mut c = root.core.borrow_mut();
        if c.send_target && !c.running {
            c.pending = Some(Resume::Send(v));
        }
    }
    drop(root);
    resume_gen(vm, &g)?;
    root_value(vm, &g)
}

fn g_throw(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    let ex = args.object(0).clone();
    ensure_initialized(vm, &g)?;
    if !g.alive() {
        // Thrown in the current context.
        return Err(Flow::Exception(ex));
    }
    let root = get_current(vm, &g, false)?;
    if root.core.borrow().running {
        return Err(vm.throw_error("Error", "Cannot resume an already running generator"));
    }
    root.core.borrow_mut().pending = Some(Resume::Throw(Flow::Exception(ex)));
    drop(root);
    resume_gen(vm, &g)?;
    root_value(vm, &g)
}

fn g_get_return(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    ensure_initialized(vm, &g)?;
    let rv = g.core.borrow().retval.clone();
    match rv {
        Some(v) => Ok(v),
        None => Err(vm.throw_error("Exception", "Cannot get return value of a generator that hasn't returned")),
    }
}

fn debug_info(g: &GenObj) -> PhpArray {
    let name = match &g.func.scope {
        Some(c) => format!("{c}::{}", g.func.name),
        None => g.func.name.to_string(),
    };
    let mut a = PhpArray::new();
    a.insert_str("function", PhpStr::from(name));
    a
}

fn g_debug_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = this_gen(vm, args)?;
    Ok(Value::array(debug_info(&g)))
}

fn gen_get_debug_info(_vm: &mut Vm, o: &ObjRef, _purpose: crate::class::PropPurpose) -> PResult<Option<PhpArray>> {
    Ok(gen_obj(o).map(|g| debug_info(&g)))
}

fn gen_create_object(_vm: &mut Vm, class: &ClassRef) -> PResult<ObjRef> {
    Ok(ObjRef::new(class.clone()))
}

static REWIND: NativeFunction = NativeFunction { sig: "public function rewind(): void {}", handler: g_rewind };
static VALID: NativeFunction = NativeFunction { sig: "public function valid(): bool {}", handler: g_valid };
static CURRENT: NativeFunction = NativeFunction { sig: "public function current(): mixed {}", handler: g_current };
static KEY: NativeFunction = NativeFunction { sig: "public function key(): mixed {}", handler: g_key };
static NEXT: NativeFunction = NativeFunction { sig: "public function next(): void {}", handler: g_next };
static SEND: NativeFunction = NativeFunction { sig: "public function send(mixed $value): mixed {}", handler: g_send };
static THROW: NativeFunction = NativeFunction { sig: "public function throw(Throwable $exception): mixed {}", handler: g_throw };
static GET_RETURN: NativeFunction = NativeFunction { sig: "public function getReturn(): mixed {}", handler: g_get_return };
static DEBUG_INFO: NativeFunction = NativeFunction { sig: "public function __debugInfo(): array {}", handler: g_debug_info };

/// Registers `Generator` and `ClosedGeneratorException`.
pub fn register(r: &mut Registry<'_>) {
    let iterator: ClassRef = r.find_class("Iterator").expect("Iterator registered");
    let mut ce = ClassEntry::new(
        "Generator",
        class_flags::FINAL | class_flags::NO_DYNAMIC_INSTANTIATION | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES,
    );
    ce.interfaces = iterator.interfaces.clone();
    ce.interfaces.push(iterator);
    for nf in [&REWIND, &VALID, &CURRENT, &KEY, &NEXT, &SEND, &THROW, &GET_RETURN, &DEBUG_INFO] {
        ce.add_method(Function::native_method(nf, "Generator", member_flags::PUBLIC));
    }
    ce.handlers.dtor_obj = Some(gen_dtor);
    ce.handlers.get_debug_info = Some(gen_get_debug_info);
    // No clone handler: generators are uncloneable.
    ce.handlers.create_object = Some(gen_create_object);
    r.class(ce);
    let exception = r.find_class("Exception").expect("Exception registered");
    r.class(ClassEntry::new_subclass("ClosedGeneratorException", 0, &exception));
}
