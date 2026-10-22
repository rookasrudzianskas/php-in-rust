//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UTF8: Encoding = Encoding {
    no: NoEncoding::Utf8,
    name: "UTF-8",
    mime_name: Some("UTF-8"),
    aliases: &["utf8"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_to_wchar,
    from_wchar: mb_wchar_to_utf8,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_DOCOMO: Encoding = Encoding {
    no: NoEncoding::Utf8Docomo,
    name: "UTF-8-Mobile#DOCOMO",
    mime_name: Some("UTF-8"),
    aliases: &["UTF-8-DOCOMO", "UTF8-DOCOMO"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_docomo_to_wchar,
    from_wchar: mb_wchar_to_utf8_docomo,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_KDDI_A: Encoding = Encoding {
    no: NoEncoding::Utf8KddiA,
    name: "UTF-8-Mobile#KDDI-A",
    mime_name: Some("UTF-8"),
    aliases: &[],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_kddi_a_to_wchar,
    from_wchar: mb_wchar_to_utf8_kddi_a,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_KDDI_B: Encoding = Encoding {
    no: NoEncoding::Utf8KddiB,
    name: "UTF-8-Mobile#KDDI-B",
    mime_name: Some("UTF-8"),
    aliases: &["UTF-8-Mobile#KDDI", "UTF-8-KDDI", "UTF8-KDDI"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_kddi_b_to_wchar,
    from_wchar: mb_wchar_to_utf8_kddi_b,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub static MBFL_ENCODING_UTF8_SB: Encoding = Encoding {
    no: NoEncoding::Utf8Sb,
    name: "UTF-8-Mobile#SOFTBANK",
    mime_name: Some("UTF-8"),
    aliases: &["UTF-8-SOFTBANK", "UTF8-SOFTBANK"],
    mblen_table: Some(&mblen::MBLEN_TABLE_UTF8),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_utf8_sb_to_wchar,
    from_wchar: mb_wchar_to_utf8_sb,
    check: None,
    cut: Some(mb_cut_utf8),
};

pub fn mb_utf8_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_to_wchar")
}

pub fn mb_wchar_to_utf8(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8")
}

pub fn mb_cut_utf8(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    todo!("mb_cut_utf8")
}

pub fn mb_utf8_docomo_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_docomo_to_wchar")
}

pub fn mb_wchar_to_utf8_docomo(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_docomo")
}

pub fn mb_utf8_kddi_a_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_kddi_a_to_wchar")
}

pub fn mb_wchar_to_utf8_kddi_a(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_kddi_a")
}

pub fn mb_utf8_kddi_b_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_kddi_b_to_wchar")
}

pub fn mb_wchar_to_utf8_kddi_b(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_kddi_b")
}

pub fn mb_utf8_sb_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf8_sb_to_wchar")
}

pub fn mb_wchar_to_utf8_sb(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf8_sb")
}
