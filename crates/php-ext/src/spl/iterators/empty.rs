//! `EmptyIterator`: an iterator without elements.

use php_runtime::class::ClassEntry;
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use crate::spl::engine;

fn ei_current(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "BadMethodCallException",
        "Accessing the value of an EmptyIterator",
    ))
}

fn ei_key(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "BadMethodCallException",
        "Accessing the key of an EmptyIterator",
    ))
}

fn ei_valid(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(false))
}

/// `rewind()` and `next()` do nothing.
fn ei_noop(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

static EI_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): never {}",
    handler: ei_current,
};
static EI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: ei_noop,
};
static EI_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): never {}",
    handler: ei_key,
};
static EI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): false {}",
    handler: ei_valid,
};
static EI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: ei_noop,
};

pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new("EmptyIterator", 0);
    let iterator = engine::class(r, "Iterator");
    engine::implement(&mut ce, &iterator);
    engine::add_methods(
        &mut ce,
        &[&EI_CURRENT, &EI_NEXT, &EI_KEY, &EI_VALID, &EI_REWIND],
    );
    r.class(ce);
}
