//! ext/standard/string.c — `strtok()`.

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// Per-request tokenizer state (`BG(strtok_string)` / `BG(strtok_last)`).
#[derive(Default)]
struct StrtokState {
    /// The string being tokenized; `None` once tokenization finished.
    string: Option<PhpStr>,
    /// Offset where the next search starts (may be one past the end).
    last: usize,
}

/// `PHP_FUNCTION(strtok)`.
fn strtok(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let token = match args.str_opt(1) {
        None => args.str(0).clone(),
        Some(tok) => {
            let state = vm.ext_data::<StrtokState>();
            state.string = Some(args.str(0).clone());
            state.last = 0;
            tok.clone()
        }
    };
    let state = vm.ext_data::<StrtokState>();
    let Some(s) = state.string.clone() else {
        vm.warning("Both arguments must be provided when starting tokenization")?;
        return Ok(Value::Bool(false));
    };
    let start = state.last;
    let end = s.len();
    if start >= end {
        // Reached the end of the string (the state is kept, as in php-src).
        return Ok(Value::Bool(false));
    }
    let mut table = [false; 256];
    for &c in token.iter() {
        table[usize::from(c)] = true;
    }
    let is_delim = |p: usize| table[usize::from(s[p])];

    // Skip leading delimiters.
    let mut p = start;
    while is_delim(p) {
        p += 1;
        if p >= end {
            state.string = None;
            return Ok(Value::Bool(false));
        }
    }
    let token_start = p;
    // `*p` is no delimiter: the token runs up to the next delimiter or the end.
    p += 1;
    while p < end && !is_delim(p) {
        p += 1;
    }
    state.last = p + 1;
    Ok(Value::String(PhpStr::from_bytes(&s[token_start..p])))
}

pub static STRTOK: NativeFunction = NativeFunction {
    sig: "strtok(string $string, ?string $token = null): string|false",
    handler: strtok,
};

pub fn register(r: &mut Registry) {
    r.function(&STRTOK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    const WARN: &str = "\nWarning: strtok(): Both arguments must be provided when starting tokenization in /t.php on line 3\n";

    fn show(v: Value) -> String {
        match v {
            Value::String(s) => format!("{:?}", s.to_string_lossy()),
            Value::Bool(false) => "false".into(),
            v => panic!("unexpected {v:?}"),
        }
    }

    /// `strtok($s, $tok)` followed by `n` calls of `strtok($tok)`.
    fn run(t: &mut TestVm, s: &[u8], tok: &[u8], n: usize) -> Vec<String> {
        let mut out = vec![show(
            t.call("strtok", vec![Value::bytes(s), Value::bytes(tok)])
                .unwrap(),
        )];
        for _ in 0..n {
            out.push(show(t.call("strtok", vec![Value::bytes(tok)]).unwrap()));
        }
        out
    }

    #[test]
    fn basic() {
        let mut t = TestVm::new();
        let got = run(&mut t, b"This testcase test strtok() function.", b" ().", 6);
        assert_eq!(
            got,
            [
                "\"This\"",
                "\"testcase\"",
                "\"test\"",
                "\"strtok\"",
                "\"function\"",
                "false",
                "false"
            ]
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn embedded_nulls() {
        // strtok_variation4.phpt
        let mut t = TestVm::new();
        assert_eq!(run(&mut t, b"\0", b"\0", 2), ["false", "false", "false"]);
        assert_eq!(t.output(), WARN.repeat(2));
        assert_eq!(
            run(&mut t, b"\0hel\0lo", b"\0", 2),
            ["\"hel\"", "\"lo\"", "false"]
        );
        assert_eq!(
            run(&mut t, b"\0\0hello\tworld\0\0", b"\0", 2),
            ["\"hello\\tworld\"", "false", "false"]
        );
        assert_eq!(t.output(), WARN);
        assert_eq!(
            run(&mut t, b"hello\\0\\0", b"\0", 1),
            ["\"hello\\\\0\\\\0\"", "false"]
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn misc_inputs() {
        // strtok_variation5.phpt
        let mut t = TestVm::new();
        assert_eq!(
            run(&mut t, b"HELLO WORLD", b"wr", 1),
            ["\"HELLO WORLD\"", "false"]
        );
        assert_eq!(
            run(&mut t, b"hello world", b"hello world", 1),
            ["false", "false"]
        );
        assert_eq!(t.output(), WARN);
        assert_eq!(
            run(&mut t, b"/thello/t/wor/ttld", b"t/", 3),
            ["\"hello\"", "\"wor\"", "\"ld\"", "false"]
        );
        assert_eq!(
            run(&mut t, b"one:$:two:!:three:#:four", b":", 5),
            ["\"one\"", "\"$\"", "\"two\"", "\"!\"", "\"three\"", "\"#\""]
        );
        assert_eq!(
            run(&mut t, b"\rhello/r/wor\rrld", b"\r", 2),
            ["\"hello/r/wor\"", "\"rld\"", "false"]
        );
    }

    #[test]
    fn changing_tokens() {
        // strtok_variation6.phpt, iteration 3 with token "/k"
        let mut t = TestVm::new();
        assert_eq!(
            run(&mut t, b"/khello\\k world/k", b"/k", 3),
            ["\"hello\\\\\"", "\" world\"", "false", "false"]
        );
        assert_eq!(t.output(), WARN);
    }

    #[test]
    fn empty_string_keeps_state() {
        let mut t = TestVm::new();
        assert_eq!(run(&mut t, b"", b" ", 2), ["false", "false", "false"]);
        assert_eq!(t.output(), "");
        // One-argument form before any two-argument call.
        let mut t = TestVm::new();
        assert_eq!(
            show(t.call("strtok", vec![Value::str("x")]).unwrap()),
            "false"
        );
        assert_eq!(t.output(), WARN);
    }
}
