//! Unit tests ported from `ext/filter/tests/*.phpt`.
//!
//! Where a test only prints `var_dump()`s, diagnostics and literal lines,
//! the output is rebuilt here and compared with the test's `--EXPECT--`
//! section, read from `tests/phpt/ext/filter/tests/`.

use std::fmt::Write as _;

use php_runtime::class::{ClassEntry, Function, member_flags};
use php_runtime::value::float_fmt::format_h;
use php_runtime::{
    Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, Registry, Value, Vm,
};

use super::*;
use crate::testing::{TestVm, thrown};

macro_rules! phpt {
    ($name:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/phpt/ext/filter/tests/",
            $name
        ))
    };
}

/// `assert_eq!` for [`Value`]s (compared through their `var_dump()`).
macro_rules! assert_v {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => {
        assert_eq!(dump(&$a), dump(&$b) $(, $($msg)+)?)
    };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn s(x: &str) -> Value {
    Value::str(x)
}

fn bytes(x: &[u8]) -> Value {
    Value::bytes(x)
}

fn int(n: i64) -> Value {
    Value::Int(n)
}

fn list(vals: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(vals))
}

fn assoc(pairs: Vec<(&str, Value)>) -> Value {
    Value::array(PhpArray::from_pairs(pairs))
}

fn flags(f: i64) -> Value {
    assoc(vec![("flags", int(f))])
}

fn options(o: Value) -> Value {
    assoc(vec![("options", o)])
}

/// `var_dump()` output.
fn dump(v: &Value) -> String {
    let mut out = String::new();
    dump_value(&mut out, v, 0, false);
    out
}

fn dump_value(out: &mut String, v: &Value, indent: usize, is_ref: bool) {
    let pad = " ".repeat(indent);
    let amp = if is_ref { "&" } else { "" };
    match v {
        Value::Ref(r) => dump_value(out, &r.get(), indent, true),
        Value::Undef | Value::Null => writeln!(out, "{pad}{amp}NULL").unwrap(),
        Value::Bool(b) => writeln!(out, "{pad}{amp}bool({b})").unwrap(),
        Value::Int(n) => writeln!(out, "{pad}{amp}int({n})").unwrap(),
        Value::Float(d) => writeln!(out, "{pad}{amp}float({})", format_h(*d, -1)).unwrap(),
        Value::String(x) => writeln!(
            out,
            "{pad}{amp}string({}) \"{}\"",
            x.len(),
            x.to_string_lossy()
        )
        .unwrap(),
        Value::Array(a) => {
            writeln!(out, "{pad}{amp}array({}) {{", a.len()).unwrap();
            for (k, v) in a.iter() {
                match k {
                    ArrayKey::Int(n) => writeln!(out, "{pad}  [{n}]=>").unwrap(),
                    ArrayKey::Str(k) => {
                        writeln!(out, "{pad}  [\"{}\"]=>", k.to_string_lossy()).unwrap()
                    }
                }
                dump_value(out, v, indent + 2, false);
            }
            writeln!(out, "{pad}}}").unwrap();
        }
        other => panic!("cannot dump {other:?}"),
    }
}

/// The `--EXPECT--` / `--EXPECTF--` section of a phpt file.
fn expected(src: &str) -> &str {
    let start = ["--EXPECT--\n", "--EXPECTF--\n"]
        .iter()
        .find_map(|m| src.find(m).map(|i| i + m.len()))
        .expect("phpt has an EXPECT section");
    let rest = &src[start..];
    let is_section = |line: &str| {
        line.len() > 4
            && line.starts_with("--")
            && line.ends_with("--")
            && line[2..line.len() - 2]
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c == b'_')
    };
    let end = rest
        .match_indices('\n')
        .find(|&(i, _)| is_section(rest[i + 1..].lines().next().unwrap_or("")))
        .map_or(rest.len(), |(i, _)| i);
    &rest[..end]
}

/// A test run collecting PHP-visible output like the phpt script would.
struct Run {
    t: TestVm,
    out: String,
}

impl Run {
    fn new() -> Run {
        Run {
            t: TestVm::new(),
            out: String::new(),
        }
    }

    /// Moves displayed diagnostics into the output (with the `%s`/`%d` of
    /// the expectations in place of the location).
    fn flush(&mut self) {
        let o = self.t.output();
        self.out
            .push_str(&o.replace(" in /t.php on line 3", " in %s on line %d"));
    }

    fn call(&mut self, name: &str, args: Vec<Value>) -> PResult<Value> {
        let r = self.t.call(name, args);
        self.flush();
        r
    }

    fn value(&mut self, name: &str, args: Vec<Value>) -> Value {
        match self.call(name, args) {
            Ok(v) => v,
            Err(e) => panic!("{name}() failed: {:?}", thrown(Err(e))),
        }
    }

    /// `var_dump(name(...args))`.
    fn var_dump(&mut self, name: &str, args: Vec<Value>) {
        let v = self.value(name, args);
        self.dump(&v);
    }

    /// `var_dump(filter_var(...args))`.
    fn fv(&mut self, args: Vec<Value>) {
        self.var_dump("filter_var", args);
    }

    fn dump(&mut self, v: &Value) {
        self.out.push_str(&dump(v));
    }

    fn echo(&mut self, x: &str) {
        self.out.push_str(x);
    }

    /// `try { name(...) } catch (Throwable $e) { echo $e::class, ': ', $e->getMessage(), "\n"; }`
    fn catch(&mut self, name: &str, args: Vec<Value>) {
        let r = self.call(name, args);
        let (class, msg) = thrown(r);
        writeln!(self.out, "{class}: {msg}").unwrap();
    }

    /// A constant fetch from code (emits the deprecation of deprecated ones).
    fn constant(&mut self, name: &str) -> Value {
        let v = self.t.vm.fetch_constant(name).unwrap().unwrap();
        // `Vm::fetch_constant` always joins the suffix with ", ", while
        // php-src prints `#[\Deprecated(since: ...)]` as " since X, msg".
        let o = self
            .t
            .output()
            .replace("deprecated, since", "deprecated since");
        self.out
            .push_str(&o.replace(" in /t.php on line 3", " in %s on line %d"));
        v
    }

    fn input(&mut self, input_type: i64, pairs: Vec<(&str, Value)>) {
        set_input_array(
            &mut self.t.vm,
            input_type,
            Some(PhpArray::from_pairs(pairs)),
        );
    }

    /// Compares with the expectation like run-tests.php (both trimmed).
    fn check(&self, src: &str) {
        assert_eq!(self.out.trim(), expected(src).trim());
    }
}

/// `filter_var($value, $filter, $options)` in a fresh VM (no diagnostics).
fn filter_var(value: Value, filter: i64, opts: Value) -> Value {
    let mut t = TestVm::new();
    let v = t
        .call("filter_var", vec![value, int(filter), opts])
        .expect("no exception");
    assert_eq!(t.output(), "", "unexpected diagnostics");
    v
}

fn stdclass(t: &TestVm) -> Value {
    Value::Object(ObjRef::new(t.vm.find_class("stdClass").unwrap()))
}

fn to_string_handler(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(args
        .this()
        .and_then(|o| o.get_prop(b"wrapped"))
        .unwrap_or_default())
}

static TO_STRING: NativeFunction = NativeFunction {
    sig: "public function __toString(): string",
    handler: to_string_handler,
};

/// An instance of a class whose `__toString()` returns `x`.
fn stringable(t: &mut TestVm, x: &str) -> Value {
    const CLASS: &str = "FilterTestString";
    if t.vm.find_class(CLASS).is_none() {
        let mut ce = ClassEntry::new(CLASS, 0);
        ce.add_method(Function::native_method(
            &TO_STRING,
            CLASS,
            member_flags::PUBLIC,
        ));
        Registry::new(&mut t.vm, "filter").class(ce);
    }
    let obj = ObjRef::new(t.vm.find_class(CLASS).unwrap());
    obj.set_prop(b"wrapped", s(x));
    Value::Object(obj)
}

