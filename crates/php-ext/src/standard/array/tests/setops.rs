//! The array_diff* and array_intersect* families.

use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, member_flags};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpArray, ResRef, Vm};

use super::*;

fn args(items: &[Value]) -> Vec<Value> {
    items.to_vec()
}

#[test]
fn array_diff_basic() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_diff",
            args(&[php("[1, 2, 3, 4]"), php("[2, 4]")])
        ),
        "[0 => 1, 2 => 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_diff",
            args(&[php("['a' => 'g', 'b' => 'h']"), php("['x' => 'g']")])
        ),
        "['b' => 'h']"
    );
    // Values are compared as strings.
    assert_eq!(
        run(
            &mut t,
            "array_diff",
            args(&[php("[1, '1', 1.0, true, 2]"), php("['1']")])
        ),
        "[4 => 2]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_diff",
            args(&[php("[1, 2, 3, 4, 5]"), php("[1]"), php("[5]"), php("[3]")])
        ),
        "[1 => 2, 3 => 4]"
    );
    // Single-element first array.
    assert_eq!(
        run(&mut t, "array_diff", args(&[php("[5]"), php("[6, 5]")])),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_diff", args(&[php("[5]"), php("[6]")])),
        "[5]"
    );
    // Empty arrays.
    assert_eq!(
        run(&mut t, "array_diff", args(&[php("[]"), php("[1]")])),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_diff", args(&[php("[1, 2]"), php("[]")])),
        "[1, 2]"
    );
    assert_eq!(run(&mut t, "array_diff", args(&[php("[1, 2]")])), "[1, 2]");
    // A numeric string and an int are different strings only if spelled differently.
    assert_eq!(
        run(
            &mut t,
            "array_diff",
            args(&[php("['1', '01', '1.0']"), php("[1]")])
        ),
        "[1 => '01', 2 => '1.0']"
    );
}

#[test]
fn array_diff_array_to_string_warnings() {
    let mut t = vm();
    t.output();
    assert_eq!(
        run(
            &mut t,
            "array_diff",
            args(&[php("[1, [2], 3]"), php("[3]")])
        ),
        "[1, [2]]"
    );
    assert!(
        t.output()
            .contains("Warning: Array to string conversion in /t.php on line 3")
    );
}

#[test]
fn array_diff_key_and_assoc() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_diff_key",
            args(&[php("['a' => 1, 'b' => 2, 3]"), php("['a' => 9, 0 => 0]")])
        ),
        "['b' => 2]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_diff_key",
            args(&[php("[1, 2, 3]"), php("[1 => 'x']"), php("[2 => 'y']")])
        ),
        "[1]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_diff_assoc",
            args(&[
                php("['a' => 'g', 'b' => 'h', 0 => 'x']"),
                php("['a' => 'g', 'b' => 'x', 0 => 'x']")
            ])
        ),
        "['b' => 'h']"
    );
    // Values compare as strings: 1 == "1".
    assert_eq!(
        run(
            &mut t,
            "array_diff_assoc",
            args(&[php("['a' => 1, 'b' => 2]"), php("['a' => '1', 'b' => '3']")])
        ),
        "['b' => 2]"
    );
    assert_eq!(
        run(&mut t, "array_diff_assoc", args(&[php("['a' => 1]")])),
        "['a' => 1]"
    );
}

