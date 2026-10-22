//! sort, rsort, asort, arsort, ksort, krsort, natsort, natcasesort, usort,
//! uasort, uksort and array_multisort.

use php_runtime::{PhpRef, Value};

use super::*;

const STRINGS: &str = "['l' => 'lemon', 'o' => 'orange', 'O' => 'Orange', 'O1' => 'Orange1', 'o2' => 'orange2', 'O3' => 'Orange3', 'o20' => 'orange20', 'b' => 'banana']";

const REGULAR: i64 = 0;
const NUMERIC: i64 = 1;
const STRING: i64 = 2;
const NATURAL: i64 = 6;
const CASE: i64 = 8;

/// The six flag variants of the `*sort_basic.phpt` tests, in order:
/// default, SORT_REGULAR, SORT_STRING, STRING|CASE, NATURAL, NATURAL|CASE.
fn flag_variants() -> Vec<Vec<Value>> {
    vec![
        vec![],
        vec![i(REGULAR)],
        vec![i(STRING)],
        vec![i(STRING | CASE)],
        vec![i(NATURAL)],
        vec![i(NATURAL | CASE)],
    ]
}

#[test]
fn sort_basic_phpt() {
    let mut t = vm();
    let expected = [
        "['Orange', 'Orange1', 'Orange3', 'banana', 'lemon', 'orange', 'orange2', 'orange20']",
        "['Orange', 'Orange1', 'Orange3', 'banana', 'lemon', 'orange', 'orange2', 'orange20']",
        "['Orange', 'Orange1', 'Orange3', 'banana', 'lemon', 'orange', 'orange2', 'orange20']",
        "['banana', 'lemon', 'orange', 'Orange', 'Orange1', 'orange2', 'orange20', 'Orange3']",
        "['Orange', 'Orange1', 'Orange3', 'banana', 'lemon', 'orange', 'orange2', 'orange20']",
        "['banana', 'lemon', 'orange', 'Orange', 'Orange1', 'orange2', 'Orange3', 'orange20']",
    ];
    for (flags, want) in flag_variants().into_iter().zip(expected) {
        assert_eq!(sorted(&mut t, "sort", STRINGS, flags), want);
    }
    assert_eq!(
        sorted(&mut t, "sort", "[100, 33, 555, 22]", vec![]),
        "[22, 33, 100, 555]"
    );
    assert_eq!(
        sorted(&mut t, "sort", "[100, 33, 555, 22]", vec![i(NUMERIC)]),
        "[22, 33, 100, 555]"
    );
}

#[test]
fn rsort_basic_phpt() {
    let mut t = vm();
    let expected = [
        "['orange20', 'orange2', 'orange', 'lemon', 'banana', 'Orange3', 'Orange1', 'Orange']",
        "['orange20', 'orange2', 'orange', 'lemon', 'banana', 'Orange3', 'Orange1', 'Orange']",
        "['orange20', 'orange2', 'orange', 'lemon', 'banana', 'Orange3', 'Orange1', 'Orange']",
        "['Orange3', 'orange20', 'orange2', 'Orange1', 'orange', 'Orange', 'lemon', 'banana']",
        "['orange20', 'orange2', 'orange', 'lemon', 'banana', 'Orange3', 'Orange1', 'Orange']",
        "['orange20', 'Orange3', 'orange2', 'Orange1', 'orange', 'Orange', 'lemon', 'banana']",
    ];
    for (flags, want) in flag_variants().into_iter().zip(expected) {
        assert_eq!(sorted(&mut t, "rsort", STRINGS, flags), want);
    }
    assert_eq!(
        sorted(&mut t, "rsort", "[100, 33, 555, 22]", vec![]),
        "[555, 100, 33, 22]"
    );
}

#[test]
fn asort_basic_phpt() {
    let mut t = vm();
    let expected = [
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'O' => 'Orange', 'O1' => 'Orange1', 'o2' => 'orange2', 'o20' => 'orange20', 'O3' => 'Orange3']",
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'O' => 'Orange', 'O1' => 'Orange1', 'o2' => 'orange2', 'O3' => 'Orange3', 'o20' => 'orange20']",
    ];
    for (flags, want) in flag_variants().into_iter().zip(expected) {
        assert_eq!(sorted(&mut t, "asort", STRINGS, flags), want);
    }
    assert_eq!(
        sorted(
            &mut t,
            "asort",
            "[1 => 100, 2 => 33, 3 => 555, 4 => 22]",
            vec![]
        ),
        "[4 => 22, 2 => 33, 1 => 100, 3 => 555]"
    );
}

