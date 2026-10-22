//! `libmbfl/filters/mbfilter_cjk.c`: Chinese, Japanese and Korean encodings.
//!
//! The C file is split by family: [`iso2022`] (JIS, ISO-2022-JP and its
//! variants, CP5022x, ISO-2022-KR, `mb_convert_kana_codepoint`), [`sjis`]
//! (Shift-JIS variants), [`eucjp`] (EUC-JP variants), [`chinese`] (EUC-CN,
//! CP936, GB18030, Big5, CP950, EUC-TW, HZ) and [`korean`] (EUC-KR, UHC).
//! Helpers shared by several families live here.

pub mod chinese;
pub mod eucjp;
pub mod iso2022;
pub mod korean;
pub mod sjis;

use crate::mbstring::tables::emoji2uni::*;

/// `translit_kana_jisx0201_jisx0208.h`: `mb_convert_kana` mode flags
/// ("Zen" = fullwidth, "Han" = halfwidth).
pub mod kana {
    pub const HAN2ZEN_ALL: u32 = 0x00001;
    pub const HAN2ZEN_ALPHA: u32 = 0x00002;
    pub const HAN2ZEN_NUMERIC: u32 = 0x00004;
    pub const HAN2ZEN_SPACE: u32 = 0x00008;
    pub const HAN2ZEN_KATAKANA: u32 = 0x00010;
    pub const HAN2ZEN_HIRAGANA: u32 = 0x00020;
    pub const HAN2ZEN_SPECIAL: u32 = 0x00040;
    pub const ZENKAKU_HIRA2KATA: u32 = 0x00080;
    pub const ZEN2HAN_ALL: u32 = 0x00100;
    pub const ZEN2HAN_ALPHA: u32 = 0x00200;
    pub const ZEN2HAN_NUMERIC: u32 = 0x00400;
    pub const ZEN2HAN_SPACE: u32 = 0x00800;
    pub const ZEN2HAN_KATAKANA: u32 = 0x01000;
    pub const ZEN2HAN_HIRAGANA: u32 = 0x02000;
    pub const ZEN2HAN_SPECIAL: u32 = 0x04000;
    pub const ZENKAKU_KATA2HIRA: u32 = 0x08000;
    pub const HAN2ZEN_GLUE: u32 = 0x10000;
}

/// `mbfl_bisec_srch`: `tbl` holds `n` inclusive ranges as pairs; returns the
/// index of the range containing `w`, or -1.
pub fn mbfl_bisec_srch(w: i32, tbl: &[u16], n: usize) -> i32 {
    let (mut l, mut r) = (0i32, n as i32 - 1);
    while l <= r {
        let probe = (l + r) >> 1;
        let lo = tbl[2 * probe as usize] as i32;
        let hi = tbl[2 * probe as usize + 1] as i32;
        if w < lo {
            r = probe - 1;
        } else if w > hi {
            l = probe + 1;
        } else {
            return probe;
        }
    }
    -1
}

/// `mbfl_bisec_srch2`: `tbl` holds `n` sorted single values; returns the
/// index of `w`, or -1.
pub fn mbfl_bisec_srch2(w: i32, tbl: &[u16], n: usize) -> i32 {
    let (mut l, mut r) = (0i32, n as i32 - 1);
    while l <= r {
        let probe = (l + r) >> 1;
        let val = tbl[probe as usize] as i32;
        if w < val {
            r = probe - 1;
        } else if w > val {
            l = probe + 1;
        } else {
            return probe;
        }
    }
    -1
}

/// `mbfl_binary_search_paired_sorted_table`: the value paired with key `w`.
pub fn mbfl_binary_search_paired_sorted_table(w: u32, tbl: &[[u16; 2]]) -> Option<u16> {
    let (mut l, mut r) = (0usize, tbl.len());
    while l < r {
        let probe = (l + r) >> 1;
        let key = tbl[probe][0] as u32;
        if w < key {
            r = probe;
        } else if w > key {
            l = probe + 1;
        } else {
            return Some(tbl[probe][1]);
        }
    }
    None
}

/// `SJIS_ENCODE(c1, c2, s1, s2)`: JIS row/cell bytes to Shift-JIS bytes.
#[inline]
pub fn sjis_encode(c1: u32, c2: u32) -> (u32, u32) {
    let s1 = ((c1.wrapping_sub(1)) >> 1) + if c1 < 0x5F { 0x71 } else { 0xB1 };
    let mut s2 = c2;
    if c1 & 1 != 0 {
        if c2 < 0x60 {
            s2 = s2.wrapping_sub(1);
        }
        s2 = s2.wrapping_add(0x20);
    } else {
        s2 = s2.wrapping_add(0x7E);
    }
    (s1, s2)
}