#[test]
fn array_udiff_family() {
    let mut t = vm();
    // array_udiff: data callback only.
    assert_eq!(
        run(
            &mut t,
            "array_udiff",
            args(&[php("['a', 'B', 'c']"), php("['A', 'c']"), s("t_strcmp")])
        ),
        "['a', 'B']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_udiff",
            args(&[php("[1, 5, 3]"), php("[3, 1]"), s("t_cmp")])
        ),
        "[1 => 5]"
    );
    // array_udiff_assoc: keys compared as strings, values with the callback.
    assert_eq!(
        run(
            &mut t,
            "array_udiff_assoc",
            args(&[
                php("['a' => 'x', 'b' => 'y']"),
                php("['a' => 'x', 'b' => 'z']"),
                s("t_strcmp")
            ])
        ),
        "['b' => 'y']"
    );
    // array_diff_ukey: key callback only.
    assert_eq!(
        run(
            &mut t,
            "array_diff_ukey",
            args(&[
                php("['a' => 1, 'b' => 2, 'c' => 3]"),
                php("['a' => 9, 'c' => 0]"),
                s("t_cmp")
            ])
        ),
        "['b' => 2]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_diff_ukey",
            args(&[php("[10 => 1, 20 => 2]"), php("[20 => 0]"), s("t_cmp")])
        ),
        "[10 => 1]"
    );
    // array_diff_uassoc: key callback plus internal value comparison.
    assert_eq!(
        run(
            &mut t,
            "array_diff_uassoc",
            args(&[
                php("['a' => 'g', 'b' => 'h']"),
                php("['a' => 'g', 'b' => 'x']"),
                s("t_cmp")
            ])
        ),
        "['b' => 'h']"
    );
    // array_udiff_uassoc: data callback first, key callback last.
    assert_eq!(
        run(
            &mut t,
            "array_udiff_uassoc",
            args(&[
                php("['a' => 'x', 'b' => 'y']"),
                php("['a' => 'x', 'b' => 'z']"),
                s("t_strcmp"),
                s("t_cmp")
            ])
        ),
        "['b' => 'y']"
    );
}

#[test]
fn array_udiff_comparison_sequence() {
    // The walk over the sorted lists mirrors php_array_diff().
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_udiff",
            args(&[php("[1, 2]"), php("[2]"), s("t_log")])
        ),
        "[1]"
    );
    assert_eq!(take_log(), ["1,2", "1,2", "1,2", "2,2"]);
}

#[test]
fn callback_argument_validation() {
    let mut t = vm();
    assert_eq!(
        run_err(&mut t, "array_udiff", args(&[php("[1]")])),
        "ArgumentCountError: array_udiff() expects at least 2 arguments, 1 given"
    );
    assert_eq!(
        run_err(
            &mut t,
            "array_udiff_uassoc",
            args(&[php("[1]"), s("t_cmp")])
        ),
        "ArgumentCountError: array_udiff_uassoc() expects at least 3 arguments, 2 given"
    );
    assert_eq!(
        run_err(
            &mut t,
            "array_udiff",
            args(&[php("[1]"), php("[2]"), s("nope")])
        ),
        "TypeError: array_udiff(): Argument #3 must be a valid callback, function \"nope\" not found or invalid function name"
    );
    assert_eq!(
        run_err(
            &mut t,
            "array_udiff_uassoc",
            args(&[php("[1]"), s("t_cmp"), s("nope")])
        ),
        "TypeError: array_udiff_uassoc(): Argument #3 must be a valid callback, function \"nope\" not found or invalid function name"
    );
    assert_eq!(
        run_err(&mut t, "array_udiff", args(&[php("[1]"), i(5), s("t_cmp")])),
        "TypeError: array_udiff(): Argument #2 must be of type array, int given"
    );
    assert_eq!(
        run_err(&mut t, "array_diff", args(&[php("[1]"), i(5)])),
        "TypeError: array_diff(): Argument #2 must be of type array, int given"
    );
    assert_eq!(
        run_err(&mut t, "array_intersect_ukey", args(&[php("[1]")])),
        "ArgumentCountError: array_intersect_ukey() expects at least 2 arguments, 1 given"
    );
}

#[test]
fn array_intersect_basic() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[php("[1, 2, 3, 4]"), php("[2, 4, 6]")])
        ),
        "[1 => 2, 3 => 4]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[
                php("['a' => 'green', 'red', 'blue']"),
                php("['b' => 'green', 'yellow', 'red']")
            ])
        ),
        "['a' => 'green', 0 => 'red']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[php("[1, 2, 3, 4, 5]"), php("[1, 2, 3]"), php("[2, 3, 9]")])
        ),
        "[1 => 2, 2 => 3]"
    );
    // Values are compared as strings.
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[php("[1, '1', 1.0, '01', 2]"), php("['1']")])
        ),
        "[1, '1', 1.0]"
    );
    // Empty arguments.
    assert_eq!(
        run(&mut t, "array_intersect", args(&[php("[1, 2]"), php("[]")])),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_intersect", args(&[php("[]"), php("[1]")])),
        "[]"
    );
    // A single array is returned as is.
    assert_eq!(
        run(&mut t, "array_intersect", args(&[php("[3, 1, 2, 1]")])),
        "[3, 1, 2, 1]"
    );
    assert_eq!(
        run_err(&mut t, "array_intersect", args(&[php("[1]"), i(5)])),
        "TypeError: array_intersect(): Argument #2 must be of type array, int given"
    );
}

