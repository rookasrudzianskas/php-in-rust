//! mb_strcut, ported from php-src's mb_strcut*.phpt and gh9535*.phpt.

use super::*;

fn cut(t: &mut TestVm, st: &[u8], start: i64, len: Option<i64>, enc: &str) -> Value {
    let len = len.map_or(Value::Null, Value::Int);
    call(t, "mb_strcut", vec![b(st), Value::Int(start), len, s(enc)])
}

/// The test's `MBStringChars()`: the hex of each character, bracketed.
fn chars(t: &mut TestVm, v: Value, enc: &str) -> String {
    let parts = hexes(&call(t, "mb_str_split", vec![v, Value::Int(1), s(enc)]));
    format!("[{}]", parts.join(" "))
}

fn cut_chars(t: &mut TestVm, st: &[u8], start: i64, len: i64, enc: &str) -> String {
    let v = cut(t, st, start, Some(len), enc);
    chars(t, v, enc)
}

fn cut_hex(t: &mut TestVm, st: &[u8], start: i64, len: Option<i64>, enc: &str) -> String {
    format!("[{}]", hex(&cut(t, st, start, len, enc)))
}

fn convert(t: &mut TestVm, st: &[u8], to: &str, from: &str) -> Vec<u8> {
    bytes(&call(t, "mb_convert_encoding", vec![b(st), s(to), s(from)]))
}

/// Compares the printed lines with a phpt's expected output.
fn assert_output(lines: &[String], test: &str) {
    let expected = normalize(&phpt_section(test, "EXPECT"));
    let got = normalize(&lines.join("\n"));
    for (i, (g, e)) in got.lines().zip(expected.lines()).enumerate() {
        assert_eq!(g, e, "{test}: line {}", i + 1);
    }
    assert_eq!(
        got.lines().count(),
        expected.lines().count(),
        "{test}: line count"
    );
}

const JP_CUTS: [(i64, i64); 16] = [
    (0, 3),
    (0, 4),
    (0, 5),
    (0, 6),
    (0, 7),
    (0, 8),
    (1, 3),
    (1, 6),
    (1, 8),
    (2, 5),
    (5, 9),
    (5, 11),
    (6, 13),
    (7, 13),
    (1, 100),
    (50, 100),
];

const JP_CUTS_LONG: [(i64, i64); 18] = [
    (0, 3),
    (0, 4),
    (0, 5),
    (0, 6),
    (0, 7),
    (0, 8),
    (0, 9),
    (1, 3),
    (1, 6),
    (1, 8),
    (1, 9),
    (2, 5),
    (5, 9),
    (5, 11),
    (6, 13),
    (7, 13),
    (1, 100),
    (50, 100),
];

