//! `libmbfl/filters/mbfilter_utf8.c`: UTF-8 and the Japanese mobile
//! carriers' variants (DoCoMo, KDDI-A, KDDI-B, SoftBank), which map the
//! carriers' private-use emoji codepoints to and from standard Unicode
//! emoji (via their Shift-JIS (ku*94)+ten codes, as in
//! `mbfilter_sjis_emoji_*`).

use crate::mbstring::filters::cjk::{
    NFLAGS_CODE_KDDI, NFLAGS_CODE_SB, NFLAGS_S, mbfilter_sjis_emoji_docomo2unicode,
    mbfilter_sjis_emoji_kddi2unicode, mbfilter_sjis_emoji_sb2unicode, mbfl_bisec_srch2, nflags,
};
use crate::mbstring::filters::mblen;
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{
    BAD_INPUT, Encoding, FromWcharFn, ILLEGAL_MODE_BADUTF8, NoEncoding,
};
use crate::mbstring::tables::emoji2uni::*;

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
                    out[o] =
                        ((c as u32 & 0xF) << 12) | ((c2 as u32 & 0x3F) << 6) | (c3 as u32 & 0x3F);
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
                    if (c == 0xF0 && c2 >= 0x90)
                        || (c == 0xF4 && c2 < 0x90)
                        || (0xF1..=0xF3).contains(&c)
                    {
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

/// `mbfilter_conv_map_tbl`: `map` rows are `[from, to, base]`; maps `c` in
/// `from..=to` to `c - from + base`.
fn mbfilter_conv_map_tbl(c: i32, w: &mut i32, map: &[[u16; 3]]) -> bool {
    for m in map {
        if m[0] as i32 <= c && c <= m[1] as i32 {
            *w = c - m[0] as i32 + m[2] as i32;
            return true;
        }
    }
    false
}

/// `mbfilter_conv_r_map_tbl`: the reverse of [`mbfilter_conv_map_tbl`].
fn mbfilter_conv_r_map_tbl(c: i32, w: &mut i32, map: &[[u16; 3]]) -> bool {
    for m in map {
        if m[2] as i32 <= c && c <= m[2] as i32 - m[0] as i32 + m[1] as i32 {
            *w = c + m[0] as i32 - m[2] as i32;
            return true;
        }
    }
    false
}

/// `mbfl_docomo2uni_pua`: Docomo emoji (ku*94)+ten ranges and their PUA
/// codepoints.
static MBFL_DOCOMO2UNI_PUA: [[u16; 3]; 4] = [
    [0x28c2, 0x292f, 0xe63e],
    [0x2930, 0x2934, 0xe6ac],
    [0x2935, 0x2951, 0xe6b1],
    [0x2952, 0x29db, 0xe6ce],
];

/// `mbfl_kddi2uni_pua`.
static MBFL_KDDI2UNI_PUA: [[u16; 3]; 7] = [
    [0x26ec, 0x2838, 0xe468],
    [0x284c, 0x2863, 0xe5b5],
    [0x24b8, 0x24ca, 0xe5cd],
    [0x24cb, 0x2545, 0xea80],
    [0x2839, 0x284b, 0xeafb],
    [0x2546, 0x25c0, 0xeb0e],
    [0x25c1, 0x25c6, 0xeb89],
];

/// `mbfl_kddi2uni_pua_b`.
static MBFL_KDDI2UNI_PUA_B: [[u16; 3]; 8] = [
    [0x24b8, 0x24f6, 0xec40],
    [0x24f7, 0x2573, 0xec80],
    [0x2574, 0x25b2, 0xed40],
    [0x25b3, 0x25c6, 0xed80],
    [0x26ec, 0x272a, 0xef40],
    [0x272b, 0x27a7, 0xef80],
    [0x27a8, 0x27e6, 0xf040],
    [0x27e7, 0x2863, 0xf080],
];

/// `mbfl_sb2uni_pua`.
static MBFL_SB2UNI_PUA: [[u16; 3]; 6] = [
    [0x27a9, 0x2802, 0xe101],
    [0x2808, 0x2861, 0xe201],
    [0x2921, 0x297a, 0xe001],
    [0x2980, 0x29cc, 0xe301],
    [0x2a99, 0x2ae4, 0xe401],
    [0x2af8, 0x2b35, 0xe501],
];

/// `mb_mobile_utf8_to_wchar`: UTF-8 whose vendor PUA codepoints (emoji)
/// are converted to their standard Unicode equivalents.
fn mb_mobile_utf8_to_wchar(
    input: &mut &[u8],
    out: &mut [u32],
    emoji_map: &[[u16; 3]],
    convert_emoji: fn(i32, &mut i32) -> i32,
) -> usize {
    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    // An emoji can expand to two codepoints.
    let limit = out.len() - 1;

    while p < e && o < limit {
        let c = s[p];
        p += 1;
        let mut cp: u32;

        if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
            continue;
        } else if (0xC2..=0xDF).contains(&c) && p < e {
            let c2 = s[p];
            p += 1;

            if (c2 & 0xC0) == 0x80 {
                cp = ((c as u32 & 0x1F) << 6) | (c2 as u32 & 0x3F);
            } else {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 1;
                continue;
            }
        } else if (0xE0..=0xEF).contains(&c) {
            if e - p < 2 {
                out[o] = BAD_INPUT;
                o += 1;
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
                continue;
            }
            let c2 = s[p];
            let c3 = s[p + 1];
            p += 2;

            if (c2 & 0xC0) != 0x80 || (c == 0xE0 && c2 < 0xA0) || (c == 0xED && c2 >= 0xA0) {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 2;
                continue;
            } else if (c3 & 0xC0) != 0x80 {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 1;
                continue;
            } else {
                cp = ((c as u32 & 0xF) << 12) | ((c2 as u32 & 0x3F) << 6) | (c3 as u32 & 0x3F);
            }
        } else if (0xF0..=0xF4).contains(&c) {
            if e - p < 3 {
                out[o] = BAD_INPUT;
                o += 1;
                if p < e {
                    let c2 = s[p];
                    if (c == 0xF0 && c2 >= 0x90)
                        || (c == 0xF4 && c2 < 0x90)
                        || (0xF1..=0xF3).contains(&c)
                    {
                        while p < e && (s[p] & 0xC0) == 0x80 {
                            p += 1;
                        }
                    }
                }
                continue;
            }
            let c2 = s[p];
            let c3 = s[p + 1];
            let c4 = s[p + 2];
            p += 3;

            if (c2 & 0xC0) != 0x80 || (c == 0xF0 && c2 < 0x90) || (c == 0xF4 && c2 >= 0x90) {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 3;
                continue;
            } else if (c3 & 0xC0) != 0x80 {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 2;
                continue;
            } else if (c4 & 0xC0) != 0x80 {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 1;
                continue;
            } else {
                cp = ((c as u32 & 0x7) << 18)
                    | ((c2 as u32 & 0x3F) << 12)
                    | ((c3 as u32 & 0x3F) << 6)
                    | (c4 as u32 & 0x3F);
            }
        } else {
            out[o] = BAD_INPUT;
            o += 1;
            continue;
        }

        let mut s1 = 0i32;
        let mut snd = 0i32;
        if mbfilter_conv_r_map_tbl(cp as i32, &mut s1, emoji_map) {
            cp = convert_emoji(s1, &mut snd) as u32;
            if snd != 0 {
                out[o] = snd as u32;
                o += 1;
            }
        }
        out[o] = cp;
        o += 1;
    }

    *input = &s[p..];
    o
}

