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
        buf.add3(
            0x8F,
            (((s >> 8) & 0xFF) | 0x80) as u8,
            ((s & 0xFF) | 0x80) as u8,
        );
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
            out[o] = if (0xA1..=0xDF).contains(&c2) {
                0xFEC0 + c2 as u32
            } else {
                BAD_INPUT
            };
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
            out[o] = if (0xA1..=0xDF).contains(&c2) {
                0xFEC0 + c2 as u32
            } else {
                BAD_INPUT
            };
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
            out[o] = if (0xA1..=0xDF).contains(&c2) {
                0xFEC0 + c2 as u32
            } else {
                BAD_INPUT
            };
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
                let k = mbfl_bisec_srch2(
                    w1 as i32,
                    &JISX0213_JIS_U5_KEY,
                    JISX0213_U5_TBL_LEN as usize,
                );
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
            out[o] = if (0xA1..=0xDF).contains(&c2) {
                0xFEC0 + c2 as u32
            } else {
                BAD_INPUT
            };
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

                if ((s1 <= 4 && s1 != 1)
                    || s1 == 7
                    || (11..=14).contains(&s1)
                    || (77..94).contains(&s1))
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
                        let k = mbfl_bisec_srch2(
                            s as i32,
                            &JISX0213_JIS_U5_KEY,
                            JISX0213_U5_TBL_LEN as usize,
                        );
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

#[cfg(test)]
mod tests {
    use super::super::sjis::MBFL_ENCODING_SJIS;
    use super::super::sjis::testutil::*;
    use super::*;

    fn set(map: &mut FromUni, cp: u32, bytes: &[u8]) {
        map.insert(vec![cp], bytes.to_vec());
    }

    // eucjp_encoding.phpt
    #[test]
    fn eucjp_exhaustive() {
        let enc = &MBFL_ENCODING_EUC_JP;
        let mut rng = Rng::new(555);
        let mut f = Failures::default();

        let (mut valid, mut from) = read_conversion_table("EUC-JP.txt");
        // Kuten 0x2140 (EUC-JP 0xA1C0) is a fullwidth backslash
        valid.insert(b"\xA1\xC0".to_vec(), vec![0xFF3C]);
        set(&mut from, 0xFF3C, b"\xA1\xC0");
        set(&mut from, 0xFFE2, b"\xA2\xCC");
        set(&mut from, 0xFFE1, b"\xA1\xF2");
        set(&mut from, 0xFFE0, b"\xA1\xF1");
        set(&mut from, 0xFF5E, b"\xA1\xC1");
        set(&mut from, 0xFF0D, b"\xA1\xDD");
        set(&mut from, 0x2225, b"\xA1\xC2");
        // U+007E and U+005C use the simpler of their two representations
        set(&mut from, 0x7E, b"\x7E");
        set(&mut from, 0x5C, b"\x5C");
        set(&mut from, 0x203E, b"\xA1\xB1");

        let (invalid, truncated) = find_invalid_chars(
            &valid,
            &[(0xA1..=0xFE, 2), (0x8E..=0x8E, 2), (0x8F..=0x8F, 3)],
        );
        assert!(
            valid.len() > 13000 && invalid.len() > 60000,
            "{} {}",
            valid.len(),
            invalid.len()
        );

        // JIS X 0212 0x2237 (EUC-JP 0x8FA2B7) is a tilde, like ASCII 0x7E
        valid.remove(b"\x8F\xA2\xB7".as_slice());

        test_all_valid_chars(&mut f, enc, &valid, true, &mut rng);
        test_all_invalid_chars(&mut f, enc, &invalid, &valid, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);
        check_valid(&mut f, enc, b"\x8F\xA2\xB7", &[0x7E], false);

        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);
        test_all_invalid_codepoints(&mut f, enc, invalid_bmp_codepoints(&from), &from, &mut rng);

        for bad in [&b"\x80"[..], b"\xFE\xFF"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("EUC-JP");
    }

