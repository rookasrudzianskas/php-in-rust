//! current, key, next, prev, reset, end, array_walk, array_walk_recursive.

use std::rc::Rc;

use php_runtime::class::{ClassEntry, PropertyInfo, member_flags};
use php_runtime::{ObjRef, PhpArray};

use super::*;

/// Calls the by-reference pointer function `name` on the variable `r`.
fn ptr(t: &mut TestVm, r: &PhpRef, name: &str) -> String {
    run(t, name, vec![Value::Ref(r.clone())])
}

/// `current()` / `key()` take their argument by value.
fn peek(t: &mut TestVm, r: &PhpRef, name: &str) -> String {
    run(t, name, vec![r.get()])
}

#[test]
fn internal_pointer_walk() {
    let mut t = vm();
    let r = PhpRef::new(php("[1, 2, 3]"));
    assert_eq!(peek(&mut t, &r, "current"), "1");
    assert_eq!(peek(&mut t, &r, "key"), "0");
    assert_eq!(ptr(&mut t, &r, "next"), "2");
    assert_eq!(peek(&mut t, &r, "current"), "2");
    assert_eq!(peek(&mut t, &r, "pos"), "2");
    assert_eq!(ptr(&mut t, &r, "next"), "3");
    assert_eq!(peek(&mut t, &r, "key"), "2");
    // Past the end.
    assert_eq!(ptr(&mut t, &r, "next"), "false");
    assert_eq!(peek(&mut t, &r, "current"), "false");
    assert_eq!(peek(&mut t, &r, "key"), "null");
    assert_eq!(ptr(&mut t, &r, "next"), "false");
    // prev() from beyond the end stays invalid.
    assert_eq!(ptr(&mut t, &r, "prev"), "false");
    assert_eq!(ptr(&mut t, &r, "reset"), "1");
    assert_eq!(ptr(&mut t, &r, "prev"), "false");
    assert_eq!(peek(&mut t, &r, "key"), "null");
    assert_eq!(ptr(&mut t, &r, "end"), "3");
    assert_eq!(ptr(&mut t, &r, "prev"), "2");
    assert_eq!(peek(&mut t, &r, "key"), "1");
    assert_eq!(ptr(&mut t, &r, "reset"), "1");
}

#[test]
fn internal_pointer_with_string_keys_and_empty_arrays() {
    let mut t = vm();
    let r = PhpRef::new(php("['a' => 'x', 'b' => 'y', 5 => 'z']"));
    assert_eq!(peek(&mut t, &r, "key"), "'a'");
    assert_eq!(ptr(&mut t, &r, "end"), "'z'");
    assert_eq!(peek(&mut t, &r, "key"), "5");
    assert_eq!(ptr(&mut t, &r, "prev"), "'y'");
    assert_eq!(peek(&mut t, &r, "key"), "'b'");

    let e = PhpRef::new(php("[]"));
    for f in ["reset", "end", "next", "prev"] {
        assert_eq!(ptr(&mut t, &e, f), "false", "{f}");
    }
    assert_eq!(peek(&mut t, &e, "current"), "false");
    assert_eq!(peek(&mut t, &e, "key"), "null");

    // Values that are references are dereferenced.
    let inner = PhpRef::new(i(7));
    let arr = PhpArray::from_values(vec![Value::Ref(inner)]);
    let r = PhpRef::new(Value::array(arr));
    assert_eq!(ptr(&mut t, &r, "reset"), "7");
    // A value that is `false` is indistinguishable from the failure value.
    let r = PhpRef::new(php("[false]"));
    assert_eq!(ptr(&mut t, &r, "reset"), "false");
    assert_eq!(peek(&mut t, &r, "key"), "0");
}

#[test]
fn pointer_moves_do_not_leak_to_copies() {
    let mut t = vm();
    let a = PhpRef::new(php("[1, 2, 3]"));
    // $b = $a: shares the array until it is written to.
    let b = PhpRef::new(a.get());
    assert_eq!(ptr(&mut t, &a, "next"), "2");
    assert_eq!(peek(&mut t, &a, "current"), "2");
    assert_eq!(peek(&mut t, &b, "current"), "1");
}

#[test]
fn internal_pointer_is_reset_by_modifying_functions() {
    let mut t = vm();
    let a = PhpRef::new(php("[1, 2, 3]"));
    ptr(&mut t, &a, "next");
    ptr(&mut t, &a, "next");
    assert_eq!(peek(&mut t, &a, "current"), "3");
    run(&mut t, "array_shift", vec![Value::Ref(a.clone())]);
    assert_eq!(peek(&mut t, &a, "current"), "2");
    ptr(&mut t, &a, "next");
    run(&mut t, "array_pop", vec![Value::Ref(a.clone())]);
    assert_eq!(peek(&mut t, &a, "current"), "2");
    run(&mut t, "sort", vec![Value::Ref(a.clone())]);
    assert_eq!(peek(&mut t, &a, "current"), "2");
}

