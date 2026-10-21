//! ext/standard/string.c — `strip_tags()` (`php_strip_tags_ex`, `php_tag_find`).

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// C-locale `isspace()` (includes `\v` and `\f`).
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Naive substring search (`strstr`).
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// `php_tag_find`: whether the collected tag (`<a href=..>`, `</a>`, ...)
/// normalised to `<a>` occurs in the lowercased allow-list `set`.
fn tag_find(tag: &[u8], set: &[u8]) -> bool {
    if tag.is_empty() {
        return false;
    }
    // php-src walks a NUL-terminated buffer.
    let at = |i: usize| tag.get(i).copied().unwrap_or(0);
    let mut norm = Vec::with_capacity(tag.len() + 1);
    let mut seen_name = false;
    let mut t = 0;
    while t < tag.len() {
        let c = at(t).to_ascii_lowercase();
        match c {
            b'<' => norm.push(c),
            b'>' => break,
            _ if !is_space(c) => {
                seen_name = true;
                // Drop the `/` of `</a>` and `<a/>`.
                let prev = if t == 0 { 0 } else { at(t - 1) };
                if c != b'/' || (prev != b'<' && at(t + 1) != b'>') {
                    norm.push(c);
                }
            }
            _ => {
                if seen_name {
                    break;
                }
            }
        }
        t += 1;
    }
    norm.push(b'>');
    contains(set, &norm)
}

