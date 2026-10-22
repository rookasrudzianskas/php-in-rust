//! array_merge, array_merge_recursive, array_replace, array_replace_recursive.

use std::rc::Rc;

use php_runtime::PhpArray;

use super::*;

#[test]
fn array_merge_basic() {
    let mut t = vm();
    assert_eq!(run(&mut t, "array_merge", vec![]), "[]");
    assert_eq!(
        run(&mut t, "array_merge", vec![php("[1, 2]"), php("[3]")]),
        "[1, 2, 3]"
    );
    // Integer keys are renumbered, string keys overwrite.
    assert_eq!(
        run(
            &mut t,
            "array_merge",
            vec![
                php("['a' => 1, 5 => 'x']"),
                php("['a' => 2, 5 => 'y', 'b' => 3]")
            ]
        ),
        "['a' => 2, 0 => 'x', 1 => 'y', 'b' => 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge",
            vec![php("[5 => 'a']"), php("[]"), php("[7 => 'b']")]
        ),
        "['a', 'b']"
    );
    assert_eq!(
        run(&mut t, "array_merge", vec![php("[3 => 'a', 1 => 'b']")]),
        "['a', 'b']"
    );
    assert_eq!(run(&mut t, "array_merge", vec![php("[]"), php("[]")]), "[]");
}

#[test]
fn array_merge_returns_the_same_array_when_possible() {
    let mut t = vm();
    let list = php("[1, 2, 3]");
    let r = t
        .call("array_merge", vec![list.clone(), php("[]")])
        .unwrap();
    match (&list, &r) {
        (Value::Array(a), Value::Array(b)) => assert!(Rc::ptr_eq(a, b)),
        _ => unreachable!(),
    }
    // A hash with integer keys must be renumbered, so it is copied.
    let holes = php("[3 => 'a']");
    let r = t.call("array_merge", vec![php("[]"), holes]).unwrap();
    assert_eq!(show(&r), "['a']");
}

#[test]
fn array_merge_errors() {
    let mut t = vm();
    assert_eq!(
        run_err(&mut t, "array_merge", vec![php("[1]"), s("x")]),
        "TypeError: array_merge(): Argument #2 must be of type array, string given"
    );
    assert_eq!(
        run_err(&mut t, "array_merge", vec![i(1)]),
        "TypeError: array_merge(): Argument #1 must be of type array, int given"
    );
}

#[test]
fn array_merge_keeps_shared_references() {
    let mut t = vm();
    let shared = PhpRef::new(i(1));
    let mut a = PhpArray::new();
    a.append(Value::Ref(shared.clone())).unwrap();
    let r = t
        .call("array_merge", vec![Value::array(a.clone()), php("[2]")])
        .unwrap();
    // The reference is shared with the result.
    shared.set(i(9));
    assert_eq!(show(&r), "[9, 2]");
    // A reference held only by the source array is copied as a plain value.
    let mut b = PhpArray::new();
    b.append(Value::Ref(PhpRef::new(i(4)))).unwrap();
    let r = t
        .call("array_merge", vec![Value::array(b), php("[5]")])
        .unwrap();
    match r {
        Value::Array(arr) => assert!(matches!(arr.get_int(0), Some(Value::Int(4)))),
        _ => unreachable!(),
    }
}

#[test]
fn array_merge_recursive_phpt() {
    let mut t = vm();
    // array_merge_recursive_basic1.phpt
    let arr1 = "[1, [1, 2]]";
    let arr2 = "[3, ['hello', 'world']]";
    let arr3 = "[[6, 7], ['str1', 'str2']]";
    assert_eq!(run(&mut t, "array_merge_recursive", vec![]), "[]");
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![php(arr1)]),
        "[1, [1, 2]]"
    );
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![php(arr1), php(arr2)]),
        "[1, [1, 2], 3, ['hello', 'world']]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php(arr1), php(arr2), php(arr3)]
        ),
        "[1, [1, 2], 3, ['hello', 'world'], [6, 7], ['str1', 'str2']]"
    );
    // array_merge_recursive_basic2.phpt
    let a1 = "[1 => 'one', 2 => [1, 2]]";
    let a2 = "[2 => 'three', 'four' => ['hello', 'world']]";
    let a3 = "[1 => [6, 7], 'four' => ['str1', 'str2']]";
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![php(a1)]),
        "['one', [1, 2]]"
    );
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![php(a1), php(a2)]),
        "[0 => 'one', 1 => [1, 2], 2 => 'three', 'four' => ['hello', 'world']]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php(a1), php(a2), php(a3)]
        ),
        "[0 => 'one', 1 => [1, 2], 2 => 'three', 'four' => ['hello', 'world', 'str1', 'str2'], 3 => [6, 7]]"
    );
}

