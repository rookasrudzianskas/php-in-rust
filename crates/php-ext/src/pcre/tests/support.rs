//! Helpers for the ext/pcre unit tests: a PHP "session" around [`TestVm`],
//! value builders, and `var_dump()` / `print_r()` renderings that match PHP's
//! output so `--EXPECT--` sections of the `.phpt` files can be reused verbatim.

#![allow(dead_code)]

use std::fmt::Write as _;

use php_runtime::{
    Args, ArrayKey, Flow, NativeFunction, PResult, PhpArray, PhpRef, Registry, Value, Vm,
};

use crate::testing::TestVm;

/// A PHP session: a VM with every module registered and the helper callbacks
/// below, plus shorthands for calling `preg_*` functions.
pub struct P {
    pub t: TestVm,
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
        ]);
        P { t }
    }

    pub fn vm(&mut self) -> &mut Vm {
        &mut self.t.vm
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
    let mut out = String::new();
    dump_into(&mut out, v, 0);
    out
}

fn dump_into(out: &mut String, v: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match v {
        Value::Undef | Value::Null => writeln!(out, "{pad}NULL").unwrap(),
        Value::Bool(b) => writeln!(out, "{pad}bool({b})").unwrap(),
        Value::Int(i) => writeln!(out, "{pad}int({i})").unwrap(),
        Value::Float(f) => writeln!(out, "{pad}float({f})").unwrap(),
        Value::String(s) => writeln!(
            out,
            "{pad}string({}) \"{}\"",
            s.len(),
            String::from_utf8_lossy(s)
        )
        .unwrap(),
        Value::Array(a) => {
            writeln!(out, "{pad}array({}) {{", a.len()).unwrap();
            for (k, val) in a.iter() {
                match k {
                    ArrayKey::Int(i) => writeln!(out, "{pad}  [{i}]=>").unwrap(),
                    ArrayKey::Str(s) => writeln!(out, "{pad}  [\"{s}\"]=>").unwrap(),
                }
                dump_into(out, &val.unref(), indent + 2);
            }
            writeln!(out, "{pad}}}").unwrap();
        }
        Value::Ref(r) => dump_into(out, &r.get(), indent),
        other => writeln!(out, "{pad}{other:?}").unwrap(),
    }
}

/// `print_r($v)` output.
pub fn print_r(v: &Value) -> String {
    let mut out = String::new();
    print_r_into(&mut out, v, 0);
    out
}

fn print_r_into(out: &mut String, v: &Value, indent: usize) {
    match v {
        Value::Array(a) => {
            let pad = " ".repeat(indent);
            writeln!(out, "Array").unwrap();
            writeln!(out, "{pad}(").unwrap();
            for (k, val) in a.iter() {
                write!(out, "{pad}    [{k}] => ").unwrap();
                print_r_into(out, &val.unref(), indent + 8);
                writeln!(out).unwrap();
            }
            writeln!(out, "{pad})").unwrap();
        }
        Value::Null | Value::Undef | Value::Bool(false) => {}
        Value::Bool(true) => out.push('1'),
        Value::Int(i) => write!(out, "{i}").unwrap(),
        Value::Float(f) => write!(out, "{f}").unwrap(),
        Value::String(s) => out.push_str(&String::from_utf8_lossy(s)),
        Value::Ref(r) => print_r_into(out, &r.get(), indent),
        other => write!(out, "{other:?}").unwrap(),
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
    vm.echo(var_dump(&Value::array(a.clone())).as_bytes())?;
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