    #[test]
    fn eucjp_truncation_and_limits() {
        let enc = &MBFL_ENCODING_EUC_JP;
        // A truncated JIS X 0212 character swallows the rest of the input
        assert_eq!(decode(enc, b"\x8F\xA2"), [BAD_INPUT]);
        assert_eq!(decode(enc, b"a\xA4"), [0x61, BAD_INPUT]);
        assert_eq!(decode(enc, b"\x8E"), [BAD_INPUT]);
        let mut inp: &[u8] = b"\xA4\xA2\xA4\xA4";
        let mut out = [0u32; 1];
        assert_eq!(mb_eucjp_to_wchar(&mut inp, &mut out, &mut 0), 1);
        assert_eq!((out[0], inp), (0x3042, &b"\xA4\xA4"[..]));
        // U+00AF uses the JIS X 0212 overline; X 0212 characters take SS3
        let buf = encode(enc, &[0xAF, 0, 0xFF61]);
        assert_eq!(buf.out, b"\x8F\xA2\xB4\x00\x8E\xA1");
    }

    // eucjp_ms_encoding.phpt
    #[test]
    fn eucjpwin_exhaustive() {
        let enc = &MBFL_ENCODING_EUCJP_WIN;
        let mut rng = Rng::new(555);
        let mut f = Failures::default();

        let (to, mut from) = read_conversion_table("EUC-JP-MS.txt");
        let (irreversible, _) = read_conversion_table("EUC-JP-MS.IRREVERSIBLE.txt");
        for (bytes, cps) in &irreversible {
            from.entry(cps.clone()).or_insert_with(|| bytes.clone());
        }

        // Several codes map to the same codepoint; use the intended ones
        for (cp, bytes) in [
            (0x2121, b"\xAD\xE4"),
            (0x2161, b"\xAD\xB6"),
            (0x2162, b"\xAD\xB7"),
            (0x2163, b"\xAD\xB8"),
            (0x2165, b"\xAD\xBA"),
            (0x2168, b"\xAD\xBD"),
            (0x2169, b"\xAD\xBE"),
            (0x221A, b"\xA2\xE5"),
            (0x2220, b"\xA2\xDC"),
            (0x2229, b"\xA2\xC1"),
            (0x222A, b"\xA2\xC0"),
            (0x222B, b"\xA2\xE9"),
            (0x2235, b"\xA2\xE8"),
            (0x2252, b"\xA2\xE2"),
            (0x2261, b"\xA2\xE1"),
            (0x22A5, b"\xA2\xDD"),
            (0x3231, b"\xAD\xEA"),
            (0xFF5E, b"\xA1\xC1"),
        ] {
            set(&mut from, cp, bytes);
        }

        let (invalid, truncated) = find_invalid_chars(&to, &[(0xA1..=0xFE, 2), (0x8F..=0x8F, 3)]);
        assert!(
            to.len() > 15000 && invalid.len() > 60000,
            "{} {}",
            to.len(),
            invalid.len()
        );
        test_all_valid_chars(&mut f, enc, &to, false, &mut rng);
        test_all_invalid_chars(&mut f, enc, &invalid, &to, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);

        test_all_invalid_codepoints(&mut f, enc, invalid_bmp_codepoints(&from), &from, &mut rng);
        // convertAllInvalidChars uses every character of `from` as a good one
        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);

        for bad in [&b"\x80"[..], b"\xFE\xFF"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("eucJP-win");
    }

    #[test]
    fn eucjpwin_specifics() {
        let mut f = Failures::default();
        let enc = &MBFL_ENCODING_EUCJP_WIN;
        // User-defined area: rows 85-94 of JIS X 0208 and of JIS X 0212
        check_valid(&mut f, enc, b"\xF5\xA1\xFE\xFE", &[0xE000, 0xE3AB], true);
        check_valid(
            &mut f,
            enc,
            b"\x8F\xF5\xA1\x8F\xFE\xFE",
            &[0xE3AC, 0xE757],
            true,
        );
        // IBM extensions in JIS X 0212 rows 83-84
        check_valid(&mut f, enc, b"\x8F\xF3\xF3", &[0x2170], true);
        // A JIS X 0212 lead byte with only one more byte
        check_valid(&mut f, enc, b"\x8F\xA2", &[BAD_INPUT], false);
        check_valid(&mut f, enc, b"\x8F", &[BAD_INPUT], false);
        check_valid_encode(
            &mut f,
            enc,
            &[0, 0xAF, 0x203E, 0xA5, 0x2014],
            b"\x00\x8F\xA2\xB4\x7E\x5C\xA1\xBD",
        );
        f.assert_none("eucJP-win specifics");
    }

