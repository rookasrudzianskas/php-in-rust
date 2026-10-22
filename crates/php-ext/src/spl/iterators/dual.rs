//! `spl_dual_it` — the iterators wrapping one inner iterator:
//! `IteratorIterator`, `FilterIterator`, `RecursiveFilterIterator`,
//! `ParentIterator`, `CallbackFilterIterator`,
//! `RecursiveCallbackFilterIterator`, `NoRewindIterator`,
//! `InfiniteIterator` and the shared state used by `LimitIterator`
//! ([`super::limit`]), `CachingIterator` ([`super::caching`]),
//! `AppendIterator` ([`super::append`]) and `RegexIterator`
//! ([`super::regex`]).
//!
//! The state ([`DualIt`]) is created by the `create_object` handler
//! (`spl_dual_it_new`) with `dit_type` unknown; the constructors
//! ([`construct`], `spl_dual_it_construct`) fill it in. Methods of an object
//! whose constructor was not run throw "The object is in an invalid state as
//! the parent constructor was not called" ([`checked`]).
//!
//! The inner iterator is an [`InnerIter`]; the current element is copied
//! into the state by [`fetch`] (`spl_dual_it_fetch`) and released by
//! [`free`] (`spl_dual_it_free`). No `RefCell` borrow of the state is held
//! while PHP code runs.
//!
//! # Interpreter hooks
//! * [`dual_it_forward_target`] — `spl_dual_it_get_method`.
//! * `clone_obj` is [`clone_forbidden`] (php-src: `clone_obj = NULL`).
//! * `ArrayAccess`, `Countable` and `__toString` (CachingIterator) use the
//!   standard handlers, which call the `offset*()` / `count()` /
//!   `__toString()` methods, as in php-src: no dimension, count or cast
//!   handlers are installed.

use std::rc::Rc;

use php_runtime::class::{ClassEntry, class_flags};
use php_runtime::{
    Args, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};

use super::inner::{
    InnerIter, clone_forbidden, get_iterator_from_aggregate, object_init_with_constructor,
};
use super::regex::RegexState;
use crate::spl::engine::{self, abstract_method, add_methods, implement};

/// `SPL_FETCH_AND_CHECK_DUAL_IT` failure message.
pub const INVALID_STATE: &str =
    "The object is in an invalid state as the parent constructor was not called";

/// `dual_it_type` (`DIT_Unknown` is `DualIt::dit_type == None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DitType {
    /// `DIT_Default`: FilterIterator, RecursiveFilterIterator, ParentIterator.
    Default,
    Limit,
    Caching,
    RecursiveCaching,
    IteratorIterator,
    NoRewind,
    Infinite,
    Append,
    Regex,
    RecursiveRegex,
    CallbackFilter,
    RecursiveCallbackFilter,
}

/// `u.caching` of `spl_dual_it_object`.
pub struct CachingState {
    /// `CIT_*` flags (public ones plus `CIT_VALID`).
    pub flags: i64,
    /// String value of the current element (`CALL_TOSTRING` / `TOSTRING_USE_INNER`).
    pub zstr: Option<PhpStr>,
    /// Children of the current element (RecursiveCachingIterator).
    pub zchildren: Option<ObjRef>,
    /// The `FULL_CACHE` cache.
    pub zcache: Rc<PhpArray>,
}

impl Default for CachingState {
    fn default() -> Self {
        CachingState {
            flags: 0,
            zstr: None,
            zchildren: None,
            zcache: Rc::new(PhpArray::new()),
        }
    }
}

/// `u.append` of `spl_dual_it_object`.
#[derive(Default)]
pub struct AppendState {
    /// The `ArrayIterator` holding the appended iterators.
    pub zarrayit: Option<ObjRef>,
    /// The iterator over `zarrayit`.
    pub iter: Option<Rc<InnerIter>>,
}

