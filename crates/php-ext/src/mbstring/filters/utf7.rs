//! `libmbfl/filters/mbfilter_utf7.c` (and `utf7_helper.h`): UTF-7
//! (RFC 2152). Characters outside the "direct" set are written as UTF-16
//! encoded in Base64 between `+` and an optional `-`.
//!
//! Besides the fast converters and the validity check, the legacy filters
//! are kept because `mbfl_strcut` walks UTF-7 strings with them.

use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{
    BAD_INPUT, ENCTYPE_GL_UNSAFE, Encoding, NoEncoding, WCSPLANE_SUPMIN, WCSPLANE_UCS2MAX,
    WCSPLANE_UTF32MAX,
};
use crate::mbstring::mbfl::filter::{
    ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, illegal_output,
};

pub static MBFL_ENCODING_UTF7: Encoding = Encoding {
    no: NoEncoding::Utf7,
    name: "UTF-7",
    mime_name: Some("UTF-7"),
    aliases: &["utf7"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_UTF7_WCHAR),
    output_filter: Some(&VTBL_WCHAR_UTF7),
    to_wchar: mb_utf7_to_wchar,
    from_wchar: mb_wchar_to_utf7,
    check: Some(mb_check_utf7),
    cut: None,
};

pub static VTBL_UTF7_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Utf7,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_utf7_wchar,
    flush: mbfl_filt_conv_utf7_wchar_flush,
};

pub static VTBL_WCHAR_UTF7: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Utf7,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_utf7,
    flush: mbfl_filt_conv_wchar_utf7_flush,
};

/// `mbfl_base64_table` (without the trailing NUL).
const MBFL_BASE64_TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Ways in which a Base64-encoded section can end (`utf7_helper.h`).
pub(crate) const DASH: u8 = 0xFC;
pub(crate) const DIRECT: u8 = 0xFD;
pub(crate) const ASCII: u8 = 0xFE;
pub(crate) const ILLEGAL: u8 = 0xFF;

/// `is_base64_end_valid` (`utf7_helper.h`).
#[inline]
pub(crate) fn is_base64_end_valid(n: u8, gap: bool, is_surrogate: bool) -> bool {
    !(gap || is_surrogate || n == ASCII || n == ILLEGAL)
}

/// `has_surrogate` (`utf7_helper.h`).
#[inline]
pub(crate) fn has_surrogate(cp: u16, is_surrogate: bool) -> bool {
    !is_surrogate && (0xD800..=0xDBFF).contains(&cp)
}

/// `decode_base64_char`: -1 for bytes outside the Base64 alphabet.
fn decode_base64_char(c: u8) -> i32 {
    match c {
        b'A'..=b'Z' => c as i32 - 65,
        b'a'..=b'z' => c as i32 - 71,
        b'0'..=b'9' => c as i32 + 4,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    }
}

/// The common tail of states 4, 7 and 9 of `mbfl_filt_conv_utf7_wchar`:
/// `s` is a complete UTF-16 unit and `n` the bits already received for the
/// next one.
fn utf7_wchar_emit(mut s: i32, n: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if (0xD800..0xDC00).contains(&s) {
        // 1st part of surrogate pair
        if f.cache & 0xFFF_0000 != 0 {
            // We were waiting for the 2nd part of a surrogate pair
            out.output(BAD_INPUT as i32);
        }
        s = (((s & 0x3FF) << 16) + 0x40_0000) | n;
        f.cache = s;
    } else if (0xDC00..0xE000).contains(&s) {
        // 2nd part of surrogate pair
        if f.cache & 0xFFF_0000 != 0 {
            s &= 0x3FF;
            s |= (f.cache & 0xFFF_0000) >> 6;
            f.cache = n;
            ck!(out.output(s));
        } else {
            ck!(out.output(BAD_INPUT as i32));
            f.cache = n;
        }
    } else {
        if f.cache & 0xFFF_0000 != 0 {
            // We were waiting for the 2nd part of a surrogate pair
            out.output(BAD_INPUT as i32);
        }
        f.cache = n;
        ck!(out.output(s));
    }
    0
}

pub fn mbfl_filt_conv_utf7_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut n = -1;

    if f.status != 0 {
        // Modified Base64
        n = decode_base64_char(c as u8);
        if n < 0 {
            if f.cache != 0 {
                // Either we were expecting the 2nd half of a surrogate pair which
                // never came, or else the last Base64 data was not padded with zeroes
                out.output(BAD_INPUT as i32);
            }
            if c == b'-' as i32 {
                if f.status == 1 {
                    // "+-" -> "+"
                    ck!(out.output(b'+' as i32));
                }
            } else if (0..0x80).contains(&c) {
                // ASCII exclude '-'
                ck!(out.output(c));
            } else {
                ck!(out.output(BAD_INPUT as i32));
            }
            f.cache = 0;
            f.status = 0;
            return 0;
        }
    }

    match f.status {
        // directly encoded characters
        0 => {
            if c == b'+' as i32 {
                f.status = 1;
            } else if (0..0x80).contains(&c) {
                ck!(out.output(c));
            } else {
                ck!(out.output(BAD_INPUT as i32));
            }
        }
        // decode Modified Base64
        1 | 2 => {
            f.cache |= n << 10;
            f.status = 3;
        }
        3 => {
            f.cache |= n << 4;
            f.status = 4;
        }
        4 => {
            let s = ((n >> 2) & 0xF) | (f.cache & 0xFFFF);
            n = (n & 0x3) << 14;
            f.status = 5;
            return utf7_wchar_emit(s, n, f, out);
        }
        5 => {
            f.cache |= n << 8;
            f.status = 6;
        }
        6 => {
            f.cache |= n << 2;
            f.status = 7;
        }
        7 => {
            let s = ((n >> 4) & 0x3) | (f.cache & 0xFFFF);
            n = (n & 0xF) << 12;
            f.status = 8;
            return utf7_wchar_emit(s, n, f, out);
        }
        8 => {
            f.cache |= n << 6;
            f.status = 9;
        }
        9 => {
            let s = n | (f.cache & 0xFFFF);
            f.status = 2;
            return utf7_wchar_emit(s, 0, f, out);
        }
        _ => unreachable!("invalid UTF-7 filter state"),
    }

    0
}

