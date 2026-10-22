//! `libmbfl/filters/mbfilter_utf7imap.c`: modified UTF-7 for IMAP mailbox
//! names (RFC 3501): `&` starts a Base64 section (with `,` instead of `/`)
//! which must always be closed by `-`; `&-` stands for `&`.
//!
//! Besides the fast converters and the validity check, the legacy filters
//! are kept because `mbfl_strcut` walks UTF7-IMAP strings with them.

use crate::mbstring::filters::utf7::{
    DASH, ILLEGAL, has_surrogate, is_base64_end_valid, restore_state, save_state,
};
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{
    BAD_INPUT, Encoding, NoEncoding, WCSPLANE_SUPMAX, WCSPLANE_SUPMIN, WCSPLANE_UCS2MAX,
    WCSPLANE_UTF32MAX,
};
use crate::mbstring::mbfl::filter::{
    ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, illegal_output,
};

pub static MBFL_ENCODING_UTF7IMAP: Encoding = Encoding {
    no: NoEncoding::Utf7imap,
    name: "UTF7-IMAP",
    mime_name: None,
    aliases: &["mUTF-7"],
    mblen_table: None,
    flag: 0,
    input_filter: Some(&VTBL_UTF7IMAP_WCHAR),
    output_filter: Some(&VTBL_WCHAR_UTF7IMAP),
    to_wchar: mb_utf7imap_to_wchar,
    from_wchar: mb_wchar_to_utf7imap,
    check: Some(mb_check_utf7imap),
    cut: None,
};

pub static VTBL_UTF7IMAP_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::Utf7imap,
    to: NoEncoding::Wchar,
    ctor: common_ctor,
    function: mbfl_filt_conv_utf7imap_wchar,
    flush: mbfl_filt_conv_utf7imap_wchar_flush,
};

pub static VTBL_WCHAR_UTF7IMAP: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::Utf7imap,
    ctor: common_ctor,
    function: mbfl_filt_conv_wchar_utf7imap,
    flush: mbfl_filt_conv_wchar_utf7imap_flush,
};

/// `mbfl_utf7imap_base64_table` / `mbfl_base64_table` of this file (without
/// the trailing NUL); both are the same.
const MBFL_UTF7IMAP_BASE64_TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

/// The common tail of states 4, 7 and 9 of `mbfl_filt_conv_utf7imap_wchar`:
/// `s` is a complete UTF-16 unit and `n` the bits already received for the
/// next one.
fn utf7imap_wchar_emit(
    mut s: i32,
    n: i32,
    f: &mut ConvertFilter,
    out: &mut dyn FilterOutput,
) -> i32 {
    if (0xD800..0xDC00).contains(&s) {
        // 1st part of surrogate pair
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
            // illegal character
            ck!(out.output(BAD_INPUT as i32));
        }
    } else {
        f.cache = n;
        // Characters which can be expressed as literal, ASCII characters
        // should not be Base64-encoded
        if !(0x20..=0x7E).contains(&s) || s == b'&' as i32 {
            ck!(out.output(s));
        } else {
            ck!(out.output(BAD_INPUT as i32));
        }
    }
    0
}

