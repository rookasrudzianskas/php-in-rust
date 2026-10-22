//! `InternalIterator` (`Zend/zend_interfaces.c`): the `Iterator` object that
//! `IteratorAggregate::getIterator()` returns for internal classes whose
//! iteration is a native `zend_object_iterator`
//! ([`ObjectIterator`], from a `get_iterator` handler);
//! `zend_create_internal_iterator_zval`.
//!
//! Extensions create one with [`create_internal_iterator`] (or
//! [`Vm::create_internal_iterator`] from the object's `get_iterator`
//! handler). `foreach` over an `InternalIterator` drives the wrapped
//! iterator directly, as `foreach` over the original object would.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::class::{
    ClassEntry, ClassRef, Function, InternalIterator, ObjectIterator, class_flags, fn_flags, member_flags,
};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::value::Value;
use crate::vm::{Flow, PResult, Vm};

/// The legacy `get_iterator` result type (see [`InternalIterator`]).
pub type IteratorCell = RefCell<Box<dyn InternalIterator>>;

pub const INTERNAL_ITERATOR: &str = "InternalIterator";

/// Packs an iterator for a `get_iterator` handler's `Rc<dyn Any>` result.
pub fn iterator_any(iter: Rc<dyn ObjectIterator>) -> Rc<dyn Any> {
    Rc::new(iter)
}

/// Unpacks a `get_iterator` handler result: an [`iterator_any`] value or a
/// legacy [`IteratorCell`].
pub fn iterator_from_any(it: Rc<dyn Any>) -> Option<Rc<dyn ObjectIterator>> {
    if let Some(i) = it.downcast_ref::<Rc<dyn ObjectIterator>>() {
        return Some(i.clone());
    }
    match it.downcast::<IteratorCell>() {
        Ok(cell) => Some(cell as Rc<dyn ObjectIterator>),
        Err(_) => None,
    }
}

/// `zend_throw_error(NULL, "An iterator cannot be used with foreach by reference")`.
pub fn by_ref_error(vm: &mut Vm) -> Flow {
    vm.throw_error("Error", "An iterator cannot be used with foreach by reference")
}

/// `zend_internal_iterator`.
struct State {
    iter: Option<Rc<dyn ObjectIterator>>,
    rewind_called: Cell<bool>,
    /// `iter->index`.
    index: Cell<i64>,
}

fn create(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(State {
        iter: None,
        rewind_called: Cell::new(false),
        index: Cell::new(0),
    });
    Ok(obj)
}

fn clone_forbidden(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!("Trying to clone an uncloneable object of class {}", obj.class().name),
    ))
}

/// `zend_create_internal_iterator_zval`: an `InternalIterator` over `iter`.
pub fn create_internal_iterator(vm: &mut Vm, iter: Rc<dyn ObjectIterator>) -> PResult<ObjRef> {
    let ce = match vm.find_class(INTERNAL_ITERATOR.as_bytes()) {
        Some(ce) => ce,
        None => {
            // Embedders that skip `interp::install` (extension unit tests).
            let mut r = Registry::new(vm, "Core");
            register(&mut r);
            vm.find_class(INTERNAL_ITERATOR.as_bytes())
                .expect("InternalIterator was just registered")
        }
    };
    let obj = ObjRef::new(ce);
    obj.set_native(State {
        iter: Some(iter),
        rewind_called: Cell::new(false),
        index: Cell::new(0),
    });
    Ok(obj)
}

impl Vm {
    /// `zend_create_internal_iterator_zval` for `obj`: wraps the iterator of
    /// its class's `get_iterator` handler in an `InternalIterator`.
    pub fn create_internal_iterator(&mut self, obj: &ObjRef) -> PResult<Value> {
        let no_iter = |vm: &mut Vm| {
            vm.throw_error("Error", format!("Class {} has no internal iterator", obj.class().name))
        };
        let Some(gi) = obj.class().handlers.get_iterator else {
            return Err(no_iter(self));
        };
        let it = gi(self, obj, false)?;
        match iterator_from_any(it) {
            Some(iter) => Ok(Value::Object(create_internal_iterator(self, iter)?)),
            None => Err(no_iter(self)),
        }
    }

