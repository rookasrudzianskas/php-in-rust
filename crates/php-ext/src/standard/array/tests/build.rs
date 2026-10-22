//! range, array_fill, array_fill_keys, array_pad, array_combine, array_flip,
//! array_chunk, array_slice, array_splice, array_reverse,
//! array_change_key_case, array_push/pop/shift/unshift.

use php_runtime::PhpArray;

use super::*;

fn b(x: bool) -> Value {
    Value::Bool(x)
}

fn f(x: f64) -> Value {
    Value::Float(x)
}

/// The float elements of an array result.
fn floats(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a
            .values()
            .map(|v| match v {
                Value::Float(f) => *f,
                other => panic!("float expected, got {other:?}"),
            })
            .collect(),
        other => panic!("array expected, got {other:?}"),
    }
}

fn assert_floats(v: &Value, want: &[f64]) {
    let got = floats(v);
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{got:?} vs {want:?}");
    }
}

// ---------------------------------------------------------------------------
// range
// ---------------------------------------------------------------------------

#[test]
fn range_int_inputs() {
    // range_inputs_int_basic.phpt
    let mut t = vm();
    assert_eq!(
        run(&mut t, "range", vec![i(1), i(10)]),
        "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]"
    );
    assert_eq!(
        run(&mut t, "range", vec![i(10), i(1)]),
        "[10, 9, 8, 7, 6, 5, 4, 3, 2, 1]"
    );
    assert_eq!(run(&mut t, "range", vec![i(5), i(5)]), "[5]");
    assert_eq!(
        run(&mut t, "range", vec![i(1), i(10), i(3)]),
        "[1, 4, 7, 10]"
    );
    assert_eq!(
        run(&mut t, "range", vec![i(10), i(1), i(3)]),
        "[10, 7, 4, 1]"
    );
    assert_eq!(
        run(&mut t, "range", vec![i(1), i(10), s("3")]),
        "[1, 4, 7, 10]"
    );
    // A negative step is made positive for decreasing ranges.
    assert_eq!(
        run(&mut t, "range", vec![i(10), i(1), i(-3)]),
        "[10, 7, 4, 1]"
    );
    assert_eq!(run(&mut t, "range", vec![i(-2), i(2)]), "[-2, -1, 0, 1, 2]");
    // Float steps give floats.
    let r = t.call("range", vec![i(1), i(2), f(0.1)]).unwrap();
    assert_floats(&r, &[1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9, 2.0]);
    let r = t.call("range", vec![i(2), i(1), f(0.1)]).unwrap();
    assert_floats(&r, &[2.0, 1.9, 1.8, 1.7, 1.6, 1.5, 1.4, 1.3, 1.2, 1.1, 1.0]);
    let r = t.call("range", vec![i(1), i(2), s("0.1")]).unwrap();
    assert_floats(&r, &[1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9, 2.0]);
    // A float step with an integral value stays integer.
    assert_eq!(run(&mut t, "range", vec![i(1), i(5), f(2.0)]), "[1, 3, 5]");
}

#[test]
fn range_float_and_numeric_string_inputs() {
    let mut t = vm();
    let r = t.call("range", vec![f(1.5), f(4.5)]).unwrap();
    assert_eq!(show(&r), "[1.5, 2.5, 3.5, 4.5]");
    let r = t.call("range", vec![f(4.5), f(1.5)]).unwrap();
    assert_eq!(show(&r), "[4.5, 3.5, 2.5, 1.5]");
    let r = t.call("range", vec![f(2.5), f(2.5)]).unwrap();
    assert_eq!(show(&r), "[2.5]");
    // Numeric strings.
    assert_eq!(
        run(&mut t, "range", vec![s("1"), s("3")]),
        "['1', '2', '3']"
    );
}

