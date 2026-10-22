//! in_array, array_search, array_keys, array_key_exists, array_key_first/last,
//! array_first/last, array_values, array_count_values, array_column,
//! array_is_list.

use super::*;

fn b(x: bool) -> Value {
    Value::Bool(x)
}

#[test]
fn in_array_loose_and_strict() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "in_array", vec![s("1e1"), php("['10']")]),
        "true"
    );
    // PHP 8: 0 == "abc" is false.
    assert_eq!(run(&mut t, "in_array", vec![s("abc"), php("[0]")]), "false");
    assert_eq!(
        run(&mut t, "in_array", vec![Value::Null, php("[0]")]),
        "true"
    );
    assert_eq!(run(&mut t, "in_array", vec![i(1), php("['1']")]), "true");
    assert_eq!(
        run(&mut t, "in_array", vec![i(1), php("['1']"), b(true)]),
        "false"
    );
    assert_eq!(
        run(&mut t, "in_array", vec![s("1"), php("[1, '1']"), b(true)]),
        "true"
    );
    assert_eq!(
        run(
            &mut t,
            "in_array",
            vec![php("[1, 2]"), php("[[1, 2]]"), b(true)]
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut t,
            "in_array",
            vec![php("[2 => 1]"), php("[[1]]"), b(true)]
        ),
        "false"
    );
    assert_eq!(run(&mut t, "in_array", vec![i(5), php("[]")]), "false");
    assert_eq!(
        run(
            &mut t,
            "in_array",
            vec![Value::Float(1.0), php("[1]"), b(true)]
        ),
        "false"
    );
}

#[test]
fn in_array_through_references() {
    // in_array_with_ref.phpt: references in the haystack are dereferenced.
    let mut t = vm();
    let r = PhpRef::new(i(3));
    let mut arr = php_runtime::PhpArray::new();
    arr.append(Value::Ref(r.clone())).unwrap();
    arr.append(i(4)).unwrap();
    let hay = Value::array(arr);
    assert_eq!(
        run(&mut t, "in_array", vec![i(3), hay.clone(), b(true)]),
        "true"
    );
    assert_eq!(
        run(&mut t, "array_search", vec![i(3), hay.clone(), b(true)]),
        "0"
    );
    assert_eq!(run(&mut t, "array_search", vec![i(4), hay]), "1");
}

#[test]
fn array_search_returns_the_key() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_search",
            vec![s("b"), php("['x' => 'a', 'y' => 'b']")]
        ),
        "'y'"
    );
    assert_eq!(
        run(&mut t, "array_search", vec![s("1"), php("[0, 1]")]),
        "1"
    );
    assert_eq!(
        run(&mut t, "array_search", vec![s("z"), php("['a']")]),
        "false"
    );
    assert_eq!(
        run(&mut t, "array_search", vec![i(0), php("['a', 'b']")]),
        "false"
    );
    assert_eq!(
        run(
            &mut t,
            "array_search",
            vec![i(1), php("[5 => '1', 7 => 1]"), b(true)]
        ),
        "7"
    );
}

#[test]
fn array_keys_phpt() {
    let mut t = vm();
    assert_eq!(run(&mut t, "array_keys", vec![php("[]")]), "[]");
    assert_eq!(
        run(
            &mut t,
            "array_keys",
            vec![php("['a' => 1, 5 => 2, 'c' => 3]")]
        ),
        "['a', 5, 'c']"
    );
    assert_eq!(
        run(&mut t, "array_keys", vec![php("[1, 2, 3]")]),
        "[0, 1, 2]"
    );
    // Filter value (loose / strict).
    assert_eq!(
        run(
            &mut t,
            "array_keys",
            vec![php("['a' => 1, 'b' => 2, 'c' => 1]"), i(1)]
        ),
        "['a', 'c']"
    );
    assert_eq!(
        run(&mut t, "array_keys", vec![php("[1, '1', true, 1.0]"), i(1)]),
        "[0, 1, 2, 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_keys",
            vec![php("[1, '1', true, 1.0]"), i(1), b(true)]
        ),
        "[0]"
    );
    // An explicit null filter value is a filter value too.
    assert_eq!(
        run(
            &mut t,
            "array_keys",
            vec![php("[null, 0, '', 1]"), Value::Null]
        ),
        "[0, 1, 2]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_keys",
            vec![php("[null, 0, '', 1]"), Value::Null, b(true)]
        ),
        "[0]"
    );
}

