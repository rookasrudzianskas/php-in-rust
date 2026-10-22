//! `RecursiveIteratorIterator` and `RecursiveTreeIterator`
//! (`spl_recursive_it_object`).
//!
//! The state ([`RecIt`]) holds one [`SubIter`] per level (the inner
//! `RecursiveIterator` and its children) with the `RS_*` state machine of
//! `spl_recursive_it_move_forward_ex`, ported step by step including its
//! re-entrancy checks and the `CATCH_GET_CHILD` exception handling (see
//! [`Pending`]). No borrow of the state is held while PHP code runs, so
//! user code may call back into the iterator (`next()` from `valid()` or
//! `endChildren()`, `rewind()` from `next()`, ...).
//!
//! The hook methods (`beginIteration`, `endIteration`, `callHasChildren`,
//! `callGetChildren`, `beginChildren`, `endChildren`, `nextElement`) are
//! called only when the object's class overrides them (php-src caches the
//! function pointers at construction and drops those declared by the base
//! class, whose implementations do nothing).
//!
//! # Interpreter hooks
//! * [`recursive_it_forward_target`] — `spl_recursive_it_get_method`.
//! * [`recursive_it_check_foreach`] — `spl_recursive_it_get_iterator`.
//! * `clone_obj` is [`clone_forbidden`].

use std::rc::Rc;

use php_runtime::class::{ClassConstant, ClassEntry};
use php_runtime::{Args, ClassRef, NativeFunction, ObjRef, PResult, PhpStr, Registry, Value, Vm};

use super::dual::INVALID_STATE;
use super::inner::{
    InnerIter, Pending, clone_forbidden, get_iterator_from_aggregate, object_init_with_constructor,
};
use crate::spl::engine::{self, add_methods, implement, is_overridden};

pub const RIT_LEAVES_ONLY: i64 = 0;
pub const RIT_SELF_FIRST: i64 = 1;
pub const RIT_CHILD_FIRST: i64 = 2;
pub const RIT_CATCH_GET_CHILD: i64 = 0x10;

pub const RTIT_BYPASS_CURRENT: i64 = 4;
pub const RTIT_BYPASS_KEY: i64 = 8;

const BASE: &str = "RecursiveIteratorIterator";

/// `RecursiveIteratorState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RsState {
    Next,
    Test,
    SelfFirst,
    Child,
    Start,
}

/// `spl_sub_iterator`: one level of the recursion.
pub struct SubIter {
    /// The iterator over `zobject` (its object is `zobject`).
    pub iter: Rc<InnerIter>,
    /// `ce`: the class of `zobject`.
    pub ce: ClassRef,
    pub state: RsState,
}

/// Which hook methods the object's class overrides.
#[derive(Clone, Copy, Default)]
struct Hooks {
    begin_iteration: bool,
    end_iteration: bool,
    call_has_children: bool,
    call_get_children: bool,
    begin_children: bool,
    end_children: bool,
    next_element: bool,
}

/// `spl_recursive_it_object`.
pub struct RecIt {
    /// `iterators` (`None` until constructed); the level is `len - 1`.
    pub iterators: Option<Vec<SubIter>>,
    /// `level` while `iterators` is `NULL`.
    uninit_level: i64,
    pub mode: i64,
    pub flags: i64,
    pub max_depth: i64,
    pub in_iteration: bool,
    hooks: Hooks,
    /// RecursiveTreeIterator prefix parts and postfix.
    pub prefix: [PhpStr; 6],
    pub postfix: PhpStr,
}

impl RecIt {
    fn new(tree: bool) -> RecIt {
        let prefix = if tree {
            ["", "| ", "  ", "|-", "\\-", ""].map(PhpStr::from)
        } else {
            std::array::from_fn(|_| PhpStr::new())
        };
        RecIt {
            iterators: None,
            uninit_level: 0,
            mode: RIT_LEAVES_ONLY,
            flags: 0,
            max_depth: -1,
            in_iteration: false,
            hooks: Hooks::default(),
            prefix,
            postfix: PhpStr::new(),
        }
    }

    /// `object->level`.
    pub fn level(&self) -> i64 {
        match &self.iterators {
            Some(v) => v.len() as i64 - 1,
            None => self.uninit_level,
        }
    }

