//! The tree-walking evaluator over the lowered IR ([`crate::compile::ir`]).
//!
//! * [`env`] — variable scopes.
//! * [`eval`] — expressions; [`exec`] — statements; [`place`] — write
//!   contexts (assignments, references, `unset`, `isset`).
//! * [`call`] — calls, argument binding and type checks, callables.
//! * [`objects`] — property access, magic methods, object instantiation.
//! * [`decl`] — function and class declaration and linking (inheritance,
//!   traits, enums, interface checks).
//! * [`closure`], [`generator`], [`fiber`] — the `Closure`, `Generator` and
//!   `Fiber` classes; generators and fibers run on stackful coroutines.
//! * [`include`] — `include`/`require`/`eval` and running scripts.
//! * [`builtins`], [`var`] — Zend built-in functions and the engine-coupled
//!   `ext/standard` functions (`var_dump`, `print_r`, ...).
//!
//! Interpreter state lives in [`State`] (`Vm::interp`); the [`Interp`] host
//! connects the runtime's [`crate::vm::Host`] hooks to the evaluator.

pub mod builtins;
pub mod call;
pub mod closure;
pub mod decl;
pub mod env;
pub mod eval;
pub mod exec;
pub mod fiber;
pub mod generator;
pub mod include;
pub mod objects;
pub mod place;
pub mod var;
pub mod weak;

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use crate::class::{ClassEntry, ClassRef, Function, FunctionKind};
use crate::compile::ir::FuncCode;
use crate::native::CallArgs;
use crate::object::ObjRef;
use crate::value::{PhpRef, PhpStr, Value};
use crate::vm::{Flow, Host, PResult, Vm};

pub use env::Env;

/// `Flow::Break` payload used to short-circuit a nullsafe chain.
pub(crate) const SHORT_CIRCUIT: u32 = u32::MAX;
/// `Flow::Break` payload used to propagate a `goto` (label in `Ctx::goto`).
pub(crate) const GOTO: u32 = u32::MAX - 1;
/// `Flow::Break` payload that unwinds everything (destroying a suspended
/// fiber): runs `finally` blocks, passes loops and function boundaries.
pub(crate) const UNWIND: u32 = u32::MAX - 2;

/// Interpreter state (`executor_globals` parts owned by the evaluator).
#[derive(Default)]
pub struct State {
    pub global_env: Option<Rc<Env>>,
    /// Files included so far (`get_included_files`), resolved paths.
    pub included_files: Vec<PhpStr>,
    included_set: HashSet<PhpStr>,
    /// `CG(rtd_key_counter)`.
    pub rtd_key: u32,
    /// `register_shutdown_function` entries.
    pub shutdown_functions: Vec<(Value, Vec<Value>)>,
    /// `register_tick_function` entries.
    pub tick_functions: Vec<(Value, Vec<Value>)>,
    /// `EG(ticks_count)`.
    pub ticks_count: u32,
    /// Tick functions being called (they are not re-entered).
    pub ticks_calling: Vec<Value>,
    /// Autoloaders (`spl_autoload_register` stores its callables here; they
    /// are called in order by class lookups).
    pub autoload_functions: Vec<Value>,
    /// Classes being autoloaded (recursion guard).
    pub autoloading: Vec<PhpStr>,
    /// The main script path.
    pub main_file: PhpStr,
    /// Lowest stack address usable before throwing the stack-overflow Error.
    pub stack_limit: Cell<usize>,
    /// Set when `exit()` was called (shutdown functions still run).
    pub exiting: bool,
    /// Nesting of user function calls (for diagnostics).
    pub depth: u32,
    /// Running shutdown functions.
    pub in_shutdown: bool,
    /// No user code is executing (`EG(current_execute_data) == NULL`), as
    /// when the user exception handler runs after the script: functions the
    /// engine calls then have no call site.
    pub detached: bool,
}

impl State {
    pub fn mark_included(&mut self, path: PhpStr) -> bool {
        if self.included_set.insert(path.clone()) {
            self.included_files.push(path);
            true
        } else {
            false
        }
    }

    pub fn is_included(&self, path: &[u8]) -> bool {
        self.included_set.contains(path)
    }
}

impl Vm {
    /// `CG(rtd_key_counter)++` (anonymous class and runtime declaration keys).
    pub fn next_rtd_key(&mut self) -> u32 {
        let k = self.interp.rtd_key;
        self.interp.rtd_key += 1;
        k
    }

    /// The global variable scope (created on first use).
    pub fn global_env(&mut self) -> Rc<Env> {
        if let Some(g) = &self.interp.global_env {
            return g.clone();
        }
        let g = Rc::new(Env::global());
        self.interp.global_env = Some(g.clone());
        g
    }
}

/// The interpreter payload of a userland [`Function`]
/// (`FunctionKind::User(Rc<UserFn>)`).
pub struct UserFn {
    pub code: Rc<FuncCode>,
    /// Static variables (`static $x`), per function object.
    pub statics: RefCell<Vec<Option<PhpRef>>>,
    /// Declaring class (set once the class is linked).
    pub scope: OnceCell<Weak<ClassEntry>>,
    /// For trait methods copied into a class: the trait they came from.
    pub from_trait: Option<PhpStr>,
}

