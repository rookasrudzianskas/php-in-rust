//! Expectations from php-src's iso8859_encodings, cp125x, cp866, cp850,
//! koi8r/u and armscii8 encoding tests.

use crate::mbstring::mbfl::convert::mb_fast_convert;
use crate::mbstring::mbfl::encoding::{ILLEGAL_MODE_CHAR, ILLEGAL_MODE_LONG};
use crate::mbstring::tests::encoding_tests::{
    convert, encoding, read_conversion_table, test_sbcs_from_utf16_table,
};

#[test]
fn iso8859_encodings() {
    for n in (1..=16).filter(|n| *n != 11 && *n != 12) {
        test_sbcs_from_utf16_table(&format!("8859-{n}.txt"), &format!("ISO-8859-{n}"), b"%");
    }
    // "long" illegal character markers
    let utf8 = encoding("UTF-8");
    let r = mb_fast_convert(
        b"\xAE",
        encoding("ISO8859-7"),
        utf8,
        '%' as u32,
        ILLEGAL_MODE_LONG,
    );
    assert_eq!(r.0, b"%");
    let r = mb_fast_convert(
        b"\xFF",
        encoding("ISO8859-8"),
        utf8,
        '%' as u32,
        ILLEGAL_MODE_LONG,
    );
    assert_eq!(r.0, b"%");
    // A substitute character which can't be encoded: '?' is used.
    let r = convert(
        b"\x23\x45",
        encoding("UTF-16BE"),
        encoding("ISO8859-7"),
        0x1234,
    );
    assert_eq!(r.0, b"?");
}

#[test]
fn windows_codepages() {
    test_sbcs_from_utf16_table("CP1251.txt", "CP1251", b"%");
    let utf8 = encoding("UTF-8");
    let r = mb_fast_convert(
        b"\x98",
        encoding("CP1251"),
        utf8,
        '%' as u32,
        ILLEGAL_MODE_LONG,
    );
    assert_eq!(r.0, b"%");
    let r = mb_fast_convert(
        b"\x12\x34",
        encoding("UTF-16BE"),
        encoding("CP1251"),
        '%' as u32,
        ILLEGAL_MODE_LONG,
    );
    assert_eq!(r.0, b"U+1234");

    test_sbcs_from_utf16_table("CP1252.txt", "CP1252", b"%");
    assert_eq!(
        convert(
            b"\x23\x45",
            encoding("UTF-16BE"),
            encoding("CP1252"),
            0x1234
        )
        .0,
        b"?"
    );

    test_sbcs_from_utf16_table("CP1254.txt", "CP1254", b"%");
    for bad in [b"\x81", b"\x9E"] {
        let r = mb_fast_convert(bad, encoding("CP1254"), utf8, '%' as u32, ILLEGAL_MODE_LONG);
        assert_eq!(r.0, b"%");
    }
    assert_eq!(
        convert(
            b"\x23\x45",
            encoding("UTF-16BE"),
            encoding("CP1254"),
            0x1234
        )
        .0,
        b"?"
    );
}

#[test]
fn dos_and_koi8() {
    for (file, name) in [
        ("CP866.txt", "CP866"),
        ("CP850.txt", "CP850"),
        ("KOI8-R.txt", "KOI8-R"),
        ("KOI8-U.txt", "KOI8-U"),
    ] {
        test_sbcs_from_utf16_table(file, name, b"%");
        assert_eq!(
            convert(b"\x23\x45", encoding("UTF-16BE"), encoding(name), 0x1234).0,
            b"?"
        );
    }
}

#[test]
fn armscii8() {
    let e = encoding("ARMSCII-8");
    let utf16 = encoding("UTF-16BE");
    let (list, _) = read_conversion_table("ARMSCII-8.txt", false);
    for (c, u) in &list {
        assert_eq!(convert(c, e, utf16, 0x25).0, *u, "{c:?}");
    }
    // These punctuation marks also exist in ASCII; ASCII wins on the way back.
    let irreversible = [0x28u8, 0x29, 0x2C, 0x2D, 0x2E];
    for (c, u) in &list {
        if irreversible.contains(&c[0]) {
            continue;
        }
        assert_eq!(convert(u, utf16, e, 0x25).0, *c, "{u:?}");
    }
    assert_eq!(convert(b"\x00\x28", utf16, e, 0x25).0, b"\xA5");
    assert_eq!(convert(b"\xFF", e, utf16, 0x25).0, b"\x00%");
    assert_eq!(convert(b"\x12\x34", utf16, e, 0x25).0, b"%");
    let _ = ILLEGAL_MODE_CHAR;
}
