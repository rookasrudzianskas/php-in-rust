//! `libmbfl/filters/mbfilter_ucs4.c`: UCS-4, UCS-4BE, UCS-4LE (any 32-bit
//! value; only the error marker cannot be encoded).

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;

pub static MBFL_ENCODING_UCS4: Encoding = Encoding {
    no: NoEncoding::Ucs4,
    name: "UCS-4",
    mime_name: Some("UCS-4"),
    aliases: &["ISO-10646-UCS-4", "UCS4"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs4_to_wchar,
    from_wchar: mb_wchar_to_ucs4be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS4BE: Encoding = Encoding {
    no: NoEncoding::Ucs4be,
    name: "UCS-4BE",
    mime_name: Some("UCS-4BE"),
    aliases: &["byte4be"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs4be_to_wchar,
    from_wchar: mb_wchar_to_ucs4be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS4LE: Encoding = Encoding {
    no: NoEncoding::Ucs4le,
    name: "UCS-4LE",
    mime_name: Some("UCS-4LE"),
    aliases: &["byte4le"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs4le_to_wchar,
    from_wchar: mb_wchar_to_ucs4le,
    check: None,
    cut: None,
};

const DETECTED_BE: u32 = 1;
const DETECTED_LE: u32 = 2;

/// `mb_ucs4_to_wchar`: a BOM selects the byte order; big-endian otherwise.
pub fn mb_ucs4_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let mut unused = 0;
    if *state == DETECTED_BE {
        return mb_ucs4be_to_wchar(input, out, &mut unused);
    } else if *state == DETECTED_LE {
        return mb_ucs4le_to_wchar(input, out, &mut unused);
    } else if input.len() >= 4 {
        let w = u32::from_be_bytes([input[0], input[1], input[2], input[3]]);
        if w == 0xFFFE0000 {
            // Little-endian BOM
            *input = &input[4..];
            *state = DETECTED_LE;
            return mb_ucs4le_to_wchar(input, out, &mut unused);
        } else if w == 0xFEFF {
            // Big-endian BOM; not sent to the output
            *input = &input[4..];
        }
    }
    *state = DETECTED_BE;
    mb_ucs4be_to_wchar(input, out, &mut unused)
}

fn ucs4_to_wchar(input: &mut &[u8], out: &mut [u32], word: fn([u8; 4]) -> u32) -> usize {
    let s = *input;
    let e = s.len() & !3;
    let mut p = 0usize;
    let mut o = 0usize;
    while p < e && o < out.len() {
        out[o] = word([s[p], s[p + 1], s[p + 2], s[p + 3]]);
        p += 4;
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

pub fn mb_ucs4be_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    ucs4_to_wchar(input, out, u32::from_be_bytes)
}

pub fn mb_ucs4le_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    ucs4_to_wchar(input, out, u32::from_le_bytes)
}

pub fn mb_wchar_to_ucs4be(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w != BAD_INPUT {
            buf.append(&w.to_be_bytes());
        } else {
            mb_illegal_output(w, mb_wchar_to_ucs4be, buf);
        }
    }
}

pub fn mb_wchar_to_ucs4le(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w != BAD_INPUT {
            buf.append(&w.to_le_bytes());
        } else {
            mb_illegal_output(w, mb_wchar_to_ucs4le, buf);
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
            decode_all(b"\xFF\xFE\x00\x00\x41\x00\x00\x00\x01", &MBFL_ENCODING_UCS4),
            vec![0x41, BAD_INPUT]
        );
        assert_eq!(
            decode_all(b"\x7F\xFF\xFF\xFF", &MBFL_ENCODING_UCS4BE),
            vec![0x7FFFFFFF]
        );
    }
}