/// `php_strip_tags_ex`: strips HTML/PHP tags and comments from `input`,
/// keeping the tags listed in `allow` (e.g. `<a><b>`; matched
/// case-insensitively). With `allow_tag_spaces`, `<` followed by whitespace
/// still starts a tag.
///
/// States: 0 = text, 1 = inside an HTML tag, 2 = inside `<?...?>`,
/// 3 = inside `<!...>`, 4 = inside a `<!-- -->` comment.
pub fn php_strip_tags_ex(input: &[u8], allow: Option<&[u8]>, allow_tag_spaces: bool) -> Vec<u8> {
    // The allow-list is used as a C string.
    let allow: Option<Vec<u8>> = allow.map(|a| {
        let a = &a[..a.iter().position(|&c| c == 0).unwrap_or(a.len())];
        a.to_ascii_lowercase()
    });
    let track = allow.is_some();
    // php-src reads one byte past the end (the NUL terminator).
    let at = |i: usize| input.get(i).copied().unwrap_or(0);
    let prev_is = |p: usize, n: usize, c: u8| p >= n && input[p - n] == c;
    let prev_in = |p: usize, n: usize, lc: u8| p >= n && input[p - n].to_ascii_lowercase() == lc;

    let mut out = Vec::with_capacity(input.len());
    let mut tbuf: Vec<u8> = Vec::new();
    let mut state = 0u8;
    let mut lc = 0u8;
    let mut br: i64 = 0;
    let mut depth: i64 = 0;
    let mut in_q = 0u8;
    let mut is_xml = false;

    let toggle_quote = |in_q: &mut u8, c: u8| {
        *in_q = if *in_q != 0 { 0 } else { c };
    };

    for (p, &c) in input.iter().enumerate() {
        match state {
            0 => match c {
                0 => {}
                b'<' => {
                    if in_q != 0 {
                        // dropped
                    } else if is_space(at(p + 1)) && !allow_tag_spaces {
                        out.push(c);
                    } else {
                        lc = b'<';
                        state = 1;
                        if track {
                            tbuf.push(b'<');
                        }
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        out.push(c);
                    }
                }
                _ => out.push(c),
            },
            1 => {
                let mut reg_char = false;
                match c {
                    0 => {}
                    b'<' => {
                        if in_q != 0 {
                            // ignored
                        } else if is_space(at(p + 1)) && !allow_tag_spaces {
                            reg_char = true;
                        } else {
                            depth += 1;
                        }
                    }
                    b'>' => {
                        if depth != 0 {
                            depth -= 1;
                        } else if in_q == 0 {
                            lc = b'>';
                            if !(is_xml && prev_is(p, 1, b'-')) {
                                in_q = 0;
                                state = 0;
                                is_xml = false;
                                if let Some(allow) = &allow {
                                    tbuf.push(b'>');
                                    if tag_find(&tbuf, allow) {
                                        out.extend_from_slice(&tbuf);
                                    }
                                    tbuf.clear();
                                }
                            }
                        }
                    }
                    b'"' | b'\'' => {
                        if p != 0 && (in_q == 0 || c == in_q) {
                            toggle_quote(&mut in_q, c);
                        }
                        reg_char = true;
                    }
                    b'!' => {
                        // JavaScript & other HTML scripting languages.
                        if prev_is(p, 1, b'<') {
                            state = 3;
                            lc = c;
                        } else {
                            reg_char = true;
                        }
                    }
                    b'?' => {
                        if prev_is(p, 1, b'<') {
                            br = 0;
                            state = 2;
                        } else {
                            reg_char = true;
                        }
                    }
                    _ => reg_char = true,
                }
                if reg_char && track {
                    tbuf.push(c);
                }
            }
            2 => match c {
                b'(' => {
                    if lc != b'"' && lc != b'\'' {
                        lc = b'(';
                        br += 1;
                    }
                }
                b')' => {
                    if lc != b'"' && lc != b'\'' {
                        lc = b')';
                        br -= 1;
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 && br == 0 && lc != b'"' && prev_is(p, 1, b'?') {
                        in_q = 0;
                        state = 0;
                        tbuf.clear();
                    }
                }
                b'"' | b'\'' => {
                    if p >= 1 && input[p - 1] != b'\\' {
                        if lc == c {
                            lc = 0;
                        } else if lc != b'\\' {
                            lc = c;
                        }
                        if p != 0 && (in_q == 0 || c == in_q) {
                            toggle_quote(&mut in_q, c);
                        }
                    }
                }
                // `<?xml` is not PHP: switch back to HTML.
                b'l' | b'L'
                    if p > 4
                        && prev_in(p, 1, b'm')
                        && prev_in(p, 2, b'x')
                        && prev_is(p, 3, b'?')
                        && prev_is(p, 4, b'<') =>
                {
                    state = 1;
                    is_xml = true;
                }
                _ => {}
            },
            3 => match c {
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        in_q = 0;
                        state = 0;
                        tbuf.clear();
                    }
                }
                b'"' | b'\'' => {
                    if p != 0 && input[p - 1] != b'\\' && (in_q == 0 || c == in_q) {
                        toggle_quote(&mut in_q, c);
                    }
                }
                b'-' => {
                    if prev_is(p, 1, b'-') && prev_is(p, 2, b'!') {
                        state = 4;
                    }
                }
                // `<!DOCTYPE` is treated like a normal tag.
                b'E' | b'e'
                    if p > 6
                        && prev_in(p, 1, b'p')
                        && prev_in(p, 2, b'y')
                        && prev_in(p, 3, b't')
                        && prev_in(p, 4, b'c')
                        && prev_in(p, 5, b'o')
                        && prev_in(p, 6, b'd') =>
                {
                    state = 1;
                }
                _ => {}
            },
            _ => {
                if c == b'>' && in_q == 0 && prev_is(p, 1, b'-') && prev_is(p, 2, b'-') {
                    in_q = 0;
                    state = 0;
                    tbuf.clear();
                }
            }
        }
    }
    out
}

fn strip_tags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let allow: Option<Vec<u8>> = match args.get(1) {
        Value::Array(tags) => {
            let tags = tags.clone();
            let mut buf = Vec::new();
            for (_, v) in tags.iter() {
                let tag = vm.to_php_string(&v.deref())?;
                buf.push(b'<');
                buf.extend_from_slice(&tag);
                buf.push(b'>');
            }
            // An empty array means no allowed tags (`tags_ss.s` stays NULL).
            (!buf.is_empty()).then_some(buf)
        }
        Value::String(s) => Some(s.to_vec()),
        _ => None,
    };
    let out = php_strip_tags_ex(args.str(0), allow.as_deref(), false);
    Ok(Value::String(PhpStr::from_vec(out)))
}

pub static STRIP_TAGS: NativeFunction = NativeFunction {
    sig: "strip_tags(string $string, array|string|null $allowed_tags = null): string",
    handler: strip_tags,
};

