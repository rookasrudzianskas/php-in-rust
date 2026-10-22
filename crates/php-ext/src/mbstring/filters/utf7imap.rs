//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_UTF7IMAP: Encoding = Encoding {
    no: NoEncoding::Utf7imap,
    name: "UTF7-IMAP",
    mime_name: None,
    aliases: &["mUTF-7"],
    mblen_table: None,
    flag: 0,
    input_filter: Some(&VTBL_UTF7IMAP_WCHAR),
    output_filter: Some(&VTBL_WCHAR_UTF7IMAP),
    to_wchar: mb_utf7imap_to_wchar,
    from_wchar: mb_wchar_to_utf7imap,
    check: Some(mb_check_utf7imap),
    cut: None,
};

pub static VTBL_UTF7IMAP_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Utf7imap,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_utf7imap_wchar,
    flush: mbfl_filt_conv_utf7imap_wchar_flush,
};

pub static VTBL_WCHAR_UTF7IMAP: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Utf7imap,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_utf7imap,
    flush: mbfl_filt_conv_wchar_utf7imap_flush,
};

pub fn mb_utf7imap_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_utf7imap_to_wchar")
}

pub fn mb_wchar_to_utf7imap(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_utf7imap")
}

pub fn mb_check_utf7imap(input: &[u8]) -> bool {
    todo!("mb_check_utf7imap")
}

pub fn mbfl_filt_conv_utf7imap_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_utf7imap_wchar")
}

pub fn mbfl_filt_conv_utf7imap_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_utf7imap_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_utf7imap(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_utf7imap")
}

pub fn mbfl_filt_conv_wchar_utf7imap_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_utf7imap_flush")
}
