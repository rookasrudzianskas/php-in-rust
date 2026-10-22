//! count, array_map, array_filter, array_reduce, array_find*, array_any,
//! array_all, array_sum, array_product, min, max, clamp.

use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, member_flags};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpArray, ResRef, Vm};

use super::*;

fn b(x: bool) -> Value {
    Value::Bool(x)
}

fn f(x: f64) -> Value {
    Value::Float(x)
}

fn stdclass(t: &mut TestVm) -> Value {
    Value::Object(t.vm.cast_to_object(&Value::Null).unwrap())
}

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

#[test]
fn count_phpt() {
    let mut t = vm();
    assert_eq!(run(&mut t, "count", vec![php("[]")]), "0");
    assert_eq!(run(&mut t, "count", vec![php("[1, [2, 3], 4]")]), "3");
    assert_eq!(run(&mut t, "sizeof", vec![php("[1, 2]")]), "2");
    // COUNT_RECURSIVE counts the nested elements too.
    assert_eq!(run(&mut t, "count", vec![php("[1, [2, 3], 4]"), i(1)]), "5");
    assert_eq!(
        run(
            &mut t,
            "count",
            vec![php("['a' => [1, [2, [3, 4]]], 'b' => 5]"), i(1)]
        ),
        "8"
    );
    assert_eq!(run(&mut t, "count", vec![php("[]"), i(1)]), "0");
    assert_eq!(
        run_err(&mut t, "count", vec![php("[1]"), i(2)]),
        "ValueError: count(): Argument #2 ($mode) must be either COUNT_NORMAL or COUNT_RECURSIVE"
    );
    assert_eq!(
        run_err(&mut t, "sizeof", vec![php("[1]"), i(-1)]),
        "ValueError: sizeof(): Argument #2 ($mode) must be either COUNT_NORMAL or COUNT_RECURSIVE"
    );
    // count_invalid.phpt
    assert_eq!(
        run_err(&mut t, "count", vec![Value::Null]),
        "TypeError: count(): Argument #1 ($value) must be of type Countable|array, null given"
    );
    assert_eq!(
        run_err(&mut t, "count", vec![s("string")]),
        "TypeError: count(): Argument #1 ($value) must be of type Countable|array, string given"
    );
    assert_eq!(
        run_err(&mut t, "count", vec![i(1)]),
        "TypeError: count(): Argument #1 ($value) must be of type Countable|array, int given"
    );
    let o = stdclass(&mut t);
    assert_eq!(
        run_err(&mut t, "count", vec![o]),
        "TypeError: count(): Argument #1 ($value) must be of type Countable|array, stdClass given"
    );
}

#[test]
fn count_detects_recursion() {
    let mut t = vm();
    // $a = [1]; $a[] = &$a;
    let r = PhpRef::new(Value::Null);
    let arr = Rc::new(PhpArray::from_values(vec![i(1), Value::Ref(r.clone())]));
    r.set(Value::Array(arr.clone()));
    t.output();
    assert_eq!(
        run(&mut t, "count", vec![Value::Array(arr.clone()), i(1)]),
        "2"
    );
    assert_eq!(
        t.output(),
        "\nWarning: count(): Recursion detected in /t.php on line 3\n"
    );
    assert_eq!(run(&mut t, "count", vec![Value::Array(arr)]), "2");
}

// ---------------------------------------------------------------------------
// array_map
// ---------------------------------------------------------------------------

