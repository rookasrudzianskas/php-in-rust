//! Settings functions and ini directives: mb_language, mb_internal_encoding,
//! mb_http_input/mb_http_output, mb_detect_order, mb_substitute_character,
//! mb_preferred_mime_name, mb_get_info, mb_list_encodings,
//! mb_encoding_aliases, mb_parse_str.

use super::*;

fn vm_with(ini: &[(&str, &str)]) -> TestVm {
    let mut t = TestVm::new();
    for (k, v) in ini {
        assert!(t.vm.ini_set_system(*k, *v), "unknown ini {k}");
    }
    t
}

#[test]
fn language() {
    // mb_language.phpt
    let mut t = vm_with(&[("mbstring.language", "")]);
    assert_eq!(text(&call(&mut t, "mb_language", vec![])), "neutral");
    assert!(t.vm.ini_set("mbstring.language", "uni").is_some());
    assert_eq!(text(&call(&mut t, "mb_language", vec![])), "uni");
    assert_eq!(dump_str(&mut t, "mb_language", vec![s("English")]), "bool(true)\n");
    assert_eq!(text(&call(&mut t, "mb_language", vec![])), "English");
    assert_eq!(
        err(&mut t, "mb_language", vec![s("Pig Latin")]),
        "ValueError: mb_language(): Argument #1 ($language) must be a valid language, \"Pig Latin\" given"
    );
    assert_eq!(text(&call(&mut t, "mb_language", vec![])), "neutral");
    // Aliases and short names.
    call(&mut t, "mb_language", vec![s("ja")]);
    assert_eq!(text(&call(&mut t, "mb_language", vec![])), "Japanese");
    call(&mut t, "mb_language", vec![s("Deutsch")]);
    assert_eq!(text(&call(&mut t, "mb_language", vec![])), "German");
}

#[test]
fn language_detect_order_defaults() {
    // zh_CN_default_encodings.phpt
    let mut t = vm_with(&[("mbstring.language", "Simplified Chinese")]);
    assert_eq!(
        strings(&call(&mut t, "mb_detect_order", vec![])),
        ["ASCII", "UTF-8", "EUC-CN", "CP936", "GB18030-2022"]
    );
    let mut t = vm_with(&[("mbstring.language", "Japanese")]);
    // mb_detect_order.phpt
    assert_eq!(dump_str(&mut t, "mb_detect_order", vec![s("auto")]), "bool(true)\n");
    assert_eq!(
        strings(&call(&mut t, "mb_detect_order", vec![])),
        ["ASCII", "JIS", "UTF-8", "EUC-JP", "SJIS"]
    );
    call(&mut t, "mb_detect_order", vec![s("SJIS,EUC-JP,JIS,UTF-8")]);
    assert_eq!(strings(&call(&mut t, "mb_detect_order", vec![])), ["SJIS", "EUC-JP", "JIS", "UTF-8"]);
    call(&mut t, "mb_detect_order", vec![list(&["ASCII", "JIS", "EUC-JP", "UTF-8"])]);
    assert_eq!(strings(&call(&mut t, "mb_detect_order", vec![])), ["ASCII", "JIS", "EUC-JP", "UTF-8"]);
    assert_eq!(
        err(&mut t, "mb_detect_order", vec![s("BAD_NAME")]),
        "ValueError: mb_detect_order(): Argument #1 ($encoding) contains invalid encoding \"BAD_NAME\""
    );
    assert_eq!(
        err(&mut t, "mb_detect_order", vec![list(&["ASCII", "BAD_NAME"])]),
        "ValueError: mb_detect_order(): Argument #1 ($encoding) contains invalid encoding \"BAD_NAME\""
    );
    assert_eq!(strings(&call(&mut t, "mb_detect_order", vec![])), ["ASCII", "JIS", "EUC-JP", "UTF-8"]);
    // mb_detect_order_empty_encoding_list.phpt
    for v in [s(""), Value::empty_array()] {
        assert_eq!(
            err(&mut t, "mb_detect_order", vec![v]),
            "ValueError: mb_detect_order(): Argument #1 ($encoding) must specify at least one encoding"
        );
    }
}

