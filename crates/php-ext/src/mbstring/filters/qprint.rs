//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_QPRINT: Encoding = Encoding {
    no: NoEncoding::Qprint,
    name: "Quoted-Printable",
    mime_name: Some("Quoted-Printable"),
    aliases: &["qprint"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_qprint_to_wchar,
    from_wchar: mb_wchar_to_qprint,
    check: None,
    cut: None,
};

pub static VTBL_8BIT_QPRINT: Vtbl = Vtbl {
    from: NoEncoding::EightBit,
    to: NoEncoding::Qprint,
    ctor: common_ctor,
    function: mbfl_filt_conv_qprintenc,
    flush: mbfl_filt_conv_qprintenc_flush,
};

pub static VTBL_QPRINT_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Qprint,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_qprintdec,
    flush: mbfl_filt_conv_qprintdec_flush,
};

pub fn mb_qprint_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_qprint_to_wchar")
}

pub fn mb_wchar_to_qprint(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_qprint")
}

pub fn mbfl_filt_conv_qprintenc(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_qprintenc")
}

pub fn mbfl_filt_conv_qprintenc_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_qprintenc_flush")
}

pub fn mbfl_filt_conv_qprintdec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_qprintdec")
}

pub fn mbfl_filt_conv_qprintdec_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_qprintdec_flush")
}
