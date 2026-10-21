//! Searching and slicing: `strpos` & co, `strstr` & co, `substr`,
//! `substr_count`, `strpbrk`, `substr_compare`, `str_contains` & co.

#![allow(clippy::collapsible_if)]

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use super::trim::strspn_common;
use super::util::{memnistr, memnrstr, memnstr, memnstr_from};

const OFFSET_ERR: &str = "must be contained in argument #1 ($haystack)";

fn found_pos(pos: Option<usize>) -> Value {
    match pos {
        Some(p) => Value::Int(p as i64),
        None => Value::Bool(false),
    }
}

/// Validates the `$offset` of `strpos`/`stripos` (`_zend_strpos`).
fn forward_offset(vm: &mut Vm, offset: i64, hay_len: usize) -> PResult<usize> {
    let mut offset = offset;
    if offset < 0 {
        offset += hay_len as i64;
    }
    if offset < 0 || offset as usize > hay_len {
        return Err(vm.argument_value_error(3, OFFSET_ERR));
    }
    Ok(offset as usize)
}

fn strpos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0).as_bytes();
    let needle = args.str(1).as_bytes();
    let offset = forward_offset(vm, args.int(2), hay.len())?;
    Ok(found_pos(memnstr_from(hay, needle, offset)))
}

fn stripos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0).as_bytes();
    let needle = args.str(1).as_bytes();
    let offset = forward_offset(vm, args.int(2), hay.len())?;
    Ok(found_pos(memnistr(hay, needle, offset)))
}

/// `[p, e)` window searched by `strrpos`/`strripos` for a given `$offset`.
fn reverse_window(
    vm: &mut Vm,
    offset: i64,
    hay_len: usize,
    needle_len: usize,
) -> PResult<(usize, usize)> {
    if offset >= 0 {
        if offset as usize > hay_len {
            return Err(vm.argument_value_error(3, OFFSET_ERR));
        }
        Ok((offset as usize, hay_len))
    } else {
        let neg = offset.unsigned_abs();
        if offset == i64::MIN || neg > hay_len as u64 {
            return Err(vm.argument_value_error(3, OFFSET_ERR));
        }
        let neg = neg as usize;
        let e = if neg < needle_len {
            hay_len
        } else {
            hay_len - neg + needle_len
        };
        Ok((0, e))
    }
}

fn strrpos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0).as_bytes();
    let needle = args.str(1).as_bytes();
    let (p, e) = reverse_window(vm, args.int(2), hay.len(), needle.len())?;
    Ok(found_pos(memnrstr(&hay[p..e], needle).map(|i| i + p)))
}

fn strripos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0).as_bytes();
    let needle = args.str(1).as_bytes();
    let offset = args.int(2);
    if needle.len() == 1 {
        // Single character search can shortcut the lowercase copies.
        let hl = hay.len() as i64;
        let (p, mut e): (i64, i64) = if offset >= 0 {
            if offset > hl {
                return Err(vm.argument_value_error(3, OFFSET_ERR));
            }
            (offset, hl - 1)
        } else {
            if offset == i64::MIN || offset.unsigned_abs() > hay.len() as u64 {
                return Err(vm.argument_value_error(3, OFFSET_ERR));
            }
            (0, hl + offset)
        };
        let lowered = needle[0].to_ascii_lowercase();
        while e >= p {
            if hay[e as usize].to_ascii_lowercase() == lowered {
                return Ok(Value::Int(e));
            }
            e -= 1;
        }
        return Ok(Value::Bool(false));
    }
    let (p, e) = reverse_window(vm, offset, hay.len(), needle.len())?;
    let lc_hay = hay.to_ascii_lowercase();
    let lc_needle = needle.to_ascii_lowercase();
    Ok(found_pos(
        memnrstr(&lc_hay[p..e], &lc_needle).map(|i| i + p),
    ))
}

/// Result shared by `strstr`, `stristr`, `strrchr`: the part of the haystack
/// before (`part`) or from the match.
fn split_at_match(hay: &PhpStr, pos: Option<usize>, part: bool) -> Value {
    match pos {
        None => Value::Bool(false),
        Some(i) if part => Value::bytes(&hay[..i]),
        Some(i) => Value::bytes(&hay[i..]),
    }
}

fn strstr(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0);
    let pos = memnstr(hay, args.str(1));
    Ok(split_at_match(hay, pos, args.bool(2)))
}

