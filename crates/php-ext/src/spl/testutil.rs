//! Helpers for SPL unit tests (no interpreter is available).
//!
//! * [`new_obj`] / [`call`] — `new Class(...)` and `$obj->method(...)`.
//! * [`closure`] — a PHP callable backed by a Rust closure (an invokable
//!   object of the internal test class `SplTestClosure`), for callbacks.
//! * [`user_class`] — a "user" subclass (not `INTERNAL`) whose methods are
//!   native functions declared by the test, to simulate userland
//!   overrides (`FilterIterator::accept`, `ArrayObject::offsetGet`, ...).
//! * [`list`] / [`assoc`] / [`array_of`] — array construction and access.

#![allow(dead_code)]

use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, member_flags};
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpArrayRef, PhpStr,
    Value, Vm,
};

/// `new $class(...$args)`.
pub fn new_obj(vm: &mut Vm, class: &str, args: Vec<Value>) -> PResult<ObjRef> {
    let ce = vm
        .find_class(class.as_bytes())
        .unwrap_or_else(|| panic!("class {class} is not registered"));
    let obj = vm.instantiate(&ce)?;
    if ce.find_method(b"__construct").is_some() {
        vm.call_method(&obj, b"__construct", args)?;
    }
    Ok(obj)
}

/// `$obj->$name(...$args)`.
pub fn call(vm: &mut Vm, obj: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    vm.call_method(obj, name.as_bytes(), args)
}

/// `Class::$name(...$args)` for static methods.
pub fn call_static(vm: &mut Vm, class: &str, name: &str, args: Vec<Value>) -> PResult<Value> {
    let callable = Value::String(PhpStr::from(format!("{class}::{name}")));
    vm.call(&callable, args)
}

type ClosureFn = Rc<dyn Fn(&mut Vm, Vec<Value>) -> PResult<Value>>;

struct TestClosure(ClosureFn);

fn closure_invoke(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = args.this().cloned().expect("closure object");
    let f = this
        .native::<TestClosure>()
        .map(|c| c.0.clone())
        .expect("SplTestClosure state");
    f(vm, args.rest(0).to_vec())
}

static CLOSURE_INVOKE: NativeFunction = NativeFunction {
    sig: "public function __invoke(mixed ...$args): mixed {}",
    handler: closure_invoke,
};

/// A callable object running `f` with the call's arguments.
pub fn closure(vm: &mut Vm, f: impl Fn(&mut Vm, Vec<Value>) -> PResult<Value> + 'static) -> Value {
    let ce = match vm.find_class(b"SplTestClosure") {
        Some(ce) => ce,
        None => {
            let mut ce = ClassEntry::new("SplTestClosure", 0);
            ce.add_method(Function::native_method(
                &CLOSURE_INVOKE,
                "SplTestClosure",
                member_flags::PUBLIC,
            ));
            ce.link_magic_methods();
            let ce = Rc::new(ce);
            vm.register_class(ce.clone());
            ce
        }
    };
    let obj = ObjRef::new(ce);
    obj.set_native(TestClosure(Rc::new(f)));
    Value::Object(obj)
}

/// Declares a non-internal subclass of `parent` named `name` whose methods
/// are `methods` (scope = the subclass, so [`super::engine::is_overridden`]
/// sees them as overrides). The parent's handlers are inherited.
pub fn user_class(
    vm: &mut Vm,
    name: &str,
    parent: &str,
    methods: &[&'static NativeFunction],
) -> ClassRef {
    let parent = vm
        .find_class(parent.as_bytes())
        .unwrap_or_else(|| panic!("class {parent} is not registered"));
    let mut ce = ClassEntry::new_subclass(name, 0, &parent);
    for nf in methods {
        ce.add_method(Function::native_method(
            nf,
            name,
            super::engine::method_flags(nf.sig),
        ));
    }
    ce.link_magic_methods();
    let ce = Rc::new(ce);
    vm.register_class(ce.clone());
    ce
}

/// A list array value.
pub fn list(values: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(values))
}

/// An array value from key/value pairs.
pub fn assoc(pairs: Vec<(ArrayKey, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(k, v);
    }
    Value::array(a)
}

/// String array key.
pub fn skey(s: &str) -> ArrayKey {
    ArrayKey::Str(PhpStr::from(s))
}

/// The array inside a value (panics otherwise).
pub fn array_of(v: &Value) -> PhpArrayRef {
    match v.unref() {
        Value::Array(a) => a,
        other => panic!("expected an array, got {other:?}"),
    }
}

/// `[key => value]` pairs of an array as `(String, Value)` for assertions.
pub fn entries(v: &Value) -> Vec<(String, Value)> {
    array_of(v)
        .iter()
        .map(|(k, v)| (k.to_php_str().to_string_lossy().into_owned(), v.clone()))
        .collect()
}

/// Shorthand for string values.
pub fn s(v: &str) -> Value {
    Value::str(v)
}
