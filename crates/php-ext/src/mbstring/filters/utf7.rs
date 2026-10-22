//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UTF7: Encoding = Encoding {
    no: NoEncoding::Utf7,
    name: "UTF-7",
    mime_name: Some("UTF-7"),
    aliases: &["utf7"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_UTF7_WCHAR),
    output_filter: Some(&VTBL_WCHAR_UTF7),
    to_wchar: mb_utf7_to_wchar,
    from_wchar: mb_wchar_to_utf7,
    check: Some(mb_check_utf7),
    cut: None,
};

pub static VTBL_UTF7_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Utf7,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_utf7_wchar,
    flush: mbfl_filt_conv_utf7_wchar_flush,
};

pub static VTBL_WCHAR_UTF7: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Utf7,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_utf7,
    flush: mbfl_filt_conv_wchar_utf7_flush,
};

pub fn mb_utf7_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf7_to_wchar")
}

pub fn mb_wchar_to_utf7(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf7")
}

pub fn mb_check_utf7(input: &[u8]) -> bool {
    todo!("mb_check_utf7")
}

pub fn mbfl_filt_conv_utf7_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_utf7_wchar")
}

pub fn mbfl_filt_conv_utf7_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_utf7_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_utf7(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_utf7")
}

pub fn mbfl_filt_conv_wchar_utf7_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_utf7_flush")
}