/// Appends `w` (below U+110000, surrogates included) as UTF-8.
#[inline]
fn add_utf8(buf: &mut ConvertBuf, w: u32) {
    if w <= 0x7F {
        buf.add(w as u8);
    } else if w <= 0x7FF {
        buf.add2((((w >> 6) & 0x1F) | 0xC0) as u8, ((w & 0x3F) | 0x80) as u8);
    } else if w <= 0xFFFF {
        buf.add3(
            (((w >> 12) & 0xF) | 0xE0) as u8,
            (((w >> 6) & 0x3F) | 0x80) as u8,
            ((w & 0x3F) | 0x80) as u8,
        );
    } else {
        buf.add4(
            (((w >> 18) & 0x7) | 0xF0) as u8,
            (((w >> 12) & 0x3F) | 0x80) as u8,
            (((w >> 6) & 0x3F) | 0x80) as u8,
            ((w & 0x3F) | 0x80) as u8,
        );
    }
}

/// `mbfl_bisec_srch2` lookup of `w` in a sorted key table; the paired value
/// or 0.
#[inline]
fn emoji_lookup(w: u32, keys: &[u16], len: u32, values: &[u16]) -> u32 {
    let i = mbfl_bisec_srch2(w as i32, keys, len as usize);
    if i >= 0 { values[i as usize] as u32 } else { 0 }
}

