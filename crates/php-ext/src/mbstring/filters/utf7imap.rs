//! `libmbfl/filters/mbfilter_utf7imap.c`: modified UTF-7 for IMAP mailbox
//! names (RFC 3501): `&` starts a Base64 section (with `,` instead of `/`)
//! which must always be closed by `-`; `&-` stands for `&`.
//!
//! Besides the fast converters and the validity check, the legacy filters
//! are kept because `mbfl_strcut` walks UTF7-IMAP strings with them.

use crate::mbstring::filters::utf7::{DASH, ILLEGAL, has_surrogate, is_base64_end_valid, restore_state, save_state};
use crate::mbstring::mbfl::convert::{ConvertBuf, mb_illegal_output};
use crate::mbstring::mbfl::encoding::{BAD_INPUT, Encoding, NoEncoding, WCSPLANE_SUPMAX, WCSPLANE_SUPMIN, WCSPLANE_UCS2MAX, WCSPLANE_UTF32MAX};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, illegal_output};

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
const MBFL_UTF7IMAP_BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

/// The common tail of states 4, 7 and 9 of `mbfl_filt_conv_utf7imap_wchar`:
/// `s` is a complete UTF-16 unit and `n` the bits already received for the
/// next one.
fn utf7imap_wchar_emit(mut s: i32, n: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
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

pub fn mbfl_filt_conv_utf7imap_wchar(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
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

pub fn mbfl_filt_conv_utf7imap_wchar_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
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

pub fn mbfl_filt_conv_wchar_utf7imap(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
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
pub fn mbfl_filt_conv_wchar_utf7imap_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
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
fn handle_base64_end(n: u8, out: &mut [u32], o: &mut usize, base64: &mut bool, abrupt: bool, surrogate1: &mut u16) {
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
            let cp = ((n3 as u32) << 14) | ((n4 as u32) << 8) | ((n5 as u32) << 2) | ((n6 as u32 & 0x30) >> 4);
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
                    buf.add(MBFL_UTF7IMAP_BASE64_TABLE[(((cache as u32) << (6 - nbits)) & 0x3F) as usize]);
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
            let cp2 = (((n3 as u32) << 14) | ((n4 as u32) << 8) | ((n5 as u32) << 2) | ((n6 as u32 & 0x30) >> 4))
                as u16;
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
