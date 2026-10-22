//! Unit tests for the array functions. Expectations are ported from php-src's
//! `.phpt` files (`tests/phpt/ext/standard/tests/array/`).
//!
//! Inputs are written as PHP array literals ([`php`]) and results are compared
//! in a compact `var_export`-like notation ([`show`]).

use std::cell::RefCell;

use php_runtime::value::numeric::is_numeric_str;
use php_runtime::{
    ArrayKey, Args, NativeFunction, PResult, PhpArray, PhpRef, PhpStr, Registry, Value, Vm,
};

use crate::testing::{TestVm, thrown};

mod access;
mod build;
mod funcs;
mod iter;
mod merge;
mod setops;
mod sort;

// ---------------------------------------------------------------------------
// PHP literal parser
// ---------------------------------------------------------------------------

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn peek_word(&self) -> String {
        let mut j = self.i;
        while j < self.s.len() && (self.s[j].is_ascii_alphanumeric() || self.s[j] == b'_') {
            j += 1;
        }
        String::from_utf8_lossy(&self.s[self.i..j]).into_owned()
    }

    fn string(&mut self, quote: u8) -> Value {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.s[self.i];
            self.i += 1;
            if c == quote {
                break;
            }
            if c == b'\\' {
                let e = self.s[self.i];
                self.i += 1;
                if quote == b'\'' {
                    match e {
                        b'\\' | b'\'' => out.push(e),
                        _ => {
                            out.push(b'\\');
                            out.push(e);
                        }
                    }
                } else {
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'0' => out.push(0),
                        b'\\' | b'"' | b'$' => out.push(e),
                        _ => {
                            out.push(b'\\');
                            out.push(e);
                        }
                    }
                }
            } else {
                out.push(c);
            }
        }
        Value::String(PhpStr::from_vec(out))
    }

    fn number(&mut self) -> Value {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_digit()
                || matches!(self.s[self.i], b'.' | b'e' | b'E')
                || (matches!(self.s[self.i], b'+' | b'-')
                    && matches!(self.s[self.i - 1], b'e' | b'E')))
        {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        if t.contains(['.', 'e', 'E']) {
            Value::Float(t.parse().unwrap())
        } else {
            match t.parse::<i64>() {
                Ok(i) => Value::Int(i),
                Err(_) => Value::Float(t.parse().unwrap()),
            }
        }
    }

    fn array(&mut self, close: &str) -> Value {
        let mut a = PhpArray::new();
        loop {
            if self.eat(close) {
                break;
            }
            let v = self.value();
            if self.eat("=>") {
                let k = match &v {
                    Value::Int(i) => ArrayKey::Int(*i),
                    Value::String(s) => ArrayKey::from_php_str(s.clone()),
                    Value::Bool(b) => ArrayKey::Int(*b as i64),
                    Value::Null => ArrayKey::Str(PhpStr::new()),
                    Value::Float(f) => ArrayKey::Int(*f as i64),
                    other => panic!("bad key {other:?}"),
                };
                let val = self.value();
                a.insert(k, val);
            } else {
                a.append(v).expect("append");
            }
            if !self.eat(",") {
                assert!(self.eat(close), "expected {close} at {}", self.i);
                break;
            }
        }
        Value::array(a)
    }

    fn value(&mut self) -> Value {
        self.ws();
        let c = self.s[self.i];
        match c {
            b'[' => {
                self.i += 1;
                self.array("]")
            }
            b'\'' | b'"' => self.string(c),
            b'-' | b'0'..=b'9' | b'.' => {
                if c == b'-' && self.s[self.i + 1..].starts_with(b"INF") {
                    self.i += 4;
                    return Value::Float(f64::NEG_INFINITY);
                }
                self.number()
            }
            _ => {
                let w = self.peek_word();
                self.i += w.len();
                match w.to_ascii_lowercase().as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "null" => Value::Null,
                    "inf" => Value::Float(f64::INFINITY),
                    "nan" => Value::Float(f64::NAN),
                    "php_int_max" => Value::Int(i64::MAX),
                    "php_int_min" => Value::Int(i64::MIN),
                    "array" => {
                        assert!(self.eat("("));
                        self.array(")")
                    }
                    other => panic!("unknown word {other:?} in php literal"),
                }
            }
        }
    }
}

