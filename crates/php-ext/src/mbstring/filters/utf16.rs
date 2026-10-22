//! `libmbfl/filters/mbfilter_utf16.c`: UTF-16, UTF-16BE, UTF-16LE (the
//! scalar implementations; php-src's AVX2 variants behave identically).

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;

pub static MBFL_ENCODING_UTF16: Encoding = Encoding {
    no: NoEncoding::Utf16,
    name: "UTF-16",
    mime_name: Some("UTF-16"),
    aliases: &["utf16"],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf16_to_wchar,
    from_wchar: mb_wchar_to_utf16be,
    check: None,
    cut: Some(mb_cut_utf16),
};

pub static MBFL_ENCODING_UTF16BE: Encoding = Encoding {
    no: NoEncoding::Utf16be,
    name: "UTF-16BE",
    mime_name: Some("UTF-16BE"),
    aliases: &[],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf16be_to_wchar,
    from_wchar: mb_wchar_to_utf16be,
    check: None,
    cut: Some(mb_cut_utf16be),
};

pub static MBFL_ENCODING_UTF16LE: Encoding = Encoding {
    no: NoEncoding::Utf16le,
    name: "UTF-16LE",
    mime_name: Some("UTF-16LE"),
    aliases: &[],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf16le_to_wchar,
    from_wchar: mb_wchar_to_utf16le,
    check: None,
    cut: Some(mb_cut_utf16le),
};

const DETECTED_BE: u32 = 1;
const DETECTED_LE: u32 = 2;

/// `mb_utf16_to_wchar`: a BOM selects the byte order; big-endian otherwise.
pub fn mb_utf16_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let mut unused = 0;
    if *state == DETECTED_BE {
        return mb_utf16be_to_wchar(input, out, &mut unused);
    } else if *state == DETECTED_LE {
        return mb_utf16le_to_wchar(input, out, &mut unused);
    } else if input.len() >= 2 {
        let n = ((input[0] as u16) << 8) | input[1] as u16;
        if n == 0xFFFE {
            // Little-endian BOM
            *input = &input[2..];
            *state = DETECTED_LE;
            return mb_utf16le_to_wchar(input, out, &mut unused);
        }
        if n == 0xFEFF {
            // Big-endian BOM; not sent to the output
            *input = &input[2..];
        }
    }
    *state = DETECTED_BE;
    mb_utf16be_to_wchar(input, out, &mut unused)
}

/// Shared body of `mb_utf16be_to_wchar` / `mb_utf16le_to_wchar`.
fn utf16_to_wchar(input: &mut &[u8], out: &mut [u32], word: fn(u8, u8) -> u16) -> usize {
    let s = *input;
    // Only whole 16-bit words; a trailing byte is handled at the end.
    let e = s.len() & !1;
    let mut p = 0usize;
    let mut o = 0usize;
    // One less than the space available: an iteration may emit two words.
    let limit = out.len().saturating_sub(1);

    while p < e && o < limit {
        let n = word(s[p], s[p + 1]);
        p += 2;
        if (0xD800..=0xDBFF).contains(&n) {
            // Surrogate pair
            if p < e {
                let n2 = word(s[p], s[p + 1]);
                p += 2;
                if (0xD800..=0xDBFF).contains(&n2) {
                    // A first half where the second was expected.
                    out[o] = BAD_INPUT;
                    o += 1;
                    p -= 2;
                } else if (0xDC00..=0xDFFF).contains(&n2) {
                    out[o] = ((((n & 0x3FF) as u32) << 10) | (n2 & 0x3FF) as u32) + 0x10000;
                    o += 1;
                } else {
                    // The first half followed by a 'normal' codepoint.
                    out[o] = BAD_INPUT;
                    out[o + 1] = n2 as u32;
                    o += 2;
                }
            } else {
                out[o] = BAD_INPUT;
                o += 1;
            }
        } else if (0xDC00..=0xDFFF).contains(&n) {
            // The second half of a surrogate pair came first.
            out[o] = BAD_INPUT;
            o += 1;
        } else {
            out[o] = n as u32;
            o += 1;
        }
    }

    if p == e && (s.len() & 1) != 0 && o < limit {
        // An extra trailing byte.
        out[o] = BAD_INPUT;
        o += 1;
        p += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_utf16be_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    utf16_to_wchar(input, out, |c1, c2| ((c1 as u16) << 8) | c2 as u16)
}

pub fn mb_utf16le_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    utf16_to_wchar(input, out, |c1, c2| ((c2 as u16) << 8) | c1 as u16)
}

