//! `libmbfl/filters/mbfilter_ucs2.c`: UCS-2, UCS-2BE, UCS-2LE.

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;

pub static MBFL_ENCODING_UCS2: Encoding = Encoding {
    no: NoEncoding::Ucs2,
    name: "UCS-2",
    mime_name: Some("UCS-2"),
    aliases: &["ISO-10646-UCS-2", "UCS2", "UNICODE"],
    mblen_table: None,
    flag: ENCTYPE_WCS2,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs2_to_wchar,
    from_wchar: mb_wchar_to_ucs2be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS2BE: Encoding = Encoding {
    no: NoEncoding::Ucs2be,
    name: "UCS-2BE",
    mime_name: Some("UCS-2BE"),
    aliases: &["byte2be"],
    mblen_table: None,
    flag: ENCTYPE_WCS2,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs2be_to_wchar,
    from_wchar: mb_wchar_to_ucs2be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS2LE: Encoding = Encoding {
    no: NoEncoding::Ucs2le,
    name: "UCS-2LE",
    mime_name: Some("UCS-2LE"),
    aliases: &["byte2le"],
    mblen_table: None,
    flag: ENCTYPE_WCS2,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs2le_to_wchar,
    from_wchar: mb_wchar_to_ucs2le,
    check: None,
    cut: None,
};

const DETECTED_BE: u32 = 1;
const DETECTED_LE: u32 = 2;

/// `mb_ucs2_to_wchar`: a BOM selects the byte order; big-endian otherwise.
pub fn mb_ucs2_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let mut unused = 0;
    if *state == DETECTED_BE {
        return mb_ucs2be_to_wchar(input, out, &mut unused);
    } else if *state == DETECTED_LE {
        return mb_ucs2le_to_wchar(input, out, &mut unused);
    } else if input.len() >= 2 {
        let w = ((input[0] as u32) << 8) | input[1] as u32;
        if w == 0xFFFE {
            // Little-endian BOM
            *input = &input[2..];
            *state = DETECTED_LE;
            return mb_ucs2le_to_wchar(input, out, &mut unused);
        } else if w == 0xFEFF {
            // Big-endian BOM; not sent to the output
            *input = &input[2..];
        }
    }
    *state = DETECTED_BE;
    mb_ucs2be_to_wchar(input, out, &mut unused)
}

fn ucs2_to_wchar(input: &mut &[u8], out: &mut [u32], word: fn([u8; 2]) -> u16) -> usize {
    let s = *input;
    let e = s.len() & !1;
    let mut p = 0usize;
    let mut o = 0usize;
    while p < e && o < out.len() {
        out[o] = word([s[p], s[p + 1]]) as u32;
        p += 2;
        o += 1;
    }
    if p == e && (s.len() & 1) != 0 && o < out.len() {
        // A trailing byte
        out[o] = BAD_INPUT;
        o += 1;
        p += 1;
    }
    *input = &s[p..];
    o
}

pub fn mb_ucs2be_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    ucs2_to_wchar(input, out, u16::from_be_bytes)
}

pub fn mb_ucs2le_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    ucs2_to_wchar(input, out, u16::from_le_bytes)
}

pub fn mb_wchar_to_ucs2be(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < WCSPLANE_UCS2MAX {
            buf.append(&(w as u16).to_be_bytes());
        } else {
            mb_illegal_output(w, mb_wchar_to_ucs2be, buf);
        }
    }
}

pub fn mb_wchar_to_ucs2le(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < WCSPLANE_UCS2MAX {
            buf.append(&(w as u16).to_le_bytes());
        } else {
            mb_illegal_output(w, mb_wchar_to_ucs2le, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::mbfl::convert::{decode_all, encode_all};

    #[test]
    fn roundtrip() {
        assert_eq!(
            decode_all(b"\xFF\xFE\x41\x00\x42", &MBFL_ENCODING_UCS2),
            vec![0x41, BAD_INPUT]
        );
        assert_eq!(decode_all(b"\xD8\x00", &MBFL_ENCODING_UCS2BE), vec![0xD800]);
        let b = encode_all(
            &[0x41, 0x10000],
            &MBFL_ENCODING_UCS2LE,
            b'?' as u32,
            ILLEGAL_MODE_CHAR,
        );
        assert_eq!(b.out, b"\x41\x00\x3F\x00");
    }
}
