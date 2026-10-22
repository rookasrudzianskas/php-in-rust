//! `str_replace`, `str_ireplace`, `strtr` and `substr_replace`.

#![allow(clippy::collapsible_if)]

use std::rc::Rc;

use php_runtime::value::array::ArrayKey;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::util::memnstr_from;

/// Replaces every non-overlapping occurrence of `needle` (`php_str_to_str_ex`,
/// `php_str_to_str_i_ex`, `php_char_to_str_ex`): `None` when nothing matched.
/// `needle` must not be empty.
fn replace_all(
    hay: &[u8],
    needle: &[u8],
    rep: &[u8],
    ci: bool,
    count: &mut i64,
) -> Option<Vec<u8>> {
    if needle.len() > hay.len() {
        return None;
    }
    let lc_hay;
    let lc_needle;
    let (h, n): (&[u8], &[u8]) = if ci {
        lc_hay = hay.to_ascii_lowercase();
        lc_needle = needle.to_ascii_lowercase();
        (&lc_hay, &lc_needle)
    } else {
        (hay, needle)
    };
    let mut out: Option<Vec<u8>> = None;
    let mut p = 0;
    while let Some(r) = memnstr_from(h, n, p) {
        let o = out.get_or_insert_with(|| Vec::with_capacity(hay.len()));
        o.extend_from_slice(&hay[p..r]);
        o.extend_from_slice(rep);
        *count += 1;
        p = r + n.len();
    }
    if let Some(o) = &mut out {
        o.extend_from_slice(&hay[p..]);
    }
    out
}