    fn top(&self) -> Option<&SubIter> {
        self.iterators.as_ref().and_then(|v| v.last())
    }

    fn top_mut(&mut self) -> Option<&mut SubIter> {
        self.iterators.as_mut().and_then(|v| v.last_mut())
    }
}

// ---------------------------------------------------------------------------
// State access
// ---------------------------------------------------------------------------

/// `spl_RecursiveIteratorIterator_new`.
fn create_rit(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(RecIt::new(false));
    Ok(obj)
}

/// `spl_RecursiveTreeIterator_new` (initialises the prefix parts).
fn create_rtit(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(RecIt::new(true));
    Ok(obj)
}

/// Runs `f` on the state (a short borrow: never call PHP code in `f`).
pub fn with<R>(obj: &ObjRef, f: impl FnOnce(&mut RecIt) -> R) -> R {
    if obj.native::<RecIt>().is_none() {
        obj.set_native(RecIt::new(false));
    }
    let mut st = obj.native_mut::<RecIt>().expect("recursive iterator state");
    f(&mut st)
}

fn initialized(obj: &ObjRef) -> bool {
    obj.native::<RecIt>().is_some_and(|s| s.iterators.is_some())
}

fn invalid_state(vm: &mut Vm) -> php_runtime::Flow {
    vm.throw_error("Error", INVALID_STATE)
}

/// The iterator of the current level.
fn top_iter(obj: &ObjRef) -> Option<Rc<InnerIter>> {
    with(obj, |s| s.top().map(|t| t.iter.clone()))
}

/// `SPL_FETCH_SUB_ITERATOR`.
fn top_iter_checked(vm: &mut Vm, obj: &ObjRef) -> PResult<Rc<InnerIter>> {
    match top_iter(obj) {
        Some(it) => Ok(it),
        None => Err(invalid_state(vm)),
    }
}

fn level_of(obj: &ObjRef) -> i64 {
    with(obj, |s| s.level())
}

fn set_state(obj: &ObjRef, state: RsState) {
    with(obj, |s| {
        if let Some(t) = s.top_mut() {
            t.state = state;
        }
    });
}

fn hooks(obj: &ObjRef) -> Hooks {
    with(obj, |s| s.hooks)
}

fn catches(obj: &ObjRef) -> bool {
    with(obj, |s| s.flags & RIT_CATCH_GET_CHILD != 0)
}

fn call_this(vm: &mut Vm, this: &ObjRef, name: &[u8]) -> PResult<Value> {
    vm.call_method(this, name, Vec::new())
}

// ---------------------------------------------------------------------------
// The iteration engine
// ---------------------------------------------------------------------------

/// `spl_recursive_it_valid_ex`.
fn valid_ex(vm: &mut Vm, this: &ObjRef) -> PResult<bool> {
    if !initialized(this) {
        return Ok(false);
    }
    let mut ex = Pending::default();
    let mut level = level_of(this);
    while level >= 0 {
        let it = with(this, |s| {
            s.iterators
                .as_ref()
                .and_then(|v| v.get(level as usize))
                .map(|t| t.iter.clone())
        });
        if let Some(it) = it {
            if ex.run(|| it.valid(vm))? == Some(true) {
                return Ok(true);
            }
        }
        level -= 1;
    }
    let (end_iteration, in_iteration) = with(this, |s| (s.hooks.end_iteration, s.in_iteration));
    if end_iteration && in_iteration {
        ex.run(|| call_this(vm, this, b"endIteration"))?;
    }
    with(this, |s| s.in_iteration = false);
    ex.finish()?;
    Ok(false)
}

/// Outcome of the `switch` of `spl_recursive_it_move_forward_ex`.
enum Step {
    /// `goto next_step`.
    NextStep,
    /// `break`: the level has no more elements.
    NoMore,
}