#[test]
fn array_key_exists_phpt() {
    let mut t = vm();
    let a = php("[1 => 'a', 'x' => null, '' => 1]");
    assert_eq!(
        run(&mut t, "array_key_exists", vec![i(1), a.clone()]),
        "true"
    );
    assert_eq!(
        run(&mut t, "array_key_exists", vec![s("1"), a.clone()]),
        "true"
    );
    assert_eq!(
        run(&mut t, "array_key_exists", vec![s("x"), a.clone()]),
        "true"
    );
    assert_eq!(
        run(&mut t, "array_key_exists", vec![s("y"), a.clone()]),
        "false"
    );
    assert_eq!(
        run(&mut t, "array_key_exists", vec![b(true), a.clone()]),
        "true"
    );
    assert_eq!(
        run(&mut t, "array_key_exists", vec![b(false), a.clone()]),
        "false"
    );
    assert_eq!(run(&mut t, "key_exists", vec![i(1), a.clone()]), "true");
    // null is the empty string, with a deprecation.
    t.output();
    assert_eq!(
        run(&mut t, "array_key_exists", vec![Value::Null, a.clone()]),
        "true"
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Using null as the key parameter for array_key_exists() is deprecated, use an empty string instead in /t.php on line 3\n"
    );
    // Floats are truncated, with a deprecation for fractions.
    assert_eq!(
        run(
            &mut t,
            "array_key_exists",
            vec![Value::Float(1.0), a.clone()]
        ),
        "true"
    );
    assert_eq!(t.output(), "");
    assert_eq!(
        run(
            &mut t,
            "array_key_exists",
            vec![Value::Float(1.5), a.clone()]
        ),
        "true"
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float 1.5 to int loses precision in /t.php on line 3\n"
    );
    // Illegal offset types.
    assert_eq!(
        run_err(&mut t, "array_key_exists", vec![php("[1]"), a.clone()]),
        "TypeError: array_key_exists(): Argument #1 ($key) must be a valid array offset type"
    );
    // The array argument must be an array (objects are rejected since PHP 8).
    assert_eq!(
        run_err(&mut t, "array_key_exists", vec![s("a"), s("x")]),
        "TypeError: array_key_exists(): Argument #2 ($array) must be of type array, string given"
    );
}

#[test]
fn array_key_first_last_and_first_last() {
    let mut t = vm();
    assert_eq!(
        run(&mut t, "array_key_first", vec![php("['a' => 1, 'b' => 2]")]),
        "'a'"
    );
    assert_eq!(
        run(&mut t, "array_key_last", vec![php("['a' => 1, 'b' => 2]")]),
        "'b'"
    );
    assert_eq!(
        run(&mut t, "array_key_first", vec![php("[5 => 1, 6 => 2]")]),
        "5"
    );
    assert_eq!(
        run(&mut t, "array_key_last", vec![php("[5 => 1, 6 => 2]")]),
        "6"
    );
    assert_eq!(run(&mut t, "array_key_first", vec![php("[]")]), "null");
    assert_eq!(run(&mut t, "array_key_last", vec![php("[]")]), "null");
    // array_first_last.phpt
    assert_eq!(
        run(&mut t, "array_first", vec![php("[3 => 'x', 'k' => 'y']")]),
        "'x'"
    );
    assert_eq!(
        run(&mut t, "array_last", vec![php("[3 => 'x', 'k' => 'y']")]),
        "'y'"
    );
    assert_eq!(run(&mut t, "array_first", vec![php("[]")]), "null");
    assert_eq!(run(&mut t, "array_last", vec![php("[]")]), "null");
    assert_eq!(run(&mut t, "array_first", vec![php("[null, 1]")]), "null");
}

#[test]
fn array_values_phpt() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_values",
            vec![php("['a' => 1, 5 => 2, 'c' => 3]")]
        ),
        "[1, 2, 3]"
    );
    assert_eq!(run(&mut t, "array_values", vec![php("[]")]), "[]");
    assert_eq!(
        run(&mut t, "array_values", vec![php("[3 => 'a', 1 => 'b']")]),
        "['a', 'b']"
    );
    // A vector-like packed array is returned as is.
    let v = php("[1, 2, 3]");
    let r = t.call("array_values", vec![v.clone()]).unwrap();
    match (&v, &r) {
        (Value::Array(a), Value::Array(b)) => assert!(std::rc::Rc::ptr_eq(a, b)),
        _ => unreachable!(),
    }
}

#[test]
fn array_count_values_phpt() {
    let mut t = vm();
    assert_eq!(
        run(
            &mut t,
            "array_count_values",
            vec![php("[1, 'hello', 1, 'world', 'hello']")]
        ),
        "[1 => 2, 'hello' => 2, 'world' => 1]"
    );
    // Numeric strings and ints share keys.
    assert_eq!(
        run(&mut t, "array_count_values", vec![php("['1', 1, '01']")]),
        "[1 => 2, '01' => 1]"
    );
    t.output();
    assert_eq!(
        run(
            &mut t,
            "array_count_values",
            vec![php("[1.5, 'a', true, null, [1]]")]
        ),
        "['a' => 1]"
    );
    let out = t.output();
    assert_eq!(
        out.matches("Warning: array_count_values(): Can only count string and integer values, entry skipped in /t.php on line 3").count(),
        4
    );
}