    /// Wraps a legacy [`InternalIterator`] in an `InternalIterator` object.
    pub fn create_internal_iterator_from(&mut self, iter: Box<dyn InternalIterator>) -> PResult<Value> {
        let iter: Rc<dyn ObjectIterator> = Rc::new(RefCell::new(iter));
        Ok(Value::Object(create_internal_iterator(self, iter)?))
    }
}

/// `zend_internal_iterator_fetch`: the wrapped iterator (copied out so no
/// borrow is held while it runs PHP code).
fn fetch(vm: &mut Vm, a: &Args) -> PResult<(ObjRef, Rc<dyn ObjectIterator>)> {
    let this = a.this().cloned().expect("InternalIterator method without $this");
    let iter = this.native::<State>().and_then(|s| s.iter.clone());
    match iter {
        Some(iter) => Ok((this, iter)),
        None => Err(vm.throw_error(
            "Error",
            "The InternalIterator object has not been properly initialized",
        )),
    }
}

fn with_state<R>(obj: &ObjRef, f: impl FnOnce(&State) -> R) -> R {
    let st = obj.native::<State>().expect("InternalIterator state");
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

fn ii_construct(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Cannot manually construct InternalIterator"))
}

fn ii_current(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, a)?;
    ensure_rewound(vm, &this, &iter)?;
    Ok(iter.current(vm)?.map_or(Value::Null, |v| v.into_unref()))
}

fn ii_key(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, a)?;
    ensure_rewound(vm, &this, &iter)?;
    match iter.key(vm)? {
        Some(k) => Ok(k),
        None => Ok(Value::Int(with_state(&this, |s| s.index.get()))),
    }
}

fn ii_next(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, a)?;
    ensure_rewound(vm, &this, &iter)?;
    // Advance the index first to match foreach behaviour.
    with_state(&this, |s| s.index.set(s.index.get() + 1));
    iter.move_forward(vm)?;
    Ok(Value::Null)
}

fn ii_valid(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, a)?;
    ensure_rewound(vm, &this, &iter)?;
    Ok(Value::Bool(iter.valid(vm)?))
}

fn ii_rewind(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let (this, iter) = fetch(vm, a)?;
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

/// `foreach` over an `InternalIterator` drives the wrapped iterator; like
/// `zend_user_it_get_iterator` it counts as having called `rewind()`.
fn ii_get_iterator(vm: &mut Vm, o: &ObjRef, by_ref: bool) -> PResult<Rc<dyn Any>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    let iter = o.native::<State>().and_then(|s| {
        s.rewind_called.set(true);
        s.iter.clone()
    });
    match iter {
        Some(iter) => Ok(iterator_any(iter)),
        None => Err(vm.throw_error(
            "Error",
            "The InternalIterator object has not been properly initialized",
        )),
    }
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
/// (`@not-serializable`), once.
pub fn register(r: &mut Registry<'_>) {
    if r.find_class(INTERNAL_ITERATOR).is_some() {
        return;
    }
    let mut ce = ClassEntry::new(INTERNAL_ITERATOR, class_flags::FINAL | class_flags::NOT_SERIALIZABLE);
    if let Some(iterator) = r.find_class("Iterator") {
        for sub in iterator.interfaces.clone() {
            ce.interfaces.push(sub);
        }
        ce.interfaces.push(iterator);
    }
    ce.add_method(Function::native_method(
        &II_CONSTRUCT,
        INTERNAL_ITERATOR,
        member_flags::PRIVATE | fn_flags::CTOR,
    ));
    for nf in [&II_CURRENT, &II_KEY, &II_NEXT, &II_VALID, &II_REWIND] {
        ce.add_method(Function::native_method(nf, INTERNAL_ITERATOR, member_flags::PUBLIC));
    }
    ce.handlers.create_object = Some(create);
    ce.handlers.clone_obj = Some(clone_forbidden);
    ce.handlers.get_iterator = Some(ii_get_iterator);
    r.class(ce);
}
