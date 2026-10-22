//! Shift-JIS variants of `libmbfl/filters/mbfilter_cjk.c`: SJIS, SJIS-mac
//! (MacJapanese), the SJIS-Mobile encodings of DOCOMO, KDDI and SoftBank
//! (Windows-932 plus vendor emoji), SJIS-2004 (JIS X 0213), CP932 and
//! SJIS-win.
//!
//! Also holds the JIS lookups shared with [`super::eucjp`].

use super::{
    NFLAGS_CODE_KDDI, NFLAGS_CODE_SB, NFLAGS_S, mbfilter_sjis_emoji_docomo2unicode,
    mbfilter_sjis_emoji_kddi2unicode, mbfilter_sjis_emoji_sb2unicode,
    mbfl_binary_search_paired_sorted_table, mbfl_bisec_srch, mbfl_bisec_srch2, nflags, sjis_encode,
};
use crate::mbstring::filters::mblen;
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::tables::emoji2uni::*;
use crate::mbstring::tables::sjis_mac2uni::*;
use crate::mbstring::tables::unicode_table_cp932_ext::*;
use crate::mbstring::tables::unicode_table_jis::*;
use crate::mbstring::tables::unicode_table_jis2004::*;

pub static MBFL_ENCODING_SJIS: Encoding = Encoding {
    no: NoEncoding::Sjis,
    name: "SJIS",
    mime_name: Some("Shift_JIS"),
    aliases: &["x-sjis", "SHIFT-JIS"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_to_wchar,
    from_wchar: mb_wchar_to_sjis,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_MAC: Encoding = Encoding {
    no: NoEncoding::SjisMac,
    name: "SJIS-mac",
    mime_name: Some("Shift_JIS"),
    aliases: &["MacJapanese", "x-Mac-Japanese"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJISMAC),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjismac_to_wchar,
    from_wchar: mb_wchar_to_sjismac,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_DOCOMO: Encoding = Encoding {
    no: NoEncoding::SjisDocomo,
    name: "SJIS-Mobile#DOCOMO",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-DOCOMO", "shift_jis-imode", "x-sjis-emoji-docomo"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_docomo_to_wchar,
    from_wchar: mb_wchar_to_sjis_docomo,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_KDDI: Encoding = Encoding {
    no: NoEncoding::SjisKddi,
    name: "SJIS-Mobile#KDDI",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-KDDI", "shift_jis-kddi", "x-sjis-emoji-kddi"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_kddi_to_wchar,
    from_wchar: mb_wchar_to_sjis_kddi,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_SB: Encoding = Encoding {
    no: NoEncoding::SjisSb,
    name: "SJIS-Mobile#SOFTBANK",
    mime_name: Some("Shift_JIS"),
    aliases: &[
        "SJIS-SOFTBANK",
        "shift_jis-softbank",
        "x-sjis-emoji-softbank",
    ],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_sb_to_wchar,
    from_wchar: mb_wchar_to_sjis_sb,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS2004: Encoding = Encoding {
    no: NoEncoding::Sjis2004,
    name: "SJIS-2004",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS2004", "Shift_JIS-2004"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis2004_to_wchar,
    from_wchar: mb_wchar_to_sjis2004,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP932: Encoding = Encoding {
    no: NoEncoding::Cp932,
    name: "CP932",
    mime_name: Some("Shift_JIS"),
    aliases: &["MS932", "Windows-31J", "MS_Kanji"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJISWIN),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp932_to_wchar,
    from_wchar: mb_wchar_to_cp932,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJISWIN: Encoding = Encoding {
    no: NoEncoding::Sjiswin,
    name: "SJIS-win",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-ms", "SJIS-open"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJISWIN),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp932_to_wchar,
    from_wchar: mb_wchar_to_sjiswin,
    check: None,
    cut: None,
};

/// `ucs_r_jis_table_min`/`_max` (`unicode_table_jis.h`; computed from the
/// table size there, so not generated).
pub(super) const UCS_R_JIS_TABLE_MIN: u32 = 0xFF00;
pub(super) const UCS_R_JIS_TABLE_MAX: u32 = 0xFF00 + UCS_R_JIS_TABLE.len() as u32;

/// The `ucs_a1/a2/i/r_jis_table` lookup most JIS-based encoders start with:
/// the entry for `w`, or `None` if `w` is outside all four tables.
pub(super) fn ucs_jis_lookup(w: u32) -> Option<u32> {
    if (UCS_A1_JIS_TABLE_MIN..UCS_A1_JIS_TABLE_MAX).contains(&w) {
        Some(UCS_A1_JIS_TABLE[(w - UCS_A1_JIS_TABLE_MIN) as usize] as u32)
    } else if (UCS_A2_JIS_TABLE_MIN..UCS_A2_JIS_TABLE_MAX).contains(&w) {
        Some(UCS_A2_JIS_TABLE[(w - UCS_A2_JIS_TABLE_MIN) as usize] as u32)
    } else if (UCS_I_JIS_TABLE_MIN..UCS_I_JIS_TABLE_MAX).contains(&w) {
        Some(UCS_I_JIS_TABLE[(w - UCS_I_JIS_TABLE_MIN) as usize] as u32)
    } else if (UCS_R_JIS_TABLE_MIN..UCS_R_JIS_TABLE_MAX).contains(&w) {
        Some(UCS_R_JIS_TABLE[(w - UCS_R_JIS_TABLE_MIN) as usize] as u32)
    } else {
        None
    }
}

/// The JIS X 0208 row 1 cells which the Microsoft-derived decoders (CP932,
/// the mobile SJIS variants, eucJP-win, CP51932) map to fullwidth or
/// different codepoints; 0 for every other (ku*94)+ten value.
pub(super) fn cp932_row1_special(s: u32) -> u32 {
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

/// `(((s / 94) + 0x21) << 8) | ((s % 94) + 0x21)`: (ku*94)+ten value to a
/// JIS code.
#[inline]
fn kuten_to_jis(s: u32) -> u32 {
    (((s / 94) + 0x21) << 8) | ((s % 94) + 0x21)
}

/// Emits the JIS code `s` (row and cell bytes) as two Shift-JIS bytes.
#[inline]
fn add_sjis(buf: &mut ConvertBuf, s: u32) {
    let (s1, s2) = sjis_encode((s >> 8) & 0xFF, s & 0xFF);
    buf.add2(s1 as u8, s2 as u8);
}

// `sjis_decode_tbl1[c] + sjis_decode_tbl2[c2]` is the (ku*94)+ten value of
// the Shift-JIS bytes `c`, `c2`; the tables are chosen so that any illegal
// byte gives a value of at least 0xFFFF. The C table holds negative numbers,
// stored here as the unsigned shorts they become.
static SJIS_DECODE_TBL1: [u16; 256] = [
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0xFFFF, 0x0000, 0x00BC, 0x0178,
    0x0234, 0x02F0, 0x03AC, 0x0468, 0x0524, 0x05E0, 0x069C, 0x0758, 0x0814, 0x08D0, 0x098C, 0x0A48,
    0x0B04, 0x0BC0, 0x0C7C, 0x0D38, 0x0DF4, 0x0EB0, 0x0F6C, 0x1028, 0x10E4, 0x11A0, 0x125C, 0x1318,
    0x13D4, 0x1490, 0x154C, 0x1608, 0xFFFF, 0xE880, 0xE93C, 0xE9F8, 0xEAB4, 0xEB70, 0xEC2C, 0xECE8,
    0xEDA4, 0xEE60, 0xEF1C, 0xEFD8, 0xF094, 0xF150, 0xF20C, 0xF2C8, 0xF384, 0xF440, 0xF4FC, 0xF5B8,
    0xF674, 0xF730, 0xF7EC, 0xF8A8, 0xF964, 0xFA20, 0xFADC, 0xFB98, 0xFC54, 0xFD10, 0xFDCC, 0xFE88,
    0xFF44, 0x0000, 0x00BC, 0x0178, 0x0234, 0x02F0, 0x03AC, 0x0468, 0x0524, 0x05E0, 0x069C, 0x0758,
    0x0814, 0x08D0, 0x098C, 0x0A48, 0x0B04, 0x0BC0, 0x0C7C, 0x0D38, 0x0DF4, 0x0EB0, 0x0F6C, 0x1028,
    0x10E4, 0x11A0, 0x125C, 0x1318, 0x13D4, 0x1490, 0x154C, 0x1608, 0x16C4, 0x1780, 0x183C, 0x18F8,
    0x19B4, 0x1A70, 0x1B2C, 0x1BE8, 0x1CA4, 0x1D60, 0x1E1C, 0x1ED8, 0x1F94, 0x2050, 0x210C, 0x21C8,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
];

static SJIS_DECODE_TBL2: [u16; 256] = [
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007,
    0x0008, 0x0009, 0x000A, 0x000B, 0x000C, 0x000D, 0x000E, 0x000F, 0x0010, 0x0011, 0x0012, 0x0013,
    0x0014, 0x0015, 0x0016, 0x0017, 0x0018, 0x0019, 0x001A, 0x001B, 0x001C, 0x001D, 0x001E, 0x001F,
    0x0020, 0x0021, 0x0022, 0x0023, 0x0024, 0x0025, 0x0026, 0x0027, 0x0028, 0x0029, 0x002A, 0x002B,
    0x002C, 0x002D, 0x002E, 0x002F, 0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037,
    0x0038, 0x0039, 0x003A, 0x003B, 0x003C, 0x003D, 0x003E, 0xFFFF, 0x003F, 0x0040, 0x0041, 0x0042,
    0x0043, 0x0044, 0x0045, 0x0046, 0x0047, 0x0048, 0x0049, 0x004A, 0x004B, 0x004C, 0x004D, 0x004E,
    0x004F, 0x0050, 0x0051, 0x0052, 0x0053, 0x0054, 0x0055, 0x0056, 0x0057, 0x0058, 0x0059, 0x005A,
    0x005B, 0x005C, 0x005D, 0x005E, 0x005F, 0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066,
    0x0067, 0x0068, 0x0069, 0x006A, 0x006B, 0x006C, 0x006D, 0x006E, 0x006F, 0x0070, 0x0071, 0x0072,
    0x0073, 0x0074, 0x0075, 0x0076, 0x0077, 0x0078, 0x0079, 0x007A, 0x007B, 0x007C, 0x007D, 0x007E,
    0x007F, 0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A,
    0x008B, 0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096,
    0x0097, 0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x00A1, 0x00A2,
    0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE,
    0x00AF, 0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA,
    0x00BB, 0xFFFF, 0xFFFF, 0xFFFF,
];

/// `sjis_mobile_decode_tbl1`: like [`SJIS_DECODE_TBL1`], but lead bytes
/// 0xF0-0xFC (user-defined area and vendor extensions) are also valid.
static SJIS_MOBILE_DECODE_TBL1: [u16; 256] = [
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0xFFFF, 0x0000, 0x00BC, 0x0178,
    0x0234, 0x02F0, 0x03AC, 0x0468, 0x0524, 0x05E0, 0x069C, 0x0758, 0x0814, 0x08D0, 0x098C, 0x0A48,
    0x0B04, 0x0BC0, 0x0C7C, 0x0D38, 0x0DF4, 0x0EB0, 0x0F6C, 0x1028, 0x10E4, 0x11A0, 0x125C, 0x1318,
    0x13D4, 0x1490, 0x154C, 0x1608, 0xFFFF, 0xE880, 0xE93C, 0xE9F8, 0xEAB4, 0xEB70, 0xEC2C, 0xECE8,
    0xEDA4, 0xEE60, 0xEF1C, 0xEFD8, 0xF094, 0xF150, 0xF20C, 0xF2C8, 0xF384, 0xF440, 0xF4FC, 0xF5B8,
    0xF674, 0xF730, 0xF7EC, 0xF8A8, 0xF964, 0xFA20, 0xFADC, 0xFB98, 0xFC54, 0xFD10, 0xFDCC, 0xFE88,
    0xFF44, 0x0000, 0x00BC, 0x0178, 0x0234, 0x02F0, 0x03AC, 0x0468, 0x0524, 0x05E0, 0x069C, 0x0758,
    0x0814, 0x08D0, 0x098C, 0x0A48, 0x0B04, 0x0BC0, 0x0C7C, 0x0D38, 0x0DF4, 0x0EB0, 0x0F6C, 0x1028,
    0x10E4, 0x11A0, 0x125C, 0x1318, 0x13D4, 0x1490, 0x154C, 0x1608, 0x16C4, 0x1780, 0x183C, 0x18F8,
    0x19B4, 0x1A70, 0x1B2C, 0x1BE8, 0x1CA4, 0x1D60, 0x1E1C, 0x1ED8, 0x1F94, 0x2050, 0x210C, 0x21C8,
    0x2284, 0x2340, 0x23FC, 0x24B8, 0x2574, 0x2630, 0x26EC, 0x27A8, 0x2864, 0x2920, 0x29DC, 0x2A98,
    0x2B54, 0xFFFF, 0xFFFF, 0xFFFF,
];

#[inline]
fn sjis_kuten(c: u8, c2: u8) -> u32 {
    SJIS_DECODE_TBL1[c as usize] as u32 + SJIS_DECODE_TBL2[c2 as usize] as u32
}

#[inline]
fn sjis_mobile_kuten(c: u8, c2: u8) -> u32 {
    SJIS_MOBILE_DECODE_TBL1[c as usize] as u32 + SJIS_DECODE_TBL2[c2 as usize] as u32
}

/*
 * SJIS
 */

pub fn mb_sjis_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let len = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0, 0);

    // The main loop stops 1 byte short of the end of the input, so a lead
    // byte always has a trail byte to read.
    while p + 1 < len && o < limit {
        let c = s[p];
        p += 1;

        if c <= 0x7F {
            out[o] = c as u32;
        } else if (0xA1..=0xDF).contains(&c) {
            // Kana
            out[o] = 0xFEC0 + c as u32;
        } else {
            let c2 = s[p];
            p += 1;
            // Illegal values of `c` or `c2` always give `w` beyond the table.
            let w = sjis_kuten(c, c2);
            if w < JISX0208_UCS_TABLE_SIZE {
                let w = JISX0208_UCS_TABLE[w as usize] as u32;
                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                if c == 0x80 || c == 0xA0 || c > 0xEF {
                    p -= 1;
                }
                out[o] = BAD_INPUT;
            }
        }
        o += 1;
    }

    // Finish up the last byte of input, if there is one.
    if p + 1 == len && o < limit {
        let c = s[p];
        p += 1;
        out[o] = if c <= 0x7F {
            c as u32
        } else if (0xA1..=0xDF).contains(&c) {
            0xFEC0 + c as u32
        } else {
            BAD_INPUT
        };
        o += 1;
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_sjis(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        let mut s = ucs_jis_lookup(w).unwrap_or(0);

        if s == 0 {
            s = match w {
                0xA5 => 0x216F,          // YEN SIGN -> FULLWIDTH YEN SIGN
                0xAF | 0x203E => 0x2131, // FULLWIDTH MACRON
                0xFF3C => 0x2140,        // FULLWIDTH REVERSE SOLIDUS
                0x2225 => 0x2142,        // PARALLEL TO
                0xFF0D => 0x215D,        // FULLWIDTH HYPHEN-MINUS
                0xFFE0 => 0x2171,        // FULLWIDTH CENT SIGN
                0xFFE1 => 0x2172,        // FULLWIDTH POUND SIGN
                0xFFE2 => 0x224C,        // FULLWIDTH NOT SIGN
                0 => 0,
                _ => {
                    mb_illegal_output(w, mb_wchar_to_sjis, buf);
                    continue;
                }
            };
        } else if s >= 0x8080 {
            // JIS X 0212; not supported
            mb_illegal_output(w, mb_wchar_to_sjis, buf);
            continue;
        }

        if s <= 0xFF {
            // Latin/Kana
            buf.add(s as u8);
        } else {
            // Kanji
            add_sjis(buf, s);
        }
    }
}

/*
 * SJIS-mac (MacJapanese)
 */

pub fn mb_sjismac_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    // A single SJIS-Mac kuten code can convert to up to 5 Unicode codepoints.
    debug_assert!(out.len() >= 5);

    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0, 0);

    'main: while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c <= 0x80 || c == 0xA0 {
            out[o] = match c {
                0x5C => 0xA5,
                0x80 => 0x5C,
                _ => c as u32,
            };
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else if c <= 0xED {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let w = sjis_kuten(c, c2);

            if w <= 0x89 {
                let special = match w {
                    0x1C => 0x2014, // EM DASH
                    0x1F => 0xFF3C, // FULLWIDTH REVERSE SOLIDUS
                    0x20 => 0x301C, // WAVE DASH
                    0x21 => 0x2016, // DOUBLE VERTICAL LINE
                    0x3C => 0x2212, // MINUS SIGN
                    0x50 => 0xA2,   // CENT SIGN
                    0x51 => 0xA3,   // POUND SIGN
                    0x89 => 0xAC,   // NOT SIGN
                    _ => 0,
                };
                if special != 0 {
                    out[o] = special;
                    o += 1;
                    continue;
                }
            } else {
                if (0x2F0..=0x3A3).contains(&w) {
                    for t in CODE_TBL.iter() {
                        if w >= t[0] as u32 && w <= t[1] as u32 {
                            out[o] = w - t[0] as u32 + t[2] as u32;
                            o += 1;
                            continue 'main;
                        }
                    }
                }

                if (0x340..=0x523).contains(&w) {
                    for t in CODE_TBL_M.iter() {
                        if w == t[0] as u32 {
                            let n = match t[1] {
                                0xF860 => 3,
                                0xF861 => 4,
                                _ => 5,
                            };
                            if limit - o < n {
                                p -= 2;
                                break 'main;
                            }
                            for &cp in &t[1..=n] {
                                out[o] = cp as u32;
                                o += 1;
                            }
                            continue 'main;
                        }
                    }
                }

                if (0x3AC..=0x20A5).contains(&w) {
                    for (i, t) in CODE_OFST_TBL.iter().enumerate() {
                        if w >= t[0] as u32 && w <= t[1] as u32 {
                            let w2 = CODE_MAP[i][(w - t[0] as u32) as usize] as u32;
                            if w2 == 0 {
                                out[o] = BAD_INPUT;
                                o += 1;
                                continue 'main;
                            }
                            if limit - o < 2 {
                                p -= 2;
                                break 'main;
                            }
                            out[o] = w2;
                            o += 1;
                            if (0x43E..=0x441).contains(&w) {
                                out[o] = 0xF87A;
                                o += 1;
                            } else if w == 0x3B1 || w == 0x3B7 {
                                out[o] = 0xF87F;
                                o += 1;
                            } else if w == 0x4B8 || w == 0x4B9 || w == 0x4C4 {
                                out[o] = 0x20DD;
                                o += 1;
                            } else if w == 0x1ED9
                                || w == 0x1EDA
                                || w == 0x1EE8
                                || w == 0x1EF3
                                || (0x1EF5..=0x1EFB).contains(&w)
                                || w == 0x1F05
                                || w == 0x1F06
                                || w == 0x1F18
                                || (0x1FF2..=0x20A5).contains(&w)
                            {
                                out[o] = 0xF87E;
                                o += 1;
                            }
                            continue 'main;
                        }
                    }
                }
            }

            if w < JISX0208_UCS_TABLE_SIZE {
                let w = JISX0208_UCS_TABLE[w as usize] as u32;
                out[o] = if w == 0 { BAD_INPUT } else { w };
            } else {
                out[o] = BAD_INPUT;
            }
            o += 1;
        } else if c == 0xFD {
            out[o] = 0xA9;
            o += 1;
        } else if c == 0xFE {
            out[o] = 0x2122;
            o += 1;
        } else if c == 0xFF {
            if limit - o < 2 {
                p -= 1;
                break;
            }
            out[o] = 0x2026;
            out[o + 1] = 0xF87F;
            o += 2;
        } else {
            out[o] = BAD_INPUT;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

/// `process_s_form`: the SJIS-mac code for `w` followed by the variant
/// selector-like codepoint `w2`, if that pair is a MacJapanese character.
fn process_s_form(w: u32, w2: u32, s: &mut u32) -> bool {
    let range = match w2 {
        0xF87A => 34 + 3 + 3..34 + 3 + 3 + 4,
        0x20DD => 34 + 3..34 + 3 + 3,
        0xF87F => 34..34 + 3,
        0xF87E => 0..34,
        _ => return false,
    };
    for i in range {
        if w == S_FORM_TBL[i] as u32 {
            *s = S_FORM_SJIS_TBL[i] as u32;
            return true;
        }
    }
    false
}

/// For codepoints F860-F862, which are treated specially in MacJapanese.
static TRANSCODING_HINT_CP_WIDTH: [usize; 3] = [3, 4, 5];

/// The `resume_transcoding_hint` part of `mb_wchar_to_sjismac`: an Apple
/// transcoding hint `w` and the codepoints of `CODE_TBL_M[i]` before
/// `index` have matched; try to match the rest. Returns false where the C
/// code returns from the encoder.
fn sjismac_transcoding_hint(
    w: u32,
    i: usize,
    mut index: usize,
    input: &[u32],
    ip: &mut usize,
    buf: &mut ConvertBuf,
    end: bool,
) -> bool {
    let expected = TRANSCODING_HINT_CP_WIDTH[(w - 0xF860) as usize];

    while index <= expected {
        if *ip == input.len() {
            if end {
                for &cp in &CODE_TBL_M[i][1..index] {
                    mb_illegal_output(cp as u32, mb_wchar_to_sjismac, buf);
                }
            } else {
                buf.state = ((i as u32) << 24) | ((index as u32) << 16) | (w & 0xFFFF);
            }
            return false;
        }

        let w2 = input[*ip];
        *ip += 1;

        if w2 != CODE_TBL_M[i][index] as u32 {
            // Didn't match
            for &cp in &CODE_TBL_M[i][1..index] {
                mb_illegal_output(cp as u32, mb_wchar_to_sjismac, buf);
            }
            mb_illegal_output(w2, mb_wchar_to_sjismac, buf);
            return true;
        }

        index += 1;
    }

    // Successful match, emit SJIS-mac bytes
    let s = CODE_TBL_M[i][0] as u32;
    let (s1, s2) = sjis_encode((s / 94) + 0x21, (s % 94) + 0x21);
    buf.add2(s1 as u8, s2 as u8);
    true
}

pub fn mb_wchar_to_sjismac(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let len = input.len();
    let mut ip = 0;
    let mut pending = None;

    // `buf.state` is either a codepoint which might start a multi-codepoint
    // character, or a partially matched transcoding hint sequence.
    if buf.state != 0 {
        let w = buf.state & 0xFFFF;
        if buf.state & 0xFF00_0000 != 0 {
            let i = (buf.state >> 24) as usize;
            let index = ((buf.state >> 16) & 0xFF) as usize;
            buf.state = 0;
            if !sjismac_transcoding_hint(w, i, index, input, &mut ip, buf, end) {
                return;
            }
        } else {
            buf.state = 0;
            pending = Some(w);
        }
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

        let mut s = if (UCS_A1_JIS_TABLE_MIN..UCS_A1_JIS_TABLE_MAX).contains(&w) {
            match w {
                0x5C => 0x80,
                0xA9 => 0xFD,
                _ => UCS_A1_JIS_TABLE[(w - UCS_A1_JIS_TABLE_MIN) as usize] as u32,
            }
        } else if (UCS_A2_JIS_TABLE_MIN..UCS_A2_JIS_TABLE_MAX).contains(&w) {
            match w {
                0x2122 => 0xFE,
                0x2014 => 0x213D,
                0x2116 => 0x2C1D,
                _ => UCS_A2_JIS_TABLE[(w - UCS_A2_JIS_TABLE_MIN) as usize] as u32,
            }
        } else {
            ucs_jis_lookup(w).unwrap_or(0)
        };

        if w >= 0x2000 {
            if let Some(i) = S_FORM_TBL.iter().position(|&x| x as u32 == w) {
                if ip == len {
                    if end {
                        s = S_FORM_SJIS_FALLBACK_TBL[i] as u32;
                        if s != 0 {
                            buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
                        } else {
                            mb_illegal_output(w, mb_wchar_to_sjismac, buf);
                        }
                    } else {
                        buf.state = w;
                    }
                    return;
                }

                let w2 = input[ip];
                ip += 1;

                if !process_s_form(w, w2, &mut s) {
                    ip -= 1;
                    // `i` is the first entry for `w`, as in the C fallback search.
                    s = S_FORM_SJIS_FALLBACK_TBL[i] as u32;
                }

                if s <= 0xFF {
                    buf.add(s as u8);
                } else {
                    buf.add2(((s >> 8) & 0xFF) as u8, (s & 0xFF) as u8);
                }
                continue;
            }

            if w == 0xF860 || w == 0xF861 || w == 0xF862 {
                // Apple 'transcoding hint' codepoints (from private use area)
                if ip == len {
                    if end {
                        mb_illegal_output(w, mb_wchar_to_sjismac, buf);
                    } else {
                        buf.state = w;
                    }
                    return;
                }

                let w2 = input[ip];
                ip += 1;

                if let Some(i) = CODE_TBL_M
                    .iter()
                    .position(|t| w == t[1] as u32 && w2 == t[2] as u32)
                {
                    // This might be a valid transcoding hint sequence. As in
                    // C, a state left behind by an error marker's re-entrant
                    // call is taken as a saved match.
                    let (i, index) = if buf.state != 0 {
                        let st = buf.state;
                        buf.state = 0;
                        ((st >> 24) as usize, ((st >> 16) & 0xFF) as usize)
                    } else {
                        (i, 3)
                    };
                    if !sjismac_transcoding_hint(w, i, index, input, &mut ip, buf, end) {
                        return;
                    }
                    continue;
                }

                // No valid transcoding hint sequence found
                ip -= 1;
                mb_illegal_output(w, mb_wchar_to_sjismac, buf);
                continue;
            }
        }

        if s == 0 {
            if w == 0xA0 {
                s = 0xA0;
            } else if w == 0xA5 {
                // YEN SIGN: Unicode has U+FFE5 for a fullwidth Yen sign, so
                // convert U+00A5 to the halfwidth one
                s = 0x5C;
            } else if w == 0xFF3C {
                // FULLWIDTH REVERSE SOLIDUS
                s = 0x2140;
            } else {
                s = 'found: {
                    for t in WCHAR2SJIS_MAC_R_TBL.iter() {
                        if w >= t[0] as u32 && w <= t[1] as u32 {
                            break 'found kuten_to_jis(w - t[0] as u32 + t[2] as u32);
                        }
                    }
                    for (i, t) in WCHAR2SJIS_MAC_R_MAP.iter().enumerate() {
                        if w >= t[0] as u32 && w <= t[1] as u32 {
                            let k = WCHAR2SJIS_MAC_CODE_MAP[i][(w - t[0] as u32) as usize] as u32;
                            if k != 0 {
                                break 'found kuten_to_jis(k);
                            }
                        }
                    }
                    for t in WCHAR2SJIS_MAC_WCHAR_TBL.iter() {
                        if w == t[0] as u32 {
                            break 'found kuten_to_jis(t[1] as u32);
                        }
                    }
                    0
                };
            }
        }

        if (s == 0 && w != 0) || s >= 0x8080 {
            mb_illegal_output(w, mb_wchar_to_sjismac, buf);
        } else if s <= 0xFF {
            buf.add(s as u8);
        } else {
            add_sjis(buf, s);
        }
    }
}

/*
 * SJIS-Mobile#DOCOMO, SJIS-Mobile#KDDI, SJIS-Mobile#SOFTBANK
 */

/// The common part of the SJIS-Mobile decoders for a two-byte character
/// which is not a vendor emoji; `None` for C's "bad input, maybe unread the
/// trail byte" branch.
#[inline]
fn sjis_mobile_kanji(w: u32) -> Option<u32> {
    if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&w) {
        Some(CP932EXT1_UCS_TABLE[(w - CP932EXT1_UCS_TABLE_MIN) as usize] as u32)
    } else if w < JISX0208_UCS_TABLE_SIZE {
        Some(JISX0208_UCS_TABLE[w as usize] as u32)
    } else if (CP932EXT2_UCS_TABLE_MIN..CP932EXT2_UCS_TABLE_MAX).contains(&w) {
        Some(CP932EXT2_UCS_TABLE[(w - CP932EXT2_UCS_TABLE_MIN) as usize] as u32)
    } else if (CP932EXT3_UCS_TABLE_MIN..CP932EXT3_UCS_TABLE_MAX).contains(&w) {
        Some(CP932EXT3_UCS_TABLE[(w - CP932EXT3_UCS_TABLE_MIN) as usize] as u32)
    } else if ((94 * 94)..(114 * 94)).contains(&w) {
        Some(w - (94 * 94) + 0xE000)
    } else {
        None
    }
}

pub fn mb_sjis_docomo_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    // Leave one extra space available in the output buffer, since some
    // iterations of the main loop emit two codepoints.
    let limit = out.len().saturating_sub(1);
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            // Kana
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            // Kanji
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let mut w = sjis_mobile_kuten(c, c2);

            if w <= 137 {
                let special = cp932_row1_special(w);
                if special != 0 {
                    out[o] = special;
                    o += 1;
                    continue;
                }
            }

            if (MB_TBL_CODE2UNI_DOCOMO1_MIN..=MB_TBL_CODE2UNI_DOCOMO1_MAX).contains(&w) {
                let mut snd = 0;
                w = mbfilter_sjis_emoji_docomo2unicode(w as i32, &mut snd) as u32;
                if snd != 0 {
                    out[o] = snd as u32;
                    o += 1;
                }
            } else if let Some(x) = sjis_mobile_kanji(w) {
                w = x;
            } else {
                if c == 0x80 || c == 0xA0 || c >= 0xFD {
                    p -= 1;
                }
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            out[o] = if w != 0 { w } else { BAD_INPUT };
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

/// The table lookups which start every SJIS-Mobile encoder, up to the
/// `process_emoji` label of `mb_wchar_to_sjis_docomo` and friends.
fn sjis_mobile_lookup(w: u32) -> u32 {
    let mut s = if let Some(s) = ucs_jis_lookup(w) {
        s
    } else if (0xE000..0xE000 + 20 * 94).contains(&w) {
        // Private User Area (95ku - 114ku)
        let s = w - 0xE000;
        return (((s / 94) + 0x7F) << 8) | ((s % 94) + 0x21);
    } else {
        0
    };

    if s == 0 {
        s = match w {
            0xA5 => 0x216F,   // YEN SIGN -> FULLWIDTH YEN SIGN
            0xFF3C => 0x2140, // FULLWIDTH REVERSE SOLIDUS
            0x2225 => 0x2142, // PARALLEL TO
            0xFF0D => 0x215D, // FULLWIDTH HYPHEN-MINUS
            0xFFE0 => 0x2171, // FULLWIDTH CENT SIGN
            0xFFE1 => 0x2172, // FULLWIDTH POUND SIGN
            0xFFE2 => 0x224C, // FULLWIDTH NOT SIGN
            _ => 0,
        };
    }

    if w != 0 && (s == 0 || s >= 0x8080) {
        let ext1 =
            &CP932EXT1_UCS_TABLE[..(CP932EXT1_UCS_TABLE_MAX - CP932EXT1_UCS_TABLE_MIN) as usize];
        if let Some(i) = ext1.iter().position(|&x| x as u32 == w) {
            let i = i as u32;
            return (((i / 94) + 0x2D) << 8) + (i % 94) + 0x21;
        }
        let ext2 =
            &CP932EXT2_UCS_TABLE[..(CP932EXT2_UCS_TABLE_MAX - CP932EXT2_UCS_TABLE_MIN) as usize];
        if let Some(i) = ext2.iter().position(|&x| x as u32 == w) {
            let i = i as u32;
            return (((i / 94) + 0x79) << 8) + (i % 94) + 0x21;
        }
        return 0;
    }

    s
}

/// The (ku*94)+ten value of a vendor emoji from the `mb_tbl_uni_*2code{2,3,5}`
/// tables, converted to a JIS code; `s` if `w` is not in them.
fn uni2code_emoji(
    w: u32,
    s: u32,
    code2: (u32, u32, &[u16], &[u16]),
    code3: (u32, u32, &[u16], &[u16]),
    code5: (u32, u32, &[u16], &[u16]),
) -> u32 {
    let (lookup, offset) = if (code2.0..=code2.1).contains(&w) {
        (code2, 0)
    } else if (code3.0..=code3.1).contains(&w) {
        (code3, 0x10000)
    } else if (code5.0..=code5.1).contains(&w) {
        (code5, 0xF0000)
    } else {
        return s;
    };
    let (_, _, key, value) = lookup;
    let i = mbfl_bisec_srch2((w - offset) as i32, key, key.len());
    if i >= 0 {
        kuten_to_jis(value[i as usize] as u32)
    } else {
        s
    }
}

pub fn mb_wchar_to_sjis_docomo(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let len = input.len();
    let mut ip = 0;
    let mut pending = None;

    if buf.state != 0 {
        // Continue what we were doing on the previous call
        pending = Some(buf.state);
        buf.state = 0;
    }

    'outer: loop {
        let w = if let Some(w) = pending.take() {
            w
        } else if ip < len {
            ip += 1;
            input[ip - 1]
        } else {
            break;
        };

        let mut s = sjis_mobile_lookup(w);

        // process_emoji: Keypad symbol emoji are converted to a digit or '#'
        // followed by U+20E3 (COMBINING ENCLOSING KEYCAP); look for those.
        'emit: {
            if w == b'#' as u32 || (b'0' as u32..=b'9' as u32).contains(&w) {
                if ip == len {
                    if end {
                        break 'emit;
                    }
                    // Another buffer of codepoints is coming; reprocess `w`
                    // together with it
                    buf.state = w;
                    break 'outer;
                }
                let w2 = input[ip];
                ip += 1;
                if w2 == 0x20E3 {
                    s = if w == b'#' as u32 {
                        0x2964
                    } else if w == b'0' as u32 {
                        0x296F
                    } else {
                        // Previous character was '1'-'9'
                        0x2966 + (w - b'1' as u32)
                    };
                    s = kuten_to_jis(s);
                } else {
                    ip -= 1;
                }
            } else if w == 0xA9 {
                // Copyright sign
                s = kuten_to_jis(0x29B5);
            } else if w == 0xAE {
                // Registered sign
                s = kuten_to_jis(0x29BA);
            } else {
                s = uni2code_emoji(
                    w,
                    s,
                    (
                        MB_TBL_UNI_DOCOMO2CODE2_MIN,
                        MB_TBL_UNI_DOCOMO2CODE2_MAX,
                        &MB_TBL_UNI_DOCOMO2CODE2_KEY[..MB_TBL_UNI_DOCOMO2CODE2_LEN as usize],
                        &MB_TBL_UNI_DOCOMO2CODE2_VALUE,
                    ),
                    (
                        MB_TBL_UNI_DOCOMO2CODE3_MIN,
                        MB_TBL_UNI_DOCOMO2CODE3_MAX,
                        &MB_TBL_UNI_DOCOMO2CODE3_KEY[..MB_TBL_UNI_DOCOMO2CODE3_LEN as usize],
                        &MB_TBL_UNI_DOCOMO2CODE3_VALUE,
                    ),
                    (
                        MB_TBL_UNI_DOCOMO2CODE5_MIN,
                        MB_TBL_UNI_DOCOMO2CODE5_MAX,
                        &MB_TBL_UNI_DOCOMO2CODE5_KEY[..MB_TBL_UNI_DOCOMO2CODE5_LEN as usize],
                        &MB_TBL_UNI_DOCOMO2CODE5_VAL,
                    ),
                );
            }
        }

        // emit_output:
        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_sjis_docomo, buf);
        } else if s <= 0xFF {
            buf.add(s as u8);
        } else {
            add_sjis(buf, s);
        }
    }
}

pub fn mb_sjis_kddi_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len().saturating_sub(1);
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            // Kana
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            // Kanji
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let mut w = sjis_mobile_kuten(c, c2);

            if w <= 137 {
                let special = cp932_row1_special(w);
                if special != 0 {
                    out[o] = special;
                    o += 1;
                    continue;
                }
            }

            if (MB_TBL_CODE2UNI_KDDI1_MIN..=MB_TBL_CODE2UNI_KDDI2_MAX).contains(&w) {
                let mut snd = 0;
                w = mbfilter_sjis_emoji_kddi2unicode(w as i32, &mut snd) as u32;
                if w == 0 {
                    w = sjis_mobile_kuten(c, c2);
                    if ((94 * 94)..(114 * 94)).contains(&w) {
                        w = w - (94 * 94) + 0xE000;
                    }
                } else if snd != 0 {
                    out[o] = snd as u32;
                    o += 1;
                }
            } else if let Some(x) = sjis_mobile_kanji(w) {
                w = x;
            } else {
                if c == 0x80 || c == 0xA0 || c >= 0xFD {
                    p -= 1;
                }
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            out[o] = if w != 0 { w } else { BAD_INPUT };
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_sjis_kddi(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let len = input.len();
    let mut ip = 0;
    let mut pending = None;

    if buf.state != 0 {
        pending = Some(buf.state);
        buf.state = 0;
    }

    'outer: loop {
        let w = if let Some(w) = pending.take() {
            w
        } else if ip < len {
            ip += 1;
            input[ip - 1]
        } else {
            break;
        };

        let mut s = sjis_mobile_lookup(w);

        // process_emoji:
        'emit: {
            if w == b'#' as u32 || (b'0' as u32..=b'9' as u32).contains(&w) {
                if ip == len {
                    if end {
                        break 'emit;
                    }
                    // Another buffer of codepoints is coming; reprocess `w`
                    // together with it
                    buf.state = w;
                    break 'outer;
                }
                let w2 = input[ip];
                ip += 1;
                if w2 == 0x20E3 {
                    s = if w == b'#' as u32 {
                        0x25BC
                    } else if w == b'0' as u32 {
                        0x2830
                    } else {
                        // Previous character was '1'-'9'
                        0x27A6 + (w - b'1' as u32)
                    };
                    s = kuten_to_jis(s);
                } else {
                    ip -= 1;
                }
            } else if (nflags(b'C') as u32..=nflags(b'U') as u32).contains(&w) {
                // C for CN, U for US
                if ip == len {
                    if end {
                        mb_illegal_output(w, mb_wchar_to_sjis_kddi, buf);
                    } else {
                        // Reprocess `w` when this function is called again
                        // with another buffer of codepoints
                        buf.state = w;
                    }
                    break 'outer;
                }
                let w2 = input[ip];
                ip += 1;
                if (nflags(b'B') as u32..=nflags(b'U') as u32).contains(&w2) {
                    // B for GB, U for RU
                    for (flag, &code) in NFLAGS_S.iter().zip(NFLAGS_CODE_KDDI.iter()) {
                        if w == nflags(flag[0]) as u32 && w2 == nflags(flag[1]) as u32 {
                            s = kuten_to_jis(code as u32);
                            break 'emit;
                        }
                    }
                }
                ip -= 1;
                mb_illegal_output(w, mb_wchar_to_sjis_kddi, buf);
                continue 'outer;
            } else if w == 0xA9 {
                // Copyright sign
                s = kuten_to_jis(0x27DC);
            } else if w == 0xAE {
                // Registered sign
                s = kuten_to_jis(0x27DD);
            } else {
                s = uni2code_emoji(
                    w,
                    s,
                    (
                        MB_TBL_UNI_KDDI2CODE2_MIN,
                        MB_TBL_UNI_KDDI2CODE2_MAX,
                        &MB_TBL_UNI_KDDI2CODE2_KEY[..MB_TBL_UNI_KDDI2CODE2_LEN as usize],
                        &MB_TBL_UNI_KDDI2CODE2_VALUE,
                    ),
                    (
                        MB_TBL_UNI_KDDI2CODE3_MIN,
                        MB_TBL_UNI_KDDI2CODE3_MAX,
                        &MB_TBL_UNI_KDDI2CODE3_KEY[..MB_TBL_UNI_KDDI2CODE3_LEN as usize],
                        &MB_TBL_UNI_KDDI2CODE3_VALUE,
                    ),
                    (
                        MB_TBL_UNI_KDDI2CODE5_MIN,
                        MB_TBL_UNI_KDDI2CODE5_MAX,
                        &MB_TBL_UNI_KDDI2CODE5_KEY[..MB_TBL_UNI_KDDI2CODE5_LEN as usize],
                        &MB_TBL_UNI_KDDI2CODE5_VAL,
                    ),
                );
            }
        }

        // emit_output:
        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_sjis_kddi, buf);
        } else if s <= 0xFF {
            buf.add(s as u8);
        } else {
            add_sjis(buf, s);
        }
    }
}

