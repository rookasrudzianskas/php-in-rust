//! `libmbfl/filters/mbfilter_base64.c`: the (deprecated) "BASE64"
//! pseudo-encoding. Conversions always go through "8bit", so the
//! "codepoints" here are raw bytes.
//!
//! The legacy filters `vtbl_8bit_b64` / `vtbl_b64_8bit` encode with CR LF
//! line breaks after 76 output characters.

use crate::mbstring::mbfl::convert::ConvertBuf;
use crate::mbstring::mbfl::encoding::{BAD_INPUT, ENCTYPE_GL_UNSAFE, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, ck, common_ctor};

pub static MBFL_ENCODING_BASE64: Encoding = Encoding {
    no: NoEncoding::Base64,
    name: "BASE64",
    mime_name: Some("BASE64"),
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: None,
    output_filter: None,
    to_wchar: mb_base64_to_wchar,
    from_wchar: mb_wchar_to_base64,
    check: None,
    cut: None,
};

pub static VTBL_8BIT_B64: Vtbl = Vtbl {
    from: NoEncoding::EightBit,
    to: NoEncoding::Base64,
    ctor: common_ctor,
    function: mbfl_filt_conv_base64enc,
    flush: mbfl_filt_conv_base64enc_flush,
};

pub static VTBL_B64_8BIT: Vtbl = Vtbl {
    from: NoEncoding::Base64,
    to: NoEncoding::EightBit,
    ctor: common_ctor,
    function: mbfl_filt_conv_base64dec,
    flush: mbfl_filt_conv_base64dec_flush,
};

/// `mbfl_base64_table` (without the trailing NUL).
const MBFL_BASE64_TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[inline]
fn b64(i: i32) -> i32 {
    MBFL_BASE64_TABLE[i as usize] as i32
}

/// any => BASE64. `status` holds the number of cached bytes (low byte) and
/// the length of the current output line (second byte).
pub fn mbfl_filt_conv_base64enc(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut n = f.status & 0xFF;
    if n == 0 {
        f.status += 1;
        f.cache = (c & 0xFF) << 16;
    } else if n == 1 {
        f.status += 1;
        f.cache |= (c & 0xFF) << 8;
    } else {
        f.status &= !0xFF;
        n = (f.status & 0xFF00) >> 8;
        if n > 72 {
            ck!(out.output(0x0D)); // CR
            ck!(out.output(0x0A)); // LF
            f.status &= !0xFF00;
        }
        f.status += 0x400;
        n = f.cache | (c & 0xFF);
        ck!(out.output(b64((n >> 18) & 0x3F)));
        ck!(out.output(b64((n >> 12) & 0x3F)));
        ck!(out.output(b64((n >> 6) & 0x3F)));
        ck!(out.output(b64(n & 0x3F)));
    }

    0
}

pub fn mbfl_filt_conv_base64enc_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let status = f.status & 0xFF;
    let cache = f.cache;
    let len = (f.status & 0xFF00) >> 8;
    f.status &= !0xFFFF;
    f.cache = 0;

    // flush fragments
    if status >= 1 {
        if len > 72 {
            ck!(out.output(0x0D)); // CR
            ck!(out.output(0x0A)); // LF
        }
        ck!(out.output(b64((cache >> 18) & 0x3F)));
        ck!(out.output(b64((cache >> 12) & 0x3F)));
        if status == 1 {
            ck!(out.output(b'=' as i32));
            ck!(out.output(b'=' as i32));
        } else {
            ck!(out.output(b64((cache >> 6) & 0x3F)));
            ck!(out.output(b'=' as i32));
        }
    }

    out.flush();
    0
}

/// BASE64 => any.
pub fn mbfl_filt_conv_base64dec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    // CR or LF or SPACE or HTAB or '='
    if c == 0x0D || c == 0x0A || c == 0x20 || c == 0x09 || c == 0x3D {
        return 0;
    }

    let mut n;
    if (0x41..=0x5A).contains(&c) {
        // A - Z
        n = c - 65;
    } else if (0x61..=0x7A).contains(&c) {
        // a - z
        n = c - 71;
    } else if (0x30..=0x39).contains(&c) {
        // 0 - 9
        n = c + 4;
    } else if c == 0x2B {
        // '+'
        n = 62;
    } else if c == 0x2F {
        // '/'
        n = 63;
    } else {
        ck!(out.output(BAD_INPUT as i32));
        return 0;
    }
    n &= 0x3F;

    match f.status {
        0 => {
            f.status = 1;
            f.cache = n << 18;
        }
        1 => {
            f.status = 2;
            f.cache |= n << 12;
        }
        2 => {
            f.status = 3;
            f.cache |= n << 6;
        }
        _ => {
            f.status = 0;
            n |= f.cache;
            ck!(out.output((n >> 16) & 0xFF));
            ck!(out.output((n >> 8) & 0xFF));
            ck!(out.output(n & 0xFF));
        }
    }

    0
}