#[inline]
fn is_keycap_base(w: u32) -> bool {
    w == b'#' as u32 || (b'0' as u32..=b'9' as u32).contains(&w)
}

/// For a Regional Indicator pair (`w`, `w2`) forming one of the flags in
/// `NFLAGS_S`, the vendor (ku*94)+ten code from `codes`.
fn flag_code(w: u32, w2: u32, codes: &[i32; 10]) -> Option<u32> {
    if w2 >= nflags(b'B') as u32 && w2 <= nflags(b'U') as u32 {
        // B for GB, U for RU
        for (i, flag) in NFLAGS_S.iter().enumerate() {
            if w == nflags(flag[0]) as u32 && w2 == nflags(flag[1]) as u32 {
                return Some(codes[i] as u32);
            }
        }
    }
    None
}

pub fn mb_utf8_docomo_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    mb_mobile_utf8_to_wchar(
        input,
        out,
        &MBFL_DOCOMO2UNI_PUA,
        mbfilter_sjis_emoji_docomo2unicode,
    )
}

pub fn mb_wchar_to_utf8_docomo(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    let mut i = 0usize;
    while i < input.len() {
        let mut w = input[i];
        i += 1;
        let mut s: u32 = 0;
        let mut c1: i32 = 0;

        if w < 0x110000 {
            if is_keycap_base(w) && i < input.len() {
                let w2 = input[i];
                i += 1;

                if w2 == 0x20E3 {
                    s = if w == b'#' as u32 {
                        0x2964
                    } else if w == b'0' as u32 {
                        0x296F
                    } else {
                        0x2966 + (w - b'1' as u32)
                    };
                } else {
                    i -= 1;
                }
            } else if w == 0xA9 {
                // Copyright sign
                s = 0x29B5;
            } else if w == 0xAE {
                // Registered sign
                s = 0x29BA;
            } else if (MB_TBL_UNI_DOCOMO2CODE2_MIN..=MB_TBL_UNI_DOCOMO2CODE2_MAX).contains(&w) {
                s = emoji_lookup(
                    w,
                    &MB_TBL_UNI_DOCOMO2CODE2_KEY,
                    MB_TBL_UNI_DOCOMO2CODE2_LEN,
                    &MB_TBL_UNI_DOCOMO2CODE2_VALUE,
                );
            } else if (MB_TBL_UNI_DOCOMO2CODE3_MIN..=MB_TBL_UNI_DOCOMO2CODE3_MAX).contains(&w) {
                s = emoji_lookup(
                    w - 0x10000,
                    &MB_TBL_UNI_DOCOMO2CODE3_KEY,
                    MB_TBL_UNI_DOCOMO2CODE3_LEN,
                    &MB_TBL_UNI_DOCOMO2CODE3_VALUE,
                );
            } else if (MB_TBL_UNI_DOCOMO2CODE5_MIN..=MB_TBL_UNI_DOCOMO2CODE5_MAX).contains(&w) {
                s = emoji_lookup(
                    w - 0xF0000,
                    &MB_TBL_UNI_DOCOMO2CODE5_KEY,
                    MB_TBL_UNI_DOCOMO2CODE5_LEN,
                    &MB_TBL_UNI_DOCOMO2CODE5_VAL,
                );
            }

            if s != 0 && mbfilter_conv_map_tbl(s as i32, &mut c1, &MBFL_DOCOMO2UNI_PUA) {
                w = c1 as u32;
            }

            add_utf8(buf, w);
        } else {
            mb_illegal_output(w, mb_wchar_to_utf8_docomo, buf);
        }
    }
}

