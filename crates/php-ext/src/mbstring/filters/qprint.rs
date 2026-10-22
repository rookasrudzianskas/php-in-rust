//! `libmbfl/filters/mbfilter_qprint.c`: the (deprecated) "Quoted-Printable"
//! pseudo-encoding. Conversions always go through "8bit", so the
//! "codepoints" here are raw bytes.
//!
//! Encoded lines are broken with soft line breaks (`=` CR LF) a little
//! before the 76 characters the RFC allows; LF becomes CR LF.

use crate::mbstring::mbfl::convert::ConvertBuf;
use crate::mbstring::mbfl::encoding::{ENCTYPE_GL_UNSAFE, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, ck, common_ctor};

pub static MBFL_ENCODING_QPRINT: Encoding = Encoding {
    no: NoEncoding::Qprint,
    name: "Quoted-Printable",
    mime_name: Some("Quoted-Printable"),
    aliases: &["qprint"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_qprint_to_wchar,
    from_wchar: mb_wchar_to_qprint,
    check: None,
    cut: None,
};

pub static VTBL_8BIT_QPRINT: Vtbl = Vtbl {
    from: NoEncoding::EightBit,
    to: NoEncoding::Qprint,
    ctor: common_ctor,
    function: mbfl_filt_conv_qprintenc,
    flush: mbfl_filt_conv_qprintenc_flush,
};

pub static VTBL_QPRINT_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Qprint,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_qprintdec,
    flush: mbfl_filt_conv_qprintdec_flush,
};

/// any => Quoted-Printable. Each byte is held back in `cache` until the next
/// one arrives (CR LF handling); `status` bits 8-15 hold the line length.
pub fn mbfl_filt_conv_qprintenc(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if f.status & 0xFF == 0 {
        f.cache = c;
        f.status += 1;
        return 0;
    }

    let s = f.cache;
    f.cache = c;
    let mut n = (f.status & 0xFF00) >> 8;

    if s == 0 {
        // null
        ck!(out.output(s));
        f.status &= !0xFF00;
        return 0;
    }

    if s == b'\n' as i32 || (s == b'\r' as i32 && c != b'\n' as i32) {
        // line feed
        ck!(out.output(b'\r' as i32));
        ck!(out.output(b'\n' as i32));
        f.status &= !0xFF00;
        return 0;
    } else if s == 0x0D {
        return 0;
    }

    if n >= 72 {
        // soft line feed
        ck!(out.output(b'=' as i32));
        ck!(out.output(b'\r' as i32));
        ck!(out.output(b'\n' as i32));
        f.status &= !0xFF00;
    }

    if s <= 0 || s >= 0x80 || s == b'=' as i32 {
        // not ASCII or '=': hex-octet
        ck!(out.output(b'=' as i32));
        n = (s >> 4) & 0xF;
        n += if n < 10 { 48 } else { 55 }; // '0' or 'A' - 10
        ck!(out.output(n));
        n = s & 0xF;
        n += if n < 10 { 48 } else { 55 };
        ck!(out.output(n));
        f.status += 0x300;
    } else {
        ck!(out.output(s));
        f.status += 0x100;
    }

    0
}

pub fn mbfl_filt_conv_qprintenc_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // flush filter cache
    ck!(mbfl_filt_conv_qprintenc(b'\0' as i32, f, out));
    f.status &= !0xFFFF;
    f.cache = 0;

    out.flush();
    0
}

/// `hex2code_map`: the value of a hex digit, or -1.
static HEX2CODE_MAP: [i8; 256] = {
    let mut map = [-1i8; 256];
    let mut i = 0;
    while i < 10 {
        map[b'0' as usize + i] = i as i8;
        i += 1;
    }
    let mut i = 0;
    while i < 6 {
        map[b'A' as usize + i] = 10 + i as i8;
        map[b'a' as usize + i] = 10 + i as i8;
        i += 1;
    }
    map
};

#[inline]
fn hex2code(c: u8) -> i32 {
    HEX2CODE_MAP[c as usize] as i32
}

