//! `mb_output_handler` (`mbstring.c`): converts output from the internal
//! encoding to the HTTP output encoding.
//!
//! The CLI SAPI discards headers and the runtime does not record the
//! response's MIME type, so the handler behaves as if no `Content-Type`
//! header was set: the default content type applies and conversion is
//! enabled when the handler starts (`mbstring.http_output_conv_mimetypes`
//! is not consulted).

use php_runtime::{Args, PResult, PhpStr, Value, Vm};

use super::globals::globals;
use super::mbfl::convert::{ConvertBuf, WCHAR_BUFSIZE};
use super::mbfl::encoding::NoEncoding;

/// `PHP_OUTPUT_HANDLER_START` / `PHP_OUTPUT_HANDLER_END` (`PHP_OUTPUT_HANDLER_FINAL`).
const HANDLER_START: i64 = 0x01;
const HANDLER_END: i64 = 0x08;

pub fn mb_output_handler(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let status = args.int(1);
    let g = globals(vm)?;
    let encoding = g.current_http_output_encoding;
    if encoding.is(NoEncoding::Pass) {
        return Ok(Value::String(s));
    }
    if status & HANDLER_START != 0 {
        // No MIME type is known, so the default content type is used and
        // conversion is enabled.
        g.outconv_enabled = true;
    }
    if !g.outconv_enabled {
        return Ok(Value::String(s));
    }

    let internal = g.current_internal_encoding;
    let mut buf = ConvertBuf::new(
        s.len(),
        g.current_filter_illegal_substchar,
        g.current_filter_illegal_mode,
    );
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut inp: &[u8] = &s;
    let last_feed = status & HANDLER_END != 0;
    let mut state = g.outconv_state;
    while !inp.is_empty() {
        let n = (internal.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        (encoding.from_wchar)(&wchar_buf[..n], &mut buf, inp.is_empty() && last_feed);
    }
    g.outconv_state = state;
    g.illegalchars += buf.errors as i64;
    if last_feed {
        g.outconv_enabled = false;
        g.outconv_state = 0;
    }
    Ok(Value::String(PhpStr::from_vec(buf.into_bytes())))
}
