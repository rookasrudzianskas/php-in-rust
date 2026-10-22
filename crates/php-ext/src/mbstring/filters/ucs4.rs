//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UCS4: Encoding = Encoding {
    no: NoEncoding::Ucs4,
    name: "UCS-4",
    mime_name: Some("UCS-4"),
    aliases: &["ISO-10646-UCS-4", "UCS4"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs4_to_wchar,
    from_wchar: mb_wchar_to_ucs4be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS4BE: Encoding = Encoding {
    no: NoEncoding::Ucs4be,
    name: "UCS-4BE",
    mime_name: Some("UCS-4BE"),
    aliases: &["byte4be"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs4be_to_wchar,
    from_wchar: mb_wchar_to_ucs4be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS4LE: Encoding = Encoding {
    no: NoEncoding::Ucs4le,
    name: "UCS-4LE",
    mime_name: Some("UCS-4LE"),
    aliases: &["byte4le"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs4le_to_wchar,
    from_wchar: mb_wchar_to_ucs4le,
    check: None,
    cut: None,
};

pub fn mb_ucs4_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ucs4_to_wchar")
}

pub fn mb_wchar_to_ucs4be(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_ucs4be")
}

pub fn mb_ucs4be_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ucs4be_to_wchar")
}

pub fn mb_ucs4le_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ucs4le_to_wchar")
}

pub fn mb_wchar_to_ucs4le(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_ucs4le")
}
