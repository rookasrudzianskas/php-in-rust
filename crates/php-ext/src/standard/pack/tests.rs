//! Ports of php-src's pack() tests (`ext/standard/tests/strings`).
//! Scripts are replayed through [`Script`] and compared byte for byte with
//! the `--EXPECT--`/`--EXPECTF--` sections of the vendored `.phpt` files.

use php_runtime::value::float_fmt;
use php_runtime::value::ops::is_identical;
use php_runtime::{ArrayKey, PResult, PhpStr, Value};

use crate::testing::{TestVm, thrown};

macro_rules! phpt {
    ($name:literal) => {
        include_str!(concat!(
            "../../../../../tests/phpt/ext/standard/tests/strings/",
            $name
        ))
    };
}

/// Replays a PHP script: function calls, echoes and dumps go to one buffer
/// in execution order (warnings are flushed after every call).
struct Script {
    t: TestVm,
    out: Vec<u8>,
}

impl Script {
    fn new(file: &str) -> Script {
        let mut t = TestVm::new();
        t.vm.current_file = PhpStr::from(file);
        Script { t, out: Vec::new() }
    }

    fn try_call(&mut self, f: &str, args: Vec<Value>) -> PResult<Value> {
        let r = self.t.call(f, args);
        self.flush();
        r
    }

    fn call(&mut self, f: &str, args: Vec<Value>) -> Value {
        match self.try_call(f, args) {
            Ok(v) => v,
            Err(e) => panic!("{f}() failed: {e:?}"),
        }
    }

    fn flush(&mut self) {
        let o = self.t.out.take();
        self.out.extend_from_slice(&o);
    }

    fn pack(&mut self, fmt: &str, values: Vec<Value>) -> Value {
        let mut args = vec![Value::str(fmt)];
        args.extend(values);
        self.call("pack", args)
    }

    fn echo(&mut self, s: impl AsRef<[u8]>) {
        self.out.extend_from_slice(s.as_ref());
    }

    fn var_dump(&mut self, v: &Value) {
        var_dump_into(&mut self.out, v, 0);
    }

    /// `catch (Throwable $e) { echo $e::class, ': ', $e->getMessage(), "\n"; }`
    fn echo_thrown(&mut self, r: PResult<Value>) {
        let (class, message) = thrown(r);
        self.echo(format!("{class}: {message}\n"));
    }

    fn check(self, phpt: &str) {
        let (expected, is_f) = expect_section(phpt);
        let actual = trim(&self.out);
        let expected = trim(expected.as_bytes());
        let ok = if is_f {
            expectf_match(expected, actual)
        } else {
            expected == actual
        };
        assert!(
            ok,
            "output mismatch\n--- expected ---\n{}\n--- actual ---\n{}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(actual)
        );
    }
}

/// The last `--EXPECT--`/`--EXPECTF--` section and whether it is EXPECTF.
fn expect_section(phpt: &str) -> (&str, bool) {
    let mut found = None;
    let mut offset = 0;
    for line in phpt.split_inclusive('\n') {
        let header = line.trim_end();
        if header == "--EXPECT--" || header == "--EXPECTF--" {
            found = Some((offset + line.len(), header == "--EXPECTF--"));
        } else if found.is_some()
            && header.starts_with("--")
            && header.ends_with("--")
            && header.len() > 4
        {
            let (start, is_f) = found.expect("checked");
            return (&phpt[start..offset], is_f);
        }
        offset += line.len();
    }
    let (start, is_f) = found.expect("no EXPECT section");
    (&phpt[start..], is_f)
}

fn trim(b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0);
    let start = b.iter().position(|c| !ws(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !ws(c)).map_or(start, |e| e + 1);
    &b[start..end]
}

/// run-tests.php's EXPECTF placeholders used by these tests.
fn expectf_match(pat: &[u8], s: &[u8]) -> bool {
    match pat {
        [] => s.is_empty(),
        [b'%', b'%', rest @ ..] => s.first() == Some(&b'%') && expectf_match(rest, &s[1..]),
        [b'%', b'c', rest @ ..] => !s.is_empty() && expectf_match(rest, &s[1..]),
        [b'%', kind @ (b's' | b'S' | b'a' | b'A' | b'd'), rest @ ..] => {
            let min = if matches!(kind, b's' | b'a' | b'd') {
                1
            } else {
                0
            };
            let allowed = |c: u8| match kind {
                b'd' => c.is_ascii_digit(),
                b's' | b'S' => c != b'\n',
                _ => true,
            };
            let max = s.iter().position(|&c| !allowed(c)).unwrap_or(s.len());
            (min..=max).rev().any(|n| expectf_match(rest, &s[n..]))
        }
        [c, rest @ ..] => s.first() == Some(c) && expectf_match(rest, &s[1..]),
    }
}

