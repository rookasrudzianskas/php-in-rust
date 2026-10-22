//! Tests of the SPL foundation: exceptions, `php_spl.c` functions, the
//! autoloader stack and the `iterator_*()` functions.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::{Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpStr, Value, Vm};

use super::engine::{self, ObjIterator};
use super::testutil::{closure, entries, list, new_obj, s};
use crate::testing::{TestVm, thrown};

fn str_list(v: &Value) -> Vec<String> {
    entries(v)
        .into_iter()
        .map(|(k, v)| {
            assert!(Value::str(&k).identical(&v), "key {k} maps to {v:?}");
            k
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Exceptions
// ---------------------------------------------------------------------------

#[test]
fn exception_hierarchy() {
    let t = TestVm::new();
    let parent = |name: &str| {
        t.vm.find_class(name.as_bytes())
            .unwrap()
            .parent
            .as_ref()
            .unwrap()
            .name
            .to_string()
    };
    for (class, p) in [
        ("LogicException", "Exception"),
        ("BadFunctionCallException", "LogicException"),
        ("BadMethodCallException", "BadFunctionCallException"),
        ("DomainException", "LogicException"),
        ("InvalidArgumentException", "LogicException"),
        ("LengthException", "LogicException"),
        ("OutOfRangeException", "LogicException"),
        ("RuntimeException", "Exception"),
        ("OutOfBoundsException", "RuntimeException"),
        ("OverflowException", "RuntimeException"),
        ("RangeException", "RuntimeException"),
        ("UnderflowException", "RuntimeException"),
        ("UnexpectedValueException", "RuntimeException"),
    ] {
        assert_eq!(parent(class), p, "parent of {class}");
    }
}

#[test]
fn spl_exceptions_carry_their_message() {
    let mut t = TestVm::new();
    let e =
        t.vm.throw_error("OutOfBoundsException", "Index invalid or out of range");
    let (class, msg) = thrown(Err(e));
    assert_eq!(class, "OutOfBoundsException");
    assert_eq!(msg, "Index invalid or out of range");
}

// ---------------------------------------------------------------------------
// class_implements / class_parents / class_uses
// ---------------------------------------------------------------------------

#[test]
fn class_parents_of_exceptions() {
    // spl_003.phpt (internal classes only).
    let mut t = TestVm::new();
    let r = t
        .call("class_parents", vec![s("BadMethodCallException")])
        .unwrap();
    assert_eq!(
        str_list(&r),
        ["BadFunctionCallException", "LogicException", "Exception"]
    );
    let obj = t.vm.create_exception("RangeException", "", 0);
    let r = t.call("class_parents", vec![Value::Object(obj)]).unwrap();
    assert_eq!(str_list(&r), ["RuntimeException", "Exception"]);
    let r = t.call("class_parents", vec![s("Exception")]).unwrap();
    assert!(str_list(&r).is_empty());
}

#[test]
fn class_functions_report_missing_classes() {
    // spl_003.phpt
    let mut t = TestVm::new();
    let r = t
        .call("class_parents", vec![s("foo"), Value::Bool(false)])
        .unwrap();
    assert!(r.identical(&Value::Bool(false)));
    let r = t
        .call("class_parents", vec![s("foo"), Value::Bool(true)])
        .unwrap();
    assert!(r.identical(&Value::Bool(false)));
    let r = t
        .call("class_implements", vec![s("bbb"), Value::Bool(false)])
        .unwrap();
    assert!(r.identical(&Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: class_parents(): Class foo does not exist in /t.php on line 3\n\
         \nWarning: class_parents(): Class foo does not exist and could not be loaded in /t.php on line 3\n\
         \nWarning: class_implements(): Class bbb does not exist in /t.php on line 3\n"
    );
}

#[test]
fn class_functions_reject_other_types() {
    let mut t = TestVm::new();
    for f in ["class_implements", "class_parents", "class_uses"] {
        let (class, msg) = thrown(t.call(f, vec![Value::Int(1)]));
        assert_eq!(class, "TypeError");
        assert_eq!(
            msg,
            format!(
                "{f}(): Argument #1 ($object_or_class) must be of type object|string, int given"
            )
        );
    }
}

#[test]
fn class_implements_lists_inherited_interfaces() {
    let mut t = TestVm::new();
    let r = t
        .call("class_implements", vec![s("RecursiveIterator")])
        .unwrap();
    assert_eq!(str_list(&r), ["Iterator", "Traversable"]);
    let r = t
        .call("class_implements", vec![s("\\SeekableIterator")])
        .unwrap();
    assert_eq!(str_list(&r), ["Iterator", "Traversable"]);
    let r = t
        .call("class_implements", vec![s("LogicException")])
        .unwrap();
    assert_eq!(str_list(&r), ["Throwable", "Stringable"]);
}

#[test]
fn implement_orders_like_zend_do_implement_interface() {
    let t = TestVm::new();
    let mut ce = ClassEntry::new("X", 0);
    let rec = t.vm.find_class(b"RecursiveIterator").unwrap();
    let countable = t.vm.find_class(b"Countable").unwrap();
    engine::implement(&mut ce, &rec);
    engine::implement(&mut ce, &countable);
    engine::implement(&mut ce, &rec);
    let names: Vec<String> = ce.interfaces.iter().map(|i| i.name.to_string()).collect();
    assert_eq!(
        names,
        ["RecursiveIterator", "Traversable", "Iterator", "Countable"]
    );
}

#[test]
fn class_uses_uses_the_traits_hook() {
    let mut t = TestVm::new();
    let r = t.call("class_uses", vec![s("ArrayAccess")]).unwrap();
    assert!(str_list(&r).is_empty());
    fn traits(_vm: &Vm, ce: &php_runtime::ClassRef) -> Vec<PhpStr> {
        if ce.name.as_bytes() == b"Countable" {
            vec![PhpStr::from("T1"), PhpStr::from("T2")]
        } else {
            Vec::new()
        }
    }
    super::functions::set_class_traits_hook(&mut t.vm, traits);
    let r = t.call("class_uses", vec![s("countable")]).unwrap();
    assert_eq!(str_list(&r), ["T1", "T2"]);
}

// ---------------------------------------------------------------------------
// spl_object_hash / spl_object_id / spl_classes
// ---------------------------------------------------------------------------

#[test]
fn spl_object_hash_format() {
    // spl_object_hash_deprecated.phpt
    let mut t = TestVm::new();
    let ce = t.vm.find_class(b"stdClass").unwrap();
    let o = ObjRef::new(ce);
    let id = o.id();
    let h = t
        .call("spl_object_hash", vec![Value::Object(o.clone())])
        .unwrap();
    assert_eq!(
        h.as_str().unwrap().to_string(),
        format!("{id:016x}0000000000000000")
    );
    assert_eq!(h.as_str().unwrap().len(), 32);
    assert_eq!(
        t.output(),
        "\nDeprecated: Function spl_object_hash() is deprecated since 8.6, consider using spl_object_id() instead in /t.php on line 3\n"
    );
    let r = t.call("spl_object_id", vec![Value::Object(o)]).unwrap();
    assert!(r.identical(&Value::Int(id as i64)));
    let (class, msg) = thrown(t.call("spl_object_id", vec![Value::Int(1)]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "spl_object_id(): Argument #1 ($object) must be of type object, int given"
    );
}

#[test]
fn spl_classes_lists_registered_classes() {
    let mut t = TestVm::new();
    let r = t.call("spl_classes", vec![]).unwrap();
    let names = str_list(&r);
    for n in [
        "LogicException",
        "RecursiveIterator",
        "SeekableIterator",
        "OuterIterator",
    ] {
        assert!(names.iter().any(|x| x == n), "{n} missing");
    }
    assert!(t.output().contains(
        "Deprecated: Function spl_classes() is deprecated since 8.6, use ReflectionExtension::getClassNames() instead"
    ));
}

// ---------------------------------------------------------------------------
// Autoloading
// ---------------------------------------------------------------------------

fn define_class(vm: &mut Vm, name: &str) {
    vm.register_class(Rc::new(ClassEntry::new(name, 0)));
}

#[test]
fn autoloaders_run_in_order_with_prepend() {
    // spl_autoload_010.phpt
    let mut t = TestVm::new();
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let mk = |t: &mut TestVm, tag: &'static str, define: bool| {
        let log = log.clone();
        closure(&mut t.vm, move |vm, args| {
            let name = args[0].as_str().unwrap().to_string();
            log.borrow_mut().push(format!("{tag} -> {name}"));
            if define {
                define_class(vm, &name);
            }
            Ok(Value::Null)
        })
    };
    let a = mk(&mut t, "A", false);
    let b = mk(&mut t, "B", false);
    let c = mk(&mut t, "C", true);
    t.call("spl_autoload_register", vec![a]).unwrap();
    t.call(
        "spl_autoload_register",
        vec![b, Value::Bool(true), Value::Bool(true)],
    )
    .unwrap();
    t.call("spl_autoload_register", vec![c]).unwrap();
    let ce = super::functions::autoload_class(&mut t.vm, b"C").unwrap();
    assert_eq!(ce.unwrap().name.to_string(), "C");
    assert_eq!(*log.borrow(), ["B -> C", "A -> C", "C -> C"]);
}

#[test]
fn autoload_register_rejects_spl_autoload_call() {
    // spl_autoload_throw_with_spl_autoloader_call_as_autoloader.phpt
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("spl_autoload_register", vec![s("spl_autoload_call")]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "spl_autoload_register(): Argument #1 ($callback) must not be the spl_autoload_call() function"
    );
    let (class, msg) = thrown(t.call("spl_autoload_register", vec![s("no_such_function")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "spl_autoload_register(): Argument #1 ($callback) must be a valid callback or null, function \"no_such_function\" not found or invalid function name"
    );
}

#[test]
fn autoload_register_notice_on_false_throw() {
    // spl_autoload_warn_on_false_do_throw.phpt
    let mut t = TestVm::new();
    let r = t
        .call(
            "spl_autoload_register",
            vec![s("ucfirst"), Value::Bool(false)],
        )
        .unwrap();
    assert!(r.identical(&Value::Bool(true)));
    assert_eq!(
        t.output(),
        "\nNotice: spl_autoload_register(): Argument #2 ($do_throw) has been ignored, spl_autoload_register() will always throw in /t.php on line 3\n"
    );
}

#[test]
fn autoload_functions_register_and_unregister() {
    // spl_autoload_005.phpt / spl_autoload_013.phpt (internal callables)
    let mut t = TestVm::new();
    let r = t.call("spl_autoload_functions", vec![]).unwrap();
    assert_eq!(entries(&r).len(), 0);
    let r = t
        .call("spl_autoload_unregister", vec![s("ucfirst")])
        .unwrap();
    assert!(r.identical(&Value::Bool(false)));

    let inv = closure(&mut t.vm, |_, _| Ok(Value::Null));
    t.call("spl_autoload_register", vec![]).unwrap();
    t.call("spl_autoload_register", vec![s("UCFIRST")]).unwrap();
    t.call("spl_autoload_register", vec![s("ucfirst")]).unwrap();
    t.call("spl_autoload_register", vec![inv.clone()]).unwrap();
    t.call(
        "spl_autoload_register",
        vec![s("SplFixedArrayDoesNotExist")],
    )
    .unwrap_err();
    let r = t.call("spl_autoload_functions", vec![]).unwrap();
    let e = entries(&r);
    assert_eq!(e.len(), 3);
    assert!(e[0].1.identical(&s("spl_autoload")));
    assert!(e[1].1.identical(&s("ucfirst")));
    assert!(e[2].1.identical(&inv));

    let r = t
        .call("spl_autoload_unregister", vec![s("Ucfirst")])
        .unwrap();
    assert!(r.identical(&Value::Bool(true)));
    let r = t.call("spl_autoload_unregister", vec![inv]).unwrap();
    assert!(r.identical(&Value::Bool(true)));
    let r = t.call("spl_autoload_functions", vec![]).unwrap();
    assert_eq!(entries(&r).len(), 1);

    // spl_autoload_call as callback: deprecated, clears everything.
    let r = t
        .call("spl_autoload_unregister", vec![s("spl_autoload_call")])
        .unwrap();
    assert!(r.identical(&Value::Bool(true)));
    assert_eq!(
        t.output(),
        "\nDeprecated: spl_autoload_unregister(): Using spl_autoload_call() as a callback for spl_autoload_unregister() is deprecated, to remove all registered autoloaders, call spl_autoload_unregister() for all values returned from spl_autoload_functions() in /t.php on line 3\n"
    );
    let r = t.call("spl_autoload_functions", vec![]).unwrap();
    assert_eq!(entries(&r).len(), 0);
}

#[test]
fn autoload_static_method_callables() {
    let mut t = TestVm::new();
    // A class with a static method "load" defining the requested class.
    fn load(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let name = args.str(0).to_string();
        define_class(vm, &name);
        Ok(Value::Null)
    }
    static LOAD: NativeFunction = NativeFunction {
        sig: "public static function Load(string $class): void {}",
        handler: load,
    };
    let mut ce = ClassEntry::new("MyLoader", 0);
    ce.add_method(Function::native_method(
        &LOAD,
        "MyLoader",
        member_flags::PUBLIC | member_flags::STATIC,
    ));
    t.vm.register_class(Rc::new(ce));
    t.call("spl_autoload_register", vec![s("MyLoader::load")])
        .unwrap();
    t.call(
        "spl_autoload_register",
        vec![list(vec![s("myloader"), s("LOAD")])],
    )
    .unwrap();
    let r = t.call("spl_autoload_functions", vec![]).unwrap();
    let e = entries(&r);
    assert_eq!(e.len(), 1, "same static method registered once");
    assert_eq!(
        entries(&e[0].1)
            .into_iter()
            .map(|(_, v)| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["MyLoader", "Load"]
    );
    t.call("spl_autoload_call", vec![s("Foo")]).unwrap();
    assert!(t.vm.find_class(b"foo").is_some());
}

#[test]
fn autoload_stops_at_first_exception() {
    // spl_autoload_012.phpt
    let mut t = TestVm::new();
    let log = Rc::new(RefCell::new(Vec::<&str>::new()));
    let l1 = log.clone();
    let first = closure(&mut t.vm, move |vm, _| {
        l1.borrow_mut().push("autoload_first");
        Err(vm.throw_error("Exception", "first"))
    });
    let l2 = log.clone();
    let second = closure(&mut t.vm, move |vm, _| {
        l2.borrow_mut().push("autoload_second");
        Err(vm.throw_error("Exception", "second"))
    });
    t.call("spl_autoload_register", vec![first]).unwrap();
    t.call("spl_autoload_register", vec![second]).unwrap();
    let r = super::functions::autoload_class(&mut t.vm, b"ThisClassDoesNotExist");
    let (class, msg) = thrown(r.map(|_| Value::Null));
    assert_eq!((class.as_str(), msg.as_str()), ("Exception", "first"));
    assert_eq!(*log.borrow(), ["autoload_first"]);
}

#[test]
fn autoloader_unregistering_itself_keeps_iterating() {
    // gh22060.phpt: the loader removes itself; the next loader still runs.
    let mut t = TestVm::new();
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let self_ref: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
    let (l1, sr) = (log.clone(), self_ref.clone());
    let first = closure(&mut t.vm, move |vm, _| {
        l1.borrow_mut().push("first".into());
        let me = sr.borrow().clone().unwrap();
        vm.call_native("spl_autoload_unregister", vec![me])?;
        Ok(Value::Null)
    });
    *self_ref.borrow_mut() = Some(first.clone());
    let l2 = log.clone();
    let second = closure(&mut t.vm, move |_, _| {
        l2.borrow_mut().push("second".into());
        Ok(Value::Null)
    });
    t.call("spl_autoload_register", vec![first]).unwrap();
    t.call("spl_autoload_register", vec![second]).unwrap();
    let ce = super::functions::autoload_class(&mut t.vm, b"Nope").unwrap();
    assert!(ce.is_none());
    assert_eq!(*log.borrow(), ["first", "second"]);
    let r = t.call("spl_autoload_functions", vec![]).unwrap();
    assert_eq!(entries(&r).len(), 1);
}

#[test]
fn autoload_recursion_guard_and_invalid_names() {
    let mut t = TestVm::new();
    let count = Rc::new(RefCell::new(0));
    let c = count.clone();
    let loader = closure(&mut t.vm, move |vm, args| {
        *c.borrow_mut() += 1;
        // Recursive autoload of the same class is refused.
        let inner = super::functions::autoload_class(vm, args[0].as_str().unwrap().as_bytes())?;
        assert!(inner.is_none());
        Ok(Value::Null)
    });
    t.call("spl_autoload_register", vec![loader]).unwrap();
    assert!(
        super::functions::autoload_class(&mut t.vm, b"\\Foo")
            .unwrap()
            .is_none()
    );
    assert_eq!(*count.borrow(), 1);
    assert!(
        super::functions::autoload_class(&mut t.vm, b"Foo-Bar")
            .unwrap()
            .is_none()
    );
    assert_eq!(*count.borrow(), 1);
}

#[test]
fn spl_autoload_extensions_default_and_set() {
    let mut t = TestVm::new();
    let r = t.call("spl_autoload_extensions", vec![]).unwrap();
    assert!(r.identical(&s(".inc,.php")));
    let r = t.call("spl_autoload_extensions", vec![s(".php")]).unwrap();
    assert!(r.identical(&s(".php")));
    let r = t
        .call("spl_autoload_extensions", vec![Value::Null])
        .unwrap();
    assert!(r.identical(&s(".php")));
}

thread_local! {
    static INCLUDED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn include_hook(vm: &mut Vm, path: &PhpStr) -> PResult<bool> {
    let p = path.to_string();
    INCLUDED.with(|i| i.borrow_mut().push(p.clone()));
    match p.as_str() {
        "testclass.php" => {
            define_class(vm, "TestClass");
            Ok(true)
        }
        "ns/sub/thing.inc" => {
            define_class(vm, "Ns\\Sub\\Thing");
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[test]
fn spl_autoload_tries_each_extension() {
    // spl_autoload_001.phpt / spl_autoload_009.phpt
    let mut t = TestVm::new();
    super::functions::set_include_hook(&mut t.vm, include_hook);
    INCLUDED.with(|i| i.borrow_mut().clear());
    t.call("spl_autoload", vec![s("TestClass")]).unwrap();
    assert!(t.vm.find_class(b"testclass").is_some());
    INCLUDED.with(|i| assert_eq!(*i.borrow(), ["testclass.inc", "testclass.php"]));

    INCLUDED.with(|i| i.borrow_mut().clear());
    t.call("spl_autoload", vec![s("\\Ns\\Sub\\Thing"), s(".inc,.php")])
        .unwrap();
    INCLUDED.with(|i| assert_eq!(*i.borrow(), ["ns/sub/thing.inc"]));

    INCLUDED.with(|i| i.borrow_mut().clear());
    t.call("spl_autoload", vec![s("Other"), s(",.x")]).unwrap();
    INCLUDED.with(|i| assert_eq!(*i.borrow(), ["other", "other.x"]));

    // spl_autoload_register() without arguments uses spl_autoload().
    INCLUDED.with(|i| i.borrow_mut().clear());
    t.call("spl_autoload_register", vec![]).unwrap();
    assert!(
        super::functions::autoload_class(&mut t.vm, b"Missing")
            .unwrap()
            .is_none()
    );
    INCLUDED.with(|i| assert_eq!(*i.borrow(), ["missing.inc", "missing.php"]));
}

// ---------------------------------------------------------------------------
// Iteration and iterator_*()
// ---------------------------------------------------------------------------

/// A test `Iterator` over fixed key/value pairs that logs its method calls.
struct TestIter {
    items: Vec<(Value, Value)>,
    pos: usize,
    log: Rc<RefCell<Vec<String>>>,
    throw_on_current: bool,
}

fn ti(args: &Args) -> ObjRef {
    engine::this(args)
}

fn ti_log(o: &ObjRef, m: &str) {
    let st = o.native::<TestIter>().unwrap();
    st.log.borrow_mut().push(m.to_string());
}

fn ti_rewind(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti(args);
    ti_log(&o, "rewind");
    o.native_mut::<TestIter>().unwrap().pos = 0;
    Ok(Value::Null)
}
fn ti_valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti(args);
    ti_log(&o, "valid");
    let st = o.native::<TestIter>().unwrap();
    Ok(Value::Bool(st.pos < st.items.len()))
}
fn ti_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti(args);
    ti_log(&o, "current");
    let (v, throw) = {
        let st = o.native::<TestIter>().unwrap();
        (st.items[st.pos].1.clone(), st.throw_on_current)
    };
    if throw {
        return Err(vm.throw_error("Exception", "Make the iterator break"));
    }
    Ok(v)
}
fn ti_key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti(args);
    ti_log(&o, "key");
    let st = o.native::<TestIter>().unwrap();
    Ok(st.items[st.pos].0.clone())
}
fn ti_next(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = ti(args);
    ti_log(&o, "next");
    o.native_mut::<TestIter>().unwrap().pos += 1;
    Ok(Value::Null)
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

fn test_iter(
    vm: &mut Vm,
    items: Vec<(Value, Value)>,
    throw_on_current: bool,
) -> (ObjRef, Rc<RefCell<Vec<String>>>) {
    let ce = match vm.find_class(b"SplTestIter") {
        Some(ce) => ce,
        None => {
            let mut ce = ClassEntry::new("SplTestIter", 0);
            let it = vm.find_class(b"Iterator").unwrap();
            engine::implement(&mut ce, &it);
            engine::add_methods(
                &mut ce,
                &[&TI_REWIND, &TI_VALID, &TI_CURRENT, &TI_KEY, &TI_NEXT],
            );
            let ce = Rc::new(ce);
            vm.register_class(ce.clone());
            ce
        }
    };
    let log = Rc::new(RefCell::new(Vec::new()));
    let o = ObjRef::new(ce);
    o.set_native(TestIter {
        items,
        pos: 0,
        log: log.clone(),
        throw_on_current,
    });
    (o, log)
}

/// An `IteratorAggregate` returning `inner` from `getIterator()`.
fn aggregate(vm: &mut Vm, inner: Value) -> ObjRef {
    struct Inner(Value);
    fn get_iterator(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = engine::this(args);
        let v = o.native::<Inner>().unwrap().0.clone();
        Ok(v)
    }
    static GET_ITERATOR: NativeFunction = NativeFunction {
        sig: "public function getIterator(): Iterator {}",
        handler: get_iterator,
    };
    let ce = match vm.find_class(b"SplTestAggregate") {
        Some(ce) => ce,
        None => {
            let mut ce = ClassEntry::new("SplTestAggregate", 0);
            let it = vm.find_class(b"IteratorAggregate").unwrap();
            engine::implement(&mut ce, &it);
            engine::add_methods(&mut ce, &[&GET_ITERATOR]);
            let ce = Rc::new(ce);
            vm.register_class(ce.clone());
            ce
        }
    };
    let o = ObjRef::new(ce);
    o.set_native(Inner(inner));
    o
}

fn kv(k: Value, v: Value) -> (Value, Value) {
    (k, v)
}

#[test]
fn iterator_to_array_calls_methods_in_php_order() {
    let mut t = TestVm::new();
    let (it, log) = test_iter(
        &mut t.vm,
        vec![kv(s("a"), Value::Int(1)), kv(Value::Int(5), Value::Int(2))],
        false,
    );
    let r = t
        .call("iterator_to_array", vec![Value::Object(it.clone())])
        .unwrap();
    let e = entries(&r);
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].0, "a");
    assert_eq!(e[1].0, "5");
    assert_eq!(
        *log.borrow(),
        [
            "rewind", "valid", "current", "key", "next", "valid", "current", "key", "next", "valid"
        ]
    );
    log.borrow_mut().clear();
    let r = t
        .call(
            "iterator_to_array",
            vec![Value::Object(it), Value::Bool(false)],
        )
        .unwrap();
    let a = super::testutil::array_of(&r);
    assert!(a.get(&ArrayKey::Int(0)).unwrap().identical(&Value::Int(1)));
    assert!(a.get(&ArrayKey::Int(1)).unwrap().identical(&Value::Int(2)));
    assert_eq!(
        *log.borrow(),
        [
            "rewind", "valid", "current", "next", "valid", "current", "next", "valid"
        ]
    );
}

#[test]
fn iterator_to_array_key_conversion() {
    // iterator_to_array_nonscalar_keys.phpt
    let mut t = TestVm::new();
    let (it, _) = test_iter(
        &mut t.vm,
        vec![
            kv(s("foo"), Value::Int(0)),
            kv(Value::Int(1), Value::Int(1)),
            kv(Value::Float(2.5), Value::Int(2)),
            kv(Value::Null, Value::Int(3)),
            kv(Value::empty_array(), Value::Int(4)),
        ],
        false,
    );
    let (class, msg) = thrown(t.call("iterator_to_array", vec![Value::Object(it)]));
    assert_eq!(class, "TypeError");
    assert_eq!(msg, "Cannot access offset of type array on array");
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float 2.5 to int loses precision in /t.php on line 3\n\
         \nDeprecated: Using null as an array offset is deprecated, use an empty string instead in /t.php on line 3\n"
    );
}

#[test]
fn iterator_to_array_and_count_accept_arrays() {
    // iterator_to_array_array.phpt / iterator_count_array.phpt
    let mut t = TestVm::new();
    let mut a = php_runtime::PhpArray::new();
    a.insert(ArrayKey::Str(PhpStr::from("a")), Value::Int(1));
    a.insert(ArrayKey::Int(7), Value::Int(2));
    let arr = Value::array(a);
    let r = t.call("iterator_to_array", vec![arr.clone()]).unwrap();
    assert!(r.identical(&arr));
    let r = t
        .call("iterator_to_array", vec![arr.clone(), Value::Bool(false)])
        .unwrap();
    assert!(r.identical(&list(vec![Value::Int(1), Value::Int(2)])));
    let r = t.call("iterator_count", vec![arr]).unwrap();
    assert!(r.identical(&Value::Int(2)));
    let (class, msg) = thrown(t.call("iterator_to_array", vec![s("test"), s("test")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "iterator_to_array(): Argument #1 ($iterator) must be of type Traversable|array, string given"
    );
}

#[test]
fn iterator_count_and_errors() {
    let mut t = TestVm::new();
    let (it, log) = test_iter(
        &mut t.vm,
        vec![
            kv(Value::Int(0), Value::Int(1)),
            kv(Value::Int(1), Value::Int(2)),
        ],
        true,
    );
    let r = t
        .call("iterator_count", vec![Value::Object(it.clone())])
        .unwrap();
    assert!(r.identical(&Value::Int(2)));
    assert_eq!(
        *log.borrow(),
        ["rewind", "valid", "next", "valid", "next", "valid"]
    );
    // spl_iterator_to_array_error.phpt
    let (class, msg) = thrown(t.call("iterator_to_array", vec![Value::Object(it.clone())]));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("Exception", "Make the iterator break")
    );
    let (class, _) = thrown(t.call(
        "iterator_to_array",
        vec![Value::Object(it), Value::Bool(false)],
    ));
    assert_eq!(class, "Exception");
}

#[test]
fn iterator_apply_counts_until_false() {
    let mut t = TestVm::new();
    let (it, _) = test_iter(
        &mut t.vm,
        vec![
            kv(Value::Int(0), Value::Int(1)),
            kv(Value::Int(1), Value::Int(2)),
            kv(Value::Int(2), Value::Int(3)),
        ],
        false,
    );
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s2 = seen.clone();
    let cb = closure(&mut t.vm, move |_, args| {
        s2.borrow_mut().push(args.clone());
        Ok(Value::Bool(true))
    });
    let r = t
        .call(
            "iterator_apply",
            vec![
                Value::Object(it.clone()),
                cb,
                list(vec![s("x"), Value::Int(9)]),
            ],
        )
        .unwrap();
    assert!(r.identical(&Value::Int(3)));
    assert_eq!(seen.borrow().len(), 3);
    assert!(seen.borrow()[0][0].identical(&s("x")));
    // A callback returning null stops after the first element.
    let cb = closure(&mut t.vm, |_, _| Ok(Value::Null));
    let r = t
        .call("iterator_apply", vec![Value::Object(it.clone()), cb])
        .unwrap();
    assert!(r.identical(&Value::Int(1)));
    // spl_iterator_apply_error_001.phpt
    let cb = closure(&mut t.vm, |vm, _| {
        Err(vm.throw_error("Exception", "Broken callback"))
    });
    let (class, msg) = thrown(t.call("iterator_apply", vec![Value::Object(it), cb]));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("Exception", "Broken callback")
    );
    let (class, msg) = thrown(t.call("iterator_apply", vec![Value::empty_array(), s("ucfirst")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "iterator_apply(): Argument #1 ($iterator) must be of type Traversable, array given"
    );
}

#[test]
fn aggregates_are_resolved() {
    let mut t = TestVm::new();
    let (it, _) = test_iter(&mut t.vm, vec![kv(Value::Int(3), s("v"))], false);
    let inner = aggregate(&mut t.vm, Value::Object(it));
    let outer = aggregate(&mut t.vm, Value::Object(inner));
    let r = t
        .call("iterator_to_array", vec![Value::Object(outer)])
        .unwrap();
    assert_eq!(entries(&r)[0].0, "3");

    let bad = aggregate(&mut t.vm, Value::Int(1));
    let (class, msg) = thrown(t.call("iterator_count", vec![Value::Object(bad)]));
    assert_eq!(class, "Exception");
    assert_eq!(
        msg,
        "Objects returned by SplTestAggregate::getIterator() must be traversable or implement interface Iterator"
    );
}

#[test]
fn obj_iterator_caches_current() {
    let mut t = TestVm::new();
    let (it, log) = test_iter(&mut t.vm, vec![kv(Value::Int(0), s("a"))], false);
    let mut iter = ObjIterator::new(&mut t.vm, &it).unwrap();
    iter.rewind(&mut t.vm).unwrap();
    assert!(iter.valid(&mut t.vm).unwrap());
    iter.current(&mut t.vm).unwrap();
    iter.current(&mut t.vm).unwrap();
    iter.next(&mut t.vm).unwrap();
    assert!(!iter.valid(&mut t.vm).unwrap());
    assert_eq!(
        *log.borrow(),
        ["rewind", "valid", "current", "next", "valid"]
    );
}

#[test]
fn interfaces_have_abstract_methods() {
    let mut t = TestVm::new();
    for (iface, method) in [
        ("RecursiveIterator", "haschildren"),
        ("RecursiveIterator", "getchildren"),
        ("OuterIterator", "getinneriterator"),
        ("SeekableIterator", "seek"),
    ] {
        let ce = t.vm.find_class(iface.as_bytes()).unwrap();
        assert!(ce.has_flag(class_flags::INTERFACE));
        let m = ce.find_method(method.as_bytes()).unwrap();
        assert!(m.has_flag(member_flags::ABSTRACT), "{iface}::{method}");
    }
    let ce = t.vm.find_class(b"SeekableIterator").unwrap();
    let m = ce.find_method(b"seek").unwrap().clone();
    let r = t.vm.call_function(
        &m,
        php_runtime::CallArgs {
            positional: vec![Value::Int(1)],
            ..Default::default()
        },
    );
    let (class, msg) = thrown(r);
    assert_eq!(class, "Error");
    assert_eq!(msg, "Cannot call abstract method SeekableIterator::seek()");
    let _ = new_obj;
}

#[test]
fn method_flags_from_signatures() {
    assert_eq!(
        engine::method_flags("public static function fromArray(array $array): SplFixedArray {}"),
        member_flags::PUBLIC | member_flags::STATIC
    );
    assert_eq!(
        engine::method_flags("abstract public function accept(): bool;"),
        member_flags::PUBLIC | member_flags::ABSTRACT
    );
    assert_eq!(
        engine::method_flags("current(): mixed"),
        member_flags::PUBLIC
    );
    assert_eq!(
        engine::method_flags("protected function compare(mixed $value1, mixed $value2): int {}"),
        member_flags::PROTECTED
    );
}
