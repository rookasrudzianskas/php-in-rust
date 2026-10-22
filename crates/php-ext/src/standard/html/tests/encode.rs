//! htmlspecialchars() / htmlentities() phpt ports.

use php_runtime::Value;

use super::super::*;
use super::*;

/// `htmlspecialchars()`/`htmlentities()` with flags and charset.
fn enc(p: &mut Php, f: &str, input: &[u8], flags: i64, charset: &[u8]) -> Vec<u8> {
    p.str(f, vec![s(input), i(flags), s(charset)])
}

#[test]
fn htmlentities01_cp1252() {
    let mut p = Php::new();
    let r = enc(
        &mut p,
        "htmlentities",
        b"\x82\x86\x99\x9f",
        ENT_QUOTES,
        b"cp1252",
    );
    p.dump(r);
    let r = enc(
        &mut p,
        "htmlentities",
        b"\x80\xa2\xa3\xa4\xa5",
        ENT_QUOTES,
        b"cp1252",
    );
    p.dump(r);
    p.check("htmlentities01.phpt");
}

/// htmlentities{05,06,07,10,11,12}: the charset comes from the ini settings.
#[test]
fn htmlentities_default_charset_single_byte() {
    for (name, ini, value, label) in [
        (
            "htmlentities05.phpt",
            "internal_encoding",
            "cp1252",
            "Windows-1252",
        ),
        ("htmlentities10.phpt", "default_charset", "cp1252", "cp1252"),
        (
            "htmlentities06.phpt",
            "internal_encoding",
            "ISO-8859-15",
            "ISO-8859-15",
        ),
        (
            "htmlentities11.phpt",
            "default_charset",
            "ISO-8859-15",
            "ISO-8859-15",
        ),
        (
            "htmlentities07.phpt",
            "internal_encoding",
            "ISO-8859-1",
            "ISO-8859-1",
        ),
        (
            "htmlentities12.phpt",
            "default_charset",
            "ISO-8859-1",
            "ISO-8859-1",
        ),
    ] {
        let mut p = Php::new();
        p.ini(ini, value);
        // mb_internal_encoding() / ini_get() output
        p.echo(format!("{label}\n"));
        let inputs: &[&[u8]] = match value {
            "cp1252" => &[b"\x82\x86\x99\x9f", b"\x80\xa2\xa3\xa4\xa5"],
            "ISO-8859-15" => &[b"\xbc\xbd\xbe"],
            _ => &[b"\xe4\xf6\xfc"],
        };
        for input in inputs {
            let r = enc(&mut p, "htmlentities", input, ENT_QUOTES, b"");
            p.dump(r);
        }
        p.check(name);
    }
}

/// htmlentities{08,09,13,14}: multi-byte charsets other than UTF-8 only get the basic
/// entities, with a notice.
#[test]
fn htmlentities_default_charset_multi_byte() {
    for (name, ini, value, label, input, hex) in [
        (
            "htmlentities08.phpt",
            "internal_encoding",
            "EUC-JP",
            "EUC-JP",
            &b"\xa1\xa2\xa1\xa3\xa1\xa4"[..],
            false,
        ),
        (
            "htmlentities13.phpt",
            "default_charset",
            "EUC-JP",
            "EUC-JP",
            b"\xa1\xa2\xa1\xa3\xa1\xa4",
            false,
        ),
        (
            "htmlentities09.phpt",
            "internal_encoding",
            "Shift_JIS",
            "SJIS",
            b"\x81\x41\x81\x42\x81\x43",
            true,
        ),
        (
            "htmlentities14.phpt",
            "default_charset",
            "Shift_JIS",
            "Shift_JIS",
            b"\x81\x41\x81\x42\x81\x43",
            false,
        ),
    ] {
        let mut p = Php::new();
        p.ini(ini, value);
        p.echo(format!("{label}\n"));
        let r = enc(&mut p, "htmlentities", input, ENT_QUOTES, b"");
        assert_eq!(r, input);
        if hex {
            p.dump(bin2hex(&r));
        } else {
            p.dump(r);
        }
        p.check(name);
    }
}

#[test]
fn htmlentities02_03_no_locale_guessing() {
    for name in ["htmlentities02.phpt", "htmlentities03.phpt"] {
        let mut p = Php::new();
        p.ini("default_charset", "");
        p.ini("internal_encoding", "");
        let input: &[u8] = if name == "htmlentities02.phpt" {
            b"\xbc\xbd\xbe"
        } else {
            b"\xe4\xf6\xfc"
        };
        let r = if name == "htmlentities02.phpt" {
            enc(&mut p, "htmlentities", input, ENT_QUOTES, b"")
        } else {
            p.str("htmlentities", vec![s(input), i(ENT_QUOTES)])
        };
        p.dump(r);
        p.check(name);
    }
}

#[test]
fn htmlentities16_cp1251() {
    let mut p = Php::new();
    p.ini("internal_encoding", "cp1251");
    let input = b"\x88\xa9\xf0\xee\xf1\xea\xee\xf8\xed\xfb\xe9";
    p.dump(bin2hex(input));
    let r = enc(&mut p, "htmlentities", input, ENT_QUOTES, b"");
    p.dump(bin2hex(&r));
    let r = enc(&mut p, "htmlentities", input, ENT_QUOTES | ENT_HTML5, b"");
    p.dump(r);
    p.check("htmlentities16.phpt");
}