fn stristr(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0);
    let pos = memnistr(hay, args.str(1), 0);
    Ok(split_at_match(hay, pos, args.bool(2)))
}

fn strrchr(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0);
    // `*ZSTR_VAL(needle)` reads the terminating NUL for an empty needle.
    let c = args.str(1).first().copied().unwrap_or(0);
    let pos = hay.iter().rposition(|&b| b == c);
    Ok(split_at_match(hay, pos, args.bool(2)))
}

fn str_contains(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(memnstr(args.str(0), args.str(1)).is_some()))
}

fn str_starts_with(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(args.str(0).starts_with(args.str(1))))
}

fn str_ends_with(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(args.str(0).ends_with(args.str(1))))
}

/// `_zend_substr`.
pub fn php_substr(s: &PhpStr, f: i64, len: Option<i64>) -> PhpStr {
    let slen = s.len();
    let f = if f < 0 {
        // `-(size_t)f > len` with f == i64::MIN wraps correctly via unsigned_abs.
        if f.unsigned_abs() > slen as u64 {
            0
        } else {
            (slen as i64 + f) as usize
        }
    } else if f as usize > slen {
        return PhpStr::new();
    } else {
        f as usize
    };
    let remain = slen - f;
    let l = match len {
        Some(l) if l < 0 => {
            if l.unsigned_abs() > remain as u64 {
                0
            } else {
                (remain as i64 + l) as usize
            }
        }
        Some(l) if l as usize > remain => remain,
        Some(l) => l as usize,
        None => remain,
    };
    if l == slen {
        s.clone()
    } else {
        PhpStr::from_bytes(&s[f..f + l])
    }
}

fn substr(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(php_substr(
        args.str(0),
        args.int(1),
        args.int_opt(2),
    )))
}

fn substr_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0).as_bytes();
    let needle = args.str(1).as_bytes();
    if needle.is_empty() {
        return Err(vm.argument_value_error(2, "must not be empty"));
    }
    let mut offset = args.int(2);
    let mut start = 0usize;
    let mut hay_len = hay.len();
    if offset != 0 {
        if offset < 0 {
            offset += hay_len as i64;
        }
        if offset < 0 || offset as usize > hay_len {
            return Err(vm.argument_value_error(3, OFFSET_ERR));
        }
        start = offset as usize;
        hay_len -= start;
    }
    let length = match args.int_opt(3) {
        Some(mut l) => {
            if l < 0 {
                l += hay_len as i64;
            }
            if l < 0 || l as usize > hay_len {
                return Err(
                    vm.argument_value_error(4, "must be contained in argument #1 ($haystack)")
                );
            }
            l as usize
        }
        None => hay_len,
    };
    let window = &hay[start..start + length];
    let mut count = 0i64;
    let mut p = 0;
    while let Some(i) = memnstr_from(window, needle, p) {
        count += 1;
        p = i + needle.len();
    }
    Ok(Value::Int(count))
}

fn strpbrk(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hay = args.str(0);
    let chars = args.str(1);
    if chars.is_empty() {
        return Err(vm.argument_value_error(2, "must be a non-empty string"));
    }
    let shift = strspn_common(hay, chars, false);
    Ok(if shift < hay.len() {
        Value::bytes(&hay[shift..])
    } else {
        Value::Bool(false)
    })
}

/// `zend_binary_strncmp` / `zend_binary_strncasecmp_l` (raw byte difference
/// like glibc's memcmp, then the length difference).
fn binary_strncmp(s1: &[u8], s2: &[u8], length: usize, ci: bool) -> i64 {
    let n = length.min(s1.len()).min(s2.len());
    for i in 0..n {
        let (a, b) = if ci {
            (s1[i].to_ascii_lowercase(), s2[i].to_ascii_lowercase())
        } else {
            (s1[i], s2[i])
        };
        if a != b {
            return a as i64 - b as i64;
        }
    }
    let (l1, l2) = (length.min(s1.len()), length.min(s2.len()));
    match l1.cmp(&l2) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn substr_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s1 = args.str(0).as_bytes();
    let s2 = args.str(1).as_bytes();
    let mut offset = args.int(2);
    let len = args.int_opt(3);
    let ci = args.bool(4);
    let mut len_v = 0i64;
    if let Some(l) = len {
        if l <= 0 {
            if l == 0 {
                return Ok(Value::Int(0));
            }
            return Err(vm.argument_value_error(4, "must be greater than or equal to 0"));
        }
        len_v = l;
    }
    if offset < 0 {
        offset += s1.len() as i64;
        if offset < 0 {
            offset = 0;
        }
    }
    if offset as usize > s1.len() {
        return Err(vm.argument_value_error(3, OFFSET_ERR));
    }
    let offset = offset as usize;
    let cmp_len = if len_v != 0 {
        len_v as usize
    } else {
        s2.len().max(s1.len() - offset)
    };
    Ok(Value::Int(binary_strncmp(&s1[offset..], s2, cmp_len, ci)))
}