/// The `softbank_emoji_escapes` loop of `mb_sjis_sb_to_wchar`: emoji
/// encoded as single bytes after ESC $ E/F/G/O/P/Q (the letter is in
/// `state`), up to a terminating SI (0x0F).
fn sjis_sb_escapes(
    s: &[u8],
    p: &mut usize,
    out: &mut [u32],
    o: &mut usize,
    limit: usize,
    state: &mut u32,
) {
    while *p < s.len() && *o < limit {
        let c = s[*p];
        *p += 1;
        if c == 0xF {
            *state = 0;
            break;
        }
        let ku = match (*state as u8, c) {
            (b'G', 0x21..=0x7A) => 0x91,
            (b'E', 0x21..=0x7A) => 0x8D,
            (b'F', 0x21..=0x7A) => 0x8E,
            (b'O', 0x21..=0x6D) => 0x92,
            (b'P', 0x21..=0x6C) => 0x95,
            (b'Q', 0x21..=0x5E) => 0x96,
            _ => {
                out[*o] = BAD_INPUT;
                *o += 1;
                *state = 0;
                break;
            }
        };
        let base: u32 = (ku - 0x21) * 94;

        let mut snd = 0;
        let w = mbfilter_sjis_emoji_sb2unicode((base + c as u32 - 0x21) as i32, &mut snd) as u32;
        if w != 0 {
            if snd != 0 {
                out[*o] = snd as u32;
                *o += 1;
            }
            out[*o] = w;
            *o += 1;
        } else {
            out[*o] = BAD_INPUT;
            *o += 1;
            *state = 0;
            break;
        }
    }
}

