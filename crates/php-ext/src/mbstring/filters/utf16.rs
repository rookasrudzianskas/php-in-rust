//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UTF16: Encoding = Encoding {
    no: NoEncoding::Utf16,
    name: "UTF-16",
    mime_name: Some("UTF-16"),
    aliases: &["utf16"],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf16_to_wchar,
    from_wchar: mb_wchar_to_utf16be,
    check: None,
    cut: Some(mb_cut_utf16),
};

pub static MBFL_ENCODING_UTF16BE: Encoding = Encoding {
    no: NoEncoding::Utf16be,
    name: "UTF-16BE",
    mime_name: Some("UTF-16BE"),
    aliases: &[],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf16be_to_wchar,
    from_wchar: mb_wchar_to_utf16be,
    check: None,
    cut: Some(mb_cut_utf16be),
};

pub static MBFL_ENCODING_UTF16LE: Encoding = Encoding {
    no: NoEncoding::Utf16le,
    name: "UTF-16LE",
    mime_name: Some("UTF-16LE"),
    aliases: &[],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf16le_to_wchar,
    from_wchar: mb_wchar_to_utf16le,
    check: None,
    cut: Some(mb_cut_utf16le),
};

pub fn mb_utf16_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf16_to_wchar")
}

pub fn mb_wchar_to_utf16be(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf16be")
}

pub fn mb_cut_utf16(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    todo!("mb_cut_utf16")
}

pub fn mb_utf16be_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf16be_to_wchar")
}

pub fn mb_cut_utf16be(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    todo!("mb_cut_utf16be")
}

pub fn mb_utf16le_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf16le_to_wchar")
}

pub fn mb_wchar_to_utf16le(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf16le")
}

pub fn mb_cut_utf16le(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    todo!("mb_cut_utf16le")
}
