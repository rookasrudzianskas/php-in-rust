//! The Korean encodings of `libmbfl/filters/mbfilter_cjk.c`: EUC-KR
//! (KS X 1001) and UHC (CP949, its Microsoft superset). Both use the UHC
//! conversion tables; EUC-KR excludes the UHC extension area.

use crate::mbstring::filters::mblen;
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::tables::unicode_table_uhc::*;

pub static MBFL_ENCODING_EUC_KR: Encoding = Encoding {
    no: NoEncoding::EucKr,
    name: "EUC-KR",
    mime_name: Some("EUC-KR"),
    aliases: &["EUC_KR", "eucKR", "x-euc-kr"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCCN),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_euckr_to_wchar,
    from_wchar: mb_wchar_to_euckr,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UHC: Encoding = Encoding {
    no: NoEncoding::Uhc,
    name: "UHC",
    mime_name: Some("UHC"),
    aliases: &["CP949"],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_uhc_to_wchar,
    from_wchar: mb_wchar_to_uhc,
    check: None,
    cut: None,
};

pub fn mb_euckr_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        out[o] = if c < 0x80 {
            c as u32
        } else if ((0xA1..=0xAC).contains(&c) || (0xB0..=0xFD).contains(&c)) && c != 0xC9 && p < e {
            let c2 = s[p];
            p += 1;
            if c2 < 0xA1 || c2 == 0xFF {
                BAD_INPUT
            } else {
                let w = if c <= 0xC6 {
                    let w = (c as u32 - 0x81) * 190 + c2 as u32 - 0x41;
                    debug_assert!(w < UHC1_UCS_TABLE_SIZE);
                    UHC1_UCS_TABLE[w as usize] as u32
                } else {
                    let w = (c as u32 - 0xC7) * 94 + c2 as u32 - 0xA1;
                    debug_assert!(w < UHC3_UCS_TABLE_SIZE);
                    UHC3_UCS_TABLE[w as usize] as u32
                };
                if w == 0 { BAD_INPUT } else { w }
            }
        } else {
            BAD_INPUT
        };
        o += 1;
    }

    *input = &s[p..];
    o
}

/// The Unicode -> UHC table lookups shared by EUC-KR and UHC; 0 if unmapped.
#[inline]
fn uhc_lookup(w: u32) -> u32 {
    if (UCS_A1_UHC_TABLE_MIN..UCS_A1_UHC_TABLE_MAX).contains(&w) {
        UCS_A1_UHC_TABLE[(w - UCS_A1_UHC_TABLE_MIN) as usize] as u32
    } else if (UCS_A2_UHC_TABLE_MIN..UCS_A2_UHC_TABLE_MAX).contains(&w) {
        UCS_A2_UHC_TABLE[(w - UCS_A2_UHC_TABLE_MIN) as usize] as u32
    } else if (UCS_A3_UHC_TABLE_MIN..UCS_A3_UHC_TABLE_MAX).contains(&w) {
        UCS_A3_UHC_TABLE[(w - UCS_A3_UHC_TABLE_MIN) as usize] as u32
    } else if (UCS_I_UHC_TABLE_MIN..UCS_I_UHC_TABLE_MAX).contains(&w) {
        UCS_I_UHC_TABLE[(w - UCS_I_UHC_TABLE_MIN) as usize] as u32
    } else if (UCS_S_UHC_TABLE_MIN..UCS_S_UHC_TABLE_MAX).contains(&w) {
        UCS_S_UHC_TABLE[(w - UCS_S_UHC_TABLE_MIN) as usize] as u32
    } else if (UCS_R1_UHC_TABLE_MIN..UCS_R1_UHC_TABLE_MAX).contains(&w) {
        UCS_R1_UHC_TABLE[(w - UCS_R1_UHC_TABLE_MIN) as usize] as u32
    } else if (UCS_R2_UHC_TABLE_MIN..UCS_R2_UHC_TABLE_MAX).contains(&w) {
        UCS_R2_UHC_TABLE[(w - UCS_R2_UHC_TABLE_MIN) as usize] as u32
    } else {
        0
    }
}