#[test]
fn array_map_phpt() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_map", vec![s("t_double"), php("[1, 2, 3]")]),
        "[2, 4, 6]"
    );
    // Keys (including string keys and gaps) are preserved with one array.
    assert_eq!(
        run(
            &mut t,
            "array_map",
            vec![s("t_double"), php("['a' => 1, 5 => 2, 'b' => 3]")]
        ),
        "['a' => 2, 5 => 4, 'b' => 6]"
    );
    // No callback and one array: the array is returned as is.
    assert_eq!(
        run(
            &mut t,
            "array_map",
            vec![Value::Null, php("['a' => 1, 5 => 2]")]
        ),
        "['a' => 1, 5 => 2]"
    );
    assert_eq!(
        run(&mut t, "array_map", vec![s("t_double"), php("[]")]),
        "[]"
    );
    // Several arrays: the callback gets one element of each; keys are lost.
    assert_eq!(
        run(
            &mut t,
            "array_map",
            vec![s("t_add"), php("['a' => 1, 'b' => 2]"), php("[10, 20]")]
        ),
        "[11, 22]"
    );
    // Shorter arrays are padded with null.
    assert_eq!(
        run(
            &mut t,
            "array_map",
            vec![s("t_concat"), php("['a', 'b', 'c']"), php("['x']")]
        ),
        "['ax', 'b', 'c']"
    );
    // Null callback with several arrays zips them.
    assert_eq!(
        run(
            &mut t,
            "array_map",
            vec![Value::Null, php("[1, 2, 3]"), php("['a', 'b']")]
        ),
        "[[1, 'a'], [2, 'b'], [3, null]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_map",
            vec![Value::Null, php("['x' => 1]"), php("[5 => 'a']")]
        ),
        "[[1, 'a']]"
    );
    assert_eq!(
        run(&mut t, "array_map", vec![s("t_add"), php("[]"), php("[]")]),
        "[]"
    );
}

#[test]
fn array_map_errors() {
    let mut t = vm();
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("t_double"), s("x")]),
        "TypeError: array_map(): Argument #2 ($array) must be of type array, string given"
    );
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("t_add"), php("[1]"), s("x")]),
        "TypeError: array_map(): Argument #3 must be of type array, string given"
    );
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("nope"), php("[1]")]),
        "TypeError: array_map(): Argument #1 ($callback) must be a valid callback or null, function \"nope\" not found or invalid function name"
    );
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("t_throw"), php("[1, 2]")]),
        "Exception: boom"
    );
}

// ---------------------------------------------------------------------------
// array_filter / array_reduce / array_find*
// ---------------------------------------------------------------------------

#[test]
fn array_filter_phpt() {
    let mut t = vm();
    // Without a callback falsy values are removed and keys preserved.
    assert_eq!(
        run(
            &mut t,
            "array_filter",
            vec![php("[1, 0, 2, null, '', '0', 'a', [], [0], false, 0.0]")]
        ),
        "[0 => 1, 2 => 2, 6 => 'a', 8 => [0]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_filter",
            vec![php("['a' => 1, 'b' => 2, 'c' => 3, 'd' => 4]"), s("t_even")]
        ),
        "['b' => 2, 'd' => 4]"
    );
    // ARRAY_FILTER_USE_KEY (2): the callback gets the key.
    assert_eq!(
        run(
            &mut t,
            "array_filter",
            vec![php("[10, 20, 30, 40]"), s("t_even"), i(2)]
        ),
        "[0 => 10, 2 => 30]"
    );
    // ARRAY_FILTER_USE_BOTH (1): value and key.
    assert_eq!(
        run(
            &mut t,
            "array_filter",
            vec![php("['a' => 1, 'b' => 2, 3 => 3]"), s("t_key_big"), i(1)]
        ),
        "['a' => 1, 'b' => 2, 3 => 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_filter",
            vec![php("[1, 2, 3]"), s("t_key_big"), i(1)]
        ),
        "[2 => 3]"
    );
    assert_eq!(run(&mut t, "array_filter", vec![php("[]")]), "[]");
    assert_eq!(
        run_err(&mut t, "array_filter", vec![php("[1]"), Value::Null, i(7)]),
        "ValueError: array_filter(): Argument #3 ($mode) must be one of ARRAY_FILTER_USE_VALUE, ARRAY_FILTER_USE_KEY, or ARRAY_FILTER_USE_BOTH"
    );
}

#[test]
fn array_reduce_phpt() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_reduce",
            vec![php("[1, 2, 3, 4]"), s("t_add")]
        ),
        "10"
    );
    assert_eq!(
        run(
            &mut t,
            "array_reduce",
            vec![php("[1, 2, 3, 4]"), s("t_add"), i(10)]
        ),
        "20"
    );
    assert_eq!(
        run(&mut t, "array_reduce", vec![php("[]"), s("t_add"), i(10)]),
        "10"
    );
    assert_eq!(
        run(&mut t, "array_reduce", vec![php("[]"), s("t_add")]),
        "null"
    );
    assert_eq!(
        run(
            &mut t,
            "array_reduce",
            vec![php("['a', 'b', 'c']"), s("t_concat"), s("")]
        ),
        "'abc'"
    );
    assert_eq!(
        run_err(&mut t, "array_reduce", vec![php("[1]"), s("t_throw")]),
        "Exception: boom"
    );
}