pub fn mb_sjis_sb_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len().saturating_sub(1);
    let (mut p, mut o) = (0, 0);

    if *state != 0 {
        sjis_sb_escapes(s, &mut p, out, &mut o, limit, state);
    }

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == 0x1B {
            // Escape sequence
            if p == e
                || {
                    let b = s[p];
                    p += 1;
                    b != b'$'
                }
                || p == e
            {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }
            let c2 = s[p];
            p += 1;
            if !(b'E'..=b'G').contains(&c2) && !(b'O'..=b'Q').contains(&c2) {
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }
            // Escape sequence was valid; next should be a series of
            // specially encoded SoftBank emoji
            *state = c2 as u32;
            sjis_sb_escapes(s, &mut p, out, &mut o, limit, state);
        } else if c <= 0x7F {
            out[o] = c as u32;
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            // Kana
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            // Kanji
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let mut w = sjis_mobile_kuten(c, c2);

            if w <= 137 {
                let special = cp932_row1_special(w);
                if special != 0 {
                    out[o] = special;
                    o += 1;
                    continue;
                }
            }

            if (MB_TBL_CODE2UNI_SB1_MIN..=MB_TBL_CODE2UNI_SB3_MAX).contains(&w) {
                let mut snd = 0;
                w = mbfilter_sjis_emoji_sb2unicode(w as i32, &mut snd) as u32;
                if w == 0 {
                    w = sjis_mobile_kuten(c, c2);
                    if (CP932EXT3_UCS_TABLE_MIN..CP932EXT3_UCS_TABLE_MAX).contains(&w) {
                        w = CP932EXT3_UCS_TABLE[(w - CP932EXT3_UCS_TABLE_MIN) as usize] as u32;
                    } else if ((94 * 94)..(114 * 94)).contains(&w) {
                        w = w - (94 * 94) + 0xE000;
                    }
                } else if snd != 0 {
                    out[o] = snd as u32;
                    o += 1;
                }
            } else if let Some(x) = sjis_mobile_kanji(w) {
                w = x;
            } else {
                if c == 0x80 || c == 0xA0 || c >= 0xFD {
                    p -= 1;
                }
                out[o] = BAD_INPUT;
                o += 1;
                continue;
            }

            out[o] = if w != 0 { w } else { BAD_INPUT };
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_sjis_sb(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let len = input.len();
    let mut ip = 0;
    let mut pending = None;

    if buf.state != 0 {
        pending = Some(buf.state);
        buf.state = 0;
    }

    'outer: loop {
        let w = if let Some(w) = pending.take() {
            w
        } else if ip < len {
            ip += 1;
            input[ip - 1]
        } else {
            break;
        };

        let mut s = sjis_mobile_lookup(w);

        // process_emoji:
        'emit: {
            if w == b'#' as u32 || (b'0' as u32..=b'9' as u32).contains(&w) {
                if ip == len {
                    if end {
                        break 'emit;
                    }
                    // Another buffer of codepoints is coming; reprocess `w`
                    // together with it
                    buf.state = w;
                    break 'outer;
                }
                let w2 = input[ip];
                ip += 1;
                if w2 == 0x20E3 {
                    s = if w == b'#' as u32 {
                        0x2817
                    } else if w == b'0' as u32 {
                        0x282C
                    } else {
                        // Previous character was '1'-'9'
                        0x2823 + (w - b'1' as u32)
                    };
                    s = kuten_to_jis(s);
                } else {
                    ip -= 1;
                }
            } else if (nflags(b'C') as u32..=nflags(b'U') as u32).contains(&w) {
                // C for CN, U for US
                if ip == len {
                    if end {
                        mb_illegal_output(w, mb_wchar_to_sjis_sb, buf);
                    } else {
                        // Reprocess `w` when this function is called again
                        // with another buffer of codepoints
                        buf.state = w;
                    }
                    break 'outer;
                }
                let w2 = input[ip];
                ip += 1;
                if (nflags(b'B') as u32..=nflags(b'U') as u32).contains(&w2) {
                    // B for GB, U for RU
                    for (flag, &code) in NFLAGS_S.iter().zip(NFLAGS_CODE_SB.iter()) {
                        if w == nflags(flag[0]) as u32 && w2 == nflags(flag[1]) as u32 {
                            s = kuten_to_jis(code as u32);
                            break 'emit;
                        }
                    }
                }
                ip -= 1;
                mb_illegal_output(w, mb_wchar_to_sjis_sb, buf);
                continue 'outer;
            } else if w == 0xA9 {
                // Copyright sign
                s = kuten_to_jis(0x2855);
            } else if w == 0xAE {
                // Registered sign
                s = kuten_to_jis(0x2856);
            } else {
                s = uni2code_emoji(
                    w,
                    s,
                    (
                        MB_TBL_UNI_SB2CODE2_MIN,
                        MB_TBL_UNI_SB2CODE2_MAX,
                        &MB_TBL_UNI_SB2CODE2_KEY[..MB_TBL_UNI_SB2CODE2_LEN as usize],
                        &MB_TBL_UNI_SB2CODE2_VALUE,
                    ),
                    (
                        MB_TBL_UNI_SB2CODE3_MIN,
                        MB_TBL_UNI_SB2CODE3_MAX,
                        &MB_TBL_UNI_SB2CODE3_KEY[..MB_TBL_UNI_SB2CODE3_LEN as usize],
                        &MB_TBL_UNI_SB2CODE3_VALUE,
                    ),
                    (
                        MB_TBL_UNI_SB2CODE5_MIN,
                        MB_TBL_UNI_SB2CODE5_MAX,
                        &MB_TBL_UNI_SB2CODE5_KEY[..MB_TBL_UNI_SB2CODE5_LEN as usize],
                        &MB_TBL_UNI_SB2CODE5_VAL,
                    ),
                );
            }
        }

        // emit_output:
        if s == 0 && w != 0 {
            mb_illegal_output(w, mb_wchar_to_sjis_sb, buf);
        } else if s <= 0xFF {
            buf.add(s as u8);
        } else {
            add_sjis(buf, s);
        }
    }
}