/// `spl_dual_it_object`.
#[derive(Default)]
pub struct DualIt {
    /// `None` until a constructor ran (`DIT_Unknown`).
    pub dit_type: Option<DitType>,
    /// `inner.zobject`.
    pub inner_obj: Option<ObjRef>,
    /// `inner.ce` (the downcast class for `IteratorIterator`).
    pub inner_ce: Option<ClassRef>,
    /// `inner.iterator`.
    pub inner_iter: Option<Rc<InnerIter>>,
    /// `current.data` (`None` is `IS_UNDEF`).
    pub data: Option<Value>,
    /// `current.key`.
    pub key: Option<Value>,
    /// `current.pos`.
    pub pos: i64,
    /// `u.limit.offset` / `u.limit.count`.
    pub limit_offset: i64,
    pub limit_count: i64,
    pub caching: CachingState,
    pub append: AppendState,
    pub regex: Option<RegexState>,
    /// `u.callback_filter`.
    pub callback: Option<Value>,
}

// ---------------------------------------------------------------------------
// State access
// ---------------------------------------------------------------------------

/// `spl_dual_it_new` (the `create_object` handler of every dual iterator).
pub fn create_dual(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(DualIt::default());
    Ok(obj)
}

/// Runs `f` on the state of `obj` (a short borrow: never call PHP code in `f`).
pub fn with<R>(obj: &ObjRef, f: impl FnOnce(&mut DualIt) -> R) -> R {
    if obj.native::<DualIt>().is_none() {
        obj.set_native(DualIt::default());
    }
    let mut st = obj.native_mut::<DualIt>().expect("dual iterator state");
    f(&mut st)
}

/// Whether `obj` carries dual-iterator state.
pub fn is_dual(obj: &ObjRef) -> bool {
    obj.native::<DualIt>().is_some()
}

/// `SPL_FETCH_AND_CHECK_DUAL_IT`: `$this`, or the invalid-state `Error`.
pub fn checked(vm: &mut Vm, args: &Args) -> PResult<ObjRef> {
    let this = engine::this(args);
    let ok = this
        .native::<DualIt>()
        .is_some_and(|s| s.dit_type.is_some());
    if !ok {
        return Err(vm.throw_error("Error", INVALID_STATE));
    }
    Ok(this)
}

pub fn inner_iter(obj: &ObjRef) -> Option<Rc<InnerIter>> {
    with(obj, |s| s.inner_iter.clone())
}

pub fn inner_obj(obj: &ObjRef) -> Option<ObjRef> {
    with(obj, |s| s.inner_obj.clone())
}

pub fn dit_type(obj: &ObjRef) -> Option<DitType> {
    with(obj, |s| s.dit_type)
}