/// `spl_recursive_it_move_forward_ex`.
fn move_forward_ex(vm: &mut Vm, this: &ObjRef) -> PResult<()> {
    if !initialized(this) {
        return Err(invalid_state(vm));
    }
    let mut iterator;
    let mut ex = Pending::default();
    while !ex.is_set() {
        loop {
            // next_step:
            let Some((it, state)) = with(this, |s| s.top().map(|t| (t.iter.clone(), t.state)))
            else {
                return ex.finish();
            };
            iterator = it;
            match step(vm, this, &mut ex, &mut iterator, state)? {
                Some(Step::NextStep) => continue,
                Some(Step::NoMore) => break,
                None => return ex.finish(),
            }
        }
        // No more elements.
        if level_of(this) > 0 {
            if hooks(this).end_children {
                ex.run(|| call_this(vm, this, b"endChildren"))?;
                if ex.is_set() {
                    if !catches(this) {
                        return ex.finish();
                    }
                    ex.clear();
                }
            }
            let garbage = with(this, |s| {
                let its = s.iterators.as_mut()?;
                if its.len() > 1 && its.last().is_some_and(|t| Rc::ptr_eq(&t.iter, &iterator)) {
                    its.pop()
                } else {
                    None
                }
            });
            drop(garbage);
        } else {
            // Done completely.
            return ex.finish();
        }
    }
    ex.finish()
}

/// One pass through the `switch (state)` with its fall-throughs. `None`
/// means "return from `move_forward_ex`".
fn step(
    vm: &mut Vm,
    this: &ObjRef,
    ex: &mut Pending,
    iterator: &mut Rc<InnerIter>,
    mut state: RsState,
) -> PResult<Option<Step>> {
    loop {
        match state {
            RsState::Next => {
                let it = iterator.clone();
                ex.run(|| it.next(vm))?;
                if ex.is_set() {
                    if !catches(this) {
                        return Ok(None);
                    }
                    ex.clear();
                }
                match top_iter(this) {
                    Some(it) => *iterator = it,
                    None => return Ok(None),
                }
                state = RsState::Start;
            }
            RsState::Start => {
                let prev_level = level_of(this);
                let it = iterator.clone();
                let valid = ex.run(|| it.valid(vm))? == Some(true);
                let reentered = level_of(this) != prev_level
                    || !top_iter(this).is_some_and(|t| Rc::ptr_eq(&t, iterator));
                if reentered {
                    return Ok(None);
                }
                if !valid {
                    return Ok(Some(Step::NoMore));
                }
                set_state(this, RsState::Test);
                state = RsState::Test;
            }
            RsState::Test => {
                let h = hooks(this);
                let retval = if h.call_has_children {
                    ex.run(|| call_this(vm, this, b"callHasChildren"))?
                } else {
                    let zobj = iterator.object().clone();
                    let zobj =
                        with(this, |s| s.top().map(|t| t.iter.object().clone())).unwrap_or(zobj);
                    ex.run(|| vm.call_method(&zobj, b"hasChildren", Vec::new()))?
                };
                if ex.is_set() {
                    if !catches(this) {
                        set_state(this, RsState::Next);
                        return Ok(None);
                    }
                    ex.clear();
                }
                if let Some(retval) = retval {
                    if vm.to_bool(&retval.into_unref())? {
                        let (max_depth, mode, level) =
                            with(this, |s| (s.max_depth, s.mode, s.level()));
                        if max_depth == -1 || max_depth > level {
                            match mode {
                                RIT_LEAVES_ONLY | RIT_CHILD_FIRST => {
                                    set_state(this, RsState::Child);
                                    return Ok(Some(Step::NextStep));
                                }
                                RIT_SELF_FIRST => {
                                    set_state(this, RsState::SelfFirst);
                                    return Ok(Some(Step::NextStep));
                                }
                                _ => {}
                            }
                        } else if mode == RIT_LEAVES_ONLY {
                            // This is not a leaf, so skip it.
                            set_state(this, RsState::Next);
                            return Ok(Some(Step::NextStep));
                        }
                    }
                }
                if h.next_element {
                    ex.run(|| call_this(vm, this, b"nextElement"))?;
                }
                set_state(this, RsState::Next);
                if ex.is_set() {
                    if !catches(this) {
                        return Ok(None);
                    }
                    ex.clear();
                }
                return Ok(None);
            }
            RsState::SelfFirst => {
                let mode = with(this, |s| s.mode);
                if hooks(this).next_element && (mode == RIT_SELF_FIRST || mode == RIT_CHILD_FIRST) {
                    ex.run(|| call_this(vm, this, b"nextElement"))?;
                }
                set_state(
                    this,
                    if mode == RIT_SELF_FIRST {
                        RsState::Child
                    } else {
                        RsState::Next
                    },
                );
                return Ok(None);
            }
            RsState::Child => {
                let h = hooks(this);
                let child = if h.call_get_children {
                    ex.run(|| call_this(vm, this, b"callGetChildren"))?
                } else {
                    let zobj = with(this, |s| s.top().map(|t| t.iter.object().clone()))
                        .unwrap_or_else(|| iterator.object().clone());
                    ex.run(|| vm.call_method(&zobj, b"getChildren", Vec::new()))?
                };
                if ex.is_set() {
                    if !catches(this) {
                        return Ok(None);
                    }
                    ex.clear();
                    set_state(this, RsState::Next);
                    return Ok(Some(Step::NextStep));
                }
                let child = match child.map(Value::into_unref) {
                    Some(Value::Object(o)) if o.class().instance_of_name(b"RecursiveIterator") => o,
                    _ => {
                        ex.throw(vm.throw_error(
                            "UnexpectedValueException",
                            "Objects returned by RecursiveIterator::getChildren() must implement RecursiveIterator",
                        ));
                        return Ok(None);
                    }
                };
                let mode = with(this, |s| s.mode);
                set_state(
                    this,
                    if mode == RIT_CHILD_FIRST {
                        RsState::SelfFirst
                    } else {
                        RsState::Next
                    },
                );
                let sub_iter = InnerIter::over(child.clone());
                let ce = child.class().clone();
                with(this, |s| {
                    if let Some(its) = s.iterators.as_mut() {
                        its.push(SubIter {
                            iter: sub_iter.clone(),
                            ce,
                            state: RsState::Start,
                        });
                    }
                });
                ex.run(|| sub_iter.rewind(vm))?;
                if h.begin_children {
                    ex.run(|| call_this(vm, this, b"beginChildren"))?;
                    if ex.is_set() {
                        if !catches(this) {
                            return Ok(None);
                        }
                        ex.clear();
                    }
                }
                return Ok(Some(Step::NextStep));
            }
        }
    }
}

