//! Ports of php-src's pack()/unpack() tests (`ext/standard/tests/strings`).
//! Scripts are replayed through [`Script`] and compared byte for byte with
//! the `--EXPECT--`/`--EXPECTF--` sections of the vendored `.phpt` files.

use php_runtime::value::float_fmt;
use php_runtime::value::ops::is_identical;
use php_runtime::{ArrayKey, PResult, PhpStr, Value};

use crate::testing::{TestVm, thrown};

/// `assert!($a === $b)` with both values in the failure message.
macro_rules! assert_same {
    ($a:expr, $b:expr) => {
        assert_same!($a, $b, "")
    };
    ($a:expr, $b:expr, $ctx:expr) => {{
        let (a, b): (&Value, &Value) = (&$a, &$b);
        assert!(is_identical(a, b), "{a:?} !== {b:?} {}", $ctx);
    }};
}

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

    fn unpack(&mut self, fmt: &str, data: &Value) -> Value {
        self.call("unpack", vec![Value::str(fmt), data.clone()])
    }

    fn echo(&mut self, s: impl AsRef<[u8]>) {
        self.out.extend_from_slice(s.as_ref());
    }

    fn var_dump(&mut self, v: &Value) {
        var_dump_into(&mut self.out, v, 0);
    }

    fn print_r(&mut self, v: &Value) {
        print_r_into(&mut self.out, v);
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

/// print_r() of a scalar or a flat array.
fn print_r_into(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Array(a) => {
            out.extend_from_slice(b"Array\n(\n");
            for (k, v) in a.iter() {
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("    [{i}] => ").as_bytes()),
                    ArrayKey::Str(s) => {
                        out.extend_from_slice(b"    [");
                        out.extend_from_slice(&s);
                        out.extend_from_slice(b"] => ");
                    }
                }
                print_r_into(out, v);
                out.push(b'\n');
            }
            out.extend_from_slice(b")\n");
        }
        Value::String(s) => out.extend_from_slice(s),
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Float(d) => out.extend_from_slice(float_fmt::double_to_string(*d, 14).as_bytes()),
        Value::Bool(true) => out.push(b'1'),
        Value::Bool(false) | Value::Null => {}
        other => panic!("print_r of {other:?} not supported in tests"),
    }
}

