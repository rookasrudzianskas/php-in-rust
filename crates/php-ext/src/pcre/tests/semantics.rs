//! Behaviour that php_pcre.c defines but the phpt files only touch lightly:
//! replacement-string references, limits and counts, empty matches,
//! callback results, `preg_filter`, `preg_split` limits.

use super::support::*;
use php_runtime::{ArrayKey, Value};

fn replace(p: &mut P, pattern: &str, replacement: &str, subject: &str) -> String {
    let r = p.call("preg_replace", vec![s(pattern), s(replacement), s(subject)]);
    String::from_utf8_lossy(r.as_str().unwrap()).into_owned()
}

#[test]
fn replacement_references() {
    let mut p = P::new();
    // `$n`, `\n` and `${n}`; an unset group is empty.
    assert_eq!(
        replace(&mut p, "/(a)(b)?/", r"[$2|\2|${2}|$1]", "a"),
        "[|||a]"
    );
    assert_eq!(replace(&mut p, "/a/", "$0$0|\\0|${0}", "a"), "aa|a|a");
    // Two digits at most.
    assert_eq!(replace(&mut p, "/(a)/", "$12", "a"), "");
    assert_eq!(replace(&mut p, "/(a)/", "$1234", "a"), "34");
    // Malformed references stay literal.
    assert_eq!(replace(&mut p, "/(a)/", "${", "a"), "${");
    assert_eq!(replace(&mut p, "/(a)/", "${1", "a"), "${1");
    assert_eq!(replace(&mut p, "/(a)/", "${1x}", "a"), "${1x}");
    assert_eq!(replace(&mut p, "/(a)/", "$", "a"), "$");
    assert_eq!(replace(&mut p, "/(a)/", "$a", "a"), "$a");
    assert_eq!(replace(&mut p, "/(a)/", "$$1", "a"), "$a");
    assert_eq!(replace(&mut p, "/(a)/", "x\\", "a"), "x\\");
    // A backslash escapes `\` and `$`.
    assert_eq!(replace(&mut p, "/a/", "\\\\", "a"), "\\");
    assert_eq!(replace(&mut p, "/(a)/", "\\$1", "a"), "$1");
    assert_eq!(replace(&mut p, "/(a)/", "\\\\$1", "a"), "\\a");
    assert_eq!(replace(&mut p, "/(a)/", "\\\\\\1", "a"), "\\a");
    assert_eq!(replace(&mut p, "/(a)/", "\\n", "a"), "\\n");
}