#[test]
fn range_char_inputs() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "range", vec![s("a"), s("e")]),
        "['a', 'b', 'c', 'd', 'e']"
    );
    assert_eq!(
        run(&mut t, "range", vec![s("e"), s("a")]),
        "['e', 'd', 'c', 'b', 'a']"
    );
    assert_eq!(
        run(&mut t, "range", vec![s("a"), s("i"), i(3)]),
        "['a', 'd', 'g']"
    );
    assert_eq!(
        run(&mut t, "range", vec![s("i"), s("a"), i(3)]),
        "['i', 'f', 'c']"
    );
    assert_eq!(run(&mut t, "range", vec![s("A"), s("A")]), "['A']");
    assert_eq!(
        run(&mut t, "range", vec![s("A"), s("C"), f(1.0)]),
        "['A', 'B', 'C']"
    );
    // Single digit strings are characters when the other side is a string too
    // (range_inputs_string_digits.phpt).
    assert_eq!(
        run(&mut t, "range", vec![s("1"), s("3")]),
        "['1', '2', '3']"
    );
    assert_eq!(
        run(&mut t, "range", vec![s("9"), s("A")]),
        "['9', ':', ';', '<', '=', '>', '?', '@', 'A']"
    );
    assert_eq!(
        run(&mut t, "range", vec![s("A"), s("9")]),
        "['A', '@', '?', '>', '=', '<', ';', ':', '9']"
    );
    // ... but numbers when the other side is a number.
    assert_eq!(run(&mut t, "range", vec![s("1"), i(3)]), "[1, 2, 3]");
}

#[test]
fn range_step_errors_phpt() {
    // range_step_errors.phpt
    let mut t = vm();
    let e = |t: &mut TestVm, a: Vec<Value>| run_err(t, "range", a);
    for args in [
        vec![f(1.0), f(7.0), f(0.0)],
        vec![i(1), i(7), i(0)],
        vec![s("A"), s("H"), i(0)],
        vec![s("A"), s("H"), f(0.0)],
    ] {
        assert_eq!(
            e(&mut t, args),
            "ValueError: range(): Argument #3 ($step) cannot be 0"
        );
    }
    for args in [
        vec![f(1.0), f(7.0), f(f64::INFINITY)],
        vec![i(1), i(7), f(f64::INFINITY)],
        vec![s("A"), s("H"), f(f64::INFINITY)],
    ] {
        assert_eq!(
            e(&mut t, args),
            "ValueError: range(): Argument #3 ($step) must be a finite number, INF provided"
        );
    }
    for args in [
        vec![f(1.0), f(7.0), f(f64::NAN)],
        vec![i(1), i(7), f(f64::NAN)],
        vec![s("A"), s("H"), f(f64::NAN)],
    ] {
        assert_eq!(
            e(&mut t, args),
            "ValueError: range(): Argument #3 ($step) must be a finite number, NAN provided"
        );
    }
    let span = "ValueError: range(): Argument #3 ($step) must be less than the range spanned by argument #1 ($start) and argument #2 ($end)";
    for args in [
        vec![f(1.0), f(7.0), f(6.5)],
        vec![f(7.0), f(1.0), f(6.5)],
        vec![s("a"), s("z"), i(100)],
        vec![s("z"), s("a"), i(100)],
    ] {
        assert_eq!(e(&mut t, args), span);
    }
    let neg =
        "ValueError: range(): Argument #3 ($step) must be greater than 0 for increasing ranges";
    for args in [
        vec![s("a"), s("c"), i(-1)],
        vec![i(1), i(3), i(-1)],
        vec![f(1.5), f(3.5), f(-1.5)],
    ] {
        assert_eq!(e(&mut t, args), neg);
    }
    assert_eq!(
        e(&mut t, vec![i(1), i(2), i(i64::MIN)]),
        "ValueError: range(): Argument #3 ($step) must be greater than -9223372036854775808"
    );
}

