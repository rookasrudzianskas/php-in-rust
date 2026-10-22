//! `libmbfl/filters/mbfilter_uuencode.c`: the (deprecated) "UUENCODE"
//! pseudo-encoding. Conversions always go through "8bit", so the
//! "codepoints" here are raw bytes.
//!
//! Decoding skips everything up to a `begin ` line; encoding emits a
//! `begin 0644 filename` header and lines of at most 45 input bytes (the
//! caller has to add the terminating "`\nend\n").

use crate::mbstring::mbfl::convert::ConvertBuf;
use crate::mbstring::mbfl::encoding::{ENCTYPE_SBCS, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, ck, common_ctor, common_flush};

pub static MBFL_ENCODING_UUENCODE: Encoding = Encoding {
    no: NoEncoding::Uuencode,
    name: "UUENCODE",
    mime_name: Some("x-uuencode"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_SBCS,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_uuencode_to_wchar,
    from_wchar: mb_wchar_to_uuencode,
    check: None,
    cut: None,
};

pub static VTBL_UUENCODE_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Uuencode,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_uudec,
    flush: common_flush,
};

/// `UUDEC(c)`.
#[inline]
fn uudec(c: i32) -> i32 {
    (c.wrapping_sub(b' ' as i32) & 0o77) as i8 as i32
}

const UUENC_BEGIN_TEXT: &[u8] = b"begin ";

// Decoder states.
const UUDEC_STATE_GROUND: i32 = 0;
const UUDEC_STATE_INBEGIN: i32 = 1;
const UUDEC_STATE_UNTIL_NEWLINE: i32 = 2;
const UUDEC_STATE_SIZE: i32 = 3;
const UUDEC_STATE_A: i32 = 4;
const UUDEC_STATE_B: i32 = 5;
const UUDEC_STATE_C: i32 = 6;
const UUDEC_STATE_D: i32 = 7;
const UUDEC_STATE_SKIP_NEWLINE: i32 = 8;

pub fn mbfl_filt_conv_uudec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    match f.status {
        UUDEC_STATE_GROUND => {
            // looking for "begin 0666 filename\n" line
            if f.cache == 0 && c == b'b' as i32 {
                f.status = UUDEC_STATE_INBEGIN;
                f.cache = 1; // move to 'e'
            } else if c == b'\n' as i32 {
                f.cache = 0;
            } else {
                f.cache += 1;
            }
        }
        UUDEC_STATE_INBEGIN => {
            let expected = UUENC_BEGIN_TEXT[f.cache as usize] as i32;
            f.cache += 1;
            if expected != c {
                // doesn't match pattern
                f.status = UUDEC_STATE_GROUND;
            } else if f.cache == 5 {
                // that's good enough - wait for a newline
                f.status = UUDEC_STATE_UNTIL_NEWLINE;
                f.cache = 0;
            }
        }
        UUDEC_STATE_UNTIL_NEWLINE => {
            if c == b'\n' as i32 {
                f.status = UUDEC_STATE_SIZE;
            }
        }
        UUDEC_STATE_SIZE => {
            // get "size" byte
            let n = uudec(c);
            f.cache = n << 24;
            f.status = UUDEC_STATE_A;
        }
        UUDEC_STATE_A => {
            // get "a" byte
            let n = uudec(c);
            f.cache |= n << 16;
            f.status = UUDEC_STATE_B;
        }
        UUDEC_STATE_B => {
            // get "b" byte
            let n = uudec(c);
            f.cache |= n << 8;
            f.status = UUDEC_STATE_C;
        }
        UUDEC_STATE_C => {
            // get "c" byte
            let n = uudec(c);
            f.cache |= n;
            f.status = UUDEC_STATE_D;
        }
        UUDEC_STATE_D => {
            // get "d" byte; the output bytes are not masked (as in php-src)
            let d = uudec(c);
            let a = (f.cache >> 16) & 0xFF;
            let b = (f.cache >> 8) & 0xFF;
            let cc = f.cache & 0xFF;
            let mut n = (f.cache >> 24) & 0xFF;
            if n > 0 {
                ck!(out.output((a << 2) | (b >> 4)));
            }
            n -= 1;
            if n > 0 {
                ck!(out.output((b << 4) | (cc >> 2)));
            }
            n -= 1;
            if n > 0 {
                ck!(out.output((cc << 6) | d));
            }
            n -= 1;
            if n < 0 {
                n = 0;
            }
            f.cache = n << 24;

            if n == 0 {
                f.status = UUDEC_STATE_SKIP_NEWLINE; // skip next byte (newline)
            } else {
                f.status = UUDEC_STATE_A; // go back to fetch "A" byte
            }
        }
        UUDEC_STATE_SKIP_NEWLINE => {
            // skip newline
            f.status = UUDEC_STATE_SIZE;
        }
        _ => {}
    }
    0
}