#[test]
fn array_column_phpt() {
    let mut t = vm();
    let records = "[['id' => 1, 'first_name' => 'John', 'last_name' => 'Doe'], ['id' => 2, 'first_name' => 'Sally', 'last_name' => 'Smith'], ['id' => 3, 'first_name' => 'Jane', 'last_name' => 'Jones']]";
    assert_eq!(
        run(&mut t, "array_column", vec![php(records), s("first_name")]),
        "['John', 'Sally', 'Jane']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(records), s("id")]),
        "[1, 2, 3]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_column",
            vec![php(records), s("last_name"), s("id")]
        ),
        "[1 => 'Doe', 2 => 'Smith', 3 => 'Jones']"
    );
    assert_eq!(
        run(
            &mut t,
            "array_column",
            vec![php(records), s("last_name"), s("first_name")]
        ),
        "['John' => 'Doe', 'Sally' => 'Smith', 'Jane' => 'Jones']"
    );
    // A null column returns whole rows, optionally re-keyed.
    assert_eq!(
        run(
            &mut t,
            "array_column",
            vec![php("[['a' => 1], ['a' => 2]]"), Value::Null, s("a")]
        ),
        "[1 => ['a' => 1], 2 => ['a' => 2]]"
    );
    assert_eq!(
        run(
            &mut t,
            "array_column",
            vec![php("[['a' => 1], ['a' => 2]]"), Value::Null]
        ),
        "[['a' => 1], ['a' => 2]]"
    );
    assert_eq!(run(&mut t, "array_column", vec![php("[]"), s("a")]), "[]");

    // Numeric column keys.
    let num = "[['aaa', '111'], ['bbb', '222'], ['ccc', '333', -1 => 'ddd']]";
    assert_eq!(
        run(&mut t, "array_column", vec![php(num), i(1)]),
        "['111', '222', '333']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(num), i(1), i(0)]),
        "['aaa' => '111', 'bbb' => '222', 'ccc' => '333']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(num), i(1), i(-1)]),
        "[0 => '111', 1 => '222', 'ddd' => '333']"
    );
    // Missing columns.
    assert_eq!(run(&mut t, "array_column", vec![php(num), i(2)]), "[]");
    assert_eq!(run(&mut t, "array_column", vec![php(num), s("foo")]), "[]");
    assert_eq!(
        run(&mut t, "array_column", vec![php(num), i(0), s("foo")]),
        "['aaa', 'bbb', 'ccc']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php("['foo', 'bar']"), i(1)]),
        "[]"
    );
    // Numeric string keys address integer offsets.
    assert_eq!(
        run(&mut t, "array_column", vec![php(num), s("1")]),
        "['111', '222', '333']"
    );

    // Columns not present in all rows.
    let mm = "[['a' => 'foo', 'b' => 'bar', 'e' => 'bbb'], ['a' => 'baz', 'c' => 'qux', 'd' => 'aaa'], ['a' => 'eee', 'b' => 'fff', 'e' => 'ggg']]";
    assert_eq!(
        run(&mut t, "array_column", vec![php(mm), s("c")]),
        "['qux']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(mm), s("c"), s("a")]),
        "['baz' => 'qux']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(mm), s("a"), s("d")]),
        "[0 => 'foo', 'aaa' => 'baz', 1 => 'eee']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(mm), s("a"), s("e")]),
        "['bbb' => 'foo', 0 => 'baz', 'ggg' => 'eee']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(mm), s("b")]),
        "['bar', 'fff']"
    );
    assert_eq!(
        run(&mut t, "array_column", vec![php(mm), s("b"), s("a")]),
        "['foo' => 'bar', 'eee' => 'fff']"
    );
}

#[test]
fn array_column_index_key_conversions() {
    // array_column_variant.phpt style: the index value is used as an array key.
    let mut t = vm();
    let rows = "[['k' => true, 'v' => 'a'], ['k' => null, 'v' => 'b'], ['k' => 2.0, 'v' => 'c'], ['k' => '7', 'v' => 'd']]";
    assert_eq!(
        run(&mut t, "array_column", vec![php(rows), s("v"), s("k")]),
        "[1 => 'a', '' => 'b', 2 => 'c', 7 => 'd']"
    );
}