#[test]
fn ksort_basic_phpt() {
    let mut t = vm();
    let expected = [
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'O' => 'Orange', 'O1' => 'Orange1', 'o2' => 'orange2', 'o20' => 'orange20', 'O3' => 'Orange3']",
        "['O' => 'Orange', 'O1' => 'Orange1', 'O3' => 'Orange3', 'b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'o2' => 'orange2', 'o20' => 'orange20']",
        "['b' => 'banana', 'l' => 'lemon', 'o' => 'orange', 'O' => 'Orange', 'O1' => 'Orange1', 'o2' => 'orange2', 'O3' => 'Orange3', 'o20' => 'orange20']",
    ];
    for (flags, want) in flag_variants().into_iter().zip(expected) {
        assert_eq!(sorted(&mut t, "ksort", STRINGS, flags), want);
    }
    let nums = "[100 => 4, 33 => 3, 555 => 2, 22 => 1]";
    assert_eq!(
        sorted(&mut t, "ksort", nums, vec![]),
        "[22 => 1, 33 => 3, 100 => 4, 555 => 2]"
    );
    assert_eq!(
        sorted(&mut t, "krsort", nums, vec![i(NUMERIC)]),
        "[555 => 2, 100 => 4, 33 => 3, 22 => 1]"
    );
    // The "next free element" is kept by key-preserving sorts.
    let (_, a) = call_ref(&mut t, "ksort", php("[5 => 'a', 2 => 'b']"), vec![]);
    assert_eq!(show(&a), "[2 => 'b', 5 => 'a']");
}

#[test]
fn ksort_mixed_keys_uses_php8_comparison() {
    let mut t = vm();
    // int keys compare with string keys like `2 <=> "b"`: as strings.
    assert_eq!(
        sorted(
            &mut t,
            "ksort",
            "['b' => 1, 2 => 2, 'a' => 3, 1 => 4]",
            vec![]
        ),
        "[1 => 4, 2 => 2, 'a' => 3, 'b' => 1]"
    );
    assert_eq!(
        sorted(
            &mut t,
            "krsort",
            "['b' => 1, 2 => 2, 'a' => 3, 1 => 4]",
            vec![]
        ),
        "['b' => 1, 'a' => 3, 2 => 2, 1 => 4]"
    );
}

#[test]
fn natsort_phpt() {
    let mut t = vm();
    let a = "['img12.png', 'img10.png', 'img2.png', 'img1.png']";
    assert_eq!(
        sorted(&mut t, "sort", a, vec![]),
        "['img1.png', 'img10.png', 'img12.png', 'img2.png']"
    );
    assert_eq!(
        sorted(&mut t, "natsort", a, vec![]),
        "[3 => 'img1.png', 2 => 'img2.png', 1 => 'img10.png', 0 => 'img12.png']"
    );
    // natcasesort ignores case, natsort does not.
    let b = "['B1', 'a10', 'A2', 'b3']";
    assert_eq!(
        sorted(&mut t, "natsort", b, vec![]),
        "[2 => 'A2', 0 => 'B1', 1 => 'a10', 3 => 'b3']"
    );
    assert_eq!(
        sorted(&mut t, "natcasesort", b, vec![]),
        "[2 => 'A2', 1 => 'a10', 0 => 'B1', 3 => 'b3']"
    );
}

#[test]
fn sort_numeric_and_string_flags() {
    let mut t = vm();
    // SORT_STRING compares "10" < "9"; SORT_NUMERIC compares by value.
    assert_eq!(
        sorted(&mut t, "sort", "['10', '9', '2', '1']", vec![i(STRING)]),
        "['1', '10', '2', '9']"
    );
    assert_eq!(
        sorted(&mut t, "sort", "['10', '9', '2', '1']", vec![i(NUMERIC)]),
        "['1', '2', '9', '10']"
    );
    assert_eq!(
        sorted(&mut t, "sort", "[10, 9, 2, 1]", vec![i(STRING)]),
        "[1, 10, 2, 9]"
    );
    // Non-numeric strings are 0 for SORT_NUMERIC; the sort is stable.
    assert_eq!(
        sorted(&mut t, "sort", "['b', 'a', 1, 0]", vec![i(NUMERIC)]),
        "['b', 'a', 0, 1]"
    );
}

