//! `zend_parse_parameters` semantics for native functions.

use std::rc::Rc;

use super::*;
use crate::native::CallArgs;
use crate::value::PhpArray;

fn call_strict(t: &mut TestVm, name: &str, args: Vec<Value>) -> PResult<Value> {
    let f = t.vm.find_function(name.as_bytes()).unwrap();
    t.vm.invoke_native(
        &f,
        CallArgs {
            positional: args,
            strict: true,
            ..CallArgs::default()
        },
    )
}

fn call_named(
    t: &mut TestVm,
    name: &str,
    args: Vec<Value>,
    named: Vec<(&str, Value)>,
) -> PResult<Value> {
    t.vm.call_native_named(name, args, named)
}

#[test]
fn arity_messages() {
    let mut t = TestVm::new();
    assert_eq!(
        thrown(t.call("strlen", vec![])),
        (
            "ArgumentCountError".into(),
            "strlen() expects exactly 1 argument, 0 given".into()
        )
    );
    assert_eq!(
        thrown(t.call("strlen", vec![s("a"), s("b")])),
        (
            "ArgumentCountError".into(),
            "strlen() expects exactly 1 argument, 2 given".into()
        )
    );
    assert_eq!(
        thrown(t.call("str_pad", vec![s("x")])),
        (
            "ArgumentCountError".into(),
            "str_pad() expects at least 2 arguments, 1 given".into()
        )
    );
    assert_eq!(
        thrown(t.call(
            "str_pad",
            vec![s("x"), 1.into(), s(" "), 1.into(), 5.into()]
        )),
        (
            "ArgumentCountError".into(),
            "str_pad() expects at most 4 arguments, 5 given".into()
        )
    );
    assert_eq!(
        thrown(t.call("t_var", vec![])),
        (
            "ArgumentCountError".into(),
            "t_var() expects at least 1 argument, 0 given".into()
        )
    );
    assert_eq!(
        thrown(t.call("intdiv", vec![1.into()])),
        (
            "ArgumentCountError".into(),
            "intdiv() expects exactly 2 arguments, 1 given".into()
        )
    );
}