pub fn mb_wchar_to_euckr(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s = uhc_lookup(w);

        // Exclude the UHC extension area (although the UHC conversion tables are used).
        if ((s >> 8) & 0xFF) < 0xA1 || (s & 0xFF) < 0xA1 {
            s = 0;
        }

        if s == 0 {
            if w < 0x80 {
                buf.add(w as u8);
            } else {
                mb_illegal_output(w, mb_wchar_to_euckr, buf);
            }
        } else if s < 0x80 {
            buf.add(s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

pub fn mb_uhc_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    if s.is_empty() {
        return 0;
    }
    // Stop the main loop 1 byte short of the end of the input, so the second
    // byte of a 2-byte character can be read without checking.
    let e = s.len() - 1;
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        out[o] = if c < 0x80 {
            c as u32
        } else if c > 0x80 && c < 0xFE {
            let c2 = s[p];
            p += 1;
            if c2 < 0x41 || c2 == 0xFF {
                BAD_INPUT
            } else {
                let mut w = 0;
                if c <= 0xC6 {
                    let i = (c as u32 - 0x81) * 190 + c2 as u32 - 0x41;
                    debug_assert!(i < UHC1_UCS_TABLE_SIZE);
                    w = UHC1_UCS_TABLE[i as usize] as u32;
                } else if c2 >= 0xA1 {
                    let i = (c as u32 - 0xC7) * 94 + c2 as u32 - 0xA1;
                    debug_assert!(i < UHC3_UCS_TABLE_SIZE);
                    w = UHC3_UCS_TABLE[i as usize] as u32;
                }
                if w == 0 {
                    // 0xC9 does not start a 2-byte character at all; php-src
                    // backs up here rather than check for it in the hot path.
                    if c == 0xC9 {
                        p -= 1;
                    }
                    BAD_INPUT
                } else {
                    w
                }
            }
        } else {
            BAD_INPUT
        };
        o += 1;
    }

    // Finish up the last byte of the input string, if there is one.
    if p == e && o < out.len() {
        let c = s[p];
        p += 1;
        out[o] = if c < 0x80 { c as u32 } else { BAD_INPUT };
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_uhc(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let s = uhc_lookup(w);

        if s == 0 {
            if w == 0 {
                buf.add(0);
            } else {
                mb_illegal_output(w, mb_wchar_to_uhc, buf);
            }
        } else if s < 0x80 {
            buf.add(s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::chinese::test_support::*;
    use super::*;

    #[test]
    fn euc_kr_encoding() {
        let euckr = &MBFL_ENCODING_EUC_KR;
        test_encoding_from_utf16_conversion_table("data/EUC-KR.txt", euckr);

        // "long" illegal character markers
        assert_invalid(b"\x80", euckr, &[PCT]);
        assert_invalid(b"\xA7\xF0", euckr, &[PCT]);
    }

    #[test]
    fn uhc_encoding() {
        let uhc = &MBFL_ENCODING_UHC;
        test_encoding_from_utf16_conversion_table("data/CP949.txt", uhc);

        // Regression test
        assert_invalid(b"\xE4\xA4\xB4<", uhc, &[0x751A, PCT]);
        // 0xC9 before a valid character which starts with a byte below 0xA1
        assert_invalid(b"\xC9\x9E\x98", uhc, &[PCT, 0xC612]);

        // "long" illegal character markers
        assert_invalid(b"\x80", uhc, &[PCT]);
        assert_invalid(b"\xA7\xF0", uhc, &[PCT]);
    }

    #[test]
    fn uhc_last_byte_and_buffer_limit() {
        let uhc = &MBFL_ENCODING_UHC;
        assert_eq!(to_unicode(b"", uhc), (vec![], 0));
        assert_eq!(to_unicode(b"a", uhc), (vec![b'a' as u32], 0));
        assert_eq!(to_unicode(b"\xB0", uhc), (vec![PCT], 1));
        assert_eq!(to_unicode(b"a\xB0", uhc), (vec![b'a' as u32, PCT], 1));
        // A full output buffer leaves the rest of the input unconsumed.
        let mut input: &[u8] = b"ab\xB0\xA1c";
        let mut out = [0u32; 2];
        assert_eq!(mb_uhc_to_wchar(&mut input, &mut out, &mut 0), 2);
        assert_eq!(input, b"\xB0\xA1c");
        assert_eq!(mb_uhc_to_wchar(&mut input, &mut out, &mut 0), 2);
        assert_eq!(out, [0xAC00, b'c' as u32]);
        assert!(input.is_empty());
    }
}
