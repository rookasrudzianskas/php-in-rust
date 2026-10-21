//! ext/standard/base64.c
//!
//! Only the scalar reference algorithm is ported; the SIMD variants of
//! php-src produce identical results.

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// `PHP_BASE64_NO_PADDING` flag of `php_base64_encode_ex`.
pub const PHP_BASE64_NO_PADDING: i64 = 1;

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const BASE64_PAD: u8 = b'=';

/// `base64_reverse_table`: the 6-bit value of a base64 digit, -1 for the
/// whitespace characters that are always skipped (`\t \n \r` and space) and
/// -2 for every other (invalid) byte.
const BASE64_REVERSE_TABLE: [i8; 256] = build_reverse_table();

const fn build_reverse_table() -> [i8; 256] {
    let mut t = [-2i8; 256];
    let mut i = 0;
    while i < 64 {
        t[BASE64_TABLE[i] as usize] = i as i8;
        i += 1;
    }
    t[b'\t' as usize] = -1;
    t[b'\n' as usize] = -1;
    t[b'\r' as usize] = -1;
    t[b' ' as usize] = -1;
    t
}

/// `php_base64_encode_ex`: standard alphabet, `=` padding unless `flags`
/// contains [`PHP_BASE64_NO_PADDING`].
pub fn php_base64_encode_ex(input: &[u8], flags: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    let mut rest = input;
    while rest.len() >= 3 {
        let (c, tail) = rest.split_at(3);
        rest = tail;
        out.push(BASE64_TABLE[(c[0] >> 2) as usize]);
        out.push(BASE64_TABLE[(((c[0] & 0x03) << 4) + (c[1] >> 4)) as usize]);
        out.push(BASE64_TABLE[(((c[1] & 0x0f) << 2) + (c[2] >> 6)) as usize]);
        out.push(BASE64_TABLE[(c[2] & 0x3f) as usize]);
    }
    let pad = flags & PHP_BASE64_NO_PADDING == 0;
    match *rest {
        [a, b] => {
            out.push(BASE64_TABLE[(a >> 2) as usize]);
            out.push(BASE64_TABLE[(((a & 0x03) << 4) + (b >> 4)) as usize]);
            out.push(BASE64_TABLE[((b & 0x0f) << 2) as usize]);
            if pad {
                out.push(BASE64_PAD);
            }
        }
        [a] => {
            out.push(BASE64_TABLE[(a >> 2) as usize]);
            out.push(BASE64_TABLE[((a & 0x03) << 4) as usize]);
            if pad {
                out.push(BASE64_PAD);
                out.push(BASE64_PAD);
            }
        }
        _ => {}
    }
    out
}

/// `php_base64_encode`: padded standard base64.
pub fn php_base64_encode(input: &[u8]) -> Vec<u8> {
    php_base64_encode_ex(input, 0)
}

/// `php_base64_decode_ex` (via `php_base64_decode_impl`). Returns `None`
/// where php-src returns `NULL` (only possible in strict mode).
pub fn php_base64_decode_ex(input: &[u8], strict: bool) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 4 + 1);
    // `i` counts the digits consumed, `padding` the `=` seen so far, `partial`
    // is the byte under construction (`out[j]` in C, not yet part of the output).
    let mut i = 0usize;
    let mut padding = 0usize;
    let mut partial = 0u8;

    for &byte in input {
        if byte == BASE64_PAD {
            padding += 1;
            continue;
        }

        let ch = BASE64_REVERSE_TABLE[byte as usize];
        if !strict {
            // Skip unknown characters and whitespace.
            if ch < 0 {
                continue;
            }
        } else {
            // Skip whitespace.
            if ch == -1 {
                continue;
            }
            // Fail on bad characters or if any data follows padding.
            if ch == -2 || padding != 0 {
                return None;
            }
        }
        let ch = ch as u8;

        match i % 4 {
            0 => partial = ch << 2,
            1 => {
                out.push(partial | (ch >> 4));
                partial = (ch & 0x0f) << 4;
            }
            2 => {
                out.push(partial | (ch >> 2));
                partial = (ch & 0x03) << 6;
            }
            _ => out.push(partial | ch),
        }
        i += 1;
    }

    // Fail if the input is truncated (only one char in last group).
    if strict && i % 4 == 1 {
        return None;
    }

    // Fail if the padding length is wrong (not VV==, VVV=), but accept zero
    // padding (RFC 4648: padding is not required in some circumstances).
    if strict && padding != 0 && (padding > 2 || (i + padding) & 3 != 0) {
        return None;
    }

    Some(out)
}