    fn read_cp51932() -> (ToUni, FromUni) {
        let mut valid = ToUni::new();
        let mut from = FromUni::new();
        for line in data_lines("CP51932.txt") {
            if line.starts_with('#') {
                continue;
            }
            let Some((cp, rest)) = line.strip_prefix("<U").and_then(hex) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix('>') else {
                continue;
            };
            let Some((b1, rest)) = rest.trim_start().strip_prefix("\\x").and_then(hex) else {
                continue;
            };
            let b2 = rest.strip_prefix("\\x").and_then(hex).map_or(0, |(b, _)| b);
            // Latin letters with accents are mapped to the letters without;
            // none of them is mapped from any CP932 character
            if (0xC0..=0xD6).contains(&cp)
                || (0xD8..=0xF6).contains(&cp)
                || (0xF8..=0xFF).contains(&cp)
            {
                continue;
            }
            let bytes = if b2 != 0 {
                vec![b1 as u8, b2 as u8]
            } else {
                vec![b1 as u8]
            };
            valid.insert(bytes.clone(), vec![cp]);
            from.insert(vec![cp], bytes);
        }
        (valid, from)
    }

    // cp51932_encoding.phpt
    #[test]
    fn cp51932_exhaustive() {
        let enc = &MBFL_ENCODING_CP51932;
        let mut rng = Rng::new(2020);
        let mut f = Failures::default();

        let (mut valid, mut from) = read_cp51932();
        set(&mut from, 0x301C, b"\xA1\xC1");
        set(&mut from, 0x2212, b"\xA1\xDD");
        set(&mut from, 0x2016, b"\xA1\xC2");
        // Duplicate mappings: the primary ones
        for (cp, bytes) in [
            (0x2220, b"\xA2\xDC"),
            (0x2229, b"\xA2\xC1"),
            (0x222B, b"\xA2\xE9"),
            (0x2235, b"\xA2\xE8"),
            (0x221A, b"\xA2\xE5"),
            (0x222A, b"\xA2\xC0"),
            (0x2261, b"\xA2\xE1"),
            (0x22A5, b"\xA2\xDD"),
            (0x2252, b"\xA2\xE2"),
            (0xFFE2, b"\xA2\xCC"),
        ] {
            set(&mut from, cp, bytes);
        }
        // Lossy mappings of the table which we don't do
        for cp in [
            0xA1, 0xA6, 0xA9, 0xAA, 0xAB, 0xAD, 0xAE, 0xAF, 0xB2, 0xB3, 0xB5, 0xB7, 0xB8, 0xB9,
            0xBA, 0xBB, 0x3094,
        ] {
            from.remove(&vec![cp]);
        }
        for i in 0..=0x7Fu32 {
            valid.insert(vec![i as u8], vec![i]);
        }
        set(&mut from, 0xA5, b"\xA1\xEF");
        set(&mut from, 0x203E, b"\xA1\xB1");
        set(&mut from, 0xAF, b"\xA1\xB1");
        assert!(
            valid.len() > 7000 && from.len() > 7000,
            "{} {}",
            valid.len(),
            from.len()
        );

        test_all_valid_chars(&mut f, enc, &valid, false, &mut rng);
        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);

        let start_bytes = [(0xA9..=0xAF, 2), (0xF5..=0xF8, 2), (0xFD..=0xFE, 2)];
        let (invalid, truncated) = find_invalid_chars(&valid, &start_bytes);
        test_all_invalid_chars(&mut f, enc, &invalid, &valid, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);

        test_all_invalid_codepoints(&mut f, enc, invalid_bmp_codepoints(&from), &from, &mut rng);

        for bad in [&b"\x80"[..], b"\xFE\xFF"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("CP51932");
    }