/// `SJIS_DECODE(c1, c2, s1, s2)`: Shift-JIS bytes to JIS row/cell bytes.
#[inline]
pub fn sjis_decode(c1: u32, c2: u32) -> (u32, u32) {
    let mut s1 = if c1 < 0xA0 {
        ((c1.wrapping_sub(0x81)) << 1).wrapping_add(0x21)
    } else {
        ((c1.wrapping_sub(0xC1)) << 1).wrapping_add(0x21)
    };
    let mut s2 = c2;
    if c2 < 0x9F {
        if c2 < 0x7F {
            s2 += 1;
        }
        s2 = s2.wrapping_sub(0x20);
    } else {
        s1 += 1;
        s2 = s2.wrapping_sub(0x7E);
    }
    (s1, s2)
}

/// `NFLAGS(c)`: uppercase ASCII letter to its Regional Indicator codepoint.
#[inline]
pub const fn nflags(c: u8) -> i32 {
    0x1F1A5 + c as i32
}

pub const NFLAGS_S: [&[u8; 2]; 10] = [
    b"CN", b"DE", b"ES", b"FR", b"GB", b"IT", b"JP", b"KR", b"RU", b"US",
];
pub const NFLAGS_CODE_KDDI: [i32; 10] = [
    0x2549, 0x2546, 0x24C0, 0x2545, 0x2548, 0x2547, 0x2750, 0x254A, 0x24C1, 0x27F7,
];
pub const NFLAGS_CODE_SB: [i32; 10] = [
    0x2B0A, 0x2B05, 0x2B08, 0x2B04, 0x2B07, 0x2B06, 0x2B02, 0x2B0B, 0x2B09, 0x2B03,
];
pub const NFLAGS_KDDI: [&[u8; 2]; 6] = [b"FR", b"DE", b"IT", b"GB", b"CN", b"KR"];
pub const NFLAGS_SB: [&[u8; 2]; 10] = [
    b"JP", b"US", b"FR", b"DE", b"IT", b"GB", b"ES", b"RU", b"CN", b"KR",
];

/// `DOCOMO_KEYPAD(n)`: (ku*94)+ten value of a telephone keypad character.
#[inline]
pub const fn docomo_keypad(n: i32) -> i32 {
    if n == 0 { 0x296F } else { 0x2965 + n }
}
pub const DOCOMO_KEYPAD_HASH: i32 = 0x2964;

/// `EMIT_FLAG_EMOJI(country)`: returns the second codepoint, `snd` the first.
#[inline]
fn emit_flag_emoji(country: &[u8; 2], snd: &mut i32) -> i32 {
    *snd = nflags(country[0]);
    nflags(country[1])
}

/// `EMIT_KEYPAD_EMOJI(c)`.
#[inline]
fn emit_keypad_emoji(c: i32, snd: &mut i32) -> i32 {
    *snd = c;
    0x20E3
}

/// `convert_emoji_cp`: emoji tables store some codepoints offset to fit in
/// 16 bits.
#[inline]
pub fn convert_emoji_cp(cp: i32) -> i32 {
    if cp > 0xF000 {
        cp + 0x10000
    } else if cp > 0xE000 {
        cp + 0xF0000
    } else {
        cp
    }
}