pub fn mbfl_filt_conv_utf7_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.cache != 0 {
        // Either we were expecting the 2nd half of a surrogate pair which
        // never came, or else the last Base64 data was not padded with zeroes
        f.cache = 0;
        out.output(BAD_INPUT as i32);
    }
    out.flush();
    0
}

#[inline]
fn b64(i: i32) -> i32 {
    MBFL_BASE64_TABLE[i as usize] as i32
}

pub fn mbfl_filt_conv_wchar_utf7(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut n = 0;
    if (0..0x80).contains(&c) {
        let b = c as u8;
        if b.is_ascii_alphanumeric() || b == b'\0' || b == b'/' || b == b'-' {
            n = 1;
        } else if matches!(
            b,
            b' ' | b'\t' | b'\r' | b'\n' | b'\'' | b'(' | b')' | b',' | b'.' | b':' | b'?'
        ) {
            n = 2;
        }
    } else if c >= 0 && c < WCSPLANE_UCS2MAX as i32 {
        // encoded as one UTF-16 unit
    } else if c >= WCSPLANE_SUPMIN as i32 && c < WCSPLANE_UTF32MAX as i32 {
        ck!(mbfl_filt_conv_wchar_utf7(
            ((c >> 10) - 0x40) | 0xD800,
            f,
            out
        ));
        ck!(mbfl_filt_conv_wchar_utf7((c & 0x3FF) | 0xDC00, f, out));
        return 0;
    } else {
        ck!(illegal_output(c, f, out));
        return 0;
    }

    match f.status {
        0 => {
            if n != 0 {
                // directly encode characters
                ck!(out.output(c));
            } else {
                // Modified Base64
                ck!(out.output(b'+' as i32));
                f.status = 1;
                f.cache = c;
            }
        }
        // encode Modified Base64
        1 => {
            let s = f.cache;
            ck!(out.output(b64((s >> 10) & 0x3F)));
            ck!(out.output(b64((s >> 4) & 0x3F)));
            if n != 0 {
                ck!(out.output(b64((s << 2) & 0x3C)));
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
                ck!(out.output(c));
                f.status = 0;
            } else {
                f.status = 2;
                f.cache = ((s & 0xF) << 16) | c;
            }
        }
        2 => {
            let s = f.cache;
            ck!(out.output(b64((s >> 14) & 0x3F)));
            ck!(out.output(b64((s >> 8) & 0x3F)));
            ck!(out.output(b64((s >> 2) & 0x3F)));
            if n != 0 {
                ck!(out.output(b64((s << 4) & 0x30)));
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
                ck!(out.output(c));
                f.status = 0;
            } else {
                f.status = 3;
                f.cache = ((s & 0x3) << 16) | c;
            }
        }
        3 => {
            let s = f.cache;
            ck!(out.output(b64((s >> 12) & 0x3F)));
            ck!(out.output(b64((s >> 6) & 0x3F)));
            ck!(out.output(b64(s & 0x3F)));
            if n != 0 {
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
                ck!(out.output(c));
                f.status = 0;
            } else {
                f.status = 1;
                f.cache = c;
            }
        }
        _ => unreachable!("invalid UTF-7 filter state"),
    }

    0
}

pub fn mbfl_filt_conv_wchar_utf7_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let status = f.status;
    let cache = f.cache;
    f.status = 0;
    f.cache = 0;

    // flush fragments
    match status {
        1 => {
            ck!(out.output(b64((cache >> 10) & 0x3F)));
            ck!(out.output(b64((cache >> 4) & 0x3F)));
            ck!(out.output(b64((cache << 2) & 0x3C)));
            ck!(out.output(b'-' as i32));
        }
        2 => {
            ck!(out.output(b64((cache >> 14) & 0x3F)));
            ck!(out.output(b64((cache >> 8) & 0x3F)));
            ck!(out.output(b64((cache >> 2) & 0x3F)));
            ck!(out.output(b64((cache << 4) & 0x30)));
            ck!(out.output(b'-' as i32));
        }
        3 => {
            ck!(out.output(b64((cache >> 12) & 0x3F)));
            ck!(out.output(b64((cache >> 6) & 0x3F)));
            ck!(out.output(b64(cache & 0x3F)));
            ck!(out.output(b'-' as i32));
        }
        _ => {}
    }

    out.flush();
    0
}

#[inline]
fn is_base64_end(c: u8) -> bool {
    c >= DASH
}

/// `is_optional_direct`: characters which may be either Base64-encoded or
/// directly encoded.
fn is_optional_direct(c: u8) -> bool {
    matches!(
        c,
        b'!' | b'"'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'*'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'@'
            | b'['
            | b']'
            | b'^'
            | b'_'
            | b'`'
            | b'{'
            | b'|'
            | b'}'
    )
}

