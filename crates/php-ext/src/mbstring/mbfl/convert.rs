//! `mb_convert_buf` (`mbfl_encoding.h`) and the fast conversion entry points
//! of `libmbfl/mbfl/mbfl_convert.c` (`mb_fast_convert`, `mb_illegal_output`).

use super::encoding::{
    BAD_INPUT, Encoding, FromWcharFn, ILLEGAL_MODE_BADUTF8, ILLEGAL_MODE_CHAR, ILLEGAL_MODE_ENTITY,
    ILLEGAL_MODE_LONG, ILLEGAL_MODE_NONE, NoEncoding, enc,
};

/// `mb_convert_buf`: the output buffer of `from_wchar` functions.
///
/// php-src writes through `out`/`limit` pointers and grows the buffer with
/// `MB_CONVERT_BUF_ENSURE`; here the buffer is a `Vec` that grows on push.
#[derive(Clone, Debug, Default)]
pub struct ConvertBuf {
    pub out: Vec<u8>,
    /// Encoder state kept between `from_wchar` calls.
    pub state: u32,
    /// Number of codepoints which could not be encoded (or were bad input).
    pub errors: u32,
    pub replacement_char: u32,
    pub error_mode: u32,
}

impl ConvertBuf {
    /// `mb_convert_buf_init`.
    pub fn new(initsize: usize, replacement_char: u32, error_mode: u32) -> ConvertBuf {
        ConvertBuf {
            out: Vec::with_capacity(initsize),
            state: 0,
            errors: 0,
            replacement_char,
            error_mode,
        }
    }

    /// `mb_convert_buf_len`.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// `mb_convert_buf_reset`.
    pub fn reset(&mut self, len: usize) {
        self.out.truncate(len);
    }

    /// `mb_convert_buf_add`.
    #[inline]
    pub fn add(&mut self, c: u8) {
        self.out.push(c);
    }

    #[inline]
    pub fn add2(&mut self, c1: u8, c2: u8) {
        self.out.extend_from_slice(&[c1, c2]);
    }

    #[inline]
    pub fn add3(&mut self, c1: u8, c2: u8, c3: u8) {
        self.out.extend_from_slice(&[c1, c2, c3]);
    }

    #[inline]
    pub fn add4(&mut self, c1: u8, c2: u8, c3: u8, c4: u8) {
        self.out.extend_from_slice(&[c1, c2, c3, c4]);
    }

    /// `mb_convert_buf_appends` / `mb_convert_buf_appendn`.
    #[inline]
    pub fn append(&mut self, s: &[u8]) {
        self.out.extend_from_slice(s);
    }

    /// `mb_convert_buf_result` / `mb_convert_buf_result_raw`.
    pub fn into_bytes(self) -> Vec<u8> {
        self.out
    }
}

const HEXCHARS: &[u8; 16] = b"0123456789ABCDEF";

/// `convert_cp_to_hex`.
fn convert_cp_to_hex(cp: u32, out: &mut Vec<u32>) {
    let mut nonzero = false;
    let mut shift: i32 = 28;
    while shift >= 0 {
        let n = (cp >> shift) & 0xF;
        if n != 0 || nonzero {
            nonzero = true;
            out.push(HEXCHARS[n as usize] as u32);
        }
        shift -= 4;
    }
    if !nonzero {
        out.push(b'0' as u32);
    }
}

/// `mb_illegal_marker`.
fn illegal_marker(bad_cp: u32, out: &mut Vec<u32>, err_mode: u32, replacement_char: u32) {
    if bad_cp == BAD_INPUT {
        // Invalid byte sequence in the 'from' encoding.
        if err_mode != ILLEGAL_MODE_NONE {
            out.push(replacement_char);
        }
    } else {
        // Valid codepoint which cannot be represented in the 'to' encoding.
        match err_mode {
            ILLEGAL_MODE_CHAR => out.push(replacement_char),
            ILLEGAL_MODE_LONG => {
                out.push(b'U' as u32);
                out.push(b'+' as u32);
                convert_cp_to_hex(bad_cp, out);
            }
            ILLEGAL_MODE_ENTITY => {
                out.extend([b'&' as u32, b'#' as u32, b'x' as u32]);
                convert_cp_to_hex(bad_cp, out);
                out.push(b';' as u32);
            }
            _ => {}
        }
    }
}

/// `mb_illegal_output` (`MB_CONVERT_ERROR`): counts the error and emits the
/// error marker through `f` (the calling encoder).
pub fn mb_illegal_output(bad_cp: u32, f: FromWcharFn, buf: &mut ConvertBuf) {
    buf.errors += 1;

    let repl_char = buf.replacement_char;
    let err_mode = buf.error_mode;

    if err_mode == ILLEGAL_MODE_BADUTF8 {
        buf.add(0xFF);
        return;
    }

    let mut temp: Vec<u32> = Vec::with_capacity(12);
    illegal_marker(bad_cp, &mut temp, err_mode, repl_char);

    // Avoid an infinite loop if `f` cannot encode `repl_char`.
    if err_mode == ILLEGAL_MODE_CHAR && repl_char != b'?' as u32 {
        buf.replacement_char = b'?' as u32;
    } else {
        buf.error_mode = ILLEGAL_MODE_NONE;
    }

    f(&temp, buf, false);

    buf.replacement_char = repl_char;
    buf.error_mode = err_mode;
}

/// Size of the codepoint buffer php-src uses for most conversions.
pub const WCHAR_BUFSIZE: usize = 128;

/// `mb_fast_convert`: returns the converted bytes and the error count.
pub fn mb_fast_convert(
    input: &[u8],
    from: &'static Encoding,
    to: &'static Encoding,
    replacement_char: u32,
    error_mode: u32,
) -> (Vec<u8>, u32) {
    let (mut from, mut to) = (from, to);
    if to.is(NoEncoding::Base64) || to.is(NoEncoding::Qprint) {
        from = enc(NoEncoding::EightBit);
    } else if from.is(NoEncoding::Base64)
        || from.is(NoEncoding::Qprint)
        || from.is(NoEncoding::Uuencode)
    {
        to = enc(NoEncoding::EightBit);
    }

    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut buf = ConvertBuf::new(input.len(), replacement_char, error_mode);
    let mut inp = input;
    while !inp.is_empty() {
        let out_len = (from.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        debug_assert!(out_len <= WCHAR_BUFSIZE);
        (to.from_wchar)(&wchar_buf[..out_len], &mut buf, inp.is_empty());
    }
    let errors = buf.errors;
    (buf.into_bytes(), errors)
}

/// Decodes the whole of `input` to codepoints in chunks of `bufsize`
/// (a helper for tests and simple callers).
pub fn decode_all(input: &[u8], from: &'static Encoding) -> Vec<u32> {
    let mut out = Vec::with_capacity(input.len());
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut inp = input;
    while !inp.is_empty() {
        let n = (from.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        out.extend_from_slice(&wchar_buf[..n]);
    }
    out
}

/// Encodes `input` codepoints in one call with `end = true`.
pub fn encode_all(input: &[u32], to: &'static Encoding, replacement_char: u32, error_mode: u32) -> ConvertBuf {
    let mut buf = ConvertBuf::new(input.len(), replacement_char, error_mode);
    (to.from_wchar)(input, &mut buf, true);
    buf
}