/// `state` packs the decoder state (low byte) and the number of bytes left
/// on the current line.
pub fn mb_uuencode_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    debug_assert!(out.len() >= 3);

    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    let limit = out.len();

    let mut st = (*state & 0xFF) as i32;
    let mut size = *state >> 8;

    while p < e && limit - o >= 3 {
        let c = s[p];
        p += 1;

        match st {
            UUDEC_STATE_GROUND => {
                if c == b'b' && e - p >= 5 && s[p..p + 5] == UUENC_BEGIN_TEXT[1..] {
                    p += 5;
                    // Consume everything up to newline
                    while p < e {
                        let ch = s[p];
                        p += 1;
                        if ch == b'\n' {
                            break;
                        }
                    }
                    st = UUDEC_STATE_SIZE;
                }
                // We didn't find "begin "
            }
            UUDEC_STATE_SIZE => {
                size = uudec(c as i32) as u32;
                st = UUDEC_STATE_A;
            }
            UUDEC_STATE_A => {
                // php-src requires 4 more bytes here although only 3 are read.
                if e - p < 4 {
                    p = e;
                    continue;
                }

                let a = uudec(c as i32) as u32;
                let b = uudec(s[p] as i32) as u32;
                let cc = uudec(s[p + 1] as i32) as u32;
                let d = uudec(s[p + 2] as i32) as u32;
                p += 3;

                if size > 0 {
                    out[o] = ((a << 2) | (b >> 4)) & 0xFF;
                    o += 1;
                    size -= 1;
                }
                if size > 0 {
                    out[o] = ((b << 4) | (cc >> 2)) & 0xFF;
                    o += 1;
                    size -= 1;
                }
                if size > 0 {
                    out[o] = ((cc << 6) | d) & 0xFF;
                    o += 1;
                    size -= 1;
                }

                st = if size != 0 { UUDEC_STATE_A } else { UUDEC_STATE_SKIP_NEWLINE };
            }
            UUDEC_STATE_SKIP_NEWLINE => {
                st = UUDEC_STATE_SIZE;
            }
            _ => {}
        }
    }

    *state = (size << 8) | st as u32;
    *input = &s[p..];
    o
}

fn uuencode_six_bits(bits: u32) -> u8 {
    if bits == 0 { b'`' } else { (bits + 32) as u8 }
}

