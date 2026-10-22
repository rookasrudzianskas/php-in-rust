//! The legacy byte-at-a-time conversion filters (`mbfl_convert_filter`,
//! `libmbfl/mbfl/mbfl_convert.c`). php-src keeps them only for the encodings
//! `mbfl_strcut` has to walk character by character (ISO-2022 variants, HZ,
//! UTF-7, UTF7-IMAP, HTML-ENTITIES, Base64, Quoted-Printable).
//!
//! A filter's `function` receives one input unit (a byte or a codepoint) and
//! emits output units through [`FilterOutput`]; `flush` emits pending state
//! and then flushes the downstream sink (php-src's `flush_function`).

use super::encoding::{BAD_INPUT, Encoding, ILLEGAL_MODE_CHAR, ILLEGAL_MODE_ENTITY, ILLEGAL_MODE_LONG, ILLEGAL_MODE_NONE, NoEncoding, enc};

/// Downstream of a filter: `output_function` and `flush_function`.
pub trait FilterOutput {
    /// `(*filter->output_function)(c, filter->data)`; negative on failure.
    fn output(&mut self, c: i32) -> i32;
    /// `(*filter->flush_function)(filter->data)` (no-op when php-src's is `NULL`).
    fn flush(&mut self) -> i32;
}

/// `filter_function`.
pub type FilterFn = fn(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32;
/// `filter_flush`.
pub type FilterFlushFn = fn(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32;
/// `filter_ctor`.
pub type FilterCtorFn = fn(f: &mut ConvertFilter);

/// `struct mbfl_convert_vtbl`. Destructors and copy functions are not needed
/// (state is owned and `Clone`).
pub struct Vtbl {
    pub from: NoEncoding,
    pub to: NoEncoding,
    pub ctor: FilterCtorFn,
    pub function: FilterFn,
    pub flush: FilterFlushFn,
}

/// `mbfl_convert_filter`.
#[derive(Clone)]
pub struct ConvertFilter {
    pub status: i32,
    pub cache: i32,
    pub from: &'static Encoding,
    pub to: &'static Encoding,
    pub illegal_mode: u32,
    pub illegal_substchar: u32,
    pub num_illegalchar: usize,
    /// Extra state (`opaque`), e.g. the HTML entity decoder's buffer.
    pub opaque: Vec<i32>,
    pub function: FilterFn,
    pub flush_fn: FilterFlushFn,
}

impl ConvertFilter {
    /// `mbfl_convert_filter_init`.
    pub fn new(from: &'static Encoding, to: &'static Encoding, vtbl: &'static Vtbl) -> ConvertFilter {
        let mut f = ConvertFilter {
            status: 0,
            cache: 0,
            from,
            to,
            illegal_mode: ILLEGAL_MODE_CHAR,
            illegal_substchar: b'?' as u32,
            num_illegalchar: 0,
            opaque: Vec::new(),
            function: vtbl.function,
            flush_fn: vtbl.flush,
        };
        (vtbl.ctor)(&mut f);
        f
    }

    /// `(*filter->filter_function)(c, filter)`.
    pub fn feed(&mut self, c: i32, out: &mut dyn FilterOutput) -> i32 {
        (self.function)(c, self, out)
    }

    /// `(*filter->filter_flush)(filter)`.
    pub fn flush(&mut self, out: &mut dyn FilterOutput) -> i32 {
        (self.flush_fn)(self, out)
    }
}

/// `mbfl_filt_conv_common_ctor`.
pub fn common_ctor(f: &mut ConvertFilter) {
    f.status = 0;
    f.cache = 0;
}

/// `mbfl_filt_conv_common_flush`.
pub fn common_flush(_f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    out.flush();
    0
}

/// `CK(statement)`: returns -1 from the enclosing filter function if the
/// statement fails.
macro_rules! ck {
    ($e:expr) => {
        if $e < 0 {
            return -1;
        }
    };
}
pub(crate) use ck;

/// `mbfl_convert_filter_strcat`: feeds a byte string through the filter itself.
fn strcat(f: &mut ConvertFilter, s: &[u8], out: &mut dyn FilterOutput) -> i32 {
    for &c in s {
        if f.feed(c as i32, out) < 0 {
            return -1;
        }
    }
    0
}

const HEXCHARS: &[u8; 16] = b"0123456789ABCDEF";

/// `mbfl_filt_conv_output_hex`.
fn output_hex(w: u32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut nonzero = false;
    let mut shift: i32 = 28;
    let mut ret = 0;
    while shift >= 0 {
        let n = (w >> shift) & 0xF;
        if n != 0 || nonzero {
            nonzero = true;
            ret = f.feed(HEXCHARS[n as usize] as i32, out);
            if ret < 0 {
                return ret;
            }
        }
        shift -= 4;
    }
    if !nonzero {
        ret = f.feed(b'0' as i32, out);
    }
    ret
}

/// `mbfl_filt_conv_illegal_output`: emits the error marker for `c` by
/// re-feeding it through the filter's own function.
pub fn illegal_output(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let w = c as u32;
    let mut ret = 0;
    let mode_backup = f.illegal_mode;
    let substchar_backup = f.illegal_substchar;

    // If the substitution character is not encodable, retry with '?', and if
    // that fails as well, drop the character.
    if f.illegal_mode == ILLEGAL_MODE_CHAR && f.illegal_substchar != b'?' as u32 {
        f.illegal_substchar = b'?' as u32;
    } else {
        f.illegal_mode = ILLEGAL_MODE_NONE;
    }

    match mode_backup {
        ILLEGAL_MODE_CHAR => ret = f.feed(substchar_backup as i32, out),
        ILLEGAL_MODE_LONG => {
            if w != BAD_INPUT {
                ret = strcat(f, b"U+", out);
                if ret >= 0 {
                    ret = output_hex(w, f, out);
                }
            } else {
                ret = f.feed(substchar_backup as i32, out);
            }
        }
        ILLEGAL_MODE_ENTITY => {
            if w != BAD_INPUT {
                ret = strcat(f, b"&#x", out);
                if ret >= 0 {
                    ret = output_hex(w, f, out);
                }
                if ret >= 0 {
                    ret = strcat(f, b";", out);
                }
            } else {
                ret = f.feed(substchar_backup as i32, out);
            }
        }
        _ => {}
    }

    f.illegal_mode = mode_backup;
    f.illegal_substchar = substchar_backup;
    f.num_illegalchar += 1;
    ret
}

/// `mbfl_convert_filter_get_vtbl`.
pub fn get_vtbl(from: &'static Encoding, to: &'static Encoding) -> Option<&'static Vtbl> {
    use crate::mbstring::filters::{base64, pass, qprint, uuencode};
    let (mut from, mut to) = (from, to);
    if to.is(NoEncoding::Base64) || to.is(NoEncoding::Qprint) {
        from = enc(NoEncoding::EightBit);
    } else if from.is(NoEncoding::Base64) || from.is(NoEncoding::Qprint) || from.is(NoEncoding::Uuencode) {
        to = enc(NoEncoding::EightBit);
    }

    if to.no == from.no && (to.is(NoEncoding::Wchar) || to.is(NoEncoding::EightBit)) {
        return Some(&pass::VTBL_PASS);
    }

    if to.is(NoEncoding::Wchar) {
        from.input_filter
    } else if from.is(NoEncoding::Wchar) {
        to.output_filter
    } else {
        let special: [&'static Vtbl; 6] = [
            &base64::VTBL_8BIT_B64,
            &base64::VTBL_B64_8BIT,
            &uuencode::VTBL_UUENCODE_8BIT,
            &qprint::VTBL_8BIT_QPRINT,
            &qprint::VTBL_QPRINT_8BIT,
            &pass::VTBL_PASS,
        ];
        special
            .into_iter()
            .find(|v| v.from == from.no && v.to == to.no)
    }
}

/// `mbfl_convert_filter_new`.
pub fn convert_filter_new(from: &'static Encoding, to: &'static Encoding) -> Option<ConvertFilter> {
    get_vtbl(from, to).map(|v| ConvertFilter::new(from, to, v))
}
