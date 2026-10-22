//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_BASE64: Encoding = Encoding {
    no: NoEncoding::Base64,
    name: "BASE64",
    mime_name: Some("BASE64"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_base64_to_wchar,
    from_wchar: mb_wchar_to_base64,
    check: None,
    cut: None,
};

pub static VTBL_8BIT_B64: Vtbl = Vtbl {
    from: NoEncoding::EightBit,
    to: NoEncoding::Base64,
    ctor: common_ctor,
    function: mbfl_filt_conv_base64enc,
    flush: mbfl_filt_conv_base64enc_flush,
};

pub static VTBL_B64_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Base64,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_base64dec,
    flush: mbfl_filt_conv_base64dec_flush,
};

pub fn mb_base64_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_base64_to_wchar")
}

pub fn mb_wchar_to_base64(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_base64")
}

pub fn mbfl_filt_conv_base64enc(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_base64enc")
}

pub fn mbfl_filt_conv_base64enc_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_base64enc_flush")
}

pub fn mbfl_filt_conv_base64dec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_base64dec")
}

pub fn mbfl_filt_conv_base64dec_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_base64dec_flush")
}
