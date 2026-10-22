//! `trim()` family, `strspn()` and `strcspn()` (ext/standard/string.c).

#![allow(clippy::collapsible_if)]

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use super::util::php_charmask;

/// `php_is_whitespace`: the default `trim()` character set `" \f\n\r\t\v\0"`.
fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | 0x0c | b'\n' | b'\r' | b'\t' | 0x0b | 0)
}

/// `php_trim`: mode 1 trims left, 2 trims right, 3 both. `what == None` selects
/// the default whitespace set.
pub fn php_trim(vm: &mut Vm, s: &PhpStr, what: Option<&[u8]>, mode: u8) -> PResult<PhpStr> {
    let b = s.as_bytes();
    let (mut start, mut end) = (0, b.len());
    match what {
        Some(w) if w.len() == 1 => {
            let p = w[0];
            if mode & 1 != 0 {
                while start != end && b[start] == p {
                    start += 1;
                }
            }
            if mode & 2 != 0 {
                while start != end && b[end - 1] == p {
                    end -= 1;
                }
            }
        }
        Some(w) => {
            let mask = php_charmask(vm, w)?;
            if mode & 1 != 0 {
                while start != end && mask[b[start] as usize] {
                    start += 1;
                }
            }
            if mode & 2 != 0 {
                while start != end && mask[b[end - 1] as usize] {
                    end -= 1;
                }
            }
        }
        None => {
            if mode & 1 != 0 {
                while start != end && is_whitespace(b[start]) {
                    start += 1;
                }
            }
            if mode & 2 != 0 {
                while start != end && is_whitespace(b[end - 1]) {
                    end -= 1;
                }
            }
        }
    }
    Ok(if end - start == b.len() {
        s.clone()
    } else {
        PhpStr::from_bytes(&b[start..end])
    })
}

fn do_trim(vm: &mut Vm, args: &mut Args, mode: u8) -> PResult<Value> {
    let s = args.str(0).clone();
    let what = if args.is_passed(1) {
        Some(args.str(1).clone())
    } else {
        None
    };
    Ok(Value::String(php_trim(
        vm,
        &s,
        what.as_ref().map(|w| w.as_bytes()),
        mode,
    )?))
}

fn trim(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_trim(vm, args, 3)
}

fn rtrim(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_trim(vm, args, 2)
}

fn ltrim(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_trim(vm, args, 1)
}

/// `php_spn_common_handler`.
fn spn_common(args: &Args, is_strspn: bool) -> PResult<Value> {
    let s = args.str(0).as_bytes();
    let chars = args.str(1).as_bytes();
    let mut start = args.int(2);
    let mut remain_len = s.len();
    if start < 0 {
        start += remain_len as i64;
        if start < 0 {
            start = 0;
        }
    } else if start as usize > remain_len {
        start = remain_len as i64;
    }
    let start = start as usize;
    remain_len -= start;
    let len = match args.int_opt(3) {
        Some(mut len) => {
            if len < 0 {
                len += remain_len as i64;
                if len < 0 {
                    len = 0;
                }
            } else if len as usize > remain_len {
                len = remain_len as i64;
            }
            len as usize
        }
        None => remain_len,
    };
    if len == 0 {
        return Ok(Value::Int(0));
    }
    let hay = &s[start..start + len];
    Ok(Value::Int(strspn_common(hay, chars, is_strspn) as i64))
}

/// `php_strspn_strcspn_common`: length of the initial segment of `hay` made of
/// bytes in (`must_match`) / not in `chars`.
pub fn strspn_common(hay: &[u8], chars: &[u8], must_match: bool) -> usize {
    if chars.len() == 1 {
        let c = chars[0];
        return hay
            .iter()
            .position(|&b| (b == c) != must_match)
            .unwrap_or(hay.len());
    }
    let mut table = [false; 256];
    for &c in chars {
        table[c as usize] = true;
    }
    hay.iter()
        .position(|&b| table[b as usize] != must_match)
        .unwrap_or(hay.len())
}

fn strspn(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    spn_common(args, true)
}

fn strcspn(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    spn_common(args, false)
}