#[test]
fn range_string_invalid_phpt() {
    // range_inputs_string_invalid.phpt
    let mut t = vm();
    t.output();
    assert_eq!(run(&mut t, "range", vec![s("AA"), s("BB")]), "['A', 'B']");
    let out = t.output();
    assert!(out.contains("Warning: range(): Argument #1 ($start) must be a single byte, subsequent bytes are ignored in /t.php on line 3"));
    assert!(out.contains(
        "Warning: range(): Argument #2 ($end) must be a single byte, subsequent bytes are ignored"
    ));

    assert_eq!(run(&mut t, "range", vec![s("Z"), s("")]), "[0]");
    let out = t.output();
    assert!(out.contains("Warning: range(): Argument #2 ($end) must not be empty, casted to 0"));
    assert!(out.contains("Warning: range(): Argument #2 ($end) must be a single byte string if argument #1 ($start) is a single byte string, argument #1 ($start) converted to 0"));

    assert_eq!(run(&mut t, "range", vec![s(""), s("Z")]), "[0]");
    let out = t.output();
    assert!(out.contains("Warning: range(): Argument #1 ($start) must not be empty, casted to 0"));
    assert!(out.contains("argument #2 ($end) converted to 0"));

    let r = t.call("range", vec![s("3.5"), s("A")]).unwrap();
    assert_eq!(show(&r), "[3.5, 2.5, 1.5, 0.5]");
    assert!(t.output().contains("Argument #1 ($start) must be a single byte string if argument #2 ($end) is a single byte string, argument #2 ($end) converted to 0"));

    let r = t.call("range", vec![s("?"), s("3.5")]).unwrap();
    assert_eq!(show(&r), "[0.0, 1.0, 2.0, 3.0]");
    assert!(t.output().contains("argument #1 ($start) converted to 0"));

    let r = t.call("range", vec![s("A"), s("H"), f(2.6)]).unwrap();
    assert_eq!(show(&r), "[0.0]");
    assert!(t.output().contains("Warning: range(): Argument #3 ($step) must be of type int when generating an array of characters, inputs converted to 0"));
}

#[test]
fn range_size_limits() {
    // range_bug70239_*.phpt
    let mut t = vm();
    let e = run_err(&mut t, "range", vec![i(0), i(i64::MAX)]);
    assert!(
        e.starts_with("ValueError: The supplied range exceeds the maximum array size by ")
            && e.contains("elements: start=0, end=9223372036854775807, step=1. Calculated size: ")
            && e.ends_with("Maximum size: 1073741824."),
        "{e}"
    );
    let e = run_err(&mut t, "range", vec![i(i64::MIN), i(0)]);
    assert!(
        e.contains("start=-9223372036854775808, end=0, step=1."),
        "{e}"
    );
    let e = run_err(&mut t, "range", vec![i(0), f(100_000_000_000.0), f(0.1)]);
    assert!(
        e.starts_with("ValueError: The supplied range exceeds the maximum array size by ")
            && e.contains(
                "elements: start=0.0, end=100000000000.0, step=0.1. Max size: 1073741824"
            ),
        "{e}"
    );
    // Infinite / NaN bounds.
    assert_eq!(
        run_err(&mut t, "range", vec![f(f64::INFINITY), i(1)]),
        "ValueError: range(): Argument #1 ($start) must be a finite number, INF provided"
    );
    assert_eq!(
        run_err(&mut t, "range", vec![i(1), f(f64::NAN)]),
        "ValueError: range(): Argument #2 ($end) must be a finite number, NAN provided"
    );
}

// ---------------------------------------------------------------------------
// array_fill / array_fill_keys / array_pad / array_combine / array_flip
// ---------------------------------------------------------------------------