/// `inner.zobject` of a constructed iterator (`$this->getInnerIterator()`).
fn inner_obj_checked(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    match inner_obj(obj) {
        Some(o) => Ok(o),
        None => Err(vm.throw_error(
            "Error",
            "The inner constructor wasn't initialized with an iterator instance",
        )),
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// `spl_dual_it_construct()`: validates the arguments for `dit` (the stub
/// signature already applied ZPP), then stores the inner iterator.
/// `ce_base` names the class in "must be called exactly once" errors and
/// `ce_inner` is the class the first argument must be an instance of.
pub fn construct(
    vm: &mut Vm,
    args: &Args,
    ce_base: &str,
    ce_inner: &str,
    dit: DitType,
) -> PResult<ObjRef> {
    let this = engine::this(args);
    if with(&this, |s| s.dit_type.is_some()) {
        return Err(vm.throw_error(
            "BadMethodCallException",
            format!("{ce_base}::getIterator() must be called exactly once per instance"),
        ));
    }
    let mut zobject = args.object(0).clone();
    // `zend_parse_parameters("O", ce_inner)` (stricter than some stubs).
    if !zobject.class().instance_of_name(ce_inner.as_bytes()) {
        let msg = format!(
            "must be of type {ce_inner}, {} given",
            Value::Object(zobject.clone()).value_name()
        );
        return Err(vm.argument_type_error(1, &msg));
    }
    let mut ce = zobject.class().clone();

    match dit {
        DitType::Limit => {
            let offset = args.int(1);
            let count = args.int(2);
            with(&this, |s| {
                s.limit_offset = offset;
                s.limit_count = count;
            });
            if offset < 0 {
                return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
            }
            if count < -1 {
                return Err(vm.argument_value_error(3, "must be greater than or equal to -1"));
            }
        }
        DitType::Caching | DitType::RecursiveCaching => {
            let flags = args.int(1);
            if !super::caching::check_flags(flags) {
                return Err(vm.argument_value_error(2, super::caching::FLAGS_ERROR));
            }
            with(&this, |s| {
                s.caching.flags |= flags & super::caching::CIT_PUBLIC;
                s.caching.zcache = Rc::new(PhpArray::new());
            });
        }
        DitType::IteratorIterator => {
            let class_name = args.str_opt(1).cloned();
            if !ce.instance_of_name(b"Iterator") {
                if let Some(class_name) = class_name {
                    let cast = vm.lookup_class(&class_name)?;
                    match cast {
                        Some(cast)
                            if ce.instance_of(&cast)
                                && !cast.is_interface()
                                && cast.instance_of_name(b"Traversable") =>
                        {
                            ce = cast;
                        }
                        _ => {
                            return Err(vm.throw_error(
                                "LogicException",
                                "Class to downcast to not found or not base class or does not implement Traversable",
                            ));
                        }
                    }
                }
                if ce.instance_of_name(b"IteratorAggregate") {
                    zobject = get_iterator_from_aggregate(vm, &ce, &zobject)?;
                    ce = zobject.class().clone();
                }
            }
        }
        DitType::Regex | DitType::RecursiveRegex => {
            let state = super::regex::construct_state(vm, args)?;
            with(&this, |s| s.regex = Some(state));
        }
        DitType::CallbackFilter | DitType::RecursiveCallbackFilter => {
            let callback = args.get(1).clone();
            with(&this, |s| s.callback = Some(callback));
        }
        _ => {}
    }

    let inner_ce = if dit == DitType::IteratorIterator {
        ce
    } else {
        zobject.class().clone()
    };
    with(&this, |s| {
        s.dit_type = Some(dit);
        s.inner_obj = Some(zobject.clone());
        s.inner_ce = Some(inner_ce);
    });
    let iter = InnerIter::new(vm, &zobject)?;
    with(&this, |s| s.inner_iter = Some(iter));
    Ok(this)
}

// ---------------------------------------------------------------------------
// spl_dual_it_* primitives
// ---------------------------------------------------------------------------

/// `spl_dual_it_free`.
pub fn free(obj: &ObjRef) {
    let it = with(obj, |s| {
        s.data = None;
        s.key = None;
        if matches!(
            s.dit_type,
            Some(DitType::Caching | DitType::RecursiveCaching)
        ) {
            s.caching.zstr = None;
            s.caching.zchildren = None;
        }
        s.inner_iter.clone()
    });
    if let Some(it) = it {
        it.invalidate_current();
    }
}

/// `spl_dual_it_rewind`.
pub fn rewind(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    free(obj);
    let it = with(obj, |s| {
        s.pos = 0;
        s.inner_iter.clone()
    });
    if let Some(it) = it {
        it.rewind(vm)?;
    }
    Ok(())
}

/// `spl_dual_it_valid`.
pub fn valid(vm: &mut Vm, obj: &ObjRef) -> PResult<bool> {
    match inner_iter(obj) {
        Some(it) => it.valid(vm),
        None => Ok(false),
    }
}

/// `spl_dual_it_fetch`: copies the inner iterator's current element and
/// key; `Ok(false)` when `check_more` found the iterator exhausted. A key
/// that throws leaves the data in place (php-src order).
pub fn fetch(vm: &mut Vm, obj: &ObjRef, check_more: bool) -> PResult<bool> {
    free(obj);
    if check_more && !valid(vm, obj)? {
        return Ok(false);
    }
    let Some(it) = inner_iter(obj) else {
        return Ok(false);
    };
    let data = it.current(vm)?.or_null();
    with(obj, |s| s.data = Some(data));
    let key = it.key(vm)?.or_null();
    with(obj, |s| s.key = Some(key));
    Ok(true)
}

/// `spl_dual_it_next`. The position advances even if `next()` throws.
pub fn next(vm: &mut Vm, obj: &ObjRef, do_free: bool) -> PResult<()> {
    if do_free {
        free(obj);
    } else if inner_iter(obj).is_none() {
        return Err(vm.throw_error(
            "Error",
            "The inner constructor wasn't initialized with an iterator instance",
        ));
    }
    let Some(it) = inner_iter(obj) else {
        return Ok(());
    };
    let r = it.next(vm);
    with(obj, |s| s.pos += 1);
    r
}

/// The current data (`RETURN_COPY_DEREF(&intern->current.data)`).
pub fn current_value(obj: &ObjRef) -> Value {
    with(obj, |s| s.data.as_ref().map_or(Value::Null, Value::unref))
}

/// The current key.
pub fn key_value(obj: &ObjRef) -> Value {
    with(obj, |s| s.key.as_ref().map_or(Value::Null, Value::unref))
}

/// `spl_filter_it_fetch`: skips elements until `accept()` returns true.
pub fn filter_fetch(vm: &mut Vm, this: &ObjRef) -> PResult<()> {
    loop {
        match fetch(vm, this, true) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                free(this);
                return Err(e);
            }
        }
        let accepted = vm.call_method(this, b"accept", Vec::new())?;
        if vm.to_bool(&accepted.into_unref())? {
            return Ok(());
        }
        // `inner.iterator->funcs->move_forward` (the position is not counted).
        if let Some(it) = inner_iter(this) {
            if let Err(e) = it.next(vm) {
                free(this);
                return Err(e);
            }
        }
    }
    free(this);
    Ok(())
}