#[test]
fn array_find_family_phpt() {
    let mut t = vm();
    let a = php("['a' => 1, 'b' => 2, 'c' => 4]");
    assert_eq!(run(&mut t, "array_find", vec![a.clone(), s("t_even")]), "2");
    assert_eq!(
        run(&mut t, "array_find_key", vec![a.clone(), s("t_even")]),
        "'b'"
    );
    assert_eq!(
        run(&mut t, "array_any", vec![a.clone(), s("t_even")]),
        "true"
    );
    assert_eq!(
        run(&mut t, "array_all", vec![a.clone(), s("t_even")]),
        "false"
    );
    let odd = php("[1, 3, 5]");
    assert_eq!(
        run(&mut t, "array_find", vec![odd.clone(), s("t_even")]),
        "null"
    );
    assert_eq!(
        run(&mut t, "array_find_key", vec![odd.clone(), s("t_even")]),
        "null"
    );
    assert_eq!(
        run(&mut t, "array_any", vec![odd.clone(), s("t_even")]),
        "false"
    );
    assert_eq!(
        run(&mut t, "array_all", vec![php("[2, 4]"), s("t_even")]),
        "true"
    );
    // Empty arrays: nothing found, any() false, all() true.
    assert_eq!(
        run(&mut t, "array_find", vec![php("[]"), s("t_even")]),
        "null"
    );
    assert_eq!(
        run(&mut t, "array_any", vec![php("[]"), s("t_even")]),
        "false"
    );
    assert_eq!(
        run(&mut t, "array_all", vec![php("[]"), s("t_even")]),
        "true"
    );
    // The callback receives (value, key); the first match stops iteration.
    assert_eq!(
        run(&mut t, "array_find", vec![php("[5, 6, 8]"), s("t_log")]),
        "5"
    );
    assert_eq!(take_log(), ["5,0"]);
    assert_eq!(
        run_err(&mut t, "array_find", vec![php("[5]"), s("t_throw")]),
        "Exception: boom"
    );
}

// ---------------------------------------------------------------------------
// array_sum / array_product
// ---------------------------------------------------------------------------

#[test]
fn array_sum_phpt() {
    let mut t = vm();
    assert_eq!(run(&mut t, "array_sum", vec![php("[1, 2, 3, 4, 5]")]), "15");
    assert_eq!(run(&mut t, "array_sum", vec![php("[]")]), "0");
    let r = t
        .call("array_sum", vec![php("[1.0, 2.2, 3.4, 4.6]")])
        .unwrap();
    assert!(matches!(r, Value::Float(x) if (x - 11.2).abs() < 1e-9));
    let r = t
        .call("array_sum", vec![php("[1, 2.3, 4, 0.6, 10]")])
        .unwrap();
    assert!(matches!(r, Value::Float(x) if (x - 17.9).abs() < 1e-9));
    // Numeric strings, bools and nulls.
    assert_eq!(
        run(
            &mut t,
            "array_sum",
            vec![php("['1', '2', true, false, null]")]
        ),
        "4"
    );
    // Integer overflow switches to float.
    assert_eq!(
        run(&mut t, "array_sum", vec![php("[PHP_INT_MAX, 1]")]),
        show_float_for_test(i64::MAX as f64 + 1.0)
    );
    // References are followed (array_sum_variation5.phpt).
    let r1 = PhpRef::new(i(-5));
    let r2 = PhpRef::new(i(100));
    let arr = PhpArray::from_values(vec![i(10), Value::Ref(r1), Value::Ref(r2), i(200)]);
    assert_eq!(run(&mut t, "array_sum", vec![Value::array(arr)]), "305");
}

