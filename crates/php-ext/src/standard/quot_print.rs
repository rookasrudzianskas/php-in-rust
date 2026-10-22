//! ext/standard/quot_print.c

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// `PHP_QPRINT_MAXL`
const PHP_QPRINT_MAXL: usize = 75;

/// `hexval_tbl` of `php_quot_print_decode`: 0-15 for hex digits, 32 for
/// space and tab, 16 for CR and LF, 64 for everything else.
const HEXVAL_TBL: [u8; 256] = build_hexval_table();

const fn build_hexval_table() -> [u8; 256] {
    let mut t = [64u8; 256];
    let mut i = 0;
    while i < 10 {
        t[b'0' as usize + i] = i as u8;
        i += 1;
    }
    let mut i = 0;
    while i < 6 {
        t[b'A' as usize + i] = 10 + i as u8;
        t[b'a' as usize + i] = 10 + i as u8;
        i += 1;
    }
    t[b'\t' as usize] = 32;
    t[b' ' as usize] = 32;
    t[b'\n' as usize] = 16;
    t[b'\r' as usize] = 16;
    t
}

/// `php_quot_print_decode`. Stops at the first NUL byte; `None` is php-src's
/// `NULL` (an `=` followed by something that is neither two hex digits nor a
/// soft line break).
pub fn php_quot_print_decode(src: &[u8], replace_us_by_ws: bool) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(src.len());
    let n = src.len();
    // `i` in the C code counts the remaining bytes; `p` is the cursor.
    let mut p = 0usize;

    while p < n && src[p] != 0 {
        if src[p] == b'=' {
            p += 1;
            if p == n || src[p] == 0 {
                break;
            }
            let mut h_nbl = HEXVAL_TBL[src[p] as usize];
            if h_nbl < 16 {
                // next char should be a hexadecimal digit
                p += 1;
                if p == n {
                    return None;
                }
                let l_nbl = HEXVAL_TBL[src[p] as usize];
                if l_nbl >= 16 {
                    return None;
                }
                out.push((h_nbl << 4) | l_nbl);
                p += 1;
            } else if h_nbl < 64 {
                // soft line break
                while h_nbl == 32 {
                    p += 1;
                    if p == n {
                        return None;
                    }
                    h_nbl = HEXVAL_TBL[src[p] as usize];
                    if h_nbl == 64 {
                        return None;
                    }
                }
                if src[p] == b'\r' && p + 1 < n && src[p + 1] == b'\n' {
                    p += 1;
                }
                p += 1;
            } else {
                return None;
            }
        } else {
            out.push(if replace_us_by_ws && src[p] == b'_' {
                b' '
            } else {
                src[p]
            });
            p += 1;
        }
    }

    Some(out)
}

/// `php_quot_print_encode`.
pub fn php_quot_print_encode(src: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    // `*str` after the cursor: the NUL terminator at the end of the string.
    let at = |i: usize| src.get(i).copied().unwrap_or(0);

    let mut lp = 0usize;
    let mut out = Vec::with_capacity(src.len() + src.len() / 2);
    let mut i = 0usize;

    while i < src.len() {
        let c = src[i];
        i += 1;
        if c == b'\r' && at(i) == b'\n' && i < src.len() {
            out.push(b'\r');
            out.push(b'\n');
            i += 1;
            lp = 0;
        } else if c < 0x20
            || c == 0x7f
            || c & 0x80 != 0
            || c == b'='
            || (c == b' ' && at(i) == b'\r')
        {
            lp += 3;
            if (lp > PHP_QPRINT_MAXL && c <= 0x7f)
                || (c > 0x7f && c <= 0xdf && lp + 3 > PHP_QPRINT_MAXL)
                || (c > 0xdf && c <= 0xef && lp + 6 > PHP_QPRINT_MAXL)
                || (c > 0xef && c <= 0xf4 && lp + 9 > PHP_QPRINT_MAXL)
            {
                out.extend_from_slice(b"=\r\n");
                lp = 3;
            }
            out.push(b'=');
            out.push(HEX[(c >> 4) as usize]);
            out.push(HEX[(c & 0xf) as usize]);
        } else {
            lp += 1;
            if lp > PHP_QPRINT_MAXL {
                out.extend_from_slice(b"=\r\n");
                lp = 1;
            }
            out.push(c);
        }
    }
    out
}