enum Search<'a> {
    Str(&'a PhpStr),
    Arr(&'a PhpArray),
}

enum Replace<'a> {
    Str(&'a PhpStr),
    Arr(&'a PhpArray),
}

/// `php_str_replace_in_subject`.
fn replace_in_subject(
    vm: &mut Vm,
    search: &Search,
    replace: &Replace,
    subject: &PhpStr,
    ci: bool,
    count: &mut i64,
) -> PResult<PhpStr> {
    if subject.is_empty() {
        return Ok(PhpStr::new());
    }
    match search {
        Search::Arr(sa) => {
            let mut subj = subject.clone();
            let mut rep_iter = match replace {
                Replace::Arr(ra) => Some(ra.values()),
                Replace::Str(_) => None,
            };
            for entry in sa.values() {
                let search_str = vm.to_php_string(entry)?;
                let rep = match (&mut rep_iter, replace) {
                    (Some(it), _) => match it.next() {
                        Some(v) => vm.to_php_string(v)?,
                        // We've run out of replacement strings, so use an empty one.
                        None => PhpStr::new(),
                    },
                    (None, Replace::Str(s)) => (*s).clone(),
                    (None, Replace::Arr(_)) => unreachable!(),
                };
                if search_str.is_empty() {
                    continue;
                }
                if let Some(new) = replace_all(&subj, &search_str, &rep, ci, count) {
                    if new.is_empty() {
                        return Ok(PhpStr::new());
                    }
                    subj = PhpStr::from_vec(new);
                }
            }
            Ok(subj)
        }
        Search::Str(s) => {
            let Replace::Str(rep) = replace else {
                unreachable!("checked by the caller")
            };
            if s.is_empty() {
                return Ok(subject.clone());
            }
            Ok(match replace_all(subject, s, rep, ci, count) {
                Some(new) => PhpStr::from_vec(new),
                None => subject.clone(),
            })
        }
    }
}

fn str_replace_common(vm: &mut Vm, args: &mut Args, ci: bool) -> PResult<Value> {
    let search = match args.get(0) {
        Value::Array(a) => Search::Arr(a),
        _ => Search::Str(args.str(0)),
    };
    let replace = match args.get(1) {
        Value::Array(a) => Replace::Arr(a),
        _ => Replace::Str(args.str(1)),
    };
    if let (Search::Str(_), Replace::Arr(_)) = (&search, &replace) {
        return Err(vm.argument_type_error(
            2,
            "must be of type string when argument #1 ($search) is a string",
        ));
    }
    let mut count = 0i64;
    let result = match args.get(2) {
        Value::Array(subjects) => {
            let mut out = PhpArray::with_capacity(subjects.len());
            for (key, entry) in subjects.iter() {
                let subject = vm.to_php_string(entry)?;
                let r = replace_in_subject(vm, &search, &replace, &subject, ci, &mut count)?;
                out.insert(key, Value::String(r));
            }
            Value::array(out)
        }
        _ => {
            let subject = args.str(2).clone();
            Value::String(replace_in_subject(
                vm, &search, &replace, &subject, ci, &mut count,
            )?)
        }
    };
    if args.is_passed(3) {
        args.by_ref(3).set(Value::Int(count));
    }
    Ok(result)
}

fn str_replace(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    str_replace_common(vm, args, false)
}

fn str_ireplace(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    str_replace_common(vm, args, true)
}

// ---------------------------------------------------------------------------
// strtr
// ---------------------------------------------------------------------------

/// `php_strtr_ex`: byte translation; later duplicates in `from` win.
fn strtr_bytes(s: &PhpStr, from: &[u8], to: &[u8]) -> PhpStr {
    let trlen = from.len().min(to.len());
    if trlen < 1 {
        return s.clone();
    }
    let mut map: [u8; 256] = std::array::from_fn(|i| i as u8);
    for i in 0..trlen {
        map[from[i] as usize] = to[i];
    }
    if s.iter().all(|&b| map[b as usize] == b) {
        return s.clone();
    }
    PhpStr::from_vec(s.iter().map(|&b| map[b as usize]).collect())
}

/// `php_strtr_array`.
fn strtr_array(vm: &mut Vm, s: &PhpStr, pats: &PhpArray) -> PResult<Value> {
    if pats.is_empty() {
        return Ok(Value::String(s.clone()));
    }
    if pats.len() == 1 {
        let (key, entry) = pats.iter().next().expect("one element");
        let key = key.to_php_str();
        let replace = vm.to_php_string(entry)?;
        if key.is_empty() {
            vm.warning("Ignoring replacement of empty string")?;
            return Ok(Value::String(s.clone()));
        }
        let mut dummy = 0;
        return Ok(Value::String(
            match replace_all(s, &key, &replace, false, &mut dummy) {
                Some(v) => PhpStr::from_vec(v),
                None => s.clone(),
            },
        ));
    }
    Ok(Value::String(strtr_array_ex(vm, s, pats)?))
}

/// `php_strtr_array_ex`: longest-match multi-pattern replacement.
fn strtr_array_ex(vm: &mut Vm, input: &PhpStr, pats: &PhpArray) -> PResult<PhpStr> {
    let slen = input.len();
    let mut minlen = 128 * 1024usize;
    let mut maxlen = 0usize;
    // pattern bytes -> replacement value (integer keys are used as decimal strings)
    let mut table: Vec<(PhpStr, &Value)> = Vec::with_capacity(pats.len());
    let mut first_bytes = [false; 256];
    let mut lengths = vec![false; slen + 1];
    let mut note = |key: &PhpStr, minlen: &mut usize, maxlen: &mut usize| {
        let len = key.len();
        *maxlen = (*maxlen).max(len);
        *minlen = (*minlen).min(len);
        lengths[len] = true;
        first_bytes[key[0] as usize] = true;
    };
    for (k, v) in pats.iter() {
        match k {
            ArrayKey::Str(key) => {
                let len = key.len();
                if len == 0 {
                    vm.warning("Ignoring replacement of empty string")?;
                    continue;
                } else if len > slen {
                    continue;
                }
                note(&key, &mut minlen, &mut maxlen);
                table.push((key, v));
            }
            ArrayKey::Int(n) => {
                let key = PhpStr::from(n.to_string());
                if key.len() > slen {
                    continue;
                }
                note(&key, &mut minlen, &mut maxlen);
                table.push((key, v));
            }
        }
    }
    if minlen > maxlen {
        return Ok(input.clone());
    }
    let lookup: std::collections::HashMap<&[u8], &Value> =
        table.iter().map(|(k, v)| (k.as_bytes(), *v)).collect();
    let bytes = input.as_bytes();
    let mut result: Option<Vec<u8>> = None;
    let (mut old_pos, mut pos) = (0usize, 0usize);
    while pos + minlen <= slen {
        if first_bytes[bytes[pos] as usize] {
            let mut len = maxlen.min(slen - pos);
            while len >= minlen {
                if lengths[len] {
                    if let Some(entry) = lookup.get(&bytes[pos..pos + len]) {
                        let rep = vm.to_php_string(entry)?;
                        let out = result.get_or_insert_with(Vec::new);
                        out.extend_from_slice(&bytes[old_pos..pos]);
                        out.extend_from_slice(&rep);
                        old_pos = pos + len;
                        pos = old_pos - 1;
                        break;
                    }
                }
                len -= 1;
            }
        }
        pos += 1;
    }
    Ok(match result {
        Some(mut out) => {
            out.extend_from_slice(&bytes[old_pos..]);
            PhpStr::from_vec(out)
        }
        None => input.clone(),
    })
}

fn strtr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    if args.len() <= 2 {
        let Value::Array(from) = args.get(1) else {
            return Err(vm.argument_type_error(
                2,
                &format!("must be of type array, {} given", args.get(1).type_name()),
            ));
        };
        if s.is_empty() {
            return Ok(Value::empty_string());
        }
        let from = Rc::clone(from);
        return strtr_array(vm, &s, &from);
    }
    let Value::String(from) = args.get(1) else {
        return Err(vm.argument_type_error(
            2,
            &format!("must be of type string, {} given", args.get(1).type_name()),
        ));
    };
    let from = from.clone();
    let to = match args.get(2) {
        Value::String(t) => t.clone(),
        _ => {
            // Z_PARAM_STRING on an explicit null: deprecation, then "".
            vm.engine_deprecated(
                "strtr(): Passing null to parameter #3 ($to) of type string is deprecated",
            )?;
            PhpStr::new()
        }
    };
    if s.is_empty() {
        return Ok(Value::empty_string());
    }
    Ok(Value::String(strtr_bytes(&s, &from, &to)))
}

// ---------------------------------------------------------------------------
// substr_replace
// ---------------------------------------------------------------------------

/// Clamps `$offset` as `substr_replace` does.
fn clamp_from(f: i64, len: usize) -> usize {
    if f < 0 {
        let f = len as i64 + f;
        if f < 0 { 0 } else { f as usize }
    } else if f as usize > len {
        len
    } else {
        f as usize
    }
}

/// Resolves `$length` against the string (negative lengths stop that many
/// bytes from the end) and clamps it to the available bytes.
fn clamp_len(l: i64, f: usize, len: usize) -> usize {
    let mut l = l;
    if l < 0 {
        l += (len - f) as i64;
        if l < 0 {
            l = 0;
        }
    }
    (l as usize).min(len - f)
}

fn splice(orig: &[u8], f: usize, l: usize, repl: &[u8]) -> PhpStr {
    let mut out = Vec::with_capacity(orig.len() - l + repl.len());
    out.extend_from_slice(&orig[..f]);
    out.extend_from_slice(repl);
    out.extend_from_slice(&orig[f + l..]);
    PhpStr::from_vec(out)
}

fn substr_replace(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let from_v = args.get(2).clone();
    let len_v = args.get(3).clone();
    let repl_v = args.get(1).clone();

    if let Value::String(s) = args.get(0) {
        let s = s.clone();
        if matches!(from_v, Value::Array(_)) {
            return Err(
                vm.argument_type_error(3, "cannot be an array when working on a single string")
            );
        }
        if matches!(len_v, Value::Array(_)) {
            return Err(
                vm.argument_type_error(4, "cannot be an array when working on a single string")
            );
        }
        let Value::Int(from_long) = from_v else {
            unreachable!("offset is coerced to int");
        };
        let l = match len_v {
            Value::Int(l) => l,
            _ => s.len() as i64,
        };
        let f = clamp_from(from_long, s.len());
        let l = clamp_len(l, f, s.len());
        let repl = match &repl_v {
            Value::Array(ra) => match ra.values().next() {
                Some(v) => vm.to_php_string(v)?,
                None => PhpStr::new(),
            },
            Value::String(r) => r.clone(),
            _ => unreachable!("replacement is coerced"),
        };
        return Ok(Value::String(splice(&s, f, l, &repl)));
    }

    // `$string` is an array of strings.
    let Value::Array(strs) = args.get(0).clone() else {
        unreachable!("string|array");
    };
    let mut out = PhpArray::with_capacity(strs.len());
    let mut from_iter = match &from_v {
        Value::Array(a) => Some(a.values()),
        _ => None,
    };
    let mut len_iter = match &len_v {
        Value::Array(a) => Some(a.values()),
        _ => None,
    };
    let mut repl_iter = match &repl_v {
        Value::Array(a) => Some(a.values()),
        _ => None,
    };
    for (key, v) in strs.iter() {
        let orig = vm.to_php_string(v)?;
        let olen = orig.len();
        let f = match &mut from_iter {
            Some(it) => match it.next() {
                Some(e) => {
                    let n = vm.to_int(e)?;
                    clamp_from(n, olen)
                }
                None => 0,
            },
            None => match from_v {
                Value::Int(n) => clamp_from(n, olen),
                _ => 0,
            },
        };
        let l = match &mut len_iter {
            Some(it) => match it.next() {
                Some(e) => vm.to_int(e)?,
                None => olen as i64,
            },
            None => match len_v {
                Value::Int(n) => n,
                _ => olen as i64,
            },
        };
        let l = clamp_len(l, f, olen);
        let result = match (&mut repl_iter, &repl_v) {
            (Some(it), _) => match it.next() {
                Some(e) => {
                    let r = vm.to_php_string(e)?;
                    splice(&orig, f, l, &r)
                }
                None => splice(&orig, f, l, b""),
            },
            (None, Value::String(r)) => splice(&orig, f, l, r),
            _ => unreachable!("replacement is coerced"),
        };
        out.insert(key, Value::String(result));
    }
    Ok(Value::array(out))
}

pub static STR_REPLACE: NativeFunction = NativeFunction {
    sig: "str_replace(array|string $search, array|string $replace, string|array $subject, &$count = null): string|array",
    handler: str_replace,
};
pub static STR_IREPLACE: NativeFunction = NativeFunction {
    sig: "str_ireplace(array|string $search, array|string $replace, string|array $subject, &$count = null): string|array",
    handler: str_ireplace,
};
pub static STRTR: NativeFunction = NativeFunction {
    sig: "strtr(string $string, string|array $from, ?string $to = null): string",
    handler: strtr,
};
pub static SUBSTR_REPLACE: NativeFunction = NativeFunction {
    sig: "substr_replace(array|string $string, array|string $replace, array|int $offset, array|int|null $length = null): string|array",
    handler: substr_replace,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&STR_REPLACE, &STR_IREPLACE, &STRTR, &SUBSTR_REPLACE]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::PhpRef;

    fn s(x: &str) -> Value {
        Value::str(x)
    }

    fn arr(items: &[&str]) -> Value {
        Value::array(items.iter().map(|x| Value::str(x)).collect::<PhpArray>())
    }

    fn assoc(items: &[(&str, &str)]) -> Value {
        Value::array(
            items
                .iter()
                .map(|(k, v)| (ArrayKey::from(*k), Value::str(v)))
                .collect::<PhpArray>(),
        )
    }

    fn st(v: Value) -> String {
        match v {
            Value::String(x) => x.to_string_lossy().into_owned(),
            v => panic!("not a string: {v:?}"),
        }
    }

    fn list(v: &Value) -> Vec<String> {
        match v {
            Value::Array(a) => a
                .values()
                .map(|v| v.as_str().unwrap().to_string_lossy().into_owned())
                .collect(),
            v => panic!("not an array: {v:?}"),
        }
    }

    #[test]
    fn str_replace_basic() {
        let mut t = TestVm::new();
        let cnt = PhpRef::new(Value::Null);
        let r = t
            .call(
                "str_replace",
                vec![s("l"), s("L"), s("Hello World"), Value::Ref(cnt.clone())],
            )
            .unwrap();
        assert_eq!(st(r), "HeLLo WorLd");
        assert_eq!(cnt.get().as_int(), Some(3));
        let r = t
            .call("str_replace", vec![s("ab"), s("x"), s("ababab")])
            .unwrap();
        assert_eq!(st(r), "xxx");
        let r = t
            .call("str_replace", vec![s("abc"), s(""), s("abc")])
            .unwrap();
        assert_eq!(st(r), "");
        let r = t
            .call("str_replace", vec![s(""), s("x"), s("abc")])
            .unwrap();
        assert_eq!(st(r), "abc");
        let r = t
            .call("str_ireplace", vec![s("L"), s("1"), s("Hello")])
            .unwrap();
        assert_eq!(st(r), "He11o");
        let r = t
            .call("str_ireplace", vec![s("WORLD"), s("php"), s("Hello World")])
            .unwrap();
        assert_eq!(st(r), "Hello php");
    }

    #[test]
    fn str_replace_arrays() {
        let mut t = TestVm::new();
        let r = t
            .call(
                "str_replace",
                vec![arr(&["a", "b", "c"]), arr(&["1", "2"]), s("abcabc")],
            )
            .unwrap();
        assert_eq!(st(r), "12".to_string() + "12");
        let r = t
            .call("str_replace", vec![arr(&["a", "b"]), s("x"), s("aabb")])
            .unwrap();
        assert_eq!(st(r), "xxxx");
        // Sequential replacement: later pairs see earlier results.
        let r = t
            .call(
                "str_replace",
                vec![arr(&["a", "b"]), arr(&["b", "c"]), s("a")],
            )
            .unwrap();
        assert_eq!(st(r), "c");
        // Subject array keeps keys and returns the total count.
        let cnt = PhpRef::new(Value::Null);
        let r = t
            .call(
                "str_replace",
                vec![
                    s("o"),
                    s("0"),
                    assoc(&[("x", "foo"), ("y", "bar"), ("z", "boo")]),
                    Value::Ref(cnt.clone()),
                ],
            )
            .unwrap();
        match &r {
            Value::Array(a) => {
                assert_eq!(a.get_str(b"x").unwrap().as_str().unwrap(), "f00");
                assert_eq!(a.get_str(b"y").unwrap().as_str().unwrap(), "bar");
                assert_eq!(a.get_str(b"z").unwrap().as_str().unwrap(), "b00");
            }
            _ => panic!(),
        }
        assert_eq!(cnt.get().as_int(), Some(4));
        assert_eq!(
            thrown(t.call("str_replace", vec![s("a"), arr(&["b"]), s("x")])),
            (
                "TypeError".into(),
                "str_replace(): Argument #2 ($replace) must be of type string when argument #1 ($search) is a string"
                    .into()
            )
        );
    }

    #[test]
    fn strtr_forms() {
        let mut t = TestVm::new();
        assert_eq!(
            st(t.call("strtr", vec![s("Hi all"), s("ai"), s("eo")])
                .unwrap()),
            "Ho ell"
        );
        assert_eq!(
            st(t.call("strtr", vec![s("abc"), s("ab"), s("x")]).unwrap()),
            "xbc"
        );
        assert_eq!(
            st(t.call("strtr", vec![s("abc"), s(""), s("x")]).unwrap()),
            "abc"
        );
        let pairs = assoc(&[("Hi", "Hello"), ("Hello", "Hi"), ("H", "J")]);
        assert_eq!(
            st(t.call("strtr", vec![s("Hi all, I said Hello"), pairs])
                .unwrap()),
            "Hello all, I said Hi"
        );
        let pairs = assoc(&[("a", "1"), ("aa", "2"), ("aaa", "3")]);
        assert_eq!(st(t.call("strtr", vec![s("aaaa"), pairs]).unwrap()), "31");
        // Numeric keys act as their decimal strings.
        let mut a = PhpArray::new();
        a.insert(ArrayKey::Int(1), Value::str("one"));
        a.insert(ArrayKey::from("x"), Value::str("y"));
        assert_eq!(
            st(t.call("strtr", vec![s("1x1"), Value::array(a)]).unwrap()),
            "oneyone"
        );
        // One-element array with empty key warns.
        let one = assoc(&[("", "x")]);
        assert_eq!(st(t.call("strtr", vec![s("abc"), one]).unwrap()), "abc");
        assert_eq!(
            t.output(),
            "\nWarning: strtr(): Ignoring replacement of empty string in /t.php on line 3\n"
        );
        assert_eq!(
            st(t.call("strtr", vec![s("abc"), assoc(&[("b", "BB")])])
                .unwrap()),
            "aBBc"
        );
        assert_eq!(
            thrown(t.call("strtr", vec![s("abc"), s("b")])).1,
            "strtr(): Argument #2 ($from) must be of type array, string given"
        );
    }

    #[test]
    fn substr_replace_string() {
        let mut t = TestVm::new();
        let r = |t: &mut TestVm, a: Vec<Value>| st(t.call("substr_replace", a).unwrap());
        assert_eq!(
            r(&mut t, vec![s("Hello"), s("J"), 0.into(), 1.into()]),
            "Jello"
        );
        assert_eq!(r(&mut t, vec![s("Hello"), s("J"), 0.into()]), "J");
        assert_eq!(
            r(&mut t, vec![s("Hello"), s("J"), 0.into(), 0.into()]),
            "JHello"
        );
        assert_eq!(
            r(&mut t, vec![s("Hello"), s("!!"), (-1).into(), 0.into()]),
            "Hell!!o"
        );
        assert_eq!(
            r(&mut t, vec![s("Hello"), s("X"), 1.into(), (-1).into()]),
            "HXo"
        );
        assert_eq!(
            r(&mut t, vec![s("Hello"), s("X"), 10.into(), 2.into()]),
            "HelloX"
        );
        assert_eq!(
            r(&mut t, vec![s("Hello"), s("X"), 1.into(), Value::Null]),
            "HX"
        );
        assert_eq!(
            r(
                &mut t,
                vec![s("Hello"), arr(&["A", "B"]), 1.into(), 1.into()]
            ),
            "HAllo"
        );
        assert_eq!(
            r(&mut t, vec![s("Hello"), arr(&[]), 1.into(), 1.into()]),
            "Hllo"
        );
        assert_eq!(
            thrown(t.call("substr_replace", vec![s("a"), s("b"), arr(&["1"])])).1,
            "substr_replace(): Argument #3 ($offset) cannot be an array when working on a single string"
        );
    }

    #[test]
    fn substr_replace_arrays() {
        let mut t = TestVm::new();
        let v = t
            .call(
                "substr_replace",
                vec![
                    arr(&["abc", "def", "ghi"]),
                    arr(&["X", "Y"]),
                    1.into(),
                    1.into(),
                ],
            )
            .unwrap();
        assert_eq!(list(&v), vec!["aXc", "dYf", "gi"]);
        let v = t
            .call(
                "substr_replace",
                vec![
                    arr(&["abc", "def"]),
                    s("-"),
                    Value::array(PhpArray::from_values(vec![Value::Int(0), Value::Int(1)])),
                    Value::array(PhpArray::from_values(vec![Value::Int(1), Value::Int(2)])),
                ],
            )
            .unwrap();
        assert_eq!(list(&v), vec!["-bc", "d-"]);
    }
}
