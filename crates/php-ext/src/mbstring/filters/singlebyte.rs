//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_ASCII: Encoding = Encoding {
    no: NoEncoding::Ascii,
    name: "ASCII",
    mime_name: Some("US-ASCII"),
    aliases: &["ANSI_X3.4-1968", "iso-ir-6", "ANSI_X3.4-1986", "ISO_646.irv:1991", "US-ASCII", "ISO646-US", "us", "IBM367", "IBM-367", "cp367", "csASCII"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ascii_to_wchar,
    from_wchar: mb_wchar_to_ascii,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_1: Encoding = Encoding {
    no: NoEncoding::Iso8859_1,
    name: "ISO-8859-1",
    mime_name: Some("ISO-8859-1"),
    aliases: &["ISO8859-1", "latin1"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_1_to_wchar,
    from_wchar: mb_wchar_to_8859_1,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_2: Encoding = Encoding {
    no: NoEncoding::Iso8859_2,
    name: "ISO-8859-2",
    mime_name: Some("ISO-8859-2"),
    aliases: &["ISO8859-2", "latin2"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_2_to_wchar,
    from_wchar: mb_wchar_to_8859_2,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_3: Encoding = Encoding {
    no: NoEncoding::Iso8859_3,
    name: "ISO-8859-3",
    mime_name: Some("ISO-8859-3"),
    aliases: &["ISO8859-3", "latin3"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_3_to_wchar,
    from_wchar: mb_wchar_to_8859_3,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_4: Encoding = Encoding {
    no: NoEncoding::Iso8859_4,
    name: "ISO-8859-4",
    mime_name: Some("ISO-8859-4"),
    aliases: &["ISO8859-4", "latin4"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_4_to_wchar,
    from_wchar: mb_wchar_to_8859_4,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_5: Encoding = Encoding {
    no: NoEncoding::Iso8859_5,
    name: "ISO-8859-5",
    mime_name: Some("ISO-8859-5"),
    aliases: &["ISO8859-5", "cyrillic"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_5_to_wchar,
    from_wchar: mb_wchar_to_8859_5,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_6: Encoding = Encoding {
    no: NoEncoding::Iso8859_6,
    name: "ISO-8859-6",
    mime_name: Some("ISO-8859-6"),
    aliases: &["ISO8859-6", "arabic"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_6_to_wchar,
    from_wchar: mb_wchar_to_8859_6,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_7: Encoding = Encoding {
    no: NoEncoding::Iso8859_7,
    name: "ISO-8859-7",
    mime_name: Some("ISO-8859-7"),
    aliases: &["ISO8859-7", "greek"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_7_to_wchar,
    from_wchar: mb_wchar_to_8859_7,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_8: Encoding = Encoding {
    no: NoEncoding::Iso8859_8,
    name: "ISO-8859-8",
    mime_name: Some("ISO-8859-8"),
    aliases: &["ISO8859-8", "hebrew"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_8_to_wchar,
    from_wchar: mb_wchar_to_8859_8,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_9: Encoding = Encoding {
    no: NoEncoding::Iso8859_9,
    name: "ISO-8859-9",
    mime_name: Some("ISO-8859-9"),
    aliases: &["ISO8859-9", "latin5"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_9_to_wchar,
    from_wchar: mb_wchar_to_8859_9,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_10: Encoding = Encoding {
    no: NoEncoding::Iso8859_10,
    name: "ISO-8859-10",
    mime_name: Some("ISO-8859-10"),
    aliases: &["ISO8859-10", "latin6"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_10_to_wchar,
    from_wchar: mb_wchar_to_8859_10,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_13: Encoding = Encoding {
    no: NoEncoding::Iso8859_13,
    name: "ISO-8859-13",
    mime_name: Some("ISO-8859-13"),
    aliases: &["ISO8859-13"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_13_to_wchar,
    from_wchar: mb_wchar_to_8859_13,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_14: Encoding = Encoding {
    no: NoEncoding::Iso8859_14,
    name: "ISO-8859-14",
    mime_name: Some("ISO-8859-14"),
    aliases: &["ISO8859-14", "latin8"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_14_to_wchar,
    from_wchar: mb_wchar_to_8859_14,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_15: Encoding = Encoding {
    no: NoEncoding::Iso8859_15,
    name: "ISO-8859-15",
    mime_name: Some("ISO-8859-15"),
    aliases: &["ISO8859-15"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_15_to_wchar,
    from_wchar: mb_wchar_to_8859_15,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_8859_16: Encoding = Encoding {
    no: NoEncoding::Iso8859_16,
    name: "ISO-8859-16",
    mime_name: Some("ISO-8859-16"),
    aliases: &["ISO8859-16"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_8859_16_to_wchar,
    from_wchar: mb_wchar_to_8859_16,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP1251: Encoding = Encoding {
    no: NoEncoding::Cp1251,
    name: "Windows-1251",
    mime_name: Some("Windows-1251"),
    aliases: &["CP1251", "CP-1251", "WINDOWS-1251"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp1251_to_wchar,
    from_wchar: mb_wchar_to_cp1251,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP1252: Encoding = Encoding {
    no: NoEncoding::Cp1252,
    name: "Windows-1252",
    mime_name: Some("Windows-1252"),
    aliases: &["cp1252"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp1252_to_wchar,
    from_wchar: mb_wchar_to_cp1252,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP1254: Encoding = Encoding {
    no: NoEncoding::Cp1254,
    name: "Windows-1254",
    mime_name: Some("Windows-1254"),
    aliases: &["CP1254", "CP-1254", "WINDOWS-1254"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp1254_to_wchar,
    from_wchar: mb_wchar_to_cp1254,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP866: Encoding = Encoding {
    no: NoEncoding::Cp866,
    name: "CP866",
    mime_name: Some("CP866"),
    aliases: &["CP-866", "IBM866", "IBM-866"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp866_to_wchar,
    from_wchar: mb_wchar_to_cp866,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP850: Encoding = Encoding {
    no: NoEncoding::Cp850,
    name: "CP850",
    mime_name: Some("CP850"),
    aliases: &["CP-850", "IBM850", "IBM-850"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp850_to_wchar,
    from_wchar: mb_wchar_to_cp850,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_KOI8R: Encoding = Encoding {
    no: NoEncoding::Koi8r,
    name: "KOI8-R",
    mime_name: Some("KOI8-R"),
    aliases: &["KOI8R"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_koi8r_to_wchar,
    from_wchar: mb_wchar_to_koi8r,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_KOI8U: Encoding = Encoding {
    no: NoEncoding::Koi8u,
    name: "KOI8-U",
    mime_name: Some("KOI8-U"),
    aliases: &["KOI8U"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_koi8u_to_wchar,
    from_wchar: mb_wchar_to_koi8u,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_ARMSCII8: Encoding = Encoding {
    no: NoEncoding::Armscii8,
    name: "ArmSCII-8",
    mime_name: Some("ArmSCII-8"),
    aliases: &["ArmSCII8", "ARMSCII-8", "ARMSCII8"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_armscii8_to_wchar,
    from_wchar: mb_wchar_to_armscii8,
    check: None,
    cut: None,
};

pub fn mb_ascii_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ascii_to_wchar")
}

pub fn mb_wchar_to_ascii(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_ascii")
}

pub fn mb_8859_1_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_1_to_wchar")
}

pub fn mb_wchar_to_8859_1(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_1")
}

pub fn mb_8859_2_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_2_to_wchar")
}

pub fn mb_wchar_to_8859_2(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_2")
}

pub fn mb_8859_3_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_3_to_wchar")
}

pub fn mb_wchar_to_8859_3(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_3")
}

pub fn mb_8859_4_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_4_to_wchar")
}

pub fn mb_wchar_to_8859_4(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_4")
}

pub fn mb_8859_5_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_5_to_wchar")
}

pub fn mb_wchar_to_8859_5(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_5")
}

pub fn mb_8859_6_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_6_to_wchar")
}

pub fn mb_wchar_to_8859_6(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_6")
}

pub fn mb_8859_7_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_7_to_wchar")
}

pub fn mb_wchar_to_8859_7(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_7")
}

pub fn mb_8859_8_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_8_to_wchar")
}

pub fn mb_wchar_to_8859_8(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_8")
}

pub fn mb_8859_9_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_9_to_wchar")
}

pub fn mb_wchar_to_8859_9(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_9")
}

pub fn mb_8859_10_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_10_to_wchar")
}

pub fn mb_wchar_to_8859_10(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_10")
}

pub fn mb_8859_13_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_13_to_wchar")
}

pub fn mb_wchar_to_8859_13(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_13")
}

pub fn mb_8859_14_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_14_to_wchar")
}

pub fn mb_wchar_to_8859_14(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_14")
}

pub fn mb_8859_15_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_15_to_wchar")
}

pub fn mb_wchar_to_8859_15(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_15")
}

pub fn mb_8859_16_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_8859_16_to_wchar")
}

pub fn mb_wchar_to_8859_16(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_8859_16")
}

pub fn mb_cp1251_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp1251_to_wchar")
}

pub fn mb_wchar_to_cp1251(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp1251")
}

pub fn mb_cp1252_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp1252_to_wchar")
}

pub fn mb_wchar_to_cp1252(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp1252")
}

pub fn mb_cp1254_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp1254_to_wchar")
}

pub fn mb_wchar_to_cp1254(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp1254")
}

pub fn mb_cp866_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp866_to_wchar")
}

pub fn mb_wchar_to_cp866(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp866")
}

pub fn mb_cp850_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp850_to_wchar")
}

pub fn mb_wchar_to_cp850(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp850")
}

pub fn mb_koi8r_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_koi8r_to_wchar")
}

pub fn mb_wchar_to_koi8r(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_koi8r")
}

pub fn mb_koi8u_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_koi8u_to_wchar")
}

pub fn mb_wchar_to_koi8u(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_koi8u")
}

pub fn mb_armscii8_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_armscii8_to_wchar")
}

pub fn mb_wchar_to_armscii8(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_armscii8")
}