#[test]
fn sort_regular_php8_semantics() {
    let mut t = vm();
    // 0 == "a" is false in PHP 8: 0 is compared with "a" as strings.
    assert_eq!(
        sorted(&mut t, "sort", "['a', 0, 'b']", vec![]),
        "[0, 'a', 'b']"
    );
    // Numeric strings compare numerically.
    assert_eq!(
        sorted(&mut t, "sort", "['10', '9', 1.5, '1e1']", vec![]),
        "[1.5, '9', '10', '1e1']"
    );
}

#[test]
fn single_element_sort_resets_state() {
    // sort_single_element.phpt
    let mut t = vm();
    let (r, a) = call_ref(&mut t, "usort", php("['answer' => 42]"), vec![s("t_cmp")]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&a), "[42]");
    let mut arr = match a {
        Value::Array(a) => (*a).clone(),
        _ => unreachable!(),
    };
    arr.append(Value::Int(43)).unwrap();
    assert_eq!(show(&Value::array(arr)), "[42, 43]");
    // Empty arrays are returned untouched.
    let (r, a) = call_ref(&mut t, "usort", php("[]"), vec![s("t_cmp")]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&a), "[]");
}

#[test]
fn usort_variants() {
    let mut t = vm();
    let (_, a) = call_ref(&mut t, "usort", php("[3, 1, 2]"), vec![s("t_cmp")]);
    assert_eq!(show(&a), "[1, 2, 3]");
    let (_, a) = call_ref(&mut t, "usort", php("[3, 1, 2]"), vec![s("t_rcmp")]);
    assert_eq!(show(&a), "[3, 2, 1]");
    let (_, a) = call_ref(
        &mut t,
        "uasort",
        php("['a' => 3, 'b' => 1, 'c' => 2]"),
        vec![s("t_cmp")],
    );
    assert_eq!(show(&a), "['b' => 1, 'c' => 2, 'a' => 3]");
    let (_, a) = call_ref(
        &mut t,
        "uksort",
        php("['b' => 3, 'a' => 1, 10 => 2]"),
        vec![s("t_cmp")],
    );
    assert_eq!(show(&a), "[10 => 2, 'a' => 1, 'b' => 3]");
}

#[test]
fn usort_is_stable() {
    // usort_stability.phpt
    let mut t = vm();
    let n = 10_000;
    let big = Value::array(php_runtime::PhpArray::from_values(
        (0..=n).map(Value::Int).collect(),
    ));
    let (_, a) = call_ref(&mut t, "usort", big.clone(), vec![s("t_zero")]);
    assert_eq!(show(&a), show(&big));
    // asort_stability: equal values keep their original order.
    let (_, a) = call_ref(
        &mut t,
        "asort",
        php("['a' => 1, 'b' => 0, 'c' => 1, 'd' => 0]"),
        vec![],
    );
    assert_eq!(show(&a), "['b' => 0, 'd' => 0, 'a' => 1, 'c' => 1]");
    let (_, a) = call_ref(
        &mut t,
        "arsort",
        php("['a' => 1, 'b' => 0, 'c' => 1, 'd' => 0]"),
        vec![],
    );
    assert_eq!(show(&a), "['a' => 1, 'c' => 1, 'b' => 0, 'd' => 0]");
}

#[test]
fn usort_exact_comparison_sequence() {
    // zend_sort_3 for three elements: cmp(a,b), cmp(c,b), cmp(b,c).
    let mut t = vm();
    let (_, a) = call_ref(&mut t, "usort", php("[3, 1, 2]"), vec![s("t_log")]);
    assert_eq!(show(&a), "[1, 2, 3]");
    assert_eq!(take_log(), ["3,1", "2,1", "3,2"]);

    // Two elements: a single comparison.
    let (_, a) = call_ref(&mut t, "usort", php("[2, 1]"), vec![s("t_log")]);
    assert_eq!(show(&a), "[1, 2]");
    assert_eq!(take_log(), ["2,1"]);

    // Already sorted five elements (zend_sort_5 chain): 4 comparisons.
    let (_, a) = call_ref(&mut t, "usort", php("[1, 2, 3, 4, 5]"), vec![s("t_log")]);
    assert_eq!(show(&a), "[1, 2, 3, 4, 5]");
    assert_eq!(take_log().len(), 4);

    // Insertion sort for 6..16 elements: sorted input needs n-1 comparisons.
    let (_, a) = call_ref(
        &mut t,
        "usort",
        php("[1, 2, 3, 4, 5, 6, 7, 8]"),
        vec![s("t_log")],
    );
    assert_eq!(show(&a), "[1, 2, 3, 4, 5, 6, 7, 8]");
    assert_eq!(take_log().len(), 7);
}

