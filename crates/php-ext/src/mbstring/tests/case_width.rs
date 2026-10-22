//! Case conversion (casemapping, casefold, mb_convert_case_various_mode,
//! mb_strtoupper/lower) and display width (mb_strwidth, mb_strimwidth).

use super::*;

fn utf8_vm() -> TestVm {
    let mut t = TestVm::new();
    t.vm.ini_set_system("internal_encoding", "UTF-8");
    t
}

fn case(t: &mut TestVm, st: &[u8], mode: i64, enc: Option<&str>) -> Vec<u8> {
    let mut args = vec![b(st), Value::Int(mode)];
    if let Some(e) = enc {
        args.push(s(e));
    }
    bytes(&call(t, "mb_convert_case", args))
}

#[test]
fn casemapping() {
    let mut t = utf8_vm();
    // Lower, LowerSimple, Upper, UpperSimple, Title, TitleSimple, Fold, FoldSimple
    let modes = [1, 5, 0, 4, 2, 6, 3, 7];
    let table = [
        ("ß", ["ß", "ß", "SS", "ß", "Ss", "ß", "ss", "ß"]),
        ("ﬀ", ["ﬀ", "ﬀ", "FF", "ﬀ", "Ff", "ﬀ", "ff", "ﬀ"]),
        ("İ", ["i̇", "i", "İ", "İ", "İ", "İ", "i̇", "İ"]),
    ];
    for (input, expected) in table {
        for (mode, e) in modes.iter().zip(expected) {
            assert_eq!(
                case(&mut t, input.as_bytes(), *mode, None),
                e.as_bytes(),
                "{input} mode {mode}"
            );
        }
    }
    // Turkish dotted/dotless i in ISO-8859-9
    call(&mut t, "mb_internal_encoding", vec![s("ISO-8859-9")]);
    let h = |t: &mut TestVm, st: &[u8], mode: i64| case(t, st, mode, None);
    for m in [1, 5, 3, 7] {
        assert_eq!(h(&mut t, b"\xdd", m), b"\x69");
    }
    assert_eq!(h(&mut t, b"\xfd", 0), b"\x49");
    assert_eq!(h(&mut t, b"\xfd", 4), b"\x49");
    assert_eq!(h(&mut t, b"\xfd", 3), b"\xfd");
    assert_eq!(h(&mut t, b"\xfd", 7), b"\xfd");
    for m in [1, 5, 3, 7] {
        assert_eq!(h(&mut t, b"\x49", m), b"\xfd");
    }
    assert_eq!(h(&mut t, b"\x69", 0), b"\xdd");
    assert_eq!(h(&mut t, b"\x69", 4), b"\xdd");
    assert_eq!(h(&mut t, b"\x69", 3), b"\x69");
    assert_eq!(h(&mut t, b"\x69", 7), b"\x69");
    let greek = "ΚΑΛΗΣΠΕΡΑ ΣΑΣ".as_bytes();
    assert_eq!(
        case(&mut t, greek, 2, Some("UTF-8")),
        "Καλησπερα Σας".as_bytes()
    );
    assert_eq!(
        case(&mut t, greek, 6, Some("UTF-8")),
        "Καλησπερα Σασ".as_bytes()
    );
    assert_eq!(
        case(&mut t, greek, 1, Some("UTF-8")),
        "καλησπερα σας".as_bytes()
    );
    assert_eq!(
        case(&mut t, greek, 5, Some("UTF-8")),
        "καλησπερα σασ".as_bytes()
    );
}

#[test]
fn casefold_latin1() {
    // casefold.phpt
    let mut t = TestVm::new();
    call(&mut t, "mb_internal_encoding", vec![s("ISO-8859-1")]);
    assert_eq!(
        bytes(&call(
            &mut t,
            "mb_strtolower",
            vec![s("ABCDEFGHIJKLMNOPQRSTUVWXYZ\n")]
        )),
        b"abcdefghijklmnopqrstuvwxyz\n"
    );
    assert_eq!(
        bytes(&call(&mut t, "mb_strtoupper", vec![b(b"\xe4\xf6\xfc\n")])),
        b"\xc4\xd6\xdc\n"
    );
    assert_eq!(case(&mut t, b"\xe4\xf6\xfc\n", 2, None), b"\xc4\xf6\xfc\n");
}