#[test]
fn array_sum_unsupported_types() {
    // array_sum_variation7.phpt: unsupported operands warn and are skipped.
    let mut t = vm();
    t.output();
    assert_eq!(
        run(&mut t, "array_sum", vec![php("['Apple', 'Banana']")]),
        "0"
    );
    assert_eq!(
        t.output(),
        "\nWarning: array_sum(): Addition is not supported on type string in /t.php on line 3\n\nWarning: array_sum(): Addition is not supported on type string in /t.php on line 3\n"
    );
    assert_eq!(
        run(&mut t, "array_sum", vec![php("[[1, 2], [], [0]]")]),
        "0"
    );
    assert_eq!(
        t.output()
            .matches("Warning: array_sum(): Addition is not supported on type array")
            .count(),
        3
    );
    // Mixed values.
    let r = t
        .call(
            "array_sum",
            vec![php("[5, -8, 7.2, -1.2, '10', 'apple', 'Mango', true, false, null, null, [[1, 2], [0], []]]")],
        )
        .unwrap();
    assert!(
        matches!(r, Value::Float(x) if (x - 14.0).abs() < 1e-9),
        "{r:?}"
    );
    assert_eq!(
        t.output()
            .matches("Warning: array_sum(): Addition is not supported on type")
            .count(),
        3
    );
    // Objects without a numeric cast.
    let o = stdclass(&mut t);
    let arr = Value::array(PhpArray::from_values(vec![o]));
    assert_eq!(run(&mut t, "array_sum", vec![arr]), "0");
    assert!(
        t.output()
            .contains("Warning: array_sum(): Addition is not supported on type stdClass")
    );
    // Resources are added as their id (array_sum_variation8.phpt).
    let res = ResRef::new("stream", Box::new(()));
    let id = res.id();
    let arr = Value::array(PhpArray::from_values(vec![i(10), Value::Resource(res)]));
    assert_eq!(run(&mut t, "array_sum", vec![arr]), (10 + id).to_string());
    assert!(
        t.output()
            .contains("Warning: array_sum(): Addition is not supported on type resource")
    );
}

#[test]
fn array_product_phpt() {
    let mut t = vm();
    assert_eq!(run(&mut t, "array_product", vec![php("[2, 3, 4]")]), "24");
    assert_eq!(run(&mut t, "array_product", vec![php("[]")]), "1");
    assert_eq!(run(&mut t, "array_product", vec![php("['2', 3.0]")]), "6.0");
    // array_product_variation4.phpt
    let big = Value::array(PhpArray::from_values(vec![i(999); 999]));
    assert_eq!(run(&mut t, "array_product", vec![big]), "INF");
    t.output();
    assert_eq!(run(&mut t, "array_product", vec![php("[10, 'x']")]), "0");
    assert_eq!(
        t.output(),
        "\nWarning: array_product(): Multiplication is not supported on type string in /t.php on line 3\n"
    );
}

// ---------------------------------------------------------------------------
// min / max / clamp
// ---------------------------------------------------------------------------

#[test]
fn min_phpt() {
    let mut t = vm();
    // min.phpt
    assert_eq!(
        run_err(&mut t, "min", vec![i(1)]),
        "TypeError: min(): Argument #1 ($value) must be of type array, int given"
    );
    assert_eq!(
        run_err(&mut t, "min", vec![php("[]")]),
        "ValueError: min(): Argument #1 ($value) must contain at least one element"
    );
    let o = stdclass(&mut t);
    assert_eq!(
        run_err(&mut t, "min", vec![o]),
        "TypeError: min(): Argument #1 ($value) must be of type array, stdClass given"
    );
    assert_eq!(run(&mut t, "min", vec![i(2), i(1), i(2)]), "1");
    assert_eq!(run(&mut t, "min", vec![f(2.1), f(2.11), f(2.09)]), "2.09");
    assert_eq!(run(&mut t, "min", vec![s(""), s("t"), s("b")]), "''");
    assert_eq!(
        run(&mut t, "min", vec![b(false), b(true), b(false)]),
        "false"
    );
    assert_eq!(
        run(&mut t, "min", vec![b(true), b(false), b(true)]),
        "false"
    );
    assert_eq!(
        run(&mut t, "min", vec![i(1), b(true), b(false), b(true)]),
        "false"
    );
    assert_eq!(
        run(&mut t, "min", vec![i(0), b(true), b(false), b(true)]),
        "0"
    );
    // Arrays of values (min_variation2.phpt).
    assert_eq!(run(&mut t, "min", vec![php("[2, 1, 2]")]), "1");
    assert_eq!(run(&mut t, "min", vec![php("[-2, 1, 2]")]), "-2");
    assert_eq!(run(&mut t, "min", vec![php("[2.1, 2.11, 2.09]")]), "2.09");
    assert_eq!(run(&mut t, "min", vec![php("['', 't', 'b']")]), "''");
    assert_eq!(run(&mut t, "min", vec![php("[0, 1, [2, 3]]")]), "0");
    assert_eq!(
        run(&mut t, "min", vec![php("[-2147483649, -2147483647]")]),
        "-2147483649"
    );
}