#[test]
fn detect_order_ini() {
    // mb_detect_order_with_ini_set.phpt
    let mut t = vm_with(&[("mbstring.detect_order", "UTF-8,ISO-8859-15,ISO-8859-1,ASCII")]);
    assert_eq!(
        strings(&call(&mut t, "mb_detect_order", vec![])),
        ["UTF-8", "ISO-8859-15", "ISO-8859-1", "ASCII"]
    );
    assert!(t.vm.ini_set("mbstring.detect_order", "UTF-8, ISO-8859-1, ASCII").is_some());
    assert_eq!(strings(&call(&mut t, "mb_detect_order", vec![])), ["UTF-8", "ISO-8859-1", "ASCII"]);
    t.vm.ini_set("mbstring.detect_order", "UTF-8");
    assert_eq!(strings(&call(&mut t, "mb_detect_order", vec![])), ["UTF-8"]);
    // An empty list is rejected.
    assert!(t.vm.ini_set("mbstring.detect_order", "").is_none());
    assert_eq!(strings(&call(&mut t, "mb_detect_order", vec![])), ["UTF-8"]);
}

#[test]
fn internal_encoding() {
    // mb_internal_encoding.phpt
    let mut t = TestVm::new();
    for e in ["EUC-JP", "UTF-8", "ASCII"] {
        assert_eq!(dump_str(&mut t, "mb_internal_encoding", vec![s(e)]), "bool(true)\n");
        assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), e);
    }
    assert_eq!(
        err(&mut t, "mb_internal_encoding", vec![s("BAD_NAME")]),
        "ValueError: mb_internal_encoding(): Argument #1 ($encoding) must be a valid encoding, \"BAD_NAME\" given"
    );
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "ASCII");

    // mb_internal_encoding_basic2.phpt
    let mut t = vm_with(&[
        ("default_charset", ""),
        ("input_encoding", "ISO-8859-1"),
        ("output_encoding", "ISO-8859-1"),
        ("internal_encoding", "ISO-8859-1"),
    ]);
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "ISO-8859-1");
    assert_eq!(text(&call(&mut t, "mb_http_output", vec![])), "ISO-8859-1");
    assert_eq!(strings(&call(&mut t, "mb_http_input", vec![s("I")])), ["ISO-8859-1"]);
    call(&mut t, "mb_internal_encoding", vec![s("UTF-8")]);
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "UTF-8");

    // mb_internal_encoding_ini_basic2.phpt / mb_internal_encoding_ini_invalid_encoding.phpt
    let mut t = vm_with(&[("mbstring.internal_encoding", "ISO-8859-7")]);
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "ISO-8859-7");
    call(&mut t, "mb_internal_encoding", vec![s("UTF-8")]);
    assert_eq!(t.vm.ini_get("mbstring.internal_encoding").unwrap(), "ISO-8859-7");
    let mut t = vm_with(&[("mbstring.internal_encoding", "BAD")]);
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "UTF-8");
    assert_eq!(t.vm.ini_get("mbstring.internal_encoding").unwrap(), "BAD");

    // ini_language.phpt: an ini value is reported by its canonical name.
    let mut t = vm_with(&[("internal_encoding", "Shift_JIS"), ("mbstring.language", "Japanese")]);
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "SJIS");
}