#[test]
fn array_fill_phpt() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_fill", vec![i(5), i(3), s("x")]),
        "[5 => 'x', 6 => 'x', 7 => 'x']"
    );
    assert_eq!(
        run(&mut t, "array_fill", vec![i(0), i(2), Value::Null]),
        "[null, null]"
    );
    assert_eq!(
        run(&mut t, "array_fill", vec![i(-3), i(3), i(1)]),
        "[-3 => 1, -2 => 1, -1 => 1]"
    );
    assert_eq!(run(&mut t, "array_fill", vec![i(0), i(0), i(1)]), "[]");
    assert_eq!(
        run(&mut t, "array_fill", vec![i(2), i(3), i(1)]),
        "[2 => 1, 3 => 1, 4 => 1]"
    );
    assert_eq!(
        run_err(&mut t, "array_fill", vec![i(0), i(-1), i(1)]),
        "ValueError: array_fill(): Argument #2 ($count) must be greater than or equal to 0"
    );
    assert_eq!(
        run_err(
            &mut t,
            "array_fill",
            vec![i(0), i(i32::MAX as i64 + 1), i(1)]
        ),
        "ValueError: array_fill(): Argument #2 ($count) is too large"
    );
    assert_eq!(
        run_err(&mut t, "array_fill", vec![i(i64::MAX), i(2), i(1)]),
        "Error: Cannot add element to the array as the next element is already occupied"
    );
}

#[test]
fn array_fill_keys_phpt() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_fill_keys",
            vec![php("['a', 5, 'b', 'a']"), s("v")]
        ),
        "['a' => 'v', 5 => 'v', 'b' => 'v']"
    );
    // Non-int keys are converted to strings (and numeric strings to ints).
    assert_eq!(
        run(
            &mut t,
            "array_fill_keys",
            vec![php("[1.5, true, null, '7']"), i(0)]
        ),
        "['1.5' => 0, 1 => 0, '' => 0, 7 => 0]"
    );
    assert_eq!(run(&mut t, "array_fill_keys", vec![php("[]"), i(0)]), "[]");
}

#[test]
fn array_pad_phpt() {
    let mut t = vm();
    let a = php("[12, 10, 9]");
    assert_eq!(
        run(&mut t, "array_pad", vec![a.clone(), i(5), i(0)]),
        "[12, 10, 9, 0, 0]"
    );
    assert_eq!(
        run(&mut t, "array_pad", vec![a.clone(), i(-7), i(-1)]),
        "[-1, -1, -1, -1, 12, 10, 9]"
    );
    assert_eq!(
        run(&mut t, "array_pad", vec![a.clone(), i(2), i(0)]),
        "[12, 10, 9]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_pad",
            vec![php("['a' => 1, 5 => 2]"), i(4), s("p")]
        ),
        "['a' => 1, 0 => 2, 1 => 'p', 2 => 'p']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_pad",
            vec![php("['a' => 1, 5 => 2]"), i(-4), s("p")]
        ),
        "[0 => 'p', 1 => 'p', 'a' => 1, 2 => 2]"
    );
    assert_eq!(
        run_err(&mut t, "array_pad", vec![a.clone(), i(1 << 31), i(0)]),
        "ValueError: array_pad(): Argument #2 ($length) must not exceed the maximum allowed array size"
    );
}

#[test]
fn array_combine_phpt() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_combine",
            vec![php("['a', 'b', 3]"), php("[1, 2, 3]")]
        ),
        "['a' => 1, 'b' => 2, 3 => 3]"
    );
    assert_eq!(
        run(&mut t, "array_combine", vec![php("[]"), php("[]")]),
        "[]"
    );
    // Later duplicates overwrite.
    assert_eq!(
        run(
            &mut t,
            "array_combine",
            vec![php("['a', 'a']"), php("[1, 2]")]
        ),
        "['a' => 2]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_combine",
            vec![php("[true, null, 1.5]"), php("[1, 2, 3]")]
        ),
        "[1 => 1, '' => 2, '1.5' => 3]"
    );
    assert_eq!(
        run_err(&mut t, "array_combine", vec![php("[1]"), php("[1, 2]")]),
        "ValueError: array_combine(): Argument #1 ($keys) and argument #2 ($values) must have the same number of elements"
    );
}

