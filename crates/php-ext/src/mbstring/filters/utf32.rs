//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UTF32: Encoding = Encoding {
    no: NoEncoding::Utf32,
    name: "UTF-32",
    mime_name: Some("UTF-32"),
    aliases: &["utf32"],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf32_to_wchar,
    from_wchar: mb_wchar_to_utf32be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UTF32BE: Encoding = Encoding {
    no: NoEncoding::Utf32be,
    name: "UTF-32BE",
    mime_name: Some("UTF-32BE"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf32be_to_wchar,
    from_wchar: mb_wchar_to_utf32be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UTF32LE: Encoding = Encoding {
    no: NoEncoding::Utf32le,
    name: "UTF-32LE",
    mime_name: Some("UTF-32LE"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf32le_to_wchar,
    from_wchar: mb_wchar_to_utf32le,
    check: None,
    cut: None,
};

pub fn mb_utf32_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf32_to_wchar")
}

pub fn mb_wchar_to_utf32be(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf32be")
}

pub fn mb_utf32be_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf32be_to_wchar")
}

pub fn mb_utf32le_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf32le_to_wchar")
}

pub fn mb_wchar_to_utf32le(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf32le")
}