#[test]
fn internal_encoding_follows_ini() {
    // internal_encoding.phpt
    let mut t = vm_with(&[("internal_encoding", "iso-8859-1")]);
    let len = |t: &mut TestVm| int(&call(t, "mb_strlen", vec![b(b"\xc3\xb6")]));
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "ISO-8859-1");
    assert_eq!(len(&mut t), 2);
    t.vm.ini_set("mbstring.internal_encoding", "utf-8");
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "UTF-8");
    assert_eq!(len(&mut t), 1);
    // mbstring.internal_encoding takes precedence.
    t.vm.ini_set("internal_encoding", "iso-8859-2");
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "UTF-8");
    t.vm.ini_set("mbstring.internal_encoding", "");
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "ISO-8859-2");
    assert_eq!(len(&mut t), 2);
    // mb_internal_encoding() takes precedence over the core setting.
    call(&mut t, "mb_internal_encoding", vec![s("utf-8")]);
    assert_eq!(len(&mut t), 1);
    t.vm.ini_set("internal_encoding", "iso-8859-3");
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "UTF-8");
    t.vm.ini_set("mbstring.internal_encoding", "");
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "ISO-8859-3");
    assert_eq!(len(&mut t), 2);

    // ini_encoding.phpt
    let mut t = vm_with(&[
        ("default_charset", "Shift_JIS"),
        ("internal_encoding", ""),
        ("input_encoding", ""),
        ("output_encoding", ""),
        ("mbstring.internal_encoding", "Shift_JIS"),
        ("mbstring.http_input", "Shift_JIS"),
        ("mbstring.http_output", "Shift_JIS"),
    ]);
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "SJIS");
    for k in [
        "default_charset",
        "internal_encoding",
        "input_encoding",
        "output_encoding",
        "mbstring.internal_encoding",
        "mbstring.http_input",
        "mbstring.http_output",
    ] {
        assert!(t.vm.ini_set(k, "UTF-8").is_some(), "{k}");
    }
    assert_eq!(text(&call(&mut t, "mb_internal_encoding", vec![])), "UTF-8");
    assert_eq!(text(&call(&mut t, "mb_http_output", vec![])), "UTF-8");
}

#[test]
fn http_input_output() {
    let mut t = TestVm::new();
    // mb_http_output.phpt
    for e in ["ASCII", "SJIS", "JIS", "UTF-8", "EUC-JP"] {
        assert_eq!(dump_str(&mut t, "mb_http_output", vec![s(e)]), "bool(true)\n");
        assert_eq!(text(&call(&mut t, "mb_http_output", vec![])), e);
    }
    assert_eq!(
        err(&mut t, "mb_http_output", vec![s("BAD_NAME")]),
        "ValueError: mb_http_output(): Argument #1 ($encoding) must be a valid encoding, \"BAD_NAME\" given"
    );
    assert_eq!(text(&call(&mut t, "mb_http_output", vec![])), "EUC-JP");
    call(&mut t, "mb_http_output", vec![s("pass")]);
    assert_eq!(text(&call(&mut t, "mb_http_output", vec![])), "pass");
    assert_eq!(
        err(&mut t, "mb_http_output", vec![b(b"UTF-8\0")]),
        "ValueError: mb_http_output(): Argument #1 ($encoding) must not contain any null bytes"
    );
    // mb_http_input_errors.phpt
    for ty in ["PN", "Q"] {
        assert_eq!(
            err(&mut t, "mb_http_input", vec![s(ty)]),
            "ValueError: mb_http_input(): Argument #1 ($type) must be one of \"G\", \"P\", \"C\", \"S\", \"I\", or \"L\""
        );
    }
    // Nothing was parsed in the CLI.
    for ty in ["G", "P", "C", "S"] {
        assert_eq!(dump_str(&mut t, "mb_http_input", vec![s(ty)]), "bool(false)\n");
    }
    assert_eq!(dump_str(&mut t, "mb_http_input", vec![]), "bool(false)\n");
    assert_eq!(text(&call(&mut t, "mb_http_input", vec![s("L")])), "UTF-8");
    // mb_http_input_pass.phpt: "pass" as the input encoding
    let mut t = vm_with(&[("input_encoding", "pass")]);
    assert_eq!(strings(&call(&mut t, "mb_http_input", vec![s("I")])), ["pass"]);
    assert_eq!(text(&call(&mut t, "mb_http_input", vec![s("L")])), "pass");
    // mb_http_input.phpt: aliases resolve to canonical names
    let mut t = vm_with(&[("input_encoding", "latin1")]);
    assert_eq!(strings(&call(&mut t, "mb_http_input", vec![s("I")])), ["ISO-8859-1"]);
}

