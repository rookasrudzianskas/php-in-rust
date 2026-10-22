//! htmlspecialchars_decode() / html_entity_decode() phpt ports.

use php_runtime::Value;

use super::super::*;
use super::*;

fn hed(p: &mut Php, input: &[u8], flags: i64, charset: &str) -> Vec<u8> {
    p.str("html_entity_decode", vec![s(input), i(flags), s(charset)])
}

fn hsd(p: &mut Php, input: &[u8], flags: Option<i64>) -> Vec<u8> {
    let mut args = vec![s(input)];
    args.extend(flags.map(i));
    p.str("htmlspecialchars_decode", args)
}

#[test]
fn bug21338_empty() {
    let mut p = Php::new();
    let r = p.str("html_entity_decode", vec![s("")]);
    p.dump(r);
    p.check("bug21338.phpt");
}

#[test]
fn bug29119_utf8() {
    let mut p = Php::new();
    let r = hed(
        &mut p,
        b"&ensp;&thinsp;&lsquo;&dagger;&prime;&frasl;&euro;",
        ENT_QUOTES,
        "UTF-8",
    );
    p.dump(bin2hex(&r));
    p.check("bug29119.phpt");
}

#[test]
fn bug61374_not_in_latin1() {
    let mut p = Php::new();
    let r = hed(&mut p, b"&OElig;", 0, "ISO-8859-1");
    p.echo(r);
    p.check("bug61374.phpt");
}

#[test]
fn bug25707_bug43927_no_over_decoding() {
    for (name, cs) in [("bug25707.phpt", "ISO-8859-1"), ("bug43927.phpt", "koi8-r")] {
        let mut p = Php::new();
        for input in [&b"&amp;lt;"[..], b"&amp;#38;", b"&amp;#38;lt;"] {
            let r = hed(&mut p, input, ENT_COMPAT, cs);
            p.dump(r);
        }
        p.check(name);
    }
}

#[test]
fn bug53021_numeric_quotes() {
    let mut p = Php::new();
    let r = hed(&mut p, b"&#233;", ENT_QUOTES, "ISO-8859-1");
    let mut arr = php_runtime::PhpArray::new();
    arr.insert(php_runtime::ArrayKey::Int(1), s(bin2hex(&r)));
    p.var_dump(&Value::array(arr));
    p.echo("double quotes variations:\n");
    for flags in [Some(ENT_NOQUOTES), Some(ENT_QUOTES), Some(ENT_COMPAT), None] {
        for ent in ["&quot;", "&#34;"] {
            let r = match flags {
                Some(f) => hed(&mut p, ent.as_bytes(), f, "UTF-8"),
                None => p.str("html_entity_decode", vec![s(ent)]),
            };
            p.echo(r);
            p.echo("\n");
        }
    }
    p.echo("\nsingle quotes variations:\n");
    for flags in [Some(ENT_NOQUOTES), Some(ENT_QUOTES), Some(ENT_COMPAT), None] {
        let r = match flags {
            Some(f) => hed(&mut p, b"&#39;", f, "UTF-8"),
            None => p.str("html_entity_decode", vec![s("&#39;")]),
        };
        p.echo(r);
        p.echo("\n");
    }
    p.check("bug53021.phpt");
}

#[test]
fn html_charset_lookup() {
    let mut p = Php::new();
    for cs in [
        "WINDOWS-1252",
        "wInDoWs-1252",
        "MACROMAN",
        "Windows-12520",
        "MacRoman-but-longer",
    ] {
        let r = hed(&mut p, b"&eacute;", ENT_QUOTES, cs);
        p.echo(format!("'{cs}': {}\n", bin2hex(&r)));
    }
    p.check("html_charset_lookup.phpt");
}

