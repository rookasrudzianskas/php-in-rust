//! String functions: mb_strlen, mb_substr, mb_strpos & co., mb_str_split,
//! mb_str_pad, mb_trim, mb_ucfirst/mb_lcfirst, mb_ord/mb_chr, mb_scrub.

use super::*;

fn utf8_vm() -> TestVm {
    let mut t = TestVm::new();
    t.vm.ini_set_system("internal_encoding", "UTF-8");
    t
}

#[test]
fn strlen() {
    let mut t = utf8_vm();
    // mb_strlen.phpt (UTF-8 and ASCII parts)
    let ascii = "abcdefghijklmnopqrstuvwxyz;]=#0123456789";
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s(ascii), s("ASCII")])), 40);
    let utf8 = "abcde あいうえお 汉字 ελληνικά";
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s(utf8), s("UTF-8")])), 23);
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s(&utf8.repeat(100)), s("UTF-8")])), 2300);
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s(&"Σ".repeat(2048)), s("UTF-8")])), 2048);
    assert_eq!(
        err(&mut t, "mb_strlen", vec![s("x"), s("BAD_NAME")]),
        "ValueError: mb_strlen(): Argument #2 ($encoding) must be a valid encoding, \"BAD_NAME\" given"
    );
    // mb_strlen_basic.phpt
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s("abc def")])), 7);
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s("日本語テキストです。01234５６７８９。"), s("UTF-8")])), 21);
    // Invalid UTF-8: every error marker counts once.
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![b(b"\xff\xfe"), s("UTF-8")])), 2);
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![b(b"\xe3\x81"), s("UTF-8")])), 1);
}

#[test]
fn unknown_encoding_errors() {
    let mut t = utf8_vm();
    // mb_str_unknown_encoding.phpt
    let cases: Vec<(&str, Vec<Value>, &str)> = vec![
        ("mb_chr", vec![Value::Int(1), s("UTF-0")], "Argument #2 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_convert_case", vec![s("coudenys"), Value::Int(0), s("UTF-0")], "Argument #3 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_convert_encoding", vec![s("coudenys"), s("UTF-8"), s("UTF-0")], "Argument #3 ($from_encoding) contains invalid encoding \"UTF-0\""),
        ("mb_convert_kana", vec![s("coudenys"), s("KV"), s("UTF-0")], "Argument #3 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_decode_numericentity", vec![s("coudenys"), Value::empty_array(), s("UTF-0")], "Argument #3 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_ord", vec![s("coudenys"), s("UTF-0")], "Argument #2 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_strcut", vec![s("coudenys"), Value::Int(0), Value::Int(4), s("UTF-0")], "Argument #4 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_strimwidth", vec![s("coudenys"), Value::Int(0), Value::Int(4), s(""), s("UTF-0")], "Argument #5 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_stripos", vec![s("coudenys"), s("cou"), Value::Int(0), s("UTF-0")], "Argument #4 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_stristr", vec![s("coudenys"), s("cou"), Value::Bool(false), s("UTF-0")], "Argument #4 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_strrchr", vec![s("coudenys"), s("cou"), Value::Bool(false), s("UTF-0")], "Argument #4 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_strtoupper", vec![s("coudenys"), s("UTF-0")], "Argument #2 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_strwidth", vec![s("coudenys"), s("UTF-0")], "Argument #2 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_substr", vec![s("coudenys"), Value::Int(0), Value::Int(4), s("UTF-0")], "Argument #4 ($encoding) must be a valid encoding, \"UTF-0\" given"),
        ("mb_substr_count", vec![s("coudenys"), s("c"), s("UTF-0")], "Argument #3 ($encoding) must be a valid encoding, \"UTF-0\" given"),
    ];
    for (f, args, msg) in cases {
        assert_eq!(err(&mut t, f, args), format!("ValueError: {f}(): {msg}"));
    }
}