/// Parses a PHP literal (scalars and arrays).
pub fn php(src: &str) -> Value {
    let mut p = P {
        s: src.as_bytes(),
        i: 0,
    };
    let v = p.value();
    p.ws();
    assert_eq!(p.i, src.len(), "trailing input in {src:?}");
    v
}

// ---------------------------------------------------------------------------
// Compact rendering
// ---------------------------------------------------------------------------

fn quote(s: &[u8]) -> String {
    let mut out = String::from("'");
    for &b in s {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0 => out.push_str("\\0"),
            32..=126 => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('\'');
    out
}

fn show_float(f: f64) -> String {
    if f.is_nan() {
        "NAN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "INF".into() } else { "-INF".into() }
    } else if f == f.trunc() && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        format!("{f:?}")
    }
}

/// Renders a value: lists as `[a, b]`, other arrays as `[k => v]`.
pub fn show(v: &Value) -> String {
    match v {
        Value::Undef | Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => show_float(*f),
        Value::String(s) => quote(s),
        Value::Array(a) => {
            let items: Vec<String> = if a.is_list() {
                a.values().map(show).collect()
            } else {
                a.iter()
                    .map(|(k, v)| {
                        let k = match k {
                            ArrayKey::Int(i) => i.to_string(),
                            ArrayKey::Str(s) => quote(&s),
                        };
                        format!("{k} => {}", show(v))
                    })
                    .collect()
            };
            format!("[{}]", items.join(", "))
        }
        Value::Object(o) => format!("object({})", o.class().name),
        Value::Resource(_) => "resource".into(),
        Value::Ref(r) => show(&r.get()),
    }
}

// ---------------------------------------------------------------------------
// Test callbacks (native functions usable as PHP callables)
// ---------------------------------------------------------------------------

thread_local! {
    /// Calls recorded by the `t_log*` callbacks.
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn take_log() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

fn log(s: String) {
    LOG.with(|l| l.borrow_mut().push(s));
}

fn t_cmp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.spaceship(args.get(0), args.get(1))
}

fn t_rcmp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.spaceship(args.get(1), args.get(0))
}

fn t_gt(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(vm.compare(args.get(0), args.get(1))? > 0))
}

fn t_lt(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(vm.compare(args.get(0), args.get(1))? < 0))
}

fn t_zero(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(0))
}

fn t_double(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.mul(args.get(0), &Value::Int(2))
}

fn t_even(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let m = vm.modulo(args.get(0), &Value::Int(2))?;
    Ok(Value::Bool(matches!(m, Value::Int(0))))
}

fn t_add(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.add(args.get(0), args.get(1))
}

fn t_concat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.concat(args.get(0), args.get(1))
}

fn t_kv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = vm.to_php_string(args.get(0))?;
    let k = vm.to_php_string(args.get(1))?;
    Ok(Value::from(format!("{k}:{v}")))
}

/// `t_key_gt1($v, $k)`: true when the key is greater than 1 (or a string).
fn t_key_big(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(match args.get(1) {
        Value::Int(i) => *i > 1,
        _ => true,
    }))
}

/// `t_inc(&$v, $k, $extra = 0)`: `$v += 1 + $extra`.
fn t_inc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let r = args.by_ref(0);
    let extra = if args.is_passed(2) {
        args.get(2).clone()
    } else {
        Value::Int(0)
    };
    let cur = r.get();
    let one = vm.add(&cur, &Value::Int(1))?;
    let new = vm.add(&one, &extra)?;
    r.set(new);
    Ok(Value::Null)
}

/// `t_walk_log(&$v, $k)`: records `k=v`.
fn t_walk_log(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = vm.to_php_string(&args.by_ref(0).get())?;
    let k = vm.to_php_string(args.get(1))?;
    log(format!("{k}={v}"));
    Ok(Value::Null)
}

/// `t_log($a, $b)`: records the call and returns `$a <=> $b`.
fn t_log(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log(format!("{},{}", show(args.get(0)), show(args.get(1))));
    vm.spaceship(args.get(0), args.get(1))
}

fn t_strcmp(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (a, b) = (args.str(0), args.str(1));
    Ok(Value::Int(super::util::binary_strcmp(a, b) as i64))
}

fn t_id(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(args.get(0).clone())
}

