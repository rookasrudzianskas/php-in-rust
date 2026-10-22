//! `libmbfl/filters/mbfilter_7bit.c`: "7bit".

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{BAD_INPUT, ENCTYPE_SBCS, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{
    ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, common_flush, illegal_output,
};

pub static MBFL_ENCODING_7BIT: Encoding = Encoding {
    no: NoEncoding::SevenBit,
    name: "7bit",
    mime_name: Some("7bit"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: Some(&VTBL_7BIT_WCHAR),
    output_filter: Some(&VTBL_WCHAR_7BIT),
    to_wchar: mb_7bit_to_wchar,
    from_wchar: mb_wchar_to_7bit,
    check: None,
    cut: None,
};

pub static VTBL_7BIT_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::SevenBit,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_7bit_any,
    flush: common_flush,
};

pub static VTBL_WCHAR_7BIT: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::SevenBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_any_7bit,
    flush: common_flush,
};

fn mbfl_filt_conv_7bit_any(c: i32, _f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    out.output(if c < 0x80 { c } else { BAD_INPUT as i32 })
}

fn mbfl_filt_conv_any_7bit(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if (0..0x80).contains(&c) {
        ck!(out.output(c));
    } else {
        ck!(illegal_output(c, f, out));
    }
    0
}

pub fn mb_7bit_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let n = input.len().min(out.len());
    for (o, &c) in out.iter_mut().zip(&input[..n]) {
        *o = if c < 0x80 { c as u32 } else { BAD_INPUT };
    }
    *input = &input[n..];
    n
}

pub fn mb_wchar_to_7bit(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w <= 0x7F {
            buf.add(w as u8);
        } else {
            mb_illegal_output(w, mb_wchar_to_7bit, buf);
        }
    }
}