/// `buf.state` packs "header emitted" (bit 0), the number of bytes encoded
/// on the current line (bits 1-7), the number of cached bits (bits 8-15)
/// and the cached bits.
pub fn mb_wchar_to_uuencode(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let mut len = input.len();
    let mut i = 0usize;

    let mut bytes_encoded = (buf.state >> 1) & 0x7F;
    // UUEncode naturally wants to process input bytes in groups of 3, but
    // our buffer size may not be a multiple of 3
    // So there may be data from the previous call which we need to flush out
    let mut n_cached_bits = (buf.state >> 8) & 0xFF;
    let mut cached_bits = buf.state >> 16;
    // `goto possible_line_break` from the flush of cached bits
    let mut line_break_pending = false;

    if buf.state == 0 {
        buf.append(b"begin 0644 filename\n");
        buf.add((len.min(45) + 32) as u8);
        buf.state |= 1;
    } else if len == 0 && end && bytes_encoded == 0 && n_cached_bits == 0 {
        // Corner case: under EXTREMELY rare circumstances, it's possible that the
        // final call to this conversion function will happen with an empty input
        // buffer, leaving an unwanted trailing len byte in the output buffer.
        buf.out.pop();
        return;
    } else {
        // UUEncode starts each line with a byte which indicates how many bytes
        // are encoded on the line; if a previous line was cut off, go back and
        // fix up the preceding len byte
        let cached_chars = if n_cached_bits != 0 {
            if n_cached_bits == 2 { 1 } else { 2 }
        } else {
            0
        };
        let len_byte = buf.out.len() - (bytes_encoded * 4 / 3) as usize - 1 - cached_chars;
        buf.out[len_byte] = ((bytes_encoded as usize + len + cached_chars).min(45) + 32) as u8;

        if n_cached_bits != 0 {
            // Flush out bits which remained from previous call
            if n_cached_bits == 2 {
                let w = cached_bits;
                let (mut w2, mut w3) = (0u32, 0u32);
                if len != 0 {
                    w2 = input[i];
                    i += 1;
                    len -= 1;
                }
                if len != 0 {
                    w3 = input[i];
                    i += 1;
                    len -= 1;
                }
                buf.add3(
                    uuencode_six_bits((w << 4) + ((w2 >> 4) & 0xF)),
                    uuencode_six_bits(((w2 & 0xF) << 2) + ((w3 >> 6) & 0x3)),
                    uuencode_six_bits(w3 & 0x3F),
                );
            } else {
                let w2 = cached_bits;
                let mut w3 = 0u32;
                if len != 0 {
                    w3 = input[i];
                    i += 1;
                    len -= 1;
                }
                buf.add2(
                    uuencode_six_bits((w2 << 2) + ((w3 >> 6) & 0x3)),
                    uuencode_six_bits(w3 & 0x3F),
                );
            }
            n_cached_bits = 0;
            cached_bits = 0;
            line_break_pending = true;
        }
    }

    loop {
        if !line_break_pending {
            if len == 0 {
                break;
            }
            len -= 1;
            let w = input[i];
            i += 1;
            let (mut w2, mut w3) = (0u32, 0u32);

            if len == 0 {
                if !end {
                    buf.add(uuencode_six_bits((w >> 2) & 0x3F));
                    // Cache 2 remaining bits from 'w'
                    cached_bits = w & 0x3;
                    n_cached_bits = 2;
                    break;
                }
            } else {
                w2 = input[i];
                i += 1;
                len -= 1;
            }

            if len == 0 {
                if !end {
                    buf.add2(
                        uuencode_six_bits((w >> 2) & 0x3F),
                        uuencode_six_bits(((w & 0x3) << 4) + ((w2 >> 4) & 0xF)),
                    );
                    // Cache 4 remaining bits from 'w2'
                    cached_bits = w2 & 0xF;
                    n_cached_bits = 4;
                    break;
                }
            } else {
                w3 = input[i];
                i += 1;
                len -= 1;
            }

            buf.add4(
                uuencode_six_bits((w >> 2) & 0x3F),
                uuencode_six_bits(((w & 0x3) << 4) + ((w2 >> 4) & 0xF)),
                uuencode_six_bits(((w2 & 0xF) << 2) + ((w3 >> 6) & 0x3)),
                uuencode_six_bits(w3 & 0x3F),
            );
        }
        line_break_pending = false;

        // possible_line_break:
        bytes_encoded += 3;

        if bytes_encoded >= 45 {
            buf.add(b'\n');
            if len != 0 || !end {
                buf.add((len.min(45) + 32) as u8);
            }
            bytes_encoded = 0;
        }
    }

    if bytes_encoded != 0 && end {
        buf.add(b'\n');
    }

    buf.state = ((cached_bits & 0xFF) << 16)
        | ((n_cached_bits & 0xFF) << 8)
        | ((bytes_encoded & 0x7F) << 1)
        | (buf.state & 1);
}