pub fn mbfl_filt_conv_utf7imap_wchar(
    c: i32,
    f: &mut ConvertFilter,
    out: &mut dyn FilterOutput,
) -> i32 {
    let mut n = -1;

    if f.status != 0 {
        // Modified Base64
        if (b'A' as i32..=b'Z' as i32).contains(&c) {
            n = c - 65;
        } else if (b'a' as i32..=b'z' as i32).contains(&c) {
            n = c - 71;
        } else if (b'0' as i32..=b'9' as i32).contains(&c) {
            n = c + 4;
        } else if c == b'+' as i32 {
            n = 62;
        } else if c == b',' as i32 {
            n = 63;
        }

        if !(0..=63).contains(&n) {
            if c == b'-' as i32 {
                if f.status == 1 {
                    // "&-" -> "&"
                    f.cache = 0;
                    f.status = 0;
                    ck!(out.output(b'&' as i32));
                } else if f.cache != 0 {
                    // Base64-encoded section ended abruptly, with partially encoded characters,
                    // or it could be that it ended on the first half of a surrogate pair
                    f.cache = 0;
                    f.status = 0;
                    ck!(out.output(BAD_INPUT as i32));
                } else {
                    // Base64-encoded section properly terminated by -
                    f.cache = 0;
                    f.status = 0;
                }
            } else {
                // illegal character
                f.cache = 0;
                f.status = 0;
                ck!(out.output(BAD_INPUT as i32));
            }
            return 0;
        }
    }

    match f.status {
        // directly encoded characters
        0 => {
            if c == b'&' as i32 {
                // shift character
                f.status += 1;
            } else if (0x20..=0x7E).contains(&c) {
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
            return utf7imap_wchar_emit(s, n, f, out);
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
            return utf7imap_wchar_emit(s, n, f, out);
        }
        8 => {
            f.cache |= n << 6;
            f.status = 9;
        }
        9 => {
            let s = n | (f.cache & 0xFFFF);
            f.status = 2;
            return utf7imap_wchar_emit(s, 0, f, out);
        }
        _ => unreachable!("invalid UTF7-IMAP filter state"),
    }

    0
}

pub fn mbfl_filt_conv_utf7imap_wchar_flush(
    f: &mut ConvertFilter,
    out: &mut dyn FilterOutput,
) -> i32 {
    if f.status != 0 {
        // It is illegal for a UTF-7 IMAP string to end in a Base-64 encoded
        // section. It should always change back to ASCII before the end.
        out.output(BAD_INPUT as i32);
        f.status = 0;
    }
    out.flush();
    0
}

#[inline]
fn b64(i: i32) -> i32 {
    MBFL_UTF7IMAP_BASE64_TABLE[i as usize] as i32
}

pub fn mbfl_filt_conv_wchar_utf7imap(
    c: i32,
    f: &mut ConvertFilter,
    out: &mut dyn FilterOutput,
) -> i32 {
    let mut n = 0;

    if c == b'&' as i32 {
        n = 1;
    } else if (0x20..=0x7E).contains(&c) || c == 0 {
        n = 2;
    } else if c >= 0 && c < WCSPLANE_UCS2MAX as i32 {
        // encoded as one UTF-16 unit
    } else if c >= WCSPLANE_SUPMIN as i32 && c < WCSPLANE_SUPMAX as i32 {
        let s = ((c >> 10) - 0x40) | 0xD800;
        ck!(mbfl_filt_conv_wchar_utf7imap(s, f, out));
        let s = (c & 0x3FF) | 0xDC00;
        ck!(mbfl_filt_conv_wchar_utf7imap(s, f, out));
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
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
            } else {
                // Modified Base64
                ck!(out.output(b'&' as i32));
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
                ck!(out.output(b'-' as i32));
                ck!(out.output(c));
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
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
                ck!(out.output(b'-' as i32));
                ck!(out.output(c));
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
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
                ck!(out.output(b'-' as i32));
                ck!(out.output(c));
                if n == 1 {
                    ck!(out.output(b'-' as i32));
                }
                f.status = 0;
            } else {
                f.status = 1;
                f.cache = c;
            }
        }
        _ => unreachable!("invalid UTF7-IMAP filter state"),
    }

    0
}

/// Unlike the other flush functions this one does not call the next
/// filter's flush function (as in php-src).
pub fn mbfl_filt_conv_wchar_utf7imap_flush(
    f: &mut ConvertFilter,
    out: &mut dyn FilterOutput,
) -> i32 {
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

    0
}

#[inline]
fn is_base64_end(c: u8) -> bool {
    c >= DASH
}

