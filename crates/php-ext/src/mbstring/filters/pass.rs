//! `libmbfl/mbfl/mbfilter_pass.c`: the "pass" pseudo-encoding (no conversion).

use crate::mbstring::mbfl::convert::ConvertBuf;
use crate::mbstring::mbfl::encoding::{Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, common_ctor, common_flush};

/// php-src has no conversion functions for "pass"; callers check for it
/// before converting. These copy bytes unchanged in case one slips through.
pub static MBFL_ENCODING_PASS: Encoding = Encoding {
    no: NoEncoding::Pass,
    name: "pass",
    mime_name: None,
    aliases: &["none"],
    mblen_table: None,
    flag: 0,
    input_filter: None,
    output_filter: None,
    to_wchar: pass_to_wchar,
    from_wchar: wchar_to_pass,
    check: None,
    cut: None,
};

pub static VTBL_PASS: Vtbl = Vtbl {
    from: NoEncoding::Pass,
    to: NoEncoding::Pass,
    ctor: common_ctor,
    function: mbfl_filt_conv_pass,
    flush: common_flush,
};

/// `mbfl_filt_conv_pass`.
pub fn mbfl_filt_conv_pass(c: i32, _f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    out.output(c)
}

pub fn pass_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let n = input.len().min(out.len());
    for (o, &c) in out.iter_mut().zip(&input[..n]) {
        *o = c as u32;
    }
    *input = &input[n..];
    n
}

pub fn wchar_to_pass(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    buf.out.extend(input.iter().map(|&w| w as u8));
}