pub fn mb_utf8_kddi_a_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    mb_mobile_utf8_to_wchar(
        input,
        out,
        &MBFL_KDDI2UNI_PUA,
        mbfilter_sjis_emoji_kddi2unicode,
    )
}

/// `mb_wchar_to_utf8_kddi`: shared by KDDI-A and KDDI-B, which differ in
/// their PUA mapping.
fn mb_wchar_to_utf8_kddi(
    input: &[u32],
    buf: &mut ConvertBuf,
    emoji_map: &[[u16; 3]],
    error_handler: FromWcharFn,
) {
    let mut i = 0usize;
    while i < input.len() {
        let mut w = input[i];
        i += 1;
        let mut s: u32 = 0;
        let mut c1: i32 = 0;

        if w < 0x110000 {
            if is_keycap_base(w) && i < input.len() {
                let w2 = input[i];
                i += 1;

                if w2 == 0x20E3 {
                    s = if w == b'#' as u32 {
                        0x25BC
                    } else if w == b'0' as u32 {
                        0x2830
                    } else {
                        0x27A6 + (w - b'1' as u32)
                    };
                } else {
                    i -= 1;
                }
            } else if w >= nflags(b'C') as u32 && w <= nflags(b'U') as u32 {
                // C for CN, U for US
                let mut flag = None;
                if i < input.len() {
                    let w2 = input[i];
                    i += 1;
                    flag = flag_code(w, w2, &NFLAGS_CODE_KDDI);
                    if flag.is_none() {
                        i -= 1;
                    }
                }
                match flag {
                    // `goto process_kuten`
                    Some(code) => s = code,
                    None => {
                        mb_illegal_output(w, error_handler, buf);
                        continue;
                    }
                }
            } else if w == 0xA9 {
                // Copyright sign
                s = 0x27DC;
            } else if w == 0xAE {
                // Registered sign
                s = 0x27DD;
            } else if (MB_TBL_UNI_KDDI2CODE2_MIN..=MB_TBL_UNI_KDDI2CODE2_MAX).contains(&w) {
                s = emoji_lookup(
                    w,
                    &MB_TBL_UNI_KDDI2CODE2_KEY,
                    MB_TBL_UNI_KDDI2CODE2_LEN,
                    &MB_TBL_UNI_KDDI2CODE2_VALUE,
                );
            } else if (MB_TBL_UNI_KDDI2CODE3_MIN..=MB_TBL_UNI_KDDI2CODE3_MAX).contains(&w) {
                s = emoji_lookup(
                    w - 0x10000,
                    &MB_TBL_UNI_KDDI2CODE3_KEY,
                    MB_TBL_UNI_KDDI2CODE3_LEN,
                    &MB_TBL_UNI_KDDI2CODE3_VALUE,
                );
            } else if (MB_TBL_UNI_KDDI2CODE5_MIN..=MB_TBL_UNI_KDDI2CODE5_MAX).contains(&w) {
                s = emoji_lookup(
                    w - 0xF0000,
                    &MB_TBL_UNI_KDDI2CODE5_KEY,
                    MB_TBL_UNI_KDDI2CODE5_LEN,
                    &MB_TBL_UNI_KDDI2CODE5_VAL,
                );
            }

            // process_kuten:
            if s != 0 && mbfilter_conv_map_tbl(s as i32, &mut c1, emoji_map) {
                w = c1 as u32;
            }

            add_utf8(buf, w);
        } else {
            mb_illegal_output(w, error_handler, buf);
        }
    }
}