/// The values of the PHP array literal following `marker`: strings, ints,
/// booleans, empty arrays and `key => value` pairs.
fn php_literals(src: &str, marker: &str) -> Vec<(Value, Option<Value>)> {
    let b = src.as_bytes();
    let mut i = src.find(marker).expect("marker") + marker.len();
    let skip_ws = |i: &mut usize| {
        while b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    let mut out = Vec::new();
    loop {
        skip_ws(&mut i);
        if b[i] == b')' || b[i] == b']' {
            return out;
        }
        let v = php_literal(b, &mut i);
        skip_ws(&mut i);
        let pair = if b[i..].starts_with(b"=>") {
            i += 2;
            skip_ws(&mut i);
            Some(php_literal(b, &mut i))
        } else {
            None
        };
        out.push((v, pair));
        skip_ws(&mut i);
        if b[i] == b',' {
            i += 1;
        }
    }
}

fn php_literal(b: &[u8], i: &mut usize) -> Value {
    let rest = &b[*i..];
    for (lit, v) in [
        ("array()", list(vec![])),
        ("[]", list(vec![])),
        ("true", Value::Bool(true)),
        ("false", Value::Bool(false)),
    ] {
        if rest.starts_with(lit.as_bytes()) {
            *i += lit.len();
            return v;
        }
    }
    match b[*i] {
        q @ (b'\'' | b'"') => {
            *i += 1;
            let mut out = Vec::new();
            while b[*i] != q {
                if b[*i] == b'\\' {
                    let unescaped = match (q, b[*i + 1]) {
                        (_, b'\\') => Some(b'\\'),
                        (b'\'', b'\'') => Some(b'\''),
                        (b'"', b'"') => Some(b'"'),
                        (b'"', b'n') => Some(b'\n'),
                        (b'"', b'r') => Some(b'\r'),
                        (b'"', b't') => Some(b'\t'),
                        _ => None,
                    };
                    if let Some(c) = unescaped {
                        out.push(c);
                        *i += 2;
                        continue;
                    }
                }
                out.push(b[*i]);
                *i += 1;
            }
            *i += 1;
            bytes(&out)
        }
        _ => {
            let start = *i;
            while b[*i] == b'-' || b[*i].is_ascii_digit() {
                *i += 1;
            }
            int(std::str::from_utf8(&b[start..*i]).unwrap().parse().unwrap())
        }
    }
}

/// The single-quoted strings of a line.
fn quoted(line: &str) -> Vec<&str> {
    line.split('\'').skip(1).step_by(2).collect()
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn constants_ini_and_classes() {
    let mut t = TestVm::new();
    for (name, value) in [
        ("INPUT_POST", 0),
        ("INPUT_GET", 1),
        ("INPUT_COOKIE", 2),
        ("INPUT_ENV", 4),
        ("INPUT_SERVER", 5),
        ("FILTER_FLAG_NONE", 0),
        ("FILTER_REQUIRE_SCALAR", 33554432),
        ("FILTER_REQUIRE_ARRAY", 16777216),
        ("FILTER_FORCE_ARRAY", 67108864),
        ("FILTER_NULL_ON_FAILURE", 134217728),
        ("FILTER_THROW_ON_FAILURE", 268435456),
        ("FILTER_VALIDATE_INT", 257),
        ("FILTER_VALIDATE_BOOLEAN", 258),
        ("FILTER_VALIDATE_BOOL", 258),
        ("FILTER_VALIDATE_FLOAT", 259),
        ("FILTER_VALIDATE_REGEXP", 272),
        ("FILTER_VALIDATE_DOMAIN", 277),
        ("FILTER_VALIDATE_URL", 273),
        ("FILTER_VALIDATE_EMAIL", 274),
        ("FILTER_VALIDATE_IP", 275),
        ("FILTER_VALIDATE_MAC", 276),
        ("FILTER_DEFAULT", 516),
        ("FILTER_UNSAFE_RAW", 516),
        ("FILTER_SANITIZE_STRING", 513),
        ("FILTER_SANITIZE_STRIPPED", 513),
        ("FILTER_SANITIZE_ENCODED", 514),
        ("FILTER_SANITIZE_SPECIAL_CHARS", 515),
        ("FILTER_SANITIZE_FULL_SPECIAL_CHARS", 522),
        ("FILTER_SANITIZE_EMAIL", 517),
        ("FILTER_SANITIZE_URL", 518),
        ("FILTER_SANITIZE_NUMBER_INT", 519),
        ("FILTER_SANITIZE_NUMBER_FLOAT", 520),
        ("FILTER_SANITIZE_ADD_SLASHES", 523),
        ("FILTER_CALLBACK", 1024),
        ("FILTER_FLAG_ALLOW_OCTAL", 1),
        ("FILTER_FLAG_ALLOW_HEX", 2),
        ("FILTER_FLAG_STRIP_LOW", 4),
        ("FILTER_FLAG_STRIP_HIGH", 8),
        ("FILTER_FLAG_STRIP_BACKTICK", 512),
        ("FILTER_FLAG_ENCODE_LOW", 16),
        ("FILTER_FLAG_ENCODE_HIGH", 32),
        ("FILTER_FLAG_ENCODE_AMP", 64),
        ("FILTER_FLAG_NO_ENCODE_QUOTES", 128),
        ("FILTER_FLAG_EMPTY_STRING_NULL", 256),
        ("FILTER_FLAG_ALLOW_FRACTION", 4096),
        ("FILTER_FLAG_ALLOW_THOUSAND", 8192),
        ("FILTER_FLAG_ALLOW_SCIENTIFIC", 16384),
        ("FILTER_FLAG_PATH_REQUIRED", 262144),
        ("FILTER_FLAG_QUERY_REQUIRED", 524288),
        ("FILTER_FLAG_IPV4", 1048576),
        ("FILTER_FLAG_IPV6", 2097152),
        ("FILTER_FLAG_NO_RES_RANGE", 4194304),
        ("FILTER_FLAG_NO_PRIV_RANGE", 8388608),
        ("FILTER_FLAG_GLOBAL_RANGE", 536870912),
        ("FILTER_FLAG_HOSTNAME", 1048576),
        ("FILTER_FLAG_EMAIL_UNICODE", 1048576),
    ] {
        assert_v!(t.vm.constant(name).unwrap(), Value::Int(value), "{name}");
    }
    // Removed in PHP 8.0.
    assert!(t.vm.constant("FILTER_FLAG_SCHEME_REQUIRED").is_none());

    assert_eq!(t.vm.ini_get("filter.default").unwrap(), "unsafe_raw");
    assert_eq!(t.vm.ini_get("filter.default_flags").unwrap(), "");
    // PHP_INI_SYSTEM|PHP_INI_PERDIR: not changeable at runtime.
    assert!(t.vm.ini_set("filter.default", "int").is_none());
    assert!(t.vm.ini_set("filter.default_flags", "4").is_none());

    let failed = t.vm.find_class(FILTER_FAILED_EXCEPTION).unwrap();
    let parent = failed.parent.clone().unwrap();
    assert_eq!(parent.name, FILTER_EXCEPTION);
    assert_eq!(parent.parent.as_ref().unwrap().name, "Exception");
    assert_eq!(t.output(), "");
}

/// 008.phpt: filter_list().
#[test]
fn filter_list_008() {
    let mut r = Run::new();
    r.var_dump("filter_list", vec![]);
    r.echo("Done\n");
    r.check(phpt!("008.phpt"));
}

/// 009.phpt: filter_id().
#[test]
fn filter_id_009() {
    let mut r = Run::new();
    for name in ["stripped", "string", "url", "int", "none"] {
        r.var_dump("filter_id", vec![s(name)]);
    }
    r.var_dump("filter_id", vec![int(-1)]);
    r.echo("Done\n");
    r.check(phpt!("009.phpt"));

    // The name is compared as a C string.
    let mut r = Run::new();
    assert_v!(r.value("filter_id", vec![bytes(b"int\0x")]), int(257));
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_INT
// ---------------------------------------------------------------------------

/// 010.phpt: arrays, unknown filters.
#[test]
fn filter_var_010() {
    let mut r = Run::new();
    r.fv(vec![
        list(vec![
            int(1),
            s("1"),
            s(""),
            s("-23234"),
            s("text"),
            s("asdf234asdfgs"),
            list(vec![]),
        ]),
        int(FILTER_VALIDATE_INT),
        int(FILTER_REQUIRE_ARRAY),
    ]);
    r.fv(vec![
        list(vec![
            Value::Float(1.2),
            s("1.7"),
            s(""),
            s("-23234.123"),
            s("text"),
            s("asdf234.2asdfgs"),
            list(vec![]),
        ]),
        int(FILTER_VALIDATE_FLOAT),
        int(FILTER_REQUIRE_ARRAY),
    ]);
    r.fv(vec![int(1), int(FILTER_SANITIZE_SPECIAL_CHARS), int(1)]);
    r.fv(vec![int(1), int(FILTER_SANITIZE_SPECIAL_CHARS), int(0)]);
    r.fv(vec![
        int(1),
        int(FILTER_SANITIZE_SPECIAL_CHARS),
        list(vec![]),
    ]);
    r.fv(vec![int(1), int(-1), list(vec![int(123)])]);
    r.fv(vec![int(1), int(0), list(vec![])]);
    r.echo("Done\n");
    r.check(phpt!("010.phpt"));
}

/// 013.phpt: FILTER_VALIDATE_INT with flags and ranges.
#[test]
fn validate_int_013() {
    let mut r = Run::new();
    let int_flags = |v: &str, f: i64| vec![s(v), int(FILTER_VALIDATE_INT), flags(f)];
    let range = |v: Value, min: i64, max: i64| {
        vec![
            v,
            int(FILTER_VALIDATE_INT),
            options(assoc(vec![
                ("min_range", int(min)),
                ("max_range", int(max)),
            ])),
        ]
    };
    for v in ["  234", "234    ", "  234  "] {
        r.fv(vec![s(v), int(FILTER_VALIDATE_INT)]);
    }
    for (v, f) in [
        ("0xff", FILTER_FLAG_ALLOW_HEX),
        ("0Xff", FILTER_FLAG_ALLOW_HEX),
        ("0xFF", FILTER_FLAG_ALLOW_HEX),
        ("0XFF", FILTER_FLAG_ALLOW_HEX),
        ("07", FILTER_FLAG_ALLOW_OCTAL),
        ("0xff0000", FILTER_FLAG_ALLOW_HEX),
        ("0666", FILTER_FLAG_ALLOW_OCTAL),
        ("08", FILTER_FLAG_ALLOW_OCTAL),
        ("00", FILTER_FLAG_ALLOW_OCTAL),
        ("000", FILTER_FLAG_ALLOW_OCTAL),
        ("-0xff", FILTER_FLAG_ALLOW_HEX),
        ("-0Xff", FILTER_FLAG_ALLOW_HEX),
        ("-0xFF", FILTER_FLAG_ALLOW_HEX),
        ("-0XFF", FILTER_FLAG_ALLOW_HEX),
        ("-07", FILTER_FLAG_ALLOW_OCTAL),
        ("-0xff0000", FILTER_FLAG_ALLOW_HEX),
        ("-0666", FILTER_FLAG_ALLOW_OCTAL),
    ] {
        r.fv(int_flags(v, f));
    }
    r.fv(range(s("6"), 1, 7));
    r.fv(range(s("6"), 0, 5));
    r.fv(range(int(-1), 1, 7));
    r.fv(range(int(-1), -4, 7));
    r.fv(range(s(""), -4, 7));
    r.fv(range(s(""), 2, 7));
    r.fv(range(s(""), -5, -3));
    r.fv(range(int(345), 500, 100));
    r.fv(vec![s("0ff"), int(FILTER_VALIDATE_INT)]);
    r.fv(vec![s("010"), int(FILTER_VALIDATE_INT)]);
    for v in ["0o16", "0O16", "0o016"] {
        r.fv(int_flags(v, FILTER_FLAG_ALLOW_OCTAL));
    }
    r.echo("Done\n");
    r.check(phpt!("013.phpt"));
}

/// 046.phpt, 047.phpt, 048.phpt, bug47745.phpt: overflow at the edges.
#[test]
fn validate_int_overflow() {
    let v = |x: &str, f: i64| filter_var(s(x), FILTER_VALIDATE_INT, flags(f));
    // 046.phpt
    assert_v!(v("9223372036854775807", 0), int(i64::MAX));
    assert_v!(v("9223372036854775808", 0), Value::Bool(false));
    assert_v!(v("-9223372036854775808", 0), int(i64::MIN));
    assert_v!(v("-9223372036854775809", 0), Value::Bool(false));
    assert_v!(v("99999999999999999999", 0), Value::Bool(false));
    // 047.phpt
    let o = FILTER_FLAG_ALLOW_OCTAL;
    assert_v!(v("0777777777777777777777", o), int(i64::MAX));
    assert_v!(v("01000000000000000000000", o), int(i64::MIN));
    assert_v!(v("01777777777777777777777", o), int(-1));
    assert_v!(v("02000000000000000000000", o), Value::Bool(false));
    // 048.phpt
    let h = FILTER_FLAG_ALLOW_HEX;
    assert_v!(v("0x7fffffffffffffff", h), int(i64::MAX));
    assert_v!(v("0x8000000000000000", h), int(i64::MIN));
    assert_v!(v("0xffffffffffffffff", h), int(-1));
    assert_v!(v("0x10000000000000000", h), Value::Bool(false));
}

/// bug80584.phpt, surprising_integer_literals.phpt, bug8315.phpt,
/// filter_data.phpt (integers).
#[test]
fn validate_int_prefixes_and_data() {
    let f = Value::Bool(false);
    let hex = FILTER_FLAG_ALLOW_HEX;
    let oct = FILTER_FLAG_ALLOW_OCTAL;
    for (v, fl, want) in [
        ("0x", hex, f.clone()),
        ("0X", hex, f.clone()),
        ("0xg", hex, f.clone()),
        ("0Xg", hex, f.clone()),
        ("", hex, f.clone()),
        ("0", hex, int(0)),
        ("0o", oct, f.clone()),
        ("0og", oct, f.clone()),
        ("0O", oct, f.clone()),
        ("0Og", oct, f.clone()),
        ("O", oct, f.clone()),
        ("Og", oct, f.clone()),
        ("", oct, f.clone()),
    ] {
        assert_v!(
            filter_var(s(v), FILTER_VALIDATE_INT, int(fl)),
            want,
            "{v:?}"
        );
    }
    assert_v!(filter_var(bytes(b"3\0foo"), FILTER_VALIDATE_INT, int(0)), f);
    assert_v!(
        filter_var(bytes(b"3\0foo"), FILTER_VALIDATE_FLOAT, int(0)),
        f
    );

    for (v, want) in [
        (s("-123"), int(-123)),
        (s("0"), int(0)),
        (s("-0"), int(0)),
        (s("+0"), int(0)),
        (s("123"), int(123)),
        (int(-123), int(-123)),
        (int(0), int(0)),
        (int(123), int(123)),
        (s(""), f.clone()),
        (s("-00"), f.clone()),
        (s("+"), f.clone()),
        (s("\t\n 42 \x0b\r"), int(42)),
        // \f is not trimmed.
        (s("\x0c42"), f.clone()),
    ] {
        assert_v!(
            filter_var(v.clone(), FILTER_VALIDATE_INT, int(0)),
            want,
            "{v:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_BOOL
// ---------------------------------------------------------------------------

fn validate_bool_test(constant: &str, src: &str) {
    let mut r = Run::new();
    let filter = r.constant(constant);
    let obj = stringable(&mut r.t, "blah");
    let fvb = |r: &mut Run, v: Value, extra: Option<i64>| {
        let mut args = vec![v, filter.clone()];
        args.extend(extra.map(int));
        r.fv(args);
    };
    fvb(&mut r, s("no"), None);
    fvb(&mut r, Value::Null, None);
    fvb(&mut r, obj, None);
    fvb(
        &mut r,
        list(vec![
            int(1),
            int(2),
            int(3),
            int(0),
            list(vec![s(""), s("123")]),
        ]),
        Some(FILTER_REQUIRE_ARRAY),
    );
    for v in ["yes", "true", "false", "off", "on", "0", "1", "NONE", ""] {
        fvb(&mut r, s(v), None);
    }
    fvb(&mut r, int(-1), None);
    fvb(&mut r, s("000000"), None);
    fvb(&mut r, s("111111"), None);
    r.echo("Done\n");
    r.check(src);
}

/// 014.phpt: FILTER_VALIDATE_BOOLEAN.
#[test]
fn validate_boolean_014() {
    validate_bool_test("FILTER_VALIDATE_BOOLEAN", phpt!("014.phpt"));
}

/// 061.phpt: FILTER_VALIDATE_BOOL.
#[test]
fn validate_bool_061() {
    validate_bool_test("FILTER_VALIDATE_BOOL", phpt!("061.phpt"));
}

/// 034.phpt, bug49510.phpt, filter_data.phpt (booleans).
#[test]
fn validate_bool_values() {
    let (t, f) = (Value::Bool(true), Value::Bool(false));
    for (v, want) in [
        (s("1"), t.clone()),
        (s("On"), t.clone()),
        (s("True"), t.clone()),
        (s("TrUe"), t.clone()),
        (s("oN"), t.clone()),
        (s("yes"), t.clone()),
        (s("  yes\n"), t.clone()),
        (int(1), t.clone()),
        (Value::Bool(true), t.clone()),
        (s("0"), f.clone()),
        (s("Off"), f.clone()),
        (s("False"), f.clone()),
        (s("faLsE"), f.clone()),
        (s("oFf"), f.clone()),
        (s("no"), f.clone()),
        (s(""), f.clone()),
        (s("   "), f.clone()),
        (int(0), f.clone()),
        (Value::Bool(false), f.clone()),
    ] {
        assert_v!(
            filter_var(v.clone(), FILTER_VALIDATE_BOOL, int(0)),
            want,
            "{v:?}"
        );
        // bug49510.phpt: the recognised values are not failures.
        assert_v!(
            filter_var(v.clone(), FILTER_VALIDATE_BOOL, int(FILTER_NULL_ON_FAILURE)),
            want,
            "{v:?}"
        );
    }
    assert_v!(
        filter_var(
            s("invalid"),
            FILTER_VALIDATE_BOOL,
            int(FILTER_NULL_ON_FAILURE)
        ),
        Value::Null
    );
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_FLOAT
// ---------------------------------------------------------------------------

/// 031.phpt: floats and the "decimal" option.
#[test]
fn validate_float_031() {
    let mut r = Run::new();
    for v in [
        "1.234   ",
        "   1.234",
        "1.234",
        "1.2e3",
        "7E3",
        "7E3     ",
        "  7E3     ",
        "  7E-3     ",
    ] {
        r.fv(vec![s(v), int(FILTER_VALIDATE_FLOAT)]);
    }
    r.echo("\ncustom decimal:\n");
    for (v, dec) in [
        ("1.234   ", ","),
        ("1,234", ","),
        ("   1.234", "."),
        ("1.234", ".."),
        ("1.2e3", ","),
    ] {
        let args = vec![
            s(v),
            int(FILTER_VALIDATE_FLOAT),
            options(assoc(vec![("decimal", s(dec))])),
        ];
        if dec.len() == 1 {
            r.fv(args);
        } else {
            r.catch("filter_var", args);
        }
    }
    r.check(phpt!("031.phpt"));
}

/// 044.phpt, bug7715.phpt, bug7733.phpt, filter_data.phpt (floats).
#[test]
fn validate_float_values() {
    let f = Value::Bool(false);
    for (v, want) in [
        (s("\n "), f.clone()),
        (s(" "), f.clone()),
        (s(" 123"), Value::Float(123.0)),
        (s(" 123.01 "), Value::Float(123.01)),
        (s(".23"), Value::Float(0.23)),
        (s("-42"), Value::Float(-42.0)),
        (s("+42"), Value::Float(42.0)),
        (s(".4"), Value::Float(0.4)),
        (s("-.4"), Value::Float(-0.4)),
        (s("1000000000000"), Value::Float(1e12)),
        (s("-1000000000000"), Value::Float(-1e12)),
        (s("02.324"), Value::Float(2.324)),
        (s("E2"), f.clone()),
        (s("10E"), f.clone()),
        (s("2E-"), f.clone()),
        (s("E-2"), f.clone()),
        (s("+E2"), f.clone()),
        (s("-0.123"), Value::Float(-0.123)),
        (s("0.00"), Value::Float(0.0)),
        (Value::Float(-1.23), Value::Float(-1.23)),
        (Value::Float(0.0), Value::Float(0.0)),
        (s(""), f.clone()),
        (s("1."), Value::Float(1.0)),
        (s("."), f.clone()),
        // A zero mantissa with a non-zero exponent counts as an underflow.
        (s("0e5"), f.clone()),
        (s("1,000"), f.clone()),
    ] {
        assert_v!(
            filter_var(v.clone(), FILTER_VALIDATE_FLOAT, int(0)),
            want,
            "{v:?}"
        );
    }
}

/// 049.phpt: thousand separators.
#[test]
#[allow(clippy::excessive_precision)]
fn validate_float_thousand_049() {
    for (v, want) in [
        ("0", Some(0.0)),
        ("12345678900.1234567165", Some(12345678900.1234567165)),
        ("1,234,567,890.1234567165", Some(1234567890.1234567165)),
        ("-1,234,567,890.1234567165", Some(-1234567890.1234567165)),
        ("1234,567,890.1234567165", None),
        ("1,234,567,89.1234567165", None),
        ("1,234,567,8900.1234567165", None),
        ("1.234.567.890.1234567165", None),
        ("1,234,567,8900.123,456", None),
        ("1'234", Some(1234.0)),
    ] {
        let want = want.map_or(Value::Bool(false), Value::Float);
        assert_v!(
            filter_var(
                s(v),
                FILTER_VALIDATE_FLOAT,
                flags(FILTER_FLAG_ALLOW_THOUSAND)
            ),
            want,
            "{v}"
        );
    }
}

/// 050.phpt: overflow and underflow.
#[test]
fn validate_float_overflow_050() {
    for (v, want) in [
        ("1e+308", Some(1e308)),
        ("1e+309", None),
        ("1e-323", Some(1e-323)),
        ("1e-324", None),
    ] {
        let want = want.map_or(Value::Bool(false), Value::Float);
        assert_v!(filter_var(s(v), FILTER_VALIDATE_FLOAT, int(0)), want, "{v}");
    }
}

/// 060.phpt: range options.
#[test]
fn validate_float_ranges_060() {
    let mut r = Run::new();
    r.fv(vec![
        list(vec![
            Value::Null,
            Value::Bool(false),
            int(0),
            int(-1),
            s("-5.4"),
            s("-5.5"),
            s("2,000.00"),
            s("2,000.01"),
            s("1,999.9999999"),
        ]),
        int(FILTER_VALIDATE_FLOAT),
        assoc(vec![
            (
                "options",
                assoc(vec![
                    ("min_range", Value::Float(-5.4)),
                    ("max_range", int(2000)),
                ]),
            ),
            (
                "flags",
                int(FILTER_FLAG_ALLOW_THOUSAND | FILTER_REQUIRE_ARRAY),
            ),
        ]),
    ]);
    r.fv(vec![
        s("1000"),
        int(FILTER_VALIDATE_FLOAT),
        options(assoc(vec![
            ("max_range", Value::Float(999.999)),
            ("default", int(0)),
        ])),
    ]);
    r.fv(vec![
        s("-11"),
        int(FILTER_VALIDATE_FLOAT),
        options(assoc(vec![("min_range", int(-10)), ("default", int(0))])),
    ]);
    r.check(phpt!("060.phpt"));
}

/// bug51368.phpt (custom thousand separators), bug81708.phpt.
#[test]
fn validate_float_options() {
    let mut r = Run::new();
    let opts = |thousand: &str| {
        assoc(vec![
            ("flags", int(FILTER_FLAG_ALLOW_THOUSAND)),
            ("options", assoc(vec![("thousand", s(thousand))])),
        ])
    };
    let v = r.value(
        "filter_var",
        vec![s("1 000"), int(FILTER_VALIDATE_FLOAT), opts(" ")],
    );
    assert_v!(v, Value::Float(1000.0));
    let v = r.value(
        "filter_var",
        vec![s("1 234.567"), int(FILTER_VALIDATE_FLOAT), opts(" ")],
    );
    assert_v!(v, Value::Float(1234.567));
    let e = thrown(r.call(
        "filter_var",
        vec![s("12345"), int(FILTER_VALIDATE_FLOAT), opts("")],
    ));
    assert_eq!(
        e,
        (
            "ValueError".into(),
            "filter_var(): \"thousand\" option must not be empty".into()
        )
    );

    // bug81708.phpt: out-of-range integers fail without touching the input.
    let input = s("+11");
    let v = r.value(
        "filter_var",
        vec![
            input.clone(),
            int(FILTER_VALIDATE_FLOAT),
            options(assoc(vec![("min_range", int(-1)), ("max_range", int(1))])),
        ],
    );
    assert_v!(v, Value::Bool(false));
    assert_v!(input, s("+11"));
    assert_eq!(r.out, "");
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_URL
// ---------------------------------------------------------------------------

/// The URL checks of 015.phpt / 062.phpt; `extra` is a top-level entry of
/// the filter arguments (062.phpt's "uri_parser_class").
fn validate_urls(r: &mut Run, src: &str, marker: &str, extra: Option<Value>) {
    let url_opts = |f: Option<i64>| match (&extra, f) {
        (None, None) => int(0),
        (None, Some(f)) => int(f),
        (Some(p), None) => assoc(vec![("uri_parser_class", p.clone())]),
        (Some(p), Some(f)) => assoc(vec![("uri_parser_class", p.clone()), ("flags", int(f))]),
    };
    for (v, _) in php_literals(src, marker) {
        r.fv(vec![v, int(FILTER_VALIDATE_URL), url_opts(None)]);
    }
    for v in [
        "qwe",
        "http://qwe",
        "http://",
        "/tmp/test",
        "http://www.example.com",
    ] {
        r.fv(vec![s(v), int(FILTER_VALIDATE_URL), url_opts(None)]);
    }
    for (v, f) in [
        ("http://www.example.com", FILTER_FLAG_PATH_REQUIRED),
        (
            "http://www.example.com/path/at/the/server/",
            FILTER_FLAG_PATH_REQUIRED,
        ),
        (
            "http://www.example.com/index.html",
            FILTER_FLAG_QUERY_REQUIRED,
        ),
        (
            "http://www.example.com/index.php?a=b&c=d",
            FILTER_FLAG_QUERY_REQUIRED,
        ),
    ] {
        r.fv(vec![s(v), int(FILTER_VALIDATE_URL), url_opts(Some(f))]);
    }
}

/// 015.phpt: FILTER_VALIDATE_URL.
#[test]
fn validate_url_015() {
    let src = phpt!("015.phpt");
    let mut r = Run::new();
    validate_urls(&mut r, src, "$values = Array(", None);
    r.echo("Done\n");
    r.check(src);
}

/// 062.phpt: a top-level "uri_parser_class" is not an option (it would have
/// to be inside "options"), so both runs use the parse_url() parser.
#[test]
fn validate_url_062() {
    let src = phpt!("062.phpt");
    let mut r = Run::new();
    r.echo("RFC3986:\n");
    validate_urls(&mut r, src, "$values = [", Some(s("Uri\\Rfc3986Uri")));
    r.echo("\nWHATWG:\n");
    validate_urls(&mut r, src, "$values = [", Some(s("Uri\\WhatWgUri")));
    r.echo("Done\n");
    r.check(src);

    // As an option, only the parse_url() parser is known.
    let mut r = Run::new();
    let url = |parser: &str| {
        vec![
            s("http://example.com/"),
            int(FILTER_VALIDATE_URL),
            options(assoc(vec![("uri_parser_class", s(parser))])),
        ]
    };
    assert_v!(
        r.value("filter_var", url("parse_url")),
        s("http://example.com/")
    );
    let e = thrown(r.call("filter_var", url("Uri\\Rfc3986\\Uri")));
    assert_eq!(
        e.1,
        "filter_var(): \"uri_parser_class\" option has invalid value"
    );
}

/// bug51192, bug64441, bug77423, bug81122, ghsa-w8qr-v226-r27w.
#[test]
fn validate_url_bugs() {
    let f = Value::Bool(false);
    for (v, ok) in [
        ("http://example.com/path", true),
        ("http://exa-mple.com/path", true),
        ("http://exa_mple.com/path", false),
        ("http://example.com./", true),
        ("http://example.com/", true),
        ("http://php.net\\@aliyun.com/aaa.do", false),
        ("https://example.com\\uFF03@bing.com", false),
        ("https://example.com:\\@test.com/", false),
        ("https://user:\\epass@test.com", false),
        ("https://user:\\@test.com", false),
        ("http://t[est@127.0.0.1", false),
        ("http://t[est@[::1]", false),
        ("http://t[est@[::1", false),
        ("http://t[est@::1]", false),
        ("http://test[@2001:db8:3333:4444:5555:6666:1.2.3.4]", false),
        ("http://te[st@2001:db8:3333:4444:5555:6666:1.2.3.4]", false),
        ("http://te[st@2001:db8:3333:4444:5555:6666:1.2.3.4", false),
        ("http://test@127.0.0.1", true),
        ("http://test@[2001:db8:3333:4444:5555:6666:1.2.3.4]", true),
        ("http://test@[::1]", true),
        ("http://user:p%41ss@example.com/", true),
        ("http://user:p%4@example.com/", false),
        ("HTTPS://example.com", true),
        ("Mailto:foo@bar.com", false),
    ] {
        let want = if ok { s(v) } else { f.clone() };
        assert_v!(filter_var(s(v), FILTER_VALIDATE_URL, int(0)), want, "{v}");
    }
    assert_v!(
        filter_var(
            s("invalid"),
            FILTER_VALIDATE_URL,
            int(FILTER_NULL_ON_FAILURE)
        ),
        Value::Null
    );
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_EMAIL
// ---------------------------------------------------------------------------

/// 016.phpt: FILTER_VALIDATE_EMAIL.
#[test]
fn validate_email_016() {
    let src = phpt!("016.phpt");
    let mut r = Run::new();
    for (v, _) in php_literals(src, "$values = Array(") {
        r.fv(vec![v, int(FILTER_VALIDATE_EMAIL)]);
    }
    r.echo("Done\n");
    r.check(src);
}

/// 058.phpt: FILTER_FLAG_EMAIL_UNICODE.
#[test]
fn validate_email_unicode_058() {
    let src = phpt!("058.phpt");
    let mut r = Run::new();
    for (v, _) in php_literals(src, "$values = Array(") {
        r.fv(vec![
            v,
            int(FILTER_VALIDATE_EMAIL),
            int(FILTER_FLAG_EMAIL_UNICODE),
        ]);
    }
    r.echo("Done\n");
    r.check(src);
}

/// CVE-2007-1900, bug44445, bug50158, bug52929, bug55478.
#[test]
fn validate_email_bugs() {
    let f = Value::Bool(false);
    let long = format!("{}@{}.zz", "x".repeat(65), "y".repeat(300));
    let huge = "x".repeat(8000);
    for (v, ok) in [
        ("test@example.com\n", false),
        ("foo@-foo.com", false),
        ("foo@foo-.com", false),
        ("test=mail@example.com", true),
        ("test-mail@example.com", true),
        ("test+mail@example.com", true),
        ("test?mail@example.com", true),
        ("valid@email.address", true),
        (long.as_str(), false),
        (huge.as_str(), false),
        ("test@xn--example--7za.de", true),
    ] {
        let want = if ok { s(v) } else { f.clone() };
        assert_v!(filter_var(s(v), FILTER_VALIDATE_EMAIL, int(0)), want, "{v}");
    }
    // Not valid UTF-8 for the /u pattern.
    assert_v!(
        filter_var(
            bytes(b"t\xc3st@example.com"),
            FILTER_VALIDATE_EMAIL,
            int(FILTER_FLAG_EMAIL_UNICODE)
        ),
        f
    );
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_REGEXP
// ---------------------------------------------------------------------------

/// 017.phpt: FILTER_VALIDATE_REGEXP.
#[test]
fn validate_regexp_017() {
    let mut r = Run::new();
    for re in ["/.*/", "/^b(.*)/", "/^d(.*)/", "/blah/", "/\\[/"] {
        r.fv(vec![
            s("data"),
            int(FILTER_VALIDATE_REGEXP),
            options(assoc(vec![("regexp", s(re))])),
        ]);
    }
    r.catch("filter_var", vec![s("data"), int(FILTER_VALIDATE_REGEXP)]);
    r.echo("Done\n");
    r.check(phpt!("017.phpt"));
}

#[test]
fn validate_regexp_bad_pattern() {
    let mut r = Run::new();
    let v = r.value(
        "filter_var",
        vec![
            s("data"),
            int(FILTER_VALIDATE_REGEXP),
            options(assoc(vec![("regexp", s("data"))])),
        ],
    );
    assert_v!(v, Value::Bool(false));
    assert_eq!(
        r.out,
        "\nWarning: filter_var(): Delimiter must not be alphanumeric, backslash, or NUL byte in %s on line %d\n"
    );
    // A non-string "regexp" option counts as missing.
    let e = thrown(r.call(
        "filter_var",
        vec![
            s("data"),
            int(FILTER_VALIDATE_REGEXP),
            options(assoc(vec![("regexp", int(1))])),
        ],
    ));
    assert_eq!(e.1, "filter_var(): \"regexp\" option is missing");
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_IP
// ---------------------------------------------------------------------------

/// 018.phpt: FILTER_VALIDATE_IP.
#[test]
fn validate_ip_018() {
    let mut r = Run::new();
    for (v, f) in [
        (s("192.168.0.1"), 0),
        (s("192.168.0.1.1"), 0),
        (s("::1"), 0),
        (s("fe00::0"), 0),
        (s("::123456"), 0),
        (s("::1::b"), 0),
        (s("127.0.0.1"), 0),
        (s("192.168.0.1"), FILTER_FLAG_NO_PRIV_RANGE),
        (s("192.0.34.166"), FILTER_FLAG_NO_PRIV_RANGE),
        (s("127.0.0.1"), FILTER_FLAG_NO_RES_RANGE),
        (s("192.0.0.1"), FILTER_FLAG_NO_RES_RANGE),
        (s("100.64.0.0"), FILTER_FLAG_NO_RES_RANGE),
        (s("100.127.255.255"), FILTER_FLAG_NO_RES_RANGE),
        (s("192.0.34.166"), 0),
        (s("256.1237.123.1"), 0),
        (s("255.255.255.255"), 0),
        (s("255.255.255.255"), FILTER_FLAG_NO_RES_RANGE),
        (s(""), 0),
        (int(-1), 0),
        (s("::1"), FILTER_FLAG_IPV4),
        (s("127.0.0.1"), FILTER_FLAG_IPV6),
        (s("::1"), FILTER_FLAG_IPV6),
        (s("::1"), FILTER_FLAG_IPV6 | FILTER_FLAG_NO_RES_RANGE),
        (s("127.0.0.1"), FILTER_FLAG_IPV4),
    ] {
        r.fv(vec![v, int(FILTER_VALIDATE_IP), int(f)]);
    }
    r.echo("Done\n");
    r.check(phpt!("018.phpt"));
}

/// 019.phpt, bug39846.phpt, bug46973.phpt, bug47435.phpt, bug48762.phpt,
/// bug53150.phpt, bug61700.phpt, gh16944.phpt, gh18309.phpt.
#[test]
fn validate_ip_bugs() {
    let v6 = FILTER_FLAG_IPV6;
    let res = FILTER_FLAG_NO_RES_RANGE;
    let privr = FILTER_FLAG_NO_PRIV_RANGE;
    for (v, f, ok) in [
        ("....", 0, false),
        ("...", 0, false),
        ("..", 0, false),
        (".", 0, false),
        ("1.1.1.1", 0, true),
        ("192.168.1.100random-text-here", 0, false),
        ("192.168.1.155\0foo", 0, false),
        ("1fff::a88:85a3::172.31.128.1", v6, false),
        ("3ffe:6a88:85a3:08d3:1319:8a2e:0370:7344", v6, true),
        ("FC00::1", v6, true),
        ("FC00::1", v6 | privr, false),
        ("::", v6, true),
        ("::", v6 | res, false),
        ("::1", v6, true),
        ("::1", v6 | res, false),
        ("fe80:5:6::1", v6, true),
        ("fe80:5:6::1", v6 | res, false),
        ("2001:0db8::1", v6, true),
        ("2001:0db8::1", v6 | res, true),
        ("2001:0010::1", v6, true),
        ("2001:0010::1", v6 | res, true),
        ("240b:0010::1", v6, true),
        ("240b:0010::1", v6 | res, true),
        ("0b15:23::3:67.98.234.17", v6, true),
        ("::67:78b:34.43.43.2", v6, true),
        ("127.0.0.1", 0, true),
        ("::1", 0, true),
        ("127.0.0.1", res, false),
        ("::1", res, false),
        ("128.0.0.1", 0, true),
        ("128.0.0.1", res, true),
        ("191.255.0.0", 0, true),
        ("191.255.0.0", res, true),
        ("::ffff:192.168.1.1", FILTER_FLAG_IPV4, false),
        ("::ffff:192.168.1.1", privr, true),
        ("0:0:0:0:0:0:0:1", res, false),
        ("::ffff:0:1", res, false),
        ("fffffffffffffffffffffffffffffffffffff::", v6, false),
        ("1.2.3.04", 0, false),
        ("1.2.3.0", 0, true),
    ] {
        let want = if ok { s(v) } else { Value::Bool(false) };
        assert_v!(
            filter_var(s(v), FILTER_VALIDATE_IP, int(f)),
            want,
            "{v:?} {f:#x}"
        );
    }
}

/// 030.phpt: IPv6 syntax.
#[test]
fn validate_ipv6_030() {
    for (ip, exp) in php_literals(phpt!("030.phpt"), "$ipv6_test = array(") {
        let out = filter_var(ip.clone(), FILTER_VALIDATE_IP, int(FILTER_FLAG_IPV6));
        let exp = matches!(exp, Some(Value::Bool(true)));
        assert_eq!(!matches!(out, Value::Bool(false)), exp, "{ip:?}");
    }
}

/// filter_ipv4_rfc6890.phpt: private and reserved IPv4 ranges.
#[test]
fn validate_ipv4_rfc6890() {
    let src = phpt!("filter_ipv4_rfc6890.phpt");
    let mut r = Run::new();
    for (var, f) in [
        ("$privateRanges['", FILTER_FLAG_NO_PRIV_RANGE),
        ("$reservedRanges['", FILTER_FLAG_NO_RES_RANGE),
    ] {
        for line in src.lines().filter(|l| l.starts_with(var)) {
            let q = quoted(line);
            r.dump(&s(q[0]));
            for ip in &q[1..3] {
                r.fv(vec![
                    s(ip),
                    int(FILTER_VALIDATE_IP),
                    int(FILTER_FLAG_IPV4 | f),
                ]);
            }
        }
    }
    r.check(src);
}

/// bug77221.phpt: FILTER_FLAG_GLOBAL_RANGE.
#[test]
fn validate_ip_global_range_77221() {
    let src = phpt!("bug77221.phpt");
    let mut r = Run::new();
    for line in src
        .lines()
        .filter(|l| l.starts_with("$non_global_ranges['"))
    {
        let q = quoted(line);
        r.dump(&s(q[0]));
        for ip in &q[1..3] {
            r.fv(vec![
                s(ip),
                int(FILTER_VALIDATE_IP),
                int(FILTER_FLAG_GLOBAL_RANGE),
            ]);
        }
    }
    for ip in ["185.85.0.29", "64:ff9b::", "64:ff9b::ffff"] {
        r.fv(vec![
            s(ip),
            int(FILTER_VALIDATE_IP),
            int(FILTER_FLAG_GLOBAL_RANGE),
        ]);
    }
    r.check(src);
}

// ---------------------------------------------------------------------------
// FILTER_VALIDATE_MAC / FILTER_VALIDATE_DOMAIN
// ---------------------------------------------------------------------------

/// 055.phpt: FILTER_VALIDATE_MAC.
#[test]
fn validate_mac_055() {
    let mut r = Run::new();
    let sep = |x: &str| options(assoc(vec![("separator", s(x))]));
    for (v, o) in [
        ("01-23-45-67-89-ab", int(0)),
        ("01-23-45-67-89-ab", sep("-")),
        ("01-23-45-67-89-ab", sep(".")),
        ("01-23-45-67-89-ab", sep(":")),
        ("01-23-45-67-89-AB", int(0)),
        ("01-23-45-67-89-aB", int(0)),
        ("01:23:45:67:89:ab", int(0)),
        ("01:23:45:67:89:AB", int(0)),
        ("01:23:45:67:89:aB", int(0)),
        ("01:23:45-67:89:aB", int(0)),
        ("xx:23:45:67:89:aB", int(0)),
        ("0123.4567.89ab", int(0)),
    ] {
        r.fv(vec![s(v), int(FILTER_VALIDATE_MAC), o]);
    }
    for x in ["--", ""] {
        r.catch(
            "filter_var",
            vec![s("01-23-45-67-89-ab"), int(FILTER_VALIDATE_MAC), sep(x)],
        );
    }
    r.echo("Done\n");
    r.check(phpt!("055.phpt"));
}

/// 056.phpt: FILTER_VALIDATE_DOMAIN.
#[test]
fn validate_domain_056() {
    let src = phpt!("056.phpt");
    let mut r = Run::new();
    for (v, _) in php_literals(src, "$values = Array(") {
        r.fv(vec![
            v,
            int(FILTER_VALIDATE_DOMAIN),
            int(FILTER_FLAG_HOSTNAME),
        ]);
    }
    for v in [
        "_example.com",
        "test_.example.com",
        "te_st.example.com",
        "test._example.com",
    ] {
        r.fv(vec![s(v), int(FILTER_VALIDATE_DOMAIN)]);
        r.fv(vec![
            s(v),
            int(FILTER_VALIDATE_DOMAIN),
            int(FILTER_FLAG_HOSTNAME),
        ]);
    }
    r.echo("Done\n");
    r.check(src);
}

/// gh16523.phpt and edge cases of php_filter_validate_domain_ex().
#[test]
fn validate_domain_edges() {
    let f = Value::Bool(false);
    for (v, fl, ok) in [
        ("a-.bc.com", FILTER_FLAG_HOSTNAME, false),
        ("a.bc-.com", FILTER_FLAG_HOSTNAME, false),
        ("a.bc.com-", FILTER_FLAG_HOSTNAME, false),
        // The trailing dot is ignored, but the hyphen check still sees it.
        ("a.bc.com-.", FILTER_FLAG_HOSTNAME, true),
        ("", 0, true),
        (".", 0, false),
        ("a..", 0, false),
        ("example.com.", FILTER_FLAG_HOSTNAME, true),
    ] {
        let want = if ok { s(v) } else { f.clone() };
        assert_v!(
            filter_var(s(v), FILTER_VALIDATE_DOMAIN, int(fl)),
            want,
            "{v:?}"
        );
    }
    let label = "a".repeat(63);
    assert_v!(
        filter_var(s(&format!("{label}.com")), FILTER_VALIDATE_DOMAIN, int(0)),
        s(&format!("{label}.com"))
    );
    assert_v!(
        filter_var(s(&format!("{label}a.com")), FILTER_VALIDATE_DOMAIN, int(0)),
        f
    );
}

/// filter_null_on_failure.phpt.
#[test]
fn null_on_failure_all_validators() {
    let mut r = Run::new();
    for (v, f) in [
        ("invalid", FILTER_VALIDATE_BOOL),
        ("invalid", FILTER_VALIDATE_BOOL),
        ("invalid", FILTER_VALIDATE_INT),
        ("invalid", FILTER_VALIDATE_FLOAT),
        (".invalid", FILTER_VALIDATE_DOMAIN),
        ("invalid", FILTER_VALIDATE_EMAIL),
        ("invalid", FILTER_VALIDATE_IP),
        ("invalid", FILTER_VALIDATE_MAC),
    ] {
        r.fv(vec![s(v), int(f), int(FILTER_NULL_ON_FAILURE)]);
    }
    r.fv(vec![
        s("invalid"),
        int(FILTER_VALIDATE_REGEXP),
        assoc(vec![
            ("flags", int(FILTER_NULL_ON_FAILURE)),
            ("options", assoc(vec![("regexp", s("/^valid$/"))])),
        ]),
    ]);
    r.fv(vec![
        s("invalid"),
        int(FILTER_VALIDATE_URL),
        int(FILTER_NULL_ON_FAILURE),
    ]);
    r.check(phpt!("filter_null_on_failure.phpt"));
}

// ---------------------------------------------------------------------------
// Sanitizing filters
// ---------------------------------------------------------------------------

/// 021.phpt: FILTER_SANITIZE_NUMBER_*.
#[test]
fn sanitize_number_021() {
    let mut r = Run::new();
    for v in [
        "qwertyu123456dfghj",
        "asd123123.asd123.23",
        "123,23",
        "",
        "0",
        "asd123.2asd",
        "qwertyuiop",
    ] {
        r.fv(vec![s(v), int(FILTER_SANITIZE_NUMBER_INT)]);
    }
    for (v, f) in [
        ("123.4", FILTER_FLAG_ALLOW_FRACTION),
        ("123,4", FILTER_FLAG_ALLOW_FRACTION),
        ("123.4", FILTER_FLAG_ALLOW_THOUSAND),
        ("123,4", FILTER_FLAG_ALLOW_THOUSAND),
        ("123.4e", FILTER_FLAG_ALLOW_SCIENTIFIC),
        ("123,4E", FILTER_FLAG_ALLOW_SCIENTIFIC),
        ("qwe123,4qwe", FILTER_FLAG_ALLOW_FRACTION),
        ("werty65456.34", FILTER_FLAG_ALLOW_FRACTION),
        ("234.56fsfd", FILTER_FLAG_ALLOW_FRACTION),
        ("", FILTER_FLAG_ALLOW_FRACTION),
    ] {
        r.fv(vec![s(v), int(FILTER_SANITIZE_NUMBER_FLOAT), int(f)]);
    }
    r.echo("Done\n");
    r.check(phpt!("021.phpt"));
}

/// 022.phpt: FILTER_SANITIZE_EMAIL.
#[test]
fn sanitize_email_022() {
    let mut r = Run::new();
    for v in [
        "a@b.c",
        "a[!@#$%^&*()@a@#$%^&*(.com@#$%^&*(",
        "white space here \\ \\ \" some more",
        "",
        "123456789000000",
    ] {
        r.fv(vec![s(v), int(FILTER_SANITIZE_EMAIL)]);
    }
    r.echo("Done\n");
    r.check(phpt!("022.phpt"));
}

/// 023.phpt: FILTER_UNSAFE_RAW with FILTER_FLAG_ENCODE_AMP.
#[test]
fn unsafe_raw_023() {
    let mut r = Run::new();
    for v in [
        "}\"<p>test para</p>",
        "a[!@#<b>$%^&*()@a@#$%^&*(.<br>com@#$%^&*(",
        "white space here \\ \\ \" some more",
        "",
        "             123456789000000       <qwertyuiop> ",
    ] {
        r.fv(vec![
            s(v),
            int(FILTER_UNSAFE_RAW),
            int(FILTER_FLAG_ENCODE_AMP),
        ]);
    }
    r.echo("Done\n");
    r.check(phpt!("023.phpt"));
}

/// 043.phpt: FILTER_UNSAFE_RAW encoding every byte.
#[test]
fn unsafe_raw_encoding_043() {
    let mut r = Run::new();
    let f = FILTER_FLAG_ENCODE_AMP | FILTER_FLAG_ENCODE_LOW | FILTER_FLAG_ENCODE_HIGH;
    for c in 0..=255u8 {
        r.fv(vec![bytes(&[c]), int(FILTER_UNSAFE_RAW), flags(f)]);
    }
    r.check(phpt!("043.phpt"));
}

/// bug53037.phpt, bug69202.phpt, bug69203.phpt.
#[test]
fn strip_and_empty_string_flags() {
    assert_v!(filter_var(s(""), FILTER_DEFAULT, int(0)), s(""));
    assert_v!(
        filter_var(s(""), FILTER_DEFAULT, flags(FILTER_FLAG_EMPTY_STRING_NULL)),
        Value::Null
    );
    let v = "``a`b`c``";
    for (f, want) in [
        (FILTER_FLAG_STRIP_BACKTICK, "abc"),
        (FILTER_FLAG_STRIP_LOW | FILTER_FLAG_STRIP_BACKTICK, "abc"),
        (FILTER_FLAG_STRIP_HIGH | FILTER_FLAG_STRIP_BACKTICK, "abc"),
        (FILTER_FLAG_STRIP_LOW | FILTER_FLAG_STRIP_HIGH, v),
    ] {
        assert_v!(filter_var(s(v), FILTER_UNSAFE_RAW, int(f)), s(want));
    }
    for filter in [
        FILTER_SANITIZE_STRING,
        FILTER_UNSAFE_RAW,
        FILTER_SANITIZE_ENCODED,
        FILTER_SANITIZE_SPECIAL_CHARS,
    ] {
        assert_v!(
            filter_var(s("\x7f"), filter, int(FILTER_FLAG_STRIP_HIGH)),
            s(""),
            "{filter}"
        );
    }
}

/// 024.phpt, 027.phpt, filter_sanitize_encoded_0xff.phpt: FILTER_SANITIZE_ENCODED.
#[test]
fn sanitize_encoded() {
    let mut r = Run::new();
    for v in [
        "\"<br>blah</ph>",
        "",
        "  text here  ",
        "!@#$%^&*()QWERTYUIOP{ASDFGHJKL:\"ZXCVBNM<>?",
    ] {
        r.fv(vec![s(v), int(FILTER_SANITIZE_ENCODED)]);
    }
    r.echo("Done\n");
    r.check(phpt!("024.phpt"));

    let mut r = Run::new();
    for f in [0, FILTER_FLAG_ENCODE_LOW, FILTER_FLAG_ENCODE_HIGH] {
        for v in [
            "?><!@#$%^&*()}{~Qwertyuilfdsasdfgmnbvcxcvbn",
            "<data&sons>",
            "",
        ] {
            r.fv(vec![s(v), int(FILTER_SANITIZE_ENCODED), int(f)]);
        }
    }
    r.echo("Done\n");
    r.check(phpt!("027.phpt"));

    let mut r = Run::new();
    r.fv(vec![bytes(b"\xff"), int(FILTER_SANITIZE_ENCODED)]);
    r.fv(vec![bytes(b"\xfe\xff\x00A"), int(FILTER_SANITIZE_ENCODED)]);
    r.check(phpt!("filter_sanitize_encoded_0xff.phpt"));
}

/// 025.phpt: FILTER_SANITIZE_STRING (deprecated constant).
#[test]
fn sanitize_string_025() {
    let mut r = Run::new();
    for (v, f) in [
        ("", None),
        ("<>", None),
        ("<>!@#$%^&*()'\"", Some(FILTER_FLAG_NO_ENCODE_QUOTES)),
        ("<>!@#$%^&*()'\"", Some(FILTER_FLAG_ENCODE_AMP)),
        ("<>`1234567890", None),
        ("`123`", None),
        (".", None),
    ] {
        let filter = r.constant("FILTER_SANITIZE_STRING");
        let mut args = vec![s(v), filter];
        args.extend(f.map(int));
        r.fv(args);
    }
    r.echo("Done\n");
    r.check(phpt!("025.phpt"));
}

/// 026.phpt: FILTER_SANITIZE_STRIPPED (deprecated constant).
#[test]
fn sanitize_stripped_026() {
    let mut r = Run::new();
    for f in [
        None,
        Some(FILTER_FLAG_STRIP_LOW),
        Some(FILTER_FLAG_STRIP_HIGH),
    ] {
        for v in [
            "<p>Let me <font color=\"#000000\">see</font> you <br /><b>Stripped</b> down to the bone</p>",
            "!@#$%^&*()><<>+_\"'<br><p /><li />",
            "",
        ] {
            let filter = r.constant("FILTER_SANITIZE_STRIPPED");
            let mut args = vec![s(v), filter];
            args.extend(f.map(int));
            r.fv(args);
        }
    }
    r.echo("Done\n");
    r.check(phpt!("026.phpt"));
}

/// 042.phpt: strip tags with whitespace after `<`.
#[test]
fn sanitize_string_strip_low_042() {
    let mut r = Run::new();
    for v in [
        "XYZ< script>alert(/ext/filter+bypass/);< /script>ABC",
        "XYZ<\nscript>alert(/ext/filter+bypass/);<\n/script>ABC",
    ] {
        let filter = r.constant("FILTER_SANITIZE_STRING");
        let v = r.value(
            "filter_var",
            vec![s(v), filter, flags(FILTER_FLAG_STRIP_LOW)],
        );
        r.echo(&format!("{}\n", v.as_str().unwrap().to_string_lossy()));
    }
    r.check(phpt!("042.phpt"));
}

#[test]
fn sanitize_string_tags_and_comments() {
    let f = |v: &str| filter_var(s(v), FILTER_SANITIZE_STRING, int(0));
    assert_v!(f("a<!-- c -->b"), s("ab"));
    assert_v!(f("a<?php echo '>'; ?>b"), s("ab"));
    assert_v!(f("a<!DOCTYPE html>b"), s("ab"));
    assert_v!(f("a\0b"), s("ab"));
    assert_v!(f("<b>"), s(""));
    assert_v!(
        filter_var(
            s("<b>"),
            FILTER_SANITIZE_STRING,
            int(FILTER_FLAG_EMPTY_STRING_NULL)
        ),
        Value::Null
    );
}

/// 028.phpt: FILTER_SANITIZE_SPECIAL_CHARS.
#[test]
fn sanitize_special_chars_028() {
    let mut r = Run::new();
    for f in [0, FILTER_FLAG_ENCODE_LOW, FILTER_FLAG_ENCODE_HIGH] {
        for v in [
            "?><!@#$%^&*()}{~Qwertyuilfdsasdfgmnbvcxcvbn",
            "<data&sons>",
            "",
        ] {
            r.fv(vec![s(v), int(FILTER_SANITIZE_SPECIAL_CHARS), int(f)]);
        }
    }
    for f in [FILTER_FLAG_ENCODE_HIGH, FILTER_FLAG_ENCODE_LOW] {
        r.fv(vec![
            s("кириллица"),
            int(FILTER_SANITIZE_SPECIAL_CHARS),
            int(f),
        ]);
    }
    r.echo("Done\n");
    r.check(phpt!("028.phpt"));
}

/// 059.phpt: FILTER_SANITIZE_ADD_SLASHES.
#[test]
fn sanitize_add_slashes_059() {
    let mut r = Run::new();
    for v in [s("test'asd'asd'' asd\\'\"asdfasdf"), s("'"), s(""), int(-1)] {
        r.fv(vec![v, int(FILTER_SANITIZE_ADD_SLASHES)]);
    }
    r.echo("Done\n");
    r.check(phpt!("059.phpt"));
}

#[test]
fn sanitize_full_special_chars() {
    let f = |v: &str, fl: i64| filter_var(s(v), FILTER_SANITIZE_FULL_SPECIAL_CHARS, int(fl));
    assert_v!(
        f("<a href='x'>é&amp;</a>", 0),
        s("&lt;a href=&#039;x&#039;&gt;&eacute;&amp;&lt;/a&gt;")
    );
    assert_v!(f("\"'", FILTER_FLAG_NO_ENCODE_QUOTES), s("\"'"));
    assert_v!(f("", 0), s(""));
}

#[test]
fn sanitize_url() {
    assert_v!(
        filter_var(
            s("http://exa mple.com/ä?q=<1>"),
            FILTER_SANITIZE_URL,
            int(0)
        ),
        s("http://example.com/?q=<1>")
    );
}

/// 033.phpt: every filter of filter_list() on the same data.
#[test]
fn all_filters_033() {
    let data = [
        "PHP",
        "1",
        "foo@bar.com",
        "http://a.b.c",
        "1.2.3.4",
        "123",
        "123abc<>()",
        "O'Henry",
        "하퍼",
        "aa:bb:cc:dd:ee:ff",
    ];
    let widths = [5, 3, 15, 20, 10, 5, 20, 15, 10, 10];
    let pad = |out: &mut Vec<u8>, text: &[u8], width: usize| {
        out.extend_from_slice(text);
        out.resize(out.len() + width.saturating_sub(text.len()), b' ');
    };
    let mut r = Run::new();
    let names = r.value("filter_list", vec![]);
    let mut out = Vec::new();
    for (_, name) in names.as_array().unwrap().iter() {
        let id = r.value("filter_id", vec![name.clone()]);
        let opts = if name.as_str().is_some_and(|n| n == "validate_regexp") {
            options(assoc(vec![("regexp", s("/^O.*/"))]))
        } else {
            options(s("strtoupper"))
        };
        pad(&mut out, name.as_str().unwrap(), 20);
        for (d, w) in data.iter().zip(widths) {
            let v = r.value("filter_var", vec![s(d), id.clone(), opts.clone()]);
            let text = r.t.vm.to_php_string(&v).unwrap();
            pad(&mut out, &text, w);
        }
        out.push(b'\n');
    }
    assert_eq!(r.out, "");
    assert_eq!(
        String::from_utf8(out).unwrap().trim_end(),
        expected(phpt!("033.phpt")).trim_end()
    );
}

// ---------------------------------------------------------------------------
// FILTER_CALLBACK
// ---------------------------------------------------------------------------

/// 029.phpt (with native callbacks), callback_non_modified_var.phpt.
#[test]
fn callback_029() {
    let mut r = Run::new();
    let odd = "~!@#$%^&*()_QWERTYUIOPASDFGHJKLZXCVBNM<>>?\"}{:";
    for v in ["data", odd, ""] {
        r.fv(vec![s(v), int(FILTER_CALLBACK), options(s("strtoupper"))]);
    }
    for o in [options(s("no such func")), options(s(""))] {
        r.catch("filter_var", vec![s("qwe"), int(FILTER_CALLBACK), o]);
    }
    r.catch("filter_var", vec![s("qwe"), int(FILTER_CALLBACK)]);
    for v in ["dAtA", odd, ""] {
        r.fv(vec![s(v), int(FILTER_CALLBACK), options(s("strtolower"))]);
    }
    r.fv(vec![s("test"), int(FILTER_CALLBACK), options(s("strval"))]);
    assert_eq!(
        r.out,
        "string(4) \"DATA\"\n\
         string(46) \"~!@#$%^&*()_QWERTYUIOPASDFGHJKLZXCVBNM<>>?\"}{:\"\n\
         string(0) \"\"\n\
         TypeError: filter_var(): Option must be a valid callback\n\
         TypeError: filter_var(): Option must be a valid callback\n\
         TypeError: filter_var(): Option must be a valid callback\n\
         string(4) \"data\"\n\
         string(46) \"~!@#$%^&*()_qwertyuiopasdfghjklzxcvbnm<>>?\"}{:\"\n\
         string(0) \"\"\n\
         string(4) \"test\"\n"
    );

    // An exception thrown by the callback propagates.
    let e = thrown(r.call(
        "filter_var",
        vec![s("data"), int(FILTER_CALLBACK), options(s("intdiv"))],
    ));
    assert_eq!(e.0, "ArgumentCountError");
}

/// 052.phpt (callbacks on arrays), filter_callback_require_scalar.phpt.
#[test]
fn callback_arrays() {
    let mut r = Run::new();
    let data = assoc(vec![(
        "bar",
        list(vec![s("fu<script>bar"), s("bar<script>fu")]),
    )]);
    r.fv(vec![
        data.clone(),
        int(FILTER_SANITIZE_SPECIAL_CHARS),
        int(FILTER_FORCE_ARRAY),
    ]);
    r.dump(&data);
    // 052.phpt's callback returns 1; str_word_count() gives 3 for both.
    let callback = s("str_word_count");
    r.fv(vec![
        data.clone(),
        int(FILTER_CALLBACK),
        options(callback.clone()),
    ]);
    r.dump(&data);
    r.var_dump(
        "filter_var_array",
        vec![
            data.clone(),
            assoc(vec![(
                "bar",
                assoc(vec![
                    ("filter", int(FILTER_CALLBACK)),
                    ("options", callback),
                ]),
            )]),
        ],
    );
    r.dump(&data);
    let expect = expected(phpt!("052.phpt")).replace("int(1)", "int(3)");
    assert_eq!(r.out.trim_end(), expect.trim_end());

    let mut r = Run::new();
    let opts = assoc(vec![
        ("options", s("strval")),
        ("flags", int(FILTER_REQUIRE_SCALAR)),
    ]);
    r.fv(vec![s("test"), int(FILTER_CALLBACK), opts.clone()]);
    r.fv(vec![list(vec![s("test")]), int(FILTER_CALLBACK), opts]);
    r.check(phpt!("filter_callback_require_scalar.phpt"));
}

// ---------------------------------------------------------------------------
// Arrays, flags and defaults
// ---------------------------------------------------------------------------

/// 032.phpt: filter_var_array().
#[test]
fn filter_var_array_032() {
    let mut r = Run::new();
    let data = assoc(vec![
        ("product_id", s("libgd<script>")),
        ("component", s("10dhsajkkdhk <do>")),
        ("versions", s("2.0.33")),
        ("testscalar", list(vec![s("2"), s("23"), s("10"), s("12")])),
        ("testarray", s("2")),
    ]);
    let args = assoc(vec![
        ("product_id", int(FILTER_SANITIZE_ENCODED)),
        (
            "component",
            assoc(vec![
                ("flags", int(FILTER_FORCE_ARRAY)),
                (
                    "options",
                    assoc(vec![("min_range", int(1)), ("max_range", int(10))]),
                ),
            ]),
        ),
        (
            "versions",
            assoc(vec![
                ("filter", int(FILTER_SANITIZE_ENCODED)),
                ("flags", int(FILTER_REQUIRE_SCALAR)),
            ]),
        ),
        ("doesnotexist", int(FILTER_VALIDATE_INT)),
        ("testscalar", int(FILTER_VALIDATE_INT)),
        (
            "testarray",
            assoc(vec![
                ("filter", int(FILTER_VALIDATE_INT)),
                ("flags", int(FILTER_FORCE_ARRAY)),
            ]),
        ),
    ]);
    r.var_dump("filter_var_array", vec![data, args]);
    r.check(phpt!("032.phpt"));
}

/// 036.phpt: filter_var_array() and references.
#[test]
fn filter_var_array_references_036() {
    let mut r = Run::new();
    let var = PhpRef::new(s("1"));
    let mut data = PhpArray::new();
    data.insert(ArrayKey::from("test1"), int(1));
    data.insert(ArrayKey::from("test2"), Value::Ref(var.clone()));
    let data = Value::array(data);
    let args = assoc(vec![
        ("test1", int(FILTER_VALIDATE_INT)),
        ("test2", int(FILTER_VALIDATE_INT)),
    ]);
    r.var_dump("filter_var_array", vec![data.clone(), args]);
    r.dump(&data);
    r.dump(&var.get());
    r.echo("Done\n");
    r.check(phpt!("036.phpt"));
}

/// filter_var_array_with_ref.phpt: references inside arrays are filtered
/// through the reference.
#[test]
fn filter_var_array_with_ref() {
    let mut r = Run::new();
    let array = PhpRef::new(list(vec![s("123foo")]));
    let array2 = list(vec![Value::Ref(array.clone())]);
    r.var_dump("filter_var_array", vec![array2, int(FILTER_VALIDATE_INT)]);
    r.dump(&array.get());
    r.check(phpt!("filter_var_array_with_ref.phpt"));
}

/// A self-referencing array is walked once (`Z_PROTECT_RECURSION`).
#[test]
fn recursive_array() {
    let mut t = TestVm::new();
    let r = PhpRef::new(Value::Null);
    r.set(list(vec![s("7"), Value::Ref(r.clone())]));
    let v = t
        .call(
            "filter_var",
            vec![r.get(), int(FILTER_VALIDATE_INT), int(FILTER_REQUIRE_ARRAY)],
        )
        .unwrap();
    assert_v!(v.as_array().unwrap().get_int(0).unwrap(), &int(7));
    assert_v!(r.get().as_array().unwrap().get_int(0).unwrap(), &int(7));
}

/// 038.phpt: scalars vs arrays.
#[test]
fn scalar_and_array_flags_038() {
    let mut r = Run::new();
    let o = FILTER_FLAG_ALLOW_OCTAL;
    for (v, f) in [
        (int(12), o),
        (list(vec![int(12)]), o),
        (int(12), o | FILTER_FORCE_ARRAY),
        (int(12), o | FILTER_REQUIRE_ARRAY),
        (list(vec![int(12)]), o | FILTER_REQUIRE_ARRAY),
        (
            list(vec![int(12)]),
            o | FILTER_FORCE_ARRAY | FILTER_REQUIRE_ARRAY,
        ),
        (list(vec![int(12)]), o | FILTER_FORCE_ARRAY),
    ] {
        r.fv(vec![v, int(FILTER_VALIDATE_INT), flags(f)]);
    }
    r.check(phpt!("038.phpt"));
}

/// 039.phpt: filter_var_array() and different arguments.
#[test]
fn filter_var_array_arguments_039() {
    let mut r = Run::new();
    let fva = "filter_var_array";
    let empty = || list(vec![]);
    r.echo("-- (1)\n");
    r.var_dump(fva, vec![empty()]);
    r.var_dump(fva, vec![assoc(vec![("0", int(1)), ("blah", s("hoho"))])]);
    r.var_dump(fva, vec![empty(), int(-1)]);
    r.var_dump(fva, vec![empty(), int(1000000)]);
    r.catch(fva, vec![empty(), s("")]);
    for (n, input) in [
        ("2", assoc(vec![("", s(""))])),
        ("3", assoc(vec![("aaa", s("bbb"))])),
    ] {
        r.echo(&format!("-- ({n})\n"));
        r.var_dump(fva, vec![input.clone(), int(-1)]);
        r.var_dump(fva, vec![input.clone(), int(1000000)]);
        r.catch(fva, vec![input, s("")]);
    }
    r.echo("-- (4)\n");
    let obj = stdclass(&r.t);
    r.catch(fva, vec![empty(), obj]);
    r.var_dump(fva, vec![empty(), empty()]);
    r.var_dump(fva, vec![empty(), assoc(vec![("var_name", int(1))])]);
    r.var_dump(fva, vec![empty(), assoc(vec![("var_name", int(-1))])]);
    r.var_dump(
        fva,
        vec![
            assoc(vec![("var_name", s(""))]),
            assoc(vec![("var_name", int(-1))]),
        ],
    );
    r.echo("-- (5)\n");
    let spec = assoc(vec![
        ("var_name", int(-1)),
        ("asdas", s("asdasd")),
        ("qwe", s("rty")),
        ("", s("")),
    ]);
    r.catch(fva, vec![assoc(vec![("var_name", s(""))]), spec.clone()]);
    r.catch(fva, vec![assoc(vec![("asdas", s("text"))]), spec]);
    let a = assoc(vec![("", s(""))]);
    for b in [int(-1), int(100000)] {
        r.var_dump(fva, vec![a.clone(), b.clone()]);
        r.dump(&a);
        r.dump(&b);
    }
    r.catch(fva, vec![a.clone(), s("")]);
    r.dump(&a);
    r.dump(&s(""));
    r.echo("Done\n");
    r.check(phpt!("039.phpt"));

    // Integer keys in the definition are rejected.
    let mut r = Run::new();
    let e = thrown(r.call(fva, vec![empty(), list(vec![int(FILTER_VALIDATE_INT)])]));
    assert_eq!(
        e,
        (
            "TypeError".into(),
            "filter_var_array(): Argument #2 ($options) must contain only string keys".into()
        )
    );
}

/// 045.phpt: options are not modified; string flags and filters convert.
#[test]
fn options_not_modified_045() {
    let mut t = TestVm::new();
    let a = assoc(vec![
        ("flags", s(&FILTER_FLAG_ALLOW_HEX.to_string())),
        (
            "options",
            assoc(vec![("min_range", s("0")), ("max_range", s("1024"))]),
        ),
    ]);
    let before = dump(&a);
    let ret = t
        .call(
            "filter_var",
            vec![s("0xff"), int(FILTER_VALIDATE_INT), a.clone()],
        )
        .unwrap();
    assert_v!(ret, int(255));
    assert_eq!(dump(&a), before);

    let ret = t
        .call(
            "filter_var",
            vec![s("0xff"), int(FILTER_VALIDATE_INT), s("2")],
        )
        .unwrap();
    assert_v!(ret, int(255));

    let a = assoc(vec![(
        "test",
        assoc(vec![
            ("filter", s(&FILTER_VALIDATE_INT.to_string())),
            ("flags", s(&FILTER_FLAG_ALLOW_HEX.to_string())),
        ]),
    )]);
    let ret = t
        .call(
            "filter_var_array",
            vec![assoc(vec![("test", s("0xff"))]), a],
        )
        .unwrap();
    assert_eq!(dump(&ret), "array(1) {\n  [\"test\"]=>\n  int(255)\n}\n");

    let a = assoc(vec![("test", s(&FILTER_VALIDATE_INT.to_string()))]);
    let ret = t
        .call("filter_var_array", vec![assoc(vec![("test", s("255"))]), a])
        .unwrap();
    assert_eq!(dump(&ret), "array(1) {\n  [\"test\"]=>\n  int(255)\n}\n");
    assert_eq!(t.output(), "");
}

/// 051.phpt, 054.phpt, bug73054.phpt, bug67167.01.phpt, bug49274.phpt,
/// bug76366.phpt, gh16993.phpt.
#[test]
fn defaults_and_definitions() {
    let def = |d: Value| options(assoc(vec![("default", d)]));
    assert_v!(
        filter_var(s("123asd"), FILTER_VALIDATE_INT, def(int(321))),
        int(321)
    );

    let mut r = Run::new();
    let foo = assoc(vec![("foo", int(123))]);
    let spec = assoc(vec![
        ("foo", assoc(vec![("filter", int(FILTER_DEFAULT))])),
        ("bar", assoc(vec![("filter", int(FILTER_DEFAULT))])),
    ]);
    r.var_dump(
        "filter_var_array",
        vec![foo.clone(), spec.clone(), Value::Bool(false)],
    );
    r.var_dump("filter_var_array", vec![foo, spec]);
    r.check(phpt!("054.phpt"));

    // bug73054: objects without __toString() honour "default".
    let mut r = Run::new();
    let obj = stdclass(&r.t);
    let v = r.value(
        "filter_var",
        vec![obj.clone(), int(FILTER_VALIDATE_INT), def(int(2))],
    );
    assert_v!(v, int(2));
    let v = r.value(
        "filter_var",
        vec![
            obj.clone(),
            int(FILTER_VALIDATE_INT),
            assoc(vec![
                ("options", assoc(vec![("default", int(2))])),
                ("flags", int(FILTER_NULL_ON_FAILURE)),
            ]),
        ],
    );
    assert_v!(v, int(2));
    // bug67167.01 and bug49274
    let v = r.value(
        "filter_var",
        vec![
            obj.clone(),
            int(FILTER_VALIDATE_BOOL),
            int(FILTER_NULL_ON_FAILURE),
        ],
    );
    assert_v!(v, Value::Null);
    let v = r.value("filter_var", vec![obj, int(FILTER_VALIDATE_EMAIL)]);
    assert_v!(v, Value::Bool(false));
    // The default also replaces a valid `false` (php_zval_filter()).
    let v = r.value(
        "filter_var",
        vec![s("off"), int(FILTER_VALIDATE_BOOL), def(Value::Bool(true))],
    );
    assert_v!(v, Value::Bool(true));

    // bug76366: the "options" entry may be a reference.
    let opts = PhpRef::new(assoc(vec![("min_range", int(1)), ("max_range", int(5))]));
    let mut foo_spec = PhpArray::new();
    foo_spec.insert(ArrayKey::from("filter"), int(FILTER_VALIDATE_INT));
    foo_spec.insert(ArrayKey::from("flags"), int(FILTER_FORCE_ARRAY));
    foo_spec.insert(ArrayKey::from("options"), Value::Ref(opts));
    r.var_dump(
        "filter_var_array",
        vec![
            assoc(vec![("foo", int(6))]),
            assoc(vec![("foo", Value::array(foo_spec))]),
        ],
    );
    r.check(phpt!("bug76366.phpt"));

    // gh16993: unknown filter IDs in the definition warn.
    let mut r = Run::new();
    let data = assoc(vec![("test", s("42"))]);
    r.var_dump(
        "filter_var_array",
        vec![
            data.clone(),
            assoc(vec![(
                "test",
                int(FILTER_VALIDATE_INT | FILTER_NULL_ON_FAILURE),
            )]),
        ],
    );
    r.var_dump(
        "filter_var_array",
        vec![
            data,
            assoc(vec![(
                "test",
                assoc(vec![
                    ("filter", int(FILTER_VALIDATE_INT)),
                    ("flags", int(FILTER_NULL_ON_FAILURE)),
                ]),
            )]),
        ],
    );
    r.check(phpt!("gh16993.phpt"));
}

/// Objects with __toString() and the conversion of other scalars.
#[test]
fn objects_and_scalars() {
    let mut t = TestVm::new();
    let o = stringable(&mut t, "<b>x</b>");
    let v = t
        .call("filter_var", vec![o, int(FILTER_SANITIZE_SPECIAL_CHARS)])
        .unwrap();
    assert_v!(v, s("&#60;b&#62;x&#60;/b&#62;"));
    // Floats are converted with `precision`.
    let v = t.call("filter_var", vec![Value::Float(0.1 + 0.2)]).unwrap();
    assert_v!(v, s("0.3"));
    assert_v!(
        t.call("filter_var", vec![Value::Bool(true)]).unwrap(),
        s("1")
    );
    assert_v!(t.call("filter_var", vec![Value::Null]).unwrap(), s(""));
    assert_eq!(t.output(), "");
}

// ---------------------------------------------------------------------------
// Input sources
// ---------------------------------------------------------------------------

/// 012.phpt, bug50632.phpt, filter_input_array_001.phpt: the CLI has no
/// request variables.
#[test]
fn filter_input_without_input() {
    let mut r = Run::new();
    for t in [PARSE_GET, PARSE_POST] {
        r.var_dump("filter_input", vec![int(t), s("test")]);
    }
    r.var_dump("filter_input", vec![int(PARSE_COOKIE), s("")]);
    for (t, name) in [
        (PARSE_GET, "test"),
        (PARSE_POST, "test"),
        (PARSE_COOKIE, ""),
    ] {
        r.var_dump(
            "filter_input",
            vec![
                int(t),
                s(name),
                int(FILTER_DEFAULT),
                int(FILTER_NULL_ON_FAILURE),
            ],
        );
    }
    r.echo("Done\n");
    r.check(phpt!("012.phpt"));

    let mut r = Run::new();
    let v = r.value(
        "filter_input",
        vec![
            int(PARSE_GET),
            s("foo"),
            int(FILTER_VALIDATE_INT),
            assoc(vec![
                ("flags", int(FILTER_REQUIRE_SCALAR)),
                ("options", assoc(vec![("default", int(23))])),
            ]),
        ],
    );
    assert_v!(v, int(23));

    let v = r.value(
        "filter_input_array",
        vec![
            int(PARSE_GET),
            assoc(vec![(
                "c",
                assoc(vec![("flags", int(FILTER_NULL_ON_FAILURE))]),
            )]),
            Value::Bool(true),
        ],
    );
    assert_v!(v, Value::Null);
    assert_v!(
        r.value("filter_has_var", vec![int(PARSE_SERVER), s("PHP_SELF")]),
        Value::Bool(false)
    );
    assert_v!(
        r.value("filter_input_array", vec![int(PARSE_ENV)]),
        Value::Null
    );

    let e = thrown(r.call("filter_input", vec![int(3), s("x")]));
    assert_eq!(
        e.1,
        "filter_input(): Argument #1 ($type) must be an INPUT_* constant"
    );
    let v = r.value("filter_input", vec![int(PARSE_GET), s("x"), int(9999)]);
    assert_v!(v, Value::Bool(false));
    assert_eq!(
        r.out,
        "\nWarning: filter_input(): Unknown filter with ID 9999 in %s on line %d\n"
    );
}

/// 006.phpt, 011.phpt: filter_input() with GET/POST variables.
#[test]
fn filter_input_011() {
    let mut r = Run::new();
    r.input(PARSE_POST, vec![("foo", s("<b>abc</b>"))]);
    let v = r.value(
        "filter_input",
        vec![
            int(PARSE_POST),
            s("foo"),
            int(FILTER_SANITIZE_SPECIAL_CHARS),
        ],
    );
    assert_v!(v, s("&#60;b&#62;abc&#60;/b&#62;"));

    let mut r = Run::new();
    r.input(
        PARSE_GET,
        vec![("a", s("<b>test</b>")), ("b", s("http://example.com"))],
    );
    r.input(
        PARSE_POST,
        vec![("c", s("<p>string</p>")), ("d", s("12345.7"))],
    );
    r.var_dump(
        "filter_input",
        vec![int(PARSE_GET), s("b"), int(FILTER_SANITIZE_URL)],
    );
    r.var_dump(
        "filter_input",
        vec![
            int(PARSE_GET),
            s("a"),
            int(FILTER_SANITIZE_SPECIAL_CHARS),
            list(vec![int(1), int(2), int(3), int(4), int(5)]),
        ],
    );
    let obj = stdclass(&r.t);
    r.catch(
        "filter_input",
        vec![int(PARSE_GET), s("b"), int(FILTER_VALIDATE_FLOAT), obj],
    );
    r.var_dump(
        "filter_input",
        vec![int(PARSE_POST), s("d"), int(FILTER_VALIDATE_FLOAT)],
    );
    r.var_dump(
        "filter_input",
        vec![int(PARSE_POST), s("c"), int(FILTER_SANITIZE_SPECIAL_CHARS)],
    );
    r.var_dump(
        "filter_input",
        vec![int(PARSE_POST), s("d"), int(FILTER_VALIDATE_INT)],
    );
    r.echo("Done\n");
    r.check(phpt!("011.phpt"));
}

/// 007.phpt, 040.phpt: filter_has_var().
#[test]
fn filter_has_var_040() {
    let mut r = Run::new();
    r.input(PARSE_GET, vec![("a", s("1")), ("b", s("2")), ("c", s("0"))]);
    r.input(
        PARSE_POST,
        vec![("ap", list(vec![s("1")])), ("bp", s("test")), ("cp", s(""))],
    );
    r.var_dump("filter_has_var", vec![int(PARSE_GET), s("")]);
    r.var_dump("filter_has_var", vec![int(PARSE_POST), s("ap")]);
    r.var_dump("filter_has_var", vec![int(PARSE_POST), s("cp")]);
    r.var_dump("filter_has_var", vec![int(PARSE_GET), s("a")]);
    r.var_dump("filter_has_var", vec![int(PARSE_GET), s("c")]);
    r.var_dump("filter_has_var", vec![int(PARSE_GET), s("abc")]);
    r.var_dump("filter_has_var", vec![int(PARSE_GET), s("cc")]);
    r.catch("filter_has_var", vec![int(-1), s("cc")]);
    r.var_dump("filter_has_var", vec![int(0), s("cc")]);
    r.echo("Done\n");
    r.check(phpt!("040.phpt"));

    let mut r = Run::new();
    r.input(PARSE_GET, vec![("a", s("qwe")), ("abc", s("<a>href</a>"))]);
    for (name, want) in [
        ("a", true),
        ("abc", true),
        ("nonex", false),
        (" ", false),
        ("", false),
    ] {
        assert_v!(
            r.value("filter_has_var", vec![int(PARSE_GET), s(name)]),
            Value::Bool(want)
        );
    }
}

/// 035.phpt: filter_input() with array variables.
#[test]
fn filter_input_arrays_035() {
    let mut r = Run::new();
    r.input(
        PARSE_GET,
        vec![
            ("ar", assoc(vec![("elm1", s("1234")), ("elm2", s("0660"))])),
            ("a", s("0234")),
        ],
    );
    r.input(PARSE_POST, vec![("d", s("379"))]);
    let fi = |name: &str, opts: Option<Value>| {
        let mut args = vec![int(PARSE_GET), s(name), int(FILTER_VALIDATE_INT)];
        args.extend(opts);
        args
    };
    r.var_dump("filter_input", fi("a", None));
    r.var_dump(
        "filter_input",
        fi("a", Some(flags(FILTER_FLAG_ALLOW_OCTAL))),
    );
    r.var_dump("filter_input", fi("ar", Some(flags(FILTER_REQUIRE_ARRAY))));
    r.var_dump(
        "filter_input",
        fi(
            "ar",
            Some(flags(FILTER_FLAG_ALLOW_OCTAL | FILTER_REQUIRE_ARRAY)),
        ),
    );
    r.check(phpt!("035.phpt"));
}

/// 037.phpt (with a native callback), 041.phpt (cookies), bug71063.phpt.
#[test]
fn filter_input_array_037() {
    let mut r = Run::new();
    r.input(PARSE_GET, vec![("a", s("ab")), ("b", s("2"))]);
    let v = r.value(
        "filter_input",
        vec![
            int(PARSE_GET),
            s("a"),
            int(FILTER_CALLBACK),
            options(s("strrev")),
        ],
    );
    assert_v!(v, s("ba"));
    r.var_dump(
        "filter_input_array",
        vec![
            int(PARSE_GET),
            assoc(vec![
                (
                    "a",
                    assoc(vec![
                        ("filter", int(FILTER_CALLBACK)),
                        ("options", s("strrev")),
                    ]),
                ),
                ("b", int(FILTER_VALIDATE_INT)),
            ]),
        ],
    );
    r.var_dump("filter_input_array", vec![int(PARSE_GET)]);
    assert_eq!(
        r.out,
        "array(2) {\n  [\"a\"]=>\n  string(2) \"ba\"\n  [\"b\"]=>\n  int(2)\n}\n\
         array(2) {\n  [\"a\"]=>\n  string(2) \"ab\"\n  [\"b\"]=>\n  string(1) \"2\"\n}\n"
    );

    // 041.phpt: the raw cookie values (the first of duplicate names wins).
    let mut r = Run::new();
    r.input(
        PARSE_COOKIE,
        vec![
            ("abc", s("dir")),
            ("def", s("true")),
            ("xyz", s("\"foo bar\"")),
        ],
    );
    r.var_dump("filter_has_var", vec![int(PARSE_COOKIE), s("abc")]);
    for name in ["abc", "def", "xyz"] {
        r.var_dump("filter_input", vec![int(PARSE_COOKIE), s(name)]);
    }
    r.var_dump("filter_has_var", vec![int(PARSE_COOKIE), s("bogus")]);
    r.var_dump(
        "filter_input",
        vec![
            int(PARSE_COOKIE),
            s("xyz"),
            int(FILTER_SANITIZE_SPECIAL_CHARS),
        ],
    );
    let expect = expected(phpt!("041.phpt"));
    let expect = &expect[expect.find("bool(true)").unwrap()..];
    assert_eq!(r.out.trim_end(), expect.trim_end());

    // bug71063.phpt: INPUT_ENV falls back to $_ENV.
    let mut r = Run::new();
    set_input_array(
        &mut r.t.vm,
        PARSE_STRING,
        Some(PhpArray::from_pairs(vec![("PATH", s("/"))])),
    );
    assert_v!(
        r.value("filter_input", vec![int(PARSE_ENV), s("PATH")]),
        s("/")
    );
}

// ---------------------------------------------------------------------------
// FILTER_THROW_ON_FAILURE
// ---------------------------------------------------------------------------

/// throw-on-failure/filter_var_failure.phpt.
#[test]
fn throw_filter_var_failure() {
    let mut r = Run::new();
    r.echo("\nvalidation fails (array type check)\n");
    r.catch(
        "filter_var",
        vec![
            s("a"),
            int(FILTER_DEFAULT),
            int(FILTER_REQUIRE_ARRAY | FILTER_THROW_ON_FAILURE),
        ],
    );
    r.echo("\nvalidation fails (object without __toString)\n");
    let obj = stdclass(&r.t);
    r.catch(
        "filter_var",
        vec![
            obj,
            int(FILTER_VALIDATE_EMAIL),
            int(FILTER_THROW_ON_FAILURE),
        ],
    );
    r.echo("\nvalidation fails (filter value)\n");
    r.catch(
        "filter_var",
        vec![
            s("a"),
            int(FILTER_VALIDATE_EMAIL),
            int(FILTER_THROW_ON_FAILURE),
        ],
    );
    r.check(phpt!("throw-on-failure/filter_var_failure.phpt"));
}

/// throw-on-failure/filter_var_array_failure.phpt.
#[test]
fn throw_filter_var_array_failure() {
    let mut r = Run::new();
    let spec = |f: Vec<(&str, Value)>| assoc(vec![("a", assoc(f))]);
    r.echo("\nvalidation fails (array type check)\n");
    r.catch(
        "filter_var_array",
        vec![
            assoc(vec![("a", s("a"))]),
            spec(vec![(
                "flags",
                int(FILTER_REQUIRE_ARRAY | FILTER_THROW_ON_FAILURE),
            )]),
        ],
    );
    r.echo("\nvalidation fails (object without __toString)\n");
    let obj = stdclass(&r.t);
    r.catch(
        "filter_var_array",
        vec![
            assoc(vec![("a", obj)]),
            spec(vec![("flags", int(FILTER_THROW_ON_FAILURE))]),
        ],
    );
    r.echo("\nvalidation fails (filter value)\n");
    r.catch(
        "filter_var_array",
        vec![
            assoc(vec![("a", Value::Bool(true))]),
            spec(vec![
                ("filter", int(FILTER_VALIDATE_EMAIL)),
                ("flags", int(FILTER_THROW_ON_FAILURE)),
            ]),
        ],
    );
    r.check(phpt!("throw-on-failure/filter_var_array_failure.phpt"));
}

/// throw-on-failure/filter_input_failure.phpt and
/// filter_input_array_failure.phpt.
#[test]
fn throw_filter_input_failure() {
    let mut r = Run::new();
    r.input(PARSE_GET, vec![("a", s("1"))]);
    r.echo("missing value\n");
    r.catch(
        "filter_input",
        vec![
            int(PARSE_GET),
            s("b"),
            int(FILTER_DEFAULT),
            int(FILTER_THROW_ON_FAILURE),
        ],
    );
    r.echo("\nvalidation fails (array type check)\n");
    r.catch(
        "filter_input",
        vec![
            int(PARSE_GET),
            s("a"),
            int(FILTER_DEFAULT),
            int(FILTER_REQUIRE_ARRAY | FILTER_THROW_ON_FAILURE),
        ],
    );
    r.echo("\nvalidation fails (filter value)\n");
    r.catch(
        "filter_input",
        vec![
            int(PARSE_GET),
            s("a"),
            int(FILTER_VALIDATE_EMAIL),
            int(FILTER_THROW_ON_FAILURE),
        ],
    );
    r.check(phpt!("throw-on-failure/filter_input_failure.phpt"));

    let mut r = Run::new();
    r.input(PARSE_GET, vec![("a", s("1"))]);
    let spec = |f: Vec<(&str, Value)>| assoc(vec![("a", assoc(f))]);
    r.echo("\nvalidation fails (array type check)\n");
    r.catch(
        "filter_input_array",
        vec![
            int(PARSE_GET),
            spec(vec![(
                "flags",
                int(FILTER_REQUIRE_ARRAY | FILTER_THROW_ON_FAILURE),
            )]),
        ],
    );
    r.echo("\nvalidation fails (filter value)\n");
    r.catch(
        "filter_input_array",
        vec![
            int(PARSE_GET),
            spec(vec![
                ("filter", int(FILTER_VALIDATE_EMAIL)),
                ("flags", int(FILTER_THROW_ON_FAILURE)),
            ]),
        ],
    );
    r.check(phpt!("throw-on-failure/filter_input_array_failure.phpt"));
}

/// throw-on-failure/filter_success.phpt.
#[test]
fn throw_filter_success() {
    let mut r = Run::new();
    let email = "daniel.e.scherzer@gmail.com";
    r.input(PARSE_GET, vec![("a", s(email))]);
    let throw = FILTER_THROW_ON_FAILURE;
    r.echo("filter_input:\n");
    for (filter, f) in [
        (FILTER_DEFAULT, throw),
        (FILTER_DEFAULT, FILTER_REQUIRE_SCALAR | throw),
        (FILTER_VALIDATE_EMAIL, throw),
    ] {
        r.var_dump(
            "filter_input",
            vec![int(PARSE_GET), s("a"), int(filter), int(f)],
        );
    }
    r.echo("\nfilter_input_array:\n");
    r.var_dump(
        "filter_input_array",
        vec![
            int(PARSE_GET),
            assoc(vec![(
                "a",
                assoc(vec![("flags", int(FILTER_REQUIRE_SCALAR | throw))]),
            )]),
        ],
    );
    r.var_dump(
        "filter_input_array",
        vec![
            int(PARSE_GET),
            assoc(vec![(
                "a",
                assoc(vec![
                    ("filter", int(FILTER_VALIDATE_EMAIL)),
                    ("flags", int(throw)),
                ]),
            )]),
        ],
    );
    r.echo("\nfilter_var:\n");
    r.fv(vec![
        s("a"),
        int(FILTER_DEFAULT),
        int(FILTER_REQUIRE_SCALAR | throw),
    ]);
    let obj = stringable(&mut r.t, email);
    r.fv(vec![obj, int(FILTER_VALIDATE_EMAIL), int(throw)]);
    r.fv(vec![s(email), int(FILTER_VALIDATE_EMAIL), int(throw)]);
    r.echo("\nfilter_var_array:\n");
    r.var_dump(
        "filter_var_array",
        vec![
            assoc(vec![("a", s("a"))]),
            assoc(vec![(
                "a",
                assoc(vec![("flags", int(FILTER_REQUIRE_SCALAR | throw))]),
            )]),
        ],
    );
    let obj = stringable(&mut r.t, "bar");
    r.var_dump(
        "filter_var_array",
        vec![
            assoc(vec![("a", obj)]),
            assoc(vec![("a", assoc(vec![("flags", int(throw))]))]),
        ],
    );
    r.var_dump(
        "filter_var_array",
        vec![
            assoc(vec![("a", s(email))]),
            assoc(vec![(
                "a",
                assoc(vec![
                    ("filter", int(FILTER_VALIDATE_EMAIL)),
                    ("flags", int(throw)),
                ]),
            )]),
        ],
    );
    r.check(phpt!("throw-on-failure/filter_success.phpt"));
}

/// throw-on-failure/throw-and-null-error.phpt.
#[test]
fn throw_and_null_on_failure() {
    let mut r = Run::new();
    r.input(PARSE_GET, vec![("a", s("1"))]);
    let both = FILTER_THROW_ON_FAILURE | FILTER_NULL_ON_FAILURE;
    let fi = |var: &str, o: Value| vec![int(PARSE_GET), s(var), int(FILTER_DEFAULT), o];
    r.echo("filter_input(), with a missing value\n");
    r.catch("filter_input", fi("b", int(both)));
    r.catch("filter_input", fi("b", flags(both)));
    r.echo("\nfilter_input(), with a missing value and a default\n");
    r.catch(
        "filter_input",
        fi(
            "b",
            assoc(vec![
                ("flags", int(both)),
                ("options", assoc(vec![("default", s("a"))])),
            ]),
        ),
    );
    r.echo("\nfilter_input(), with a present value\n");
    r.catch("filter_input", fi("a", int(both)));
    r.catch("filter_input", fi("a", flags(both)));
    r.echo("\nfilter_var()\n");
    r.catch(
        "filter_var",
        vec![Value::Bool(true), int(FILTER_DEFAULT), int(both)],
    );
    r.catch(
        "filter_var",
        vec![Value::Bool(true), int(FILTER_DEFAULT), flags(both)],
    );
    r.echo("\nfilter_input_array()\n");
    r.catch(
        "filter_input_array",
        vec![int(PARSE_GET), assoc(vec![("a", flags(both))])],
    );
    r.echo("\nfilter_var_array()\n");
    r.catch(
        "filter_var_array",
        vec![
            assoc(vec![("a", Value::Bool(true))]),
            assoc(vec![("a", flags(both))]),
        ],
    );
    r.check(phpt!("throw-on-failure/throw-and-null-error.phpt"));
}

/// filter_throw_on_failure_leak.phpt: converted scalars in messages.
#[test]
fn throw_on_failure_messages() {
    let mut r = Run::new();
    let e = thrown(r.call(
        "filter_var",
        vec![
            Value::Float(1.5),
            int(FILTER_VALIDATE_INT),
            flags(FILTER_THROW_ON_FAILURE),
        ],
    ));
    assert_eq!(
        e,
        (
            "Filter\\FilterFailedException".into(),
            "filter validation failed: filter int not satisfied by '1.5'".into()
        )
    );
    let v = r.value(
        "filter_var",
        vec![
            int(15),
            int(FILTER_VALIDATE_INT),
            flags(FILTER_THROW_ON_FAILURE),
        ],
    );
    assert_v!(v, int(15));
    // Arrays fail the scalar check.
    let e = thrown(r.call(
        "filter_var",
        vec![
            list(vec![int(1)]),
            int(FILTER_VALIDATE_INT),
            int(FILTER_THROW_ON_FAILURE),
        ],
    ));
    assert_eq!(
        e.1,
        "filter validation failed: not a scalar value (got an array)"
    );
    // The message stops at a NUL byte (`%s`).
    let e = thrown(r.call(
        "filter_var",
        vec![
            bytes(b"x\0y"),
            int(FILTER_VALIDATE_INT),
            int(FILTER_THROW_ON_FAILURE),
        ],
    ));
    assert_eq!(
        e.1,
        "filter validation failed: filter int not satisfied by 'x'"
    );
    // A ValueError from an option is not wrapped.
    let e = thrown(r.call(
        "filter_var",
        vec![
            s("1"),
            int(FILTER_VALIDATE_FLOAT),
            assoc(vec![
                ("flags", int(FILTER_THROW_ON_FAILURE)),
                ("options", assoc(vec![("decimal", s(""))])),
            ]),
        ],
    ));
    assert_eq!(e.0, "ValueError");
    assert_eq!(r.out, "");
}