#[test]
fn htmlentities25_default_utf8() {
    let mut p = Php::new();
    p.ini("default_charset", "");
    p.ini("internal_encoding", "");
    // the deprecation is emitted at startup by mbstring
    p.echo("Deprecated: PHP Startup: Use of mbstring.internal_encoding is deprecated in Unknown on line 0\n");
    let r = p.str("htmlentities", vec![s("äöü")]);
    p.dump(r);
    p.check("htmlentities25.phpt");
}

#[test]
fn htmlentities_iso8859_1() {
    let mut p = Php::new();
    let input = b"<>\"&\xe5\xc4\n";
    let sc = enc(&mut p, "htmlspecialchars", input, ENT_COMPAT, b"ISO-8859-1");
    p.echo(&sc);
    let ent = enc(&mut p, "htmlentities", input, ENT_COMPAT, b"ISO-8859-1");
    p.echo(&ent);
    let r = enc(&mut p, "html_entity_decode", &sc, ENT_COMPAT, b"ISO-8859-1");
    p.echo(r);
    let r = enc(
        &mut p,
        "html_entity_decode",
        &ent,
        ENT_COMPAT,
        b"ISO-8859-1",
    );
    p.echo(r);
    p.check("htmlentities.phpt");
}

#[test]
fn htmlentities17_table() {
    let tests: [(&str, &str); 10] = [
        ("&oplus;", "e28a95"),
        ("&otimes;", "e28a97"),
        ("&perp;", "e28aa5"),
        ("&sdot;", "e28b85"),
        ("&lceil;", "e28c88"),
        ("&rceil;", "e28c89"),
        ("&lfloor;", "e28c8a"),
        ("&rfloor;", "e28c8b"),
        ("&lang;", "e28ca9"),
        ("&rang;", "e28caa"),
    ];
    let unhex = |h: &str| {
        (0..h.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
            .collect::<Vec<u8>>()
    };
    let mut p = Php::new();
    for (_, hex) in tests {
        let r = enc(&mut p, "htmlentities", &unhex(hex), ENT_QUOTES, b"UTF-8");
        p.dump(r);
    }
    for (ent, _) in tests {
        let r = enc(
            &mut p,
            "html_entity_decode",
            ent.as_bytes(),
            ENT_QUOTES,
            b"UTF-8",
        );
        p.dump(bin2hex(&r[..3]));
    }
    p.check("htmlentities17.phpt");
}

#[test]
fn htmlentities18_no_double_encode() {
    let tests: [&[u8]; 10] = [
        b"abc",
        b"abc&amp;sfdsa",
        b"test&#043;s &amp; some more &#68;",
        b"test&#x2b;s &amp; some more &#X44;",
        b"&; &amp &#a; &9; &#xyz;",
        b"&kffjadfdhsjfhjasdhffasdfas;",
        b"&#8787978789",
        b"&",
        b"&&amp;&",
        b"&ab&amp;&",
    ];
    let mut p = Php::new();
    for t in tests {
        for f in ["htmlentities", "htmlspecialchars"] {
            let r = p.str(
                f,
                vec![s(t), i(ENT_QUOTES), Value::Null, Value::Bool(false)],
            );
            p.dump(r);
        }
    }
    p.check("htmlentities18.phpt");
}

#[test]
fn htmlentities19_substitute() {
    let tests: [&[u8]; 3] = [
        b"\x41\xC2\x3E\x42",
        b"\xE3\x80\x22",
        b"\x41\x98\xBA\x42\xE2\x98\x43\xE2\x98\xBA\xE2\x98",
    ];
    let mut p = Php::new();
    for t in tests {
        for f in ["htmlentities", "htmlspecialchars"] {
            let a = enc(&mut p, f, t, ENT_QUOTES | ENT_SUBSTITUTE, b"UTF-8");
            p.dump(&a);
            p.dump(bin2hex(&a));
        }
    }
    p.check("htmlentities19.phpt");
}

const DOCTYPES: [(i64, &str); 4] = [
    (ENT_HTML401, "HTML 4.01"),
    (ENT_XHTML, "XHTML 1.0"),
    (ENT_HTML5, "HTML 5"),
    (ENT_XML1, "XML 1.0"),
];

#[test]
fn htmlentities20_disallowed() {
    let tests: [u32; 26] = [
        0x00, 0x01, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x1F, 0x20, 0x7F, 0x80, 0x9F, 0xA0, 0xD7FF,
        0xD800, 0xDFFF, 0xE000, 0xFFFE, 0xFFFF, 0xFDCF, 0xFDD0, 0xFDEF, 0xFDF0, 0x2FFFE, 0x2FFFF,
    ];
    let mut p = Php::new();
    for (n, (flag, label)) in DOCTYPES.into_iter().enumerate() {
        p.echo(format!(
            "{}*** Testing {label} ***\n",
            if n == 0 { "" } else { "\n" }
        ));
        for cp in tests {
            let t = utf8(cp);
            let a = enc(&mut p, "htmlentities", &t, flag | ENT_DISALLOWED, b"UTF-8");
            let b = enc(
                &mut p,
                "htmlspecialchars",
                &t,
                flag | ENT_DISALLOWED,
                b"UTF-8",
            );
            if a.is_empty() && b.is_empty() {
                p.echo(format!("{cp:05X}: INVALID SEQUENCE\n"));
            } else {
                p.echo(format!("{cp:05X}: {} {}\n", bin2hex(&a), bin2hex(&b)));
            }
        }
    }
    p.check("htmlentities20.phpt");
}

#[test]
fn htmlentities21_disallowed_charsets() {
    let tests: [u8; 14] = [
        0x00, 0x01, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x1F, 0x20, 0x7F, 0x80, 0x9F, 0xA0,
    ];
    let mut p = Php::new();
    let mut first = true;
    for charset in ["Windows-1251", "SJIS"] {
        for (flag, label) in DOCTYPES {
            p.echo(format!(
                "{}*** Testing {label}/{charset} ***\n",
                if first { "" } else { "\n" }
            ));
            first = false;
            for c in tests {
                let a = enc(
                    &mut p,
                    "htmlentities",
                    &[c],
                    flag | ENT_DISALLOWED,
                    charset.as_bytes(),
                );
                let b = enc(
                    &mut p,
                    "htmlspecialchars",
                    &[c],
                    flag | ENT_DISALLOWED,
                    charset.as_bytes(),
                );
                if a.is_empty() && b.is_empty() {
                    p.echo(format!("{c:05X}: INVALID SEQUENCE\n"));
                } else {
                    p.echo(format!("{c:05X}: {} {}\n", bin2hex(&a), bin2hex(&b)));
                }
            }
        }
    }
    let out = p.output();
    // SJIS produces notices for htmlentities()
    check_phpt("htmlentities21.phpt", &out);
}

#[test]
fn htmlentities22_disallowed_no_double_encode() {
    let tests: [&str; 27] = [
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
        "&#x110000;",
    ];
    let mut p = Php::new();
    let run = |p: &mut Php, flag: i64, flag2: i64, charset: &str| {
        for t in tests {
            let args = |p: &mut Php, f: &str| {
                p.str(
                    f,
                    vec![s(t), i(flag | flag2), s(charset), Value::Bool(false)],
                )
            };
            let a = args(p, "htmlentities");
            let b = args(p, "htmlspecialchars");
            if a == b {
                let changed = if a == t.as_bytes() {
                    "NOT CHANGED"
                } else {
                    "CHANGED"
                };
                p.echo(format!("{t}\t{changed}\n"));
            } else {
                p.echo(format!("{t}\tCHANGED ("));
                p.echo(&a);
                p.echo(", ");
                p.echo(&b);
                p.echo(")\n");
            }
        }
    };
    for (n, (flag, label)) in DOCTYPES.into_iter().enumerate() {
        p.echo(format!(
            "{}*** Testing {label} ***\n",
            if n == 0 { "" } else { "\n" }
        ));
        run(&mut p, flag, ENT_DISALLOWED, "UTF-8");
    }
    p.echo("\n*** Testing 5 without the flag ***\n");
    run(&mut p, ENT_HTML5, 0, "UTF-8");
    p.echo("\n*** Testing HTML 5 with another single-byte encoding ***\n");
    run(&mut p, ENT_HTML5, ENT_DISALLOWED, "Windows-1251");
    p.echo("\n*** Testing HTML 5 with another multibyte-byte encoding ***\n");
    run(&mut p, ENT_HTML5, ENT_DISALLOWED, "SJIS");
    p.check("htmlentities22.phpt");
}

#[test]
fn htmlentities23_substitute_eucjp() {
    let tests: [&[u8]; 13] = [
        b"\x8F\xA1\xFF",
        b"\x8F\xA1",
        b"\x8F",
        b"\x8F\xA0",
        b"\x8F\xA1\x21",
        b"\x8F\x21",
        b"\x8E\xAE",
        b"\x8E",
        b"\x8E\x21",
        b"\xB2\xFF",
        b"\xB2",
        b"\xB2\x21",
        b"\xA0",
    ];
    let mut p = Php::new();
    for t in tests {
        for f in ["htmlentities", "htmlspecialchars"] {
            let a = enc(&mut p, f, t, ENT_QUOTES | ENT_SUBSTITUTE, b"EUC-JP");
            p.dump(&a);
            p.dump(bin2hex(&a));
        }
        p.echo("\n");
    }
    p.check("htmlentities23.phpt");
}

/// htmlentities24.phpt and htmlspecialchars.phpt (both encode the literal "chr($i)").
#[test]
fn htmlentities24_and_htmlspecialchars() {
    let mut p = Php::new();
    p.echo("*** Retrieving htmlentities for 256 characters ***\n");
    for n in 0..256 {
        let r = p.str("htmlentities", vec![s(format!("chr({n})"))]);
        p.dump(bin2hex(&r));
    }
    p.echo("\n*** Testing htmlentities() with NULL as first,second and third argument ***\n");
    for flags in [ENT_QUOTES, ENT_NOQUOTES, ENT_COMPAT] {
        let r = p.str(
            "htmlentities",
            vec![s(b"\x82\x86\x99\x9f\x80\x82\x81"), i(flags), Value::Null],
        );
        p.dump(r);
    }
    p.echo("\n*** Checking for proper memory allocation with long string ***\n");
    let long = b"\x82\x86\x99\x9f\x80\x82\x86\x84\x80\x89\x85\x83\x86\x84\x80\x91\x83\x91\x86\x87\x85\x86\x88\x82\x89\x92\x91\x83";
    let r = enc(&mut p, "htmlentities", long, ENT_QUOTES, b"cp1252");
    p.dump(r);
    p.echo("\n*** Testing a normal string with htmlentities() ***\n");
    let r = p.str("htmlentities", vec![s("<html> This is a test! </html>")]);
    p.dump(r);
    p.echo("\n*** Testing htmlentites() on a quote ***\n");
    let quote = "A 'quote' is <b>bold</b>";
    let r = p.str("htmlentities", vec![s(quote)]);
    p.dump(r);
    for flags in [ENT_QUOTES, ENT_NOQUOTES, ENT_COMPAT] {
        let r = p.str("htmlentities", vec![s(quote), i(flags)]);
        p.dump(r);
    }
    p.echo("Done\n");
    p.check("htmlentities24.phpt");

    let mut p = Php::new();
    p.echo("*** Retrieving htmlspecialchars for 256 characters ***\n");
    for n in 0..256 {
        let r = p.str("htmlspecialchars", vec![s(format!("chr({n})"))]);
        p.dump(bin2hex(&r));
    }
    p.echo("\n*** Testing htmlspecialchars() with NULL as first, second and third argument ***\n");
    for flags in [ENT_NOQUOTES, ENT_QUOTES, ENT_COMPAT] {
        let r = p.str("htmlspecialchars", vec![s("<br>"), i(flags), Value::Null]);
        p.dump(r);
    }
    p.echo("\n*** Checking a long string for proper memory allocation ***\n");
    let r = enc(
        &mut p,
        "htmlspecialchars",
        b"<br>Testing<p>New file.</p><p><br>File <b><i><u>WORKS!!!</i></u></b></p><br><p>End of file!!!</p>",
        ENT_QUOTES,
        b"iso-8859-1",
    );
    p.dump(r);
    p.echo("\n*** Testing a normal string with htmlspecialchars() ***\n");
    let r = enc(
        &mut p,
        "htmlspecialchars",
        b"<br>Testing<p>New file.</p> ",
        ENT_QUOTES,
        b"iso-8859-1",
    );
    p.dump(r);
    p.echo("\n*** Testing htmlspecialchars() on a quote...\n");
    let r = p.str("htmlspecialchars", vec![s(quote)]);
    p.dump(r);
    for flags in [ENT_QUOTES, ENT_NOQUOTES, ENT_COMPAT] {
        let r = p.str("htmlspecialchars", vec![s(quote), i(flags)]);
        p.dump(r);
    }
    p.echo("Done\n");
    p.check("htmlspecialchars.phpt");
}

#[test]
fn htmlspecialchars_basic() {
    let strs: [&[u8]; 5] = [
        b"abc<>\"&\n",
        b"&&abc<>\"&\n",
        b"a>,\\<bc<>\"&\n",
        b"a\\'\\'&bc<>\"&\n",
        b"&amp;&lt;\n",
    ];
    let mut p = Php::new();
    p.echo("*** Testing htmlspecialchars() : basic functionality ***\n");
    p.echo("Basic tests\n");
    let mut n = 1;
    for flags in [None, Some(ENT_NOQUOTES), Some(ENT_COMPAT), Some(ENT_QUOTES)] {
        for st in strs {
            let mut args = vec![s(st)];
            args.extend(flags.map(i));
            let r = p.str("htmlspecialchars", args);
            p.echo(format!("Test {n}: "));
            p.echo(r);
            n += 1;
        }
    }
    p.echo("\nTry with char set option - specify default ISO-8859-1\n");
    for (st, flags) in [
        (strs[0], ENT_NOQUOTES),
        (strs[1], ENT_COMPAT),
        (strs[2], ENT_QUOTES),
        (strs[4], ENT_QUOTES),
    ] {
        let r = enc(&mut p, "htmlspecialchars", st, flags, b"ISO-8859-1");
        p.echo(format!("Test {n}: "));
        p.echo(r);
        n += 1;
    }
    let strs: [&[u8]; 3] = [
        b"&quot;&amp;xyz&gt;abc&quot;\n",
        b"&quot;&amp;123&lt;456&quot;\n",
        b"\"300 < 400\"\n",
    ];
    p.echo("\nTry with double decode FALSE\n");
    for st in strs {
        let r = p.str(
            "htmlspecialchars",
            vec![s(st), i(ENT_NOQUOTES), s("ISO-8859-1"), Value::Bool(false)],
        );
        p.echo(format!("Test {n}: "));
        p.echo(r);
        n += 1;
    }
    p.echo("\nTry with double decode TRUE\n");
    for st in &strs[..2] {
        let r = p.str(
            "htmlspecialchars",
            vec![s(st), i(ENT_NOQUOTES), s("ISO-8859-1"), Value::Bool(true)],
        );
        p.echo(format!("Test {n}: "));
        p.echo(r);
        n += 1;
    }
    p.check("htmlspecialchars_basic.phpt");
}

#[test]
fn htmlspecialchars_unchanged() {
    let mut p = Php::new();
    // the debug_zval_dump() lines: the input string itself is returned
    let input = PhpStr::from("xbcabcabcabcabcabcabcabcabcabc");
    let r = p.call("htmlspecialchars", vec![Value::String(input.clone())]);
    assert!(std::ptr::eq(
        r.as_str().unwrap().as_bytes().as_ptr(),
        input.as_bytes().as_ptr()
    ));
    p.echo("string(30) \"xbcabcabcabcabcabcabcabcabcabc\" refcount(3)\n");
    let r = p.str("htmlspecialchars", vec![s("caf\u{E9}")]);
    assert_eq!(r, "café".as_bytes());
    p.echo("string(5) \"café\" interned\n");
    let input = PhpStr::from("\"it's\" ".repeat(4));
    let r = p.call(
        "htmlspecialchars",
        vec![Value::String(input.clone()), i(ENT_NOQUOTES)],
    );
    assert!(std::ptr::eq(
        r.as_str().unwrap().as_bytes().as_ptr(),
        input.as_bytes().as_ptr()
    ));
    p.echo("string(28) \"\"it's\" \"it's\" \"it's\" \"it's\" \" refcount(3)\n");

    for n in [0, 1, 15, 16, 17, 31, 32, 33] {
        let pre = "a".repeat(n);
        let a = p.str(
            "htmlspecialchars",
            vec![s(format!("{pre}<")), i(ENT_QUOTES)],
        );
        let b = p.str(
            "htmlspecialchars",
            vec![s(format!("{pre}>'")), i(ENT_QUOTES)],
        );
        let c = p.str(
            "htmlspecialchars",
            vec![s(format!("{pre}\"'")), i(ENT_NOQUOTES)],
        );
        let d = p.str(
            "htmlspecialchars",
            vec![s(format!("{pre}\"'")), i(ENT_COMPAT)],
        );
        p.echo(
            [
                a,
                b" ".to_vec(),
                b,
                b" ".to_vec(),
                c,
                b" ".to_vec(),
                d,
                b"\n".to_vec(),
            ]
            .concat(),
        );
    }

    let pre = "\u{E9}\u{20AC}\u{1F600}".repeat(3).into_bytes();
    let r = p.str(
        "htmlspecialchars",
        vec![s([&pre[..], b"\xED\xA0\x80<"].concat()), i(ENT_QUOTES)],
    );
    p.dump(r);
    let r = p.str(
        "htmlspecialchars",
        vec![
            s([&pre[..], b"\xED\xA0\x80<"].concat()),
            i(ENT_QUOTES | ENT_IGNORE),
        ],
    );
    p.dump(r);
    let r = p.str(
        "htmlspecialchars",
        vec![
            s([&pre[..], b"\xF4\x90\x80\x80<"].concat()),
            i(ENT_QUOTES | ENT_SUBSTITUTE),
        ],
    );
    p.dump(r);

    let r = enc(
        &mut p,
        "htmlspecialchars",
        &[&[0xE9; 20][..], b"<"].concat(),
        ENT_QUOTES,
        b"ISO-8859-1",
    );
    p.dump(bin2hex(&r));
    let r = enc(
        &mut p,
        "htmlspecialchars",
        &[&b"a".repeat(20)[..], b"\x81<"].concat(),
        ENT_QUOTES | ENT_SUBSTITUTE,
        b"Shift_JIS",
    );
    p.dump(r);
    let r = p.str(
        "htmlspecialchars",
        vec![
            s("a".repeat(20) + "&amp;&foo &#39;"),
            i(ENT_QUOTES),
            s("UTF-8"),
            Value::Bool(false),
        ],
    );
    p.dump(r);
    p.check("htmlspecialchars_unchanged.phpt");
}

#[test]
fn htmlentities_utf_invalid() {
    let strings: [&[u8]; 25] = [
        b"<",
        b"\xD0",
        b"\xD0\x90",
        b"\xD0\x90\xD0",
        b"\xD0\x90\xD0\xB0",
        b"\xE0",
        b"A\xE0",
        b"\xE0\x80",
        b"\xE0\x79",
        b"\xE0\x80\xBE",
        b"Voil\xE0",
        b"Clich\xE9s",
        b"\xFE",
        b"\xFE\x41",
        b"\xC3\xA9",
        b"\xC3\x79",
        b"\xF7\xBF\xBF\xBF",
        b"\xFB\xBF\xBF\xBF\xBF",
        b"\xFD\xBF\xBF\xBF\xBF\xBF",
        b"\x41\xF7\xF7\x42",
        b"\x42\xFB\xFB\x42",
        b"\x43\xFD\xFD\x42",
        b"\x44\xF7\xF7",
        b"\x45\xFB\xFB",
        b"\x46\xFD\xFD",
    ];
    for (name, extra) in [
        ("htmlentities-utf.phpt", 0),
        ("htmlentities-utf-2.phpt", ENT_IGNORE),
    ] {
        let mut p = Php::new();
        for st in strings {
            for f in ["htmlspecialchars", "htmlentities"] {
                let r = enc(&mut p, f, st, ENT_QUOTES | extra, b"utf-8");
                p.dump(bin2hex(&r));
            }
        }
        p.check(name);
    }
}

/// htmlentities-utf-3.phpt: validity of UTF-8 sequences (Unicode 5.2, section 3.9, D92).
#[test]
fn htmlentities_utf_validity() {
    let ranges: [&[(u8, u8)]; 9] = [
        &[(0x00, 0x7F)],
        &[(0xC2, 0xDF), (0x80, 0xBF)],
        &[(0xE0, 0xE0), (0xA0, 0xBF), (0x80, 0xBF)],
        &[(0xE1, 0xEC), (0x80, 0xBF), (0x80, 0xBF)],
        &[(0xED, 0xED), (0x80, 0x9F), (0x80, 0xBF)],
        &[(0xEE, 0xEF), (0x80, 0xBF), (0x80, 0xBF)],
        &[(0xF0, 0xF0), (0x90, 0xBF), (0x80, 0xBF), (0x80, 0xBF)],
        &[(0xF1, 0xF3), (0x80, 0xBF), (0x80, 0xBF), (0x80, 0xBF)],
        &[(0xF4, 0xF4), (0x80, 0x8F), (0x80, 0xBF), (0x80, 0xBF)],
    ];
    let is_valid = |seq: &[u8]| {
        for l in ranges {
            if (l[0].0..=l[0].1).contains(&seq[0]) {
                return l.len() == seq.len()
                    && (1..seq.len()).all(|n| (l[n].0..=l[n].1).contains(&seq[n]));
            }
        }
        false
    };
    let mut p = Php::new();
    let check = |p: &mut Php, seq: &[u8]| {
        let encoded = enc(p, "htmlspecialchars", seq, ENT_QUOTES, b"UTF-8");
        if !encoded.is_empty() != is_valid(seq) {
            p.echo(format!("Discordance for {}\n", bin2hex(seq)));
        }
    };
    for b1 in 0xC0..0xE0u8 {
        for b2 in 0x80..0xBFu8 {
            check(&mut p, &[b1, b2]);
        }
    }
    for b1 in 0xE0..0xEFu8 {
        for b2 in 0x80..0xBFu8 {
            check(&mut p, &[b1, b2, 0x80]);
            check(&mut p, &[b1, b2, 0xBF]);
        }
    }
    for b1 in 0xF0..0xFFu8 {
        for b2 in 0x80..0xBFu8 {
            for tail in [[0x80, 0x80], [0xBF, 0x80], [0x80, 0xBF], [0xBF, 0xBF]] {
                check(&mut p, &[b1, b2, tail[0], tail[1]]);
            }
        }
    }
    p.echo("Done.\n");
    p.check("htmlentities-utf-3.phpt");
}

#[test]
fn htmlentities_html4_conformance() {
    let mut p = Php::new();
    let table = p.call(
        "get_html_translation_table",
        vec![i(PHP_HTML_ENTITIES), i(ENT_QUOTES), s("UTF-8")],
    );
    let mut table = entries(&table);
    for cp in 0..0x2710u32 {
        if (0xD800..0xE000).contains(&cp) {
            continue;
        }
        let key = utf8(cp);
        if let Some(pos) = table.iter().position(|(k, _)| *k == key) {
            let (_, v) = table.remove(pos);
            p.echo(v);
            p.echo(format!("\tU+{cp:05X}\n"));
        }
    }
    assert!(table.is_empty());
    p.check("htmlentities_html4.phpt");
}

#[test]
fn htmlentities_html5_conformance() {
    let mut p = Php::new();
    for cp in 0..0x1DFFFu32 {
        if (0xD800..0xE000).contains(&cp) {
            continue;
        }
        let st = utf8(cp);
        let r = enc(
            &mut p,
            "htmlentities",
            &st,
            ENT_QUOTES | ENT_HTML5,
            b"UTF-8",
        );
        if r != st {
            p.echo(r);
            p.echo(format!("\tU+{cp:05X}\n"));
        }
    }
    // multicodepoint entities
    let script = String::from_utf8(phpt_file("htmlentities_html5.phpt")).unwrap();
    let pairs: Vec<(u32, u32)> = script
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("array(0x")?
                .strip_suffix("),")
                .map(str::to_owned)
        })
        .map(|l| {
            let (a, b) = l.split_once(", 0x").unwrap();
            (
                u32::from_str_radix(a, 16).unwrap(),
                u32::from_str_radix(b, 16).unwrap(),
            )
        })
        .collect();
    assert_eq!(pairs.len(), 65);
    for (a, b) in pairs {
        let st = [utf8(a), utf8(b)].concat();
        let r = enc(
            &mut p,
            "htmlentities",
            &st,
            ENT_QUOTES | ENT_HTML5,
            b"UTF-8",
        );
        p.echo(r);
        p.echo(format!("\tU+{a:05X} U+{b:05X}\n"));
    }
    p.check("htmlentities_html5.phpt");
}

