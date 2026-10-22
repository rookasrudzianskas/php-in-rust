//! Helpers for the ext/pcre unit tests: a PHP "session" around [`TestVm`],
//! value builders, and `var_dump()` / `print_r()` renderings that match PHP's
//! output so `--EXPECT--` sections of the `.phpt` files can be reused verbatim.

#![allow(dead_code)]

use std::cell::RefCell;

use php_runtime::{
    Args, ArrayKey, Flow, NativeFunction, PResult, PhpArray, PhpRef, Registry, Value, Vm,
};

use crate::testing::TestVm;

/// A PHP session: a VM with every module registered and the helper callbacks
/// below, plus shorthands for calling `preg_*` functions.
pub struct P {
    pub t: TestVm,
    /// The phpt this session transliterates (see [`P::phpt`]).
    name: String,
}

impl P {
    pub fn new() -> P {
        let mut t = TestVm::new();
        let mut r = Registry::new(&mut t.vm, "pcre_test");
        r.functions(&[
            &CB_UPPER,
            &CB_DUMP,
            &CB_FIRST_GROUP,
            &CB_RET_INT,
            &CB_RET_ARRAY,
            &CB_THROW,
            &CB_BRACKET,
            &CB_COUNT,
            &CB_RUST0,
            &CB_RUST1,
            &CB_RUST2,
            &CB_RUST3,
        ]);
        P {
            t,
            name: String::new(),
        }
    }

    /// A session transliterating `tests/phpt/ext/pcre/tests/<name>.phpt`:
    /// the script runs as `/phpt/<name>.php`, everything it prints (via
    /// [`P::dump`], [`P::echo`], warnings...) is collected and
    /// [`P::finish`] compares it with the phpt's expected output.
    pub fn phpt(name: &str) -> P {
        let mut p = P::new();
        p.name = name.to_string();
        p.t.vm.current_file = format!("/phpt/{name}.php").into();
        p
    }

    /// `var_dump($v)`.
    pub fn dump(&mut self, v: &Value) {
        let text = var_dump_bytes(v);
        self.echo_bytes(&text);
    }

    /// `var_dump(...$values)`.
    pub fn dump_all(&mut self, values: &[Value]) {
        for v in values {
            self.dump(v);
        }
    }

    /// `var_export($v)`.
    pub fn export(&mut self, v: &Value) {
        let text = var_export_bytes(v);
        self.echo_bytes(&text);
    }

    /// `print_r($v)`.
    pub fn print_r(&mut self, v: &Value) {
        let text = print_r_bytes(v);
        self.echo_bytes(&text);
    }

    /// `echo $text`.
    pub fn echo(&mut self, text: &str) {
        self.echo_bytes(text.as_bytes());
    }

    pub fn echo_bytes(&mut self, text: &[u8]) {
        self.t.vm.echo(text).expect("echo");
    }

    /// Compares the output so far with the phpt's `--EXPECT--`/`--EXPECTF--`.
    #[track_caller]
    pub fn finish(&mut self) {
        let out = self.t.out.take();
        super::phpt::assert_phpt(&self.name.clone(), &out);
    }

    pub fn vm(&mut self) -> &mut Vm {
        &mut self.t.vm
    }

    /// `new stdClass`.
    pub fn std_object(&mut self) -> Value {
        let class = self.t.vm.find_class("stdClass").expect("stdClass");
        Value::Object(self.t.vm.instantiate(&class).expect("instantiate"))
    }

    /// Installs `f` as the PHP callback `cb_rust<slot>` (slots 0..4) and
    /// returns its name for use as a `callable` argument. This stands in for
    /// the closures and functions the phpt scripts define; `f` gets the call
    /// arguments (`args.get(0)` is the match array).
    pub fn callback(
        &mut self,
        slot: usize,
        f: impl FnMut(&mut Vm, &Args) -> PResult<Value> + 'static,
    ) -> Value {
        CALLBACKS.with(|c| c.borrow_mut()[slot] = Some(Box::new(f)));
        Value::String(format!("cb_rust{slot}").into())
    }