#[test]
fn defaults_are_filled_from_the_signature() {
    let mut t = TestVm::new();
    assert_eq!(
        t.call("str_pad", vec![s("ab"), 5.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        "ab   "
    );
    assert_eq!(
        t.call("str_pad", vec![s("ab"), 5.into(), s("-"), 0.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        "---ab"
    );
    let r = t.call("t_defaults", vec![]).unwrap();
    let a = r.as_array().unwrap();
    assert_eq!(a.len(), 5);
    assert_eq!(a.get_int(0).unwrap().as_str().unwrap(), "<default>");
    let f = t.vm.find_function(b"t_defaults").unwrap();
    let mut args = Vec::new();
    t.vm.push_frame(crate::vm::Frame::user(&t.vm, f.clone(), None, vec![]));
    let prepared = t.vm.prepare_native_args(&f, CallArgs::default()).unwrap();
    t.vm.pop_frame();
    for i in 0..5 {
        args.push(prepared.get(i).clone());
        assert!(!prepared.is_passed(i));
    }
    assert!(matches!(args[0], Value::Int(1)));
    assert_eq!(args[1].as_str().unwrap(), "x");
    assert!(matches!(args[2], Value::Null));
    assert!(matches!(args[3], Value::Int(2)));
    assert!(matches!(args[4], Value::Float(f) if f == -1.5));
}

#[test]
fn weak_int_coercion() {
    let mut t = TestVm::new();
    let ok = |t: &mut TestVm, v: Value| t.call("t_int", vec![v]).unwrap().as_int().unwrap();
    assert_eq!(ok(&mut t, 5.into()), 5);
    assert_eq!(ok(&mut t, s("42")), 42);
    assert_eq!(ok(&mut t, s(" 42 ")), 42);
    assert_eq!(ok(&mut t, s("1e3")), 1000);
    assert_eq!(ok(&mut t, true.into()), 1);
    assert_eq!(ok(&mut t, false.into()), 0);
    assert_eq!(ok(&mut t, 3.0.into()), 3);
    assert_eq!(t.output(), "");

    assert_eq!(ok(&mut t, 1.5.into()), 1);
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float 1.5 to int loses precision in /t.php on line 3\n"
    );
    assert_eq!(ok(&mut t, s("1.5")), 1);
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float-string \"1.5\" to int loses precision in /t.php on line 3\n"
    );
    assert_eq!(ok(&mut t, Value::Null), 0);
    assert_eq!(
        t.output(),
        "\nDeprecated: t_int(): Passing null to parameter #1 ($i) of type int is deprecated in /t.php on line 3\n"
    );

    for (v, given) in [
        (s("abc"), "string"),
        (s("5 apples"), "string"),
        (s(""), "string"),
        (Value::empty_array(), "array"),
        (1e20.into(), "float"),
        (f64::NAN.into(), "float"),
        (f64::INFINITY.into(), "float"),
        (s("1e100"), "string"),
    ] {
        assert_eq!(
            thrown(t.call("t_int", vec![v])),
            (
                "TypeError".into(),
                format!("t_int(): Argument #1 ($i) must be of type int, {given} given")
            )
        );
    }
}

#[test]
fn weak_float_string_bool_coercion() {
    let mut t = TestVm::new();
    assert!(matches!(t.call("t_float", vec![5.into()]).unwrap(), Value::Float(f) if f == 5.0));
    assert!(matches!(t.call("t_float", vec![s("1.5")]).unwrap(), Value::Float(f) if f == 1.5));
    assert!(matches!(t.call("t_float", vec![s("1e3")]).unwrap(), Value::Float(f) if f == 1000.0));
    assert!(matches!(t.call("t_float", vec![true.into()]).unwrap(), Value::Float(f) if f == 1.0));
    assert_eq!(
        thrown(t.call("t_float", vec![s("x")])).1,
        "t_float(): Argument #1 ($f) must be of type float, string given"
    );

    assert_eq!(
        t.call("t_str", vec![5.into()]).unwrap().as_str().unwrap(),
        "5"
    );
    assert_eq!(
        t.call("t_str", vec![1.5.into()]).unwrap().as_str().unwrap(),
        "1.5"
    );
    assert_eq!(
        t.call("t_str", vec![0.1f64.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        "0.1"
    );
    assert_eq!(
        t.call("t_str", vec![true.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        "1"
    );
    assert_eq!(
        t.call("t_str", vec![false.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert_eq!(
        t.call("t_str", vec![Value::Null])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: t_str(): Passing null to parameter #1 ($s) of type string is deprecated in /t.php on line 3\n"
    );
    assert_eq!(
        thrown(t.call("t_str", vec![Value::empty_array()])).1,
        "t_str(): Argument #1 ($s) must be of type string, array given"
    );
    let std = t.vm.cast_to_object(&Value::Null).unwrap();
    assert_eq!(
        thrown(t.call("t_str", vec![Value::Object(std)])).1,
        "t_str(): Argument #1 ($s) must be of type string, stdClass given"
    );

    assert!(matches!(
        t.call("t_bool", vec![s("0")]).unwrap(),
        Value::Bool(false)
    ));
    assert!(matches!(
        t.call("t_bool", vec![s("")]).unwrap(),
        Value::Bool(false)
    ));
    assert!(matches!(
        t.call("t_bool", vec![s("a")]).unwrap(),
        Value::Bool(true)
    ));
    assert!(matches!(
        t.call("t_bool", vec![0.0.into()]).unwrap(),
        Value::Bool(false)
    ));
    assert!(matches!(
        t.call("t_bool", vec![2.into()]).unwrap(),
        Value::Bool(true)
    ));
    assert_eq!(
        thrown(t.call("t_bool", vec![Value::empty_array()])).1,
        "t_bool(): Argument #1 ($b) must be of type bool, array given"
    );
    let std2 = t.vm.cast_to_object(&Value::Null).unwrap();
    assert_eq!(
        thrown(t.call("t_bool", vec![Value::Object(std2)])).1,
        "t_bool(): Argument #1 ($b) must be of type bool, stdClass given"
    );
}

#[test]
fn strict_mode() {
    let mut t = TestVm::new();
    assert_eq!(
        thrown(call_strict(&mut t, "t_int", vec![s("5")])).1,
        "t_int(): Argument #1 ($i) must be of type int, string given"
    );
    assert_eq!(
        thrown(call_strict(&mut t, "t_int", vec![5.0.into()])).1,
        "t_int(): Argument #1 ($i) must be of type int, float given"
    );
    assert_eq!(
        thrown(call_strict(&mut t, "t_str", vec![Value::Null])).1,
        "t_str(): Argument #1 ($s) must be of type string, null given"
    );
    assert_eq!(
        thrown(call_strict(&mut t, "t_bool", vec![1.into()])).1,
        "t_bool(): Argument #1 ($b) must be of type bool, int given"
    );
    assert!(
        matches!(call_strict(&mut t, "t_float", vec![5.into()]).unwrap(), Value::Float(f) if f == 5.0)
    );
    assert_eq!(t.output(), "");
}

#[test]
fn union_types_follow_preference_order() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("t_int_str", vec![5.into()]).unwrap(),
        Value::Int(5)
    ));
    assert_eq!(
        t.call("t_int_str", vec![s("5")]).unwrap().as_str().unwrap(),
        "5"
    );
    assert!(matches!(
        t.call("t_int_str", vec![42.0.into()]).unwrap(),
        Value::Int(42)
    ));
    assert!(matches!(
        t.call("t_int_str", vec![true.into()]).unwrap(),
        Value::Int(1)
    ));
    assert_eq!(
        t.call("t_int_str", vec![f64::INFINITY.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        "INF"
    );
    assert!(matches!(
        t.call("t_int_str", vec![1.5.into()]).unwrap(),
        Value::Int(1)
    ));
    assert!(
        t.output()
            .contains("Implicit conversion from float 1.5 to int loses precision")
    );
    assert_eq!(
        thrown(t.call("t_int_str", vec![Value::empty_array()])).1,
        "t_int_str(): Argument #1 ($v) must be of type string|int, array given"
    );
    assert!(matches!(
        t.call("t_int_str", vec![Value::Null]).unwrap(),
        Value::Int(0)
    ));
    assert!(
        t.output()
            .contains("Passing null to parameter #1 ($v) of type string|int is deprecated")
    );

    assert!(matches!(
        t.call("t_int_float", vec![s("42")]).unwrap(),
        Value::Int(42)
    ));
    assert!(
        matches!(t.call("t_int_float", vec![s("42.0")]).unwrap(), Value::Float(f) if f == 42.0)
    );
    assert_eq!(
        thrown(t.call("t_int_float", vec![s("42x")])).1,
        "t_int_float(): Argument #1 ($v) must be of type int|float, string given"
    );

    assert_eq!(
        t.call("t_arr_str", vec![42.into()])
            .unwrap()
            .as_str()
            .unwrap(),
        "42"
    );
    assert_eq!(
        t.call("t_arr_str", vec![Value::Null])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert!(t.output().contains("of type array|string is deprecated"));
}

#[test]
fn nullable_class_array_iterable_object_mixed() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("t_nullable", vec![Value::Null]).unwrap(),
        Value::Null
    ));
    assert_eq!(t.output(), "");
    assert_eq!(
        thrown(t.call("t_nullable", vec![s("x")])).1,
        "t_nullable(): Argument #1 ($v) must be of type ?int, string given"
    );
    assert_eq!(
        thrown(t.call("t_arr", vec![s("x")])).1,
        "t_arr(): Argument #1 ($a) must be of type array, string given"
    );
    assert_eq!(
        thrown(t.call("t_arr", vec![Value::Null])).1,
        "t_arr(): Argument #1 ($a) must be of type array, null given"
    );
    assert_eq!(
        thrown(t.call("t_iter", vec![1.into()])).1,
        "t_iter(): Argument #1 ($it) must be of type Traversable|array, int given"
    );
    assert_eq!(
        thrown(t.call("t_obj", vec![true.into()])).1,
        "t_obj(): Argument #1 ($o) must be of type object, true given"
    );
    let ex = t.vm.create_exception("Exception", "m", 0);
    assert!(t.call("t_throwable", vec![Value::Object(ex)]).is_ok());
    let std = t.vm.cast_to_object(&Value::Null).unwrap();
    assert_eq!(
        thrown(t.call("t_throwable", vec![Value::Object(std)])).1,
        "t_throwable(): Argument #1 ($t) must be of type ?Throwable, stdClass given"
    );
    assert!(matches!(
        t.call("t_mixed", vec![Value::Null]).unwrap(),
        Value::Null
    ));
}

#[test]
fn callable_params() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("t_cb", vec![s("strlen"), s("abcd")]).unwrap(),
        Value::Int(4)
    ));
    assert_eq!(
        thrown(t.call("t_cb", vec![s("nope")])).1,
        "t_cb(): Argument #1 ($cb) must be a valid callback, function \"nope\" not found or invalid function name"
    );
    assert_eq!(
        thrown(t.call("t_ncb", vec![1.into()])).1,
        "t_ncb(): Argument #1 ($cb) must be a valid callback or null, no array or string given"
    );
    assert!(matches!(
        t.call("t_ncb", vec![Value::Null]).unwrap(),
        Value::Null
    ));
}

#[test]
fn variadics() {
    let mut t = TestVm::new();
    let r = t.call("t_var", vec![1.into(), s("a"), 5.into()]).unwrap();
    let a = r.as_array().unwrap();
    assert_eq!(a.len(), 3);
    assert_eq!(a.get_int(2).unwrap().as_str().unwrap(), "5");
    assert_eq!(
        thrown(t.call("t_var", vec![1.into(), s("a"), Value::empty_array()])).1,
        "t_var(): Argument #3 must be of type string, array given"
    );
    assert_eq!(
        thrown(call_named(
            &mut t,
            "t_var",
            vec![1.into()],
            vec![("extra", s("x"))]
        )),
        (
            "ArgumentCountError".into(),
            "Internal function t_var() does not accept named variadic arguments".into()
        )
    );
}

#[test]
fn named_arguments() {
    let mut t = TestVm::new();
    let r = call_named(
        &mut t,
        "str_pad",
        vec![],
        vec![("string", s("x")), ("length", 3.into())],
    )
    .unwrap();
    assert_eq!(r.as_str().unwrap(), "x  ");
    let r = call_named(
        &mut t,
        "str_pad",
        vec![s("x"), 3.into()],
        vec![("pad_type", 0.into())],
    )
    .unwrap();
    assert_eq!(r.as_str().unwrap(), "  x");
    assert_eq!(
        thrown(call_named(
            &mut t,
            "str_pad",
            vec![s("x"), 3.into()],
            vec![("foo", 1.into())]
        )),
        ("Error".into(), "Unknown named parameter $foo".into())
    );
    assert_eq!(
        thrown(call_named(
            &mut t,
            "str_pad",
            vec![s("x"), 3.into()],
            vec![("string", s("y"))]
        )),
        (
            "Error".into(),
            "Named parameter $string overwrites previous argument".into()
        )
    );
    assert_eq!(
        thrown(call_named(
            &mut t,
            "str_pad",
            vec![],
            vec![("length", 3.into())]
        )),
        (
            "ArgumentCountError".into(),
            "str_pad(): Argument #1 ($string) not passed".into()
        )
    );
    assert_eq!(
        thrown(call_named(&mut t, "t_unknown", vec![Value::empty_array()], vec![("strict", true.into())])),
        (
            "ArgumentCountError".into(),
            "t_unknown(): Argument #2 ($filter_value) must be passed explicitly, because the default value is not known"
                .into()
        )
    );
    let r = t.call("t_unknown", vec![Value::empty_array()]).unwrap();
    assert_eq!(r.as_array().unwrap().len(), 3);
}

#[test]
fn by_reference_params() {
    let mut t = TestVm::new();
    let arr = crate::value::PhpRef::new(Value::array(PhpArray::from_values(vec![
        3.into(),
        1.into(),
        2.into(),
    ])));
    t.call("sort_like", vec![Value::Ref(arr.clone())]).unwrap();
    let sorted: Vec<i64> = arr
        .get()
        .as_array()
        .unwrap()
        .values()
        .map(|v| v.as_int().unwrap())
        .collect();
    assert_eq!(sorted, [1, 2, 3]);
    let not_array = crate::value::PhpRef::new(s("x"));
    assert_eq!(
        thrown(t.call("sort_like", vec![Value::Ref(not_array)])).1,
        "sort_like(): Argument #1 ($array) must be of type array, string given"
    );
}

#[test]
fn value_errors_and_exception_traces() {
    let mut t = TestVm::new();
    assert_eq!(
        thrown(t.call("str_repeat", vec![s("x"), (-1).into()])),
        (
            "ValueError".into(),
            "str_repeat(): Argument #2 ($times) must be greater than or equal to 0".into()
        )
    );
    match t.call("intdiv", vec![1.into(), 0.into()]) {
        Err(Flow::Exception(e)) => {
            assert_eq!(e.class().name, "DivisionByZeroError");
            assert_eq!(Vm::exception_message(&e), "Division by zero");
            assert_eq!(e.get_prop(b"file").unwrap().as_str().unwrap(), "/t.php");
            assert_eq!(e.get_prop(b"line").unwrap().as_int(), Some(3));
            let trace = crate::builtins::exceptions::trace_as_string(&t.vm, &e);
            assert_eq!(trace, "#0 /t.php(3): intdiv(1, 0)\n#1 {main}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        thrown(t.call("intdiv", vec![i64::MIN.into(), (-1).into()])),
        (
            "ArithmeticError".into(),
            "Division of PHP_INT_MIN by -1 is not an integer".into()
        )
    );
    assert!(t.vm.frames.is_empty());
}

#[test]
fn native_methods_on_exceptions() {
    let mut t = TestVm::new();
    let prev = t.vm.create_exception("LogicException", "inner", 0);
    let ex = t.vm.create_exception("RuntimeException", "outer", 5);
    ex.set_prop(b"previous", Value::Object(prev.clone()));
    assert_eq!(
        t.vm.call_method(&ex, b"getMessage", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "outer"
    );
    assert!(matches!(
        t.vm.call_method(&ex, b"getCode", vec![]).unwrap(),
        Value::Int(5)
    ));
    // Unknown classes fall back to Error.
    assert_eq!(ex.class().name, "Error");
    let text = t.vm.call_method(&ex, b"__toString", vec![]).unwrap();
    assert_eq!(
        text.as_str().unwrap(),
        "Error: inner in /t.php:3\nStack trace:\n#0 {main}\n\nNext Error: outer in /t.php:3\nStack trace:\n#0 {main}"
    );
    let e = t.vm.create_exception("Exception", "", 0);
    let text = t.vm.call_method(&e, b"__toString", vec![]).unwrap();
    assert_eq!(
        text.as_str().unwrap(),
        "Exception in /t.php:3\nStack trace:\n#0 {main}"
    );

    // Constructing through the native constructor.
    let ce = t.vm.find_class(b"ErrorException").unwrap();
    let obj = t.vm.instantiate(&ce).unwrap();
    t.vm.call_method(
        &obj,
        b"__construct",
        vec![s("m"), 1.into(), 2.into(), s("f.php"), 9.into()],
    )
    .unwrap();
    assert!(matches!(
        t.vm.call_method(&obj, b"getSeverity", vec![]).unwrap(),
        Value::Int(2)
    ));
    assert_eq!(
        t.vm.call_method(&obj, b"getFile", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "f.php"
    );
    assert!(matches!(
        t.vm.call_method(&obj, b"getLine", vec![]).unwrap(),
        Value::Int(9)
    ));
    let _ = Rc::strong_count(&ce);
}

#[test]
fn internal_classes_and_hierarchy() {
    let t = TestVm::new();
    let dbz = t.vm.find_class(b"divisionbyzeroerror").unwrap();
    assert!(dbz.instance_of_name(b"ArithmeticError"));
    assert!(dbz.instance_of_name(b"Error"));
    assert!(dbz.instance_of_name(b"Throwable"));
    assert!(dbz.instance_of_name(b"Stringable"));
    assert!(!dbz.instance_of_name(b"Exception"));
    let ace = t.vm.find_class(b"ArgumentCountError").unwrap();
    assert!(ace.instance_of_name(b"TypeError"));
    let ee = t.vm.find_class(b"ErrorException").unwrap();
    assert!(ee.instance_of_name(b"Exception"));
    assert!(ee.find_property(b"severity").is_some());
    assert_eq!(
        t.vm.constant(b"PHP_VERSION").unwrap().as_str().unwrap(),
        crate::vm::version::VERSION
    );
    assert!(matches!(t.vm.constant(b"E_ALL"), Some(Value::Int(30719))));
    assert!(matches!(
        t.vm.constant(b"PHP_INT_SIZE"),
        Some(Value::Int(8))
    ));
    assert!(t.vm.constant(b"PHP_OS").is_some());
}

#[test]
fn call_ordering_and_traces() {
    let mut t = TestVm::new();
    // Arity is checked before the named-variadic restriction (ZPP order).
    assert_eq!(
        thrown(call_named(&mut t, "t_var", vec![], vec![("extra", s("x"))])),
        (
            "ArgumentCountError".into(),
            "t_var() expects at least 1 argument, 0 given".into()
        )
    );
    // Declared parameters are parsed before the named-variadic restriction.
    assert_eq!(
        thrown(call_named(
            &mut t,
            "t_var",
            vec![Value::empty_array()],
            vec![("extra", s("x"))]
        ))
        .1,
        "t_var(): Argument #1 ($a) must be of type int, array given"
    );
    // Named-argument errors are raised in the caller's frame.
    match call_named(&mut t, "str_repeat", vec![s("x")], vec![("nope", 1.into())]) {
        Err(Flow::Exception(e)) => {
            assert_eq!(
                crate::builtins::exceptions::trace_as_string(&t.vm, &e),
                "#0 {main}"
            );
        }
        other => panic!("{other:?}"),
    }
    // Traces show coerced arguments in slot order, without names.
    match call_named(
        &mut t,
        "str_repeat",
        vec![5.into()],
        vec![("times", (-1).into())],
    ) {
        Err(Flow::Exception(e)) => {
            assert_eq!(
                crate::builtins::exceptions::trace_as_string(&t.vm, &e),
                "#0 /t.php(3): str_repeat('5', -1)\n#1 {main}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn dynamic_calls_warn_for_by_value_references() {
    let mut t = TestVm::new();
    let arr = Value::array(PhpArray::from_values(vec![2.into(), 1.into()]));
    let r = t.vm.call(&s("sort_like"), vec![arr]).unwrap();
    assert!(matches!(r, Value::Bool(true)));
    assert_eq!(
        t.output(),
        "\nWarning: sort_like(): Argument #1 ($array) must be passed by reference, value given in /t.php on line 3\n"
    );
    // Direct calls (compiled code passes references itself) do not warn.
    t.call("sort_like", vec![Value::array(PhpArray::new())])
        .unwrap();
    assert_eq!(t.output(), "");
}