#[test]
fn html_entity_decode1_after_invalid() {
    let arr = [
        "&",
        "&&amp;",
        "&&#x24;",
        "&#&amp;",
        "&#&#x24;",
        "&#x&amp;",
        "&#x&#x24;",
        "&#x1&amp;",
        "&#x1&#x24;",
        "&#x20&amp;",
        "&#x20&#x24;",
        "&#1&amp;",
        "&#1&#x24;",
        "&#20&amp;",
        "&#20&#x24;",
        "&a&amp;",
        "&a&#x24;",
        "&aa&amp;",
        "&aa&#x24;",
        "&aa;&amp;",
        "&aa;&#x24;",
        "&;&amp;",
        "&;&#x24;",
    ];
    let mut p = Php::new();
    for (n, ent) in arr.into_iter().enumerate() {
        let a = hed(&mut p, ent.as_bytes(), ENT_QUOTES, "UTF-8");
        if n % 2 == 1 {
            let b = hsd(&mut p, ent.as_bytes(), Some(ENT_QUOTES));
            assert_eq!(a, b, "{ent}");
        }
        p.echo(a);
        p.echo("\n");
    }
    p.echo("Done.\n");
    p.check("html_entity_decode1.phpt");
}

#[test]
fn html_entity_decode2_apos() {
    let mut p = Php::new();
    for (label, flags) in [
        ("HTML 4.01 implicit (shouldn't decode)", ENT_QUOTES),
        ("HTML 4.01 (shouldn't decode)", ENT_QUOTES | ENT_HTML401),
        ("HTML 5", ENT_QUOTES | ENT_HTML5),
        ("XHTML 1.0", ENT_QUOTES | ENT_XHTML),
        ("XML 1.0", ENT_QUOTES | ENT_XML1),
    ] {
        p.echo(format!("*** {label} ***\n"));
        let r = hed(&mut p, b"&apos;", flags, "UTF-8");
        p.echo(r);
        p.echo("\n");
    }
    p.echo("Done.\n");
    p.check("html_entity_decode2.phpt");
}

#[test]
fn html_entity_decode3_disallowed_numeric() {
    let tests = [
        "&#0;",
        "&#1;",
        "&#x09;",
        "&#x0A;",
        "&#x0B;",
        "&#x0C;",
        "&#x0D;",
        "&#x0E;",
        "&#x1F;",
        "&#x20;",
        "&#x27;",
        "&#x7F;",
        "&#x80;",
        "&#x9F;",
        "&#xA0;",
        "&#xD7FF;",
        "&#xD800;",
        "&#xDFFF;",
        "&#xE000;",
        "&#xFFFE;",
        "&#xFFFF;",
        "&#xFDCF;",
        "&#xFDD0;",
        "&#xFDEF;",
        "&#xFDF0;",
        "&#x2FFFE;",
        "&#x2FFFF;",
    ];
    let mut p = Php::new();
    let mut first = true;
    for (label, flags) in [
        ("HTML 4.01", Some(ENT_QUOTES | ENT_HTML401)),
        ("XHTML 1.0", Some(ENT_QUOTES | ENT_XHTML)),
        ("HTML5", Some(ENT_QUOTES | ENT_HTML5)),
        ("XML 1.0", Some(ENT_QUOTES | ENT_XML1)),
        ("Default options", None),
    ] {
        let sep = if flags.is_some() { "  " } else { " " };
        p.echo(format!(
            "{}*** {label}{sep}***\n",
            if first { "" } else { "\n" }
        ));
        first = false;
        for t in tests {
            let dec = match flags {
                Some(f) => hed(&mut p, t.as_bytes(), f, "UTF-8"),
                None => p.str("html_entity_decode", vec![s(t)]),
            };
            let state = if dec == t.as_bytes() {
                "NOT DECODED"
            } else {
                "DECODED"
            };
            p.echo(format!("{t}\t{state}\n"));
        }
    }
    p.echo("\nDone.\n");
    p.check("html_entity_decode3.phpt");
}

#[test]
fn html_entity_decode_html4_conformance() {
    let script = phpt_file("html_entity_decode_html4.phpt");
    let map = dq_literals_between(&script, "$map = array(", ");");
    assert_eq!(map.len(), 252);
    let mut p = Php::new();
    for ent in map {
        let r = hed(&mut p, &ent, ENT_QUOTES, "UTF-8");
        p.echo(format!("{}\n", bin2hex(&r)));
    }
    p.check("html_entity_decode_html4.phpt");
}

#[test]
fn html_entity_decode_html5_conformance() {
    let script = phpt_file("html_entity_decode_html5.phpt");
    let map = dq_literals_between(&script, "$map = array(", ");");
    assert_eq!(map.len(), 2125);
    let mut p = Php::new();
    for ent in map {
        let de = hed(&mut p, &ent, ENT_QUOTES | ENT_HTML5, "UTF-8");
        p.echo([&ent[..], b" => ", &de, b" ", bin2hex(&de).as_bytes(), b"\n"].concat());
    }
    p.check("html_entity_decode_html5.phpt");
}

