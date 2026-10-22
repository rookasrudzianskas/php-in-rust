//! Smoke tests: constants, ini entries, quoting, marks, named groups.

use super::support::*;
use php_runtime::Value;

#[test]
fn constants() {
    // pcre.constants.phpt
    let mut p = P::new();
    let vm = p.vm();
    let get = |vm: &mut php_runtime::Vm, n: &str| vm.constant(n).unwrap();
    assert_eq!(get(vm, "PREG_PATTERN_ORDER").as_int(), Some(1));
    assert_eq!(get(vm, "PREG_SET_ORDER").as_int(), Some(2));
    assert_eq!(get(vm, "PREG_OFFSET_CAPTURE").as_int(), Some(256));
    assert_eq!(get(vm, "PREG_UNMATCHED_AS_NULL").as_int(), Some(512));
    assert_eq!(get(vm, "PREG_SPLIT_NO_EMPTY").as_int(), Some(1));
    assert_eq!(get(vm, "PREG_SPLIT_DELIM_CAPTURE").as_int(), Some(2));
    assert_eq!(get(vm, "PREG_SPLIT_OFFSET_CAPTURE").as_int(), Some(4));
    assert_eq!(get(vm, "PREG_GREP_INVERT").as_int(), Some(1));
    assert_eq!(get(vm, "PREG_NO_ERROR").as_int(), Some(0));
    assert_eq!(get(vm, "PREG_INTERNAL_ERROR").as_int(), Some(1));
    assert_eq!(get(vm, "PREG_BACKTRACK_LIMIT_ERROR").as_int(), Some(2));
    assert_eq!(get(vm, "PREG_RECURSION_LIMIT_ERROR").as_int(), Some(3));
    assert_eq!(get(vm, "PREG_BAD_UTF8_ERROR").as_int(), Some(4));
    assert_eq!(get(vm, "PREG_BAD_UTF8_OFFSET_ERROR").as_int(), Some(5));
    assert_eq!(get(vm, "PREG_JIT_STACKLIMIT_ERROR").as_int(), Some(6));
    assert_eq!(
        get(vm, "PCRE_VERSION").as_str().unwrap(),
        "10.44 2024-06-07"
    );
    assert_eq!(get(vm, "PCRE_VERSION_MAJOR").as_int(), Some(10));
    assert_eq!(get(vm, "PCRE_VERSION_MINOR").as_int(), Some(44));
    assert!(matches!(get(vm, "PCRE_JIT_SUPPORT"), Value::Bool(false)));
}

#[test]
fn ini_entries() {
    let mut p = P::new();
    let vm = p.vm();
    assert_eq!(vm.ini_get("pcre.backtrack_limit").unwrap(), "1000000");
    assert_eq!(vm.ini_get("pcre.recursion_limit").unwrap(), "100000");
    assert_eq!(vm.ini_get("pcre.jit").unwrap(), "1");
    assert!(vm.ini_set("pcre.backtrack_limit", "5").is_some());
    assert_eq!(vm.ini_int("pcre.backtrack_limit"), 5);
}

#[test]
fn quote() {
    // preg_quote_basic.phpt (excerpt)
    let mut p = P::new();
    let r = p.call("preg_quote", vec![s("Hello.World?")]);
    assert_eq!(show(&r), "\"Hello\\\\.World\\\\?\"");
    let r = p.call("preg_quote", vec![s("a/b#c"), s("/")]);
    assert_eq!(r.as_str().unwrap(), "a\\/b\\#c");
    let r = p.call("preg_quote", vec![sb(b"a\0b")]);
    assert_eq!(r.as_str().unwrap().as_bytes(), b"a\\000b");
    let r = p.call("preg_quote", vec![s("")]);
    assert_eq!(r.as_str().unwrap(), "");
    // The delimiter is only the first byte; an empty delimiter quotes nothing.
    let r = p.call("preg_quote", vec![s("a/b"), s("/x")]);
    assert_eq!(r.as_str().unwrap(), "a\\/b");
    let r = p.call("preg_quote", vec![s("a/b"), s("")]);
    assert_eq!(r.as_str().unwrap(), "a/b");
}

#[test]
fn marks() {
    // marks.phpt
    let regex =
        "/\n    _ (a) (*MARK:A_MARK) _\n  | _ (b) _\n  | _ (c) (*MARK:C_MARK) _\n  | _ (d) _\n/x";
    let mut p = P::new();

    let (r, m) = p.match_(regex, "_c_", 0, 0);
    assert_eq!(var_dump(&r), "int(1)\n");
    assert_eq!(
        var_dump(&m),
        "array(5) {\n  [0]=>\n  string(3) \"_c_\"\n  [1]=>\n  string(0) \"\"\n  [2]=>\n  string(0) \"\"\n  [3]=>\n  string(1) \"c\"\n  [\"MARK\"]=>\n  string(6) \"C_MARK\"\n}\n"
    );

    let (r, m) = p.match_all(regex, "_a__b__c__d_", 1, 0);
    assert_eq!(var_dump(&r), "int(4)\n");
    assert_eq!(
        show(&m),
        concat!(
            "[0 => [0 => \"_a_\", 1 => \"_b_\", 2 => \"_c_\", 3 => \"_d_\"], ",
            "1 => [0 => \"a\", 1 => \"\", 2 => \"\", 3 => \"\"], ",
            "2 => [0 => \"\", 1 => \"b\", 2 => \"\", 3 => \"\"], ",
            "3 => [0 => \"\", 1 => \"\", 2 => \"c\", 3 => \"\"], ",
            "4 => [0 => \"\", 1 => \"\", 2 => \"\", 3 => \"d\"], ",
            "\"MARK\" => [0 => \"A_MARK\", 2 => \"C_MARK\"]]"
        )
    );

    let (r, m) = p.match_all(regex, "_a__b__c__d_", 2, 0);
    assert_eq!(var_dump(&r), "int(4)\n");
    assert_eq!(
        show(&m),
        concat!(
            "[0 => [0 => \"_a_\", 1 => \"a\", \"MARK\" => \"A_MARK\"], ",
            "1 => [0 => \"_b_\", 1 => \"\", 2 => \"b\"], ",
            "2 => [0 => \"_c_\", 1 => \"\", 2 => \"\", 3 => \"c\", \"MARK\" => \"C_MARK\"], ",
            "3 => [0 => \"_d_\", 1 => \"\", 2 => \"\", 3 => \"\", 4 => \"d\"]]"
        )
    );

    let out = p.call(
        "preg_replace_callback",
        vec![s(regex), s("cb_dump"), s("_a__b__c__d_")],
    );
    assert_eq!(out.as_str().unwrap(), "_a__b__c__d_");
    let printed = p.out();
    assert!(printed.starts_with(
        "array(3) {\n  [0]=>\n  string(3) \"_a_\"\n  [1]=>\n  string(1) \"a\"\n  [\"MARK\"]=>\n  string(6) \"A_MARK\"\n}\n"
    ));
    assert_eq!(printed.matches("array(").count(), 4);
}

#[test]
fn mark_named_collision() {
    // mark_named_collision.phpt
    let mut p = P::new();
    let (_, m) = p.match_("/(?P<MARK>[abc])(*MARK:value)/", "abc", 0, 0);
    assert_eq!(show(&m), "[0 => \"a\", \"MARK\" => \"value\", 1 => \"a\"]");
}