#[test]
fn min_max_int_float_optimisation_phpt() {
    // min_int_float_optimisation.phpt / max_int_float_optimisation.phpt
    let mut t = vm();
    assert_eq!(run(&mut t, "min", vec![i(10), i(5), i(3), i(2)]), "2");
    assert_eq!(run(&mut t, "min", vec![i(10), i(5), f(3.5), i(2)]), "2");
    assert_eq!(run(&mut t, "min", vec![i(2), f(3.5), i(5), i(10)]), "2");
    assert_eq!(run(&mut t, "min", vec![i(10), i(5), s("3"), i(2)]), "2");
    assert_eq!(run(&mut t, "min", vec![i(2), i(3), s("1"), i(10)]), "'1'");
    assert_eq!(
        run(&mut t, "min", vec![f(10.5), f(5.5), i(3), f(2.5)]),
        "2.5"
    );
    assert_eq!(
        run(&mut t, "min", vec![f(2.5), s("3.5"), f(5.5), f(10.5)]),
        "2.5"
    );
    assert_eq!(
        run(&mut t, "min", vec![f(2.5), f(3.5), s("1.5"), f(10.5)]),
        "'1.5'"
    );
    // Ints that are not exactly representable as floats go the generic way.
    assert_eq!(
        run(
            &mut t,
            "min",
            vec![i(i64::MAX - 1), i(i64::MAX), f(i64::MAX as f64 * 2.0)]
        ),
        (i64::MAX - 1).to_string()
    );
    assert_eq!(
        run(
            &mut t,
            "min",
            vec![i(i64::MIN + 1), i(i64::MIN), f(i64::MIN as f64 * 2.0)]
        ),
        "-1.8446744073709552e19"
    );

    assert_eq!(run(&mut t, "max", vec![i(10), i(5), i(3), i(2)]), "10");
    assert_eq!(run(&mut t, "max", vec![i(2), i(3), s("15"), i(10)]), "'15'");
    assert_eq!(
        run(&mut t, "max", vec![f(10.5), f(5.5), f(3.5), f(2.5)]),
        "10.5"
    );
    assert_eq!(
        run(&mut t, "max", vec![f(2.5), f(3.5), s("15.5"), f(10.5)]),
        "'15.5'"
    );
    assert_eq!(
        run(
            &mut t,
            "max",
            vec![i(i64::MIN + 1), i(i64::MIN), f(i64::MIN as f64 * 2.0)]
        ),
        (i64::MIN + 1).to_string()
    );
    assert_eq!(
        run(
            &mut t,
            "max",
            vec![i(i64::MAX - 1), i(i64::MAX), f(f64::INFINITY)]
        ),
        "INF"
    );
}

#[test]
fn min_max_two_arguments() {
    // Direct two-argument calls pick the right-hand value on ties for min.
    let mut t = vm();
    assert_eq!(run(&mut t, "min", vec![i(1), f(1.0)]), "1.0");
    assert_eq!(run(&mut t, "max", vec![i(1), f(1.0)]), "1");
    assert_eq!(run(&mut t, "min", vec![i(2), i(1)]), "1");
    assert_eq!(run(&mut t, "max", vec![i(2), i(1)]), "2");
    assert_eq!(run(&mut t, "max", vec![f(f64::NAN), i(1)]), "1");
    assert_eq!(run(&mut t, "min", vec![s("a"), s("b")]), "'a'");
    assert_eq!(run(&mut t, "max", vec![s("a"), s("b")]), "'b'");
    assert_eq!(
        run(&mut t, "max", vec![php("[1, 2]"), php("[1, 3]")]),
        "[1, 3]"
    );
}