pub fn mb_wchar_to_utf16be(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < WCSPLANE_UCS2MAX {
            buf.add2((w >> 8) as u8, w as u8);
        } else if w < WCSPLANE_UTF32MAX {
            let n1 = (((w >> 10) - 0x40) | 0xD800) as u16;
            let n2 = ((w & 0x3FF) | 0xDC00) as u16;
            buf.add4((n1 >> 8) as u8, n1 as u8, (n2 >> 8) as u8, n2 as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_utf16be, buf);
        }
    }
}

pub fn mb_wchar_to_utf16le(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < WCSPLANE_UCS2MAX {
            buf.add2(w as u8, (w >> 8) as u8);
        } else if w < WCSPLANE_UTF32MAX {
            let n1 = (((w >> 10) - 0x40) | 0xD800) as u16;
            let n2 = ((w & 0x3FF) | 0xDC00) as u16;
            buf.add4(n1 as u8, (n1 >> 8) as u8, n2 as u8, (n2 >> 8) as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_utf16le, buf);
        }
    }
}

/// Shared body of `mb_cut_utf16be` / `mb_cut_utf16le`. php-src's attempt
/// to back up over the second half of a surrogate pair at the start
/// adjusts `from` after the start was computed, so it has no effect.
fn cut_utf16(s: &[u8], from: usize, len: usize, word: fn(u8, u8) -> u32) -> Vec<u8> {
    let end = s.len();
    let mut len = len.min(end - from);
    let from = from & !1;
    len &= !1;
    let start = from;
    if len < 2 || end - start < 2 {
        return Vec::new();
    }
    // Do not end in the middle of a surrogate pair.
    let mut stop = (start + len).min(end);
    let ending_cp = word(s[stop - 2], s[stop - 1]);
    if (0xD800..=0xDBFF).contains(&ending_cp) {
        stop -= 2;
    }
    s[start..stop].to_vec()
}

pub fn mb_cut_utf16be(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    cut_utf16(s, from, len, |a, b| ((a as u32) << 8) + b as u32)
}

pub fn mb_cut_utf16le(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    cut_utf16(s, from, len, |a, b| ((b as u32) << 8) + a as u32)
}

/// `mb_cut_utf16`: skips a BOM and cuts in the order it selects.
pub fn mb_cut_utf16(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    if len < 2 || s.len() < 2 {
        return Vec::new();
    }
    let cp = ((s[0] as u32) << 8) + s[1] as u32;
    if cp == 0xFFFE {
        // Little-endian BOM
        mb_cut_utf16le(s, from.max(2), len)
    } else {
        let from = if cp == 0xFEFF { from.max(2) } else { from };
        mb_cut_utf16be(s, from, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::mbfl::convert::{decode_all, encode_all};

    #[test]
    fn decode_surrogates_and_errors() {
        let be = &MBFL_ENCODING_UTF16BE;
        assert_eq!(decode_all(b"\xD8\x3D\xDE\x00", be), vec![0x1F600]);
        assert_eq!(decode_all(b"\xD8\x3D\x00\x41", be), vec![BAD_INPUT, 0x41]);
        assert_eq!(decode_all(b"\xDC\x00\x00\x41", be), vec![BAD_INPUT, 0x41]);
        assert_eq!(
            decode_all(b"\xD8\x00\xD8\x00\xDC\x00", be),
            vec![BAD_INPUT, 0x10000]
        );
        assert_eq!(decode_all(b"\x00\x41\x42", be), vec![0x41, BAD_INPUT]);
        assert_eq!(
            decode_all(b"\xFF\xFE\x41\x00", &MBFL_ENCODING_UTF16),
            vec![0x41]
        );
        assert_eq!(
            decode_all(b"\xFE\xFF\x00\x41", &MBFL_ENCODING_UTF16),
            vec![0x41]
        );
        assert_eq!(decode_all(b"\x00\x41", &MBFL_ENCODING_UTF16), vec![0x41]);
    }

    #[test]
    fn encode() {
        let b = encode_all(
            &[0x41, 0x1F600, 0x110000],
            &MBFL_ENCODING_UTF16LE,
            b'?' as u32,
            ILLEGAL_MODE_CHAR,
        );
        assert_eq!(b.out, b"\x41\x00\x3D\xD8\x00\xDE\x3F\x00");
        assert_eq!(b.errors, 1);
    }

    #[test]
    fn cut() {
        // mb_strcut.phpt: UTF-16LE
        let s = b"\x3d\xd8\x00\xde\x41\x00";
        assert_eq!(mb_cut_utf16le(s, 0, 2), b"");
        assert_eq!(mb_cut_utf16le(s, 0, 4), b"\x3d\xd8\x00\xde");
        assert_eq!(mb_cut_utf16le(s, 1, 5), b"\x3d\xd8\x00\xde");
    }
}
