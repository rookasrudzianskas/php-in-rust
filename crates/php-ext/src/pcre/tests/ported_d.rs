//! Ported .phpt tests, batch D (`preg_match_all_basic` .. `ungreedy`).
//!
//! Not ported: preg_match_error3 (JIT stack limit),
//! preg_match_frameless_leak (objects with `__toString`), the trampoline
//! tests, preg_replace_callback_fatal_error_leak (redeclaring a function),
//! preg_replace_callback_matches_refcount (`debug_zval_dump()`), and
//! preg_match_all_error3 / preg_replace_callback_array_fatal_error /
//! preg_replace_callback_array_numeric_index_error, whose uncaught errors
//! are checked by message instead (see `uncaught_errors`).

use super::phpt::{between, file_section};
use super::support::*;
use php_runtime::{Args, NativeFunction, PResult, PhpRef, Value, Vm};

/// `var_dump(preg_match_all($regex, $subject, $m, $flags, $offset))` followed
/// by `var_dump($m)`.
fn match_all_dump(p: &mut P, regex: &Value, subject: &Value, flags: i64, offset: i64) {
    let m = nref();
    let r = p.call(
        "preg_match_all",
        vec![
            regex.clone(),
            subject.clone(),
            rf(&m),
            int(flags),
            int(offset),
        ],
    );
    p.dump_all(&[r, m.get()]);
}

fn match_dump(p: &mut P, regex: &Value, subject: &Value, flags: i64, offset: i64) {
    let m = nref();
    let r = p.call(
        "preg_match",
        vec![
            regex.clone(),
            subject.clone(),
            rf(&m),
            int(flags),
            int(offset),
        ],
    );
    p.dump_all(&[r, m.get()]);
}