#[test]
fn substr() {
    let mut t = utf8_vm();
    // mb_substr.phpt (UTF-8 / UTF-32 parts)
    let utf8 = "Greek: Σὲ γνωρίζω ἀπὸ τὴν κόψη Russian: Зарегистрируйтесь";
    let sub = |t: &mut TestVm, from: i64, len: Value| {
        text(&call(t, "mb_substr", vec![s(utf8), Value::Int(from), len, s("UTF-8")]))
    };
    assert_eq!(sub(&mut t, 0, Value::Int(3)), "Gre");
    assert_eq!(sub(&mut t, -1, Value::Null), "ь");
    assert_eq!(sub(&mut t, -5, Value::Int(3)), "йте");
    assert_eq!(sub(&mut t, 1, Value::Null), "reek: Σὲ γνωρίζω ἀπὸ τὴν κόψη Russian: Зарегистрируйтесь");
    assert_eq!(sub(&mut t, 10, Value::Int(0)), "");

    let utf32 = call(&mut t, "mb_convert_encoding", vec![s(utf8), s("UTF-32"), s("UTF-8")]);
    for (from, len, expected) in [(0, Value::Int(3), "Gre"), (-1, Value::Null, "ь"), (-5, Value::Int(3), "йте")] {
        let part = call(&mut t, "mb_substr", vec![utf32.clone(), Value::Int(from), len, s("UTF-32")]);
        let back = call(&mut t, "mb_convert_encoding", vec![part, s("UTF-8"), s("UTF-32")]);
        assert_eq!(text(&back), expected);
    }

    // Agreement with mb_strpos on an invalid UTF-8 string.
    let data = b(b"\xF0AAA<b>");
    let pos = call(&mut t, "mb_strpos", vec![data.clone(), s("<"), Value::Int(0), s("UTF-8")]);
    assert_eq!(text(&call(&mut t, "mb_substr", vec![data, Value::Int(0), pos, s("UTF-8")])), "?AAA");

    // Regression: non-zero start and more than 128 characters.
    let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vestibulum dapibus feugiat ex non cursus. Pellentesque vestibulum tellus sit lectus.";
    assert_eq!(
        vd(&mut t, "mb_substr", vec![s(lorem), Value::Int(19), Value::Int(-1)]),
        "string(121) \"it amet, consectetur adipiscing elit. Vestibulum dapibus feugiat ex non cursus. Pellentesque vestibulum tellus sit lectus\"\n"
    );

    assert_eq!(
        err(&mut t, "mb_substr", vec![s("abc"), Value::Int(i64::MIN)]),
        "ValueError: mb_substr(): Argument #2 ($start) must be between -9223372036854775807 and 9223372036854775807"
    );
    assert_eq!(
        err(&mut t, "mb_substr", vec![s("abc"), Value::Int(0), Value::Int(i64::MIN)]),
        "ValueError: mb_substr(): Argument #3 ($length) must be between -9223372036854775807 and 9223372036854775807"
    );
}

#[test]
fn substr_basic_latin1() {
    // mb_substr_basic.phpt
    let mut t = TestVm::new();
    t.vm.ini_set_system("internal_encoding", "ISO-8859-1");
    assert_eq!(vd(&mut t, "mb_substr", vec![s("ABCDEF"), Value::Int(3)]), "string(3) \"DEF\"\n");
    assert_eq!(
        vd(&mut t, "mb_substr", vec![s("ABCDEF"), Value::Int(3), Value::Int(5), s("ISO-8859-1")]),
        "string(3) \"DEF\"\n"
    );
    let mb = "日本語テキストです。01234５６７８９。";
    // In ISO-8859-1 the UTF-8 bytes are characters.
    assert_eq!(bytes(&call(&mut t, "mb_substr", vec![s(mb), Value::Int(2), Value::Int(7)])), &mb.as_bytes()[2..9]);
    assert_eq!(
        text(&call(&mut t, "mb_substr", vec![s(mb), Value::Int(2), Value::Int(7), s("utf-8")])),
        "語テキストです"
    );
}

