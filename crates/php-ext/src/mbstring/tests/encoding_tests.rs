//! A Rust version of php-src's `ext/mbstring/tests/encoding_tests.inc`
//! (`testEncodingFromUTF16ConversionTable` and friends), working directly
//! on the converters.

use std::collections::HashMap;

use crate::mbstring::mbfl::convert::mb_fast_convert;
use crate::mbstring::mbfl::encoding::{
    Encoding, ILLEGAL_MODE_CHAR, NoEncoding, enc, name2encoding,
};
use crate::mbstring::strings::check_encoding;

/// Path of a file in php-src's `ext/mbstring/tests/data`.
pub fn data_path(name: &str) -> String {
    format!(
        "{}/../../tests/phpt/ext/mbstring/tests/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Character bytes and Unicode bytes, in file order and keyed by Unicode.
pub type ConversionTable = (Vec<(Vec<u8>, Vec<u8>)>, HashMap<Vec<u8>, Vec<u8>>);

/// `readConversionTable`: character bytes to big-endian Unicode (2 bytes,
/// or 4 with `utf32`). Codepoints 0xFFFD are skipped.
pub fn read_conversion_table(name: &str, utf32: bool) -> ConversionTable {
    let text = std::fs::read(data_path(name)).expect("data file");
    let mut list = Vec::new();
    let mut to: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    for line in text.split(|&c| c == b'\n') {
        if line.first() == Some(&b'#') || !line.starts_with(b"0x") {
            continue;
        }
        let line = String::from_utf8_lossy(line);
        let mut cols = line.split('\t');
        let (Some(c), Some(u)) = (cols.next(), cols.next()) else {
            continue;
        };
        let (Some(c), Some(u)) = (c.strip_prefix("0x"), u.trim().strip_prefix("0x")) else {
            continue;
        };
        let u = u
            .split(|ch: char| !ch.is_ascii_hexdigit())
            .next()
            .unwrap_or("");
        let (Ok(ch), Ok(cp)) = (
            u64::from_str_radix(c.trim(), 16),
            u32::from_str_radix(u, 16),
        ) else {
            continue;
        };
        if cp == 0xFFFD {
            continue;
        }
        let cp_bytes = if utf32 {
            cp.to_be_bytes().to_vec()
        } else {
            (cp as u16).to_be_bytes().to_vec()
        };
        let ch_bytes = if ch <= 0xFF {
            vec![ch as u8]
        } else if ch <= 0xFFFF {
            (ch as u16).to_be_bytes().to_vec()
        } else if ch <= 0xFF_FFFF {
            vec![(ch >> 16) as u8, (ch >> 8) as u8, ch as u8]
        } else {
            (ch as u32).to_be_bytes().to_vec()
        };
        list.push((ch_bytes.clone(), cp_bytes.clone()));
        to.insert(cp_bytes, ch_bytes);
    }
    (list, to)
}

/// `mb_convert_encoding` with substitute character `subst` (mode CHAR);
/// returns the output and the error count.
pub fn convert(
    s: &[u8],
    from: &'static Encoding,
    to: &'static Encoding,
    subst: u32,
) -> (Vec<u8>, u32) {
    mb_fast_convert(s, from, to, subst, ILLEGAL_MODE_CHAR)
}

pub fn encoding(name: &str) -> &'static Encoding {
    name2encoding(name.as_bytes()).unwrap_or_else(|| panic!("unknown encoding {name}"))
}

/// `testEncodingFromUTF16ConversionTable` for single-byte encodings: every
/// mapped byte converts both ways, unmapped bytes become "\0%" and
/// unmappable UTF-16BE characters become `replacement`.
pub fn test_sbcs_from_utf16_table(file: &str, name: &str, replacement: &[u8]) {
    let e = encoding(name);
    let utf16 = enc(NoEncoding::Utf16be);
    let (list, from_unicode) = read_conversion_table(file, false);
    let to_unicode: HashMap<Vec<u8>, Vec<u8>> = list.iter().cloned().collect();

    // testAllValidChars
    let (mut all_from, mut all_to) = (Vec::new(), Vec::new());
    for (c, u) in &list {
        all_from.extend_from_slice(c);
        all_to.extend_from_slice(u);
    }
    assert!(
        check_encoding(&all_from, e),
        "{name}: valid string rejected"
    );
    let (out, errors) = convert(&all_from, e, utf16, 0x25);
    assert_eq!(out, all_to, "{name} -> UTF-16BE");
    assert_eq!(errors, 0);
    let (back, errors) = convert(&all_to, utf16, e, 0x25);
    assert_eq!(back, all_from, "UTF-16BE -> {name}");
    assert_eq!(errors, 0);

    // testAllInvalidChars: unmapped bytes
    let good = &list[0];
    for b in 0..=255u8 {
        if to_unicode.contains_key(&vec![b]) {
            continue;
        }
        let mut s = vec![b];
        s.extend_from_slice(&good.0);
        assert!(!check_encoding(&s, e), "{name}: {b:#x} accepted");
        let (out, errors) = convert(&s, e, utf16, 0x25);
        let mut expected = b"\x00%".to_vec();
        expected.extend_from_slice(&good.1);
        assert_eq!(out, expected, "{name}: bad byte {b:#x}");
        assert!(errors > 0);
    }

    // convertAllInvalidChars: unmappable UTF-16BE words
    for w in 0..=0xFFFFu32 {
        let u = (w as u16).to_be_bytes().to_vec();
        if from_unicode.contains_key(&u) {
            continue;
        }
        let mut s = u.clone();
        s.extend_from_slice(&good.1);
        let (out, errors) = convert(&s, utf16, e, 0x25);
        let mut expected = replacement.to_vec();
        expected.extend_from_slice(&good.0);
        assert_eq!(out, expected, "UTF-16BE -> {name}: {w:#x}");
        assert!(errors > 0);
    }
}