#[test]
fn array_is_list_phpt() {
    let mut t = vm();
    assert_eq!(run(&mut t, "array_is_list", vec![php("[]")]), "true");
    assert_eq!(run(&mut t, "array_is_list", vec![php("[1, 2]")]), "true");
    assert_eq!(run(&mut t, "array_is_list", vec![php("[1 => 1]")]), "false");
    assert_eq!(
        run(&mut t, "array_is_list", vec![php("[1 => 'a', 0 => 'b']")]),
        "false"
    );
    assert_eq!(
        run(&mut t, "array_is_list", vec![php("['a' => 1]")]),
        "false"
    );
}

#[test]
fn constants_are_registered() {
    let t = vm();
    for (name, value) in [
        ("COUNT_NORMAL", 0),
        ("COUNT_RECURSIVE", 1),
        ("CASE_LOWER", 0),
        ("CASE_UPPER", 1),
        ("SORT_REGULAR", 0),
        ("SORT_NUMERIC", 1),
        ("SORT_STRING", 2),
        ("SORT_DESC", 3),
        ("SORT_ASC", 4),
        ("SORT_LOCALE_STRING", 5),
        ("SORT_NATURAL", 6),
        ("SORT_FLAG_CASE", 8),
        ("EXTR_OVERWRITE", 0),
        ("EXTR_SKIP", 1),
        ("EXTR_PREFIX_SAME", 2),
        ("EXTR_PREFIX_ALL", 3),
        ("EXTR_PREFIX_INVALID", 4),
        ("EXTR_PREFIX_IF_EXISTS", 5),
        ("EXTR_IF_EXISTS", 6),
        ("EXTR_REFS", 0x100),
        ("ARRAY_FILTER_USE_VALUE", 0),
        ("ARRAY_FILTER_USE_BOTH", 1),
        ("ARRAY_FILTER_USE_KEY", 2),
    ] {
        assert_eq!(
            t.vm.constant(name).and_then(|v| v.as_int()),
            Some(value),
            "{name}"
        );
    }
}

#[test]
fn sort_locale_string_is_deprecated() {
    let mut t = vm();
    t.output();
    let v = t.vm.fetch_constant("SORT_LOCALE_STRING").ok().flatten();
    assert_eq!(v.and_then(|v| v.as_int()), Some(5));
    let out = t.output();
    assert!(
        out.contains("Constant SORT_LOCALE_STRING is deprecated")
            && out.contains("since 8.6, use one of the Collator::*sort*() methods instead"),
        "{out:?}"
    );
}

#[test]
fn every_array_function_is_registered() {
    // The functions of the array.c section of basic_functions.stub.php (minus
    // compact, extract, shuffle and array_rand) plus strnatcmp/strnatcasecmp.
    let t = vm();
    let names = [
        "array_push",
        "krsort",
        "ksort",
        "count",
        "sizeof",
        "natsort",
        "natcasesort",
        "asort",
        "arsort",
        "sort",
        "rsort",
        "usort",
        "uasort",
        "uksort",
        "end",
        "prev",
        "next",
        "reset",
        "current",
        "pos",
        "key",
        "min",
        "max",
        "clamp",
        "array_walk",
        "array_walk_recursive",
        "in_array",
        "array_search",
        "array_fill",
        "array_fill_keys",
        "range",
        "array_pop",
        "array_shift",
        "array_unshift",
        "array_splice",
        "array_slice",
        "array_merge",
        "array_merge_recursive",
        "array_replace",
        "array_replace_recursive",
        "array_keys",
        "array_key_first",
        "array_key_last",
        "array_first",
        "array_last",
        "array_values",
        "array_count_values",
        "array_column",
        "array_reverse",
        "array_pad",
        "array_flip",
        "array_change_key_case",
        "array_unique",
        "array_intersect_key",
        "array_intersect_ukey",
        "array_intersect",
        "array_uintersect",
        "array_intersect_assoc",
        "array_uintersect_assoc",
        "array_intersect_uassoc",
        "array_uintersect_uassoc",
        "array_diff_key",
        "array_diff_ukey",
        "array_diff",
        "array_udiff",
        "array_diff_assoc",
        "array_diff_uassoc",
        "array_udiff_assoc",
        "array_udiff_uassoc",
        "array_multisort",
        "array_sum",
        "array_product",
        "array_reduce",
        "array_filter",
        "array_find",
        "array_find_key",
        "array_any",
        "array_all",
        "array_map",
        "array_key_exists",
        "key_exists",
        "array_chunk",
        "array_combine",
        "array_is_list",
        "strnatcmp",
        "strnatcasecmp",
    ];
    assert_eq!(names.len(), 86);
    for name in names {
        assert!(
            t.vm.find_function(name).is_some(),
            "{name} is not registered"
        );
    }
    for name in ["compact", "extract", "shuffle", "array_rand"] {
        assert!(
            t.vm.find_function(name).is_none(),
            "{name} belongs to another module"
        );
    }
}
