//! ext/standard/string.c — `utf8_encode()` / `utf8_decode()` (both deprecated since 8.2).

use php_runtime::class::fn_flags;
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// Decodes the UTF-8 character at `*cursor` and advances the cursor
/// (`get_next_char_utf8` / `php_next_utf8_char` in ext/standard/html.c).
///
/// On an ill-formed sequence returns `None` and skips the bytes that
/// cannot start a valid sequence (strategy 2 of UTR #36 §3.6.1), exactly like
/// php-src. `*cursor` must be before the end of `s`.
pub fn next_utf8_char(s: &[u8], cursor: &mut usize) -> Option<u32> {
    let pos = *cursor;
    let avail = s.len() - pos;
    let lead = |c: u8| c < 0x80 || (0xc2..=0xf4).contains(&c);
    let trail = |c: u8| (0x80..=0xbf).contains(&c);
    let fail = |cursor: &mut usize, advance: usize| {
        *cursor = pos + advance;
        None
    };
    let c = s[pos];
    if c < 0x80 {
        *cursor = pos + 1;
        Some(u32::from(c))
    } else if c < 0xc2 {
        fail(cursor, 1)
    } else if c < 0xe0 {
        if avail < 2 {
            return fail(cursor, 1);
        }
        if !trail(s[pos + 1]) {
            return fail(cursor, if lead(s[pos + 1]) { 1 } else { 2 });
        }
        let ch = (u32::from(c & 0x1f) << 6) | u32::from(s[pos + 1] & 0x3f);
        if ch < 0x80 {
            return fail(cursor, 2);
        }
        *cursor = pos + 2;
        Some(ch)
    } else if c < 0xf0 {
        if avail < 3 || !trail(s[pos + 1]) || !trail(s[pos + 2]) {
            let advance = if avail < 2 || lead(s[pos + 1]) {
                1
            } else if avail < 3 || lead(s[pos + 2]) {
                2
            } else {
                3
            };
            return fail(cursor, advance);
        }
        let ch = (u32::from(c & 0x0f) << 12)
            | (u32::from(s[pos + 1] & 0x3f) << 6)
            | u32::from(s[pos + 2] & 0x3f);
        if ch < 0x800 || (0xd800..=0xdfff).contains(&ch) {
            return fail(cursor, 3);
        }
        *cursor = pos + 3;
        Some(ch)
    } else if c < 0xf5 {
        if avail < 4 || !trail(s[pos + 1]) || !trail(s[pos + 2]) || !trail(s[pos + 3]) {
            let advance = if avail < 2 || lead(s[pos + 1]) {
                1
            } else if avail < 3 || lead(s[pos + 2]) {
                2
            } else if avail < 4 || lead(s[pos + 3]) {
                3
            } else {
                4
            };
            return fail(cursor, advance);
        }
        let ch = (u32::from(c & 0x07) << 18)
            | (u32::from(s[pos + 1] & 0x3f) << 12)
            | (u32::from(s[pos + 2] & 0x3f) << 6)
            | u32::from(s[pos + 3] & 0x3f);
        if !(0x10000..=0x10ffff).contains(&ch) {
            return fail(cursor, 4);
        }
        *cursor = pos + 4;
        Some(ch)
    } else {
        fail(cursor, 1)
    }
}

/// `php_utf8_encode`: ISO-8859-1 → UTF-8.
pub fn utf8_encode_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for &c in s {
        if c < 0x80 {
            out.push(c);
        } else {
            out.push(0xc0 | (c >> 6));
            out.push(0x80 | (c & 0x3f));
        }
    }
    out
}

/// `php_utf8_decode`: UTF-8 → ISO-8859-1; ill-formed sequences and
/// characters above U+00FF become `?`.
pub fn utf8_decode_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut pos = 0;
    while pos < s.len() {
        match next_utf8_char(s, &mut pos) {
            Some(c) if c <= 0xff => out.push(c as u8),
            _ => out.push(b'?'),
        }
    }
    out
}

fn utf8_encode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(utf8_encode_bytes(
        args.str(0),
    ))))
}

fn utf8_decode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(utf8_decode_bytes(
        args.str(0),
    ))))
}

pub static UTF8_ENCODE: NativeFunction = NativeFunction {
    sig: "utf8_encode(string $string): string",
    handler: utf8_encode,
};
pub static UTF8_DECODE: NativeFunction = NativeFunction {
    sig: "utf8_decode(string $string): string",
    handler: utf8_decode,
};

pub fn register(r: &mut Registry) {
    r.function_with_flags(&UTF8_ENCODE, fn_flags::DEPRECATED);
    r.function_with_flags(&UTF8_DECODE, fn_flags::DEPRECATED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn call(t: &mut TestVm, f: &str, s: &[u8]) -> Vec<u8> {
        match t.call(f, vec![Value::bytes(s)]).unwrap() {
            Value::String(s) => s.to_vec(),
            v => panic!("unexpected {v:?}"),
        }
    }

    #[test]
    fn encode_latin1() {
        let mut t = TestVm::new();
        assert_eq!(call(&mut t, "utf8_encode", b"abc"), b"abc");
        assert_eq!(
            call(&mut t, "utf8_encode", b"\xe9t\xe9 \xff\x80"),
            "été ÿ\u{80}".as_bytes()
        );
        assert_eq!(call(&mut t, "utf8_encode", b""), b"");
    }

    #[test]
    fn decode_roundtrip() {
        let mut t = TestVm::new();
        let all: Vec<u8> = (0..=255u8).collect();
        let enc = call(&mut t, "utf8_encode", &all);
        assert_eq!(call(&mut t, "utf8_decode", &enc), all);
        assert_eq!(call(&mut t, "utf8_decode", "€".as_bytes()), b"?");
        assert_eq!(call(&mut t, "utf8_decode", "😀x".as_bytes()), b"?x");
    }

    #[test]
    fn bug43957_truncated_lead_byte() {
        let mut t = TestVm::new();
        assert_eq!(call(&mut t, "utf8_decode", b"abc\xe0"), b"abc?");
    }

    #[test]
    fn bug49687_ill_formed_sequences() {
        let mut t = TestVm::new();
        assert_eq!(
            call(&mut t, "utf8_decode", b"\x41\xC2\x3E\x42"),
            b"\x41?\x3e\x42"
        );
        assert_eq!(call(&mut t, "utf8_decode", b"\xE3\x80\x22"), b"?\x22");
        assert_eq!(
            call(
                &mut t,
                "utf8_decode",
                b"\x41\x98\xBA\x42\xE2\x98\x43\xE2\x98\xBA\xE2\x98"
            ),
            b"\x41??\x42?\x43??"
        );
    }

    #[test]
    fn next_char_failure_advances() {
        // Overlong, surrogate, out-of-range, invalid lead.
        let mut pos = 0;
        assert_eq!(next_utf8_char(b"\xc0\x80", &mut pos), None);
        assert_eq!(pos, 1);
        pos = 0;
        assert_eq!(next_utf8_char(b"\xed\xa0\x80", &mut pos), None);
        assert_eq!(pos, 3);
        pos = 0;
        assert_eq!(next_utf8_char(b"\xf4\x90\x80\x80", &mut pos), None);
        assert_eq!(pos, 4);
        pos = 0;
        assert_eq!(next_utf8_char(b"\xf8", &mut pos), None);
        assert_eq!(pos, 1);
        pos = 0;
        assert_eq!(next_utf8_char("é".as_bytes(), &mut pos), Some(0xe9));
        assert_eq!(pos, 2);
    }
}