#[test]
fn array_intersect_empty_result_keeps_next_free_index() {
    let mut t = vm();
    let r = t
        .call("array_intersect", args(&[php("[1, 2, 3]"), php("[]")]))
        .unwrap();
    let Value::Array(mut a) = r else {
        unreachable!()
    };
    std::rc::Rc::make_mut(&mut a).append(i(9)).unwrap();
    assert_eq!(show(&Value::Array(a)), "[3 => 9]");
}

#[test]
fn array_intersect_key_and_assoc() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_intersect_key",
            args(&[
                php("['blue' => 1, 'red' => 2, 'green' => 3, 'purple' => 4]"),
                php("['green' => 5, 'blue' => 6, 'yellow' => 7, 'cyan' => 8]")
            ])
        ),
        "['blue' => 1, 'green' => 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect_key",
            args(&[
                php("[1, 2, 3]"),
                php("[1 => 'x', 2 => 'y']"),
                php("[2 => 'z']")
            ])
        ),
        "[2 => 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect_assoc",
            args(&[
                php("['a' => 'green', 'b' => 'brown', 'c' => 'blue', 'red']"),
                php("['a' => 'green', 'yellow', 'red']")
            ])
        ),
        "['a' => 'green']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect_assoc",
            args(&[php("['a' => 1, 'b' => 2]"), php("['a' => '1', 'b' => 3]")])
        ),
        "['a' => 1]"
    );
}

#[test]
fn array_uintersect_family() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_uintersect",
            args(&[
                php("['a', 'B', 'c', 'd']"),
                php("['A', 'c', 'd']"),
                s("t_strcmp")
            ])
        ),
        "[2 => 'c', 3 => 'd']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_uintersect",
            args(&[php("[5, 1, 3, 1]"), php("[3, 1]"), s("t_cmp")])
        ),
        "[1 => 1, 2 => 3, 3 => 1]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_uintersect_assoc",
            args(&[
                php("['a' => 'x', 'b' => 'y']"),
                php("['a' => 'x', 'b' => 'z']"),
                s("t_strcmp")
            ])
        ),
        "['a' => 'x']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect_ukey",
            args(&[
                php("['a' => 1, 'b' => 2, 'c' => 3]"),
                php("['a' => 9, 'c' => 0]"),
                s("t_cmp")
            ])
        ),
        "['a' => 1, 'c' => 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect_uassoc",
            args(&[
                php("['a' => 'g', 'b' => 'h']"),
                php("['a' => 'g', 'b' => 'x']"),
                s("t_cmp")
            ])
        ),
        "['a' => 'g']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_uintersect_uassoc",
            args(&[
                php("['a' => 'x', 'b' => 'y']"),
                php("['a' => 'x', 'b' => 'z']"),
                s("t_strcmp"),
                s("t_cmp")
            ])
        ),
        "['a' => 'x']"
    );
    // Three arrays.
    assert_eq!(
        run(
            &mut t,
            "array_intersect_ukey",
            args(&[
                php("['a' => 1, 'b' => 2, 'c' => 3]"),
                php("['a' => 0, 'b' => 0]"),
                php("['b' => 0, 'c' => 0]"),
                s("t_cmp")
            ])
        ),
        "['b' => 2]"
    );
}

#[test]
fn bool_returning_callbacks_are_deprecated_once() {
    let mut t = vm();
    t.output();
    // A callback returning bool (`$a > $b`) works with the swap-and-retry rule.
    assert_eq!(
        run(
            &mut t,
            "array_udiff",
            args(&[php("[1, 5, 3]"), php("[3, 1]"), s("t_gt")])
        ),
        "[1 => 5]"
    );
    let out = t.output();
    assert_eq!(
        out.matches("Deprecated: array_udiff(): Returning bool from comparison function is deprecated, return an integer less than, equal to, or greater than zero").count(),
        1,
        "{out}"
    );
}

