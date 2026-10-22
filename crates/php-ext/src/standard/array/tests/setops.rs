//! The array_diff* and array_intersect* families.

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
        run(&mut t, "array_diff_uassoc", args(&[php(a), php(a), s("t_cmp")])),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_diff_ukey", args(&[php(a), php(a), s("t_cmp")])),
        "[]"
    );
    assert_eq!(
        run(&mut t, "array_intersect_uassoc", args(&[php(a), php(a), s("t_cmp")])),
        "['a' => 'green', 'b' => 'Brown', 'c' => 'blue', 0 => 'red']"
    );
}