#[test]
fn mb_strcut_phpt() {
    let mut t = TestVm::new();
    let t = &mut t;
    let mut out: Vec<String> = Vec::new();
    let euc_jp = unhex(
        "30313233a4b3a4cecab8bbfacef3a4cfc6fccbdcb8eca4c7a4b9a1a34555432d4a50a4f2bbc8a4c3a4c6a4a4a4dea4b9a1a3c6fccbdcb8eca4cfccccc5ddbdada4a4a1a3",
    );
    let utf8 = unhex("e288ae2045e28b856461203d2051");
    let utf16le = unhex(
        "1a043804400438043b043b04380446043004200069007300200043007900720069006c006c0069006300",
    );
    let utf32be = convert(t, &utf8, "UTF-32BE", "UTF-8");
    let sample = "漢字 abc カナ".as_bytes();
    let iso2022jp = convert(t, sample, "ISO-2022-JP", "UTF-8");
    let jis = convert(t, sample, "JIS", "UTF-8");
    let iso2022jp2004 = convert(t, "漢字 abc カナ凜".as_bytes(), "ISO-2022-JP-2004", "UTF-8");
    let iso2022jpms = convert(t, sample, "ISO-2022-JP-MS", "UTF-8");
    let iso2022jp_kddi = convert(t, sample, "ISO-2022-JP-KDDI", "UTF-8");
    let gb18030 = convert(t, sample, "GB18030", "UTF-8");

    out.push("== EUC-JP ==".into());
    for (start, len) in [(6, 5), (5, 5), (0, 100)] {
        out.push(cut_chars(t, &euc_jp, start, len, "EUC-JP"));
    }
    let v = cut(t, &euc_jp, 100, Some(10), "EUC-JP");
    out.push(
        if bytes(&v).is_empty() {
            "OK"
        } else {
            "No good"
        }
        .into(),
    );
    let v = cut(t, &euc_jp, -100, Some(10), "EUC-JP");
    out.push(
        if !bytes(&v).is_empty() {
            "OK"
        } else {
            "No good"
        }
        .into(),
    );

    out.push("== UTF-8 ==".into());
    for (start, len) in [
        (0, 0),
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (0, 5),
        (1, 2),
        (1, 3),
        (1, 4),
    ] {
        out.push(cut_chars(t, &utf8, start, len, "UTF-8"));
    }
    out.push(cut_chars(t, "AåBäCöDü".as_bytes(), 2, 100, "UTF-8"));

    out.push("== UTF-16 ==".into());
    for (label, st, start, len) in [
        ("Single byte", &b"\xFF"[..], 0, 100),
        ("With from=1", b"\xff\x01", 1, 100),
        ("Bad surrogate", b"\xD9\xFF", 0, 100),
        (
            "Bad surrogate followed by other bytes",
            b"\xd9\x00\x12C",
            0,
            100,
        ),
        ("BE byte order mark", b"\xFE\xFF", 0, 100),
        ("LE byte order mark", b"\xFF\xFE", 0, 100),
        ("Length=0", b"\x00\x01\x00\x00", 1, -512),
    ] {
        out.push(format!(
            "{label}: {}",
            cut_hex(t, st, start, Some(len), "UTF-16")
        ));
    }

    out.push("== UTF-16LE ==".into());
    for (start, len) in [(0, 0), (0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (1, 4)] {
        out.push(cut_chars(t, &utf16le, start, len, "UTF-16LE"));
    }
    out.push(format!(
        "Single byte: {}",
        cut_hex(t, b"\xFF", 0, Some(100), "UTF-16LE")
    ));

    out.push("== UTF-32BE ==".into());
    for (start, len) in [(0, 3), (0, 4), (0, 5), (1, 8), (3, 9)] {
        out.push(cut_chars(t, &utf32be, start, len, "UTF-32BE"));
    }

    out.push("== ISO-2022-JP ==".into());
    for (start, len) in JP_CUTS {
        out.push(cut_chars(t, &iso2022jp, start, len, "ISO-2022-JP"));
    }
    out.push(format!(
        "Error followed by ASCII char: {}",
        cut_hex(t, b"\xdaK", 0, Some(100), "ISO-2022-JP")
    ));

    for (name, st) in [
        ("ISO-2022-JP-2004", &iso2022jp2004),
        ("ISO-2022-JP-MS", &iso2022jpms),
    ] {
        out.push(format!("== {name} =="));
        for (start, len) in JP_CUTS_LONG {
            out.push(cut_chars(t, st, start, len, name));
        }
    }

    out.push("== JIS ==".into());
    for (start, len) in JP_CUTS {
        out.push(cut_chars(t, &jis, start, len, "JIS"));
    }
    out.push(format!(
        "0xA3: {}",
        cut_hex(t, b"\xA3aaaaaa", 0, Some(100), "JIS")
    ));
    out.push(format!(
        "Bad escape sequence followed by null byte: {}",
        cut_hex(t, b"\x1b\x00", 1, Some(100), "JIS")
    ));

    out.push("== ISO-2022-JP-KDDI ==".into());
    for (start, len) in JP_CUTS {
        out.push(cut_chars(
            t,
            &iso2022jp_kddi,
            start,
            len,
            "ISO-2022-JP-KDDI",
        ));
    }

    out.push("== CP50220 ==".into());
    for (label, st) in [
        ("Single byte 0xFF", &b"\xFF"[..]),
        ("Double byte 0xFF", b"\xFF\xFF"),
        (
            "Sample string with multiple null bytes",
            b"\xCF\x00\x00\x00\x00\x00d\x00\x00",
        ),
        (
            "Bad escape sequence preceded by bad bytes",
            b"\xFF\xFF\x1B\x00",
        ),
        (
            "Good JISX 0208 sequence, but it won't fit in max number of bytes",
            b"\x1B$BGV\x17",
        ),
        ("Bad escape sequence followed by GR kana", b"\x1B$\xAC\x13"),
    ] {
        out.push(format!(
            "{label}: {}",
            cut_hex(t, st, 0, Some(100), "CP50220")
        ));
    }

    out.push("== UTF-7 ==".into());
    for (label, st, start, len) in [
        ("Single byte 0x01", &b"\x01"[..], 0, 100),
        ("UTF-16 section ends abruptly", b"+Q", 1, 100),
        (
            "UTF-16 section ends abruptly in middle of 2nd codepoint",
            b"+QxxC",
            0,
            100,
        ),
        ("Cutting in middle of UTF-16 section", b"+UUU", -1, 255),
        ("Cutting in middle of UTF-16 section (2)", b"+UUUU", -2, 255),
    ] {
        out.push(format!(
            "{label}: [{}]",
            text(&cut(t, st, start, Some(len), "UTF-7"))
        ));
    }

    out.push("== UTF7-IMAP ==".into());
    for (label, st, start) in [
        ("Single byte 0x01", &b"\x01"[..], 0),
        ("UTF-16 section ends abruptly", b"&Q", 1),
        (
            "UTF-16 section ends abruptly in middle of 2nd codepoint",
            b"&QxxC",
            0,
        ),
        ("UTF-16 section is terminated improperly", b"&i6o\x83", 0),
    ] {
        out.push(format!(
            "{label}: [{}]",
            text(&cut(t, st, start, Some(100), "UTF7-IMAP"))
        ));
    }

    out.push("== GB18030 ==".into());
    for (label, st, start, len) in [
        ("Empty string", &b""[..], 0, 5),
        ("Empty string 2", b"", -2, 1),
        ("Empty string 3", b"", 0, -1),
        ("Invalid byte 0xF5", b"\xF5a", 1, 100),
        ("Double-byte char", b"\xAFw", -1, 100),
    ] {
        out.push(format!(
            "{label}: {}",
            cut_hex(t, st, start, Some(len), "GB18030")
        ));
    }
    for (start, len) in [
        (0, 0),
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (0, 5),
        (1, 2),
        (1, 3),
        (1, 4),
    ] {
        out.push(cut_chars(t, &gb18030, start, len, "GB18030"));
    }
    out.push("Operating on 4-byte GB18030 character:".into());
    let fourbyte = convert(t, b"\x21\x0A", "GB18030", "UTF-16BE");
    for (start, len) in [
        (0, 4),
        (1, 4),
        (2, 4),
        (3, 4),
        (4, 4),
        (1, 3),
        (2, 3),
        (2, 4),
        (0, -1),
    ] {
        out.push(cut_chars(t, &fourbyte, start, len, "GB18030"));
    }
    out.push(cut_hex(t, &unhex("84308130"), 2, None, "GB18030"));

    out.push("== UHC ==".into());
    out.push(format!(
        "Single byte 0x96: {}",
        cut_hex(t, b"\x96", 1, Some(1280), "UHC")
    ));

    out.push("== ASCII ==".into());
    for (label, start, len) in [
        ("Empty", 0, 0),
        ("Empty", 1, 0),
        ("Empty", 2, 0),
        ("One char", 2, 1),
        ("Two chars", 1, 2),
        ("Two chars", 1, 3),
    ] {
        out.push(format!(
            "{label}: {}",
            cut_hex(t, b"ABC", start, Some(len), "ASCII")
        ));
    }

    out.push("== UCS-2BE ==".into());
    for (label, start, len) in [
        ("Empty", 0, 0),
        ("Empty", 1, 0),
        ("Empty", 2, 0),
        ("Empty", 2, 1),
        ("One char", 1, 2),
        ("Cut in middle of following char", 1, 3),
        ("Two chars", 1, 4),
    ] {
        out.push(format!(
            "{label}: {}",
            cut_hex(t, b"\x00A\x00B\x00C", start, Some(len), "UCS-2BE")
        ));
    }

    out.push("== UCS-4BE ==".into());
    for len in [5, 6, 8] {
        let st = b"\x00\x00\x00\x41\x00\x00\x00\x42";
        out.push(format!(
            "From 1, Length {len}: {}",
            cut_hex(t, st, 1, Some(len), "UCS-4BE")
        ));
    }

    assert_output(&out, "mb_strcut");
    assert_eq!(t.output(), "");
}

#[test]
fn missing_boundary_check_and_negative_length() {
    let mut t = TestVm::new();
    // mb_strcut_missing_boundary_check.phpt
    for (enc, st, expected) in [
        (
            "UCS-4LE",
            &b"\x61\x00\x00\x00\x62\x00\x00\x00\x63\x00\x00\x00"[..],
            "610000006200000063000000",
        ),
        (
            "UCS-4BE",
            b"\x00\x00\x00\x61\x00\x00\x00\x62\x00\x00\x00\x63",
            "000000610000006200000063",
        ),
        ("UCS-2LE", b"\x61\x00\x62\x00\x63\x00", "610062006300"),
        ("UCS-2BE", b"\x00\x61\x00\x62\x00\x63", "006100620063"),
        ("UTF-16", b"\x00\x61\x00\x62\x00\x63", "006100620063"),
        ("UTF-8", b"abc", "616263"),
        ("ISO-8859-1", b"abc", "616263"),
    ] {
        call(&mut t, "mb_internal_encoding", vec![s(enc)]);
        let r = call(
            &mut t,
            "mb_strcut",
            vec![b(st), Value::Int(0), Value::Int(32)],
        );
        assert_eq!(hex(&r), expected, "{enc}");
    }
    // mb_strcut_negative_length.phpt
    call(&mut t, "mb_internal_encoding", vec![s("UTF-8")]);
    for (len, expected) in [(-3, "éjà"), (-4, "éj"), (-5, "éj"), (-6, "é"), (-999, "")] {
        let r = call(
            &mut t,
            "mb_strcut",
            vec![s("Déjà vu"), Value::Int(1), Value::Int(len)],
        );
        assert_eq!(text(&r), expected, "{len}");
    }
}

const GH9535_CASES: [(&str, i64); 5] = [
    ("宛如繁星般宛如皎月般", 15),
    ("星のように月のように", 20),
    ("あaいb", 10),
    (
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        10,
    ),
    ("???", 2),
];

/// gh9535*.phpt: cutting a stateful encoding must neither leave half a
/// character nor drop the escape sequence that ends it.
fn gh9535_output(t: &mut TestVm, encodings: &[&str], trimmed_last: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (input, len)) in GH9535_CASES.iter().enumerate() {
        let last = i == GH9535_CASES.len() - 1;
        for enc in encodings {
            let converted = convert(t, input.as_bytes(), enc, "UTF-8");
            let cut_str = bytes(&cut(t, &converted, 0, Some(*len), enc));
            let back = String::from_utf8_lossy(&convert(t, &cut_str, "UTF-8", enc)).into_owned();
            let line = format!(": {back}");
            let line = if last && trimmed_last {
                line.trim().to_string()
            } else {
                line
            };
            out.push(format!("{enc}{line}"));
        }
        out.push(String::new());
    }
    let (input, len) = GH9535_CASES[GH9535_CASES.len() - 1];
    for enc in encodings {
        let v = cut(t, input.as_bytes(), 0, Some(len), enc);
        if trimmed_last {
            out.push(format!("{enc}{}", format!(": {}", text(&v)).trim()));
        } else {
            out.push(dump(t, v).trim_end().to_string());
        }
    }
    out
}

#[test]
fn gh9535() {
    let mut t = TestVm::new();
    let encodings = [
        "UTF-16",
        "UTF-16BE",
        "UTF-16LE",
        "UTF-7",
        "UTF7-IMAP",
        "ISO-2022-JP-MS",
        "GB18030",
        "HZ",
        "ISO-2022-KR",
        "ISO-2022-JP-MOBILE#KDDI",
        "CP50220",
        "CP50221",
        "CP50222",
    ];
    let out = gh9535_output(&mut t, &encodings, true);
    assert_eq!(
        normalize(&out.join("\n")),
        normalize(&phpt_section("gh9535", "EXPECT"))
    );
    let out = gh9535_output(&mut t, &["JIS", "ISO-2022-JP", "ISO-2022-JP-2004"], false);
    assert_eq!(
        normalize(&out.join("\n")),
        normalize(&phpt_section("gh9535b", "EXPECTF"))
    );
    assert_eq!(t.output(), "");
}