    /// Calls a function, panicking if it throws.
    pub fn call(&mut self, name: &str, args: Vec<Value>) -> Value {
        match self.t.call(name, args) {
            Ok(v) => v,
            Err(Flow::Exception(e)) => panic!(
                "{name}() threw {}: {}",
                e.class().name,
                Vm::exception_message(&e)
            ),
            Err(f) => panic!("{name}() failed with {f:?}"),
        }
    }

    /// Calls a function that must throw; returns (class, message).
    pub fn call_err(&mut self, name: &str, args: Vec<Value>) -> (String, String) {
        crate::testing::thrown(self.t.call(name, args))
    }

    /// `try { f(...) } catch (Throwable $e) { echo $e::class, ': ',
    /// $e->getMessage(), "\n"; }`: echoes the exception and returns `None`
    /// when the call threw.
    pub fn try_call(&mut self, name: &str, args: Vec<Value>) -> Option<Value> {
        match self.t.call(name, args) {
            Ok(v) => Some(v),
            Err(Flow::Exception(e)) => {
                let line = format!("{}: {}\n", e.class().name, Vm::exception_message(&e));
                self.echo(&line);
                None
            }
            Err(f) => panic!("{name}() failed with {f:?}"),
        }
    }

    /// `var_dump()` of the result of a call.
    pub fn dump_call(&mut self, name: &str, args: Vec<Value>) -> String {
        let v = self.call(name, args);
        var_dump(&v)
    }

    /// Drains the output written so far (displayed warnings and notices).
    pub fn out(&mut self) -> String {
        self.t.output()
    }

    pub fn ini(&mut self, name: &str, value: &str) {
        assert!(self.t.vm.ini_set(name, value).is_some(), "ini_set({name})");
    }

    pub fn last_error(&mut self) -> i64 {
        self.call("preg_last_error", vec![]).as_int().unwrap()
    }

    pub fn last_error_msg(&mut self) -> String {
        self.call("preg_last_error_msg", vec![])
            .as_str()
            .unwrap()
            .to_string()
    }

    // ----- preg_match family with a fresh `$matches` --------------------

    /// `preg_match($re, $subject, $m, $flags, $offset)`; returns the result
    /// and `$m`.
    pub fn match_(&mut self, re: &str, subject: &str, flags: i64, offset: i64) -> (Value, Value) {
        let m = nref();
        let r = self.call(
            "preg_match",
            vec![
                s(re),
                s(subject),
                Value::Ref(m.clone()),
                Value::Int(flags),
                Value::Int(offset),
            ],
        );
        (r, m.get())
    }

    pub fn match_all(
        &mut self,
        re: &str,
        subject: &str,
        flags: i64,
        offset: i64,
    ) -> (Value, Value) {
        let m = nref();
        let r = self.call(
            "preg_match_all",
            vec![
                s(re),
                s(subject),
                Value::Ref(m.clone()),
                Value::Int(flags),
                Value::Int(offset),
            ],
        );
        (r, m.get())
    }

    /// `preg_match($re, $subject, $m)`: `var_dump`s the result and `$m`.
    pub fn dump_match(&mut self, re: &str, subject: &str) {
        let (r, m) = self.match_(re, subject, 0, 0);
        self.dump(&r);
        self.dump(&m);
    }

    /// `preg_match_all($re, $subject, $m, $flags)`: `var_dump`s the result and `$m`.
    pub fn dump_match_all(&mut self, re: &str, subject: &str, flags: i64) {
        let (r, m) = self.match_all(re, subject, flags, 0);
        self.dump(&r);
        self.dump(&m);
    }
}

impl Default for P {
    fn default() -> Self {
        P::new()
    }
}

// ----- value builders ---------------------------------------------------

pub fn s(x: &str) -> Value {
    Value::str(x)
}

pub fn sb(x: &[u8]) -> Value {
    Value::bytes(x)
}

pub fn int(x: i64) -> Value {
    Value::Int(x)
}

/// A fresh by-reference slot holding `null`.
pub fn nref() -> PhpRef {
    PhpRef::new(Value::Null)
}

/// Passes `slot` by reference.
pub fn rf(slot: &PhpRef) -> Value {
    Value::Ref(slot.clone())
}