/// `spl_recursive_it_rewind_ex`.
fn rewind_ex(vm: &mut Vm, this: &ObjRef) -> PResult<()> {
    if !initialized(this) {
        return Err(invalid_state(vm));
    }
    let mut ex = Pending::default();
    loop {
        let popped = with(this, |s| match s.iterators.as_mut() {
            Some(its) if its.len() > 1 => its.pop(),
            _ => None,
        });
        if popped.is_none() {
            break;
        }
        drop(popped);
        if hooks(this).end_children {
            ex.run(|| call_this(vm, this, b"endChildren"))?;
        }
    }
    let first = with(this, |s| {
        let its = s.iterators.as_mut()?;
        its.truncate(1);
        let first = its.first_mut()?;
        first.state = RsState::Start;
        Some(first.iter.clone())
    });
    if let Some(first) = first {
        ex.run(|| first.rewind(vm))?;
    }
    let (begin_iteration, in_iteration) = with(this, |s| (s.hooks.begin_iteration, s.in_iteration));
    if begin_iteration && !in_iteration {
        ex.run(|| call_this(vm, this, b"beginIteration"))?;
    }
    with(this, |s| s.in_iteration = true);
    if ex.is_set() {
        // move_forward_ex() does nothing while an exception is pending.
        return ex.finish();
    }
    move_forward_ex(vm, this)
}

// ---------------------------------------------------------------------------
// Interpreter hooks
// ---------------------------------------------------------------------------

