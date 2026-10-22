//! Statement iteration: `pdo_stmt_iter_get` and its iterator functions,
//! exposed through `InternalIterator` (`Zend/zend_interfaces.c`) as returned
//! by `PDOStatement::getIterator()`.
//!
//! The runtime does not define the `get_iterator` handler protocol yet, so
//! `foreach` over a statement goes through `IteratorAggregate::getIterator()`
//! and these native `Iterator` methods.

use std::cell::RefCell;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::{Args, ClassRef, NativeFunction, ObjRef, PResult, Registry, Value, Vm};

use super::consts::{FETCH_ORI_NEXT, FETCH_USE_DEFAULT};
use super::error::handle_stmt_err;
use super::stmt::{do_fetch, stmt_of};
use super::{PdoGlobals, finish};

pub const ITERATOR_CLASS: &str = "InternalIterator";

/// `struct php_pdo_iterator` wrapped in a `zend_internal_iterator`.
struct StmtIterator {
    stmt_obj: ObjRef,
    /// `I->key`; `None` is `(zend_ulong)-1`.
    key: Option<u64>,
    fetch_ahead: Option<Value>,
    /// `iter->index`.
    index: u64,
    rewind_called: bool,
}

/// Advances to the next row (`pdo_stmt_iter_move_forwards`); the first call
/// happens when the iterator is created (`pdo_stmt_iter_get`).
fn move_forward(vm: &mut Vm, it: &RefCell<StmtIterator>) -> PResult<()> {
    let stmt_obj = it.borrow().stmt_obj.clone();
    it.borrow_mut().fetch_ahead = None;
    let Some(stmt) = stmt_of(&stmt_obj) else {
        return Ok(());
    };
    let Some(dbh) = stmt.dbh() else {
        return Ok(());
    };
    match do_fetch(
        vm,
        &stmt_obj,
        &stmt,
        &dbh,
        FETCH_USE_DEFAULT,
        FETCH_ORI_NEXT,
        0,
        None,
    )? {
        Some(v) => {
            let mut i = it.borrow_mut();
            i.fetch_ahead = Some(v);
            i.key = Some(i.key.map_or(0, |k| k.wrapping_add(1)));
        }
        None => {
            handle_stmt_err(vm, &dbh, &stmt)?;
            let mut i = it.borrow_mut();
            i.key = None;
            i.fetch_ahead = None;
        }
    }
    Ok(())
}

/// `zend_create_internal_iterator_zval` over `pdo_stmt_iter_get`.
pub fn create(vm: &mut Vm, stmt_obj: &ObjRef) -> PResult<ObjRef> {
    let Some(stmt) = stmt_of(stmt_obj).filter(|s| s.dbh().is_some()) else {
        return Err(vm.throw_error(
            "Error",
            format!("{} object is uninitialized", stmt_obj.class().name),
        ));
    };
    let ce = iterator_class(vm);
    let obj = ObjRef::new(ce);
    let it = RefCell::new(StmtIterator {
        stmt_obj: stmt_obj.clone(),
        key: None,
        fetch_ahead: None,
        index: 0,
        rewind_called: false,
    });
    // The fetch-ahead starts with key -1 + 1 = 0.
    let r = move_forward(vm, &it);
    obj.set_native(it);
    let dbh = stmt.dbh();
    finish(vm, dbh.as_ref(), r.map(|_| Value::Null))?;
    Ok(obj)
}

fn iterator_class(vm: &mut Vm) -> ClassRef {
    if let Some(ce) = vm.ext_data::<PdoGlobals>().iterator_class.clone() {
        return ce;
    }
    super::find_class(vm, ITERATOR_CLASS)
}

fn state(vm: &mut Vm, args: &Args) -> PResult<ObjRef> {
    let this = args
        .this()
        .cloned()
        .expect("InternalIterator method without $this");
    if this.native::<RefCell<StmtIterator>>().is_none() {
        return Err(vm.throw_error(
            "Error",
            "The InternalIterator object has not been properly initialized",
        ));
    }
    Ok(this)
}