#[test]
fn substitute_character() {
    let mut t = TestVm::new();
    // mb_substitute_character.phpt (without the CP932 conversions)
    assert_eq!(dump_str(&mut t, "mb_substitute_character", vec![Value::Int(0x3044)]), "bool(true)\n");
    assert_eq!(dump_str(&mut t, "mb_substitute_character", vec![]), "int(12356)\n");
    for mode in ["long", "none", "entity"] {
        assert_eq!(dump_str(&mut t, "mb_substitute_character", vec![s(mode)]), "bool(true)\n");
        assert_eq!(text(&call(&mut t, "mb_substitute_character", vec![])), mode);
    }
    assert_eq!(
        err(&mut t, "mb_substitute_character", vec![s("BAD_NAME")]),
        "ValueError: mb_substitute_character(): Argument #1 ($substitute_character) must be \"none\", \"long\", \"entity\" or a valid codepoint"
    );
    for cp in [-1, 0xD800, 0x110000] {
        assert_eq!(
            err(&mut t, "mb_substitute_character", vec![Value::Int(cp)]),
            "ValueError: mb_substitute_character(): Argument #1 ($substitute_character) is not a valid codepoint"
        );
    }
    // mb_substitute_character_variation2.phpt
    let mut t = TestVm::new();
    let mb = "日本語テキスト";
    let conv = |t: &mut TestVm| hex(&call(t, "mb_convert_encoding", vec![s(mb), s("ISO-8859-1"), s("UTF-8")]));
    assert_eq!(conv(&mut t), "3f3f3f3f3f3f3f");
    call(&mut t, "mb_substitute_character", vec![Value::Int(66)]);
    assert_eq!(conv(&mut t), "42424242424242");
    call(&mut t, "mb_substitute_character", vec![s("none")]);
    assert_eq!(conv(&mut t), "");
    call(&mut t, "mb_substitute_character", vec![Value::Int(280)]);
    assert_eq!(conv(&mut t), "3f3f3f3f3f3f3f");
    // "long" and "entity" markers
    call(&mut t, "mb_substitute_character", vec![s("long")]);
    assert_eq!(text(&call(&mut t, "mb_convert_encoding", vec![s("a♠"), s("ASCII"), s("UTF-8")])), "aU+2660");
    call(&mut t, "mb_substitute_character", vec![s("entity")]);
    assert_eq!(text(&call(&mut t, "mb_convert_encoding", vec![s("a♠"), s("ASCII"), s("UTF-8")])), "a&#x2660;");
    // Bad input becomes the substitute character (280, not encodable).
    assert_eq!(text(&call(&mut t, "mb_convert_encoding", vec![b(b"a\xff"), s("ASCII"), s("UTF-8")])), "a");
    // The ini setting
    let mut t = vm_with(&[("mbstring.substitute_character", "123")]);
    assert_eq!(dump_str(&mut t, "mb_substitute_character", vec![]), "int(123)\n");
    let mut t = vm_with(&[("mbstring.substitute_character", "none")]);
    assert_eq!(text(&call(&mut t, "mb_substitute_character", vec![])), "none");
    let mut t = vm_with(&[("mbstring.substitute_character", "U+3000")]);
    assert_eq!(dump_str(&mut t, "mb_substitute_character", vec![]), "int(63)\n");
}