    // eucjp_2004_encoding.phpt
    #[test]
    fn eucjp2004_exhaustive() {
        let enc = &MBFL_ENCODING_EUCJP2004;
        let mut rng = Rng::new(200);
        let mut f = Failures::default();

        let (mut valid, mut from) = read_jisx0213_table("EUC-JP-2004.txt", false);
        // 0xA1B1 is FULLWIDTH MACRON, 0xA1EF FULLWIDTH YEN SIGN
        valid.insert(b"\xA1\xB1".to_vec(), vec![0xFFE3]);
        set(&mut from, 0xFFE3, b"\xA1\xB1");
        valid.insert(b"\xA1\xEF".to_vec(), vec![0xFFE5]);
        set(&mut from, 0xFFE5, b"\xA1\xEF");
        // YEN SIGN and OVERLINE go to the bytes legacy software used for them
        set(&mut from, 0xA5, b"\x5C");
        set(&mut from, 0x203E, b"\x7E");
        let pairs = valid.values().filter(|v| v.len() == 2).count();
        assert!(
            valid.len() > 11000 && pairs == 25,
            "{} {pairs}",
            valid.len()
        );

        test_all_valid_chars(&mut f, enc, &valid, true, &mut rng);

        let (invalid, truncated) = find_invalid_chars(&valid, &[]);
        test_all_invalid_chars(&mut f, enc, &invalid, &valid, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);

        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);
        test_all_invalid_codepoints(&mut f, enc, invalid_bmp_codepoints(&from), &from, &mut rng);

        for bad in [&b"\x80"[..], b"\xFE\xFF"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("EUC-JP-2004");
    }

    #[test]
    fn eucjp2004_combining_across_buffers() {
        let enc = &MBFL_ENCODING_EUCJP2004;
        // か + U+309A is one character, also when split between calls
        for chunk in 1..=3 {
            let buf = encode_with(
                enc,
                &[0x41, 0x304B, 0x309A],
                chunk,
                b'%' as u32,
                ILLEGAL_MODE_CHAR,
            );
            assert_eq!(buf.out, b"A\xA4\xF7", "chunk size {chunk}");
        }
        // ... but か at the very end falls back to the plain character
        assert_eq!(encode(enc, &[0x304B]).out, b"\xA4\xAB");
        // Combining characters need two output slots
        let mut inp: &[u8] = b"a\xA4\xF7";
        let mut out = [0u32; 2];
        assert_eq!(mb_eucjp2004_to_wchar(&mut inp, &mut out, &mut 0), 1);
        assert_eq!(inp, b"\xA4\xF7");
        assert_eq!(
            decode_with(enc, b"\xA4\xF7\xA4\xF7", 3),
            [0x304B, 0x309A, 0x304B, 0x309A]
        );
        // Plane 2 characters (row 1 here) round-trip through JISX0213_P2_OFST
        let cps = decode(enc, b"\x8F\xA1\xA1");
        assert_eq!(cps.len(), 1);
        assert_ne!(cps[0], BAD_INPUT);
        assert_eq!(encode(enc, &cps).out, b"\x8F\xA1\xA1");
    }

    // bug25140.phpt, bug69086.phpt, bug69079.phpt
    #[test]
    fn eucjp_bug_regressions() {
        // Converting an empty string
        assert!(decode(&MBFL_ENCODING_EUC_JP, b"").is_empty());
        assert!(encode(&MBFL_ENCODING_SJIS, &[]).out.is_empty());

        // A replacement character the target can't encode becomes '?'
        let bad = decode(&MBFL_ENCODING_EUC_JP, b"\x80");
        assert_eq!(bad, [BAD_INPUT]);
        let buf = encode_with(&MBFL_ENCODING_SJIS, &bad, 128, 0xFFFD, ILLEGAL_MODE_CHAR);
        assert_eq!(buf.out, b"?");
        assert_eq!(decode(&MBFL_ENCODING_EUCJP2004, b"\x80"), [BAD_INPUT]);

        // mb_scrub in EUC-JP-2004 with various substitute characters
        let enc = &MBFL_ENCODING_EUCJP2004;
        let bad = decode(enc, b"\x8D");
        assert_eq!(bad, [BAD_INPUT]);
        assert_eq!(
            encode_with(enc, &bad, 128, 0x63, ILLEGAL_MODE_CHAR).out,
            b"c"
        );
        assert_eq!(
            encode_with(enc, &bad, 128, 0x50AA, ILLEGAL_MODE_CHAR).out,
            b"\x8F\xA1\xEF"
        );
    }
}
