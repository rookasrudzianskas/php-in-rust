//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UTF8: Encoding = Encoding {
    no: NoEncoding::Utf8,
    name: "UTF-8",
    mime_name: Some("UTF-8"),
    aliases: &["utf8"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_to_wchar,
    from_wchar: mb_wchar_to_utf8,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_DOCOMO: Encoding = Encoding {
    no: NoEncoding::Utf8Docomo,
    name: "UTF-8-Mobile#DOCOMO",
    mime_name: Some("UTF-8"),
    aliases: &["UTF-8-DOCOMO", "UTF8-DOCOMO"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_docomo_to_wchar,
    from_wchar: mb_wchar_to_utf8_docomo,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_KDDI_A: Encoding = Encoding {
    no: NoEncoding::Utf8KddiA,
    name: "UTF-8-Mobile#KDDI-A",
    mime_name: Some("UTF-8"),
    aliases: &[],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_kddi_a_to_wchar,
    from_wchar: mb_wchar_to_utf8_kddi_a,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_KDDI_B: Encoding = Encoding {
    no: NoEncoding::Utf8KddiB,
    name: "UTF-8-Mobile#KDDI-B",
    mime_name: Some("UTF-8"),
    aliases: &["UTF-8-Mobile#KDDI", "UTF-8-KDDI", "UTF8-KDDI"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_kddi_b_to_wchar,
    from_wchar: mb_wchar_to_utf8_kddi_b,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_SB: Encoding = Encoding {
    no: NoEncoding::Utf8Sb,
    name: "UTF-8-Mobile#SOFTBANK",
    mime_name: Some("UTF-8"),
    aliases: &["UTF-8-SOFTBANK", "UTF8-SOFTBANK"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_sb_to_wchar,
    from_wchar: mb_wchar_to_utf8_sb,
    check: None,
    cut: Some(mb_cut_utf8),
};

/// `mb_utf8_to_wchar`: strict UTF-8; each maximal invalid subpart becomes
/// one `BAD_INPUT`.
pub fn mb_utf8_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    let limit = out.len();

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if c < 0xC2 {
            out[o] = BAD_INPUT;
        } else if c <= 0xDF {
            // 2-byte character
            if p < e {
                let c2 = s[p];
                p += 1;
                if (c2 & 0xC0) != 0x80 {
                    out[o] = BAD_INPUT;
                    p -= 1;
                } else {
                    out[o] = ((c as u32 & 0x1F) << 6) | (c2 as u32 & 0x3F);
                }
            } else {
                out[o] = BAD_INPUT;
            }
        } else if c <= 0xEF {
            // 3-byte character
            if e - p >= 2 {
                let c2 = s[p];
                let c3 = s[p + 1];
                p += 2;
                if (c2 & 0xC0) != 0x80 || (c == 0xE0 && c2 < 0xA0) || (c == 0xED && c2 >= 0xA0) {
                    out[o] = BAD_INPUT;
                    p -= 2;
                } else if (c3 & 0xC0) != 0x80 {
                    out[o] = BAD_INPUT;
                    p -= 1;
                } else {
                    out[o] = ((c as u32 & 0xF) << 12) | ((c2 as u32 & 0x3F) << 6) | (c3 as u32 & 0x3F);
                }
            } else {
                out[o] = BAD_INPUT;
                if p < e
                    && (c != 0xE0 || s[p] >= 0xA0)
                    && (c != 0xED || s[p] < 0xA0)
                    && (s[p] & 0xC0) == 0x80
                {
                    p += 1;
                    if p < e && (s[p] & 0xC0) == 0x80 {
                        p += 1;
                    }
                }
            }
        } else if c <= 0xF4 {
            // 4-byte character
            if e - p >= 3 {
                let c2 = s[p];
                let c3 = s[p + 1];
                let c4 = s[p + 2];
                p += 3;
                // 0xF0 followed by < 0x90 is overlong; 0xF4 followed by >= 0x90
                // is above U+10FFFF.
                if (c2 & 0xC0) != 0x80 || (c == 0xF0 && c2 < 0x90) || (c == 0xF4 && c2 >= 0x90) {
                    out[o] = BAD_INPUT;
                    p -= 3;
                } else if (c3 & 0xC0) != 0x80 {
                    out[o] = BAD_INPUT;
                    p -= 2;
                } else if (c4 & 0xC0) != 0x80 {
                    out[o] = BAD_INPUT;
                    p -= 1;
                } else {
                    out[o] = ((c as u32 & 0x7) << 18)
                        | ((c2 as u32 & 0x3F) << 12)
                        | ((c3 as u32 & 0x3F) << 6)
                        | (c4 as u32 & 0x3F);
                }
            } else {
                out[o] = BAD_INPUT;
                if p < e {
                    let c2 = s[p];
                    if (c == 0xF0 && c2 >= 0x90) || (c == 0xF4 && c2 < 0x90) || (0xF1..=0xF3).contains(&c) {
                        while p < e && (s[p] & 0xC0) == 0x80 {
                            p += 1;
                        }
                    }
                }
            }
        } else {
            out[o] = BAD_INPUT;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

/// `mb_wchar_to_utf8`.
pub fn mb_wchar_to_utf8(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < 0x80 {
            buf.add(w as u8);
        } else if w < 0x800 {
            buf.add2((((w >> 6) & 0x1F) | 0xC0) as u8, ((w & 0x3F) | 0x80) as u8);
        } else if (0xD800..=0xDFFF).contains(&w) && buf.error_mode != ILLEGAL_MODE_BADUTF8 {
            // Surrogates are not valid in UTF-8; BADUTF8 mode (internal, for
            // searching) encodes them to keep them distinct.
            mb_illegal_output(w, mb_wchar_to_utf8, buf);
        } else if w < 0x10000 {
            buf.add3(
                (((w >> 12) & 0xF) | 0xE0) as u8,
                (((w >> 6) & 0x3F) | 0x80) as u8,
                ((w & 0x3F) | 0x80) as u8,
            );
        } else if w < 0x110000 {
            buf.add4(
                (((w >> 18) & 0x7) | 0xF0) as u8,
                (((w >> 12) & 0x3F) | 0x80) as u8,
                (((w >> 6) & 0x3F) | 0x80) as u8,
                ((w & 0x3F) | 0x80) as u8,
            );
        } else {
            mb_illegal_output(w, mb_wchar_to_utf8, buf);
        }
    }
}

/// `mb_cut_utf8`: moves both ends back to character boundaries.
pub fn mb_cut_utf8(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    // Bytes 0x80-0xBF (signed < -64) are continuation bytes; the byte at
    // `s.len()` is the zend_string's NUL terminator.
    let is_cont = |i: usize| s.get(i).is_some_and(|&b| (b as i8) < -64);
    let mut start = from;
    while start > 0 && is_cont(start) {
        start -= 1;
    }
    let end = start.saturating_add(len);
    if end >= s.len() {
        return s[start..].to_vec();
    }
    let mut end = end;
    while end > start && is_cont(end) {
        end -= 1;
    }
    s[start..end].to_vec()
}

pub fn mb_utf8_docomo_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_docomo_to_wchar")
}

pub fn mb_wchar_to_utf8_docomo(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_docomo")
}

pub fn mb_utf8_kddi_a_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_kddi_a_to_wchar")
}

pub fn mb_wchar_to_utf8_kddi_a(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_kddi_a")
}

pub fn mb_utf8_kddi_b_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_kddi_b_to_wchar")
}

pub fn mb_wchar_to_utf8_kddi_b(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_kddi_b")
}

pub fn mb_utf8_sb_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_sb_to_wchar")
}

pub fn mb_wchar_to_utf8_sb(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_sb")
}