fn bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.to_vec(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn bin2hex(v: &Value) -> Value {
    let hex: String = bytes(v).iter().map(|b| format!("{b:02x}")).collect();
    Value::str(&hex)
}

fn hex2bin(s: &str) -> Value {
    let raw: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect();
    Value::bytes(&raw)
}

fn elem(v: &Value, key: i64) -> Value {
    match v {
        Value::Array(a) => a.get_int(key).cloned().expect("missing element"),
        other => panic!("expected an array, got {other:?}"),
    }
}

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn float(d: f64) -> Value {
    Value::Float(d)
}

/// `0x8000000000000002` and friends: hex literals above PHP_INT_MAX are floats.
fn big_hex(u: u64) -> Value {
    Value::Float(u as f64)
}

// ---------------------------------------------------------------------------
// php-src .phpt ports
// ---------------------------------------------------------------------------

#[test]
fn pack64_phpt() {
    let mut s = Script::new("/t.php");
    for code in ["Q", "J", "P", "q"] {
        for v in [
            int(0xfffffffffffe),
            int(0),
            big_hex(0x8000000000000002),
            int(-1),
            big_hex(0x8000000000000000),
        ] {
            let p = s.pack(code, vec![v]);
            let u = s.unpack(code, &p);
            s.print_r(&u);
        }
    }
    for (code, v) in [
        ("i", 2147483647),
        ("i", -2147483647),
        ("i", -2147483648),
        ("I", 4294967295),
    ] {
        let p = s.pack(code, vec![int(v)]);
        let u = s.unpack(code, &p);
        s.print_r(&u);
    }
    s.check(phpt!("pack64.phpt"));
}

#[test]
fn pack_a_upper_phpt() {
    let mut s = Script::new("/t.php");
    let results = [
        s.pack("A5", vec![Value::str("foo ")]),
        s.pack("A4", vec![Value::str("fooo")]),
        s.pack("A4", vec![Value::str("foo")]),
        s.unpack("A*", &Value::bytes(b"foo\0\rbar\0 \t\r\n")),
        s.unpack("A4", &Value::bytes(b"foo\0\rbar\0 \t\r\n")),
    ];
    for r in &results {
        s.var_dump(r);
    }
    s.check(phpt!("pack_A.phpt"));
}

#[test]
fn pack_z_phpt() {
    let mut s = Script::new("/t.php");
    let results = [
        s.pack("Z0", vec![Value::str("f")]),
        s.pack("Z5", vec![Value::bytes(b"foo\0")]),
        s.pack("Z4", vec![Value::str("fooo")]),
        s.pack("Z4", vec![Value::str("foo")]),
        s.pack("Z*", vec![Value::str("foo")]),
        s.unpack("Z*", &Value::bytes(b"foo\0\rbar\0 \t\r\n")),
        s.unpack("Z9", &Value::bytes(b"foo\0\rbar\0 \t\r\n")),
        s.unpack("Z2", &Value::bytes(b"\0")),
        s.unpack("Z2", &Value::bytes(b"\0\0")),
        s.unpack("Z2", &Value::bytes(b"A\0")),
        s.unpack("Z2", &Value::bytes(b"AB\0")),
        s.unpack("Z2", &Value::bytes(b"ABC")),
    ];
    for r in &results {
        s.var_dump(r);
    }
    s.check(phpt!("pack_Z.phpt"));
}

#[test]
fn pack_arrays_phpt() {
    let mut s = Script::new("/t.php");
    let data = s.pack(
        "VVV",
        vec![int(0x00010203), int(0x04050607), int(0x08090a0b)],
    );
    for fmt in ["Vaa/Vbb/Vcc", "V2aa/Vcc", "V3aa", "V*aa", "V*"] {
        let u = s.unpack(fmt, &data);
        s.print_r(&u);
    }
    s.check(phpt!("pack_arrays.phpt"));
}

#[test]
#[allow(clippy::approx_constant)] // 3.14 is the value used by the .phpt
fn pack_endian_modifiers_phpt() {
    let mut s = Script::new("/t.php");
    let hexes = |s: &mut Script, pairs: &[(&str, i64)]| {
        for &(fmt, v) in pairs {
            let p = s.pack(fmt, vec![int(v)]);
            s.var_dump(&bin2hex(&p));
        }
    };
    let same = |s: &mut Script, pairs: &[(&str, &str, Value)]| {
        for (a, b, v) in pairs {
            let x = s.pack(a, vec![v.clone()]);
            let y = s.pack(b, vec![v.clone()]);
            s.var_dump(&Value::Bool(is_identical(&x, &y)));
        }
    };
    hexes(
        &mut s,
        &[
            ("s<", 0x0102),
            ("S<", 0x0102),
            ("s>", 0x0102),
            ("S>", 0x0102),
        ],
    );
    same(
        &mut s,
        &[
            ("s<", "v", int(0x0102)),
            ("S<", "v", int(0x0102)),
            ("s>", "n", int(0x0102)),
            ("S>", "n", int(0x0102)),
        ],
    );
    hexes(
        &mut s,
        &[
            ("l<", 0x01020304),
            ("L<", 0x01020304),
            ("l>", 0x01020304),
            ("L>", 0x01020304),
        ],
    );
    same(
        &mut s,
        &[
            ("l<", "V", int(0x01020304)),
            ("L<", "V", int(0x01020304)),
            ("l>", "N", int(0x01020304)),
            ("L>", "N", int(0x01020304)),
        ],
    );
    let unpacks: [(&str, &[u8]); 12] = [
        ("s<", b"\x02\x01"),
        ("S<", b"\x02\x01"),
        ("s>", b"\x01\x02"),
        ("S>", b"\x01\x02"),
        ("s<", b"\xfe\xff"),
        ("s>", b"\xff\xfe"),
        ("l<", b"\x04\x03\x02\x01"),
        ("L<", b"\x04\x03\x02\x01"),
        ("l>", b"\x01\x02\x03\x04"),
        ("L>", b"\x01\x02\x03\x04"),
        ("l<", b"\xfe\xff\xff\xff"),
        ("l>", b"\xff\xff\xff\xfe"),
    ];
    for (fmt, data) in unpacks {
        let u = s.unpack(fmt, &Value::bytes(data));
        s.var_dump(&u);
    }
    for fmt in ["s<2", "s>2"] {
        let p = s.pack(fmt, vec![int(0x0102), int(0x0304)]);
        s.var_dump(&bin2hex(&p));
    }
    let unpacks: [(&str, &[u8]); 3] = [
        ("s<2", b"\x02\x01\x04\x03"),
        ("s>2", b"\x01\x02\x03\x04"),
        ("s<value/L>count", b"\x02\x01\x00\x00\x00\x05"),
    ];
    for (fmt, data) in unpacks {
        let u = s.unpack(fmt, &Value::bytes(data));
        s.var_dump(&u);
    }
    same(
        &mut s,
        &[
            ("f<", "g", float(3.14)),
            ("f>", "G", float(3.14)),
            ("d<", "e", float(3.14)),
            ("d>", "E", float(3.14)),
        ],
    );
    for (le, be, modle, modbe) in [("g", "G", "f<", "f>"), ("e", "E", "d<", "d>")] {
        let packed_le = s.pack(le, vec![float(3.14)]);
        let packed_be = s.pack(be, vec![float(3.14)]);
        let a = s.unpack(modle, &packed_le);
        let b = s.unpack(modbe, &packed_be);
        let c = s.unpack(le, &packed_le);
        let d = s.unpack(be, &packed_be);
        s.var_dump(&Value::Bool(is_identical(&elem(&a, 1), &elem(&c, 1))));
        s.var_dump(&Value::Bool(is_identical(&elem(&b, 1), &elem(&d, 1))));
    }
    for code in ["f", "d"] {
        let p = s.pack(code, vec![float(1.5)]);
        let u = s.unpack(code, &p);
        s.var_dump(&Value::Bool(is_identical(&elem(&u, 1), &float(1.5))));
    }
    s.check(phpt!("pack_endian_modifiers.phpt"));
}

#[test]
fn pack_endian_modifiers_64_phpt() {
    let mut s = Script::new("/t.php");
    for fmt in ["q<", "Q<", "q>", "Q>"] {
        let p = s.pack(fmt, vec![int(0x0102030405060708)]);
        s.var_dump(&bin2hex(&p));
    }
    for (a, b) in [("q<", "P"), ("Q<", "P"), ("q>", "J"), ("Q>", "J")] {
        let x = s.pack(a, vec![int(0x0102030405060708)]);
        let y = s.pack(b, vec![int(0x0102030405060708)]);
        s.var_dump(&Value::Bool(is_identical(&x, &y)));
    }
    let unpacks: [(&str, &[u8]); 6] = [
        ("q<", b"\x08\x07\x06\x05\x04\x03\x02\x01"),
        ("Q<", b"\x08\x07\x06\x05\x04\x03\x02\x01"),
        ("q>", b"\x01\x02\x03\x04\x05\x06\x07\x08"),
        ("Q>", b"\x01\x02\x03\x04\x05\x06\x07\x08"),
        ("q<", b"\xfe\xff\xff\xff\xff\xff\xff\xff"),
        ("q>", b"\xff\xff\xff\xff\xff\xff\xff\xfe"),
    ];
    for (fmt, data) in unpacks {
        let u = s.unpack(fmt, &Value::bytes(data));
        s.var_dump(&u);
    }
    s.check(phpt!("pack_endian_modifiers_64.phpt"));
}

#[test]
fn pack_endian_modifiers_error_phpt() {
    let mut s = Script::new("/t.php");
    let inherent = ["n<", "v>", "N<", "V>", "J<", "P>"];
    let inherent_float = ["g<", "G>", "e<", "E>"];
    let unsupported = [
        "c<", "C>", "a<", "A>", "h<", "H>", "i<", "I>", "x<", "X>", "@<",
    ];
    let pack_cases = [
        (&inherent[..], int(1)),
        (&inherent_float[..], float(1.0)),
        (&unsupported[..], int(1)),
    ];
    for (fmts, v) in &pack_cases {
        for fmt in fmts.iter() {
            let r = s.try_call("pack", vec![Value::str(fmt), v.clone()]);
            let (class, message) = thrown(r);
            assert_eq!(class, "ValueError");
            s.echo(format!("pack('{fmt}'): {message}\n"));
        }
    }
    let data = Value::bytes(&[0; 8]);
    for fmts in [&inherent[..], &inherent_float[..], &unsupported[..]] {
        for fmt in fmts {
            let r = s.try_call("unpack", vec![Value::str(fmt), data.clone()]);
            let (class, message) = thrown(r);
            assert_eq!(class, "ValueError");
            s.echo(format!("unpack('{fmt}'): {message}\n"));
        }
    }
    s.check(phpt!("pack_endian_modifiers_error.phpt"));
}

#[test]
fn pack_float_phpt() {
    let mut s = Script::new("/t.php");
    let inputs = [
        Value::str(""),
        Value::str("a"),
        Value::str(" "),
        Value::Null,
        int(0),
        int(1),
        float(1.0),
        int(10000000000000000),
        float(0.591234709823149),
        float(12345678901234567890.1234567898765432123456789),
        int(-1),
        float(-1.0),
        int(-10000000000000000),
        float(-0.591234709823149),
        float(-12345678901234567890.1234567898765432123456789),
    ];
    let mut results = Vec::new();
    for code in ["e", "E", "g", "G"] {
        results.push(Value::str(&format!("pack {code}")));
        for v in &inputs {
            let p = s.pack(code, vec![v.clone()]);
            results.push(bin2hex(&p));
        }
    }
    let unpacks: [(&str, &[&str]); 4] = [
        (
            "e",
            &[
                "0000000000000000",
                "000000000000f03f",
                "0080e03779c34143",
                "4a6ade0d65ebe23f",
                "000000000000f0bf",
                "0080e03779c341c3",
                "4a6ade0d65ebe2bf",
                "e1639d31956ae5c3",
            ],
        ),
        (
            "E",
            &[
                "3ff0000000000000",
                "4341c37937e08000",
                "3fe2eb650dde6a4a",
                "43e56a95319d63e1",
                "bff0000000000000",
                "c341c37937e08000",
                "bfe2eb650dde6a4a",
                "c3e56a95319d63e1",
            ],
        ),
        (
            "g",
            &[
                "0000803f", "ca1b0e5a", "285b173f", "aa542b5f", "000080bf", "ca1b0eda", "285b17bf",
                "aa542bdf",
            ],
        ),
        (
            "G",
            &[
                "3f800000", "5a0e1bca", "3f175b28", "5f2b54aa", "bf800000", "da0e1bca", "bf175b28",
                "df2b54aa",
            ],
        ),
    ];
    for (code, hexes) in unpacks {
        results.push(Value::str(&format!("unpack {code}")));
        for h in hexes {
            results.push(s.unpack(code, &hex2bin(h)));
        }
    }
    for r in &results {
        s.var_dump(r);
    }
    s.check(phpt!("pack_float.phpt"));
}

#[test]
fn unpack_endian_modifier_names_phpt() {
    let mut s = Script::new("/t.php");
    let cases: [(&str, &[u8]); 7] = [
        ("s<value", b"\x02\x01"),
        ("s>value", b"\x01\x02"),
        ("s<2name", b"\x02\x01\x04\x03"),
        ("s<*name", b"\x02\x01\x04\x03"),
        ("v1<value", b"\x02\x01"),
        ("n2>name", b"\x01\x02\x03\x04"),
        ("C1>name", b"\x01"),
    ];
    for (fmt, data) in cases {
        let u = s.unpack(fmt, &Value::bytes(data));
        s.var_dump(&u);
    }
    s.check(phpt!("unpack_endian_modifier_names.phpt"));
}

#[test]
fn unpack_endian_modifier_names_error_phpt() {
    let mut s = Script::new("/t.php");
    for fmt in ["C>name", "@>name", "n<name", "V>name"] {
        let r = s.try_call(
            "unpack",
            vec![
                Value::str(fmt),
                Value::bytes(b"\x01\x02\x03\x04\x05\x06\x07\x08"),
            ],
        );
        let (_, message) = thrown(r);
        s.echo(format!("unpack('{fmt}'): {message}\n"));
    }
    s.check(phpt!("unpack_endian_modifier_names_error.phpt"));
}

#[test]
fn unpack_error_phpt() {
    let mut s = Script::new("/t.php");
    let p = s.pack("I", vec![int(65534)]);
    let r = s.try_call("unpack", vec![Value::str("B"), p]);
    s.echo_thrown(r);
    s.check(phpt!("unpack_error.phpt"));
}

#[test]
fn unpack_offset_phpt() {
    let mut s = Script::new("/t.php");
    let mut data = b"pad".to_vec();
    data.extend(bytes(&s.pack("ll", vec![int(0x01020304), int(0x05060708)])));
    let data = Value::bytes(&data);
    let a = s.call("unpack", vec![Value::str("l2"), data.clone(), int(3)]);
    let (Value::Int(a1), Value::Int(a2)) = (elem(&a, 1), elem(&a, 2)) else {
        panic!("ints expected")
    };
    s.echo(format!("0x{a1:08x} 0x{a2:08x}\n"));
    let b = s.call("unpack", vec![Value::str("l"), data.clone(), int(3)]);
    let c = s.call("unpack", vec![Value::str("@4/l"), data, int(3)]);
    let (Value::Int(b1), Value::Int(c1)) = (elem(&b, 1), elem(&c, 1)) else {
        panic!("ints expected")
    };
    s.echo(format!("0x{b1:08x} 0x{c1:08x}\n"));
    for offset in [10, -1] {
        let r = s.try_call(
            "unpack",
            vec![Value::str("l"), Value::str("foo"), int(offset)],
        );
        s.echo_thrown(r);
    }
    // The ReflectionFunction part of the test checks the parameter names,
    // which come straight from the stub signature.
    for name in ["format", "string", "offset"] {
        assert!(super::UNPACK.sig.contains(&format!("${name}")));
        s.echo(format!("${name}\n"));
    }
    s.check(phpt!("unpack_offset.phpt"));
}

#[test]
fn unpack_bug68225_phpt() {
    let mut s = Script::new("/t.php");
    let data = s.pack("VV", vec![int(1), int(2)]);
    for fmt in ["Va/X", "Va/X4", "V1a/X4/V1b/V1c/X4/V1d"] {
        let u = s.unpack(fmt, &data);
        s.var_dump(&u);
    }
    s.check(phpt!("unpack_bug68225.phpt"));
}

#[test]
fn bug35817_phpt() {
    let mut s = Script::new("/t.php");
    for (fmt, v) in [("H3", "181"), ("H2", "18"), ("H", "1")] {
        let p = s.pack(fmt, vec![Value::str(v)]);
        let u = s.unpack(fmt, &p);
        s.var_dump(&u);
    }
    s.check(phpt!("bug35817.phpt"));
}

#[test]
fn bug36148_phpt() {
    let mut s = Script::new("/t.php");
    for v in ["a", "aa", "aaa", "aaaa"] {
        let p = s.pack("H*", vec![Value::str(v)]);
        let u = s.unpack("H*", &p);
        s.echo(format!("{v}: "));
        s.var_dump(&u);
    }
    s.check(phpt!("bug36148.phpt"));
}

#[test]
fn bug38770_phpt() {
    let mut s = Script::new("/t.php");
    for code in ["N", "l"] {
        let p = s.pack(code, vec![int(-30000)]);
        let u = s.unpack(code, &p);
        s.print_r(&u);
    }
    s.echo("Done\n");
    s.check(phpt!("bug38770.phpt"));
}

#[test]
fn bug53021_unpack_part() {
    // html_entity_decode("&#233;", ENT_QUOTES, "ISO-8859-1") is "\xe9".
    let mut s = Script::new("/t.php");
    let u = s.unpack("H*", &Value::bytes(b"\xe9"));
    s.var_dump(&u);
    assert_eq!(
        String::from_utf8_lossy(&s.out),
        "array(1) {\n  [1]=>\n  string(2) \"e9\"\n}\n"
    );
}

#[test]
fn bug61038_phpt() {
    let mut s = Script::new("/t.php");
    for fmt in ["a4", "a5", "a6", "a*"] {
        let u = s.unpack(fmt, &Value::bytes(b"str\0\0"));
        s.var_dump(&u);
    }
    s.check(phpt!("bug61038.phpt"));
}

#[test]
fn bug61764_phpt() {
    let mut s = Script::new("/t.php");
    let p = s.pack("L", vec![int(-30000)]);
    let u = s.unpack("I", &p);
    s.print_r(&u);
    s.check(phpt!("bug61764.phpt"));
}

#[test]
fn bug69522_phpt() {
    let mut s = Script::new("/t.php");
    let a = s.pack("AAAAAAAAAAAA", (1..=12).map(int).collect());
    let r = s.unpack("h2147483648", &a);
    assert_same!(r, Value::Bool(false));
    s.check(phpt!("bug69522.phpt"));
}

#[test]
fn bug70487_phpt() {
    let mut s = Script::new("/t.php");
    let p = s.pack("x", vec![]);
    s.var_dump(&Value::Bool(is_identical(&p, &Value::bytes(b"\0"))));
    s.check(phpt!("bug70487.phpt"));
}

#[test]
fn bug75075_phpt() {
    let mut s = Script::new("/tests/bug75075.php");
    let u = s.unpack("X*", &Value::str(""));
    s.var_dump(&u);
    s.check(phpt!("bug75075.phpt"));
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

#[test]
fn gh10940_phpt() {
    let mut s = Script::new("/t.php");
    let a = s.pack("AAAAAAAAAAAA", (1..=12).map(int).collect());
    s.unpack("h2147483647", &a);
    s.check(phpt!("gh10940.phpt"));
}

// ---------------------------------------------------------------------------
// pack.phpt (a 32-bit test upstream): the same cases with 64-bit results.
// ---------------------------------------------------------------------------

/// `unpack(code, pack(code, v))[1]` plus the warnings emitted on the way.
fn roundtrip(code: &str, v: Value) -> (Value, String) {
    let mut s = Script::new("/t.php");
    let p = s.pack(code, vec![v]);
    let u = s.unpack(code, &p);
    (elem(&u, 1), String::from_utf8_lossy(&s.out).into_owned())
}

#[test]
fn pack_phpt_cases_on_64_bit() {
    let ints: &[(&str, i64, i64)] = &[
        ("C", -127, 129),
        ("C", 127, 127),
        ("C", 255, 255),
        ("C", -129, 127),
        ("I", 65534, 65534),
        ("I", 0, 0),
        ("I", -1000, 4294966296),
        ("I", -64434, 4294902862),
        ("L", 65534, 65534),
        ("L", 0, 0),
        ("L", 2147483650, 2147483650),
        ("L", 4294967295, 4294967295),
        ("L", -2147483648, 2147483648),
        ("N", 65534, 65534),
        ("N", 0, 0),
        ("N", 2147483650, 2147483650),
        ("N", 4294967296, 0),
        ("N", -2147483648, 2147483648),
        ("N", -30000, 4294937296),
        ("S", 65534, 65534),
        ("S", 65537, 1),
        ("S", 0, 0),
        ("S", -1000, 64536),
        ("S", -64434, 1102),
        ("S", -65535, 1),
        ("V", 65534, 65534),
        ("V", 0, 0),
        ("V", 2147483650, 2147483650),
        ("V", 4294967296, 0),
        ("V", -2147483648, 2147483648),
        ("c", -127, -127),
        ("c", 127, 127),
        ("c", 255, -1),
        ("c", -129, 127),
        ("i", 65534, 65534),
        ("i", 0, 0),
        ("i", -1000, -1000),
        ("i", -64434, -64434),
        ("i", -65535, -65535),
        ("i", -2147483647, -2147483647),
        ("l", 65534, 65534),
        ("l", 0, 0),
        ("l", 2147483650, -2147483646),
        ("l", 4294967296, 0),
        ("l", -2147483648, -2147483648),
        ("n", 65534, 65534),
        ("n", 65537, 1),
        ("n", 0, 0),
        ("n", -1000, 64536),
        ("n", -64434, 1102),
        ("n", -65535, 1),
        ("s", 32767, 32767),
        ("s", 65535, -1),
        ("s", 0, 0),
        ("s", -1000, -1000),
        ("s", -64434, 1102),
        ("s", -65535, 1),
        ("v", 65534, 65534),
        ("v", 65537, 1),
        ("v", 0, 0),
        ("v", -1000, 64536),
        ("v", -64434, 1102),
        ("v", -65535, 1),
    ];
    for &(code, input, expected) in ints {
        let (got, out) = roundtrip(code, int(input));
        assert_same!(got, int(expected), format!("{code} {input}"));
        assert_eq!(out, "", "{code} {input}");
    }

    let strs: &[(&str, &str, &str)] = &[
        ("A", "hello world", "h"),
        ("A*", "hello world", "hello world"),
        ("a", "hello world", "h"),
        ("a*", "hello world", "hello world"),
    ];
    for &(code, input, expected) in strs {
        let (got, _) = roundtrip(code, Value::str(input));
        assert_same!(got, Value::str(expected), code);
    }

    let mut s = Script::new("/t.php");
    let p = s.pack("A9", vec![Value::str("hello")]);
    assert_same!(p, Value::str("hello    "));

    // pack("H", 0x04) packs the string "4"; pack("h", 3000000) packs "3".
    assert_same!(roundtrip("H", int(4)).0, Value::str("4"));
    assert_same!(roundtrip("h", int(3000000)).0, Value::str("3"));
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

fn unpack_out(fmt: &str, data: &[u8]) -> (Value, String) {
    let mut s = Script::new("/t.php");
    let u = s.unpack(fmt, &Value::bytes(data));
    (u, String::from_utf8_lossy(&s.out).into_owned())
}

fn dump(v: &Value) -> String {
    let mut out = Vec::new();
    var_dump_into(&mut out, v, 0);
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn unpack_names_and_keys() {
    let (u, _) = unpack_out("C2chars/nint", b"\x01\x02\x00\x03");
    assert_eq!(
        dump(&u),
        "array(3) {\n  [\"chars1\"]=>\n  int(1)\n  [\"chars2\"]=>\n  int(2)\n  [\"int\"]=>\n  int(3)\n}\n"
    );
    // Later codes overwrite earlier keys in place.
    let (u, _) = unpack_out("C2/C", b"\x01\x02\x03");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [1]=>\n  int(3)\n  [2]=>\n  int(2)\n}\n"
    );
    // Numeric names become integer keys.
    let (u, _) = unpack_out("C2", b"\x05\x06");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [1]=>\n  int(5)\n  [2]=>\n  int(6)\n}\n"
    );
    let (u, _) = unpack_out("C7/C2", b"\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d");
    let Value::Array(a) = &u else { panic!() };
    assert_eq!(a.len(), 7);
    let (u, _) = unpack_out("Cx/C2y", b"\x05\x06\x07");
    assert_eq!(
        dump(&u),
        "array(3) {\n  [\"x\"]=>\n  int(5)\n  [\"y1\"]=>\n  int(6)\n  [\"y2\"]=>\n  int(7)\n}\n"
    );
    let (u, _) = unpack_out("C1/C2", b"\x05\x06\x07");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [1]=>\n  int(6)\n  [2]=>\n  int(7)\n}\n"
    );
    let (u, _) = unpack_out("Cfoo1/Cbar", b"\x05\x06");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [\"foo1\"]=>\n  int(5)\n  [\"bar\"]=>\n  int(6)\n}\n"
    );
    let (u, _) = unpack_out("C12", b"\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10");
    let Value::Array(a) = &u else { panic!() };
    assert_eq!(a.len(), 12);
    // Names are capped at 200 bytes.
    let long = "n".repeat(250);
    let (u, _) = unpack_out(&format!("C{long}"), b"\x05");
    let Value::Array(a) = &u else { panic!() };
    assert!(a.get_str(&long.as_bytes()[..200]).is_some());
}