pub fn register(r: &mut Registry) {
    r.function(&STRIP_TAGS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::PhpArray;

    fn st(t: &mut TestVm, s: &str, allow: Option<Value>) -> String {
        let mut args = vec![Value::str(s)];
        if let Some(a) = allow {
            args.push(a);
        }
        match t.call("strip_tags", args).unwrap() {
            Value::String(s) => s.to_string_lossy().into_owned(),
            v => panic!("unexpected {v:?}"),
        }
    }

    fn strip(s: &str) -> String {
        String::from_utf8(php_strip_tags_ex(s.as_bytes(), None, false)).unwrap()
    }

    fn allow(s: &str, a: &str) -> String {
        String::from_utf8(php_strip_tags_ex(s.as_bytes(), Some(a.as_bytes()), false)).unwrap()
    }

    #[test]
    fn php_and_comment_blocks() {
        // strip_tags.phpt
        assert_eq!(strip("NEAT <? cool < blah ?> STUFF"), "NEAT  STUFF");
        assert_eq!(strip("NEAT <? cool > blah ?> STUFF"), "NEAT  STUFF");
        assert_eq!(strip("NEAT <!-- cool < blah --> STUFF"), "NEAT  STUFF");
        assert_eq!(strip("NEAT <!-- cool > blah --> STUFF"), "NEAT  STUFF");
        assert_eq!(strip(r#"NEAT <? echo \"\\"\"?> STUFF"#), "NEAT  STUFF");
        assert_eq!(strip(r"NEAT <? echo '\''?> STUFF"), "NEAT  STUFF");
        assert_eq!(strip("TESTS ?!!?!?!!!?!!"), "TESTS ?!!?!?!!!?!!");
    }

    #[test]
    fn basic1() {
        // strip_tags_basic1.phpt
        assert_eq!(strip("<html>hello</html>"), "hello");
        assert_eq!(strip("<?php echo hello ?>"), "");
        assert_eq!(strip("<? echo hello ?>"), "");
        assert_eq!(strip("<% echo hello %>"), "");
        assert_eq!(
            strip("<script language=\"PHP\"> echo hello </script>"),
            " echo hello "
        );
        assert_eq!(
            strip("<script language=\\\"PHP\\\"> echo hello </script>"),
            " echo hello "
        );
        assert_eq!(strip("<html><b>hello</b><p>world</p></html>"), "helloworld");
        assert_eq!(strip("<html><!-- COMMENT --></html>"), "");
    }

    #[test]
    fn basic2_allowed_string() {
        // strip_tags_basic2.phpt
        let s = "<html><p>hello</p><b>world</b><a href=\"#fragment\">Other text</a></html><?php echo hello ?>";
        assert_eq!(allow(s, "<html>"), "<html>helloworldOther text</html>");
        assert_eq!(allow(s, "<p>"), "<p>hello</p>worldOther text");
        assert_eq!(
            allow(s, "<a>"),
            "helloworld<a href=\"#fragment\">Other text</a>"
        );
        assert_eq!(allow(s, "<?php"), "helloworldOther text");
        assert_eq!(
            allow(s, "<html><p><a><?php"),
            "<html><p>hello</p>world<a href=\"#fragment\">Other text</a></html>"
        );
    }

    #[test]
    fn variation4_odd_input() {
        // strip_tags_variation4.phpt
        let q = "<p><a><?php<html>";
        assert_eq!(
            allow(
                "<abc>hello</abc> \t\tworld... <ppp>strip_tags_test</ppp>",
                q
            ),
            "hello \t\tworld... strip_tags_test"
        );
        assert_eq!(allow("<%?php hello\t world?%>", q), "");
        assert_eq!(allow("<<htmL>>hello<</htmL>>", q), "<htmL>hello</htmL>");
        assert_eq!(allow("<a.>HtMl text</.a>", q), "HtMl text");
        assert_eq!(
            allow(
                "<nnn>I am a quoted (\") string with special chars like $,\\!,\\@,\\%,\\&</nnn>",
                q
            ),
            "I am a quoted (\") string with special chars like $,\\!,\\@,\\%,\\&"
        );
    }

    #[test]
    fn variations_6_to_11() {
        // strip_tags_variation6.phpt (binary safety)
        assert_eq!(
            strip("<html> I am html string </html>\0<?php I am php string ?>"),
            " I am html string "
        );
        assert_eq!(
            strip("<html> I am html string\0 </html><?php I am php string ?>"),
            " I am html string "
        );
        // strip_tags_variation7.phpt
        let q = "<nnn><abc><%?<<html>>";
        assert_eq!(
            allow(
                "<abc>hello</abc> \t\tworld... <ppp>strip_tags_test</ppp>",
                q
            ),
            "<abc>hello</abc> \t\tworld... strip_tags_test"
        );
        assert_eq!(allow("<%?php hello\t world?%>", q), "");
        assert_eq!(allow("<<htmL>>hello<</htmL>>", q), "<htmL>hello</htmL>");
        assert_eq!(
            allow("<nnn>I am not a valid html text</nnn>", q),
            "<nnn>I am not a valid html text</nnn>"
        );
        // strip_tags_variation8.phpt
        let s =
            "<html>hello</html> \tworld... <p>strip_tags_test\x0b\x0c</p><?php hello\t wo\rrld?>";
        assert_eq!(
            allow(s, "<%?php"),
            "hello \tworld... strip_tags_test\x0b\x0c"
        );
        assert_eq!(
            allow(s, "<<html>>"),
            "<html>hello</html> \tworld... strip_tags_test\x0b\x0c"
        );
        // strip_tags_variation9.phpt
        let q = "<html><a><p><b><?php";
        assert_eq!(
            allow(
                "<html> $ -> This represents the dollar sign</html><?php echo hello ?>",
                q
            ),
            "<html> $ -> This represents the dollar sign</html>"
        );
        assert_eq!(allow("<? <html>hello world\\t</html>?>", q), "");
        // strip_tags_variation11.phpt
        for s in [
            "hello <img title=\"<\"> world",
            "hello <img title=\">\"> world",
            "hello <img title=\">_<\"> world",
            "hello <img title='>_<'> world",
        ] {
            assert_eq!(strip(s), "hello  world", "{s}");
        }
    }

    #[test]
    fn bugs() {
        assert_eq!(
            strip(
                "\n<table>\n    <tr><td>first cell before < first cell after</td></tr>\n</table>"
            ),
            "\n\n    first cell before < first cell after\n"
        );
        let s = "<a href=\"test?test\\!!!test\">test</a>\n<!-- test -->";
        assert_eq!(allow(s, ""), "test\n");
        assert_eq!(allow(s, "<a>"), "<a href=\"test?test\\!!!test\">test</a>\n");
        // bug22008
        let html = "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\">\n<html>\n<head>\n<title>test</title>\n</head>\n<body>\n<b>PHP!</b>\n</body>\n</html>\n";
        assert_eq!(allow(html, "<b>").trim(), "test\n\n\n<b>PHP!</b>");
        // bug23650
        let s = "1:<!-- abc -  -->\n2:<!doctype -- >\n3:\n4:<abc - def>\n5:abc - def\n6:</abc>\n";
        assert_eq!(strip(s), "1:\n2:\n3:\n4:\n5:abc - def\n6:\n");
        assert_eq!(
            allow(s, "<abc>"),
            "1:\n2:\n3:\n4:<abc - def>\n5:abc - def\n6:</abc>\n"
        );
        assert_eq!(strip("<span title=\"test > all\">this</span>"), "this");
        assert_eq!(strip("<span title=\"Bug ' Trigger\">Text</span>"), "Text");
        assert_eq!(
            strip("<div>Bug ' Trigger</div> Missing Text"),
            "Bug ' Trigger Missing Text"
        );
        // bug45485
        assert_eq!(
            strip("This text is shown <?XML:NAMESPACE PREFIX = ST1 /><b>This Text disappears</b>"),
            "This text is shown This Text disappears"
        );
        assert_eq!(
            strip("This text is shown <?xml:NAMESPACE PREFIX = ST1 /><b>This Text disappears</b>"),
            "This text is shown This Text disappears"
        );
        // bug46578
        assert_eq!(strip("<!-- testing I've been to mars -->foobar"), "foobar");
        assert_eq!(strip("<a alt=\"foobar\">foo<!-- foo! --></a>bar"), "foobar");
        assert_eq!(
            strip("<a alt=\"foobar\"/>foo<?= foo! /* <!-- \"cool\" --> */ ?>bar"),
            "foobar"
        );
        assert_eq!(strip("< ax"), "< ax");
        assert_eq!(strip("<! a>"), "");
        assert_eq!(strip("<? ax"), "");
        // bug50847
        let var = format!("<param value=\"{}\" />", "a".repeat(2048));
        assert_eq!(allow(&var, "<param>"), var);
        assert_eq!(strip(&var), "");
        // bug53319
        let s = "<br /><br  />USD<input type=\"text\"/><br/>CDN<br><input type=\"text\" />";
        assert_eq!(
            allow(s, "<input>"),
            "USD<input type=\"text\"/>CDN<input type=\"text\" />"
        );
        assert_eq!(allow(s, "<br><input>"), s);
        assert_eq!(strip(s), "USDCDN");
        assert_eq!(allow("<a/b>", "<a>"), "");
        // bug70720
        assert_eq!(
            strip("<?php $dom->test(); ?> this is a test"),
            " this is a test"
        );
        assert_eq!(
            strip("<?php $xml->test(); ?> this is a test"),
            " this is a test"
        );
        assert_eq!(
            strip("<?xml $xml->test(); ?> this is a test"),
            " this is a test"
        );
        assert_eq!(
            strip("<span class=sf-dump-> this is a test</span>"),
            " this is a test"
        );
        // bug78003
        assert_eq!(strip("<foo<>bar>"), "");
        assert_eq!(strip("<foo<!>bar>"), "");
        assert_eq!(strip("<foo<?>bar>"), "");
        // bug78346
        assert_eq!(strip("<?= '<?= 1 ?>' ?>2"), "2");
        // bug78814
        assert_eq!(allow("<s/trong>b</strong>", "<strong>"), "b</strong>");
        // file/bug79099
        assert_eq!(strip("<?\n\"\n"), "");
        assert_eq!(strip("<\0\n!\n"), "");
        assert_eq!(strip("<\0\n?\n"), "");
    }

    #[test]
    fn allowed_tags_array() {
        // strip_tags_array.phpt
        let mut t = TestVm::new();
        let s = "<p>foo <b>bar</b> <a href=\"#\">foobar</a></p>";
        let list = |vals: Vec<Value>| Value::array(PhpArray::from_values(vals));
        assert_eq!(st(&mut t, s, None), "foo bar foobar");
        assert_eq!(
            st(&mut t, s, Some(list(vec![Value::str("a")]))),
            "foo bar <a href=\"#\">foobar</a>"
        );
        assert_eq!(
            st(
                &mut t,
                s,
                Some(list(vec![Value::str("p"), Value::str("a")]))
            ),
            "<p>foo bar <a href=\"#\">foobar</a></p>"
        );
        assert_eq!(st(&mut t, s, Some(list(vec![]))), "foo bar foobar");
        let mut a = PhpArray::new();
        a.insert_str("p", Value::Bool(true));
        a.insert_str("a", Value::Bool(false));
        assert_eq!(st(&mut t, s, Some(Value::array(a))), "foo bar foobar");
        let mut a = PhpArray::new();
        a.insert_str("p", "a");
        assert_eq!(
            st(&mut t, s, Some(Value::array(a))),
            "foo bar <a href=\"#\">foobar</a>"
        );
        assert_eq!(
            st(&mut t, s, Some(list(vec![Value::Int(1), Value::Int(2)]))),
            "foo bar foobar"
        );
        // Non-string elements go through zval_get_string().
        let r = t.call(
            "strip_tags",
            vec![Value::str(s), list(vec![Value::array(PhpArray::new())])],
        );
        assert_eq!(
            match r.unwrap() {
                Value::String(s) => s.to_vec(),
                _ => unreachable!(),
            },
            b"foo bar foobar"
        );
        assert!(t.output().contains("Warning: Array to string conversion"));
    }

    #[test]
    fn variation2_scalar_allowed_tags() {
        // strip_tags_variation2.phpt: scalars are coerced to string.
        let mut t = TestVm::new();
        let s = "<html><a>hello</a></html><p>world</p><!-- COMMENT --><?php echo hello ?>";
        for v in [
            Value::Int(0),
            Value::Float(10.5),
            Value::Null,
            Value::Bool(true),
            Value::str(""),
        ] {
            assert_eq!(st(&mut t, s, Some(v)), "helloworld");
        }
        let r = t.call(
            "strip_tags",
            vec![Value::str(s), Value::str("<p>"), Value::Int(1)],
        );
        assert_eq!(thrown(r).0, "ArgumentCountError");
    }

    #[test]
    fn allow_list_is_a_c_string() {
        assert_eq!(allow("<b>x</b><i>y</i>", "<b>\0<i>"), "<b>x</b>y");
        assert_eq!(allow("<B>x</B>", "<b>"), "<B>x</B>");
        assert_eq!(allow("<b>x</b>", "<B>"), "<b>x</b>");
        assert!(tag_find(b"< b >", b"<b>"));
        assert!(tag_find(b"<b/>", b"<b>"));
        assert!(!tag_find(b"", b"<b>"));
    }
}