/*
 * SJIS-2004
 */

pub fn mb_sjis2004_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len().saturating_sub(1);
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c <= 0x7F {
            out[o] = match c {
                0x5C => 0xA5,
                0x7E => 0x203E,
                _ => c as u32,
            };
            o += 1;
        } else if (0xA1..=0xDF).contains(&c) {
            out[o] = 0xFEC0 + c as u32;
            o += 1;
        } else {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let w1 = sjis_mobile_kuten(c, c2);

            // Conversion for combining characters
            if (0x0170..=0x03F1).contains(&w1) {
                let k =
                    mbfl_bisec_srch2(w1 as i32, &JISX0213_U2_KEY_B, JISX0213_U2_TBL_LEN as usize);
                if k >= 0 {
                    let k = k as usize;
                    out[o] = JISX0213_U2_TBL[2 * k] as u32;
                    out[o + 1] = JISX0213_U2_TBL[2 * k + 1] as u32;
                    o += 2;
                    continue;
                }
            }

            // Conversion for BMP
            if w1 < JISX0213_UCS_TABLE_SIZE {
                let w = JISX0213_UCS_TABLE[w1 as usize] as u32;
                if w != 0 {
                    out[o] = w;
                    o += 1;
                    continue;
                }
            }

            // Conversion for CJK Unified Ideographs extension B (U+2XXXX)
            let k = mbfl_bisec_srch2(
                w1 as i32,
                &JISX0213_JIS_U5_KEY,
                JISX0213_U5_TBL_LEN as usize,
            );
            if k >= 0 {
                out[o] = JISX0213_JIS_U5_TBL[k as usize] as u32 + 0x20000;
            } else {
                if c == 0x80 || c == 0xA0 || c >= 0xFD {
                    p -= 1;
                }
                out[o] = BAD_INPUT;
            }
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

/// The Unicode to JIS X 0213 lookup shared by `mb_wchar_to_sjis2004` and
/// `mb_wchar_to_eucjp2004`: the (row << 8 | cell) code of `w` (plane 2 rows
/// are stored as 0x7F + their index in `JISX0213_P2_OFST`), or 0.
///
/// `w` may combine with the following codepoint, which is then consumed
/// from `input`. Returns `None` if `w` might combine but the input ended
/// and more is to come; the caller saves `w` in its state and returns.
pub(super) fn ucs_to_jisx0213(w: u32, input: &[u32], ip: &mut usize, end: bool) -> Option<u32> {
    let mut s: u32 = 0;

    // Check for 1st char of combining characters
    if w == 0xE6
        || (0x254..=0x2E9).contains(&w)
        || (0x304B..=0x3053).contains(&w)
        || (0x30AB..=0x30C8).contains(&w)
        || w == 0x31F7
    {
        for k0 in 0..JISX0213_U2_TBL_LEN as usize {
            if w == JISX0213_U2_TBL[2 * k0] as u32 {
                let mut k = k0;
                s = 'found: {
                    if *ip == input.len() {
                        if !end {
                            return None;
                        }
                    } else {
                        let w2 = input[*ip];
                        *ip += 1;
                        if (w == 0x254 || w == 0x28C || w == 0x259 || w == 0x25A) && w2 == 0x301 {
                            k += 1;
                        }
                        if w2 == JISX0213_U2_TBL[2 * k + 1] as u32 {
                            break 'found JISX0213_U2_KEY[k] as u32;
                        }
                        *ip -= 1;
                    }
                    // Fallback
                    JISX0213_U2_FB_TBL[k] as u32
                };
                break;
            }
        }
    }

    // Check for major Japanese chars: U+4E00-U+9FFF
    if s == 0 {
        for (tbl, range) in UNI2JIS_TBL.iter().zip(UNI2JIS_TBL_RANGE.iter()) {
            if w >= range[0] as u32 && w <= range[1] as u32 {
                s = tbl[(w - range[0] as u32) as usize] as u32;
                break;
            }
        }
    }

    // Check for Japanese chars in compressed mapping area: U+1E00-U+4DBF
    if s == 0 && (UCS_C1_JISX0213_MIN..=UCS_C1_JISX0213_MAX).contains(&w) {
        let k = mbfl_bisec_srch(
            w as i32,
            &UCS_C1_JISX0213_TBL,
            UCS_C1_JISX0213_TBL_LEN as usize,
        );
        if k >= 0 {
            let k = k as usize;
            s = UCS_C1_JISX0213_OFST[k] as u32 + w - UCS_C1_JISX0213_TBL[2 * k] as u32;
        }
    }

    // Check for Japanese chars in CJK Unified Ideographs ext.B (U+2XXXX)
    if s == 0 && (JISX0213_U5_TBL_MIN..=JISX0213_U5_TBL_MAX).contains(&w) {
        let k = mbfl_bisec_srch2(
            (w - 0x20000) as i32,
            &JISX0213_U5_JIS_KEY,
            JISX0213_U5_TBL_LEN as usize,
        );
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
            let k = mbfl_bisec_srch2(
                w as i32,
                &UCS_R2B_JISX0213_CMAP_KEY,
                UCS_R2B_JISX0213_CMAP_LEN as usize,
            );
            if k >= 0 {
                s = UCS_R2B_JISX0213_CMAP_VAL[k as usize] as u32;
            }
        }
    }

    Some(s)
}