#[test]
fn unpack_strings_and_hex() {
    let (u, _) = unpack_out("Nlen/a*data", b"\0\0\0\x03abc");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [\"len\"]=>\n  int(3)\n  [\"data\"]=>\n  string(3) \"abc\"\n}\n"
    );
    let (u, _) = unpack_out("a0", b"abc");
    assert_eq!(dump(&u), "array(1) {\n  [1]=>\n  string(0) \"\"\n}\n");
    let (u, _) = unpack_out("h*", b"\xab\x0c");
    assert_eq!(dump(&u), "array(1) {\n  [1]=>\n  string(4) \"bac0\"\n}\n");
    let (u, _) = unpack_out("H3", b"\xab\xcd");
    assert_eq!(dump(&u), "array(1) {\n  [1]=>\n  string(3) \"abc\"\n}\n");
    // 'H*' moves the input position back by one (`inputpos += size` with size -1).
    let (u, _) = unpack_out("H*h/Cc", b"\x12\x34");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [\"h\"]=>\n  string(4) \"1234\"\n  [\"c\"]=>\n  int(18)\n}\n"
    );
    let (u, _) = unpack_out("A*", b"  \0");
    assert_eq!(dump(&u), "array(1) {\n  [1]=>\n  string(0) \"\"\n}\n");
}

