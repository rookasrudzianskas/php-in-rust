//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UCS2: Encoding = Encoding {
    no: NoEncoding::Ucs2,
    name: "UCS-2",
    mime_name: Some("UCS-2"),
    aliases: &["ISO-10646-UCS-2", "UCS2", "UNICODE"],
    mblen_table: None,
    flag: ENCTYPE_WCS2,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs2_to_wchar,
    from_wchar: mb_wchar_to_ucs2be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS2BE: Encoding = Encoding {
    no: NoEncoding::Ucs2be,
    name: "UCS-2BE",
    mime_name: Some("UCS-2BE"),
    aliases: &["byte2be"],
    mblen_table: None,
    flag: ENCTYPE_WCS2,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs2be_to_wchar,
    from_wchar: mb_wchar_to_ucs2be,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_UCS2LE: Encoding = Encoding {
    no: NoEncoding::Ucs2le,
    name: "UCS-2LE",
    mime_name: Some("UCS-2LE"),
    aliases: &["byte2le"],
    mblen_table: None,
    flag: ENCTYPE_WCS2,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_ucs2le_to_wchar,
    from_wchar: mb_wchar_to_ucs2le,
    check: None,
    cut: None,
};

pub fn mb_ucs2_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ucs2_to_wchar")
}

pub fn mb_wchar_to_ucs2be(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_ucs2be")
}

pub fn mb_ucs2be_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ucs2be_to_wchar")
}

pub fn mb_ucs2le_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_ucs2le_to_wchar")
}

pub fn mb_wchar_to_ucs2le(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_ucs2le")
}
