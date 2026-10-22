//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_EUC_JP: Encoding = Encoding {
    no: NoEncoding::EucJp,
    name: "EUC-JP",
    mime_name: Some("EUC-JP"),
    aliases: &["EUC", "EUC_JP", "eucJP", "x-euc-jp"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_eucjp_to_wchar,
    from_wchar: mb_wchar_to_eucjp,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_EUCJP2004: Encoding = Encoding {
    no: NoEncoding::Eucjp2004,
    name: "EUC-JP-2004",
    mime_name: Some("EUC-JP"),
    aliases: &["EUC_JP-2004"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_eucjp2004_to_wchar,
    from_wchar: mb_wchar_to_eucjp2004,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_EUCJP_WIN: Encoding = Encoding {
    no: NoEncoding::EucjpWin,
    name: "eucJP-win",
    mime_name: Some("EUC-JP"),
    aliases: &["eucJP-open", "eucJP-ms"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_eucjpwin_to_wchar,
    from_wchar: mb_wchar_to_eucjpwin,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP51932: Encoding = Encoding {
    no: NoEncoding::Cp51932,
    name: "CP51932",
    mime_name: Some("CP51932"),
    aliases: &["cp51932"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCJP),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp51932_to_wchar,
    from_wchar: mb_wchar_to_cp51932,
    check: None,
    cut: None,
};

pub fn mb_eucjp_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_eucjp_to_wchar")
}

pub fn mb_wchar_to_eucjp(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_eucjp")
}

pub fn mb_eucjp2004_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_eucjp2004_to_wchar")
}

pub fn mb_wchar_to_eucjp2004(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_eucjp2004")
}

pub fn mb_eucjpwin_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_eucjpwin_to_wchar")
}

pub fn mb_wchar_to_eucjpwin(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_eucjpwin")
}

pub fn mb_cp51932_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp51932_to_wchar")
}

pub fn mb_wchar_to_cp51932(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp51932")
}
