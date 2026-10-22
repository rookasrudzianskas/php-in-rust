//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

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

pub fn mb_euckr_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_euckr_to_wchar")
}

pub fn mb_wchar_to_euckr(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_euckr")
}

pub fn mb_uhc_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_uhc_to_wchar")
}

pub fn mb_wchar_to_uhc(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_uhc")
}
