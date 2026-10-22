//! Ported .phpt tests, batch C (`errors04` .. `preg_grep_error_utf8`).
//!
//! Not ported: errors05 and no_jit_bug70110 and grep2's JIT condition (JIT
//! only), gh15205_1 (`RegexIterator`), gh15205_2 (stream wrapper), locales
//! (`setlocale()`), the `oom_*` tests (memory-limit simulation), and the
//! tests already ported in basic.rs (`marks`, `mark_named_collision`,
//! `pcre.constants`).

use super::support::*;
use php_runtime::Value;

#[test]
fn errors04_backtrack_limit_message() {
    let mut p = P::phpt("errors04");
    p.ini("pcre.backtrack_limit", "2");
    let r = p.call(
        "preg_match_all",
        vec![s(r"/.*\p{N}/"), s("0123456789"), rf(&nref())],
    );
    p.dump(&r);
    let m = p.last_error_msg();
    p.dump(&Value::Bool(m == "Backtrack limit exhausted"));
    let r = p.call(
        "preg_match_all",
        vec![s(r"/\p{Nd}/"), s("0123456789"), rf(&nref())],
    );
    p.dump(&r);
    let m = p.last_error_msg();
    p.dump(&Value::Bool(m == "No error"));
    p.finish();
}

#[test]
fn errors06_malformed_utf8_offset() {
    let mut p = P::phpt("errors06");
    p.call(
        "preg_match",
        vec![s("/a/u"), sb(b"\xE3\x82\xA2"), rf(&nref()), int(0), int(1)],
    );
    let e = p.last_error();
    p.dump(&Value::Bool(e == 5));
    let m = p.last_error_msg();
    p.dump(&Value::Bool(
        m == "The offset did not correspond to the beginning of a valid UTF-8 code point",
    ));
    p.finish();
}