#[test]
fn preg_match_all_basic() {
    let mut p = P::phpt("preg_match_all_basic");
    let string = sq(r"Hello, world! This is a test. This is another test. \[4]. 34534 string.");
    match_all_dump(
        &mut p,
        &s("/[0-35-9]/"),
        &string,
        PREG_OFFSET_CAPTURE | PREG_PATTERN_ORDER,
        -10,
    );
    match_all_dump(
        &mut p,
        &s(r"/[tT]his is a(.*?)\./"),
        &string,
        PREG_SET_ORDER,
        0,
    );
    match_all_dump(
        &mut p,
        &sq(r"@\. \\\(.*).@"),
        &string,
        PREG_PATTERN_ORDER,
        0,
    );
    match_all_dump(&mut p, &s(r"/\d{2}$/"), &string, 0, 0);
    match_all_dump(&mut p, &s(r"/(This is a ){2}(.*)\stest/"), &string, 0, 0);
    // Test not passing in a subpatterns array.
    for pattern in [
        sq("/test/"),
        sq(r"/this isn\'t in the string/"),
        sq("/world/"),
        sq("/[0-9]/"),
    ] {
        let r = p.call("preg_match_all", vec![pattern, string.clone()]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn preg_match_all_edit_basic() {
    let mut p = P::phpt("preg_match_all_edit_basic");
    let string = sq(r"Hello, world! This is a test. This is another test. \[4]. 34534 string.");
    match_all_dump(
        &mut p,
        &s("/[0-35-9]/"),
        &string,
        PREG_OFFSET_CAPTURE | PREG_PATTERN_ORDER,
        -10,
    );
    match_all_dump(
        &mut p,
        &s(r"/[tT]his is a(.*?)\./"),
        &string,
        PREG_SET_ORDER,
        0,
    );
    match_all_dump(
        &mut p,
        &sq(r"@\. \\\(.*).@"),
        &string,
        PREG_PATTERN_ORDER,
        0,
    );
    match_all_dump(&mut p, &s(r"/\d{2}$/"), &string, 0, 0);
    match_all_dump(&mut p, &s(r"/(This is a ){2}(.*)\stest/"), &string, 0, 0);
    p.finish();
}

#[test]
fn preg_match_all_error1() {
    let mut p = P::phpt("preg_match_all_error1");
    let matches1 = nref();
    let values = [
        s("abcdef"),
        s("/[a-zA-Z]"),
        s("[a-zA-Z]/"),
        s("/[a-zA-Z]/F"),
        strs(&["[a-z]", "[A-Z]", "[0-9]"]),
        s("/[a-zA-Z]/"),
    ];
    for value in values {
        if let Some(r) = p.try_call("preg_match_all", vec![value, s("test"), rf(&matches1)]) {
            p.dump(&r);
        }
        p.dump(&matches1.get());
    }
    let matches = nref();
    let std = p.std_object();
    if let Some(r) = p.try_call("preg_match_all", vec![std, s("test"), rf(&matches)]) {
        p.dump(&r);
    }
    p.dump(&matches.get());
    p.finish();
}

#[test]
fn preg_match_basic() {
    let mut p = P::phpt("preg_match_basic");
    let string = sq(r"Hello, world. [*], this is \ a string");
    match_dump(&mut p, &s(r"/^[hH]ello,\s/"), &string, 0, 0);
    match_dump(
        &mut p,
        &s(r"/l^o,\s\w{5}/"),
        &string,
        PREG_OFFSET_CAPTURE,
        0,
    );
    match_dump(&mut p, &s(r"/\[\*\],\s(.*)/"), &string, 0, 0);
    match_dump(
        &mut p,
        &sq(r"@\w{4}\s\w{2}\s\\\(?:\s.*)@"),
        &string,
        PREG_OFFSET_CAPTURE,
        14,
    );
    match_dump(&mut p, &s("/hello world/"), &string, 0, 0);
    p.finish();
}

#[test]
fn preg_match_basic_edit() {
    let mut p = P::phpt("preg_match_basic_edit");
    let string = sq(r"Hello, world. [*], this is \ a string");
    match_dump(&mut p, &s(r"/^[hH]ello,\s/"), &string, 0, 0);
    match_dump(
        &mut p,
        &s(r"/l^o,\s\w{5}/"),
        &string,
        PREG_OFFSET_CAPTURE,
        0,
    );
    match_dump(&mut p, &s(r"/\[\*\],\s(.*)/"), &string, 0, 0);
    match_dump(
        &mut p,
        &sq(r"@\w{4}\s\w{2}\s\\\(?:\s.*)@"),
        &string,
        PREG_OFFSET_CAPTURE,
        14,
    );
    match_dump(&mut p, &s("/hello world/"), &string, 0, 0);
    p.finish();
}

#[test]
fn preg_match_basic_002() {
    let mut p = P::phpt("preg_match_basic_002");
    let (_, m) = p.match_("/M(.*)/", "My\nName\nIs\nStrange", 0, 0);
    p.dump(&m);
    p.finish();
}

#[test]
fn preg_match_caseless_restrict() {
    // The `r` modifier (PCRE2_EXTRA_CASELESS_RESTRICT, PCRE2 >= 10.43).
    enum Step {
        Echo(&'static str),
        Match(&'static str, &'static str),
    }
    use Step::*;
    let steps = [
        Echo("SK substitute matching\n"),
        Match("/AskZ/iur", "AskZ"),
        Match("/AskZ/iur", "aSKz"),
        Match("/AskZ/iur", "A\u{17f}kZ"),
        Match("/AskZ/iur", "As\u{212a}Z"),
        Match("/AskZ/iu", "AskZ"),
        Match("/AskZ/iu", "aSKz"),
        Match("/AskZ/iu", "A\u{17f}kZ"),
        Match("/AskZ/iu", "As\u{212a}Z"),
        Echo("K substitute matching\n"),
        Match("/k/iu", "\u{212A}"),
        Match("/k/iur", "\u{212A}"),
        Echo("non-ASCII in expressions\n"),
        Match(r"/A\x{17f}\x{212a}Z/iu", "AskZ"),
        Match(r"/A\x{17f}\x{212a}Z/iur", "AskZ"),
        Echo("Character sets\n"),
        Match("/[AskZ]+/iur", "AskZ"),
        Match("/[AskZ]+/iur", "aSKz"),
        Match("/[AskZ]+/iur", "A\u{17f}kZ"),
        Match("/[AskZ]+/iur", "As\u{212a}Z"),
        Match("/[AskZ]+/iu", "AskZ"),
        Match("/[AskZ]+/iu", "aSKz"),
        Match("/[AskZ]+/iu", "A\u{17f}kZ"),
        Match("/[AskZ]+/iu", "As\u{212a}Z"),
        Echo("non-ASCII in character sets\n"),
        Match(r"/[\x{17f}\x{212a}]+/iur", "AskZ"),
        Match(r"/[\x{17f}\x{212a}]+/iu", "AskZ"),
        Echo("Meta characters and negate character sets\n"),
        Match("/[^s]+/iur", "A\u{17f}Z"),
        Match("/[^s]+/iu", "A\u{17f}Z"),
        Match("/[^s]+/iu", "A\u{17f}Z"),
        Match("/[^k]+/iur", "A\u{212a}Z"),
        Match("/[^k]+/iu", "A\u{212a}Z"),
        Match("/[^sk]+/iur", "A\u{17f}\u{212a}Z"),
        Match("/[^sk]+/iu", "A\u{17f}\u{212a}Z"),
        Match(r"/[^\x{17f}]+/iur", "AsSZ"),
        Match(r"/[^\x{17f}]+/iu", "AsSZ"),
        Echo("Modifier used within the expression\n"),
        Match("/s(?r)s(?-r)s(?r:s)s/iu", "\u{17f}S\u{17f}S\u{17f}"),
        Match("/s(?r)s(?-r)s(?r:s)s/iu", "\u{17f}\u{17f}\u{17f}S\u{17f}"),
        Match("/s(?r)s(?-r)s(?r:s)s/iu", "\u{17f}S\u{17f}\u{17f}\u{17f}"),
        Match("/k(?^i)k/iur", "K\u{212a}"),
        Match("/k(?^i)k/iur", "\u{212a}\u{212a}"),
    ];
    let mut p = P::phpt("preg_match_caseless_restrict");
    for step in steps {
        match step {
            Echo(text) => p.echo(text),
            Match(pattern, subject) => {
                let r = p.call("preg_match", vec![s(pattern), s(subject)]);
                p.dump(&r);
            }
        }
    }
    p.echo("Done");
    p.finish();
}

#[test]
fn preg_match_latin() {
    let mut p = P::phpt("preg_match_latin");
    let pattern1 = sq(r"/^[\w\p{Cyrillic}\s\-\']+$/u");
    let pattern2 = sq(r"/^[\w\s\-\']+$/u");
    let (_, test1) = {
        let m = nref();
        let r = p.call("preg_match", vec![pattern1.clone(), s("latin"), rf(&m)]);
        (r, m.get())
    };
    let m = nref();
    p.call("preg_match", vec![pattern1, s("кириллица"), rf(&m)]);
    let test2 = m.get();
    let m = nref();
    p.call("preg_match", vec![pattern2, s("latin"), rf(&m)]);
    let test3 = m.get();
    p.dump(&list(vec![test1, test2, test3]));
    p.echo("===Done===");
    p.finish();
}

#[test]
fn preg_match_non_capture() {
    let mut p = P::phpt("preg_match_non_capture");
    let (_, m) = p.match_("/.(.)./n", "abc", 0, 0);
    p.dump(&m);
    let (_, m) = p.match_("/.(?P<test>.)./n", "abc", 0, 0);
    p.dump(&m);
    p.finish();
}

#[test]
fn preg_match_variation1() {
    let mut p = P::phpt("preg_match_variation1");
    // `$string` is both the subject (by value) and the by-ref `$matches`.
    let string = PhpRef::new(s("-1"));
    p.call(
        "preg_match",
        vec![s(r"/[\-\+]?[0-9\.]*/"), string.get(), rf(&string)],
    );
    p.dump(&string.get());
    p.finish();
}

#[test]
fn preg_quote_basic() {
    let mut p = P::phpt("preg_quote_basic");
    let string_before = sq_bytes(r"/this *-has \ metacharacters^ in $");
    p.echo("$string_before looks like: ");
    p.echo_bytes(&string_before);
    p.echo("\n");
    let after = p.call(
        "preg_quote",
        vec![Value::from(string_before.clone()), s("/")],
    );
    p.echo("$string_after looks like: ");
    p.echo_bytes(after.as_str().unwrap());
    p.echo(", with metacharacters and / (set as delimiter) escaped\n");
    let string1 = sq(r"testing - /this *-has \ metacharacters^ in $ should   work");
    let mut pattern = b"/^[tT]\\w{6} - ".to_vec();
    pattern.extend_from_slice(after.as_str().unwrap());
    pattern.extend_from_slice(br" [a-z]*\s*work$/");
    let m = nref();
    let r = p.call("preg_match", vec![Value::from(pattern), string1, rf(&m)]);
    p.dump_all(&[r, m.get()]);
    p.finish();
}

#[test]
fn preg_replace_basic_cases() {
    let mut p = P::phpt("preg_replace");
    let r = p.call("preg_replace", vec![s(r"{{\D+}}"), s("x"), s("{abcd}")]);
    p.dump(&r);
    let r = p.call("preg_replace", vec![s(r"{{\D+}}"), s("ddd"), s("abcd")]);
    p.dump(&r);
    let r = p.call(
        "preg_replace",
        vec![
            s("/(ab)(c)(d)(e)(f)(g)(h)(i)(j)(k)/"),
            s("a${1}2$103"),
            s("zabcdefghijkl"),
        ],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_replace2_arrays() {
    let mut p = P::phpt("preg_replace2");
    let r = p.call(
        "preg_replace",
        vec![strs(&[r"/\da(.)/ui", "@..@"]), s("$1"), s("12Abc")],
    );
    p.dump(&r);
    let r = p.call(
        "preg_replace",
        vec![
            strs(&[r"/\da(.)/ui", "@(.)@"]),
            s("$1"),
            strs(&["x", "a2aA", "1av2Ab"]),
        ],
    );
    p.dump(&r);
    let r = p.call(
        "preg_replace",
        vec![strs(&[r"/[\w]+/"]), strs(&["$"]), strs(&["xyz", "bdbd"])],
    );
    p.dump(&r);
    let r = p.call(
        "preg_replace",
        vec![
            strs(&[r"/\s+/", "~[b-d]~"]),
            strs(&["$"]),
            strs(&["x y", "bd bc"]),
        ],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_replace_basic() {
    let mut p = P::phpt("preg_replace_basic");
    let string = s("123456789 - Hello, world -           This is a string.");
    p.dump(&string);
    for (pattern, replacement) in [
        (s("<- This is a string$>"), sq(r"This shouldn\'t work")),
        (s("<[0-35-9]>"), s("4")),
        (s(r"<\b[hH]\w{2,4}>"), s("Bonjour")),
        (s(r"<(\w)\s*-\s*(\w)>"), dq(r"\\1. \\2")),
    ] {
        let r = p.call("preg_replace", vec![pattern, replacement, string.clone()]);
        p.dump(&r);
    }
    let r = p.call(
        "preg_replace",
        vec![
            s(r"<(^[a-z]\w+)@(\w+)\.(\w+)\.([a-z]{2,}$)>"),
            dq(r"\\1 at \\2 dot \\3 dot \\4"),
            s("josmessa@uk.ibm.com"),
        ],
    );
    p.dump(&r);
    p.finish();
}

/// `parseTagsRecursive()` of preg_replace_callback.phpt: a callback that
/// calls `preg_replace_callback()` itself.
fn parse_tags_recursive(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let regex = sq(r"#\[indent]((?:[^[]|\[(?!/?indent])|(?R))+)\[/indent]#");
    let input = match args.get(0) {
        Value::Array(m) => {
            let mut out = br#"<div style="margin-left: 10px">"#.to_vec();
            out.extend_from_slice(m.get_int(1).unwrap().unref().as_str().unwrap());
            out.extend_from_slice(b"</div>");
            Value::from(out)
        }
        other => other.clone(),
    };
    vm.call_native(
        "preg_replace_callback",
        vec![regex, s("parseTagsRecursive"), input],
    )
}

static PARSE_TAGS_RECURSIVE: NativeFunction = NativeFunction {
    sig: "parseTagsRecursive(mixed $input): string",
    handler: parse_tags_recursive,
};

#[test]
fn preg_replace_callback_recursive() {
    let mut p = P::phpt("preg_replace_callback");
    p.vm().register_module("preg_replace_callback", |r| {
        r.function(&PARSE_TAGS_RECURSIVE);
    });
    let input =
        s("plain [indent] deep [indent] [abcd]deeper[/abcd] [/indent] deep [/indent] plain");
    let output = p.call("parseTagsRecursive", vec![input]);
    p.echo_bytes(output.as_str().unwrap());
    p.echo("\n");
    p.finish();
}

#[test]
fn preg_replace_callback2_exceptions_and_arrays() {
    let mut p = P::phpt("preg_replace_callback2");
    // function f() { throw new Exception(); }
    let f = p.callback(0, |vm, _| Err(vm.throw_error("Exception", "")));
    assert!(matches!(
        p.t.call("preg_replace_callback", vec![s(r"/\w/"), f, s("z")]),
        Err(php_runtime::Flow::Exception(_))
    ));
    // function g($x) { return "'$x[0]'"; }
    let g = p.callback(1, |vm, args| {
        let m = args.get(0);
        let first = vm.to_php_string(&idx(m, 0))?;
        let mut out = b"'".to_vec();
        out.extend_from_slice(&first);
        out.push(b'\'');
        Ok(Value::from(out))
    });
    let subject = assoc(vec![
        ("0", s("a b3 bcd")),
        ("v", s("aksfjk")),
        ("12", s("aa bb")),
    ]);
    let r = p.call(
        "preg_replace_callback",
        vec![s(r"@\b\w{1,2}\b@"), g.clone(), subject],
    );
    p.dump(&r);
    let r = p.call(
        "preg_replace_callback",
        vec![s(r"~\A.~"), g, list(vec![strs(&["xyz"])])],
    );
    p.dump(&r);
    // function($m) { return strtolower($m[0]); }
    let lower = p.callback(2, |_, args| {
        let m0 = idx(args.get(0), 0);
        Ok(Value::from(
            m0.as_str().unwrap().to_ascii_lowercase().to_vec(),
        ))
    });
    let r = p.call("preg_replace_callback", vec![s(r"~\A.~"), lower, s("ABC")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_replace_callback_array_basic() {
    let mut p = P::phpt("preg_replace_callback_array");
    let rep = |text: &'static str| move |_: &mut Vm, _: &Args| Ok(s(text));

    // "/a/" => 'b', "/b/" => closure "c", "/c/" => new Rep ("d"), '/d/' => Foo::rep ("ok")
    let b = p.callback(0, rep("b"));
    let c = p.callback(1, rep("c"));
    let d = p.callback(2, rep("d"));
    let ok = p.callback(3, rep("ok"));
    let r = p.call(
        "preg_replace_callback_array",
        vec![
            assoc(vec![("/a/", b), ("/b/", c), ("/c/", d), ("/d/", ok)]),
            s("a"),
        ],
    );
    p.dump(&r);

    // "/a/" => 'b', "/c/" => Rep, "/b/" => closure "ok", '/d/' => Foo::rep
    let b = p.callback(0, rep("b"));
    let d = p.callback(2, rep("d"));
    let closure_ok = p.callback(1, rep("ok"));
    let ok = p.callback(3, rep("ok"));
    let r = p.call(
        "preg_replace_callback_array",
        vec![
            assoc(vec![
                ("/a/", b),
                ("/c/", d),
                ("/b/", closure_ok),
                ("/d/", ok),
            ]),
            s("a"),
        ],
    );
    p.dump(&r);

    // '/d/' => Foo::rep, "/c/" => Rep, "/a/" => 'b', "/b/" => closure "ok"; count
    let ok1 = p.callback(3, rep("ok"));
    let d = p.callback(2, rep("d"));
    let b = p.callback(0, rep("b"));
    let ok2 = p.callback(1, rep("ok"));
    let count = nref();
    let r = p.call(
        "preg_replace_callback_array",
        vec![
            assoc(vec![("/d/", ok1), ("/c/", d), ("/a/", b), ("/b/", ok2)]),
            s("a"),
            int(-1),
            rf(&count),
        ],
    );
    p.dump(&r);
    p.dump(&count.get());

    let b = p.callback(0, rep("b"));
    let d = p.callback(2, rep("d"));
    let r = p.call(
        "preg_replace_callback_array",
        vec![assoc(vec![("/a/", b), ("/c/", d)]), strs(&["a", "c"])],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_replace_callback_array2_errors() {
    let mut p = P::phpt("preg_replace_callback_array2");
    let a = list(vec![]);
    let b = nref();
    b.set(s(""));
    let cb = p.callback(0, |_, _| Ok(s("")));
    if let Some(r) = p.try_call(
        "preg_replace_callback_array",
        vec![assoc(vec![("xx", s("s"))]), a, int(-1), rf(&b)],
    ) {
        p.dump(&r);
    }
    p.dump(&b.get());

    // function f() { throw new Exception($count); } with `$count = 1`
    let throw = |vm: &mut Vm, _: &Args| Err::<Value, _>(vm.throw_error("Exception", "1"));
    let f = p.callback(1, throw);
    let r = p.call(
        "preg_replace_callback_array",
        vec![assoc(vec![(r"/\w", f.clone())]), s("z")],
    );
    p.dump(&r);
    p.try_call(
        "preg_replace_callback_array",
        vec![assoc(vec![(r"/\w/", f.clone()), ("/.*/", f)]), s("z")],
    );
    p.echo("Done\n");
    let _ = cb;
    p.finish();
}

#[test]
fn preg_replace_callback_array_error() {
    let mut p = P::phpt("preg_replace_callback_array_error");
    let b = |p: &mut P| p.callback(0, |_, _| Ok(s("b")));
    let ok = |p: &mut P| p.callback(1, |_, _| Ok(s("ok")));
    let patterns: [&[u8]; 5] = [b"", b"\\b\\", b"aba", b"1b1", b"\0b\0"];
    for pattern in patterns {
        let array = keyed(vec![
            (php_runtime::ArrayKey::from("/a/"), b(&mut p)),
            (php_runtime::ArrayKey::Str(pattern.into()), ok(&mut p)),
        ]);
        let r = p.call("preg_replace_callback_array", vec![array, s("a")]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn uncaught_errors() {
    // preg_match_all_error3.phpt: passing a literal for `&$matches` is an Error
    // raised by the interpreter. preg_replace_callback_array_fatal_error.phpt
    // and preg_replace_callback_array_numeric_index_error.phpt end in an
    // uncaught TypeError.
    let mut p = P::new();
    let b = p.callback(0, |_, _| Ok(s("b")));
    let (class, message) = p.call_err(
        "preg_replace_callback_array",
        vec![
            assoc(vec![("/a/", b), ("/b/", s("invalid callable"))]),
            s("a"),
        ],
    );
    assert_eq!(class, "TypeError");
    assert_eq!(
        message,
        "preg_replace_callback_array(): Argument #1 ($pattern) must contain only valid callbacks"
    );
    let cb = p.callback(1, |_, _| Ok(Value::Null));
    let (class, message) = p.call_err(
        "preg_replace_callback_array",
        vec![keyed(vec![(php_runtime::ArrayKey::Int(42), cb)]), s("a")],
    );
    assert_eq!(class, "TypeError");
    assert_eq!(
        message,
        "preg_replace_callback_array(): Argument #1 ($pattern) must contain only string patterns as keys"
    );
}

#[test]
fn preg_replace_callback_integer_words() {
    let mut p = P::phpt("preg_replace_callback_basic");
    // function integer_word($matches) { global $replacement;
    //     return $replacement[$matches[0]]; }
    let words = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let cb = p.callback(0, move |_, args| {
        let m0 = idx(args.get(0), 0);
        let digit = (m0.as_str().unwrap()[0] - b'0') as usize;
        Ok(s(words[digit]))
    });
    let r = p.call(
        "preg_replace_callback",
        vec![
            s(r"/\d/"),
            cb.clone(),
            s("there are 7 words in this sentence."),
        ],
    );
    p.echo_bytes(r.as_str().unwrap());
    p.echo(" \n");
    let r = p.call(
        "preg_replace_callback",
        vec![
            s(r"/\d/"),
            cb.clone(),
            s("1 2 3 4 is now written in words"),
            int(3),
        ],
    );
    p.echo_bytes(r.as_str().unwrap());
    p.echo(" \n");
    let count = nref();
    let r = p.call(
        "preg_replace_callback",
        vec![
            s(r"/\d/"),
            cb,
            s("there are no numbers in this string"),
            int(5),
            rf(&count),
        ],
    );
    p.echo_bytes(r.as_str().unwrap());
    p.echo(" \n");
    p.dump_count(&count);
    p.finish();
}

impl P {
    /// `print $count`.
    fn dump_count(&mut self, count: &PhpRef) {
        let n = count.get().as_int().unwrap();
        self.echo(&n.to_string());
    }
}

#[test]
fn preg_replace_callback_error1() {
    let mut p = P::phpt("preg_replace_callback_error1");
    let words = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let cb = p.callback(0, move |_, args| {
        let m0 = idx(args.get(0), 0);
        let digit = (m0.as_str().unwrap()[0] - b'0') as usize;
        Ok(s(words[digit]))
    });
    let regex_array = [
        s("abcdef"),
        s("/[a-zA-Z]"),
        s("[a-zA-Z]/"),
        s("/[a-zA-Z]/F"),
        strs(&["[a-z]", "[A-Z]", "[0-9]"]),
        s("/[0-9]/"),
    ];
    for regex in regex_array {
        let r = p.call(
            "preg_replace_callback",
            vec![regex, cb.clone(), s("number 1.")],
        );
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn preg_replace_callback_flags() {
    let mut p = P::phpt("preg_replace_callback_flags");
    // function($matches) { var_dump($matches); return $matches[0][0]; }
    let first_of_first = |vm: &mut Vm, args: &Args| {
        vm.echo(&var_dump_bytes(args.get(0)))?;
        Ok(idx(&idx(args.get(0), 0), 0))
    };
    let whole = |vm: &mut Vm, args: &Args| {
        vm.echo(&var_dump_bytes(args.get(0)))?;
        Ok(idx(args.get(0), 0))
    };

    let count = nref();
    let cb = p.callback(0, first_of_first);
    let r = p.call(
        "preg_replace_callback",
        vec![
            s("/./"),
            cb,
            s("abc"),
            int(-1),
            rf(&count),
            int(PREG_OFFSET_CAPTURE),
        ],
    );
    p.dump(&r);
    p.echo("\n");

    let cb = p.callback(0, first_of_first);
    let r = p.call(
        "preg_replace_callback_array",
        vec![
            assoc(vec![("/./", cb)]),
            s("abc"),
            int(-1),
            rf(&count),
            int(PREG_OFFSET_CAPTURE),
        ],
    );
    p.dump(&r);
    p.echo("\n");

    let cb = p.callback(0, whole);
    let r = p.call(
        "preg_replace_callback",
        vec![
            s("/(a)|(b)/"),
            cb,
            s("abc"),
            int(-1),
            rf(&count),
            int(PREG_UNMATCHED_AS_NULL),
        ],
    );
    p.dump(&r);
    p.echo("\n");

    let cb = p.callback(0, whole);
    let r = p.call(
        "preg_replace_callback_array",
        vec![
            assoc(vec![("/(a)|(b)/", cb)]),
            s("abc"),
            int(-1),
            rf(&count),
            int(PREG_UNMATCHED_AS_NULL),
        ],
    );
    p.dump(&r);
    p.echo("\n");
    p.finish();
}

#[test]
fn preg_replace_edit_basic() {
    let mut p = P::phpt("preg_replace_edit_basic");
    let string = s("123456789 - Hello, world -           This is a string.");
    p.dump(&string);
    let r = p.call(
        "preg_replace",
        vec![
            s("<- This is a string$>"),
            sq(r"This shouldn\'t work"),
            string.clone(),
        ],
    );
    p.dump(&r);
    let count = nref();
    let r = p.call(
        "preg_replace",
        vec![s("<[0-35-9]>"), s("4"), string.clone(), s("5"), rf(&count)],
    );
    p.dump(&r);
    p.dump(&count.get());
    for (pattern, replacement) in [
        (s(r"<\b[hH]\w{2,4}>"), s("Bonjour")),
        (s(r"<(\w)\s*-\s*(\w)>"), dq(r"\\1. \\2")),
    ] {
        let r = p.call("preg_replace", vec![pattern, replacement, string.clone()]);
        p.dump(&r);
    }
    let r = p.call(
        "preg_replace",
        vec![
            s(r"<(^[a-z]\w+)@(\w+)\.(\w+)\.([a-z]{2,}$)>"),
            dq(r"\\1 at \\2 dot \\3 dot \\4"),
            s("josmessa@uk.ibm.com"),
        ],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn preg_replace_error1() {
    let mut p = P::phpt("preg_replace_error1");
    let regex_array = [
        s("abcdef"),
        s("/[a-zA-Z]"),
        s("[a-zA-Z]/"),
        s("/[a-zA-Z]/F"),
        strs(&["[a-z]", "[A-Z]", "[0-9]"]),
        s("/[a-zA-Z]/"),
    ];
    for regex in regex_array {
        let r = p.call("preg_replace", vec![regex, int(1), s("a")]);
        p.dump(&r);
    }
    let std = p.std_object();
    if let Some(r) = p.try_call("preg_replace", vec![std, int(1), s("a")]) {
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn preg_replace_error2() {
    let mut p = P::phpt("preg_replace_error2");
    let regex = s("/[a-zA-Z]/");
    let std = p.std_object();
    for value in [s("this is a string"), strs(&["this is", "a subarray"]), std] {
        if let Some(r) = p.try_call("preg_replace", vec![regex.clone(), value, s("test")]) {
            p.dump(&r);
        }
    }
    p.finish();
}

#[test]
fn preg_replace_variation1() {
    let mut p = P::phpt("preg_replace_variation1");
    let string = s(
        "This is a string. It contains numbers (0-9) as well as parentheses and some other things!",
    );
    let r = p.call(
        "preg_replace",
        vec![
            strs(&[r"/\b\w{1}s/", r"/(\d{1})-(\d{1})/", r"/[\(!\)]/"]),
            strs(&["test", "$1 to $2", "*"]),
            string,
        ],
    );
    p.echo_bytes(r.as_str().unwrap());
    p.finish();
}

#[test]
fn preg_split_basic() {
    let mut p = P::phpt("preg_split_basic");
    let string = s("this is a_list: value1, Test__, string; Hello, world!_(parentheses)");
    for (pattern, flags) in [
        (r"/[:,;\(\)]/", PREG_SPLIT_NO_EMPTY),
        (r"/:\s*(\w*,*\s*)+;/", 0),
        (r"/(\(|\))/", PREG_SPLIT_DELIM_CAPTURE | PREG_SPLIT_NO_EMPTY),
        ("/NAME/i", 0),
        (r"/\w/", PREG_SPLIT_NO_EMPTY),
    ] {
        let r = p.call(
            "preg_split",
            vec![s(pattern), string.clone(), int(-1), int(flags)],
        );
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn preg_split_error1() {
    let mut p = P::phpt("preg_split_error1");
    let regex_array = [
        s("abcdef"),
        s("/[a-zA-Z]"),
        s("[a-zA-Z]/"),
        s("/[a-zA-Z]/F"),
        strs(&["[a-z]", "[A-Z]", "[0-9]"]),
        s("/[a-zA-Z]/"),
    ];
    let subject = s("1 2 a 3 4 b 5 6");
    for regex in regex_array {
        if let Some(r) = p.try_call("preg_split", vec![regex, subject.clone()]) {
            p.dump(&r);
        }
    }
    let std = p.std_object();
    if let Some(r) = p.try_call("preg_split", vec![std, subject]) {
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn recursion_limit() {
    let mut p = P::phpt("recursion_limit");
    p.ini("pcre.recursion_limit", "2");
    let r = p.call(
        "preg_match_all",
        vec![
            s(r"/\p{Ll}(\p{L}((\p{Ll}\p{Ll})))/"),
            s("aeiou"),
            rf(&nref()),
        ],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 3));
    let r = p.call(
        "preg_match_all",
        vec![s(r"/\p{Ll}\p{L}\p{Ll}\p{Ll}/"), s("aeiou"), rf(&nref())],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 0));
    p.finish();
}

#[test]
fn request47456_j_option() {
    let mut p = P::phpt("request47456");
    let (_, m) = p.match_all(
        r"/(?J)(?<chr>[ac])(?<num>\d)|(?<chr>[b])/",
        "a1bc3",
        PREG_SET_ORDER,
        0,
    );
    p.dump(&m);
    let (_, m) = p.match_all(
        r"/(?<chr>[ac])(?<num>\d)|(?<chr>[b])/J",
        "a1bc3",
        PREG_SET_ORDER,
        0,
    );
    p.dump(&m);
    p.finish();
}

#[test]
fn split_basic() {
    let mut p = P::phpt("split");
    let r = p.call("preg_split", vec![s("/*/"), s("x")]);
    p.dump(&r);
    let subject = s("x yy,zzz");
    let pattern = s(r"/[\s, ]+/");
    let r = p.call("preg_split", vec![pattern.clone(), subject.clone()]);
    p.dump(&r);
    for limit in [-1, 0, 1, 2] {
        let r = p.call(
            "preg_split",
            vec![pattern.clone(), subject.clone(), int(limit)],
        );
        p.dump(&r);
    }
    let r = p.call("preg_split", vec![s(r"/\d*/"), s("ab2c3u")]);
    p.dump(&r);
    let r = p.call(
        "preg_split",
        vec![s(r"/\d*/"), s("ab2c3u"), int(-1), int(PREG_SPLIT_NO_EMPTY)],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn split2_flags_and_recursion_limit() {
    let mut p = P::phpt("split2");
    for flags in [
        PREG_SPLIT_DELIM_CAPTURE,
        PREG_SPLIT_OFFSET_CAPTURE,
        PREG_SPLIT_NO_EMPTY | PREG_SPLIT_DELIM_CAPTURE,
        PREG_SPLIT_NO_EMPTY | PREG_SPLIT_OFFSET_CAPTURE,
        PREG_SPLIT_DELIM_CAPTURE | PREG_SPLIT_OFFSET_CAPTURE,
        PREG_SPLIT_NO_EMPTY | PREG_SPLIT_DELIM_CAPTURE | PREG_SPLIT_OFFSET_CAPTURE,
    ] {
        let r = p.call(
            "preg_split",
            vec![s(r"/(\d*)/"), s("ab2c3u"), int(-1), int(flags)],
        );
        p.dump(&r);
    }
    p.ini("pcre.recursion_limit", "1");
    let e = p.last_error();
    p.dump(&Value::Bool(e == 0));
    let r = p.call("preg_split", vec![s(r"/(\d*)/"), s("ab2c3u")]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 3));
    p.finish();
}

#[test]
fn study_regex() {
    let mut p = P::phpt("study");
    for (pattern, subject, group) in [
        ("/(?:(?:(?:(?:(?:(.))))))/  S", "aeiou", 1),
        ("/(?:(?:(?:(?:(?:(.))))))/", "aeiou", 1),
        ("/(?>..)((?:(?>.)|.|.|.|u))/S", "aeiou", 1),
        // try to trigger usual "match known text" optimization
        ("/^aeiou$/S", "aeiou", 0),
        ("/aeiou/S", "aeiou", 0),
    ] {
        let (r, m) = p.match_(pattern, subject, 0, 0);
        p.dump(&r);
        p.dump(&idx(&m, group));
    }
    p.finish();
}

#[test]
fn ungreedy() {
    let mut p = P::phpt("ungreedy");
    for pattern in ["/<.*>/", "/<.*>/U", "/(?U)<.*>/"] {
        p.dump_match(pattern, "<aa> <bb> <cc>");
    }
    p.finish();
}

#[test]
fn preg_match_error1() {
    let mut p = P::phpt("preg_match_error1");
    let regex_array = [
        s("abcdef"),
        s("/[a-zA-Z]"),
        s("[a-zA-Z]/"),
        s("/[a-zA-Z]/F"),
        strs(&["[a-z]", "[A-Z]", "[0-9]"]),
        s("/[a-zA-Z]/"),
    ];
    let subject = s("this is a test");
    for regex in regex_array {
        if let Some(r) = p.try_call("preg_match", vec![regex, subject.clone()]) {
            p.dump(&r);
        }
    }
    let std = p.std_object();
    if let Some(r) = p.try_call("preg_match", vec![std, subject]) {
        p.dump(&r);
    }
    p.finish();
}

/// preg_match_error4.phpt exhausts the JIT stack in a JIT build; without JIT
/// the pattern has to match without hitting any limit.
#[test]
fn preg_match_error4_big_pattern() {
    let mut p = P::phpt("preg_match_error4");
    let script = file_section("preg_match_error4");
    let literal = |start: &[u8], end: &[u8]| {
        sq_bytes(std::str::from_utf8(&between(&script, start, end)).unwrap())
    };
    let re = literal(b"$re = '", b"';\n\n$str");
    let str_ = literal(b"$str = '", b"';\n\n$count");
    let m = nref();
    let r = p.call(
        "preg_match",
        vec![Value::from(re), Value::from(str_), rf(&m)],
    );
    match r {
        Value::Int(count) => p.echo(&format!("string(16) \"Done, {count} matches.\"")),
        other => panic!("preg_match failed: {other:?}, error {}", p.last_error_msg()),
    }
    p.finish();
}
