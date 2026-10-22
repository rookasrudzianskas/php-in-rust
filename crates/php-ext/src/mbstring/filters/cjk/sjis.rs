//! STUB: to be ported from php-src.
#![allow(unused_variables, dead_code, unused_imports)]

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::*;
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush, illegal_output};
use crate::mbstring::filters::mblen;

pub static MBFL_ENCODING_SJIS: Encoding = Encoding {
    no: NoEncoding::Sjis,
    name: "SJIS",
    mime_name: Some("Shift_JIS"),
    aliases: &["x-sjis", "SHIFT-JIS"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_to_wchar,
    from_wchar: mb_wchar_to_sjis,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_MAC: Encoding = Encoding {
    no: NoEncoding::SjisMac,
    name: "SJIS-mac",
    mime_name: Some("Shift_JIS"),
    aliases: &["MacJapanese", "x-Mac-Japanese"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJISMAC),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjismac_to_wchar,
    from_wchar: mb_wchar_to_sjismac,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_DOCOMO: Encoding = Encoding {
    no: NoEncoding::SjisDocomo,
    name: "SJIS-Mobile#DOCOMO",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-DOCOMO", "shift_jis-imode", "x-sjis-emoji-docomo"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_docomo_to_wchar,
    from_wchar: mb_wchar_to_sjis_docomo,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_KDDI: Encoding = Encoding {
    no: NoEncoding::SjisKddi,
    name: "SJIS-Mobile#KDDI",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-KDDI", "shift_jis-kddi", "x-sjis-emoji-kddi"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_kddi_to_wchar,
    from_wchar: mb_wchar_to_sjis_kddi,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS_SB: Encoding = Encoding {
    no: NoEncoding::SjisSb,
    name: "SJIS-Mobile#SOFTBANK",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-SOFTBANK", "shift_jis-softbank", "x-sjis-emoji-softbank"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis_sb_to_wchar,
    from_wchar: mb_wchar_to_sjis_sb,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJIS2004: Encoding = Encoding {
    no: NoEncoding::Sjis2004,
    name: "SJIS-2004",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS2004", "Shift_JIS-2004"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJIS_MOBILE),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_sjis2004_to_wchar,
    from_wchar: mb_wchar_to_sjis2004,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_CP932: Encoding = Encoding {
    no: NoEncoding::Cp932,
    name: "CP932",
    mime_name: Some("Shift_JIS"),
    aliases: &["MS932", "Windows-31J", "MS_Kanji"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJISWIN),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp932_to_wchar,
    from_wchar: mb_wchar_to_cp932,
    check: None,
    cut: None,
};

pub static MBFL_ENCODING_SJISWIN: Encoding = Encoding {
    no: NoEncoding::Sjiswin,
    name: "SJIS-win",
    mime_name: Some("Shift_JIS"),
    aliases: &["SJIS-ms", "SJIS-open"],
    mblen_table: Some(&mblen::MBLEN_TABLE_SJISWIN),
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_cp932_to_wchar,
    from_wchar: mb_wchar_to_sjiswin,
    check: None,
    cut: None,
};

pub fn mb_sjis_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_sjis_to_wchar")
}

pub fn mb_wchar_to_sjis(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjis")
}

pub fn mb_sjismac_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_sjismac_to_wchar")
}

pub fn mb_wchar_to_sjismac(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjismac")
}

pub fn mb_sjis_docomo_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_sjis_docomo_to_wchar")
}

pub fn mb_wchar_to_sjis_docomo(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjis_docomo")
}

pub fn mb_sjis_kddi_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_sjis_kddi_to_wchar")
}

pub fn mb_wchar_to_sjis_kddi(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjis_kddi")
}

pub fn mb_sjis_sb_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_sjis_sb_to_wchar")
}

pub fn mb_wchar_to_sjis_sb(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjis_sb")
}

pub fn mb_sjis2004_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_sjis2004_to_wchar")
}

pub fn mb_wchar_to_sjis2004(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjis2004")
}

pub fn mb_cp932_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    todo!("mb_cp932_to_wchar")
}

pub fn mb_wchar_to_cp932(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_cp932")
}

pub fn mb_wchar_to_sjiswin(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    todo!("mb_wchar_to_sjiswin")
}
