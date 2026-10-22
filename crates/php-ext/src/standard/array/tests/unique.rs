//! array_unique().

use php_runtime::PhpArray;

use super::*;

const REGULAR: i64 = 0;
const NUMERIC: i64 = 1;
const STRING: i64 = 2;
const CASE: i64 = 8;

#[test]
fn array_unique_basic_phpt() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_unique", vec![php("[1, 2, '1', '2']")]),
        "[1, 2]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("['1' => 'one', 1 => 'one', 2 => 'two', '2' => 'two']")]
        ),
        "[1 => 'one', 2 => 'two']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("['1' => 'one', 'two', 'one', 2 => 'two', 'three']")]
        ),
        "[1 => 'one', 2 => 'two', 4 => 'three']"
    );
}

#[test]
fn array_unique_variation2_phpt() {
    let mut t = vm();
    let mut check = |input: &str, want: &str| {
        assert_eq!(
            run(&mut t, "array_unique", vec![php(input)]),
            want,
            "{input}"
        );
    };
    check("[1, 2, 2, 1]", "[1, 2]");
    check("[1.1, 2.2, 1.1]", "[1.1, 2.2]");
    check("[false, true, false]", "[false, true]");
    check("[]", "[]");
    check("[null, null]", "[null]");
    check(
        "[1 => 'one', 2 => 'two', 2 => 'two']",
        "[1 => 'one', 2 => 'two']",
    );
    check(
        "['one' => 1, 'two' => 2, '1' => 1]",
        "['one' => 1, 'two' => 2]",
    );
    check(
        "[1 => 10, 2 => 20, 4 => 40, 5 => 10]",
        "[1 => 10, 2 => 20, 4 => 40]",
    );
    check(
        "['one' => 'ten', 'two' => 'twenty', '10' => 'ten']",
        "['one' => 'ten', 'two' => 'twenty']",
    );
    check(
        "[true => 'true', false => 'false', 'false' => false, 'true' => true]",
        "[1 => 'true', 0 => 'false', 'false' => false, 'true' => true]",
    );
    check(
        "['' => 'emptyd', '' => 'emptys', 'emptyd' => '', 'emptys' => '']",
        "['' => 'emptys', 'emptyd' => '']",
    );
    check(
        "[1 => '', 2 => '', 5 => false, 6 => true]",
        "[1 => '', 6 => true]",
    );
    check(
        "['' => 1, '' => 2, false => 5, true => 6]",
        "['' => 2, 0 => 5, 1 => 6]",
    );
}

#[test]
fn array_unique_sort_flags() {
    let mut t = vm();
    // Default (SORT_STRING): "a" and "A" differ.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("[4, '4', '3', 4, 3, 'a', 'A']")]
        ),
        "[0 => 4, 2 => '3', 5 => 'a', 6 => 'A']"
    );
    // SORT_REGULAR compares loosely: 4 == "4", "3" == 3.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("[4, '4', '3', 4, 3, 'a', 'A']"), i(REGULAR)]
        ),
        "[0 => 4, 2 => '3', 5 => 'a', 6 => 'A']"
    );
    // Case-insensitive string comparison.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("['a', 'A', 'b', 'B', 'a']"), i(STRING | CASE)]
        ),
        "[0 => 'a', 2 => 'b']"
    );
    // Numeric comparison: "1", "1.0" and "01" are the same number.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("['1', '1.0', '01', 2, 2.0]"), i(NUMERIC)]
        ),
        "[0 => '1', 3 => 2]"
    );
    // String comparison distinguishes them.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("['1', '1.0', '01', 2, 2.0]"), i(STRING)]
        ),
        "['1', '1.0', '01', 2]"
    );
    // Natural order comparison.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("['img1', 'img01', 'IMG1']"), i(6)]
        ),
        "['img1', 'img01', 'IMG1']"
    );
}

#[test]
fn array_unique_with_references_and_arrays() {
    // array_unique_variation6.phpt: a reference with several owners stays one.
    let mut t = vm();
    let hello = PhpRef::new(s("hello"));
    let mut a = PhpArray::new();
    a.append(i(0)).unwrap();
    a.append(Value::Ref(hello.clone())).unwrap();
    a.append(Value::Ref(hello.clone())).unwrap();
    a.append(s("hello")).unwrap();
    let r = t
        .call("array_unique", vec![Value::array(a), i(STRING)])
        .unwrap();
    assert_eq!(show(&r), "[0, 'hello']");
    match &r {
        Value::Array(arr) => assert!(matches!(arr.get_int(1), Some(Value::Ref(_)))),
        _ => unreachable!(),
    }

    // array_unique_variation8.phpt: one warning per converted array.
    t.output();
    let r = t
        .call(
            "array_unique",
            vec![
                php("[[1, 2, 3, 1], ['hello', 'world'], [1, 2], [1, 2, 3, 1]]"),
                i(STRING),
            ],
        )
        .unwrap();
    assert_eq!(show(&r), "[[1, 2, 3, 1]]");
    assert_eq!(
        t.output()
            .matches("Warning: Array to string conversion in /t.php on line 3")
            .count(),
        4
    );
    // With SORT_REGULAR arrays compare by content.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("[[1, 2], [1, 2], [2, 1]]"), i(REGULAR)]
        ),
        "[0 => [1, 2], 2 => [2, 1]]"
    );
}

#[test]
fn array_unique_small_arrays_are_returned_as_is() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_unique", vec![php("[5 => 'a']")]),
        "[5 => 'a']"
    );
    assert_eq!(run(&mut t, "array_unique", vec![php("[]")]), "[]");
}

#[test]
fn array_unique_keeps_the_next_free_index() {
    let mut t = vm();
    let r = t
        .call("array_unique", vec![php("[1, 1, 2]"), i(REGULAR)])
        .unwrap();
    let Value::Array(mut a) = r else {
        unreachable!()
    };
    std::rc::Rc::make_mut(&mut a).append(i(7)).unwrap();
    assert_eq!(show(&Value::Array(a)), "[0 => 1, 2 => 2, 3 => 7]");
}

#[test]
fn array_unique_keeps_the_first_occurrence_bug79868() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_unique", vec![php("['b', 'a', 'b']"), i(REGULAR)]),
        "['b', 'a']"
    );
    // The surviving element is always the earliest one, whatever the sort did.
    assert_eq!(
        run(
            &mut t,
            "array_unique",
            vec![php("[3, 1, 3, 2, 1, 2, 3, 0, 0, 5, 4, 5, 4, 1, 2, 3, 3, 3]"), i(REGULAR)]
        ),
        "[0 => 3, 1 => 1, 3 => 2, 7 => 0, 9 => 5, 10 => 4]"
    );
}
