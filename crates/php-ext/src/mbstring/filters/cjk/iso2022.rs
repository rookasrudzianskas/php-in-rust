//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_JIS: Encoding = Encoding {
    no: NoEncoding::Jis,
    name: "JIS",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_JIS_WCHAR),
    output_filter: Some(&VTBL_WCHAR_JIS),
    to_wchar: mb_iso2022jp_to_wchar,
    from_wchar: mb_wchar_to_jis,
    check: Some(mb_check_jis),
    cut: None,
};

pub static MBFL_ENCODING_2022JP: Encoding = Encoding {
    no: NoEncoding::Iso2022jp,
    name: "ISO-2022-JP",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JP_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JP),
    to_wchar: mb_iso2022jp_to_wchar,
    from_wchar: mb_wchar_to_iso2022jp,
    check: Some(mb_check_iso2022jp),
    cut: None,
};

pub static MBFL_ENCODING_2022JP_KDDI: Encoding = Encoding {
    no: NoEncoding::Iso2022jpKddi,
    name: "ISO-2022-JP-MOBILE#KDDI",
    mime_name: Some("ISO-2022-JP"),
    aliases: &["ISO-2022-JP-KDDI"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JP_KDDI_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JP_KDDI),
    to_wchar: mb_iso2022jp_kddi_to_wchar,
    from_wchar: mb_wchar_to_iso2022jp_kddi,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_2022JP_2004: Encoding = Encoding {
    no: NoEncoding::Iso2022jp2004,
    name: "ISO-2022-JP-2004",
    mime_name: Some("ISO-2022-JP-2004"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JP_2004_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JP_2004),
    to_wchar: mb_iso2022jp2004_to_wchar,
    from_wchar: mb_wchar_to_iso2022jp2004,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP50220: Encoding = Encoding {
    no: NoEncoding::Cp50220,
    name: "CP50220",
    mime_name: Some("ISO-2022-JP"),
    aliases: &["cp50220raw", "cp50220-raw", "JIS-ms"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_CP50220_WCHAR),
    output_filter: Some(&VTBL_WCHAR_CP50220),
    to_wchar: mb_cp5022x_to_wchar,
    from_wchar: mb_wchar_to_cp50220,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP50221: Encoding = Encoding {
    no: NoEncoding::Cp50221,
    name: "CP50221",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_CP50221_WCHAR),
    output_filter: Some(&VTBL_WCHAR_CP50221),
    to_wchar: mb_cp5022x_to_wchar,
    from_wchar: mb_wchar_to_cp50221,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP50222: Encoding = Encoding {
    no: NoEncoding::Cp50222,
    name: "CP50222",
    mime_name: Some("ISO-2022-JP"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_CP50222_WCHAR),
    output_filter: Some(&VTBL_WCHAR_CP50222),
    to_wchar: mb_cp5022x_to_wchar,
    from_wchar: mb_wchar_to_cp50222,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_2022JPMS: Encoding = Encoding {
    no: NoEncoding::Iso2022jpms,
    name: "ISO-2022-JP-MS",
    mime_name: Some("ISO-2022-JP"),
    aliases: &["ISO2022JPMS"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022JPMS_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022JPMS),
    to_wchar: mb_iso2022jpms_to_wchar,
    from_wchar: mb_wchar_to_iso2022jpms,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_2022KR: Encoding = Encoding {
    no: NoEncoding::Iso2022kr,
    name: "ISO-2022-KR",
    mime_name: Some("ISO-2022-KR"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_2022KR_WCHAR),
    output_filter: Some(&VTBL_WCHAR_2022KR),
    to_wchar: mb_iso2022kr_to_wchar,
    from_wchar: mb_wchar_to_iso2022kr,
    check: None,
    cut: None,
};

pub static VTBL_JIS_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Jis,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_jis_wchar,
    flush: mbfl_filt_conv_jis_wchar_flush,
};

pub static VTBL_WCHAR_JIS: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Jis,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_jis,
    flush: mbfl_filt_conv_any_jis_flush,
};

pub static VTBL_2022JP_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jp,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_jis_wchar,
    flush: mbfl_filt_conv_jis_wchar_flush,
};

pub static VTBL_WCHAR_2022JP: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jp,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022jp,
    flush: mbfl_filt_conv_any_jis_flush,
};

pub static VTBL_2022JP_KDDI_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jpKddi,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_2022jp_mobile_wchar,
    flush: mbfl_filt_conv_2022jp_mobile_wchar_flush,
};

pub static VTBL_WCHAR_2022JP_KDDI: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jpKddi,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022jp_mobile,
    flush: mbfl_filt_conv_wchar_2022jp_mobile_flush,
};

pub static VTBL_2022JP_2004_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jp2004,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_jis2004_wchar,
    flush: mbfl_filt_conv_jis2004_wchar_flush,
};

pub static VTBL_WCHAR_2022JP_2004: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jp2004,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_jis2004,
    flush: mbfl_filt_conv_wchar_jis2004_flush,
};

pub static VTBL_CP50220_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Cp50220,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_cp5022x_wchar,
    flush: mbfl_filt_conv_cp5022x_wchar_flush,
};