/// `spl_recursive_it_get_method`. The interpreter must call this for every
/// method call `$obj->m()` on an object of (a subclass of)
/// `RecursiveIteratorIterator`, before resolving `m`:
/// * `Err(..)`: throw it — the object was not initialised ("The %s
///   instance wasn't initialized properly", even for existing methods);
/// * `Ok(Some(sub))`: if `m` is not found on `$obj` (no method, no
///   `__call`), dispatch the call to `sub` (the sub-iterator of the current
///   level) instead, with its normal lookup;
/// * `Ok(None)`: `obj` is not a recursive iterator.
pub fn recursive_it_forward_target(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<ObjRef>> {
    let target = {
        let Some(st) = obj.native::<RecIt>() else {
            return Ok(None);
        };
        match &st.iterators {
            Some(_) => st.top().map(|t| t.iter.object().clone()),
            None => None,
        }
    };
    match target {
        Some(t) => Ok(Some(t)),
        None => Err(vm.throw_error(
            "Error",
            format!(
                "The {} instance wasn't initialized properly",
                obj.class().name
            ),
        )),
    }
}

/// `spl_recursive_it_get_iterator`: the checks `foreach` makes before
/// iterating a `RecursiveIteratorIterator` (through its methods): by
/// reference is an error, and so is an object whose constructor did not
/// run ("Object is not initialized"). Returns `Ok(())` for other objects.
pub fn recursive_it_check_foreach(vm: &mut Vm, obj: &ObjRef, by_ref: bool) -> PResult<()> {
    let Some(init) = obj.native::<RecIt>().map(|s| s.iterators.is_some()) else {
        return Ok(());
    };
    if by_ref {
        return Err(vm.throw_error(
            "Error",
            "An iterator cannot be used with foreach by reference",
        ));
    }
    if !init {
        return Err(vm.throw_error("Error", "Object is not initialized"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// RecursiveIteratorIterator methods
// ---------------------------------------------------------------------------

/// `spl_recursive_it_it_construct`.
fn rit_construct_common(vm: &mut Vm, args: &Args, tree: bool) -> PResult<()> {
    let this = engine::this(args);
    let mut iterator = args.object(0).clone();
    let (mode, flags) = if tree {
        let flags = args.int(1);
        let caching_flags = args.int(2);
        let mode = args.int(3);
        if iterator.class().instance_of_name(b"IteratorAggregate") {
            let ce = iterator.class().clone();
            iterator = get_iterator_from_aggregate(vm, &ce, &iterator)?;
        }
        let rci = vm
            .find_class(b"RecursiveCachingIterator")
            .expect("RecursiveCachingIterator is registered");
        iterator = object_init_with_constructor(
            vm,
            &rci,
            vec![Value::Object(iterator), Value::Int(caching_flags)],
        )?;
        (mode, flags)
    } else {
        let mode = args.int(1);
        let flags = args.int(2);
        if iterator.class().instance_of_name(b"IteratorAggregate") {
            let ce = iterator.class().clone();
            iterator = get_iterator_from_aggregate(vm, &ce, &iterator)?;
        }
        (mode, flags)
    };
    if !iterator.class().instance_of_name(b"RecursiveIterator") {
        return Err(vm.throw_error(
            "InvalidArgumentException",
            "An instance of RecursiveIterator or IteratorAggregate creating it is required",
        ));
    }
    let ce = this.class().clone();
    let hooks = Hooks {
        begin_iteration: is_overridden(&ce, b"beginiteration", BASE),
        end_iteration: is_overridden(&ce, b"enditeration", BASE),
        call_has_children: is_overridden(&ce, b"callhaschildren", BASE),
        call_get_children: is_overridden(&ce, b"callgetchildren", BASE),
        begin_children: is_overridden(&ce, b"beginchildren", BASE),
        end_children: is_overridden(&ce, b"endchildren", BASE),
        next_element: is_overridden(&ce, b"nextelement", BASE),
    };
    let sub = SubIter {
        iter: InnerIter::over(iterator.clone()),
        ce: iterator.class().clone(),
        state: RsState::Start,
    };
    let old = with(&this, |s| {
        let old = s.iterators.replace(vec![sub]);
        // `mode` is an enum and `flags` an int in php-src.
        s.mode = mode as i32 as i64;
        s.flags = flags as i32 as i64;
        s.max_depth = -1;
        s.in_iteration = false;
        s.hooks = hooks;
        old
    });
    drop(old);
    Ok(())
}

fn rit_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    rit_construct_common(vm, args, false)?;
    Ok(Value::Null)
}

fn rit_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    rewind_ex(vm, &this)?;
    Ok(Value::Null)
}

fn rit_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    Ok(Value::Bool(valid_ex(vm, &this)?))
}

fn rit_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let it = top_iter_checked(vm, &this)?;
    it.key(vm)
}

fn rit_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let it = top_iter_checked(vm, &this)?;
    Ok(it.current(vm)?.unref())
}