pub fn mbfl_filt_conv_base64dec_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let status = f.status;
    let cache = f.cache;
    f.status = 0;
    f.cache = 0;

    // flush fragments
    if status >= 2 {
        ck!(out.output((cache >> 16) & 0xFF));
        if status >= 3 {
            ck!(out.output((cache >> 8) & 0xFF));
        }
    }

    out.flush();
    0
}

/// `decode_base64`: -1 for bytes outside the Base64 alphabet.
fn decode_base64(c: u8) -> i32 {
    match c {
        b'A'..=b'Z' => (c - b'A') as i32,
        b'a'..=b'z' => (c - b'a') as i32 + 26,
        b'0'..=b'9' => (c - b'0') as i32 + 52,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    }
}

/// `state` packs the number of cached bits (low byte) and the cached bits.
pub fn mb_base64_to_wchar(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize {
    debug_assert!(out.len() >= 3);

    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    let limit = out.len();

    let mut bits = *state & 0xFF;
    let mut cache = *state >> 8;

    while p < e && limit - o >= 3 {
        let c = s[p];
        p += 1;

        if c == b'\r' || c == b'\n' || c == b' ' || c == b'\t' || c == b'=' {
            continue;
        }

        let value = decode_base64(c);

        if value == -1 {
            out[o] = BAD_INPUT;
            o += 1;
        } else {
            bits += 6;
            cache = (cache << 6) | (value as u32 & 0x3F);
            if bits == 24 {
                out[o] = (cache >> 16) & 0xFF;
                out[o + 1] = (cache >> 8) & 0xFF;
                out[o + 2] = cache & 0xFF;
                o += 3;
                bits = 0;
                cache = 0;
            }
        }
    }

    if p == e {
        if bits != 0 {
            // If we reach here, there will be at least 3 spaces remaining in output buffer
            if bits == 18 {
                out[o] = (cache >> 10) & 0xFF;
                out[o + 1] = (cache >> 2) & 0xFF;
                o += 2;
            } else if bits == 12 {
                out[o] = (cache >> 4) & 0xFF;
                o += 1;
            }
        }
    } else {
        *state = (cache << 8) | (bits & 0xFF);
    }

    *input = &s[p..];
    o
}

/// `buf.state` packs the number of cached bytes (bits 0-1), the length of
/// the current output line / 4 (bits 2-7) and the cached bytes.
pub fn mb_wchar_to_base64(input: &[u32], buf: &mut ConvertBuf, end: bool) {
    let mut bits = (buf.state & 0x3) * 8;
    let mut chars_output = ((buf.state >> 2) & 0x3F) * 4;
    let mut cache = buf.state >> 8;

    for &w in input {
        cache = (cache << 8) | (w & 0xFF);
        bits += 8;
        if bits == 24 {
            if chars_output > 72 {
                buf.add2(b'\r', b'\n');
                chars_output = 0;
            }
            buf.add4(
                MBFL_BASE64_TABLE[((cache >> 18) & 0x3F) as usize],
                MBFL_BASE64_TABLE[((cache >> 12) & 0x3F) as usize],
                MBFL_BASE64_TABLE[((cache >> 6) & 0x3F) as usize],
                MBFL_BASE64_TABLE[(cache & 0x3F) as usize],
            );
            chars_output += 4;
            bits = 0;
            cache = 0;
        }
    }

    if end && bits != 0 {
        if chars_output > 72 {
            buf.add2(b'\r', b'\n');
        }
        if bits == 8 {
            buf.add4(
                MBFL_BASE64_TABLE[((cache >> 2) & 0x3F) as usize],
                MBFL_BASE64_TABLE[((cache & 0x3) << 4) as usize],
                b'=',
                b'=',
            );
        } else {
            buf.add4(
                MBFL_BASE64_TABLE[((cache >> 10) & 0x3F) as usize],
                MBFL_BASE64_TABLE[((cache >> 4) & 0x3F) as usize],
                MBFL_BASE64_TABLE[((cache & 0xF) << 2) as usize],
                b'=',
            );
        }
    } else {
        buf.state = (cache << 8) | (((chars_output / 4) & 0x3F) << 2) | ((bits / 8) & 0x3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::filters::eightbit::MBFL_ENCODING_8BIT;
    use crate::mbstring::mbfl::convert::{decode_all, mb_fast_convert};
    use crate::mbstring::mbfl::encoding::ILLEGAL_MODE_CHAR;
    use crate::mbstring::mbfl::filter::convert_filter_new;

    struct Collect(Vec<u8>);
    impl FilterOutput for Collect {
        fn output(&mut self, c: i32) -> i32 {
            self.0.push(c as u8);
            0
        }
        fn flush(&mut self) -> i32 {
            0
        }
    }

    /// Runs `s` through the legacy filter from `from` to `to`.
    fn legacy(s: &[u8], from: &'static Encoding, to: &'static Encoding) -> Vec<u8> {
        let mut f = convert_filter_new(from, to).unwrap();
        let mut out = Collect(Vec::new());
        for &c in s {
            f.feed(c as i32, &mut out);
        }
        f.flush(&mut out);
        out.0
    }

    fn conv(s: &[u8], from: &'static Encoding, to: &'static Encoding) -> Vec<u8> {
        mb_fast_convert(s, from, to, b'?' as u32, ILLEGAL_MODE_CHAR).0
    }

    /// base64_encoding.phpt (plus the same vectors through the legacy filters)
    #[test]
    fn base64_encoding() {
        let long = "ABCDEFGHIJ".repeat(20);
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"a", b"YQ=="),
            (b"ab", b"YWI="),
            (b"\x01\x02\x03", b"AQID"),
            (b"\xFF\xFE\x11\x22", b"//4RIg=="),
            (b"\x00", b"AA=="),
            (b"\x00\x00", b"AAA="),
            (b"\x00\x00\x00", b"AAAA"),
            (
                long.as_bytes(),
                b"QUJDREVGR0hJSkFCQ0RFRkdISUpBQkNERUZHSElKQUJDREVGR0hJSkFCQ0RFRkdISUpBQkNERUZH\r\nSElKQUJDREVGR0hJSkFCQ0RFRkdISUpBQkNERUZHSElKQUJDREVGR0hJSkFCQ0RFRkdISUpBQkNE\r\nRUZHSElKQUJDREVGR0hJSkFCQ0RFRkdISUpBQkNERUZHSElKQUJDREVGR0hJSkFCQ0RFRkdISUpB\r\nQkNERUZHSElKQUJDREVGR0hJSkFCQ0RFRkdISUo=",
            ),
        ];
        let b64 = &MBFL_ENCODING_BASE64;
        let bit8 = &MBFL_ENCODING_8BIT;
        for &(raw, base64) in cases {
            assert_eq!(conv(raw, bit8, b64), base64, "{raw:X?}");
            assert_eq!(
                conv(base64, b64, bit8),
                raw,
                "{:?}",
                String::from_utf8_lossy(base64)
            );
            assert_eq!(legacy(raw, bit8, b64), base64, "{raw:X?}");
            assert_eq!(legacy(base64, b64, bit8), raw);
        }
    }

    #[test]
    fn decode_errors_and_state() {
        // Bytes outside the alphabet are errors; whitespace and '=' are skipped.
        // The trailing partial group is only emitted at the end of the input.
        assert_eq!(
            decode_all(b"Y Q\r\n=*=", &MBFL_ENCODING_BASE64),
            [BAD_INPUT, 0x61]
        );
        // Decoding with the minimum buffer size keeps the cached bits.
        let data = format!("{}xy", "ABCDEFGHIJ".repeat(20));
        let input = conv(data.as_bytes(), &MBFL_ENCODING_8BIT, &MBFL_ENCODING_BASE64);
        let mut all = Vec::new();
        let mut buf = [0u32; 3];
        let mut state = 0;
        let mut inp = &input[..];
        while !inp.is_empty() {
            let n = mb_base64_to_wchar(&mut inp, &mut buf, &mut state);
            all.extend_from_slice(&buf[..n]);
        }
        let expected: Vec<u32> = data.bytes().map(|b| b as u32).collect();
        assert_eq!(all, expected);
    }
}