/// html_entity_decode_<charset>.phpt: numeric entities of each character of a single-byte
/// charset, and of its bytes.
#[test]
fn html_entity_decode_single_byte_charsets() {
    for (name, charset, special) in [
        ("html_entity_decode_cp866.phpt", "CP866", None),
        ("html_entity_decode_iso8859-5.phpt", "ISO-8859-5", None),
        ("html_entity_decode_iso8859-15.phpt", "ISO-8859-15", None),
        ("html_entity_decode_koi8-r.phpt", "KOI8-R", None),
        (
            "html_entity_decode_macroman.phpt",
            "MacRoman",
            Some(&["&#x7F;"][..]),
        ),
        (
            "html_entity_decode_win1251.phpt",
            "WINDOWS-1251",
            Some(&["&#x98;"][..]),
        ),
        (
            "html_entity_decode_win1252.phpt",
            "WINDOWS-1252",
            Some(&["&#x81;", "&#x8D;", "&#x8F;", "&#x90;", "&#x9D;"][..]),
        ),
    ] {
        let script = phpt_file(name);
        let map = charset_map_lines(&script);
        assert!(map.len() > 20, "{name}");
        let mut p = Php::new();
        for ent in special.unwrap_or_default() {
            let res = hed(&mut p, ent.as_bytes(), ENT_QUOTES, charset);
            p.echo(format!("Special test for {ent} (shouldn't decode):\n"));
            p.echo(res);
            p.echo("\n\n");
        }
        for (u, b, label) in map {
            let ent = format!("&#x{u:X};");
            let res = hed(&mut p, ent.as_bytes(), ENT_QUOTES, charset);
            p.echo(format!("{label}: {ent} => {}\n", bin2hex(&res)));
            let ent = format!("&#x{b:X};");
            let res = hed(&mut p, ent.as_bytes(), ENT_QUOTES, charset);
            let res = if res.starts_with(b"&#") {
                res
            } else {
                bin2hex(&res).into_bytes()
            };
            p.echo(format!("{ent} => "));
            p.echo(res);
            p.echo("\n\n");
        }
        p.check(name);
    }
}

#[test]
fn htmlspecialchars_decode_basic() {
    let st = b"Roy&#039;s height &gt; Sam&#039;s height. 13 &lt; 25. 1111 &amp; 0000 = 0000. &quot; double quoted string &quot;";
    let mut p = Php::new();
    p.echo("*** Testing htmlspecialchars_decode() : basic functionality ***\n");
    for flags in [None, Some(ENT_COMPAT), Some(ENT_NOQUOTES), Some(ENT_QUOTES)] {
        for _ in 0..2 {
            let r = hsd(&mut p, st, flags);
            p.dump(r);
        }
    }
    p.echo("Done");
    p.check("htmlspecialchars_decode_basic.phpt");
}

#[test]
fn htmlspecialchars_decode_variation3_heredoc() {
    let strings: [&[u8]; 6] = [
        b"",
        b"",
        b"<html>Roy&#039;s height &gt; Sam&#039;s height\n13 &lt; 25\n1111 &amp; 0000 = 0000\n&quot;This is a double quoted string&quot;",
        b"<html>Roy&#039;s height\r &gt; Sam\t&#039;s height\n1111\t\t &amp; 0000\x0b\x0b = \x0c0000\n&quot; heredoc\ndouble quoted string. with\x0bdifferent\x0cwhite\x0bspaces&quot;",
        b"<html>11 &lt; 12. 123 string 4567\n&quot;string&quot; 1111\t &amp; 0000\t = 0000\n;",
        b"<html>&lt; This's a string with quotes:\n\"strings in double quote\" &amp;\n'strings in single quote' &quot;\nthis\\line is &#039;single quoted&#039; /with\\slashes </html>",
    ];
    let mut p = Php::new();
    p.echo("*** Testing htmlspecialchars_decode() : usage variations ***\n");
    for (n, st) in strings.into_iter().enumerate() {
        p.echo(format!("-- Iteration {} --\n", n + 1));
        let r = hsd(&mut p, st, None);
        p.dump(r);
    }
    p.echo("Done\n");
    p.check("htmlspecialchars_decode_variation3.phpt");
}