/// `t_stringify($v)`: `"<" . $v . ">"`.
fn t_stringify(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = vm.to_php_string(args.get(0))?;
    Ok(Value::from(format!("<{s}>")))
}

/// `t_throw(...)`: throws an Exception("boom").
fn t_throw(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "boom"))
}

macro_rules! natives {
    ($($sig:literal => $handler:ident),* $(,)?) => {
        static TEST_FUNCS: &[NativeFunction] = &[
            $(NativeFunction { sig: $sig, handler: $handler },)*
        ];
    };
}

natives! {
    "t_cmp(mixed $a, mixed $b): int" => t_cmp,
    "t_rcmp(mixed $a, mixed $b): int" => t_rcmp,
    "t_gt(mixed $a, mixed $b)" => t_gt,
    "t_lt(mixed $a, mixed $b)" => t_lt,
    "t_zero(mixed $a, mixed $b): int" => t_zero,
    "t_double(mixed $x): mixed" => t_double,
    "t_even(mixed $x): bool" => t_even,
    "t_add(mixed $a, mixed $b): mixed" => t_add,
    "t_concat(mixed $a, mixed $b): mixed" => t_concat,
    "t_kv(mixed $v, mixed $k): string" => t_kv,
    "t_key_big(mixed $v, mixed $k): bool" => t_key_big,
    "t_inc(&$v, mixed $k, mixed $extra = UNKNOWN)" => t_inc,
    "t_walk_log(&$v, mixed $k)" => t_walk_log,
    "t_log(mixed $a, mixed $b): int" => t_log,
    "t_strcmp(string $a, string $b): int" => t_strcmp,
    "t_id(mixed $x): mixed" => t_id,
    "t_stringify(mixed $x): string" => t_stringify,
    "t_throw(mixed ...$args)" => t_throw,
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub fn vm() -> TestVm {
    let mut t = TestVm::new();
    {
        let mut r = Registry::new(&mut t.vm, "standard");
        for f in TEST_FUNCS {
            r.function(f);
        }
    }
    take_log();
    t
}

/// Calls `name(&$first, ...rest)` and returns the result and the final value
/// of the by-reference argument.
pub fn call_ref(
    t: &mut TestVm,
    name: &str,
    first: Value,
    rest: Vec<Value>,
) -> (PResult<Value>, Value) {
    let r = PhpRef::new(first);
    let mut args = vec![Value::Ref(r.clone())];
    args.extend(rest);
    let ret = t.call(name, args);
    (ret, r.get())
}

/// Calls `name(...args)` and renders the result (panics on exceptions).
pub fn run(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    match t.call(name, args) {
        Ok(v) => show(&v),
        Err(e) => panic!("{name}() threw: {:?}", thrown(Err(e))),
    }
}

/// Calls `name(...args)` expecting an exception; returns `"Class: message"`.
pub fn run_err(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let (c, m) = thrown(t.call(name, args));
    format!("{c}: {m}")
}

/// Sorts `array` with `name(&$array, ...flags)`; returns the sorted array.
pub fn sorted(t: &mut TestVm, name: &str, array: &str, rest: Vec<Value>) -> String {
    let (r, a) = call_ref(t, name, php(array), rest);
    assert_eq!(r.map(|v| show(&v)).ok().as_deref(), Some("true"), "{name}");
    show(&a)
}

pub fn s(x: &str) -> Value {
    Value::str(x)
}

pub fn i(x: i64) -> Value {
    Value::Int(x)
}

/// Whether `v` is a numeric string (helper for assertions).
#[allow(dead_code)]
pub fn is_num(v: &str) -> bool {
    is_numeric_str(v.as_bytes()).is_some()
}

#[test]
fn dsl_roundtrip() {
    assert_eq!(show(&php("[1, 'a', 2.5, true, null]")), "[1, 'a', 2.5, true, null]");
    assert_eq!(
        show(&php("['a' => 1, 5 => [1, 2], '7' => 'x']")),
        "['a' => 1, 5 => [1, 2], 7 => 'x']"
    );
    assert_eq!(show(&php("[3 => 'a']")), "[3 => 'a']");
}

#[test]
fn smoke() {
    let mut t = vm();
    assert_eq!(run(&mut t, "count", vec![php("[1, 2]")]), "2");
}
