//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_HTML_ENT: Encoding = Encoding {
    no: NoEncoding::HtmlEnt,
    name: "HTML-ENTITIES",
    mime_name: Some("HTML-ENTITIES"),
    aliases: &["HTML", "html"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_HTML_WCHAR),
    output_filter: Some(&VTBL_WCHAR_HTML),
    to_wchar: mb_htmlent_to_wchar,
    from_wchar: mb_wchar_to_htmlent,
    check: None,
    cut: None,
};

pub static VTBL_WCHAR_HTML: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::HtmlEnt,
    ctor: common_ctor,
    function: mbfl_filt_conv_html_enc,
    flush: mbfl_filt_conv_html_enc_flush,
};

pub static VTBL_HTML_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::HtmlEnt,
    to: NoEncoding::Wchar,
    ctor: mbfl_filt_conv_html_dec_ctor,
    function: mbfl_filt_conv_html_dec,
    flush: mbfl_filt_conv_html_dec_flush,
};

pub fn mb_htmlent_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_htmlent_to_wchar")
}

pub fn mb_wchar_to_htmlent(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_htmlent")
}

pub fn mbfl_filt_conv_html_enc(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_html_enc")
}

pub fn mbfl_filt_conv_html_enc_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_html_enc_flush")
}

pub fn mbfl_filt_conv_html_dec_ctor(f: &mut ConvertFilter) {
    todo!("mbfl_filt_conv_html_dec_ctor")
}

pub fn mbfl_filt_conv_html_dec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_html_dec")
}

pub fn mbfl_filt_conv_html_dec_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_html_dec_flush")
}