/// `[v0, v1, ...]`
pub fn list(vals: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(vals))
}

/// A list of strings.
pub fn strs(vals: &[&str]) -> Value {
    list(vals.iter().map(|v| s(v)).collect())
}

/// `["k" => v, ...]` (keys are normalised like PHP array keys).
pub fn assoc(pairs: Vec<(&str, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(ArrayKey::from(k), v);
    }
    Value::array(a)
}

/// An array from explicit keys.
pub fn keyed(pairs: Vec<(ArrayKey, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(k, v);
    }
    Value::array(a)
}

// ----- var_dump / print_r -------------------------------------------------

/// `var_dump($v)` output.
pub fn var_dump(v: &Value) -> String {
    String::from_utf8_lossy(&var_dump_bytes(v)).into_owned()
}

/// `var_dump($v)` output as raw bytes.
pub fn var_dump_bytes(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    dump_into(&mut out, v, 0);
    out
}

fn dump_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    let line = |out: &mut Vec<u8>, text: String| {
        out.extend_from_slice(pad.as_bytes());
        out.extend_from_slice(text.as_bytes());
        out.push(b'\n');
    };
    match v {
        Value::Undef | Value::Null => line(out, "NULL".into()),
        Value::Bool(b) => line(out, format!("bool({b})")),
        Value::Int(i) => line(out, format!("int({i})")),
        Value::Float(f) => line(out, format!("float({f})")),
        Value::String(s) => {
            out.extend_from_slice(pad.as_bytes());
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s);
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            line(out, format!("array({}) {{", a.len()));
            for (k, val) in a.iter() {
                match k {
                    ArrayKey::Int(i) => line(out, format!("  [{i}]=>")),
                    ArrayKey::Str(s) => {
                        out.extend_from_slice(pad.as_bytes());
                        out.extend_from_slice(b"  [\"");
                        out.extend_from_slice(&s);
                        out.extend_from_slice(b"\"]=>\n");
                    }
                }
                // A reference that is still shared with something else.
                let is_ref = matches!(val, Value::Ref(r) if r.refcount() > 1);
                if is_ref {
                    let mut inner = Vec::new();
                    dump_into(&mut inner, &val.unref(), indent + 2);
                    out.extend_from_slice(&inner[..indent + 2]);
                    out.push(b'&');
                    out.extend_from_slice(&inner[indent + 2..]);
                } else {
                    dump_into(out, &val.unref(), indent + 2);
                }
            }
            line(out, "}".into());
        }
        Value::Ref(r) => dump_into(out, &r.get(), indent),
        other => line(out, format!("{other:?}")),
    }
}

/// `print_r($v)` output.
pub fn print_r(v: &Value) -> String {
    String::from_utf8_lossy(&print_r_bytes(v)).into_owned()
}

/// `print_r($v)` output as raw bytes.
pub fn print_r_bytes(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    print_r_into(&mut out, v, 0);
    out
}

fn print_r_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    match v {
        Value::Array(a) => {
            let pad = " ".repeat(indent);
            out.extend_from_slice(b"Array\n");
            out.extend_from_slice(format!("{pad}(\n").as_bytes());
            for (k, val) in a.iter() {
                out.extend_from_slice(format!("{pad}    [{k}] => ").as_bytes());
                print_r_into(out, &val.unref(), indent + 8);
                out.push(b'\n');
            }
            out.extend_from_slice(format!("{pad})\n").as_bytes());
        }
        Value::Null | Value::Undef | Value::Bool(false) => {}
        Value::Bool(true) => out.push(b'1'),
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Float(f) => out.extend_from_slice(f.to_string().as_bytes()),
        Value::String(s) => out.extend_from_slice(s),
        Value::Ref(r) => print_r_into(out, &r.get(), indent),
        other => out.extend_from_slice(format!("{other:?}").as_bytes()),
    }
}

/// `var_export($v, true)` as raw bytes (no floats or objects).
pub fn var_export_bytes(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    export_into(&mut out, v, 1);
    out
}