/// htmlspecialchars_decode_variation{4,5}: the same strings single- and double-quoted.
#[test]
fn htmlspecialchars_decode_variation4_5_quoted() {
    let raw: [&str; 5] = [
        r"Roy&#039s height &gt; Sam&#039;s \$height... 1111 &ap; 0000 = 0000... &quot; double quote string &quot;",
        r"Roy&#039;s height &gt; Sam&#039;s height... \t\t 13 &lt; 15...\n\r &quot; double quote\f\v string &quot;",
        r"\nRoy&#039;s height &gt\t; Sam&#039;s\v height\f",
        r"\r\tRoy&#039;s height &gt\r; Sam\t&#039;s height",
        r"\n 1\t3 &\tgt; 11 but 11 &\tlt; 12",
    ];
    for (name, double) in [
        ("htmlspecialchars_decode_variation4.phpt", false),
        ("htmlspecialchars_decode_variation5.phpt", true),
    ] {
        let mut p = Php::new();
        p.echo("*** Testing htmlspecialchars_decode() : usage variations ***\n");
        for (n, st) in raw.into_iter().enumerate() {
            let value = if double {
                php_dq(st.as_bytes())
            } else {
                st.as_bytes().to_vec()
            };
            p.echo(format!("-- Iteration {} --\n", n + 1));
            for flags in [None, Some(ENT_COMPAT), Some(ENT_NOQUOTES), Some(ENT_QUOTES)] {
                let r = hsd(&mut p, &value, flags);
                p.dump(r);
            }
        }
        p.echo("Done");
        p.check(name);
    }
}

#[test]
fn htmlspecialchars_decode_variation6_binary_safe() {
    let strings: [&[u8]; 4] = [
        b"\tHello $world \0\\&!)The big brown fox jumped over the\t\x0c lazy dog\x0b\n",
        b"\tHello \"world\"\t\x0b \0 This is a valid\t string",
        b"This converts\t decimal to $string1000001Hello world",
        b"This is a binary\t \x0b\x0cstring",
    ];
    let mut p = Php::new();
    p.echo("*** Testing htmlspecialchars_decode() : usage variations ***\n");
    for (n, st) in strings.into_iter().enumerate() {
        p.echo(format!("-- Iteration {} --\n", n + 1));
        let r = hsd(&mut p, st, None);
        if n < 3 {
            p.dump(r);
        } else {
            p.dump(bin2hex(&r));
        }
    }
    p.echo("Done");
    p.check("htmlspecialchars_decode_variation6.phpt");
}

#[test]
fn htmlspecialchars_decode_variation7_numeric() {
    let tests = [
        "&quot;", "&#x22;", "&#34;", "&apos;", "&#39;", "&#x27;", "&amp;", "&#x26;", "&lt;",
        "&gt;", "&#x3C;", "&#60;", "&lt;", "&#x3E;", "&#62;", "&#63;",
    ];
    let mut p = Php::new();
    let mut first = true;
    for (label, flags) in [
        ("HTML 4.01/ENT_QUOTES", ENT_QUOTES | ENT_HTML401),
        ("XHTML 1.0/ENT_QUOTES", ENT_QUOTES | ENT_XHTML),
        ("HTML5/ENT_QUOTES", ENT_QUOTES | ENT_HTML5),
        ("XML 1.0/ENT_QUOTES", ENT_QUOTES | ENT_XML1),
        ("HTML5/ENT_NOQUOTES", ENT_NOQUOTES | ENT_HTML5),
        ("HTML5/ENT_COMPAT", ENT_COMPAT | ENT_HTML5),
    ] {
        p.echo(format!(
            "{}*** {label}  ***\n",
            if first { "" } else { "\n" }
        ));
        first = false;
        for t in tests {
            let dec = hsd(&mut p, t.as_bytes(), Some(flags));
            let state = if dec == t.as_bytes() {
                "NOT DECODED"
            } else {
                "DECODED"
            };
            p.echo(format!("{t}\t{state}\n"));
        }
    }
    p.echo("\nDone.\n");
    p.check("htmlspecialchars_decode_variation7.phpt");
}
