//! Splitting and joining: `explode`, `implode`/`join`, `str_split`,
//! `chunk_split`, `wordwrap`, `nl2br`, `str_word_count`.

#![allow(clippy::collapsible_if)]

use php_runtime::value::array::ArrayKey;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::util::{memnstr_from, php_charmask};

// ---------------------------------------------------------------------------
// explode / implode
// ---------------------------------------------------------------------------

/// `php_explode` / `php_explode_negative_limit` for a non-empty `delim` and `s`.
pub fn php_explode(delim: &[u8], s: &PhpStr, limit: i64) -> PhpArray {
    let b = s.as_bytes();
    let mut parts: Vec<Value> = Vec::new();
    if limit > 1 {
        let mut limit = limit;
        let mut p1 = 0;
        while let Some(p2) = memnstr_from(b, delim, p1) {
            parts.push(Value::bytes(&b[p1..p2]));
            p1 = p2 + delim.len();
            limit -= 1;
            if limit <= 1 {
                break;
            }
        }
        parts.push(Value::bytes(&b[p1..]));
    } else if limit < 0 {
        // All components except the last abs(limit).
        let mut positions = vec![0usize];
        let mut p1 = 0;
        while let Some(p2) = memnstr_from(b, delim, p1) {
            p1 = p2 + delim.len();
            positions.push(p1);
        }
        if positions.len() > 1 {
            let found = positions.len() as i64;
            let to_return = limit + found;
            for i in 0..to_return.max(0) as usize {
                parts.push(Value::bytes(
                    &b[positions[i]..positions[i + 1] - delim.len()],
                ));
            }
        }
    } else {
        parts.push(Value::String(s.clone()));
    }
    PhpArray::from_values(parts)
}

fn explode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let delim = args.str(0).clone();
    let s = args.str(1).clone();
    let limit = args.int(2);
    if delim.is_empty() {
        return Err(vm.argument_value_error(
            1,
            "must not be empty, use str_split() to split a string into characters",
        ));
    }
    if s.is_empty() {
        let mut a = PhpArray::new();
        if limit >= 0 {
            let _ = a.append(Value::empty_string());
        }
        return Ok(Value::array(a));
    }
    Ok(Value::array(php_explode(&delim, &s, limit)))
}

/// `php_implode`.
pub fn php_implode(vm: &mut Vm, glue: &[u8], pieces: &PhpArray) -> PResult<PhpStr> {
    if pieces.is_empty() {
        return Ok(PhpStr::new());
    }
    if pieces.len() == 1 {
        let v = pieces.values().next().expect("one element");
        return vm.to_php_string(v);
    }
    let mut out: Vec<u8> = Vec::new();
    let mut first = true;
    for v in pieces.values() {
        if !first {
            out.extend_from_slice(glue);
        }
        first = false;
        match v {
            Value::String(s) => out.extend_from_slice(s),
            Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            v => {
                let s = vm.to_php_string(v)?;
                out.extend_from_slice(&s);
            }
        }
    }
    Ok(PhpStr::from_vec(out))
}

fn implode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let arg1 = args.get(0).clone();
    let pieces = args.get(1).clone();
    let (glue, pieces) = match (&arg1, &pieces) {
        (Value::Array(a), Value::Null | Value::Undef) => (PhpStr::new(), a.clone()),
        (_, Value::Null | Value::Undef) => {
            let name = vm.current_function_name();
            return Err(vm.throw_type_error(format!(
                "{name}(): If argument #1 ($separator) is of type string, argument #2 ($array) must be of type array, null given"
            )));
        }
        (Value::Array(_), _) => {
            return Err(vm.argument_type_error(1, "must be of type string, array given"));
        }
        (Value::String(g), Value::Array(a)) => (g.clone(), a.clone()),
        _ => unreachable!("string|array, ?array after ZPP"),
    };
    Ok(Value::String(php_implode(vm, &glue, &pieces)?))
}

// ---------------------------------------------------------------------------
// str_split / chunk_split
// ---------------------------------------------------------------------------

fn str_split(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let split_length = args.int(1);
    if split_length <= 0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    if split_length as u64 >= s.len() as u64 {
        if s.is_empty() {
            return Ok(Value::empty_array());
        }
        return Ok(Value::array(PhpArray::from_values(vec![Value::String(s)])));
    }
    let parts = s
        .chunks(split_length as usize)
        .map(Value::bytes)
        .collect::<Vec<_>>();
    Ok(Value::array(PhpArray::from_values(parts)))
}

