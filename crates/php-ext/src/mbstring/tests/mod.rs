//! Unit tests of the mbstring functions, with expectations taken from
//! php-src's `ext/mbstring/tests/*.phpt`.

mod case_width;
mod check_encoding;
pub mod encoding_tests;
mod functions;
mod info;
mod mime;
mod numericentity;
mod strcut;
mod strings;

use php_runtime::{PhpArray, Value};

use crate::testing::TestVm;

/// `var_dump($v)` output (the interpreter's `var_dump` is not available).
pub fn dump(_t: &mut TestVm, v: Value) -> String {
    let mut out = Vec::new();
    var_dump_into(&mut out, &v, 0);
    String::from_utf8_lossy(&out).into_owned()
}

fn var_dump_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    out.extend_from_slice(pad.as_bytes());
    match v {
        Value::Null | Value::Undef => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
        Value::Int(i) => out.extend_from_slice(format!("int({i})\n").as_bytes()),
        Value::Float(f) => out.extend_from_slice(format!("float({f})\n").as_bytes()),
        Value::String(s) => {
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s);
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, v) in a.iter() {
                out.extend_from_slice(pad.as_bytes());
                match k {
                    php_runtime::ArrayKey::Int(i) => {
                        out.extend_from_slice(format!("  [{i}]=>\n").as_bytes())
                    }
                    php_runtime::ArrayKey::Str(s) => {
                        out.extend_from_slice(b"  [\"");
                        out.extend_from_slice(&s);
                        out.extend_from_slice(b"\"]=>\n");
                    }
                }
                var_dump_into(out, &v.unref(), indent + 2);
            }
            out.extend_from_slice(pad.as_bytes());
            out.extend_from_slice(b"}\n");
        }
        Value::Ref(r) => var_dump_into(out, &r.get(), indent),
        other => out.extend_from_slice(format!("{other:?}\n").as_bytes()),
    }
}

/// Calls `name` and returns its `var_dump` (plus any diagnostics printed).
pub fn vd(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let v = call(t, name, args);
    let before = t.output();
    format!("{before}{}", dump(t, v))
}

/// A section (`EXPECT`, `FILE`, ...) of a php-src mbstring `.phpt` test.
pub fn phpt_section(test: &str, section: &str) -> String {
    let path = format!(
        "{}/../../tests/phpt/ext/mbstring/tests/{test}.phpt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("phpt file")).into_owned();
    let marker = format!("--{section}--\n");
    let start = text.find(&marker).expect("section") + marker.len();
    let rest = &text[start..];
    let end = rest
        .find("\n--")
        .filter(|&i| rest[i + 3..].starts_with(|c: char| c.is_ascii_uppercase()))
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// run-tests.php's comparison: CRLF as LF, surrounding whitespace trimmed.
pub fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim().to_string()
}

/// `var_dump()` of a call's result (without diagnostics).
pub fn dump_str(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let v = call(t, name, args);
    dump(t, v)
}

/// The exception a call throws, as `Class: message`.
pub fn err(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let (class, msg) = crate::testing::thrown(t.call(name, args));
    format!("{class}: {msg}")
}

/// Hex of each string in an array result.
pub fn hexes(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.values().map(hex).collect(),
        other => panic!("expected array, got {other:?}"),
    }
}

/// An int result.
pub fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        other => panic!("expected int, got {other:?}"),
    }
}

/// A PHP string from bytes.
pub fn b(s: &[u8]) -> Value {
    Value::bytes(s)
}

/// A PHP string from text.
pub fn s(s: &str) -> Value {
    Value::from(s)
}

/// Calls `name` and unwraps the result.
pub fn call(t: &mut TestVm, name: &str, args: Vec<Value>) -> Value {
    match t.call(name, args) {
        Ok(v) => v,
        Err(e) => panic!("{name}() failed: {e:?} / output: {}", t.output()),
    }
}

/// The bytes of a string result.
pub fn bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.to_vec(),
        other => panic!("expected string, got {other:?}"),
    }
}

/// A string result as text.
pub fn text(v: &Value) -> String {
    String::from_utf8_lossy(&bytes(v)).into_owned()
}

/// Lowercase hex of a string result (`bin2hex`).
pub fn hex(v: &Value) -> String {
    bytes(v).iter().map(|c| format!("{c:02x}")).collect()
}

/// Decodes hex text.
pub fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

/// A list array of strings.
pub fn list(items: &[&str]) -> Value {
    Value::array(items.iter().map(|i| Value::from(*i)).collect::<PhpArray>())
}

/// The values of an array result as strings.
pub fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.values().map(text).collect(),
        other => panic!("expected array, got {other:?}"),
    }
}