#[test]
fn zend_sort_sorts_everything() {
    // Exercise the quick-sort path (n > 16) and the 5-pivot path (n >= 1024).
    for n in [17usize, 40, 100, 1000, 1025, 3000] {
        let mut v: Vec<i64> = (0..n as i64).map(|k| (k * 7919 + 13) % 1009).collect();
        let mut expect = v.clone();
        expect.sort();
        let mut cmp = |a: &i64, b: &i64| (a > b) as i32 - (a < b) as i32;
        super::super::sort::zend_sort(&mut v, &mut cmp);
        assert_eq!(v, expect, "n = {n}");
    }
    // Also with many duplicates and reversed input.
    let mut v: Vec<i64> = (0..500).rev().map(|k| k % 7).collect();
    let mut expect = v.clone();
    expect.sort();
    let mut cmp = |a: &i64, b: &i64| (a > b) as i32 - (a < b) as i32;
    super::super::sort::zend_sort(&mut v, &mut cmp);
    assert_eq!(v, expect);
}

#[test]
fn usort_bool_return_is_deprecated() {
    // usort_variation11.phpt: `return $a > $b;` still sorts, with one
    // deprecation per call.
    let mut t = vm();
    for r in [2, 4, 8, 16, 32] {
        let items: Vec<i64> = (0..=r).map(|k| (k * 7 + 3) % (r + 1)).collect();
        let arr = Value::array(php_runtime::PhpArray::from_values(
            items.iter().map(|&n| Value::Int(n)).collect(),
        ));
        t.output();
        let (_, a) = call_ref(&mut t, "usort", arr, vec![s("t_gt")]);
        let want: Vec<String> = (0..=r).map(|n| n.to_string()).collect();
        assert_eq!(show(&a), format!("[{}]", want.join(", ")), "r = {r}");
        let out = t.output();
        assert_eq!(
            out.matches("Deprecated: usort(): Returning bool from comparison function is deprecated, return an integer less than, equal to, or greater than zero in ").count(),
            1,
            "{out}"
        );
    }
    // uksort / uasort likewise.
    t.output();
    let (_, a) = call_ref(
        &mut t,
        "uksort",
        php("[3 => 'a', 1 => 'b', 2 => 'c']"),
        vec![s("t_gt")],
    );
    assert_eq!(show(&a), "[1 => 'b', 2 => 'c', 3 => 'a']");
    assert!(t.output().contains("Deprecated: uksort(): Returning bool"));
    let (_, a) = call_ref(
        &mut t,
        "uasort",
        php("['x' => 3, 'y' => 1]"),
        vec![s("t_gt")],
    );
    assert_eq!(show(&a), "['y' => 1, 'x' => 3]");
    assert!(t.output().contains("Deprecated: uasort(): Returning bool"));
}

#[test]
fn usort_callback_errors() {
    let mut t = vm();
    let (r, _) = call_ref(
        &mut t,
        "usort",
        php("[3, 1, 2]"),
        vec![s("nope_not_a_function")],
    );
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "usort(): Argument #2 ($callback) must be a valid callback, function \"nope_not_a_function\" not found or invalid function name".into()
        )
    );
    // An exception thrown by the callback propagates.
    let (r, _) = call_ref(&mut t, "usort", php("[3, 1, 2]"), vec![s("t_throw")]);
    assert_eq!(thrown(r), ("Exception".into(), "boom".into()));
}

#[test]
fn sort_requires_an_array() {
    let mut t = vm();
    let (r, _) = call_ref(&mut t, "sort", s("x"), vec![]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "sort(): Argument #1 ($array) must be of type array, string given".into()
        )
    );
}