fn var_dump_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    match v {
        Value::Null | Value::Undef => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
        Value::Int(i) => out.extend_from_slice(format!("int({i})\n").as_bytes()),
        Value::Float(d) => {
            out.extend_from_slice(format!("float({})\n", float_fmt::repr(*d, -1)).as_bytes())
        }
        Value::String(s) => {
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s);
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            let pad = " ".repeat(indent + 2);
            for (k, v) in a.iter() {
                out.extend_from_slice(pad.as_bytes());
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
                    ArrayKey::Str(s) => {
                        out.extend_from_slice(b"[\"");
                        out.extend_from_slice(&s);
                        out.extend_from_slice(b"\"]=>\n");
                    }
                }
                out.extend_from_slice(pad.as_bytes());
                var_dump_into(out, v, indent + 2);
            }
            out.extend_from_slice(" ".repeat(indent).as_bytes());
            out.extend_from_slice(b"}\n");
        }
        other => panic!("var_dump of {other:?} not supported in tests"),
    }
}

fn bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.to_vec(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn float(d: f64) -> Value {
    Value::Float(d)
}

// ---------------------------------------------------------------------------
// php-src .phpt ports
// ---------------------------------------------------------------------------

#[test]
fn bug70487_phpt() {
    let mut s = Script::new("/t.php");
    let p = s.pack("x", vec![]);
    s.var_dump(&Value::Bool(is_identical(&p, &Value::bytes(b"\0"))));
    s.check(phpt!("bug70487.phpt"));
}

#[test]
fn bug78833_phpt() {
    let mut s = Script::new("/t.php");
    let r = s.try_call(
        "pack",
        vec![Value::str("E2E2147483647H*"), int(0), int(0), int(0)],
    );
    s.echo_thrown(r);
    s.check(phpt!("bug78833.phpt"));
}

// ---------------------------------------------------------------------------
// Diagnostics and edge cases straight from pack.c
// ---------------------------------------------------------------------------

fn pack_err(fmt: &str, args: Vec<Value>) -> (String, String) {
    let mut t = TestVm::new();
    let mut all = vec![Value::str(fmt)];
    all.extend(args);
    thrown(t.call("pack", all))
}

fn pack_out(fmt: &str, args: Vec<Value>) -> (Vec<u8>, String) {
    let mut s = Script::new("/t.php");
    let p = s.pack(fmt, args);
    (bytes(&p), String::from_utf8_lossy(&s.out).into_owned())
}

fn warning(fname: &str, msg: &str) -> String {
    format!("\nWarning: {fname}(): {msg} in /t.php on line 3\n")
}

#[test]
fn pack_argument_errors() {
    assert_eq!(
        pack_err("a", vec![]),
        ("ValueError".into(), "Type a: not enough arguments".into())
    );
    assert_eq!(
        pack_err("N2", vec![int(1)]),
        ("ValueError".into(), "Type N: too few arguments".into())
    );
    assert_eq!(
        pack_err("y", vec![int(1)]),
        ("ValueError".into(), "Type y: unknown format code".into())
    );
    assert_eq!(
        pack_err("x2147483647x", vec![]),
        (
            "ValueError".into(),
            "Type x: integer overflow in format string".into()
        )
    );
    assert_eq!(
        pack_err("N1073741824", (0..2).map(int).collect()),
        ("ValueError".into(), "Type N: too few arguments".into())
    );
    // Unknown codes are reported with their raw byte.
    let (_, m) = pack_err("\u{7f}", vec![]);
    assert_eq!(m, "Type \u{7f}: unknown format code");
}

#[test]
fn pack_warnings() {
    let (p, out) = pack_out("N", vec![int(1), int(2), int(3)]);
    assert_eq!(p, b"\0\0\0\x01");
    assert_eq!(out, warning("pack", "2 arguments unused"));

    let (p, out) = pack_out("x*", vec![]);
    assert_eq!(p, b"\0");
    assert_eq!(out, warning("pack", "Type x: '*' ignored"));

    let (p, out) = pack_out("CX2", vec![int(65)]);
    assert_eq!(p, b"");
    assert_eq!(out, warning("pack", "Type X: outside of string"));

    let (p, out) = pack_out("H*", vec![Value::str("1g2")]);
    assert_eq!(p, b"\x10\x20");
    assert_eq!(out, warning("pack", "Type H: illegal hex digit g"));

    let (p, out) = pack_out("h5", vec![Value::str("12")]);
    assert_eq!(p, b"\x21");
    assert_eq!(
        out,
        warning("pack", "Type h: not enough characters in string")
    );
}

#[test]
fn pack_positioning_codes() {
    assert_eq!(
        pack_out("a3@1a1", vec![Value::str("abc"), Value::str("z")]).0,
        b"az"
    );
    assert_eq!(pack_out("C@4C", vec![int(1), int(2)]).0, b"\x01\0\0\0\x02");
    assert_eq!(pack_out("x3", vec![]).0, b"\0\0\0");
    assert_eq!(pack_out("NX2", vec![int(0x01020304)]).0, b"\x01\x02");
    // '@' zero-fills over earlier output.
    assert_eq!(pack_out("a4X4@3", vec![Value::str("abcd")]).0, b"\0\0\0");
    assert_eq!(pack_out("", vec![]).0, b"");
    assert_eq!(pack_out("a0", vec![Value::str("abc")]).0, b"");
}

#[test]
fn pack_star_and_strings() {
    assert_eq!(
        pack_out("C*", vec![int(1), int(2), int(3)]).0,
        b"\x01\x02\x03"
    );
    assert_eq!(pack_out("n*", vec![]).0, b"");
    assert_eq!(pack_out("A*", vec![int(123)]).0, b"123");
    assert_eq!(pack_out("a5", vec![Value::str("ab")]).0, b"ab\0\0\0");
    assert_eq!(pack_out("Z*", vec![Value::str("")]).0, b"\0");
    assert_eq!(pack_out("H*", vec![Value::str("abc")]).0, b"\xab\xc0");
    assert_eq!(pack_out("h*", vec![Value::str("abc")]).0, b"\xba\x0c");
    // Numeric strings and floats go through zval_get_long()/zval_get_double().
    assert_eq!(pack_out("N", vec![Value::str("123abc")]).0, b"\0\0\0\x7b");
    assert_eq!(pack_out("C", vec![float(65.9)]).0, b"A");
    assert_eq!(
        pack_out("E", vec![Value::str("1.5")]).0,
        1.5f64.to_be_bytes()
    );
    assert_eq!(
        pack_out("g", vec![Value::Bool(true)]).0,
        1.0f32.to_le_bytes()
    );
    let (p, out) = pack_out("a*", vec![Value::empty_array()]);
    assert_eq!(p, b"Array");
    assert_eq!(
        out,
        "\nWarning: Array to string conversion in /t.php on line 3\n"
    );
}

#[test]
fn pack_64_bit_values() {
    assert_eq!(pack_out("J", vec![int(1)]).0, b"\0\0\0\0\0\0\0\x01");
    assert_eq!(pack_out("P", vec![int(1)]).0, b"\x01\0\0\0\0\0\0\0");
    assert_eq!(pack_out("q", vec![int(-2)]).0, (-2i64).to_ne_bytes());
    assert_eq!(pack_out("Q>", vec![int(i64::MIN)]).0, b"\x80\0\0\0\0\0\0\0");
    let (p, out) = pack_out("J", vec![float(1e19)]);
    assert_eq!(p, (1e19f64 as u64).to_be_bytes());
    assert_eq!(
        out,
        "\nWarning: The float 1.0E+19 is not representable as an int, cast occurred in /t.php on line 3\n"
    );
}

#[test]
fn pack_int_helper_orders_bytes() {
    let mut buf = [0u8; 8];
    super::pack_int(0x0102030405060708, 3, super::Endian::Big, &mut buf);
    assert_eq!(&buf[..3], b"\x06\x07\x08");
    super::pack_int(0x0102030405060708, 3, super::Endian::Little, &mut buf);
    assert_eq!(&buf[..3], b"\x08\x07\x06");
    super::pack_int(-1, 8, super::MACHINE_ENDIAN, &mut buf);
    assert_eq!(buf, [0xff; 8]);
}
