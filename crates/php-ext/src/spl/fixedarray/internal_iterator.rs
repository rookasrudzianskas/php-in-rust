//! `zend_object_iterator` for internal classes and the `InternalIterator`
//! class of `Zend/zend_interfaces.c`.
//!
//! [`ObjectIterator`] is the Rust form of `zend_object_iterator_funcs`: the
//! iterator an internal class's `get_iterator` returns (`foreach` over an
//! SPL data structure, `zend_create_internal_iterator_zval`). Implementors
//! keep their position in `Cell`s so that the iterator can be shared
//! (`Rc`) and re-entered from PHP code.
//!
//! `InternalIterator` wraps such an iterator in a PHP object
//! ([`create_internal_iterator`]); `SplFixedArray::getIterator()` returns
//! one. The class belongs to Core; it is registered here (owned by "Core")
//! only when the runtime does not provide it yet.

use std::cell::Cell;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, class_flags};
use php_runtime::{Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, Registry, Value, Vm};

use crate::spl::engine;

/// `zend_object_iterator_funcs`.
pub trait ObjectIterator {
    /// `funcs->valid`.
    fn valid(&self, vm: &mut Vm) -> PResult<bool>;
    /// `funcs->get_current_data`; `Ok(None)` is a `NULL` result (`foreach`
    /// stops, `InternalIterator::current()` returns `null`).
    fn current(&self, vm: &mut Vm) -> PResult<Option<Value>>;
    /// `funcs->get_current_key`; `Ok(None)` when the iterator has no key
    /// function (the iteration index is used instead).
    fn key(&self, vm: &mut Vm) -> PResult<Option<Value>>;
    /// `funcs->move_forward`.
    fn move_forward(&self, vm: &mut Vm) -> PResult<()>;
    /// `funcs->rewind` (only called when [`ObjectIterator::can_rewind`]).
    fn rewind(&self, vm: &mut Vm) -> PResult<()>;
    /// Whether `funcs->rewind` is set.
    fn can_rewind(&self) -> bool {
        true
    }
}

/// `zend_throw_error(NULL, "An iterator cannot be used with foreach by reference")`.
pub fn by_ref_error(vm: &mut Vm) -> Flow {
    vm.throw_error(
        "Error",
        "An iterator cannot be used with foreach by reference",
    )
}

pub const INTERNAL_ITERATOR: &str = "InternalIterator";

/// `zend_internal_iterator`.
struct InternalIteratorState {
    iter: Option<Rc<dyn ObjectIterator>>,
    rewind_called: Cell<bool>,
    /// `iter->index`.
    index: Cell<i64>,
}

fn create(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(InternalIteratorState {
        iter: None,
        rewind_called: Cell::new(false),
        index: Cell::new(0),
    });
    Ok(obj)
}

fn clone_forbidden(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            obj.class().name
        ),
    ))
}

/// `zend_create_internal_iterator_zval`: an `InternalIterator` over `iter`
/// (the result of the calling class's `get_iterator`).
pub fn create_internal_iterator(vm: &mut Vm, iter: Rc<dyn ObjectIterator>) -> PResult<ObjRef> {
    let ce = vm
        .find_class(INTERNAL_ITERATOR.as_bytes())
        .expect("InternalIterator is registered");
    let obj = vm.instantiate(&ce)?;
    obj.set_native(InternalIteratorState {
        iter: Some(iter),
        rewind_called: Cell::new(false),
        index: Cell::new(0),
    });
    Ok(obj)
}

/// `zend_internal_iterator_fetch`: the wrapped iterator, its index and
/// whether `rewind()` ran (copied out so that no borrow is held while the
/// iterator runs PHP code).
fn fetch(vm: &mut Vm, args: &Args) -> PResult<(ObjRef, Rc<dyn ObjectIterator>)> {
    let this = engine::this(args);
    let iter = this
        .native::<InternalIteratorState>()
        .and_then(|s| s.iter.clone());
    match iter {
        Some(iter) => Ok((this, iter)),
        None => Err(vm.throw_error(
            "Error",
            "The InternalIterator object has not been properly initialized",
        )),
    }
}

fn with_state<R>(obj: &ObjRef, f: impl FnOnce(&InternalIteratorState) -> R) -> R {
    let st = obj
        .native::<InternalIteratorState>()
        .expect("InternalIterator state");
    f(&st)
}

/// `zend_internal_iterator_ensure_rewound`.
fn ensure_rewound(vm: &mut Vm, obj: &ObjRef, iter: &Rc<dyn ObjectIterator>) -> PResult<()> {
    let rewound = with_state(obj, |s| s.rewind_called.replace(true));
    if !rewound && iter.can_rewind() {
        iter.rewind(vm)?;
    }
    Ok(())
}

fn ii_construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Cannot manually construct InternalIterator"))
}

fn ii_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, args)?;
    ensure_rewound(vm, &this, &iter)?;
    Ok(iter.current(vm)?.map_or(Value::Null, |v| v.unref()))
}

fn ii_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, args)?;
    ensure_rewound(vm, &this, &iter)?;
    match iter.key(vm)? {
        Some(k) => Ok(k),
        None => Ok(Value::Int(with_state(&this, |s| s.index.get()))),
    }
}

fn ii_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, args)?;
    ensure_rewound(vm, &this, &iter)?;
    // Advance the index first to match foreach behaviour.
    with_state(&this, |s| s.index.set(s.index.get() + 1));
    iter.move_forward(vm)?;
    Ok(Value::Null)
}

fn ii_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, args)?;
    ensure_rewound(vm, &this, &iter)?;
    Ok(Value::Bool(iter.valid(vm)?))
}

fn ii_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, args)?;
    with_state(&this, |s| s.rewind_called.set(true));
    if !iter.can_rewind() {
        // Allow calling rewind() if no iteration has happened yet, even if
        // the iterator does not support rewinding.
        if with_state(&this, |s| s.index.get()) != 0 {
            return Err(vm.throw_error("Error", "Iterator does not support rewinding"));
        }
        return Ok(Value::Null);
    }
    iter.rewind(vm)?;
    with_state(&this, |s| s.index.set(0));
    Ok(Value::Null)
}

static II_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "private function __construct() {}",
    handler: ii_construct,
};
static II_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: ii_current,
};
static II_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: ii_key,
};
static II_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: ii_next,
};
static II_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: ii_valid,
};
static II_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: ii_rewind,
};

/// Registers `final class InternalIterator implements Iterator`
/// (`@not-serializable`) unless the runtime already provides it.
pub fn register(r: &mut Registry) {
    if r.find_class(INTERNAL_ITERATOR).is_some() {
        return;
    }
    let mut ce = ClassEntry::new(
        INTERNAL_ITERATOR,
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE | class_flags::INTERNAL,
    );
    engine::implement(&mut ce, &engine::class(r, "Iterator"));
    engine::add_methods(
        &mut ce,
        &[
            &II_CONSTRUCT,
            &II_CURRENT,
            &II_KEY,
            &II_NEXT,
            &II_VALID,
            &II_REWIND,
        ],
    );
    ce.handlers.create_object = Some(create);
    ce.handlers.clone_obj = Some(clone_forbidden);
    // A Zend class: registered like `Registry::class` but owned by Core.
    ce.module = Some("Core");
    ce.link_magic_methods();
    r.vm.register_class(Rc::new(ce));
}
