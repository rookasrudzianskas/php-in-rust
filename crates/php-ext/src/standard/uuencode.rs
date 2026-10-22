//! ext/standard/uuencode.c
//!
//! Portions of the original are based on Berkeley's uuencode/uudecode.

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// `PHP_UU_ENC`: a zero 6-bit value is written as a backtick, not a space.
fn uu_enc(c: usize) -> u8 {
    if c != 0 {
        (c & 0o77) as u8 + b' '
    } else {
        b'`'
    }
}

/// `PHP_UU_DEC`
fn uu_dec(c: u8) -> u8 {
    c.wrapping_sub(b' ') & 0o77
}

/// `php_uuencode`.
pub fn php_uuencode(src: &[u8]) -> Vec<u8> {
    // The C code reads one byte past the input (the NUL terminator of the
    // zend_string) in a few places; model it with a zero byte.
    let at = |i: usize| src.get(i).copied().unwrap_or(0) as usize;

    let mut len = 45usize;
    let e = src.len();
    let mut s = 0usize;
    let mut out = Vec::with_capacity(src.len() / 2 * 3 + 46);

    while s + 3 < e {
        let mut ee = s + len;
        if ee > e {
            ee = e;
            len = ee - s;
            // Whole triples only; a 1-2 byte tail is encoded after the loop.
            ee = s + (len / 3) * 3;
        }
        out.push(uu_enc(len));

        while s < ee {
            out.push(uu_enc(at(s) >> 2));
            out.push(uu_enc(((at(s) << 4) & 0o60) | ((at(s + 1) >> 4) & 0o17)));
            out.push(uu_enc(((at(s + 1) << 2) & 0o74) | ((at(s + 2) >> 6) & 0o3)));
            out.push(uu_enc(at(s + 2) & 0o77));
            s += 3;
        }

        if len == 45 {
            out.push(b'\n');
        }
    }

    if s < e {
        if len == 45 {
            out.push(uu_enc(e - s));
            len = 0;
        }

        out.push(uu_enc(at(s) >> 2));
        out.push(uu_enc(((at(s) << 4) & 0o60) | ((at(s + 1) >> 4) & 0o17)));
        out.push(if e - s > 1 {
            uu_enc(((at(s + 1) << 2) & 0o74) | ((at(s + 2) >> 6) & 0o3))
        } else {
            uu_enc(0)
        });
        out.push(if e - s > 2 {
            uu_enc(at(s + 2) & 0o77)
        } else {
            uu_enc(0)
        });
    }

    if len < 45 {
        out.push(b'\n');
    }

    out.push(uu_enc(0));
    out.push(b'\n');
    out
}

/// `php_uudecode`. `None` is php-src's `NULL` (empty or malformed input).
pub fn php_uudecode(src: &[u8]) -> Option<Vec<u8>> {
    if src.is_empty() {
        return None;
    }
    // Reads past the end see the NUL terminator, as in C.
    let at = |i: usize| src.get(i).copied().unwrap_or(0);

    let e = src.len();
    let mut s = 0usize;
    let mut total_len = 0usize;
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 3 / 4 + 1);

    while s < e {
        let len = uu_dec(src[s]) as usize;
        s += 1;
        if len == 0 {
            break;
        }
        // sanity check
        if len > src.len() {
            return None;
        }

        total_len += len;

        let ee = s + if len == 45 {
            60
        } else {
            (len as f64 * 1.33).floor() as usize
        };
        // sanity check
        if ee > e {
            return None;
        }

        while s < ee {
            if s + 4 > e {
                return None;
            }
            out.push(uu_dec(src[s]) << 2 | uu_dec(src[s + 1]) >> 4);
            out.push(uu_dec(src[s + 1]) << 4 | uu_dec(src[s + 2]) >> 2);
            out.push(uu_dec(src[s + 2]) << 6 | uu_dec(src[s + 3]));
            s += 4;
        }

        if len < 45 {
            break;
        }

        // skip \n
        s += 1;
    }

    if total_len > out.len() {
        out.push(uu_dec(at(s)) << 2 | uu_dec(at(s + 1)) >> 4);
        if total_len > 1 {
            out.push(uu_dec(at(s + 1)) << 4 | uu_dec(at(s + 2)) >> 2);
            if total_len > 2 {
                out.push(uu_dec(at(s + 2)) << 6 | uu_dec(at(s + 3)));
            }
        }
    }

    // `ZSTR_LEN(dest) = total_len`: the last group may have decoded padding.
    out.resize(total_len, 0);
    Some(out)
}

/// uuencode a string.
fn convert_uuencode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(php_uuencode(args.str(0)))))
}

/// decode a uuencoded string.
fn convert_uudecode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match php_uudecode(args.str(0)) {
        Some(out) => Ok(Value::String(PhpStr::from_vec(out))),
        None => {
            vm.warning("Argument #1 ($string) is not a valid uuencoded string")?;
            Ok(Value::Bool(false))
        }
    }
}

pub static CONVERT_UUENCODE: NativeFunction = NativeFunction {
    sig: "convert_uuencode(string $string): string",
    handler: convert_uuencode,
};

pub static CONVERT_UUDECODE: NativeFunction = NativeFunction {
    sig: "convert_uudecode(string $string): string|false",
    handler: convert_uudecode,
};