/// Quoted-Printable => any.
pub fn mbfl_filt_conv_qprintdec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    match f.status {
        1 => {
            if hex2code((c & 0xFF) as u8) >= 0 {
                f.cache = c;
                f.status = 2;
            } else if c == 0x0D {
                // soft line feed
                f.status = 3;
            } else if c == 0x0A {
                // soft line feed
                f.status = 0;
            } else {
                ck!(out.output(0x3D)); // '='
                ck!(out.output(c));
                f.status = 0;
            }
        }
        2 => {
            let m = hex2code((c & 0xFF) as u8);
            let n;
            if m < 0 {
                ck!(out.output(0x3D)); // '='
                ck!(out.output(f.cache));
                n = c;
            } else {
                // `cache` always holds a hex digit here.
                n = (hex2code((f.cache & 0xFF) as u8) << 4) | m;
            }
            ck!(out.output(n));
            f.status = 0;
        }
        3 => {
            if c != 0x0A {
                // LF
                ck!(out.output(c));
            }
            f.status = 0;
        }
        _ => {
            if c == 0x3D {
                // '='
                f.status = 1;
            } else {
                ck!(out.output(c));
            }
        }
    }

    0
}

pub fn mbfl_filt_conv_qprintdec_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let status = f.status;
    let cache = f.cache;
    f.status = 0;
    f.cache = 0;

    // flush fragments
    if status == 1 {
        ck!(out.output(0x3D)); // '='
    } else if status == 2 {
        ck!(out.output(0x3D)); // '='
        ck!(out.output(cache));
    }

    out.flush();
    0
}

pub fn mb_qprint_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    // One iteration can emit up to 3 bytes.
    let limit = out.len().saturating_sub(2);

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == b'=' && p < e {
            let c2 = s[p];
            p += 1;

            if hex2code(c2) >= 0 && p < e {
                let c3 = s[p];
                p += 1;

                if hex2code(c3) >= 0 {
                    out[o] = ((hex2code(c2) << 4) | hex2code(c3)) as u32;
                    o += 1;
                } else {
                    out[o] = b'=' as u32;
                    out[o + 1] = c2 as u32;
                    out[o + 2] = c3 as u32;
                    o += 3;
                }
            } else if c2 == b'\r' && p < e {
                let c3 = s[p];
                p += 1;

                if c3 != b'\n' {
                    out[o] = c3 as u32;
                    o += 1;
                }
            } else if c2 != b'\n' {
                out[o] = b'=' as u32;
                out[o + 1] = c2 as u32;
                o += 2;
            }
        } else {
            out[o] = c as u32;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

fn qprint_enc_nibble(nibble: u8) -> u8 {
    if nibble < 10 { nibble + b'0' } else { nibble - 10 + b'A' }
}

/// `buf.state` is the length of the current output line.
pub fn mb_wchar_to_qprint(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    let mut chars_output = buf.state;

    for &w in input {
        // We assume that all the input 'codepoints' are not really Unicode codepoints at all,
        // but raw bytes from 0x00-0xFF
        if w == 0 {
            buf.add(b'\0');
            chars_output = 0;
            continue;
        } else if w == b'\n' as u32 {
            buf.add2(b'\r', b'\n');
            chars_output = 0;
            continue;
        } else if w == b'\r' as u32 {
            // No output
            continue;
        }

        // QPrint actually mandates that line length should not be more than 76 characters,
        // but mbstring stops slightly short of that
        if chars_output >= 72 {
            buf.add3(b'=', b'\r', b'\n');
            chars_output = 0;
        }

        if w >= 0x80 || w == b'=' as u32 {
            // Not ASCII
            buf.add3(
                b'=',
                qprint_enc_nibble(((w >> 4) & 0xF) as u8),
                qprint_enc_nibble((w & 0xF) as u8),
            );
            chars_output += 3;
        } else {
            // Plain ASCII
            buf.add(w as u8);
            chars_output += 1;
        }
    }

    buf.state = chars_output;
}
