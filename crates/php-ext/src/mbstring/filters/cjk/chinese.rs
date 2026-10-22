//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_EUC_CN: Encoding = Encoding {
    no: NoEncoding::EucCn,
    name: "EUC-CN",
    mime_name: Some("CN-GB"),
    aliases: &["CN-GB", "EUC_CN", "eucCN", "x-euc-cn", "gb2312"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCCN),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_euccn_to_wchar,
    from_wchar: mb_wchar_to_euccn,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_EUC_TW: Encoding = Encoding {
    no: NoEncoding::EucTw,
    name: "EUC-TW",
    mime_name: Some("EUC-TW"),
    aliases: &["EUC_TW", "eucTW", "x-euc-tw"],
    mblen_table: Some(&mblen::MBLEN_TABLE_EUCCN),
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_euctw_to_wchar,
    from_wchar: mb_wchar_to_euctw,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_GB18030: Encoding = Encoding {
    no: NoEncoding::Gb18030,
    name: "GB18030",
    mime_name: Some("GB18030"),
    aliases: &["gb-18030", "gb-18030-2000"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_gb18030_to_wchar,
    from_wchar: mb_wchar_to_gb18030,
    check: None,
    cut: Some(mb_cut_gb18030),
};

pub static MBFL_ENCODING_CP936: Encoding = Encoding {
    no: NoEncoding::Cp936,
    name: "CP936",
    mime_name: Some("CP936"),
    aliases: &["CP-936", "GBK"],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp936_to_wchar,
    from_wchar: mb_wchar_to_cp936,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_GB18030_2022: Encoding = Encoding {
    no: NoEncoding::Gb18030_2022,
    name: "GB18030-2022",
    mime_name: Some("GB18030-2022"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_gb18030_2022_to_wchar,
    from_wchar: mb_wchar_to_gb18030_2022,
    check: None,
    cut: Some(mb_cut_gb18030),
};

pub static MBFL_ENCODING_BIG5: Encoding = Encoding {
    no: NoEncoding::Big5,
    name: "BIG-5",
    mime_name: Some("BIG5"),
    aliases: &["CN-BIG5", "BIG-FIVE", "BIGFIVE"],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_big5_to_wchar,
    from_wchar: mb_wchar_to_big5,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP950: Encoding = Encoding {
    no: NoEncoding::Cp950,
    name: "CP950",
    mime_name: Some("BIG5"),
    aliases: &[],
    mblen_table: Some(&mblen::MBLEN_TABLE_81_TO_FE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp950_to_wchar,
    from_wchar: mb_wchar_to_cp950,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_HZ: Encoding = Encoding {
    no: NoEncoding::Hz,
    name: "HZ",
    mime_name: Some("HZ-GB-2312"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_HZ_WCHAR),
    output_filter: Some(&VTBL_WCHAR_HZ),
    to_wchar: mb_hz_to_wchar,
    from_wchar: mb_wchar_to_hz,
    check: None,
    cut: None,
};

pub static VTBL_HZ_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Hz,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_hz_wchar,
    flush: mbfl_filt_conv_hz_wchar_flush,
};

pub static VTBL_WCHAR_HZ: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Hz,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_hz,
    flush: mbfl_filt_conv_any_hz_flush,
};

pub fn mb_euccn_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_euccn_to_wchar")
}

pub fn mb_wchar_to_euccn(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_euccn")
}

pub fn mb_euctw_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_euctw_to_wchar")
}

pub fn mb_wchar_to_euctw(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_euctw")
}

pub fn mb_gb18030_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_gb18030_to_wchar")
}

pub fn mb_wchar_to_gb18030(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_gb18030")
}

pub fn mb_cut_gb18030(s: &[u8], from: usize, len: usize) -> Vec<u8> {
    todo!("mb_cut_gb18030")
}

pub fn mb_cp936_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp936_to_wchar")
}

pub fn mb_wchar_to_cp936(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp936")
}

pub fn mb_gb18030_2022_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_gb18030_2022_to_wchar")
}

pub fn mb_wchar_to_gb18030_2022(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_gb18030_2022")
}

pub fn mb_big5_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_big5_to_wchar")
}

pub fn mb_wchar_to_big5(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_big5")
}

pub fn mb_cp950_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp950_to_wchar")
}

pub fn mb_wchar_to_cp950(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp950")
}

pub fn mb_hz_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_hz_to_wchar")
}

pub fn mb_wchar_to_hz(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_hz")
}

pub fn mbfl_filt_conv_hz_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_hz_wchar")
}

pub fn mbfl_filt_conv_hz_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_hz_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_hz(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_hz")
}

pub fn mbfl_filt_conv_any_hz_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_any_hz_flush")
}