#[test]
fn bug20934_utf8_unchanged() {
    let mut p = Php::new();
    let st = b"\xc3\xa0\xc3\xa1";
    p.echo(format!("{}\n", bin2hex(st)));
    let r = enc(&mut p, "htmlspecialchars", st, ENT_COMPAT, b"UTF-8");
    p.echo(format!("{}\n", bin2hex(&r)));
    p.check("bug20934.phpt");
}

#[test]
fn bug44703_bad_charset() {
    let mut p = Php::new();
    let link = "<a href='test'>Test</a>";
    for cs in ["1", "12", "125", "1252", "12526"] {
        let r = p.str("htmlspecialchars", vec![s(link), i(ENT_COMPAT), s(cs)]);
        p.dump(r);
    }
    for cs in [Some("866"), Some("8666"), None, Some("SJIS"), Some("SjiS")] {
        let r = p.str(
            "htmlspecialchars",
            vec![s("<>"), i(ENT_COMPAT), cs.map_or(Value::Null, s)],
        );
        p.dump(r);
    }
    let r = p.str(
        "htmlspecialchars",
        vec![s("<>"), i(ENT_COMPAT), s("a".repeat(100))],
    );
    p.dump(r);
    p.check("bug44703.phpt");
}

#[test]
fn bug45485_after_strip_tags() {
    // strip_tags() of the test leaves "This text is shown This Text disappears"
    let mut p = Php::new();
    for _ in 0..2 {
        let r = p.str(
            "htmlspecialchars",
            vec![s("This text is shown This Text disappears")],
        );
        p.echo(r);
        p.echo("\n");
    }
    p.check("bug45485.phpt");
}