fn export_str(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'\'');
    for &c in s {
        match c {
            b'\\' | b'\'' => {
                out.push(b'\\');
                out.push(c);
            }
            0 => out.extend_from_slice(b"' . \"\\0\" . '"),
            _ => out.push(c),
        }
    }
    out.push(b'\'');
}

fn export_into(out: &mut Vec<u8>, v: &Value, level: usize) {
    match v {
        Value::Undef | Value::Null => out.extend_from_slice(b"NULL"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::String(s) => export_str(out, s),
        Value::Array(a) => {
            if level > 1 {
                out.push(b'\n');
                out.extend_from_slice(" ".repeat(level - 1).as_bytes());
            }
            out.extend_from_slice(b"array (\n");
            for (k, val) in a.iter() {
                out.extend_from_slice(" ".repeat(level + 1).as_bytes());
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(s) => export_str(out, &s),
                }
                out.extend_from_slice(b" => ");
                export_into(out, &val.unref(), level + 2);
                out.extend_from_slice(b",\n");
            }
            if level > 1 {
                out.extend_from_slice(" ".repeat(level - 1).as_bytes());
            }
            out.push(b')');
        }
        Value::Ref(r) => export_into(out, &r.get(), level),
        other => panic!("var_export of {other:?} is not supported"),
    }
}

/// Compact one-line rendering for small assertions: `[0 => "a", "k" => null]`.
pub fn show(v: &Value) -> String {
    match v {
        Value::Undef | Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("{:?}", String::from_utf8_lossy(s)),
        Value::Array(a) => {
            let items: Vec<String> = a
                .iter()
                .map(|(k, v)| match k {
                    ArrayKey::Int(i) => format!("{i} => {}", show(&v.unref())),
                    ArrayKey::Str(s) => format!("{:?} => {}", s.to_string(), show(&v.unref())),
                })
                .collect();
            format!("[{}]", items.join(", "))
        }
        Value::Ref(r) => show(&r.get()),
        other => format!("{other:?}"),
    }
}

/// Like [`show`], but arrays whose keys are `0..n` are rendered as lists:
/// `[["a", 0], "k" => null]`.
pub fn sh(v: &Value) -> String {
    match v {
        Value::Array(a) => {
            let is_list = a
                .keys()
                .enumerate()
                .all(|(i, k)| k == ArrayKey::Int(i as i64));
            let items: Vec<String> = a
                .iter()
                .map(|(k, v)| match (is_list, k) {
                    (true, _) => sh(&v.unref()),
                    (false, ArrayKey::Int(i)) => format!("{i} => {}", sh(&v.unref())),
                    (false, ArrayKey::Str(s)) => {
                        format!("{:?} => {}", s.to_string(), sh(&v.unref()))
                    }
                })
                .collect();
            format!("[{}]", items.join(", "))
        }
        Value::Ref(r) => sh(&r.get()),
        other => show(other),
    }
}

// ----- native callbacks for preg_replace_callback ------------------------

fn matches_arg(args: &Args) -> PhpArray {
    match args.get(0) {
        Value::Array(a) => (**a).clone(),
        other => panic!("callback expects the match array, got {other:?}"),
    }
}

fn group(a: &PhpArray, i: i64) -> Value {
    a.get_int(i).map(Value::unref).unwrap_or(Value::Null)
}

/// `strtoupper($m[0])`
fn cb_upper(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let a = matches_arg(args);
    match group(&a, 0) {
        Value::String(s) => Ok(Value::bytes(&s.to_ascii_uppercase())),
        other => Ok(other),
    }
}

/// Echoes the match array (`var_dump`) and returns `$m[0]`.
fn cb_dump(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let a = matches_arg(args);
    vm.echo(&var_dump_bytes(&Value::array(a.clone())))?;
    Ok(group(&a, 0))
}

/// `$m[1]`
fn cb_first_group(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let a = matches_arg(args);
    Ok(group(&a, 1))
}

/// Returns an int (converted to a string by the caller).
fn cb_ret_int(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(42))
}

/// Returns an array ("Array" with a warning).
fn cb_ret_array(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(list(vec![]))
}