fn rit_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    move_forward_ex(vm, &this)?;
    Ok(Value::Null)
}

fn rit_get_depth(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    Ok(Value::Int(level_of(&this)))
}

fn rit_get_sub_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let cur = level_of(&this);
    let level = match args.int_opt(0) {
        None => cur,
        Some(l) if l < 0 || l > cur => return Ok(Value::Null),
        Some(l) => l,
    };
    if !initialized(&this) {
        return Err(invalid_state(vm));
    }
    Ok(with(&this, |s| {
        s.iterators
            .as_ref()
            .and_then(|v| v.get(level as usize))
            .map_or(Value::Null, |t| Value::Object(t.iter.object().clone()))
    }))
}

fn rit_get_inner_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let it = top_iter_checked(vm, &this)?;
    Ok(Value::Object(it.object().clone()))
}

/// The hooks' base implementations do nothing.
fn rit_noop(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

fn rit_call_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let Some(it) = top_iter(&this) else {
        return Ok(Value::Bool(false));
    };
    let zobj = it.object().clone();
    Ok(vm
        .call_method(&zobj, b"hasChildren", Vec::new())?
        .into_unref())
}

fn rit_call_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let it = top_iter_checked(vm, &this)?;
    let zobj = it.object().clone();
    Ok(vm
        .call_method(&zobj, b"getChildren", Vec::new())?
        .into_unref())
}

fn rit_set_max_depth(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let max_depth = args.int(0);
    if max_depth < -1 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to -1"));
    }
    let max_depth = max_depth.min(i64::from(i32::MAX));
    with(&this, |s| s.max_depth = max_depth);
    Ok(Value::Null)
}

fn rit_get_max_depth(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    Ok(match with(&this, |s| s.max_depth) {
        -1 => Value::Bool(false),
        d => Value::Int(d),
    })
}

static RIT_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Traversable $iterator, int $mode = RecursiveIteratorIterator::LEAVES_ONLY, int $flags = 0) {}",
    handler: rit_construct,
};
static RIT_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: rit_rewind,
};
static RIT_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: rit_valid,
};
static RIT_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: rit_key,
};
static RIT_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: rit_current,
};
static RIT_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: rit_next,
};
static RIT_GET_DEPTH: NativeFunction = NativeFunction {
    sig: "public function getDepth(): int {}",
    handler: rit_get_depth,
};
static RIT_GET_SUB_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getSubIterator(?int $level = null): ?RecursiveIterator {}",
    handler: rit_get_sub_iterator,
};
static RIT_GET_INNER_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getInnerIterator(): RecursiveIterator {}",
    handler: rit_get_inner_iterator,
};
static RIT_BEGIN_ITERATION: NativeFunction = NativeFunction {
    sig: "public function beginIteration(): void {}",
    handler: rit_noop,
};
static RIT_END_ITERATION: NativeFunction = NativeFunction {
    sig: "public function endIteration(): void {}",
    handler: rit_noop,
};
static RIT_CALL_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function callHasChildren(): bool {}",
    handler: rit_call_has_children,
};
static RIT_CALL_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function callGetChildren(): ?RecursiveIterator {}",
    handler: rit_call_get_children,
};
static RIT_BEGIN_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function beginChildren(): void {}",
    handler: rit_noop,
};
static RIT_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function endChildren(): void {}",
    handler: rit_noop,
};
static RIT_NEXT_ELEMENT: NativeFunction = NativeFunction {
    sig: "public function nextElement(): void {}",
    handler: rit_noop,
};
static RIT_SET_MAX_DEPTH: NativeFunction = NativeFunction {
    sig: "public function setMaxDepth(int $maxDepth = -1): void {}",
    handler: rit_set_max_depth,
};
static RIT_GET_MAX_DEPTH: NativeFunction = NativeFunction {
    sig: "public function getMaxDepth(): int|false {}",
    handler: rit_get_max_depth,
};

// ---------------------------------------------------------------------------
// RecursiveTreeIterator
// ---------------------------------------------------------------------------