#[test]
fn bug60965_long_numeric_entity() {
    let mut p = Php::new();
    let script = String::from_utf8(phpt_file("bug60965.phpt")).unwrap();
    let input = script
        .split("htmlspecialchars('")
        .nth(1)
        .unwrap()
        .split("',")
        .next()
        .unwrap();
    let r = p.str(
        "htmlspecialchars",
        vec![s(input), i(ENT_QUOTES), s("UTF-8"), Value::Bool(false)],
    );
    p.echo(r);
    p.echo("\nDone.\n");
    p.check("bug60965.phpt");
}

#[test]
fn html_charset_utf8() {
    let mut p = Php::new();
    for cs in [
        "UTF-8", "utf-8", "uTf-8", "UTF8", "UTF-", "UTF-8 ", "UTF-88", "UTF_8",
    ] {
        let r = enc(
            &mut p,
            "htmlspecialchars",
            b"\xC3\xA9<",
            ENT_QUOTES,
            cs.as_bytes(),
        );
        p.echo(format!("'{cs}': "));
        p.echo(r);
        p.echo("\n");
    }
    p.ini("default_charset", "uTF-8");
    let r = p.str("htmlspecialchars", vec![s(b"\xC3\xA9<"), i(ENT_QUOTES)]);
    p.echo(r);
    p.echo("\n");
    p.check("html_charset_utf8.phpt");
}