fn my_class() -> Rc<ClassEntry> {
    let mut ce = ClassEntry::new("MyClass", 0);
    ce.declare_property(PropertyInfo::new(
        "pri_value",
        "MyClass",
        member_flags::PRIVATE,
    ));
    ce.declare_property(PropertyInfo::new(
        "pub_value",
        "MyClass",
        member_flags::PUBLIC,
    ));
    ce.declare_property(PropertyInfo::new(
        "pro_value",
        "MyClass",
        member_flags::PROTECTED,
    ));
    Rc::new(ce)
}

fn my_object() -> Value {
    let o = ObjRef::new(my_class());
    o.set_prop(b"pri_value", i(1));
    o.set_prop(b"pub_value", i(2));
    o.set_prop(b"pro_value", i(3));
    Value::Object(o)
}

#[test]
fn internal_pointer_of_objects_is_deprecated() {
    let mut t = vm();
    let o = my_object();
    let r = PhpRef::new(o.clone());
    t.output();
    assert_eq!(peek(&mut t, &r, "current"), "1");
    assert_eq!(
        t.output(),
        "\nDeprecated: current(): Calling current() on an object is deprecated in /t.php on line 3\n"
    );
    assert_eq!(ptr(&mut t, &r, "next"), "2");
    assert!(
        t.output()
            .contains("Deprecated: next(): Calling next() on an object is deprecated")
    );
    assert_eq!(peek(&mut t, &r, "key"), "'pub_value'");
    t.output();
    assert_eq!(ptr(&mut t, &r, "next"), "3");
    assert_eq!(peek(&mut t, &r, "key"), "'\\0*\\0pro_value'");
    assert_eq!(ptr(&mut t, &r, "next"), "false");
    assert_eq!(ptr(&mut t, &r, "end"), "3");
    assert_eq!(ptr(&mut t, &r, "reset"), "1");
    assert_eq!(peek(&mut t, &r, "pos"), "1");
    assert!(
        t.output()
            .contains("Deprecated: pos(): Calling pos() on an object is deprecated")
    );
}

#[test]
fn array_walk_modifies_by_reference() {
    let mut t = vm();
    let (r, a) = call_ref(&mut t, "array_walk", php("[1, 2, 3]"), vec![s("t_inc")]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&a), "[2, 3, 4]");
    // With the extra argument.
    let (_, a) = call_ref(
        &mut t,
        "array_walk",
        php("['a' => 1, 'b' => 2]"),
        vec![s("t_inc"), i(10)],
    );
    assert_eq!(show(&a), "['a' => 12, 'b' => 13]");
    // Empty arrays do nothing.
    let (r, a) = call_ref(&mut t, "array_walk", php("[]"), vec![s("t_inc")]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&a), "[]");
}

#[test]
fn array_walk_passes_value_and_key_in_order() {
    let mut t = vm();
    let (_, _) = call_ref(
        &mut t,
        "array_walk",
        php("['lemon', 'orange', 'x' => 'banana', 9 => 'apple']"),
        vec![s("t_walk_log")],
    );
    assert_eq!(take_log(), ["0=lemon", "1=orange", "x=banana", "9=apple"]);
}

#[test]
fn array_walk_recursive_phpt() {
    let mut t = vm();
    // array_walk_recursive1.phpt: nested arrays are descended into.
    let (_, _) = call_ref(
        &mut t,
        "array_walk_recursive",
        php("[1, 2, [2, 3]]"),
        vec![s("t_walk_log")],
    );
    assert_eq!(take_log(), ["0=1", "1=2", "0=2", "1=3"]);
    let (_, a) = call_ref(
        &mut t,
        "array_walk_recursive",
        php("[1, [2, [3, 4]], 'k' => 5]"),
        vec![s("t_inc")],
    );
    assert_eq!(show(&a), "[0 => 2, 1 => [3, [4, 5]], 'k' => 6]");
    // Exceptions propagate.
    let (r, _) = call_ref(
        &mut t,
        "array_walk_recursive",
        php("[1, [2]]"),
        vec![s("t_throw")],
    );
    assert_eq!(thrown(r), ("Exception".into(), "boom".into()));
}

#[test]
fn array_walk_recursive_detects_recursion() {
    let mut t = vm();
    // $a = [1]; $a[] = &$a;
    let var = PhpRef::new(Value::Null);
    var.set(Value::array(PhpArray::from_values(vec![
        i(1),
        Value::Ref(var.clone()),
    ])));
    let r = t.call(
        "array_walk_recursive",
        vec![Value::Ref(var.clone()), s("t_walk_log")],
    );
    assert_eq!(thrown(r), ("Error".into(), "Recursion detected".into()));
}