/// `spl_recursive_tree_iterator_get_prefix`.
fn tree_prefix(vm: &mut Vm, this: &ObjRef) -> PResult<PhpStr> {
    let part = |i: usize| with(this, |s| s.prefix[i].clone());
    let mut out = part(0).to_vec();
    let has_next = |vm: &mut Vm, level: i64| -> PResult<Option<bool>> {
        let zobj = with(this, |s| {
            s.iterators
                .as_ref()
                .and_then(|v| v.get(level as usize))
                .map(|t| t.iter.object().clone())
        });
        match zobj {
            Some(z) => {
                let r = vm.call_method(&z, b"hasNext", Vec::new())?;
                Ok(Some(matches!(r.into_unref(), Value::Bool(true))))
            }
            None => Ok(None),
        }
    };
    let mut level = 0;
    while level < level_of(this) {
        if let Some(next) = has_next(vm, level)? {
            out.extend_from_slice(&part(if next { 1 } else { 2 }));
        }
        level += 1;
    }
    if let Some(next) = has_next(vm, level)? {
        out.extend_from_slice(&part(if next { 3 } else { 4 }));
    }
    out.extend_from_slice(&part(5));
    Ok(PhpStr::from_vec(out))
}

/// `spl_recursive_tree_iterator_get_entry`: arrays read as "Array"
/// without a warning.
fn tree_entry(vm: &mut Vm, this: &ObjRef) -> PResult<PhpStr> {
    let it = top_iter_checked(vm, this)?;
    let data = it.current(vm)?.unref();
    if matches!(data, Value::Array(_)) {
        return Ok(PhpStr::from("Array"));
    }
    vm.to_php_string(&data)
}

fn tree_postfix(this: &ObjRef) -> PhpStr {
    with(this, |s| s.postfix.clone())
}

fn concat3(a: &[u8], b: &[u8], c: &[u8]) -> Value {
    let mut out = Vec::with_capacity(a.len() + b.len() + c.len());
    out.extend_from_slice(a);
    out.extend_from_slice(b);
    out.extend_from_slice(c);
    Value::String(PhpStr::from_vec(out))
}

fn rtit_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    rit_construct_common(vm, args, true)?;
    Ok(Value::Null)
}

fn rtit_set_prefix_part(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let part = args.int(0);
    let prefix = args.str(1).clone();
    if !(0..=5).contains(&part) {
        return Err(
            vm.argument_value_error(1, "must be a RecursiveTreeIterator::PREFIX_* constant")
        );
    }
    with(&this, |s| s.prefix[part as usize] = prefix);
    Ok(Value::Null)
}

fn rtit_get_prefix(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if !initialized(&this) {
        return Err(invalid_state(vm));
    }
    Ok(Value::String(tree_prefix(vm, &this)?))
}

fn rtit_set_postfix(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let postfix = args.str(0).clone();
    with(&this, |s| s.postfix = postfix);
    Ok(Value::Null)
}

fn rtit_get_entry(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if !initialized(&this) {
        return Err(invalid_state(vm));
    }
    Ok(Value::String(tree_entry(vm, &this)?))
}

fn rtit_get_postfix(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if !initialized(&this) {
        return Err(invalid_state(vm));
    }
    Ok(Value::String(tree_postfix(&this)))
}

/// The current element prefixed and postfixed.
fn rtit_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if !initialized(&this) {
        return Err(invalid_state(vm));
    }
    if with(&this, |s| s.flags & RTIT_BYPASS_CURRENT != 0) {
        let it = top_iter_checked(vm, &this)?;
        return Ok(it.current(vm)?.unref());
    }
    let entry = tree_entry(vm, &this)?;
    let prefix = tree_prefix(vm, &this)?;
    let postfix = tree_postfix(&this);
    Ok(concat3(&prefix, &entry, &postfix))
}

/// The current key prefixed and postfixed.
fn rtit_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let it = top_iter_checked(vm, &this)?;
    let key = it.key(vm)?;
    if with(&this, |s| s.flags & RTIT_BYPASS_KEY != 0) {
        return Ok(key);
    }
    let key = vm.to_php_string(&key)?;
    let prefix = tree_prefix(vm, &this)?;
    let postfix = tree_postfix(&this);
    Ok(concat3(&prefix, &key, &postfix))
}