#[test]
fn default_charset() {
    let mut p = Php::new();
    let show = |p: &mut Php| {
        for name in [
            "default_charset",
            "internal_encoding",
            "input_encoding",
            "output_encoding",
        ] {
            let v = p.t.vm.ini_get(name).map(|v| v.to_vec()).unwrap_or_default();
            p.dump(v);
        }
    };
    let run = |p: &mut Php| {
        let r = p.str("htmlentities", vec![s(b"\xA3"), i(ENT_HTML5)]);
        p.dump(r);
        let r = enc(p, "htmlentities", b"\xA3", ENT_HTML5, b"cp1252");
        p.dump(r);
        let r = p.str("html_entity_decode", vec![s("&pound;"), i(ENT_HTML5)]);
        p.dump(bin2hex(&r));
        let r = enc(p, "html_entity_decode", b"&pound;", ENT_HTML5, b"cp1252");
        p.dump(bin2hex(&r));
    };
    for name in ["internal_encoding", "input_encoding", "output_encoding"] {
        p.ini(name, "");
    }
    p.echo("*** Default php.ini value ***\n");
    show(&mut p);
    p.echo("*** Runtime change of default_charset ***\n");
    let old = p.t.vm.ini_set("default_charset", "cp1252").unwrap();
    p.dump(old.to_vec());
    p.echo("*** Test with updated default_charset ***\n");
    show(&mut p);
    run(&mut p);
    p.echo("*** Change internal_encoding to empty ***\n");
    let old = p.t.vm.ini_set("internal_encoding", "").unwrap();
    p.dump(old.to_vec());
    show(&mut p);
    run(&mut p);
    p.echo("*** Update input/output_encoding ***\n");
    for name in ["input_encoding", "output_encoding"] {
        let old = p.t.vm.ini_set(name, "ISO-8859-1").unwrap();
        p.dump(old.to_vec());
    }
    show(&mut p);
    run(&mut p);
    p.echo("Done\n");
    p.check("default_charset.phpt");
}

