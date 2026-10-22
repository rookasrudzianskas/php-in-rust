//! `libmbfl/filters/mbfilter_utf32.c`: UTF-32, UTF-32BE, UTF-32LE.

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;

pub static MBFL_ENCODING_UTF32: Encoding = Encoding {
    no: NoEncoding::Utf32,
    name: "UTF-32",
    mime_name: Some("UTF-32"),
    aliases: &["utf32"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf32_to_wchar,
    from_wchar: mb_wchar_to_utf32be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UTF32BE: Encoding = Encoding {
    no: NoEncoding::Utf32be,
    name: "UTF-32BE",
    mime_name: Some("UTF-32BE"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf32be_to_wchar,
    from_wchar: mb_wchar_to_utf32be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UTF32LE: Encoding = Encoding {
    no: NoEncoding::Utf32le,
    name: "UTF-32LE",
    mime_name: Some("UTF-32LE"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf32le_to_wchar,
    from_wchar: mb_wchar_to_utf32le,
    check: None,
    cut: None,
};

const DETECTED_BE: u32 = 1;
const DETECTED_LE: u32 = 2;

/// `mb_utf32_to_wchar`: a BOM selects the byte order; big-endian otherwise.
pub fn mb_utf32_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let mut unused = 0;
    if *state == DETECTED_BE {
        return mb_utf32be_to_wchar(input, out, &mut unused);
    } else if *state == DETECTED_LE {
        return mb_utf32le_to_wchar(input, out, &mut unused);
    } else if input.len() >= 4 {
        let w = u32::from_be_bytes([input[0], input[1], input[2], input[3]]);
        if w == 0xFFFE0000 {
            // Little-endian BOM
            *input = &input[4..];
            *state = DETECTED_LE;
            return mb_utf32le_to_wchar(input, out, &mut unused);
        } else if w == 0xFEFF {
            // Big-endian BOM; not sent to the output
            *input = &input[4..];
        }
    }
    *state = DETECTED_BE;
    mb_utf32be_to_wchar(input, out, &mut unused)
}

fn utf32_to_wchar(input: &mut &[u8], out: &mut [u32], word: fn([u8; 4]) -> u32) -> usize {
    let s = *input;
    let e = s.len() & !3;
    let mut p = 0usize;
    let mut o = 0usize;
    while p < e && o < out.len() {
        let w = word([s[p], s[p + 1], s[p + 2], s[p + 3]]);
        p += 4;
        out[o] = if w < WCSPLANE_UTF32MAX && !(0xD800..=0xDFFF).contains(&w) {
            w
        } else {
            BAD_INPUT
        };
        o += 1;
    }
    if p == e && (s.len() & 3) != 0 && o < out.len() {
        // 1-3 trailing bytes
        out[o] = BAD_INPUT;
        o += 1;
        p = s.len();
    }
    *input = &s[p..];
    o
}

pub fn mb_utf32be_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    utf32_to_wchar(input, out, u32::from_be_bytes)
}

pub fn mb_utf32le_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    utf32_to_wchar(input, out, u32::from_le_bytes)
}

pub fn mb_wchar_to_utf32be(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < WCSPLANE_UTF32MAX {
            buf.append(&w.to_be_bytes());
        } else {
            mb_illegal_output(w, mb_wchar_to_utf32be, buf);
        }
    }
}

pub fn mb_wchar_to_utf32le(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < WCSPLANE_UTF32MAX {
            buf.append(&w.to_le_bytes());
        } else {
            mb_illegal_output(w, mb_wchar_to_utf32le, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::mbfl::convert::decode_all;

    #[test]
    fn decode() {
        assert_eq!(
            decode_all(b"\x00\x00\xFE\xFF\x00\x00\x00\x41", &MBFL_ENCODING_UTF32),
            vec![0x41]
        );
        assert_eq!(
            decode_all(b"\xFF\xFE\x00\x00\x41\x00\x00\x00", &MBFL_ENCODING_UTF32),
            vec![0x41]
        );
        assert_eq!(
            decode_all(
                b"\x00\x00\xD8\x00\x00\x11\x00\x00\x00\x00",
                &MBFL_ENCODING_UTF32BE
            ),
            vec![BAD_INPUT, BAD_INPUT, BAD_INPUT]
        );
    }
}