fn multisort(t: &mut TestVm, items: Vec<Value>) -> (PResult<Value>, Vec<Value>) {
    let refs: Vec<Option<PhpRef>> = items
        .iter()
        .map(|v| match v {
            Value::Array(_) => Some(PhpRef::new(v.clone())),
            _ => None,
        })
        .collect();
    let args: Vec<Value> = items
        .into_iter()
        .zip(&refs)
        .map(|(v, r)| match r {
            Some(r) => Value::Ref(r.clone()),
            None => v,
        })
        .collect();
    let ret = t.call("array_multisort", args);
    (ret, refs.into_iter().flatten().map(|r| r.get()).collect())
}

#[test]
fn array_multisort_basic_phpt() {
    let mut t = vm();
    let ar1 = php("['row1' => 2, 'row2' => 1, 'row3' => 1]");
    let ar2 = php("['row1' => 2, 'row2' => 'aa', 'row3' => '1']");
    let (r, out) = multisort(&mut t, vec![ar1, i(4), i(0), ar2, i(3), i(2)]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&out[0]), "['row2' => 1, 'row3' => 1, 'row1' => 2]");
    assert_eq!(
        show(&out[1]),
        "['row2' => 'aa', 'row3' => '1', 'row1' => 2]"
    );

    let (r, out) = multisort(&mut t, vec![out[1].clone()]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(
        show(&out[0]),
        "['row3' => '1', 'row1' => 2, 'row2' => 'aa']"
    );
}

#[test]
fn array_multisort_two_columns() {
    let mut t = vm();
    // ORDER BY data1 DESC, data2 ASC
    let data1 = php("[10, 100, 100, 0]");
    let data2 = php("[1, 3, 2, 4]");
    let (r, out) = multisort(&mut t, vec![data1, i(3), data2]);
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&out[0]), "[100, 100, 10, 0]");
    assert_eq!(show(&out[1]), "[2, 3, 1, 4]");
}

#[test]
fn array_multisort_errors() {
    let mut t = vm();
    // array_multisort_error.phpt
    let (r, _) = multisort(&mut t, vec![php("[1]"), i(4), i(4)]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "array_multisort(): Argument #3 must be an array or a sort flag that has not already been specified".into()
        )
    );
    let (r, _) = multisort(&mut t, vec![php("[1]"), i(2), i(1)]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "array_multisort(): Argument #3 must be an array or a sort flag that has not already been specified".into()
        )
    );
    let (r, _) = multisort(&mut t, vec![php("[1]"), i(99)]);
    assert_eq!(
        thrown(r),
        (
            "ValueError".into(),
            "array_multisort(): Argument #2 must be a valid sort flag".into()
        )
    );
    let (r, _) = multisort(&mut t, vec![php("[1, 2]"), php("[1]")]);
    assert_eq!(
        thrown(r),
        ("ValueError".into(), "Array sizes are inconsistent".into())
    );
    let (r, _) = multisort(&mut t, vec![php("[1]"), s("x")]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "array_multisort(): Argument #2 must be an array or a sort flag".into()
        )
    );
}

#[test]
fn array_multisort_stability_and_flags() {
    let mut t = vm();
    // Stable for equal rows.
    let (_, out) = multisort(
        &mut t,
        vec![php("[1, 1, 1, 1]"), php("['a', 'b', 'c', 'd']")],
    );
    assert_eq!(show(&out[1]), "['a', 'b', 'c', 'd']");
    // Case-insensitive string sort.
    let (_, out) = multisort(&mut t, vec![php("['b', 'A', 'c', 'B']"), i(2 | CASE)]);
    assert_eq!(show(&out[0]), "['A', 'b', 'B', 'c']");
    // Natural sort.
    let (_, out) = multisort(
        &mut t,
        vec![php("['img12', 'img10', 'img2', 'img1']"), i(NATURAL)],
    );
    assert_eq!(show(&out[0]), "['img1', 'img2', 'img10', 'img12']");
    // Integer keys are renumbered, string keys are kept.
    let (_, out) = multisort(&mut t, vec![php("['x' => 2, 5 => 1, 9 => 3]")]);
    assert_eq!(show(&out[0]), "[0 => 1, 'x' => 2, 1 => 3]");
}