/// `decode_base64`: a 6-bit value, `DASH` or `ILLEGAL`.
fn decode_base64(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - 65,
        b'a'..=b'z' => c - 71,
        b'0'..=b'9' => c + 4,
        b'+' => 62,
        b',' => 63,
        b'-' => DASH,
        _ => ILLEGAL,
    }
}

/// `handle_utf16_cp`: like UTF-7's, but printable ASCII other than `&` must
/// not be Base64-encoded.
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
        } else if (0x20..=0x7E).contains(&cp) && cp != b'&' as u16 {
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
fn handle_base64_end(
    n: u8,
    out: &mut [u32],
    o: &mut usize,
    base64: &mut bool,
    abrupt: bool,
    surrogate1: &mut u16,
) {
    if abrupt || n == ILLEGAL || *surrogate1 != 0 {
        out[*o] = BAD_INPUT;
        *o += 1;
        *surrogate1 = 0;
    }
    *base64 = false;
}

pub fn mb_utf7imap_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    // With less room the Base64 loop would never make progress (see the
    // comment in mb_utf7_to_wchar).
    debug_assert!(out.len() >= 5);

    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    // Always leave one empty space in output buffer in case the string ends while
    // in Base64 mode and we need to emit an error marker
    let limit = out.len() - 1;

    let mut base64 = *state & 1 != 0;
    // First half of a surrogate pair
    let mut surrogate1 = (*state >> 1) as u16;

    while p < e && o < limit {
        if base64 {
            // Base64 section
            if limit - o < 4 {
                break;
            }

            let n1 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n1) {
                handle_base64_end(n1, out, &mut o, &mut base64, false, &mut surrogate1);
                continue;
            } else if p == e {
                handle_base64_end(n1, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n2 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n2) || p == e {
                handle_base64_end(n2, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n3 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n3) {
                handle_base64_end(n3, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let cp = ((n1 as u32) << 10) | ((n2 as u32) << 4) | ((n3 as u32 & 0x3C) >> 2);
            handle_utf16_cp(cp as u16, out, &mut o, &mut surrogate1);
            if p == e {
                // It is an error if trailing padding bits are not zeroes or if we were
                // expecting the 2nd part of a surrogate pair when Base64 section ends
                // (`surrogate1` is not reset here, as in php-src)
                if (n3 & 0x3) != 0 || surrogate1 != 0 {
                    out[o] = BAD_INPUT;
                    o += 1;
                }
                break;
            }

            let n4 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n4) {
                handle_base64_end(n4, out, &mut o, &mut base64, n3 & 0x3 != 0, &mut surrogate1);
                continue;
            } else if p == e {
                handle_base64_end(n4, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n5 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n5) || p == e {
                handle_base64_end(n5, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n6 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n6) {
                handle_base64_end(n6, out, &mut o, &mut base64, true, &mut surrogate1);
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
                }
                break;
            }

            let n7 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n7) {
                handle_base64_end(n7, out, &mut o, &mut base64, n6 & 0xF != 0, &mut surrogate1);
                continue;
            } else if p == e {
                handle_base64_end(n7, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let n8 = decode_base64(s[p]);
            p += 1;
            if is_base64_end(n8) {
                handle_base64_end(n8, out, &mut o, &mut base64, true, &mut surrogate1);
                continue;
            }
            let cp = ((n6 as u32) << 12) | ((n7 as u32) << 6) | n8 as u32;
            handle_utf16_cp(cp as u16, out, &mut o, &mut surrogate1);
        } else {
            let c = s[p];
            p += 1;

            if c == b'&' {
                if p < e && s[p] == b'-' {
                    out[o] = b'&' as u32;
                    o += 1;
                    p += 1;
                } else {
                    base64 = true;
                }
            } else if (0x20..=0x7E).contains(&c) {
                out[o] = c as u32;
                o += 1;
            } else {
                out[o] = BAD_INPUT;
                o += 1;
            }
        }
    }

    if p == e && base64 {
        // UTF7-IMAP doesn't allow strings to end in Base64 mode
        // One space in output buffer was reserved just for this
        out[o] = BAD_INPUT;
        o += 1;
    }

    *state = ((surrogate1 as u32) << 1) | base64 as u32;
    *input = &s[p..];
    o
}

pub fn mb_wchar_to_utf7imap(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    // `nbits` is the number of cached bits; either 0, 2, or 4
    let (mut base64, mut nbits, mut cache) = restore_state(buf.state);

    let mut i = 0usize;
    while i < input.len() {
        let w = input[i];
        i += 1;
        if base64 {
            if (0x20..=0x7E).contains(&w) {
                // End of Base64 section. Drain buffered bits (if any), close Base64 section
                base64 = false;
                // Unconsume codepoint; it will be handled by 'ASCII section' code below
                i -= 1;
                if nbits != 0 {
                    buf.add(
                        MBFL_UTF7IMAP_BASE64_TABLE
                            [(((cache as u32) << (6 - nbits)) & 0x3F) as usize],
                    );
                }
                nbits = 0;
                cache = 0;
                buf.add(b'-');
            } else if w >= WCSPLANE_UTF32MAX {
                // Make recursive call to add an error marker character
                buf.state = save_state(cache, nbits, base64);
                mb_illegal_output(w, mb_wchar_to_utf7imap, buf);
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
                    buf.add(MBFL_UTF7IMAP_BASE64_TABLE[((bits >> (nbits - 6)) & 0x3F) as usize]);
                    nbits -= 6;
                }
                cache = bits as u8;
            }
        } else {
            // ASCII section
            if w == b'&' as u32 {
                buf.add2(b'&', b'-');
            } else if (0x20..=0x7E).contains(&w) {
                buf.add(w as u8);
            } else if w >= WCSPLANE_UTF32MAX {
                buf.state = 0;
                mb_illegal_output(w, mb_wchar_to_utf7imap, buf);
                (base64, nbits, cache) = restore_state(buf.state);
            } else {
                buf.add(b'&');
                base64 = true;
                // Unconsume codepoint; it will be handled by Base64 code above
                i -= 1;
            }
        }
    }

    if end {
        if nbits != 0 {
            buf.add(MBFL_UTF7IMAP_BASE64_TABLE[(((cache as u32) << (6 - nbits)) & 0x3F) as usize]);
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
    } else if (0xDC00..=0xDFFF).contains(&cp) {
        // 2nd part of surrogate pair came unexpectedly
        false
    } else {
        !((0x20..=0x7E).contains(&cp) && cp != b'&' as u16)
    }
}

pub fn mb_check_utf7imap(input: &[u8]) -> bool {
    let s = input;
    let e = s.len();
    let mut p = 0usize;
    let mut base64 = false;
    let mut is_surrogate = false;

    while p < e {
        if base64 {
            // Base64 section
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
                return false;
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
                return false;
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

            if c == b'&' {
                if p == e {
                    return false;
                }
                let n = decode_base64(s[p]);
                if n == DASH {
                    p += 1;
                } else if n == ILLEGAL {
                    return false;
                } else {
                    base64 = true;
                }
            } else if (0x20..=0x7E).contains(&c) {
                continue;
            } else {
                return false;
            }
        }
    }
    !base64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::filters::utf8::MBFL_ENCODING_UTF8;
    use crate::mbstring::mbfl::convert::{decode_all, encode_all, mb_fast_convert};
    use crate::mbstring::mbfl::encoding::{
        ILLEGAL_MODE_CHAR, ILLEGAL_MODE_LONG, MIN_WCHAR_BUFSIZE,
    };
    use crate::mbstring::mbfl::strcut::mbfl_strcut;

    const IMAP: &Encoding = &MBFL_ENCODING_UTF7IMAP;
    const UTF8: &Encoding = &MBFL_ENCODING_UTF8;

    /// `mBase64`: Base64 with ',' for '/' and without padding.
    fn m_base64(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            for i in 0..=chunk.len() {
                out.push(MBFL_UTF7IMAP_BASE64_TABLE[((n >> (18 - 6 * i)) & 0x3F) as usize]);
            }
        }
        out
    }

    fn utf16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    /// `"&" . mBase64($utf16) . $tail`.
    fn amp(utf16: &[u8], tail: &[u8]) -> Vec<u8> {
        [b"&", &m_base64(utf16)[..], tail].concat()
    }

    fn conv(s: &[u8], from: &'static Encoding, to: &'static Encoding) -> (Vec<u8>, u32) {
        mb_fast_convert(s, from, to, b'%' as u32, ILLEGAL_MODE_CHAR)
    }

    /// `testValid`: `testValidString($from, $to, 'UTF7-IMAP', 'UTF-8', $both_ways)`.
    fn test_valid(from: &[u8], to: &[u8], both_ways: bool) {
        assert!(
            mb_check_utf7imap(from),
            "check failed on good string {:?}",
            String::from_utf8_lossy(from)
        );
        assert_eq!(
            conv(from, IMAP, UTF8),
            (to.to_vec(), 0),
            "{:?}",
            String::from_utf8_lossy(from)
        );
        if both_ways {
            assert_eq!(
                conv(to, UTF8, IMAP),
                (from.to_vec(), 0),
                "{:?}",
                String::from_utf8_lossy(to)
            );
        }
    }

    /// `testInvalid`: `testInvalidString($from, $to, 'UTF7-IMAP', 'UTF-8')`.
    fn test_invalid(from: &[u8], to: &[u8]) {
        identify_invalid(from);
        let (out, errors) = conv(from, IMAP, UTF8);
        assert_eq!(out, to, "{:?}", String::from_utf8_lossy(from));
        assert!(errors > 0);
    }

    fn identify_invalid(s: &[u8]) {
        assert!(
            !mb_check_utf7imap(s),
            "check passed on bad string {:?}",
            String::from_utf8_lossy(s)
        );
    }

    #[test]
    fn utf7imap_encoding() {
        // An empty string is valid
        test_valid(b"", b"", true);

        // RFC says that 0x00 should be Base64-encoded
        assert_eq!(conv(b"\x00", UTF8, IMAP), (b"&AAA-".to_vec(), 0));
        assert_eq!(conv(b"&AAA-", IMAP, UTF8), (b"\x00".to_vec(), 0));
        assert!(mb_check_utf7imap(b"&AAA-"));

        // Identification and conversion of ASCII characters (minus &)
        for i in 0x20..=0x7Eu8 {
            if i != b'&' {
                test_valid(&[i], &[i], true);
            }
        }

        // Identification and conversion of non-ASCII characters
        for i in (0..0x20u8).chain(0x7F..=0xFF) {
            test_invalid(&[i], b"%");
        }

        // Identification of '&' when Base-64 encoded
        test_valid(&amp(&utf16be("&"), b"-"), b"&", false);

        // Identification of unterminated & section
        identify_invalid(b"&");
        identify_invalid(b"abc&");
        identify_invalid(&amp(&utf16be("ハムサンドイッチ"), b""));

        // Conversion of Base64-encoded ASCII characters (excluding &)
        for i in 0x20..=0x7Eu8 {
            if i != b'&' {
                test_invalid(&amp(&[0, i], b"-"), b"%");
            }
        }

        // Conversion of & encoded as &-
        test_valid(b"&-", b"&", true);
        test_valid(b"abc&-", b"abc&", true);
        test_valid(b"&-.&-", b"&.&", true);

        // Identification of & sections containing non-Base64
        let test_string = m_base64(&utf16be("我是打酱油的"));
        assert_eq!(test_string.len(), 16);
        for i in 0..=255u8 {
            if i.is_ascii_alphanumeric() || i == b'+' || i == b',' || i == b'-' {
                continue;
            }
            identify_invalid(&[b"&", &test_string[..11], &[i], b"-"].concat());
        }

        // 1. The second half of a surrogate pair could come first,
        let pair = utf16be("\u{10400}");
        assert_eq!(pair.len(), 4);
        let reversed = [&pair[2..], &pair[..2]].concat();
        identify_invalid(&amp(&reversed, b"-"));
        // (Or could appear by itself)
        let low = reversed[..2].to_vec();
        identify_invalid(&amp(&low, b"-"));

        // ...and we should detect this wherever it occurs
        let single: &[u8] = b"\x00\x01";
        let double = utf16be("\u{10401}");
        for bad in [&reversed, &low] {
            for prefix in [
                vec![],
                single.to_vec(),
                [single, single].concat(),
                [single, single, single].concat(),
                double.clone(),
                [single, &double].concat(),
                [single, single, &double].concat(),
            ] {
                identify_invalid(&amp(&[&prefix, &bad[..]].concat(), b"-"));
            }
        }

        // 2. The first half of a surrogate pair might be followed by an invalid 2nd part,
        let bad_pair = [&pair[..2], b"\x00a"].concat();
        for prefix in [
            vec![],
            single.to_vec(),
            [single, single].concat(),
            double.clone(),
        ] {
            identify_invalid(&amp(&[&prefix, &bad_pair[..]].concat(), b"-"));
        }

        // 3. The first half of a surrogate pair could come at the end of the string,
        let high = &pair[..2];
        test_invalid(&amp(high, b"-"), b"%");
        test_invalid(&amp(&[single, high].concat(), b"-"), b"\x01%");
        test_invalid(&amp(&[single, single, high].concat(), b"-"), b"\x01\x01%");
        // ...and the string could even be improperly terminated...
        test_invalid(&amp(high, b""), b"%%");
        test_invalid(&amp(&[single, high].concat(), b""), b"\x01%%");
        // No error marker for the trailing first half of surrogate pair when the string
        // abruptly ends after a group of 3 Base64-encoded codepoints
        test_invalid(&amp(&[single, single, high].concat(), b""), b"\x01\x01%");

        // 4. Or, it could have an odd number of bytes in it!
        let mut odd = utf16be("ドーナツ");
        odd.pop();
        identify_invalid(&amp(&odd, b"-"));

        // The Base64 might not be padded with zeroes
        let encoded = m_base64(&utf16be("☺⛑"));
        assert_eq!(encoded.len(), 6);
        identify_invalid(&[b"&", &encoded[..5], b",-"].concat());

        // OK, let's try valid Base64-encoded text now
        let long1 = utf16be("\u{10401}");
        let long2 = "\u{10401}";
        for text in [
            "☺".to_string(),
            "饺子".to_string(),
            "１２３".to_string(),
            "ﾰﾱﾲﾳ".to_string(),
            long2.to_string(),
            format!("饼{long2}"),
            format!("{long2}饼"),
            format!("☺{long2}饼"),
            format!("西瓜{long2}"),
            format!("西瓜{long2}☺"),
            format!("西瓜{long2}{long2}"),
            format!("西红柿{long2}"),
        ] {
            test_valid(&amp(&utf16be(&text), b"-"), text.as_bytes(), true);
        }
        assert_eq!(long1.len(), 4);

        // Multiple sections of valid ASCII _and_ Base64-encoded text
        test_valid(
            &[
                b"123",
                &amp(&utf16be("１２３"), b"-abc")[..],
                &amp(&utf16be("☺"), b"-."),
            ]
            .concat(),
            "123１２３abc☺.".as_bytes(),
            true,
        );

        // If a & character appears right after a non-ASCII character, we must first close the
        // Base64 section and then emit &-
        assert_eq!(conv("☺&".as_bytes(), UTF8, IMAP), (b"&Jjo-&-".to_vec(), 0));
        assert_eq!(
            conv("西瓜&".as_bytes(), UTF8, IMAP),
            (b"&iX903A-&-".to_vec(), 0)
        );
        assert_eq!(
            conv("西红柿&".as_bytes(), UTF8, IMAP),
            (b"&iX9+omf,-&-".to_vec(), 0)
        );

        // Try illegal Unicode codepoint (> 0x10FFFF)
        let buf = encode_all(&[0x0020_0000], IMAP, b'%' as u32, ILLEGAL_MODE_CHAR);
        assert_eq!((buf.out, buf.errors), (b"%".to_vec(), 1));

        // Test "long" illegal character markers
        let long = |s: &[u8]| mb_fast_convert(s, IMAP, UTF8, b'%' as u32, ILLEGAL_MODE_LONG);
        assert_eq!(long(b"\x10"), (b"%".to_vec(), 1));
        assert_eq!(long(b"\x80"), (b"%".to_vec(), 1));
        // The & starts a Base-64 coded section, which is OK... but there's no data in it
        assert_eq!(long(b"abc&"), (b"abc%".to_vec(), 1));
        // When we hit the first bad byte in a Base-64 coded section, it drops us back into
        // the default mode, so the following characters are literal
        assert_eq!(long(b"&**-"), (b"%*-".to_vec(), 1));

        // Try strings where Base64 has an extra trailing byte which is not needed
        assert_eq!(long(b"&RR8I"), (b"\xE4\x94\x9F%".to_vec(), 1));
        assert_eq!(
            long(b"&RR8IAAA"),
            (b"\xE4\x94\x9F\xE0\xA0\x80%".to_vec(), 1)
        );

        // It is useless for a Base64 section to only contain a single 'A'
        assert_eq!(decode_all(b"&A", IMAP), [BAD_INPUT]);
    }

    /// gh10192_utf7imap.phpt: (input, mb_check_encoding, conversion to UTF-8
    /// with '?' as substitute character, number of errors).
    #[test]
    fn gh10192() {
        let cases: &[(&[u8], bool, &[u8], u32)] = &[
            (b"A & B", false, b"A ?B", 1),
            (b"A - B", true, b"A - B", 0),
            (b"A 1& B", false, b"A 1?B", 1),
            (b"A 1- B", true, b"A 1- B", 0),
            (b"A &1 B", false, b"A ?B", 1),
            (b"A -1 B", true, b"A -1 B", 0),
            (b"A 1&1 B", false, b"A 1?B", 1),
            (b"A 1-1 B", true, b"A 1-1 B", 0),
            (b"A &", false, b"A ?", 1),
            (b"A -", true, b"A -", 0),
            (b"A &- B", true, b"A & B", 0),
            (b"A -& B", false, b"A -?B", 1),
            (b"A ~ B", true, b"A ~ B", 0),
            (b"A \x1B B", false, b"A ? B", 1),
            (b"A &~ B", false, b"A ? B", 1),
            (b"A &\x1B B", false, b"A ? B", 1),
            (
                b"A &ZeVnLIqe- B",
                true,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E B",
                0,
            ),
            (b"A &ZeVnLIq- B", false, b"A \xE6\x97\xA5\xE6\x9C\xAC? B", 1),
            (
                b"A &ZeVnLIqe B",
                false,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E?B",
                1,
            ),
            (b"A &ZeVnLIq B", false, b"A \xE6\x97\xA5\xE6\x9C\xAC?B", 1),
            (
                b"A &ZeVnLIqe1 B",
                false,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E?B",
                1,
            ),
            (
                b"A &ZeVnLIq1 B",
                false,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\xB5?B",
                1,
            ),
            (
                b"A &ZeVnLIqe",
                false,
                b"A \xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E?",
                1,
            ),
            (b"A &ZeVnLIq", false, b"A \xE6\x97\xA5\xE6\x9C\xAC?", 1),
            (b"A &2GfePQ- B", true, b"A \xF0\xA9\xB8\xBD B", 0),
            (b"A &2Gc- B", false, b"A ? B", 1),
            (b"A &2GfePQ B", false, b"A \xF0\xA9\xB8\xBD?B", 1),
            (b"A &2Gc B", false, b"A ?B", 1),
            (b"A &2GfePQ1 B", false, b"A \xF0\xA9\xB8\xBD?B", 1),
            (b"A &2Gc1 B", false, b"A ?B", 1),
            (b"A &2GfePQ", false, b"A \xF0\xA9\xB8\xBD?", 1),
            (b"A &2Gc", false, b"A ??", 2),
            (b"A &3j3YZw- B", false, b"A ?? B", 2),
            (b"A &3j0- B", false, b"A ? B", 1),
            (b"A &3j3YZw B", false, b"A ??B", 2),
            (b"A &3j0 B", false, b"A ??B", 2),
            (b"A &3j3YZw1 B", false, b"A ??B", 2),
            (b"A &3j01 B", false, b"A ??B", 2),
            (b"A &3j3YZw", false, b"A ???", 3),
            (b"A &3j0", false, b"A ??", 2),
        ];
        for &(input, valid, expected, errors) in cases {
            assert_eq!(
                mb_check_utf7imap(input),
                valid,
                "check {:?}",
                String::from_utf8_lossy(input)
            );
            assert_eq!(
                mb_fast_convert(input, IMAP, UTF8, b'?' as u32, ILLEGAL_MODE_CHAR),
                (expected.to_vec(), errors),
                "convert {:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn small_output_buffer_keeps_state() {
        let input = [
            b"ab",
            &amp(&utf16be("日本\u{10400}語"), b"-c&-")[..],
            b"&Jjo",
        ]
        .concat();
        let mut all = Vec::new();
        let mut buf = [0u32; MIN_WCHAR_BUFSIZE];
        let mut state = 0;
        let mut inp = &input[..];
        while !inp.is_empty() {
            let n = mb_utf7imap_to_wchar(&mut inp, &mut buf, &mut state);
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all, decode_all(&input, IMAP));
        assert_eq!(
            all,
            [
                0x61, 0x62, 0x65E5, 0x672C, 0x10400, 0x8A9E, 0x63, 0x26, 0x263A, BAD_INPUT
            ]
        );

        let cps = [
            0x41, 0x3042, 0x10400, 0x26, 0x3044, 0x2E, 0x3046, 0x110000, 0x42, 0x3048,
        ];
        let whole = encode_all(&cps, IMAP, b'%' as u32, ILLEGAL_MODE_CHAR);
        for split in 0..=cps.len() {
            let mut buf = ConvertBuf::new(0, b'%' as u32, ILLEGAL_MODE_CHAR);
            mb_wchar_to_utf7imap(&cps[..split], &mut buf, false);
            mb_wchar_to_utf7imap(&cps[split..], &mut buf, true);
            assert_eq!(
                (&buf.out, buf.errors),
                (&whole.out, whole.errors),
                "split at {split}"
            );
        }
    }

    /// The UTF7-IMAP part of mb_strcut.phpt (through the legacy filters).
    #[test]
    fn strcut() {
        let cut = |s: &[u8], from: usize, len: usize| mbfl_strcut(s, IMAP, from, len).unwrap();
        assert_eq!(cut(b"\x01", 0, 100), b"?");
        assert_eq!(cut(b"&Q", 1, 100), b"");
        assert_eq!(cut(b"&QxxC", 0, 100), b"");
        assert_eq!(cut(b"&i6o\x83", 0, 100), b"");
        let s = [b"ab ", &amp(&utf16be("日本語"), b"- &-cd")[..]].concat();
        assert_eq!(cut(&s, 0, s.len()), s);
        assert_eq!(cut(&s, 0, 3), b"ab ");
    }
}