pub fn mb_wchar_to_sjis2004(input: &[u32], buf: &mut ConvertBuf, end: bool) {
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
            mb_illegal_output(w, mb_wchar_to_sjis2004, buf);
        } else if s <= 0xFF {
            buf.add(s as u8);
        } else {
            add_sjis(buf, s);
        }
    }
}

/*
 * CP932 and SJIS-win
 */

pub fn mb_cp932_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let limit = out.len();
    let (mut p, mut o) = (0, 0);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c < 0x80 {
            out[o] = c as u32;
        } else if c > 0xA0 && c < 0xE0 {
            // Kana
            out[o] = 0xFEC0 + c as u32;
        } else {
            if p == e {
                out[o] = BAD_INPUT;
                o += 1;
                break;
            }
            let c2 = s[p];
            p += 1;
            let s = sjis_mobile_kuten(c, c2);
            let mut w = 0;

            if s <= 137 {
                w = cp932_row1_special(s);
            }

            if w == 0 {
                if (CP932EXT1_UCS_TABLE_MIN..CP932EXT1_UCS_TABLE_MAX).contains(&s) {
                    w = CP932EXT1_UCS_TABLE[(s - CP932EXT1_UCS_TABLE_MIN) as usize] as u32;
                } else if s < JISX0208_UCS_TABLE_SIZE {
                    w = JISX0208_UCS_TABLE[s as usize] as u32;
                } else if (CP932EXT2_UCS_TABLE_MIN..CP932EXT2_UCS_TABLE_MAX).contains(&s) {
                    w = CP932EXT2_UCS_TABLE[(s - CP932EXT2_UCS_TABLE_MIN) as usize] as u32;
                } else if (CP932EXT3_UCS_TABLE_MIN..CP932EXT3_UCS_TABLE_MAX).contains(&s) {
                    w = CP932EXT3_UCS_TABLE[(s - CP932EXT3_UCS_TABLE_MIN) as usize] as u32;
                } else if ((94 * 94)..(114 * 94)).contains(&s) {
                    w = s - (94 * 94) + 0xE000;
                }
            }

            if w == 0 {
                if c == 0x80 || c == 0xA0 || c >= 0xFD {
                    p -= 1;
                }
                w = BAD_INPUT;
            }
            out[o] = w;
        }
        o += 1;
    }

    *input = &s[p..];
    o
}

/// The body of `mb_wchar_to_cp932` and `mb_wchar_to_sjiswin`, which differ
/// only in how U+00A5 (YEN SIGN) and U+203E (OVERLINE) are mapped. Both
/// report errors through `mb_wchar_to_cp932`.
fn wchar_to_cp932_common(input: &[u32], buf: &mut ConvertBuf, sjiswin: bool) {
    for &w in input {
        let mut s1: u32 = 0;
        let mut s2: u32 = 0;

        if (UCS_A1_JIS_TABLE_MIN..UCS_A1_JIS_TABLE_MAX).contains(&w) {
            s1 = UCS_A1_JIS_TABLE[(w - UCS_A1_JIS_TABLE_MIN) as usize] as u32;
        } else if !sjiswin && w == 0x203E {
            s1 = 0x7E;
        } else if let Some(s) = ucs_jis_lookup(w) {
            s1 = s;
        } else if (0xE000..0xE000 + 20 * 94).contains(&w) {
            let s = w - 0xE000;
            let c1 = s / 94 + 0x7F;
            let c2 = s % 94 + 0x21;
            s1 = (c1 << 8) | c2;
            s2 = 1;
        }

        match w {
            // YEN SIGN
            0xA5 => s1 = if sjiswin { 0x216F } else { 0x5C },
            0xFF3C => s1 = 0x2140, // FULLWIDTH REVERSE SOLIDUS
            0x2225 => s1 = 0x2142, // PARALLEL TO
            0xFF0D => s1 = 0x215D, // FULLWIDTH HYPHEN-MINUS
            0xFFE0 => s1 = 0x2171, // FULLWIDTH CENT SIGN
            0xFFE1 => s1 = 0x2172, // FULLWIDTH POUND SIGN
            0xFFE2 => s1 = 0x224C, // FULLWIDTH NOT SIGN
            0 => {
                buf.add(0);
                continue;
            }
            _ => {}
        }

        if s1 == 0 || (s1 >= 0x8080 && s2 == 0) {
            // not found or X 0212
            if let Some(v) =
                mbfl_binary_search_paired_sorted_table(w, &CP932EXT1_UCS_TABLE_PAIRED_SORTED)
            {
                let v = v as u32;
                s1 = ((v / 94 + 0x2D) << 8) + (v % 94 + 0x21);
            } else if let Some(v) =
                mbfl_binary_search_paired_sorted_table(w, &CP932EXT3_UCS_TABLE_PAIRED_SORTED)
            {
                let v = v as u32;
                s1 = ((v / 94 + 0x93) << 8) + (v % 94 + 0x21);
            } else {
                mb_illegal_output(w, mb_wchar_to_cp932, buf);
                continue;
            }
        }

        // emit_output:
        if s1 < 0x100 {
            buf.add(s1 as u8);
        } else {
            add_sjis(buf, s1);
        }
    }
}

pub fn mb_wchar_to_cp932(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    wchar_to_cp932_common(input, buf, false);
}

pub fn mb_wchar_to_sjiswin(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    wchar_to_cp932_common(input, buf, true);
}

#[cfg(test)]
pub(super) mod testutil {
    //! Helpers for the exhaustive conversion tests ported from the
    //! `*_encoding.phpt` files and their `encoding_tests.inc`. They work on
    //! codepoints directly instead of going through UTF-16BE/UTF-32BE.

    use crate::mbstring::mbfl::convert::{ConvertBuf, WCHAR_BUFSIZE};
    use crate::mbstring::mbfl::encoding::{BAD_INPUT, Encoding, ILLEGAL_MODE_CHAR};
    use std::collections::{BTreeMap, BTreeSet};
    use std::ops::RangeInclusive;

    /// Encoded character -> codepoints.
    pub type ToUni = BTreeMap<Vec<u8>, Vec<u32>>;
    /// Codepoints -> encoded character.
    pub type FromUni = BTreeMap<Vec<u32>, Vec<u8>>;