/// bug49785.phpt: input string validation of each multi-byte charset.
#[test]
fn bug49785_validation() {
    let mut p = Php::new();
    let hex = |p: &mut Php, f: &str, input: &[u8], flags: i64, cs: &str| {
        let r = enc(p, f, input, flags, cs.as_bytes());
        p.dump(bin2hex(&r));
    };
    p.echo("UTF-8: basic tests\n");
    for input in [
        &b"\xc1\xbf"[..],
        b"\xc2\x80",
        b"\xc2\x00",
        b"\xc2\xc0",
        b"\xce\x91",
        b"\xce\xb1",
        b"\xdf\xbf",
        b"\xe0\xa0\x80",
        b"\xe0\x9f\xbf",
        b"\xe0\x9f\xbf",
        b"\xe0\x1f\xbf",
        b"\xe0\x9f\x3f",
        b"\xe0\x1f\x3f",
        b"\xe2\x99\xa5",
        b"\xef\xbf\xbf",
        b"\xef\xff\xbf",
        b"\xef\xbf\xff",
        b"\xf0\x8f\xbf\xbf",
        b"\xf0\x90\x80\x80",
        b"\xf7\xbf\xbf\xbf",
        b"\xf7\x3f\xbf\xbf",
        b"\xf7\xbf\x3f\xbf",
        b"\xf7\xbf\xbf\x3f",
        b"\xf7\xff\xbf\xbf",
        b"\xf7\xbf\xff\xbf",
        b"\xf7\xbf\xbf\xff",
        b"\xf8\x88\x80\x80\x80",
    ] {
        hex(&mut p, "htmlentities", input, ENT_QUOTES, "UTF-8");
    }
    p.echo("--\nUTF-8: with ENT_IGNORE\n");
    for input in [
        &b"\xc0\xa0\xc2\x80"[..],
        b"\xe0\x80\x80\xe0\xa0\x80",
        b"\xf0\x80\x80\x80\xf0\x90\x80\x80",
    ] {
        hex(
            &mut p,
            "htmlentities",
            input,
            ENT_QUOTES | ENT_IGNORE,
            "UTF-8",
        );
    }
    p.echo("--\nUTF-8: alternative (invalid) UTF-8 sequence / surrogate pairs\n");
    for input in [
        &b"\xc0\xa6"[..],
        b"\xe0\x80\xa6",
        b"\xf0\x80\x80\xa6",
        b"\xec\xbf\xbf",
        b"\xed\xa0\x80",
        b"\xed\xbf\xbf",
        b"\xee\x80\x80",
    ] {
        hex(&mut p, "htmlspecialchars", input, ENT_QUOTES, "UTF-8");
    }

    let hsc =
        |p: &mut Php, input: &[u8], cs: &str| hex(p, "htmlspecialchars", input, ENT_QUOTES, cs);
    p.echo("--\nShift_JIS: non-lead byte >= 0x80\n");
    hsc(&mut p, b"\x80", "Shift_JIS");
    for c in 0xa0..=0xdfu8 {
        hsc(&mut p, &[c], "Shift_JIS");
    }
    for c in [0xfd, 0xfe, 0xff] {
        hsc(&mut p, &[c], "Shift_JIS");
    }
    p.echo("--\nShift_JIS: incomplete / invalid multibyte sequences\n");
    for c in (0x81..=0x9fu8).chain(0xe0..=0xfc) {
        hsc(&mut p, &[c], "Shift_JIS");
        for t in [0x3f, 0x40, 0x7e, 0x7f, 0x80, 0xfc, 0xfd, 0xfe, 0xff] {
            hsc(&mut p, &[c, t], "Shift_JIS");
        }
    }
    p.echo("--\nEUC-JP: non-lead byte >= 0x80\n");
    for c in (0x80..=0x8du8).chain(0x90..=0x9f) {
        hsc(&mut p, &[c], "EUC-JP");
    }
    hsc(&mut p, b"\xff", "EUC-JP");
    for input in [
        &b"\x8e"[..],
        b"\x8f",
        b"\x8e\xa1",
        b"\x8f\xa1",
        b"\x8e\xa1\xa3",
        b"\x8f\xa1\xa3",
    ] {
        hsc(&mut p, input, "EUC-JP");
    }
    p.echo("--\nEUC-JP: incomplete / invalid multibyte sequences\n");
    for c in 0xa1..=0xfeu8 {
        for prefix in [&[][..], &[0x8e], &[0x8f]] {
            hsc(&mut p, &[prefix, &[c]].concat(), "EUC-JP");
            for t in [0x26, 0x80, 0xa0, 0xa1, 0xfe, 0xff] {
                hsc(&mut p, &[prefix, &[c, t]].concat(), "EUC-JP");
            }
        }
    }
    p.echo("--\nBIG5: non-lead byte >= 0x80\n");
    hsc(&mut p, b"\x80", "BIG5");
    hsc(&mut p, b"\xff", "BIG5");
    p.echo("--\nBIG5: incomplete / invalid multibyte sequences\n");
    for c in 0x81..=0xfeu8 {
        hsc(&mut p, &[c], "BIG5");
        for t in [0x3f, 0x40, 0x7e, 0x7f, 0x80, 0xa0, 0xa1, 0xfe, 0xff] {
            hsc(&mut p, &[c, t], "BIG5");
        }
    }
    p.check("bug49785.phpt");
}
