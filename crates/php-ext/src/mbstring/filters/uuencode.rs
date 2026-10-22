//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UUENCODE: Encoding = Encoding {
    no: NoEncoding::Uuencode,
    name: "UUENCODE",
    mime_name: Some("x-uuencode"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_uuencode_to_wchar,
    from_wchar: mb_wchar_to_uuencode,
    check: None,
    cut: None,
};

pub static VTBL_UUENCODE_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Uuencode,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_uudec,
    flush: common_flush,
};

pub fn mb_uuencode_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_uuencode_to_wchar")
}

pub fn mb_wchar_to_uuencode(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_uuencode")
}

pub fn mbfl_filt_conv_uudec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_uudec")
}