pub fn mb_wchar_to_utf8_kddi_a(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    mb_wchar_to_utf8_kddi(input, buf, &MBFL_KDDI2UNI_PUA, mb_wchar_to_utf8_kddi_a);
}

pub fn mb_utf8_kddi_b_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    mb_mobile_utf8_to_wchar(
        input,
        out,
        &MBFL_KDDI2UNI_PUA_B,
        mbfilter_sjis_emoji_kddi2unicode,
    )
}

pub fn mb_wchar_to_utf8_kddi_b(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    mb_wchar_to_utf8_kddi(input, buf, &MBFL_KDDI2UNI_PUA_B, mb_wchar_to_utf8_kddi_b);
}

pub fn mb_utf8_sb_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    mb_mobile_utf8_to_wchar(input, out, &MBFL_SB2UNI_PUA, mbfilter_sjis_emoji_sb2unicode)
}

pub fn mb_wchar_to_utf8_sb(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    let mut i = 0usize;
    while i < input.len() {
        let mut w = input[i];
        i += 1;
        let mut s: u32 = 0;
        let mut c1: i32 = 0;

        if w < 0x110000 {
            if is_keycap_base(w) && i < input.len() {
                let w2 = input[i];
                i += 1;

                if w2 == 0x20E3 {
                    s = if w == b'#' as u32 {
                        0x2817
                    } else if w == b'0' as u32 {
                        0x282C
                    } else {
                        0x2823 + (w - b'1' as u32)
                    };
                } else {
                    i -= 1;
                }
            } else if w >= nflags(b'C') as u32 && w <= nflags(b'U') as u32 {
                // C for CN, U for US
                let mut flag = None;
                if i < input.len() {
                    let w2 = input[i];
                    i += 1;
                    flag = flag_code(w, w2, &NFLAGS_CODE_SB);
                    if flag.is_none() {
                        i -= 1;
                    }
                }
                match flag {
                    // `goto process_kuten`
                    Some(code) => s = code,
                    None => {
                        mb_illegal_output(w, mb_wchar_to_utf8_sb, buf);
                        continue;
                    }
                }
            } else if w == 0xA9 {
                // Copyright sign
                s = 0x2855;
            } else if w == 0xAE {
                // Registered sign
                s = 0x2856;
            } else if (MB_TBL_UNI_SB2CODE2_MIN..=MB_TBL_UNI_SB2CODE2_MAX).contains(&w) {
                s = emoji_lookup(
                    w,
                    &MB_TBL_UNI_SB2CODE2_KEY,
                    MB_TBL_UNI_SB2CODE2_LEN,
                    &MB_TBL_UNI_SB2CODE2_VALUE,
                );
            } else if (MB_TBL_UNI_SB2CODE3_MIN..=MB_TBL_UNI_SB2CODE3_MAX).contains(&w) {
                s = emoji_lookup(
                    w - 0x10000,
                    &MB_TBL_UNI_SB2CODE3_KEY,
                    MB_TBL_UNI_SB2CODE3_LEN,
                    &MB_TBL_UNI_SB2CODE3_VALUE,
                );
            } else if (MB_TBL_UNI_SB2CODE5_MIN..=MB_TBL_UNI_SB2CODE5_MAX).contains(&w) {
                s = emoji_lookup(
                    w - 0xF0000,
                    &MB_TBL_UNI_SB2CODE5_KEY,
                    MB_TBL_UNI_SB2CODE5_LEN,
                    &MB_TBL_UNI_SB2CODE5_VAL,
                );
            }

            // process_kuten:
            if s != 0 && mbfilter_conv_map_tbl(s as i32, &mut c1, &MBFL_SB2UNI_PUA) {
                w = c1 as u32;
            }

            add_utf8(buf, w);
        } else {
            mb_illegal_output(w, mb_wchar_to_utf8_sb, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::mbfl::convert::{decode_all, encode_all};
    use crate::mbstring::mbfl::encoding::ILLEGAL_MODE_CHAR;
    use std::collections::{BTreeMap, BTreeSet};

    const P: u32 = b'%' as u32;

    /// `readUTF8ConversionTable`: mobile UTF-8 bytes to codepoint, codepoint to
    /// bytes, and the codepoints marked BAD.
    struct Table {
        to_unicode: BTreeMap<Vec<u8>, u32>,
        from_unicode: BTreeMap<u32, Vec<u8>>,
        invalid: BTreeSet<u32>,
    }

    /// `intToString`.
    fn int_to_string(v: u32) -> Vec<u8> {
        if v <= 0xFF {
            vec![v as u8]
        } else if v <= 0xFFFF {
            (v as u16).to_be_bytes().to_vec()
        } else if v <= 0xFF_FFFF {
            v.to_be_bytes()[1..].to_vec()
        } else {
            v.to_be_bytes().to_vec()
        }
    }

    fn read_table(name: &str) -> Table {
        let path = format!(
            "{}/../../tests/phpt/ext/mbstring/tests/data/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(path).expect("data file");
        let mut t = Table {
            to_unicode: BTreeMap::new(),
            from_unicode: BTreeMap::new(),
            invalid: BTreeSet::new(),
        };
        for line in text.lines() {
            let mut cols = line.split('\t');
            let (Some(cp), Some(ch)) = (cols.next(), cols.next()) else {
                continue;
            };
            let Some(cp) = cp
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
            else {
                continue;
            };
            if let Some(ch) = ch
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
            {
                let ch = int_to_string(ch);
                t.to_unicode.insert(ch.clone(), cp);
                t.from_unicode.insert(cp, ch);
            } else if ch.starts_with("BAD") {
                t.invalid.insert(cp);
            }
        }
        t
    }

    fn encode(cps: &[u32], e: &'static Encoding) -> (Vec<u8>, u32) {
        let buf = encode_all(cps, e, P, ILLEGAL_MODE_CHAR);
        (buf.out, buf.errors)
    }

    /// Decoded codepoints with error markers shown as '%'.
    fn decode(s: &[u8], e: &'static Encoding) -> Vec<u32> {
        decode_all(s, e)
            .into_iter()
            .map(|c| if c == BAD_INPUT { P } else { c })
            .collect()
    }

    /// `$badUTF8` of utf8_mobile_encodings.phpt.
    fn bad_utf8() -> Vec<(&'static [u8], Vec<u32>)> {
        let a = b'A' as u32;
        vec![
            // Codepoints outside of valid 0-0x10FFFF range for Unicode
            (b"\xF4\x90\x80\x80", vec![P; 4]),
            (b"\xF7\x80\x80\x80", vec![P; 4]),
            (b"\xF7\xBF\xBF\xBF", vec![P; 4]),
            // Reserved range for UTF-16 surrogate pairs
            (b"\xED\xA0\x80", vec![P; 3]),
            (b"\xED\xAF\xBF", vec![P; 3]),
            (b"\xED\xBF\xBF", vec![P; 3]),
            // Truncated characters
            (b"\xDF", vec![P]),
            (b"\xEF\xBF", vec![P]),
            (b"\xF0\xBF\xBF", vec![P]),
            (b"\xF1\x96", vec![P]),
            (b"\xF1\x96\x80", vec![P]),
            (b"\xF2\x94", vec![P]),
            (b"\xF2\x94\x80", vec![P]),
            (b"\xF3\x94", vec![P]),
            (b"\xF3\x94\x80", vec![P]),
            (b"\xE0\x9F", vec![P, P]),
            (b"\xED\xA6", vec![P, P]),
            // Multi-byte characters which end too soon and go to ASCII
            (b"\xDFA", vec![P, a]),
            (b"\xEF\xBFA", vec![P, a]),
            (b"\xF0\xBFA", vec![P, a]),
            (b"\xF0\xBF\xBFA", vec![P, a]),
            // Multi-byte characters which end too soon and go to another MB char
            (b"\xDF\xDF\xBF", vec![P, 0x7FF]),
            (b"\xEF\xBF\xDF\xBF", vec![P, 0x7FF]),
            (b"\xF0\xBF\xBF\xDF\xBF", vec![P, 0x7FF]),
            // Continuation bytes which appear outside of a MB char
            (b"\x80", vec![P]),
            (b"A\x80", vec![a, P]),
            (b"\xDF\xBF\x80", vec![0x7FF, P]),
            // Overlong code units
            (b"\xC1\xBF", vec![P; 2]),
            (b"\xE0\x9F\xBF", vec![P; 3]),
            (b"\xF0\x8F\xBF\xBF", vec![P; 4]),
        ]
    }

    /// `testUTF8Variant` of utf8_mobile_encodings.phpt.
    fn test_utf8_variant(e: &'static Encoding, file: &str) {
        let mut t = read_table(file);

        // Test some plain, vanilla codepoints (to/from mobile encoding)
        assert_eq!(encode(&[0], e), (vec![0], 0));
        assert_eq!(decode_all(b"\x00", e), [0]);

        // Codepoints which are not in the table convert as in UTF-8 (php-src
        // tries 1000 random ones; surrogates are rejected by its UTF-32BE input)
        let mut mobile = ConvertBuf::new(8, P, ILLEGAL_MODE_CHAR);
        let mut plain = ConvertBuf::new(8, P, ILLEGAL_MODE_CHAR);
        for cp in 1..0x11_0000u32 {
            if (0xD800..0xE000).contains(&cp)
                || t.from_unicode.contains_key(&cp)
                || t.invalid.contains(&cp)
            {
                continue;
            }
            mobile.out.clear();
            plain.out.clear();
            (e.from_wchar)(&[cp], &mut mobile, true);
            mb_wchar_to_utf8(&[cp], &mut plain, true);
            assert_eq!(mobile.out, plain.out, "U+{cp:X} in {}", e.name);
        }
        assert_eq!(mobile.errors, 0);

        if e.no == NoEncoding::Utf8Docomo {
            // Docomo's U+FEE26-U+FEE28 and U+FEE2C convert to emoji which convert back
            // to other codepoints, so these mappings are not reversible
            for ch in [
                b"\xEE\x9B\x80",
                b"\xEE\x9B\x81",
                b"\xEE\x9B\x82",
                b"\xEE\x9B\x86",
            ] {
                t.to_unicode.remove(&ch[..]);
            }
        }

        // Test all characters which are different in mobile encoding (from standard UTF-8)
        for (ch, &cp) in &t.to_unicode {
            assert_eq!(decode_all(ch, e), [cp], "{ch:X?} in {}", e.name);
        }
        for (&cp, ch) in &t.from_unicode {
            assert_eq!(encode(&[cp], e), (ch.clone(), 0), "U+{cp:X} in {}", e.name);
        }
        for &cp in &t.invalid {
            assert_eq!(
                encode(&[cp], e),
                (b"%".to_vec(), 1),
                "U+{cp:X} in {}",
                e.name
            );
        }

        // Try malformed UTF-8 sequences
        for (bad, expected) in bad_utf8() {
            assert!(decode_all(bad, e).contains(&BAD_INPUT));
            assert_eq!(decode(bad, e), expected, "{bad:X?} in {}", e.name);
        }
    }

    #[test]
    fn utf8_docomo() {
        test_utf8_variant(&MBFL_ENCODING_UTF8_DOCOMO, "UTF-8-DOCOMO.txt");
    }

    #[test]
    fn utf8_kddi_a() {
        test_utf8_variant(&MBFL_ENCODING_UTF8_KDDI_A, "UTF-8-KDDI-A.txt");
    }

    #[test]
    fn utf8_kddi_b() {
        test_utf8_variant(&MBFL_ENCODING_UTF8_KDDI_B, "UTF-8-KDDI-B.txt");
    }

    #[test]
    fn utf8_softbank() {
        test_utf8_variant(&MBFL_ENCODING_UTF8_SB, "UTF-8-SOFTBANK.txt");
    }

    /// Keycap and flag emoji are two codepoints but one vendor character.
    #[test]
    fn keycaps_and_flags() {
        let jp = [nflags(b'J') as u32, nflags(b'P') as u32];
        for e in [
            &MBFL_ENCODING_UTF8_DOCOMO,
            &MBFL_ENCODING_UTF8_KDDI_A,
            &MBFL_ENCODING_UTF8_KDDI_B,
            &MBFL_ENCODING_UTF8_SB,
        ] {
            for keycap in [[0x23, 0x20E3], [0x30, 0x20E3], [0x35, 0x20E3]] {
                let (bytes, errors) = encode(&keycap, e);
                assert_eq!((bytes.len(), errors), (3, 0), "{keycap:X?} in {}", e.name);
                assert_eq!(decode_all(&bytes, e), keycap, "{keycap:X?} in {}", e.name);
            }
            // A digit which is not followed by U+20E3 is just a digit
            assert_eq!(encode(&[0x31, 0x32], e), (b"12".to_vec(), 0));
        }
        for e in [
            &MBFL_ENCODING_UTF8_KDDI_A,
            &MBFL_ENCODING_UTF8_KDDI_B,
            &MBFL_ENCODING_UTF8_SB,
        ] {
            let (bytes, errors) = encode(&jp, e);
            assert_eq!((bytes.len(), errors), (3, 0), "{}", e.name);
            assert_eq!(decode_all(&bytes, e), jp, "{}", e.name);
            // A Regional Indicator which does not start a known flag is an error
            assert_eq!(encode(&[jp[0], 0x41], e), (b"%A".to_vec(), 1));
            assert_eq!(encode(&[jp[0]], e), (b"%".to_vec(), 1));
        }
        // DoCoMo has no flags; Regional Indicators pass through
        let (bytes, errors) = encode(&jp, &MBFL_ENCODING_UTF8_DOCOMO);
        assert_eq!(
            (bytes, errors),
            ("\u{1F1EF}\u{1F1F5}".as_bytes().to_vec(), 0)
        );
    }

    /// The decoders reserve one slot for the second codepoint of an emoji.
    #[test]
    fn small_output_buffer() {
        let e = &MBFL_ENCODING_UTF8_KDDI_A;
        let mut input = encode(
            &[
                0x41,
                0x23,
                0x20E3,
                0x3042,
                nflags(b'J') as u32,
                nflags(b'P') as u32,
            ],
            e,
        )
        .0;
        input.extend_from_slice(b"\xE3\x81");
        let mut all = Vec::new();
        let mut buf = [0u32; 2];
        let mut state = 0;
        let mut inp = &input[..];
        while !inp.is_empty() {
            let n = (e.to_wchar)(&mut inp, &mut buf, &mut state);
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all, decode_all(&input, e));
        assert_eq!(
            all,
            [
                0x41,
                0x23,
                0x20E3,
                0x3042,
                nflags(b'J') as u32,
                nflags(b'P') as u32,
                BAD_INPUT
            ]
        );
    }
}