fn chunk_split(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let chunklen = args.int(1);
    let end = args.str(2).clone();
    if chunklen <= 0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    if chunklen as u64 > s.len() as u64 {
        // To maintain BC, return the original string plus the ending.
        return Ok(Value::String(s.concat(&end)));
    }
    if s.is_empty() {
        return Ok(Value::empty_string());
    }
    let mut out = Vec::with_capacity(s.len() + (s.len() / chunklen as usize + 1) * end.len());
    for chunk in s.chunks(chunklen as usize) {
        out.extend_from_slice(chunk);
        out.extend_from_slice(&end);
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

// ---------------------------------------------------------------------------
// wordwrap / nl2br
// ---------------------------------------------------------------------------

fn wordwrap(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let text = args.str(0).clone();
    let linelength = args.int(1);
    let brk = args.str(2).clone();
    let docut = args.bool(3);

    if text.is_empty() {
        return Ok(Value::empty_string());
    }
    if brk.is_empty() {
        return Err(vm.argument_value_error(3, "must not be empty"));
    }
    if linelength == 0 && docut {
        return Err(vm.argument_value_error(4, "cannot be true when argument #2 ($width) is 0"));
    }
    let t = text.as_bytes();
    let len = t.len() as i64;

    if brk.len() == 1 && !docut {
        // Single-character break: no additional storage needed.
        let mut new = t.to_vec();
        let (mut laststart, mut lastspace) = (0i64, 0i64);
        for current in 0..len {
            let c = t[current as usize];
            if c == brk[0] {
                laststart = current + 1;
                lastspace = laststart;
            } else if c == b' ' {
                if current - laststart >= linelength {
                    new[current as usize] = brk[0];
                    laststart = current + 1;
                }
                lastspace = current;
            } else if current - laststart >= linelength && laststart != lastspace {
                new[lastspace as usize] = brk[0];
                laststart = lastspace + 1;
            }
        }
        return Ok(Value::String(PhpStr::from_vec(new)));
    }

    // Multiple character line break or forced cut.
    let blen = brk.len() as i64;
    let mut new: Vec<u8> = Vec::with_capacity(t.len() + t.len() / 8);
    let (mut laststart, mut lastspace) = (0i64, 0i64);
    let mut current = 0i64;
    while current < len {
        let c = t[current as usize];
        if c == brk[0]
            && current + blen <= len
            && t[current as usize..(current + blen) as usize] == *brk.as_bytes()
        {
            // An existing break: copy through it and fix up laststart/lastspace.
            new.extend_from_slice(&t[laststart as usize..(current + blen) as usize]);
            current += blen - 1;
            laststart = current + 1;
            lastspace = laststart;
        } else if c == b' ' {
            // A space at the line boundary: copy and insert a break, or just
            // keep track of it.
            if current - laststart >= linelength {
                new.extend_from_slice(&t[laststart as usize..current as usize]);
                new.extend_from_slice(&brk);
                laststart = current + 1;
            }
            lastspace = current;
        } else if current - laststart >= linelength && docut && laststart >= lastspace {
            // Cutting, enough characters accumulated and no space on this line.
            new.extend_from_slice(&t[laststart as usize..current as usize]);
            new.extend_from_slice(&brk);
            laststart = current;
            lastspace = current;
        } else if current - laststart >= linelength && laststart < lastspace {
            // The current word overflows: copy back up to the last space,
            // insert a break and move laststart up.
            new.extend_from_slice(&t[laststart as usize..lastspace as usize]);
            new.extend_from_slice(&brk);
            lastspace += 1;
            laststart = lastspace;
        }
        current += 1;
    }
    // Copy over any stragglers.
    if laststart < current {
        new.extend_from_slice(&t[laststart as usize..current as usize]);
    }
    Ok(Value::String(PhpStr::from_vec(new)))
}

fn nl2br(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    let is_xhtml = args.bool(1);
    let b = s.as_bytes();
    let br: &[u8] = if is_xhtml { b"<br />" } else { b"<br>" };
    if !b.iter().any(|&c| c == b'\r' || c == b'\n') {
        return Ok(Value::String(s.clone()));
    }
    let mut out = Vec::with_capacity(b.len() + 16);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\r' || c == b'\n' {
            out.extend_from_slice(br);
            out.push(c);
            // `\r\n` and `\n\r` count as one line break.
            if let Some(&n) = b.get(i + 1) {
                if (c == b'\r' && n == b'\n') || (c == b'\n' && n == b'\r') {
                    out.push(n);
                    i += 1;
                }
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

// ---------------------------------------------------------------------------
// str_word_count
// ---------------------------------------------------------------------------

fn str_word_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let ty = args.int(1);
    let char_list = args.str_opt(2).cloned();
    let mut result = PhpArray::new();
    match ty {
        1 | 2 => {
            if s.is_empty() {
                return Ok(Value::array(result));
            }
        }
        0 => {
            if s.is_empty() {
                return Ok(Value::Int(0));
            }
        }
        _ => return Err(vm.argument_value_error(2, "must be a valid format value")),
    }
    let mask = match &char_list {
        Some(cl) => Some(php_charmask(vm, cl)?),
        None => None,
    };
    let allowed = |c: u8| mask.as_ref().is_some_and(|m| m[c as usize]);
    let b = s.as_bytes();
    let mut p = 0usize;
    let mut e = b.len();
    // The first character cannot be ' or -, unless explicitly allowed.
    if (b[0] == b'\'' && !allowed(b'\'')) || (b[0] == b'-' && !allowed(b'-')) {
        p += 1;
    }
    // The last character cannot be -, unless explicitly allowed.
    if b[e - 1] == b'-' && !allowed(b'-') {
        e -= 1;
    }
    let mut word_count = 0i64;
    while p < e {
        let start = p;
        while p < e
            && (b[p].is_ascii_alphabetic() || allowed(b[p]) || b[p] == b'\'' || b[p] == b'-')
        {
            p += 1;
        }
        if p > start {
            match ty {
                1 => {
                    let _ = result.append(Value::bytes(&b[start..p]));
                }
                2 => {
                    result.insert(ArrayKey::Int(start as i64), Value::bytes(&b[start..p]));
                }
                _ => word_count += 1,
            }
        }
        p += 1;
    }
    Ok(if ty == 0 {
        Value::Int(word_count)
    } else {
        Value::array(result)
    })
}

pub static EXPLODE: NativeFunction = NativeFunction {
    sig: "explode(string $separator, string $string, int $limit = PHP_INT_MAX): array",
    handler: explode,
};
pub static IMPLODE: NativeFunction = NativeFunction {
    sig: "implode(string|array $separator, ?array $array = null): string",
    handler: implode,
};
pub static JOIN: NativeFunction = NativeFunction {
    sig: "join(string|array $separator, ?array $array = null): string",
    handler: implode,
};
pub static STR_SPLIT: NativeFunction = NativeFunction {
    sig: "str_split(string $string, int $length = 1): array",
    handler: str_split,
};
pub static CHUNK_SPLIT: NativeFunction = NativeFunction {
    sig: r#"chunk_split(string $string, int $length = 76, string $separator = "\r\n"): string"#,
    handler: chunk_split,
};
pub static WORDWRAP: NativeFunction = NativeFunction {
    sig: r#"wordwrap(string $string, int $width = 75, string $break = "\n", bool $cut_long_words = false): string"#,
    handler: wordwrap,
};
pub static NL2BR: NativeFunction = NativeFunction {
    sig: "nl2br(string $string, bool $use_xhtml = true): string",
    handler: nl2br,
};
pub static STR_WORD_COUNT: NativeFunction = NativeFunction {
    sig: "str_word_count(string $string, int $format = 0, ?string $characters = null): array|int",
    handler: str_word_count,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &EXPLODE,
        &IMPLODE,
        &JOIN,
        &STR_SPLIT,
        &CHUNK_SPLIT,
        &WORDWRAP,
        &NL2BR,
        &STR_WORD_COUNT,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn s(x: &str) -> Value {
        Value::str(x)
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

    fn arr(items: &[&str]) -> Value {
        Value::array(items.iter().map(|x| Value::str(x)).collect::<PhpArray>())
    }

    #[test]
    fn explode_limits() {
        let mut t = TestVm::new();
        let e = |t: &mut TestVm, a: Vec<Value>| list(&t.call("explode", a).unwrap());
        assert_eq!(e(&mut t, vec![s(","), s("a,b,c")]), ["a", "b", "c"]);
        assert_eq!(e(&mut t, vec![s(","), s("a,b,c"), 2.into()]), ["a", "b,c"]);
        assert_eq!(e(&mut t, vec![s(","), s("a,b,c"), 1.into()]), ["a,b,c"]);
        assert_eq!(e(&mut t, vec![s(","), s("a,b,c"), 0.into()]), ["a,b,c"]);
        assert_eq!(e(&mut t, vec![s(","), s("a,b,c"), (-1).into()]), ["a", "b"]);
        assert_eq!(e(&mut t, vec![s(","), s("a,b,c"), (-2).into()]), ["a"]);
        assert!(e(&mut t, vec![s(","), s("a,b,c"), (-3).into()]).is_empty());
        assert!(e(&mut t, vec![s(","), s("abc"), (-1).into()]).is_empty());
        assert_eq!(e(&mut t, vec![s(","), s("")]), [""]);
        assert!(e(&mut t, vec![s(","), s(""), (-1).into()]).is_empty());
        assert_eq!(e(&mut t, vec![s(","), s(",a,"), 10.into()]), ["", "a", ""]);
        assert_eq!(
            e(&mut t, vec![s("ab"), s("1ab2ab3"), 2.into()]),
            ["1", "2ab3"]
        );
        assert_eq!(
            thrown(t.call("explode", vec![s(""), s("a")])),
            (
                "ValueError".into(),
                "explode(): Argument #1 ($separator) must not be empty, use str_split() to split a string into characters"
                    .into()
            )
        );
    }

    #[test]
    fn implode_forms() {
        let mut t = TestVm::new();
        assert_eq!(
            st(t.call("implode", vec![s(", "), arr(&["a", "b", "c"])])
                .unwrap()),
            "a, b, c"
        );
        assert_eq!(st(t.call("implode", vec![arr(&["a", "b"])]).unwrap()), "ab");
        assert_eq!(st(t.call("join", vec![s("-"), arr(&["x"])]).unwrap()), "x");
        assert_eq!(st(t.call("implode", vec![s("-"), arr(&[])]).unwrap()), "");
        let mixed = Value::array(PhpArray::from_values(vec![
            Value::Int(1),
            Value::Float(1.5),
            Value::Bool(true),
            Value::Null,
            Value::str("x"),
        ]));
        assert_eq!(
            st(t.call("implode", vec![s("|"), mixed]).unwrap()),
            "1|1.5|1||x"
        );
        assert_eq!(
            thrown(t.call("implode", vec![s(","), Value::Null])).1,
            "implode(): If argument #1 ($separator) is of type string, argument #2 ($array) must be of type array, null given"
        );
        assert_eq!(
            thrown(t.call("join", vec![s(",")])).1,
            "join(): If argument #1 ($separator) is of type string, argument #2 ($array) must be of type array, null given"
        );
        assert_eq!(
            thrown(t.call("implode", vec![arr(&["a"]), arr(&["b"])])).1,
            "implode(): Argument #1 ($separator) must be of type string, array given"
        );
        // Nested arrays convert with a warning.
        let nested = Value::array(PhpArray::from_values(vec![arr(&["a"]), s("b")]));
        assert_eq!(
            st(t.call("implode", vec![s(","), nested]).unwrap()),
            "Array,b"
        );
        assert_eq!(
            t.output(),
            "\nWarning: Array to string conversion in /t.php on line 3\n"
        );
    }

    #[test]
    fn split_and_chunk() {
        let mut t = TestVm::new();
        assert_eq!(
            list(&t.call("str_split", vec![s("abcdef"), 4.into()]).unwrap()),
            ["abcd", "ef"]
        );
        assert_eq!(
            list(&t.call("str_split", vec![s("abc")]).unwrap()),
            ["a", "b", "c"]
        );
        assert_eq!(
            list(&t.call("str_split", vec![s("abc"), 10.into()]).unwrap()),
            ["abc"]
        );
        assert!(list(&t.call("str_split", vec![s("")]).unwrap()).is_empty());
        assert_eq!(
            thrown(t.call("str_split", vec![s("abc"), 0.into()])).1,
            "str_split(): Argument #2 ($length) must be greater than 0"
        );
        assert_eq!(
            st(t.call("chunk_split", vec![s("abcd"), 2.into(), s("-")])
                .unwrap()),
            "ab-cd-"
        );
        assert_eq!(
            st(t.call("chunk_split", vec![s("abcde"), 2.into(), s("-")])
                .unwrap()),
            "ab-cd-e-"
        );
        assert_eq!(
            st(t.call("chunk_split", vec![s("abc")]).unwrap()),
            "abc\r\n"
        );
        assert_eq!(
            st(t.call("chunk_split", vec![s(""), 2.into()]).unwrap()),
            "\r\n"
        );
        assert_eq!(
            thrown(t.call("chunk_split", vec![s("abc"), 0.into()])).1,
            "chunk_split(): Argument #2 ($length) must be greater than 0"
        );
    }

    #[test]
    fn wordwrap_cases() {
        let mut t = TestVm::new();
        let w = |t: &mut TestVm, a: Vec<Value>| st(t.call("wordwrap", a).unwrap());
        assert_eq!(
            w(
                &mut t,
                vec![
                    s("The quick brown fox sat over the lazy dog"),
                    15.into(),
                    s("\n"),
                    true.into()
                ]
            ),
            "The quick brown\nfox sat over\nthe lazy dog"
        );
        let wb = |t: &mut TestVm, text: &str, width: i64, brk: &str, cut: bool| {
            w(t, vec![s(text), width.into(), s(brk), cut.into()])
        };
        // tests/phpt/ext/standard/tests/strings/wordwrap.phpt
        assert_eq!(
            w(&mut t, vec![s("12345 12345 12345 12345")]),
            "12345 12345 12345 12345"
        );
        assert_eq!(
            w(
                &mut t,
                vec![s("12345 12345 1234567890 1234567890"), 12.into()]
            ),
            "12345 12345\n1234567890\n1234567890"
        );
        assert_eq!(
            w(&mut t, vec![s("12345 12345 12345 12345"), 0.into()]),
            "12345\n12345\n12345\n12345"
        );
        assert_eq!(
            w(
                &mut t,
                vec![s("12345 12345 12345 12345"), 0.into(), s("ab")]
            ),
            "12345ab12345ab12345ab12345"
        );
        assert_eq!(
            w(
                &mut t,
                vec![s("12345 12345 1234567890 1234567890"), 12.into(), s("ab")]
            ),
            "12345 12345ab1234567890ab1234567890"
        );
        assert_eq!(
            w(&mut t, vec![s("123ab123ab123"), 3.into(), s("ab")]),
            "123ab123ab123"
        );
        assert_eq!(
            w(&mut t, vec![s("123ab123ab123"), 5.into(), s("ab")]),
            "123ab123ab123"
        );
        assert_eq!(
            w(&mut t, vec![s("123  123ab123"), 3.into(), s("ab")]),
            "123ab 123ab123"
        );
        assert_eq!(
            w(&mut t, vec![s("123 123ab123"), 5.into(), s("ab")]),
            "123ab123ab123"
        );
        assert_eq!(
            w(&mut t, vec![s("123 123 123"), 10.into(), s("ab")]),
            "123 123ab123"
        );
        assert_eq!(wb(&mut t, "123ab123ab123", 3, "ab", true), "123ab123ab123");
        assert_eq!(wb(&mut t, "123ab123ab123", 5, "ab", true), "123ab123ab123");
        assert_eq!(
            wb(&mut t, "123  123ab123", 3, "ab", true),
            "123ab 12ab3ab123"
        );
        assert_eq!(wb(&mut t, "123  123ab123", 5, "ab", true), "123 ab123ab123");
        assert_eq!(wb(&mut t, "123  123  123", 8, "ab", true), "123  123ab 123");
        assert_eq!(
            wb(&mut t, "123  12345  123", 8, "ab", true),
            "123 ab12345 ab123"
        );
        assert_eq!(wb(&mut t, "1234", 1, "ab", true), "1ab2ab3ab4");
        assert_eq!(
            wb(&mut t, "12345 1234567890", 5, "|", true),
            "12345|12345|67890"
        );
        assert_eq!(
            wb(&mut t, "123 1234567890 123", 10, "|==", true),
            "123|==1234567890|==123"
        );
        // wordwrap_trailing_break.phpt
        assert_eq!(wb(&mut t, "abc\n", 3, "\n", true), "abc\n");
        assert_eq!(wb(&mut t, "ab cd\n", 5, "\n", true), "ab cd\n");
        assert_eq!(wb(&mut t, "abcd\n", 3, "\n", true), "abc\nd\n");
        assert_eq!(wb(&mut t, "abc\nabc\n", 3, "\n", true), "abc\nabc\n");
        assert_eq!(wb(&mut t, "abc-", 3, "-", true), "abc-");
        assert_eq!(wb(&mut t, "abc\r\n", 3, "\r\n", true), "abc\r\n");
        assert_eq!(wb(&mut t, "\r\n", 1, "\r\n", true), "\r\n");
        assert_eq!(wb(&mut t, "ab cd\r\n", 5, "\r\n", false), "ab cd\r\n");
        // wordwrap_basic.phpt
        let str = "The quick brown foooooooooox jummmmmmmmmmmmped over the lazzzzzzzzzzzy doooooooooooooooooooooog.";
        assert_eq!(
            w(&mut t, vec![s(str)]),
            "The quick brown foooooooooox jummmmmmmmmmmmped over the lazzzzzzzzzzzy\ndoooooooooooooooooooooog."
        );
        assert_eq!(
            wb(&mut t, str, 10, "<br />\\n", true),
            "The quick<br />\\nbrown<br />\\nfooooooooo<br />\\nox<br />\\njummmmmmmm<br />\\nmmmmped<br />\\nover the<br />\\nlazzzzzzzz<br />\\nzzzy<br />\\ndooooooooo<br />\\noooooooooo<br />\\nooog."
        );
        assert_eq!(
            wb(&mut t, str, 10, "<br />\\n", false),
            "The quick<br />\\nbrown<br />\\nfoooooooooox<br />\\njummmmmmmmmmmmped<br />\\nover the<br />\\nlazzzzzzzzzzzy<br />\\ndoooooooooooooooooooooog."
        );
        assert_eq!(
            w(
                &mut t,
                vec![
                    s("A very long woooooooooooord."),
                    8.into(),
                    s("\n"),
                    true.into()
                ]
            ),
            "A very\nlong\nwooooooo\nooooord."
        );
        assert_eq!(
            w(
                &mut t,
                vec![
                    s("A very long woooooooooooord."),
                    8.into(),
                    s("\n"),
                    false.into()
                ]
            ),
            "A very\nlong\nwoooooooooooord."
        );
        assert_eq!(w(&mut t, vec![s("")]), "");
        assert_eq!(w(&mut t, vec![s("abc def"), 3.into(), s("|")]), "abc|def");
        assert_eq!(
            thrown(t.call("wordwrap", vec![s("abc"), 5.into(), s("")])).1,
            "wordwrap(): Argument #3 ($break) must not be empty"
        );
        assert_eq!(
            thrown(t.call("wordwrap", vec![s("abc"), 0.into(), s("\n"), true.into()])).1,
            "wordwrap(): Argument #4 ($cut_long_words) cannot be true when argument #2 ($width) is 0"
        );
    }

    #[test]
    fn nl2br_cases() {
        let mut t = TestVm::new();
        let n = |t: &mut TestVm, a: Vec<Value>| st(t.call("nl2br", a).unwrap());
        assert_eq!(
            n(&mut t, vec![s("a\nb\r\nc\rd\n\re")]),
            "a<br />\nb<br />\r\nc<br />\rd<br />\n\re"
        );
        assert_eq!(n(&mut t, vec![s("a\nb"), false.into()]), "a<br>\nb");
        assert_eq!(n(&mut t, vec![s("abc")]), "abc");
        assert_eq!(n(&mut t, vec![s("\n\n")]), "<br />\n<br />\n");
    }

    #[test]
    fn word_count() {
        let mut t = TestVm::new();
        let c = |t: &mut TestVm, a: Vec<Value>| t.call("str_word_count", a).unwrap();
        assert_eq!(
            c(&mut t, vec![s("Hello fri3nd, you're looking good today!")]).as_int(),
            Some(7)
        );
        assert_eq!(
            list(&c(&mut t, vec![s("Hello fri3nd, you're"), 1.into()])),
            ["Hello", "fri", "nd", "you're"]
        );
        let v = c(&mut t, vec![s("Hello fri3nd, you're"), 2.into()]);
        match &v {
            Value::Array(a) => {
                assert_eq!(a.get_int(0).unwrap().as_str().unwrap(), "Hello");
                assert_eq!(a.get_int(6).unwrap().as_str().unwrap(), "fri");
                assert_eq!(a.get_int(10).unwrap().as_str().unwrap(), "nd");
                assert_eq!(a.get_int(14).unwrap().as_str().unwrap(), "you're");
            }
            _ => panic!(),
        }
        assert_eq!(
            list(&c(
                &mut t,
                vec![s("Hello fri3nd, you're"), 1.into(), s("0..9")]
            )),
            ["Hello", "fri3nd", "you're"]
        );
        assert_eq!(c(&mut t, vec![s("-abc- 'x -")]).as_int(), Some(2));
        assert_eq!(c(&mut t, vec![s("")]).as_int(), Some(0));
        assert_eq!(
            thrown(t.call("str_word_count", vec![s("a"), 3.into()])).1,
            "str_word_count(): Argument #2 ($format) must be a valid format value"
        );
    }
}