// ---------------------------------------------------------------------------
// Interpreter hook
// ---------------------------------------------------------------------------

/// `spl_dual_it_get_method`: when a method called on a dual iterator (an
/// object whose class descends from `IteratorIterator`) is not found by the
/// standard lookup (no such method and no `__call`), the interpreter must
/// call this. `Some((inner, ce))` means "dispatch the call to `inner`":
/// resolve the method in `ce` first (`inner.ce`, which differs from the
/// object's class only for `IteratorIterator`'s `$class` downcast), then by
/// the normal lookup on `inner` (including its own forwarding/`__call`). If
/// that fails too, the "Call to undefined method" error names the outer
/// object's class (GH-16574). `None`: no forwarding (not a dual iterator, or
/// no inner iterator yet).
pub fn dual_it_forward_target(obj: &ObjRef) -> Option<(ObjRef, ClassRef)> {
    let st = obj.native::<DualIt>()?;
    match (&st.inner_obj, &st.inner_ce) {
        (Some(o), Some(ce)) => Some((o.clone(), ce.clone())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// IteratorIterator
// ---------------------------------------------------------------------------

fn ii_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(
        vm,
        args,
        "IteratorIterator",
        "Traversable",
        DitType::IteratorIterator,
    )?;
    Ok(Value::Null)
}

fn ii_get_inner_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(inner_obj(&this).map_or(Value::Null, Value::Object))
}

fn ii_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    rewind(vm, &this)?;
    fetch(vm, &this, true)?;
    Ok(Value::Null)
}

fn ii_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(Value::Bool(with(&this, |s| s.data.is_some())))
}

fn ii_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(key_value(&this))
}

fn ii_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    Ok(current_value(&this))
}

fn ii_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    next(vm, &this, true)?;
    fetch(vm, &this, true)?;
    Ok(Value::Null)
}

static II_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Traversable $iterator, ?string $class = null) {}",
    handler: ii_construct,
};
static II_GET_INNER_ITERATOR: NativeFunction = NativeFunction {
    sig: "public function getInnerIterator(): ?Iterator {}",
    handler: ii_get_inner_iterator,
};
static II_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: ii_rewind,
};
static II_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: ii_valid,
};
static II_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: ii_key,
};
static II_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: ii_current,
};
static II_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: ii_next,
};

// ---------------------------------------------------------------------------
// FilterIterator
// ---------------------------------------------------------------------------

fn fi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, "FilterIterator", "Iterator", DitType::Default)?;
    Ok(Value::Null)
}

fn fi_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    rewind(vm, &this)?;
    filter_fetch(vm, &this)?;
    Ok(Value::Null)
}

fn fi_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    next(vm, &this, true)?;
    filter_fetch(vm, &this)?;
    Ok(Value::Null)
}

static FI_ACCEPT: NativeFunction = NativeFunction {
    sig: "abstract public function accept(): bool;",
    handler: abstract_method,
};
static FI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator) {}",
    handler: fi_construct,
};
static FI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: fi_rewind,
};
static FI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: fi_next,
};

// ---------------------------------------------------------------------------
// RecursiveFilterIterator / ParentIterator
// ---------------------------------------------------------------------------

fn rfi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(
        vm,
        args,
        "RecursiveFilterIterator",
        "RecursiveIterator",
        DitType::Default,
    )?;
    Ok(Value::Null)
}