#[test]
fn gh10968_callback_array_with_empty_input() {
    let mut p = P::phpt("gh10968");
    let r = p.call(
        "preg_replace_callback_array",
        vec![list(vec![]), list(vec![])],
    );
    p.dump(&r);
    let r = p.call("preg_replace_callback_array", vec![list(vec![]), s("")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn gh11374_result_does_not_depend_on_jit() {
    let mut p = P::phpt("gh11374");
    let regex = "\n    (?<types>\n        (?:\n            (?:\\{ (?&types) \\})\n            | (a)\n        )\n        (\\*?)\n    )\n";
    p.ini("pcre.jit", "0");
    let m = nref();
    p.call(
        "preg_match",
        vec![
            s(&format!("{{^{regex}$}}x")),
            s("{a}"),
            rf(&m),
            int(PREG_OFFSET_CAPTURE),
        ],
    );
    p.ini("pcre.jit", "1");
    // the regex must be different to prevent the regex cache
    let m2 = nref();
    p.call(
        "preg_match",
        vec![
            s(&format!("{{^{regex}$}}xx")),
            s("{a}"),
            rf(&m2),
            int(PREG_OFFSET_CAPTURE),
        ],
    );
    p.dump(&Value::Bool(
        var_dump_bytes(&m.get()) == var_dump_bytes(&m2.get()),
    ));
    p.print_r(&m.get());
    p.finish();
}

#[test]
fn gh11956_jit_result() {
    let mut p = P::phpt("gh11956");
    let (_, m) = p.match_(
        r#"/<(\w+)[\s\w\-]+ id="S44_i89ew">/"#,
        r#"<br><div id="S44_i89ew">"#,
        0,
        0,
    );
    p.dump(&m);
    p.finish();
}

#[test]
fn gh16184_replace_with_array_patterns() {
    let mut p = P::phpt("gh16184");
    let string =
        "This is a string. It contains numbers (0*9) as well as parentheses and some other things!";
    let r = p.call(
        "preg_replace",
        vec![
            strs(&[r"/\b\w{1}s/", r"/(\d{1})*(\d{1})/", r"/[\(!\)]/"]),
            strs(&["test", "$1 to $2", "*"]),
            s(string),
        ],
    );
    p.echo_bytes(r.as_str().unwrap());
    p.echo("\n");
    p.finish();
}

#[test]
fn gh16189_offset_underflow() {
    let mut p = P::phpt("gh16189");
    for name in ["preg_match", "preg_match_all"] {
        p.try_call(
            name,
            vec![
                s(r#"/<(\w+)[\s\w\-]+ id="S44_i89ew">/"#),
                s(r#"<br><div id="S44_i89ew">"#),
                rf(&nref()),
                int(0),
                int(i64::MIN),
            ],
        );
    }
    p.finish();
}

#[test]
fn gh17122_integer_subject() {
    let mut p = P::phpt("gh17122");
    let m1 = nref();
    p.call(
        "preg_match",
        vec![s(r"|(?P<name>)(\d+)|"), int(0xffffffff), rf(&m1)],
    );
    p.dump(&m1.get());
    let m2 = nref();
    p.call(
        "preg_match",
        vec![s(r"|(?P<name2>)(\d+)|"), int(0), rf(&m2)],
    );
    p.dump(&m2.get());
    p.finish();
}

#[test]
fn gh21134_backslash_c_with_utf8() {
    let mut p = P::phpt("gh21134");
    let m = nref();
    let r = p.call("preg_match_all", vec![dq(r"/(.*)\\C/u"), s("à"), rf(&m)]);
    p.dump(&r);
    p.dump(&m.get());
    p.finish();
}

#[test]
fn grep_basic() {
    let mut p = P::phpt("grep");
    let array = strs(&["a", "1", "q6", "h20"]);
    let r = p.call("preg_grep", vec![s(r"/^(\d|.\d)$/"), array.clone()]);
    p.dump(&r);
    let r = p.call(
        "preg_grep",
        vec![s(r"/^(\d|.\d)$/"), array, int(PREG_GREP_INVERT)],
    );
    p.dump(&r);
    p.finish();
}

/// grep2.phpt requires a JIT build (its last part expects the JIT to ignore
/// `pcre.recursion_limit`); this is the JIT-independent beginning.
#[test]
fn grep2_errors_and_keys() {
    let mut p = P::new();
    let r = p.call("preg_grep", vec![s("/+/"), list(vec![])]);
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        p.out(),
        "\nWarning: preg_grep(): Compilation failed: quantifier does not follow a repeatable item at offset 0 in /t.php on line 3\n"
    );

    let array = keyed(vec![
        (php_runtime::ArrayKey::Int(5), s("a")),
        (php_runtime::ArrayKey::from("x"), s("1")),
        (php_runtime::ArrayKey::from("xyz"), s("q6")),
        (php_runtime::ArrayKey::Int(6), s("h20")),
    ]);
    let r = p.call("preg_grep", vec![s("@^[a-z]+@"), array.clone()]);
    assert_eq!(show(&r), r#"[5 => "a", "xyz" => "q6", 6 => "h20"]"#);
    let r = p.call(
        "preg_grep",
        vec![s("@^[a-z]+@"), array, int(PREG_GREP_INVERT)],
    );
    assert_eq!(show(&r), r#"["x" => "1"]"#);
    assert_eq!(p.last_error(), 0);
}

#[test]
fn invalid_utf8_replace() {
    let mut p = P::phpt("invalid_utf8");
    let string = sb(b"search\xe4");
    let r = p.call(
        "preg_replace",
        vec![dq(r"#(&\#x*)([0-9A-F]+);*#iu"), s("$1$2;"), string],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&int(e));
    p.echo("Done\n");
    p.finish();
}

#[test]
fn invalid_utf8_offset() {
    let mut p = P::phpt("invalid_utf8_offset");
    let string = "\u{e9} uma string utf8 bem formada";
    let (r, m) = p.match_("~.*~u", string, 0, 1);
    p.dump(&r);
    p.dump(&m);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 5));
    let (r, m) = p.match_("~.*~u", string, 0, 2);
    p.dump(&r);
    p.dump(&m);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 0));
    p.echo("Done\n");
    p.finish();
}

#[test]
fn match_flags() {
    let mut p = P::phpt("match_flags");
    p.dump_match_all("/(.)x/", "zxax", PREG_PATTERN_ORDER);
    p.dump_match_all("/(.)x/", "zxyx", PREG_SET_ORDER);
    p.dump_match_all("/(.)x/", "zxyx", PREG_OFFSET_CAPTURE);
    p.dump_match_all("/(.)x/", "zxyx", PREG_SET_ORDER | PREG_OFFSET_CAPTURE);
    p.finish();
}

#[test]
fn match_flags2() {
    let mut p = P::phpt("match_flags2");
    let (r, m) = p.match_("/x(.)/", "fjszxax", PREG_OFFSET_CAPTURE, 0);
    p.dump_all(&[r, m]);
    let (r, m) = p.match_("/(.)x/", "fjszxax", PREG_OFFSET_CAPTURE, 4);
    p.dump_all(&[r, m]);
    let (r, m) = p.match_(
        r"/(?P<capt1>.)(x)(?P<letsmix>\S+)/",
        "fjszxax",
        PREG_OFFSET_CAPTURE,
        0,
    );
    p.dump_all(&[r, m]);
    p.finish();
}

#[test]
fn match_flags3() {
    let mut p = P::phpt("match_flags3");
    let r = p.call("preg_match", vec![s(""), s(""), rf(&nref()), int(0xfff)]);
    p.dump(&r);
    let (r, m) = p.match_(r"/\d+/", "123 456 789 012", 0, -8);
    p.dump_all(&[r, m]);
    let (r, m) = p.match_(r"/\d+/", "123 456 789 012", 0, -500);
    p.dump_all(&[r, m]);
    let (r, m) = p.match_all(r"/\d+/", "123 456 789 012", 0, -8);
    p.dump_all(&[r, m]);
    let r = p.call("preg_match", vec![s("/(?P<3>)/"), s("")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn multiline() {
    let mut p = P::phpt("multiline");
    for (pattern, subject) in [
        ("/^.{2,3}$/", "aei\nou"),
        ("/^.{2,3}$/", "aei\nou\n"),
        ("/^.{2,3}$/m", "aei\nou"),
        ("/^.{2,3}$/m", "aei\nou\n"),
    ] {
        let r = p.call("preg_match_all", vec![s(pattern), s(subject), rf(&nref())]);
        p.dump(&r);
    }
    p.echo("done\n");
    p.finish();
}

#[test]
fn null_bytes() {
    let mut p = P::phpt("null_bytes");
    let calls: &[(&str, &str)] = &[
        (r"\0//i", ""),
        (r"/\0/i", ""),
        (r"/\0/i", r"\0"),
        (r"//\0i", ""),
        (r"//i\0", ""),
        (r"/\\\0/i", ""),
        (r"/\\\0/i", r"\\\0"),
        (r"\0[]i", ""),
        (r"[\0]i", ""),
        (r"[\0]i", r"\0"),
        (r"[]\0i", ""),
        (r"[]i\0", ""),
        (r"[\\\0]i", ""),
        (r"[\\\0]i", r"\\\0"),
        (r"/abc\0def/", "abc"),
        (r"/abc\0def/", r"abc\0def"),
        (r"/abc\0def/", r"abc\0fed"),
        (r"[abc\0def]", "abc"),
        (r"[abc\0def]", r"abc\0def"),
        (r"[abc\0def]", r"abc\0fed"),
    ];
    for (pattern, subject) in calls {
        let r = p.call("preg_match", vec![dq(pattern), dq(subject)]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn oss_fuzz_65021() {
    let mut p = P::phpt("oss-fuzz-65021");
    let r = p.call("preg_match", vec![s(">"), s("")]);
    p.dump(&r);
    let r = p.call("preg_match", vec![s(">foo>i"), s("FOO")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn pcre_anchored() {
    let mut p = P::phpt("pcre_anchored");
    p.dump_match(r"/\PN+/", "123abc");
    for pattern in [r"/\P{N}+/A", r"/^\P{N}+/", r"/^\P{N}+/A"] {
        let r = p.call("preg_match", vec![s(pattern), s("123abc")]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn pcre_count() {
    let mut p = P::phpt("pcre_count");
    let count = nref();
    count.set(int(0));
    let regex = sq(r"/(([0-9a-z]+)-([0-9]+))-(([0-9]+)-([0-9]+))/");
    let string = s(
        "1-2-3-4 a-2-3-4 1-a-3-4 1-2-a-4 1-2-3-a a-a-a-a 4-3-2-1 100-200-300-400-500-600-700-800",
    );
    let r = p.call(
        "preg_replace",
        vec![regex, s("xxxx"), string, int(-1), rf(&count)],
    );
    p.dump(&r);
    p.dump(&count.get());

    let string = s("Here must only number like 42 and 13 appear");
    let r = p.call(
        "preg_replace",
        vec![s("/([a-z]+)/"), s("xxxx"), string, int(-1), rf(&count)],
    );
    p.dump(&r);
    p.dump(&count.get());

    let regex = sq(r"~((V(I|1)(4|A)GR(4|A))|(V(I|1)C(0|O)D(I|1)(N|\/\\\/)))~i");
    let string = sq(r"Viagra V14GR4 Vicodin V1C0D1/\/ v1c0d1/|/");
    let r = p.call(
        "preg_replace",
        vec![regex.clone(), s("..."), string.clone(), int(-1), rf(&count)],
    );
    p.dump(&r);
    p.dump(&count.get());

    // no $count argument: the variable stays NULL
    let count = nref();
    let r = p.call("preg_replace", vec![regex, s("..."), string, int(-1)]);
    p.dump(&r);
    p.dump(&count.get());
    p.finish();
}

#[test]
fn pcre_extended() {
    let mut p = P::phpt("pcre_extended");
    p.dump_match("/a e i o u/", "aeiou");
    p.dump_match("/a e i o u/x", "aeiou");
    let m = nref();
    let r = p.call(
        "preg_match",
        vec![dq(r"/a e\ni\to\ru/x"), s("aeiou"), rf(&m)],
    );
    p.dump_all(&[r, m.get()]);
    p.finish();
}

#[test]
fn pcre_extra() {
    let mut p = P::phpt("pcre_extra");
    let r = p.call("preg_match", vec![s(r"/\y/"), s(r"\y")]);
    p.dump(&r);
    let r = p.call("preg_match", vec![s(r"/\y/X"), s(r"\y")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn pcre_reentrancy() {
    let mut p = P::phpt("pcre_reentrancy");
    p.echo("Testing nested PCRE calls...\n");
    let cb = p.callback(0, |vm, args| {
        let ch = idx(args.get(0), 0);
        let mut line = b"Outer match: ".to_vec();
        line.extend_from_slice(ch.as_str().unwrap());
        line.push(b'\n');
        vm.echo(&line)?;
        // 1. Nested preg_match
        vm.call_native("preg_match", vec![s("/./"), s("inner"), rf(&nref())])?;
        // 2. Nested preg_replace (string version)
        vm.call_native("preg_replace", vec![s("/n/"), s("N"), s("inner")])?;
        // 3. Nested preg_split
        vm.call_native("preg_split", vec![s("/n/"), s("inner")])?;
        // 4. Nested preg_grep
        vm.call_native("preg_grep", vec![s("/n/"), strs(&["inner"])])?;
        Ok(Value::from(ch.as_str().unwrap().to_ascii_uppercase()))
    });
    let r = p.call("preg_replace_callback", vec![s("/./"), cb, s("abc")]);
    p.dump(&r);

    p.echo("\nTesting deep nesting...\n");
    let c = p.callback(2, |_, _| Ok(s("SUCCESS")));
    let b = p.callback(1, move |vm, _| {
        vm.call_native("preg_replace_callback", vec![s("/c/"), c.clone(), s("c")])
    });
    let a = p.callback(0, move |vm, _| {
        vm.call_native("preg_replace_callback", vec![s("/b/"), b.clone(), s("b")])
    });
    let r = p.call("preg_replace_callback", vec![s("/a/"), a, s("a")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_filter() {
    let mut p = P::phpt("preg_filter");
    let subject = strs(&["1", "a", "2", "b", "3", "A", "B", "4"]);
    let pattern = strs(&[r"/\d/", "/[a-z]/", "/[1a]/"]);
    let replace = strs(&["A:$0", "B:$0", "C:$0"]);
    let r = p.call("preg_filter", vec![pattern, replace, subject]);
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_grep_basic() {
    let mut p = P::phpt("preg_grep_basic");
    let array = strs(&[
        "HTTP://WWW.EXAMPLE.COM",
        "/index.html",
        "/info/stat/",
        "http://test.uk.com/index/html",
        "/display/dept.php",
    ]);
    p.dump(&array);
    for (pattern, flags) in [
        (r"@^HTTP(.*?)\w{2,}$@i", 0),
        (r"@(/\w+\.*/*)+@", 0),
        (r"@^http://[^w]{3}.*$@i", 0),
        (r"@.*?\.co\.uk$@i", 0),
        (r"@^HTTP(.*?)\w{2,}$@i", PREG_GREP_INVERT),
    ] {
        let r = p.call("preg_grep", vec![s(pattern), array.clone(), int(flags)]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn preg_grep_error1() {
    let mut p = P::phpt("preg_grep_error1");
    let array = list(vec![int(123), s("abc"), s("test")]);
    let std = p.std_object();
    let values = [
        s("abcdef"),
        s("/[a-zA-Z]"),
        s("[a-zA-Z]/"),
        s("/[a-zA-Z]/F"),
        strs(&["[a-z]", "[A-Z]", "[0-9]"]),
        s("/[a-zA-Z]/"),
        std,
    ];
    for value in values {
        if let Some(r) = p.try_call("preg_grep", vec![value, array.clone()]) {
            p.dump(&r);
        }
    }
    p.finish();
}

#[test]
fn preg_grep_error_utf8() {
    let mut p = P::phpt("preg_grep_error_utf8");
    let ff = sb(b"\xff");
    let r = p.call("preg_match", vec![s("/.*/u"), ff.clone()]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    let r = p.call("preg_grep", vec![s("/.*/u"), list(vec![ff.clone()])]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    let r = p.call(
        "preg_grep",
        vec![s("/.*/u"), list(vec![s("foo"), ff.clone(), s("bar")])],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    let r = p.call(
        "preg_grep",
        vec![s("/.*/u"), list(vec![ff]), int(PREG_GREP_INVERT)],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    let r = p.call("preg_grep", vec![s("/.*/u"), strs(&["foo", "bar"])]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 0));
    p.finish();
}