pub static TRIM: NativeFunction = NativeFunction {
    sig: r#"trim(string $string, string $characters = " \f\n\r\t\v\0"): string"#,
    handler: trim,
};
pub static RTRIM: NativeFunction = NativeFunction {
    sig: r#"rtrim(string $string, string $characters = " \f\n\r\t\v\0"): string"#,
    handler: rtrim,
};
pub static CHOP: NativeFunction = NativeFunction {
    sig: r#"chop(string $string, string $characters = " \f\n\r\t\v\0"): string"#,
    handler: rtrim,
};
pub static LTRIM: NativeFunction = NativeFunction {
    sig: r#"ltrim(string $string, string $characters = " \f\n\r\t\v\0"): string"#,
    handler: ltrim,
};
pub static STRSPN: NativeFunction = NativeFunction {
    sig: "strspn(string $string, string $characters, int $offset = 0, ?int $length = null): int",
    handler: strspn,
};
pub static STRCSPN: NativeFunction = NativeFunction {
    sig: "strcspn(string $string, string $characters, int $offset = 0, ?int $length = null): int",
    handler: strcspn,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&TRIM, &RTRIM, &CHOP, &LTRIM, &STRSPN, &STRCSPN]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn s(x: &str) -> Value {
        Value::str(x)
    }

    fn call_s(t: &mut TestVm, f: &str, args: Vec<Value>) -> String {
        match t.call(f, args).unwrap() {
            Value::String(x) => x.to_string_lossy().into_owned(),
            v => panic!("not a string: {v:?}"),
        }
    }

    #[test]
    fn trim_defaults() {
        let mut t = TestVm::new();
        assert_eq!(
            call_s(&mut t, "trim", vec![s(" \t\n\r\0\x0bhi \x0c")]),
            "hi"
        );
        assert_eq!(call_s(&mut t, "ltrim", vec![s("  hi  ")]), "hi  ");
        assert_eq!(call_s(&mut t, "rtrim", vec![s("  hi  ")]), "  hi");
        assert_eq!(call_s(&mut t, "chop", vec![s("hi\n")]), "hi");
        assert_eq!(call_s(&mut t, "trim", vec![s("")]), "");
    }

    #[test]
    fn trim_char_lists_and_ranges() {
        let mut t = TestVm::new();
        assert_eq!(
            call_s(&mut t, "trim", vec![s("abcHELLOcba"), s("a..c")]),
            "HELLO"
        );
        assert_eq!(call_s(&mut t, "trim", vec![s("xxhixx"), s("x")]), "hi");
        assert_eq!(call_s(&mut t, "rtrim", vec![s("1.500"), s("0")]), "1.5");
        assert_eq!(call_s(&mut t, "ltrim", vec![s("0012"), s("0..9")]), "");
        // Invalid ranges warn but still trim the plain characters.
        assert_eq!(call_s(&mut t, "trim", vec![s("..abc.."), s("..a")]), "bc");
        assert_eq!(
            t.output(),
            "\nWarning: trim(): Invalid '..'-range, no character to the left of '..' in /t.php on line 3\n"
        );
        assert_eq!(call_s(&mut t, "trim", vec![s("abc"), s("a..")]), "bc");
        assert_eq!(
            t.output(),
            "\nWarning: trim(): Invalid '..'-range, no character to the right of '..' in /t.php on line 3\n"
        );
        assert_eq!(call_s(&mut t, "trim", vec![s("abc"), s("c..a")]), "b");
        assert_eq!(
            t.output(),
            "\nWarning: trim(): Invalid '..'-range, '..'-range needs to be incrementing in /t.php on line 3\n"
        );
        assert_eq!(call_s(&mut t, "trim", vec![s("abc"), s("a..b..c")]), "");
        assert_eq!(
            t.output(),
            "\nWarning: trim(): Invalid '..'-range in /t.php on line 3\n"
        );
    }

    #[test]
    fn spn() {
        let mut t = TestVm::new();
        let i = |t: &mut TestVm, f: &str, a: Vec<Value>| t.call(f, a).unwrap().as_int().unwrap();
        assert_eq!(
            i(
                &mut t,
                "strspn",
                vec![s("42 is the answer"), s("1234567890")]
            ),
            2
        );
        assert_eq!(i(&mut t, "strcspn", vec![s("abcd"), s("cd")]), 2);
        assert_eq!(
            i(&mut t, "strspn", vec![s("foo"), s("o"), 1.into(), 2.into()]),
            2
        );
        assert_eq!(
            i(
                &mut t,
                "strspn",
                vec![s("foo"), s("o"), 1.into(), (-1).into()]
            ),
            1
        );
        assert_eq!(i(&mut t, "strspn", vec![s("foo"), s("o"), (-2).into()]), 2);
        assert_eq!(i(&mut t, "strspn", vec![s("foo"), s("o"), 10.into()]), 0);
        assert_eq!(
            i(
                &mut t,
                "strcspn",
                vec![s("hello"), s("l"), 0.into(), Value::Null]
            ),
            2
        );
        assert_eq!(
            i(
                &mut t,
                "strcspn",
                vec![s("abcdhelloabcd"), s("abcd"), (-9).into()]
            ),
            5
        );
        assert_eq!(i(&mut t, "strspn", vec![s(""), s("a")]), 0);
    }
}
