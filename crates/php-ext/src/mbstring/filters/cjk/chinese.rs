//! The Chinese encodings of `libmbfl/filters/mbfilter_cjk.c`: EUC-CN
//! (GB 2312), EUC-TW (CNS 11643), CP936 (GBK), GB18030 and GB18030-2022,
//! BIG-5, CP950 and HZ.
//!
//! HZ is the only one which still has legacy byte-at-a-time filters
//! (`mbfl_filt_conv_*`); `mbfl_strcut` walks HZ strings with them. GB18030
//! has a dedicated `mb_strcut` implementation, [`mb_cut_gb18030`].

use super::{mbfl_bisec_srch, mbfl_bisec_srch2};
use crate::mbstring::filters::mblen;
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{
    ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, illegal_output,
};
use crate::mbstring::tables::unicode_table_big5::*;
use crate::mbstring::tables::unicode_table_cns11643::*;
use crate::mbstring::tables::unicode_table_cp936::*;
use crate::mbstring::tables::unicode_table_gb2312::*;
use crate::mbstring::tables::unicode_table_gb18030::*;

pub static MBFL_ENCODING_EUC_CN: Encoding = Encoding {
    no: NoEncoding::EucCn,
    name: "EUC-CN",
    mime_name: Some("CN-GB"),
    aliases: &["CN-GB", "EUC_CN", "eucCN", "x-euc-cn", "gb2312"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCCN),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_euccn_to_wchar,
    from_wchar: mb_wchar_to_euccn,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_EUC_TW: Encoding = Encoding {
    no: NoEncoding::EucTw,
    name: "EUC-TW",
    mime_name: Some("EUC-TW"),
    aliases: &["EUC_TW", "eucTW", "x-euc-tw"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCCN),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_euctw_to_wchar,
    from_wchar: mb_wchar_to_euctw,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_GB18030: Encoding = Encoding {
    no: NoEncoding::Gb18030,
    name: "GB18030",
    mime_name: Some("GB18030"),
    aliases: &["gb-18030", "gb-18030-2000"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_gb18030_to_wchar,
    from_wchar: mb_wchar_to_gb18030,
    check: None,
    cut: Some(mb_cut_gb18030),
};

pub static MBFL_ENCODING_CP936: Encoding = Encoding {
    no: NoEncoding::Cp936,
    name: "CP936",
    mime_name: Some("CP936"),
    aliases: &["CP-936", "GBK"],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp936_to_wchar,
    from_wchar: mb_wchar_to_cp936,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_GB18030_2022: Encoding = Encoding {
    no: NoEncoding::Gb18030_2022,
    name: "GB18030-2022",
    mime_name: Some("GB18030-2022"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_gb18030_2022_to_wchar,
    from_wchar: mb_wchar_to_gb18030_2022,
    check: None,
    cut: Some(mb_cut_gb18030),
};

pub static MBFL_ENCODING_BIG5: Encoding = Encoding {
    no: NoEncoding::Big5,
    name: "BIG-5",
    mime_name: Some("BIG5"),
    aliases: &["CN-BIG5", "BIG-FIVE", "BIGFIVE"],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_big5_to_wchar,
    from_wchar: mb_wchar_to_big5,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP950: Encoding = Encoding {
    no: NoEncoding::Cp950,
    name: "CP950",
    mime_name: Some("BIG5"),
    aliases: &[],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp950_to_wchar,
    from_wchar: mb_wchar_to_cp950,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_HZ: Encoding = Encoding {
    no: NoEncoding::Hz,
    name: "HZ",
    mime_name: Some("HZ-GB-2312"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_HZ_WCHAR),
    output_filter: Some(&VTBL_WCHAR_HZ),
    to_wchar: mb_hz_to_wchar,
    from_wchar: mb_wchar_to_hz,
    check: None,
    cut: None,
};

pub static VTBL_HZ_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Hz,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_hz_wchar,
    flush: mbfl_filt_conv_hz_wchar_flush,
};

pub static VTBL_WCHAR_HZ: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Hz,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_hz,
    flush: mbfl_filt_conv_any_hz_flush,
};

/// `MBFL_BAD_INPUT` as the `int` the legacy filters pass around.
const FILTER_BAD_INPUT: i32 = BAD_INPUT as i32;

/*
 * EUC-CN
 */

pub fn mb_euccn_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        out[o] = if c < 0x80 {
            c as u32
        } else if ((0xA1..=0xA9).contains(&c) || (0xB0..=0xF7).contains(&c)) && p < e {
            let c2 = s[p];
            p += 1;

            if (0xA1..=0xFE).contains(&c2) {
                let w = (c as u32 - 0x81) * 192 + c2 as u32 - 0x40;
                debug_assert!(w < CP936_UCS_TABLE_SIZE);
                let w = if w == 0x1864 {
                    0x30FB
                } else if w == 0x186A {
                    0x2015
                } else if (0x1921..=0x192A).contains(&w)
                    || w == 0x1963
                    || (0x1C59..=0x1C7E).contains(&w)
                    || (0x1DBB..=0x1DC4).contains(&w)
                {
                    0
                } else {
                    CP936_UCS_TABLE[w as usize] as u32
                };
                if w == 0 { BAD_INPUT } else { w }
            } else {
                BAD_INPUT
            }
        } else {
            BAD_INPUT
        };
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_euccn(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s: u32 = 0;

        if (UCS_A1_CP936_TABLE_MIN..UCS_A1_CP936_TABLE_MAX).contains(&w) {
            if w != 0xB7 && w != 0x144 && w != 0x148 && w != 0x251 && w != 0x261 {
                s = UCS_A1_CP936_TABLE[(w - UCS_A1_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A2_CP936_TABLE_MIN..UCS_A2_CP936_TABLE_MAX).contains(&w) {
            if w == 0x2015 {
                s = 0xA1AA;
            } else if w != 0x2014 && !(0x2170..=0x2179).contains(&w) {
                s = UCS_A2_CP936_TABLE[(w - UCS_A2_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A3_CP936_TABLE_MIN..UCS_A3_CP936_TABLE_MAX).contains(&w) {
            if w == 0x30FB {
                s = 0xA1A4;
            } else {
                s = UCS_A3_CP936_TABLE[(w - UCS_A3_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_I_CP936_TABLE_MIN..UCS_I_CP936_TABLE_MAX).contains(&w) {
            s = UCS_I_CP936_TABLE[(w - UCS_I_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_HFF_CP936_TABLE_MIN..UCS_HFF_CP936_TABLE_MAX).contains(&w) {
            if w == 0xFF04 {
                s = 0xA1E7;
            } else if w == 0xFF5E {
                s = 0xA1AB;
            } else if (0xFF01..=0xFF5D).contains(&w) {
                s = w - 0xFF01 + 0xA3A1;
            } else if (0xFFE0..=0xFFE5).contains(&w) {
                s = UCS_HFF_S_CP936_TABLE[(w - 0xFFE0) as usize] as u32;
            }
        }

        // Exclude CP936 extensions.
        if ((s >> 8) & 0xFF) < 0xA1 || (s & 0xFF) < 0xA1 {
            s = 0;
        }

        if s == 0 {
            if w < 0x80 {
                buf.add(w as u8);
            } else {
                mb_illegal_output(w, mb_wchar_to_euccn, buf);
            }
        } else if s < 0x80 {
            buf.add(s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

/*
 * EUC-TW
 */

pub fn mb_euctw_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if ((0xA1..=0xA6).contains(&c) || (0xC2..=0xFD).contains(&c)) && c != 0xC3 && p < e {
            let c2 = s[p];
            p += 1;

            if (0xA1..=0xFE).contains(&c2) {
                let w = (c as u32 - 0xA1) * 94 + (c2 as u32 - 0xA1);
                let w = if w < CNS11643_1_UCS_TABLE_SIZE {
                    CNS11643_1_UCS_TABLE[w as usize] as u32
                } else {
                    0
                };
                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
        } else if c == 0x8E && p < e {
            let c2 = s[p];
            p += 1;

            if (c2 == 0xA1 || c2 == 0xA2 || c2 == 0xAE) && p < e {
                // This is actually the CNS-11643 plane minus one.
                let plane = (c2 - 0xA1) as u32;
                let c3 = s[p];
                p += 1;

                if c3 >= 0xA1
                    && ((plane == 0
                        && ((0xA1..=0xA6).contains(&c3) || (0xC2..=0xFD).contains(&c3))
                        && c3 != 0xC3)
                        || (plane == 1 && c3 <= 0xF2)
                        || (plane == 13 && c3 <= 0xE7))
                    && p < e
                {
                    let c4 = s[p];
                    p += 1;

                    if c2 <= 0xAE && c4 > 0xA0 && c4 < 0xFF {
                        let s2 = (c3 as u32 - 0xA1) * 94 + c4 as u32 - 0xA1;
                        let mut w = 0;

                        // A later version of CNS-11643 moved all the characters in "plane 14"
                        // to "plane 3", and added tens of thousands more characters in planes
                        // 4-7; only the older version is supported (like glibc 2.2's iconv).
                        if plane == 0 && s2 < CNS11643_1_UCS_TABLE_SIZE {
                            w = CNS11643_1_UCS_TABLE[s2 as usize] as u32;
                        } else if plane == 1 && s2 < CNS11643_2_UCS_TABLE_SIZE {
                            w = CNS11643_2_UCS_TABLE[s2 as usize] as u32;
                        } else if plane == 13 && s2 < CNS11643_14_UCS_TABLE_SIZE {
                            w = CNS11643_14_UCS_TABLE[s2 as usize] as u32;
                        }

                        out[o] = if w == 0 { BAD_INPUT } else { w };
                        o += 1;
                        continue;
                    }
                }
            }

            out[o] = BAD_INPUT;
        } else {
            out[o] = BAD_INPUT;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_euctw(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s: u32 = 0;

        if (UCS_A1_CNS11643_TABLE_MIN..UCS_A1_CNS11643_TABLE_MAX).contains(&w) {
            s = UCS_A1_CNS11643_TABLE[(w - UCS_A1_CNS11643_TABLE_MIN) as usize];
        } else if (UCS_A2_CNS11643_TABLE_MIN..UCS_A2_CNS11643_TABLE_MAX).contains(&w) {
            s = UCS_A2_CNS11643_TABLE[(w - UCS_A2_CNS11643_TABLE_MIN) as usize];
        } else if (UCS_A3_CNS11643_TABLE_MIN..UCS_A3_CNS11643_TABLE_MAX).contains(&w) {
            s = UCS_A3_CNS11643_TABLE[(w - UCS_A3_CNS11643_TABLE_MIN) as usize];
        } else if (UCS_I_CNS11643_TABLE_MIN..UCS_I_CNS11643_TABLE_MAX).contains(&w) {
            s = UCS_I_CNS11643_TABLE[(w - UCS_I_CNS11643_TABLE_MIN) as usize];
        } else if (UCS_R_CNS11643_TABLE_MIN..UCS_R_CNS11643_TABLE_MAX).contains(&w) {
            s = UCS_R_CNS11643_TABLE[(w - UCS_R_CNS11643_TABLE_MIN) as usize];
        }

        if s == 0 {
            if w == 0 {
                buf.add(0);
            } else {
                mb_illegal_output(w, mb_wchar_to_euctw, buf);
            }
        } else {
            let plane = s >> 16;
            if plane <= 1 {
                if s < 0x80 {
                    buf.add(s as u8);
                } else {
                    buf.add2(((s >> 8) & 0xFF) as u8 | 0x80, (s & 0xFF) as u8 | 0x80);
                }
            } else {
                buf.add4(
                    0x8E,
                    (0xA0 + plane) as u8,
                    ((s >> 8) & 0xFF) as u8 | 0x80,
                    (s & 0xFF) as u8 | 0x80,
                );
            }
        }
    }
}

/*
 * GB18030/CP936
 */

/// `gb18030_pua_tbl3`: PUA mappings for lookup indices 0x5DD0-0x5E20
/// (bytes 0xFE50-0xFEA0); 0 means "use `cp936_ucs_table`".
static GB18030_PUA_TBL3: [u16; 81] = [
    // 0xFE50
    0x0000, 0xE816, 0xE817, 0xE818, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0xE81E, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0xE826, 0x0000, 0x0000, 0x0000, 0x0000, 0xE82B, 0xE82C, //
    0x0000, 0x0000, 0x0000, 0x0000, 0xE831, 0xE832, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0xE83B, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0xE843, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0xE854, 0xE855, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    // 0xFEA0
    0xE864,
];

/// `gb18030_2022_pua_tbl3`: as [`GB18030_PUA_TBL3`], for GB18030-2022.
static GB18030_2022_PUA_TBL3: [u16; 81] = [
    // 0xFE50
    0x0000, 0xE816, 0xE817, 0xE818, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x9FB4, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x9FB5, 0x0000, 0x0000, 0x0000, 0x0000, 0x9FB6, 0x9FB7, //
    0x0000, 0x0000, 0x0000, 0x0000, 0xE831, 0x9FB8, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0xE83B, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x9FB9, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x9FBA, 0xE855, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    // 0xFEA0
    0x9FBB,
];

/// The 4-byte code for index `c1` of a GB18030 4-byte range starting at
/// lead byte `lead` (0x81 for the BMP, 0x90 for U+10000-U+10FFFF).
#[inline]
fn gb18030_4byte(mut c1: u32, lead: u32) -> u32 {
    let mut s = (c1 % 10) + 0x30;
    c1 /= 10;
    s |= ((c1 % 126) + 0x81) << 8;
    c1 /= 126;
    s |= ((c1 % 10) + 0x30) << 16;
    c1 /= 10;
    s | ((c1 + lead) << 24)
}

/// GB18030 user-defined areas 1-3 (U+E000-U+E765) to their 2-byte code.
#[inline]
fn gb18030_uda(w: u32) -> u32 {
    if w < 0xE4C6 {
        let mut c1 = w - 0xE000;
        let mut s = (c1 % 94) + 0xA1;
        c1 /= 94;
        s |= (c1 + if c1 < 0x06 { 0xAA } else { 0xF2 }) << 8;
        s
    } else {
        let mut c1 = w - 0xE4C6;
        let mut s = ((c1 / 96) + 0xA1) << 8;
        c1 %= 96;
        s |= c1 + if c1 >= 0x3F { 0x41 } else { 0x40 };
        s
    }
}

/// Binary search in a `{first, last, code}` PUA range table
/// (`mbfl_gb18030_pua_tbl` and friends); 0 if `w` is not covered.
#[inline]
fn pua_range_search(w: u32, tbl: &[[u16; 3]], max: u32) -> u32 {
    let (mut k1, mut k2) = (0usize, max as usize);
    while k1 < k2 {
        let k = (k1 + k2) >> 1;
        if w < tbl[k][0] as u32 {
            k2 = k;
        } else if w > tbl[k][1] as u32 {
            k1 = k + 1;
        } else {
            return w - tbl[k][0] as u32 + tbl[k][2] as u32;
        }
    }
    0
}

pub fn mb_gb18030_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if c == 0x80 || c == 0xFF {
            out[o] = BAD_INPUT;
        } else {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let (c, c2) = (c as u32, c2 as u32);

            if ((0x81..=0x84).contains(&c) || (0x90..=0xE3).contains(&c))
                && (0x30..=0x39).contains(&c2)
            {
                if p >= e {
                    out[o] = BAD_INPUT;
                    o += 1;
                    break;
                }
                let c3 = s[p] as u32;
                p += 1;

                if (0x81..=0xFE).contains(&c3) && p < e {
                    let c4 = s[p] as u32;
                    p += 1;

                    if (0x30..=0x39).contains(&c4) {
                        if (0x90..=0xE3).contains(&c) {
                            let w = (((c - 0x90) * 10 + (c2 - 0x30)) * 126 + (c3 - 0x81)) * 10
                                + (c4 - 0x30)
                                + 0x10000;
                            out[o] = if w > 0x10FFFF { BAD_INPUT } else { w };
                        } else {
                            // Unicode BMP
                            let w = (((c - 0x81) * 10 + (c2 - 0x30)) * 126 + (c3 - 0x81)) * 10
                                + (c4 - 0x30);
                            out[o] = if w <= 39419 {
                                let i = mbfl_bisec_srch(
                                    w as i32,
                                    &MBFL_GB2UNI_TBL,
                                    MBFL_GB_UNI_MAX as usize,
                                );
                                w + MBFL_GB_UNI_OFST[i as usize] as u32
                            } else {
                                BAD_INPUT
                            };
                        }
                    } else {
                        out[o] = BAD_INPUT;
                    }
                } else {
                    out[o] = BAD_INPUT;
                }
            } else if ((0xAA..=0xAF).contains(&c) || (0xF8..=0xFE).contains(&c))
                && (0xA1..=0xFE).contains(&c2)
            {
                // UDA part 1, 2: U+E000-U+E4C5
                out[o] = 94 * (if c >= 0xF8 { c - 0xF2 } else { c - 0xAA }) + (c2 - 0xA1) + 0xE000;
            } else if (0xA1..=0xA7).contains(&c) && (0x40..0xA1).contains(&c2) && c2 != 0x7F {
                // UDA part 3: U+E4C6-U+E765
                out[o] = 96 * (c - 0xA1) + c2 - (if c2 >= 0x80 { 0x41 } else { 0x40 }) + 0xE4C6;
            } else if c2 >= 0x40 && c2 != 0x7F && c2 != 0xFF {
                let w = (c - 0x81) * 192 + c2 - 0x40;

                if w >= 0x192B {
                    if w <= 0x1EBE {
                        if w != 0x1963
                            && w != 0x1DBF
                            && !(0x1E49..=0x1E55).contains(&w)
                            && w != 0x1E7F
                        {
                            out[o] = CP936_PUA_TBL1[(w - 0x192B) as usize] as u32;
                            o += 1;
                            continue;
                        }
                    } else if w >= 0x413A {
                        if w <= 0x413E {
                            out[o] = CP936_PUA_TBL2[(w - 0x413A) as usize] as u32;
                            o += 1;
                            continue;
                        } else if (0x5DD0..=0x5E20).contains(&w) {
                            let c = GB18030_PUA_TBL3[(w - 0x5DD0) as usize] as u32;
                            if c != 0 {
                                out[o] = c;
                                o += 1;
                                continue;
                            }
                        }
                    }
                }

                if (0x81..=0xA9).contains(&c)
                    || ((0xB0..=0xF7).contains(&c) && c2 >= 0xA1)
                    || ((0xAA..=0xFE).contains(&c) && c2 <= 0xA0)
                {
                    debug_assert!(w < CP936_UCS_TABLE_SIZE);
                    out[o] = CP936_UCS_TABLE[w as usize] as u32;
                } else {
                    out[o] = BAD_INPUT;
                }
            } else {
                out[o] = BAD_INPUT;
            }
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_gb18030(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s: u32 = 0;

        if w == 0 {
            buf.add(0);
            continue;
        } else if (UCS_A1_CP936_TABLE_MIN..UCS_A1_CP936_TABLE_MAX).contains(&w) {
            if w == 0x1F9 {
                s = 0xA8BF;
            } else {
                s = UCS_A1_CP936_TABLE[(w - UCS_A1_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A2_CP936_TABLE_MIN..UCS_A2_CP936_TABLE_MAX).contains(&w) {
            if w == 0x20AC {
                // Euro sign
                s = 0xA2E3;
            } else {
                s = UCS_A2_CP936_TABLE[(w - UCS_A2_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A3_CP936_TABLE_MIN..UCS_A3_CP936_TABLE_MAX).contains(&w) {
            s = UCS_A3_CP936_TABLE[(w - UCS_A3_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_I_CP936_TABLE_MIN..UCS_I_CP936_TABLE_MAX).contains(&w) {
            s = UCS_I_CP936_TABLE[(w - UCS_I_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_CI_CP936_TABLE_MIN..UCS_CI_CP936_TABLE_MAX).contains(&w) {
            // U+F900-U+FA2F CJK Compatibility Ideographs
            if w == 0xF92C {
                s = 0xFD9C;
            } else if w == 0xF979 {
                s = 0xFD9D;
            } else if w == 0xF995 {
                s = 0xFD9E;
            } else if w == 0xF9E7 {
                s = 0xFD9F;
            } else if w == 0xF9F1 {
                s = 0xFDA0;
            } else if (0xFA0C..=0xFA29).contains(&w) {
                s = UCS_CI_S_CP936_TABLE[(w - 0xFA0C) as usize] as u32;
            }
        } else if (UCS_CF_CP936_TABLE_MIN..UCS_CF_CP936_TABLE_MAX).contains(&w) {
            // CJK Compatibility Forms
            s = UCS_CF_CP936_TABLE[(w - UCS_CF_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_SFV_CP936_TABLE_MIN..UCS_SFV_CP936_TABLE_MAX).contains(&w) {
            // U+FE50-U+FE6F Small Form Variants
            s = UCS_SFV_CP936_TABLE[(w - UCS_SFV_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_HFF_CP936_TABLE_MIN..UCS_HFF_CP936_TABLE_MAX).contains(&w) {
            // U+FF00-U+FFFF HW/FW Forms
            if w == 0xFF04 {
                s = 0xA1E7;
            } else if w == 0xFF5E {
                s = 0xA1AB;
            } else if (0xFF01..=0xFF5D).contains(&w) {
                s = w - 0xFF01 + 0xA3A1;
            } else if (0xFFE0..=0xFFE5).contains(&w) {
                s = UCS_HFF_S_CP936_TABLE[(w - 0xFFE0) as usize] as u32;
            }
        } else if (0xE000..=0xE864).contains(&w) {
            // PUA
            if w < 0xE766 {
                s = gb18030_uda(w);
            } else {
                // U+E766-U+E864
                s = pua_range_search(w, &MBFL_GB18030_PUA_TBL, MBFL_GB18030_PUA_TBL_MAX);
            }
        }

        s = gb18030_common_tail(w, s, &MBFL_UNI2GB_TBL, &MBFL_GB_UNI_OFST, MBFL_GB_UNI_MAX);

        if s == 0 {
            mb_illegal_output(w, mb_wchar_to_gb18030, buf);
        } else if s < 0x80 {
            buf.add(s as u8);
        } else if s > 0xFFFFFF {
            buf.add4((s >> 24) as u8, (s >> 16) as u8, (s >> 8) as u8, s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

/// The part of `mb_wchar_to_gb18030` (and its GB18030-2022 twin) after the
/// table lookups: codepoints mapped differently from CP936, then 4-byte
/// codes for the BMP (through `uni2gb`/`ofst`) and U+10000-U+10FFFF.
#[inline]
fn gb18030_common_tail(w: u32, mut s: u32, uni2gb: &[u16], ofst: &[u16], uni_max: u32) -> u32 {
    // While GB18030 and CP936 are very similar, some mappings are different
    // between these encodings; look for one in a table of differing codepoints.
    let n = MBFL_GB18030_C_TBL_MAX as usize;
    if s == 0 && w >= MBFL_GB18030_C_TBL_KEY[0] as u32 && w <= MBFL_GB18030_C_TBL_KEY[n - 1] as u32
    {
        let i = mbfl_bisec_srch2(w as i32, &MBFL_GB18030_C_TBL_KEY, n);
        if i >= 0 {
            s = MBFL_GB18030_C_TBL_VAL[i as usize] as u32;
        }
    }

    // A codepoint without a mapping yet requires a 4-byte code.
    if s == 0 && (0x80..=0xFFFF).contains(&w) {
        // BMP
        let i = mbfl_bisec_srch(w as i32, uni2gb, uni_max as usize);
        if i >= 0 {
            s = gb18030_4byte(w.wrapping_sub(ofst[i as usize] as u32), 0x81);
        }
    } else if (0x10000..=0x10FFFF).contains(&w) {
        // Code set 3: Unicode U+10000-U+10FFFF
        s = gb18030_4byte(w - 0x10000, 0x90);
    }
    s
}

pub fn mb_cp936_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if c == 0x80 {
            // Euro sign
            out[o] = 0x20AC;
        } else if c < 0xFF {
            if p >= e {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            let c2 = s[p];
            p += 1;
            if c2 < 0x40 || c2 == 0x7F || c2 == 0xFF {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }
            let (c, c2) = (c as u32, c2 as u32);

            if ((0xAA..=0xAF).contains(&c) || (0xF8..=0xFE).contains(&c)) && c2 >= 0xA1 {
                // UDA part 1, 2: U+E000-U+E4C5
                out[o] = 94 * (if c >= 0xF8 { c - 0xF2 } else { c - 0xAA }) + (c2 - 0xA1) + 0xE000;
            } else if (0xA1..=0xA7).contains(&c) && c2 < 0xA1 {
                // UDA part 3: U+E4C6-U+E765
                out[o] = 96 * (c - 0xA1) + c2 - (if c2 >= 0x80 { 0x41 } else { 0x40 }) + 0xE4C6;
            } else {
                // Convert c, c2 into a GB 2312 table lookup index.
                let w = (c - 0x81) * 192 + c2 - 0x40;

                // For CP936 and GB18030, certain GB 2312 byte combinations are mapped to PUA
                // codepoints, whereas the same combinations aren't mapped to any codepoint for
                // HZ and EUC-CN; three auxiliary tables cover those lookup index ranges.
                if w >= 0x192B {
                    if w <= 0x1EBE {
                        out[o] = CP936_PUA_TBL1[(w - 0x192B) as usize] as u32;
                        o += 1;
                        continue;
                    } else if w >= 0x413A {
                        if w <= 0x413E {
                            out[o] = CP936_PUA_TBL2[(w - 0x413A) as usize] as u32;
                            o += 1;
                            continue;
                        } else if (0x5DD0..=0x5E20).contains(&w) {
                            out[o] = CP936_PUA_TBL3[(w - 0x5DD0) as usize] as u32;
                            o += 1;
                            continue;
                        }
                    }
                }

                debug_assert!(w < CP936_UCS_TABLE_SIZE);
                out[o] = CP936_UCS_TABLE[w as usize] as u32;
            }
        } else {
            out[o] = 0xF8F5;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_cp936(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s: u32 = 0;

        if (UCS_A1_CP936_TABLE_MIN..UCS_A1_CP936_TABLE_MAX).contains(&w) {
            // U+0000-U+0451
            s = UCS_A1_CP936_TABLE[(w - UCS_A1_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_A2_CP936_TABLE_MIN..UCS_A2_CP936_TABLE_MAX).contains(&w) {
            // U+2000-U+26FF
            if w == 0x203E {
                s = 0xA3FE;
            } else if w == 0x2218 {
                s = 0xA1E3;
            } else if w == 0x223C {
                s = 0xA1AB;
            } else {
                s = UCS_A2_CP936_TABLE[(w - UCS_A2_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A3_CP936_TABLE_MIN..UCS_A3_CP936_TABLE_MAX).contains(&w) {
            // U+2F00-U+33FF
            s = UCS_A3_CP936_TABLE[(w - UCS_A3_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_I_CP936_TABLE_MIN..UCS_I_CP936_TABLE_MAX).contains(&w) {
            // U+4D00-9FFF CJK Unified Ideographs (+ Extension A)
            s = UCS_I_CP936_TABLE[(w - UCS_I_CP936_TABLE_MIN) as usize] as u32;
        } else if (0xE000..=0xE864).contains(&w) {
            // PUA
            if w < 0xE766 {
                s = gb18030_uda(w);
            } else {
                // U+E766-U+E864
                s = pua_range_search(w, &MBFL_CP936_PUA_TBL, MBFL_CP936_PUA_TBL_MAX);
            }
        } else if w == 0xF8F5 {
            s = 0xFF;
        } else if (UCS_CI_CP936_TABLE_MIN..UCS_CI_CP936_TABLE_MAX).contains(&w) {
            // U+F900-U+FA2F CJK Compatibility Ideographs
            s = UCS_CI_CP936_TABLE[(w - UCS_CI_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_CF_CP936_TABLE_MIN..UCS_CF_CP936_TABLE_MAX).contains(&w) {
            s = UCS_CF_CP936_TABLE[(w - UCS_CF_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_SFV_CP936_TABLE_MIN..UCS_SFV_CP936_TABLE_MAX).contains(&w) {
            // U+FE50-U+FE6F Small Form Variants
            s = UCS_SFV_CP936_TABLE[(w - UCS_SFV_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_HFF_CP936_TABLE_MIN..UCS_HFF_CP936_TABLE_MAX).contains(&w) {
            // U+FF00-U+FFFF HW/FW Forms
            if w == 0xFF04 {
                s = 0xA1E7;
            } else if w == 0xFF5E {
                s = 0xA1AB;
            } else if (0xFF01..=0xFF5D).contains(&w) {
                s = w - 0xFF01 + 0xA3A1;
            } else if (0xFFE0..=0xFFE5).contains(&w) {
                s = UCS_HFF_S_CP936_TABLE[(w - 0xFFE0) as usize] as u32;
            }
        }

        if s == 0 {
            if w == 0 {
                buf.add(0);
            } else {
                mb_illegal_output(w, mb_wchar_to_cp936, buf);
            }
        } else if s <= 0x80 || s == 0xFF {
            buf.add(s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

pub fn mb_gb18030_2022_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if c == 0x80 || c == 0xFF {
            out[o] = BAD_INPUT;
        } else {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let (c, c2) = (c as u32, c2 as u32);

            if ((0x81..=0x84).contains(&c) || (0x90..=0xE3).contains(&c))
                && (0x30..=0x39).contains(&c2)
            {
                if p >= e {
                    out[o] = BAD_INPUT;
                    o += 1;
                    break;
                }
                let c3 = s[p] as u32;
                p += 1;

                if (0x81..=0xFE).contains(&c3) && p < e {
                    let c4 = s[p] as u32;
                    p += 1;

                    if (0x30..=0x39).contains(&c4) {
                        if (0x90..=0xE3).contains(&c) {
                            let w = (((c - 0x90) * 10 + (c2 - 0x30)) * 126 + (c3 - 0x81)) * 10
                                + (c4 - 0x30)
                                + 0x10000;
                            out[o] = if w > 0x10FFFF { BAD_INPUT } else { w };
                        } else {
                            // Unicode BMP
                            let w = (((c - 0x81) * 10 + (c2 - 0x30)) * 126 + (c3 - 0x81)) * 10
                                + (c4 - 0x30);
                            out[o] = if w == 0x98A4 {
                                0xE78D
                            } else if w == 0x98A6 {
                                0xE78E
                            } else if w == 0x98A5 {
                                0xE78F
                            } else if (0x98A7..=0x98AD).contains(&w) {
                                w + (0xE790 - 0x98A7)
                            } else if w == 0x1D21 {
                                0xE7C7
                            } else if w == 0x4A71 {
                                0xE81E
                            } else if w == 0x4A72 {
                                0xE826
                            } else if (0x4A73..=0x4A74).contains(&w) {
                                w + (0xE82B - 0x4A73)
                            } else if w == 0x4A75 {
                                0xE832
                            } else if w == 0x4A76 {
                                0xE843
                            } else if w == 0x4A77 {
                                0xE854
                            } else if w == 0x4A78 {
                                0xE864
                            } else if w <= 0x99FB {
                                let i = mbfl_bisec_srch(
                                    w as i32,
                                    &MBFL_GB2UNI_TBL,
                                    MBFL_GB_UNI_MAX as usize,
                                );
                                w + MBFL_GB_UNI_OFST[i as usize] as u32
                            } else {
                                BAD_INPUT
                            };
                        }
                    } else {
                        out[o] = BAD_INPUT;
                    }
                } else {
                    out[o] = BAD_INPUT;
                }
            } else if ((0xAA..=0xAF).contains(&c) || (0xF8..=0xFE).contains(&c))
                && (0xA1..=0xFE).contains(&c2)
            {
                // UDA part 1, 2: U+E000-U+E4C5
                out[o] = 94 * (if c >= 0xF8 { c - 0xF2 } else { c - 0xAA }) + (c2 - 0xA1) + 0xE000;
            } else if (0xA1..=0xA7).contains(&c) && (0x40..0xA1).contains(&c2) && c2 != 0x7F {
                // UDA part 3: U+E4C6-U+E765
                out[o] = 96 * (c - 0xA1) + c2 - (if c2 >= 0x80 { 0x41 } else { 0x40 }) + 0xE4C6;
            } else if c2 >= 0x40 && c2 != 0x7F && c2 != 0xFF {
                let w = (c - 0x81) * 192 + c2 - 0x40;

                if w >= 0x192B {
                    if w <= 0x1EBE {
                        if w != 0x1963
                            && w != 0x1DBF
                            && !(0x1E49..=0x1E55).contains(&w)
                            && w != 0x1E7F
                        {
                            out[o] = GB18030_2022_PUA_TBL1[(w - 0x192B) as usize] as u32;
                            o += 1;
                            continue;
                        }
                    } else if w >= 0x413A {
                        if w <= 0x413E {
                            out[o] = CP936_PUA_TBL2[(w - 0x413A) as usize] as u32;
                            o += 1;
                            continue;
                        } else if (0x5DD0..=0x5E20).contains(&w) {
                            let c = GB18030_2022_PUA_TBL3[(w - 0x5DD0) as usize] as u32;
                            if c != 0 {
                                out[o] = c;
                                o += 1;
                                continue;
                            }
                        }
                    }
                }

                if (0x81..=0xA9).contains(&c)
                    || ((0xB0..=0xF7).contains(&c) && c2 >= 0xA1)
                    || ((0xAA..=0xFE).contains(&c) && c2 <= 0xA0)
                {
                    debug_assert!(w < CP936_UCS_TABLE_SIZE);
                    out[o] = CP936_UCS_TABLE[w as usize] as u32;
                } else {
                    out[o] = BAD_INPUT;
                }
            } else {
                out[o] = BAD_INPUT;
            }
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_gb18030_2022(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s: u32 = 0;

        if w == 0 {
            buf.add(0);
            continue;
        } else if (UCS_A1_CP936_TABLE_MIN..UCS_A1_CP936_TABLE_MAX).contains(&w) {
            if w == 0x1F9 {
                s = 0xA8BF;
            } else {
                s = UCS_A1_CP936_TABLE[(w - UCS_A1_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A2_CP936_TABLE_MIN..UCS_A2_CP936_TABLE_MAX).contains(&w) {
            if w == 0x20AC {
                // Euro sign
                s = 0xA2E3;
            } else {
                s = UCS_A2_CP936_TABLE[(w - UCS_A2_CP936_TABLE_MIN) as usize] as u32;
            }
        } else if (UCS_A3_CP936_TABLE_MIN..UCS_A3_CP936_TABLE_MAX).contains(&w) {
            s = UCS_A3_CP936_TABLE[(w - UCS_A3_CP936_TABLE_MIN) as usize] as u32;
        } else if (0x9FB4..=0x9FBB).contains(&w) {
            // Newly mapped in GB18030-2022
            s = match w {
                0x9FB4 => 0xFE59,
                0x9FB5 => 0xFE61,
                0x9FB6 => 0xFE66,
                0x9FB7 => 0xFE67,
                0x9FB8 => 0xFE6D,
                0x9FB9 => 0xFE7E,
                0x9FBA => 0xFE90,
                _ => 0xFEA0,
            };
        } else if (UCS_I_CP936_TABLE_MIN..UCS_I_CP936_TABLE_MAX).contains(&w) {
            s = UCS_I_CP936_TABLE[(w - UCS_I_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_CI_CP936_TABLE_MIN..UCS_CI_CP936_TABLE_MAX).contains(&w) {
            // U+F900-U+FA2F CJK Compatibility Ideographs
            if w == 0xF92C {
                s = 0xFD9C;
            } else if w == 0xF979 {
                s = 0xFD9D;
            } else if w == 0xF995 {
                s = 0xFD9E;
            } else if w == 0xF9E7 {
                s = 0xFD9F;
            } else if w == 0xF9F1 {
                s = 0xFDA0;
            } else if (0xFA0C..=0xFA29).contains(&w) {
                s = UCS_CI_S_CP936_TABLE[(w - 0xFA0C) as usize] as u32;
            }
        } else if (UCS_CF_CP936_TABLE_MIN..UCS_CF_CP936_TABLE_MAX).contains(&w) {
            // CJK Compatibility Forms
            s = UCS_CF_CP936_TABLE[(w - UCS_CF_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_SFV_CP936_TABLE_MIN..UCS_SFV_CP936_TABLE_MAX).contains(&w) {
            // U+FE50-U+FE6F Small Form Variants
            s = UCS_SFV_CP936_TABLE[(w - UCS_SFV_CP936_TABLE_MIN) as usize] as u32;
        } else if (UCS_HFF_CP936_TABLE_MIN..UCS_HFF_CP936_TABLE_MAX).contains(&w) {
            // U+FF00-U+FFFF HW/FW Forms
            if w == 0xFF04 {
                s = 0xA1E7;
            } else if w == 0xFF5E {
                s = 0xA1AB;
            } else if (0xFF01..=0xFF5D).contains(&w) {
                s = w - 0xFF01 + 0xA3A1;
            } else if (0xFFE0..=0xFFE5).contains(&w) {
                s = UCS_HFF_S_CP936_TABLE[(w - 0xFFE0) as usize] as u32;
            }
        } else if (0xE000..=0xE864).contains(&w) {
            // PUA
            if w < 0xE766 {
                s = gb18030_uda(w);
            } else {
                // U+E766-U+E864
                s = pua_range_search(w, &MBFL_GB18030_2022_PUA_TBL, MBFL_GB18030_2022_PUA_TBL_MAX);
            }
        } else if (0xFE10..=0xFE19).contains(&w) {
            // Newly mapped codepoints in GB18030-2022
            if w == 0xFE11 {
                s = 0xA6DB;
            } else if w == 0xFE12 {
                s = 0xA6DA;
            } else if w <= 0xFE16 {
                s = w - (0xFE10 - 0xA6D9);
            } else if w <= 0xFE18 {
                s = w - (0xFE17 - 0xA6EC);
            } else {
                s = 0xA6F3;
            }
        } else if w == 0x1E3F {
            // Newly mapped codepoint in GB18030-2022
            s = 0xA8BC;
        }

        s = gb18030_common_tail(
            w,
            s,
            &MBFL_UNI2GB2022_TBL,
            &MBFL_GB2022_UNI_OFST,
            MBFL_GB2022_UNI_MAX,
        );

        if s == 0 {
            // php-src passes the GB18030 (2000) encoder for the error marker.
            mb_illegal_output(w, mb_wchar_to_gb18030, buf);
        } else if s < 0x80 {
            buf.add(s as u8);
        } else if s > 0xFFFFFF {
            buf.add4((s >> 24) as u8, (s >> 16) as u8, (s >> 8) as u8, s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

/// `step_through_gb18030_str`: steps through a GB18030 string one character
/// at a time from `p`; returns the last position at or before `limit` which
/// falls directly after the end of a (single or multi-byte) character.
#[inline]
fn step_through_gb18030_str(s: &[u8], mut p: usize, limit: usize) -> usize {
    while p < limit {
        let c = s[p];
        if c < 0x81 || c == 0xFF {
            p += 1;
        } else {
            if limit - p == 1 {
                break;
            }
            let c2 = s[p + 1];
            // For a 4-byte char, the 2nd byte will be 0x30-0x39.
            let w = if (0x30..=0x39).contains(&c2) { 4 } else { 2 };
            if limit - p < w {
                break;
            }
            p += w;
        }
    }
    p
}

pub fn mb_cut_gb18030(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    let end = s.len();
    debug_assert!(from <= end);
    let from = from.min(end);
    let start = step_through_gb18030_str(s, 0, from);
    let mut len = len;
    if from.saturating_add(len) > end {
        len = end - from;
    }
    if start + len >= end {
        s[start..].to_vec()
    } else {
        let cut_end = step_through_gb18030_str(s, start, start + len);
        s[start..cut_end].to_vec()
    }
}

/*
 * BIG5/CP950
 */

/// `cp950_pua_tbl`: `{first codepoint, last codepoint, first code, last code}`;
/// rows with 63 + 94 = 157 or 94 codes per lead byte.
static CP950_PUA_TBL: [[u16; 4]; 5] = [
    [0xE000, 0xE310, 0xFA40, 0xFEFE],
    [0xE311, 0xEEB7, 0x8E40, 0xA0FE],
    [0xEEB8, 0xF6B0, 0x8140, 0x8DFE],
    [0xF6B1, 0xF70E, 0xC6A1, 0xC6FE],
    [0xF70F, 0xF848, 0xC740, 0xC8FE],
];

#[inline]
fn is_in_cp950_pua(c1: u32, c: u32) -> bool {
    if (0xFA..=0xFE).contains(&c1)
        || (0x8E..=0xA0).contains(&c1)
        || (0x81..=0x8D).contains(&c1)
        || (0xC7..=0xC8).contains(&c1)
    {
        return (0x40..=0x7E).contains(&c) || (0xA1..=0xFE).contains(&c);
    } else if c1 == 0xC6 {
        return (0xA1..=0xFE).contains(&c);
    }
    false
}

pub fn mb_big5_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
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

        out[o] = if c <= 0x7F {
            c as u32
        } else if c > 0xA0 && c <= 0xF9 {
            let c2 = s[p];
            p += 1;

            if (0x40..=0x7E).contains(&c2) || (0xA1..=0xFE).contains(&c2) {
                let w = (c as u32 - 0xA1) * 157 + c2 as u32
                    - (if c2 <= 0x7E { 0x40 } else { 0xA1 - 0x3F });
                debug_assert!(w < BIG5_UCS_TABLE_SIZE);
                let w = BIG5_UCS_TABLE[w as usize] as u32;
                if w == 0 {
                    if c == 0xC8 {
                        p -= 1;
                    }
                    BAD_INPUT
                } else {
                    w
                }
            } else {
                BAD_INPUT
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
        out[o] = if c <= 0x7F { c as u32 } else { BAD_INPUT };
        o += 1;
    }

    *input = &s[p..];
    o
}

/// The Unicode -> Big5 table lookups shared by BIG-5 and CP950.
#[inline]
fn big5_lookup(w: u32) -> u32 {
    if (UCS_A1_BIG5_TABLE_MIN..UCS_A1_BIG5_TABLE_MAX).contains(&w) {
        UCS_A1_BIG5_TABLE[(w - UCS_A1_BIG5_TABLE_MIN) as usize] as u32
    } else if (UCS_A2_BIG5_TABLE_MIN..UCS_A2_BIG5_TABLE_MAX).contains(&w) {
        UCS_A2_BIG5_TABLE[(w - UCS_A2_BIG5_TABLE_MIN) as usize] as u32
    } else if (UCS_A3_BIG5_TABLE_MIN..UCS_A3_BIG5_TABLE_MAX).contains(&w) {
        UCS_A3_BIG5_TABLE[(w - UCS_A3_BIG5_TABLE_MIN) as usize] as u32
    } else if (UCS_I_BIG5_TABLE_MIN..UCS_I_BIG5_TABLE_MAX).contains(&w) {
        UCS_I_BIG5_TABLE[(w - UCS_I_BIG5_TABLE_MIN) as usize] as u32
    } else if (UCS_R1_BIG5_TABLE_MIN..UCS_R1_BIG5_TABLE_MAX).contains(&w) {
        UCS_R1_BIG5_TABLE[(w - UCS_R1_BIG5_TABLE_MIN) as usize] as u32
    } else if (UCS_R2_BIG5_TABLE_MIN..UCS_R2_BIG5_TABLE_MAX).contains(&w) {
        UCS_R2_BIG5_TABLE[(w - UCS_R2_BIG5_TABLE_MIN) as usize] as u32
    } else {
        0
    }
}

pub fn mb_wchar_to_big5(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let s = big5_lookup(w);

        if s == 0 {
            if w == 0 {
                buf.add(0);
            } else {
                mb_illegal_output(w, mb_wchar_to_big5, buf);
            }
        } else if s <= 0x80 {
            buf.add(s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

pub fn mb_cp950_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        if c <= 0x7F {
            out[o] = c as u32;
        } else if c > 0x80 && c <= 0xFE && p < e {
            let c2 = s[p];
            p += 1;
            let (c, c2) = (c as u32, c2 as u32);

            if (0x40..=0x7E).contains(&c2) || (0xA1..=0xFE).contains(&c2) {
                // Lead bytes below 0xA1 make this negative in C; as unsigned it
                // is then out of the table's range.
                let mut w = ((c as i32 - 0xA1) * 157 + c2 as i32
                    - (if c2 <= 0x7E { 0x40 } else { 0xA1 - 0x3F }))
                    as u32;
                w = if w < BIG5_UCS_TABLE_SIZE {
                    BIG5_UCS_TABLE[w as usize] as u32
                } else {
                    0
                };

                // PUA for CP950
                if is_in_cp950_pua(c, c2) {
                    let s2 = (c << 8) | c2;

                    let mut k = 0;
                    while k < CP950_PUA_TBL.len() {
                        if s2 >= CP950_PUA_TBL[k][2] as u32 && s2 <= CP950_PUA_TBL[k][3] as u32 {
                            break;
                        }
                        k += 1;
                    }
                    let row = CP950_PUA_TBL[k];

                    if (row[2] & 0xFF) == 0x40 {
                        w = 157 * (c - (row[2] >> 8) as u32) + c2
                            - (if c2 >= 0xA1 { 0x62 } else { 0x40 })
                            + row[0] as u32;
                    } else {
                        w = s2 - row[2] as u32 + row[0] as u32;
                    }
                } else if c == 0xA1 {
                    match c2 {
                        0x45 => w = 0x2027,
                        0x4E => w = 0xFE51,
                        0x5A => w = 0x2574,
                        0xC2 => w = 0x00AF,
                        0xC3 => w = 0xFFE3,
                        0xC5 => w = 0x02CD,
                        0xE3 => w = 0xFF5E,
                        0xF2 => w = 0x2295,
                        0xF3 => w = 0x2299,
                        0xFE => w = 0xFF0F,
                        _ => {}
                    }
                } else if c == 0xA2 {
                    match c2 {
                        0x40 => w = 0xFF3C,
                        0x41 => w = 0x2215,
                        0x42 => w = 0xFE68,
                        0x46 => w = 0xFFE0,
                        0x47 => w = 0xFFE1,
                        0xCC => w = 0x5341,
                        0xCE => w = 0x5345,
                        _ => {}
                    }
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

pub fn mb_wchar_to_cp950(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s = big5_lookup(w);

        if (0xE000..=0xF848).contains(&w) {
            let mut k = 0;
            while k < CP950_PUA_TBL.len() {
                if w <= CP950_PUA_TBL[k][1] as u32 {
                    break;
                }
                k += 1;
            }
            let row = CP950_PUA_TBL[k];

            let mut c1 = w - row[0] as u32;
            if (row[2] & 0xFF) == 0x40 {
                let c2 = (row[2] >> 8) as u32;
                s = ((c1 / 157) + c2) << 8;
                c1 %= 157;
                s |= c1 + if c1 >= 0x3F { 0x62 } else { 0x40 };
            } else {
                s = c1 + row[2] as u32;
            }
        } else if w == 0xA2
            || w == 0xA3
            || w == 0x401
            || (0x414..=0x41C).contains(&w)
            || (0x423..=0x44F).contains(&w)
            || w == 0x451
            || w == 0x2022
            || w == 0x203E
            || w == 0x223C
            || (0x2460..=0x247D).contains(&w)
            || w == 0x2609
            || w == 0x2641
            || w == 0x3005
            || (0x302A..=0x30FF).contains(&w)
            || w == 0xFF64
        {
            s = 0;
        } else {
            match w {
                0xAF => s = 0xA1C2,
                0x2CD => s = 0xA1C5,
                0x2027 => s = 0xA145,
                0x2215 => s = 0xA241,
                0x2295 => s = 0xA1F2,
                0x2299 => s = 0xA1F3,
                0x2574 => s = 0xA15A,
                0xFE51 => s = 0xA14E,
                0xFE68 => s = 0xA242,
                0xFF3C => s = 0xA240,
                0xFF5E => s = 0xA1E3,
                0xFFE0 => s = 0xA246,
                0xFFE1 => s = 0xA247,
                0xFFE3 => s = 0xA1C3,
                0xFF0F => s = 0xA1FE,
                _ => {}
            }
        }

        if s == 0 {
            if w == 0 {
                buf.add(0);
            } else {
                // php-src passes the BIG-5 encoder for the error marker.
                mb_illegal_output(w, mb_wchar_to_big5, buf);
            }
        } else if s <= 0x80 {
            buf.add(s as u8);
        } else {
            buf.add2((s >> 8) as u8, s as u8);
        }
    }
}

/*
 * HZ
 */

/// The GB 2312 (EUC-CN) lookup index for HZ bytes `c1`, `c2` to a codepoint,
/// with the HZ-specific exclusions; 0 if unmapped.
#[inline]
fn hz_gb2312_to_ucs(c1: u32, c2: u32) -> u32 {
    let s = (c1 - 1) * 192 + c2 + 0x40;
    debug_assert!(s < CP936_UCS_TABLE_SIZE);
    if s == 0x1864 {
        0x30FB
    } else if s == 0x186A {
        0x2015
    } else if s == 0x186C {
        0x2225
    } else if (0x1920..=0x192A).contains(&s)
        || s == 0x1963
        || (0x1C60..=0x1C7F).contains(&s)
        || (0x1DBB..=0x1DC4).contains(&s)
    {
        0
    } else {
        CP936_UCS_TABLE[s as usize] as u32
    }
}

/// The Unicode -> GB 2312 lookups of HZ (`mb_wchar_to_hz` and
/// `mbfl_filt_conv_wchar_hz`): the EUC-CN code, before masking off the high
/// bits, or 0.
#[inline]
fn hz_ucs_to_gb2312(w: u32) -> u32 {
    let mut s = 0;
    if (UCS_A1_CP936_TABLE_MIN..UCS_A1_CP936_TABLE_MAX).contains(&w) {
        if !(w == 0xB7
            || w == 0x144
            || w == 0x148
            || w == 0x251
            || w == 0x261
            || w == 0x2CA
            || w == 0x2CB
            || w == 0x2D9)
        {
            s = UCS_A1_CP936_TABLE[(w - UCS_A1_CP936_TABLE_MIN) as usize] as u32;
        }
    } else if (UCS_A2_CP936_TABLE_MIN..UCS_A2_CP936_TABLE_MAX).contains(&w) {
        if w == 0x2015 {
            s = 0xA1AA;
        } else if !(w == 0x2010
            || w == 0x2013
            || w == 0x2014
            || w == 0x2016
            || w == 0x2025
            || w == 0x2035
            || w == 0x2105
            || w == 0x2109
            || w == 0x2121
            || (0x2170..=0x2179).contains(&w)
            || (0x2196..=0x2199).contains(&w)
            || w == 0x2215
            || w == 0x221F
            || w == 0x2223
            || w == 0x2252
            || w == 0x2266
            || w == 0x2267
            || w == 0x2295
            || (0x2550..=0x2573).contains(&w)
            || w == 0x22BF
            || w == 0x2609
            || (0x2581..=0x258F).contains(&w)
            || (0x2593..=0x2595).contains(&w)
            || w == 0x25BC
            || w == 0x25BD
            || (0x25E2..=0x25E5).contains(&w))
        {
            s = UCS_A2_CP936_TABLE[(w - UCS_A2_CP936_TABLE_MIN) as usize] as u32;
        }
    } else if (UCS_A3_CP936_TABLE_MIN..UCS_A3_CP936_TABLE_MAX).contains(&w) {
        if w == 0x30FB {
            s = 0xA1A4;
        } else if !(w == 0x3006
            || w == 0x3007
            || w == 0x3012
            || w == 0x3231
            || w == 0x32A3
            || w >= 0x3300
            || (0x3018..=0x3040).contains(&w)
            || (0x309B..=0x309E).contains(&w)
            || (0x30FC..=0x30FE).contains(&w))
        {
            s = UCS_A3_CP936_TABLE[(w - UCS_A3_CP936_TABLE_MIN) as usize] as u32;
        }
    } else if (UCS_I_GB2312_TABLE_MIN..UCS_I_GB2312_TABLE_MAX).contains(&w) {
        s = UCS_I_GB2312_TABLE[(w - UCS_I_GB2312_TABLE_MIN) as usize] as u32;
    } else if (UCS_HFF_CP936_TABLE_MIN..UCS_HFF_CP936_TABLE_MAX).contains(&w) {
        if w == 0xFF04 {
            s = 0xA1E7;
        } else if w == 0xFF5E {
            s = 0xA1AB;
        } else if (0xFF01..=0xFF5D).contains(&w) {
            s = w - 0xFF01 + 0xA3A1;
        } else if w == 0xFFE0 || w == 0xFFE1 || w == 0xFFE3 || w == 0xFFE5 {
            s = UCS_HFF_S_CP936_TABLE[(w - 0xFFE0) as usize] as u32;
        }
    }
    s
}

pub fn mbfl_filt_conv_hz_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    match f.status & 0xF {
        // case 0x00: ASCII
        // case 0x10: GB2312
        0 => {
            if c == b'~' as i32 {
                f.status += 2;
            } else if f.status == 0x10 && ((c > 0x20 && c <= 0x29) || (0x30..=0x77).contains(&c)) {
                // DBCS first char
                f.cache = c;
                f.status += 1;
            } else if f.status == 0 && (0..0x80).contains(&c) {
                // latin, CTLs
                ck!(out.output(c));
            } else {
                ck!(out.output(FILTER_BAD_INPUT));
            }
        }

        // case 0x11: GB2312 second char
        1 => {
            f.status &= !0xF;
            let c1 = f.cache;
            if c1 > 0x20 && c1 < 0x7F && c > 0x20 && c < 0x7F {
                let w = hz_gb2312_to_ucs(c1 as u32, c as u32) as i32;
                ck!(out.output(if w <= 0 { FILTER_BAD_INPUT } else { w }));
            } else {
                ck!(out.output(FILTER_BAD_INPUT));
            }
        }

        // '~'
        2 => {
            if c == b'}' as i32 && f.status == 0x12 {
                f.status = 0;
            } else if c == b'{' as i32 && f.status == 2 {
                f.status = 0x10;
            } else if c == b'~' as i32 && f.status == 2 {
                ck!(out.output(b'~' as i32));
                f.status -= 2;
            } else if c == b'\n' as i32 {
                // "~\n" is a line continuation; no output is needed, nor
                // should we shift modes.
                f.status -= 2;
            } else {
                // Invalid character after ~
                f.status -= 2;
                ck!(out.output(FILTER_BAD_INPUT));
            }
        }

        _ => unreachable!("invalid HZ decoder status {:#x}", f.status),
    }

    0
}

pub fn mbfl_filt_conv_hz_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status == 0x11 {
        // 2-byte character was truncated
        ck!(out.output(FILTER_BAD_INPUT));
    }

    f.status = 0;
    out.flush();
    0
}

pub fn mbfl_filt_conv_wchar_hz(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut s = if c < 0 {
        0
    } else {
        hz_ucs_to_gb2312(c as u32) as i32
    };

    if s & 0x8000 != 0 {
        s -= 0x8080;
    }

    if s <= 0 {
        s = if c == 0 { 0 } else { -1 };
    } else if (0x80..0x2121).contains(&s) || s > 0x8080 {
        s = -1;
    }

    if s >= 0 {
        if s < 0x80 {
            // ASCII
            if (f.status & 0xFF00) != 0 {
                ck!(out.output(b'~' as i32));
                ck!(out.output(b'}' as i32));
            }
            f.status = 0;
            if s == 0x7E {
                ck!(out.output(b'~' as i32));
            }
            ck!(out.output(s));
        } else {
            // GB 2312-80
            if (f.status & 0xFF00) != 0x200 {
                ck!(out.output(b'~' as i32));
                ck!(out.output(b'{' as i32));
            }
            f.status = 0x200;
            ck!(out.output((s >> 8) & 0x7F));
            ck!(out.output(s & 0x7F));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

/// Unlike most flush functions, php-src's does not flush the next filter.
pub fn mbfl_filt_conv_any_hz_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // back to latin
    if (f.status & 0xFF00) != 0 {
        ck!(out.output(b'~' as i32));
        ck!(out.output(b'}' as i32));
    }
    f.status = 0;
    0
}

const HZ_ASCII: u32 = 0;
const HZ_GB2312: u32 = 1;

pub fn mb_hz_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < out.len() {
        let c = s[p];
        p += 1;

        if c == b'~' {
            if p == e {
                break;
            }
            let c2 = s[p];
            p += 1;

            if c2 == b'}' && *state == HZ_GB2312 {
                *state = HZ_ASCII;
            } else if c2 == b'{' && *state == HZ_ASCII {
                *state = HZ_GB2312;
            } else if c2 == b'~' && *state == HZ_ASCII {
                out[o] = b'~' as u32;
                o += 1;
            } else if c2 == b'\n' {
                // "~\n" is a line continuation; no output is needed, nor
                // should we shift modes.
            } else {
                // Invalid character after ~
                out[o] = BAD_INPUT;
                o += 1;
            }
        } else if ((c > 0x20 && c <= 0x29) || (0x30..=0x77).contains(&c))
            && p < e
            && *state == HZ_GB2312
        {
            let c2 = s[p];
            p += 1;

            out[o] = if c > 0x20 && c < 0x7F && c2 > 0x20 && c2 < 0x7F {
                let w = hz_gb2312_to_ucs(c as u32, c2 as u32);
                if w == 0 { BAD_INPUT } else { w }
            } else {
                BAD_INPUT
            };
            o += 1;
        } else if c < 0x80 && *state == HZ_ASCII {
            out[o] = c as u32;
            o += 1;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_hz(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let s = hz_ucs_to_gb2312(w) & !0x8080;

        if (s == 0 && w != 0) || (0x80..0x2121).contains(&s) {
            mb_illegal_output(w, mb_wchar_to_hz, buf);
        } else if s < 0x80 {
            // ASCII
            if buf.state != HZ_ASCII {
                buf.add2(b'~', b'}');
                buf.state = HZ_ASCII;
            }
            if s == b'~' as u32 {
                buf.add2(b'~', b'~');
            } else {
                buf.add(s as u8);
            }
        } else {
            // GB 2312-80
            if buf.state != HZ_GB2312 {
                buf.add2(b'~', b'{');
                buf.state = HZ_GB2312;
            }
            buf.add2(((s >> 8) & 0x7F) as u8, (s & 0x7F) as u8);
        }
    }

    if end && buf.state != HZ_ASCII {
        // If not in ASCII state, need to emit closing control chars.
        buf.add2(b'~', b'}');
    }
}

/// Rust versions of the helpers of php-src's `tests/encoding_tests.inc`,
/// working on codepoints instead of UTF-16BE/UTF-32BE strings (also used by
/// the Korean tests).
#[cfg(test)]
pub(super) mod test_support {
    use std::collections::{BTreeMap, HashSet};

    use crate::mbstring::mbfl::convert::ConvertBuf;
    use crate::mbstring::mbfl::encoding::{BAD_INPUT, Encoding, ILLEGAL_MODE_CHAR};

    /// Encoded character -> codepoint (`$toUnicode`).
    pub type ToUnicode = BTreeMap<Vec<u8>, u32>;
    /// Codepoint -> encoded character (`$fromUnicode`).
    pub type FromUnicode = BTreeMap<u32, Vec<u8>>;

    /// The substitution character the tests use (`mb_substitute_character(0x25)`).
    pub const PCT: u32 = b'%' as u32;

    const TESTS_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/phpt/ext/mbstring/tests"
    );

    pub fn read_test_file(name: &str) -> String {
        let path = format!("{TESTS_DIR}/{name}");
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn hex(b: &[u8]) -> String {
        b.iter().map(|c| format!("{c:02x}")).collect()
    }

    fn parse_hex(s: &str) -> Option<(u64, &str)> {
        let n = s.bytes().take_while(u8::is_ascii_hexdigit).count();
        if n == 0 {
            return None;
        }
        Some((u64::from_str_radix(&s[..n], 16).ok()?, &s[n..]))
    }

    /// How `readConversionTable` turns the integer of a character into bytes.
    pub fn char_bytes(v: u64) -> Vec<u8> {
        if v <= 0xFF {
            vec![v as u8]
        } else if v <= 0xFFFF {
            vec![(v >> 8) as u8, v as u8]
        } else if v <= 0xFF_FFFF {
            vec![(v >> 16) as u8, (v >> 8) as u8, v as u8]
        } else {
            vec![(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8]
        }
    }

    /// `readConversionTable`: lines of `0x<char>\t0x<codepoint>`; codepoints
    /// U+FFFD are skipped and later lines win.
    pub fn read_conversion_table(name: &str) -> (ToUnicode, FromUnicode) {
        let (mut to, mut from) = (ToUnicode::new(), FromUnicode::new());
        for line in read_test_file(name).lines() {
            let parsed = (|| {
                let (ch, rest) = parse_hex(line.strip_prefix("0x")?)?;
                let (cp, _) = parse_hex(rest.trim_start().strip_prefix("0x")?)?;
                Some((ch, cp as u32))
            })();
            if let Some((ch, cp)) = parsed {
                if cp == 0xFFFD {
                    continue;
                }
                to.insert(char_bytes(ch), cp);
                from.insert(cp, char_bytes(ch));
            }
        }
        (to, from)
    }

    /// `readGB18030_2022_ConversionTable`: lines of `<codepoint>\t<char>`.
    pub fn read_gb18030_2022_table(name: &str) -> (ToUnicode, FromUnicode) {
        let (mut to, mut from) = (ToUnicode::new(), FromUnicode::new());
        for line in read_test_file(name).lines() {
            if line.starts_with('#') {
                continue;
            }
            let parsed = (|| {
                let (cp, rest) = parse_hex(line.trim_start())?;
                let (ch, _) = parse_hex(rest.trim_start())?;
                Some((ch, cp as u32))
            })();
            if let Some((ch, cp)) = parsed {
                to.insert(char_bytes(ch), cp);
                from.insert(cp, char_bytes(ch));
            }
        }
        (to, from)
    }

    /// The PHP double-quoted string literals of a line (only the escapes
    /// the tests use).
    pub fn php_strings(line: &str) -> Vec<Vec<u8>> {
        let b = line.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'"' {
                i += 1;
                continue;
            }
            i += 1;
            let mut s = Vec::new();
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && b.get(i + 1) == Some(&b'x') {
                    let n = b[i + 2..]
                        .iter()
                        .take(2)
                        .take_while(|c| c.is_ascii_hexdigit())
                        .count();
                    s.push(u8::from_str_radix(&line[i + 2..i + 2 + n], 16).unwrap());
                    i += 2 + n;
                } else if b[i] == b'\\' {
                    match b[i + 1] {
                        b'n' => s.push(b'\n'),
                        b't' => s.push(b'\t'),
                        b'\\' | b'"' | b'$' => s.push(b[i + 1]),
                        c => s.extend_from_slice(&[b'\\', c]),
                    }
                    i += 2;
                } else {
                    s.push(b[i]);
                    i += 1;
                }
            }
            i += 1;
            out.push(s);
        }
        out
    }

    pub fn be16(b: &[u8]) -> u32 {
        assert_eq!(b.len(), 2);
        (u32::from(b[0]) << 8) | u32::from(b[1])
    }

    pub fn be32(b: &[u8]) -> u32 {
        u32::from_be_bytes(b.try_into().unwrap())
    }

    /// The `$fromUnicode["\x..\x.."] = "...";` additions of a phpt file.
    pub fn phpt_from_unicode_additions(phpt: &str) -> Vec<(u32, Vec<u8>)> {
        phpt.lines()
            .filter(|l| l.starts_with("$fromUnicode[\""))
            .map(|l| {
                let s = php_strings(l);
                (be16(&s[0]), s[1].clone())
            })
            .collect()
    }

    /// The `"key" => "value"` pairs of the PHP array literal opened on the
    /// line containing `start`.
    pub fn phpt_array_pairs(phpt: &str, start: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut pairs = Vec::new();
        for line in phpt.lines().skip_while(|l| !l.contains(start)).skip(1) {
            if line.contains("=>") {
                let s = php_strings(line);
                pairs.push((s[0].clone(), s[1].clone()));
            }
            if line.contains("];") {
                break;
            }
        }
        assert!(!pairs.is_empty(), "no pairs after {start}");
        pairs
    }

    /// A deterministic stand-in for PHP's seeded `shuffle()`.
    pub struct Rng(u64);

    impl Rng {
        pub fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }

        pub fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        pub fn shuffle<T>(&mut self, v: &mut [T]) {
            for i in (1..v.len()).rev() {
                let j = (self.next() % (i as u64 + 1)) as usize;
                v.swap(i, j);
            }
        }
    }

    /// Decodes all of `input` with an output buffer of `bufsize` codepoints.
    pub fn decode_with(input: &[u8], enc: &Encoding, bufsize: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut buf = vec![0u32; bufsize];
        let mut state = 0;
        let mut inp = input;
        while !inp.is_empty() {
            let n = (enc.to_wchar)(&mut inp, &mut buf, &mut state);
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    /// `mb_convert_encoding` to Unicode with '%' as substitution character:
    /// the codepoints and the number of errors.
    pub fn to_unicode(input: &[u8], enc: &Encoding) -> (Vec<u32>, usize) {
        let cps = decode_with(input, enc, 128);
        let errors = cps.iter().filter(|&&c| c == BAD_INPUT).count();
        (
            cps.into_iter()
                .map(|c| if c == BAD_INPUT { PCT } else { c })
                .collect(),
            errors,
        )
    }

    /// Encodes `cps` in chunks of `chunk` codepoints (`end` on the last).
    pub fn encode_mode(
        cps: &[u32],
        enc: &Encoding,
        chunk: usize,
        repl: u32,
        mode: u32,
    ) -> (Vec<u8>, u32) {
        let mut buf = ConvertBuf::new(0, repl, mode);
        let mut chunks = cps.chunks(chunk).peekable();
        while let Some(c) = chunks.next() {
            (enc.from_wchar)(c, &mut buf, chunks.peek().is_none());
        }
        (buf.out, buf.errors)
    }

    /// `mb_convert_encoding` from Unicode with '%' as substitution character.
    pub fn from_unicode(cps: &[u32], enc: &Encoding) -> (Vec<u8>, u32) {
        encode_mode(cps, enc, usize::MAX, PCT, ILLEGAL_MODE_CHAR)
    }

    fn first_difference<T: PartialEq>(a: &[T], b: &[T]) -> usize {
        a.iter()
            .zip(b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()))
    }

    /// Valid input decodes to `expected` without errors, whatever the size
    /// of the output buffer.
    pub fn assert_valid_decode(input: &[u8], expected: &[u32], enc: &Encoding) {
        for bufsize in [1, 2, 3, 5, 128] {
            let got = decode_with(input, enc, bufsize);
            if got != expected {
                let i = first_difference(&got, expected);
                panic!(
                    "{}: decoding {} (bufsize {bufsize}) differs at codepoint {i}: got {:x?}, expected {:x?}",
                    enc.name,
                    hex(&input[..input.len().min(64)]),
                    &got[i..got.len().min(i + 4)],
                    &expected[i..expected.len().min(i + 4)]
                );
            }
        }
    }

    /// Valid codepoints encode to `expected` without errors, also when
    /// passed to the encoder in chunks.
    pub fn assert_valid_encode(cps: &[u32], expected: &[u8], enc: &Encoding) {
        for chunk in [1, 3, usize::MAX] {
            let (got, errors) = encode_mode(cps, enc, chunk, PCT, ILLEGAL_MODE_CHAR);
            assert_eq!(
                errors,
                0,
                "{}: encoding {:x?} (chunk {chunk}) had errors",
                enc.name,
                &cps[..cps.len().min(8)]
            );
            if got != expected {
                let i = first_difference(&got, expected);
                panic!(
                    "{}: encoding (chunk {chunk}) differs at byte {i}: got {}, expected {}",
                    enc.name,
                    hex(&got[i..got.len().min(i + 8)]),
                    hex(&expected[i..expected.len().min(i + 8)])
                );
            }
        }
    }

    /// `testValidString` (minus the check of the Unicode side).
    pub fn assert_valid(input: &[u8], cps: &[u32], enc: &Encoding, both_ways: bool) {
        assert_eq!(
            to_unicode(input, enc),
            (cps.to_vec(), 0),
            "{}: decoding {}",
            enc.name,
            hex(input)
        );
        if both_ways {
            assert_eq!(
                from_unicode(cps, enc),
                (input.to_vec(), 0),
                "{}: encoding {cps:x?}",
                enc.name
            );
        }
    }

    /// `testInvalidString`/`convertInvalidString` towards Unicode: the
    /// conversion gives `expected` (with '%' for errors) and counts errors.
    pub fn assert_invalid(input: &[u8], enc: &Encoding, expected: &[u32]) {
        let (cps, errors) = to_unicode(input, enc);
        assert_eq!(cps, expected, "{}: decoding {}", enc.name, hex(input));
        assert!(errors > 0, "{}: no error for {}", enc.name, hex(input));
    }

    /// `findInvalidChars` without fixed-length start bytes. Truncated
    /// sequences for which `prune` is true are recorded but not extended.
    pub fn find_invalid_chars(
        valid: &ToUnicode,
        prune: impl Fn(&[u8]) -> bool,
    ) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let mut prefixes = HashSet::new();
        for k in valid.keys() {
            for len in 1..k.len() {
                prefixes.insert(k[..len].to_vec());
            }
        }

        let (mut invalid, mut truncated) = (Vec::new(), Vec::new());
        let mut stack = vec![Vec::new()];
        while let Some(prefix) = stack.pop() {
            for byte in 0..=255u8 {
                let mut s = prefix.clone();
                s.push(byte);
                if valid.contains_key(&s) {
                    continue;
                }
                if prefixes.contains(&s) {
                    if !prune(&s) {
                        stack.push(s.clone());
                    }
                    truncated.push(s);
                } else {
                    invalid.push(s);
                }
            }
        }
        (invalid, truncated)
    }

    /// `testAllValidChars` for a `$toUnicode` map: one long string of all
    /// characters, then each character on its own.
    pub fn test_all_valid_chars(map: &ToUnicode, enc: &Encoding, both_ways: bool, rng: &mut Rng) {
        let mut chars: Vec<(&Vec<u8>, u32)> = map.iter().map(|(k, &v)| (k, v)).collect();
        rng.shuffle(&mut chars);
        let from: Vec<u8> = chars.iter().flat_map(|(k, _)| k.iter().copied()).collect();
        let to: Vec<u32> = chars.iter().map(|&(_, v)| v).collect();
        assert_valid_decode(&from, &to, enc);
        if both_ways {
            assert_valid_encode(&to, &from, enc);
        }
        for (k, v) in chars {
            assert_valid(k, &[v], enc, both_ways);
        }
    }

    /// `testAllValidChars` for a `$fromUnicode` map (Unicode -> `enc`).
    pub fn test_all_valid_unicode(map: &FromUnicode, enc: &Encoding, rng: &mut Rng) {
        let mut chars: Vec<(u32, &Vec<u8>)> = map.iter().map(|(&k, v)| (k, v)).collect();
        rng.shuffle(&mut chars);
        let from: Vec<u32> = chars.iter().map(|&(k, _)| k).collect();
        let to: Vec<u8> = chars.iter().flat_map(|(_, v)| v.iter().copied()).collect();
        assert_valid_encode(&from, &to, enc);
        for (k, v) in chars {
            assert_eq!(
                from_unicode(&[k], enc),
                (v.clone(), 0),
                "{}: encoding U+{k:04X}",
                enc.name
            );
        }
    }

    /// `testAllInvalidChars`: each bad sequence followed by a good character
    /// converts to '%' and the good character.
    pub fn test_all_invalid_chars(bad: &[Vec<u8>], map: &ToUnicode, enc: &Encoding, rng: &mut Rng) {
        assert!(!bad.is_empty());
        let mut pool: Vec<(&Vec<u8>, u32)> = Vec::new();
        for b in bad.iter().rev() {
            if pool.is_empty() {
                pool = map.iter().map(|(k, &v)| (k, v)).collect();
                rng.shuffle(&mut pool);
            }
            let (good, cp) = pool.pop().unwrap();
            assert_invalid(&[b.as_slice(), good.as_slice()].concat(), enc, &[PCT, cp]);
        }
    }

    /// `testTruncatedChars`.
    pub fn test_truncated_chars(truncated: &[Vec<u8>], enc: &Encoding) {
        for t in truncated {
            assert_invalid(t, enc, &[PCT]);
        }
    }

    /// `findInvalidChars($fromUnicode, ...)` + `convertAllInvalidChars`
    /// from UTF-16BE: every 16-bit value which is not a key of `map` (nor
    /// `skip`ped), followed by a good codepoint, converts to `expected(good)`.
    /// UTF-16BE turns lone surrogates into bad input.
    pub fn convert_all_invalid_unicode(
        map: &FromUnicode,
        enc: &Encoding,
        skip: impl Fn(u32) -> bool,
        expected: impl Fn(&[u8]) -> Vec<u8>,
        rng: &mut Rng,
    ) {
        let bad: Vec<u32> = (0..=0xFFFF)
            .filter(|v| !map.contains_key(v) && !skip(*v))
            .collect();
        let mut pool: Vec<(u32, &Vec<u8>)> = Vec::new();
        for &b in bad.iter().rev() {
            if pool.is_empty() {
                pool = map
                    .iter()
                    .filter(|(k, _)| **k <= 0xFFFF)
                    .map(|(&k, v)| (k, v))
                    .collect();
                rng.shuffle(&mut pool);
            }
            let (good, good_bytes) = pool.pop().unwrap();
            let b = if (0xD800..=0xDFFF).contains(&b) {
                BAD_INPUT
            } else {
                b
            };
            let (got, errors) = from_unicode(&[b, good], enc);
            assert_eq!(
                got,
                expected(good_bytes),
                "{}: encoding [{b:#x}, {good:#x}]",
                enc.name
            );
            assert!(errors > 0, "{}: no error for {b:#x}", enc.name);
        }
    }

    /// `"%"` followed by `good`: the expected output of most
    /// [`convert_all_invalid_unicode`] calls.
    pub fn pct_then(good: &[u8]) -> Vec<u8> {
        [&b"%"[..], good].concat()
    }

    /// `testEncodingFromUTF16ConversionTable`.
    pub fn test_encoding_from_utf16_conversion_table(file: &str, enc: &Encoding) {
        let mut rng = Rng::new(1000);
        let (to, from) = read_conversion_table(file);
        let (invalid, truncated) = find_invalid_chars(&to, |_| false);
        test_all_valid_chars(&to, enc, true, &mut rng);
        test_all_invalid_chars(&invalid, &to, enc, &mut rng);
        test_truncated_chars(&truncated, enc);
        convert_all_invalid_unicode(&from, enc, |_| false, pct_then, &mut rng);
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::mbstring::mbfl::encoding::{ILLEGAL_MODE_CHAR, ILLEGAL_MODE_LONG};
    use crate::mbstring::mbfl::strcut::mbfl_strcut;

    #[test]
    fn euc_cn_encoding() {
        let euccn = &MBFL_ENCODING_EUC_CN;
        test_encoding_from_utf16_conversion_table("data/EUC-CN.txt", euccn);

        // "long" illegal character markers
        assert_invalid(b"\x80", euccn, &[PCT]);
        assert_invalid(b"\xA1\x50", euccn, &[PCT]);
        assert_invalid(b"\xF7\xFF", euccn, &[PCT]);
    }

    #[test]
    fn euc_tw_encoding() {
        let euctw = &MBFL_ENCODING_EUC_TW;
        let mut rng = Rng::new(1000);
        let (to, from) = read_conversion_table("data/EUC-TW.txt");
        assert!(to.keys().any(|k| k.len() == 4));

        let (invalid, truncated) = find_invalid_chars(&to, |_| false);
        test_all_valid_chars(&to, euctw, false, &mut rng);
        test_all_valid_unicode(&from, euctw, &mut rng);
        test_all_invalid_chars(&invalid, &to, euctw, &mut rng);
        test_truncated_chars(&truncated, euctw);
        convert_all_invalid_unicode(&from, euctw, |_| false, pct_then, &mut rng);

        // "long" illegal character markers
        let bad: [&[u8]; 8] = [
            b"\x80",
            b"\x8E\x20",
            b"\x8E\xA1\x20",
            b"\x8E\xA1\xA1\x20",
            b"\x8E\xA2\xA3\x20",
            b"\x8F",
            b"\xA1\x50",
            b"\xFD\xCC",
        ];
        for b in bad {
            assert_invalid(b, euctw, &[PCT]);
        }
    }

    fn not_four_byte_code(gb: &[u8]) -> bool {
        (!(0x81..=0x84).contains(&gb[0]) && !(0x90..=0xE3).contains(&gb[0]))
            || (gb.len() > 1 && !(0x30..=0x39).contains(&gb[1]))
    }

    /// Prefixes of 4-byte codes need not be extended by `find_invalid_chars`:
    /// every extension is filtered out by `not_four_byte_code` anyway.
    fn is_four_byte_prefix(s: &[u8]) -> bool {
        s.len() >= 2 && !not_four_byte_code(s)
    }

    /// `fourByteCodeFromIndex`.
    fn four_byte_code_from_index(index: u32) -> Vec<u8> {
        vec![
            (index / 12600 + 0x81) as u8,
            (index / 1260 % 10 + 0x30) as u8,
            (index / 10 % 126 + 0x81) as u8,
            (index % 10 + 0x30) as u8,
        ]
    }

    /// `$gb18030_BMP_Mappings` of gb18030_encoding.phpt: (index of the first
    /// 4-byte code, its codepoint, number of sequential codepoints).
    fn gb18030_bmp_mappings() -> Vec<(u32, u32, u32)> {
        let phpt = read_test_file("gb18030_encoding.phpt");
        let rows: Vec<(u32, u32, u32)> = phpt
            .lines()
            .filter(|l| l.starts_with("  [0x"))
            .map(|l| {
                let n: Vec<u32> = l
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches("],")
                    .split(',')
                    .map(|v| {
                        let v = v.trim();
                        match v.strip_prefix("0x") {
                            Some(h) => u32::from_str_radix(h, 16).unwrap(),
                            None => v.parse().unwrap(),
                        }
                    })
                    .collect();
                let index = (n[0] - 0x81) * 12600
                    + (n[1] - 0x30) * 1260
                    + (n[2] - 0x81) * 10
                    + (n[3] - 0x30);
                (index, n[4], n[5])
            })
            .collect();
        assert_eq!(rows.len(), 206);
        rows
    }

    /// The tests of 4-byte and invalid codes shared by GB18030 and
    /// GB18030-2022.
    fn gb18030_common_checks(gb: &Encoding) {
        // Invalid 4-byte codes in range for the BMP
        assert_invalid(b"\x81\x30\x81\xFF", gb, &[PCT]);
        assert_invalid(b"\x84\x31\xA4\x40", gb, &[PCT]);
        assert_invalid(b"\x84\x31\xA5\x30", gb, &[PCT]);
        assert_invalid(b"\x84\x32\x81\x30", gb, &[PCT]);
        assert_invalid(b"\x85\x31\x81\x30", gb, &[PCT, PCT]);

        // Valid 4-byte codes for other Unicode planes
        assert_valid(b"\x90\x30\x81\x30", &[0x10000], gb, true);
        assert_valid(b"\xE3\x32\x9A\x35", &[0x10FFFF], gb, true);

        // Invalid 4-byte codes for other Unicode planes
        assert_invalid(b"\x90\x30\x81\xFF", gb, &[PCT]);
        assert_invalid(b"\xE3\x32\x9A\x36", gb, &[PCT]);
        assert_invalid(b"\xE4\x30\x81\x35", gb, &[PCT, PCT]);
        assert_invalid(b"\x90\x30\x80\x30", gb, &[PCT, 0x30]);

        // Regression test found by a fuzzer (the invalid codepoint was once
        // converted to \x00); UTF-32BE turns it into bad input.
        assert_eq!(
            from_unicode(&[0xAAB8_2D38, b'#' as u32], gb),
            (b"%#".to_vec(), 1)
        );
        assert_eq!(
            from_unicode(&[BAD_INPUT, b'#' as u32], gb),
            (b"%#".to_vec(), 1)
        );
    }

    #[test]
    fn gb18030_encoding() {
        let gb = &MBFL_ENCODING_GB18030;
        let mut rng = Rng::new(1111);
        let (to, from) = read_conversion_table("data/GB18030-2byte.txt");

        // 4-byte codes are tested separately.
        let (invalid, truncated) = find_invalid_chars(&to, is_four_byte_prefix);
        let invalid: Vec<_> = invalid
            .into_iter()
            .filter(|s| not_four_byte_code(s))
            .collect();
        let truncated: Vec<_> = truncated
            .into_iter()
            .filter(|s| not_four_byte_code(s))
            .collect();

        test_all_valid_chars(&to, gb, false, &mut rng);
        test_all_invalid_chars(&invalid, &to, gb, &mut rng);
        test_truncated_chars(&truncated, gb);

        // Every 4-byte code of each range used for codepoints in the BMP.
        for (index, unicode, n) in gb18030_bmp_mappings() {
            for i in 0..n {
                assert_valid(
                    &four_byte_code_from_index(index + i),
                    &[unicode + i],
                    gb,
                    true,
                );
            }
        }

        gb18030_common_checks(gb);
        test_all_valid_unicode(&from, gb, &mut rng);
    }

    #[test]
    fn gb18030_2022_encoding() {
        let gb = &MBFL_ENCODING_GB18030_2022;
        let mut rng = Rng::new(2323);
        let phpt = read_test_file("gb18030_2022_encoding.phpt");

        let updated = phpt_array_pairs(&phpt, "$updatedMappings = [");
        let to: ToUnicode = updated.iter().map(|(k, v)| (k.clone(), be16(v))).collect();
        test_all_valid_chars(&to, gb, false, &mut rng);
        let from: FromUnicode = updated.iter().map(|(k, v)| (be16(v), k.clone())).collect();
        test_all_valid_unicode(&from, gb, &mut rng);

        let smp = phpt_array_pairs(&phpt, "$sampleSMP = [");
        let from: FromUnicode = smp.iter().map(|(k, v)| (be32(k), v.clone())).collect();
        test_all_valid_unicode(&from, gb, &mut rng);

        let (to, from) = read_gb18030_2022_table("data/GB18030-2022MappingTableBMP.txt");
        assert!(to.len() > 60000);
        let (invalid, truncated) = find_invalid_chars(&to, is_four_byte_prefix);
        let invalid: Vec<_> = invalid
            .into_iter()
            .filter(|s| not_four_byte_code(s))
            .collect();
        let truncated: Vec<_> = truncated
            .into_iter()
            .filter(|s| not_four_byte_code(s))
            .collect();

        test_all_valid_chars(&to, gb, false, &mut rng);
        test_all_invalid_chars(&invalid, &to, gb, &mut rng);
        test_truncated_chars(&truncated, gb);

        gb18030_common_checks(gb);
        test_all_valid_unicode(&from, gb, &mut rng);
    }

    #[test]
    fn cp936_encoding() {
        let cp936 = &MBFL_ENCODING_CP936;
        let mut rng = Rng::new(1000);
        let (to, mut from) = read_conversion_table("data/CP936.txt");
        let additions = phpt_from_unicode_additions(&read_test_file("cp936_encoding.phpt"));
        assert_eq!(additions.len(), 286);
        from.extend(additions);

        let (invalid, truncated) = find_invalid_chars(&to, |_| false);
        test_all_valid_chars(&to, cp936, true, &mut rng);
        test_all_invalid_chars(&invalid, &to, cp936, &mut rng);
        test_truncated_chars(&truncated, cp936);
        test_all_valid_unicode(&from, cp936, &mut rng);
        convert_all_invalid_unicode(&from, cp936, |_| false, pct_then, &mut rng);

        // "long" illegal character markers
        assert_invalid(b"\x81\x20", cp936, &[PCT]);
        assert_invalid(b"\x81\x7F", cp936, &[PCT]);
        assert_invalid(b"\xFE\xFF", cp936, &[PCT]);
    }

    #[test]
    fn big5_encoding() {
        let big5 = &MBFL_ENCODING_BIG5;
        let mut rng = Rng::new(1000);
        let (to, mut from) = read_conversion_table("data/BIG5.txt");
        let additions = phpt_from_unicode_additions(&read_test_file("big5_encoding.phpt"));
        assert_eq!(additions.len(), 8);
        from.extend(additions);

        let (invalid, truncated) = find_invalid_chars(&to, |_| false);
        test_all_valid_chars(&to, big5, false, &mut rng);
        test_all_invalid_chars(&invalid, &to, big5, &mut rng);
        test_truncated_chars(&truncated, big5);

        test_all_valid_unicode(&from, big5, &mut rng);
        convert_all_invalid_unicode(&from, big5, |_| false, pct_then, &mut rng);

        // "long" illegal character markers
        assert_invalid(b"\x80", big5, &[PCT]);
        assert_invalid(b"\xB0\x9F", big5, &[PCT]);
        assert_invalid(b"\xA3\xED", big5, &[PCT]);
        assert_eq!(
            encode_mode(&[0x7654], big5, usize::MAX, PCT, ILLEGAL_MODE_LONG),
            (b"U+7654".to_vec(), 1)
        );
    }

    #[test]
    fn cp950_encoding() {
        let cp950 = &MBFL_ENCODING_CP950;
        let mut rng = Rng::new(1000);
        let (mut to, mut from) = read_conversion_table("data/CP950.txt");
        let additions = phpt_from_unicode_additions(&read_test_file("cp950_encoding.phpt"));
        assert_eq!(additions.len(), 8);
        from.extend(additions);

        // CP950 explicitly maps some unused Big5 ranges to the PUA.
        let mut add_pua = |hi: u32, lo: u32, unicode: u32| {
            let code = vec![hi as u8, lo as u8];
            from.insert(unicode, code.clone());
            to.insert(code, unicode);
        };
        let trail = |lo: u32| if lo >= 0xA1 { lo - 0x62 } else { lo - 0x40 };
        let trails = || (0x40..=0x7E).chain(0xA1..=0xFE);
        for hi in 0x81..=0x8D {
            for lo in trails() {
                add_pua(hi, lo, 0xEEB8 + 157 * (hi - 0x81) + trail(lo));
            }
        }
        for hi in 0x8E..=0xA0 {
            for lo in trails() {
                add_pua(hi, lo, 0xE311 + 157 * (hi - 0x8E) + trail(lo));
            }
        }
        for lo in 0xA1..=0xFE {
            add_pua(0xC6, lo, 0xF672 + trail(lo));
        }
        for hi in 0xC7..=0xC8 {
            for lo in trails() {
                add_pua(hi, lo, 0xF672 + 157 * (hi - 0xC6) + trail(lo));
            }
        }
        for hi in 0xFA..=0xFE {
            for lo in trails() {
                add_pua(hi, lo, 0xE000 + 157 * (hi - 0xFA) + trail(lo));
            }
        }

        let (invalid, truncated) = find_invalid_chars(&to, |_| false);
        test_all_valid_chars(&to, cp950, false, &mut rng);
        test_all_invalid_chars(&invalid, &to, cp950, &mut rng);
        test_truncated_chars(&truncated, cp950);

        test_all_valid_unicode(&from, cp950, &mut rng);
        convert_all_invalid_unicode(&from, cp950, |_| false, pct_then, &mut rng);

        // "long" illegal character markers
        assert_invalid(b"\x80", cp950, &[PCT]);
        assert_eq!(
            encode_mode(&[0x2609], cp950, usize::MAX, PCT, ILLEGAL_MODE_LONG),
            (b"U+2609".to_vec(), 1)
        );
    }

    #[test]
    fn hz_encoding() {
        let hz = &MBFL_ENCODING_HZ;
        let mut rng = Rng::new(1000);

        // ~ is special and tested separately.
        for i in (0..0x80u8).filter(|&i| i != 0x7E) {
            assert_valid(&[i], &[i as u32], hz, true);
        }
        // Non-ASCII bytes in ASCII mode
        for i in 0x80..0xFFu8 {
            assert_invalid(&[i], hz, &[PCT]);
        }

        // Valid ~ escapes
        assert_valid(b"~~", &[b'~' as u32], hz, true);
        assert_valid(b"~\n", &[], hz, false);
        assert_valid(b"~{~}", &[], hz, false);
        assert_valid(b"~{~\n~}", &[], hz, false);
        assert_valid(b"~~?", &[b'~' as u32, b'?' as u32], hz, true);

        // All invalid ~ escapes, in ASCII and GB 2312 modes
        for i in (0..0xFFu8).filter(|&i| i != 0x0A) {
            if i != 0x7E && i != 0x7B {
                assert_invalid(&[b'~', i], hz, &[PCT]);
            }
            if i != 0x7D {
                assert_invalid(&[b'~', b'{', b'~', i, b'~', b'}'], hz, &[PCT]);
            }
        }

        let (to, from) = read_conversion_table("data/GB2312.txt");
        let (invalid, truncated) = find_invalid_chars(&to, |_| false);
        let (open, close): (&[u8], &[u8]) = (b"~{", b"~}");

        // Every GB 2312 character within ~{ ~}; two characters convert to
        // U+2225, so 0x212C is irreversible.
        for (ch, &cp) in &to {
            let wrapped = [open, ch.as_slice(), close].concat();
            assert_valid(&wrapped, &[cp], hz, ch.as_slice() != b"\x21\x2C");
        }

        // Invalid GB 2312 characters within ~{ ~} (escapes are tested above)
        let mut pool: Vec<(&Vec<u8>, u32)> = Vec::new();
        for bad in invalid.iter().rev().filter(|b| b.as_slice() != b"~") {
            if pool.is_empty() {
                pool = to.iter().map(|(k, &v)| (k, v)).collect();
                rng.shuffle(&mut pool);
            }
            let (good, cp) = pool.pop().unwrap();
            assert_invalid(
                &[open, bad.as_slice(), good.as_slice(), close].concat(),
                hz,
                &[PCT, cp],
            );
        }
        for t in &truncated {
            assert_invalid(&[open, t.as_slice()].concat(), hz, &[PCT]);
        }

        // Unicode -> HZ; ASCII is valid although not in the GB 2312 table.
        let expected = |good: &[u8]| [&b"%~{"[..], good, close].concat();
        convert_all_invalid_unicode(&from, hz, |w| w <= 0x7F, expected, &mut rng);

        // Regression tests
        assert_eq!(
            from_unicode(&[0x7AFA, 0], hz),
            (b"~{\x73\x43~}\x00".to_vec(), 0)
        );
        assert_eq!(to_unicode(b"~", hz), (vec![], 0));

        // "long" illegal character markers
        assert_invalid(b"~A", hz, &[PCT]);
        assert_invalid(b"\x80", hz, &[PCT]);
        assert_invalid(b"~{\x22\x21", hz, &[PCT]);
    }

    #[test]
    fn hz_state_across_calls() {
        let hz = &MBFL_ENCODING_HZ;
        // "汉字 a~字"
        let cps = [
            0x6C49,
            0x5B57,
            b' ' as u32,
            b'a' as u32,
            b'~' as u32,
            0x5B57,
        ];
        let bytes = b"~{::WV~} a~~~{WV~}";
        assert_valid_encode(&cps, bytes, hz);
        assert_valid_decode(bytes, &cps, hz);
        // The encoder state is kept between calls; only `end` closes GB mode.
        let mut buf = ConvertBuf::new(0, PCT, ILLEGAL_MODE_CHAR);
        mb_wchar_to_hz(&[0x6C49], &mut buf, false);
        assert_eq!(buf.out, b"~{::");
        mb_wchar_to_hz(&[0x5B57], &mut buf, true);
        assert_eq!(buf.out, b"~{::WV~}");
    }

    /// Collects the output of a legacy filter.
    struct Collect(Vec<i32>);

    impl FilterOutput for Collect {
        fn output(&mut self, c: i32) -> i32 {
            self.0.push(c);
            0
        }
        fn flush(&mut self) -> i32 {
            0
        }
    }

    fn hz_filter_decode(input: &[u8]) -> Vec<u32> {
        let mut f = ConvertFilter::new(&MBFL_ENCODING_HZ, enc(NoEncoding::Wchar), &VTBL_HZ_WCHAR);
        let mut out = Collect(Vec::new());
        for &c in input {
            assert_eq!(f.feed(c as i32, &mut out), 0);
        }
        f.flush(&mut out);
        out.0.into_iter().map(|c| c as u32).collect()
    }

    fn hz_filter_encode(input: &[u32]) -> Vec<u8> {
        let mut f = ConvertFilter::new(enc(NoEncoding::Wchar), &MBFL_ENCODING_HZ, &VTBL_WCHAR_HZ);
        let mut out = Collect(Vec::new());
        for &c in input {
            assert_eq!(f.feed(c as i32, &mut out), 0);
        }
        f.flush(&mut out);
        out.0.into_iter().map(|c| c as u8).collect()
    }

    #[test]
    fn hz_legacy_filters_match_fast_converters() {
        let hz = &MBFL_ENCODING_HZ;
        let mut rng = Rng::new(42);
        let alphabet = b"~{}\n\x21\x22\x29\x2A\x30\x3A\x57\x56\x77\x78\x7E\x20\x41\x80\xA1\xFF";
        for len in 0..9 {
            for _ in 0..3000 {
                let s: Vec<u8> = (0..len)
                    .map(|_| alphabet[(rng.next() % alphabet.len() as u64) as usize])
                    .collect();
                assert_eq!(
                    hz_filter_decode(&s),
                    decode_with(&s, hz, 128),
                    "decoding {}",
                    hex(&s)
                );
            }
        }

        let cps = [
            0x6C49, 0x5B57, 0x20, 0x7E, 0x41, 0x00B7, BAD_INPUT, 0x3000, 0x1F600, 0,
        ];
        for len in 0..7 {
            for _ in 0..3000 {
                let s: Vec<u32> = (0..len)
                    .map(|_| cps[(rng.next() % cps.len() as u64) as usize])
                    .collect();
                let (fast, _) = encode_mode(&s, hz, usize::MAX, b'?' as u32, ILLEGAL_MODE_CHAR);
                assert_eq!(hz_filter_encode(&s), fast, "encoding {s:x?}");
            }
        }
    }

    #[test]
    fn hz_strcut() {
        let hz = &MBFL_ENCODING_HZ;
        let s = b"~{::WV~} abc";
        let cut = |from, len| mbfl_strcut(s, hz, from, len).unwrap();
        assert_eq!(cut(0, 6), b"~{::~}");
        assert_eq!(cut(0, 8), b"~{::WV~}");
        assert_eq!(cut(2, 6), b"~{::~}");
        assert_eq!(cut(2, 4), b"");
        assert_eq!(cut(0, 12), b"~{::WV~} abc");
        assert_eq!(cut(8, 100), b" abc");
    }

    /// `mb_strcut()`'s argument handling before it calls `cut`.
    fn php_mb_strcut(s: &[u8], from: i64, len: Option<i64>) -> Vec<u8> {
        let n = s.len() as i64;
        let mut len = len.unwrap_or(n);
        let mut from = from;
        if from < 0 {
            from = (n + from).max(0);
        }
        if len < 0 {
            len = ((n - from) + len).max(0);
        }
        if from > n {
            return Vec::new();
        }
        mb_cut_gb18030(s, from as usize, len as usize)
    }

    #[test]
    fn gb18030_strcut() {
        assert_eq!(php_mb_strcut(b"", 0, Some(5)), b"");
        assert_eq!(php_mb_strcut(b"", -2, Some(1)), b"");
        assert_eq!(php_mb_strcut(b"", 0, Some(-1)), b"");
        assert_eq!(php_mb_strcut(b"\xF5a", 1, Some(100)), b"");
        assert_eq!(php_mb_strcut(b"\xAFw", -1, Some(100)), b"");

        // "漢字 abc カナ"
        let (gb, errors) = from_unicode(
            &[0x6F22, 0x5B57, 0x20, 0x61, 0x62, 0x63, 0x20, 0x30AB, 0x30CA],
            &MBFL_ENCODING_GB18030,
        );
        assert_eq!(errors, 0);
        assert_eq!(gb, b"\x9d\x68\xd7\xd6 abc \xa5\xab\xa5\xca");
        let cases: [(i64, i64, &[u8]); 9] = [
            (0, 0, b""),
            (0, 1, b""),
            (0, 2, b"\x9d\x68"),
            (0, 3, b"\x9d\x68"),
            (0, 4, b"\x9d\x68\xd7\xd6"),
            (0, 5, b"\x9d\x68\xd7\xd6 "),
            (1, 2, b"\x9d\x68"),
            (1, 3, b"\x9d\x68"),
            (1, 4, b"\x9d\x68\xd7\xd6"),
        ];
        for (from, len, expected) in cases {
            assert_eq!(
                php_mb_strcut(&gb, from, Some(len)),
                expected,
                "mb_strcut(gb, {from}, {len})"
            );
        }

        // U+210A is encoded using 4 bytes in GB18030.
        let (four, _) = from_unicode(&[0x210A], &MBFL_ENCODING_GB18030);
        assert_eq!(four, b"\x81\x36\xbc\x32");
        assert_eq!(php_mb_strcut(&four, 0, Some(4)), four);
        for (from, len) in [
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 4),
            (1, 3),
            (2, 3),
            (2, 4),
            (0, -1),
        ] {
            assert_eq!(
                php_mb_strcut(&four, from, Some(len)),
                b"",
                "mb_strcut(four, {from}, {len})"
            );
        }

        assert_eq!(php_mb_strcut(b"\x84\x30\x81\x30", 2, None), b"");
        assert_eq!(
            MBFL_ENCODING_GB18030_2022.cut.map(|f| f(&gb, 2, 2)),
            Some(b"\xd7\xd6".to_vec())
        );
    }
}