#[test]
fn mime_names_aliases_list() {
    let mut t = TestVm::new();
    // mb_preferred_mime_name.phpt
    for (name, mime) in [
        ("sjis-win", "Shift_JIS"),
        ("SJIS", "Shift_JIS"),
        ("EUC-JP", "EUC-JP"),
        ("UTF-8", "UTF-8"),
        ("ISO-2022-JP", "ISO-2022-JP"),
        ("JIS", "ISO-2022-JP"),
        ("ISO-8859-1", "ISO-8859-1"),
        ("UCS2", "UCS-2"),
        ("UCS4", "UCS-4"),
    ] {
        assert_eq!(text(&call(&mut t, "mb_preferred_mime_name", vec![s(name)])), mime);
    }
    assert_eq!(
        err(&mut t, "mb_preferred_mime_name", vec![s("BAD_NAME")]),
        "ValueError: mb_preferred_mime_name(): Argument #1 ($encoding) must be a valid encoding, \"BAD_NAME\" given"
    );
    assert_eq!(vd(&mut t, "mb_preferred_mime_name", vec![s("UTF7-IMAP")]),
        "\nWarning: mb_preferred_mime_name(): No MIME preferred name corresponding to \"UTF7-IMAP\" in /t.php on line 3\nbool(false)\n");
    // mb_encoding_aliases.phpt
    let mut aliases = strings(&call(&mut t, "mb_encoding_aliases", vec![s("ASCII")]));
    aliases.sort();
    assert_eq!(
        aliases,
        [
            "ANSI_X3.4-1968", "ANSI_X3.4-1986", "IBM-367", "IBM367", "ISO646-US",
            "ISO_646.irv:1991", "US-ASCII", "cp367", "csASCII", "iso-ir-6", "us"
        ]
    );
    assert!(strings(&call(&mut t, "mb_encoding_aliases", vec![s("7bit")])).is_empty());
    assert_eq!(strings(&call(&mut t, "mb_encoding_aliases", vec![s("8bit")])), ["binary"]);
    assert_eq!(
        err(&mut t, "mb_encoding_aliases", vec![s("BAD")]),
        "ValueError: mb_encoding_aliases(): Argument #1 ($encoding) must be a valid encoding, \"BAD\" given"
    );
    // mb_list_encodings.phpt; php-src's order
    let all = strings(&call(&mut t, "mb_list_encodings", vec![]));
    assert_eq!(all.len(), 79);
    assert_eq!(&all[..6], ["BASE64", "UUENCODE", "HTML-ENTITIES", "Quoted-Printable", "7bit", "8bit"]);
    assert_eq!(all.last().unwrap(), "CP50222");
    assert!(all.contains(&"ASCII".to_string()));
    assert!(!all.contains(&"pass".to_string()));
}

#[test]
fn get_info() {
    // mb_get_info.phpt
    let mut t = vm_with(&[
        ("mbstring.encoding_translation", "1"),
        ("mbstring.language", "Korean"),
        ("mbstring.internal_encoding", "UTF-8"),
        ("mbstring.http_input", "ISO-8859-1"),
        ("mbstring.http_output", "ISO-8859-15"),
        ("mbstring.http_output_conv_mimetypes", "abc"),
        ("mbstring.detect_order", "UTF-8,ISO-8859-15,ISO-8859-1,ASCII"),
        ("mbstring.substitute_character", "123"),
        ("mbstring.strict_detection", "1"),
    ]);
    let r = Value::Ref(php_runtime::PhpRef::new(Value::Null));
    call(&mut t, "mb_parse_str", vec![s("abc=def"), r]);
    call(&mut t, "mb_convert_encoding", vec![b(b"\xff\xff"), s("Shift_JIS"), s("UCS-2BE")]);
    let info = call(&mut t, "mb_get_info", vec![]);
    assert_eq!(
        dump(&mut t, info.clone()),
        r#"array(13) {
  ["internal_encoding"]=>
  string(5) "UTF-8"
  ["http_input"]=>
  string(10) "ISO-8859-1"
  ["http_output"]=>
  string(11) "ISO-8859-15"
  ["http_output_conv_mimetypes"]=>
  string(3) "abc"
  ["mail_charset"]=>
  string(11) "ISO-2022-KR"
  ["mail_header_encoding"]=>
  string(6) "BASE64"
  ["mail_body_encoding"]=>
  string(4) "7bit"
  ["illegal_chars"]=>
  int(1)
  ["encoding_translation"]=>
  string(2) "On"
  ["language"]=>
  string(6) "Korean"
  ["detect_order"]=>
  array(4) {
    [0]=>
    string(5) "UTF-8"
    [1]=>
    string(11) "ISO-8859-15"
    [2]=>
    string(10) "ISO-8859-1"
    [3]=>
    string(5) "ASCII"
  }
  ["substitute_character"]=>
  int(123)
  ["strict_detection"]=>
  string(2) "On"
}
"#
    );
    let Value::Array(a) = &info else { unreachable!() };
    for (k, v) in a.iter() {
        let one = call(&mut t, "mb_get_info", vec![k.to_value()]);
        assert_eq!(dump(&mut t, one), dump(&mut t, v.clone()));
    }
    // mb_get_info_errors.phpt
    let mut t = TestVm::new();
    assert_eq!(dump_str(&mut t, "mb_get_info", vec![s("http_input")]), "NULL\n");
    assert_eq!(strings(&call(&mut t, "mb_get_info", vec![s("detect_order")])), ["ASCII", "UTF-8"]);
    assert_eq!(
        vd(&mut t, "mb_get_info", vec![s("non_sense")]),
        "\nWarning: mb_get_info(): argument #1 ($type) must be a valid type in /t.php on line 3\nbool(false)\n"
    );
}