#[test]
fn unpack_positioning_codes() {
    let (u, out) = unpack_out("Ca/x/Cb", b"\x01\x02\x03");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [\"a\"]=>\n  int(1)\n  [\"b\"]=>\n  int(3)\n}\n"
    );
    assert_eq!(out, "");
    let (u, _) = unpack_out("C*", b"\x01\x02\x03");
    assert_eq!(
        dump(&u),
        "array(3) {\n  [1]=>\n  int(1)\n  [2]=>\n  int(2)\n  [3]=>\n  int(3)\n}\n"
    );
    let (u, _) = unpack_out("@2/Ca", b"\x01\x02\x03");
    assert_eq!(dump(&u), "array(1) {\n  [\"a\"]=>\n  int(3)\n}\n");
    let (u, out) = unpack_out("@5/Ca", b"\x01\x02\x03");
    assert_eq!(dump(&u), "array(1) {\n  [\"a\"]=>\n  int(1)\n}\n");
    assert_eq!(out, warning("unpack", "Type @: outside of string"));
    let (u, out) = unpack_out("C/@*/Ca", b"\x01\x02\x03");
    assert_eq!(
        dump(&u),
        "array(2) {\n  [1]=>\n  int(1)\n  [\"a\"]=>\n  int(1)\n}\n"
    );
    assert_eq!(out, warning("unpack", "Type @: outside of string"));
    // X never warns: its size is -1.
    let (u, out) = unpack_out("X/Ca", b"\x01");
    assert_eq!(dump(&u), "array(1) {\n  [\"a\"]=>\n  int(1)\n}\n");
    assert_eq!(out, "");
    let (u, out) = unpack_out("x*", b"\x01\x02");
    assert_eq!(dump(&u), "array(0) {\n}\n");
    assert_eq!(out, "");
}