#[test]
fn array_flip_phpt() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_flip", vec![php("['a', 'b', 'c']")]),
        "['a' => 0, 'b' => 1, 'c' => 2]"
    );
    assert_eq!(
        run(&mut t, "array_flip", vec![php("['x' => 1, 'y' => '1']")]),
        "[1 => 'y']"
    );
    t.output();
    assert_eq!(
        run(
            &mut t,
            "array_flip",
            vec![php("[1.5, 'a', true, null, [1]]")]
        ),
        "['a' => 1]"
    );
    assert_eq!(
        t.output().matches("Warning: array_flip(): Can only flip string and integer values, entry skipped in /t.php on line 3").count(),
        4
    );
}

#[test]
fn array_change_key_case_phpt() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_change_key_case",
            vec![php("['FirSt' => 1, 'SecOnd' => 4, 3 => 5]")]
        ),
        "['first' => 1, 'second' => 4, 3 => 5]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_change_key_case",
            vec![php("['FirSt' => 1, 'SecOnd' => 4]"), i(1)]
        ),
        "['FIRST' => 1, 'SECOND' => 4]"
    );
    // Colliding keys: the last one wins, in the first one's position.
    assert_eq!(
        run(
            &mut t,
            "array_change_key_case",
            vec![php("['a' => 1, 'A' => 2, 'b' => 3]")]
        ),
        "['a' => 2, 'b' => 3]"
    );
    assert_eq!(
        run_err(&mut t, "array_change_key_case", vec![php("[]"), i(2)]),
        "ValueError: array_change_key_case(): Argument #2 ($case) must be either CASE_LOWER or CASE_UPPER"
    );
}

// ---------------------------------------------------------------------------
// array_chunk / array_slice / array_splice / array_reverse
// ---------------------------------------------------------------------------

#[test]
fn array_chunk_phpt() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_chunk", vec![php("[1, 2, 3, 4, 5]"), i(2)]),
        "[[1, 2], [3, 4], [5]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_chunk",
            vec![php("['a' => 1, 'b' => 2, 'c' => 3]"), i(2)]
        ),
        "[[1, 2], [3]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_chunk",
            vec![php("['a' => 1, 'b' => 2, 'c' => 3]"), i(2), b(true)]
        ),
        "[['a' => 1, 'b' => 2], ['c' => 3]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_chunk",
            vec![php("[5 => 1, 6 => 2, 7 => 3]"), i(2), b(true)]
        ),
        "[[5 => 1, 6 => 2], [7 => 3]]"
    );
    assert_eq!(
        run(&mut t, "array_chunk", vec![php("[1, 2]"), i(10)]),
        "[[1, 2]]"
    );
    assert_eq!(run(&mut t, "array_chunk", vec![php("[]"), i(3)]), "[]");
    assert_eq!(
        run_err(&mut t, "array_chunk", vec![php("[1]"), i(0)]),
        "ValueError: array_chunk(): Argument #2 ($length) must be greater than 0"
    );
}

#[test]
fn array_slice_phpt() {
    let mut t = vm();
    let a = php("['a', 'b', 'c', 'd', 'e']");
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(2)]),
        "['c', 'd', 'e']"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(-2)]),
        "['d', 'e']"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(1), i(2)]),
        "['b', 'c']"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(1), i(-1)]),
        "['b', 'c', 'd']"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(1), Value::Null]),
        "['b', 'c', 'd', 'e']"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(1), i(2), b(true)]),
        "[1 => 'b', 2 => 'c']"
    );
    assert_eq!(run(&mut t, "array_slice", vec![a.clone(), i(10)]), "[]");
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(-10), i(2)]),
        "['a', 'b']"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(1), i(0)]),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![a.clone(), i(1), i(i64::MAX)]),
        "['b', 'c', 'd', 'e']"
    );
    // String keys are always preserved, integer keys renumbered.
    let m = php("['x' => 1, 5 => 2, 'y' => 3, 9 => 4]");
    assert_eq!(
        run(&mut t, "array_slice", vec![m.clone(), i(1), i(3)]),
        "[0 => 2, 'y' => 3, 1 => 4]"
    );
    assert_eq!(
        run(&mut t, "array_slice", vec![m, i(1), i(3), b(true)]),
        "[5 => 2, 'y' => 3, 9 => 4]"
    );
}