/// `php_base64_decode`: lenient decode.
pub fn php_base64_decode(input: &[u8]) -> Vec<u8> {
    php_base64_decode_ex(input, false).unwrap_or_default()
}

/// Encodes string using MIME base64 algorithm.
fn base64_encode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(php_base64_encode(
        args.str(0),
    ))))
}

/// Decodes string using MIME base64 algorithm.
fn base64_decode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let strict = args.bool(1);
    Ok(match php_base64_decode_ex(args.str(0), strict) {
        Some(out) => Value::String(PhpStr::from_vec(out)),
        None => Value::Bool(false),
    })
}

pub static BASE64_ENCODE: NativeFunction = NativeFunction {
    sig: "base64_encode(string $string): string",
    handler: base64_encode,
};

pub static BASE64_DECODE: NativeFunction = NativeFunction {
    sig: "base64_decode(string $string, bool $strict = false): string|false",
    handler: base64_decode,
};

pub fn register(r: &mut Registry) {
    r.function(&BASE64_ENCODE);
    r.function(&BASE64_DECODE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn enc(t: &mut TestVm, s: &[u8]) -> Vec<u8> {
        match t.call("base64_encode", vec![Value::bytes(s)]).unwrap() {
            Value::String(s) => s.to_vec(),
            v => panic!("{v:?}"),
        }
    }

    fn dec(t: &mut TestVm, s: &[u8], strict: Option<bool>) -> Option<Vec<u8>> {
        let mut args = vec![Value::bytes(s)];
        if let Some(b) = strict {
            args.push(Value::Bool(b));
        }
        match t.call("base64_decode", args).unwrap() {
            Value::String(s) => Some(s.to_vec()),
            Value::Bool(false) => None,
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn encode_single_bytes() {
        // url/base64_encode_basic_001.phpt
        let mut t = TestVm::new();
        let cases: &[(u8, &str)] = &[
            (0x00, "AA=="),
            (0x01, "AQ=="),
            (0x0A, "Cg=="),
            (0x3E, "Pg=="),
            (0x3F, "Pw=="),
            (0x7F, "fw=="),
            (0x80, "gA=="),
            (0xC6, "xg=="),
            (0xF8, "+A=="),
            (0xFB, "+w=="),
            (0xFC, "/A=="),
            (0xFF, "/w=="),
        ];
        for &(b, e) in cases {
            assert_eq!(enc(&mut t, &[b]), e.as_bytes(), "0x{b:X}");
        }
    }

    #[test]
    fn encode_values() {
        let mut t = TestVm::new();
        let cases: &[(&[u8], &str)] = &[
            (b"Hello World", "SGVsbG8gV29ybGQ="),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890!%^&*(){}[]",
                "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVoxMjM0NTY3ODkwISVeJiooKXt9W10=",
            ),
            (
                b"\n\t Line with control characters\r\n",
                "CgkgTGluZSB3aXRoIGNvbnRyb2wgY2hhcmFjdGVycw0K",
            ),
            (b"\xC1\xC2\xC3\xC4\xC5\xC6", "wcLDxMXG"),
            (
                b"\x3d\x3e\x3f\x07\x38\x07\x39\x5c\x38\x30",
                "PT4/BzgHOVw4MA==",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789%!",
                "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODklIQ==",
            ),
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ];
        for &(i, e) in cases {
            assert_eq!(enc(&mut t, i), e.as_bytes());
        }
    }

    #[test]
    fn encode_no_padding_flag() {
        assert_eq!(php_base64_encode_ex(b"f", PHP_BASE64_NO_PADDING), b"Zg");
        assert_eq!(php_base64_encode_ex(b"fo", PHP_BASE64_NO_PADDING), b"Zm8");
        assert_eq!(php_base64_encode_ex(b"foo", PHP_BASE64_NO_PADDING), b"Zm9v");
        assert_eq!(php_base64_encode_ex(b"f", 0), b"Zg==");
    }

    #[test]
    fn decode_alphabet() {
        // url/base64_decode_basic_001.phpt
        let all = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/VV==";
        let hex = "00108310518720928b30d38f41149351559761969b71d79f8218a39259a7a29aabb2dbafc31cb3d35db7e39ebbf3dfbf55";
        let expect: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let mut t = TestVm::new();
        assert_eq!(dec(&mut t, all, None), Some(expect.clone()));
        assert_eq!(dec(&mut t, all, Some(false)), Some(expect.clone()));
        assert_eq!(dec(&mut t, all, Some(true)), Some(expect));
    }

    #[test]
    fn decode_strict_vs_lenient() {
        // url/base64_decode_basic_002.phpt
        let mut t = TestVm::new();
        let plain = b"aGVsbG8gd29ybGQh";
        for s in [None, Some(false), Some(true)] {
            assert_eq!(dec(&mut t, plain, s), Some(b"hello world!".to_vec()));
        }
        let ws = b"a GVs   bG8gd2\n                        9ybGQh";
        for s in [None, Some(false), Some(true)] {
            assert_eq!(dec(&mut t, ws, s), Some(b"hello world!".to_vec()));
        }
        let mut bad = plain.to_vec();
        bad.push(b'*');
        assert_eq!(dec(&mut t, &bad, None), Some(b"hello world!".to_vec()));
        assert_eq!(
            dec(&mut t, &bad, Some(false)),
            Some(b"hello world!".to_vec())
        );
        assert_eq!(dec(&mut t, &bad, Some(true)), None);
    }

    #[test]
    fn decode_padding_table() {
        // url/base64_decode_basic_003.phpt: (input, non-strict, strict)
        let cases: &[(&[u8], &str, Option<&str>)] = &[
            (b"", "", Some("")),
            (b"=", "", None),
            (b"==", "", None),
            (b"===", "", None),
            (b"====", "", None),
            (b"V", "", None),
            (b"V=", "", None),
            (b"V==", "", None),
            (b"V===", "", None),
            (b"V====", "", None),
            (b"VV", "U", Some("U")),
            (b"VV=", "U", None),
            (b"VV==", "U", Some("U")),
            (b"VV===", "U", None),
            (b"VV====", "U", None),
            (b"VVV", "UU", Some("UU")),
            (b"VVV=", "UU", Some("UU")),
            (b"VVV==", "UU", None),
            (b"VVV===", "UU", None),
            (b"VVV====", "UU", None),
            (b"VVVV", "UUU", Some("UUU")),
            (b"VVVV=", "UUU", None),
            (b"VVVV==", "UUU", None),
            (b"VVVV===", "UUU", None),
            (b"VVVV====", "UUU", None),
            (b"=V", "", None),
            (b"=VV", "U", None),
            (b"=VVV", "UU", None),
            (b"==V", "", None),
            (b"==VV", "U", None),
            (b"==VVV", "UU", None),
            (b"===V", "", None),
            (b"===VV", "U", None),
            (b"===VVV", "UU", None),
            (b"====V", "", None),
            (b"====VV", "U", None),
            (b"====VVV", "UU", None),
            (b"V=VV", "UU", None),
            (b"VV=V", "UU", None),
            (b"=VVVV", "UUU", None),
            (b"V=VVV", "UUU", None),
            (b"VV=VV", "UUU", None),
            (b"VVV=V", "UUU", None),
            (b"=VVV=", "UU", None),
            (b"V=VV=", "UU", None),
            (b"VV=V=", "UU", None),
            (b"\nVV", "U", Some("U")),
            (b"V\nV", "U", Some("U")),
            (b"VV\n", "U", Some("U")),
            (b"\nVV==", "U", Some("U")),
            (b"V\nV==", "U", Some("U")),
            (b"VV\n==", "U", Some("U")),
            (b"VV=\n=", "U", Some("U")),
            (b"VV==\n", "U", Some("U")),
            (b"*VV", "U", None),
            (b"V*V", "U", None),
            (b"VV*", "U", None),
            (b"*VV==", "U", None),
            (b"V*V==", "U", None),
            (b"VV*==", "U", None),
            (b"VV=*=", "U", None),
            (b"VV==*", "U", None),
            (b"\0VV==", "U", None),
            (b"V\0V==", "U", None),
            (b"VV\0==", "U", None),
            (b"VV=\0=", "U", None),
            (b"VV==\0", "U", None),
            (b"\0VVV==", "UU", None),
            (b"V\0VV==", "UU", None),
            (b"VV\0V==", "UU", None),
            (b"VVV\0==", "UU", None),
            (b"VVV=\0=", "UU", None),
            (b"VVV==\0", "UU", None),
        ];
        for &(input, lenient, strict) in cases {
            assert_eq!(
                php_base64_decode_ex(input, false),
                Some(lenient.as_bytes().to_vec()),
                "non-strict {input:?}"
            );
            assert_eq!(
                php_base64_decode_ex(input, true),
                strict.map(|s| s.as_bytes().to_vec()),
                "strict {input:?}"
            );
        }
    }

    #[test]
    fn decode_bugs() {
        let mut t = TestVm::new();
        // bug72263
        assert_eq!(dec(&mut t, b"*", Some(true)), None);
        assert_eq!(dec(&mut t, b"=*", Some(true)), None);
        assert_eq!(dec(&mut t, b"VVV=", Some(true)), Some(b"UU".to_vec()));
        assert_eq!(dec(&mut t, b"VVV=*", Some(true)), None);
        // bug72264
        assert_eq!(dec(&mut t, b"VV= =", Some(true)), Some(b"U".to_vec()));
        // bug72152
        assert_eq!(dec(&mut t, b"\x00", Some(true)), None);
        assert_eq!(dec(&mut t, b"\x00VVVV", Some(true)), None);
        assert_eq!(dec(&mut t, b"VVVV\x00", Some(true)), None);
        // bug34214
        assert_eq!(
            dec(&mut t, b"Zm9vYmFyIG\n Zvb2Jhcg==", None),
            Some(b"foobar foobar".to_vec())
        );
        // bug47174: pad char in the middle does not terminate (lenient)
        assert_ne!(
            dec(&mut t, b"dGVzdA==", None),
            dec(&mut t, b"dGVzdA==CRAP", None)
        );
        let input = [enc(&mut t, b"foo"), b"==".to_vec(), enc(&mut t, b"bar")].concat();
        assert_eq!(input, b"Zm9v==YmFy");
        assert_eq!(dec(&mut t, &input, None), Some(b"foobar".to_vec()));
        // bug52327
        let s = b"=VGhl=ICc9=JyBz=eW1i=b2xz=IGFy=ZW4n=dCBh=bGxv=d2Vk=IHdo=ZXJl=IGkg=cHV0=IHRo=ZW0g=by5P";
        assert_eq!(
            dec(&mut t, s, None),
            Some(b"The '=' symbols aren't allowed where i put them o.O".to_vec())
        );
        assert_eq!(dec(&mut t, s, Some(true)), None);
        // bug55273: whitespace after the pad in strict mode
        for (raw, enc_s) in [
            (&b"PHP"[..], &b"UEhQ\n"[..]),
            (b"PH", b"UEg=\n"),
            (b"P", b"UA==\n"),
        ] {
            assert_eq!(dec(&mut t, enc_s, Some(true)), Some(raw.to_vec()));
        }
        // bug24312: 0xF0..=0xFF are skipped like any invalid byte
        for c in 0xF0u8..=0xFF {
            let mut s = Vec::new();
            for chunk in enc(&mut t, &[b'a'; 100]).chunks(10) {
                s.extend_from_slice(chunk);
                s.push(c);
            }
            assert_eq!(dec(&mut t, &s, None), Some(vec![b'a'; 100]));
        }
    }

    #[test]
    fn round_trip_loop() {
        // url/base64_loop_001.phpt
        let mut s = Vec::new();
        for i in 0..1024usize {
            s.push(i as u8);
            let e = php_base64_encode(&s);
            assert_eq!(php_base64_decode(&e), s);
            assert_eq!(php_base64_decode_ex(&e, true), Some(s.clone()));
        }
    }

    #[test]
    fn decode_long_string() {
        let mut t = TestVm::new();
        let plain = b"UEhQIGlzIGEgcG9wdWxhciBnZW5lcmFsLXB1cnBvc2Ugc2NyaXB0aW5nIGxhbmd1YWdlIHRoYXQgaXMgZXNwZWNpYWxseSBzdWl0ZWQgdG8gd2ViIGRldmVsb3BtZW50";
        let want = b"PHP is a popular general-purpose scripting language that is especially suited to web development".to_vec();
        assert_eq!(dec(&mut t, plain, Some(true)), Some(want.clone()));
        let ws = b"UEhQIGlzIGE gcG9wdWxhciBnZW5lcmFsLXB1cnBvc2Ugc2NyaXB0aW5nIGxhbmd1YWdl IHRoYXQga\n                        XMgZXNwZWNpYWxseSBzdWl0ZWQgdG8gd2ViIGRldmVsb3BtZW50";
        assert_eq!(dec(&mut t, ws, Some(true)), Some(want.clone()));
        let mut bad = plain.to_vec();
        bad.push(b'*');
        assert_eq!(dec(&mut t, &bad, None), Some(want));
        assert_eq!(dec(&mut t, &bad, Some(true)), None);
    }
}
