//! Ported .phpt tests, batch B (`bug70345_old` .. `errors03`).
//!
//! Not ported: bug70345_old (needs PCRE < 10.38), bug72463 and bug72463_2
//! (`mail()`), bug76850 and ctype_back_to_c (`setlocale()`), bug78272
//! (`pcntl_fork()`), bug81243 (`memory_get_usage()`), check_jit_enabled
//! (`phpinfo()`).

use super::phpt::{between, file_section};
use super::support::*;
use php_runtime::Value;

#[test]
fn bug71537_replacement_array_with_int() {
    let mut p = P::phpt("bug71537");
    let r = p.call(
        "preg_replace",
        vec![
            strs(&["/Monkey/"]),
            list(vec![int(2016)]),
            s("Happy Year of Monkey"),
        ],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn bug72685_utf8_validated_once() {
    // The subject is validated once, not at every call (the phpt measures the
    // time of 64K calls).
    let mut p = P::phpt("bug72685");
    let input_size = 64 * 1024;
    let subject = s(&"a".repeat(input_size));
    let start = std::time::Instant::now();
    let mut pos = 0;
    while p
        .call(
            "preg_match",
            vec![
                s(r"/\G\w/u"),
                subject.clone(),
                rf(&nref()),
                int(0),
                int(pos),
            ],
        )
        .as_int()
        == Some(1)
    {
        pos += 1;
    }
    assert_eq!(pos as usize, input_size);
    // Re-validating on every call would take far longer than this.
    assert!(start.elapsed().as_secs_f64() < 5.0);
}

#[test]
fn bug72688_many_named_groups() {
    let mut p = P::phpt("bug72688");
    let pattern: Vec<String> = (0..300).map(|i| format!("(?'group{i}'{i}$)")).collect();
    let full = format!("/{}/uix", pattern.join("|"));
    let (_, m) = p.match_(&full, "290", 0, 0);
    p.dump(&key(&m, "group290"));
    p.finish();
}

#[test]
fn bug73392_callback_array_with_invalid_pattern() {
    let mut p = P::phpt("bug73392");
    let b = p.callback(0, |_, _| Ok(s("b")));
    let closure = p.callback(1, |_, _| Ok(s("c")));
    let rep = p.callback(2, |_, _| Ok(s("d")));
    let foo_rep = p.callback(3, |_, _| Ok(s("ok")));
    let patterns = assoc(vec![
        ("/a/", b),
        ("/b/", closure),
        ("/c/", rep),
        ("reporting", foo_rep.clone()),
        ("a1", foo_rep),
    ]);
    let r = p.call("preg_replace_callback_array", vec![patterns, s("a")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn bug73483_nested_replace_with_the_same_pattern() {
    let mut p = P::phpt("bug73483");
    let cb = p.callback(0, |vm, args| {
        let subject = idx(args.get(0), 0);
        vm.call_native("preg_replace", vec![s("#dummy#"), s("okey"), subject])
    });
    let r = p.call("preg_replace_callback", vec![s("#dummy#"), cb, s("dummy")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn bug73612_leaks() {
    // The by-ref result parameters previously held cyclic objects.
    let mut p = P::phpt("bug73612");
    let r = nref();
    p.call("preg_match", vec![s("/./"), s("x"), rf(&r)]);
    p.call(
        "preg_replace",
        vec![s("/./"), s(""), s("x"), int(-1), rf(&nref())],
    );
    p.call(
        "preg_replace_callback",
        vec![s("/./"), s("cb_count"), s("x"), int(-1), rf(&nref())],
    );
    p.call(
        "preg_replace_callback_array",
        vec![
            assoc(vec![("/./", s("cb_count"))]),
            s("x"),
            int(-1),
            rf(&nref()),
        ],
    );
    p.call(
        "preg_filter",
        vec![s("/./"), s(""), s("x"), int(-1), rf(&nref())],
    );
    p.echo("===DONE===");
    p.finish();
}

#[test]
fn bug74183_pattern_too_large() {
    let mut p = P::phpt("bug74183");
    let script = file_section("bug74183");
    let regex =
        dq_bytes(std::str::from_utf8(&between(&script, b"$sRegex = \"", b"\";\n$sTest")).unwrap());
    let r = p.call("preg_match", vec![Value::from(regex), s("Hello world")]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 1));
    p.finish();
}

#[test]
fn bug74873_offset_out_of_range() {
    let mut p = P::phpt("bug74873");
    let r = p.call(
        "preg_match",
        vec![s(r"/\S+/"), s("foo bar"), rf(&nref()), int(0), int(99999)],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn bug75089_grep_reports_bad_utf8_after_first_input() {
    let mut p = P::phpt("bug75089");
    p.call(
        "preg_grep",
        vec![s(r"#\d#u"), list(vec![s("a"), sb(b"1\xFF")])],
    );
    let e = p.last_error();
    p.dump(&int(e));
    p.finish();
}

#[test]
fn bug75207_cve_2016_1283() {
    let mut p = P::phpt("bug75207");
    let script = file_section("bug75207");
    let pattern =
        dq_bytes(std::str::from_utf8(&between(&script, b"preg_match(\"", b"\", \"*b")).unwrap());
    p.call("preg_match", vec![Value::from(pattern), dq(r"*b\dc")]);
    p.finish();
}

#[test]
fn bug75355_preg_quote_hash() {
    let mut p = P::phpt("bug75355");
    let q = p.call("preg_quote", vec![s("#")]);
    p.dump(&q);
    let quoted = p.call("preg_quote", vec![s("hello#world"), s("~")]);
    let mut pattern = b"~^(".to_vec();
    pattern.extend_from_slice(quoted.as_str().unwrap());
    pattern.extend_from_slice(br")\z~x");
    let m = nref();
    let r = p.call(
        "preg_match",
        vec![Value::from(pattern), s("hello#world"), rf(&m)],
    );
    p.dump(&r);
    p.dump(&idx(&m.get(), 1));
    p.finish();
}

#[test]
fn bug75457_callout_condition() {
    let mut p = P::phpt("bug75457");
    let r = p.call(
        "preg_match",
        vec![dq(r"/(((?(?C)0?=))(?!()0|.(?0)0)())/"), s("hello")],
    );
    p.dump(&r);
    p.finish();
}

#[test]
fn bug75539_recursive_call_errors() {
    let mut p = P::phpt("bug75539");
    let r = p.call("preg_match", vec![s("/((?1)?z)/"), s("")]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 1));
    p.finish();
}

#[test]
fn bug76127_split_invalid_utf8() {
    let mut p = P::phpt("bug76127");
    let r = p.call("preg_split", vec![s("/a/u"), sb(b"a\xff")]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    p.finish();
}

#[test]
fn bug76512_w_includes_unicode() {
    let mut p = P::phpt("bug76512");
    let r = p.call("preg_match", vec![s(r"/\w/u"), s("ä")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn bug76514_recursive_json_pattern() {
    let mut p = P::phpt("bug76514");
    let script = file_section("bug76514");
    let subject =
        sq_bytes(std::str::from_utf8(&between(&script, b"$str = '", b"';\n\n$res")).unwrap());
    // `'{(?(DEFINE)...(?P<property>'.preg_quote('"require"').'\s*:...}sx'`
    let head = sq_bytes(
        std::str::from_utf8(&between(&script, b"preg_match('{", b"'.preg_quote(")).unwrap(),
    );
    let tail = sq_bytes(
        std::str::from_utf8(&between(
            &script,
            b"'.preg_quote('\"require\"').'",
            b"', $str, $match);",
        ))
        .unwrap(),
    );
    let mut pattern = b"{".to_vec();
    pattern.extend_from_slice(&head);
    let quoted = p.call("preg_quote", vec![s("\"require\"")]);
    pattern.extend_from_slice(quoted.as_str().unwrap());
    pattern.extend_from_slice(&tail);
    let m = nref();
    let r = p.call(
        "preg_match",
        vec![Value::from(pattern), Value::from(subject), rf(&m)],
    );
    p.dump(&r);
    p.dump(&key(&m.get(), "value"));
    let e = p.last_error();
    p.dump(&Value::Bool(e == 6));
    p.finish();
}

#[test]
fn bug76909_no_jit_verb() {
    let mut p = P::phpt("bug76909");
    let data = s(" domain.com");
    let reg0 = dq(r"/^[\x{0100}-\x{017f}]{1,63}$/iu");
    let reg1 = dq(r"/(*NO_JIT)^[\x{0100}-\x{017f}]{1,63}$/iu");
    let m0 = nref();
    let m1 = nref();
    let n0 = p.call("preg_match", vec![reg0, data.clone(), rf(&m0)]);
    let n1 = p.call("preg_match", vec![reg1, data, rf(&m1)]);
    let count = |v: Value| match v {
        Value::Array(a) => int(a.len() as i64),
        other => panic!("{other:?}"),
    };
    p.dump_all(&[n0, n1, count(m0.get()), count(m1.get())]);
    p.finish();
}

#[test]
fn bug77193_callback_returning_null() {
    let mut p = P::phpt("bug77193");
    p.echo("1");
    // The callback echoes `$matches[1]` and returns nothing.
    let cb = p.callback(0, |vm, args| {
        let g = idx(args.get(0), 1);
        vm.echo(g.as_str().unwrap())?;
        Ok(Value::Null)
    });
    p.call(
        "preg_replace_callback",
        vec![s("/([0-9]+)/i"), cb, s("{CCM:CID_2}")],
    );
    p.echo("3\n");
    p.finish();
}

#[test]
fn bug77827_carriage_return_in_flags() {
    let mut p = P::phpt("bug77827");
    let r = p.call("preg_match", vec![dq("/foo/i\\r"), s("FOO")]);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&int(e));
    p.finish();
}

#[test]
fn bug78338_array_cross_border_reading() {
    let mut p = P::phpt("bug78338");
    // hex2bin('2f5c583f3d3f223f3536ff3636ffffffff36a8a8a83636367a7a7a7a3d2aff2f0a')
    let hex = "2f5c583f3d3f223f3536ff3636ffffffff36a8a8a83636367a7a7a7a3d2aff2f0a";
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    p.call("preg_match", vec![sb(&bytes), sb(&bytes)]);
    p.echo("DONE\n");
    p.finish();
}

#[test]
fn bug78853_match_count_is_at_most_one() {
    let mut p = P::phpt("bug78853");
    let r = p.call("preg_match", vec![s(r"/^|\d{1,2}$/"), s("7")]);
    p.dump(&r);
    p.finish();
}

#[test]
fn bug79188_empty_matches_with_astral_characters() {
    let mut p = P::phpt("bug79188");
    let subject = format!("a{}", "\u{1f612}".repeat(10));
    let r = p.call("preg_replace", vec![s("//u"), s(""), s(&subject)]);
    p.dump(&r);
    let cb = p.callback(0, |_, _| Ok(s("")));
    let r = p.call("preg_replace_callback", vec![s("//u"), cb, s(&subject)]);
    p.dump(&r);
    p.finish();
}

#[test]
fn bug79241_offset_in_the_middle_of_a_character() {
    let mut p = P::phpt("bug79241");
    let text = s("\u{2019}");
    let pattern = s(r"/\b/u");
    let call = |p: &mut P, text: &Value, offset: i64| {
        p.call(
            "preg_match",
            vec![
                pattern.clone(),
                text.clone(),
                rf(&nref()),
                int(0),
                int(offset),
            ],
        )
    };
    let r = call(&mut p, &text, 0);
    p.dump(&r);
    let r = call(&mut p, &text, 1);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 5));
    p.echo("\n");

    let text = sb(b"VA\xffLID");
    let r = call(&mut p, &text, 4);
    p.dump(&r);
    let r = call(&mut p, &text, 0);
    p.dump(&r);
    let e = p.last_error();
    p.dump(&Value::Bool(e == 4));
    p.finish();
}

#[test]
fn bug79257_duplicate_named_groups() {
    let mut p = P::phpt("bug79257");
    for pattern in [
        "/(?J)(?:(?<g>foo)|(?<g>bar))/",
        "/(?J)(?:(?<g>foo)|(?<g>bar))(?<h>baz)/",
    ] {
        let subject = if pattern.contains("baz") {
            "foobaz"
        } else {
            "foo"
        };
        for flags in [
            0,
            PREG_UNMATCHED_AS_NULL,
            PREG_OFFSET_CAPTURE,
            PREG_UNMATCHED_AS_NULL | PREG_OFFSET_CAPTURE,
        ] {
            let (_, m) = p.match_(pattern, subject, flags, 0);
            p.dump(&m);
        }
    }
    p.finish();
}

#[test]
fn bug79363_unicode_classes() {
    let mut p = P::phpt("bug79363");
    for pattern in [r"/[\p{L}\p{Arabic}]/", r"/[^\p{L}\p{Arabic}]/"] {
        let r = p.call("preg_replace", vec![s(pattern), s("0"), s("lower UPPER")]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn bug79846_simple_regexp() {
    let mut p = P::phpt("bug79846");
    let (_, m) = p.match_(
        r"/([a-z]+_[a-z]+_*[a-z]+)_?(\d+)?/",
        "component_phase_1",
        0,
        0,
    );
    p.dump(&m);
    p.finish();
}

#[test]
fn bug80118_erroneous_whitespace_match() {
    let mut p = P::phpt("bug80118");
    let (_, m) = p.match_(r"~[^\p{Han}\p{Z}]~u", "     ", 0, 0);
    p.dump(&m);
    p.finish();
}

#[test]
fn bug80866_split_limit_with_k() {
    let mut p = P::phpt("bug80866");
    let r = p.call(
        "preg_split",
        vec![s("~.{3}\\K~"), s("abcdefghijklm"), int(3)],
    );
    p.export(&r);
    p.finish();
}

#[test]
fn bug81101_single_character_repetition() {
    let mut p = P::phpt("bug81101");
    let pattern = sq(r"/\\\\([^\\\\]+)\s*$/");
    let m = nref();
    p.call(
        "preg_match",
        vec![
            pattern.clone(),
            s(r" App\Domain\Repository\MetaData\SomethingRepositoryInterface"),
            rf(&m),
        ],
    );
    p.dump(&m.get());
    p.call(
        "preg_match",
        vec![
            pattern,
            s(r" App\Domain\Exception\NotFoundException"),
            rf(&m),
        ],
    );
    p.dump(&m.get());
    p.finish();
}

#[test]
fn bug81424a_alternation_with_named_group() {
    let mut p = P::phpt("bug81424a");
    let (r, m) = p.match_(r"/(?P<size>\d+)m|M/", "4M", 0, 0);
    p.dump(&r);
    p.dump(&m);
    p.finish();
}

#[test]
fn cache_limit() {
    let mut p = P::phpt("cache_limit");
    let cache_size = 4096 + 1;
    let mut re = String::new();
    let subject = s(&"x".repeat(cache_size));
    for _ in 0..cache_size {
        re.push('.');
        let r = p.call("preg_match", vec![s(&format!("/{re}/")), subject.clone()]);
        assert_eq!(r.as_int(), Some(1), "non match. error");
    }
    // this one was already deleted from the cache
    let r = p.call("preg_match", vec![s("/./"), subject.clone()]);
    p.dump(&r);
    // but not this one
    let r = p.call("preg_match", vec![s(&format!("/{re}/")), subject]);
    p.dump(&r);
    p.echo("done\n");
    p.finish();
}

#[test]
fn delimiters() {
    let mut p = P::phpt("delimiters");
    for pattern in [
        &b""[..],
        b"      ",
        b"@@",
        b"12",
        b"<>",
        b"~a",
        b"@\\@\\@@",
        b"//z",
        b"{",
        b"\0\0",
    ] {
        let subject = if pattern == b"@\\@\\@@" || pattern == b"//z" {
            "@@"
        } else {
            ""
        };
        let r = p.call("preg_match", vec![sb(pattern), s(subject)]);
        p.dump(&r);
    }
    p.finish();
}

#[test]
fn dollar_endonly() {
    let mut p = P::phpt("dollar_endonly");
    for pattern in [r"/^\S+.+$/", r"/^\S+.+$/D", r"/^\S+\s$/D"] {
        p.dump_match_all(pattern, "aeiou\n", PREG_PATTERN_ORDER);
    }
    p.finish();
}

#[test]
fn errors01_recursion_limit_in_split() {
    let mut p = P::phpt("errors01");
    p.ini("pcre.recursion_limit", "1");
    let m = p.last_error_msg();
    p.dump(&Value::Bool(m == "No error"));
    p.call("preg_split", vec![s(r"/(\d*)/"), s("ab2c3u")]);
    let m = p.last_error_msg();
    p.dump(&Value::Bool(m == "Recursion limit exhausted"));
    p.finish();
}

#[test]
fn errors02_malformed_utf8_in_split() {
    let mut p = P::phpt("errors02");
    let r = p.call("preg_split", vec![s("/a/u"), sb(b"a\xff")]);
    p.dump(&r);
    let m = p.last_error_msg();
    p.dump(&Value::Bool(
        m == "Malformed UTF-8 characters, possibly incorrectly encoded",
    ));
    p.finish();
}

#[test]
fn errors03_internal_error() {
    let mut p = P::phpt("errors03");
    let r = p.call("preg_match", vec![s("/"), s("Hello world")]);
    p.dump(&r);
    let m = p.last_error_msg();
    p.dump(&Value::Bool(m == "Internal error"));
    p.finish();
}