pub static VTBL_WCHAR_CP50220: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Cp50220,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_cp50220,
    flush: mbfl_filt_conv_wchar_cp50220_flush,
};

pub static VTBL_CP50221_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Cp50221,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_cp5022x_wchar,
    flush: mbfl_filt_conv_cp5022x_wchar_flush,
};

pub static VTBL_WCHAR_CP50221: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Cp50221,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_cp50221,
    flush: mbfl_filt_conv_any_jis_flush,
};

pub static VTBL_CP50222_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Cp50222,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_cp5022x_wchar,
    flush: mbfl_filt_conv_cp5022x_wchar_flush,
};

pub static VTBL_WCHAR_CP50222: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Cp50222,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_cp50222,
    flush: mbfl_filt_conv_wchar_cp50222_flush,
};

pub static VTBL_2022JPMS_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022jpms,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_2022jpms_wchar,
    flush: mbfl_filt_conv_2022jpms_wchar_flush,
};

pub static VTBL_WCHAR_2022JPMS: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022jpms,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022jpms,
    flush: mbfl_filt_conv_any_2022jpms_flush,
};

pub static VTBL_WCHAR_2022KR: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Iso2022kr,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_2022kr,
    flush: mbfl_filt_conv_any_2022kr_flush,
};

pub static VTBL_2022KR_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Iso2022kr,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_2022kr_wchar,
    flush: mbfl_filt_conv_2022kr_wchar_flush,
};

pub fn mb_iso2022jp_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_iso2022jp_to_wchar")
}

pub fn mb_wchar_to_jis(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_jis")
}

pub fn mb_check_jis(input: &[u8]) -> bool {
    todo!("mb_check_jis")
}

pub fn mb_wchar_to_iso2022jp(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_iso2022jp")
}

pub fn mb_check_iso2022jp(input: &[u8]) -> bool {
    todo!("mb_check_iso2022jp")
}

pub fn mb_iso2022jp_kddi_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_iso2022jp_kddi_to_wchar")
}

pub fn mb_wchar_to_iso2022jp_kddi(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_iso2022jp_kddi")
}

pub fn mb_iso2022jp2004_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_iso2022jp2004_to_wchar")
}

pub fn mb_wchar_to_iso2022jp2004(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_iso2022jp2004")
}

pub fn mb_cp5022x_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp5022x_to_wchar")
}

pub fn mb_wchar_to_cp50220(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp50220")
}

pub fn mb_wchar_to_cp50221(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp50221")
}

pub fn mb_wchar_to_cp50222(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp50222")
}

pub fn mb_iso2022jpms_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_iso2022jpms_to_wchar")
}

pub fn mb_wchar_to_iso2022jpms(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_iso2022jpms")
}

pub fn mb_iso2022kr_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_iso2022kr_to_wchar")
}

pub fn mb_wchar_to_iso2022kr(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_iso2022kr")
}

pub fn mbfl_filt_conv_jis_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_jis_wchar")
}

pub fn mbfl_filt_conv_jis_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_jis_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_jis(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_jis")
}

pub fn mbfl_filt_conv_any_jis_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_any_jis_flush")
}

pub fn mbfl_filt_conv_wchar_2022jp(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_2022jp")
}

pub fn mbfl_filt_conv_2022jp_mobile_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_2022jp_mobile_wchar")
}

pub fn mbfl_filt_conv_2022jp_mobile_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_2022jp_mobile_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_2022jp_mobile(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_2022jp_mobile")
}

pub fn mbfl_filt_conv_wchar_2022jp_mobile_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_2022jp_mobile_flush")
}

pub fn mbfl_filt_conv_jis2004_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_jis2004_wchar")
}

pub fn mbfl_filt_conv_jis2004_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_jis2004_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_jis2004(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_jis2004")
}

pub fn mbfl_filt_conv_wchar_jis2004_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_jis2004_flush")
}

pub fn mbfl_filt_conv_cp5022x_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_cp5022x_wchar")
}

pub fn mbfl_filt_conv_cp5022x_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_cp5022x_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_cp50220(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_cp50220")
}

pub fn mbfl_filt_conv_wchar_cp50220_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_cp50220_flush")
}

pub fn mbfl_filt_conv_wchar_cp50221(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_cp50221")
}

pub fn mbfl_filt_conv_wchar_cp50222(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_cp50222")
}

pub fn mbfl_filt_conv_wchar_cp50222_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_cp50222_flush")
}

pub fn mbfl_filt_conv_2022jpms_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_2022jpms_wchar")
}

pub fn mbfl_filt_conv_2022jpms_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_2022jpms_wchar_flush")
}

pub fn mbfl_filt_conv_wchar_2022jpms(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_2022jpms")
}

pub fn mbfl_filt_conv_any_2022jpms_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_any_2022jpms_flush")
}

pub fn mbfl_filt_conv_wchar_2022kr(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_wchar_2022kr")
}

pub fn mbfl_filt_conv_any_2022kr_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_any_2022kr_flush")
}

pub fn mbfl_filt_conv_2022kr_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_2022kr_wchar")
}

pub fn mbfl_filt_conv_2022kr_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    todo!("mbfl_filt_conv_2022kr_wchar_flush")
}
