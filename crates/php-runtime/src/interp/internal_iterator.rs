//! `InternalIterator` (`Zend/zend_interfaces.c`): the `Iterator` object that
//! `IteratorAggregate::getIterator()` returns for internal classes whose
//! iteration is a native [`ObjectHandlers::get_iterator`] handler
//! (`zend_create_internal_iterator_zval`).
//!
//! [`ObjectHandlers::get_iterator`]: crate::class::ObjectHandlers::get_iterator

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::class::{ClassEntry, Function, InternalIterator, class_flags, fn_flags, member_flags};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::value::Value;
use crate::vm::{PResult, Vm};

/// How a `get_iterator` handler hands out its iterator.
pub type IteratorCell = RefCell<Box<dyn InternalIterator>>;

/// `zend_internal_iterator`.
struct State {
    iter: Rc<IteratorCell>,
    rewind_called: Cell<bool>,
}

impl Vm {
    /// `zend_create_internal_iterator_zval`: wraps the iterator of `obj`'s
    /// `get_iterator` handler in an `InternalIterator` object (what
    /// `getIterator()` of an internal `IteratorAggregate` returns).
    pub fn create_internal_iterator(&mut self, obj: &ObjRef) -> PResult<Value> {
        let Some(gi) = obj.class().handlers.get_iterator else {
            return Err(self.throw_error(
                "Error",
                format!("Class {} has no internal iterator", obj.class().name),
            ));
        };
        let it = gi(self, obj, false)?;
        let iter = match it.downcast::<IteratorCell>() {
            Ok(cell) => cell,
            Err(_) => {
                return Err(self.throw_error(
                    "Error",
                    format!("Class {} has no internal iterator", obj.class().name),
                ));
            }
        };
        self.wrap_internal_iterator(iter)
    }

    /// Wraps an already created iterator in an `InternalIterator` object.
    pub fn create_internal_iterator_from(&mut self, iter: Box<dyn InternalIterator>) -> PResult<Value> {
        self.wrap_internal_iterator(Rc::new(RefCell::new(iter)))
    }

    fn wrap_internal_iterator(&mut self, iter: Rc<IteratorCell>) -> PResult<Value> {
        let class = self
            .find_class(b"InternalIterator")
            .expect("InternalIterator is a core class");
        let o = ObjRef::new(class);
        o.set_native(State {
            iter,
            rewind_called: Cell::new(false),
        });
        Ok(Value::Object(o))
    }
}

/// `zend_internal_iterator_fetch` + `zend_internal_iterator_ensure_rewound`.
fn fetch(vm: &mut Vm, a: &Args, rewind: bool) -> PResult<Rc<IteratorCell>> {
    let o = a.this().cloned().expect("InternalIterator method without $this");
    let state = o.native::<State>().map(|s| (s.iter.clone(), s.rewind_called.replace(true)));
    let Some((iter, was_rewound)) = state else {
        return Err(vm.throw_error(
            "Error",
            "The InternalIterator object has not been properly initialized",
        ));
    };
    if rewind && !was_rewound {
        iter.borrow_mut().rewind(vm)?;
    }
    Ok(iter)
}

fn ii_construct(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Cannot manually construct InternalIterator"))
}

fn ii_current(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let iter = fetch(vm, a, true)?;
    let v = iter.borrow_mut().current(vm)?;
    Ok(v.into_unref())
}

fn ii_key(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let iter = fetch(vm, a, true)?;
    let k = iter.borrow_mut().key(vm)?;
    Ok(k)
}

fn ii_next(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let iter = fetch(vm, a, true)?;
    iter.borrow_mut().next(vm)?;
    Ok(Value::Null)
}

fn ii_valid(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let iter = fetch(vm, a, true)?;
    let ok = iter.borrow_mut().valid(vm)?;
    Ok(Value::Bool(ok))
}

fn ii_rewind(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let iter = fetch(vm, a, false)?;
    iter.borrow_mut().rewind(vm)?;
    Ok(Value::Null)
}

/// `foreach` over an `InternalIterator` drives the wrapped iterator.
fn ii_get_iterator(vm: &mut Vm, o: &ObjRef, by_ref: bool) -> PResult<Rc<dyn Any>> {
    if by_ref {
        return Err(vm.throw_error("Error", "An iterator cannot be used with foreach by reference"));
    }
    // foreach calls rewind() first (zend_user_it_get_iterator).
    let iter = o.native::<State>().map(|s| {
        s.rewind_called.set(true);
        s.iter.clone()
    });
    match iter {
        Some(iter) => Ok(iter),
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

pub fn register(r: &mut Registry<'_>) {
    let mut ce = ClassEntry::new(
        "InternalIterator",
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE,
    );
    if let Some(iterator) = r.find_class("Iterator") {
        for sub in iterator.interfaces.clone() {
            ce.interfaces.push(sub);
        }
        ce.interfaces.push(iterator);
    }
    ce.add_method(Function::native_method(
        &II_CONSTRUCT,
        "InternalIterator",
        member_flags::PRIVATE | fn_flags::CTOR,
    ));
    for nf in [&II_CURRENT, &II_KEY, &II_NEXT, &II_VALID, &II_REWIND] {
        ce.add_method(Function::native_method(nf, "InternalIterator", member_flags::PUBLIC));
    }
    ce.handlers.get_iterator = Some(ii_get_iterator);
    r.class(ce);
}
