//! The ISO-2022 based encodings of `libmbfl/filters/mbfilter_cjk.c`: JIS
//! (JIS7/JIS8), ISO-2022-JP, ISO-2022-JP-MOBILE#KDDI, ISO-2022-JP-2004,
//! CP50220/CP50221/CP50222, ISO-2022-JP-MS and ISO-2022-KR, plus
//! `mb_convert_kana_codepoint` (the transform behind `mb_convert_kana`, also
//! used by CP50220 to fold halfwidth katakana).
//!
//! Every encoding has the fast `mb_*_to_wchar` / `mb_wchar_to_*` converters
//! and the legacy byte-at-a-time `mbfl_filt_conv_*` filters, which
//! `mbfl_strcut` still uses for these stateful encodings.

use super::{
    NFLAGS_CODE_KDDI, NFLAGS_S, kana, mbfilter_sjis_emoji_kddi2unicode, mbfl_bisec_srch, mbfl_bisec_srch2, nflags,
    sjis_decode, sjis_encode,
};
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{BAD_INPUT, ENCTYPE_GL_UNSAFE, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, illegal_output};
use crate::mbstring::tables::emoji2uni::*;
use crate::mbstring::tables::unicode_table_cp932_ext::*;
use crate::mbstring::tables::unicode_table_jis::*;
use crate::mbstring::tables::unicode_table_jis2004::*;
use crate::mbstring::tables::unicode_table_uhc::*;

pub static MBFL_ENCODING_JIS: Encoding = Encoding {
    no: NoEncoding::Jis,
    name: "JIS",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_JIS_WCHAR),
    output_filter: Some(&VTBL_WCHAR_JIS),
    to_wchar: mb_iso2022jp_to_wchar,
    from_wchar: mb_wchar_to_jis,
    check: Some(mb_check_jis),
    cut: None,
};

pub static MBFL_ENCODING_2022JP: Encoding = Encoding {
    no: NoEncoding::Iso2022jp,
    name: "ISO-2022-JP",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JP_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JP),
    to_wchar: mb_iso2022jp_to_wchar,
    from_wchar: mb_wchar_to_iso2022jp,
    check: Some(mb_check_iso2022jp),
    cut: None,
};

pub static MBFL_ENCODING_2022JP_KDDI: Encoding = Encoding {
    no: NoEncoding::Iso2022jpKddi,
    name: "ISO-2022-JP-MOBILE#KDDI",
    mime_name: Some("ISO-2022-JP"),
    aliases: &["ISO-2022-JP-KDDI"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JP_KDDI_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JP_KDDI),
    to_wchar: mb_iso2022jp_kddi_to_wchar,
    from_wchar: mb_wchar_to_iso2022jp_kddi,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_2022JP_2004: Encoding = Encoding {
    no: NoEncoding::Iso2022jp2004,
    name: "ISO-2022-JP-2004",
    mime_name: Some("ISO-2022-JP-2004"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JP_2004_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JP_2004),
    to_wchar: mb_iso2022jp2004_to_wchar,
    from_wchar: mb_wchar_to_iso2022jp2004,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP50220: Encoding = Encoding {
    no: NoEncoding::Cp50220,
    name: "CP50220",
    mime_name: Some("ISO-2022-JP"),
    aliases: &["cp50220raw", "cp50220-raw", "JIS-ms"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_CP50220_WCHAR),
    output_filter: Some(&VTBL_WCHAR_CP50220),
    to_wchar: mb_cp5022x_to_wchar,
    from_wchar: mb_wchar_to_cp50220,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP50221: Encoding = Encoding {
    no: NoEncoding::Cp50221,
    name: "CP50221",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_CP50221_WCHAR),
    output_filter: Some(&VTBL_WCHAR_CP50221),
    to_wchar: mb_cp5022x_to_wchar,
    from_wchar: mb_wchar_to_cp50221,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP50222: Encoding = Encoding {
    no: NoEncoding::Cp50222,
    name: "CP50222",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_CP50222_WCHAR),
    output_filter: Some(&VTBL_WCHAR_CP50222),
    to_wchar: mb_cp5022x_to_wchar,
    from_wchar: mb_wchar_to_cp50222,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_2022JPMS: Encoding = Encoding {
    no: NoEncoding::Iso2022jpms,
    name: "ISO-2022-JP-MS",
    mime_name: Some("ISO-2022-JP"),
    aliases: &["ISO2022JPMS"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JPMS_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JPMS),
    to_wchar: mb_iso2022jpms_to_wchar,
    from_wchar: mb_wchar_to_iso2022jpms,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_2022KR: Encoding = Encoding {
    no: NoEncoding::Iso2022kr,
    name: "ISO-2022-KR",
    mime_name: Some("ISO-2022-KR"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022KR_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022KR),
    to_wchar: mb_iso2022kr_to_wchar,
    from_wchar: mb_wchar_to_iso2022kr,
    check: None,
    cut: None,
};

pub static VTBL_JIS_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Jis,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_jis_wchar,
    flush: mbfl_filt_conv_jis_wchar_flush,
};

pub static VTBL_WCHAR_JIS: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Jis,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_jis,
    flush: mbfl_filt_conv_any_jis_flush,
};

pub static VTBL_2022JP_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jp,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_jis_wchar,
    flush: mbfl_filt_conv_jis_wchar_flush,
};

pub static VTBL_WCHAR_2022JP: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jp,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022jp,
    flush: mbfl_filt_conv_any_jis_flush,
};

pub static VTBL_2022JP_KDDI_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jpKddi,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_2022jp_mobile_wchar,
    flush: mbfl_filt_conv_2022jp_mobile_wchar_flush,
};

pub static VTBL_WCHAR_2022JP_KDDI: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jpKddi,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022jp_mobile,
    flush: mbfl_filt_conv_wchar_2022jp_mobile_flush,
};

pub static VTBL_2022JP_2004_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jp2004,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_jis2004_wchar,
    flush: mbfl_filt_conv_jis2004_wchar_flush,
};

pub static VTBL_WCHAR_2022JP_2004: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jp2004,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_jis2004,
    flush: mbfl_filt_conv_wchar_jis2004_flush,
};

pub static VTBL_CP50220_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Cp50220,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_cp5022x_wchar,
    flush: mbfl_filt_conv_cp5022x_wchar_flush,
};

pub static VTBL_WCHAR_CP50220: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Cp50220,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_cp50220,
    flush: mbfl_filt_conv_wchar_cp50220_flush,
};

pub static VTBL_CP50221_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Cp50221,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_cp5022x_wchar,
    flush: mbfl_filt_conv_cp5022x_wchar_flush,
};

pub static VTBL_WCHAR_CP50221: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Cp50221,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_cp50221,
    flush: mbfl_filt_conv_any_jis_flush,
};

pub static VTBL_CP50222_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Cp50222,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_cp5022x_wchar,
    flush: mbfl_filt_conv_cp5022x_wchar_flush,
};

pub static VTBL_WCHAR_CP50222: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Cp50222,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_cp50222,
    flush: mbfl_filt_conv_wchar_cp50222_flush,
};

pub static VTBL_2022JPMS_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jpms,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_2022jpms_wchar,
    flush: mbfl_filt_conv_2022jpms_wchar_flush,
};

pub static VTBL_WCHAR_2022JPMS: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jpms,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022jpms,
    flush: mbfl_filt_conv_any_2022jpms_flush,
};

pub static VTBL_WCHAR_2022KR: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022kr,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022kr,
    flush: mbfl_filt_conv_any_2022kr_flush,
};

pub static VTBL_2022KR_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022kr,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_2022kr_wchar,
    flush: mbfl_filt_conv_2022kr_wchar_flush,
};

/// `MBFL_BAD_INPUT` as the legacy filters' `int`.
const BAD: i32 = BAD_INPUT as i32;

// php-src redefines these state names several times in mbfilter_cjk.c;
// each group below is used by the functions that follow the matching
// `#define`s there.

// JIS, ISO-2022-JP and CP5022x states (fast converters).
const ASCII: u32 = 0;
const JISX_0201_LATIN: u32 = 1;
const JISX_0201_KANA: u32 = 2;
const JISX_0208: u32 = 3;
const JISX_0212: u32 = 4;
/// JIS7 kana selected with Shift Out (only tracked by `mb_check_jis`).
const JISX_0201_KANA_SO: u32 = 5;

// ISO-2022-JP-KDDI and ISO-2022-JP-MS states.
const JISX0201_KANA: u32 = 0x20;
const JISX0208_KANJI: u32 = 0x80;
const UDC: u32 = 0xA0;

// ISO-2022-JP-2004 states (fast converters).
const JISX0208: u32 = 1;
const JISX0213_PLANE1: u32 = 2;
const JISX0213_PLANE2: u32 = 3;

// ISO-2022-KR states (fast converters).
const KSC5601: u32 = 1;
const EMITTED_ESC_SEQUENCE: u32 = 0x10;

/// `ucs_r_jis_table_min` / `ucs_r_jis_table_max` (plain `int` variables in
/// `unicode_table_jis.h`, so the table generator does not emit them).
const UCS_R_JIS_TABLE_MIN: u32 = 0xFF00;
const UCS_R_JIS_TABLE_MAX: u32 = 0xFF00 + UCS_R_JIS_TABLE.len() as u32;

/// Lookup in `ucs_a1_jis_table`, `ucs_a2_jis_table`, `ucs_i_jis_table` and
/// `ucs_r_jis_table`, in that order; 0 when unmapped. JIS X 0212 codes have
/// 0x8080 set and JIS X 0201 kana are 0xA1-0xDF.
fn ucs_to_jis(w: u32) -> u32 {
    if (UCS_A1_JIS_TABLE_MIN..UCS_A1_JIS_TABLE_MAX).contains(&w) {
        UCS_A1_JIS_TABLE[(w - UCS_A1_JIS_TABLE_MIN) as usize] as u32
    } else if (UCS_A2_JIS_TABLE_MIN..UCS_A2_JIS_TABLE_MAX).contains(&w) {
        UCS_A2_JIS_TABLE[(w - UCS_A2_JIS_TABLE_MIN) as usize] as u32
    } else if (UCS_I_JIS_TABLE_MIN..UCS_I_JIS_TABLE_MAX).contains(&w) {
        UCS_I_JIS_TABLE[(w - UCS_I_JIS_TABLE_MIN) as usize] as u32
    } else if (UCS_R_JIS_TABLE_MIN..UCS_R_JIS_TABLE_MAX).contains(&w) {
        UCS_R_JIS_TABLE[(w - UCS_R_JIS_TABLE_MIN) as usize] as u32
    } else {
        0
    }
}

/// Codepoints which `ucs_*_jis_table` leave unmapped, but which have a JIS X
/// 0208 equivalent; `yen` is the code used for U+00A5 YEN SIGN. 0 otherwise.
fn jis_compat(w: u32, yen: u32) -> u32 {
    match w {
        0xA5 => yen,       // YEN SIGN
        0xFF3C => 0x2140, // FULLWIDTH REVERSE SOLIDUS
        0x2225 => 0x2142, // PARALLEL TO
        0xFF0D => 0x215D, // FULLWIDTH HYPHEN-MINUS
        0xFFE0 => 0x2171, // FULLWIDTH CENT SIGN
        0xFFE1 => 0x2172, // FULLWIDTH POUND SIGN
        0xFFE2 => 0x224C, // FULLWIDTH NOT SIGN
        _ => 0,
    }
}

/// Private Use Area codepoint U+E000-U+E757 to its user-defined kuten code
/// (rows 95-114).
fn pua_to_jis(w: u32) -> u32 {
    let s = w - 0xE000;
    (((s / 94) + 0x7F) << 8) | ((s % 94) + 0x21)
}

/// `jisx0208_ucs_table[s]`, or 0 when `s` is out of range.
fn jisx0208_ucs(s: u32) -> u32 {
    JISX0208_UCS_TABLE.get(s as usize).map_or(0, |&w| w as u32)
}

/// `jisx0212_ucs_table[s]`, or 0 when `s` is out of range.
fn jisx0212_ucs(s: u32) -> u32 {
    JISX0212_UCS_TABLE.get(s as usize).map_or(0, |&w| w as u32)
}

/// The JIS X 0208 codes which the KDDI and CP932 based decoders map to
/// fullwidth forms (the kuten index `s` must be <= 137); 0 for others.
fn jis_ms_fullwidth(s: u32) -> u32 {
    match s {
        31 => 0xFF3C,  // FULLWIDTH REVERSE SOLIDUS
        32 => 0xFF5E,  // FULLWIDTH TILDE
        33 => 0x2225,  // PARALLEL TO
        60 => 0xFF0D,  // FULLWIDTH HYPHEN-MINUS
        80 => 0xFFE0,  // FULLWIDTH CENT SIGN
        81 => 0xFFE1,  // FULLWIDTH POUND SIGN
        137 => 0xFFE2, // FULLWIDTH NOT SIGN
        _ => 0,
    }
}

/// Index of `w` in `cp932ext1_ucs_table` (NEC row 13).
fn cp932ext1_index(w: u32) -> Option<u32> {
    CP932EXT1_UCS_TABLE.iter().position(|&x| x as u32 == w).map(|i| i as u32)
}

/// Index of `w` in `cp932ext2_ucs_table` (NEC-selected IBM extensions).
fn cp932ext2_index(w: u32) -> Option<u32> {
    CP932EXT2_UCS_TABLE.iter().position(|&x| x as u32 == w).map(|i| i as u32)
}

/// Index of `w` in `cp932ext3_ucs_table` (IBM extensions).
fn cp932ext3_index(w: u32) -> Option<u32> {
    CP932EXT3_UCS_TABLE.iter().position(|&x| x as u32 == w).map(|i| i as u32)
}

/// Kuten code of entry `i` of `cp932ext1_ucs_table` (row 13).
fn cp932ext1_jis(i: u32) -> u32 {
    (((i / 94) + 0x2D) << 8) + (i % 94) + 0x21
}

/// Sends `seq` (an escape sequence) to the filter's output, `CK`ing each byte.
fn emit(out: &mut dyn FilterOutput, seq: &[u8]) -> i32 {
    for &b in seq {
        ck!(out.output(b as i32));
    }
    0
}

/*
 * JIS and ISO-2022-JP
 */

