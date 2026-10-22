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
            let n = if m < 0 {
                ck!(out.output(0x3D)); // '='
                ck!(out.output(f.cache));
                c
            } else {
                // `cache` always holds a hex digit here.
                (hex2code((f.cache & 0xFF) as u8) << 4) | m
            };
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
    if nibble < 10 {
        nibble + b'0'
    } else {
        nibble - 10 + b'A'
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::filters::eightbit::MBFL_ENCODING_8BIT;
    use crate::mbstring::mbfl::convert::mb_fast_convert;
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

    /// qprint_encoding.phpt (plus the same vectors through the legacy filters)
    #[test]
    fn qprint_encoding() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            ("あ".as_bytes(), b"=E3=81=82"),
            (
                b"J'interdis aux marchands de vanter trop leurs marchandises. Car ils se font vite p\xC3\xA9dagogues et t'enseignent comme but ce qui n'est par essence qu'un moyen, et te trompant ainsi sur la route \xC3\xA0 suivre les voil\xC3\xA0 bient\xC3\xB4t qui te d\xC3\xA9gradent, car si leur musique est vulgaire ils te fabriquent pour te la vendre une \xC3\xA2me vulgaire.\n\xE2\x80\x94\xE2\x80\x89Antoine de Saint-Exup\xC3\xA9ry, Citadelle",
                b"J'interdis aux marchands de vanter trop leurs marchandises. Car ils se f=\r\nont vite p=C3=A9dagogues et t'enseignent comme but ce qui n'est par esse=\r\nnce qu'un moyen, et te trompant ainsi sur la route =C3=A0 suivre les voi=\r\nl=C3=A0 bient=C3=B4t qui te d=C3=A9gradent, car si leur musique est vulg=\r\naire ils te fabriquent pour te la vendre une =C3=A2me vulgaire.\r\n=E2=80=94=E2=80=89Antoine de Saint-Exup=C3=A9ry, Citadelle",
            ),
            // Regression test, found by a fuzzer
            (
                b"***,*-S,\xAC,,\xB7,l,,,,\xB6UTF7,\xB5\xB5\xB5\xB5\xB5\xB5K\xB5\xB5!\xA4\xA4\xA4\xA4\xA4\xBD\xB5,\xB5\xB5\xB5\xB5\xB5\xB5\xB5\xB5\xB5&",
                b"***,*-S,=AC,,=B7,l,,,,=B6UTF7,=B5=B5=B5=B5=B5=B5K=B5=B5!=A4=A4=A4=A4=A4=BD=\r\n=B5,=B5=B5=B5=B5=B5=B5=B5=B5=B5&",
            ),
        ];
        let qp = &MBFL_ENCODING_QPRINT;
        let bit8 = &MBFL_ENCODING_8BIT;
        for &(raw, qprint) in cases {
            let crlf: Vec<u8> = raw
                .iter()
                .flat_map(|&c| {
                    if c == b'\n' {
                        vec![b'\r', b'\n']
                    } else {
                        vec![c]
                    }
                })
                .collect();
            assert_eq!(conv(raw, bit8, qp), qprint);
            assert_eq!(conv(qprint, qp, bit8), crlf);
            assert_eq!(legacy(raw, bit8, qp), qprint);
            assert_eq!(legacy(qprint, qp, bit8), crlf);
        }
    }

    #[test]
    fn decode_edge_cases() {
        let qp = &MBFL_ENCODING_QPRINT;
        let bit8 = &MBFL_ENCODING_8BIT;
        // Soft line breaks, a bad hex digit, lowercase hex, '=' at the end
        let input: &[u8] = b"a=\r\nb=\nc=4G=3d=";
        assert_eq!(conv(input, qp, bit8), b"abc=4G==");
        assert_eq!(legacy(input, qp, bit8), b"abc=4G==");
        // A bad 2nd hex digit is kept literally (even when it is '=')
        assert_eq!(conv(b"=4=3d", qp, bit8), b"=4=3d");
        assert_eq!(legacy(b"=4=3d", qp, bit8), b"=4=3d");
        // A lone CR is dropped by the encoder; NUL passes through
        assert_eq!(conv(b"a\rb\x00c", bit8, qp), b"ab\x00c");
    }
}