#[test]
fn array_diff_uassoc_bug42838() {
    let mut t = vm();
    let a = "['a' => 'green', 'b' => 'Brown', 'c' => 'blue', 0 => 'red']";
    assert_eq!(
        run(
            &mut t,
            "array_diff_uassoc",
            args(&[php(a), php(a), s("t_cmp")])
        ),
        "[]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_diff_ukey",
            args(&[php(a), php(a), s("t_cmp")])
        ),
        "[]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect_uassoc",
            args(&[php(a), php(a), s("t_cmp")])
        ),
        "['a' => 'green', 'b' => 'Brown', 'c' => 'blue', 0 => 'red']"
    );
}

fn to_string_object(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::from("object"))
}

static OBJECT_TO_STRING: NativeFunction = NativeFunction {
    sig: "__toString(): string",
    handler: to_string_object,
};

fn throwing_to_string(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "conversion failed"))
}

static THROWING_TO_STRING: NativeFunction = NativeFunction {
    sig: "__toString(): string",
    handler: throwing_to_string,
};

fn stringable(name: &str, f: &'static NativeFunction) -> Value {
    let mut ce = ClassEntry::new(name, 0);
    ce.add_method(Function::native_method(f, name, member_flags::PUBLIC));
    ce.link_magic_methods();
    Value::Object(ObjRef::new(Rc::new(ce)))
}

#[test]
fn array_intersect_values_phpt() {
    // array_intersect_values.phpt
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[
                php(
                    "['first' => 1, 'duplicate' => 1, 'leading' => '01', 'plus' => '+1', 'exponent' => '1e0', 'zero' => 0, 'negative-zero' => '-0']"
                ),
                php("['1', '0']"),
            ])
        ),
        "['first' => 1, 'duplicate' => 1, 'zero' => 0]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[
                php("[1, 2, '2', 3]"),
                php("[1, 2, 2, '2', 3]"),
                php("[2, '2', 3]")
            ])
        ),
        "[1 => 2, 2 => '2', 3 => 3]"
    );
    // Integers at the limits compare equal to their string forms only.
    let max = i(i64::MAX);
    let min = i(i64::MIN);
    let smax = s(&i64::MAX.to_string());
    let smin = s(&i64::MIN.to_string());
    let above = s("9223372036854775808");
    let below = s("-9223372036854775809");
    let both = Value::array(PhpArray::from_values(vec![max.clone(), min.clone()]));
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[
                both.clone(),
                Value::array(PhpArray::from_values(vec![smax, smin]))
            ])
        ),
        show(&both)
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[
                both,
                Value::array(PhpArray::from_values(vec![above.clone(), below.clone()]))
            ])
        ),
        "[]"
    );
    let strs = Value::array(PhpArray::from_values(vec![above, below]));
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[strs.clone(), strs.clone()])
        ),
        show(&strs)
    );
    // References in the first array are kept when they have other owners.
    let integer = PhpRef::new(i(2));
    let string = PhpRef::new(s("2"));
    let first = PhpArray::from_values(vec![Value::Ref(integer.clone()), i(3)]);
    let second = PhpArray::from_values(vec![Value::Ref(string), i(3)]);
    let r = t
        .call(
            "array_intersect",
            args(&[Value::array(first), Value::array(second)]),
        )
        .unwrap();
    integer.set(i(9));
    let Value::Array(r) = r else { unreachable!() };
    assert_eq!(show(r.get_int(0).unwrap()), "9");
    // The result is a copy of the first array: its holes and keys survive.
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[
                php("[0 => 'drop', 100 => 'x', 200 => 'y']"),
                php("['x', 'y']")
            ])
        ),
        "[100 => 'x', 200 => 'y']"
    );
}