/// `RecursiveFilterIterator::hasChildren()` (also `ParentIterator::accept()`,
/// `RecursiveCallbackFilterIterator::hasChildren()` and
/// `RecursiveRegexIterator::hasChildren()`).
pub fn rfi_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let inner = inner_obj_checked(vm, &this)?;
    Ok(vm
        .call_method(&inner, b"hasChildren", Vec::new())?
        .into_unref())
}

fn rfi_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let inner = inner_obj_checked(vm, &this)?;
    let children = vm
        .call_method(&inner, b"getChildren", Vec::new())?
        .into_unref();
    let ce = this.class().clone();
    let obj = object_init_with_constructor(vm, &ce, vec![children])?;
    Ok(Value::Object(obj))
}

fn pi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(
        vm,
        args,
        "ParentIterator",
        "RecursiveIterator",
        DitType::Default,
    )?;
    Ok(Value::Null)
}

static RFI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(RecursiveIterator $iterator) {}",
    handler: rfi_construct,
};
static RFI_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: rfi_has_children,
};
static RFI_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): ?RecursiveFilterIterator {}",
    handler: rfi_get_children,
};
static PI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(RecursiveIterator $iterator) {}",
    handler: pi_construct,
};
static PI_ACCEPT: NativeFunction = NativeFunction {
    sig: "public function accept(): bool {}",
    handler: rfi_has_children,
};

// ---------------------------------------------------------------------------
// CallbackFilterIterator / RecursiveCallbackFilterIterator
// ---------------------------------------------------------------------------

fn cfi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(
        vm,
        args,
        "CallbackFilterIterator",
        "Iterator",
        DitType::CallbackFilter,
    )?;
    Ok(Value::Null)
}

/// Calls the callback with the current value, the current key and the inner
/// iterator; its return value is returned as is.
fn cfi_accept(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let (data, key, inner, callback) = with(&this, |s| {
        (
            s.data.clone(),
            s.key.clone(),
            s.inner_obj.clone(),
            s.callback.clone(),
        )
    });
    let (Some(data), Some(key), Some(callback)) = (data, key, callback) else {
        return Ok(Value::Bool(false));
    };
    let inner = inner.map_or(Value::Null, Value::Object);
    let ret = vm.call(&callback, vec![data, key, inner])?;
    Ok(ret.into_unref())
}

fn rcfi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(
        vm,
        args,
        "RecursiveCallbackFilterIterator",
        "RecursiveIterator",
        DitType::RecursiveCallbackFilter,
    )?;
    Ok(Value::Null)
}

fn rcfi_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    let inner = inner_obj_checked(vm, &this)?;
    let children = vm
        .call_method(&inner, b"getChildren", Vec::new())?
        .into_unref();
    let callback = with(&this, |s| s.callback.clone()).unwrap_or(Value::Null);
    let ce = this.class().clone();
    let obj = object_init_with_constructor(vm, &ce, vec![children, callback])?;
    Ok(Value::Object(obj))
}

static CFI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator, callable $callback) {}",
    handler: cfi_construct,
};
static CFI_ACCEPT: NativeFunction = NativeFunction {
    sig: "public function accept(): bool {}",
    handler: cfi_accept,
};
static RCFI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(RecursiveIterator $iterator, callable $callback) {}",
    handler: rcfi_construct,
};
static RCFI_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: rfi_has_children,
};
static RCFI_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): RecursiveCallbackFilterIterator {}",
    handler: rcfi_get_children,
};

// ---------------------------------------------------------------------------
// NoRewindIterator
// ---------------------------------------------------------------------------

fn nri_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, "NoRewindIterator", "Iterator", DitType::NoRewind)?;
    Ok(Value::Null)
}

/// Prevents a call to the inner iterator's `rewind()` (no state check).
fn nri_rewind(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

fn nri_inner(vm: &mut Vm, args: &Args) -> PResult<Rc<InnerIter>> {
    let this = checked(vm, args)?;
    match inner_iter(&this) {
        Some(it) => Ok(it),
        None => Err(vm.throw_error("Error", INVALID_STATE)),
    }
}

fn nri_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let it = nri_inner(vm, args)?;
    Ok(Value::Bool(it.valid(vm)?))
}

fn nri_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let it = nri_inner(vm, args)?;
    it.key(vm)
}

fn nri_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let it = nri_inner(vm, args)?;
    Ok(it.current(vm)?.unref())
}