fn with_it<R>(obj: &ObjRef, f: impl FnOnce(&RefCell<StmtIterator>) -> R) -> R {
    let it = obj
        .native::<RefCell<StmtIterator>>()
        .expect("checked by state()");
    f(&it)
}

/// `zend_internal_iterator_ensure_rewound` (PDO iterators have no rewind).
fn ensure_rewound(obj: &ObjRef) {
    with_it(obj, |it| it.borrow_mut().rewind_called = true);
}

fn construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Cannot manually construct InternalIterator"))
}

fn current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = state(vm, args)?;
    ensure_rewound(&this);
    Ok(with_it(&this, |it| it.borrow().fetch_ahead.clone()).unwrap_or(Value::Null))
}

fn key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = state(vm, args)?;
    ensure_rewound(&this);
    Ok(with_it(&this, |it| it.borrow().key).map_or(Value::Null, |k| Value::Int(k as i64)))
}

fn next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = state(vm, args)?;
    ensure_rewound(&this);
    // Only shared borrows of the native state are held while fetching, so
    // callbacks may use the iterator again.
    let it = this
        .native::<RefCell<StmtIterator>>()
        .expect("checked by state()");
    // Advance the index first to match foreach behaviour.
    it.borrow_mut().index += 1;
    let stmt = stmt_of(&it.borrow().stmt_obj);
    let r = move_forward(vm, &it);
    drop(it);
    let dbh = stmt.and_then(|s| s.dbh());
    finish(vm, dbh.as_ref(), r.map(|_| Value::Null))
}

fn valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = state(vm, args)?;
    ensure_rewound(&this);
    Ok(Value::Bool(with_it(&this, |it| {
        it.borrow().fetch_ahead.is_some()
    })))
}

fn rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = state(vm, args)?;
    let index = with_it(&this, |it| {
        let mut it = it.borrow_mut();
        it.rewind_called = true;
        it.index
    });
    // Rewinding is allowed before iteration starts, even without support.
    if index != 0 {
        return Err(vm.throw_error("Error", "Iterator does not support rewinding"));
    }
    Ok(Value::Null)
}

macro_rules! natives {
    ($($id:ident: $sig:literal => $h:ident;)*) => {
        $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $h };)*
    };
}

natives! {
    CONSTRUCT: "private function __construct()" => construct;
    CURRENT: "public function current(): mixed" => current;
    KEY: "public function key(): mixed" => key;
    NEXT: "public function next(): void" => next;
    VALID: "public function valid(): bool" => valid;
    REWIND: "public function rewind(): void" => rewind;
}

fn create_object(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let _ = vm;
    Ok(ObjRef::new(ce.clone()))
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

/// Registers `final class InternalIterator implements Iterator` unless the
/// engine already provides it; PDO keeps its own class entry either way.
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(
        ITERATOR_CLASS,
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE,
    );
    if let Some(iter) = r.find_class("Iterator") {
        ce.interfaces.extend(iter.interfaces.iter().cloned());
        ce.interfaces.push(iter);
    }
    ce.add_method(Function::native_method(
        &CONSTRUCT,
        ITERATOR_CLASS,
        member_flags::PRIVATE,
    ));
    for nf in [&CURRENT, &KEY, &NEXT, &VALID, &REWIND] {
        ce.add_method(Function::native_method(
            nf,
            ITERATOR_CLASS,
            member_flags::PUBLIC,
        ));
    }
    ce.handlers.create_object = Some(create_object);
    ce.handlers.clone_obj = Some(clone_forbidden);
    let existing = r.find_class(ITERATOR_CLASS);
    let ce = match existing {
        Some(_) => std::rc::Rc::new(ce),
        None => r.class(ce),
    };
    r.vm.ext_data::<PdoGlobals>().iterator_class = Some(ce);
}