#[test]
fn max_phpt() {
    let mut t = vm();
    assert_eq!(
        run_err(&mut t, "max", vec![php("[]")]),
        "ValueError: max(): Argument #1 ($value) must contain at least one element"
    );
    assert_eq!(run(&mut t, "max", vec![php("[2, 1, 2]")]), "2");
    assert_eq!(run(&mut t, "max", vec![php("['a', 'c', 'b']")]), "'c'");
    assert_eq!(run(&mut t, "max", vec![php("[1, [2, 3], 4]")]), "[2, 3]");
    assert_eq!(run(&mut t, "max", vec![i(1), s("a"), i(2)]), "'a'");
    assert_eq!(
        run(&mut t, "max", vec![b(false), b(true), b(false)]),
        "true"
    );
}

#[test]
fn clamp_phpt() {
    let mut t = vm();
    assert_eq!(run(&mut t, "clamp", vec![i(5), i(1), i(10)]), "5");
    assert_eq!(run(&mut t, "clamp", vec![i(-5), i(1), i(10)]), "1");
    assert_eq!(run(&mut t, "clamp", vec![i(50), i(1), i(10)]), "10");
    assert_eq!(run(&mut t, "clamp", vec![f(5.5), i(1), i(10)]), "5.5");
    assert_eq!(
        run_err(&mut t, "clamp", vec![i(5), i(10), i(1)]),
        "ValueError: clamp(): Argument #2 ($min) must be smaller than or equal to argument #3 ($max)"
    );
    assert_eq!(
        run_err(&mut t, "clamp", vec![i(5), f(f64::NAN), i(1)]),
        "ValueError: clamp(): Argument #2 ($min) must not be NAN"
    );
    assert_eq!(
        run_err(&mut t, "clamp", vec![i(5), i(1), f(f64::NAN)]),
        "ValueError: clamp(): Argument #3 ($max) must not be NAN"
    );
}

fn show_float_for_test(x: f64) -> String {
    show(&Value::Float(x))
}

// ---------------------------------------------------------------------------
// count() on objects
// ---------------------------------------------------------------------------

fn count_method(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::from("7.9"))
}

static COUNT_METHOD: NativeFunction = NativeFunction {
    sig: "count(): int",
    handler: count_method,
};

fn handler_count(_vm: &mut Vm, _o: &ObjRef) -> PResult<Option<i64>> {
    Ok(Some(42))
}

#[test]
fn count_objects() {
    let mut t = vm();
    let countable = t.vm.find_class(b"Countable").unwrap();

    // An object implementing Countable: its count() method is called and the
    // result converted to int (sizeof_object1.phpt).
    let mut ce = ClassEntry::new("MyCountable", 0);
    ce.interfaces.push(countable.clone());
    ce.add_method(Function::native_method(
        &COUNT_METHOD,
        "MyCountable",
        member_flags::PUBLIC,
    ));
    ce.link_magic_methods();
    let o = Value::Object(ObjRef::new(std::rc::Rc::new(ce)));
    assert_eq!(run(&mut t, "count", vec![o.clone()]), "7");
    assert_eq!(run(&mut t, "sizeof", vec![o]), "7");

    // A class with a `count_elements` handler.
    let mut ce = ClassEntry::new("HandlerCountable", 0);
    ce.interfaces.push(countable);
    ce.handlers.count_elements = Some(handler_count);
    let o = Value::Object(ObjRef::new(std::rc::Rc::new(ce)));
    assert_eq!(run(&mut t, "count", vec![o]), "42");
}

#[test]
fn array_map_argument_numbers_bug77931() {
    let mut t = vm();
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("t_id"), php("[]"), i(1)]),
        "TypeError: array_map(): Argument #3 must be of type array, int given"
    );
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("t_id"), php("[]"), php("[]"), b(true)]),
        "TypeError: array_map(): Argument #4 must be of type array, true given"
    );
    assert_eq!(
        run_err(&mut t, "array_map", vec![s("t_id"), php("[]"), php("[]"), php("[]"), Value::Null]),
        "TypeError: array_map(): Argument #5 must be of type array, null given"
    );
}

#[test]
fn array_sum_overflows_to_float_bug65304() {
    let mut t = vm();
    let r = t.call("array_sum", vec![php("[PHP_INT_MAX, 1]")]).unwrap();
    assert!(matches!(r, Value::Float(f) if f == 9.223372036854775808e18));
    let r = t.call("array_product", vec![php("[PHP_INT_MAX, 2]")]).unwrap();
    assert!(matches!(r, Value::Float(f) if f == 1.8446744073709552e19));
}