#[test]
fn array_merge_recursive_scalars_and_nulls() {
    let mut t = vm();
    // Scalars with equal string keys are collected into arrays.
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php("['a' => 1]"), php("['a' => 2]")]
        ),
        "['a' => [1, 2]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php("['a' => [1]]"), php("['a' => 2]")]
        ),
        "['a' => [1, 2]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php("['a' => 1]"), php("['a' => [2, 3]]")]
        ),
        "['a' => [1, 2, 3]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php("['a' => null]"), php("['a' => 'x']")]
        ),
        "['a' => [null, 'x']]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![
                php("['a' => ['b' => 1]]"),
                php("['a' => ['b' => 2, 'c' => 3]]")
            ]
        ),
        "['a' => ['b' => [1, 2], 'c' => 3]]"
    );
}

#[test]
fn array_merge_recursive_detects_recursion() {
    // bug43495.phpt
    let mut t = vm();
    let make = || {
        // $a = ['key1' => ['key2' => []]]; $a['key1']['key2']['key3'] = &$a;
        let var = PhpRef::new(Value::Null);
        let mut key2 = PhpArray::new();
        key2.insert("key3".into(), Value::Ref(var.clone()));
        let mut key1 = PhpArray::new();
        key1.insert("key2".into(), Value::array(key2));
        let mut top = PhpArray::new();
        top.insert("key1".into(), Value::array(key1));
        var.set(Value::array(top));
        var
    };
    let (a, b) = (make(), make());
    let r = t.call("array_merge_recursive", vec![a.get(), b.get()]);
    assert_eq!(thrown(r), ("Error".into(), "Recursion detected".into()));
}

#[test]
fn array_replace_phpt() {
    let mut t = vm();
    let array1 = "[0 => 'dontclobber', '1' => 'unclobbered', 'test2' => 0.0, 'test3' => ['testarray2' => true, 1 => ['testsubarray1' => 'dontclobber2', 'testsubarray2' => 'dontclobber3']]]";
    let array2 = "[1 => 'clobbered', 'test3' => ['testarray2' => false], 'test4' => ['clobbered3' => [0, 1, 2]]]";
    assert_eq!(
        run(&mut t, "array_replace", vec![php(array1), php(array2)]),
        "[0 => 'dontclobber', 1 => 'clobbered', 'test2' => 0.0, 'test3' => ['testarray2' => false], 'test4' => ['clobbered3' => [0, 1, 2]]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_replace_recursive",
            vec![php(array1), php(array2)]
        ),
        "[0 => 'dontclobber', 1 => 'clobbered', 'test2' => 0.0, 'test3' => ['testarray2' => false, 1 => ['testsubarray1' => 'dontclobber2', 'testsubarray2' => 'dontclobber3']], 'test4' => ['clobbered3' => [0, 1, 2]]]"
    );
}

#[test]
fn array_replace_basic() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_replace",
            vec![php("[1, 2, 3]"), php("[1 => 'b']"), php("[3 => 'd']")]
        ),
        "[1, 'b', 3, 'd']"
    );
    assert_eq!(
        run(&mut t, "array_replace", vec![php("['a' => 1]")]),
        "['a' => 1]"
    );
    assert_eq!(
        run_err(&mut t, "array_replace", vec![php("[1]"), s("x")]),
        "TypeError: array_replace(): Argument #2 must be of type array, string given"
    );
    // Later arguments win.
    assert_eq!(
        run(
            &mut t,
            "array_replace_recursive",
            vec![
                php("['a' => [1, 2, 3]]"),
                php("['a' => [1 => 'x']]"),
                php("['a' => [2 => 'y']]")
            ]
        ),
        "['a' => [1, 'x', 'y']]"
    );
    // A non-array replacement overwrites an array.
    assert_eq!(
        run(
            &mut t,
            "array_replace_recursive",
            vec![php("['a' => [1]]"), php("['a' => 'x']")]
        ),
        "['a' => 'x']"
    );
    // An array replaces a scalar.
    assert_eq!(
        run(
            &mut t,
            "array_replace_recursive",
            vec![php("['a' => 'x']"), php("['a' => [1]]")]
        ),
        "['a' => [1]]"
    );
}

#[test]
fn array_replace_recursive_detects_recursion() {
    // array_replace.phpt: endless recursion.
    let mut t = vm();
    let array3 = php("[[[[]]]]");
    // $array4 = []; $array4[] = &$array4;
    let var = PhpRef::new(Value::Null);
    var.set(Value::array(PhpArray::from_values(vec![Value::Ref(
        var.clone(),
    )])));
    let r = t.call("array_replace_recursive", vec![array3, var.get()]);
    assert_eq!(thrown(r), ("Error".into(), "Recursion detected".into()));
}