#[test]
fn strpos_offsets() {
    let mut t = utf8_vm();
    // mb_strpos_offset_errors.phpt
    for f in ["mb_strpos", "mb_strrpos", "mb_stripos", "mb_strripos"] {
        for off in [3, -3] {
            assert_eq!(
                err(&mut t, f, vec![s("f"), s("bar"), Value::Int(off)]),
                format!("ValueError: {f}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)")
            );
        }
    }
    // mb_strpos_empty_needle.phpt / mb_strrpos_empty_needle.phpt
    let ascii = "abc def";
    let mb = "日本語テキストです。01234５６７８９。";
    let pos = |t: &mut TestVm, f: &str, h: &str, off: i64| {
        dump_str(t, f, vec![s(h), s(""), Value::Int(off)])
    };
    assert_eq!(pos(&mut t, "mb_strpos", ascii, 0), "int(0)\n");
    assert_eq!(pos(&mut t, "mb_strpos", ascii, 2), "int(2)\n");
    assert_eq!(pos(&mut t, "mb_strpos", ascii, -2), "int(5)\n");
    assert_eq!(pos(&mut t, "mb_strpos", mb, 2), "int(2)\n");
    assert_eq!(pos(&mut t, "mb_strpos", mb, -2), "int(19)\n");
    assert_eq!(pos(&mut t, "mb_strrpos", ascii, 0), "int(7)\n");
    assert_eq!(pos(&mut t, "mb_strrpos", ascii, 2), "int(7)\n");
    assert_eq!(pos(&mut t, "mb_strrpos", ascii, -2), "int(5)\n");
    assert_eq!(pos(&mut t, "mb_strrpos", mb, 0), "int(21)\n");
    assert_eq!(pos(&mut t, "mb_strrpos", mb, -2), "int(19)\n");
    for (f, off) in [("mb_strpos", 15i64), ("mb_strpos", -15), ("mb_strrpos", 150), ("mb_strrpos", -150)] {
        let h = if off.abs() > 100 { mb } else { ascii };
        assert!(err(&mut t, f, vec![s(h), s(""), Value::Int(off)]).ends_with("must be contained in argument #1 ($haystack)"));
    }
    // mb_stripos_invalid_offset.phpt
    let string = "0123この文字列は日本語です。UTF-8を使っています。0123日本語は面倒臭い。";
    assert_eq!(int(&call(&mut t, "mb_strlen", vec![s(string)])), 42);
    for (needle, off) in [("日本語", 44), ("日本語", 50), ("0", 50), ("3", 50), ("日本語", -50), ("0", -44)] {
        assert_eq!(
            err(&mut t, "mb_stripos", vec![s(string), s(needle), Value::Int(off)]),
            "ValueError: mb_stripos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        );
    }
}

#[test]
fn strpos_family() {
    let mut t = utf8_vm();
    let string = "0123この文字列は日本語です。UTF-8を使っています。0123日本語は面倒臭い。";
    assert_eq!(int(&call(&mut t, "mb_strpos", vec![s(string), s("日本語")])), 10);
    assert_eq!(int(&call(&mut t, "mb_strrpos", vec![s(string), s("日本語")])), 33);
    assert_eq!(int(&call(&mut t, "mb_strpos", vec![s(string), s("0"), Value::Int(5)])), 29);
    assert_eq!(dump_str(&mut t, "mb_strpos", vec![s(string), s("xyz")]), "bool(false)\n");
    // Case-insensitive variants use simple case folding.
    assert_eq!(int(&call(&mut t, "mb_stripos", vec![s("ΑΒΓ abc"), s("βγ")])), 1);
    assert_eq!(int(&call(&mut t, "mb_strripos", vec![s("aBcAbC"), s("ABC")])), 3);
    assert_eq!(text(&call(&mut t, "mb_strstr", vec![s("user@example.com"), s("@")])), "@example.com");
    assert_eq!(text(&call(&mut t, "mb_strstr", vec![s("user@example.com"), s("@"), Value::Bool(true)])), "user");
    assert_eq!(text(&call(&mut t, "mb_strrchr", vec![s("a/b/c"), s("/")])), "/c");
    assert_eq!(text(&call(&mut t, "mb_stristr", vec![s("ÄpfelÖl"), s("öL")])), "Öl");
    assert_eq!(text(&call(&mut t, "mb_strrichr", vec![s("xAyaz"), s("A"), Value::Bool(true)])), "xAy");
    assert_eq!(dump_str(&mut t, "mb_strstr", vec![s("abc"), s("d")]), "bool(false)\n");
}

#[test]
fn substr_count() {
    let mut t = utf8_vm();
    assert_eq!(int(&call(&mut t, "mb_substr_count", vec![s("日本語日本語日本"), s("日本")])), 3);
    assert_eq!(int(&call(&mut t, "mb_substr_count", vec![s("aaaa"), s("aa")])), 2);
    assert_eq!(
        err(&mut t, "mb_substr_count", vec![s("abc"), s("")]),
        "ValueError: mb_substr_count(): Argument #2 ($needle) must not be empty"
    );
}

#[test]
fn str_split() {
    let mut t = utf8_vm();
    // mb_str_split_error_conditions.phpt
    for len in [0, -5] {
        assert_eq!(
            err(&mut t, "mb_str_split", vec![s("日本"), Value::Int(len)]),
            "ValueError: mb_str_split(): Argument #2 ($length) must be greater than 0"
        );
    }
    assert_eq!(
        err(&mut t, "mb_str_split", vec![s("日本"), Value::Int(1), s("BAD_ENCODING")]),
        "ValueError: mb_str_split(): Argument #3 ($encoding) must be a valid encoding, \"BAD_ENCODING\" given"
    );
    let parts = call(&mut t, "mb_str_split", vec![b(b"abc\xFFabc"), Value::Int(2), s("UTF-8")]);
    assert_eq!(hexes(&parts), ["6162", "63ff", "6162", "63"]);
    assert_eq!(
        err(&mut t, "mb_str_split", vec![s("abc"), Value::Int(1234567890), s("UTF-8")]),
        "ValueError: mb_str_split(): Argument #2 ($length) is too large"
    );
    // mb_str_split_utf8_utf16.phpt
    let p = call(&mut t, "mb_str_split", vec![b(&unhex("313233f09280a9")), Value::Int(2)]);
    assert_eq!(hexes(&p), ["3132", "33f09280a9"]);
    let p = call(&mut t, "mb_str_split", vec![b(&unhex("313233f092")), Value::Int(2)]);
    assert_eq!(hexes(&p), ["3132", "33f092"]);
    let p = call(&mut t, "mb_str_split", vec![b(&unhex("d800dc00dbffdfff")), Value::Int(1), s("UTF-16BE")]);
    assert_eq!(hexes(&p), ["d800dc00", "dbffdfff"]);
    let p = call(&mut t, "mb_str_split", vec![b(&unhex("00d800dcffdbffdf")), Value::Int(1), s("UTF-16LE")]);
    assert_eq!(hexes(&p), ["00d800dc", "ffdbffdf"]);
    let p = call(&mut t, "mb_str_split", vec![b(&unhex("d800dc00dc00dc00")), Value::Int(1), s("UTF-16BE")]);
    assert_eq!(hexes(&p), ["d800dc00", "003f", "003f"]);
    let p = call(&mut t, "mb_str_split", vec![b(&unhex("00d800dc00dc00dc")), Value::Int(1), s("UTF-16LE")]);
    assert_eq!(hexes(&p), ["00d800dc", "3f00", "3f00"]);
    for enc in ["ASCII", "UTF-8", "UTF-16LE"] {
        assert_eq!(dump_str(&mut t, "mb_str_split", vec![s(""), Value::Int(1), s(enc)]), "array(0) {\n}\n");
    }
    // mb_str_split_other.phpt
    let p = call(&mut t, "mb_str_split", vec![b(b"\x00\x01\x02\x03\x04\x05\x06\x07"), Value::Int(2), s("UCS-2BE")]);
    assert_eq!(hexes(&p), ["00010203", "04050607"]);
    let iso = call(&mut t, "mb_convert_encoding", vec![s("test ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏ"), s("ISO-8859-1"), s("UTF-8")]);
    let p = call(&mut t, "mb_str_split", vec![iso.clone(), Value::Int(4), s("ISO-8859-1")]);
    assert_eq!(hexes(&p), ["74657374", "20c0c1c2", "c3c4c5c6", "c7c8c9ca", "cbcccdce", "cf"]);
    let p = call(&mut t, "mb_str_split", vec![iso, Value::Int(13), s("ISO-8859-1")]);
    assert_eq!(hexes(&p), ["7465737420c0c1c2c3c4c5c6c7", "c8c9cacbcccdcecf"]);
    // Splitting by characters in a multi-byte encoding with a table.
    let p = call(&mut t, "mb_str_split", vec![s("日本語テキスト"), Value::Int(3)]);
    assert_eq!(strings(&p), ["日本語", "テキス", "ト"]);
}

#[test]
fn str_pad() {
    let mut t = utf8_vm();
    // mb_str_pad.phpt
    for ty in [1, 0, 2] {
        assert_eq!(
            err(&mut t, "mb_str_pad", vec![s("▶▶"), Value::Int(6), s(""), Value::Int(ty)]),
            "ValueError: mb_str_pad(): Argument #3 ($pad_string) must not be empty"
        );
    }
    assert_eq!(
        err(&mut t, "mb_str_pad", vec![s("▶▶"), Value::Int(6), s(" "), Value::Int(123456)]),
        "ValueError: mb_str_pad(): Argument #4 ($pad_type) must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH"
    );
    assert_eq!(
        err(&mut t, "mb_str_pad", vec![s("▶▶"), Value::Int(6), s(" "), Value::Int(2), s("non-existing")]),
        "ValueError: mb_str_pad(): Argument #5 ($encoding) must be a valid encoding, \"non-existing\" given"
    );
    let pad = |t: &mut TestVm, st: &str, n: i64, p: &str, ty: i64| {
        text(&call(t, "mb_str_pad", vec![s(st), Value::Int(n), s(p), Value::Int(ty)]))
    };
    assert_eq!(pad(&mut t, "Hello", 7, "+-", 2), "+Hello+");
    assert_eq!(pad(&mut t, "World", 10, "+-", 2), "+-World+-+");
    assert_eq!(pad(&mut t, "Hello", 7, "+-", 0), "+-Hello");
    assert_eq!(pad(&mut t, "World", 10, "+-", 0), "+-+-+World");
    assert_eq!(pad(&mut t, "World", 10, "+-", 1), "World+-+-+");
    for n in [2, 1, 0, -1] {
        assert_eq!(pad(&mut t, "▶▶", n, " ", 2), "▶▶");
    }
    assert_eq!(pad(&mut t, "", 2, " ", 2), "  ");
    assert_eq!(pad(&mut t, "", 1, " ", 2), " ");
    assert_eq!(text(&call(&mut t, "mb_str_pad", vec![s("▶▶"), Value::Int(4)])), "▶▶  ");
    let expected = [
        (6, ["▶▶❤❓❇❤", "❤❓❇❤▶▶", "❤❓▶▶❤❓"]),
        (5, ["▶▶❤❓❇", "❤❓❇▶▶", "❤▶▶❤❓"]),
        (4, ["▶▶❤❓", "❤❓▶▶", "❤▶▶❤"]),
        (3, ["▶▶❤", "❤▶▶", "▶▶❤"]),
        (2, ["▶▶", "▶▶", "▶▶"]),
    ];
    for (n, [r, l, both]) in expected {
        assert_eq!(pad(&mut t, "▶▶", n, "❤❓❇", 1), r);
        assert_eq!(pad(&mut t, "▶▶", n, "❤❓❇", 0), l);
        assert_eq!(pad(&mut t, "▶▶", n, "❤❓❇", 2), both);
    }
    let utf8 = "Σὲ γνωρίζω ἀπὸ τὴν κόψη Зарегистрируйтесь";
    let utf32 = call(&mut t, "mb_convert_encoding", vec![s(utf8), s("UTF-32"), s("UTF-8")]);
    let pad32 = call(&mut t, "mb_convert_encoding", vec![s("▶▶"), s("UTF-32"), s("UTF-8")]);
    let r = call(&mut t, "mb_str_pad", vec![utf32, Value::Int(44), pad32, Value::Int(2), s("UTF-32")]);
    let back = call(&mut t, "mb_convert_encoding", vec![r, s("UTF-8"), s("UTF-32")]);
    assert_eq!(text(&back), format!("▶{utf8}▶▶"));
}

#[test]
fn trim() {
    let mut t = utf8_vm();
    // mb_trim.phpt
    let tr = |t: &mut TestVm, f: &str, st: &[u8], chars: Option<&str>| {
        let mut args = vec![b(st)];
        if let Some(c) = chars {
            args.push(s(c));
        }
        bytes(&call(t, f, args))
    };
    assert_eq!(tr(&mut t, "mb_trim", b" \0\t\nABC \0\t\n", None), b"ABC");
    assert_eq!(tr(&mut t, "mb_ltrim", b" \0\t\nABC \0\t\n", None), b"ABC \0\t\n");
    assert_eq!(tr(&mut t, "mb_rtrim", b" \0\t\nABC \0\t\n", None), b" \0\t\nABC");
    assert_eq!(tr(&mut t, "mb_trim", b" \0\t\nABC \0\t\n", Some("")), b" \0\t\nABC \0\t\n");
    assert_eq!(tr(&mut t, "mb_trim", b"", None), b"");
    assert_eq!(tr(&mut t, "mb_trim", "　あいうえおあお　".as_bytes(), Some("　")), "あいうえおあお".as_bytes());
    assert_eq!(tr(&mut t, "mb_trim", "foo BAR Spaß".as_bytes(), Some("ß")), b"foo BAR Spa");
    assert_eq!(tr(&mut t, "mb_trim", "foo BAR Spaß".as_bytes(), Some("f")), "oo BAR Spaß".as_bytes());
    assert_eq!(tr(&mut t, "mb_trim", "foo BAR Spaß".as_bytes(), Some("ßf")), b"oo BAR Spa");
    assert_eq!(tr(&mut t, "mb_trim", "　あいうおえお 　あ".as_bytes(), Some("　あ")), "いうおえお ".as_bytes());
    assert_eq!(tr(&mut t, "mb_trim", "　あいうおえお 　a".as_bytes(), Some("あa")), "　あいうおえお 　".as_bytes());
    let mut s_e3 = "　あいうおえお 　a".as_bytes().to_vec();
    assert_eq!(
        bytes(&call(&mut t, "mb_trim", vec![b(&s_e3), b(b"\xe3"), s("UTF-8")])),
        s_e3.clone()
    );
    s_e3.clear();
    assert_eq!(tr(&mut t, "mb_trim", "　".repeat(129).as_bytes(), None), b"");
    assert_eq!(tr(&mut t, "mb_trim", format!("{}a", "　".repeat(129)).as_bytes(), None), b"a");
    assert_eq!(tr(&mut t, "mb_rtrim", format!("{}a", "　".repeat(129)).as_bytes(), None).len(), 388);
    let trim_chars: String = (1024u32..2048).filter_map(char::from_u32).collect();
    let padded = format!("{trim_chars}hello{trim_chars}");
    assert_eq!(tr(&mut t, "mb_trim", padded.as_bytes(), Some(&trim_chars)), b"hello");
    assert_eq!(tr(&mut t, "mb_ltrim", padded.as_bytes(), Some(&trim_chars)).len(), 2053);
    assert_eq!(tr(&mut t, "mb_rtrim", padded.as_bytes(), Some(&trim_chars)).len(), 2053);
    let a = "あ".repeat(33);
    assert_eq!(tr(&mut t, "mb_ltrim", format!("{a}いああああ").as_bytes(), Some("あ")), "いああああ".as_bytes());
    assert_eq!(tr(&mut t, "mb_rtrim", format!("{a}いああああ").as_bytes(), Some("あ")), format!("{a}い").as_bytes());
    let defaults = " \x0c\n\r\x0b\0\u{00A0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{0085}\u{180E}";
    assert_eq!(tr(&mut t, "mb_trim", defaults.as_bytes(), None), b"");
    assert_eq!(tr(&mut t, "mb_ltrim", "\u{FFFE}漢字".as_bytes(), Some("\u{FFFE}\u{FEFF}")), "漢字".as_bytes());
    assert_eq!(tr(&mut t, "mb_trim", b"foo", Some("oo")), b"f");
    assert_eq!(
        err(&mut t, "mb_trim", vec![s(" abcd "), s(""), s("NULL")]),
        "ValueError: mb_trim(): Argument #3 ($encoding) must be a valid encoding, \"NULL\" given"
    );
}

#[test]
fn ucfirst_lcfirst() {
    let mut t = utf8_vm();
    // mb_ucfirst_lcfirst.phpt
    assert_eq!(text(&call(&mut t, "mb_ucfirst", vec![s("")])), "");
    for c in 0u8..128 {
        let up = if c.is_ascii_lowercase() { c - 32 } else { c };
        assert_eq!(bytes(&call(&mut t, "mb_ucfirst", vec![b(&[c])])), [up]);
        let low = if c.is_ascii_uppercase() { c + 32 } else { c };
        assert_eq!(bytes(&call(&mut t, "mb_lcfirst", vec![b(&[c])])), [low]);
    }
    for (input, expected) in [("ａｂ", "Ａｂ"), ("ＡＢＳ", "ＡＢＳ"), ("đắt quá!", "Đắt quá!"), ("აბგ", "აბგ"), ("ǉ", "ǈ")] {
        assert_eq!(text(&call(&mut t, "mb_ucfirst", vec![s(input)])), expected);
    }
    for (input, expected) in [("ＡＢＳ", "ａＢＳ"), ("Xin chào", "xin chào"), ("Đẹp quá!", "đẹp quá!")] {
        assert_eq!(text(&call(&mut t, "mb_lcfirst", vec![s(input)])), expected);
    }
    let long = format!("э{}", "A".repeat(65536));
    assert_eq!(text(&call(&mut t, "mb_ucfirst", vec![s(&long)])), format!("Э{}", "A".repeat(65536)));
}

#[test]
fn ord_chr() {
    let mut t = utf8_vm();
    // mb_chr.phpt / mb_ord.phpt (UTF-8 parts)
    assert_eq!(text(&call(&mut t, "mb_chr", vec![Value::Int(0x20bb7)])), "\u{20bb7}");
    assert_eq!(dump_str(&mut t, "mb_chr", vec![Value::Int(0xd800)]), "bool(false)\n");
    assert_eq!(int(&call(&mut t, "mb_ord", vec![s("\u{20bb7}")])), 0x20bb7);
    assert_eq!(dump_str(&mut t, "mb_ord", vec![b(b"\xed\xa0\x80")]), "bool(false)\n");
    assert_eq!(
        err(&mut t, "mb_ord", vec![s("")]),
        "ValueError: mb_ord(): Argument #1 ($string) must not be empty"
    );
    for f in ["mb_chr", "mb_ord"] {
        let first = if f == "mb_chr" { Value::Int(0xd800) } else { b(b"\xed\xa0\x80") };
        for (enc, msg) in [
            ("typo", format!("{f}(): Argument #2 ($encoding) must be a valid encoding, \"typo\" given")),
            ("pass", format!("{f}(): Argument #2 ($encoding) must be a valid encoding, \"pass\" given")),
            ("jis", format!("{f}() does not support the \"JIS\" encoding")),
            ("cp50222", format!("{f}() does not support the \"CP50222\" encoding")),
            ("utf-7", format!("{f}() does not support the \"UTF-7\" encoding")),
        ] {
            assert_eq!(err(&mut t, f, vec![first.clone(), s(enc)]), format!("ValueError: {msg}"));
        }
    }
    call(&mut t, "mb_internal_encoding", vec![s("utf-7")]);
    assert_eq!(
        err(&mut t, "mb_chr", vec![Value::Int(0xd800)]),
        "ValueError: mb_chr() does not support the \"UTF-7\" encoding"
    );
    // mb_chr_ord.phpt
    let mut t = utf8_vm();
    for ch in 1u32..80000 {
        let c = call(&mut t, "mb_chr", vec![Value::Int(ch as i64)]);
        match char::from_u32(ch) {
            Some(_) => assert_eq!(int(&call(&mut t, "mb_ord", vec![c])), ch as i64),
            None => assert!(matches!(c, Value::Bool(false))),
        }
    }
    // Non-UTF-8 encodings go through UCS-4BE.
    assert_eq!(hex(&call(&mut t, "mb_chr", vec![Value::Int(0x41), s("UTF-16BE")])), "0041");
    assert_eq!(dump_str(&mut t, "mb_chr", vec![Value::Int(0x100), s("ISO-8859-1")]), "bool(false)\n");
    assert_eq!(int(&call(&mut t, "mb_ord", vec![b(b"\x00\x41"), s("UTF-16BE")])), 0x41);
}

#[test]
fn scrub() {
    let mut t = utf8_vm();
    // mb_scrub.phpt
    assert_eq!(text(&call(&mut t, "mb_scrub", vec![b(b"\x80")])), "?");
    assert_eq!(text(&call(&mut t, "mb_scrub", vec![b(b"\x80"), s("UTF-8")])), "?");
    let ok = "abc 日本語 Οὐχὶ ταὐτὰ παρίσταταί μοι γιγνώσκειν ⡍⠔⠙⠖ ⡊ ⠙⠕⠝⠰⠞";
    assert_eq!(text(&call(&mut t, "mb_scrub", vec![s(ok)])), ok);
}