#[test]
fn array_splice_phpt() {
    let mut t = vm();
    let a = "['a', 'b', 'c', 'd', 'e']";
    // Remove from offset to the end.
    let (r, arr) = call_ref(&mut t, "array_splice", php(a), vec![i(2)]);
    assert_eq!(show(&r.unwrap()), "['c', 'd', 'e']");
    assert_eq!(show(&arr), "['a', 'b']");
    // Remove with length.
    let (r, arr) = call_ref(&mut t, "array_splice", php(a), vec![i(1), i(2)]);
    assert_eq!(show(&r.unwrap()), "['b', 'c']");
    assert_eq!(show(&arr), "['a', 'd', 'e']");
    // Negative offset and length.
    let (r, arr) = call_ref(&mut t, "array_splice", php(a), vec![i(-3), i(-1)]);
    assert_eq!(show(&r.unwrap()), "['c', 'd']");
    assert_eq!(show(&arr), "['a', 'b', 'e']");
    // Replacement (array and scalar).
    let (r, arr) = call_ref(
        &mut t,
        "array_splice",
        php(a),
        vec![i(1), i(2), php("['X', 'Y', 'Z']")],
    );
    assert_eq!(show(&r.unwrap()), "['b', 'c']");
    assert_eq!(show(&arr), "['a', 'X', 'Y', 'Z', 'd', 'e']");
    let (r, arr) = call_ref(&mut t, "array_splice", php(a), vec![i(1), i(1), s("X")]);
    assert_eq!(show(&r.unwrap()), "['b']");
    assert_eq!(show(&arr), "['a', 'X', 'c', 'd', 'e']");
    // Insert without removing.
    let (r, arr) = call_ref(&mut t, "array_splice", php(a), vec![i(1), i(0), s("X")]);
    assert_eq!(show(&r.unwrap()), "[]");
    assert_eq!(show(&arr), "['a', 'X', 'b', 'c', 'd', 'e']");
    // Offset beyond the end appends the replacement.
    let (r, arr) = call_ref(
        &mut t,
        "array_splice",
        php("['a', 'b']"),
        vec![i(10), i(0), php("['X']")],
    );
    assert_eq!(show(&r.unwrap()), "[]");
    assert_eq!(show(&arr), "['a', 'b', 'X']");
    // null length means "to the end".
    let (r, arr) = call_ref(
        &mut t,
        "array_splice",
        php(a),
        vec![i(3), Value::Null, php("['X']")],
    );
    assert_eq!(show(&r.unwrap()), "['d', 'e']");
    assert_eq!(show(&arr), "['a', 'b', 'c', 'X']");
    // String keys are kept; integer keys renumbered in both arrays.
    let (r, arr) = call_ref(
        &mut t,
        "array_splice",
        php("['x' => 1, 5 => 2, 'y' => 3, 9 => 4]"),
        vec![i(1), i(2)],
    );
    assert_eq!(show(&r.unwrap()), "[0 => 2, 'y' => 3]");
    assert_eq!(show(&arr), "['x' => 1, 0 => 4]");
    // An empty replacement array deletes only.
    let (_, arr) = call_ref(&mut t, "array_splice", php(a), vec![i(1), i(1), php("[]")]);
    assert_eq!(show(&arr), "['a', 'c', 'd', 'e']");
    // Next free index follows the renumbering.
    let (_, arr) = call_ref(&mut t, "array_splice", php("[1, 2, 3]"), vec![i(0), i(1)]);
    let Value::Array(mut arr) = arr else {
        unreachable!()
    };
    std::rc::Rc::make_mut(&mut arr).append(i(9)).unwrap();
    assert_eq!(show(&Value::Array(arr)), "[2, 3, 9]");
}