#[test]
fn unpack_failures() {
    let (u, out) = unpack_out("c2", b"a");
    assert_same!(u, Value::Bool(false));
    assert_eq!(
        out,
        warning(
            "unpack",
            "Type c: not enough input values, need 1 values but only 0 were provided"
        )
    );
    let (u, out) = unpack_out("N", b"ab");
    assert_same!(u, Value::Bool(false));
    assert_eq!(
        out,
        warning(
            "unpack",
            "Type N: not enough input values, need 4 values but only 2 were provided"
        )
    );
    let (u, out) = unpack_out("C99999999999999999999", b"a");
    assert_same!(u, Value::Bool(false));
    assert_eq!(out, warning("unpack", "Type C: integer overflow"));
    let mut t = TestVm::new();
    let r = t.call("unpack", vec![Value::str("\u{7f}"), Value::str("")]);
    assert_eq!(
        thrown(r),
        ("ValueError".into(), "Invalid format type \u{7f}".into())
    );
    // An empty format gives an empty array; offset == strlen is allowed.
    let r = t
        .call("unpack", vec![Value::str(""), Value::str("abc"), int(3)])
        .unwrap();
    assert_eq!(dump(&r), "array(0) {\n}\n");
}

#[test]
fn unpack_signedness() {
    let (u, _) = unpack_out("c/C/s/S", b"\xff\xff\xff\xff\xff\xff");
    assert_eq!(
        dump(&u),
        "array(1) {\n  [1]=>\n  int(65535)\n}\n",
        "each code overwrites key 1"
    );
    let (u, _) = unpack_out("i", &(-5i32).to_ne_bytes());
    assert_same!(elem(&u, 1), int(-5));
    let (u, _) = unpack_out("I", &(-5i32).to_ne_bytes());
    assert_same!(elem(&u, 1), int(4294967291));
    let (u, _) = unpack_out("Q", &u64::MAX.to_ne_bytes());
    assert_same!(elem(&u, 1), int(-1));
    let (u, _) = unpack_out("J", b"\x80\0\0\0\0\0\0\0");
    assert_same!(elem(&u, 1), int(i64::MIN));
    let (u, _) = unpack_out("d>", &2.5f64.to_be_bytes());
    assert_same!(elem(&u, 1), float(2.5));
    let (u, _) = unpack_out("f", &0.1f32.to_ne_bytes());
    assert_same!(elem(&u, 1), float(f64::from(0.1f32)));
}