fn is_xdigit(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

fn hex2int(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'A'..=b'F' => c - b'A' + 10,
        _ => c - b'a' + 10,
    }
}

/// Convert a quoted-printable string to an 8 bit string.
fn quoted_printable_decode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let src = args.str(0);
    // Bytes past the end read as the NUL terminator, which also ends the loop.
    let at = |i: usize| src.get(i).copied().unwrap_or(0);

    let mut out = Vec::with_capacity(src.len());
    let mut i = 0usize;
    while at(i) != 0 {
        if at(i) != b'=' {
            out.push(src[i]);
            i += 1;
            continue;
        }
        if at(i + 1) != 0 && at(i + 2) != 0 && is_xdigit(at(i + 1)) && is_xdigit(at(i + 2)) {
            out.push((hex2int(at(i + 1)) << 4) + hex2int(at(i + 2)));
            i += 3;
        } else {
            // check for soft line break according to RFC 2045
            let mut k = 1;
            while at(i + k) != 0 && (at(i + k) == 32 || at(i + k) == 9) {
                // Possibly, skip spaces/tabs at the end of line
                k += 1;
            }
            if at(i + k) == 0 {
                // End of line reached
                i += k;
            } else if at(i + k) == 13 && at(i + k + 1) == 10 {
                // CRLF
                i += k + 2;
            } else if at(i + k) == 13 || at(i + k) == 10 {
                // CR or LF
                i += k + 1;
            } else {
                out.push(src[i]);
                i += 1;
            }
        }
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

fn quoted_printable_encode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let src = args.str(0);
    if src.is_empty() {
        return Ok(Value::String(PhpStr::from_vec(Vec::new())));
    }
    Ok(Value::String(PhpStr::from_vec(php_quot_print_encode(src))))
}

pub static QUOTED_PRINTABLE_DECODE: NativeFunction = NativeFunction {
    sig: "quoted_printable_decode(string $string): string",
    handler: quoted_printable_decode,
};

pub static QUOTED_PRINTABLE_ENCODE: NativeFunction = NativeFunction {
    sig: "quoted_printable_encode(string $string): string",
    handler: quoted_printable_encode,
};