/// Throws an exception.
fn cb_throw(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "callback failed"))
}

/// `"[" . $m[0] . "]"`
fn cb_bracket(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let a = matches_arg(args);
    let m = match group(&a, 0) {
        Value::String(s) => s.to_vec(),
        _ => Vec::new(),
    };
    let mut out = b"[".to_vec();
    out.extend_from_slice(&m);
    out.push(b']');
    Ok(Value::from(out))
}

/// `(string) count($m)`
fn cb_count(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let a = matches_arg(args);
    Ok(Value::str(&a.len().to_string()))
}

pub static CB_UPPER: NativeFunction = NativeFunction {
    sig: "cb_upper(array $m): string",
    handler: cb_upper,
};
pub static CB_DUMP: NativeFunction = NativeFunction {
    sig: "cb_dump(array $m): string",
    handler: cb_dump,
};
pub static CB_FIRST_GROUP: NativeFunction = NativeFunction {
    sig: "cb_first_group(array $m): mixed",
    handler: cb_first_group,
};
pub static CB_RET_INT: NativeFunction = NativeFunction {
    sig: "cb_ret_int(array $m): int",
    handler: cb_ret_int,
};
pub static CB_RET_ARRAY: NativeFunction = NativeFunction {
    sig: "cb_ret_array(array $m): array",
    handler: cb_ret_array,
};
pub static CB_THROW: NativeFunction = NativeFunction {
    sig: "cb_throw(array $m): string",
    handler: cb_throw,
};
pub static CB_BRACKET: NativeFunction = NativeFunction {
    sig: "cb_bracket(array $m): string",
    handler: cb_bracket,
};
pub static CB_COUNT: NativeFunction = NativeFunction {
    sig: "cb_count(array $m): string",
    handler: cb_count,
};

// ----- closure-like callbacks ---------------------------------------------

type CallbackFn = Box<dyn FnMut(&mut Vm, &Args) -> PResult<Value>>;

thread_local! {
    static CALLBACKS: RefCell<[Option<CallbackFn>; 4]> =
        const { RefCell::new([None, None, None, None]) };
}

fn run_callback(slot: usize, vm: &mut Vm, args: &Args) -> PResult<Value> {
    let mut f = CALLBACKS
        .with(|c| c.borrow_mut()[slot].take())
        .unwrap_or_else(|| panic!("callback cb_rust{slot} is not installed (or re-entered)"));
    let r = f(vm, args);
    CALLBACKS.with(|c| c.borrow_mut()[slot] = Some(f));
    r
}

fn cb_rust0(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    run_callback(0, vm, args)
}
fn cb_rust1(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    run_callback(1, vm, args)
}
fn cb_rust2(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    run_callback(2, vm, args)
}
fn cb_rust3(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    run_callback(3, vm, args)
}

pub static CB_RUST0: NativeFunction = NativeFunction {
    sig: "cb_rust0(mixed $m): mixed",
    handler: cb_rust0,
};
pub static CB_RUST1: NativeFunction = NativeFunction {
    sig: "cb_rust1(mixed $m): mixed",
    handler: cb_rust1,
};
pub static CB_RUST2: NativeFunction = NativeFunction {
    sig: "cb_rust2(mixed $m): mixed",
    handler: cb_rust2,
};
pub static CB_RUST3: NativeFunction = NativeFunction {
    sig: "cb_rust3(mixed $m): mixed",
    handler: cb_rust3,
};

#[allow(unused_imports)]
pub use super::lit::*;

// ----- PREG_* constants -----------------------------------------------------

pub const PREG_PATTERN_ORDER: i64 = 1;
pub const PREG_SET_ORDER: i64 = 2;
pub const PREG_OFFSET_CAPTURE: i64 = 1 << 8;
pub const PREG_UNMATCHED_AS_NULL: i64 = 1 << 9;
pub const PREG_SPLIT_NO_EMPTY: i64 = 1;
pub const PREG_SPLIT_DELIM_CAPTURE: i64 = 2;
pub const PREG_SPLIT_OFFSET_CAPTURE: i64 = 4;
pub const PREG_GREP_INVERT: i64 = 1;