impl UserFn {
    pub fn new(code: Rc<FuncCode>) -> UserFn {
        let n = code.num_statics as usize;
        UserFn {
            code,
            statics: RefCell::new(vec![None; n]),
            scope: OnceCell::new(),
            from_trait: None,
        }
    }

    /// A copy sharing the code but with fresh static variables (closures,
    /// trait methods).
    pub fn fork(&self) -> UserFn {
        UserFn {
            code: self.code.clone(),
            statics: RefCell::new(self.statics.borrow().iter().map(|s| s.as_ref().map(|r| PhpRef::new(r.get()))).collect()),
            scope: OnceCell::new(),
            from_trait: self.from_trait.clone(),
        }
    }

    pub fn scope_class(&self) -> Option<ClassRef> {
        self.scope.get().and_then(Weak::upgrade)
    }
}

/// The [`UserFn`] payload of a function, if it is a userland function.
pub fn user_fn(f: &Function) -> Option<&UserFn> {
    match &f.kind {
        FunctionKind::User(p) => p.downcast_ref::<UserFn>(),
        FunctionKind::Native(_) => None,
    }
}

/// The evaluation context of a running piece of code.
pub struct Ctx<'a> {
    pub env: &'a Rc<Env>,
    pub code: &'a FuncCode,
    pub ufn: &'a UserFn,
    /// CV → env-slot map for code running in a foreign scope (included files).
    pub map: Option<&'a [u32]>,
    /// Pending `goto` label.
    pub goto: Cell<Option<u32>>,
    /// The generator being executed (for `yield`).
    pub generator: Option<&'a generator::GenCtx>,
}

impl<'a> Ctx<'a> {
    #[inline]
    pub fn slot(&self, cv: u32) -> u32 {
        match self.map {
            None => cv,
            Some(m) => m[cv as usize],
        }
    }

    pub fn strict(&self) -> bool {
        self.code.strict()
    }

    /// Name of compiled variable `cv`.
    pub fn cv_name(&self, cv: u32) -> PhpStr {
        self.code.cvs.names[cv as usize].clone()
    }

    pub fn scope(&self) -> Option<&ClassRef> {
        self.env.scope.as_ref()
    }
}

/// The interpreter's [`Host`] implementation.
pub struct Interp;

impl Host for Interp {
    fn call_callable(&self, vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
        call::call_value(vm, callable, args, Vec::new(), true)
    }

    fn call_user_function(&self, vm: &mut Vm, func: &Rc<Function>, call: CallArgs) -> PResult<Value> {
        call::call_function_args(vm, func, call)
    }

    fn call_method(&self, vm: &mut Vm, obj: &ObjRef, name: &[u8], args: Vec<Value>) -> PResult<Value> {
        call::call_method_internal(vm, obj, name, args)
    }

    fn check_callable(&self, vm: &mut Vm, callable: &Value) -> Result<PhpStr, String> {
        call::check_callable(vm, callable, false).map(|c| c.name)
    }

    fn run_destructor(&self, vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
        objects::run_destructor(vm, obj)
    }

    fn autoload(&self, vm: &mut Vm, name: &[u8]) -> PResult<Option<ClassRef>> {
        decl::autoload(vm, name)
    }

    fn eval_class_constant(&self, vm: &mut Vm, class: &ClassRef, expr: &Rc<dyn Any>) -> PResult<Value> {
        decl::eval_const_payload(vm, class, expr)
    }
}

/// Approximate stack pointer (address of a local).
#[inline(always)]
pub(crate) fn stack_ptr() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

/// Throws php-src's stack-overflow Error when the Rust stack runs low.
#[inline]
pub(crate) fn check_stack(vm: &mut Vm) -> PResult<()> {
    let limit = vm.interp.stack_limit.get();
    if limit != 0 && stack_ptr() < limit {
        // Leave headroom for building the exception.
        vm.interp.stack_limit.set(limit.saturating_sub(256 * 1024));
        let e = vm.throw_error(
            "Error",
            "Maximum call stack size reached. Infinite recursion?",
        );
        vm.interp.stack_limit.set(limit);
        return Err(e);
    }
    Ok(())
}

/// Installs the interpreter into `vm` and registers the engine's built-in
/// classes and functions (Core module extras and the engine-coupled
/// `standard` functions).
pub fn install(vm: &mut Vm) {
    vm.set_host(Rc::new(Interp));
    vm.register_module("Core", builtins::register_core_extras);
}

/// Registers the engine-coupled `ext/standard` functions (`var_dump`, ...)
/// under module "standard". Call after the extensions are registered.
pub fn install_standard(vm: &mut Vm) {
    vm.register_module("standard", var::register);
    vm.register_module("standard", builtins::register_standard);
}

/// Converts a `Flow` escaping a script to the uncaught-exception report.
pub(crate) fn is_exception(f: &Flow) -> bool {
    matches!(f, Flow::Exception(_))
}