pub fn mbfl_filt_conv_jis_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    loop {
        match f.status & 0xF {
            // Status 0x00: ASCII, 0x10: JIS X 0201 latin, 0x20: JIS X 0201
            // kana, 0x80: JIS X 0208, 0x90: JIS X 0212.
            0 => {
                if c == 0x1B {
                    f.status += 2;
                } else if c == 0x0E {
                    // "Kana in"
                    f.status = 0x20;
                } else if c == 0x0F {
                    // "Kana out"
                    f.status = 0;
                } else if f.status == 0x10 && c == 0x5C {
                    ck!(out.output(0xA5)); // YEN SIGN
                } else if f.status == 0x10 && c == 0x7E {
                    ck!(out.output(0x203E)); // OVER LINE
                } else if f.status == 0x20 && (0x21..0x60).contains(&c) {
                    ck!(out.output(0xFF40 + c));
                } else if (f.status == 0x80 || f.status == 0x90) && (0x21..0x7F).contains(&c) {
                    // Kanji first byte
                    f.cache = c;
                    f.status += 1;
                } else if (0..0x80).contains(&c) {
                    ck!(out.output(c));
                } else if (0xA1..0xE0).contains(&c) {
                    // GR kana
                    ck!(out.output(0xFEC0 + c));
                } else {
                    ck!(out.output(BAD));
                }
            }

            // JIS X 0208 / 0212 second byte
            1 => {
                f.status &= !0xF;
                let c1 = f.cache;
                if (0x21..0x7F).contains(&c) {
                    let s = (c1 - 0x21) * 94 + c - 0x21;
                    // A negative `s` wraps to a huge index, which the lookups reject.
                    let w = if f.status == 0x80 {
                        jisx0208_ucs(s as u32)
                    } else {
                        jisx0212_ucs(s as u32)
                    };
                    ck!(out.output(if w == 0 { BAD } else { w as i32 }));
                } else {
                    ck!(out.output(BAD));
                }
            }

            // ESC
            2 => {
                if c == 0x24 {
                    f.status += 1; // '$'
                } else if c == 0x28 {
                    f.status += 3; // '('
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    continue;
                }
            }

            // ESC $
            3 => {
                if c == 0x40 || c == 0x42 {
                    f.status = 0x80; // '@' or 'B'
                } else if c == 0x28 {
                    f.status += 1; // '('
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    ck!(out.output(0x24));
                    continue;
                }
            }

            // ESC $ (
            4 => {
                if c == 0x40 || c == 0x42 {
                    f.status = 0x80; // '@' or 'B'
                } else if c == 0x44 {
                    f.status = 0x90; // 'D'
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    ck!(out.output(0x24));
                    ck!(out.output(0x28));
                    continue;
                }
            }

            // ESC (
            5 => {
                if c == 0x42 || c == 0x48 {
                    f.status = 0; // 'B' or 'H'
                } else if c == 0x4A {
                    f.status = 0x10; // 'J'
                } else if c == 0x49 {
                    f.status = 0x20; // 'I'
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    ck!(out.output(0x28));
                    continue;
                }
            }

            _ => unreachable!(),
        }
        return 0;
    }
}

pub fn mbfl_filt_conv_jis_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        // 2-byte (JIS X 0208 or 0212) character or escape sequence was truncated
        ck!(out.output(BAD));
    }
    f.status = 0;
    out.flush();
    0
}

pub fn mbfl_filt_conv_wchar_jis(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // Negative `c` (bad input) wraps to a huge codepoint which nothing maps.
    let mut s = if c == 0x203E {
        0x1007E // OVERLINE -> JIS X 0201 OVERLINE
    } else {
        ucs_to_jis(c as u32) as i32
    };
    if s <= 0 {
        s = jis_compat(c as u32, 0x1005C) as i32;
        if c == 0 {
            s = 0;
        } else if s <= 0 {
            s = -1;
        }
    }

    if s >= 0 {
        if s < 0x80 {
            // ASCII
            if f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
            }
            f.status = 0;
            ck!(out.output(s));
        } else if s < 0x8080 {
            // JIS X 0208
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$B"));
            }
            f.status = 0x200;
            ck!(out.output((s >> 8) & 0x7F));
            ck!(out.output(s & 0x7F));
        } else if s < 0x10000 {
            // JIS X 0212
            if f.status & 0xFF00 != 0x300 {
                ck!(emit(out, b"\x1B$(D"));
            }
            f.status = 0x300;
            ck!(out.output((s >> 8) & 0x7F));
            ck!(out.output(s & 0x7F));
        } else {
            // JIS X 0201 latin
            if f.status & 0xFF00 != 0x400 {
                ck!(emit(out, b"\x1B(J"));
            }
            f.status = 0x400;
            ck!(out.output(s & 0x7F));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

/// Like `mbfl_filt_conv_wchar_jis`, but without JIS X 0212 or kana; note
/// that php-src silently drops unencodable codepoints here.
pub fn mbfl_filt_conv_wchar_2022jp(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut s = ucs_to_jis(c as u32) as i32;

    if s <= 0 {
        s = jis_compat(c as u32, 0x1005C) as i32;
        if c == 0 {
            s = 0;
        } else if s <= 0 {
            s = -1;
        }
    } else if (0x80..0x2121).contains(&s) || s > 0x8080 {
        s = -1;
    }

    if s >= 0 {
        if s < 0x80 {
            // ASCII
            if f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
            }
            f.status = 0;
            ck!(out.output(s));
        } else if s < 0x10000 {
            // JIS X 0208
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$B"));
            }
            f.status = 0x200;
            ck!(out.output((s >> 8) & 0x7F));
            ck!(out.output(s & 0x7F));
        } else {
            // JIS X 0201 latin
            if f.status & 0xFF00 != 0x400 {
                ck!(emit(out, b"\x1B(J"));
            }
            f.status = 0x400;
            ck!(out.output(s & 0x7F));
        }
    }

    0
}

pub fn mbfl_filt_conv_any_jis_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // Back to latin
    if f.status & 0xFF00 != 0 {
        ck!(emit(out, b"\x1B(B"));
    }
    f.status = 0;
    out.flush()
}