#[test]
fn array_walk_objects_are_deprecated() {
    // array_walk_object1.phpt: keys are the mangled property names.
    let mut t = vm();
    t.output();
    let (r, _) = call_ref(&mut t, "array_walk", my_object(), vec![s("t_walk_log")]);
    assert_eq!(show(&r.unwrap()), "true");
    let out = t.output();
    assert!(out.contains(
        "Deprecated: array_walk(): Passing an object for argument #1 $array to array_walk() is deprecated, call get_object_vars() first instead in /t.php on line 3"
    ));
    assert_eq!(
        take_log(),
        ["\0MyClass\0pri_value=1", "pub_value=2", "\0*\0pro_value=3"]
    );
}

#[test]
fn array_walk_callback_is_validated() {
    let mut t = vm();
    let (r, _) = call_ref(&mut t, "array_walk", php("[1]"), vec![s("nope")]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "array_walk(): Argument #2 ($callback) must be a valid callback, function \"nope\" not found or invalid function name".into()
        )
    );
    let (r, _) = call_ref(&mut t, "array_walk", s("x"), vec![s("t_inc")]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "array_walk(): Argument #1 ($array) must be of type object|array, string given".into()
        )
    );
}

#[test]
fn pointer_at_the_end_sees_later_appends() {
    // current_variation6.phpt
    let mut t = vm();
    let r = PhpRef::new(php("['foo' => 1, 'bar' => 2, 'baz' => 3]"));
    assert_eq!(ptr(&mut t, &r, "reset"), "1");
    assert_eq!(ptr(&mut t, &r, "next"), "2");
    assert_eq!(ptr(&mut t, &r, "next"), "3");
    assert_eq!(ptr(&mut t, &r, "next"), "false");
    // unset($array["baz"]); $array[] = 4;
    {
        let mut b = r.borrow_mut();
        let Value::Array(a) = &mut *b else {
            unreachable!()
        };
        let a = Rc::make_mut(a);
        a.remove(&"baz".into());
        a.append(i(4)).unwrap();
    }
    assert_eq!(peek(&mut t, &r, "current"), "4");
}

#[test]
fn pointer_is_shared_between_references() {
    // end_variation3.phpt
    let mut t = vm();
    let a1 = PhpRef::new(php("['zero', 'one', 'two']"));
    assert_eq!(peek(&mut t, &a1, "current"), "'zero'");
    ptr(&mut t, &a1, "end");
    let a2 = a1.clone(); // $array2 = &$array1
    assert_eq!(peek(&mut t, &a1, "current"), "'two'");
    assert_eq!(peek(&mut t, &a2, "current"), "'two'");
}

fn typed_class(name: &str, props: &[&str]) -> Rc<ClassEntry> {
    let mut ce = ClassEntry::new(name, 0);
    for p in props {
        let mut info = PropertyInfo::new(*p, name, member_flags::PUBLIC);
        info.default = Value::Undef; // `public int $p;` is uninitialised
        ce.declare_property(info);
    }
    Rc::new(ce)
}

#[test]
fn internal_pointer_skips_uninitialised_properties() {
    // GH-16905: typed properties without a value are skipped.
    let mut t = vm();
    let x = ObjRef::new(typed_class("TestSomeUndef", &["a", "b", "c", "d"]));
    x.set_prop(b"b", i(1));
    x.set_prop(b"c", i(2));
    let r = PhpRef::new(Value::Object(x));
    assert_eq!(ptr(&mut t, &r, "reset"), "1");
    assert_eq!(peek(&mut t, &r, "current"), "1");
    assert_eq!(ptr(&mut t, &r, "end"), "2");
    assert_eq!(ptr(&mut t, &r, "reset"), "1");
    assert_eq!(ptr(&mut t, &r, "next"), "2");
    assert_eq!(ptr(&mut t, &r, "end"), "2");
    assert_eq!(ptr(&mut t, &r, "prev"), "1");
    assert_eq!(peek(&mut t, &r, "key"), "'b'");
    assert_eq!(peek(&mut t, &r, "current"), "1");

    // An object without any initialised property: the pointer is moved past
    // the end by the first read and stays there until reset().
    let y = ObjRef::new(typed_class("TestAllUndef", &["a"]));
    let r = PhpRef::new(Value::Object(y.clone()));
    assert_eq!(peek(&mut t, &r, "key"), "null");
    assert_eq!(peek(&mut t, &r, "current"), "false");
    y.set_prop(b"a", i(1));
    assert_eq!(peek(&mut t, &r, "key"), "null");
    assert_eq!(peek(&mut t, &r, "current"), "false");
    ptr(&mut t, &r, "reset");
    assert_eq!(peek(&mut t, &r, "key"), "'a'");
    assert_eq!(peek(&mut t, &r, "current"), "1");
}
