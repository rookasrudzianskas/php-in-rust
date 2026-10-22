//! Shared plumbing of the iterator classes.
//!
//! * [`InnerIter`] — the `zend_object_iterator` an SPL iterator keeps over
//!   its inner `Iterator` (`zend_user_iterator` of `zend_interfaces.c`):
//!   every step calls the object's `Iterator` methods through
//!   [`Vm::call_method`] (so user overrides run) and `current()` is cached
//!   per position, exactly like `zend_user_it_get_current_data`. It is the
//!   re-entrancy-safe counterpart of [`crate::spl::engine::ObjIterator`]:
//!   it lives behind an `Rc` with the cache in a `RefCell` that is only
//!   borrowed between calls, so PHP code run by a step may freely use the
//!   same iterator again (nested `next()` from `valid()`, ...).
//! * [`Pending`] — `EG(exception)` for code ported from php-src that keeps
//!   running after a call threw: later calls are skipped (like
//!   `zend_call_function` does while an exception is pending) and the
//!   exception can be cleared (`zend_clear_exception`).
//! * [`object_init_with_constructor`], [`clone_forbidden`],
//!   [`get_iterator_from_aggregate`]: small engine helpers.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::{ClassRef, Flow, ObjRef, PResult, Value, Vm};

use crate::spl::engine;

/// A `zend_object_iterator` over an object implementing `Iterator`.
pub struct InnerIter {
    obj: ObjRef,
    /// Cached `current()` of the position (`zend_user_iterator::value`).
    value: RefCell<Option<Value>>,
    /// Whether `current()` is cached (`false` mimics iterators that read
    /// their storage on every access, like `spl_array_it_get_current_data`).
    cache: bool,
}

impl InnerIter {
    /// `ce->get_iterator(ce, obj, 0)`: aggregates are resolved with
    /// [`engine::get_iterator`] (`zend_user_it_get_new_iterator`).
    pub fn new(vm: &mut Vm, traversable: &ObjRef) -> PResult<Rc<InnerIter>> {
        let obj = engine::get_iterator(vm, traversable)?;
        Ok(InnerIter::over(obj))
    }

    /// An iterator over an object known to implement `Iterator`.
    pub fn over(obj: ObjRef) -> Rc<InnerIter> {
        Rc::new(InnerIter {
            obj,
            value: RefCell::new(None),
            cache: true,
        })
    }

    /// Like [`InnerIter::over`] without caching `current()`.
    pub fn uncached(obj: ObjRef) -> Rc<InnerIter> {
        Rc::new(InnerIter {
            obj,
            value: RefCell::new(None),
            cache: false,
        })
    }

    /// The `Iterator` object being walked.
    pub fn object(&self) -> &ObjRef {
        &self.obj
    }

    /// `zend_user_it_invalidate_current`.
    pub fn invalidate_current(&self) {
        self.value.borrow_mut().take();
    }

    /// `zend_user_it_rewind`.
    pub fn rewind(&self, vm: &mut Vm) -> PResult<()> {
        self.invalidate_current();
        vm.call_method(&self.obj, b"rewind", Vec::new())?;
        Ok(())
    }

    /// `zend_user_it_valid` (`i_zend_is_true()` of `valid()`).
    pub fn valid(&self, vm: &mut Vm) -> PResult<bool> {
        let more = vm.call_method(&self.obj, b"valid", Vec::new())?;
        vm.to_bool(&more.into_unref())
    }

    /// `zend_user_it_get_current_data`: `current()`, called once per position.
    pub fn current(&self, vm: &mut Vm) -> PResult<Value> {
        if let Some(v) = self.value.borrow().as_ref() {
            return Ok(v.clone());
        }
        let v = vm.call_method(&self.obj, b"current", Vec::new())?;
        if self.cache {
            *self.value.borrow_mut() = Some(v.clone());
        }
        Ok(v)
    }

    /// `zend_user_it_get_current_key` (references are unwrapped).
    pub fn key(&self, vm: &mut Vm) -> PResult<Value> {
        Ok(vm.call_method(&self.obj, b"key", Vec::new())?.into_unref())
    }

    /// `zend_user_it_move_forward`.
    pub fn next(&self, vm: &mut Vm) -> PResult<()> {
        self.invalidate_current();
        vm.call_method(&self.obj, b"next", Vec::new())?;
        Ok(())
    }
}

/// `EG(exception)` for ported code that continues after a failed call.
#[derive(Default)]
pub struct Pending(Option<Flow>);

impl Pending {
    /// Runs `f` unless an exception is pending (then it is skipped like
    /// `zend_call_function` skips calls). A thrown exception becomes pending
    /// (`Ok(None)`); other control flow (`exit`, fatal errors) propagates.
    pub fn run<T>(&mut self, f: impl FnOnce() -> PResult<T>) -> PResult<Option<T>> {
        if self.0.is_some() {
            return Ok(None);
        }
        match f() {
            Ok(v) => Ok(Some(v)),
            Err(Flow::Exception(e)) => {
                self.0 = Some(Flow::Exception(e));
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }

    /// Whether an exception is pending (`EG(exception) != NULL`).
    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }

    /// `zend_clear_exception()`.
    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// Makes `flow` pending unless an exception already is (`zend_throw_*`
    /// while returning to the caller right after).
    pub fn throw(&mut self, flow: Flow) {
        if self.0.is_none() {
            self.0 = Some(flow);
        }
    }

    /// Returns to the caller: `Err` with the pending exception, if any.
    pub fn finish(self) -> PResult<()> {
        match self.0 {
            Some(f) => Err(f),
            None => Ok(()),
        }
    }
}

/// `object_init_with_constructor()`: instantiates `ce` and calls its
/// constructor (if any) with `args`; the object is dropped if it throws.
pub fn object_init_with_constructor(
    vm: &mut Vm,
    ce: &ClassRef,
    args: Vec<Value>,
) -> PResult<ObjRef> {
    let obj = vm.instantiate(ce)?;
    if ce.find_method(b"__construct").is_some() {
        vm.call_method(&obj, b"__construct", args)?;
    }
    Ok(obj)
}

/// `clone_obj = NULL`: "Trying to clone an uncloneable object of class X".
pub fn clone_forbidden(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            old.class().name
        ),
    ))
}

/// `spl_get_iterator_from_aggregate()`: `$obj->getIterator()` as declared
/// by `ce` (which may be a parent of the object's class), which must return
/// a `Traversable`.
pub fn get_iterator_from_aggregate(vm: &mut Vm, ce: &ClassRef, obj: &ObjRef) -> PResult<ObjRef> {
    let ret = if obj.class().lc_name == ce.lc_name {
        vm.call_method(obj, b"getIterator", Vec::new())?
    } else {
        match ce.find_method(b"getiterator").cloned() {
            Some(m) => vm.call_function(
                &m,
                php_runtime::CallArgs {
                    this: Some(obj.clone()),
                    scope: Some(ce.clone()),
                    dynamic: true,
                    ..php_runtime::CallArgs::default()
                },
            )?,
            None => vm.call_method(obj, b"getIterator", Vec::new())?,
        }
    };
    match ret.into_unref() {
        Value::Object(o) if o.class().instance_of_name(b"Traversable") => Ok(o),
        _ => Err(vm.throw_error(
            "LogicException",
            format!(
                "{}::getIterator() must return an object that implements Traversable",
                ce.name
            ),
        )),
    }
}