#[test]
fn array_merge_recursive_next_free_index_bugs() {
    let mut t = vm();
    // bug76505.phpt
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php("['k' => [2 => 100, 98 => 200]]"), php("['k' => [64 => 300]]")]
        ),
        "['k' => [2 => 100, 98 => 200, 99 => 300]]"
    );
    // bug70808.phpt: the next index of the first array's sub-array is kept
    // after an element was unset.
    let mut inner = PhpArray::from_values(vec![i(0), i(1)]);
    inner.remove(&1.into());
    let mut top = PhpArray::new();
    top.insert("key".into(), Value::array(inner));
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![Value::array(top), php("['key' => [2]]")]),
        "['key' => [0 => 0, 2 => 2]]"
    );
}

#[test]
fn replace_and_merge_do_not_leak_references() {
    let mut t = vm();
    // bug72622.phpt: array_walk() leaves a refcount-1 reference in the array.
    let (_, walked) = call_ref(&mut t, "array_walk", php("['foo' => 'bar']"), vec![s("t_zero")]);
    match &walked {
        Value::Array(a) => assert!(matches!(a.get_str(b"foo"), Some(Value::Ref(_)))),
        _ => unreachable!(),
    }
    let r = t
        .call("array_replace_recursive", vec![php("['foo' => 'foo']"), walked.clone()])
        .unwrap();
    match &r {
        Value::Array(a) => assert!(matches!(a.get_str(b"foo"), Some(Value::String(_)))),
        _ => unreachable!(),
    }
    // bug79930.phpt: array_merge_recursive() with such an array.
    let m = t
        .call("array_merge_recursive", vec![php("['foo' => 'a']"), walked])
        .unwrap();
    assert_eq!(show(&m), "['foo' => ['a', 'bar']]");
}

#[test]
fn array_merge_recursive_bug24198_and_bug42177() {
    let mut t = vm();
    // bug24198.phpt: merging an array with itself is not a recursion.
    let c = "['a' => 'aa', 'b' => 'bb']";
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![php(c), php(c)]),
        "['a' => ['aa', 'aa'], 'b' => ['bb', 'bb']]"
    );

    // bug42177.phpt: $a1 = []; $a2 = ['key1' => &$a1];
    // $a1 = array_merge_recursive($a1, $a2); then once more.
    let a1 = PhpRef::new(Value::empty_array());
    let mut a2 = PhpArray::new();
    a2.insert("key1".into(), Value::Ref(a1.clone()));
    let a2 = Value::array(a2);
    let first = t
        .call("array_merge_recursive", vec![a1.get(), a2.clone()])
        .unwrap();
    a1.set(first);
    let second = t.call("array_merge_recursive", vec![a1.get(), a2]);
    assert_eq!(thrown(second), ("Error".into(), "Recursion detected".into()));

    // Plain values in both arrays (the first two blocks of bug42177.phpt).
    assert_eq!(
        run(
            &mut t,
            "array_merge_recursive",
            vec![php("['key1' => 1, 'key3' => 2]"), php("[]")]
        ),
        "['key1' => 1, 'key3' => 2]"
    );
}

#[test]
fn array_merge_recursive_does_not_modify_its_arguments() {
    // bug20381.phpt / bug48854.phpt
    let mut t = vm();
    let a = php("['a1' => 1, 'a2' => [1, 2, 3], 'a3' => ['a' => [10, 20, 30], 'b' => 'b']]");
    let b = php("['a1' => 2, 'a2' => [3, 4, 5], 'a3' => ['c' => 'cc', 'a' => [10, 40]]]");
    let before = (show(&a), show(&b));
    let merged = run(&mut t, "array_merge_recursive", vec![a.clone(), b.clone()]);
    assert_eq!(
        merged,
        "['a1' => [1, 2], 'a2' => [1, 2, 3, 3, 4, 5], 'a3' => ['a' => [10, 20, 30, 10, 40], 'b' => 'b', 'c' => 'cc']]"
    );
    assert_eq!((show(&a), show(&b)), before);

    let array1 = php("['friends' => 5, 'children' => ['dogs' => 0]]");
    let array2 = php("['friends' => 10, 'children' => ['cats' => 5]]");
    let before = (show(&array1), show(&array2));
    assert_eq!(
        run(&mut t, "array_merge_recursive", vec![array1.clone(), array2.clone()]),
        "['friends' => [5, 10], 'children' => ['dogs' => 0, 'cats' => 5]]"
    );
    assert_eq!((show(&array1), show(&array2)), before);

    // array_replace_recursive likewise.
    let before = show(&array1);
    run(&mut t, "array_replace_recursive", vec![array1.clone(), array2]);
    assert_eq!(show(&array1), before);
}