#[test]
fn sort_mixed_values_matches_php_exactly() {
    // sort_variation11.phpt ("OK to fail as result is unpredictable"): the
    // result of sorting values without a total order depends on the exact
    // zend_sort algorithm and the stable-sort fallback.
    let mut t = vm();
    let input = r#"[[], [[33, -5, 6], [11], [22, -55], []], -4, "4", 4.00, "b", "5", -2, -2.0, -2.98989, "-.9", "True", "", NULL, "ab", "abcd", 0.0, -0, "abcd\0abcd\0abcd", '', true, false]"#;
    let want = r#"['', null, '', 0.0, false, -4, -2.98989, -2, -2.0, '-.9', 0, '4', 4.0, '5', 'True', 'ab', 'abcd', 'abcd\0abcd\0abcd', 'b', [], [[33, -5, 6], [11], [22, -55], []], true]"#;
    assert_eq!(sorted(&mut t, "sort", input, vec![]), want);
    assert_eq!(sorted(&mut t, "sort", input, vec![i(REGULAR)]), want);
}

#[test]
fn asort_arsort_rsort_mixed_values_match_php_exactly() {
    // asort_variation11.phpt, arsort_variation11.phpt, rsort_variation11.phpt
    let mut t = vm();
    let keyed = r#"["array1" => [], "array2" => ["sub_array[2,1]" => [33, -5, 6], "sub_array[2,2]" => [11], "sub_array[2,3]" => [22, -55], "sub_array[2,4]" => []], 4 => 4, "4" => "4", 4 => 4.01, "b" => "b", "5" => "5", -2 => -2, -2 => -2.01, -2 => -2.98989, "-.9" => "-.9", "True" => "True", "" => "", "ab" => "ab", "abcd" => "abcd", 0 => 0.01, -0 => -0, '' => '', "abcd\0abcd\0abcd" => "abcd\0abcd\0abcd", 0 => 0.001]"#;
    let want_asort = r#"['' => '', -2 => -2.98989, '-.9' => '-.9', 0 => 0.001, 4 => 4.01, 5 => '5', 'True' => 'True', 'ab' => 'ab', 'abcd' => 'abcd', 'abcd\0abcd\0abcd' => 'abcd\0abcd\0abcd', 'b' => 'b', 'array1' => [], 'array2' => ['sub_array[2,1]' => [33, -5, 6], 'sub_array[2,2]' => [11], 'sub_array[2,3]' => [22, -55], 'sub_array[2,4]' => []]]"#;
    let want_arsort = r#"['array2' => ['sub_array[2,1]' => [33, -5, 6], 'sub_array[2,2]' => [11], 'sub_array[2,3]' => [22, -55], 'sub_array[2,4]' => []], 'array1' => [], 'b' => 'b', 'abcd\0abcd\0abcd' => 'abcd\0abcd\0abcd', 'abcd' => 'abcd', 'ab' => 'ab', 'True' => 'True', 5 => '5', 4 => 4.01, 0 => 0.001, '-.9' => '-.9', -2 => -2.98989, '' => '']"#;
    assert_eq!(sorted(&mut t, "asort", keyed, vec![]), want_asort);
    assert_eq!(sorted(&mut t, "arsort", keyed, vec![]), want_arsort);

    let list = r#"[[], [[33, -5, 6], [11], [22, -55], []], -4, "4", 4.00, "b", "5", -2, -2.0, -2.98989, "-.9", "True", "", NULL, "ab", "abcd", 0.0, -0, "abcd\0abcd\0abcd", '', true, false]"#;
    let want_rsort = r#"[[[33, -5, 6], [11], [22, -55], []], [], 'b', 'abcd\0abcd\0abcd', 'abcd', 'ab', 'True', '5', '4', 4.0, '-.9', -2, -2.0, -2.98989, true, '', null, 0.0, 0, -4, '', false]"#;
    assert_eq!(sorted(&mut t, "rsort", list, vec![]), want_rsort);
}

#[test]
fn array_multisort_variations_phpt() {
    let mut t = vm();
    // array_multisort_variation4.phpt
    let (r, out) = multisort(
        &mut t,
        vec![
            php("[4, 3, 3, 3]"),
            php("[9, 3, 2, 2]"),
            php("[9, 9, 2, 1]"),
        ],
    );
    assert_eq!(show(&r.unwrap()), "true");
    assert_eq!(show(&out[0]), "[3, 3, 3, 4]");
    assert_eq!(show(&out[1]), "[2, 2, 3, 9]");
    assert_eq!(show(&out[2]), "[1, 2, 9, 9]");
    // array_multisort_variation10.phpt: an anonymous array argument.
    assert_eq!(
        run(&mut t, "array_multisort", vec![php("[1, 3, 2, 4]")]),
        "true"
    );
}