#[test]
fn various_modes_and_sigma() {
    let mut t = utf8_vm();
    let st = "foo BAR Spaß".as_bytes();
    let expected = [
        "FOO BAR SPASS",
        "foo bar spaß",
        "Foo Bar Spaß",
        "foo bar spass",
        "FOO BAR SPAß",
        "foo bar spaß",
        "Foo Bar Spaß",
        "foo bar spaß",
    ];
    for (mode, e) in expected.iter().enumerate() {
        assert_eq!(case(&mut t, st, mode as i64, None), e.as_bytes());
    }
    assert_eq!(
        err(&mut t, "mb_convert_case", vec![b(st), Value::Int(100)]),
        "ValueError: mb_convert_case(): Argument #2 ($mode) must be one of the MB_CASE_* constants"
    );
    let dots63 = ".".repeat(63);
    let dots64 = ".".repeat(64);
    let dots256 = ".".repeat(256);
    let sigma = [
        ("Σ".to_string(), "Σ".to_string()),
        ("aΣ".into(), "Aς".into()),
        ("aΣb".into(), "Aσb".into()),
        ("aΣ b".into(), "Aς B".into()),
        (" ΣΣΣΣ ".into(), " Σσσς ".into()),
        ("'Σ".into(), "'Σ".into()),
        ("ab'Σ".into(), "Ab'ς".into()),
        ("Σ'".into(), "Σ'".into()),
        ("Σ'a".into(), "Σ'a".into()),
        ("a'Σ'a".into(), "A'σ'a".into()),
        (format!("a{dots63}Σ"), format!("A{dots63}ς")),
        // Context-sensitive casing doesn't reach back this far.
        (format!("a{dots64}Σ"), format!("A{dots64}σ")),
        (format!("abcΣ{dots64} abc"), format!("Abcς{dots64} Abc")),
        (format!("abcΣ{dots64}a abc"), format!("Abcσ{dots64}a Abc")),
        (format!("abcΣ{dots256} abc"), format!("Abcς{dots256} Abc")),
    ];
    for (input, e) in sigma {
        assert_eq!(
            text(&Value::bytes(&case(
                &mut t,
                input.as_bytes(),
                2,
                Some("UTF-8")
            ))),
            e,
            "{input}"
        );
    }
    assert_eq!(
        hex(&Value::bytes(&case(
            &mut t,
            b"\x01I\x01,",
            2,
            Some("UCS-2BE")
        ))),
        "02bc004e012d"
    );
    assert_eq!(
        hex(&Value::bytes(&case(
            &mut t,
            b"\x01I\x01,",
            6,
            Some("UCS-2BE")
        ))),
        "0149012d"
    );
}

#[test]
fn upper_lower_invalid_input() {
    let mut t = utf8_vm();
    // Invalid bytes become the substitute character.
    assert_eq!(
        bytes(&call(&mut t, "mb_strtoupper", vec![b(b"a\xffb")])),
        b"A?B"
    );
    call(&mut t, "mb_substitute_character", vec![s("none")]);
    assert_eq!(
        bytes(&call(&mut t, "mb_strtolower", vec![b(b"A\xffB")])),
        b"ab"
    );
    // Full mappings expand.
    assert_eq!(
        text(&call(&mut t, "mb_strtoupper", vec![s("ŉ ǰ ﬁ")])),
        "ʼN J̌ FI"
    );
}

#[test]
fn strwidth() {
    let mut t = utf8_vm();
    // mb_strwidth.phpt (UTF-8/UTF-16/KOI8-R parts)
    assert_eq!(
        int(&call(
            &mut t,
            "mb_strwidth",
            vec![s("∮ E⋅da = Q"), s("UTF-8")]
        )),
        10
    );
    let utf16le = unhex(
        "1a043804400438043b043b04380446043004200069007300200043007900720069006c006c0069006300",
    );
    assert_eq!(
        int(&call(
            &mut t,
            "mb_strwidth",
            vec![b(&utf16le), s("UTF-16LE")]
        )),
        21
    );
    let utf16be = unhex(
        "041a043804400438043b043b04380446043000200069007300200043007900720069006c006c00690063",
    );
    assert_eq!(
        int(&call(
            &mut t,
            "mb_strwidth",
            vec![b(&utf16be), s("UTF-16BE")]
        )),
        21
    );
    let koi8r = unhex("ebc9d2c9ccccc9c3c120697320437972696c6c6963");
    assert_eq!(
        int(&call(&mut t, "mb_strwidth", vec![b(&koi8r), s("KOI8-R")])),
        21
    );
    assert_eq!(int(&call(&mut t, "mb_strwidth", vec![s("日本語abc")])), 9);
}

/// `testStrimwidthWithMarker`: the characters of the result in hex.
fn strim(t: &mut TestVm, st: &[u8], marker: &[u8], start: i64, width: i64, enc: &str) -> String {
    let r = call(
        t,
        "mb_strimwidth",
        vec![
            b(st),
            Value::Int(start),
            Value::Int(width),
            b(marker),
            s(enc),
        ],
    );
    let chars = call(t, "mb_str_split", vec![r.clone(), Value::Int(1), s(enc)]);
    let len = int(&call(t, "mb_strlen", vec![r.clone(), s(enc)]));
    let w = int(&call(t, "mb_strwidth", vec![r, s(enc)]));
    format!("[{}] length={len} width={w}", hexes(&chars).join(" "))
}