/// `mbfilter_sjis_emoji_kddi2unicode`: KDDI emoji (ku*94)+ten value to
/// Unicode; 0 if `s` is not an emoji. A second codepoint (emitted before
/// the returned one) is stored in `snd`.
pub fn mbfilter_sjis_emoji_kddi2unicode(s: i32, snd: &mut i32) -> i32 {
    let s_u = s as u32;
    if (MB_TBL_CODE2UNI_KDDI1_MIN..=MB_TBL_CODE2UNI_KDDI1_MAX).contains(&s_u) {
        if s == 0x24C0 {
            return emit_flag_emoji(b"ES", snd);
        } else if s == 0x24C1 {
            return emit_flag_emoji(b"RU", snd);
        } else if (0x2545..=0x254A).contains(&s) {
            return emit_flag_emoji(NFLAGS_KDDI[(s - 0x2545) as usize], snd);
        } else if s == 0x25BC {
            return emit_keypad_emoji(b'#' as i32, snd);
        } else {
            *snd = 0;
            return convert_emoji_cp(
                MB_TBL_CODE2UNI_KDDI1[(s_u - MB_TBL_CODE2UNI_KDDI1_MIN) as usize] as i32,
            );
        }
    } else if (MB_TBL_CODE2UNI_KDDI2_MIN..=MB_TBL_CODE2UNI_KDDI2_MAX).contains(&s_u) {
        if s == 0x2750 {
            return emit_flag_emoji(b"JP", snd);
        } else if (0x27A6..=0x27AE).contains(&s) {
            return emit_keypad_emoji(s - 0x27A6 + b'1' as i32, snd);
        } else if s == 0x27F7 {
            return emit_flag_emoji(b"US", snd);
        } else if s == 0x2830 {
            return emit_keypad_emoji(b'0' as i32, snd);
        } else {
            *snd = 0;
            return convert_emoji_cp(
                MB_TBL_CODE2UNI_KDDI2[(s_u - MB_TBL_CODE2UNI_KDDI2_MIN) as usize] as i32,
            );
        }
    }
    0
}

/// `mbfilter_sjis_emoji_docomo2unicode`.
pub fn mbfilter_sjis_emoji_docomo2unicode(s: i32, snd: &mut i32) -> i32 {
    let s_u = s as u32;
    if (MB_TBL_CODE2UNI_DOCOMO1_MIN..=MB_TBL_CODE2UNI_DOCOMO1_MAX).contains(&s_u) {
        let cp = convert_emoji_cp(
            MB_TBL_CODE2UNI_DOCOMO1[(s_u - MB_TBL_CODE2UNI_DOCOMO1_MIN) as usize] as i32,
        );
        if (s >= docomo_keypad(1) && s <= docomo_keypad(9))
            || s == docomo_keypad(0)
            || s == DOCOMO_KEYPAD_HASH
        {
            return emit_keypad_emoji(cp, snd);
        } else {
            *snd = 0;
            return cp;
        }
    }
    0
}

/// `mbfilter_sjis_emoji_sb2unicode`.
pub fn mbfilter_sjis_emoji_sb2unicode(s: i32, snd: &mut i32) -> i32 {
    let s_u = s as u32;
    if (MB_TBL_CODE2UNI_SB1_MIN..=MB_TBL_CODE2UNI_SB1_MAX).contains(&s_u) {
        let v = MB_TBL_CODE2UNI_SB1[(s_u - MB_TBL_CODE2UNI_SB1_MIN) as usize] as i32;
        if s == 0x2817 || (0x2823..=0x282C).contains(&s) {
            return emit_keypad_emoji(v, snd);
        } else {
            *snd = 0;
            return convert_emoji_cp(v);
        }
    } else if (MB_TBL_CODE2UNI_SB2_MIN..=MB_TBL_CODE2UNI_SB2_MAX).contains(&s_u) {
        *snd = 0;
        return convert_emoji_cp(
            MB_TBL_CODE2UNI_SB2[(s_u - MB_TBL_CODE2UNI_SB2_MIN) as usize] as i32,
        );
    } else if (MB_TBL_CODE2UNI_SB3_MIN..=MB_TBL_CODE2UNI_SB3_MAX).contains(&s_u) {
        if (0x2B02..=0x2B0B).contains(&s) {
            return emit_flag_emoji(NFLAGS_SB[(s - 0x2B02) as usize], snd);
        } else {
            *snd = 0;
            return convert_emoji_cp(
                MB_TBL_CODE2UNI_SB3[(s_u - MB_TBL_CODE2UNI_SB3_MIN) as usize] as i32,
            );
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sjis_encode_decode_roundtrip() {
        for c1 in 0x21..=0x7E {
            for c2 in 0x21..=0x7E {
                let (s1, s2) = sjis_encode(c1, c2);
                assert_eq!(sjis_decode(s1, s2), (c1, c2));
            }
        }
    }

    #[test]
    fn bisect() {
        let t = [1u16, 3, 5, 9, 20, 30];
        assert_eq!(mbfl_bisec_srch(4, &t, 3), -1);
        assert_eq!(mbfl_bisec_srch(6, &t, 3), 1);
        assert_eq!(mbfl_bisec_srch2(20, &t, 6), 4);
        assert_eq!(mbfl_binary_search_paired_sorted_table(5, &[[1, 2], [5, 7]]), Some(7));
    }
}
