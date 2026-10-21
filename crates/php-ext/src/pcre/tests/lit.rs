//! PHP string literals and array access for transliterating phpt scripts.

#![allow(dead_code)]

use php_runtime::{ArrayKey, Value};

/// The value of a PHP single-quoted string literal whose source text (without
/// the quotes) is `src`: only `\\` and `\'` are escapes. Lets the tests paste
/// the phpt's source verbatim into a Rust raw string.
pub fn sq_bytes(src: &str) -> Vec<u8> {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && matches!(b.get(i + 1), Some(b'\\' | b'\'')) {
            i += 1;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// A PHP single-quoted string literal (see [`sq_bytes`]).
pub fn sq(src: &str) -> Value {
    Value::from(sq_bytes(src))
}

/// The value of a PHP double-quoted string literal whose source text
/// (without the quotes and without variable interpolation) is `src`.
pub fn dq_bytes(src: &str) -> Vec<u8> {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let c = b[i + 1];
        i += 2;
        match c {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'v' => out.push(0x0b),
            b'e' => out.push(0x1b),
            b'f' => out.push(0x0c),
            b'\\' | b'$' | b'"' => out.push(c),
            b'0'..=b'7' => {
                let mut v = (c - b'0') as u32;
                for _ in 0..2 {
                    match b.get(i) {
                        Some(d @ b'0'..=b'7') => {
                            v = v * 8 + (d - b'0') as u32;
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(v as u8);
            }
            b'x' if b.get(i).is_some_and(u8::is_ascii_hexdigit) => {
                let mut v = 0u32;
                for _ in 0..2 {
                    match b.get(i) {
                        Some(d) if d.is_ascii_hexdigit() => {
                            v = v * 16 + (*d as char).to_digit(16).unwrap();
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(v as u8);
            }
            b'u' if b.get(i) == Some(&b'{') => {
                let end = src[i..].find('}').expect("unterminated \\u{...}") + i;
                let cp = u32::from_str_radix(&src[i + 1..end], 16).unwrap();
                let mut buf = [0u8; 4];
                let ch = char::from_u32(cp).unwrap();
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                i = end + 1;
            }
            _ => {
                out.push(b'\\');
                out.push(c);
            }
        }
    }
    out
}

/// A PHP double-quoted string literal (see [`dq_bytes`]).
pub fn dq(src: &str) -> Value {
    Value::from(dq_bytes(src))
}

/// `$v[$i]`
pub fn idx(v: &Value, i: i64) -> Value {
    match v.unref() {
        Value::Array(a) => a.get_int(i).map(Value::unref).unwrap_or(Value::Null),
        other => panic!("not an array: {other:?}"),
    }
}

/// `$v['key']`
pub fn key(v: &Value, k: &str) -> Value {
    match v.unref() {
        Value::Array(a) => a
            .get(&ArrayKey::from(k))
            .map(Value::unref)
            .unwrap_or(Value::Null),
        other => panic!("not an array: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        assert_eq!(sq_bytes(r"a\'b\\c\d"), b"a'b\\c\\d");
        assert_eq!(
            dq_bytes(r#"a\n\"\\\x41\101\u{20ac}\s\$"#),
            "a\n\"\\AA\u{20ac}\\s$".as_bytes()
        );
        assert_eq!(dq_bytes(r"\\\\\\1"), b"\\\\\\1");
    }
}
