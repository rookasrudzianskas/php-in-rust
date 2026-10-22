//! Pattern errors, warnings and `preg_last_error()` handling.

use super::support::*;
use php_runtime::Value;

#[test]
fn delimiters() {
    // delimiters.phpt
    let mut p = P::new();
    let check = |p: &mut P, re: &[u8], subject: &str, expect: &str, warning: Option<&str>| {
        let r = p.call("preg_match", vec![sb(re), s(subject)]);
        assert_eq!(var_dump(&r), expect, "{re:?}");
        let out = p.out();
        match warning {
            Some(w) => assert_eq!(
                out,
                format!("\nWarning: preg_match(): {w} in /t.php on line 3\n"),
                "{re:?}"
            ),
            None => assert_eq!(out, "", "{re:?}"),
        }
    };
    check(
        &mut p,
        b"",
        "",
        "bool(false)\n",
        Some("Empty regular expression"),
    );
    check(
        &mut p,
        b"      ",
        "",
        "bool(false)\n",
        Some("Empty regular expression"),
    );
    check(&mut p, b"@@", "", "int(1)\n", None);
    check(
        &mut p,
        b"12",
        "",
        "bool(false)\n",
        Some("Delimiter must not be alphanumeric, backslash, or NUL byte"),
    );
    check(&mut p, b"<>", "", "int(1)\n", None);
    check(
        &mut p,
        b"~a",
        "",
        "bool(false)\n",
        Some("No ending delimiter '~' found"),
    );
    check(&mut p, b"@\\@\\@@", "@@", "int(1)\n", None);
    check(
        &mut p,
        b"//z",
        "@@",
        "bool(false)\n",
        Some("Unknown modifier 'z'"),
    );
    check(
        &mut p,
        b"{",
        "",
        "bool(false)\n",
        Some("No ending matching delimiter '}' found"),
    );
    check(
        &mut p,
        b"\0\0",
        "",
        "bool(false)\n",
        Some("Delimiter must not be alphanumeric, backslash, or NUL byte"),
    );
}

#[test]
fn compilation_failure_and_last_error() {
    let mut p = P::new();
    let r = p.call("preg_match", vec![s("/(ab/"), s("x")]);
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        p.out(),
        "\nWarning: preg_match(): Compilation failed: missing closing parenthesis at offset 3 in /t.php on line 3\n"
    );
    assert_eq!(p.last_error(), 1);
    assert_eq!(p.last_error_msg(), "Internal error");

    // A successful call resets the error.
    let r = p.call("preg_match", vec![s("/a/"), s("a")]);
    assert_eq!(var_dump(&r), "int(1)\n");
    assert_eq!(p.last_error(), 0);
    assert_eq!(p.last_error_msg(), "No error");

    let r = p.call("preg_replace", vec![s(""), s("x"), s("y")]);
    assert!(matches!(r, Value::Null));
    assert_eq!(
        p.out(),
        "\nWarning: preg_replace(): Empty regular expression in /t.php on line 3\n"
    );
}

#[test]
fn pattern_must_be_a_string() {
    // preg_match_error1.phpt
    let mut p = P::new();
    for (re, w) in [
        (
            "abcdef",
            "Delimiter must not be alphanumeric, backslash, or NUL byte",
        ),
        ("/[a-zA-Z]", "No ending delimiter '/' found"),
        ("[a-zA-Z]/", "Unknown modifier '/'"),
        ("/[a-zA-Z]/F", "Unknown modifier 'F'"),
    ] {
        let r = p.call("preg_match", vec![s(re), s("this is a test")]);
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            p.out(),
            format!("\nWarning: preg_match(): {w} in /t.php on line 3\n")
        );
    }
    let (c, m) = p.call_err("preg_match", vec![list(vec![s("[a-z]")]), s("x")]);
    assert_eq!(c, "TypeError");
    assert_eq!(
        m,
        "preg_match(): Argument #1 ($pattern) must be of type string, array given"
    );
}

#[test]
fn recursion_limit_in_split() {
    // errors01.phpt
    let mut p = P::new();
    p.ini("pcre.recursion_limit", "1");
    assert_eq!(p.last_error_msg(), "No error");
    p.call("preg_split", vec![s("/(\\d*)/"), s("ab2c3u")]);
    assert_eq!(p.last_error_msg(), "Recursion limit exhausted");
}

#[test]
fn flags_validation() {
    let mut p = P::new();
    // preg_match: any order bits are invalid; the by-ref array is reset first.
    let m = nref();
    let (c, msg) = p.call_err(
        "preg_match",
        vec![s("/a/"), s("a"), Value::Ref(m.clone()), int(1)],
    );
    assert_eq!(c, "ValueError");
    assert_eq!(
        msg,
        "preg_match(): Argument #4 ($flags) must be a PREG_* constant"
    );
    assert_eq!(var_dump(&m.get()), "array(0) {\n}\n");
    let (c, msg) = p.call_err(
        "preg_match_all",
        vec![s("/a/"), s("a"), Value::Ref(nref()), int(3)],
    );
    assert_eq!(c, "ValueError");
    assert_eq!(
        msg,
        "preg_match_all(): Argument #4 ($flags) must be a PREG_* constant"
    );
    let (c, msg) = p.call_err(
        "preg_match",
        vec![s("/a/"), s("a"), Value::Ref(nref()), int(0), int(i64::MIN)],
    );
    assert_eq!(c, "ValueError");
    assert_eq!(
        msg,
        "preg_match(): Argument #5 ($offset) must be greater than -9223372036854775808"
    );
}

#[test]
fn offset_past_the_end() {
    let mut p = P::new();
    let (r, m) = p.match_("/a/", "abc", 0, 4);
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(var_dump(&m), "array(0) {\n}\n");
    assert_eq!(p.last_error(), 1);
    // A negative offset is clamped to the start.
    let (r, _) = p.match_("/a/", "abc", 0, -10);
    assert_eq!(var_dump(&r), "int(1)\n");
}