fn nri_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let it = nri_inner(vm, args)?;
    it.next(vm)?;
    Ok(Value::Null)
}

static NRI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator) {}",
    handler: nri_construct,
};
static NRI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: nri_rewind,
};
static NRI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: nri_valid,
};
static NRI_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: nri_key,
};
static NRI_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: nri_current,
};
static NRI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: nri_next,
};

// ---------------------------------------------------------------------------
// InfiniteIterator
// ---------------------------------------------------------------------------

fn infi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, "InfiniteIterator", "Iterator", DitType::Infinite)?;
    Ok(Value::Null)
}

/// Moves forward, rewinding the inner iterator when it is exhausted.
fn infi_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = checked(vm, args)?;
    next(vm, &this, true)?;
    if valid(vm, &this)? {
        fetch(vm, &this, false)?;
    } else {
        rewind(vm, &this)?;
        if valid(vm, &this)? {
            fetch(vm, &this, false)?;
        }
    }
    Ok(Value::Null)
}

static INFI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(Iterator $iterator) {}",
    handler: infi_construct,
};
static INFI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: infi_next,
};

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// A dual-iterator subclass of `parent` with `methods` and extra interfaces.
pub fn subclass(
    r: &mut Registry,
    name: &str,
    flags: u32,
    parent: &str,
    interfaces: &[&str],
    methods: &[&'static NativeFunction],
) -> ClassRef {
    let parent = engine::class(r, parent);
    let mut ce = ClassEntry::new_subclass(name, flags, &parent);
    for iface in interfaces {
        let iface = engine::class(r, iface);
        implement(&mut ce, &iface);
    }
    add_methods(&mut ce, methods);
    r.class(ce)
}

/// `IteratorIterator` (the root of the dual iterators).
pub fn register_iterator_iterator(r: &mut Registry) {
    let mut ce = ClassEntry::new("IteratorIterator", 0);
    let outer = engine::class(r, super::OUTER_ITERATOR);
    implement(&mut ce, &outer);
    add_methods(
        &mut ce,
        &[
            &II_CONSTRUCT,
            &II_GET_INNER_ITERATOR,
            &II_REWIND,
            &II_VALID,
            &II_KEY,
            &II_CURRENT,
            &II_NEXT,
        ],
    );
    ce.handlers.create_object = Some(create_dual);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}

pub fn register_filter_iterators(r: &mut Registry) {
    subclass(
        r,
        "FilterIterator",
        class_flags::ABSTRACT,
        "IteratorIterator",
        &[],
        &[&FI_ACCEPT, &FI_CONSTRUCT, &FI_REWIND, &FI_NEXT],
    );
    subclass(
        r,
        "RecursiveFilterIterator",
        class_flags::ABSTRACT,
        "FilterIterator",
        &[super::RECURSIVE_ITERATOR],
        &[&RFI_CONSTRUCT, &RFI_HAS_CHILDREN, &RFI_GET_CHILDREN],
    );
    subclass(
        r,
        "CallbackFilterIterator",
        0,
        "FilterIterator",
        &[],
        &[&CFI_CONSTRUCT, &CFI_ACCEPT],
    );
    subclass(
        r,
        "RecursiveCallbackFilterIterator",
        0,
        "CallbackFilterIterator",
        &[super::RECURSIVE_ITERATOR],
        &[&RCFI_CONSTRUCT, &RCFI_HAS_CHILDREN, &RCFI_GET_CHILDREN],
    );
    subclass(
        r,
        "ParentIterator",
        0,
        "RecursiveFilterIterator",
        &[],
        &[&PI_CONSTRUCT, &PI_ACCEPT],
    );
}

pub fn register_no_rewind_iterator(r: &mut Registry) {
    subclass(
        r,
        "NoRewindIterator",
        0,
        "IteratorIterator",
        &[],
        &[
            &NRI_CONSTRUCT,
            &NRI_REWIND,
            &NRI_VALID,
            &NRI_KEY,
            &NRI_CURRENT,
            &NRI_NEXT,
        ],
    );
}

pub fn register_infinite_iterator(r: &mut Registry) {
    subclass(
        r,
        "InfiniteIterator",
        0,
        "IteratorIterator",
        &[],
        &[&INFI_CONSTRUCT, &INFI_NEXT],
    );
}