#[test]
fn replace_limit_and_count() {
    let mut p = P::new();
    let count = nref();
    let call = |p: &mut P, limit: i64, count: &php_runtime::PhpRef| {
        let r = p.call(
            "preg_replace",
            vec![s("/a/"), s("b"), s("aaa"), int(limit), rf(count)],
        );
        String::from_utf8_lossy(r.as_str().unwrap()).into_owned()
    };
    assert_eq!(call(&mut p, -1, &count), "bbb");
    assert_eq!(count.get().as_int(), Some(3));
    assert_eq!(call(&mut p, 2, &count), "bba");
    assert_eq!(count.get().as_int(), Some(2));
    assert_eq!(call(&mut p, 0, &count), "aaa");
    assert_eq!(count.get().as_int(), Some(0));
    // Any other negative limit is "unlimited" too (it is converted to size_t).
    assert_eq!(call(&mut p, -5, &count), "bbb");
    assert_eq!(count.get().as_int(), Some(3));

    // The count adds up over patterns and subjects.
    let r = p.call(
        "preg_replace",
        vec![
            strs(&["/a/", "/b/"]),
            strs(&["b", "c"]),
            strs(&["ab", "ba", "xx"]),
            int(-1),
            rf(&count),
        ],
    );
    assert_eq!(sh(&r), r#"["cc", "cc", "xx"]"#);
    assert_eq!(count.get().as_int(), Some(6));
}

#[test]
fn replace_with_empty_matches() {
    let mut p = P::new();
    assert_eq!(replace(&mut p, "/x*/", "-", "abc"), "-a-b-c-");
    assert_eq!(replace(&mut p, "/x*/u", "-", "äö"), "-ä-ö-");
    assert_eq!(replace(&mut p, "/a*/", "-", "baac"), "-b--c-");
    assert_eq!(replace(&mut p, "/^/m", "> ", "a\nb"), "> a\n> b");
    // With a limit the empty-match retry stops at the limit.
    let r = p.call("preg_replace", vec![s("/x*/"), s("-"), s("abc"), int(2)]);
    assert_eq!(r.as_str().unwrap(), "-a-bc");
}

#[test]
fn match_all_empty_matches() {
    let mut p = P::new();
    let (r, m) = p.match_all("/a*/", "baac", 0, 0);
    assert_eq!(r.as_int(), Some(4));
    assert_eq!(sh(&m), r#"[["", "aa", "", ""]]"#);
    let (r, m) = p.match_all("/(?=(.))/u", "é!", PREG_OFFSET_CAPTURE, 0);
    assert_eq!(r.as_int(), Some(2));
    assert_eq!(sh(&m), r#"[[["", 0], ["", 2]], [["é", 0], ["!", 2]]]"#);
}

#[test]
fn trailing_unmatched_groups_are_trimmed() {
    let mut p = P::new();
    let (_, m) = p.match_("/(a)(b)?(c)?/", "a", 0, 0);
    assert_eq!(sh(&m), r#"["a", "a"]"#);
    let (_, m) = p.match_("/(a)(b)?(c)?/", "ac", 0, 0);
    assert_eq!(sh(&m), r#"["ac", "a", "", "c"]"#);
    let (_, m) = p.match_("/(a)(b)?(c)?/", "a", PREG_UNMATCHED_AS_NULL, 0);
    assert_eq!(sh(&m), r#"["a", "a", null, null]"#);
    let (_, m) = p.match_("/(a)(b)?(c)?/", "a", PREG_OFFSET_CAPTURE, 0);
    assert_eq!(sh(&m), r#"[["a", 0], ["a", 0]]"#);
    // PREG_PATTERN_ORDER pads every group.
    let (_, m) = p.match_all("/(a)(b)?/", "a", 0, 0);
    assert_eq!(sh(&m), r#"[["a"], ["a"], [""]]"#);
    let (_, m) = p.match_all("/(a)(b)?/", "a", PREG_SET_ORDER, 0);
    assert_eq!(sh(&m), r#"[["a", "a"]]"#);
}

#[test]
fn named_groups_keep_their_position() {
    let mut p = P::new();
    let (_, m) = p.match_("/(?<x>a)(b)(?<y>c)?/", "ab", 0, 0);
    assert_eq!(sh(&m), r#"[0 => "ab", "x" => "a", 1 => "a", 2 => "b"]"#);
    let (_, m) = p.match_all("/(?<x>a)(b)/", "abab", PREG_SET_ORDER, 0);
    assert_eq!(
        sh(&m),
        r#"[[0 => "ab", "x" => "a", 1 => "a", 2 => "b"], [0 => "ab", "x" => "a", 1 => "a", 2 => "b"]]"#
    );
    let (_, m) = p.match_all("/(?<x>a)(b)/", "ab", 0, 0);
    assert_eq!(
        sh(&m),
        r#"[0 => ["ab"], "x" => ["a"], 1 => ["a"], 2 => ["b"]]"#
    );
}

#[test]
fn callback_results_are_converted_to_strings() {
    let mut p = P::new();
    let r = p.call(
        "preg_replace_callback",
        vec![s("/a/"), s("cb_ret_int"), s("aa")],
    );
    assert_eq!(r.as_str().unwrap(), "4242");

    let r = p.call(
        "preg_replace_callback",
        vec![s("/a/"), s("cb_ret_array"), s("a")],
    );
    assert_eq!(r.as_str().unwrap(), "Array");
    assert_eq!(
        p.out(),
        "\nWarning: Array to string conversion in /t.php on line 3\n"
    );

    let cb = p.callback(0, |_, _| Ok(Value::Null));
    let r = p.call("preg_replace_callback", vec![s("/a/"), cb, s("xax")]);
    assert_eq!(r.as_str().unwrap(), "xx");

    // An exception propagates and `preg_last_error()` reports an internal error.
    let (class, message) = p.call_err(
        "preg_replace_callback",
        vec![s("/a/"), s("cb_throw"), s("a")],
    );
    assert_eq!(
        (class.as_str(), message.as_str()),
        ("Exception", "callback failed")
    );
    assert_eq!(p.last_error(), 1);
}

#[test]
fn callback_count_and_array_subjects() {
    let mut p = P::new();
    let count = nref();
    let subject = keyed(vec![
        (ArrayKey::from("k"), s("ab")),
        (ArrayKey::Int(7), s("b")),
        (ArrayKey::Int(8), s("aa")),
    ]);
    let r = p.call(
        "preg_replace_callback",
        vec![s("/a/"), s("cb_bracket"), subject, int(2), rf(&count)],
    );
    // The limit applies to each subject separately.
    assert_eq!(sh(&r), r#"["k" => "[a]b", 7 => "b", 8 => "[a][a]"]"#);
    assert_eq!(count.get().as_int(), Some(3));
}

#[test]
fn preg_filter_semantics() {
    let mut p = P::new();
    let r = p.call("preg_filter", vec![s("/a/"), s("b"), s("xyz")]);
    assert!(matches!(r, Value::Null));
    let r = p.call("preg_filter", vec![s("/a/"), s("b"), s("xaz")]);
    assert_eq!(r.as_str().unwrap(), "xbz");
    let subject = keyed(vec![
        (ArrayKey::from("one"), s("a")),
        (ArrayKey::from("two"), s("x")),
        (ArrayKey::Int(3), s("ba")),
    ]);
    let count = nref();
    let r = p.call(
        "preg_filter",
        vec![s("/a/"), s("A"), subject, int(-1), rf(&count)],
    );
    assert_eq!(sh(&r), r#"["one" => "A", 3 => "bA"]"#);
    assert_eq!(count.get().as_int(), Some(2));
    // Without a match in an array every pattern still has to match once.
    let r = p.call("preg_filter", vec![s("/a/"), s("b"), strs(&["x", "y"])]);
    assert_eq!(sh(&r), "[]");
}

#[test]
fn replace_skips_subjects_that_fail() {
    let mut p = P::new();
    p.ini("pcre.backtrack_limit", "100");
    // The last subject exhausts the backtrack limit (`preg_last_error()` only
    // reflects the last one), the first does not.
    let pattern = r"/(?:\D+|<\d+>)*[!?]/";
    let long = "foobar foobar foobar";
    let r = p.call(
        "preg_replace",
        vec![s(pattern), s("X"), list(vec![s("!"), s(long)])],
    );
    assert_eq!(sh(&r), r#"["X"]"#);
    assert_eq!(p.last_error(), 2);

    // A single failing subject gives null.
    let r = p.call("preg_replace", vec![s(pattern), s("X"), s(long)]);
    assert!(matches!(r, Value::Null));
    assert_eq!(p.last_error_msg(), "Backtrack limit exhausted");
}

#[test]
fn split_limits() {
    let mut p = P::new();
    let split = |p: &mut P, limit: i64, flags: i64| {
        let r = p.call(
            "preg_split",
            vec![s("/,/"), s(",a,,b"), int(limit), int(flags)],
        );
        sh(&r)
    };
    assert_eq!(split(&mut p, -1, 0), r#"["", "a", "", "b"]"#);
    assert_eq!(split(&mut p, 0, 0), r#"["", "a", "", "b"]"#);
    assert_eq!(split(&mut p, 1, 0), r#"[",a,,b"]"#);
    assert_eq!(split(&mut p, -2, 0), r#"[",a,,b"]"#);
    assert_eq!(split(&mut p, 2, 0), r#"["", "a,,b"]"#);
    assert_eq!(split(&mut p, 3, 0), r#"["", "a", ",b"]"#);
    // Only non-empty pieces count against the limit.
    assert_eq!(split(&mut p, -1, PREG_SPLIT_NO_EMPTY), r#"["a", "b"]"#);
    assert_eq!(split(&mut p, 2, PREG_SPLIT_NO_EMPTY), r#"["a", ",b"]"#);
    assert_eq!(split(&mut p, 1, PREG_SPLIT_NO_EMPTY), r#"[",a,,b"]"#);
}

#[test]
fn split_delimiters_and_offsets() {
    let mut p = P::new();
    let r = p.call(
        "preg_split",
        vec![
            s(r"/(-)|(\+)/"),
            s("a-b+c"),
            int(-1),
            int(PREG_SPLIT_DELIM_CAPTURE | PREG_SPLIT_OFFSET_CAPTURE),
        ],
    );
    // An unset group before a set one is reported as an empty string with
    // offset -1; trailing unset groups are not reported at all.
    assert_eq!(
        sh(&r),
        r#"[["a", 0], ["-", 1], ["b", 2], ["", -1], ["+", 3], ["c", 4]]"#
    );
    let r = p.call(
        "preg_split",
        vec![
            s(r"/(-)|(\+)/"),
            s("a-b+c"),
            int(-1),
            int(PREG_SPLIT_DELIM_CAPTURE | PREG_SPLIT_NO_EMPTY),
        ],
    );
    assert_eq!(sh(&r), r#"["a", "-", "b", "+", "c"]"#);
    // UTF-8: empty matches advance by whole characters.
    let r = p.call(
        "preg_split",
        vec![s("//u"), s("aé"), int(-1), int(PREG_SPLIT_NO_EMPTY)],
    );
    assert_eq!(sh(&r), r#"["a", "é"]"#);
    let r = p.call("preg_split", vec![s("//"), s("abc")]);
    assert_eq!(sh(&r), r#"["", "a", "b", "c", ""]"#);
}

#[test]
fn pattern_cache_distinguishes_modifiers() {
    let mut p = P::new();
    for _ in 0..3 {
        let r = p.call("preg_match", vec![s("/a/"), s("A")]);
        assert_eq!(r.as_int(), Some(0));
        let r = p.call("preg_match", vec![s("/a/i"), s("A")]);
        assert_eq!(r.as_int(), Some(1));
        let r = p.call("preg_match", vec![s(" /a/ "), s("a")]);
        assert_eq!(r.as_int(), Some(1));
    }
}

#[test]
fn utf8_validation_is_per_call_for_unknown_subjects() {
    let mut p = P::new();
    // A valid subject is remembered; a different (invalid) one is checked.
    let valid = s("héllo");
    for _ in 0..2 {
        let r = p.call("preg_match", vec![s("/l+/u"), valid.clone()]);
        assert_eq!(r.as_int(), Some(1));
    }
    let r = p.call("preg_match", vec![s("/l+/u"), sb(b"h\xe9llo")]);
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(p.last_error(), 4);
    // The remembered subject is still fine, even starting inside a character.
    let (r, _) = p.match_("/l+/u", "héllo", 0, 0);
    assert_eq!(r.as_int(), Some(1));
    let r = p.call(
        "preg_match",
        vec![s("/l+/u"), valid, rf(&nref()), int(0), int(2)],
    );
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(p.last_error(), 5);
}

#[test]
fn match_offset_and_anchors() {
    let mut p = P::new();
    // The offset is not the start of the subject for lookbehinds and ^.
    let (r, _) = p.match_("/^b/", "ab", 0, 1);
    assert_eq!(r.as_int(), Some(0));
    let (r, m) = p.match_(r"/(?<=a)b/", "ab", PREG_OFFSET_CAPTURE, 1);
    assert_eq!(r.as_int(), Some(1));
    assert_eq!(sh(&m), r#"[["b", 1]]"#);
    let (r, m) = p.match_(r"/\Gb/", "abb", PREG_OFFSET_CAPTURE, 2);
    assert_eq!(r.as_int(), Some(1));
    assert_eq!(sh(&m), r#"[["b", 2]]"#);
}