#[test]
fn array_intersect_converts_every_value_type() {
    let mut t = vm();
    // Resources convert to "Resource id #N", objects through __toString.
    let resource = ResRef::new("stream", Box::new(()));
    let rid = format!("Resource id #{}", resource.id());
    let mut first = PhpArray::new();
    first.insert("null".into(), Value::Null);
    first.insert("false".into(), Value::Bool(false));
    first.insert("true".into(), Value::Bool(true));
    first.insert("zero-float".into(), Value::Float(0.0));
    first.insert("float".into(), Value::Float(1.5));
    first.insert("resource".into(), Value::Resource(resource.clone()));
    first.insert(
        "object".into(),
        stringable("StringableValue", &OBJECT_TO_STRING),
    );
    let second = PhpArray::from_values(vec![s(""), s("1"), s("0"), s("1.5"), s(&rid), s("object")]);
    let third = PhpArray::from_values(vec![
        Value::Bool(false),
        Value::Bool(true),
        i(0),
        s("1.5"),
        Value::Resource(resource),
        stringable("StringableValue2", &OBJECT_TO_STRING),
    ]);
    let r = t
        .call(
            "array_intersect",
            args(&[
                Value::array(first),
                Value::array(second),
                Value::array(third),
            ]),
        )
        .unwrap();
    let Value::Array(r) = r else { unreachable!() };
    let keys: Vec<String> = r.keys().map(|k| k.to_php_str().to_string()).collect();
    assert_eq!(
        keys.join(","),
        "null,false,true,zero-float,float,resource,object"
    );
}

#[test]
fn array_intersect_conversion_errors() {
    let mut t = vm();
    assert_eq!(
        run_err(
            &mut t,
            "array_intersect",
            args(&[
                php("['value']"),
                Value::array(PhpArray::from_values(vec![
                    stringable("ThrowingStringableValue", &THROWING_TO_STRING),
                    s("value"),
                ])),
            ])
        ),
        "Exception: conversion failed"
    );
    let std = Value::Object(t.vm.cast_to_object(&Value::Null).unwrap());
    assert_eq!(
        run_err(
            &mut t,
            "array_intersect",
            args(&[php("['']"), Value::array(PhpArray::from_values(vec![std]))])
        ),
        "Error: Object of class stdClass could not be converted to string"
    );
    // Arrays convert to "Array" with a warning.
    t.output();
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[php("[[1]]"), php("[[2]]")])
        ),
        "[[1]]"
    );
    assert!(t.output().contains("Warning: Array to string conversion"));
}

#[test]
fn array_intersect_does_not_convert_when_an_argument_is_empty() {
    // array_intersect_empty.phpt
    let mut t = vm();
    let throwing = || {
        Value::array(PhpArray::from_values(vec![stringable(
            "ThrowingStringableValue",
            &THROWING_TO_STRING,
        )]))
    };
    t.output();
    assert_eq!(
        run(&mut t, "array_intersect", args(&[php("[]"), php("[[1]]")])),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_intersect", args(&[php("[[1]]"), php("[]")])),
        "[]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_intersect",
            args(&[throwing(), php("['value']"), php("[]")])
        ),
        "[]"
    );
    assert_eq!(t.output(), "");
    assert_eq!(
        run_err(
            &mut t,
            "array_intersect",
            args(&[php("[]"), php("[]"), i(5)])
        ),
        "TypeError: array_intersect(): Argument #3 must be of type array, int given"
    );
}

#[test]
fn single_array_arguments_are_returned_unchanged() {
    // array_diff_single_array.phpt
    let mut t = vm();
    let a = php("['a' => 42]");
    for f in [
        "array_diff",
        "array_diff_key",
        "array_diff_assoc",
        "array_intersect_key",
        "array_intersect_assoc",
    ] {
        assert_eq!(run(&mut t, f, vec![a.clone()]), "['a' => 42]", "{f}");
    }
    for f in [
        "array_diff_ukey",
        "array_diff_uassoc",
        "array_udiff",
        "array_udiff_assoc",
        "array_uintersect",
        "array_uintersect_assoc",
        "array_intersect_ukey",
        "array_intersect_uassoc",
    ] {
        assert_eq!(
            run(&mut t, f, args(&[a.clone(), s("t_cmp")])),
            "['a' => 42]",
            "{f}"
        );
    }
    assert_eq!(
        run(
            &mut t,
            "array_udiff_uassoc",
            args(&[a.clone(), s("t_cmp"), s("t_cmp")])
        ),
        "['a' => 42]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_uintersect_uassoc",
            args(&[a, s("t_cmp"), s("t_cmp")])
        ),
        "['a' => 42]"
    );
}