fn can_end_base64(c: u32) -> bool {
    c == b' ' as u32
        || c == b'\t' as u32
        || c == b'\r' as u32
        || c == b'\n' as u32
        || c == b'\'' as u32
        || c == b'(' as u32
        || c == b')' as u32
        || c == b',' as u32
        || c == b'.' as u32
        || c == b':' as u32
        || c == b'?' as u32
}

/// `decode_base64`: a 6-bit value, or one of `DASH`, `DIRECT`, `ASCII`,
/// `ILLEGAL`.
fn decode_base64(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - 65,
        b'a'..=b'z' => c - 71,
        b'0'..=b'9' => c + 4,
        b'+' => 62,
        b'/' => 63,
        b'-' => DASH,
        _ if can_end_base64(c as u32) || is_optional_direct(c) || c == b'\0' => DIRECT,
        _ if c <= 0x7F => ASCII,
        _ => ILLEGAL,
    }
}

/// `handle_utf16_cp`.
fn handle_utf16_cp(cp: u16, out: &mut [u32], o: &mut usize, surrogate1: &mut u16) {
    loop {
        if *surrogate1 != 0 {
            if (0xDC00..=0xDFFF).contains(&cp) {
                out[*o] = ((*surrogate1 as u32 & 0x3FF) << 10) + (cp as u32 & 0x3FF) + 0x10000;
                *o += 1;
                *surrogate1 = 0;
            } else {
                out[*o] = BAD_INPUT;
                *o += 1;
                *surrogate1 = 0;
                continue;
            }
        } else if (0xD800..=0xDBFF).contains(&cp) {
            *surrogate1 = cp;
        } else if (0xDC00..=0xDFFF).contains(&cp) {
            // 2nd part of surrogate pair came unexpectedly
            out[*o] = BAD_INPUT;
            *o += 1;
        } else {
            out[*o] = cp as u32;
            *o += 1;
        }
        return;
    }
}

/// `handle_base64_end`.
#[allow(clippy::too_many_arguments)]
fn handle_base64_end(
    n: u8,
    p: &mut usize,
    out: &mut [u32],
    o: &mut usize,
    base64: &mut bool,
    abrupt: bool,
    surrogate1: &mut u16,
) {
    if abrupt || *surrogate1 != 0 {
        out[*o] = BAD_INPUT;
        *o += 1;
        *surrogate1 = 0;
    }

    if n == ILLEGAL {
        out[*o] = BAD_INPUT;
        *o += 1;
    } else if n == DIRECT || n == ASCII {
        // Unconsume byte
        *p -= 1;
    }

    *base64 = false;
}