pub fn mb_iso2022jp_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    debug_assert!(out.len() >= 3);
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0usize, 0usize);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == 0x1B {
            // ESC seen; this is an escape sequence
            if e - p < 2 {
                out[o] = BAD_INPUT;
                o += 1;
                if p != e && (s[p] == b'$' || s[p] == b'(') {
                    p += 1;
                }
                continue;
            }

            let c2 = s[p];
            p += 1;
            if c2 == b'$' {
                let c3 = s[p];
                p += 1;
                if c3 == b'@' || c3 == b'B' {
                    *state = JISX_0208;
                } else if c3 == b'(' {
                    if p == e {
                        out[o] = BAD_INPUT;
                        o += 1;
                        break;
                    }
                    let c4 = s[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        *state = JISX_0208;
                    } else if c4 == b'D' {
                        *state = JISX_0212;
                    } else {
                        if limit - o < 3 {
                            p -= 4;
                            break;
                        }
                        out[o..o + 3].copy_from_slice(&[BAD_INPUT, b'$' as u32, b'(' as u32]);
                        o += 3;
                        p -= 1;
                    }
                } else {
                    if limit - o < 2 {
                        p -= 3;
                        break;
                    }
                    out[o..o + 2].copy_from_slice(&[BAD_INPUT, b'$' as u32]);
                    o += 2;
                    p -= 1;
                }
            } else if c2 == b'(' {
                let c3 = s[p];
                p += 1;
                if c3 == b'B' || c3 == b'H' {
                    *state = ASCII;
                } else if c3 == b'J' {
                    *state = JISX_0201_LATIN;
                } else if c3 == b'I' {
                    *state = JISX_0201_KANA;
                } else {
                    if limit - o < 2 {
                        p -= 3;
                        break;
                    }
                    out[o..o + 2].copy_from_slice(&[BAD_INPUT, b'(' as u32]);
                    o += 2;
                    p -= 1;
                }
            } else {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 1;
            }
        } else if c == 0xE {
            // "Kana In" marker; this is just for JIS-7/8, but ISO-2022-JP accepts it too
            *state = JISX_0201_KANA;
        } else if c == 0xF {
            // "Kana Out" marker
            *state = ASCII;
        } else if *state == JISX_0201_LATIN && c == 0x5C {
            out[o] = 0xA5; // YEN SIGN
            o += 1;
        } else if *state == JISX_0201_LATIN && c == 0x7E {
            out[o] = 0x203E; // OVER LINE
            o += 1;
        } else if *state == JISX_0201_KANA && (0x21..0x60).contains(&c) {
            out[o] = 0xFF40 + c as u32;
            o += 1;
        } else if *state >= JISX_0208 && (0x21..0x7F).contains(&c) {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            if (0x21..0x7F).contains(&c2) {
                let s = (c as u32 - 0x21) * 94 + c2 as u32 - 0x21;
                let w = if *state == JISX_0208 {
                    jisx0208_ucs(s)
                } else {
                    jisx0212_ucs(s)
                };
                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
            o += 1;
        } else if c < 0x80 {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            // GR-invoked kana (JIS8); 0xE/0xF Shift In/Shift Out (JIS7) are
            // supported as well
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_iso2022jp(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let mut s = ucs_to_jis(w);

        if s == 0 {
            s = jis_compat(w, 0x1005C);
            if s == 0 && w != 0 {
                mb_illegal_output(w, mb_wchar_to_iso2022jp, buf);
                continue;
            }
        } else if (0x80..0x2121).contains(&s) || s > 0x8080 {
            mb_illegal_output(w, mb_wchar_to_iso2022jp, buf);
            continue;
        }

        if s < 0x80 {
            // ASCII
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if s < 0x8080 {
            // JIS X 0208
            if buf.state != JISX_0208 {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX_0208;
            }
            buf.add2(((s >> 8) & 0x7F) as u8, (s & 0x7F) as u8);
        } else if s < 0x10000 {
            // JIS X 0212
            if buf.state != JISX_0212 {
                buf.add4(0x1B, b'$', b'(', b'D');
                buf.state = JISX_0212;
            }
            buf.add2(((s >> 8) & 0x7F) as u8, (s & 0x7F) as u8);
        } else {
            // JIS X 0201 latin
            if buf.state != JISX_0201_LATIN {
                buf.add3(0x1B, b'(', b'J');
                buf.state = JISX_0201_LATIN;
            }
            buf.add((s & 0x7F) as u8);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

pub fn mb_wchar_to_jis(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let mut s = if w == 0x203E {
            0x1007E // OVERLINE -> JIS X 0201 OVERLINE
        } else {
            ucs_to_jis(w)
        };

        if s == 0 {
            s = jis_compat(w, 0x1005C);
            if s == 0 && w != 0 {
                // php-src emits the error marker through the ISO-2022-JP encoder
                mb_illegal_output(w, mb_wchar_to_iso2022jp, buf);
                continue;
            }
        }

        if s < 0x80 {
            // ASCII
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if (0xA1..=0xDF).contains(&s) {
            // JIS X 0201 kana
            if buf.state != JISX_0201_KANA {
                buf.add3(0x1B, b'(', b'I');
                buf.state = JISX_0201_KANA;
            }
            buf.add((s & 0x7F) as u8);
        } else if s < 0x8080 {
            // JIS X 0208
            if buf.state != JISX_0208 {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX_0208;
            }
            buf.add2(((s >> 8) & 0x7F) as u8, (s & 0x7F) as u8);
        } else if s < 0x10000 {
            // JIS X 0212
            if buf.state != JISX_0212 {
                buf.add4(0x1B, b'$', b'(', b'D');
                buf.state = JISX_0212;
            }
            buf.add2(((s >> 8) & 0x7F) as u8, (s & 0x7F) as u8);
        } else {
            // JIS X 0201 latin
            if buf.state != JISX_0201_LATIN {
                buf.add3(0x1B, b'(', b'J');
                buf.state = JISX_0201_LATIN;
            }
            buf.add((s & 0x7F) as u8);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

pub fn mb_check_jis(input: &[u8]) -> bool {
    let e = input.len();
    let mut p = 0usize;
    let mut state = ASCII;

    while p < e {
        let c = input[p];
        p += 1;
        if c == 0x1B {
            // ESC seen; this is an escape sequence
            if state == JISX_0201_KANA_SO || e - p < 2 {
                return false;
            }
            let c2 = input[p];
            p += 1;
            if c2 == b'$' {
                let c3 = input[p];
                p += 1;
                if c3 == b'@' || c3 == b'B' {
                    state = JISX_0208;
                } else if c3 == b'(' {
                    if p == e {
                        return false;
                    }
                    let c4 = input[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        state = JISX_0208;
                    } else if c4 == b'D' {
                        state = JISX_0212;
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            } else if c2 == b'(' {
                let c3 = input[p];
                p += 1;
                // ESC ( H is treated as a sequence transitioning to ASCII for
                // historical reasons
                if c3 == b'B' || c3 == b'H' {
                    state = ASCII;
                } else if c3 == b'J' {
                    state = JISX_0201_LATIN;
                } else if c3 == b'I' {
                    state = JISX_0201_KANA;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        } else if c == 0xE {
            // "Kana In" marker
            if state != ASCII {
                return false;
            }
            state = JISX_0201_KANA_SO;
        } else if c == 0xF {
            // "Kana Out" marker
            if state != JISX_0201_KANA_SO {
                return false;
            }
            state = ASCII;
        } else if (state == JISX_0208 || state == JISX_0212) && (0x21..0x7F).contains(&c) {
            if p == e {
                return false;
            }
            let c2 = input[p];
            p += 1;
            if !(0x21..0x7F).contains(&c2) {
                return false;
            }
            let s = (c as u32 - 0x21) * 94 + c2 as u32 - 0x21;
            let w = if state == JISX_0208 {
                jisx0208_ucs(s)
            } else {
                jisx0212_ucs(s)
            };
            if w == 0 {
                return false;
            }
        } else if c >= 0x80 && !(0xA1..=0xDF).contains(&c) {
            // Only ASCII and GR-invoked kana are allowed
            return false;
        }
    }

    state == ASCII
}

pub fn mb_check_iso2022jp(input: &[u8]) -> bool {
    let e = input.len();
    let mut p = 0usize;
    let mut state = ASCII;

    while p < e {
        let c = input[p];
        p += 1;
        if c == 0x1B {
            // ESC seen; this is an escape sequence
            if e - p < 2 {
                return false;
            }
            let c2 = input[p];
            let c3 = input[p + 1];
            p += 2;
            if c2 == b'$' {
                if c3 == b'@' || c3 == b'B' {
                    state = JISX_0208;
                } else {
                    return false;
                }
            } else if c2 == b'(' {
                if c3 == b'B' {
                    state = ASCII;
                } else if c3 == b'J' {
                    state = JISX_0201_LATIN;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        } else if c == 0xE || c == 0xF {
            // "Kana In" or "Kana Out" marker; not accepted for ISO-2022-JP
            return false;
        } else if state == JISX_0208 && (0x21..0x7F).contains(&c) {
            if p == e {
                return false;
            }
            let c2 = input[p];
            p += 1;
            if !(0x21..0x7F).contains(&c2) || jisx0208_ucs((c as u32 - 0x21) * 94 + c2 as u32 - 0x21) == 0 {
                return false;
            }
        } else if c >= 0x80 {
            return false;
        }
    }

    state == ASCII
}

/*
 * ISO-2022-JP-MOBILE#KDDI
 */

pub fn mbfl_filt_conv_2022jp_mobile_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    match f.status & 0xF {
        0 => {
            if c == 0x1B {
                f.status += 2;
            } else if f.status == JISX0201_KANA as i32 && (0x21..0x60).contains(&c) {
                ck!(out.output(0xFF40 + c));
            } else if f.status == JISX0208_KANJI as i32 && (0x21..0x80).contains(&c) {
                f.cache = c;
                f.status += 1;
            } else if (0..0x80).contains(&c) {
                ck!(out.output(c)); // ASCII
            } else if (0xA1..0xE0).contains(&c) {
                ck!(out.output(0xFEC0 + c)); // Kana
            } else {
                ck!(out.output(BAD));
            }
        }

        // JIS X 0208, second byte
        1 => {
            let mut w = 0;
            f.status &= !0xF;
            let c1 = f.cache;
            if (0x21..0x7F).contains(&c) {
                let mut s = ((c1 - 0x21) * 94) + c - 0x21;

                if s <= 137 {
                    w = jis_ms_fullwidth(s as u32) as i32;
                }

                if s >= 84 * 94 && s < 91 * 94 {
                    // KDDI emoji are mapped 22 rows above their Shift-JIS rows
                    let mut snd = 0;
                    s += 22 * 94;
                    w = mbfilter_sjis_emoji_kddi2unicode(s, &mut snd);
                    if w > 0 && snd > 0 {
                        out.output(snd);
                    }
                }

                if w == 0 {
                    if (CP932EXT1_UCS_TABLE_MIN as i32..CP932EXT1_UCS_TABLE_MAX as i32).contains(&s) {
                        w = CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN as i32) as usize] as i32;
                    } else if s >= 0 {
                        w = jisx0208_ucs(s as u32) as i32;
                    }
                }

                if w <= 0 {
                    w = BAD;
                }
                ck!(out.output(w));
            } else {
                ck!(out.output(BAD));
            }
        }

        // ESC
        2 => {
            if c == b'$' as i32 {
                f.status += 1;
            } else if c == b'(' as i32 {
                f.status += 3;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $
        3 => {
            if c == b'@' as i32 || c == b'B' as i32 {
                f.status = JISX0208_KANJI as i32;
            } else if c == b'(' as i32 {
                f.status += 1;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $ (
        4 => {
            if c == b'@' as i32 || c == b'B' as i32 {
                f.status = JISX0208_KANJI as i32;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC (
        5 => {
            if c == b'B' as i32 || c == b'J' as i32 {
                f.status = 0; // ASCII mode
            } else if c == b'I' as i32 {
                f.status = JISX0201_KANA as i32;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        _ => {}
    }

    0
}

pub fn mbfl_filt_conv_2022jp_mobile_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        out.output(BAD);
    }
    f.status = 0;
    out.flush();
    0
}

/// `mbfilter_unicode2sjis_emoji_kddi`: stores a KDDI emoji's (ku*94)+ten
/// value in `s1` and returns 1 if `c` is one; digits and '#' are held back
/// (status 1) since they may combine with a following U+20E3.
fn mbfilter_unicode2sjis_emoji_kddi(c: i32, s1: &mut i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF == 1 {
        let c1 = f.cache;
        f.cache = 0;
        f.status &= !0xFF;
        if c == 0x20E3 {
            *s1 = if c1 == b'#' as i32 {
                0x25BC
            } else if c1 == b'0' as i32 {
                0x2830
            } else {
                // Previous character was '1'-'9'
                0x27A6 + (c1 - b'1' as i32)
            };
            return 1;
        }
        if f.status & 0xFF00 != 0 {
            ck!(emit(out, b"\x1B(B"));
        }
        ck!(out.output(c1));
        f.status = 0;
    }

    if c == b'#' as i32 || (b'0' as i32..=b'9' as i32).contains(&c) {
        f.status |= 1;
        f.cache = c;
        return 0;
    }

    if c == 0xA9 {
        // Copyright sign
        *s1 = 0x27DC;
        return 1;
    } else if c == 0xAE {
        // Registered sign
        *s1 = 0x27DD;
        return 1;
    } else if (MB_TBL_UNI_KDDI2CODE2_MIN as i32..=MB_TBL_UNI_KDDI2CODE2_MAX as i32).contains(&c) {
        let i = mbfl_bisec_srch2(c, &MB_TBL_UNI_KDDI2CODE2_KEY, MB_TBL_UNI_KDDI2CODE2_LEN as usize);
        if i >= 0 {
            *s1 = MB_TBL_UNI_KDDI2CODE2_VALUE[i as usize] as i32;
            return 1;
        }
    } else if (MB_TBL_UNI_KDDI2CODE3_MIN as i32..=MB_TBL_UNI_KDDI2CODE3_MAX as i32).contains(&c) {
        let i = mbfl_bisec_srch2(c - 0x10000, &MB_TBL_UNI_KDDI2CODE3_KEY, MB_TBL_UNI_KDDI2CODE3_LEN as usize);
        if i >= 0 {
            *s1 = MB_TBL_UNI_KDDI2CODE3_VALUE[i as usize] as i32;
            return 1;
        }
    } else if (MB_TBL_UNI_KDDI2CODE5_MIN as i32..=MB_TBL_UNI_KDDI2CODE5_MAX as i32).contains(&c) {
        let i = mbfl_bisec_srch2(c - 0xF0000, &MB_TBL_UNI_KDDI2CODE5_KEY, MB_TBL_UNI_KDDI2CODE5_LEN as usize);
        if i >= 0 {
            *s1 = MB_TBL_UNI_KDDI2CODE5_VAL[i as usize] as i32;
            return 1;
        }
    }
    0
}

pub fn mbfl_filt_conv_wchar_2022jp_mobile(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut s2 = 0;
    let mut s1 = ucs_to_jis(c as u32) as i32;
    if s1 <= 0 {
        s1 = jis_compat(c as u32, 0x216F) as i32; // YEN SIGN -> FULLWIDTH YEN SIGN
    }

    if mbfilter_unicode2sjis_emoji_kddi(c, &mut s1, f, out) > 0 {
        // A KDDI emoji was detected and stored in s1: CODE2JIS, then move it
        // down 22 rows
        let c1 = s1 / 94 + 0x21;
        let c2 = s1 - 94 * (c1 - 0x21) + 0x21;
        s1 = (c1 << 8) | c2;
        s2 = 1;
        s1 -= 0x1600;
    } else if f.status & 0xFF == 1 && f.cache != 0 {
        // We are just processing one of KDDI's special emoji for a phone keypad button
        return 0;
    }

    if s1 <= 0 || (s1 >= 0xA1A1 && s2 == 0) {
        // Not found or JIS X 0212
        s1 = cp932ext1_index(c as u32).map_or(-1, |i| cp932ext1_jis(i) as i32);
        if c == 0 {
            s1 = 0;
        }
    }

    if s1 >= 0 {
        if s1 < 0x80 {
            // ASCII
            if f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
            }
            ck!(out.output(s1));
            f.status = 0;
        } else if s1 > 0xA0 && s1 < 0xE0 {
            // Kana
            if f.status & 0xFF00 != 0x100 {
                ck!(emit(out, b"\x1B(I"));
            }
            f.status = 0x100;
            ck!(out.output(s1 & 0x7F));
        } else if s1 < 0x7E7F {
            // JIS X 0208
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$B"));
            }
            f.status = 0x200;
            ck!(out.output((s1 >> 8) & 0xFF));
            ck!(out.output(s1 & 0x7F));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

pub fn mbfl_filt_conv_wchar_2022jp_mobile_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // Go back to ASCII mode (so strings can be safely concatenated)
    if f.status & 0xFF00 != 0 {
        out.output(0x1B);
        out.output(b'(' as i32);
        out.output(b'B' as i32);
    }

    let c1 = f.cache;
    if f.status & 0xFF == 1 && (c1 == b'#' as i32 || (b'0' as i32..=b'9' as i32).contains(&c1)) {
        out.output(c1);
    }
    f.status = 0;
    f.cache = 0;

    out.flush();
    0
}

pub fn mb_iso2022jp_kddi_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    // Leave room for the extra codepoint of 2-codepoint emoji
    let limit = out.len() - 1;
    let (mut p, mut o) = (0usize, 0usize);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == 0x1B {
            if e - p < 2 {
                p = e;
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            let c3 = s[p + 1];
            p += 2;

            if c2 == b'$' {
                if c3 == b'@' || c3 == b'B' {
                    *state = JISX0208_KANJI;
                } else if c3 == b'(' {
                    if p == e {
                        out[o] = BAD_INPUT;
                        o += 1;
                        break;
                    }
                    let c4 = s[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        *state = JISX0208_KANJI;
                    } else {
                        out[o] = BAD_INPUT;
                        o += 1;
                    }
                } else {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
            } else if c2 == b'(' {
                if c3 == b'B' || c3 == b'J' {
                    *state = ASCII;
                } else if c3 == b'I' {
                    *state = JISX0201_KANA;
                } else {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
            } else {
                p -= 1;
                out[o] = BAD_INPUT;
                o += 1;
            }
        } else if *state == JISX0201_KANA && (0x21..=0x5F).contains(&c) {
            out[o] = 0xFF40 + c as u32;
            o += 1;
        } else if *state == JISX0208_KANJI && (0x21..=0x7F).contains(&c) {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;

            if (0x21..=0x7E).contains(&c2) {
                let mut s = ((c as u32 - 0x21) * 94) + c2 as u32 - 0x21;
                let mut w = 0u32;

                if s <= 137 {
                    w = jis_ms_fullwidth(s);
                }

                if (84 * 94..91 * 94).contains(&s) {
                    let mut snd = 0;
                    s += 22 * 94;
                    w = mbfilter_sjis_emoji_kddi2unicode(s as i32, &mut snd) as u32;
                    if w != 0 && snd != 0 {
                        out[o] = snd as u32;
                        o += 1;
                    }
                }

                if w == 0 {
                    if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&s) {
                        w = CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN) as usize] as u32;
                    } else {
                        w = jisx0208_ucs(s);
                    }
                }

                out[o] = if w != 0 { w } else { BAD_INPUT };
            } else {
                out[o] = BAD_INPUT;
            }
            o += 1;
        } else if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

/// KDDI emoji (ku*94)+ten value to the JIS code used in ISO-2022-JP-KDDI
/// (22 rows below its Shift-JIS row).
fn kddi_emoji_jis(s1: u32) -> u32 {
    ((((s1 / 94) + 0x21) << 8) + ((s1 % 94) + 0x21)).wrapping_sub(0x1600)
}

pub fn mb_wchar_to_iso2022jp_kddi(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let mut i = 0usize;
    while i < input.len() {
        let w = input[i];
        i += 1;
        let mut s = ucs_to_jis(w);

        if s == 0 {
            s = jis_compat(w, 0x216F); // YEN SIGN -> FULLWIDTH YEN SIGN
        }

        if (w == b'#' as u32 || (b'0' as u32..=b'9' as u32).contains(&w)) && i < input.len() {
            let w2 = input[i];
            i += 1;
            if w2 == 0x20E3 {
                let s1 = if w == b'#' as u32 {
                    0x25BC
                } else if w == b'0' as u32 {
                    0x2830
                } else {
                    // Previous character was '1'-'9'
                    0x27A6 + (w - b'1' as u32)
                };
                s = kddi_emoji_jis(s1);
            } else {
                i -= 1;
            }
        } else if (nflags(b'C') as u32..=nflags(b'U') as u32).contains(&w) && i < input.len() {
            // C for CN, U for US
            let w2 = input[i];
            i += 1;
            let mut found = false;
            if (nflags(b'B') as u32..=nflags(b'U') as u32).contains(&w2) {
                // B for GB, U for RU
                if let Some(k) =
                    (0..NFLAGS_S.len()).find(|&k| w == nflags(NFLAGS_S[k][0]) as u32 && w2 == nflags(NFLAGS_S[k][1]) as u32)
                {
                    s = kddi_emoji_jis(NFLAGS_CODE_KDDI[k] as u32);
                    found = true;
                }
            }
            if !found {
                i -= 1;
            }
        }

        if w == 0xA9 {
            // Copyright sign
            s = kddi_emoji_jis(0x27DC);
        } else if w == 0xAE {
            // Registered sign
            s = kddi_emoji_jis(0x27DD);
        } else if (MB_TBL_UNI_KDDI2CODE2_MIN..=MB_TBL_UNI_KDDI2CODE2_MAX).contains(&w) {
            let k = mbfl_bisec_srch2(w as i32, &MB_TBL_UNI_KDDI2CODE2_KEY, MB_TBL_UNI_KDDI2CODE2_LEN as usize);
            if k >= 0 {
                s = kddi_emoji_jis(MB_TBL_UNI_KDDI2CODE2_VALUE[k as usize] as u32);
            }
        } else if (MB_TBL_UNI_KDDI2CODE3_MIN..=MB_TBL_UNI_KDDI2CODE3_MAX).contains(&w) {
            let k = mbfl_bisec_srch2(
                (w - 0x10000) as i32,
                &MB_TBL_UNI_KDDI2CODE3_KEY,
                MB_TBL_UNI_KDDI2CODE3_LEN as usize,
            );
            if k >= 0 {
                s = kddi_emoji_jis(MB_TBL_UNI_KDDI2CODE3_VALUE[k as usize] as u32);
            }
        } else if (MB_TBL_UNI_KDDI2CODE5_MIN..=MB_TBL_UNI_KDDI2CODE5_MAX).contains(&w) {
            let k = mbfl_bisec_srch2(
                (w - 0xF0000) as i32,
                &MB_TBL_UNI_KDDI2CODE5_KEY,
                MB_TBL_UNI_KDDI2CODE5_LEN as usize,
            );
            if k >= 0 {
                s = kddi_emoji_jis(MB_TBL_UNI_KDDI2CODE5_VAL[k as usize] as u32);
            }
        }

        if s == 0 || s >= 0xA1A1 {
            s = cp932ext1_index(w).map_or(0, cp932ext1_jis);
            if w == 0 {
                s = 0;
            }
        }

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_iso2022jp_kddi, buf);
        } else if s <= 0x7F {
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if (0xA1..=0xDF).contains(&s) {
            if buf.state != JISX0201_KANA {
                buf.add3(0x1B, b'(', b'I');
                buf.state = JISX0201_KANA;
            }
            buf.add((s & 0x7F) as u8);
        } else if s <= 0x7E7E {
            if buf.state != JISX0208_KANJI {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX0208_KANJI;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_iso2022jp_kddi, buf);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

/*
 * ISO-2022-JP-2004 (the legacy filters are shared with EUC-JP-2004 and
 * SJIS-2004 in php-src, hence the checks of `from`/`to`)
 */

/// JIS X 0213 plane 1 codes of characters which decode to a base character
/// plus a combining character (`jisx0213_u2_key`).
fn is_jisx0213_combining(w1: i32) -> bool {
    (0x2477..=0x2479).contains(&w1)
        || (0x2479..=0x247B).contains(&w1)
        || (0x2577..=0x257E).contains(&w1)
        || w1 == 0x2678
        || w1 == 0x2B44
        || (0x2B48..=0x2B4F).contains(&w1)
        || (0x2B65..=0x2B66).contains(&w1)
}

/// Unicode codepoints which can start a JIS X 0213 combining sequence.
fn is_jisx0213_combining_start(w: u32) -> bool {
    w == 0xE6
        || (0x254..=0x2E9).contains(&w)
        || (0x304B..=0x3053).contains(&w)
        || (0x30AB..=0x30C8).contains(&w)
        || w == 0x31F7
}

/// Offset of JIS X 0213 plane 2 row `s1` (0-based ku) within the plane 2
/// part of `jisx0213_ucs_table` (the `k -= jisx0213_p2_ofst[k]` step).
fn jisx0213_p2_row_offset(s1: i32) -> i32 {
    // Callers only pass the rows which plane 2 defines, all of which are in
    // `jisx0213_p2_ofst`.
    let k = JISX0213_P2_OFST
        .iter()
        .position(|&o| o as i32 == s1)
        .expect("JIS X 0213 plane 2 row");
    k as i32 - JISX0213_P2_OFST[k] as i32
}

/// Whether `s1` (0-based ku) is a row defined in JIS X 0213 plane 2.
fn is_jisx0213_p2_row(s1: i32) -> bool {
    ((0..=4).contains(&s1) && s1 != 1) || s1 == 7 || (11..=14).contains(&s1) || (77..94).contains(&s1)
}

pub fn mbfl_filt_conv_jis2004_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let from = f.from.no;
    let mut w = 0;

    match f.status & 0xF {
        0 => {
            if (0..0x80).contains(&c) {
                // Latin
                if from == NoEncoding::Eucjp2004 {
                    ck!(out.output(c));
                } else if from == NoEncoding::Sjis2004 {
                    if c == 0x5C {
                        ck!(out.output(0xA5));
                    } else if c == 0x7E {
                        ck!(out.output(0x203E));
                    } else {
                        ck!(out.output(c));
                    }
                } else if c == 0x1B {
                    // ISO-2022-JP-2004
                    f.status += 6;
                } else if (f.status == 0x80 || f.status == 0x90 || f.status == 0xA0) && (0x21..0x7F).contains(&c) {
                    // Kanji first byte
                    f.cache = c;
                    if f.status == 0x90 {
                        f.status += 1; // JIS X 0213 plane 1
                    } else if f.status == 0xA0 {
                        f.status += 4; // JIS X 0213 plane 2
                    } else {
                        f.status += 5; // JIS X 0208
                    }
                } else {
                    ck!(out.output(c));
                }
            } else if from == NoEncoding::Eucjp2004 {
                if (0xA1..0xFF).contains(&c) {
                    // JIS X 0213 plane 1 first byte
                    f.status = 1;
                    f.cache = c;
                } else if c == 0x8E {
                    // Kana first byte; caching it makes a truncation here an error
                    f.cache = 0x8E;
                    f.status = 2;
                } else if c == 0x8F {
                    // JIS X 0213 plane 2 first byte
                    f.status = 3;
                } else {
                    ck!(out.output(BAD));
                }
            } else if from == NoEncoding::Sjis2004 {
                if (0xA1..0xE0).contains(&c) {
                    ck!(out.output(0xFEC0 + c)); // Kana
                } else if (0x81..0xFD).contains(&c) && c != 0xA0 {
                    // Kanji first byte
                    f.status = 1;
                    f.cache = c;
                } else {
                    ck!(out.output(BAD));
                }
            } else {
                ck!(out.output(BAD));
            }
        }

        // Kanji second byte
        1 => {
            f.status &= !0xF;
            let c1 = f.cache;
            let (s1, s2);

            if from == NoEncoding::Eucjp2004 {
                if !(0xA1..0xFF).contains(&c) {
                    ck!(out.output(BAD));
                    return 0;
                }
                s1 = c1 - 0x80;
                s2 = c - 0x80;
            } else if from == NoEncoding::Sjis2004 {
                if !(0x40..=0xFC).contains(&c) || c == 0x7F {
                    ck!(out.output(BAD));
                    return 0;
                }
                let (a, b) = sjis_decode(c1 as u32, c as u32);
                s1 = a as i32;
                s2 = b as i32;
            } else {
                // ISO-2022-JP-2004
                if !(0x21..=0x7E).contains(&c) {
                    ck!(out.output(BAD));
                    return 0;
                }
                s1 = c1;
                s2 = c;
            }
            let mut w1 = (s1 << 8) | s2;

            // Conversion for combining characters
            if is_jisx0213_combining(w1) {
                let k = mbfl_bisec_srch2(w1, &JISX0213_U2_KEY, JISX0213_U2_TBL_LEN as usize);
                if k >= 0 {
                    let k = k as usize;
                    w = JISX0213_U2_TBL[2 * k] as i32;
                    ck!(out.output(w));
                    w = JISX0213_U2_TBL[2 * k + 1] as i32;
                }
            }

            // Conversion for BMP
            if w <= 0 {
                w1 = (s1 - 0x21) * 94 + s2 - 0x21;
                if (0..JISX0213_UCS_TABLE_SIZE as i32).contains(&w1) {
                    w = JISX0213_UCS_TABLE[w1 as usize] as i32;
                }
            }

            // Conversion for CJK Unified Ideographs ext.B (U+2XXXX)
            if w <= 0 {
                let k = mbfl_bisec_srch2(w1, &JISX0213_JIS_U5_KEY, JISX0213_U5_TBL_LEN as usize);
                if k >= 0 {
                    w = JISX0213_JIS_U5_TBL[k as usize] as i32 + 0x20000;
                }
            }

            if w <= 0 {
                w = BAD;
            }
            ck!(out.output(w));
        }

        // Got 0x8E: EUC-JP-2004 kana
        2 => {
            f.status = 0;
            if (0xA1..0xE0).contains(&c) {
                ck!(out.output(0xFEC0 + c));
            } else {
                ck!(out.output(BAD));
            }
        }

        // JIS X 0213 plane 2 first byte: EUC-JP-2004 (0x8F)
        3 => {
            if c == 0xA1
                || (0xA3..=0xA5).contains(&c)
                || c == 0xA8
                || (0xAC..=0xAF).contains(&c)
                || (0xEE..=0xFE).contains(&c)
            {
                f.cache = c - 0x80;
                f.status += 1;
            } else {
                f.status = 0;
                ck!(out.output(BAD));
            }
        }

        // JIS X 0213 plane 2 second byte: EUC-JP-2004, ISO-2022-JP-2004
        4 => {
            f.status &= !0xF;
            let c1 = f.cache;
            let c2 = if from == NoEncoding::Eucjp2004 { c - 0x80 } else { c };

            if !(0x21..=0x7E).contains(&c2) {
                ck!(out.output(BAD));
                return 0;
            }

            let s1 = c1 - 0x21;
            let s2 = c2 - 0x21;

            if is_jisx0213_p2_row(s1) && (0..94).contains(&s2) {
                let k = jisx0213_p2_row_offset(s1);

                // Check for Japanese chars in BMP
                let s = (s1 + 94 + k) * 94 + s2;
                w = JISX0213_UCS_TABLE[s as usize] as i32;

                // Check for Japanese chars in CJK Unified Ideographs ext.B (U+2XXXX)
                if w <= 0 {
                    let k = mbfl_bisec_srch2(s, &JISX0213_JIS_U5_KEY, JISX0213_U5_TBL_LEN as usize);
                    if k >= 0 {
                        w = JISX0213_JIS_U5_TBL[k as usize] as i32 + 0x20000;
                    }
                }

                if w <= 0 {
                    w = BAD;
                }
                ck!(out.output(w));
            } else {
                ck!(out.output(BAD));
            }
        }

        // JIS X 0208: ISO-2022-JP-2004
        5 => {
            f.status &= !0xF;
            let c1 = f.cache;
            if (0x21..0x7F).contains(&c) {
                let s = (c1 - 0x21) * 94 + c - 0x21;
                if s >= 0 {
                    w = jisx0208_ucs(s as u32) as i32;
                }
            }

            if w <= 0 {
                w = BAD;
            }
            ck!(out.output(w));
        }

        // ESC: ISO-2022-JP-2004
        6 => {
            if c == b'$' as i32 {
                f.status += 1;
            } else if c == b'(' as i32 {
                f.status += 3;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $: ISO-2022-JP-2004
        7 => {
            if c == b'B' as i32 {
                f.status = 0x80; // JIS X 0208-1983
            } else if c == b'(' as i32 {
                f.status += 1;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $ (: ISO-2022-JP-2004
        8 => {
            if c == b'Q' as i32 {
                f.status = 0x90; // JIS X 0213 plane 1
            } else if c == b'P' as i32 {
                f.status = 0xA0; // JIS X 0213 plane 2
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC (: ISO-2022-JP-2004
        9 => {
            if c == b'B' as i32 {
                f.status = 0;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        _ => unreachable!(),
    }

    0
}

pub fn mbfl_filt_conv_jis2004_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        ck!(out.output(BAD));
    }
    f.status = 0;
    out.flush()
}

pub fn mbfl_filt_conv_wchar_jis2004(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let to = f.to.no;
    let u2_len = JISX0213_U2_TBL_LEN as usize;
    let mut s1: i32;
    let mut s2: i32;

    loop {
        s1 = 0;

        // Check for 1st char of combining characters
        if f.status & 0xF == 0 && is_jisx0213_combining_start(c as u32) {
            if let Some(k) = (0..u2_len).find(|&k| c == JISX0213_U2_TBL[2 * k] as i32) {
                f.status += 1;
                f.cache = k as i32;
                return 0;
            }
        }

        // Check for 2nd char of combining characters
        if f.status & 0xF == 1 && (0..u2_len as i32).contains(&f.cache) {
            let mut k = f.cache as usize;
            f.status &= !0xF;
            f.cache = 0;

            let c1 = JISX0213_U2_TBL[2 * k] as i32;
            if (c1 == 0x254 || c1 == 0x28C || c1 == 0x259 || c1 == 0x25A) && c == 0x301 {
                k += 1;
            }
            if c == JISX0213_U2_TBL[2 * k + 1] as i32 {
                s1 = JISX0213_U2_KEY[k] as i32;
            } else {
                // Fallback: emit the cached character alone, then retry `c`
                s1 = JISX0213_U2_FB_TBL[k] as i32;

                if to == NoEncoding::Sjis2004 {
                    let (a, b) = sjis_encode(((s1 >> 8) & 0xFF) as u32, (s1 & 0xFF) as u32);
                    s1 = a as i32;
                    s2 = b as i32;
                } else if to == NoEncoding::Eucjp2004 {
                    s2 = (s1 & 0xFF) + 0x80;
                    s1 = ((s1 >> 8) & 0xFF) + 0x80;
                } else {
                    if f.status != 0x200 {
                        ck!(emit(out, b"\x1B$(Q"));
                    }
                    f.status = 0x200;
                    s2 = s1 & 0x7F;
                    s1 = (s1 >> 8) & 0x7F;
                }

                ck!(out.output(s1));
                ck!(out.output(s2));
                continue;
            }
        }
        break;
    }

    // Check for major Japanese chars: U+4E00-U+9FFF
    if s1 <= 0 {
        if let Some(k) = UNI2JIS_TBL_RANGE
            .iter()
            .position(|range| (range[0]..=range[1]).contains(&c))
        {
            s1 = UNI2JIS_TBL[k][(c - UNI2JIS_TBL_RANGE[k][0]) as usize] as i32;
        }
    }

    // Check for Japanese chars in compressed mapping area: U+1E00-U+4DBF
    if s1 <= 0 && (UCS_C1_JISX0213_MIN as i32..=UCS_C1_JISX0213_MAX as i32).contains(&c) {
        let k = mbfl_bisec_srch(c, &UCS_C1_JISX0213_TBL, UCS_C1_JISX0213_TBL_LEN as usize);
        if k >= 0 {
            let k = k as usize;
            s1 = UCS_C1_JISX0213_OFST[k] as i32 + c - UCS_C1_JISX0213_TBL[2 * k] as i32;
        }
    }

    // Check for Japanese chars in CJK Unified Ideographs ext.B (U+2XXXX)
    if s1 <= 0 && (JISX0213_U5_TBL_MIN as i32..=JISX0213_U5_TBL_MAX as i32).contains(&c) {
        let k = mbfl_bisec_srch2(c - 0x20000, &JISX0213_U5_JIS_KEY, JISX0213_U5_TBL_LEN as usize);
        if k >= 0 {
            s1 = JISX0213_U5_JIS_TBL[k as usize] as i32;
        }
    }

    if s1 <= 0 {
        // CJK Compatibility Forms: U+FE30-U+FE4F
        if c == 0xFE45 {
            s1 = 0x233E;
        } else if c == 0xFE46 {
            s1 = 0x233D;
        } else if (0xF91D..=0xF9DC).contains(&c) {
            // CJK Compatibility Ideographs: U+F900-U+F92A
            let k = mbfl_bisec_srch2(c, &UCS_R2B_JISX0213_CMAP_KEY, UCS_R2B_JISX0213_CMAP_LEN as usize);
            if k >= 0 {
                s1 = UCS_R2B_JISX0213_CMAP_VAL[k as usize] as i32;
            }
        }
    }

    if s1 <= 0 {
        s1 = if c == 0 { 0 } else { -1 };
    }

    if s1 >= 0 {
        if s1 < 0x80 {
            // ASCII
            if to == NoEncoding::Iso2022jp2004 && f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
            }
            f.status = 0;
            ck!(out.output(s1));
        } else if s1 < 0x100 {
            // Latin or kana
            if to == NoEncoding::Eucjp2004 {
                ck!(out.output(0x8E));
                ck!(out.output(s1));
            } else if to == NoEncoding::Sjis2004 && (0xA1..=0xDF).contains(&s1) {
                ck!(out.output(s1));
            } else {
                ck!(illegal_output(c, f, out));
            }
        } else if s1 < 0x7F00 {
            // JIS X 0213 plane 1
            if to == NoEncoding::Sjis2004 {
                let (a, b) = sjis_encode(((s1 >> 8) & 0xFF) as u32, (s1 & 0xFF) as u32);
                s1 = a as i32;
                s2 = b as i32;
            } else if to == NoEncoding::Eucjp2004 {
                s2 = (s1 & 0xFF) + 0x80;
                s1 = ((s1 >> 8) & 0xFF) + 0x80;
            } else {
                if f.status & 0xFF00 != 0x200 {
                    ck!(emit(out, b"\x1B$(Q"));
                }
                f.status = 0x200;
                s2 = s1 & 0xFF;
                s1 = (s1 >> 8) & 0xFF;
            }
            ck!(out.output(s1));
            ck!(out.output(s2));
        } else {
            // JIS X 0213 plane 2; php-src does not switch the escape sequence
            // when going directly from plane 1 to plane 2 here
            if to == NoEncoding::Sjis2004 {
                let (a, b) = sjis_encode(((s1 >> 8) & 0xFF) as u32, (s1 & 0xFF) as u32);
                s1 = a as i32;
                s2 = b as i32;
            } else {
                s2 = s1 & 0xFF;
                let k = ((s1 >> 8) & 0xFF) - 0x7F;
                if (0..JISX0213_P2_OFST_LEN as i32).contains(&k) {
                    s1 = JISX0213_P2_OFST[k as usize] as i32 + 0x21;
                }
                if to == NoEncoding::Eucjp2004 {
                    s2 |= 0x80;
                    s1 |= 0x80;
                    ck!(out.output(0x8F));
                } else {
                    if f.status & 0xFF00 != 0x200 {
                        ck!(emit(out, b"\x1B$(P"));
                    }
                    f.status = 0x200;
                }
            }

            ck!(out.output(s1));
            ck!(out.output(s2));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

pub fn mbfl_filt_conv_wchar_jis2004_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let k = f.cache;
    f.cache = 0;

    // php-src checks `k <= jisx0213_u2_tbl_len`, but a cached index is
    // always a valid one
    if f.status == 1 && (0..JISX0213_U2_TBL_LEN as i32).contains(&k) {
        let mut s1 = JISX0213_U2_FB_TBL[k as usize] as i32;
        let s2;

        if f.to.no == NoEncoding::Sjis2004 {
            let (a, b) = sjis_encode(((s1 >> 8) & 0xFF) as u32, (s1 & 0xFF) as u32);
            s1 = a as i32;
            s2 = b as i32;
        } else if f.to.no == NoEncoding::Eucjp2004 {
            s2 = (s1 & 0xFF) | 0x80;
            s1 = ((s1 >> 8) & 0xFF) | 0x80;
        } else {
            s2 = s1 & 0x7F;
            s1 = (s1 >> 8) & 0x7F;
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$(Q"));
            }
            f.status = 0x200;
        }

        ck!(out.output(s1));
        ck!(out.output(s2));
    }

    // If we had switched to a different charset, go back to ASCII mode; this
    // makes it possible to concatenate arbitrary valid strings
    if f.status & 0xFF00 != 0 {
        ck!(emit(out, b"\x1B(B"));
    }

    f.status = 0;
    out.flush()
}

pub fn mb_iso2022jp2004_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    // Leave room for the second codepoint of combining sequences
    let limit = out.len() - 1;
    let (mut p, mut o) = (0usize, 0usize);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c > 0x7F {
            out[o] = BAD_INPUT;
            o += 1;
        } else if c == 0x1B {
            if e - p < 2 {
                out[o] = BAD_INPUT;
                o += 1;
                p = e;
                break;
            }
            let c2 = s[p];
            let c3 = s[p + 1];
            p += 2;
            if c2 == b'$' {
                if c3 == b'B' {
                    *state = JISX0208;
                } else if c3 == b'(' {
                    if p == e {
                        out[o] = BAD_INPUT;
                        o += 1;
                        break;
                    }
                    let c4 = s[p];
                    p += 1;
                    if c4 == b'Q' {
                        *state = JISX0213_PLANE1;
                    } else if c4 == b'P' {
                        *state = JISX0213_PLANE2;
                    } else {
                        out[o] = BAD_INPUT;
                        o += 1;
                    }
                } else {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
            } else if c2 == b'(' {
                if c3 == b'B' {
                    *state = ASCII;
                } else {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
            } else {
                p -= 1;
                out[o] = BAD_INPUT;
                o += 1;
            }
        } else if *state >= JISX0208 && (0x21..0x7F).contains(&c) {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            if !(0x21..=0x7E).contains(&c2) {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            if *state == JISX0213_PLANE1 {
                let w1 = ((c as u32) << 8) | c2 as u32;

                // Conversion for combining characters
                if is_jisx0213_combining(w1 as i32) {
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
                let w1 = (c as u32 - 0x21) * 94 + c2 as u32 - 0x21;
                let mut w = JISX0213_UCS_TABLE.get(w1 as usize).map_or(0, |&x| x as u32);

                // Conversion for CJK Unified Ideographs ext.B (U+2XXXX)
                if w == 0 {
                    let k = mbfl_bisec_srch2(w1 as i32, &JISX0213_JIS_U5_KEY, JISX0213_U5_TBL_LEN as usize);
                    if k >= 0 {
                        w = JISX0213_JIS_U5_TBL[k as usize] as u32 + 0x20000;
                    }
                }

                out[o] = if w != 0 { w } else { BAD_INPUT };
            } else if *state == JISX0213_PLANE2 {
                let s1 = c as i32 - 0x21;
                let s2 = c2 as i32 - 0x21;

                if is_jisx0213_p2_row(s1) && s2 < 94 {
                    let k = jisx0213_p2_row_offset(s1);

                    // Check for Japanese chars in BMP
                    let s = (s1 + 94 + k) * 94 + s2;
                    let mut w = JISX0213_UCS_TABLE[s as usize] as u32;

                    // Check for Japanese chars in CJK Unified Ideographs ext.B (U+2XXXX)
                    if w == 0 {
                        let k = mbfl_bisec_srch2(s, &JISX0213_JIS_U5_KEY, JISX0213_U5_TBL_LEN as usize);
                        if k >= 0 {
                            w = JISX0213_JIS_U5_TBL[k as usize] as u32 + 0x20000;
                        }
                    }

                    out[o] = if w != 0 { w } else { BAD_INPUT };
                } else {
                    out[o] = BAD_INPUT;
                }
            } else {
                // JIS X 0208
                let w = jisx0208_ucs((c as u32 - 0x21) * 94 + c2 as u32 - 0x21);
                out[o] = if w != 0 { w } else { BAD_INPUT };
            }
            o += 1;
        } else {
            out[o] = c as u32;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_iso2022jp2004(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let u2_len = JISX0213_U2_TBL_LEN as usize;
    let mut i = 0usize;

    // A codepoint which may combine with the following one was held back at
    // the end of the previous buffer
    let mut pending = None;
    if buf.state & 0xFF00 != 0 {
        let k = ((buf.state >> 8) - 1) as usize;
        pending = Some(JISX0213_U2_TBL[2 * k] as u32);
        buf.state &= 0xFF;
    }

    loop {
        let w = match pending.take() {
            Some(w) => w,
            None if i < input.len() => {
                i += 1;
                input[i - 1]
            }
            None => break,
        };
        let mut s: u32 = 0;

        if is_jisx0213_combining_start(w) {
            if let Some(mut k) = (0..u2_len).find(|&k| w == JISX0213_U2_TBL[2 * k] as u32) {
                if i == input.len() {
                    if !end {
                        buf.state |= ((k + 1) as u32) << 8;
                        return;
                    }
                    s = JISX0213_U2_FB_TBL[k] as u32;
                } else {
                    let w2 = input[i];
                    if (w == 0x254 || w == 0x28C || w == 0x259 || w == 0x25A) && w2 == 0x301 {
                        k += 1;
                    }
                    if w2 == JISX0213_U2_TBL[2 * k + 1] as u32 {
                        i += 1;
                        s = JISX0213_U2_KEY[k] as u32;
                    } else {
                        s = JISX0213_U2_FB_TBL[k] as u32;
                    }
                }
            }
        }

        // Check for major Japanese chars: U+4E00-U+9FFF
        if s == 0 {
            if let Some(k) = UNI2JIS_TBL_RANGE
                .iter()
                .position(|range| (range[0] as u32..=range[1] as u32).contains(&w))
            {
                s = UNI2JIS_TBL[k][(w - UNI2JIS_TBL_RANGE[k][0] as u32) as usize] as u32;
            }
        }

        // Check for Japanese chars in compressed mapping area: U+1E00-U+4DBF
        if s == 0 && (UCS_C1_JISX0213_MIN..=UCS_C1_JISX0213_MAX).contains(&w) {
            let k = mbfl_bisec_srch(w as i32, &UCS_C1_JISX0213_TBL, UCS_C1_JISX0213_TBL_LEN as usize);
            if k >= 0 {
                let k = k as usize;
                s = UCS_C1_JISX0213_OFST[k] as u32 + w - UCS_C1_JISX0213_TBL[2 * k] as u32;
            }
        }

        // Check for Japanese chars in CJK Unified Ideographs ext B (U+2XXXX)
        if s == 0 && (JISX0213_U5_TBL_MIN..=JISX0213_U5_TBL_MAX).contains(&w) {
            let k = mbfl_bisec_srch2((w - 0x20000) as i32, &JISX0213_U5_JIS_KEY, JISX0213_U5_TBL_LEN as usize);
            if k >= 0 {
                s = JISX0213_U5_JIS_TBL[k as usize] as u32;
            }
        }

        if s == 0 {
            // CJK Compatibility Forms: U+FE30-U+FE4F
            if w == 0xFE45 {
                s = 0x233E;
            } else if w == 0xFE46 {
                s = 0x233D;
            } else if (0xF91D..=0xF9DC).contains(&w) {
                // CJK Compatibility Ideographs: U+F900-U+F92A
                let k = mbfl_bisec_srch2(w as i32, &UCS_R2B_JISX0213_CMAP_KEY, UCS_R2B_JISX0213_CMAP_LEN as usize);
                if k >= 0 {
                    s = UCS_R2B_JISX0213_CMAP_VAL[k as usize] as u32;
                }
            }
        }

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_iso2022jp2004, buf);
        } else if s <= 0x7F {
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if s <= 0xFF {
            mb_illegal_output(w, mb_wchar_to_iso2022jp2004, buf);
        } else if s <= 0x7EFF {
            if buf.state != JISX0213_PLANE1 {
                buf.add4(0x1B, b'$', b'(', b'Q');
                buf.state = JISX0213_PLANE1;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
        } else {
            if buf.state != JISX0213_PLANE2 {
                buf.add4(0x1B, b'$', b'(', b'P');
                buf.state = JISX0213_PLANE2;
            }
            let s2 = s & 0xFF;
            let k = (((s >> 8) & 0xFF) - 0x7F) as usize;
            let s1 = JISX0213_P2_OFST[k] as u32 + 0x21;
            buf.add2(s1 as u8, s2 as u8);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

/*
 * CP50220, CP50221 and CP50222
 */

pub fn mbfl_filt_conv_cp5022x_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    loop {
        match f.status & 0xF {
            // Status 0x00: ASCII, 0x10: JIS X 0201 latin, 0x20: JIS X 0201
            // kana, 0x80: JIS X 0208 (CP932), 0x90: JIS X 0212.
            0 => {
                if c == 0x1B {
                    f.status += 2;
                } else if c == 0x0E {
                    // "Kana in"
                    f.status = 0x20;
                } else if c == 0x0F {
                    // "Kana out"
                    f.status = 0;
                } else if f.status == 0x10 && c == 0x5C {
                    ck!(out.output(0xA5)); // YEN SIGN
                } else if f.status == 0x10 && c == 0x7E {
                    ck!(out.output(0x203E)); // OVER LINE
                } else if f.status == 0x20 && (0x21..0x60).contains(&c) {
                    ck!(out.output(0xFF40 + c));
                } else if (f.status == 0x80 || f.status == 0x90) && (0x21..=0x97).contains(&c) {
                    // Kanji first byte
                    f.cache = c;
                    f.status += 1;
                } else if (0..0x80).contains(&c) {
                    ck!(out.output(c));
                } else if (0xA1..0xE0).contains(&c) {
                    // GR kana
                    ck!(out.output(0xFEC0 + c));
                } else {
                    ck!(out.output(BAD));
                }
            }

            // JIS X 0208 / 0212 second byte
            1 => {
                f.status &= !0xF;
                let c1 = f.cache;
                if (0x21..0x7F).contains(&c) {
                    let s = (c1 - 0x21) * 94 + c - 0x21;
                    let mut w;
                    if f.status == 0x80 {
                        w = if (CP932EXT1_UCS_TABLE_MIN as i32..CP932EXT1_UCS_TABLE_MAX as i32).contains(&s) {
                            CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN as i32) as usize] as i32
                        } else if (0..JISX0208_UCS_TABLE_SIZE as i32).contains(&s) {
                            JISX0208_UCS_TABLE[s as usize] as i32
                        } else if (CP932EXT2_UCS_TABLE_MIN as i32..CP932EXT2_UCS_TABLE_MAX as i32).contains(&s) {
                            CP932EXT2_UCS_TABLE[(s - CP932EXT2_UCS_TABLE_MIN as i32) as usize] as i32
                        } else if (CP932EXT3_UCS_TABLE_MIN as i32..CP932EXT3_UCS_TABLE_MAX as i32).contains(&s) {
                            CP932EXT3_UCS_TABLE[(s - CP932EXT3_UCS_TABLE_MIN as i32) as usize] as i32
                        } else if (94 * 94..114 * 94).contains(&s) {
                            // User-defined => PUA (Microsoft extended)
                            s - 94 * 94 + 0xE000
                        } else {
                            0
                        };
                    } else {
                        w = if s >= 0 { jisx0212_ucs(s as u32) as i32 } else { 0 };
                    }
                    if w <= 0 {
                        w = BAD;
                    }
                    ck!(out.output(w));
                } else {
                    ck!(out.output(BAD));
                }
            }

            // ESC
            2 => {
                if c == 0x24 {
                    f.status += 1; // '$'
                } else if c == 0x28 {
                    f.status += 3; // '('
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    continue;
                }
            }

            // ESC $
            3 => {
                if c == 0x40 || c == 0x42 {
                    f.status = 0x80; // '@' or 'B'
                } else if c == 0x28 {
                    f.status += 1; // '('
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    ck!(out.output(0x24));
                    continue;
                }
            }

            // ESC $ (
            4 => {
                if c == 0x40 || c == 0x42 {
                    f.status = 0x80; // '@' or 'B'
                } else if c == 0x44 {
                    f.status = 0x90; // 'D'
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    ck!(out.output(0x24));
                    ck!(out.output(0x28));
                    continue;
                }
            }

            // ESC (
            5 => {
                if c == 0x42 || c == 0x48 {
                    f.status = 0; // 'B' or 'H'
                } else if c == 0x4A {
                    f.status = 0x10; // 'J'
                } else if c == 0x49 {
                    f.status = 0x20; // 'I'
                } else {
                    f.status &= !0xF;
                    ck!(out.output(BAD));
                    ck!(out.output(0x28));
                    continue;
                }
            }

            _ => unreachable!(),
        }
        return 0;
    }
}

pub fn mbfl_filt_conv_cp5022x_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        // 2-byte (JIS X 0208 or 0212) character or escape sequence was truncated
        ck!(out.output(BAD));
    }
    f.status = 0;
    out.flush();
    0
}

static HANKANA2ZENKANA_TABLE: [u8; 64] = [
    0x00, 0x02, 0x0C, 0x0D, 0x01, 0xFB, 0xF2, 0xA1, 0xA3, 0xA5, 0xA7, 0xA9, 0xE3, 0xE5, 0xE7, 0xC3, 0xFC, 0xA2,
    0xA4, 0xA6, 0xA8, 0xAA, 0xAB, 0xAD, 0xAF, 0xB1, 0xB3, 0xB5, 0xB7, 0xB9, 0xBB, 0xBD, 0xBF, 0xC1, 0xC4, 0xC6,
    0xC8, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF, 0xD2, 0xD5, 0xD8, 0xDB, 0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE4, 0xE6,
    0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED, 0xEF, 0xF3, 0x9B, 0x9C,
];

static HANKANA2ZENHIRA_TABLE: [u8; 64] = [
    0x00, 0x02, 0x0C, 0x0D, 0x01, 0xFB, 0x92, 0x41, 0x43, 0x45, 0x47, 0x49, 0x83, 0x85, 0x87, 0x63, 0xFC, 0x42,
    0x44, 0x46, 0x48, 0x4A, 0x4B, 0x4D, 0x4F, 0x51, 0x53, 0x55, 0x57, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x64, 0x66,
    0x68, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x72, 0x75, 0x78, 0x7B, 0x7E, 0x7F, 0x80, 0x81, 0x82, 0x84, 0x86,
    0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D, 0x8F, 0x93, 0x9B, 0x9C,
];

static ZENKANA2HANKANA_TABLE: [[u8; 2]; 84] = [
    [0x67, 0x00], [0x71, 0x00], [0x68, 0x00], [0x72, 0x00], [0x69, 0x00],
    [0x73, 0x00], [0x6A, 0x00], [0x74, 0x00], [0x6B, 0x00], [0x75, 0x00],
    [0x76, 0x00], [0x76, 0x9E], [0x77, 0x00], [0x77, 0x9E], [0x78, 0x00],
    [0x78, 0x9E], [0x79, 0x00], [0x79, 0x9E], [0x7A, 0x00], [0x7A, 0x9E],
    [0x7B, 0x00], [0x7B, 0x9E], [0x7C, 0x00], [0x7C, 0x9E], [0x7D, 0x00],
    [0x7D, 0x9E], [0x7E, 0x00], [0x7E, 0x9E], [0x7F, 0x00], [0x7F, 0x9E],
    [0x80, 0x00], [0x80, 0x9E], [0x81, 0x00], [0x81, 0x9E], [0x6F, 0x00],
    [0x82, 0x00], [0x82, 0x9E], [0x83, 0x00], [0x83, 0x9E], [0x84, 0x00],
    [0x84, 0x9E], [0x85, 0x00], [0x86, 0x00], [0x87, 0x00], [0x88, 0x00],
    [0x89, 0x00], [0x8A, 0x00], [0x8A, 0x9E], [0x8A, 0x9F], [0x8B, 0x00],
    [0x8B, 0x9E], [0x8B, 0x9F], [0x8C, 0x00], [0x8C, 0x9E], [0x8C, 0x9F],
    [0x8D, 0x00], [0x8D, 0x9E], [0x8D, 0x9F], [0x8E, 0x00], [0x8E, 0x9E],
    [0x8E, 0x9F], [0x8F, 0x00], [0x90, 0x00], [0x91, 0x00], [0x92, 0x00],
    [0x93, 0x00], [0x6C, 0x00], [0x94, 0x00], [0x6D, 0x00], [0x95, 0x00],
    [0x6E, 0x00], [0x96, 0x00], [0x97, 0x00], [0x98, 0x00], [0x99, 0x00],
    [0x9A, 0x00], [0x9B, 0x00], [0x9C, 0x00], [0x9C, 0x00], [0x72, 0x00],
    [0x74, 0x00], [0x66, 0x00], [0x9D, 0x00], [0x73, 0x9E],
];

/// `mb_convert_kana_codepoint`: applies the transforms selected by `mode`
/// (`kana::*` flags; it must not ask for transforms which cancel each other
/// out) to `c`, such as halfwidth katakana to fullwidth katakana.
///
/// When `next` (the following codepoint, or 0 at the end) is merged into the
/// result, `*consumed` is set; otherwise it is left alone. When `c` converts
/// to two codepoints, the second one is stored in `*second`. Returns `c`
/// unchanged if no transform applies.
pub fn mb_convert_kana_codepoint(c: u32, next: u32, consumed: Option<&mut bool>, second: &mut u32, mode: u32) -> u32 {
    if mode & kana::HAN2ZEN_ALL != 0
        && (0x21..=0x7D).contains(&c)
        && c != b'"' as u32
        && c != b'\'' as u32
        && c != b'\\' as u32
    {
        return c + 0xFEE0;
    }
    if mode & kana::HAN2ZEN_ALPHA != 0
        && ((b'A' as u32..=b'Z' as u32).contains(&c) || (b'a' as u32..=b'z' as u32).contains(&c))
    {
        return c + 0xFEE0;
    }
    if mode & kana::HAN2ZEN_NUMERIC != 0 && (b'0' as u32..=b'9' as u32).contains(&c) {
        return c + 0xFEE0;
    }
    if mode & kana::HAN2ZEN_SPACE != 0 && c == b' ' as u32 {
        return 0x3000;
    }

    if mode & (kana::HAN2ZEN_KATAKANA | kana::HAN2ZEN_HIRAGANA) != 0 {
        // Convert hankaku kana to zenkaku kana; either all hankaku kana are
        // converted to zenkaku katakana, or to zenkaku hiragana
        let hankana = (0xFF61..=0xFF9F).contains(&c);
        if mode & kana::HAN2ZEN_KATAKANA != 0 && mode & kana::HAN2ZEN_GLUE != 0 && hankana {
            let n = (c - 0xFF60) as usize;

            if (0xFF61..=0xFF9F).contains(&next) {
                if next == 0xFF9E && ((22..=36).contains(&n) || (42..=46).contains(&n)) {
                    if let Some(consumed) = consumed {
                        *consumed = true;
                    }
                    return 0x3001 + HANKANA2ZENKANA_TABLE[n] as u32;
                }
                if next == 0xFF9E && n == 19 {
                    if let Some(consumed) = consumed {
                        *consumed = true;
                    }
                    return 0x30F4;
                }
                if next == 0xFF9F && (42..=46).contains(&n) {
                    if let Some(consumed) = consumed {
                        *consumed = true;
                    }
                    return 0x3002 + HANKANA2ZENKANA_TABLE[n] as u32;
                }
            }

            return 0x3000 + HANKANA2ZENKANA_TABLE[n] as u32;
        }
        if mode & kana::HAN2ZEN_HIRAGANA != 0 && mode & kana::HAN2ZEN_GLUE != 0 && hankana {
            let n = (c - 0xFF60) as usize;

            if (0xFF61..=0xFF9F).contains(&next) {
                if next == 0xFF9E && ((22..=36).contains(&n) || (42..=46).contains(&n)) {
                    if let Some(consumed) = consumed {
                        *consumed = true;
                    }
                    return 0x3001 + HANKANA2ZENHIRA_TABLE[n] as u32;
                }
                if next == 0xFF9F && (42..=46).contains(&n) {
                    if let Some(consumed) = consumed {
                        *consumed = true;
                    }
                    return 0x3002 + HANKANA2ZENHIRA_TABLE[n] as u32;
                }
            }

            return 0x3000 + HANKANA2ZENHIRA_TABLE[n] as u32;
        }
        if mode & kana::HAN2ZEN_KATAKANA != 0 && hankana {
            return 0x3000 + HANKANA2ZENKANA_TABLE[(c - 0xFF60) as usize] as u32;
        }
        if mode & kana::HAN2ZEN_HIRAGANA != 0 && hankana {
            return 0x3000 + HANKANA2ZENHIRA_TABLE[(c - 0xFF60) as usize] as u32;
        }
    }

    if mode & kana::HAN2ZEN_SPECIAL != 0 {
        // Special ASCII to symbol
        if c == b'\\' as u32 || c == 0xA5 {
            return 0xFFE5; // YEN SIGN -> FULLWIDTH YEN SIGN
        }
        if c == 0x7E || c == 0x203E {
            return 0xFFE3; // FULLWIDTH MACRON
        }
        if c == b'\'' as u32 {
            return 0x2019; // RIGHT SINGLE QUOTATION MARK
        }
        if c == b'"' as u32 {
            return 0x201D; // RIGHT DOUBLE QUOTATION MARK
        }
    }

    if mode & (kana::ZEN2HAN_ALL | kana::ZEN2HAN_ALPHA | kana::ZEN2HAN_NUMERIC | kana::ZEN2HAN_SPACE) != 0 {
        // Zenkaku to hankaku
        if mode & kana::ZEN2HAN_ALL != 0 && (0xFF01..=0xFF5D).contains(&c) && c != 0xFF02 && c != 0xFF07 && c != 0xFF3C {
            // All except " ' \ ~
            return c - 0xFEE0;
        }
        if mode & kana::ZEN2HAN_ALPHA != 0 && ((0xFF21..=0xFF3A).contains(&c) || (0xFF41..=0xFF5A).contains(&c)) {
            return c - 0xFEE0;
        }
        if mode & kana::ZEN2HAN_NUMERIC != 0 && (0xFF10..=0xFF19).contains(&c) {
            return c - 0xFEE0;
        }
        if mode & kana::ZEN2HAN_SPACE != 0 && c == 0x3000 {
            return b' ' as u32;
        }
        if mode & kana::ZEN2HAN_ALL != 0 && c == 0x2212 {
            return b'-' as u32; // MINUS SIGN
        }
    }

    if mode & (kana::ZEN2HAN_KATAKANA | kana::ZEN2HAN_HIRAGANA) != 0 {
        // Zenkaku kana to hankaku kana
        if mode & kana::ZEN2HAN_KATAKANA != 0 && (0x30A1..=0x30F4).contains(&c) {
            // Zenkaku katakana to hankaku kana
            let n = (c - 0x30A1) as usize;
            if ZENKANA2HANKANA_TABLE[n][1] != 0 {
                *second = 0xFF00 + ZENKANA2HANKANA_TABLE[n][1] as u32;
            }
            return 0xFF00 + ZENKANA2HANKANA_TABLE[n][0] as u32;
        }
        if mode & kana::ZEN2HAN_HIRAGANA != 0 && (0x3041..=0x3093).contains(&c) {
            // Zenkaku hiragana to hankaku kana
            let n = (c - 0x3041) as usize;
            if ZENKANA2HANKANA_TABLE[n][1] != 0 {
                *second = 0xFF00 + ZENKANA2HANKANA_TABLE[n][1] as u32;
            }
            return 0xFF00 + ZENKANA2HANKANA_TABLE[n][0] as u32;
        }
        match c {
            0x3001 => return 0xFF64, // HALFWIDTH IDEOGRAPHIC COMMA
            0x3002 => return 0xFF61, // HALFWIDTH IDEOGRAPHIC FULL STOP
            0x300C => return 0xFF62, // HALFWIDTH LEFT CORNER BRACKET
            0x300D => return 0xFF63, // HALFWIDTH RIGHT CORNER BRACKET
            0x309B => return 0xFF9E, // HALFWIDTH KATAKANA VOICED SOUND MARK
            0x309C => return 0xFF9F, // HALFWIDTH KATAKANA SEMI-VOICED SOUND MARK
            0x30FC => return 0xFF70, // HALFWIDTH KATAKANA-HIRAGANA PROLONGED SOUND MARK
            0x30FB => return 0xFF65, // HALFWIDTH KATAKANA MIDDLE DOT
            _ => {}
        }
    }

    if mode & (kana::ZENKAKU_HIRA2KATA | kana::ZENKAKU_KATA2HIRA) != 0 {
        if mode & kana::ZENKAKU_HIRA2KATA != 0 && ((0x3041..=0x3093).contains(&c) || c == 0x309D || c == 0x309E) {
            // Zenkaku hiragana to zenkaku katakana
            return c + 0x60;
        }
        if mode & kana::ZENKAKU_KATA2HIRA != 0 && ((0x30A1..=0x30F3).contains(&c) || c == 0x30FD || c == 0x30FE) {
            // Zenkaku katakana to zenkaku hiragana
            return c - 0x60;
        }
    }

    if mode & kana::ZEN2HAN_SPECIAL != 0 {
        // Special symbol to ASCII
        if c == 0xFFE5 || c == 0xFF3C {
            return b'\\' as u32; // FULLWIDTH YEN SIGN / FULLWIDTH REVERSE SOLIDUS
        }
        if c == 0xFFE3 || c == 0x203E {
            return b'~' as u32; // FULLWIDTH MACRON / OVERLINE
        }
        if c == 0x2018 || c == 0x2019 {
            return b'\'' as u32; // LEFT/RIGHT SINGLE QUOTATION MARK
        }
        if c == 0x201C || c == 0x201D {
            return b'"' as u32; // LEFT/RIGHT DOUBLE QUOTATION MARK
        }
    }

    c
}

pub fn mbfl_filt_conv_wchar_cp50220(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mode = kana::HAN2ZEN_KATAKANA | kana::HAN2ZEN_GLUE;
    let mut consumed = false;

    if f.cache != 0 {
        let mut second = 0;
        let s = mb_convert_kana_codepoint(f.cache as u32, c as u32, Some(&mut consumed), &mut second, mode) as i32;
        f.cache = if consumed { 0 } else { c };
        // php-src swaps the filter function while encoding, so that error
        // markers (which `illegal_output` feeds back through it) are emitted
        // in the proper position, not reordered with subsequent characters
        f.function = mbfl_filt_conv_wchar_cp50221;
        mbfl_filt_conv_wchar_cp50221(s, f, out);
        f.function = mbfl_filt_conv_wchar_cp50220;
        if c == 0 && !consumed {
            out.output(0);
        }
    } else if c == 0 {
        // `f.cache == 0` means that nothing is cached, so a NUL is emitted
        // immediately
        out.output(0);
    } else {
        f.cache = c;
    }

    0
}

pub fn mbfl_filt_conv_wchar_cp50220_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mode = kana::HAN2ZEN_KATAKANA | kana::HAN2ZEN_GLUE;

    if f.cache != 0 {
        let mut second = 0;
        let s = mb_convert_kana_codepoint(f.cache as u32, 0, None, &mut second, mode) as i32;
        f.function = mbfl_filt_conv_wchar_cp50221;
        mbfl_filt_conv_wchar_cp50221(s, f, out);
        f.function = mbfl_filt_conv_wchar_cp50220;
        f.cache = 0;
    }

    mbfl_filt_conv_any_jis_flush(f, out)
}

/// The kuten code lookup shared by the legacy CP50221/CP50222 encoders;
/// returns -1 for unencodable codepoints.
fn cp5022x_filter_lookup(c: i32) -> i32 {
    let mut s = if c == 0x203E {
        0x1007E // OVERLINE -> JIS X 0201 OVERLINE
    } else if (0xE000..=0xE757).contains(&c) {
        // 'Private'/'user' codepoints
        pua_to_jis(c as u32) as i32
    } else {
        ucs_to_jis(c as u32) as i32
    };

    if s <= 0 {
        s = jis_compat(c as u32, 0x1005C) as i32;
    }

    // A zero result means the codepoint is not in JIS X 0208; one with the
    // high bits set in both bytes is JIS X 0212. In either case, check the
    // CP932 extensions.
    if s == 0 || ((s & 0x8000) != 0 && (s & 0x80) != 0) {
        s = -1;
        if let Some(i) = cp932ext1_index(c as u32) {
            let oh = CP932EXT1_UCS_TABLE_MIN / 94;
            s = (((i / 94 + oh + 0x21) << 8) + (i % 94 + 0x21)) as i32;
        }
        if s <= 0 {
            if let Some(i) = cp932ext2_index(c as u32) {
                let oh = CP932EXT2_UCS_TABLE_MIN / 94;
                s = (((i / 94 + oh + 0x21) << 8) + (i % 94 + 0x21)) as i32;
            }
        }

        if c == 0 {
            s = 0;
        } else if s <= 0 {
            s = -1;
        }
    }

    s
}

pub fn mbfl_filt_conv_wchar_cp50221(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let s = cp5022x_filter_lookup(c);

    if s >= 0 {
        if s < 0x80 {
            // ASCII
            if f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
                f.status = 0;
            }
            ck!(out.output(s));
        } else if (0xA0..0xE0).contains(&s) {
            // JIS X 0201 kana
            if f.status & 0xFF00 != 0x500 {
                ck!(emit(out, b"\x1B(I"));
                f.status = 0x500;
            }
            ck!(out.output(s - 0x80));
        } else if s <= 0x927E {
            // JIS X 0208 + extensions
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$B"));
                f.status = 0x200;
            }
            ck!(out.output((s >> 8) & 0xFF));
            ck!(out.output(s & 0xFF));
        } else if s < 0x10000 {
            // JIS X 0212
            ck!(illegal_output(c, f, out));
        } else {
            // JIS X 0201 latin
            if f.status & 0xFF00 != 0x400 {
                ck!(emit(out, b"\x1B(J"));
            }
            f.status = 0x400;
            ck!(out.output(s & 0x7F));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

pub fn mbfl_filt_conv_wchar_cp50222(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let s = cp5022x_filter_lookup(c);

    if s >= 0 {
        if s < 0x80 {
            // ASCII
            if f.status & 0xFF00 == 0x500 {
                ck!(out.output(0x0F)); // SO
                f.status = 0;
            } else if f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
                f.status = 0;
            }
            ck!(out.output(s));
        } else if (0xA0..0xE0).contains(&s) {
            // JIS X 0201 kana
            if f.status & 0xFF00 != 0x500 {
                ck!(out.output(0x0E)); // SI
                f.status = 0x500;
            }
            ck!(out.output(s - 0x80));
        } else if s <= 0x927E {
            // JIS X 0208
            if f.status & 0xFF00 == 0x500 {
                ck!(out.output(0x0F)); // SO
                f.status = 0;
            }
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$B"));
                f.status = 0x200;
            }
            ck!(out.output((s >> 8) & 0xFF));
            ck!(out.output(s & 0xFF));
        } else if s < 0x10000 {
            // JIS X 0212
            ck!(illegal_output(c, f, out));
        } else {
            // JIS X 0201 latin
            if f.status & 0xFF00 == 0x500 {
                ck!(out.output(0x0F)); // SO
                f.status = 0;
            }
            if f.status & 0xFF00 != 0x400 {
                ck!(emit(out, b"\x1B(J"));
            }
            f.status = 0x400;
            ck!(out.output(s & 0x7F));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

pub fn mbfl_filt_conv_wchar_cp50222_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // Back to latin
    if f.status & 0xFF00 == 0x500 {
        ck!(out.output(0x0F)); // SO
    } else if f.status & 0xFF00 != 0 {
        ck!(emit(out, b"\x1B(B"));
    }
    f.status = 0;
    out.flush();
    0
}

pub fn mb_cp5022x_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    debug_assert!(out.len() >= 3);
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0usize, 0usize);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == 0x1B {
            // Escape sequence
            if e - p < 2 {
                out[o] = BAD_INPUT;
                o += 1;
                // Duplicate error-handling behavior of legacy code
                if p < e && (s[p] == b'(' || s[p] == b'$') {
                    p += 1;
                }
                continue;
            }
            let c2 = s[p];
            p += 1;
            if c2 == b'$' {
                let c3 = s[p];
                p += 1;
                if c3 == b'@' || c3 == b'B' {
                    *state = JISX_0208;
                } else if c3 == b'(' {
                    if p == e {
                        out[o] = BAD_INPUT;
                        o += 1;
                        break;
                    }
                    let c4 = s[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        *state = JISX_0208;
                    } else if c4 == b'D' {
                        *state = JISX_0212;
                    } else {
                        if limit - o < 3 {
                            p -= 4;
                            break;
                        }
                        out[o..o + 3].copy_from_slice(&[BAD_INPUT, b'$' as u32, b'(' as u32]);
                        o += 3;
                        p -= 1;
                    }
                } else {
                    if limit - o < 2 {
                        p -= 3;
                        break;
                    }
                    out[o..o + 2].copy_from_slice(&[BAD_INPUT, b'$' as u32]);
                    o += 2;
                    p -= 1;
                }
            } else if c2 == b'(' {
                let c3 = s[p];
                p += 1;
                if c3 == b'B' || c3 == b'H' {
                    *state = ASCII;
                } else if c3 == b'J' {
                    *state = JISX_0201_LATIN;
                } else if c3 == b'I' {
                    *state = JISX_0201_KANA;
                } else {
                    if limit - o < 2 {
                        p -= 3;
                        break;
                    }
                    out[o..o + 2].copy_from_slice(&[BAD_INPUT, b'(' as u32]);
                    o += 2;
                    p -= 1;
                }
            } else {
                out[o] = BAD_INPUT;
                o += 1;
                p -= 1;
            }
        } else if c == 0xE {
            *state = JISX_0201_KANA;
        } else if c == 0xF {
            *state = ASCII;
        } else if *state == JISX_0201_LATIN && c == 0x5C {
            out[o] = 0xA5; // YEN SIGN
            o += 1;
        } else if *state == JISX_0201_LATIN && c == 0x7E {
            out[o] = 0x203E; // OVER LINE
            o += 1;
        } else if *state == JISX_0201_KANA && (0x21..0x60).contains(&c) {
            out[o] = 0xFF40 + c as u32;
            o += 1;
        } else if *state >= JISX_0208 && (0x21..=0x97).contains(&c) {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            if (0x21..0x7F).contains(&c2) {
                let s = (c as u32 - 0x21) * 94 + c2 as u32 - 0x21;
                let w = if *state == JISX_0208 {
                    if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&s) {
                        CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN) as usize] as u32
                    } else if s < JISX0208_UCS_TABLE_SIZE {
                        JISX0208_UCS_TABLE[s as usize] as u32
                    } else if (CP932EXT2_UCS_TABLE_MIN..CP932EXT2_UCS_TABLE_MAX).contains(&s) {
                        CP932EXT2_UCS_TABLE[(s - CP932EXT2_UCS_TABLE_MIN) as usize] as u32
                    } else if (CP932EXT3_UCS_TABLE_MIN..CP932EXT3_UCS_TABLE_MAX).contains(&s) {
                        CP932EXT3_UCS_TABLE[(s - CP932EXT3_UCS_TABLE_MIN) as usize] as u32
                    } else if (94 * 94..114 * 94).contains(&s) {
                        // Microsoft extension
                        s - 94 * 94 + 0xE000
                    } else {
                        0
                    }
                } else {
                    jisx0212_ucs(s)
                };
                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
            o += 1;
        } else if c < 0x80 {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

/// `lookup_wchar`: kuten code of `w` for CP5022x; JIS X 0201 latin codes are
/// tagged by adding 0x10000. 0 if not encodable (or `w` is 0).
fn lookup_wchar(w: u32) -> u32 {
    let mut s = if w == 0x203E {
        0x1007E // OVERLINE -> JIS X 0201 OVERLINE
    } else if (0xE000..=0xE757).contains(&w) {
        // Private Use Area codepoints
        pua_to_jis(w)
    } else {
        ucs_to_jis(w)
    };

    if s == 0 {
        s = jis_compat(w, 0x1005C);
        if s == 0 && w == 0 {
            return 0;
        }
    }

    // A zero result means the codepoint is not in JIS X 0208; one >= 0x8080
    // is JIS X 0212 (or JIS X 0201 latin). In either case, check the CP932
    // extensions.
    if s == 0 || s >= 0x8080 {
        if let Some(i) = cp932ext1_index(w) {
            return (((i / 94) + (CP932EXT1_UCS_TABLE_MIN / 94) + 0x21) << 8) + (i % 94) + 0x21;
        }
        if let Some(i) = cp932ext2_index(w) {
            return (((i / 94) + (CP932EXT2_UCS_TABLE_MIN / 94) + 0x21) << 8) + (i % 94) + 0x21;
        }
    }

    s
}

pub fn mb_wchar_to_cp50220(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let mut i = 0usize;

    // Reprocess a codepoint cached at the end of the previous buffer
    let mut pending = None;
    if buf.state & 0xFFFF00 != 0 {
        pending = Some(buf.state >> 8);
        buf.state &= 0xFF;
    }

    loop {
        let w = match pending.take() {
            Some(w) => w,
            None if i < input.len() => {
                i += 1;
                input[i - 1]
            }
            None => break,
        };
        let remaining = input.len() - i;

        if (0xFF61..=0xFF9F).contains(&w) && remaining == 0 && !end {
            // This codepoint may need to combine with the next one, but the
            // 'next one' will come in a separate buffer
            buf.state |= w << 8;
            break;
        }

        let mut consumed = false;
        let mut second = 0;
        let next = if remaining > 0 { input[i] } else { 0 };
        let w = mb_convert_kana_codepoint(
            w,
            next,
            Some(&mut consumed),
            &mut second,
            kana::HAN2ZEN_KATAKANA | kana::HAN2ZEN_GLUE,
        );
        if consumed {
            // Two successive codepoints were converted into one
            i += 1;
        }

        let s = lookup_wchar(w);

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_cp50221, buf);
        } else if s < 0x80 {
            // ASCII
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if (0xA0..0xE0).contains(&s) {
            // JIS X 0201 kana
            if buf.state != JISX_0201_KANA {
                buf.add3(0x1B, b'(', b'I');
                buf.state = JISX_0201_KANA;
            }
            buf.add((s - 0x80) as u8);
        } else if s <= 0x927E {
            // JIS X 0208 kanji
            if buf.state != JISX_0208 {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX_0208;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
        } else if s >= 0x10000 {
            // JIS X 0201 latin; tagged by adding 0x10000
            if buf.state != JISX_0201_LATIN {
                buf.add3(0x1B, b'(', b'J');
                buf.state = JISX_0201_LATIN;
            }
            buf.add((s & 0x7F) as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_cp50221, buf);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

pub fn mb_wchar_to_cp50221(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let s = lookup_wchar(w);

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_cp50221, buf);
        } else if s < 0x80 {
            // ASCII
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if (0xA0..0xE0).contains(&s) {
            // JIS X 0201 kana
            if buf.state != JISX_0201_KANA {
                buf.add3(0x1B, b'(', b'I');
                buf.state = JISX_0201_KANA;
            }
            buf.add((s - 0x80) as u8);
        } else if s <= 0x927E {
            // JIS X 0208 kanji
            if buf.state != JISX_0208 {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX_0208;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
        } else if s >= 0x10000 {
            // JIS X 0201 latin; tagged by adding 0x10000
            if buf.state != JISX_0201_LATIN {
                buf.add3(0x1B, b'(', b'J');
                buf.state = JISX_0201_LATIN;
            }
            buf.add((s & 0x7F) as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_cp50221, buf);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

pub fn mb_wchar_to_cp50222(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let s = lookup_wchar(w);

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_cp50222, buf);
        } else if s < 0x80 {
            // ASCII
            if buf.state == JISX_0201_KANA {
                buf.add(0xF);
                buf.state = ASCII;
            } else if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if (0xA0..0xE0).contains(&s) {
            // JIS X 0201 kana
            if buf.state != JISX_0201_KANA {
                buf.add(0xE);
                buf.state = JISX_0201_KANA;
            }
            buf.add((s - 0x80) as u8);
        } else if s <= 0x927E {
            // JIS X 0208 kanji
            if buf.state == JISX_0201_KANA {
                buf.add(0xF);
            }
            if buf.state != JISX_0208 {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX_0208;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
        } else if s >= 0x10000 {
            // JIS X 0201 latin; tagged by adding 0x10000
            if buf.state == JISX_0201_KANA {
                buf.add(0xF);
            }
            if buf.state != JISX_0201_LATIN {
                buf.add3(0x1B, b'(', b'J');
                buf.state = JISX_0201_LATIN;
            }
            buf.add((s & 0x7F) as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_cp50222, buf);
        }
    }

    if end {
        if buf.state == JISX_0201_KANA {
            buf.add(0xF);
        } else if buf.state != ASCII {
            buf.add3(0x1B, b'(', b'B');
        }
    }
}

/*
 * ISO-2022-JP-MS
 */

pub fn mbfl_filt_conv_2022jpms_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    match f.status & 0xF {
        0 => {
            if c == 0x1B {
                f.status += 2;
            } else if f.status == JISX0201_KANA as i32 && (0x21..0x60).contains(&c) {
                ck!(out.output(0xFF40 + c));
            } else if (f.status == JISX0208_KANJI as i32 || f.status == UDC as i32) && (0x21..0x80).contains(&c) {
                f.cache = c;
                f.status += 1;
            } else if (0..0x80).contains(&c) {
                ck!(out.output(c)); // ASCII
            } else if (0xA1..0xE0).contains(&c) {
                ck!(out.output(0xFEC0 + c)); // Kana
            } else {
                ck!(out.output(BAD));
            }
        }

        // Kanji, second byte
        1 => {
            let mut w = 0;
            f.status &= !0xF;
            let c1 = f.cache;
            if (0x21..0x7F).contains(&c) {
                let s = ((c1 - 0x21) * 94) + c - 0x21;
                if f.status == JISX0208_KANJI as i32 {
                    if s <= 137 {
                        w = jis_ms_fullwidth(s as u32) as i32;
                    }

                    if w == 0 {
                        if (CP932EXT1_UCS_TABLE_MIN as i32..CP932EXT1_UCS_TABLE_MAX as i32).contains(&s) {
                            // Vendor ext1 (13ku)
                            w = CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN as i32) as usize] as i32;
                        } else if (0..JISX0208_UCS_TABLE_SIZE as i32).contains(&s) {
                            w = JISX0208_UCS_TABLE[s as usize] as i32;
                        } else if (CP932EXT2_UCS_TABLE_MIN as i32..CP932EXT2_UCS_TABLE_MAX as i32).contains(&s) {
                            // Vendor ext2 (89ku - 92ku)
                            w = CP932EXT2_UCS_TABLE[(s - CP932EXT2_UCS_TABLE_MIN as i32) as usize] as i32;
                        }
                    }

                    if w <= 0 {
                        w = BAD;
                    }
                } else if (0x21..0x35).contains(&c1) {
                    w = 0xE000 + ((c1 - 0x21) * 94) + c - 0x21;
                } else {
                    w = BAD;
                }
                ck!(out.output(w));
            } else {
                ck!(out.output(BAD));
            }
        }

        // ESC
        2 => {
            if c == b'$' as i32 {
                f.status += 1;
            } else if c == b'(' as i32 {
                f.status += 3;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $
        3 => {
            if c == b'@' as i32 || c == b'B' as i32 {
                f.status = JISX0208_KANJI as i32;
            } else if c == b'(' as i32 {
                f.status += 1;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $ (
        4 => {
            if c == b'@' as i32 || c == b'B' as i32 {
                f.status = JISX0208_KANJI as i32;
            } else if c == b'?' as i32 {
                f.status = UDC as i32;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC (
        5 => {
            if c == b'B' as i32 || c == b'J' as i32 {
                f.status = 0;
            } else if c == b'I' as i32 {
                f.status = JISX0201_KANA as i32;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        _ => {}
    }

    0
}

pub fn mbfl_filt_conv_2022jpms_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        out.output(BAD);
    }
    f.status = 0;
    out.flush();
    0
}

/// `sjistoidx(c1, c2)`: index of a Shift-JIS code in rows of 188.
const fn sjistoidx(c1: i32, c2: i32) -> i32 {
    let base = if c1 > 0x9F { (c1 - 0xC1) * 188 } else { (c1 - 0x81) * 188 };
    base + c2 - if c2 > 0x7E { 0x41 } else { 0x40 }
}

/// `cp932ext3_cp932ext2_jis`: JIS code of the NEC-selected IBM extension
/// equivalent to entry `c` of `cp932ext3_ucs_table` (IBM extensions).
fn cp932ext3_cp932ext2_jis(c: i32) -> i32 {
    let mut idx = sjistoidx(0xFA, 0x40) + c;
    if idx >= sjistoidx(0xFA, 0x5C) {
        idx -= sjistoidx(0xFA, 0x5C) - sjistoidx(0xED, 0x40);
    } else if idx >= sjistoidx(0xFA, 0x55) {
        idx -= sjistoidx(0xFA, 0x55) - sjistoidx(0xEE, 0xFA);
    } else if idx >= sjistoidx(0xFA, 0x40) {
        idx -= sjistoidx(0xFA, 0x40) - sjistoidx(0xEE, 0xEF);
    }
    (((idx / 94) + 0x21) << 8) | ((idx % 94) + 0x21)
}

pub fn mbfl_filt_conv_wchar_2022jpms(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut s1 = if (0xE000..0xE000 + 20 * 94).contains(&c) {
        // Private User Area (95ku - 114ku)
        pua_to_jis(c as u32) as i32
    } else {
        ucs_to_jis(c as u32) as i32
    };

    if s1 <= 0 {
        s1 = jis_compat(c as u32, 0x216F) as i32; // YEN SIGN -> FULLWIDTH YEN SIGN
    }

    if s1 <= 0 || s1 >= 0xA1A1 {
        // Not found or JIS X 0212
        s1 = cp932ext1_index(c as u32).map_or(-1, |i| cp932ext1_jis(i) as i32);

        if s1 <= 0 {
            if let Some(i) = cp932ext3_index(c as u32) {
                s1 = cp932ext3_cp932ext2_jis(i as i32);
            }
        }

        if c == 0 {
            s1 = 0;
        }
    }

    if s1 >= 0 {
        if s1 < 0x80 {
            // Latin
            if f.status & 0xFF00 != 0 {
                ck!(emit(out, b"\x1B(B"));
            }
            ck!(out.output(s1));
            f.status = 0;
        } else if s1 > 0xA0 && s1 < 0xE0 {
            // Kana
            if f.status & 0xFF00 != 0x100 {
                ck!(emit(out, b"\x1B(I"));
            }
            f.status = 0x100;
            ck!(out.output(s1 & 0x7F));
        } else if s1 < 0x7E7F {
            // JIS X 0208
            if f.status & 0xFF00 != 0x200 {
                ck!(emit(out, b"\x1B$B"));
            }
            f.status = 0x200;
            ck!(out.output((s1 >> 8) & 0xFF));
            ck!(out.output(s1 & 0x7F));
        } else if s1 < 0x927F {
            // UDC
            if f.status & 0xFF00 != 0x800 {
                ck!(emit(out, b"\x1B$(?"));
            }
            f.status = 0x800;
            ck!(out.output(((s1 >> 8) - 0x5E) & 0x7F));
            ck!(out.output(s1 & 0x7F));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

pub fn mbfl_filt_conv_any_2022jpms_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // Go back to ASCII (so strings can be safely concatenated)
    if f.status & 0xFF00 != 0 {
        ck!(emit(out, b"\x1B(B"));
    }
    f.status = 0;
    out.flush();
    0
}

pub fn mb_iso2022jpms_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0usize, 0usize);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == 0x1B {
            if e - p < 2 {
                out[o] = BAD_INPUT;
                o += 1;
                p = e;
                break;
            }
            let c2 = s[p];
            let c3 = s[p + 1];
            p += 2;

            if c2 == b'$' {
                if c3 == b'@' || c3 == b'B' {
                    *state = JISX0208_KANJI;
                } else if c3 == b'(' && p < e {
                    let c4 = s[p];
                    p += 1;
                    if c4 == b'@' || c4 == b'B' {
                        *state = JISX0208_KANJI;
                    } else if c4 == b'?' {
                        *state = UDC;
                    } else {
                        out[o] = BAD_INPUT;
                        o += 1;
                    }
                } else {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
            } else if c2 == b'(' {
                if c3 == b'B' || c3 == b'J' {
                    *state = ASCII;
                } else if c3 == b'I' {
                    *state = JISX0201_KANA;
                } else {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
            } else {
                p -= 1;
                out[o] = BAD_INPUT;
                o += 1;
            }
        } else if *state == JISX0201_KANA && (0x21..=0x5F).contains(&c) {
            out[o] = 0xFF40 + c as u32;
            o += 1;
        } else if (*state == JISX0208_KANJI || *state == UDC) && (0x21..=0x7F).contains(&c) {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let mut w = 0u32;

            if (0x21..=0x7E).contains(&c2) {
                let s = ((c as u32 - 0x21) * 94) + c2 as u32 - 0x21;
                if *state == JISX0208_KANJI {
                    if s <= 137 {
                        w = jis_ms_fullwidth(s);
                    }

                    if w == 0 {
                        if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&s) {
                            w = CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN) as usize] as u32;
                        } else if s < JISX0208_UCS_TABLE_SIZE {
                            w = JISX0208_UCS_TABLE[s as usize] as u32;
                        } else if (CP932EXT2_UCS_TABLE_MIN..CP932EXT2_UCS_TABLE_MAX).contains(&s) {
                            w = CP932EXT2_UCS_TABLE[(s - CP932EXT2_UCS_TABLE_MIN) as usize] as u32;
                        }
                    }
                } else if (0x21..=0x34).contains(&c) {
                    w = 0xE000 + ((c as u32 - 0x21) * 94) + c2 as u32 - 0x21;
                }

                out[o] = if w != 0 { w } else { BAD_INPUT };
            } else {
                out[o] = BAD_INPUT;
            }
            o += 1;
        } else if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_iso2022jpms(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    for &w in input {
        let mut s = if (0xE000..0xE000 + 20 * 94).contains(&w) {
            // Private User Area (95ku - 114ku)
            pua_to_jis(w)
        } else {
            ucs_to_jis(w)
        };

        if s == 0 {
            s = jis_compat(w, 0x216F); // YEN SIGN -> FULLWIDTH YEN SIGN
        }

        if s >= 0xA1A1 {
            // JIS X 0212
            s = 0;
        }

        if s == 0 && w != 0 {
            s = cp932ext1_index(w)
                .map(cp932ext1_jis)
                .or_else(|| cp932ext3_index(w).map(|i| cp932ext3_cp932ext2_jis(i as i32) as u32))
                .unwrap_or(0);
        }

        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_iso2022jpms, buf);
        } else if s <= 0x7F {
            if buf.state != ASCII {
                buf.add3(0x1B, b'(', b'B');
                buf.state = ASCII;
            }
            buf.add(s as u8);
        } else if (0xA1..=0xDF).contains(&s) {
            if buf.state != JISX0201_KANA {
                buf.add3(0x1B, b'(', b'I');
                buf.state = JISX0201_KANA;
            }
            buf.add((s & 0x7F) as u8);
        } else if s <= 0x7E7E {
            if buf.state != JISX0208_KANJI {
                buf.add3(0x1B, b'$', b'B');
                buf.state = JISX0208_KANJI;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0x7F) as u8);
        } else if s < 0x927F {
            if buf.state != UDC {
                buf.add4(0x1B, b'$', b'(', b'?');
                buf.state = UDC;
            }
            buf.add2((((s >> 8) - 0x5E) & 0x7F) as u8, (s & 0x7F) as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_iso2022jpms, buf);
        }
    }

    if end && buf.state != ASCII {
        buf.add3(0x1B, b'(', b'B');
    }
}

/*
 * ISO-2022-KR (RFC 1557)
 *
 * The RFC says that ESC $ ) C must appear once in an ISO-2022-KR string, at
 * the beginning of a line, before any Shift In/Shift Out bytes (used to
 * switch between ASCII/KSC 5601). That is not enforced for input.
 */

pub fn mbfl_filt_conv_2022kr_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    match f.status & 0xF {
        // Status 0x00: ASCII, 0x10: KSC5601
        0 => {
            if c == 0x1B {
                f.status += 2; // ESC
            } else if c == 0x0F {
                f.status = 0; // Shift In (ASCII)
            } else if c == 0x0E {
                f.status = 0x10; // Shift Out (KSC5601)
            } else if f.status & 0x10 != 0 && (0x21..0x7F).contains(&c) {
                // KSC5601 lead byte
                f.cache = c;
                f.status = 0x11;
            } else if f.status & 0x10 == 0 && (0..0x80).contains(&c) {
                ck!(out.output(c)); // Latin, CTLs
            } else {
                ck!(out.output(BAD));
            }
        }

        // DBCS second byte
        1 => {
            f.status = 0x10;
            let c1 = f.cache;
            let mut w = 0;
            let flag = if (0x21..0x47).contains(&c1) {
                1
            } else if (0x47..=0x7E).contains(&c1) && c1 != 0x49 {
                2
            } else {
                0
            };

            if flag > 0 && (0x21..0x7F).contains(&c) {
                if flag == 1 {
                    if c1 != 0x22 || c <= 0x65 {
                        w = (c1 - 1) * 190 + (c - 0x41) + 0x80;
                        w = UHC1_UCS_TABLE[w as usize] as i32;
                    }
                } else {
                    w = (c1 - 0x47) * 94 + c - 0x21;
                    w = UHC3_UCS_TABLE.get(w as usize).map_or(BAD, |&x| x as i32);
                }

                if w <= 0 {
                    w = BAD;
                }
                ck!(out.output(w));
            } else {
                ck!(out.output(BAD));
            }
        }

        // ESC
        2 => {
            if c == b'$' as i32 {
                f.status += 1;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $
        3 => {
            if c == b')' as i32 {
                f.status += 1;
            } else {
                f.status &= !0xF;
                ck!(out.output(BAD));
            }
        }

        // ESC $ )
        4 => {
            f.status = 0;
            if c != b'C' as i32 {
                ck!(out.output(BAD));
            }
        }

        _ => unreachable!(),
    }

    0
}

pub fn mbfl_filt_conv_2022kr_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        // 2-byte character was truncated
        ck!(out.output(BAD));
    }
    f.status = 0;
    out.flush();
    0
}

/// Lookup in the `ucs_*_uhc_table`s; 0 when unmapped.
fn ucs_to_uhc(w: u32) -> u32 {
    let lookup = |tbl: &[u16], min: u32| tbl[(w - min) as usize] as u32;
    if (UCS_A1_UHC_TABLE_MIN..UCS_A1_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_A1_UHC_TABLE, UCS_A1_UHC_TABLE_MIN)
    } else if (UCS_A2_UHC_TABLE_MIN..UCS_A2_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_A2_UHC_TABLE, UCS_A2_UHC_TABLE_MIN)
    } else if (UCS_A3_UHC_TABLE_MIN..UCS_A3_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_A3_UHC_TABLE, UCS_A3_UHC_TABLE_MIN)
    } else if (UCS_I_UHC_TABLE_MIN..UCS_I_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_I_UHC_TABLE, UCS_I_UHC_TABLE_MIN)
    } else if (UCS_S_UHC_TABLE_MIN..UCS_S_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_S_UHC_TABLE, UCS_S_UHC_TABLE_MIN)
    } else if (UCS_R1_UHC_TABLE_MIN..UCS_R1_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_R1_UHC_TABLE, UCS_R1_UHC_TABLE_MIN)
    } else if (UCS_R2_UHC_TABLE_MIN..UCS_R2_UHC_TABLE_MAX).contains(&w) {
        lookup(&UCS_R2_UHC_TABLE, UCS_R2_UHC_TABLE_MIN)
    } else {
        0
    }
}

pub fn mbfl_filt_conv_wchar_2022kr(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0x100 == 0 {
        ck!(emit(out, b"\x1B$)C"));
        f.status |= 0x100;
    }

    let mut s = ucs_to_uhc(c as u32) as i32;

    let c1 = (s >> 8) & 0xFF;
    let c2 = s & 0xFF;
    // Exclude the UHC extension area
    if c1 < 0xA1 || c2 < 0xA1 {
        s = c;
    } else if s & 0x8000 != 0 {
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
            if f.status & 0x10 != 0 {
                ck!(out.output(0x0F)); // Shift In
                f.status &= !0x10;
            }
            ck!(out.output(s));
        } else {
            if f.status & 0x10 == 0 {
                ck!(out.output(0x0E)); // Shift Out
                f.status |= 0x10;
            }
            ck!(out.output((s >> 8) & 0xFF));
            ck!(out.output(s & 0xFF));
        }
    } else {
        ck!(illegal_output(c, f, out));
    }

    0
}

pub fn mbfl_filt_conv_any_2022kr_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xF != 0 {
        // Escape sequence or 2-byte character was truncated
        out.output(BAD);
    }
    // Back to ASCII
    if f.status & 0x10 != 0 {
        ck!(out.output(0x0F)); // Shift In
    }

    f.status = 0;
    f.cache = 0;
    out.flush()
}

pub fn mb_iso2022kr_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0usize, 0usize);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == 0x1B {
            if e - p < 3 {
                out[o] = BAD_INPUT;
                o += 1;
                // Skip what there is of a truncated ESC $ ) sequence
                if p < e {
                    p += 1;
                    if s[p - 1] == b'$' && p < e {
                        p += 1;
                    }
                }
                continue;
            }
            let (c2, c3, c4) = (s[p], s[p + 1], s[p + 2]);
            p += 3;
            if c2 == b'$' && c3 == b')' && c4 == b'C' {
                *state = ASCII;
            } else {
                if c3 != b')' {
                    p -= 1;
                    if c2 != b'$' {
                        p -= 1;
                    }
                }
                out[o] = BAD_INPUT;
                o += 1;
            }
        } else if c == 0xF {
            *state = ASCII;
        } else if c == 0xE {
            *state = KSC5601;
        } else if (0x21..=0x7E).contains(&c) && *state == KSC5601 {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let mut w = 0u32;

            if !(0x21..=0x7E).contains(&c2) {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            if c < 0x47 {
                if c != 0x22 || c2 <= 0x65 {
                    w = UHC1_UCS_TABLE[((c as u32 - 1) * 190 + c2 as u32 - 0x41 + 0x80) as usize] as u32;
                }
            } else if c != 0x49 && c <= 0x7D {
                w = UHC3_UCS_TABLE[((c as u32 - 0x47) * 94 + c2 as u32 - 0x21) as usize] as u32;
            }

            out[o] = if w == 0 { BAD_INPUT } else { w };
            o += 1;
        } else if c < 0x80 && *state == ASCII {
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

pub fn mb_wchar_to_iso2022kr(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    // ESC $ ) C needs to come *somewhere* at the beginning of a line before
    // the Shift In/Shift Out bytes are used, but only once per string; the
    // beginning of the output is always "the beginning of a line"
    if !input.is_empty() && buf.state & EMITTED_ESC_SEQUENCE == 0 {
        buf.add4(0x1B, b'$', b')', b'C');
        buf.state |= EMITTED_ESC_SEQUENCE;
    }

    for &w in input {
        let mut s = ucs_to_uhc(w);

        if ((s >> 8) & 0xFF) < 0xA1 || (s & 0xFF) < 0xA1 {
            s = w;
        } else {
            s -= 0x8080;
        }

        if (0x80..0x2121).contains(&s) || s > 0x8080 {
            mb_illegal_output(w, mb_wchar_to_iso2022kr, buf);
        } else if s < 0x80 {
            if buf.state & 1 != ASCII {
                buf.add(0xF);
                buf.state &= !KSC5601;
            }
            buf.add(s as u8);
        } else {
            if buf.state & 1 != KSC5601 {
                buf.add(0xE);
                buf.state |= KSC5601;
            }
            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
        }
    }

    if end && buf.state & 1 != ASCII {
        buf.add(0xF);
    }
}