    const DATA_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/phpt/ext/mbstring/tests/data/"
    );

    pub fn data_lines(name: &str) -> Vec<String> {
        let bytes =
            std::fs::read(format!("{DATA_DIR}{name}")).unwrap_or_else(|e| panic!("{name}: {e}"));
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Hex digits at the start of `s` (`%x` in `sscanf`).
    pub fn hex(s: &str) -> Option<(u32, &str)> {
        let n = s.bytes().take_while(u8::is_ascii_hexdigit).count();
        if n == 0 {
            return None;
        }
        Some((u32::from_str_radix(&s[..n], 16).ok()?, &s[n..]))
    }

    /// PHP's `hexdec`: hex digits, ignoring anything else.
    pub fn hexdec(s: &str) -> u32 {
        s.chars()
            .filter_map(|c| c.to_digit(16))
            .fold(0, |acc, d| (acc << 4) | d)
    }

    /// The string `readConversionTable` makes of a character code: as many
    /// bytes as needed, big-endian.
    pub fn code_bytes(c: u32) -> Vec<u8> {
        if c <= 0xFF {
            vec![c as u8]
        } else if c <= 0xFFFF {
            vec![(c >> 8) as u8, c as u8]
        } else if c <= 0xFF_FFFF {
            vec![(c >> 16) as u8, (c >> 8) as u8, c as u8]
        } else {
            c.to_be_bytes().to_vec()
        }
    }

    /// `pack('n', v)`.
    pub fn be16(v: u32) -> Vec<u8> {
        vec![(v >> 8) as u8, v as u8]
    }

    /// `readConversionTable`: lines of `0x<code>\t0x<codepoint>`.
    pub fn read_conversion_table(name: &str) -> (ToUni, FromUni) {
        let mut to = ToUni::new();
        let mut from = FromUni::new();
        for line in data_lines(name) {
            if line.starts_with('#') {
                continue;
            }
            let Some((c, rest)) = line.strip_prefix("0x").and_then(hex) else {
                continue;
            };
            let Some((cp, _)) = rest.trim_start().strip_prefix("0x").and_then(hex) else {
                continue;
            };
            // Skip codepoints that do not have a mapping
            if cp == 0xFFFD {
                continue;
            }
            let bytes = code_bytes(c);
            to.insert(bytes.clone(), vec![cp]);
            from.insert(vec![cp], bytes);
        }
        (to, from)
    }

    /// Tables in the `0x<code>\tU+<cp>[+<cp>]` format of the JIS X 0213
    /// data files (`SJIS-2004.txt`, `EUC-JP-2004.txt`). With `fullwidth`,
    /// characters annotated with a fullwidth form map to that form.
    pub fn read_jisx0213_table(name: &str, fullwidth: bool) -> (ToUni, FromUni) {
        let mut to = ToUni::new();
        let mut from = FromUni::new();
        for line in data_lines(name) {
            if line.starts_with('#') {
                continue;
            }
            let Some((code, rest)) = line.strip_prefix("0x").and_then(hex) else {
                continue;
            };
            let Some((mut cp1, rest)) = rest.trim_start().strip_prefix("U+").and_then(hex) else {
                continue;
            };
            let cp2 = rest.strip_prefix('+').and_then(hex).map_or(0, |(c, _)| c);
            let bytes = code_bytes(code);
            if cp2 != 0 {
                to.insert(bytes, vec![cp1, cp2]);
                continue;
            }
            if fullwidth && let Some(pos) = line.find("Fullwidth: U+") {
                let digits: String = line[pos + 13..]
                    .chars()
                    .take_while(|c| matches!(c, '0'..='9' | 'A'..='F'))
                    .collect();
                if !digits.is_empty() {
                    cp1 = hexdec(&digits);
                }
            }
            to.insert(bytes.clone(), vec![cp1]);
            if cp1 <= 0xFFFF {
                from.insert(vec![cp1], bytes);
            }
        }
        (to, from)
    }

    /// `findInvalidChars`: all byte sequences which are neither valid
    /// characters nor prefixes of one, and those prefixes (truncated
    /// characters). `start_bytes` gives the fixed length of characters
    /// starting with those bytes.
    pub fn find_invalid_chars(
        valid: &ToUni,
        start_bytes: &[(RangeInclusive<u8>, usize)],
    ) -> (BTreeSet<Vec<u8>>, BTreeSet<Vec<u8>>) {
        struct Ctx<'a> {
            valid: &'a ToUni,
            prefixes: BTreeSet<Vec<u8>>,
            invalid: BTreeSet<Vec<u8>>,
            truncated: BTreeSet<Vec<u8>>,
        }

        fn var_length(cx: &mut Ctx, prefix: &[u8]) {
            for b in 0..=255u8 {
                let mut s = prefix.to_vec();
                s.push(b);
                if !cx.valid.contains_key(&s) {
                    if cx.prefixes.contains(&s) {
                        cx.truncated.insert(s.clone());
                        var_length(cx, &s);
                    } else {
                        cx.invalid.insert(s);
                    }
                }
            }
        }

        fn fixed_length(cx: &mut Ctx, prefix: &[u8], remaining: usize) {
            if remaining == 0 {
                if !cx.valid.contains_key(prefix) {
                    cx.invalid.insert(prefix.to_vec());
                }
                return;
            }
            cx.truncated.insert(prefix.to_vec());
            for b in 0..=255u8 {
                let mut s = prefix.to_vec();
                s.push(b);
                if remaining == 1 {
                    if !cx.valid.contains_key(&s) {
                        cx.invalid.insert(s);
                    }
                } else {
                    fixed_length(cx, &s, remaining - 1);
                }
            }
        }

        let mut lens = [0usize; 256];
        for (range, n) in start_bytes {
            for b in range.clone() {
                lens[b as usize] = *n;
            }
        }

        let mut prefixes = BTreeSet::new();
        for k in valid.keys() {
            for l in 1..k.len() {
                prefixes.insert(k[..l].to_vec());
            }
        }
        let mut cx = Ctx {
            valid,
            prefixes,
            invalid: BTreeSet::new(),
            truncated: BTreeSet::new(),
        };

        for b in 0..=255u8 {
            if lens[b as usize] != 0 {
                fixed_length(&mut cx, &[b], lens[b as usize] - 1);
            } else {
                let s = vec![b];
                if !cx.valid.contains_key(&s) {
                    if cx.prefixes.contains(&s) {
                        cx.truncated.insert(s.clone());
                        var_length(&mut cx, &s);
                    } else {
                        cx.invalid.insert(s);
                    }
                }
            }
        }
        (cx.invalid, cx.truncated)
    }

    /// Deterministic shuffling (xorshift64), standing in for `srand`/`shuffle`.
    pub struct Rng(u64);

    impl Rng {
        pub fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }

        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        pub fn shuffle<T>(&mut self, v: &mut [T]) {
            for i in (1..v.len()).rev() {
                let j = (self.next() % (i as u64 + 1)) as usize;
                v.swap(i, j);
            }
        }
    }

    /// Decodes all of `bytes` the way `mb_fast_convert` does, with an output
    /// buffer of `bufsize` codepoints.
    pub fn decode_with(enc: &'static Encoding, bytes: &[u8], bufsize: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut buf = vec![0u32; bufsize];
        let mut state = 0;
        let mut inp = bytes;
        while !inp.is_empty() {
            let before = inp.len();
            let n = (enc.to_wchar)(&mut inp, &mut buf, &mut state);
            assert!(
                n > 0 || inp.len() < before,
                "{}: no progress on {bytes:02X?}",
                enc.name
            );
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    pub fn decode(enc: &'static Encoding, bytes: &[u8]) -> Vec<u32> {
        decode_with(enc, bytes, WCHAR_BUFSIZE)
    }

    /// Encodes `cps` in chunks of `chunk` codepoints (`end` set on the last
    /// one), like `mb_fast_convert` does with the output of a decoder.
    pub fn encode_with(
        enc: &'static Encoding,
        cps: &[u32],
        chunk: usize,
        replacement: u32,
        mode: u32,
    ) -> ConvertBuf {
        let mut buf = ConvertBuf::new(cps.len(), replacement, mode);
        let n = cps.len().div_ceil(chunk);
        for (i, c) in cps.chunks(chunk).enumerate() {
            (enc.from_wchar)(c, &mut buf, i + 1 == n);
        }
        buf
    }

    /// Encodes with '%' as the replacement character.
    pub fn encode(enc: &'static Encoding, cps: &[u32]) -> ConvertBuf {
        encode_with(enc, cps, WCHAR_BUFSIZE, b'%' as u32, ILLEGAL_MODE_CHAR)
    }

    /// Collects failed checks, so that a test reports many at once.
    #[derive(Default)]
    pub struct Failures(Vec<String>);

    impl Failures {
        pub fn check(&mut self, ok: bool, msg: impl FnOnce() -> String) {
            if !ok {
                let mut msg = msg();
                if msg.len() > 400 {
                    let cut = (0..=400)
                        .rev()
                        .find(|&i| msg.is_char_boundary(i))
                        .unwrap_or(0);
                    msg.truncate(cut);
                    msg.push_str("...");
                }
                self.0.push(msg);
            }
        }

        pub fn assert_none(self, what: &str) {
            assert!(
                self.0.is_empty(),
                "{what}: {} failures; first ones:\n{}",
                self.0.len(),
                self.0
                    .iter()
                    .take(25)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }

    /// `testValidString`/`convertValidString`: `bytes` decodes to `cps`
    /// and, with `both_ways`, `cps` encodes back to `bytes` without errors.
    pub fn check_valid(
        f: &mut Failures,
        enc: &'static Encoding,
        bytes: &[u8],
        cps: &[u32],
        both_ways: bool,
    ) {
        let got = decode(enc, bytes);
        f.check(got == cps, || {
            format!(
                "{} -> Unicode {bytes:02X?}: expected {cps:X?}, got {got:X?}",
                enc.name
            )
        });
        if both_ways {
            check_valid_encode(f, enc, cps, bytes);
        }
    }

    pub fn check_valid_encode(f: &mut Failures, enc: &'static Encoding, cps: &[u32], bytes: &[u8]) {
        let buf = encode(enc, cps);
        f.check(buf.out == bytes && buf.errors == 0, || {
            format!(
                "Unicode -> {} {cps:X?}: expected {bytes:02X?}, got {:02X?} ({} errors)",
                enc.name, buf.out, buf.errors
            )
        });
    }

    /// `convertInvalidString` from Unicode: `cps` encodes to `bytes`, with errors.
    pub fn check_invalid_encode(
        f: &mut Failures,
        enc: &'static Encoding,
        cps: &[u32],
        bytes: &[u8],
    ) {
        let buf = encode(enc, cps);
        f.check(buf.out == bytes && buf.errors > 0, || {
            format!(
                "Unicode -> {} {cps:X?}: expected {bytes:02X?} with errors, got {:02X?} ({} errors)",
                enc.name, buf.out, buf.errors
            )
        });
    }

    /// `testAllValidChars` from `enc`: every character alone, and all of
    /// them concatenated in a shuffled order.
    pub fn test_all_valid_chars(
        f: &mut Failures,
        enc: &'static Encoding,
        chars: &ToUni,
        both_ways: bool,
        rng: &mut Rng,
    ) {
        let mut keys: Vec<&Vec<u8>> = chars.keys().collect();
        rng.shuffle(&mut keys);
        let (mut bytes, mut cps) = (Vec::new(), Vec::new());
        for k in &keys {
            bytes.extend_from_slice(k);
            cps.extend_from_slice(&chars[*k]);
        }
        check_valid(f, enc, &bytes, &cps, both_ways);
        for k in keys {
            check_valid(f, enc, k, &chars[k], both_ways);
        }
    }

    /// `testAllValidChars` from Unicode to `enc`.
    pub fn test_all_valid_codepoints(
        f: &mut Failures,
        enc: &'static Encoding,
        chars: &FromUni,
        rng: &mut Rng,
    ) {
        let mut keys: Vec<&Vec<u32>> = chars.keys().collect();
        rng.shuffle(&mut keys);
        let (mut bytes, mut cps) = (Vec::new(), Vec::new());
        for k in &keys {
            cps.extend_from_slice(k);
            bytes.extend_from_slice(&chars[*k]);
        }
        check_valid_encode(f, enc, &cps, &bytes);
        for k in keys {
            check_valid_encode(f, enc, k, &chars[k]);
        }
    }

    /// `testAllInvalidChars`: each bad sequence followed by a good character
    /// decodes to one error marker and that character.
    pub fn test_all_invalid_chars(
        f: &mut Failures,
        enc: &'static Encoding,
        bad: &BTreeSet<Vec<u8>>,
        good: &ToUni,
        rng: &mut Rng,
    ) {
        let mut goods: Vec<(&Vec<u8>, &Vec<u32>)> = good.iter().collect();
        rng.shuffle(&mut goods);
        for (i, b) in bad.iter().enumerate() {
            let (gb, gc) = goods[i % goods.len()];
            let mut bytes = b.clone();
            bytes.extend_from_slice(gb);
            let mut expected = vec![BAD_INPUT];
            expected.extend_from_slice(gc);
            let got = decode(enc, &bytes);
            f.check(got == expected, || {
                format!(
                    "{} -> Unicode {bytes:02X?}: expected {expected:X?}, got {got:X?}",
                    enc.name
                )
            });
        }
    }

    /// `testTruncatedChars`: each truncated character decodes to one error marker.
    pub fn test_truncated_chars(
        f: &mut Failures,
        enc: &'static Encoding,
        truncated: &BTreeSet<Vec<u8>>,
    ) {
        for t in truncated {
            let got = decode(enc, t);
            f.check(got == [BAD_INPUT], || {
                format!("{} -> Unicode {t:02X?}: got {got:X?}", enc.name)
            });
        }
    }

    /// `convertAllInvalidChars` from Unicode: each codepoint in `bad`
    /// followed by a good character encodes to '%' and that character.
    pub fn test_all_invalid_codepoints(
        f: &mut Failures,
        enc: &'static Encoding,
        bad: impl IntoIterator<Item = u32>,
        good: &FromUni,
        rng: &mut Rng,
    ) {
        let mut goods: Vec<(&Vec<u32>, &Vec<u8>)> = good.iter().collect();
        rng.shuffle(&mut goods);
        for (i, w) in bad.into_iter().enumerate() {
            let (gc, gb) = goods[i % goods.len()];
            let mut cps = vec![w];
            cps.extend_from_slice(gc);
            let mut expected = b"%".to_vec();
            expected.extend_from_slice(gb);
            check_invalid_encode(f, enc, &cps, &expected);
        }
    }

    /// Codepoints 0-0xFFFF which are not a key of `from` (`findInvalidChars`
    /// on a UTF-16BE table).
    pub fn invalid_bmp_codepoints(from: &FromUni) -> Vec<u32> {
        (0..=0xFFFF)
            .filter(|&cp| !from.contains_key(&vec![cp]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use std::collections::BTreeSet;

    fn set(map: &mut FromUni, cp: u32, bytes: &[u8]) {
        map.insert(vec![cp], bytes.to_vec());
    }

    // sjis_encoding.phpt
    #[test]
    fn sjis_exhaustive() {
        let enc = &MBFL_ENCODING_SJIS;
        let mut rng = Rng::new(999);
        let mut f = Failures::default();

        let (mut valid, mut from) = read_conversion_table("SHIFTJIS.txt");
        for i in 0..0x20u32 {
            valid.insert(vec![i as u8], vec![i]);
            from.insert(vec![i], vec![i as u8]);
        }
        // SJIS 0x5C and 0x7E are taken as the same ASCII bytes; 0x7F is DEL
        for b in [0x5Cu8, 0x7E, 0x7F] {
            valid.insert(vec![b], vec![b as u32]);
            set(&mut from, b as u32, &[b]);
        }
        set(&mut from, 0x203E, b"\x81\x50");
        set(&mut from, 0xAF, b"\x81\x50");
        set(&mut from, 0xA5, b"\x81\x8F");
        valid.insert(b"\x81\x5F".to_vec(), vec![0xFF3C]);
        set(&mut from, 0xFF3C, b"\x81\x5F");
        set(&mut from, 0xFFE2, b"\x81\xCA");
        set(&mut from, 0xFFE1, b"\x81\x92");
        set(&mut from, 0xFFE0, b"\x81\x91");
        set(&mut from, 0xFF5E, b"\x81\x60");
        set(&mut from, 0xFF0D, b"\x81\x7C");
        set(&mut from, 0x2225, b"\x81\x61");
        assert!(valid.len() > 7000 && from.len() > 7000);

        test_all_valid_chars(&mut f, enc, &valid, true, &mut rng);

        let (invalid, truncated) =
            find_invalid_chars(&valid, &[(0x81..=0x9F, 2), (0xE0..=0xEF, 2)]);
        assert!(invalid.len() > 5000 && truncated.len() == 0x1F + 0x10);
        test_all_invalid_chars(&mut f, enc, &invalid, &valid, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);

        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);
        test_all_invalid_codepoints(&mut f, enc, invalid_bmp_codepoints(&from), &from, &mut rng);

        check_valid_encode(&mut f, enc, &[0xFF5E], b"\x81\x60");
        for bad in [&b"\x80"[..], b"\x81\x20", b"\xEA\xA9"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("SJIS");
    }

    #[test]
    fn sjis_buffer_limits() {
        let enc = &MBFL_ENCODING_SJIS;
        // Output buffer full: the rest of the input is left unconsumed
        let mut inp: &[u8] = b"a\x82\xA0b";
        let mut out = [0u32; 2];
        let n = mb_sjis_to_wchar(&mut inp, &mut out, &mut 0);
        assert_eq!((n, &out[..], inp), (2, &[0x61, 0x3042][..], &b"b"[..]));
        // A bad lead byte 0x80, 0xA0 or 0xF0-0xFF does not swallow the next byte
        assert_eq!(decode(enc, b"\xF0A\x80"), [BAD_INPUT, 0x41, BAD_INPUT]);
        assert_eq!(
            decode_with(enc, b"\x82\xA0\x82\xA2\x82\xA4", 1),
            [0x3042, 0x3044, 0x3046]
        );
        assert!(decode(enc, b"").is_empty());
    }

    fn read_macjapanese() -> (ToUni, FromUni) {
        let mut valid = ToUni::new();
        let mut from = FromUni::new();
        for line in data_lines("MacJapanese-SJIS.txt") {
            if line.starts_with('#') {
                continue;
            }
            let Some((code, rest)) = line.strip_prefix("0x").and_then(hex) else {
                continue;
            };
            let Some(mut rest) = rest.trim_start().strip_prefix("0x") else {
                continue;
            };
            let mut cps = Vec::new();
            while let Some((cp, r)) = hex(rest) {
                cps.push(cp);
                match r.strip_prefix("+0x") {
                    Some(r) => rest = r,
                    None => break,
                }
            }
            if cps.is_empty() {
                continue;
            }
            let bytes = if code < 256 {
                vec![code as u8]
            } else {
                be16(code)
            };
            valid.insert(bytes.clone(), cps.clone());
            from.insert(cps, bytes);
        }
        (valid, from)
    }

    // sjismac_encoding.phpt
    #[test]
    fn sjismac_exhaustive() {
        let enc = &MBFL_ENCODING_SJIS_MAC;
        let mut rng = Rng::new(300);
        let mut f = Failures::default();

        let (mut valid, mut from) = read_macjapanese();
        // 0x0-0x1F and 0x7F are valid 'control characters'
        for i in (0..0x20u32).chain([0x7F]) {
            valid.insert(vec![i as u8], vec![i]);
            from.insert(vec![i], vec![i as u8]);
        }
        set(&mut from, 0x2015, b"\x81\x5C");
        set(&mut from, 0x203E, b"\x81\x50");
        set(&mut from, 0xAF, b"\x81\x50");
        set(&mut from, 0xFF5E, b"\x81\x60");
        let multi = from.keys().filter(|k| k.len() > 1).count();
        assert!(valid.len() > 7300 && multi == 56, "{} {multi}", valid.len());

        test_all_valid_chars(&mut f, enc, &valid, true, &mut rng);

        let (invalid, truncated) =
            find_invalid_chars(&valid, &[(0x81..=0x9F, 2), (0xE0..=0xED, 2)]);
        test_all_invalid_chars(&mut f, enc, &invalid, &valid, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);

        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);

        // A transcoding hint (U+F860-F862) followed by a codepoint which can
        // continue it is not a single error, so give the hints a following
        // character which cannot.
        let (hints, others): (Vec<u32>, Vec<u32>) = invalid_bmp_codepoints(&from)
            .into_iter()
            .partition(|cp| (0xF860..=0xF862).contains(cp));
        test_all_invalid_codepoints(&mut f, enc, others, &from, &mut rng);
        let mut kana = FromUni::new();
        set(&mut kana, 0x3042, b"\x82\xA0");
        test_all_invalid_codepoints(&mut f, enc, hints, &kana, &mut rng);

        // Regression test
        check_valid(&mut f, enc, b"\x81\x63\x9F\x6F", &[0x2026, 0x6BAA], true);

        // Special combining characters when *not* appearing in an expected combination
        check_invalid_encode(&mut f, enc, &[0x2010, 0xF87A], b"\x81\x5D%");
        check_invalid_encode(&mut f, enc, &[0x2010, 0x20DD], b"\x81\x5D%");
        check_invalid_encode(&mut f, enc, &[0x2010, 0xF87F], b"\x81\x5D%");
        check_invalid_encode(&mut f, enc, &[0x21E6, 0xF87E], b"\x86\xD0%");

        check_invalid_encode(&mut f, enc, &[0xF860, 0x30, 0x1234], b"%%%");
        check_invalid_encode(&mut f, enc, &[0xF861, 0x46, 0x41, 0x1234], b"%%%%");
        check_invalid_encode(&mut f, enc, &[0xF862, 0x58, 0x49, 0x49, 0x1234], b"%%%%%");

        for bad in [&b"\x81"[..], b"\x81\x20", b"\xED\x9F"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("SJIS-mac");
    }

    #[test]
    fn sjismac_buffer_and_state() {
        let enc = &MBFL_ENCODING_SJIS_MAC;
        let (_, from) = read_macjapanese();

        // 0x85AB is 5 codepoints; with only 4 slots left it is not consumed
        let mut inp: &[u8] = b"a\x85\xAB";
        let mut out = [0u32; 5];
        let n = mb_sjismac_to_wchar(&mut inp, &mut out, &mut 0);
        assert_eq!((n, inp), (1, &b"\x85\xAB"[..]));
        assert_eq!(
            decode_with(enc, b"aa\xFF\x85\xAB", 5),
            [0x61, 0x61, 0x2026, 0xF87F, 0xF862, 0x58, 0x49, 0x49, 0x49]
        );

        // Multi-codepoint characters split across calls of the encoder
        let chars: [&[u32]; 4] = [
            &[0xF862, 0x58, 0x49, 0x49, 0x49],
            &[0x2026, 0xF87F],
            &[0x2010, 0xF87E],
            &[0xF861, 0x46, 0x41, 0x58],
        ];
        let cps = chars.concat();
        let expected: Vec<u8> = chars.iter().flat_map(|c| from[*c].clone()).collect();
        for chunk in 1..=cps.len() {
            let buf = encode_with(enc, &cps, chunk, b'%' as u32, ILLEGAL_MODE_CHAR);
            assert_eq!((&buf.out, buf.errors), (&expected, 0), "chunk size {chunk}");
        }

        // As in C, a partial match of the first transcoding hint sequence
        // (U+F860 U+0030 ...) is saved as a state indistinguishable from a
        // lone U+F860, so it does not survive a buffer boundary.
        let buf = encode_with(
            enc,
            &[0xF860, 0x30, 0x2E],
            3,
            b'%' as u32,
            ILLEGAL_MODE_CHAR,
        );
        assert_eq!(buf.out, from[&vec![0xF860, 0x30, 0x2E]]);
        let buf = encode_with(
            enc,
            &[0xF860, 0x30, 0x2E],
            2,
            b'%' as u32,
            ILLEGAL_MODE_CHAR,
        );
        assert_eq!((&buf.out[..], buf.errors), (&b"%."[..], 1));

        // A trailing s_form codepoint uses its fallback mapping
        let buf = encode_with(enc, &[0x2010], 1, b'%' as u32, ILLEGAL_MODE_CHAR);
        assert_eq!(buf.out, b"\x81\x5D");
    }

    /// Shift-JIS bytes of a JIS row/cell (`shiftJISEncode` in the test).
    fn shift_jis_encode(ku: u32, ten: u32) -> Vec<u8> {
        let (ku, ten) = (ku - 0x21, ten - 0x21);
        let (hi, lo) = (ku >> 1, ku % 2);
        let lead = if hi < 31 { hi + 0x81 } else { hi - 31 + 0xE0 };
        let trail = if lo == 0 {
            if ten < 63 {
                ten + 0x40
            } else {
                ten - 63 + 0x80
            }
        } else {
            ten + 0x9F
        };
        vec![lead as u8, trail as u8]
    }

    struct MobileTables {
        docomo: ToUni,
        kddi: ToUni,
        softbank: ToUni,
        non_invertible: ToUni,
        non_invertible_docomo: ToUni,
        non_invertible_softbank: ToUni,
        from: FromUni,
        invalid_codepoints: BTreeSet<u32>,
    }

    /// The character tables of sjis_mobile_encodings.phpt.
    fn mobile_tables() -> MobileTables {
        // The SJIS-Mobile encodings all use the Windows-932 extensions
        let (mut sjis, mut from) = read_conversion_table("CP932.txt");
        set(&mut from, 0x301C, b"\x81\x60");
        set(&mut from, 0x2212, b"\x81\x7C");
        set(&mut from, 0x203E, b"\x81\x50");
        set(&mut from, 0x2016, b"\x81\x61");
        set(&mut from, 0xAF, b"\x81\x50");
        set(&mut from, 0xAC, b"\x81\xCA");
        set(&mut from, 0xA5, b"\x81\x8F");
        set(&mut from, 0xA3, b"\x81\x92");
        set(&mut from, 0xA2, b"\x81\x91");

        // User area 0xF040-0xF9FC maps to U+E000-E757
        let mut cp = 0xE000;
        for i in 0xF0..=0xF9u8 {
            for j in (0x40..=0xFCu8).filter(|&j| j != 0x7F) {
                sjis.insert(vec![i, j], vec![cp]);
                from.insert(vec![cp], vec![i, j]);
                cp += 1;
            }
        }

        let mut invalid_codepoints: BTreeSet<u32> =
            invalid_bmp_codepoints(&from).into_iter().collect();

        // 0xFA00-0xFC4B (and a few others) duplicate other codes, which are
        // preferred when converting from Unicode
        let mut non_invertible = ToUni::new();
        let collisions = [
            0x8790, 0x8791, 0x8792, 0x8795, 0x8796, 0x8797, 0x879A, 0x879B, 0x879C, 0xEEF9,
        ];
        for i in (0xFA00..=0xFC4B).chain(collisions) {
            let bytes = be16(i);
            if let Some(cps) = sjis.get(&bytes) {
                from.remove(cps);
                non_invertible.insert(bytes, cps.clone());
            }
        }
        let mut non_invertible_softbank = non_invertible.clone();
        let mut non_invertible_docomo = non_invertible.clone();

        // Vendor-specific emoji
        let mut docomo = sjis.clone();
        let mut kddi = sjis.clone();
        let mut softbank = sjis;
        let mut sb_emoji = ToUni::new();
        let truthy = |s: &str| !s.is_empty() && s != "0";
        for line in data_lines("EmojiSources.txt") {
            if line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.trim_end().split(';').collect();
            if fields.len() < 4 {
                continue;
            }
            let cps = match fields[0].split_once(' ') {
                Some((a, b)) if hex(a).is_some() && hex(b).is_some() => vec![hexdec(a), hexdec(b)],
                _ => {
                    let cp = hexdec(fields[0]);
                    invalid_codepoints.remove(&cp);
                    vec![cp]
                }
            };
            if truthy(fields[1]) {
                docomo.insert(be16(hexdec(fields[1])), cps.clone());
            }
            if truthy(fields[2]) {
                kddi.insert(be16(hexdec(fields[2])), cps.clone());
            }
            if truthy(fields[3]) {
                let bytes = be16(hexdec(fields[3]));
                non_invertible_softbank.remove(&bytes);
                sb_emoji.insert(bytes, cps);
            }
        }

        // Vendor-specific emoji which are not in EmojiSources.txt
        let docomo_extra = [
            (0xF94A, 0xFEE16),
            (0xF94B, 0xFEE17),
            (0xF94C, 0xFEE18),
            (0xF94D, 0xFEE19),
            (0xF94E, 0xFEE1A),
            (0xF94F, 0xFEE1B),
            (0xF953, 0xFEE1C),
            (0xF954, 0xFEE1D),
            (0xF958, 0xFEE1E),
            (0xF959, 0xFEE1F),
            (0xF95A, 0xFEE20),
            (0xF95F, 0xFEE21),
            (0xF960, 0xFEE22),
            (0xF961, 0xFEE23),
            (0xF962, 0xFEE24),
            (0xF963, 0xFEE25),
            (0xF964, 0x25EA),
            (0xF965, 0x25A0),
            (0xF966, 0x25BF),
            (0xF967, 0xFEE29),
            (0xF968, 0xFEE2A),
            (0xF969, 0xFEE2B),
            (0xF96A, 0x2020),
            (0xF96B, 0xFEE2D),
            (0xF96C, 0xFEE2E),
            (0xF96D, 0xFEE2F),
            (0xF96E, 0xFEE30),
            (0xF96F, 0xFEE31),
            (0xF970, 0xFEE32),
            (0xF971, 0xFEE33),
            (0xF975, 0xFEE10),
            (0xF976, 0xFEE11),
            (0xF978, 0xFEE12),
            (0xF979, 0xFEE13),
            (0xF984, 0x27BF),
            (0xF986, 0xFE82D),
            (0xF9B1, 0xFEE14),
            (0xF9B2, 0xFEE15),
        ];
        for (code, cp) in docomo_extra {
            docomo.insert(be16(code), vec![cp]);
            if matches!(code, 0xF964 | 0xF965 | 0xF966 | 0xF96A) {
                non_invertible_docomo.insert(be16(code), vec![cp]);
            }
        }
        invalid_codepoints.remove(&0x27BF);

        let kddi_extra = [
            (0xF794, 0xFEE40),
            (0xF7CF, 0xFEE41),
            (0xF370, 0xFEE42),
            (0xF478, 0xFEE43),
            (0xF486, 0xFEE44),
            (0xF48E, 0xFEE45),
            (0xF48F, 0xFEE46),
            (0xF490, 0xFEE47),
            (0xF491, 0xFEE48),
            (0xF492, 0xFEE49),
            (0xF493, 0xFEE4A),
            (0xF748, 0xFE82D),
            (0xF7A3, 0xFE83C),
            (0xF7D2, 0xFEB89),
        ];
        for (code, cp) in kddi_extra {
            kddi.insert(be16(code), vec![cp]);
        }

        let sb_extra = [
            (0xF7B1, 0x27BF),
            (0xF7F4, 0xFEE77),
            (0xF7F5, 0xFEE78),
            (0xF7F6, 0xFEE79),
            (0xF7F7, 0xFEE7A),
            (0xF7F8, 0xFEE7B),
            (0xF7F9, 0xFEE7C),
            (0xF7FA, 0xFEE7D),
        ];
        for (code, cp) in sb_extra {
            sb_emoji.insert(be16(code), vec![cp]);
        }
        for i in 0xFBD8..=0xFBDE {
            sb_emoji.insert(be16(i), vec![0xFEE70 + i - 0xFBD8]);
            non_invertible_softbank.remove(&be16(i));
        }
        sb_emoji.insert(be16(0xFBAA), vec![0xFE4C5]);
        non_invertible_softbank.remove(&be16(0xFBAA));

        for (k, v) in &sb_emoji {
            softbank.insert(k.clone(), v.clone());
        }

        // SoftBank's alternative representation of emoji: ESC $ and a letter
        // for the ku code, then single bytes for the ten codes
        let escapes = [
            (b'G', 0x91, 0x7A),
            (b'E', 0x8D, 0x7A),
            (b'F', 0x8E, 0x7A),
            (b'O', 0x92, 0x6D),
            (b'P', 0x95, 0x6C),
            (b'Q', 0x96, 0x5E),
        ];
        for (ch, ku, max_ten) in escapes {
            for ten in 0x21..=max_ten {
                let sjis = shift_jis_encode(ku, ten);
                if sb_emoji.contains_key(&sjis) {
                    let bytes = vec![0x1B, b'$', ch, ten as u8];
                    let unicode = softbank[&sjis].clone();
                    non_invertible_softbank.insert(bytes.clone(), unicode.clone());
                    softbank.insert(bytes, unicode);
                }
            }
        }
        // A bare ESC is not valid for SoftBank
        softbank.remove(&vec![0x1B]);

        MobileTables {
            docomo,
            kddi,
            softbank,
            non_invertible,
            non_invertible_docomo,
            non_invertible_softbank,
            from,
            invalid_codepoints,
        }
    }

    /// `testSJISVariant` of sjis_mobile_encodings.phpt.
    fn test_sjis_variant(
        f: &mut Failures,
        enc: &'static Encoding,
        mut valid: ToUni,
        mut non_invertible: ToUni,
        t: &MobileTables,
        rng: &mut Rng,
    ) {
        let (mut invalid, mut truncated) =
            find_invalid_chars(&valid, &[(0xE0..=0xFC, 2), (0x81..=0x9F, 2)]);
        for ch in *b"GEFOPQ" {
            invalid.remove(&vec![0x1B, b'$', ch, 0x0F]);
            truncated.remove(&vec![0x1B, b'$', ch]);
        }

        let mut escapes = Vec::new();
        for bytes in non_invertible.keys() {
            valid.remove(bytes);
            if bytes[0] == 0x1B {
                escapes.push(bytes.clone());
            }
        }
        // 0x0F terminates a run of emoji encoded using an ESC sequence
        for bytes in escapes {
            let v = non_invertible.remove(&bytes).unwrap();
            let mut terminated = bytes;
            terminated.push(0x0F);
            non_invertible.insert(terminated, v);
        }

        test_all_valid_chars(f, enc, &valid, true, rng);
        test_all_valid_chars(f, enc, &non_invertible, false, rng);

        test_all_invalid_chars(f, enc, &invalid, &valid, rng);
        test_truncated_chars(f, enc, &truncated);

        test_all_invalid_codepoints(f, enc, t.invalid_codepoints.iter().copied(), &t.from, rng);

        for bad in [&b"\x80"[..], b"\x81\x20", b"\xEA\xA9"] {
            check_valid(f, enc, bad, &[BAD_INPUT], false);
        }

        // A lone Regional Indicator codepoint (C) can't be converted
        check_invalid_encode(f, enc, &[0x1F1E9], b"%");
        check_invalid_encode(f, enc, &[0x1F1E9, 0x41], b"%A");
    }

    // sjis_mobile_encodings.phpt
    #[test]
    fn sjis_mobile_exhaustive() {
        let t = mobile_tables();
        assert!(
            t.docomo.len() > 9500
                && t.non_invertible.len() > 300
                && t.invalid_codepoints.len() > 50000
        );
        assert!(t.softbank.keys().filter(|k| k[0] == 0x1B).count() > 400);
        let mut rng = Rng::new(818);
        let mut f = Failures::default();

        let docomo = &MBFL_ENCODING_SJIS_DOCOMO;
        test_sjis_variant(
            &mut f,
            docomo,
            t.docomo.clone(),
            t.non_invertible_docomo.clone(),
            &t,
            &mut rng,
        );
        let kddi = &MBFL_ENCODING_SJIS_KDDI;
        test_sjis_variant(
            &mut f,
            kddi,
            t.kddi.clone(),
            t.non_invertible.clone(),
            &t,
            &mut rng,
        );
        let sb = &MBFL_ENCODING_SJIS_SB;
        test_sjis_variant(
            &mut f,
            sb,
            t.softbank.clone(),
            t.non_invertible_softbank.clone(),
            &t,
            &mut rng,
        );
        f.assert_none("SJIS-Mobile");
    }

    #[test]
    fn sjis_mobile_regressions() {
        let mut f = Failures::default();
        let sb = &MBFL_ENCODING_SJIS_SB;

        // Special SoftBank escape sequences can appear at end of string
        for esc in [&b"\x1B$O"[..], b"\x1B$P", b"\x1B$Q"] {
            check_valid(&mut f, sb, esc, &[], false);
        }
        // Invalid and truncated escape sequences
        check_valid(&mut f, sb, b"\x1B$X", &[BAD_INPUT], false);
        check_valid(&mut f, sb, b"\x1B$", &[BAD_INPUT], false);

        // The escape state carries over to the next buffer
        let ten = (0x21..=0x7Au8)
            .find(|&t| {
                let w = decode(sb, &[0x1B, b'$', b'G', t]);
                w.len() == 1 && w[0] != BAD_INPUT
            })
            .unwrap();
        let emoji = decode(sb, &[0x1B, b'$', b'G', ten]);
        let mut bytes = b"\x1B$G".to_vec();
        bytes.extend([ten; 7]);
        bytes.extend(b"\x0Fa");
        assert_eq!(
            decode_with(sb, &bytes, 5),
            [emoji.repeat(7), vec![0x61]].concat()
        );

        let mobile: [(&'static Encoding, &[u8], &[u8]); 3] = [
            (&MBFL_ENCODING_SJIS_DOCOMO, b"\xF9\xD6", b"\xF9\x90"),
            (&MBFL_ENCODING_SJIS_KDDI, b"\xF7\x74", b"\xF7\xC9"),
            (&MBFL_ENCODING_SJIS_SB, b"\xF7\xEE", b"\xF7\xC5"),
        ];
        for (enc, copyright, keycap0) in mobile {
            // Enough space in the output buffer when the input is shorter
            check_valid(&mut f, enc, &copyright.repeat(4), &[0xA9; 4], true);

            // Digits at the end of the input are not dropped
            for d in b'0'..=b'9' {
                check_valid(
                    &mut f,
                    enc,
                    &[b'a', b'b', b'c', d],
                    &[0x61, 0x62, 0x63, d as u32],
                    true,
                );
            }

            for i in 0..=256 {
                // A digit at the end of one buffer, U+20E3 at the start of the next
                let mut cps = vec![0x61; i];
                cps.extend([0x30, 0x20E3]);
                let mut bytes = vec![b'a'; i];
                bytes.extend_from_slice(keycap0);
                check_valid(&mut f, enc, &bytes, &cps, true);

                // ... and a digit at the end of one buffer, no U+20E3 next
                check_valid(&mut f, enc, &vec![b'0'; i], &vec![0x30; i], true);
            }
        }

        // Found by a fuzzer: not enough space made in the output buffer
        let s = b"\x04\xff\x930\x00\xffUTF7~'F\x00A\x00\xffA\x0018030@\x00[\x1b$EEEEE\x5C\x80(8~\x00F\x00zgb-18030$\x008~\x00F\x00z-gb-18EUC_JP-2004\x00z-g0\x0018030\x00b-18030$\x008~\x00F\x00z-gb-18EUC_JP-2004\x00z-g0\x0018030\x00";
        let cps = decode(sb, s);
        encode(sb, &cps);
        f.assert_none("SJIS-Mobile regressions");
    }

    // sjis2004_encoding.phpt
    #[test]
    fn sjis2004_exhaustive() {
        let enc = &MBFL_ENCODING_SJIS2004;
        let mut rng = Rng::new(101);
        let mut f = Failures::default();

        let (valid, mut from) = read_jisx0213_table("SJIS-2004.txt", true);
        set(&mut from, 0x7E, b"\x7E");
        set(&mut from, 0x5C, b"\x5C");
        assert!(valid.len() > 11000 && valid.values().filter(|v| v.len() == 2).count() == 25);

        test_all_valid_chars(&mut f, enc, &valid, true, &mut rng);

        let (invalid, truncated) =
            find_invalid_chars(&valid, &[(0x81..=0x9F, 2), (0xE0..=0xFC, 2)]);
        test_all_invalid_chars(&mut f, enc, &invalid, &valid, &mut rng);
        test_truncated_chars(&mut f, enc, &truncated);

        test_all_valid_codepoints(&mut f, enc, &from, &mut rng);
        test_all_invalid_codepoints(&mut f, enc, invalid_bmp_codepoints(&from), &from, &mut rng);

        // The first codepoint looks like it might start a combined
        // character, but the second one doesn't match
        check_valid_encode(&mut f, enc, &[0x304B, 0x41], b"\x82\xA9A");

        for bad in [&b"\x80"[..], b"\x81\x20", b"\xFC\xF5"] {
            check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
        }
        f.assert_none("SJIS-2004");
    }

    #[test]
    fn sjis2004_combining_across_buffers() {
        let enc = &MBFL_ENCODING_SJIS2004;
        // か + U+309A is one character; split between two calls of the encoder
        for chunk in 1..=3 {
            let buf = encode_with(
                enc,
                &[0x41, 0x304B, 0x309A],
                chunk,
                b'%' as u32,
                ILLEGAL_MODE_CHAR,
            );
            assert_eq!(buf.out, b"A\x82\xF5", "chunk size {chunk}");
        }
        // Combining characters need two output slots
        let mut inp: &[u8] = b"a\x82\xF5";
        let mut out = [0u32; 2];
        let n = mb_sjis2004_to_wchar(&mut inp, &mut out, &mut 0);
        assert_eq!((n, inp), (1, &b"\x82\xF5"[..]));
        assert_eq!(
            decode_with(enc, b"\x82\xF5\x82\xF5", 3),
            [0x304B, 0x309A, 0x304B, 0x309A]
        );
        assert_eq!(decode(enc, &[0x82]), [BAD_INPUT]);
    }

    struct Cp932Tables {
        valid: ToUni,
        from: FromUni,
        non_invertible: ToUni,
        invalid: BTreeSet<Vec<u8>>,
        invalid_codepoints: Vec<u32>,
    }

    /// The tables of cp932_encoding.phpt.
    fn cp932_tables() -> Cp932Tables {
        let (mut valid, mut from) = read_conversion_table("CP932.txt");

        // User area 0xF040-0xF9FC maps to U+E000-E757
        let mut cp = 0xE000;
        for i in 0xF0..=0xF9u8 {
            for j in (0x40..=0xFCu8).filter(|&j| j != 0x7F) {
                valid.insert(vec![i, j], vec![cp]);
                from.insert(vec![cp], vec![i, j]);
                cp += 1;
            }
        }

        set(&mut from, 0xA2, b"\x81\x91");
        set(&mut from, 0xA3, b"\x81\x92");
        set(&mut from, 0xA5, b"\x5C");
        set(&mut from, 0x301C, b"\x81\x60");
        set(&mut from, 0x2212, b"\x81\x7C");
        set(&mut from, 0x2016, b"\x81\x61");
        set(&mut from, 0xAC, b"\x81\xCA");
        set(&mut from, 0xAF, b"\x81\x50");
        set(&mut from, 0x203E, b"\x7E");

        let (invalid, _) = find_invalid_chars(&valid, &[(0x81..=0x9F, 2), (0xE0..=0xFC, 2)]);
        let invalid_codepoints = invalid_bmp_codepoints(&from);

        // Codes which map to the same codepoints as other codes
        let mut non_invertible = ToUni::new();
        let others = [
            0x8790, 0x8791, 0x8792, 0x8795, 0x8796, 0x8797, 0x879A, 0x879B, 0x879C, 0xFA54, 0xFA58,
            0xFA59, 0xFA5A, 0xFA5B,
        ];
        for i in (0xED00..=0xEEFF).chain(0xFA4A..=0xFA53).chain(others) {
            let bytes = be16(i);
            if let Some(cps) = valid.remove(&bytes) {
                from.remove(&cps);
                non_invertible.insert(bytes, cps);
            }
        }
        Cp932Tables {
            valid,
            from,
            non_invertible,
            invalid,
            invalid_codepoints,
        }
    }

    // cp932_encoding.phpt
    #[test]
    fn cp932_and_sjiswin_exhaustive() {
        let mut rng = Rng::new(4321);
        let mut f = Failures::default();
        let mut t = cp932_tables();
        let sizes = (t.valid.len(), t.non_invertible.len(), t.invalid.len());
        assert!(
            sizes.0 > 9000 && sizes.1 == 398 && sizes.2 > 5000,
            "{sizes:?}"
        );

        for enc in [&MBFL_ENCODING_CP932, &MBFL_ENCODING_SJISWIN] {
            if enc.is(NoEncoding::Sjiswin) {
                // SJIS-win maps U+00A5 and U+203E to JIS X 0208 characters
                set(&mut t.from, 0xA5, b"\x81\x8F");
                set(&mut t.from, 0x203E, b"\x81\x50");
            }
            test_all_valid_chars(&mut f, enc, &t.valid, true, &mut rng);
            for (bytes, cps) in &t.non_invertible {
                check_valid(&mut f, enc, bytes, cps, false);
            }
            test_all_invalid_chars(&mut f, enc, &t.invalid, &t.valid, &mut rng);
            test_all_invalid_codepoints(
                &mut f,
                enc,
                t.invalid_codepoints.iter().copied(),
                &t.from,
                &mut rng,
            );

            for bad in [&b"\x80"[..], b"\xEA", b"\x81\x20", b"\xEA\xA9"] {
                check_valid(&mut f, enc, bad, &[BAD_INPUT], false);
            }
        }
        f.assert_none("CP932");
    }

    #[test]
    fn cp932_specifics() {
        let mut f = Failures::default();
        let (cp932, sjiswin) = (&MBFL_ENCODING_CP932, &MBFL_ENCODING_SJISWIN);
        check_valid_encode(&mut f, cp932, &[0xA5, 0x203E], b"\x5C\x7E");
        check_valid_encode(&mut f, sjiswin, &[0xA5, 0x203E], b"\x81\x8F\x81\x50");
        // U+0000 is emitted as is; the user area round-trips
        check_valid(
            &mut f,
            cp932,
            b"\x00\xF0\x40\xF9\xFC",
            &[0, 0xE000, 0xE757],
            true,
        );
        // SJIS-win reports errors through the CP932 encoder: same marker
        check_invalid_encode(&mut f, sjiswin, &[0x10000], b"%");
        f.assert_none("CP932 specifics");
    }
}
