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

// ---------------------------------------------------------------------------
// Test iterators
// ---------------------------------------------------------------------------

/// Method-call log shared by a test iterator and its children.
pub type CallLog = Rc<std::cell::RefCell<Vec<String>>>;

/// State of `SplTestIterator` / `SplTestRecursiveIterator`.
struct TestIterState {
    items: Vec<(Value, Value)>,
    pos: usize,
    log: CallLog,
    /// Log entries are prefixed with this (one `>` per nesting level).
    tag: String,
}

fn ti_this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("test iterator method without $this")
}

fn ti_log(o: &ObjRef, m: &str) {
    let st = o.native::<TestIterState>().expect("test iterator state");
    let entry = format!("{}{m}", st.tag);
    st.log.borrow_mut().push(entry);
}

fn ti_rewind(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "rewind");
    o.native_mut::<TestIterState>().unwrap().pos = 0;
    Ok(Value::Null)
}

fn ti_valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "valid");
    let st = o.native::<TestIterState>().unwrap();
    Ok(Value::Bool(st.pos < st.items.len()))
}

fn ti_current(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "current");
    let st = o.native::<TestIterState>().unwrap();
    Ok(st.items.get(st.pos).map_or(Value::Null, |(_, v)| v.clone()))
}

fn ti_key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "key");
    let st = o.native::<TestIterState>().unwrap();
    Ok(st.items.get(st.pos).map_or(Value::Null, |(k, _)| k.clone()))
}

fn ti_next(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "next");
    o.native_mut::<TestIterState>().unwrap().pos += 1;
    Ok(Value::Null)
}

fn ti_has_children(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "hasChildren");
    let st = o.native::<TestIterState>().unwrap();
    Ok(Value::Bool(matches!(
        st.items.get(st.pos).map(|(_, v)| v.unref()),
        Some(Value::Array(_))
    )))
}

fn ti_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti_this(args);
    ti_log(&o, "getChildren");
    let (child, log, tag) = {
        let st = o.native::<TestIterState>().unwrap();
        let child = st.items.get(st.pos).map_or(Value::Null, |(_, v)| v.unref());
        (child, st.log.clone(), format!("{}>", st.tag))
    };
    let items = array_items(&child);
    Ok(Value::Object(make_test_iterator(vm, items, log, tag, true)))
}

static TI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: ti_rewind,
};
static TI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: ti_valid,
};
static TI_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: ti_current,
};
static TI_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: ti_key,
};
static TI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: ti_next,
};
static TI_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: ti_has_children,
};
static TI_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): ?RecursiveIterator {}",
    handler: ti_get_children,
};

fn test_iterator_class(vm: &mut Vm, recursive: bool) -> ClassRef {
    let name = if recursive {
        "SplTestRecursiveIterator"
    } else {
        "SplTestIterator"
    };
    if let Some(ce) = vm.find_class(name.as_bytes()) {
        return ce;
    }
    let mut ce = ClassEntry::new(name, 0);
    let iface = if recursive {
        "RecursiveIterator"
    } else {
        "Iterator"
    };
    let iface = vm.find_class(iface.as_bytes()).expect("iterator interface");
    super::engine::implement(&mut ce, &iface);
    super::engine::add_methods(
        &mut ce,
        &[&TI_REWIND, &TI_VALID, &TI_CURRENT, &TI_KEY, &TI_NEXT],
    );
    if recursive {
        super::engine::add_methods(&mut ce, &[&TI_HAS_CHILDREN, &TI_GET_CHILDREN]);
    }
    let ce = Rc::new(ce);
    vm.register_class(ce.clone());
    ce
}

fn make_test_iterator(
    vm: &mut Vm,
    items: Vec<(Value, Value)>,
    log: CallLog,
    tag: String,
    recursive: bool,
) -> ObjRef {
    let ce = test_iterator_class(vm, recursive);
    let o = ObjRef::new(ce);
    o.set_native(TestIterState {
        items,
        pos: 0,
        log,
        tag,
    });
    o
}

/// `(key, value)` pairs of an array value.
pub fn array_items(v: &Value) -> Vec<(Value, Value)> {
    match v.unref() {
        Value::Array(a) => a.iter().map(|(k, v)| (k.to_value(), v.clone())).collect(),
        _ => Vec::new(),
    }
}

/// A plain `Iterator` (class `SplTestIterator`, not internal) over `items`
/// that logs every method call (`"rewind"`, `"valid"`, `"current"`, `"key"`,
/// `"next"`) — a stand-in for userland iterators and `ArrayIterator`.
pub fn test_iterator(vm: &mut Vm, items: Vec<(Value, Value)>) -> (ObjRef, CallLog) {
    let log = CallLog::default();
    let o = make_test_iterator(vm, items, log.clone(), String::new(), false);
    (o, log)
}

/// A `RecursiveIterator` (class `SplTestRecursiveIterator`) over an array
/// value: elements that are arrays have children (like
/// `RecursiveArrayIterator`). Children log with one `">"` prefix per level
/// (`">rewind"`, `">>valid"`, ...) into the same log.
pub fn test_recursive_iterator(vm: &mut Vm, array: &Value) -> (ObjRef, CallLog) {
    let log = CallLog::default();
    let o = make_test_iterator(vm, array_items(array), log.clone(), String::new(), true);
    (o, log)
}