#[test]
fn array_reverse_phpt() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_reverse", vec![php("[1, 2, 3]")]),
        "[3, 2, 1]"
    );
    assert_eq!(
        run(&mut t, "array_reverse", vec![php("[1, 2, 3]"), b(true)]),
        "[2 => 3, 1 => 2, 0 => 1]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_reverse",
            vec![php("['a' => 1, 5 => 2, 'b' => 3]")]
        ),
        "['b' => 3, 0 => 2, 'a' => 1]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_reverse",
            vec![php("['a' => 1, 5 => 2, 'b' => 3]"), b(true)]
        ),
        "['b' => 3, 5 => 2, 'a' => 1]"
    );
    assert_eq!(run(&mut t, "array_reverse", vec![php("[]")]), "[]");
}

// ---------------------------------------------------------------------------
// Stack functions
// ---------------------------------------------------------------------------

#[test]
fn array_push_phpt() {
    let mut t = vm();
    let (r, a) = call_ref(&mut t, "array_push", php("[1, 2]"), vec![i(3), s("x")]);
    assert_eq!(show(&r.unwrap()), "4");
    assert_eq!(show(&a), "[1, 2, 3, 'x']");
    let (r, a) = call_ref(&mut t, "array_push", php("['a' => 1, 5 => 2]"), vec![i(3)]);
    assert_eq!(show(&r.unwrap()), "3");
    assert_eq!(show(&a), "['a' => 1, 5 => 2, 6 => 3]");
    // array_push_empty.phpt: no values.
    let (r, a) = call_ref(&mut t, "array_push", php("[1]"), vec![]);
    assert_eq!(show(&r.unwrap()), "1");
    assert_eq!(show(&a), "[1]");
    // The next index is already taken.
    let (r, _) = call_ref(&mut t, "array_push", php("[PHP_INT_MAX => 1]"), vec![i(2)]);
    assert_eq!(
        thrown(r),
        (
            "Error".into(),
            "Cannot add element to the array as the next element is already occupied".into()
        )
    );
    // Arrays and nulls are pushed as values.
    let (_, a) = call_ref(
        &mut t,
        "array_push",
        php("[]"),
        vec![php("[1]"), Value::Null],
    );
    assert_eq!(show(&a), "[[1], null]");
}

#[test]
fn array_pop_shift_phpt() {
    let mut t = vm();
    let (r, a) = call_ref(&mut t, "array_pop", php("[1, 2, 3]"), vec![]);
    assert_eq!(show(&r.unwrap()), "3");
    assert_eq!(show(&a), "[1, 2]");
    let (r, a) = call_ref(&mut t, "array_pop", php("[]"), vec![]);
    assert_eq!(show(&r.unwrap()), "null");
    assert_eq!(show(&a), "[]");
    // The next free index is decremented when popping the last integer key.
    let (_, a) = call_ref(&mut t, "array_pop", php("[1, 2, 3]"), vec![]);
    let Value::Array(mut arr) = a else {
        unreachable!()
    };
    std::rc::Rc::make_mut(&mut arr).append(i(7)).unwrap();
    assert_eq!(show(&Value::Array(arr)), "[1, 2, 7]");
    // ... but not for other keys.
    let (_, a) = call_ref(
        &mut t,
        "array_pop",
        php("[5 => 1, 6 => 2, 'k' => 3]"),
        vec![],
    );
    let Value::Array(mut arr) = a else {
        unreachable!()
    };
    std::rc::Rc::make_mut(&mut arr).append(i(7)).unwrap();
    assert_eq!(show(&Value::Array(arr)), "[5 => 1, 6 => 2, 7 => 7]");

    let (r, a) = call_ref(&mut t, "array_shift", php("[1, 2, 3]"), vec![]);
    assert_eq!(show(&r.unwrap()), "1");
    assert_eq!(show(&a), "[2, 3]");
    let (r, a) = call_ref(
        &mut t,
        "array_shift",
        php("['a' => 1, 5 => 2, 'b' => 3, 9 => 4]"),
        vec![],
    );
    assert_eq!(show(&r.unwrap()), "1");
    assert_eq!(show(&a), "[0 => 2, 'b' => 3, 1 => 4]");
    let (r, _) = call_ref(&mut t, "array_shift", php("[]"), vec![]);
    assert_eq!(show(&r.unwrap()), "null");
    // A popped reference is unwrapped.
    let rf = PhpRef::new(i(5));
    let mut arr = PhpArray::new();
    arr.append(Value::Ref(rf)).unwrap();
    let (r, _) = call_ref(&mut t, "array_pop", Value::array(arr), vec![]);
    assert!(matches!(r.unwrap(), Value::Int(5)));
}

