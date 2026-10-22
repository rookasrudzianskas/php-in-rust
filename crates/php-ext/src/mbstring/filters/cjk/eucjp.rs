//! EUC-JP variants of `libmbfl/filters/mbfilter_cjk.c`: EUC-JP (JIS X 0208
//! and JIS X 0212), eucJP-win (with the CP932 extensions), CP51932 and
//! EUC-JP-2004 (JIS X 0213).

use super::mbfl_bisec_srch2;
use super::sjis::{cp932_row1_special, ucs_jis_lookup, ucs_to_jisx0213};
use crate::mbstring::filters::mblen;
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::tables::cp932_table::*;
use crate::mbstring::tables::unicode_table_cp932_ext::*;
use crate::mbstring::tables::unicode_table_jis::*;
use crate::mbstring::tables::unicode_table_jis2004::*;

pub static MBFL_ENCODING_EUC_JP: Encoding = Encoding {
    no: NoEncoding::EucJp,
    name: "EUC-JP",
    mime_name: Some("EUC-JP"),
    aliases: &["EUC", "EUC_JP", "eucJP", "x-euc-jp"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_eucjp_to_wchar,
    from_wchar: mb_wchar_to_eucjp,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_EUCJP2004: Encoding = Encoding {
    no: NoEncoding::Eucjp2004,
    name: "EUC-JP-2004",
    mime_name: Some("EUC-JP"),
    aliases: &["EUC_JP-2004"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_eucjp2004_to_wchar,
    from_wchar: mb_wchar_to_eucjp2004,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_EUCJP_WIN: Encoding = Encoding {
    no: NoEncoding::EucjpWin,
    name: "eucJP-win",
    mime_name: Some("EUC-JP"),
    aliases: &["eucJP-open", "eucJP-ms"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_eucjpwin_to_wchar,
    from_wchar: mb_wchar_to_eucjpwin,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP51932: Encoding = Encoding {
    no: NoEncoding::Cp51932,
    name: "CP51932",
    mime_name: Some("CP51932"),
    aliases: &["cp51932"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp51932_to_wchar,
    from_wchar: mb_wchar_to_cp51932,
    check: None,
    cut: None,
};

/// Emits a JIS code the EUC-JP way: ASCII as is, JIS X 0201 kana after SS2,
/// JIS X 0208 with the high bits set, and JIS X 0212 (codes from 0x8080)
/// after SS3.
#[inline]
fn add_eucjp(buf: &mut ConvertBuf, s: u32) {
    if s < 0x80 {
        buf.add(s as u8);
    } else if s < 0x100 {
        buf.add2(0x8E, s as u8);
    } else if s < 0x8080 {
        buf.add2((((s >> 8) & 0xFF) | 0x80) as u8, ((s & 0xFF) | 0x80) as u8);
    } else {
        buf.add3(0x8F, (((s >> 8) & 0xFF) | 0x80) as u8, ((s & 0xFF) | 0x80) as u8);
    }
}

/*
 * EUC-JP
 */

pub fn mb_eucjp_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if (0xA1..=0xFE).contains(&c) && p < e {
            // JIS X 0208
            let c2 = s[p];
            p += 1;
            if (0xA1..=0xFE).contains(&c2) {
                let k = (c as u32 - 0xA1) * 94 + c2 as u32 - 0xA1;
                if k < JISX0208_UCS_TABLE_SIZE {
                    let w = JISX0208_UCS_TABLE[k as usize] as u32;
                    out[o] = if w == 0 { BAD_INPUT } else { w };
                } else {
                    out[o] = BAD_INPUT;
                }
            } else {
                out[o] = BAD_INPUT;
            }
        } else if c == 0x8E && p < e {
            // Kana
            let c2 = s[p];
            p += 1;
            out[o] = if (0xA1..=0xDF).contains(&c2) { 0xFEC0 + c2 as u32 } else { BAD_INPUT };
        } else if c == 0x8F {
            // JIS X 0212
            if e - p >= 2 {
                let c2 = s[p];
                let c3 = s[p + 1];
                p += 2;
                if (0xA1..=0xFE).contains(&c3) && (0xA1..=0xFE).contains(&c2) {
                    let k = (c2 as u32 - 0xA1) * 94 + c3 as u32 - 0xA1;
                    if k < JISX0212_UCS_TABLE_SIZE {
                        let w = JISX0212_UCS_TABLE[k as usize] as u32;
                        out[o] = if w == 0 { BAD_INPUT } else { w };
                    } else {
                        out[o] = BAD_INPUT;
                    }
                } else {
                    out[o] = BAD_INPUT;
                }
            } else {
                out[o] = BAD_INPUT;
                p = e; // Jump to end of string
            }
        } else {
            out[o] = BAD_INPUT;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_eucjp(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s = if w == 0xAF {
            // U+00AF is MACRON; use JIS X 0212 overline
            0xA2B4
        } else {
            ucs_jis_lookup(w).unwrap_or(0)
        };

        if s == 0 {
            s = match w {
                0xFF3C => 0x2140, // FULLWIDTH REVERSE SOLIDUS
                0x2225 => 0x2142, // PARALLEL TO
                0xFF0D => 0x215D, // FULLWIDTH HYPHEN-MINUS
                0xFFE0 => 0x2171, // FULLWIDTH CENT SIGN
                0xFFE1 => 0x2172, // FULLWIDTH POUND SIGN
                0xFFE2 => 0x224C, // FULLWIDTH NOT SIGN
                0 => {
                    buf.add(0);
                    continue;
                }
                _ => {
                    mb_illegal_output(w, mb_wchar_to_eucjp, buf);
                    continue;
                }
            };
        }

        add_eucjp(buf, s);
    }
}

/*
 * eucJP-win
 */

pub fn mb_eucjpwin_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if (0xA1..=0xFE).contains(&c) && p < e {
            let c2 = s[p];
            p += 1;

            if (0xA1..=0xFE).contains(&c2) {
                let k = (c as u32 - 0xA1) * 94 + c2 as u32 - 0xA1;
                let mut w = 0;

                if k <= 137 {
                    w = cp932_row1_special(k);
                }

                if w == 0 {
                    if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&k) {
                        w = CP932EXT1_UCS_TABLE[(k - CP932EXT1_UCS_TABLE_MIN) as usize] as u32;
                    } else if k < JISX0208_UCS_TABLE_SIZE {
                        w = JISX0208_UCS_TABLE[k as usize] as u32;
                    } else if k >= 84 * 94 {
                        w = k - (84 * 94) + 0xE000;
                    }
                }

                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
        } else if c == 0x8E && p < e {
            let c2 = s[p];
            p += 1;
            out[o] = if (0xA1..=0xDF).contains(&c2) { 0xFEC0 + c2 as u32 } else { BAD_INPUT };
        } else if c == 0x8F && p < e {
            let c2 = s[p];
            p += 1;
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }
            let c3 = s[p];
            p += 1;

            if (0xA1..=0xFE).contains(&c2) && (0xA1..=0xFE).contains(&c3) {
                let k = (c2 as u32 - 0xA1) * 94 + c3 as u32 - 0xA1;
                let mut w = 0;

                if k < JISX0212_UCS_TABLE_SIZE {
                    w = JISX0212_UCS_TABLE[k as usize] as u32;
                    if w == 0x7E {
                        w = 0xFF5E; // FULLWIDTH TILDE
                    }
                } else if ((82 * 94)..(84 * 94)).contains(&k) {
                    let code = ((c2 as u32) << 8) | c3 as u32;
                    let ext3 = &CP932EXT3_EUCJP_TABLE[..CP932EXT3_EUCJP_TABLE_SIZE as usize];
                    if let Some(i) = ext3.iter().position(|&x| x as u32 == code) {
                        w = CP932EXT3_UCS_TABLE[i] as u32;
                    }
                } else if k >= 84 * 94 {
                    w = k - (84 * 94) + 0xE000 + (94 * 10);
                }

                if w == 0xA6 {
                    w = 0xFFE4; // FULLWIDTH BROKEN BAR
                }

                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
        } else {
            out[o] = BAD_INPUT;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_eucjpwin(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s = if w == 0 {
            buf.add(0);
            continue;
        } else if w == 0xAF {
            // U+00AF is MACRON; use JIS X 0212 overline
            0xA2B4
        } else if w == 0x203E {
            0x7E
        } else if let Some(s) = ucs_jis_lookup(w) {
            s
        } else if (0xE000..0xE000 + 10 * 94).contains(&w) {
            let s = w - 0xE000;
            ((s / 94 + 0x75) << 8) + (s % 94) + 0x21
        } else if (0xE000 + 10 * 94..0xE000 + 20 * 94).contains(&w) {
            let s = w - (0xE000 + 10 * 94);
            ((s / 94 + 0xF5) << 8) + (s % 94) + 0xA1
        } else {
            0
        };

        if s == 0xA2F1 {
            s = 0x2D62; // NUMERO SIGN
        }

        if s == 0 {
            s = match w {
                0xA5 => 0x5C,     // YEN SIGN
                0x2014 => 0x213D, // EM DASH
                0xFF3C => 0x2140, // FULLWIDTH REVERSE SOLIDUS
                0x2225 => 0x2142, // PARALLEL TO
                0xFF0D => 0x215D, // FULLWIDTH HYPHEN-MINUS
                0xFFE0 => 0x2171, // FULLWIDTH CENT SIGN
                0xFFE1 => 0x2172, // FULLWIDTH POUND SIGN
                0xFFE2 => 0x224C, // FULLWIDTH NOT SIGN
                _ => {
                    let ext1 = &CP932EXT1_UCS_TABLE
                        [..(CP932EXT1_UCS_TABLE_MAX - CP932EXT1_UCS_TABLE_MIN) as usize];
                    let ext3 = &CP932EXT3_UCS_TABLE
                        [..(CP932EXT3_UCS_TABLE_MAX - CP932EXT3_UCS_TABLE_MIN) as usize];
                    if let Some(i) = ext1.iter().position(|&x| x as u32 == w) {
                        let i = i as u32;
                        (((i / 94) + (CP932EXT1_UCS_TABLE_MIN / 94) + 0x21) << 8) + (i % 94) + 0x21
                    } else if let Some(i) = ext3.iter().position(|&x| x as u32 == w) {
                        CP932EXT3_EUCJP_TABLE[i] as u32
                    } else {
                        0
                    }
                }
            };
        }

        if s == 0 {
            mb_illegal_output(w, mb_wchar_to_eucjpwin, buf);
        } else {
            add_eucjp(buf, s);
        }
    }
}

/*
 * CP51932
 */

pub fn mb_cp51932_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if (0xA1..=0xFE).contains(&c) && p < e {
            let c2 = s[p];
            p += 1;
            if (0xA1..=0xFE).contains(&c2) {
                let k = (c as u32 - 0xA1) * 94 + c2 as u32 - 0xA1;
                let mut w = 0;

                if k <= 137 {
                    w = cp932_row1_special(k);
                }

                if w == 0 {
                    if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&k) {
                        w = CP932EXT1_UCS_TABLE[(k - CP932EXT1_UCS_TABLE_MIN) as usize] as u32;
                    } else if k < JISX0208_UCS_TABLE_SIZE {
                        w = JISX0208_UCS_TABLE[k as usize] as u32;
                    } else if (CP932EXT2_UCS_TABLE_MIN..CP932EXT2_UCS_TABLE_MAX).contains(&k) {
                        w = CP932EXT2_UCS_TABLE[(k - CP932EXT2_UCS_TABLE_MIN) as usize] as u32;
                    }
                }

                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
        } else if c == 0x8E && p < e {
            let c2 = s[p];
            p += 1;
            out[o] = if (0xA1..=0xDF).contains(&c2) { 0xFEC0 + c2 as u32 } else { BAD_INPUT };
        } else {
            out[o] = BAD_INPUT;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_cp51932(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w == 0 {
            buf.add(0);
            continue;
        }
        let mut s = ucs_jis_lookup(w).unwrap_or(0);

        if s >= 0x8080 {
            s = 0; // We don't support JIS X 0212
        }

        if s == 0 {
            s = match w {
                0xA5 => 0x216F,   // YEN SIGN -> FULLWIDTH YEN SIGN
                0xFF3C => 0x2140, // FULLWIDTH REVERSE SOLIDUS
                0x2225 => 0x2142, // PARALLEL TO
                0xFF0D => 0x215D, // FULLWIDTH HYPHEN-MINUS
                0xFFE0 => 0x2171, // FULLWIDTH CENT SIGN
                0xFFE1 => 0x2172, // FULLWIDTH POUND SIGN
                0xFFE2 => 0x224C, // FULLWIDTH NOT SIGN
                _ => {
                    let ext1 = &CP932EXT1_UCS_TABLE
                        [..(CP932EXT1_UCS_TABLE_MAX - CP932EXT1_UCS_TABLE_MIN) as usize];
                    let ext2 = &CP932EXT2_UCS_TABLE
                        [..(CP932EXT2_UCS_TABLE_MAX - CP932EXT2_UCS_TABLE_MIN) as usize];
                    if let Some(i) = ext1.iter().position(|&x| x as u32 == w) {
                        let i = i as u32;
                        ((i / 94 + 0x2D) << 8) + (i % 94) + 0x21
                    } else if let Some(i) = ext2.iter().position(|&x| x as u32 == w) {
                        let i = i as u32;
                        ((i / 94 + 0x79) << 8) + (i % 94) + 0x21
                    } else {
                        0
                    }
                }
            };
        }

        if s == 0 || s >= 0x8080 {
            mb_illegal_output(w, mb_wchar_to_cp51932, buf);
        } else {
            add_eucjp(buf, s);
        }
    }
}

/*
 * EUC-JP-2004
 */

pub fn mb_eucjp2004_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    // Combining characters convert to two codepoints
    let limit = out.len().saturating_sub(1);
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xFE).contains(&c) {
            // Kanji
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            if c2 <= 0xA0 || c2 == 0xFF {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            let s1 = c as u32 - 0x80;
            let s2 = c2 as u32 - 0x80;
            let mut w1 = (s1 << 8) | s2;
            let mut w = 0;

            // Conversion for combining characters
            if (0x2477..=0x2479).contains(&w1)
                || (0x2479..=0x247B).contains(&w1)
                || (0x2577..=0x257E).contains(&w1)
                || w1 == 0x2678
                || w1 == 0x2B44
                || (0x2B48..=0x2B4F).contains(&w1)
                || (0x2B65..=0x2B66).contains(&w1)
            {
                let k = mbfl_bisec_srch2(w1 as i32, &JISX0213_U2_KEY, JISX0213_U2_TBL_LEN as usize);
                if k >= 0 {
                    let k = k as usize;
                    out[o] = JISX0213_U2_TBL[2 * k] as u32;
                    out[o + 1] = JISX0213_U2_TBL[2 * k + 1] as u32;
                    o += 2;
                    continue;
                }
            }

            // Conversion for BMP
            w1 = (s1 - 0x21) * 94 + s2 - 0x21;
            if w1 < JISX0213_UCS_TABLE_SIZE {
                w = JISX0213_UCS_TABLE[w1 as usize] as u32;
            }

            // Conversion for CJK Unified Ideographs ext.B (U+2XXXX)
            if w == 0 {
                let k = mbfl_bisec_srch2(w1 as i32, &JISX0213_JIS_U5_KEY, JISX0213_U5_TBL_LEN as usize);
                if k >= 0 {
                    w = JISX0213_JIS_U5_TBL[k as usize] as u32 + 0x20000;
                }
            }

            out[o] = if w != 0 { w } else { BAD_INPUT };
            o += 1;
        } else if c == 0x8E && p < e {
            // Kana
            let c2 = s[p];
            p += 1;
            out[o] = if (0xA1..=0xDF).contains(&c2) { 0xFEC0 + c2 as u32 } else { BAD_INPUT };
            o += 1;
        } else if c == 0x8F && p < e {
            let c2 = s[p];
            p += 1;
            if (c2 == 0xA1
                || (0xA3..=0xA5).contains(&c2)
                || c2 == 0xA8
                || (0xAC..=0xAF).contains(&c2)
                || (0xEE..=0xFE).contains(&c2))
                && p < e
            {
                let c3 = s[p];
                p += 1;

                if c3 < 0xA1 || c3 == 0xFF {
                    out[o] = BAD_INPUT;
                    o += 1;
                    continue;
                }

                let s1 = c2 as u32 - 0xA1;
                let s2 = c3 as u32 - 0xA1;

                if ((s1 <= 4 && s1 != 1) || s1 == 7 || (11..=14).contains(&s1) || (77..94).contains(&s1))
                    && s2 < 94
                {
                    // Plane 2 rows are stored after plane 1 in the order of
                    // `JISX0213_P2_OFST`
                    let n = JISX0213_P2_OFST_LEN as usize;
                    let mut k = 0;
                    while k < n {
                        if s1 == JISX0213_P2_OFST[k] as u32 {
                            break;
                        }
                        k += 1;
                    }
                    let k = k as i32 - JISX0213_P2_OFST[k] as i32;

                    // Check for Japanese chars in BMP
                    let s = ((s1 as i32 + 94 + k) * 94 + s2 as i32) as u32;
                    debug_assert!(s < JISX0213_UCS_TABLE_SIZE);
                    let mut w = JISX0213_UCS_TABLE[s as usize] as u32;

                    // Check for Japanese chars in CJK Unified Ideographs ext B (U+2XXXX)
                    if w == 0 {
                        let k =
                            mbfl_bisec_srch2(s as i32, &JISX0213_JIS_U5_KEY, JISX0213_U5_TBL_LEN as usize);
                        if k >= 0 {
                            w = JISX0213_JIS_U5_TBL[k as usize] as u32 + 0x20000;
                        }
                    }

                    out[o] = if w != 0 { w } else { BAD_INPUT };
                } else {
                    out[o] = BAD_INPUT;
                }
            } else {
                out[o] = BAD_INPUT;
            }
            o += 1;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_eucjp2004(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let len = input.len();
    let mut ip = 0;
    let mut pending = None;

    if buf.state != 0 {
        pending = Some(buf.state);
        buf.state = 0;
    }

    loop {
        let w = if let Some(w) = pending.take() {
            w
        } else if ip < len {
            ip += 1;
            input[ip - 1]
        } else {
            break;
        };

        let Some(s) = ucs_to_jisx0213(w, input, &mut ip, end) else {
            buf.state = w;
            return;
        };

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_eucjp2004, buf);
        } else if s <= 0x7F {
            buf.add(s as u8);
        } else if s <= 0xFF {
            buf.add2(0x8E, s as u8);
        } else if s <= 0x7EFF {
            buf.add2((((s >> 8) & 0xFF) + 0x80) as u8, ((s & 0xFF) + 0x80) as u8);
        } else {
            let s2 = s & 0xFF;
            let k = ((s >> 8) & 0xFF) as i32 - 0x7F;
            debug_assert!(k < JISX0213_P2_OFST_LEN as i32);
            let s = JISX0213_P2_OFST[k as usize] as u32 + 0x21;
            buf.add3(0x8F, (s | 0x80) as u8, (s2 | 0x80) as u8);
        }
    }
}