pub fn mb_utf7_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    // One iteration of the Base64 loop can emit up to 5 codepoints (see the
    // comment in php-src); with less room it would never make progress.
    debug_assert!(out.len() >= 5);

    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    let limit = out.len();

    let mut base64 = *state & 1 != 0;
    // First half of a surrogate pair which still needs 2nd half
    let mut surrogate1 = (*state >> 1) as u16;

    while p < e && o < limit {
        if base64 {
            // Base64 section
            if limit - o < 5 {
                break;
            }

            let n1 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n1) {
                handle_base64_end(n1, &mut p, out, &mut o, &mut base64, false, &mut surrogate1);
                continue;
            } else if p == e {
                handle_base64_end(n1, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n2 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n2) || p == e {
                handle_base64_end(n2, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n3 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n3) {
                handle_base64_end(n3, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let cp = ((n1 as u32) << 10) | ((n2 as u32) << 4) | ((n3 as u32 & 0x3C) >> 2);
            handle_utf16_cp(cp as u16, out, &mut o, &mut surrogate1);
            if p == e {
                // It is an error if trailing padding bits are not zeroes or if we were
                // expecting the 2nd part of a surrogate pair when Base64 section ends
                if (n3 & 0x3) != 0 || surrogate1 != 0 {
                    out[o] = BAD_INPUT;
                    o += 1;
                    surrogate1 = 0;
                }
                break;
            }

            let n4 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n4) {
                handle_base64_end(
                    n4,
                    &mut p,
                    out,
                    &mut o,
                    &mut base64,
                    n3 & 0x3 != 0,
                    &mut surrogate1,
                );
                continue;
            } else if p == e {
                handle_base64_end(n4, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n5 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n5) || p == e {
                handle_base64_end(n5, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n6 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n6) {
                handle_base64_end(n6, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let cp = ((n3 as u32) << 14)
                | ((n4 as u32) << 8)
                | ((n5 as u32) << 2)
                | ((n6 as u32 & 0x30) >> 4);
            handle_utf16_cp(cp as u16, out, &mut o, &mut surrogate1);
            if p == e {
                if (n6 & 0xF) != 0 || surrogate1 != 0 {
                    out[o] = BAD_INPUT;
                    o += 1;
                    surrogate1 = 0;
                }
                break;
            }

            let n7 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n7) {
                handle_base64_end(
                    n7,
                    &mut p,
                    out,
                    &mut o,
                    &mut base64,
                    n6 & 0xF != 0,
                    &mut surrogate1,
                );
                continue;
            } else if p == e {
                handle_base64_end(n7, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n8 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n8) {
                handle_base64_end(n8, &mut p, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let cp = ((n6 as u32) << 12) | ((n7 as u32) << 6) | n8 as u32;
            handle_utf16_cp(cp as u16, out, &mut o, &mut surrogate1);
        } else {
            // ASCII text section
            let c = s[p];
            p += 1;

            if c == b'+' {
                if p < e {
                    if s[p] == b'-' {
                        out[o] = b'+' as u32;
                        o += 1;
                        p += 1;
                    } else {
                        base64 = true;
                    }
                }
                // If a + comes at the end of the input string... do nothing about it
            } else if c <= 0x7F {
                out[o] = c as u32;
                o += 1;
            } else {
                out[o] = BAD_INPUT;
                o += 1;
            }
        }
    }

    if p == e && surrogate1 != 0 {
        out[o] = BAD_INPUT;
        o += 1;
    }

    *state = ((surrogate1 as u32) << 1) | base64 as u32;
    *input = &s[p..];
    o
}

fn should_direct_encode(c: u32) -> bool {
    (b'A' as u32..=b'Z' as u32).contains(&c)
        || (b'a' as u32..=b'z' as u32).contains(&c)
        || (b'0' as u32..=b'9' as u32).contains(&c)
        || c == 0
        || c == b'/' as u32
        || c == b'-' as u32
        || can_end_base64(c)
}

/// `SAVE_CONVERSION_STATE`.
#[inline]
pub(crate) fn save_state(cache: u8, nbits: u8, base64: bool) -> u32 {
    ((cache as u32) << 4) | ((nbits as u32) << 1) | base64 as u32
}

/// `RESTORE_CONVERSION_STATE`: `(base64, nbits, cache)`.
#[inline]
pub(crate) fn restore_state(state: u32) -> (bool, u8, u8) {
    (
        state & 1 != 0,
        ((state >> 1) & 0x7) as u8,
        (state >> 4) as u8,
    )
}

pub fn mb_wchar_to_utf7(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    // `nbits` is the number of cached bits; either 0, 2, or 4
    let (mut base64, mut nbits, mut cache) = restore_state(buf.state);

    let mut i = 0usize;
    while i < input.len() {
        let w = input[i];
        i += 1;
        if base64 {
            if should_direct_encode(w) {
                // End of Base64 section. Drain buffered bits (if any), close Base64 section
                base64 = false;
                // Unconsume codepoint; it will be handled by 'ASCII section' code below
                i -= 1;
                if nbits != 0 {
                    buf.add(MBFL_BASE64_TABLE[(((cache as u32) << (6 - nbits)) & 0x3F) as usize]);
                }
                nbits = 0;
                cache = 0;
                if !can_end_base64(w) {
                    buf.add(b'-');
                }
            } else if w >= WCSPLANE_UTF32MAX {
                // Make recursive call to add an error marker character
                buf.state = save_state(cache, nbits, base64);
                mb_illegal_output(w, mb_wchar_to_utf7, buf);
                (base64, nbits, cache) = restore_state(buf.state);
            } else {
                // Encode codepoint, preceded by any cached bits, as Base64
                let bits: u64;
                if w >= WCSPLANE_SUPMIN {
                    // Must use surrogate pair
                    let w = w - 0x10000;
                    bits = ((cache as u64) << 32)
                        | 0xD800_DC00
                        | (((w & 0xFFC00) << 6) as u64)
                        | (w & 0x3FF) as u64;
                    nbits += 32;
                } else {
                    bits = ((cache as u64) << 16) | w as u64;
                    nbits += 16;
                }

                while nbits >= 6 {
                    buf.add(MBFL_BASE64_TABLE[((bits >> (nbits - 6)) & 0x3F) as usize]);
                    nbits -= 6;
                }
                cache = bits as u8;
            }
        } else {
            // ASCII section
            if should_direct_encode(w) {
                buf.add(w as u8);
            } else if w >= WCSPLANE_UTF32MAX {
                buf.state = 0;
                mb_illegal_output(w, mb_wchar_to_utf7, buf);
                (base64, nbits, cache) = restore_state(buf.state);
            } else {
                buf.add(b'+');
                base64 = true;
                // Unconsume codepoint; it will be handled by Base64 code above
                i -= 1;
            }
        }
    }

    if end {
        if nbits != 0 {
            buf.add(MBFL_BASE64_TABLE[(((cache as u32) << (6 - nbits)) & 0x3F) as usize]);
        }
        if base64 {
            buf.add(b'-');
        }
    } else {
        buf.state = save_state(cache, nbits, base64);
    }
}

fn is_utf16_cp_valid(cp: u16, is_surrogate: bool) -> bool {
    if is_surrogate {
        (0xDC00..=0xDFFF).contains(&cp)
    } else {
        // 2nd part of surrogate pair came unexpectedly
        !(0xDC00..=0xDFFF).contains(&cp)
    }
}

fn can_encode_directly(c: u8) -> bool {
    should_direct_encode(c as u32) || is_optional_direct(c) || c == b'\0'
}

pub fn mb_check_utf7(input: &[u8]) -> bool {
    let s = input;
    let e = s.len();
    let mut p = 0usize;
    let mut base64 = false;
    let mut is_surrogate = false;

    while p < e {
        if base64 {
            let n1 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n1) {
                if !is_base64_end_valid(n1, false, is_surrogate) {
                    return false;
                }
                base64 = false;
                continue;
            } else if p == e {
                return false;
            }
            let n2 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n2) || p == e {
                return false;
            }
            let n3 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n3) {
                return false;
            }
            let cp1 = (((n1 as u32) << 10) | ((n2 as u32) << 4) | ((n3 as u32 & 0x3C) >> 2)) as u16;
            if !is_utf16_cp_valid(cp1, is_surrogate) {
                return false;
            }
            is_surrogate = has_surrogate(cp1, is_surrogate);
            if p == e {
                // It is an error if trailing padding bits are not zeroes or if we were
                // expecting the 2nd part of a surrogate pair when Base64 section ends
                return !((n3 & 0x3) != 0 || is_surrogate);
            }

            let n4 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n4) {
                if !is_base64_end_valid(n4, n3 & 0x3 != 0, is_surrogate) {
                    return false;
                }
                base64 = false;
                continue;
            } else if p == e {
                return false;
            }
            let n5 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n5) || p == e {
                return false;
            }
            let n6 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n6) {
                return false;
            }
            let cp2 = (((n3 as u32) << 14)
                | ((n4 as u32) << 8)
                | ((n5 as u32) << 2)
                | ((n6 as u32 & 0x30) >> 4)) as u16;
            if !is_utf16_cp_valid(cp2, is_surrogate) {
                return false;
            }
            is_surrogate = has_surrogate(cp2, is_surrogate);
            if p == e {
                return !((n6 & 0xF) != 0 || is_surrogate);
            }

            let n7 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n7) {
                if !is_base64_end_valid(n7, n6 & 0xF != 0, is_surrogate) {
                    return false;
                }
                base64 = false;
                continue;
            } else if p == e {
                return false;
            }
            let n8 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n8) {
                return false;
            }
            let cp3 = (((n6 as u32) << 12) | ((n7 as u32) << 6) | n8 as u32) as u16;
            if !is_utf16_cp_valid(cp3, is_surrogate) {
                return false;
            }
            is_surrogate = has_surrogate(cp3, is_surrogate);
        } else {
            // ASCII text section
            let c = s[p];
            p += 1;

            if c == b'+' {
                if p == e {
                    return !is_surrogate;
                }
                let n = decode_base64(s[p]);
                if n == DASH {
                    p += 1;
                } else if n > DASH {
                    // If a "+" character followed immediately by any character other than base64 or "-"
                    return false;
                } else {
                    base64 = true;
                }
            } else if can_encode_directly(c) {
                continue;
            } else {
                return false;
            }
        }
    }
    !is_surrogate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::filters::utf8::MBFL_ENCODING_UTF8;
    use crate::mbstring::mbfl::convert::{decode_all, encode_all, mb_fast_convert};
    use crate::mbstring::mbfl::encoding::{ILLEGAL_MODE_CHAR, MIN_WCHAR_BUFSIZE};
    use crate::mbstring::mbfl::strcut::mbfl_strcut;

    /// php's `base64_encode` without the `=` padding.
    fn raw_encode(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            for i in 0..=chunk.len() {
                out.push(MBFL_BASE64_TABLE[((n >> (18 - 6 * i)) & 0x3F) as usize]);
            }
        }
        out
    }

    fn utf16be(cps: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &cp in cps {
            if cp >= 0x10000 {
                let v = cp - 0x10000;
                out.extend_from_slice(&((0xD800 | (v >> 10)) as u16).to_be_bytes());
                out.extend_from_slice(&((0xDC00 | (v & 0x3FF)) as u16).to_be_bytes());
            } else {
                out.extend_from_slice(&(cp as u16).to_be_bytes());
            }
        }
        out
    }

    /// `'+' . encode(...)`: a Base64 section holding `cps` as UTF-16BE.
    fn section(cps: &[u32]) -> Vec<u8> {
        let mut s = b"+".to_vec();
        s.extend(raw_encode(&utf16be(cps)));
        s
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// Decoded codepoints with error markers shown as '%'.
    fn decode(s: &[u8]) -> Vec<u32> {
        decode_all(s, &MBFL_ENCODING_UTF7)
            .into_iter()
            .map(|c| if c == BAD_INPUT { b'%' as u32 } else { c })
            .collect()
    }

    fn encode(cps: &[u32]) -> (Vec<u8>, u32) {
        let buf = encode_all(cps, &MBFL_ENCODING_UTF7, b'%' as u32, ILLEGAL_MODE_CHAR);
        (buf.out, buf.errors)
    }

    /// `testValidString($utf7, <cps as UTF-32BE>, 'UTF-7', 'UTF-32BE', $both_ways)`.
    fn test_valid(utf7: &[u8], cps: &[u32], both_ways: bool) {
        assert!(mb_check_utf7(utf7), "check failed on good string {utf7:?}");
        let decoded = decode_all(utf7, &MBFL_ENCODING_UTF7);
        assert_eq!(decoded, cps, "decoding {:?}", String::from_utf8_lossy(utf7));
        if both_ways {
            assert_eq!(encode(cps), (utf7.to_vec(), 0), "encoding {cps:X?}");
        }
    }

    /// `testInvalidString($utf7, <expected as UTF-32BE>, 'UTF-7', 'UTF-32BE')`
    /// with '%' as substitute character.
    fn test_invalid(utf7: &[u8], expected: &[u32]) {
        assert!(!mb_check_utf7(utf7), "check passed on bad string {utf7:?}");
        assert!(decode_all(utf7, &MBFL_ENCODING_UTF7).contains(&BAD_INPUT));
        assert_eq!(
            decode(utf7),
            expected,
            "decoding {:?}",
            String::from_utf8_lossy(utf7)
        );
    }

    fn conv(s: &[u8], from: &'static Encoding, to: &'static Encoding, subst: u8) -> (Vec<u8>, u32) {
        mb_fast_convert(s, from, to, subst as u32, ILLEGAL_MODE_CHAR)
    }

    const P: u32 = b'%' as u32;

    #[test]
    fn utf_encodings_valid() {
        test_valid(b"", &[], true);

        // 'Direct' characters
        for c in (b'A'..=b'Z')
            .chain(b'a'..=b'z')
            .chain(b'0'..=b'9')
            .chain(*b"'(),-./:?")
        {
            test_valid(&[c], &[c as u32], true);
        }
        // Whitespace
        for c in *b" \t\r\n\x00" {
            test_valid(&[c], &[c as u32], true);
        }
        // Encoding + as +-
        test_valid(b"+-", &[b'+' as u32], false);

        // UTF-16 + Base64 encoding
        for i in 0..256u32 {
            let c = i as u8;
            let reversible = !(c.is_ascii_alphanumeric() || b"'(),-./:?\x00 \t\r\n".contains(&c));
            test_valid(&cat(&[&section(&[i]), b"-"]), &[i], reversible);
        }

        test_valid(&cat(&[&section(&[0x1234]), b"-"]), &[0x1234], true);
        test_valid(
            &cat(&[&section(&[0x1234, 0x5678]), b"-"]),
            &[0x1234, 0x5678],
            true,
        );
        test_valid(
            &cat(&[&section(&[0x1234, 0x5678, 0x40]), b"-"]),
            &[0x1234, 0x5678, 0x40],
            true,
        );
        test_valid(
            &cat(&[&section(&[0xFFEE, 0xEEFF]), b"-"]),
            &[0xFFEE, 0xEEFF],
            true,
        );

        // Surrogate pair
        test_valid(&cat(&[&section(&[0x10400]), b"-"]), &[0x10400], true);
        test_valid(
            &cat(&[&section(&[0x41, 0x10400, 0x42]), b"-"]),
            &[0x41, 0x10400, 0x42],
            false,
        );
        test_valid(
            &cat(&[&section(&[0x10400, 0x10400]), b"-"]),
            &[0x10400, 0x10400],
            true,
        );

        // Unterminated + section (this is not considered illegal)
        test_valid(&section(&[0x41, 0x42, 0x43]), &[0x41, 0x42, 0x43], false);

        // + sections immediately after each other (this isn't illegal either)
        test_valid(
            &cat(&[&section(&[0x41, 0x42]), b"-", &section(&[0x43, 0x44]), b"-"]),
            &[0x41, 0x42, 0x43, 0x44],
            false,
        );
        // + sections not immediately after each other
        test_valid(
            &cat(&[
                &section(&[0x41, 0x42]),
                b"-!",
                &section(&[0x43, 0x44]),
                b"-",
            ]),
            &[0x41, 0x42, 0x21, 0x43, 0x44],
            false,
        );

        // + section terminated by a non-Base64 direct character which is NOT -
        for &c in b" \t\r\n'(),.:?!\"#$%&*;<=>@[]^_`{|}\x00" {
            test_valid(
                &cat(&[&section(&[0x1234]), &[c]]),
                &[0x1234, c as u32],
                false,
            );
        }

        // Non-direct character followed by direct character
        test_valid(b"+ACU-A", &[P, 0x41], true);
        test_valid(b"+ACUAJQ-A", &[P, P, 0x41], true);
        test_valid(b"+ACUAJQAl-A", &[P, P, P, 0x41], true);
    }

    #[test]
    fn utf_encodings_invalid() {
        // Totally bogus byte
        test_invalid(b"\xFF", &[P]);
        // Totally bogus codepoint... '+ACU-' is '%' in UTF-7
        assert_eq!(encode(&[0x1234_5678]), (b"+ACU-".to_vec(), 1));

        // Second half of surrogate pair coming first
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\xDC\x01\xD8\x02"), b"-"]),
            &[P, P],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\x00.\xDC\x01\xD8\x02"), b"-"]),
            &[0x2E, P, P],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\x00.\x00.\xDC\x01\xD8\x02"), b"-"]),
            &[0x2E, 0x2E, P, P],
        );

        // First half of surrogate pair not followed by second half
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\xD8\x01\x00A"), b"-"]),
            &[P, 0x41],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\xD8\x01\xD9\x02"), b"-"]),
            &[P, P],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\x00.\xD8\x01\x00A"), b"-"]),
            &[0x2E, P, 0x41],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\x00.\xD8\x01\xD9\x02"), b"-"]),
            &[0x2E, P, P],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\x00.\x00.\xD8\x01\x00A"), b"-"]),
            &[0x2E, 0x2E, P, 0x41],
        );
        test_invalid(
            &cat(&[b"+", &raw_encode(b"\x00.\x00.\xD8\x01\xD9\x02"), b"-"]),
            &[0x2E, 0x2E, P, P],
        );

        // First half of surrogate pair appearing at end of string
        test_invalid(&cat(&[b"+", &raw_encode(b"\xD8\x01"), b"-"]), &[P]);
        test_invalid(&cat(&[b"+", &raw_encode(b"\xD8\x01")]), &[P]);
        let to_utf8 = |s: &[u8]| conv(s, &MBFL_ENCODING_UTF7, &MBFL_ENCODING_UTF8, b'%').0;
        for (input, expected) in [
            (&b"+999999uJ"[..], &b"\xEF\x9F\x9F\xE7\xB7\xB7%"[..]),
            (b"+999euJ", b"\xEF\x9F\x9F\xE5\xBA\xB8%"),
            (b"+euJ", b"\xE7\xAB\xA2%"),
            // Base64 section should not have 4 ASCII characters; the first 3 can encode one
            // UTF-16 character, so there is no need for the 4th
            (b"+RR8I", b"\xE4\x94\x9F%"),
            // Likewise with 7 characters
            (b"+RR8IAAA", b"\xE4\x94\x9F\xE0\xA0\x80%"),
        ] {
            assert!(!mb_check_utf7(input));
            assert_eq!(to_utf8(input), expected, "{input:?}");
        }

        // Truncated string
        test_invalid(&cat(&[b"+", &raw_encode(b"\x01"), b"-"]), &[P]);
        test_invalid(b"+l", &[P]);
        // It is useless for a Base64 section to only contain a single 'A'
        test_invalid(b"+A", &[P]);

        // Bad padding on + section (not zeroes)
        let mut corrupted = section(&[0x1234]);
        corrupted[3] += 1;
        corrupted.push(b'-');
        test_invalid(&corrupted, &[0x1234, P]);

        // Characters which are not Base64 (and not even ASCII) appearing in Base64 section
        test_invalid(b"+\x80", &[P]);

        // Codepoints over U+10FFFF; '+ACU-' is the error marker '%'
        assert_eq!(encode(&[0x1234_5678]), (b"+ACU-".to_vec(), 1));
        assert_eq!(encode(&[0x0011_5678]), (b"+ACU-".to_vec(), 1));

        // If error marker character needs to be ASCII-encoded but is able to serve as an
        // ending character for a Base64 section, no need to add an additional dash
        assert_eq!(
            conv(b"\x1E\xBE", &MBFL_ENCODING_UTF7, &MBFL_ENCODING_UTF7, b'?'),
            (b"+AB4?".to_vec(), 1)
        );
    }

    /// gh10192_utf7.phpt: (input, mb_check_encoding, conversion to UTF-8 with
    /// '?' as substitute character, number of errors).
    #[test]
    fn gh10192() {
        let cases: &[(&[u8], bool, &[u8], u32)] = &[
            (b"A + B", false, b"A  B", 0),
            (b"A - B", true, b"A - B", 0),
            (b"A 1+ B", false, b"A 1 B", 0),
            (b"A 1- B", true, b"A 1- B", 0),
            (b"A +1 B", false, b"A ? B", 1),
            (b"A -1 B", true, b"A -1 B", 0),
            (b"A 1+1 B", false, b"A 1? B", 1),
            (b"A 1-1 B", true, b"A 1-1 B", 0),
            (b"A +", true, b"A ", 0),
            (b"A -", true, b"A -", 0),
            (b"A +- B", true, b"A + B", 0),
            (b"A -+ B", false, b"A - B", 0),
            (b"A = B", true, b"A = B", 0),
            (b"A ~ B", false, b"A ~ B", 0),
            (b"A \\ B", false, b"A \\ B", 0),
            (b"A \x1B B", false, b"A \x1B B", 0),
            (b"A += B", false, b"A = B", 0),
            (b"A +~ B", false, b"A ~ B", 0),
            (b"A +\\ B", false, b"A \\ B", 0),
            (b"A +\x1B B", false, b"A \x1B B", 0),
            (
                b"A +ZeVnLIqe- B",
                true,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E B",
                0,
            ),
            (b"A +ZeVnLIq- B", false, b"A \xE6\x97\xA5\xE6\x9C\xAC? B", 1),
            (
                b"A +ZeVnLIqe B",
                true,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E B",
                0,
            ),
            (b"A +ZeVnLIq B", false, b"A \xE6\x97\xA5\xE6\x9C\xAC? B", 1),
            (
                b"A +ZeVnLIqe1 B",
                false,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E? B",
                1,
            ),
            (
                b"A +ZeVnLIq1 B",
                true,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\xB5 B",
                0,
            ),
            (
                b"A +ZeVnLIqe",
                true,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E",
                0,
            ),
            (b"A +ZeVnLIq", false, b"A \xE6\x97\xA5\xE6\x9C\xAC?", 1),
            (
                b"A +++++++++- B",
                true,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE B",
                0,
            ),
            (
                b"A +++++++++- B",
                true,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE B",
                0,
            ),
            (
                b"A +++++++++ B",
                true,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE B",
                0,
            ),
            (
                b"A +++++++++ B",
                true,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE B",
                0,
            ),
            (
                b"A +++++++++1 B",
                false,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE? B",
                1,
            ),
            (
                b"A +++++++++1 B",
                false,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE? B",
                1,
            ),
            (
                b"A +++++++++",
                true,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE",
                0,
            ),
            (
                b"A +++++++++",
                true,
                b"A \xEF\xAF\xAF\xEB\xBB\xBB\xEE\xBE\xBE",
                0,
            ),
            (b"A +2GfePQ- B", true, b"A \xF0\xA9\xB8\xBD B", 0),
            (b"A +2Gc- B", false, b"A ? B", 1),
            (b"A +2GfePQ B", true, b"A \xF0\xA9\xB8\xBD B", 0),
            (b"A +2Gc B", false, b"A ? B", 1),
            (b"A +2GfePQ1 B", false, b"A \xF0\xA9\xB8\xBD? B", 1),
            (b"A +2Gc1 B", false, b"A ? B", 1),
            (b"A +2GfePQ", true, b"A \xF0\xA9\xB8\xBD", 0),
            (b"A +2Gc", false, b"A ?", 1),
            (b"A +3j3YZw- B", false, b"A ?? B", 2),
            (b"A +3j0- B", false, b"A ? B", 1),
            (b"A +3j3YZw B", false, b"A ?? B", 2),
            (b"A +3j0 B", false, b"A ? B", 1),
            (b"A +3j3YZw1 B", false, b"A ?? B", 2),
            (b"A +3j01 B", false, b"A ?? B", 2),
            (b"A +3j3YZw", false, b"A ??", 2),
            (b"A +3j0", false, b"A ?", 1),
        ];
        for &(input, valid, expected, errors) in cases {
            assert_eq!(
                mb_check_utf7(input),
                valid,
                "check {:?}",
                String::from_utf8_lossy(input)
            );
            assert_eq!(
                conv(input, &MBFL_ENCODING_UTF7, &MBFL_ENCODING_UTF8, b'?'),
                (expected.to_vec(), errors),
                "convert {:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn small_output_buffer_keeps_state() {
        // Decoding with the minimum buffer size gives the same result as
        // decoding in one go.
        let input = cat(&[
            b"ab",
            &section(&[0x65E5, 0x672C, 0x10400, 0x8A9E, 0xD801]),
            b"-c+-",
        ]);
        let mut all = Vec::new();
        let mut buf = [0u32; MIN_WCHAR_BUFSIZE];
        let mut state = 0;
        let mut inp = &input[..];
        while !inp.is_empty() {
            let n = mb_utf7_to_wchar(&mut inp, &mut buf, &mut state);
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all, decode_all(&input, &MBFL_ENCODING_UTF7));
        assert_eq!(
            all,
            [
                0x61, 0x62, 0x65E5, 0x672C, 0x10400, 0x8A9E, BAD_INPUT, 0x63, 0x2B
            ]
        );

        // Encoding in pieces gives the same result as encoding in one go.
        let cps = [
            0x41, 0x3042, 0x10400, 0x3044, 0x2E, 0x3046, 0x110000, 0x42, 0x3048,
        ];
        let whole = encode(&cps);
        for split in 0..=cps.len() {
            let mut buf = ConvertBuf::new(0, b'%' as u32, ILLEGAL_MODE_CHAR);
            mb_wchar_to_utf7(&cps[..split], &mut buf, false);
            mb_wchar_to_utf7(&cps[split..], &mut buf, true);
            assert_eq!((buf.out, buf.errors), whole, "split at {split}");
        }
    }

    /// The UTF-7 part of mb_strcut.phpt (through the legacy filters).
    #[test]
    fn strcut() {
        let cut = |s: &[u8], from: usize, len: usize| {
            mbfl_strcut(s, &MBFL_ENCODING_UTF7, from, len).unwrap()
        };
        assert_eq!(cut(b"\x01", 0, 100), b"");
        assert_eq!(cut(b"+Q", 1, 100), b"");
        assert_eq!(cut(b"+QxxC", 0, 100), b"+Qxw-");
        // mb_strcut("+UUU", -1, 255) and mb_strcut("+UUUU", -2, 255)
        assert_eq!(cut(b"+UUU", 3, 255), b"");
        assert_eq!(cut(b"+UUUU", 3, 255), b"");
        // oom_mb_strcut_length.phpt
        assert_eq!(cut(b"abc", 0, 100 * 1024 * 1024), b"abc");
        // The re-encoded text does not need the '-' before ' '
        let s = cat(&[b"ab ", &section(&[0x65E5, 0x672C, 0x8A9E]), b"- cd"]);
        assert_eq!(cut(&s, 0, s.len()), b"ab +ZeVnLIqe cd");
        assert_eq!(cut(&s, 0, 3), b"ab ");
    }

    /// The legacy filters decode and encode like the fast converters.
    #[test]
    fn legacy_filters() {
        use crate::mbstring::mbfl::encoding::{NoEncoding, enc};
        use crate::mbstring::mbfl::filter::convert_filter_new;

        struct Collect(Vec<i32>);
        impl FilterOutput for Collect {
            fn output(&mut self, c: i32) -> i32 {
                self.0.push(c);
                0
            }
            fn flush(&mut self) -> i32 {
                0
            }
        }

        let wchar = enc(NoEncoding::Wchar);
        let mut dec = convert_filter_new(&MBFL_ENCODING_UTF7, wchar).unwrap();
        let mut out = Collect(Vec::new());
        let input = cat(&[b"a+-b", &section(&[0x65E5, 0x10400, 0x8A9E]), b"-c"]);
        for &c in &input {
            dec.feed(c as i32, &mut out);
        }
        dec.flush(&mut out);
        assert_eq!(out.0, [0x61, 0x2B, 0x62, 0x65E5, 0x10400, 0x8A9E, 0x63]);

        let mut encf = convert_filter_new(wchar, &MBFL_ENCODING_UTF7).unwrap();
        let mut out = Collect(Vec::new());
        for cp in [0x61, 0x65E5, 0x10400, 0x8A9E, 0x63, 0x3042] {
            encf.feed(cp, &mut out);
        }
        encf.flush(&mut out);
        let bytes: Vec<u8> = out.0.iter().map(|&c| c as u8).collect();
        assert_eq!(
            bytes,
            encode(&[0x61, 0x65E5, 0x10400, 0x8A9E, 0x63, 0x3042]).0
        );
    }
}
