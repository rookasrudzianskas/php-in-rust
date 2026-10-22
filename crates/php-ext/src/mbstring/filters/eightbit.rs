//! `libmbfl/mbfl/mbfilter_8bit.c`: "8bit" (binary).

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{ENCTYPE_SBCS, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{
    ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, common_flush, illegal_output,
};

pub static MBFL_ENCODING_8BIT: Encoding = Encoding {
    no: NoEncoding::EightBit,
    name: "8bit",
    mime_name: Some("8bit"),
    aliases: &["binary"],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: Some(&VTBL_8BIT_WCHAR),
    output_filter: Some(&VTBL_WCHAR_8BIT),
    to_wchar: mb_8bit_to_wchar,
    from_wchar: mb_wchar_to_8bit,
    check: None,
    cut: None,
};

pub static VTBL_8BIT_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::EightBit,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_8bit_wchar,
    flush: common_flush,
};

pub static VTBL_WCHAR_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_8bit,
    flush: common_flush,
};

fn mbfl_filt_conv_8bit_wchar(c: i32, _f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    out.output(c)
}

fn mbfl_filt_conv_wchar_8bit(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if (0..0x100).contains(&c) {
        ck!(out.output(c));
    } else {
        ck!(illegal_output(c, f, out));
    }
    0
}

pub fn mb_8bit_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let n = input.len().min(out.len());
    for (o, &c) in out.iter_mut().zip(&input[..n]) {
        *o = c as u32;
    }
    *input = &input[n..];
    n
}

pub fn mb_wchar_to_8bit(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w <= 0xFF {
            buf.add(w as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_8bit, buf);
        }
    }
}