pub fn register(r: &mut Registry) {
    r.function(&QUOTED_PRINTABLE_DECODE);
    r.function(&QUOTED_PRINTABLE_ENCODE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn call(t: &mut TestVm, f: &str, s: &[u8]) -> Vec<u8> {
        match t.call(f, vec![Value::bytes(s)]).unwrap() {
            Value::String(s) => s.to_vec(),
            v => panic!("{v:?}"),
        }
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn decode_basic() {
        // quoted_printable_decode_basic.phpt
        let mut t = TestVm::new();
        let s = b"=FAwow-factor=C1=d0=D5=DD=C5=CE=CE=D9=C5=0A=\n=20=D4=cf=D2=C7=CF=D7=D9=C5=\n=20=\n=D0=\n=D2=CF=C5=CB=D4=D9";
        assert_eq!(
            hex(&call(&mut t, "quoted_printable_decode", s)),
            "fa776f772d666163746f72c1d0d5ddc5ceced9c50a20d4cfd2c7cfd7d9c520d0d2cfc5cbd4d9"
        );
    }

    #[test]
    fn decode_soft_breaks_and_invalid() {
        let mut t = TestVm::new();
        let d = |t: &mut TestVm, s: &[u8]| call(t, "quoted_printable_decode", s);
        assert_eq!(d(&mut t, b""), b"");
        assert_eq!(d(&mut t, b"a=\r\nb"), b"ab");
        assert_eq!(d(&mut t, b"a=\nb"), b"ab");
        assert_eq!(d(&mut t, b"a=\rb"), b"ab");
        assert_eq!(d(&mut t, b"a= \t\r\nb"), b"ab");
        // `=` at the end of the string or before an invalid sequence stays.
        assert_eq!(d(&mut t, b"a="), b"a");
        assert_eq!(d(&mut t, b"a=x"), b"a=x");
        assert_eq!(d(&mut t, b"a=4"), b"a=4");
        assert_eq!(d(&mut t, b"a=4x"), b"a=4x");
        assert_eq!(d(&mut t, b"=3D=3d"), b"==");
        assert_eq!(d(&mut t, b"a_b"), b"a_b");
        // decoding stops at a NUL byte
        assert_eq!(d(&mut t, b"ab\0cd"), b"ab");
        // an encoded NUL is kept
        assert_eq!(d(&mut t, b"a=00b"), b"a\0b");
    }

    #[test]
    fn encode_basic() {
        // quoted_printable_encode_001.phpt
        let mut t = TestVm::new();
        let e = |t: &mut TestVm, s: &[u8]| call(t, "quoted_printable_encode", s);
        assert_eq!(e(&mut t, b""), b"");
        assert_eq!(e(&mut t, b"test"), b"test");
        assert_eq!(e(&mut t, b"1"), b"1");
        assert_eq!(e(&mut t, b"a=b"), b"a=3Db");
        assert_eq!(e(&mut t, b"a\r\nb"), b"a\r\nb");
        assert_eq!(e(&mut t, b"a\nb"), b"a=0Ab");
        assert_eq!(e(&mut t, b"a \r\nb"), b"a=20\r\nb");
        assert_eq!(e(&mut t, b"tail\r"), b"tail=0D");
        assert_eq!(e(&mut t, b"\x7f\xc3\xa9"), b"=7F=C3=A9");
    }

    #[test]
    fn encode_long_lines() {
        // quoted_printable_encode_002.phpt: 200 NULs
        let mut t = TestVm::new();
        let d = call(&mut t, "quoted_printable_encode", &[0u8; 200]);
        assert_eq!(d.len(), 621);
        let text = String::from_utf8(d.clone()).unwrap();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0], format!("{}=", "=00".repeat(25)));
        assert_eq!(lines[6], format!("{}=", "=00".repeat(25)));
        assert_eq!(lines[7], "=00".repeat(25));
        assert_eq!(call(&mut t, "quoted_printable_decode", &d), vec![0u8; 200]);

        // The same test with a multibyte string (never split in the middle).
        let s = "строка в юникоде".repeat(50);
        let d = call(&mut t, "quoted_printable_encode", s.as_bytes());
        assert_eq!(d.len(), 4480);
        let text = String::from_utf8(d.clone()).unwrap();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "=D1=81=D1=82=D1=80=D0=BE=D0=BA=D0=B0 =D0=B2 =D1=8E=D0=BD=D0=B8=D0=BA=D0="
        );
        assert_eq!(
            lines[1],
            "=BE=D0=B4=D0=B5=D1=81=D1=82=D1=80=D0=BE=D0=BA=D0=B0 =D0=B2 =D1=8E=D0=BD="
        );
        assert_eq!(
            lines.last().unwrap(),
            &"=8E=D0=BD=D0=B8=D0=BA=D0=BE=D0=B4=D0=B5"
        );
        assert_eq!(call(&mut t, "quoted_printable_decode", &d), s.as_bytes());
    }

    #[test]
    fn encode_does_not_split_multibyte() {
        // bug62462.phpt
        let mut t = TestVm::new();
        let d = call(&mut t, "quoted_printable_encode", &b"\xc4\x85".repeat(77));
        let text = String::from_utf8(d).unwrap();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines.len(), 7);
        for l in &lines[..6] {
            assert_eq!(*l, format!("{}=", "=C4=85".repeat(12)));
        }
        assert_eq!(lines[6], "=C4=85".repeat(5));
    }

    #[test]
    fn encode_size_calculation() {
        // bug64879.phpt: must not overflow
        let mut t = TestVm::new();
        let d = call(&mut t, "quoted_printable_encode", &[0xf4u8; 1000]);
        assert!(d.starts_with(b"=F4=F4"));
        call(&mut t, "quoted_printable_encode", &vec![0xf4u8; 100_000]);
    }

    #[test]
    fn decode_helper() {
        assert_eq!(
            php_quot_print_decode(b"a_b=20c", true),
            Some(b"a b c".to_vec())
        );
        assert_eq!(php_quot_print_decode(b"a_b", false), Some(b"a_b".to_vec()));
        assert_eq!(
            php_quot_print_decode(b"a=\r\nb", false),
            Some(b"ab".to_vec())
        );
        assert_eq!(
            php_quot_print_decode(b"a= \r\nb", false),
            Some(b"ab".to_vec())
        );
        assert_eq!(php_quot_print_decode(b"a=4", false), None);
        assert_eq!(php_quot_print_decode(b"a=4x", false), None);
        assert_eq!(php_quot_print_decode(b"a=x", false), None);
        assert_eq!(php_quot_print_decode(b"a=", false), Some(b"a".to_vec()));
    }
}
