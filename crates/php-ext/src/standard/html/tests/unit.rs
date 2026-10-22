//! Direct tests of the html.c port (edge cases of the C code not covered by phpt files).

use std::borrow::Cow;

use php_runtime::Value;

use super::super::*;
use super::*;

#[test]
fn constants() {
    let t = crate::testing::TestVm::new();
    for (name, value) in [
        ("HTML_SPECIALCHARS", 0),
        ("HTML_ENTITIES", 1),
        ("ENT_COMPAT", 2),
        ("ENT_QUOTES", 3),
        ("ENT_NOQUOTES", 0),
        ("ENT_IGNORE", 4),
        ("ENT_SUBSTITUTE", 8),
        ("ENT_DISALLOWED", 128),
        ("ENT_HTML401", 0),
        ("ENT_XML1", 16),
        ("ENT_XHTML", 32),
        ("ENT_HTML5", 48),
    ] {
        match t.vm.constant(name) {
            Some(Value::Int(n)) => assert_eq!(n, value, "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn defaults_and_quotes() {
    let mut p = Php::new();
    // default flags: ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401
    assert_eq!(
        p.str("htmlspecialchars", vec![s("<'\"&>")]),
        b"&lt;&#039;&quot;&amp;&gt;"
    );
    assert_eq!(
        p.str("htmlspecialchars", vec![s(b"a\xffb")]),
        "a\u{FFFD}b".as_bytes()
    );
    assert_eq!(
        p.str("htmlentities", vec![s("caf\u{E9} \u{20AC}")]),
        b"caf&eacute; &euro;"
    );
    assert_eq!(
        p.str(
            "htmlspecialchars_decode",
            vec![s("&lt;&#039;&quot;&amp;&gt;&eacute;")]
        ),
        b"<'\"&>&eacute;"
    );
    assert_eq!(
        p.str("html_entity_decode", vec![s("&eacute;&euro;&#x41;&#66;")]),
        "\u{E9}\u{20AC}AB".as_bytes()
    );
    // ' is &#039; in HTML 4.01 and &apos; in the other document types
    for (flags, apos) in [
        (ENT_HTML401, "&#039;"),
        (ENT_XHTML, "&apos;"),
        (ENT_XML1, "&apos;"),
        (ENT_HTML5, "&apos;"),
    ] {
        assert_eq!(
            p.str("htmlspecialchars", vec![s("'"), i(ENT_QUOTES | flags)]),
            apos.as_bytes()
        );
    }
    // htmlentities() in XHTML uses the HTML 4.01 table
    assert_eq!(
        p.str("htmlentities", vec![s("'"), i(ENT_QUOTES | ENT_XHTML)]),
        b"&#039;"
    );
    assert_eq!(
        p.str("htmlentities", vec![s("'"), i(ENT_QUOTES | ENT_HTML5)]),
        b"&apos;"
    );
    // htmlentities() in XML 1.0 only encodes the basic entities
    assert_eq!(
        p.str("htmlentities", vec![s("\u{E9}'"), i(ENT_QUOTES | ENT_XML1)]),
        "\u{E9}&apos;".as_bytes()
    );
    assert_eq!(p.output(), b"");
}

#[test]
fn html5_entities() {
    let mut p = Php::new();
    let h5 = |p: &mut Php, f: &str, st: &[u8]| {
        p.str(f, vec![s(st), i(ENT_QUOTES | ENT_HTML5), s("UTF-8")])
    };
    // two code points
    assert_eq!(
        h5(&mut p, "html_entity_decode", b"&nGt;&NotEqualTilde;&fjlig;"),
        "\u{226B}\u{20D2}\u{2242}\u{338}fj".as_bytes()
    );
    assert_eq!(
        h5(
            &mut p,
            "htmlentities",
            "\u{226B}\u{20D2}fj\u{226B}".as_bytes()
        ),
        b"&nGt;&fjlig;&gg;"
    );
    assert_eq!(h5(&mut p, "htmlentities", b"<\xe2\x83\x92 f"), b"&nvlt; f");
    // the second code point is not consumed when it does not complete an entity
    assert_eq!(
        h5(&mut p, "htmlentities", b"<\xe2\x83\x93"),
        "&lt;\u{20D3}".as_bytes()
    );
    // in other charsets two-code-point entities are not decoded
    let r = p.str(
        "html_entity_decode",
        vec![
            s("&nGt;&fjlig;&gt;"),
            i(ENT_QUOTES | ENT_HTML5),
            s("ISO-8859-1"),
        ],
    );
    assert_eq!(r, b"&nGt;&fjlig;>");
    // ... and the second byte is not mapped to Unicode when encoding
    let r = p.str(
        "htmlentities",
        vec![s(b"fj\xe9"), i(ENT_QUOTES | ENT_HTML5), s("cp1252")],
    );
    assert_eq!(r, b"&fjlig;&eacute;");
    // the longest entity name
    let r = h5(
        &mut p,
        "html_entity_decode",
        b"&CounterClockwiseContourIntegral;",
    );
    assert_eq!(r, "\u{2233}".as_bytes());
    assert_eq!(
        h5(&mut p, "html_entity_decode", b"&apos;&Tab;&NewLine;"),
        b"'\t\n"
    );
}

#[test]
fn numeric_entities() {
    let mut p = Php::new();
    let dec =
        |p: &mut Php, st: &str| p.str("html_entity_decode", vec![s(st), i(ENT_QUOTES), s("UTF-8")]);
    // strtol() accepts a "0x" prefix after "&#x"
    assert_eq!(dec(&mut p, "&#x0x41;&#X0X42;&#x0xg;"), b"AB&#x0xg;");
    assert_eq!(
        dec(&mut p, "&#x10FFFF;&#x110000;&#99999999999999999999999;"),
        b"\xf4\x8f\xbf\xbf&#x110000;&#99999999999999999999999;"
    );
    assert_eq!(
        dec(&mut p, "&#065;&#x0041;&#x;&#;&# 65;"),
        b"AA&#x;&#;&# 65;"
    );
    assert_eq!(dec(&mut p, "&#xD800;&#0;&#9;"), b"&#xD800;&#0;\t");
    // the 4-byte minimum
    assert_eq!(dec(&mut p, "x&lt"), b"x&lt");
    assert_eq!(dec(&mut p, "&lt;&lt"), b"<&lt");
    // not double encoding keeps the valid entities
    let keep = |p: &mut Php, st: &str, flags: i64| {
        p.str(
            "htmlspecialchars",
            vec![s(st), i(flags), s("UTF-8"), Value::Bool(false)],
        )
    };
    assert_eq!(
        keep(&mut p, "&#x0x41;&#1114112;&#1114111;", ENT_QUOTES),
        b"&#x0x41;&amp;#1114112;&#1114111;"
    );
    assert_eq!(
        keep(&mut p, "&apos;&hellip;&nGt;", ENT_QUOTES),
        b"&amp;apos;&hellip;&amp;nGt;"
    );
    assert_eq!(
        keep(&mut p, "&apos;&nGt;", ENT_QUOTES | ENT_HTML5),
        b"&apos;&nGt;"
    );
    assert_eq!(
        keep(&mut p, "&apos;&hellip;", ENT_QUOTES | ENT_XHTML),
        b"&apos;&hellip;"
    );
    assert_eq!(
        keep(&mut p, "&apos;&hellip;", ENT_QUOTES | ENT_XML1),
        b"&apos;&amp;hellip;"
    );
    assert_eq!(
        keep(
            &mut p,
            "&#1;&#x0d;",
            ENT_QUOTES | ENT_HTML5 | ENT_DISALLOWED
        ),
        b"&amp;#1;&amp;#x0d;"
    );
    assert_eq!(keep(&mut p, "&a-b;&", ENT_QUOTES), b"&amp;a-b;&amp;");
}

#[test]
fn decode_quotes_and_doctypes() {
    let mut p = Php::new();
    let dec = |p: &mut Php, f: &str, st: &str, flags: i64| p.str(f, vec![s(st), i(flags)]);
    assert_eq!(
        dec(
            &mut p,
            "html_entity_decode",
            "&quot;&#39;&apos;",
            ENT_NOQUOTES | ENT_HTML5
        ),
        b"&quot;&#39;&apos;"
    );
    assert_eq!(
        dec(
            &mut p,
            "html_entity_decode",
            "&quot;&#39;&apos;",
            ENT_COMPAT | ENT_HTML5
        ),
        b"\"&#39;&apos;"
    );
    assert_eq!(
        dec(
            &mut p,
            "html_entity_decode",
            "&apos;&eacute;",
            ENT_QUOTES | ENT_XML1
        ),
        "'&eacute;".as_bytes()
    );
    assert_eq!(
        dec(
            &mut p,
            "html_entity_decode",
            "&#13;",
            ENT_QUOTES | ENT_HTML5
        ),
        b"&#13;"
    );
    assert_eq!(
        dec(&mut p, "html_entity_decode", "&#13;", ENT_QUOTES),
        b"\r"
    );
    // decoding uses the stricter rules of unicode_cp_is_allowed()
    assert_eq!(
        dec(
            &mut p,
            "html_entity_decode",
            "&#x80;&#xFFFE;",
            ENT_QUOTES | ENT_HTML401
        ),
        "&#x80;\u{FFFE}".as_bytes()
    );
    assert_eq!(
        dec(
            &mut p,
            "html_entity_decode",
            "&#x80;&#xFFFE;",
            ENT_QUOTES | ENT_XML1
        ),
        "\u{80}&#xFFFE;".as_bytes()
    );
    // htmlspecialchars_decode() only decodes &, <, >, " and ' (numeric entities included)
    assert_eq!(
        dec(
            &mut p,
            "htmlspecialchars_decode",
            "&#38;&#x3c;&#62;&#65;&eacute;&apos;",
            ENT_QUOTES
        ),
        b"&<>&#65;&eacute;&apos;"
    );
    assert_eq!(
        dec(
            &mut p,
            "htmlspecialchars_decode",
            "&apos;&#039;",
            ENT_QUOTES | ENT_HTML5
        ),
        b"''"
    );
    assert_eq!(p.output(), b"");
}

#[test]
fn single_byte_charsets() {
    let mut p = Php::new();
    let enc =
        |p: &mut Php, st: &[u8], cs: &str| p.str("htmlentities", vec![s(st), i(ENT_QUOTES), s(cs)]);
    assert_eq!(enc(&mut p, b"\xa4\xbc", "ISO-8859-15"), b"&euro;&OElig;");
    // HTML 4.01 has no entities for Cyrillic letters, HTML5 does
    assert_eq!(enc(&mut p, b"\xc1\xf0", "KOI8-R"), b"\xc1\xf0");
    let r = p.str(
        "htmlentities",
        vec![s(b"\xc1\xf0"), i(ENT_QUOTES | ENT_HTML5), s("KOI8-R")],
    );
    assert_eq!(r, b"&acy;&Pcy;");
    assert_eq!(enc(&mut p, b"\xa9\x81", "cp1252"), b"&copy;\x81");
    assert_eq!(enc(&mut p, b"\xa5\xf0", "MacRoman"), b"&bull;\xf0");
    assert_eq!(enc(&mut p, b"\xf0\xfd", "ISO-8859-5"), b"\xf0&sect;");
    let dec = |p: &mut Php, st: &str, cs: &str| {
        p.str("html_entity_decode", vec![s(st), i(ENT_QUOTES), s(cs)])
    };
    assert_eq!(
        dec(&mut p, "&euro;&OElig;&eacute;", "ISO-8859-15"),
        b"\xa4\xbc\xe9"
    );
    assert_eq!(
        dec(&mut p, "&numero;&sect;&#x45D;", "ISO-8859-5"),
        b"&numero;\xfd&#x45D;"
    );
    assert_eq!(dec(&mut p, "&#x2116;", "ISO-8859-5"), b"\xf0");
    // ENT_SUBSTITUTE uses a numeric entity outside UTF-8
    let r = p.str(
        "htmlspecialchars",
        vec![
            s(b"a\x80b\x81"),
            i(ENT_QUOTES | ENT_SUBSTITUTE),
            s("Shift_JIS"),
        ],
    );
    assert_eq!(r, b"a&#xFFFD;b&#xFFFD;");
    let r = p.str(
        "htmlspecialchars",
        vec![s("\x01"), i(ENT_QUOTES | ENT_DISALLOWED), s("cp1252")],
    );
    assert_eq!(r, b"&#xFFFD;");
    assert_eq!(p.output(), b"");
}

#[test]
fn charset_names() {
    assert_eq!(Charset::from_name(b"UTF-8"), Some(Charset::Utf8));
    assert_eq!(Charset::from_name(b"utf8"), None);
    assert_eq!(Charset::from_name(b"Windows-1252"), Some(Charset::Cp1252));
    assert_eq!(Charset::from_name(b"windows-12520"), None);
    assert_eq!(Charset::from_name(b"EUCJP-WIN"), Some(Charset::EucJp));
    assert_eq!(Charset::from_name(b"koi8-ru"), Some(Charset::Koi8R));
    assert_eq!(Charset::from_name(b"IBM866"), Some(Charset::Cp866));
    assert_eq!(Charset::from_name(b"950"), Some(Charset::Big5));
    assert_eq!(Charset::from_name(b"BIG5-HKSCS"), Some(Charset::Big5Hkscs));
    assert_eq!(Charset::from_name(b"936"), Some(Charset::Gb2312));

    let mut p = Php::new();
    // the charset name ends at a NUL byte, like the C string
    let r = p.str(
        "htmlentities",
        vec![s("\u{E9}"), i(ENT_QUOTES), s("UTF-8\0junk")],
    );
    assert_eq!(r, b"&eacute;");
    let r = p.str(
        "htmlentities",
        vec![s(b"\xe9"), i(ENT_QUOTES), s("cp1252\0")],
    );
    assert_eq!(r, b"&eacute;");
    assert_eq!(p.output(), b"");
    let r = p.str(
        "htmlspecialchars",
        vec![s("<"), i(ENT_QUOTES), s("bogus\0x")],
    );
    assert_eq!(r, b"&lt;");
    assert_eq!(
        String::from_utf8(p.output()).unwrap(),
        "\nWarning: htmlspecialchars(): Charset \"bogus\" is not supported, assuming UTF-8 in /t.php on line 3\n"
    );
    // an empty string or one without '&' never looks at the charset
    assert_eq!(
        p.str("htmlspecialchars", vec![s(""), i(ENT_QUOTES), s("bogus")]),
        b""
    );
    assert_eq!(
        p.str(
            "html_entity_decode",
            vec![s("abc"), i(ENT_QUOTES), s("bogus")]
        ),
        b"abc"
    );
    assert_eq!(p.str("htmlspecialchars_decode", vec![s("&lt;")]), b"<");
    assert_eq!(p.output(), b"");
    assert_eq!(
        p.str(
            "html_entity_decode",
            vec![s("&lt;"), i(ENT_QUOTES), s("bogus")]
        ),
        b"<"
    );
    assert!(
        String::from_utf8(p.output())
            .unwrap()
            .contains("html_entity_decode(): Charset \"bogus\" is not supported")
    );
    // an unsupported default charset warns as well
    p.ini("default_charset", "cp1250");
    assert_eq!(p.str("htmlspecialchars", vec![s("<")]), b"&lt;");
    assert!(
        String::from_utf8(p.output())
            .unwrap()
            .contains("Charset \"cp1250\" is not supported, assuming UTF-8")
    );
    // internal_encoding wins over default_charset
    p.ini("internal_encoding", "ISO-8859-1");
    assert_eq!(p.str("htmlentities", vec![s(b"\xe9")]), b"&eacute;");
}

#[test]
fn translation_tables() {
    let mut p = Php::new();
    let tt = |p: &mut Php, which: i64, flags: i64, cs: &str| {
        let v = p.call(
            "get_html_translation_table",
            vec![i(which), i(flags), s(cs)],
        );
        entries(&v)
    };
    let get = |t: &[(Vec<u8>, Vec<u8>)], k: &[u8]| {
        t.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
    };
    // every doctype
    assert_eq!(
        tt(&mut p, PHP_HTML_ENTITIES, ENT_QUOTES | ENT_HTML401, "UTF-8").len(),
        253
    );
    assert_eq!(
        tt(&mut p, PHP_HTML_ENTITIES, ENT_QUOTES | ENT_XHTML, "UTF-8").len(),
        253
    );
    assert_eq!(
        tt(&mut p, PHP_HTML_ENTITIES, ENT_QUOTES | ENT_XML1, "UTF-8").len(),
        5
    );
    let h5 = tt(&mut p, PHP_HTML_ENTITIES, ENT_QUOTES | ENT_HTML5, "UTF-8");
    assert_eq!(h5.len(), 1511);
    // insertion order follows the code points
    assert_eq!(h5[0], (b"\t".to_vec(), b"&Tab;".to_vec()));
    assert_eq!(get(&h5, "\u{226B}\u{20D2}".as_bytes()).unwrap(), b"&nGt;");
    assert_eq!(get(&h5, b"fj").unwrap(), b"&fjlig;");
    assert_eq!(get(&h5, b"f"), None);
    // ISO-8859-1 keeps the low byte of the second code point, as the C code does
    let latin1 = tt(
        &mut p,
        PHP_HTML_ENTITIES,
        ENT_QUOTES | ENT_HTML5,
        "ISO-8859-1",
    );
    assert_eq!(get(&latin1, b"<\xd2").unwrap(), b"&nvlt;");
    assert_eq!(get(&latin1, b"\xe9").unwrap(), b"&eacute;");
    // single-byte charsets map their bytes (and skip unrepresentable second code points)
    let cp1251 = tt(&mut p, PHP_HTML_ENTITIES, ENT_QUOTES | ENT_HTML5, "cp1251");
    assert_eq!(get(&cp1251, b"\xc0").unwrap(), b"&Acy;");
    assert_eq!(get(&cp1251, b"fj").unwrap(), b"&fjlig;");
    assert!(cp1251.iter().all(|(k, _)| k.len() <= 2));
    let koi8 = tt(&mut p, PHP_HTML_ENTITIES, ENT_COMPAT | ENT_HTML5, "KOI8-R");
    assert_eq!(get(&koi8, b"\xe1").unwrap(), b"&Acy;");
    assert_eq!(get(&koi8, b"\"").unwrap(), b"&quot;");
    assert_eq!(get(&koi8, b"'"), None);
    let koi8 = tt(&mut p, PHP_HTML_ENTITIES, ENT_COMPAT, "KOI8-R");
    assert_eq!(get(&koi8, b"\xe1"), None);
    assert_eq!(get(&koi8, b"\x9c").unwrap(), b"&deg;");
    // multi-byte charsets other than UTF-8 only have the basic entities (and no notice)
    assert_eq!(tt(&mut p, PHP_HTML_ENTITIES, ENT_QUOTES, "BIG5").len(), 5);
    assert_eq!(
        tt(&mut p, PHP_HTML_SPECIALCHARS, ENT_NOQUOTES, "UTF-8").len(),
        3
    );
    assert_eq!(p.output(), b"");
    // any non-zero table selects HTML_ENTITIES
    assert_eq!(tt(&mut p, -7, ENT_QUOTES, "UTF-8").len(), 253);
}

#[test]
fn pure_api() {
    assert!(matches!(
        escape_html(
            b"plain text \xc3\xa9",
            false,
            ENT_QUOTES,
            Charset::Utf8,
            true
        ),
        Cow::Borrowed(_)
    ));
    assert!(matches!(
        escape_html(b"it's", false, ENT_COMPAT, Charset::Utf8, true),
        Cow::Borrowed(_)
    ));
    assert_eq!(
        &*escape_html(b"it's", false, ENT_QUOTES, Charset::Utf8, true),
        b"it&#039;s"
    );
    assert_eq!(
        &*escape_html(b"\xe9<", true, ENT_QUOTES, Charset::Iso8859_1, true),
        b"&eacute;&lt;"
    );
    // invalid sequences without ENT_IGNORE/ENT_SUBSTITUTE give an empty string
    assert_eq!(
        &*escape_html(b"a\xff", false, ENT_QUOTES, Charset::Utf8, true),
        b""
    );
    assert_eq!(
        &*escape_html(
            b"a\xffb",
            false,
            ENT_QUOTES | ENT_IGNORE,
            Charset::Utf8,
            true
        ),
        b"ab"
    );
    assert!(matches!(
        unescape_html(b"no entities", true, ENT_QUOTES, Charset::Utf8),
        Cow::Borrowed(_)
    ));
    assert_eq!(
        &*unescape_html(b"&eacute;&lt;", true, ENT_QUOTES, Charset::Iso8859_1),
        b"\xe9<"
    );
    assert_eq!(
        &*unescape_html(b"&eacute;&lt;", false, ENT_QUOTES, Charset::Utf8),
        b"&eacute;<"
    );

    let mut cursor = 0;
    assert_eq!(
        next_utf8_char("\u{1F600}".as_bytes(), &mut cursor),
        Some(0x1F600)
    );
    assert_eq!(cursor, 4);
    // invalid sequences report how many bytes they span (UTR #36 strategy 2)
    for (bytes, advance) in [
        (&b"\xc2\x41"[..], 1),
        (b"\xe2\x98", 2),
        (b"\xe2\x98\x41", 2),
        (b"\xed\xa0\x80", 3),
        (b"\xf4\x90\x80\x80", 4),
        (b"\xc0\x80", 1),
        (b"\xf5", 1),
    ] {
        let mut cursor = 0;
        assert_eq!(next_utf8_char(bytes, &mut cursor), None, "{bytes:x?}");
        assert_eq!(cursor, advance, "{bytes:x?}");
    }
    let mut cursor = 0;
    assert_eq!(
        next_char(Charset::EucJp, b"\x8f\xa1\xa2", &mut cursor),
        Some(0x8FA1A2)
    );
    let mut cursor = 0;
    assert_eq!(
        next_char(Charset::Big5Hkscs, b"\x81\x80", &mut cursor),
        None
    );
    assert_eq!(cursor, 2);
    let mut cursor = 0;
    assert_eq!(next_char(Charset::Big5, b"\x81\x80", &mut cursor), None);
    assert_eq!(cursor, 1);

    assert!(unicode_cp_is_allowed(0x0C, ENT_HTML5) && !unicode_cp_is_allowed(0x0C, ENT_HTML401));
    assert!(!unicode_cp_is_allowed(0xFDD0, ENT_HTML5) && unicode_cp_is_allowed(0xFDD0, ENT_XML1));
    assert!(
        numeric_entity_is_allowed(0xD800, ENT_HTML5) && !numeric_entity_is_allowed(0x0D, ENT_HTML5)
    );
    assert!(
        numeric_entity_is_allowed(0x01, ENT_HTML401) && !numeric_entity_is_allowed(0x01, ENT_XHTML)
    );
    assert_eq!(map_from_unicode(0x20AC, Charset::Cp1252), Some(0x80));
    assert_eq!(map_from_unicode(0x5C, Charset::Sjis), None);
    assert_eq!(map_from_unicode(0x5C, Charset::Big5), Some(0x5C));
    assert_eq!(map_from_unicode(0x7F, Charset::MacRoman), None);
}

/// Every HTML5 and HTML 4.01 entity decodes to its code points and encodes back to an entity
/// of the same code points.
#[test]
fn entity_tables_round_trip() {
    for (map, doctype) in [
        (&tables::HTML5_NAMES[..], ENT_HTML5),
        (&tables::HTML4_NAMES[..], ENT_HTML401),
    ] {
        for &(name, cp1, cp2) in map {
            let mut expected = utf8(cp1);
            if cp2 != 0 {
                expected.extend(utf8(cp2));
            }
            let ent = format!("&{name};");
            assert_eq!(
                &*unescape_html(ent.as_bytes(), true, ENT_QUOTES | doctype, Charset::Utf8),
                &expected[..],
                "{name}"
            );
            let encoded = escape_html(&expected, true, ENT_QUOTES | doctype, Charset::Utf8, true);
            assert_eq!(
                &*unescape_html(&encoded, true, ENT_QUOTES | doctype, Charset::Utf8),
                &expected[..],
                "{name}"
            );
        }
    }
}
