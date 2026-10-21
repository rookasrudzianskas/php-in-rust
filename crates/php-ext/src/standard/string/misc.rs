//! Remaining small string functions of ext/standard/string.c: hex
//! conversion, case mapping, escaping, padding, repetition, rot13, ...

#![allow(clippy::collapsible_if)]

use php_runtime::value::array::ArrayKey;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::util::php_charmask;

pub const STR_PAD_LEFT: i64 = 0;
pub const STR_PAD_RIGHT: i64 = 1;
pub const STR_PAD_BOTH: i64 = 2;

// ---------------------------------------------------------------------------
// bin2hex / hex2bin
// ---------------------------------------------------------------------------

/// `zend_bin2hex_str`: lowercase hexadecimal.
pub fn bin2hex_bytes(s: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(s.len() * 2);
    for &b in s {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 15) as usize]);
    }
    out
}

fn bin2hex(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(bin2hex_bytes(args.str(0)))))
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex2bin(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).as_bytes();
    if s.len() & 1 == 1 {
        vm.warning("Hexadecimal input string must have an even length")?;
        return Ok(Value::Bool(false));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let (pairs, _) = s.as_chunks::<2>();
    for pair in pairs {
        match (hex_digit(pair[0]), hex_digit(pair[1])) {
            (Some(h), Some(l)) => out.push(h << 4 | l),
            _ => {
                vm.warning("Input string must be hexadecimal string")?;
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

// ---------------------------------------------------------------------------
// Case mapping
// ---------------------------------------------------------------------------

fn strtoupper(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(args.str(0).to_ascii_uppercase()))
}

fn strtolower(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(args.str(0).to_ascii_lowercase()))
}

fn ucfirst(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    Ok(Value::String(match s.first() {
        Some(c) if c.is_ascii_lowercase() => {
            let mut v = s.to_vec();
            v[0] = c.to_ascii_uppercase();
            PhpStr::from_vec(v)
        }
        _ => s.clone(),
    }))
}

fn lcfirst(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    Ok(Value::String(match s.first() {
        Some(c) if c.is_ascii_uppercase() => {
            let mut v = s.to_vec();
            v[0] = c.to_ascii_lowercase();
            PhpStr::from_vec(v)
        }
        _ => s.clone(),
    }))
}

fn ucwords(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let delims = args.str(1).clone();
    if s.is_empty() {
        return Ok(Value::empty_string());
    }
    let mask = php_charmask(vm, &delims)?;
    let mut v = s.to_vec();
    v[0] = v[0].to_ascii_uppercase();
    for i in 0..v.len() - 1 {
        if mask[v[i] as usize] {
            v[i + 1] = v[i + 1].to_ascii_uppercase();
        }
    }
    Ok(Value::String(PhpStr::from_vec(v)))
}

// ---------------------------------------------------------------------------
// str_increment / str_decrement
// ---------------------------------------------------------------------------

fn alnum_checked(vm: &mut Vm, s: &PhpStr) -> PResult<()> {
    if s.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    if !s.iter().all(u8::is_ascii_alphanumeric) {
        return Err(
            vm.argument_value_error(1, "must be composed only of alphanumeric ASCII characters")
        );
    }
    Ok(())
}

fn str_increment(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    alnum_checked(vm, &s)?;
    let mut v = s.to_vec();
    let mut position = v.len() - 1;
    let mut carry;
    loop {
        let c = v[position];
        if c != b'z' && c != b'Z' && c != b'9' {
            carry = false;
            v[position] += 1;
        } else {
            carry = true;
            if c == b'9' {
                v[position] = b'0';
            } else {
                v[position] -= 25;
            }
        }
        if !carry || position == 0 {
            break;
        }
        position -= 1;
    }
    if carry {
        let first = if v[0] == b'0' { b'1' } else { v[0] };
        v.insert(0, first);
    }
    Ok(Value::String(PhpStr::from_vec(v)))
}

fn str_decrement(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    alnum_checked(vm, &s)?;
    let out_of_range = |vm: &mut Vm| {
        vm.argument_value_error(
            1,
            &format!("\"{}\" is out of decrement range", s.to_string_lossy()),
        )
    };
    if s[0] == b'0' {
        return Err(out_of_range(vm));
    }
    let mut v = s.to_vec();
    let mut position = v.len() - 1;
    let mut carry;
    loop {
        let c = v[position];
        if c != b'a' && c != b'A' && c != b'0' {
            carry = false;
            v[position] -= 1;
        } else {
            carry = true;
            if c == b'0' {
                v[position] = b'9';
            } else {
                v[position] += 25;
            }
        }
        if !carry || position == 0 {
            break;
        }
        position -= 1;
    }
    if carry || (v[0] == b'0' && v.len() > 1) {
        if v.len() == 1 {
            return Err(out_of_range(vm));
        }
        v.remove(0);
    }
    Ok(Value::String(PhpStr::from_vec(v)))
}

// ---------------------------------------------------------------------------
// quotemeta / ord / chr / strrev / str_rot13
// ---------------------------------------------------------------------------

fn quotemeta(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    let mut out = Vec::with_capacity(s.len() * 2);
    for &c in s.iter() {
        if matches!(
            c,
            b'.' | b'\\' | b'+' | b'*' | b'?' | b'[' | b'^' | b']' | b'$' | b'(' | b')'
        ) {
            out.push(b'\\');
        }
        out.push(c);
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

fn ord(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    if s.len() != 1 {
        if s.is_empty() {
            vm.deprecated("Providing an empty string is deprecated")?;
        } else {
            vm.deprecated(
                "Providing a string that is not one byte long is deprecated. Use ord($str[0]) instead",
            )?;
        }
    }
    Ok(Value::Int(s.first().copied().unwrap_or(0) as i64))
}

fn chr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let c = args.int(0);
    if !(0..=255).contains(&c) {
        vm.deprecated(
            "Providing a value not in-between 0 and 255 is deprecated, this is because a byte value must be in the [0, 255] interval. The value used will be constrained using % 256",
        )?;
    }
    Ok(Value::bytes(&[(c & 0xff) as u8]))
}

fn strrev(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut v = args.str(0).to_vec();
    v.reverse();
    Ok(Value::String(PhpStr::from_vec(v)))
}

fn str_rot13(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    let out = s
        .iter()
        .map(|&c| match c {
            b'a'..=b'z' => b'a' + (c - b'a' + 13) % 26,
            b'A'..=b'Z' => b'A' + (c - b'A' + 13) % 26,
            _ => c,
        })
        .collect();
    Ok(Value::String(PhpStr::from_vec(out)))
}

// ---------------------------------------------------------------------------
// Slashes
// ---------------------------------------------------------------------------

/// `php_addslashes`: escapes `'`, `"`, `\` and NUL.
pub fn php_addslashes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 8);
    for &c in s {
        match c {
            0 => out.extend_from_slice(b"\\0"),
            b'\'' | b'"' | b'\\' => {
                out.push(b'\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// `php_stripslashes`: removes one level of backslashes (`\0` becomes NUL).
pub fn php_stripslashes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' {
            i += 1;
            if i < s.len() {
                out.push(if s[i] == b'0' { 0 } else { s[i] });
                i += 1;
            }
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// `php_stripcslashes`: C-style unescaping.
pub fn php_stripcslashes(s: &[u8]) -> Vec<u8> {
    let end = s.len();
    let mut out = Vec::with_capacity(end);
    let mut i = 0;
    while i < end {
        if s[i] == b'\\' && i + 1 < end {
            i += 1;
            match s[i] {
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b'a' => out.push(0x07),
                b't' => out.push(b'\t'),
                b'v' => out.push(0x0b),
                b'b' => out.push(0x08),
                b'f' => out.push(0x0c),
                b'\\' => out.push(b'\\'),
                b'x' if i + 1 < end && s[i + 1].is_ascii_hexdigit() => {
                    i += 1;
                    let mut v = hex_digit(s[i]).expect("hex digit") as u32;
                    if i + 1 < end && s[i + 1].is_ascii_hexdigit() {
                        i += 1;
                        v = v * 16 + hex_digit(s[i]).expect("hex digit") as u32;
                    }
                    out.push(v as u8);
                }
                _ => {
                    // Up to three octal digits, otherwise the character itself.
                    let mut n = 0;
                    let mut v = 0u32;
                    while i < end && (b'0'..=b'7').contains(&s[i]) && n < 3 {
                        v = v * 8 + (s[i] - b'0') as u32;
                        i += 1;
                        n += 1;
                    }
                    if n > 0 {
                        out.push(v as u8);
                        i -= 1;
                    } else {
                        out.push(s[i]);
                    }
                }
            }
        } else {
            out.push(s[i]);
        }
        i += 1;
    }
    out
}

/// `php_addcslashes_str`.
pub fn php_addcslashes(vm: &mut Vm, s: &[u8], what: &[u8]) -> PResult<Vec<u8>> {
    let flags = php_charmask(vm, what)?;
    let mut out = Vec::with_capacity(s.len() * 2);
    for &c in s {
        if flags[c as usize] {
            out.push(b'\\');
            if !(32..=126).contains(&c) {
                match c {
                    b'\n' => out.push(b'n'),
                    b'\t' => out.push(b't'),
                    b'\r' => out.push(b'r'),
                    0x07 => out.push(b'a'),
                    0x0b => out.push(b'v'),
                    0x08 => out.push(b'b'),
                    0x0c => out.push(b'f'),
                    _ => {
                        // Three octal digits, including leading zeros.
                        out.push((c >> 6) + b'0');
                        out.push(((c >> 3) & 7) + b'0');
                        out.push((c & 7) + b'0');
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    Ok(out)
}

fn addcslashes(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let what = args.str(1).clone();
    if s.is_empty() {
        return Ok(Value::empty_string());
    }
    if what.is_empty() {
        return Ok(Value::String(s));
    }
    Ok(Value::String(PhpStr::from_vec(php_addcslashes(
        vm, &s, &what,
    )?)))
}

fn addslashes(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(php_addslashes(args.str(0)))))
}

fn stripcslashes(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(php_stripcslashes(
        args.str(0),
    ))))
}

fn stripslashes(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(php_stripslashes(
        args.str(0),
    ))))
}

// ---------------------------------------------------------------------------
// str_repeat / count_chars / str_pad
// ---------------------------------------------------------------------------

fn str_repeat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let mult = args.int(1);
    if mult < 0 {
        return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
    }
    if s.is_empty() || mult == 0 {
        return Ok(Value::empty_string());
    }
    if mult == 1 {
        return Ok(Value::String(s));
    }
    match s.len().checked_mul(mult as usize) {
        Some(total) if total < isize::MAX as usize / 2 => {
            Ok(Value::String(PhpStr::from_vec(s.repeat(mult as usize))))
        }
        _ => Err(vm.fatal_error(format!(
            "Possible integer overflow in memory allocation ({} * {} + 0)",
            s.len(),
            mult
        ))),
    }
}

fn count_chars(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.str(0);
    let mode = args.int(1);
    if !(0..=4).contains(&mode) {
        return Err(vm.argument_value_error(2, "must be between 0 and 4 (inclusive)"));
    }
    let mut chars = [0i64; 256];
    for &b in input.iter() {
        chars[b as usize] += 1;
    }
    if mode >= 3 {
        let want_used = mode == 3;
        let out: Vec<u8> = (0..256usize)
            .filter(|&i| (chars[i] != 0) == want_used)
            .map(|i| i as u8)
            .collect();
        return Ok(Value::String(PhpStr::from_vec(out)));
    }
    let mut arr = PhpArray::new();
    for (i, &n) in chars.iter().enumerate() {
        let keep = match mode {
            0 => true,
            1 => n != 0,
            _ => n == 0,
        };
        if keep {
            arr.insert(ArrayKey::Int(i as i64), Value::Int(n));
        }
    }
    Ok(Value::array(arr))
}

fn str_pad_fill(out: &mut Vec<u8>, pad_chars: usize, pad: &[u8]) {
    for i in 0..pad_chars {
        out.push(pad[i % pad.len()]);
    }
}

fn str_pad(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.str(0).clone();
    let pad_length = args.int(1);
    let pad = args.str(2).clone();
    let pad_type = args.int(3);
    if pad_length < 0 || pad_length as usize <= input.len() {
        return Ok(Value::String(input));
    }
    if pad.is_empty() {
        return Err(vm.argument_value_error(3, "must not be empty"));
    }
    if !(STR_PAD_LEFT..=STR_PAD_BOTH).contains(&pad_type) {
        return Err(
            vm.argument_value_error(4, "must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH")
        );
    }
    let num_pad = pad_length as usize - input.len();
    let (left, right) = match pad_type {
        STR_PAD_RIGHT => (0, num_pad),
        STR_PAD_LEFT => (num_pad, 0),
        _ => (num_pad / 2, num_pad - num_pad / 2),
    };
    let mut out = Vec::with_capacity(pad_length as usize);
    str_pad_fill(&mut out, left, &pad);
    out.extend_from_slice(&input);
    str_pad_fill(&mut out, right, &pad);
    Ok(Value::String(PhpStr::from_vec(out)))
}

macro_rules! native {
    ($name:ident, $sig:expr, $handler:expr) => {
        pub static $name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

native!(BIN2HEX, "bin2hex(string $string): string", bin2hex);
native!(HEX2BIN, "hex2bin(string $string): string|false", hex2bin);
native!(STRTOUPPER, "strtoupper(string $string): string", strtoupper);
native!(STRTOLOWER, "strtolower(string $string): string", strtolower);
native!(UCFIRST, "ucfirst(string $string): string", ucfirst);
native!(LCFIRST, "lcfirst(string $string): string", lcfirst);
native!(
    UCWORDS,
    r#"ucwords(string $string, string $separators = " \t\r\n\f\v"): string"#,
    ucwords
);
native!(
    STR_INCREMENT,
    "str_increment(string $string): string",
    str_increment
);
native!(
    STR_DECREMENT,
    "str_decrement(string $string): string",
    str_decrement
);
native!(QUOTEMETA, "quotemeta(string $string): string", quotemeta);
native!(ORD, "ord(string $character): int", ord);
native!(CHR, "chr(int $codepoint): string", chr);
native!(STRREV, "strrev(string $string): string", strrev);
native!(STR_ROT13, "str_rot13(string $string): string", str_rot13);
native!(
    ADDCSLASHES,
    "addcslashes(string $string, string $characters): string",
    addcslashes
);
native!(ADDSLASHES, "addslashes(string $string): string", addslashes);
native!(
    STRIPCSLASHES,
    "stripcslashes(string $string): string",
    stripcslashes
);
native!(
    STRIPSLASHES,
    "stripslashes(string $string): string",
    stripslashes
);
native!(
    STR_REPEAT,
    "str_repeat(string $string, int $times): string",
    str_repeat
);
native!(
    COUNT_CHARS,
    "count_chars(string $string, int $mode = 0): array|string",
    count_chars
);
native!(
    STR_PAD,
    "str_pad(string $string, int $length, string $pad_string = \" \", int $pad_type = STR_PAD_RIGHT): string",
    str_pad
);

pub fn register(r: &mut Registry) {
    r.constant("STR_PAD_LEFT", STR_PAD_LEFT);
    r.constant("STR_PAD_RIGHT", STR_PAD_RIGHT);
    r.constant("STR_PAD_BOTH", STR_PAD_BOTH);
    r.functions(&[
        &BIN2HEX,
        &HEX2BIN,
        &STRTOUPPER,
        &STRTOLOWER,
        &UCFIRST,
        &LCFIRST,
        &UCWORDS,
        &STR_INCREMENT,
        &STR_DECREMENT,
        &QUOTEMETA,
        &ORD,
        &CHR,
        &STRREV,
        &STR_ROT13,
        &ADDCSLASHES,
        &ADDSLASHES,
        &STRIPCSLASHES,
        &STRIPSLASHES,
        &STR_REPEAT,
        &COUNT_CHARS,
        &STR_PAD,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn s(x: &str) -> Value {
        Value::str(x)
    }

    fn b(x: &[u8]) -> Value {
        Value::bytes(x)
    }

    fn st(v: Value) -> Vec<u8> {
        match v {
            Value::String(x) => x.to_vec(),
            v => panic!("not a string: {v:?}"),
        }
    }

    fn call(t: &mut TestVm, f: &str, a: Vec<Value>) -> Vec<u8> {
        st(t.call(f, a).unwrap())
    }

    #[test]
    fn hex_roundtrip() {
        let mut t = TestVm::new();
        assert_eq!(
            call(&mut t, "bin2hex", vec![b(b"abc\xff\x00")]),
            b"616263ff00"
        );
        assert_eq!(
            call(&mut t, "hex2bin", vec![s("616263FF00")]),
            b"abc\xff\x00"
        );
        assert_eq!(
            t.call("hex2bin", vec![s("abc")]).unwrap().as_bool(),
            Some(false)
        );
        assert_eq!(
            t.output(),
            "\nWarning: hex2bin(): Hexadecimal input string must have an even length in /t.php on line 3\n"
        );
        assert_eq!(
            t.call("hex2bin", vec![s("zz")]).unwrap().as_bool(),
            Some(false)
        );
        assert_eq!(
            t.output(),
            "\nWarning: hex2bin(): Input string must be hexadecimal string in /t.php on line 3\n"
        );
        assert_eq!(call(&mut t, "hex2bin", vec![s("")]), b"");
    }

    #[test]
    fn case_functions() {
        let mut t = TestVm::new();
        assert_eq!(
            call(&mut t, "strtoupper", vec![b(b"abc\xe9z")]),
            b"ABC\xe9Z"
        );
        assert_eq!(
            call(&mut t, "strtolower", vec![b(b"ABC\xc9Z")]),
            b"abc\xc9z"
        );
        assert_eq!(call(&mut t, "ucfirst", vec![s("hello")]), b"Hello");
        assert_eq!(call(&mut t, "ucfirst", vec![s("")]), b"");
        assert_eq!(call(&mut t, "lcfirst", vec![s("HELLO")]), b"hELLO");
        assert_eq!(
            call(&mut t, "ucwords", vec![s("hello world-foo\tbar")]),
            b"Hello World-foo\tBar"
        );
        assert_eq!(
            call(&mut t, "ucwords", vec![s("hello world-foo"), s("-")]),
            b"Hello world-Foo"
        );
        assert_eq!(
            call(&mut t, "ucwords", vec![s("hello|world"), s("|")]),
            b"Hello|World"
        );
    }

    #[test]
    fn increment_decrement() {
        let mut t = TestVm::new();
        let inc = |t: &mut TestVm, x: &str| {
            String::from_utf8(call(t, "str_increment", vec![s(x)])).unwrap()
        };
        assert_eq!(inc(&mut t, "a"), "b");
        assert_eq!(inc(&mut t, "z"), "aa");
        assert_eq!(inc(&mut t, "Zz"), "AAa");
        assert_eq!(inc(&mut t, "9"), "10");
        assert_eq!(inc(&mut t, "a9"), "b0");
        assert_eq!(inc(&mut t, "Az"), "Ba");
        assert_eq!(inc(&mut t, "zz"), "aaa");
        assert_eq!(inc(&mut t, "99"), "100");
        let dec = |t: &mut TestVm, x: &str| {
            String::from_utf8(call(t, "str_decrement", vec![s(x)])).unwrap()
        };
        assert_eq!(dec(&mut t, "b"), "a");
        assert_eq!(dec(&mut t, "aa"), "z");
        assert_eq!(dec(&mut t, "AAa"), "Zz");
        assert_eq!(dec(&mut t, "10"), "9");
        assert_eq!(dec(&mut t, "Ba"), "Az");
        assert_eq!(
            thrown(t.call("str_increment", vec![s("")])).1,
            "str_increment(): Argument #1 ($string) must not be empty"
        );
        assert_eq!(
            thrown(t.call("str_increment", vec![s("a-")])).1,
            "str_increment(): Argument #1 ($string) must be composed only of alphanumeric ASCII characters"
        );
        assert_eq!(
            thrown(t.call("str_decrement", vec![s("a")])).1,
            "str_decrement(): Argument #1 ($string) \"a\" is out of decrement range"
        );
        assert_eq!(
            thrown(t.call("str_decrement", vec![s("0")])).1,
            "str_decrement(): Argument #1 ($string) \"0\" is out of decrement range"
        );
        assert_eq!(
            thrown(t.call("str_decrement", vec![s("01")])).1,
            "str_decrement(): Argument #1 ($string) \"01\" is out of decrement range"
        );
    }

    #[test]
    fn quoting_and_slashes() {
        let mut t = TestVm::new();
        assert_eq!(
            call(
                &mut t,
                "quotemeta",
                vec![s("1+1=2 (a.b) [c] $d ^e *f ?g \\h")]
            ),
            b"1\\+1=2 \\(a\\.b\\) \\[c\\] \\$d \\^e \\*f \\?g \\\\h"
        );
        assert_eq!(
            call(&mut t, "addslashes", vec![b(b"O'Re\"il\\ly\0")]),
            b"O\\'Re\\\"il\\\\ly\\0"
        );
        assert_eq!(
            call(&mut t, "stripslashes", vec![s("O\\'Re\\\"il\\\\ly\\0\\")]),
            b"O'Re\"il\\ly\0"
        );
        assert_eq!(
            call(&mut t, "addcslashes", vec![s("zoo['.']"), s("z..A")]),
            b"\\zoo['\\.']"
        );
        assert_eq!(
            t.output(),
            "\nWarning: addcslashes(): Invalid '..'-range, '..'-range needs to be incrementing in /t.php on line 3\n"
        );
        assert_eq!(
            call(
                &mut t,
                "addcslashes",
                vec![b(b"a\n\x01\x7f\xff"), b(b"\x00..\x1f\x7f..\xff")]
            ),
            b"a\\n\\001\\177\\377"
        );
        assert_eq!(
            call(
                &mut t,
                "stripcslashes",
                vec![s("a\\n\\t\\x41\\x4g\\101\\\\\\q\\")]
            ),
            b"a\n\tA\x04gA\\q\\"
        );
        assert_eq!(
            call(&mut t, "stripcslashes", vec![s("\\xZ\\777")]),
            b"xZ\xff"
        );
    }

    #[test]
    fn ord_chr_rev_rot13() {
        let mut t = TestVm::new();
        assert_eq!(t.call("ord", vec![s("A")]).unwrap().as_int(), Some(65));
        assert_eq!(t.call("ord", vec![s("")]).unwrap().as_int(), Some(0));
        assert_eq!(
            t.output(),
            "\nDeprecated: ord(): Providing an empty string is deprecated in /t.php on line 3\n"
        );
        assert_eq!(t.call("ord", vec![s("ab")]).unwrap().as_int(), Some(97));
        assert_eq!(
            t.output(),
            "\nDeprecated: ord(): Providing a string that is not one byte long is deprecated. Use ord($str[0]) instead in /t.php on line 3\n"
        );
        assert_eq!(call(&mut t, "chr", vec![65.into()]), b"A");
        assert_eq!(call(&mut t, "chr", vec![(-1).into()]), b"\xff");
        assert!(
            t.output()
                .contains("Deprecated: chr(): Providing a value not in-between 0 and 255")
        );
        assert_eq!(call(&mut t, "chr", vec![321.into()]), b"A");
        t.output();
        assert_eq!(call(&mut t, "strrev", vec![s("Hello")]), b"olleH");
        assert_eq!(
            call(&mut t, "str_rot13", vec![s("Hello World 123")]),
            b"Uryyb Jbeyq 123"
        );
    }

    #[test]
    fn repeat_pad_count() {
        let mut t = TestVm::new();
        assert_eq!(
            call(&mut t, "str_repeat", vec![s("ab"), 3.into()]),
            b"ababab"
        );
        assert_eq!(call(&mut t, "str_repeat", vec![s("ab"), 0.into()]), b"");
        assert_eq!(
            thrown(t.call("str_repeat", vec![s("ab"), (-1).into()])).1,
            "str_repeat(): Argument #2 ($times) must be greater than or equal to 0"
        );
        assert_eq!(
            call(
                &mut t,
                "str_pad",
                vec![s("5"), 3.into(), s("0"), STR_PAD_LEFT.into()]
            ),
            b"005"
        );
        assert_eq!(
            call(&mut t, "str_pad", vec![s("Alien"), 10.into()]),
            b"Alien     "
        );
        assert_eq!(
            call(
                &mut t,
                "str_pad",
                vec![s("Alien"), 10.into(), s("-="), STR_PAD_BOTH.into()]
            ),
            b"-=Alien-=-"
        );
        assert_eq!(
            call(&mut t, "str_pad", vec![s("Alien"), 3.into()]),
            b"Alien"
        );
        assert_eq!(
            call(
                &mut t,
                "str_pad",
                vec![s("a"), 6.into(), s("xyz"), STR_PAD_LEFT.into()]
            ),
            b"xyzxya"
        );
        assert_eq!(
            thrown(t.call("str_pad", vec![s("a"), 6.into(), s("")])).1,
            "str_pad(): Argument #3 ($pad_string) must not be empty"
        );
        assert_eq!(
            thrown(t.call("str_pad", vec![s("a"), 6.into(), s("x"), 7.into()])).1,
            "str_pad(): Argument #4 ($pad_type) must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH"
        );
        let v = t.call("count_chars", vec![s("abca"), 1.into()]).unwrap();
        let a = v.as_array().unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a.get_int(97).unwrap().as_int(), Some(2));
        assert_eq!(a.get_int(98).unwrap().as_int(), Some(1));
        assert_eq!(
            t.call("count_chars", vec![s("abca")])
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            256
        );
        assert_eq!(
            call(&mut t, "count_chars", vec![s("abca"), 3.into()]),
            b"abc"
        );
        assert_eq!(
            call(&mut t, "count_chars", vec![b(&[0u8, 1, 2, 3]), 4.into()]).len(),
            252
        );
        assert_eq!(
            thrown(t.call("count_chars", vec![s("a"), 5.into()])).1,
            "count_chars(): Argument #2 ($mode) must be between 0 and 4 (inclusive)"
        );
    }
}