#[test]
fn array_unshift_phpt() {
    let mut t = vm();
    let (r, a) = call_ref(&mut t, "array_unshift", php("[1, 2]"), vec![i(8), i(9)]);
    assert_eq!(show(&r.unwrap()), "4");
    assert_eq!(show(&a), "[8, 9, 1, 2]");
    let (r, a) = call_ref(
        &mut t,
        "array_unshift",
        php("['a' => 1, 5 => 2]"),
        vec![s("x")],
    );
    assert_eq!(show(&r.unwrap()), "3");
    assert_eq!(show(&a), "[0 => 'x', 'a' => 1, 1 => 2]");
    // Without values the integer keys are still renumbered.
    let (r, a) = call_ref(&mut t, "array_unshift", php("[5 => 'a', 9 => 'b']"), vec![]);
    assert_eq!(show(&r.unwrap()), "2");
    assert_eq!(show(&a), "['a', 'b']");
    let (_, a) = call_ref(&mut t, "array_unshift", php("[]"), vec![php("[1]")]);
    assert_eq!(show(&a), "[[1]]");
}

#[test]
fn array_pop_shift_on_self_referencing_arrays() {
    // gh16957.phpt / gh17447.phpt
    let mut t = vm();
    // $new_array = [&$new_array, 1, 'two']; $shifted = array_shift($new_array);
    let var = PhpRef::new(Value::Null);
    var.set(Value::array(PhpArray::from_values(vec![
        Value::Ref(var.clone()),
        i(1),
        s("two"),
    ])));
    let r = t
        .call("array_shift", vec![Value::Ref(var.clone())])
        .unwrap();
    assert_eq!(show(&r), "[1, 'two']");
    assert_eq!(show(&var.get()), "[1, 'two']");

    // $new_array2 = [&$new_array2, 2 => 1, 300 => 'two'];
    let var = PhpRef::new(Value::Null);
    let mut a = PhpArray::new();
    a.append(Value::Ref(var.clone())).unwrap();
    a.insert(2.into(), i(1));
    a.insert(300.into(), s("two"));
    var.set(Value::array(a));
    let r = t
        .call("array_shift", vec![Value::Ref(var.clone())])
        .unwrap();
    assert_eq!(show(&r), "[1, 'two']");
    assert_eq!(show(&var.get()), "[1, 'two']");

    // $input[] = &$input; array_pop($input)
    let var = PhpRef::new(Value::Null);
    var.set(Value::array(PhpArray::from_values(vec![Value::Ref(
        var.clone(),
    )])));
    let r = t.call("array_pop", vec![Value::Ref(var.clone())]).unwrap();
    assert_eq!(show(&r), "[]");
    assert_eq!(show(&var.get()), "[]");
}

#[test]
fn array_fill_allows_appending_bug72116() {
    let mut t = vm();
    let r = t.call("array_fill", vec![i(0), i(1), s("..")]).unwrap();
    let Value::Array(mut r) = r else {
        unreachable!()
    };
    std::rc::Rc::make_mut(&mut r).append(s("a")).unwrap();
    assert_eq!(show(&Value::Array(r)), "['..', 'a']");
}