#[test]
fn parse_str() {
    // mb_parse_str.phpt
    let mut t = TestVm::new();
    let parse = |t: &mut TestVm, q: &[u8]| {
        let r = php_runtime::PhpRef::new(Value::Null);
        let ok = call(t, "mb_parse_str", vec![b(q), Value::Ref(r.clone())]);
        (ok, r.get())
    };
    let (ok, a) = parse(&mut t, b"foo=abc&bar=def");
    assert!(matches!(ok, Value::Bool(true)));
    assert_eq!(dump(&mut t, a), "array(2) {\n  [\"foo\"]=>\n  string(3) \"abc\"\n  [\"bar\"]=>\n  string(3) \"def\"\n}\n");
    let (_, a) = parse(&mut t, b"%2bfoo=def&-bar=jkl");
    assert_eq!(dump(&mut t, a), "array(2) {\n  [\"+foo\"]=>\n  string(3) \"def\"\n  [\"-bar\"]=>\n  string(3) \"jkl\"\n}\n");
    let (_, a) = parse(&mut t, b"foo[]=abc&foo[]=def&foo[]=ghi&bar[]=jkl");
    assert_eq!(
        dump(&mut t, a),
        "array(2) {\n  [\"foo\"]=>\n  array(3) {\n    [0]=>\n    string(3) \"abc\"\n    [1]=>\n    string(3) \"def\"\n    [2]=>\n    string(3) \"ghi\"\n  }\n  [\"bar\"]=>\n  array(1) {\n    [0]=>\n    string(3) \"jkl\"\n  }\n}\n"
    );
    // Empty input: nothing detected.
    let (ok, a) = parse(&mut t, b"");
    assert!(matches!(ok, Value::Bool(false)));
    assert_eq!(dump(&mut t, a), "array(0) {\n}\n");

    // mb_parse_str02.phpt
    let mut t = vm_with(&[("arg_separator.input", "&#")]);
    let (_, a) = parse(&mut t, b"foo=abc#bar=def&fubar=ghi");
    assert_eq!(dump(&mut t, a).matches("string(3)").count(), 3);
    let (_, a) = parse(&mut t, b"  foo[]=abc&foo[]=def#foo[]=ghi#bar[]=#foo[]&fubar[]==");
    assert_eq!(
        dump(&mut t, a),
        "array(3) {\n  [\"foo\"]=>\n  array(4) {\n    [0]=>\n    string(3) \"abc\"\n    [1]=>\n    string(3) \"def\"\n    [2]=>\n    string(3) \"ghi\"\n    [3]=>\n    string(0) \"\"\n  }\n  [\"bar\"]=>\n  array(1) {\n    [0]=>\n    string(0) \"\"\n  }\n  [\"fubar\"]=>\n  array(1) {\n    [0]=>\n    string(1) \"=\"\n  }\n}\n"
    );

    // mb_parse_str_error.phpt
    let mut t = vm_with(&[("internal_encoding", "UTF-8")]);
    let (_, a) = parse(&mut t, b"\x80\x80\x80");
    assert_eq!(dump(&mut t, a), "array(1) {\n  [\"???\"]=>\n  string(0) \"\"\n}\n");
    let (_, a) = parse(&mut t, b"\xFF=\xFF");
    assert_eq!(dump(&mut t, a), "array(1) {\n  [\"?\"]=>\n  string(1) \"?\"\n}\n");
    assert_eq!(text(&call(&mut t, "mb_http_input", vec![])), "UTF-8");
}
