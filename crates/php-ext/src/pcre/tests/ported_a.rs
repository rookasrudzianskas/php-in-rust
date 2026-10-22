//! Ported .phpt tests, batch A (`001` .. `bug70345`).
//!
//! Each test transliterates the `--FILE--` section of the phpt of the same
//! name (see `tests/phpt/ext/pcre/tests/`) and compares what it prints with
//! the phpt's expected output. PHP closures and classes are replaced by
//! native callbacks where a test uses them.

use super::support::*;
use php_runtime::{Args, NativeFunction, PResult, Value, Vm};

#[test]
fn t001_abusing_preg_match() {
    let mut p = P::phpt("001");
    for subject in ["2006-05-13", "06-12-12", "data: \"12-Aug-87\""] {
        p.dump_match(
            "~\n        (?P<date>\n        (?P<year>(\\d{2})?\\d\\d) -\n        (?P<month>(?:\\d\\d|[a-zA-Z]{2,3})) -\n        (?P<day>[0-3]?\\d))\n    ~x",
            subject,
        );
    }
    p.finish();
}

#[test]
fn t002_bogus_values() {
    let mut p = P::phpt("002");
    let r = p.try_call(
        "preg_match_all",
        vec![s("//"), s(""), rf(&nref()), int(0xdead)],
    );
    assert!(r.is_none());
    let r = p.call("preg_quote", vec![s("")]);
    p.dump(&r);
    let r = p.call("preg_replace", vec![s("/(.)/"), s("${1}${1"), s("abc")]);
    p.dump(&r);
    let r = p.call(
        "preg_replace",
        vec![s("/.++\\d*+[/"), s("for ($"), s("abc")],
    );
    p.dump(&r);
    let r = p.call("preg_replace", vec![s("/(.)/e"), s("for ($"), s("abc")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn t003_abusing_preg_match_all() {
    let mut p = P::phpt("003");
    for flag in [PREG_PATTERN_ORDER, PREG_SET_ORDER] {
        p.dump_match_all(
            "~\n        (?P<date>\n        (?P<year>(\\d{2})?\\d\\d) -\n        (?P<month>(?:\\d\\d|[a-zA-Z]{2,3})) -\n        (?P<day>[0-3]?\\d))\n        ~x",
            "2006-05-13 e outra data: \"12-Aug-37\"",
            flag,
        );
    }
    p.finish();
}

#[test]
fn t004_abusing_pcre() {
    let mut p = P::phpt("004");
    p.dump_match_all(
        r#"/((?:(?:unsigned|struct)\s+)?\w+)(?:\s*(\*+)\s+|\s+(\**))(\w+(?:\[\s*\w*\s*\])?)\s*(?:(=)[^,;]+)?((?:\s*,\s*\**\s*\w+(?:\[\s*\w*\s*\])?\s*(?:=[^,;]+)?)*)\s*;/S"#,
        "unsigned int xpto = 124; short a, b;",
        PREG_SET_ORDER,
    );
    p.dump_match_all(
        r#"/(?:\([^)]+\))?(&?)([\w>.()-]+(?:\[\w+\])?)\s*,?((?:\)*\s*=)?)/S"#,
        "&a, b, &c",
        PREG_SET_ORDER,
    );
    p.dump_match_all(
        r#"/zend_parse_parameters(?:_ex\s*\([^,]+,[^,]+|\s*\([^,]+),\s*"([^"]*)"\s*,\s*([^{;]*)/S"#,
        "zend_parse_parameters( 0, \"addd|s/\", a, b, &c);",
        PREG_SET_ORDER | PREG_OFFSET_CAPTURE,
    );
    let r = p.call(
        "preg_split",
        vec![
            s(r#"/PHP_(?:NAMED_)?(?:FUNCTION|METHOD)\s*\((\w+(?:,\s*\w+)?)\)/S"#),
            s("PHP_FUNCTION(s, preg_match)\n{\nlalala"),
            int(-1),
            int(PREG_SPLIT_DELIM_CAPTURE | PREG_SPLIT_OFFSET_CAPTURE),
        ],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn t006_replace_with_failing_patterns() {
    let mut p = P::phpt("006");
    p.ini("pcre.backtrack_limit", "100000");
    let text = "[CODE]&lt;td align=&quot;$stylevar[right]&quot;&gt;[/CODE]";
    let pattern = r#"#\[(right)\](((?R)|[^[]+?|\[)*)\[/\\1\]#siU"#;
    let result = p.call(
        "preg_replace",
        vec![strs(&[pattern, pattern]), s(""), s(text)],
    );
    p.dump(&s(text));
    p.dump(&result);
    let result = p.call("preg_replace", vec![s(pattern), s(""), s(text)]);
    p.dump(&s(text));
    p.dump(&result);
    p.echo("Done\n");
    p.finish();
}

/// `evil()` of 007: dumps the match array and returns `$x[0]` (the global
/// `$txt` it modifies is emulated by the test).
#[test]
fn t007_callback_modifies_subject() {
    let mut p = P::phpt("007");
    let txt = "ola123";
    let r = p.call(
        "preg_replace_callback",
        vec![s("#.#u"), s("cb_dump"), s(txt)],
    );
    p.dump(&r);
    // `$txt[3] = "\xFF"` happened in the callback.
    p.dump(&sb(b"ola\xFF23"));
    let e = p.last_error();
    p.dump(&Value::Bool(e == 0));

    let r = p.call(
        "preg_replace_callback",
        vec![s("#.#u"), s("cb_dump"), sb(b"ola\xFF23")],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    p.echo("Done!\n");
    p.finish();
}

#[test]
fn backtrack_limit() {
    let mut p = P::phpt("backtrack_limit");
    p.ini("pcre.backtrack_limit", "2");
    let r = p.call(
        "preg_match_all",
        vec![s("/.*\\p{N}/"), s("0123456789"), rf(&nref())],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 2));
    let r = p.call(
        "preg_match_all",
        vec![s("/\\p{Nd}/"), s("0123456789"), rf(&nref())],
    );
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 0));
    p.finish();
}

#[test]
fn bug20528_split_on_word_boundary() {
    let mut p = P::phpt("bug20528");
    let r = p.call("preg_split", vec![s("/\\b/"), s("(#11/19/2002#)")]);
    p.dump(&r);
    p.finish();
}

/// `foo::cb()`: dumps its argument and returns "yes!".
fn cb_yes(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.echo(&var_dump_bytes(args.get(0)))?;
    Ok(Value::str("yes!"))
}

static CB_YES: NativeFunction = NativeFunction {
    sig: "cb_yes(array $m): string",
    handler: cb_yes,
};

#[test]
fn bug21732_invalid_parameters() {
    let mut p = P::phpt("bug21732");
    p.vm().register_module("bug21732", |r| {
        r.function(&CB_YES);
    });
    // `array(new foo(), 'cb')` is emulated by a native callback.
    p.try_call("preg_replace", vec![s(""), list(vec![]), s("")]);
    let r = p.call(
        "preg_replace_callback",
        vec![s("/(ab)(cd)(e)/"), s("cb_yes"), s("abcde")],
    );
    p.dump(&r);
    p.finish();
}

fn cb_working(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::str("working"))
}

static CB_WORKING: NativeFunction = NativeFunction {
    sig: "cb_working(array $m): string",
    handler: cb_working,
};

#[test]
fn bug21758_callback_with_methods() {
    let mut p = P::phpt("bug21758");
    p.vm().register_module("bug21758", |r| {
        r.function(&CB_WORKING);
    });
    let r = p.call(
        "preg_replace_callback",
        vec![
            s("/broken/"),
            s("cb_working"),
            s("preg_replace() is broken"),
        ],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn bug26927_preg_quote_nul() {
    let mut p = P::phpt("bug26927");
    let quoted = p.call("preg_quote", vec![sb(b"a\0b")]);
    let mut pattern = b"!".to_vec();
    pattern.extend_from_slice(quoted.as_str().unwrap());
    pattern.push(b'!');
    let r = p.call("preg_match", vec![sb(&pattern), sb(b"a\0b")]);
    p.dump(&r);
    p.dump(&quoted);
    p.finish();
}

#[test]
fn bug27011_match_all_without_matches() {
    let mut p = P::phpt("bug27011");
    p.dump_match_all(
        r#"|(\w+)://([^\s"<]*[\w+#?/&=])|"#,
        "This is a text string",
        PREG_SET_ORDER,
    );
    p.finish();
}

#[test]
fn bug27103_split_utf8() {
    let mut p = P::phpt("bug27103");
    // iter(): `htmlentities($c, 0, "UTF-8"), ": ", strlen($c)`; the only
    // non-ASCII character of the subject is the euro sign.
    fn iter(p: &mut P, pieces: &Value) {
        let Value::Array(a) = pieces else { panic!() };
        for (_, v) in a.iter() {
            let Value::String(c) = v.unref() else {
                panic!()
            };
            let shown = if &*c == "\u{20ac}".as_bytes() {
                b"&euro;".to_vec()
            } else {
                c.to_vec()
            };
            p.echo_bytes(&shown);
            p.echo(&format!(": {}\n", c.len()));
        }
    }
    let teststr = "\u{20ac} hi there";
    let r = p.call(
        "preg_split",
        vec![s("//u"), s(teststr), int(-1), int(PREG_SPLIT_NO_EMPTY)],
    );
    iter(&mut p, &r);
    let (_, m) = p.match_all("/./u", teststr, 0, 0);
    let Value::Array(m) = m else { panic!() };
    iter(&mut p, &m.get_int(0).unwrap().unref());
    p.finish();
}

#[test]
fn bug34790_named_group_result() {
    let mut p = P::phpt("bug34790");
    let m = nref();
    p.call(
        "preg_match_all",
        vec![
            s("/(?P<word>the)/"),
            s("what the word and the other word the"),
            rf(&m),
        ],
    );
    p.dump(&key(&m.get(), "word"));
    p.finish();
}

#[test]
fn bug37800_limit_parameter() {
    let mut p = P::phpt("bug37800");
    for limit in [1, 10] {
        let count = nref();
        let out = p.call(
            "preg_replace",
            vec![s("/1/"), s("One "), s("1111111111"), int(limit), rf(&count)],
        );
        let out = out.as_str().unwrap().to_string();
        p.echo(&format!("Output = '{out}'\n"));
        p.echo(&format!("Count  = {}\n", count.get().as_int().unwrap()));
        let e = p.last_error();
        p.dump(&Value::Bool(e == 0));
    }
    p.finish();
}

#[test]
fn bug37911_named_groups_in_callback() {
    let mut p = P::phpt("bug37911");
    // function callback($match) { var_dump($match);
    //     return $match[1].'/'.strlen($match['name']); }
    let cb = p.callback(0, |vm, args| {
        vm.echo(&var_dump_bytes(args.get(0)))?;
        let m = args.get(0);
        let g1 = idx(m, 1);
        let name_len = key(m, "name").as_str().map_or(0, |n| n.len());
        let mut out = g1.as_str().unwrap().to_vec();
        out.extend_from_slice(format!("/{name_len}").as_bytes());
        Ok(Value::from(out))
    });
    let r = p.call(
        "preg_replace_callback",
        vec![s("|(?P<name>blub)|"), cb.clone(), s("bla blub blah")],
    );
    p.dump(&r);
    p.dump_match("|(?P<name>blub)|", "bla blub blah");
    let r = p.call(
        "preg_replace_callback",
        vec![s("|(?P<1>blub)|"), cb, s("bla blub blah")],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn bug38600_extended_class() {
    let mut p = P::phpt("bug38600");
    p.dump_match(
        r"/(?<!\w)(0x[\p{N}]+[lL]?|[\p{Nd}]+(e[\p{Nd}]*)?[lLdDfF]?)(?!\w)/",
        "bla bla bla",
    );
    p.finish();
}

#[test]
fn bug40195_pcre_regression() {
    let mut p = P::phpt("bug40195");
    p.dump_match("@^(/([a-z]*))*$@", "//abcde");
    p.dump_match("@^(/(?:[a-z]*))*$@", "//abcde");
    p.dump_match("@^(/([a-z]+))+$@", "/a/abcde");
    p.dump_match("@^(/(?:[a-z]+))+$@", "/a/abcde");
    p.finish();
}

#[test]
fn bug40909_pcre_regression() {
    let mut p = P::phpt("bug40909");
    let pattern = dq(
        r#"/\s([\w_\.\/]+)(?:=([\'\"]?(?:[\w\d\s\?=\(\)\.,'_#\/\\:;&-]|(?:\\\\\"|\\\')?)+[\'\"]?))?/"#,
    );
    let context = dq(r#"<simpletag an_attribute=\"simpleValueInside\">"#);
    let m = nref();
    let r = p.call("preg_match_all", vec![pattern, context, rf(&m)]);
    if r.as_int().unwrap() != 0 {
        p.dump(&r);
        p.dump(&m.get());
    }
    p.finish();
}

#[test]
fn bug41050_pcre_regression() {
    let mut p = P::phpt("bug41050");
    let mut regex = sq_bytes(r"/(insert|drop|create|select|delete|update)([^;\']*(");
    regex.extend_from_slice(b"('[^']*')+");
    regex.extend_from_slice(&sq_bytes(r")?)*(;|$)/i"));
    let m = nref();
    let r = p.call(
        "preg_match",
        vec![Value::from(regex), s("SELECT * FROM #__components"), rf(&m)],
    );
    p.echo(if r.as_int() == Some(1) {
        "matched"
    } else {
        "not matched"
    });
    p.print_r(&m.get());
    p.finish();
}

#[test]
fn bug41148_pcre_regression() {
    let mut p = P::phpt("bug41148");
    // $ligne_horizontale = $puce = $debut_intertitre = $fin_intertitre = ''
    let cherche1 = [
        r#"/\n(----+|____+)/S"#,
        r#"/\n-- */S"#,
        r#"/\n- */S"#,
        r#"/\n_ +/S"#,
        r#"/(^|[^{])[{][{][{]/S"#,
        r#"/[}][}][}]($|[^}])/S"#,
        r#"/(( *)\n){2,}(<br[[:space:]]*\/?>)?/S"#,
        r#"/[{][{]/S"#,
        r#"/[}][}]/S"#,
        r#"/[{]/S"#,
        r#"/[}]/S"#,
        r#"/(<br[[:space:]]*\/?>){2,}/S"#,
        r#"/<p>([\n]*(<br[[:space:]]*\/?>)*)*/S"#,
        r#"/<quote>/S"#,
        r#"/<\/quote>/S"#,
    ];
    let remplace1 = [
        r#"\n\n\n\n"#,
        r#"\n<br />&mdash;&nbsp;"#,
        r#"\n<br />&nbsp;"#,
        r#"\n<br />"#,
        r#"\$1\n\n"#,
        r#"\n\n\$1"#,
        "<p>",
        r#"<strong class=\"spip\">"#,
        "</strong>",
        r#"<i class=\"spip\">"#,
        "</i>",
        "<p>",
        "<p>",
        r#"<blockquote class=\"spip\"><p>"#,
        "</blockquote><p>",
    ];
    let letexte = p.call(
        "preg_replace",
        vec![
            list(cherche1.iter().map(|v| dq(v)).collect()),
            list(remplace1.iter().map(|v| dq(v)).collect()),
            s("<br><br>"),
        ],
    );
    let letexte = p.call("preg_replace", vec![s("@^ <br />@S"), s(""), letexte]);
    p.echo_bytes(letexte.as_str().unwrap());
    p.finish();
}

#[test]
fn bug41638_pcre_regression() {
    let mut p = P::phpt("bug41638");
    let subject = s("repeater id='loopt' dataSrc=subject columns=2");
    for pattern in [
        r#"/(['\"])((.*(\\\\\\1)*)*)\\1/sU"#,
        r#"/(['\"])((?:\\\\\\1|.)*)\\1/sU"#,
        r#"/(['\"])(.*)(?<!\\\\)\\1/sU"#,
    ] {
        let m = nref();
        p.call("preg_match_all", vec![dq(pattern), subject.clone(), rf(&m)]);
        p.print_r(&m.get());
    }
    p.finish();
}

#[test]
fn bug42298_bogus_results_with_u() {
    let mut p = P::phpt("bug42298");
    let str1 = dq(r"A\xc2\xa3BC");
    let str2 = dq(r"A\xe2\x82\xac ");
    for (pattern, subject) in [
        (r"/\S\S/u", &str1),
        (r"/\S{2}/u", &str1),
        (r"/\W\W/u", &str2),
        (r"/\W{2}/u", &str2),
    ] {
        let m = nref();
        p.call("preg_match_all", vec![s(pattern), subject.clone(), rf(&m)]);
        p.dump(&m.get());
    }
    p.finish();
}

#[test]
fn bug42737_split_u_with_newlines() {
    let mut p = P::phpt("bug42737");
    let r = p.call(
        "preg_split",
        vec![s("//u"), sb(b"\r\n"), int(-1), int(PREG_SPLIT_NO_EMPTY)],
    );
    // array_map('ord', $array)
    let Value::Array(pieces) = r else { panic!() };
    let ords = pieces
        .values()
        .map(|v| int(v.as_str().unwrap()[0] as i64))
        .collect();
    p.dump(&list(ords));
    p.finish();
}

#[test]
fn bug42945_split_swallows_part_of_the_string() {
    let mut p = P::phpt("bug42945");
    let (r, m) = p.match_all(r"/\b/", "a'", PREG_OFFSET_CAPTURE, 0);
    p.dump(&r);
    p.dump(&m);
    for flags in [
        0,
        PREG_SPLIT_OFFSET_CAPTURE,
        PREG_SPLIT_NO_EMPTY,
        PREG_SPLIT_NO_EMPTY | PREG_SPLIT_OFFSET_CAPTURE,
    ] {
        let r = p.call("preg_split", vec![s(r"/\b/"), s("a'"), int(-1), int(flags)]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn bug44191_grep_keeps_the_input_array() {
    let mut p = P::phpt("bug44191");
    let array = list((1..=10).map(int).collect());
    p.call("preg_grep", vec![s("/asdf/"), array.clone()]);
    let Value::Array(a) = &array else { panic!() };
    for k in a.keys() {
        p.echo(&k.to_string());
    }
    p.finish();
}

#[test]
fn bug44214_callback_and_global_variable() {
    let mut p = P::phpt("bug44214");
    let collected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = collected.clone();
    let cb = p.callback(0, move |_vm, args| {
        sink.borrow_mut().push(args.get(0).clone());
        Ok(s("xxx"))
    });
    let r = p.call(
        "preg_replace_callback",
        vec![s("`a+`"), cb, s("aaa bbb ccc ddd eee ccc aaa bbb")],
    );
    p.dump(&r);
    p.dump(&list(collected.borrow().clone()));
    p.finish();
}

#[test]
fn bug44214_2_callback_and_global_variable() {
    let mut p = P::phpt("bug44214_2");
    let collected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = collected.clone();
    let cb = p.callback(0, move |_vm, args| {
        sink.borrow_mut().push(idx(args.get(0), 0));
        Ok(s("xxx"))
    });
    let r = p.call(
        "preg_replace_callback",
        vec![s("`a+`"), cb, s("aaa bbb ccc ddd eee ccc aaa bbb")],
    );
    p.dump(&r);
    p.dump(&list(collected.borrow().clone()));
    p.finish();
}

#[test]
fn bug44925_grep_does_not_modify_the_input() {
    let mut p = P::phpt("bug44925");
    let str2 = php_runtime::PhpRef::new(s("b"));
    let array = list(vec![
        s("1"),
        int(2),
        int(3),
        Value::Float(1.1),
        Value::Bool(false),
        Value::Null,
        list(vec![]),
        s("a"),
        rf(&str2),
    ]);
    p.dump(&array);
    let r = p.call("preg_grep", vec![s("/do not match/"), array.clone()]);
    p.dump(&r);
    let a = p.call("preg_grep", vec![s("/./"), array.clone()]);
    p.dump(&a);
    str2.set(s("y"));
    p.dump(&a); // check if array is still ok
    p.dump(&array);
    p.finish();
}

#[test]
fn bug47229_preg_quote_dash() {
    let mut p = P::phpt("bug47229");
    let r = p.call("preg_quote", vec![s("-oh really?")]);
    p.dump(&r);
    for (pattern, subject) in [
        (r"/[a\-c]+/", "a---b"),
        (r"/[a\-c]+/", r"a\-"),
        (r"/a\-{2,}/", "a----a"),
        (r"/a\-{1,}/", r"a\----a"),
    ] {
        let (_, m) = p.match_(pattern, subject, 0, 0);
        p.dump(&m);
    }
    p.finish();
}

#[test]
fn bug47662_many_named_subpatterns() {
    let mut p = P::phpt("bug47662");
    let mut regex = String::from("@");
    for bar in 0..129 {
        regex.push_str(&format!("((?P<x{bar}>))"));
    }
    regex.push_str("fo+bar@");
    let r = p.call("preg_match", vec![s(&regex), s("foobar")]);
    p.dump(&r);
    p.echo("Done!\n");
    p.finish();
}

#[test]
fn t005_abusing_preg_match_all_2() {
    let mut p = P::phpt("005");
    let script = super::phpt::file_section("005");
    let sampledata = dq_bytes(
        &String::from_utf8(super::phpt::between(&script, b"$sampledata = \"", b"\";")).unwrap(),
    );
    // Regular expression to select the error and warning information
    // tuned for gcc 3.4, 4.0 and 4.1
    let mut gcc_regex =
        sq_bytes(r#"/^((.+)(\(\.text\+0x[[:xdigit:]]+\))?: In function [`\'](\w+)\':\s+)?"#);
    gcc_regex.extend(sq_bytes(
        r#"((?(1)(?(3)[^:\n]+|\2)|[^:\n]+)):(\d+): (?:(error|warning):\s+)?(.+)"#,
    ));
    // capture up to 100 errors
    gcc_regex.extend(sq_bytes(r#"(?:\s+\5:(\d+): (?:(error|warning):\s+)?(.+))?"#).repeat(99));
    gcc_regex.extend(b"/mS");

    let m = nref();
    let r = p.call(
        "preg_match_all",
        vec![
            Value::from(gcc_regex),
            Value::from(sampledata),
            rf(&m),
            int(PREG_SET_ORDER),
        ],
    );
    p.dump(&r);
    p.print_r(&m.get());
    p.finish();
}

#[test]
fn bug52732_match_limit_returns_false() {
    let mut p = P::phpt("bug52732");
    p.ini("pcre.backtrack_limit", "1");
    let r = p.call(
        "preg_match",
        vec![s(r"/(?:\D+|<\d+>)*[!?]/"), s("foobar foobar foobar")],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn bug52971_utf8_meta_characters() {
    let mut p = P::phpt("bug52971");
    let message = "Der ist ein Süßwasserpool Süsswasserpool ... verschiedene Wassersportmöglichkeiten bei ...";
    for pattern in [r"/\bwasser/iu", r"/[^\w]wasser/iu"] {
        let (_, m) = p.match_all(pattern, message, PREG_OFFSET_CAPTURE, 0);
        p.dump(&m);
    }
    p.finish();
}

#[test]
fn bug53823_star_quantifier_on_unicode_replace() {
    let mut p = P::phpt("bug53823");
    for subject in [
        "áéíóú".as_bytes(),
        // invalid UTF-8
        b"\xFC\xC3\xA1\xC3\xA9\xC3\xAD\xC3\xB3\xC3\xBA",
        b"\xC3\xA1\xC3\xA9\xC3\xAD\xC3\xB3\xC3\xBA\xFC",
    ] {
        let r = p.call(
            "preg_replace",
            vec![s(r"/[^\pL\pM]*/iu"), s(""), sb(subject)],
        );
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn bug61780_unmatched_as_null() {
    let mut p = P::phpt("bug61780");
    let (_, m) = p.match_(r"/(a)?([a-z]*)(\d*)/", "123", PREG_UNMATCHED_AS_NULL, 0);
    p.dump(&m);
    p.finish();
}

/// The six calls of bug61780_1 and bug61780_2 (the pattern differs).
fn unmatched_as_null_calls(p: &mut P, pattern: &str) {
    let (_, m) = p.match_(pattern, "23456", PREG_UNMATCHED_AS_NULL, 0);
    p.export(&m);
    p.echo("\n\n");
    let (_, m) = p.match_(
        pattern,
        "23456",
        PREG_OFFSET_CAPTURE | PREG_UNMATCHED_AS_NULL,
        0,
    );
    p.export(&m);
    p.echo("\n\n");
    for (i, flags) in [
        PREG_UNMATCHED_AS_NULL,
        PREG_OFFSET_CAPTURE | PREG_UNMATCHED_AS_NULL,
        PREG_SET_ORDER | PREG_UNMATCHED_AS_NULL,
        PREG_SET_ORDER | PREG_OFFSET_CAPTURE | PREG_UNMATCHED_AS_NULL,
    ]
    .into_iter()
    .enumerate()
    {
        let (_, m) = p.match_all(pattern, "123456", flags, 0);
        p.export(&m);
        if i < 3 {
            p.echo("\n\n");
        }
    }
}

#[test]
fn bug61780_1_numeric_subpatterns() {
    let mut p = P::phpt("bug61780_1");
    unmatched_as_null_calls(&mut p, r"/(4)?(2)?\d/");
    p.finish();
}

#[test]
fn bug61780_2_named_subpatterns() {
    let mut p = P::phpt("bug61780_2");
    unmatched_as_null_calls(&mut p, r"/(?<a>4)?(?<b>2)?\d/");
    p.finish();
}

#[test]
fn bug63055_recursive_matches_array() {
    // The by-ref `$matches` held recursive arrays; it is simply overwritten.
    let mut p = P::phpt("bug63055");
    let m = nref();
    m.set(assoc(vec![("foo", s("bar"))]));
    p.call(
        "preg_match_all",
        vec![s(r"/(\d)+/"), s("foo123456bar"), rf(&m)],
    );
    p.echo("okey");
    p.finish();
}

#[test]
fn bug66121_utf8_lookbehinds() {
    let mut p = P::phpt("bug66121");
    // Sinhala characters
    for subject in ["ක", "ම"] {
        let r = p.call("preg_replace", vec![s("/(?<!ක)/u"), s("*"), s(subject)]);
        p.dump(&r);
    }
    // English characters
    for subject in ["k", "m"] {
        let r = p.call("preg_replace", vec![s("/(?<!k)/u"), s("*"), s(subject)]);
        p.dump(&r);
    }
    // Sinhala characters
    let (_, m) = p.match_all("/(?<!ක)/u", "ම", PREG_OFFSET_CAPTURE, 0);
    p.dump(&m);
    // invalid UTF-8
    for subject in [&b"\xFC\xE0\xB6\x9A"[..], b"\xE0\xB6\x9A\xFC"] {
        let r = p.call("preg_replace", vec![s("/(?<!ක)/u"), s("*"), sb(subject)]);
        p.dump(&r);
    }
    for _ in 0..2 {
        let m = nref();
        let r = p.call(
            "preg_match_all",
            vec![
                s("/(?<!ක)/u"),
                sb(b"\xFC\xE0\xB6\xAE"),
                rf(&m),
                int(PREG_OFFSET_CAPTURE),
            ],
        );
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn bug67238_ungreedy_min_max_quantifier() {
    let mut p = P::phpt("bug67238");
    let r = p.call("preg_match", vec![s("/a{1,3}b/U"), s("ab")]);
    p.echo(&r.as_int().unwrap().to_string());
    p.finish();
}

/// Runs `f` for 4096 distinct patterns from inside a callback, which makes
/// the compiled-pattern cache evict entries while the callback's pattern is in
/// use.
fn cache_churn(p: &mut P, f: fn(&mut php_runtime::Vm, usize)) {
    let cb = p.callback(0, move |vm, _args| {
        for i in 0..4096 {
            f(vm, i);
        }
        Ok(s("b"))
    });
    let r = p.call("preg_replace_callback", vec![s("/a/"), cb, s("aa")]);
    p.dump(&r);
}

#[test]
fn bug69864_cache_eviction_inside_callback() {
    let mut p = P::phpt("bug69864");
    cache_churn(&mut p, |vm, i| {
        vm.call_native(
            "preg_match",
            vec![s(&format!("/foo{i}bar/")), s(&format!("???foo{i}bar???"))],
        )
        .unwrap();
    });
    cache_churn(&mut p, |vm, i| {
        vm.call_native(
            "preg_replace",
            vec![
                s(&format!("/foo{i}bar/")),
                s("baz"),
                s(&format!("???foo{i}bar???")),
            ],
        )
        .unwrap();
    });
    cache_churn(&mut p, |vm, i| {
        vm.call_native(
            "preg_split",
            vec![s(&format!("/foo{i}bar/")), s(&format!("???foo{i}bar???"))],
        )
        .unwrap();
    });
    cache_churn(&mut p, |vm, i| {
        vm.call_native(
            "preg_grep",
            vec![
                s(&format!("/foo{i}bar/")),
                list(vec![s(&format!("???foo{i}bar???"))]),
            ],
        )
        .unwrap();
    });
    p.finish();
}

#[test]
fn bug70232_bump_along_with_k() {
    let mut p = P::phpt("bug70232");
    let pattern = r"~(?: |\G)\d\B\K~";
    let subject = "123 a123 1234567 b123 123";
    let (_, m) = p.match_all(pattern, subject, 0, 0);
    p.dump(&m);
    let r = p.call("preg_replace", vec![s(pattern), s("*"), s(subject)]);
    p.dump(&r);
    let r = p.call("preg_split", vec![s(pattern), s(subject)]);
    p.dump(&r);
    p.finish();
}

#[test]
fn bug70345_k_in_lookahead() {
    let mut p = P::phpt("bug70345");
    let r = p.call("preg_split", vec![s("/(?=xyz\\K)/"), s("aaaaxyzaaaa")]);
    p.dump(&r);
    let (_, m) = p.match_("/(a(?=xyz\\K))/", "aaaaxyzaaaa", 0, 0);
    p.dump(&m);
    p.finish();
}