pub fn register(r: &mut Registry) {
    r.function(&CONVERT_UUENCODE);
    r.function(&CONVERT_UUDECODE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn encode(t: &mut TestVm, s: &[u8]) -> Vec<u8> {
        match t.call("convert_uuencode", vec![Value::bytes(s)]).unwrap() {
            Value::String(s) => s.to_vec(),
            v => panic!("{v:?}"),
        }
    }

    fn decode(t: &mut TestVm, s: &[u8]) -> Option<Vec<u8>> {
        match t.call("convert_uudecode", vec![Value::bytes(s)]).unwrap() {
            Value::String(s) => Some(s.to_vec()),
            Value::Bool(false) => None,
            v => panic!("{v:?}"),
        }
    }

    const WARNING: &str = "\nWarning: convert_uudecode(): Argument #1 ($string) is not a valid uuencoded string in /t.php on line 3\n";

    #[test]
    fn encode_basic() {
        // convert_uuencode_basic.phpt
        let mut t = TestVm::new();
        let cases: &[(&[u8], &[u8])] = &[
            (b"123", b"#,3(S\n`\n"),
            (b"abc", b"#86)C\n`\n"),
            (b"1a2b3c", b"&,6$R8C-C\n`\n"),
            (
                b"Here is a simple string to test convert_uuencode/decode",
                b"M2&5R92!I<R!A('-I;7!L92!S=')I;F<@=&\\@=&5S=\"!C;VYV97)T7W5U96YC\n*;V1E+V1E8V]D90``\n`\n",
            ),
            (
                b"\t This String contains \t\t some control characters\r\n",
                b"M\"2!4:&ES(%-T<FEN9R!C;VYT86EN<R`)\"2!S;VUE(&-O;G1R;VP@8VAA<F%C\n&=&5R<PT*\n`\n",
            ),
            (
                b"\x90\x91\x00\x93\x94\x90\x91\x95\x96\x97\x98\x99\x9a\x9b\x9c\x9d\x9e\x9f",
                b"2D)$`DY20D966EYB9FIN<G9Z?\n`\n",
            ),
            (
                b"\\t This String contains \\t\\t some control characters\\r\\n",
                b"M7'0@5&AI<R!3=')I;F<@8V]N=&%I;G,@7'1<=\"!S;VUE(&-O;G1R;VP@8VAA\n+<F%C=&5R<UQR7&X`\n`\n",
            ),
        ];
        for &(i, o) in cases {
            assert_eq!(encode(&mut t, i), o, "{:?}", String::from_utf8_lossy(i));
        }
    }

    #[test]
    fn round_trip() {
        // convert_uudecode_basic.phpt
        let mut t = TestVm::new();
        let strings: &[&[u8]] = &[
            b"123",
            b"abc",
            b"1a2b3c",
            b"Here is a simple string to test convert_uuencode/decode",
            b"\t This String contains \t\t some control characters\r\n",
            b"\x90\x91\x00\x93\x94\x90\x91\x95\x96\x97\x98\x99\x9a\x9b\x9c\x9d\x9e\x9f",
        ];
        for &s in strings {
            let e = encode(&mut t, s);
            assert_eq!(decode(&mut t, &e), Some(s.to_vec()));
        }
        // Every length around the 45-byte line boundary, with binary data.
        let data: Vec<u8> = (0..400u32).map(|i| (i * 7 + 3) as u8).collect();
        for n in 0..data.len() {
            let e = php_uuencode(&data[..n]);
            assert_eq!(php_uudecode(&e).as_deref(), Some(&data[..n]), "len {n}");
        }
    }

    #[test]
    fn uuencode_phpt() {
        // uuencode.phpt
        let mut t = TestVm::new();
        let enc = encode(&mut t, b"");
        assert_eq!(enc, b"`\n");
        assert_eq!(decode(&mut t, &enc), Some(Vec::new()));

        let enc = encode(&mut t, b"~!@#$%^&*()_}{POIUYTREWQQSDFGHJKL:<MNBVCXZ");
        assert_eq!(
            enc,
            b"J?B%`(R0E7B8J*\"E??7M03TE5651215=145-$1D=(2DM,.CQ-3D)60UA:\n`\n"
        );
        assert_eq!(enc.len(), 60);
        // EXPECTF string(1) "%s": a single decoded byte.
        assert_eq!(
            decode(&mut t, b"!@#$%^YUGFDFGHJKLUYTFBNMLOYT").map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            decode(&mut t, &enc),
            Some(b"~!@#$%^&*()_}{POIUYTREWQQSDFGHJKL:<MNBVCXZ".to_vec())
        );

        let enc = encode(&mut t, b"not very sophisticated");
        assert_eq!(enc, b"6;F]T('9E<GD@<V]P:&ES=&EC871E9```\n`\n");
        assert_eq!(
            decode(&mut t, &enc),
            Some(b"not very sophisticated".to_vec())
        );
        t.output();

        assert_eq!(decode(&mut t, b""), None);
        assert_eq!(t.output(), WARNING);
        assert_eq!(decode(&mut t, &enc[..enc.len() - 10]), None);
        assert_eq!(t.output(), WARNING);
    }

    #[test]
    fn decode_never_underflows_total_len() {
        // The post-loop fallback of php_uudecode relies on every line
        // producing at least `len` bytes.
        for len in 1usize..=63 {
            let ee = if len == 45 {
                60
            } else {
                (len as f64 * 1.33).floor() as usize
            };
            assert!(ee.div_ceil(4) * 3 >= len, "len {len}");
        }
    }
}