macro_rules! native {
    ($name:ident, $sig:expr, $handler:expr) => {
        pub static $name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

native!(
    STRPOS,
    "strpos(string $haystack, string $needle, int $offset = 0): int|false",
    strpos
);
native!(
    STRIPOS,
    "stripos(string $haystack, string $needle, int $offset = 0): int|false",
    stripos
);
native!(
    STRRPOS,
    "strrpos(string $haystack, string $needle, int $offset = 0): int|false",
    strrpos
);
native!(
    STRRIPOS,
    "strripos(string $haystack, string $needle, int $offset = 0): int|false",
    strripos
);
native!(
    STRSTR,
    "strstr(string $haystack, string $needle, bool $before_needle = false): string|false",
    strstr
);
native!(
    STRCHR,
    "strchr(string $haystack, string $needle, bool $before_needle = false): string|false",
    strstr
);
native!(
    STRISTR,
    "stristr(string $haystack, string $needle, bool $before_needle = false): string|false",
    stristr
);
native!(
    STRRCHR,
    "strrchr(string $haystack, string $needle, bool $before_needle = false): string|false",
    strrchr
);
native!(
    STR_CONTAINS,
    "str_contains(string $haystack, string $needle): bool",
    str_contains
);
native!(
    STR_STARTS_WITH,
    "str_starts_with(string $haystack, string $needle): bool",
    str_starts_with
);
native!(
    STR_ENDS_WITH,
    "str_ends_with(string $haystack, string $needle): bool",
    str_ends_with
);
native!(
    SUBSTR,
    "substr(string $string, int $offset, ?int $length = null): string",
    substr
);
native!(
    SUBSTR_COUNT,
    "substr_count(string $haystack, string $needle, int $offset = 0, ?int $length = null): int",
    substr_count
);
native!(
    STRPBRK,
    "strpbrk(string $string, string $characters): string|false",
    strpbrk
);
native!(
    SUBSTR_COMPARE,
    "substr_compare(string $haystack, string $needle, int $offset, ?int $length = null, bool $case_insensitive = false): int",
    substr_compare
);

pub fn register(r: &mut Registry) {
    r.functions(&[
        &STRPOS,
        &STRIPOS,
        &STRRPOS,
        &STRRIPOS,
        &STRSTR,
        &STRCHR,
        &STRISTR,
        &STRRCHR,
        &STR_CONTAINS,
        &STR_STARTS_WITH,
        &STR_ENDS_WITH,
        &SUBSTR,
        &SUBSTR_COUNT,
        &STRPBRK,
        &SUBSTR_COMPARE,
    ]);
}

#[cfg(test)]
mod tests {
    use crate::testing::{TestVm, thrown};
    use php_runtime::Value;

    fn s(x: &str) -> Value {
        Value::str(x)
    }

    fn call(t: &mut TestVm, f: &str, a: Vec<Value>) -> Value {
        t.call(f, a).unwrap()
    }

    fn st(v: Value) -> String {
        match v {
            Value::String(x) => x.to_string_lossy().into_owned(),
            v => panic!("not a string: {v:?}"),
        }
    }

    fn int_or_false(v: Value) -> Option<i64> {
        match v {
            Value::Int(i) => Some(i),
            Value::Bool(false) => None,
            v => panic!("unexpected {v:?}"),
        }
    }

    #[test]
    fn strpos_family() {
        let mut t = TestVm::new();
        let p = |t: &mut TestVm, f: &str, a: Vec<Value>| int_or_false(call(t, f, a));
        assert_eq!(p(&mut t, "strpos", vec![s("abcabc"), s("b")]), Some(1));
        assert_eq!(
            p(&mut t, "strpos", vec![s("abcabc"), s("b"), 2.into()]),
            Some(4)
        );
        assert_eq!(
            p(&mut t, "strpos", vec![s("abcabc"), s("b"), (-2).into()]),
            Some(4)
        );
        assert_eq!(p(&mut t, "strpos", vec![s("abc"), s("")]), Some(0));
        assert_eq!(
            p(&mut t, "strpos", vec![s("abc"), s(""), 3.into()]),
            Some(3)
        );
        assert_eq!(p(&mut t, "strpos", vec![s("abc"), s("d")]), None);
        assert_eq!(p(&mut t, "stripos", vec![s("aBc"), s("bC")]), Some(1));
        assert_eq!(p(&mut t, "strrpos", vec![s("abcabc"), s("b")]), Some(4));
        assert_eq!(
            p(&mut t, "strrpos", vec![s("abcabc"), s("b"), (-3).into()]),
            Some(1)
        );
        assert_eq!(
            p(&mut t, "strrpos", vec![s("abcabc"), s("b"), (-2).into()]),
            Some(4)
        );
        assert_eq!(p(&mut t, "strrpos", vec![s("abc"), s("")]), Some(3));
        assert_eq!(p(&mut t, "strripos", vec![s("aBcAbC"), s("b")]), Some(4));
        assert_eq!(p(&mut t, "strripos", vec![s("aBcAbC"), s("bc")]), Some(4));
        assert_eq!(
            p(&mut t, "strripos", vec![s("aBcAbC"), s("b"), 5.into()]),
            None
        );
        assert_eq!(
            p(&mut t, "strripos", vec![s("aBcAbC"), s("b"), (-2).into()]),
            Some(4)
        );
        assert_eq!(
            p(&mut t, "strripos", vec![s("aBcAbC"), s("b"), (-3).into()]),
            Some(1)
        );
    }

    #[test]
    fn offset_errors() {
        let mut t = TestVm::new();
        assert_eq!(
            thrown(t.call("strpos", vec![s("abc"), s("a"), 4.into()])),
            (
                "ValueError".into(),
                "strpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
                    .into()
            )
        );
        assert_eq!(
            thrown(t.call("strrpos", vec![s("abc"), s("a"), (-4).into()])).1,
            "strrpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        );
        assert_eq!(
            thrown(t.call("stripos", vec![s("abc"), s("a"), (-4).into()])).1,
            "stripos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        );
        assert_eq!(
            thrown(t.call("strripos", vec![s("abc"), s("ab"), 4.into()])).1,
            "strripos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        );
    }

    #[test]
    fn strstr_family() {
        let mut t = TestVm::new();
        assert_eq!(
            st(call(&mut t, "strstr", vec![s("user@example.com"), s("@")])),
            "@example.com"
        );
        assert_eq!(
            st(call(
                &mut t,
                "strstr",
                vec![s("user@example.com"), s("@"), true.into()]
            )),
            "user"
        );
        assert_eq!(st(call(&mut t, "strchr", vec![s("abc"), s("c")])), "c");
        assert_eq!(
            call(&mut t, "strstr", vec![s("abc"), s("x")]).as_bool(),
            Some(false)
        );
        assert_eq!(st(call(&mut t, "strstr", vec![s("abc"), s("")])), "abc");
        assert_eq!(
            st(call(&mut t, "stristr", vec![s("USER@EXAMPLE.com"), s("e")])),
            "ER@EXAMPLE.com"
        );
        assert_eq!(
            st(call(
                &mut t,
                "stristr",
                vec![s("USER@EXAMPLE.com"), s("e"), true.into()]
            )),
            "US"
        );
        assert_eq!(st(call(&mut t, "strrchr", vec![s("a/b/c"), s("/")])), "/c");
        assert_eq!(
            st(call(
                &mut t,
                "strrchr",
                vec![s("a/b/c"), s("/x"), true.into()]
            )),
            "a/b"
        );
        assert_eq!(
            call(&mut t, "strrchr", vec![s("abc"), s("")]).as_bool(),
            Some(false)
        );
        assert_eq!(
            call(&mut t, "str_contains", vec![s("abc"), s("")]).as_bool(),
            Some(true)
        );
        assert_eq!(
            call(&mut t, "str_starts_with", vec![s("abc"), s("ab")]).as_bool(),
            Some(true)
        );
        assert_eq!(
            call(&mut t, "str_ends_with", vec![s("abc"), s("bc")]).as_bool(),
            Some(true)
        );
        assert_eq!(
            call(&mut t, "str_ends_with", vec![s("abc"), s("abcd")]).as_bool(),
            Some(false)
        );
    }

    #[test]
    fn substr_cases() {
        let mut t = TestVm::new();
        let sub = |t: &mut TestVm, a: Vec<Value>| st(call(t, "substr", a));
        assert_eq!(sub(&mut t, vec![s("abcdef"), 1.into()]), "bcdef");
        assert_eq!(sub(&mut t, vec![s("abcdef"), (-2).into()]), "ef");
        assert_eq!(sub(&mut t, vec![s("abcdef"), 1.into(), 3.into()]), "bcd");
        assert_eq!(
            sub(&mut t, vec![s("abcdef"), 1.into(), (-1).into()]),
            "bcde"
        );
        assert_eq!(sub(&mut t, vec![s("abcdef"), 6.into()]), "");
        assert_eq!(sub(&mut t, vec![s("abcdef"), 7.into()]), "");
        assert_eq!(sub(&mut t, vec![s("abcdef"), (-10).into(), 2.into()]), "ab");
        assert_eq!(sub(&mut t, vec![s("abcdef"), 2.into(), (-10).into()]), "");
        assert_eq!(sub(&mut t, vec![s("abcdef"), i64::MIN.into()]), "abcdef");
        assert_eq!(sub(&mut t, vec![s("abc"), 0.into(), Value::Null]), "abc");
    }

    #[test]
    fn substr_count_cases() {
        let mut t = TestVm::new();
        let c = |t: &mut TestVm, a: Vec<Value>| call(t, "substr_count", a).as_int().unwrap();
        assert_eq!(c(&mut t, vec![s("hello world, hello"), s("hello")]), 2);
        assert_eq!(c(&mut t, vec![s("aaa"), s("aa")]), 1);
        assert_eq!(c(&mut t, vec![s("hello hello"), s("hello"), 1.into()]), 1);
        assert_eq!(
            c(
                &mut t,
                vec![s("hello hello"), s("l"), (-4).into(), 2.into()]
            ),
            1
        );
        assert_eq!(
            c(
                &mut t,
                vec![s("hello hello"), s("l"), 0.into(), (-3).into()]
            ),
            2
        );
        assert_eq!(
            thrown(t.call("substr_count", vec![s("a"), s("")])).1,
            "substr_count(): Argument #2 ($needle) must not be empty"
        );
        assert_eq!(
            thrown(t.call("substr_count", vec![s("abc"), s("a"), 4.into()])).1,
            "substr_count(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        );
        assert_eq!(
            thrown(t.call("substr_count", vec![s("abc"), s("a"), 0.into(), 4.into()])).1,
            "substr_count(): Argument #4 ($length) must be contained in argument #1 ($haystack)"
        );
    }

    #[test]
    fn strpbrk_and_substr_compare() {
        let mut t = TestVm::new();
        assert_eq!(
            st(call(&mut t, "strpbrk", vec![s("This is a test"), s("st")])),
            "s is a test"
        );
        assert_eq!(
            call(&mut t, "strpbrk", vec![s("abc"), s("x")]).as_bool(),
            Some(false)
        );
        assert_eq!(
            thrown(t.call("strpbrk", vec![s("abc"), s("")])).1,
            "strpbrk(): Argument #2 ($characters) must be a non-empty string"
        );
        let sc = |t: &mut TestVm, a: Vec<Value>| call(t, "substr_compare", a).as_int().unwrap();
        assert_eq!(sc(&mut t, vec![s("abcde"), s("bc"), 1.into(), 2.into()]), 0);
        assert_eq!(
            sc(
                &mut t,
                vec![s("abcde"), s("BC"), 1.into(), 2.into(), true.into()]
            ),
            0
        );
        assert!(sc(&mut t, vec![s("abcde"), s("df"), (-2).into()]) < 0);
        assert!(sc(&mut t, vec![s("abcde"), s("bc"), 1.into(), 3.into()]) > 0);
        assert_eq!(sc(&mut t, vec![s("abcde"), s("cd"), 1.into(), 0.into()]), 0);
        assert_eq!(
            thrown(t.call(
                "substr_compare",
                vec![s("abcde"), s("cd"), 1.into(), (-1).into()]
            ))
            .1,
            "substr_compare(): Argument #4 ($length) must be greater than or equal to 0"
        );
        assert_eq!(
            thrown(t.call("substr_compare", vec![s("abcde"), s("cd"), 6.into()])).1,
            "substr_compare(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
        );
    }
}