static RTIT_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(RecursiveIterator|IteratorAggregate $iterator, int $flags = RecursiveTreeIterator::BYPASS_KEY, int $cachingIteratorFlags = CachingIterator::CATCH_GET_CHILD, int $mode = RecursiveTreeIterator::SELF_FIRST) {}",
    handler: rtit_construct,
};
static RTIT_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: rtit_key,
};
static RTIT_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: rtit_current,
};
static RTIT_GET_PREFIX: NativeFunction = NativeFunction {
    sig: "public function getPrefix(): string {}",
    handler: rtit_get_prefix,
};
static RTIT_SET_POSTFIX: NativeFunction = NativeFunction {
    sig: "public function setPostfix(string $postfix): void {}",
    handler: rtit_set_postfix,
};
static RTIT_SET_PREFIX_PART: NativeFunction = NativeFunction {
    sig: "public function setPrefixPart(int $part, string $value): void {}",
    handler: rtit_set_prefix_part,
};
static RTIT_GET_ENTRY: NativeFunction = NativeFunction {
    sig: "public function getEntry(): string {}",
    handler: rtit_get_entry,
};
static RTIT_GET_POSTFIX: NativeFunction = NativeFunction {
    sig: "public function getPostfix(): string {}",
    handler: rtit_get_postfix,
};

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register_recursive_iterator_iterator(r: &mut Registry) {
    let mut ce = ClassEntry::new(BASE, 0);
    let outer = engine::class(r, super::OUTER_ITERATOR);
    implement(&mut ce, &outer);
    for (name, value) in [
        ("LEAVES_ONLY", RIT_LEAVES_ONLY),
        ("SELF_FIRST", RIT_SELF_FIRST),
        ("CHILD_FIRST", RIT_CHILD_FIRST),
        ("CATCH_GET_CHILD", RIT_CATCH_GET_CHILD),
    ] {
        ce.add_constant(ClassConstant::new(name, Value::Int(value), BASE));
    }
    add_methods(
        &mut ce,
        &[
            &RIT_CONSTRUCT,
            &RIT_REWIND,
            &RIT_VALID,
            &RIT_KEY,
            &RIT_CURRENT,
            &RIT_NEXT,
            &RIT_GET_DEPTH,
            &RIT_GET_SUB_ITERATOR,
            &RIT_GET_INNER_ITERATOR,
            &RIT_BEGIN_ITERATION,
            &RIT_END_ITERATION,
            &RIT_CALL_HAS_CHILDREN,
            &RIT_CALL_GET_CHILDREN,
            &RIT_BEGIN_CHILDREN,
            &RIT_END_CHILDREN,
            &RIT_NEXT_ELEMENT,
            &RIT_SET_MAX_DEPTH,
            &RIT_GET_MAX_DEPTH,
        ],
    );
    ce.handlers.create_object = Some(create_rit);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}

pub fn register_recursive_tree_iterator(r: &mut Registry) {
    let parent = engine::class(r, BASE);
    let mut ce = ClassEntry::new_subclass("RecursiveTreeIterator", 0, &parent);
    for (name, value) in [
        ("BYPASS_CURRENT", RTIT_BYPASS_CURRENT),
        ("BYPASS_KEY", RTIT_BYPASS_KEY),
        ("PREFIX_LEFT", 0),
        ("PREFIX_MID_HAS_NEXT", 1),
        ("PREFIX_MID_LAST", 2),
        ("PREFIX_END_HAS_NEXT", 3),
        ("PREFIX_END_LAST", 4),
        ("PREFIX_RIGHT", 5),
    ] {
        ce.add_constant(ClassConstant::new(
            name,
            Value::Int(value),
            "RecursiveTreeIterator",
        ));
    }
    add_methods(
        &mut ce,
        &[
            &RTIT_CONSTRUCT,
            &RTIT_KEY,
            &RTIT_CURRENT,
            &RTIT_GET_PREFIX,
            &RTIT_SET_POSTFIX,
            &RTIT_SET_PREFIX_PART,
            &RTIT_GET_ENTRY,
            &RTIT_GET_POSTFIX,
        ],
    );
    ce.handlers.create_object = Some(create_rtit);
    r.class(ce);
}