#[test]
fn strimwidth() {
    let mut t = utf8_vm();
    // mb_strimwidth.phpt (UTF-16LE and "Other" parts)
    let u = unhex("310032003300610062006300226f575b4b306a30");
    let dots = b".\x00.\x00.\x00";
    let all = "3100 3200 3300 6100 6200 6300 226f 575b 4b30 6a30";
    assert_eq!(
        strim(&mut t, &u, dots, 0, 100, "UTF-16LE"),
        format!("[{all}] length=10 width=14")
    );
    assert_eq!(
        strim(&mut t, &u, dots, 0, 14, "UTF-16LE"),
        format!("[{all}] length=10 width=14")
    );
    assert_eq!(
        strim(&mut t, &u, dots, 0, 13, "UTF-16LE"),
        "[3100 3200 3300 6100 6200 6300 226f 575b 2e00 2e00 2e00] length=11 width=13"
    );
    assert_eq!(
        strim(&mut t, &u, dots, 0, 12, "UTF-16LE"),
        "[3100 3200 3300 6100 6200 6300 226f 2e00 2e00 2e00] length=10 width=11"
    );
    assert_eq!(
        strim(&mut t, &u, dots, 0, 0, "UTF-16LE"),
        "[2e00 2e00 2e00] length=3 width=3"
    );
    assert_eq!(
        strim(&mut t, &u, dots, 0, 1, "UTF-16LE"),
        "[2e00 2e00 2e00] length=3 width=3"
    );
    assert_eq!(
        strim(&mut t, &u, dots, -3, 5, "UTF-16LE"),
        "[575b 2e00 2e00 2e00] length=4 width=5"
    );
    assert_eq!(
        strim(&mut t, &u, dots, -4, -2, "UTF-16LE"),
        "[226f 2e00 2e00 2e00] length=4 width=5"
    );
    assert!(t.output().contains(
        "Deprecated: mb_strimwidth(): passing a negative integer to argument #3 ($width) is deprecated"
    ));

    // "Other": every start/width combination on an ASCII string.
    let st = b"abcdefghijklmnop";
    let sub = |t: &mut TestVm, v: Value, from: i64, len: i64| {
        bytes(&call(
            t,
            "mb_substr",
            vec![v, Value::Int(from), Value::Int(len), s("ASCII")],
        ))
    };
    for from in -5i64..=5 {
        for width in -5i64..=5 {
            let r = t.call(
                "mb_strimwidth",
                vec![
                    b(st),
                    Value::Int(from),
                    Value::Int(width),
                    s("..."),
                    s("ASCII"),
                ],
            );
            if from < 0 && width < 0 && width < from {
                assert!(r.is_err(), "from={from} width={width}");
                continue;
            }
            let r = r.unwrap();
            if from < 0 && width < 0 && (width - from) <= 3 {
                assert_eq!(bytes(&r), b"...");
                continue;
            }
            if !(0..=3).contains(&width) {
                assert_eq!(sub(&mut t, r.clone(), 0, 1), sub(&mut t, b(st), from, 1));
            }
            if width >= 3 && (from >= 0 || width <= from.abs()) {
                assert_eq!(
                    int(&call(&mut t, "mb_strwidth", vec![r.clone(), s("ASCII")])),
                    width
                );
            }
            if width < 0 {
                assert_eq!(sub(&mut t, r.clone(), -3, 3), b"...");
                assert_eq!(sub(&mut t, r, -4, 1), sub(&mut t, b(st), width - 4, 1));
            }
        }
    }
    let a268 = "a".repeat(268);
    let r = strim(&mut t, a268.as_bytes(), b"", 12, 255, "ASCII");
    assert!(r.ends_with("length=255 width=255"));
    assert_eq!(
        strim(&mut t, b"\x80\x80\x80", b"...", 0, 10, "UTF-8"),
        "[3f 3f 3f] length=3 width=3"
    );
    assert_eq!(
        strim(&mut t, b"abcdefghijklmnop", b"\x80\x80\x80", 0, 10, "UTF-8"),
        "[61 62 63 64 65 66 67 80 80 80] length=10 width=10"
    );
    assert_eq!(
        strim(&mut t, b"abc", b"abcdefghijklmnop", 0, 1, "ASCII"),
        "[61 62 63 64 65 66 67 68 69 6a 6b 6c 6d 6e 6f 70] length=16 width=16"
    );
    assert_eq!(
        strim(&mut t, "日本語abc".as_bytes(), b"...", -3, 10, "UTF-8"),
        "[61 62 63] length=3 width=3"
    );
    assert_eq!(
        strim(&mut t, "日本語abcdef".as_bytes(), b"...", 3, -1, "UTF-8"),
        "[61 62 2e 2e 2e] length=5 width=5"
    );
    let greek = b"\x00\x00\x00\x00+\x00\x00\x00\x00k\x00'\x11Yz";
    assert_eq!(
        strim(&mut t, greek, b"...", 1, 0, "greek"),
        "[2e 2e 2e] length=3 width=3"
    );
    assert_eq!(
        err(
            &mut t,
            "mb_strimwidth",
            vec![
                s("abc"),
                Value::Int(100),
                Value::Int(10),
                s("..."),
                s("UTF-8")
            ]
        ),
        "ValueError: mb_strimwidth(): Argument #2 ($start) is out of range"
    );
    assert_eq!(
        err(
            &mut t,
            "mb_strimwidth",
            vec![
                s("abc"),
                Value::Int(0),
                Value::Int(-100),
                s("..."),
                s("UTF-8")
            ]
        ),
        "ValueError: mb_strimwidth(): Argument #3 ($width) is out of range"
    );
}
