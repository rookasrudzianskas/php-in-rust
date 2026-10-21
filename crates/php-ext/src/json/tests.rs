//! Unit tests ported from `ext/json/tests/*.phpt`.
//!
//! Userland classes cannot be declared yet (the interpreter is developed
//! separately), so classes with `jsonSerialize()` methods, visibility mixes
//! and enums are declared natively with the runtime's class API.

mod location_cases;

use php_runtime::class::{
    ClassEntry, EnumBacking, Function, PropPurpose, PropertyInfo, class_flags, member_flags,
};
use php_runtime::value::float_fmt;
use php_runtime::{
    Args, ArrayKey, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, Registry,
    ResRef, Value, Vm,
};

use super::*;
use crate::testing::{TestVm, thrown};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn s(x: &str) -> Value {
    Value::str(x)
}

fn list(v: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(v))
}

fn map(pairs: Vec<(&str, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(ArrayKey::from_bytes(k.as_bytes()), v);
    }
    Value::array(a)
}

fn std_object(t: &TestVm, props: Vec<(&str, Value)>) -> Value {
    let o = ObjRef::new(t.vm.find_class(b"stdClass").unwrap());
    for (k, v) in props {
        o.set_prop(k.as_bytes(), v);
    }
    Value::Object(o)
}

/// `json_encode($v, $flags)`; `None` for `false`.
fn encode(t: &mut TestVm, v: Value, flags: i64) -> Option<String> {
    match t
        .call("json_encode", vec![v, Value::Int(flags)])
        .expect("no exception")
    {
        Value::String(s) => Some(String::from_utf8(s.to_vec()).expect("json_encode yields UTF-8")),
        Value::Bool(false) => None,
        other => panic!("json_encode returned {other:?}"),
    }
}

fn enc(t: &mut TestVm, v: Value) -> String {
    encode(t, v, 0).expect("encodable")
}

fn decode_with(t: &mut TestVm, json: &[u8], assoc: Option<bool>, depth: i64, flags: i64) -> Value {
    let assoc = assoc.map_or(Value::Null, Value::Bool);
    t.call(
        "json_decode",
        vec![
            Value::bytes(json),
            assoc,
            Value::Int(depth),
            Value::Int(flags),
        ],
    )
    .expect("no exception")
}

fn decode(t: &mut TestVm, json: &str) -> Value {
    decode_with(t, json.as_bytes(), None, DEFAULT_DEPTH, 0)
}

fn decode_assoc(t: &mut TestVm, json: &str) -> Value {
    decode_with(t, json.as_bytes(), Some(true), DEFAULT_DEPTH, 0)
}

fn last_error(t: &mut TestVm) -> (i64, String) {
    let code = t.call("json_last_error", vec![]).unwrap();
    let msg = t.call("json_last_error_msg", vec![]).unwrap();
    (
        code.as_int().expect("int"),
        msg.as_str().expect("string").to_string(),
    )
}

fn last_code(t: &mut TestVm) -> i64 {
    last_error(t).0
}

/// A compact `var_dump`-like rendering.
fn repr(v: &Value) -> String {
    match v {
        Value::Undef | Value::Null => "NULL".into(),
        Value::Bool(b) => format!("{b}"),
        Value::Int(i) => format!("{i}"),
        Value::Float(d) => format!("float({})", float_fmt::format_h(*d, -1)),
        Value::String(s) => format!("{:?}", s.to_string_lossy()),
        Value::Array(a) => {
            let items: Vec<String> = a
                .iter()
                .map(|(k, v)| match k {
                    ArrayKey::Int(i) => format!("{i} => {}", repr(&v.unref())),
                    ArrayKey::Str(k) => {
                        format!("{:?} => {}", k.to_string_lossy(), repr(&v.unref()))
                    }
                })
                .collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(o) => {
            let items: Vec<String> = o
                .properties()
                .iter()
                .map(|(k, v)| format!("{:?} => {}", k.to_string_lossy(), repr(&v.unref())))
                .collect();
            format!("{}{{{}}}", o.class().name, items.join(", "))
        }
        Value::Resource(_) => "resource".into(),
        Value::Ref(r) => repr(&r.get()),
    }
}

fn dec_repr(t: &mut TestVm, json: &str) -> String {
    let v = decode(t, json);
    repr(&v)
}

fn dec_repr_assoc(t: &mut TestVm, json: &str) -> String {
    let v = decode_assoc(t, json);
    repr(&v)
}

fn validate(t: &mut TestVm, json: &[u8], depth: i64, flags: i64) -> PResult<bool> {
    let r = t.call(
        "json_validate",
        vec![Value::bytes(json), Value::Int(depth), Value::Int(flags)],
    )?;
    Ok(r.as_bool().expect("bool"))
}

/// Runs `f` on a thread with a large stack (deeply nested values are
/// dropped recursively).
fn with_big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

// ---------------------------------------------------------------------------
// Native test classes
// ---------------------------------------------------------------------------

fn this_obj(args: &Args) -> ObjRef {
    args.this().cloned().expect("method call")
}

/// `return $this->data;`
fn m_return_data(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(this_obj(args).get_prop(b"data").unwrap_or(Value::Null))
}

/// `return $this;`
fn m_return_this(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Object(this_obj(args)))
}

/// `return array_values(is_array($this->data) ? $this->data : get_object_vars($this->data));`
fn m_return_values(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = this_obj(args).get_prop(b"data").unwrap_or(Value::Null);
    let values: Vec<Value> = match &data {
        Value::Array(a) => a.values().cloned().collect(),
        Value::Object(o) => o.properties().into_iter().map(|(_, v)| v).collect(),
        _ => Vec::new(),
    };
    Ok(list(values))
}

fn m_throw(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "Not implemented!"))
}

/// `return ['test' => $this->test, 'me' => $this];`
fn m_return_me(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this_obj(args);
    Ok(map(vec![
        ("test", this.get_prop(b"test").unwrap_or(Value::Null)),
        ("me", Value::Object(this)),
    ]))
}

/// `return json_encode($this);`
fn m_encode_this(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = Value::Object(this_obj(args));
    let r = vm.call_native("json_encode", vec![this])?;
    // Remember the nested result like `var_dump($result)` would show it.
    vm.ext_data::<NestedResult>().0.push(r);
    Ok(Value::Object(this_obj(args)))
}

/// `return json_encode([1], JSON_PRETTY_PRINT);`
fn m_nested_pretty(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.call_native(
        "json_encode",
        vec![list(vec![Value::Int(1)]), Value::Int(PRETTY_PRINT as i64)],
    )
}

#[derive(Default)]
struct NestedResult(Vec<Value>);

macro_rules! method {
    ($name:ident, $handler:path) => {
        static $name: NativeFunction = NativeFunction {
            sig: "public function jsonSerialize(): mixed",
            handler: $handler,
        };
    };
}
method!(JS_DATA, m_return_data);
method!(JS_THIS, m_return_this);
method!(JS_VALUES, m_return_values);
method!(JS_THROW, m_throw);
method!(JS_ME, m_return_me);
method!(JS_ENCODE_THIS, m_encode_this);
method!(JS_NESTED_PRETTY, m_nested_pretty);

fn declare(
    t: &mut TestVm,
    name: &str,
    parent: Option<&str>,
    json_serializable: Option<&'static NativeFunction>,
    props: &[(&str, u32, Value)],
) -> ClassRef {
    let parent = parent.map(|p| t.vm.find_class(p.as_bytes()).expect("parent"));
    let mut ce = match &parent {
        Some(p) => ClassEntry::new_subclass(name, 0, p),
        None => ClassEntry::new(name, 0),
    };
    for (pname, flags, default) in props {
        let mut info = PropertyInfo::new(*pname, name, *flags);
        info.default = default.clone();
        ce.declare_property(info);
    }
    if let Some(nf) = json_serializable {
        if parent.is_none() {
            let iface = t.vm.find_class(b"JsonSerializable").expect("interface");
            ce.interfaces.push(iface);
        }
        ce.add_method(Function::native_method(nf, name, member_flags::PUBLIC));
    }
    let mut r = Registry::new(&mut t.vm, "json_tests");
    r.class(ce)
}

const PUB: u32 = member_flags::PUBLIC;
const PROT: u32 = member_flags::PROTECTED;
const PRIV: u32 = member_flags::PRIVATE;

fn new_object(class: &ClassRef) -> ObjRef {
    ObjRef::new(class.clone())
}

// ---------------------------------------------------------------------------
// json_encode
// ---------------------------------------------------------------------------

/// 002.phpt, json_encode_basic.phpt
#[test]
fn encode_basics() {
    let mut t = TestVm::new();
    assert_eq!(enc(&mut t, s("")), r#""""#);
    assert_eq!(enc(&mut t, Value::Null), "null");
    assert_eq!(enc(&mut t, Value::Bool(true)), "true");
    assert_eq!(enc(&mut t, Value::Bool(false)), "false");
    assert_eq!(enc(&mut t, map(vec![("", s(""))])), r#"{"":""}"#);
    assert_eq!(enc(&mut t, list(vec![list(vec![Value::Int(1)])])), "[[1]]");
    assert_eq!(enc(&mut t, Value::empty_array()), "[]");
    assert_eq!(
        encode(&mut t, map(vec![("", s(""))]), FORCE_OBJECT as i64).unwrap(),
        r#"{"":""}"#
    );
    assert_eq!(
        encode(
            &mut t,
            list(vec![list(vec![Value::Int(1)])]),
            FORCE_OBJECT as i64
        )
        .unwrap(),
        r#"{"0":{"0":1}}"#
    );
    assert_eq!(
        encode(&mut t, Value::empty_array(), FORCE_OBJECT as i64).unwrap(),
        "{}"
    );
    assert_eq!(enc(&mut t, Value::Int(1)), "1");
    assert_eq!(
        enc(&mut t, s("руссиш")),
        r#""\u0440\u0443\u0441\u0441\u0438\u0448""#
    );

    assert_eq!(enc(&mut t, Value::Int(-2147483648)), "-2147483648");
    assert_eq!(enc(&mut t, Value::Float(123.456)), "123.456");
    assert_eq!(enc(&mut t, Value::Float(1.23e3)), "1230");
    assert_eq!(enc(&mut t, Value::Float(-1.23e3)), "-1230");
    assert_eq!(enc(&mut t, s("Hello\t\tWorld\n")), r#""Hello\t\tWorld\n""#);
    assert_eq!(
        enc(&mut t, list((1..=5).map(Value::Int).collect())),
        "[1,2,3,4,5]"
    );
    let mut a = PhpArray::new();
    for (i, d) in ["Sun", "Mon", "Tue"].iter().enumerate() {
        a.insert(ArrayKey::Int(i as i64 + 1), s(d));
    }
    assert_eq!(
        enc(&mut t, Value::array(a)),
        r#"{"1":"Sun","2":"Mon","3":"Tue"}"#
    );
    assert_eq!(
        enc(
            &mut t,
            map(vec![("Jan", Value::Int(31)), ("Feb", Value::Int(29))])
        ),
        r#"{"Jan":31,"Feb":29}"#
    );
    let obj = std_object(
        &t,
        vec![
            ("MyInt", Value::Int(99)),
            ("MyFloat", Value::Float(123.45)),
            ("MyBool", Value::Bool(true)),
            ("MyNull", Value::Null),
            ("MyString", s("Hello World")),
        ],
    );
    assert_eq!(
        enc(&mut t, obj),
        r#"{"MyInt":99,"MyFloat":123.45,"MyBool":true,"MyNull":null,"MyString":"Hello World"}"#
    );
}

/// 009.phpt: a "holey" array that is still a list.
#[test]
fn encode_list_detection() {
    let mut t = TestVm::new();
    let mut a = PhpArray::from_values(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    a.insert(ArrayKey::from("foo"), s("bar"));
    a.remove(&ArrayKey::from("foo"));
    assert_eq!(enc(&mut t, Value::array(a)), "[1,2,3]");
    // Keys out of order are an object.
    let mut a = PhpArray::new();
    a.insert(ArrayKey::Int(1), Value::Int(1));
    a.insert(ArrayKey::Int(0), Value::Int(0));
    assert_eq!(enc(&mut t, Value::array(a)), r#"{"1":1,"0":0}"#);
}

/// bug40503.phpt, bug50224.phpt, serialize_precision handling.
#[test]
fn encode_floats() {
    let mut t = TestVm::new();
    assert_eq!(enc(&mut t, Value::Int(2147483648)), "2147483648");
    let pz = PRESERVE_ZERO_FRACTION as i64;
    assert_eq!(encode(&mut t, Value::Float(12.3), pz).unwrap(), "12.3");
    assert_eq!(encode(&mut t, Value::Int(12), pz).unwrap(), "12");
    assert_eq!(encode(&mut t, Value::Float(12.0), pz).unwrap(), "12.0");
    assert_eq!(encode(&mut t, Value::Float(0.0), pz).unwrap(), "0.0");
    assert_eq!(
        encode(
            &mut t,
            list(vec![Value::Int(12), Value::Float(12.0), Value::Float(12.3)]),
            pz
        )
        .unwrap(),
        "[12,12.0,12.3]"
    );
    let obj = std_object(
        &t,
        vec![("float", Value::Float(12.0)), ("integer", Value::Int(12))],
    );
    assert_eq!(
        encode(&mut t, obj, pz).unwrap(),
        r#"{"float":12.0,"integer":12}"#
    );
    // Exponent format uses a lowercase `e`.
    assert_eq!(enc(&mut t, Value::Float(1.23456789e34)), "1.23456789e+34");
    assert_eq!(enc(&mut t, Value::Float(1.23456789e-13)), "1.23456789e-13");
    assert_eq!(enc(&mut t, Value::Float(0.1 + 0.2)), "0.30000000000000004");
    assert_eq!(enc(&mut t, Value::Float(-0.0)), "-0");
    assert_eq!(encode(&mut t, Value::Float(-0.0), pz).unwrap(), "-0.0");
    assert_eq!(enc(&mut t, Value::Float(100.1)), "100.1");

    // serialize_precision = 17
    t.vm.ini_set("serialize_precision", "17");
    assert_eq!(enc(&mut t, Value::Float(0.1)), "0.10000000000000001");
    t.vm.ini_set("serialize_precision", "5");
    assert_eq!(enc(&mut t, Value::Float(1234.56789)), "1234.6");
    assert_eq!(enc(&mut t, Value::Float(1.0e100)), "1.0e+100");
    t.vm.ini_set("serialize_precision", "-1");
}

/// inf_nan_error.phpt
#[test]
fn encode_inf_nan() {
    let mut t = TestVm::new();
    for d in [f64::INFINITY, f64::NAN, f64::NEG_INFINITY] {
        assert_eq!(encode(&mut t, Value::Float(d), 0), None);
        assert_eq!(
            last_error(&mut t),
            (7, "Inf and NaN cannot be JSON encoded".to_string())
        );
        assert_eq!(
            encode(&mut t, Value::Float(d), PARTIAL_OUTPUT_ON_ERROR as i64).as_deref(),
            Some("0")
        );
        assert_eq!(last_code(&mut t), 7);
    }
    // A successful call resets the error.
    assert_eq!(enc(&mut t, Value::Int(1)), "1");
    assert_eq!(last_code(&mut t), 0);
}

/// 006.phpt
#[test]
fn encode_hex_flags() {
    let mut t = TestVm::new();
    let a = || list(vec![s("<foo>"), s("'bar'"), s("\"baz\""), s("&blong&")]);
    assert_eq!(enc(&mut t, a()), r#"["<foo>","'bar'","\"baz\"","&blong&"]"#);
    assert_eq!(
        encode(&mut t, a(), HEX_TAG as i64).unwrap(),
        r#"["\u003Cfoo\u003E","'bar'","\"baz\"","&blong&"]"#
    );
    assert_eq!(
        encode(&mut t, a(), HEX_APOS as i64).unwrap(),
        r#"["<foo>","\u0027bar\u0027","\"baz\"","&blong&"]"#
    );
    assert_eq!(
        encode(&mut t, a(), HEX_QUOT as i64).unwrap(),
        r#"["<foo>","'bar'","\u0022baz\u0022","&blong&"]"#
    );
    assert_eq!(
        encode(&mut t, a(), HEX_AMP as i64).unwrap(),
        r#"["<foo>","'bar'","\"baz\"","\u0026blong\u0026"]"#
    );
    let all = (HEX_TAG | HEX_APOS | HEX_QUOT | HEX_AMP) as i64;
    assert_eq!(
        encode(&mut t, a(), all).unwrap(),
        r#"["\u003Cfoo\u003E","\u0027bar\u0027","\u0022baz\u0022","\u0026blong\u0026"]"#
    );
}

/// json_encode_unescaped_slashes.phpt, json_encode_u2028_u2029.phpt, bug53946.phpt
#[test]
fn encode_slashes_and_unicode() {
    let mut t = TestVm::new();
    assert_eq!(enc(&mut t, s("a/b")), r#""a\/b""#);
    assert_eq!(
        encode(&mut t, s("a/b"), UNESCAPED_SLASHES as i64).unwrap(),
        r#""a/b""#
    );

    let u = UNESCAPED_UNICODE as i64;
    let lt = UNESCAPED_LINE_TERMINATORS as i64;
    let b = |bytes: &[u8]| Value::bytes(bytes);
    assert_eq!(enc(&mut t, list(vec![b(b"a\xC3\xA1b")])), r#"["a\u00e1b"]"#);
    assert_eq!(
        encode(&mut t, list(vec![b(b"a\xC3\xA1b")]), u).unwrap(),
        "[\"a\u{e1}b\"]"
    );
    assert_eq!(enc(&mut t, b(b"a\xE2\x80\xA7b")), r#""a\u2027b""#);
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xA7b"), u).unwrap(),
        "\"a\u{2027}b\""
    );
    assert_eq!(enc(&mut t, b(b"a\xE2\x80\xA8b")), r#""a\u2028b""#);
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xA8b"), u).unwrap(),
        r#""a\u2028b""#
    );
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xA8b"), lt).unwrap(),
        r#""a\u2028b""#
    );
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xA8b"), u | lt).unwrap(),
        "\"a\u{2028}b\""
    );
    assert_eq!(enc(&mut t, b(b"a\xE2\x80\xA9b")), r#""a\u2029b""#);
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xA9b"), u).unwrap(),
        r#""a\u2029b""#
    );
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xA9b"), u | lt).unwrap(),
        "\"a\u{2029}b\""
    );
    assert_eq!(enc(&mut t, b(b"a\xE2\x80\xAAb")), r#""a\u202ab""#);
    assert_eq!(
        encode(&mut t, b(b"a\xE2\x80\xAAb"), u).unwrap(),
        "\"a\u{202a}b\""
    );

    let text = "latin 1234 -/    russian мама мыла раму  specialchars \x02   \x08 \n   U+1D11E >𝄞<";
    assert_eq!(
        enc(&mut t, s(text)),
        r#""latin 1234 -\/    russian \u043c\u0430\u043c\u0430 \u043c\u044b\u043b\u0430 \u0440\u0430\u043c\u0443  specialchars \u0002   \b \n   U+1D11E >\ud834\udd1e<""#
    );
    assert_eq!(
        encode(&mut t, s(text), u).unwrap(),
        "\"latin 1234 -\\/    russian мама мыла раму  specialchars \\u0002   \\b \\n   U+1D11E >𝄞<\""
    );
    // Control characters and DEL.
    assert_eq!(
        enc(&mut t, b(b"\x00\x1f\x7f\x0c")),
        "\"\\u0000\\u001f\u{7f}\\f\""
    );
}

/// bug46944.phpt: 4-byte sequences become surrogate pairs.
#[test]
fn encode_four_byte_utf8() {
    let mut t = TestVm::new();
    let want = [
        "\\ud83f\\udffd",
        "\\ud87f\\udffd",
        "\\ud8bf\\udffd",
        "\\ud8ff\\udffd",
        "\\ud93f\\udffd",
        "\\ud97f\\udffd",
        "\\ud9bf\\udffd",
        "\\ud9ff\\udffd",
        "\\uda3f\\udffd",
        "\\uda7f\\udffd",
        "\\udabf\\udffd",
        "\\udaff\\udffd",
        "\\udb3f\\udffd",
        "\\udb7f\\udffd",
        "\\udbbf\\udffd",
        "\\udbff\\udffd",
    ];
    for i in 1..=16u8 {
        let first = 0xf0 | (i >> 2);
        let second = 0x8f | ((i & 3) << 4);
        let string = [b'a', b'a', first, second, 0xbf, 0xbd, b'z', b'z'];
        assert_eq!(
            enc(&mut t, Value::bytes(&string)),
            format!("\"aa{}zz\"", want[i as usize - 1])
        );
    }
    // bug41067.phpt
    assert_eq!(
        enc(&mut t, list(vec![Value::bytes(b"\xf0\x9d\x84\x80")])),
        r#"["\ud834\udd00"]"#
    );
}

/// bug41034.phpt: NUL-prefixed keys of arrays are kept (and escaped).
#[test]
fn encode_nul_keys_of_arrays() {
    let mut t = TestVm::new();
    let mut a = PhpArray::new();
    a.append(Value::Int(0)).unwrap();
    a.insert(ArrayKey::Str(b"\0ab".as_slice().into()), Value::Int(1));
    a.append(s("\0null-prefixed value")).unwrap();
    assert_eq!(
        enc(&mut t, Value::array(a)),
        r#"{"0":0,"\u0000ab":1,"1":"\u0000null-prefixed value"}"#
    );
}

/// json_encode_invalid_utf8.phpt, bug43941.phpt, bug61537.phpt, bug68567.phpt, bug54058.phpt
#[test]
fn encode_invalid_utf8() {
    let mut t = TestVm::new();
    let ignore = INVALID_UTF8_IGNORE as i64;
    let subst = INVALID_UTF8_SUBSTITUTE as i64;
    let u = UNESCAPED_UNICODE as i64;
    let partial = PARTIAL_OUTPUT_ON_ERROR as i64;
    for (input, ignored, substituted) in [
        (&b"a\xb0b"[..], r#""ab""#, r#""a\ufffdb""#),
        (&b"a\xf0\x80\x80A"[..], r#""aA""#, r#""a\ufffdA""#),
    ] {
        let v = || Value::bytes(input);
        assert_eq!(encode(&mut t, v(), 0), None);
        assert_eq!(last_code(&mut t), 5);
        assert_eq!(encode(&mut t, v(), ignore).as_deref(), Some(ignored));
        assert_eq!(encode(&mut t, v(), subst).as_deref(), Some(substituted));
        assert_eq!(encode(&mut t, v(), u), None);
        let sub_unescaped = encode(&mut t, v(), u | subst).unwrap();
        assert!(
            sub_unescaped
                .as_bytes()
                .windows(3)
                .any(|w| w == b"\xef\xbf\xbd")
        );
    }
    assert_eq!(enc(&mut t, s("abc")), r#""abc""#);
    assert_eq!(encode(&mut t, Value::bytes(b"ab\xE0"), 0), None);
    assert_eq!(
        encode(&mut t, Value::bytes(b"ab\xE0"), partial).as_deref(),
        Some("null")
    );
    assert_eq!(
        encode(
            &mut t,
            list(vec![
                Value::bytes(b"ab\xE0"),
                Value::bytes(b"ab\xE0c"),
                s("abc")
            ]),
            partial
        )
        .as_deref(),
        Some(r#"[null,null,"abc"]"#)
    );
    assert_eq!(last_code(&mut t), 5);
    assert_eq!(
        last_error(&mut t).1,
        "Malformed UTF-8 characters, possibly incorrectly encoded"
    );
    assert_eq!(encode(&mut t, Value::bytes(b"\x9f"), 0), None);
    assert_eq!(
        encode(
            &mut t,
            Value::bytes(b"an invalid sequen\xce in the middle"),
            partial
        )
        .as_deref(),
        Some("null")
    );

    // bug68567: an invalid key becomes "" with partial output.
    let mut a = PhpArray::new();
    a.insert(ArrayKey::Str(b"\x80".as_slice().into()), Value::Int(1));
    assert_eq!(
        encode(&mut t, Value::array(a.clone()), partial).as_deref(),
        Some(r#"{"":1}"#)
    );
    assert_eq!(encode(&mut t, Value::array(a), 0), None);

    // bug54058: invalid UTF-8 in object properties and array values.
    let o = std_object(
        &t,
        vec![("foo", Value::bytes(b"\xb0")), ("bar", Value::Int(1))],
    );
    assert_eq!(encode(&mut t, o, 0), None);
    assert_eq!(last_code(&mut t), 5);
    let a = map(vec![("foo", Value::bytes(b"\xb0")), ("bar", Value::Int(1))]);
    assert_eq!(encode(&mut t, a.clone(), 0), None);
    assert_eq!(last_code(&mut t), 5);
    assert_eq!(
        encode(&mut t, a, partial).as_deref(),
        Some(r#"{"foo":null,"bar":1}"#)
    );
}

/// php_next_utf8_char(): how many bytes an invalid sequence swallows.
#[test]
fn encode_invalid_utf8_advance() {
    let mut t = TestVm::new();
    let ignore = INVALID_UTF8_IGNORE as i64;
    let subst = INVALID_UTF8_SUBSTITUTE as i64;
    // Truncated 3-byte sequence followed by a lead byte: only the first byte is invalid.
    assert_eq!(
        encode(&mut t, Value::bytes(b"\xe2\x82a"), subst).unwrap(),
        r#""\ufffda""#
    );
    // C0/C1 are never valid lead bytes: each byte is replaced separately.
    assert_eq!(
        encode(&mut t, Value::bytes(b"\xc0\x80a"), subst).unwrap(),
        r#""\ufffd\ufffda""#
    );
    // Overlong 3-byte form swallows all three bytes.
    assert_eq!(
        encode(&mut t, Value::bytes(b"\xe0\x80\x80a"), subst).unwrap(),
        r#""\ufffda""#
    );
    // Surrogate (ED A0 80) swallows three bytes.
    assert_eq!(
        encode(&mut t, Value::bytes(b"\xed\xa0\x80a"), subst).unwrap(),
        r#""\ufffda""#
    );
    // Above U+10FFFF swallows four bytes.
    assert_eq!(
        encode(&mut t, Value::bytes(b"\xf4\x90\x80\x80a"), subst).unwrap(),
        r#""\ufffda""#
    );
    // Lone continuation bytes are one each.
    assert_eq!(
        encode(&mut t, Value::bytes(b"\x80\x80"), subst).unwrap(),
        r#""\ufffd\ufffd""#
    );
    assert_eq!(
        encode(&mut t, Value::bytes(b"\xff\xfe!"), ignore).unwrap(),
        r#""!""#
    );
}

/// json_encode_numeric.phpt, bug64695.phpt, bug55543.phpt
#[test]
fn encode_numeric_check() {
    let mut t = TestVm::new();
    let nc = NUMERIC_CHECK as i64;
    assert_eq!(encode(&mut t, s("1"), nc).unwrap(), "1");
    assert_eq!(encode(&mut t, s("9.4324"), nc).unwrap(), "9.4324");
    assert_eq!(
        encode(&mut t, list(vec![s("122321"), s("3232595.33423")]), nc).unwrap(),
        "[122321,3232595.33423]"
    );
    assert_eq!(enc(&mut t, s("1")), r#""1""#);
    assert_eq!(enc(&mut t, s("9.4324")), r#""9.4324""#);
    assert_eq!(
        enc(&mut t, list(vec![s("122321"), s("3232595.33423")])),
        r#"["122321","3232595.33423"]"#
    );
    // Overflowing exponent: not a valid double, stays a string.
    assert_eq!(
        encode(&mut t, map(vec![("test", s("123343e871700"))]), nc).unwrap(),
        r#"{"test":"123343e871700"}"#
    );
    // Whitespace around numbers is allowed by is_numeric_string; garbage is not.
    assert_eq!(encode(&mut t, s(" 12"), nc).unwrap(), "12");
    assert_eq!(encode(&mut t, s("12 "), nc).unwrap(), "12");
    assert_eq!(encode(&mut t, s("12abc"), nc).unwrap(), r#""12abc""#);
    assert_eq!(encode(&mut t, s("1e3"), nc).unwrap(), "1000");
    assert_eq!(encode(&mut t, s("0x1A"), nc).unwrap(), r#""0x1A""#);
    // Keys are never converted; values of numeric-keyed properties are.
    let o = std_object(&t, vec![("1", s("5"))]);
    assert_eq!(encode(&mut t, o, nc).unwrap(), r#"{"1":5}"#);
    assert_eq!(
        encode(&mut t, map(vec![("12", s("a"))]), nc).unwrap(),
        r#"{"12":"a"}"#
    );
}

/// json_encode_pretty_print.phpt, json_encode_pretty_print2.phpt, bug66021.phpt
#[test]
fn encode_pretty_print() {
    let mut t = TestVm::new();
    let pp = PRETTY_PRINT as i64;
    let v = list(vec![
        Value::Int(1),
        Value::Int(2),
        Value::Int(3),
        list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    ]);
    assert_eq!(
        encode(&mut t, v, pp).unwrap(),
        "[\n    1,\n    2,\n    3,\n    [\n        1,\n        2,\n        3\n    ]\n]"
    );
    let v = map(vec![
        ("a", Value::Int(1)),
        ("b", list(vec![Value::Int(1), Value::Int(2)])),
        ("c", map(vec![("d", Value::Int(42))])),
    ]);
    assert_eq!(
        encode(&mut t, v, pp).unwrap(),
        "{\n    \"a\": 1,\n    \"b\": [\n        1,\n        2\n    ],\n    \"c\": {\n        \"d\": 42\n    }\n}"
    );

    // bug66021: empty containers stay on one line.
    let foo = declare(&mut t, "Foo", None, None, &[("bar", PRIV, s("baz"))]);
    let v = list(vec![
        Value::empty_array(),
        std_object(&t, vec![]),
        Value::Object(new_object(&foo)),
    ]);
    assert_eq!(
        encode(&mut t, v, pp).unwrap(),
        "[\n    [],\n    {},\n    {}\n]"
    );
    assert_eq!(encode(&mut t, Value::empty_array(), pp).unwrap(), "[]");
    assert_eq!(
        encode(&mut t, map(vec![("a", Value::empty_array())]), pp).unwrap(),
        "{\n    \"a\": []\n}"
    );

    // Declared and dynamic properties, uninitialised ones are skipped.
    let my = declare(
        &mut t,
        "MyClass",
        None,
        None,
        &[
            ("x", PUB, Value::Int(123)),
            ("y", PUB, Value::empty_array()),
        ],
    );
    let obj = new_object(&my);
    assert_eq!(
        enc(&mut t, Value::Object(obj.clone())),
        r#"{"x":123,"y":[]}"#
    );
    assert_eq!(
        encode(&mut t, Value::Object(obj.clone()), pp).unwrap(),
        "{\n    \"x\": 123,\n    \"y\": []\n}"
    );
    let inner = new_object(&my);
    inner.set_prop(b"x", Value::Null);
    obj.set_prop(b"dynamic", Value::Object(inner));
    assert_eq!(
        enc(&mut t, Value::Object(obj.clone())),
        r#"{"x":123,"y":[],"dynamic":{"x":null,"y":[]}}"#
    );
    assert_eq!(
        encode(&mut t, Value::Object(obj), pp).unwrap(),
        "{\n    \"x\": 123,\n    \"y\": [],\n    \"dynamic\": {\n        \"x\": null,\n        \"y\": []\n    }\n}"
    );
    let obj = new_object(&my);
    obj.obj().props.borrow_mut()[1] = Value::Undef; // unset($obj->y)
    assert_eq!(enc(&mut t, Value::Object(obj.clone())), r#"{"x":123}"#);
    obj.obj().props.borrow_mut()[0] = Value::Undef; // unset($obj->x)
    assert_eq!(enc(&mut t, Value::Object(obj.clone())), "{}");
    assert_eq!(encode(&mut t, Value::Object(obj), pp).unwrap(), "{}");

    // Pretty printing combined with other flags.
    assert_eq!(
        encode(
            &mut t,
            map(vec![("k/", s("v/"))]),
            pp | UNESCAPED_SLASHES as i64
        )
        .unwrap(),
        "{\n    \"k/\": \"v/\"\n}"
    );
}

/// Visibility: only public properties are encoded (bug46215.phpt).
#[test]
fn encode_object_visibility() {
    let mut t = TestVm::new();
    let c = declare(
        &mut t,
        "Vis",
        None,
        None,
        &[
            ("a", PUB, Value::Int(1)),
            ("b", PROT, Value::Int(2)),
            ("c", PRIV, Value::Int(3)),
            ("d", PUB, Value::Int(4)),
        ],
    );
    let obj = new_object(&c);
    assert_eq!(enc(&mut t, Value::Object(obj.clone())), r#"{"a":1,"d":4}"#);
    obj.set_prop(b"dyn", Value::Int(5));
    assert_eq!(
        enc(&mut t, Value::Object(obj.clone())),
        r#"{"a":1,"d":4,"dyn":5}"#
    );
    // A subclass adding a public property after the parent's slots.
    let sub = declare(
        &mut t,
        "VisSub",
        Some("Vis"),
        None,
        &[("e", PUB, Value::Int(6))],
    );
    assert_eq!(
        enc(&mut t, Value::Object(new_object(&sub))),
        r#"{"a":1,"d":4,"e":6}"#
    );
    // Only protected/private members.
    let c = declare(&mut t, "Hidden", None, None, &[("bar", PRIV, s("baz"))]);
    assert_eq!(enc(&mut t, Value::Object(new_object(&c))), "{}");
    // bug46215: the object is unchanged.
    assert_eq!(
        new_object(&c).get_prop(b"bar").map(|v| repr(&v)),
        Some("\"baz\"".to_string())
    );
}

fn m_json_props(_vm: &mut Vm, _o: &ObjRef, purpose: PropPurpose) -> PResult<Option<PhpArray>> {
    assert_eq!(purpose, PropPurpose::Json);
    let mut a = PhpArray::new();
    a.append(Value::Int(10)).unwrap();
    a.insert(ArrayKey::from("name"), s("x"));
    a.insert(
        ArrayKey::Str(b"\0*\0hidden".as_slice().into()),
        Value::Int(1),
    );
    Ok(Some(a))
}

/// Internal classes provide their JSON properties through `get_properties_for`.
#[test]
fn encode_get_properties_for_hook() {
    let mut t = TestVm::new();
    let mut ce = ClassEntry::new("HookedProps", 0);
    ce.handlers.get_properties_for = Some(m_json_props);
    let class = Registry::new(&mut t.vm, "json_tests").class(ce);
    assert_eq!(
        enc(&mut t, Value::Object(new_object(&class))),
        r#"{"0":10,"name":"x"}"#
    );
}

/// 003.phpt, 004.phpt, 005.phpt, json_encode_recursion
#[test]
fn encode_recursion() {
    let mut t = TestVm::new();
    let partial = PARTIAL_OUTPUT_ON_ERROR as i64;

    // $a = array(); $a[] = &$a;
    let r = PhpRef::new(Value::empty_array());
    let mut arr = PhpArray::new();
    arr.append(Value::Ref(r.clone())).unwrap();
    r.set(Value::array(arr));
    let a = r.get();
    assert_eq!(encode(&mut t, a.clone(), 0), None);
    assert_eq!(last_error(&mut t), (6, "Recursion detected".to_string()));
    assert_eq!(encode(&mut t, a, partial).as_deref(), Some("[null]"));
    assert_eq!(last_code(&mut t), 6);
    // break the cycle
    r.set(Value::Null);

    // $a = new stdclass; $a->prop = $a;
    let o = ObjRef::new(t.vm.find_class(b"stdClass").unwrap());
    o.set_prop(b"prop", Value::Object(o.clone()));
    assert_eq!(encode(&mut t, Value::Object(o.clone()), 0), None);
    assert_eq!(last_code(&mut t), 6);
    assert_eq!(
        encode(&mut t, Value::Object(o.clone()), partial).as_deref(),
        Some(r#"{"prop":null}"#)
    );
    o.set_prop(b"prop", Value::Null);

    // $a = array(); $a[] = $a;  (a copy, not a recursion)
    let inner = Value::empty_array();
    assert_eq!(enc(&mut t, list(vec![inner])), "[[]]");
    // The same array twice (siblings) is fine.
    let shared = list(vec![Value::Int(1)]);
    assert_eq!(enc(&mut t, list(vec![shared.clone(), shared])), "[[1],[1]]");
    // The recursion bookkeeping is cleaned up after errors.
    assert_eq!(enc(&mut t, list(vec![Value::Int(1)])), "[1]");
}

/// bug61978.phpt, json_encode_recursion_01.phpt
#[test]
fn encode_recursion_with_json_serializable() {
    let mut t = TestVm::new();
    let partial = PARTIAL_OUTPUT_ON_ERROR as i64;

    // Plain object referencing itself.
    let c1 = declare(
        &mut t,
        "JsonTest1",
        None,
        None,
        &[("test", PUB, Value::Null), ("me", PUB, Value::Null)],
    );
    let o1 = new_object(&c1);
    o1.set_prop(b"test", s("123"));
    o1.set_prop(b"me", Value::Object(o1.clone()));
    assert_eq!(
        encode(&mut t, Value::Object(o1.clone()), partial).as_deref(),
        Some(r#"{"test":"123","me":null}"#)
    );
    o1.set_prop(b"me", Value::Null);

    // JsonSerializable returning an array containing the object itself.
    let c2 = declare(
        &mut t,
        "JsonTest2",
        None,
        Some(&JS_ME),
        &[("test", PUB, Value::Null)],
    );
    let o2 = new_object(&c2);
    o2.set_prop(b"test", s("123"));
    assert_eq!(
        encode(&mut t, Value::Object(o2.clone()), partial).as_deref(),
        Some(r#"{"test":"123","me":null}"#)
    );
    assert_eq!(encode(&mut t, Value::Object(o2), 0), None);
    assert_eq!(last_code(&mut t), 6);

    // json_encode($this) inside jsonSerialize() is a recursion; the outer
    // call still succeeds by returning $this.
    let c3 = declare(
        &mut t,
        "SerializingTest3",
        None,
        Some(&JS_ENCODE_THIS),
        &[("a", PUB, Value::Int(1))],
    );
    let o3 = new_object(&c3);
    assert_eq!(enc(&mut t, Value::Object(o3)), r#"{"a":1}"#);
    let nested = std::mem::take(&mut t.vm.ext_data::<NestedResult>().0);
    assert!(
        matches!(nested.as_slice(), [Value::Bool(false)]),
        "{nested:?}"
    );
}

/// serialize.phpt
#[test]
fn encode_json_serializable() {
    let mut t = TestVm::new();
    let non = declare(
        &mut t,
        "NonSerializingTest",
        None,
        None,
        &[("data", PUB, Value::Null)],
    );
    let ser = declare(
        &mut t,
        "SerializingTest",
        None,
        Some(&JS_DATA),
        &[("data", PUB, Value::Null)],
    );
    let val = declare(
        &mut t,
        "ValueSerializingTest",
        Some("SerializingTest"),
        Some(&JS_VALUES),
        &[],
    );
    let this = declare(
        &mut t,
        "SelfSerializingTest",
        Some("SerializingTest"),
        Some(&JS_THIS),
        &[],
    );

    let std = t.vm.find_class(b"stdClass").unwrap();
    let new_std = || Value::Object(ObjRef::new(std.clone()));
    let adata = || {
        map(vec![
            ("str", s("foo")),
            ("int", Value::Int(1)),
            ("float", Value::Float(2.3)),
            ("bool", Value::Bool(false)),
            ("nil", Value::Null),
            (
                "arr",
                list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            ),
            ("obj", new_std()),
        ])
    };
    let ndata = || {
        list(vec![
            s("foo"),
            Value::Int(1),
            Value::Float(2.3),
            Value::Bool(false),
            Value::Null,
            list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            new_std(),
        ])
    };
    let odata = || {
        let Value::Array(a) = adata() else {
            unreachable!()
        };
        let o = ObjRef::new(std.clone());
        for (k, v) in a.iter() {
            o.set_prop(&k.to_php_str(), v.clone());
        }
        Value::Object(o)
    };
    let with_data = |class: &ClassRef, data: Value| {
        let o = new_object(class);
        o.set_prop(b"data", data);
        Value::Object(o)
    };

    let a_json =
        r#"{"str":"foo","int":1,"float":2.3,"bool":false,"nil":null,"arr":[1,2,3],"obj":{}}"#;
    let n_json = r#"["foo",1,2.3,false,null,[1,2,3],{}]"#;

    assert_eq!(
        enc(&mut t, with_data(&non, adata())),
        format!(r#"{{"data":{a_json}}}"#)
    );
    assert_eq!(
        enc(&mut t, with_data(&non, ndata())),
        format!(r#"{{"data":{n_json}}}"#)
    );
    let od = odata();
    assert_eq!(
        enc(&mut t, with_data(&non, od)),
        format!(r#"{{"data":{a_json}}}"#)
    );

    assert_eq!(enc(&mut t, with_data(&ser, adata())), a_json);
    assert_eq!(enc(&mut t, with_data(&ser, ndata())), n_json);
    let od = odata();
    assert_eq!(enc(&mut t, with_data(&ser, od)), a_json);

    assert_eq!(enc(&mut t, with_data(&val, adata())), n_json);
    assert_eq!(enc(&mut t, with_data(&val, ndata())), n_json);
    let od = odata();
    assert_eq!(enc(&mut t, with_data(&val, od)), n_json);

    assert_eq!(
        enc(&mut t, with_data(&this, adata())),
        format!(r#"{{"data":{a_json}}}"#)
    );
    assert_eq!(
        enc(&mut t, with_data(&this, ndata())),
        format!(r#"{{"data":{n_json}}}"#)
    );
    let od = odata();
    assert_eq!(
        enc(&mut t, with_data(&this, od)),
        format!(r#"{{"data":{a_json}}}"#)
    );
}

/// bug68992.phpt, bug73113.phpt: exceptions from jsonSerialize() propagate unchanged.
#[test]
fn encode_json_serializable_exceptions() {
    let mut t = TestVm::new();
    let c = declare(&mut t, "MyClass", None, Some(&JS_THROW), &[]);
    let objs: Vec<Value> = (0..5).map(|_| Value::Object(new_object(&c))).collect();
    let r = t.call("json_encode", vec![list(objs)]);
    let (class, msg) = thrown(r);
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("Exception", "Not implemented!")
    );
    // The guard is released: the same object can be encoded (and throws) again.
    let r = t.call("json_encode", vec![Value::Object(new_object(&c))]);
    assert_eq!(thrown(r).1, "Not implemented!");
    assert_eq!(enc(&mut t, Value::Int(1)), "1");
}

/// bug66025.phpt: nested json_encode() calls do not share formatting state.
#[test]
fn encode_nested_json_encode() {
    let mut t = TestVm::new();
    let c = declare(&mut t, "Foo", None, Some(&JS_NESTED_PRETTY), &[]);
    assert_eq!(
        enc(&mut t, list(vec![Value::Object(new_object(&c))])),
        r#"["[\n    1\n]"]"#
    );
}

/// Enums: backed enums encode their value, pure enums fail (8.1+).
#[test]
fn encode_enums() {
    let mut t = TestVm::new();
    let make = |t: &mut TestVm, name: &str, backing: Option<EnumBacking>| {
        let mut ce = ClassEntry::new(name, class_flags::ENUM | class_flags::FINAL);
        ce.enum_backing = backing;
        ce.declare_property(PropertyInfo::new(
            "name",
            name,
            PUB | member_flags::READONLY,
        ));
        if backing != Some(EnumBacking::Pure) && backing.is_some() {
            ce.declare_property(PropertyInfo::new(
                "value",
                name,
                PUB | member_flags::READONLY,
            ));
        }
        Registry::new(&mut t.vm, "json_tests").class(ce)
    };
    let suit = make(&mut t, "Suit", Some(EnumBacking::String));
    let hearts = new_object(&suit);
    hearts.set_prop(b"name", s("Hearts"));
    hearts.set_prop(b"value", s("H"));
    assert_eq!(enc(&mut t, Value::Object(hearts.clone())), r#""H""#);
    assert_eq!(
        enc(&mut t, list(vec![Value::Object(hearts.clone())])),
        r#"["H"]"#
    );
    let num = make(&mut t, "Num", Some(EnumBacking::Int));
    let one = new_object(&num);
    one.set_prop(b"name", s("One"));
    one.set_prop(b"value", Value::Int(1));
    assert_eq!(enc(&mut t, Value::Object(one)), "1");

    let pure = make(&mut t, "Pure", None);
    let case = new_object(&pure);
    case.set_prop(b"name", s("A"));
    assert_eq!(encode(&mut t, Value::Object(case.clone()), 0), None);
    assert_eq!(
        last_error(&mut t),
        (
            11,
            "Non-backed enums have no default serialization".to_string()
        )
    );
    // A '0' is emitted for the failed enum even though it is not partial output.
    assert_eq!(
        encode(&mut t, Value::Object(case), PARTIAL_OUTPUT_ON_ERROR as i64).as_deref(),
        Some("0")
    );
    assert_eq!(last_code(&mut t), 11);
}

/// unsupported_type_error.phpt
#[test]
fn encode_unsupported_type() {
    let mut t = TestVm::new();
    let res = Value::Resource(ResRef::new("stream", Box::new(())));
    assert_eq!(encode(&mut t, res.clone(), 0), None);
    assert_eq!(last_error(&mut t), (8, "Type is not supported".to_string()));
    assert_eq!(
        encode(&mut t, res.clone(), PARTIAL_OUTPUT_ON_ERROR as i64).as_deref(),
        Some("null")
    );
    assert_eq!(last_code(&mut t), 8);
    assert_eq!(
        encode(
            &mut t,
            list(vec![res, Value::Int(1)]),
            PARTIAL_OUTPUT_ON_ERROR as i64
        )
        .as_deref(),
        Some("[null,1]")
    );
}

/// bug81532.phpt, bug62369.phpt
#[test]
fn encode_depth() {
    let mut t = TestVm::new();
    let call = |t: &mut TestVm, v: Value, depth: i64| {
        let r = t
            .call("json_encode", vec![v, Value::Int(0), Value::Int(depth)])
            .unwrap();
        r.as_str().map(|s| s.to_string())
    };
    let a = std_object(&t, vec![]);
    let b = std_object(&t, vec![("x", a.clone())]);
    let c = std_object(&t, vec![("x", list(vec![a.clone()]))]);
    assert_eq!(call(&mut t, a.clone(), 0), None);
    assert_eq!(call(&mut t, a.clone(), 1).as_deref(), Some("{}"));
    assert_eq!(call(&mut t, b.clone(), 1), None);
    assert_eq!(
        last_error(&mut t),
        (1, "Maximum stack depth exceeded".to_string())
    );
    assert_eq!(call(&mut t, b, 2).as_deref(), Some(r#"{"x":{}}"#));
    assert_eq!(call(&mut t, c.clone(), 2), None);
    assert_eq!(call(&mut t, c, 3).as_deref(), Some(r#"{"x":[{}]}"#));

    // 551 levels of arrays.
    let mut v = Value::empty_array();
    for _ in 0..550 {
        v = list(vec![v]);
    }
    assert!(call(&mut t, v.clone(), 551).is_some());
    assert_eq!(last_code(&mut t), 0);
    assert_eq!(call(&mut t, v.clone(), 540), None);
    assert_eq!(last_code(&mut t), 1);
    // Partial output keeps going and still reports the error.
    let r = t
        .call(
            "json_encode",
            vec![v, Value::Int(PARTIAL_OUTPUT_ON_ERROR as i64), Value::Int(3)],
        )
        .unwrap();
    assert!(r.as_str().is_some_and(|s| s.starts_with(b"[[[[[")));
    assert_eq!(last_code(&mut t), 1);
}

/// gh15168.phpt: absurdly deep nesting fails with a depth error instead of
/// overflowing the native stack.
#[test]
fn encode_very_deep_nesting_hits_stack_limit() {
    with_big_stack(|| {
        let mut t = TestVm::new();
        let class = declare(&mut t, "Node", None, None, &[("next", PUB, Value::Null)]);
        let first = new_object(&class);
        let mut node = first.clone();
        for _ in 0..30000 {
            let n = new_object(&class);
            node.set_prop(b"next", Value::Object(n.clone()));
            node = n;
        }
        let r = t
            .call(
                "json_encode",
                vec![
                    Value::Object(first.clone()),
                    Value::Int(0),
                    Value::Int(500000),
                ],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            last_error(&mut t),
            (1, "Maximum stack depth exceeded".to_string())
        );
        // Unlink iteratively to avoid a deep recursive drop.
        let mut cur = first.get_prop(b"next");
        first.set_prop(b"next", Value::Null);
        while let Some(Value::Object(o)) = cur {
            cur = o.get_prop(b"next");
            o.set_prop(b"next", Value::Null);
        }
    });
}

/// json_encode_exceptions.phpt, json_exceptions_error_clearing.phpt
#[test]
fn encode_throw_on_error() {
    let mut t = TestVm::new();
    let r = t.call(
        "json_encode",
        vec![Value::bytes(b"\x80"), Value::Int(THROW_ON_ERROR as i64)],
    );
    let Err(Flow::Exception(e)) = r else {
        panic!("expected exception");
    };
    assert_eq!(e.class().name, "JsonException");
    assert_eq!(
        Vm::exception_message(&e),
        "Malformed UTF-8 characters, possibly incorrectly encoded"
    );
    assert_eq!(e.get_prop(b"code").and_then(|v| v.as_int()), Some(5));
    assert!(e.class().instance_of_name(b"Exception"));
    assert_eq!(last_code(&mut t), 0);

    // PARTIAL_OUTPUT_ON_ERROR overrides THROW_ON_ERROR.
    let r = t
        .call(
            "json_encode",
            vec![
                Value::bytes(b"\x80"),
                Value::Int((THROW_ON_ERROR | PARTIAL_OUTPUT_ON_ERROR) as i64),
            ],
        )
        .unwrap();
    assert_eq!(r.as_str().map(|s| s.to_string()).as_deref(), Some("null"));
    assert_eq!(
        last_error(&mut t),
        (
            5,
            "Malformed UTF-8 characters, possibly incorrectly encoded".to_string()
        )
    );

    // The global error state is untouched by THROW_ON_ERROR calls.
    let r = t.call(
        "json_encode",
        vec![Value::Float(f64::NAN), Value::Int(THROW_ON_ERROR as i64)],
    );
    assert_eq!(thrown(r).1, "Inf and NaN cannot be JSON encoded");
    assert_eq!(last_code(&mut t), 5);
    // ... also on success.
    let r = t
        .call(
            "json_encode",
            vec![Value::Int(1), Value::Int(THROW_ON_ERROR as i64)],
        )
        .unwrap();
    assert_eq!(r.as_str().map(|s| s.to_string()).as_deref(), Some("1"));
    assert_eq!(last_code(&mut t), 5);
}

#[test]
fn encode_argument_errors() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("json_encode", vec![]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "json_encode() expects at least 1 argument, 0 given");
    let (class, msg) = thrown(t.call("json_encode", vec![Value::Int(1), s("x")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "json_encode(): Argument #2 ($flags) must be of type int, string given"
    );
    let (class, msg) = thrown(t.call("json_last_error", vec![Value::Bool(true)]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(
        msg,
        "json_last_error() expects exactly 0 arguments, 1 given"
    );
    let (_, msg) = thrown(t.call("json_last_error_msg", vec![Value::Bool(true)]));
    assert_eq!(
        msg,
        "json_last_error_msg() expects exactly 0 arguments, 1 given"
    );
}

// ---------------------------------------------------------------------------
// json_decode
// ---------------------------------------------------------------------------

/// 001.phpt
#[test]
fn decode_basic_failures_and_objects() {
    let mut t = TestVm::new();
    for bad in ["", ".", "<?>", ";", "руссиш", "blah"] {
        assert_eq!(dec_repr(&mut t, bad), "NULL", "{bad}");
        assert_ne!(last_code(&mut t), 0);
        assert_eq!(dec_repr_assoc(&mut t, bad), "NULL", "{bad}");
    }
    assert_eq!(
        dec_repr(&mut t, r#"{ "test": { "foo": "bar" } }"#),
        r#"stdClass{"test" => stdClass{"foo" => "bar"}}"#
    );
    assert_eq!(
        dec_repr(&mut t, r#"{ "test": { "foo": "" } }"#),
        r#"stdClass{"test" => stdClass{"foo" => ""}}"#
    );
    assert_eq!(
        dec_repr(&mut t, r#"{ "": { "foo": "" } }"#),
        r#"stdClass{"" => stdClass{"foo" => ""}}"#
    );
    assert_eq!(
        dec_repr(&mut t, r#"{ "": { "": "" } }"#),
        r#"stdClass{"" => stdClass{"" => ""}}"#
    );
    assert_eq!(dec_repr(&mut t, r#"{ "": { "": "" }"#), "NULL");
    assert_eq!(dec_repr(&mut t, r#"{ "": "": "" } }"#), "NULL");

    // json_decode(NULL): the deprecation comes from the parameter parser.
    assert!(matches!(
        t.call("json_decode", vec![Value::Null]).unwrap(),
        Value::Null
    ));
    assert!(t.output().contains(
        "Deprecated: json_decode(): Passing null to parameter #1 ($json) of type string is deprecated in /t.php on line 3"
    ));
    assert_eq!(last_code(&mut t), 4);
}

/// json_decode_basic.phpt
#[test]
fn decode_basic_values() {
    let mut t = TestVm::new();
    let cases = [
        ("0", "0"),
        ("123", "123"),
        ("-123", "-123"),
        ("2147483647", "2147483647"),
        ("-2147483648", "-2147483648"),
        ("123.456", "float(123.456)"),
        ("1230", "1230"),
        ("true", "true"),
        ("false", "false"),
        ("null", "NULL"),
        (r#""abc""#, r#""abc""#),
        (r#""Hello World\r\n""#, r#""Hello World\r\n""#),
        ("[]", "[]"),
        ("[1,2,3,4,5]", "[0 => 1, 1 => 2, 2 => 3, 3 => 4, 4 => 5]"),
        (r#""""#, r#""""#),
    ];
    for (json, want) in cases {
        assert_eq!(dec_repr(&mut t, json), want, "{json}");
        assert_eq!(dec_repr_assoc(&mut t, json), want, "{json} (assoc)");
        assert_eq!(last_code(&mut t), 0);
    }
    assert_eq!(
        dec_repr(
            &mut t,
            r#"{"myInt":99,"myFloat":123.45,"myNull":null,"myBool":true,"myString":"Hello World"}"#
        ),
        r#"stdClass{"myInt" => 99, "myFloat" => float(123.45), "myNull" => NULL, "myBool" => true, "myString" => "Hello World"}"#
    );
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"Jan":31,"Feb":29}"#),
        r#"["Jan" => 31, "Feb" => 29]"#
    );
    assert_eq!(dec_repr(&mut t, "{}"), "stdClass{}");
    assert_eq!(dec_repr_assoc(&mut t, "{}"), "[]");
}

/// bug73991.phpt: `$associative` and JSON_OBJECT_AS_ARRAY.
#[test]
fn decode_assoc_semantics() {
    let mut t = TestVm::new();
    let json = br#"{"foo":"bar"}"#;
    let want_obj = r#"stdClass{"foo" => "bar"}"#;
    let want_arr = r#"["foo" => "bar"]"#;
    let r = |t: &mut TestVm, assoc: Option<bool>, flags: i64| {
        let v = decode_with(t, json, assoc, 512, flags);
        repr(&v)
    };
    assert_eq!(r(&mut t, Some(false), 0), want_obj);
    assert_eq!(r(&mut t, Some(true), 0), want_arr);
    assert_eq!(r(&mut t, None, 0), want_obj);
    assert_eq!(r(&mut t, None, OBJECT_AS_ARRAY as i64), want_arr);
    // A bool $associative overrides the flag.
    assert_eq!(r(&mut t, Some(false), OBJECT_AS_ARRAY as i64), want_obj);
    assert_eq!(r(&mut t, Some(true), OBJECT_AS_ARRAY as i64), want_arr);
}

/// bug41504.phpt, duplicate keys, numeric keys.
#[test]
fn decode_keys() {
    let mut t = TestVm::new();
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"":"value"}"#),
        r#"["" => "value"]"#
    );
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"":"value", "key":"value"}"#),
        r#"["" => "value", "key" => "value"]"#
    );
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"key":"value", "":"value"}"#),
        r#"["key" => "value", "" => "value"]"#
    );
    // Duplicate keys: the last value wins, at the first position.
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"a":1,"b":2,"a":3}"#),
        r#"["a" => 3, "b" => 2]"#
    );
    assert_eq!(
        dec_repr(&mut t, r#"{"a":1,"b":2,"a":3}"#),
        r#"stdClass{"a" => 3, "b" => 2}"#
    );
    // Numeric string keys: integers in arrays, strings in objects.
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"1":"a","01":"b","-5":"c","1.5":"d"}"#),
        r#"[1 => "a", "01" => "b", -5 => "c", "1.5" => "d"]"#
    );
    assert_eq!(
        dec_repr(&mut t, r#"{"1":"a","2":"b"}"#),
        r#"stdClass{"1" => "a", "2" => "b"}"#
    );
    // A NUL inside (not at the start of) a property name is fine.
    let v = decode_with(&mut t, br#"{"a\u0000b":1}"#, None, 512, 0);
    assert!(matches!(v, Value::Object(_)));
    assert_eq!(last_code(&mut t), 0);
}

/// bug68546.phpt: property names starting with NUL.
#[test]
fn decode_invalid_property_name() {
    let mut t = TestVm::new();
    assert_eq!(dec_repr(&mut t, r#"{"key": {"\u0000": "aa"}}"#), "NULL");
    assert_eq!(last_code(&mut t), 9);
    assert_eq!(
        dec_repr(&mut t, r#"[{"key1": 0, "\u1234": 1, "\u0000": 1}]"#),
        "NULL"
    );
    assert_eq!(
        last_error(&mut t),
        (
            9,
            "The decoded property name is invalid near location 1:37".to_string()
        )
    );
    // Arrays accept them, and so does json_validate().
    assert_eq!(
        dec_repr_assoc(&mut t, r#"{"\u0000a": 1}"#),
        "[\"\\0a\" => 1]"
    );
    assert!(validate(&mut t, br#"{"\u0000a": 1}"#, 512, 0).unwrap());
    // The error is detected when the member is reduced, so the reported token
    // is the last token of the value.
    assert_eq!(dec_repr(&mut t, "{\"\\u0000a\": [1, 2]  \n}"), "NULL");
    assert_eq!(
        last_error(&mut t).1,
        "The decoded property name is invalid near location 1:18"
    );
}

/// 008.phpt, bug63737.phpt, bug47644.phpt, bug45791.phpt
#[test]
fn decode_numbers() {
    let mut t = TestVm::new();
    let big = "123456789012345678901234567890";
    assert_eq!(
        dec_repr(&mut t, &format!(r#"{{"largenum":{big}}}"#)),
        r#"stdClass{"largenum" => float(1.2345678901234568E+29)}"#
    );
    let r = decode_with(
        &mut t,
        format!(r#"{{"largenum":{big}}}"#).as_bytes(),
        None,
        512,
        BIGINT_AS_STRING as i64,
    );
    assert_eq!(repr(&r), format!(r#"stdClass{{"largenum" => "{big}"}}"#));

    let d = |t: &mut TestVm, j: &str, f: i64| repr(&decode_with(t, j.as_bytes(), None, 512, f));
    let bs = BIGINT_AS_STRING as i64;
    assert_eq!(
        d(&mut t, &format!("-{big}"), 0),
        "float(-1.2345678901234568E+29)"
    );
    assert_eq!(d(&mut t, &format!("-{big}"), bs), format!("\"-{big}\""));
    // Floats are unaffected by the flag.
    assert_eq!(
        d(&mut t, &format!("{big}.1"), bs),
        "float(1.2345678901234568E+29)"
    );
    assert_eq!(
        d(&mut t, &format!("-{big}.1"), bs),
        "float(-1.2345678901234568E+29)"
    );

    // Integer range boundaries.
    assert_eq!(d(&mut t, "9223372036854775807", 0), "9223372036854775807");
    assert_eq!(
        d(&mut t, "9223372036854775808", 0),
        "float(9.223372036854776E+18)"
    );
    assert_eq!(
        d(&mut t, "9223372036854775808", bs),
        "\"9223372036854775808\""
    );
    assert_eq!(d(&mut t, "-9223372036854775808", 0), "-9223372036854775808");
    assert_eq!(
        d(&mut t, "-9223372036854775809", 0),
        "float(-9.223372036854776E+18)"
    );
    assert_eq!(
        d(&mut t, "-9223372036854775809", bs),
        "\"-9223372036854775809\""
    );
    assert_eq!(d(&mut t, "10000000000000000", 0), "10000000000000000");
    assert_eq!(d(&mut t, "-0", 0), "0");
    assert_eq!(d(&mut t, "-0.0", 0), "float(-0)");
    assert_eq!(d(&mut t, "0e0", 0), "float(0)");
    assert_eq!(d(&mut t, "1E+34", 0), "float(1.0E+34)");
    assert_eq!(d(&mut t, "0.123456789e-12", 0), "float(1.23456789E-13)");
    assert_eq!(d(&mut t, "23456789012E666", 0), "float(INF)");
    assert_eq!(d(&mut t, "-1e999", 0), "float(-INF)");
    assert_eq!(d(&mut t, "1.5e+3", 0), "float(1500)");
    assert_eq!(d(&mut t, "[2.1]", 0), "[0 => float(2.1)]");
    assert_eq!(d(&mut t, "[123,13452345]", 0), "[0 => 123, 1 => 13452345]");

    // Invalid numbers: leading zeros, hex, exponent without digits, lone sign.
    for bad in [
        "013", "0x14", "1e", "1.", "-", "--1", "+1", ".5", "1.2.3", "1e+", "- 1",
    ] {
        assert_eq!(d(&mut t, bad, 0), "NULL", "{bad}");
        assert_eq!(last_code(&mut t), 4, "{bad}");
    }
    // The scanner stops at the longest valid number.
    assert_eq!(d(&mut t, "[1,2]", 0), "[0 => 1, 1 => 2]");
}

/// bug42090.phpt, bug68817.phpt, escapes and surrogate pairs.
#[test]
fn decode_strings() {
    let mut t = TestVm::new();
    let d = |t: &mut TestVm, j: &[u8], f: i64| {
        let v = decode_with(t, j, None, 512, f);
        match v {
            Value::String(s) => Some(s.to_vec()),
            _ => None,
        }
    };
    assert_eq!(d(&mut t, br#""""#, 0), Some(vec![]));
    assert_eq!(d(&mut t, br#""..""."#, 0), None);
    assert_eq!(d(&mut t, br#"""#, 0), None);
    assert_eq!(d(&mut t, br#""""""#, 0), None);
    assert_eq!(
        d(&mut t, br#""\"\\\/\b\f\n\r\t""#, 0),
        Some(b"\"\\/\x08\x0c\n\r\t".to_vec())
    );
    assert_eq!(
        d(&mut t, br#""\u0041\u00e9\u20ac""#, 0),
        Some("A\u{e9}\u{20ac}".as_bytes().to_vec())
    );
    assert_eq!(
        d(&mut t, br#""\u0000\u007f\u0080\u07ff\u0800\uffff""#, 0),
        Some("\0\u{7f}\u{80}\u{7ff}\u{800}\u{ffff}".as_bytes().to_vec())
    );
    // Surrogate pairs (bug41067).
    assert_eq!(
        d(&mut t, br#""\ud834\udd00""#, 0),
        Some(b"\xf0\x9d\x84\x80".to_vec())
    );
    assert_eq!(
        d(&mut t, br#""\uD83D\uDE00""#, 0),
        Some("😀".as_bytes().to_vec())
    );
    // Hex digits are case-insensitive.
    assert_eq!(
        d(&mut t, br#""\u89AB\uCDEF\uabcd\uef4A""#, 0),
        Some("\u{89ab}\u{cdef}\u{abcd}\u{ef4a}".as_bytes().to_vec())
    );
    // Raw UTF-8 passes through.
    assert_eq!(
        d(&mut t, "\"héllo 😀\"".as_bytes(), 0),
        Some("héllo 😀".as_bytes().to_vec())
    );

    // Unpaired surrogates (bug62010.phpt).
    for bad in [
        &br#""\ud834""#[..],
        br#""\udd00""#,
        br#""\ud834\u0041""#,
        br#""\ud834x""#,
    ] {
        assert_eq!(d(&mut t, bad, 0), None);
        assert_eq!(last_code(&mut t), 10, "{}", String::from_utf8_lossy(bad));
    }
    assert_eq!(
        last_error(&mut t).1,
        "Single unpaired UTF-16 surrogate in unicode escape near location 1:1"
    );
    // Bad escapes are syntax errors; short \u escapes too.
    for bad in [
        &br#""\x""#[..],
        br#""\'""#,
        br#""\017""#,
        br#""\u12""#,
        br#""\uGGGG""#,
        br#""\u123""#,
        b"\"\\",
    ] {
        assert_eq!(d(&mut t, bad, 0), None);
        assert_eq!(last_code(&mut t), 4, "{}", String::from_utf8_lossy(bad));
    }
    // Control characters inside strings.
    assert_eq!(d(&mut t, b"\"\x01 invalid json\"", 0), None);
    assert_eq!(last_code(&mut t), 3);
    assert_eq!(d(&mut t, b"\"a\nb\"", 0), None);
    assert_eq!(last_code(&mut t), 3);
    assert_eq!(d(&mut t, b"\"\x00\"", 0), None);
    assert_eq!(last_code(&mut t), 3);
    assert_eq!(d(&mut t, b"\"\x7f\"", 0), Some(vec![0x7f]));
}

/// json_decode_invalid_utf8.phpt, bug69187.phpt
#[test]
fn decode_invalid_utf8() {
    let mut t = TestVm::new();
    let ignore = INVALID_UTF8_IGNORE as i64;
    let subst = INVALID_UTF8_SUBSTITUTE as i64;
    let hex = |v: &Value| -> String {
        match v {
            Value::String(s) => s.iter().map(|b| format!("{b:02x}")).collect(),
            other => panic!("{other:?}"),
        }
    };
    for (input, ignored, substituted) in [
        (&b"\"a\xb0b\""[..], "6162", "61efbfbd62"),
        (&b"\"a\xd0\xf2b\""[..], "6162", "61efbfbdefbfbd62"),
        (
            &b"\"\x61\xf0\x80\x80\x41\""[..],
            "6141",
            "61efbfbdefbfbdefbfbd41",
        ),
    ] {
        assert!(matches!(
            decode_with(&mut t, input, None, 512, 0),
            Value::Null
        ));
        assert_eq!(last_code(&mut t), 5);
        assert_eq!(
            hex(&decode_with(&mut t, input, Some(true), 512, ignore)),
            ignored
        );
        assert_eq!(
            hex(&decode_with(&mut t, input, Some(true), 512, subst)),
            substituted
        );
    }
    let input = b"[\"\xc1\xc1\",\"a\"]";
    let Value::Array(a) = decode_with(&mut t, input, Some(true), 512, ignore) else {
        panic!()
    };
    assert_eq!(hex(&a.get_int(0).unwrap().clone()), "");
    assert_eq!(hex(&a.get_int(1).unwrap().clone()), "61");
    let Value::Array(a) = decode_with(&mut t, input, Some(true), 512, subst) else {
        panic!()
    };
    assert_eq!(hex(&a.get_int(0).unwrap().clone()), "efbfbdefbfbd");

    // Outside of strings: UTF-8 errors vs syntax errors (bug69187).
    for (input, code) in [
        (&b"\xED\xA0\xB4"[..], 5),
        (b"\x00", 3),
        (b"\"\xED\xA0\xB4\"", 5),
        (b"\"\x00\"", 3),
        ("é".as_bytes(), 4), // valid UTF-8 outside a string: plain syntax error
        (b"\xff", 5),
    ] {
        decode_with(&mut t, input, None, 512, 0);
        assert_eq!(last_code(&mut t), code, "{input:?}");
    }
}

/// bug64874_part1.phpt, bug64874_part2.phpt
#[test]
fn decode_whitespace_and_case() {
    let mut t = TestVm::new();
    for ws1 in ["", " ", "\t", "\n", "\r", "\r\n", " \t\r\n "] {
        for ws2 in ["", " ", "\t", "\n", "\r", "\r\n"] {
            let j = format!("{ws1}true{ws2}");
            assert_eq!(dec_repr(&mut t, &j), "true", "{j:?}");
            assert_eq!(last_code(&mut t), 0);
        }
    }
    for (bad, good) in [
        ("True", "true"),
        ("False", "false"),
        ("Null", "null"),
        ("TRUE", "true"),
    ] {
        assert_eq!(dec_repr(&mut t, bad), "NULL");
        assert_eq!(last_code(&mut t), 4);
        assert_eq!(dec_repr(&mut t, &format!("[{bad}]")), "NULL");
        assert_eq!(last_code(&mut t), 4);
        dec_repr(&mut t, good);
        assert_eq!(last_code(&mut t), 0);
    }
    // Vertical tab and form feed are control characters, not whitespace.
    assert_eq!(dec_repr(&mut t, "\x0btrue"), "NULL");
    assert_eq!(last_code(&mut t), 3);
    assert_eq!(dec_repr(&mut t, "\x0ctrue"), "NULL");
    assert_eq!(last_code(&mut t), 3);
    // Whitespace only.
    assert_eq!(dec_repr(&mut t, "  \n "), "NULL");
    assert_eq!(last_code(&mut t), 4);
}

/// fail001.phpt
#[test]
fn decode_crockford_fail_cases() {
    let mut t = TestVm::new();
    let deep = format!("{}\"Too deep\"{}", "[".repeat(600), "]".repeat(600));
    let fails = [
        r#"["Unclosed array""#,
        r#"{unquoted_key: "keys must be quoted}"#,
        r#"["extra comma",]"#,
        r#"["double extra comma",,]"#,
        r#"[   , "<-- missing value"]"#,
        r#"["Comma after the close"],"#,
        r#"["Extra close"]]"#,
        r#"{"Extra comma": true,}"#,
        r#"{"Extra value after close": true} "misplaced quoted value""#,
        r#"{"Illegal expression": 1 + 2}"#,
        r#"{"Illegal invocation": alert()}"#,
        r#"{"Numbers cannot have leading zeroes": 013}"#,
        r#"{"Numbers cannot be hex": 0x14}"#,
        r#"["Illegal backslash escape: \x15"]"#,
        r#"["Illegal backslash escape: \'"]"#,
        r#"["Illegal backslash escape: \017"]"#,
        deep.as_str(),
        r#"{"Missing colon" null}"#,
        r#"{"Double colon":: null}"#,
        r#"{"Comma instead of colon", null}"#,
        r#"["Colon instead of comma": false]"#,
        r#"["Bad value", truth]"#,
        "['single quote']",
    ];
    for f in fails {
        assert_eq!(dec_repr(&mut t, f), "NULL", "{f}");
        assert_eq!(dec_repr_assoc(&mut t, f), "NULL", "{f}");
        assert_ne!(last_code(&mut t), 0, "{f}");
    }
    assert_eq!(
        dec_repr(
            &mut t,
            r#""A JSON payload should be an object or array, not a string.""#
        ),
        r#""A JSON payload should be an object or array, not a string.""#
    );
}

/// pass001.phpt: decode, re-encode (objects and arrays).
#[test]
fn decode_encode_pass1() {
    let mut t = TestVm::new();
    let test = "
[
    \"JSON Test Pattern pass1\",
    {\"object with 1 member\":[\"array with 1 element\"]},
    {},
    [],
    -42,
    true,
    false,
    null,
    {
        \"integer\": 1234567890,
        \"real\": -9876.543210,
        \"e\": 0.123456789e-12,
        \"E\": 1.234567890E+34,
        \"\":  23456789012E666,
        \"zero\": 0,
        \"one\": 1,
        \"space\": \" \",
        \"quote\": \"\\\"\",
        \"backslash\": \"\\\\\",
        \"controls\": \"\\b\\f\\n\\r\\t\",
        \"slash\": \"/ & \\/\",
        \"alpha\": \"abcdefghijklmnopqrstuvwyz\",
        \"ALPHA\": \"ABCDEFGHIJKLMNOPQRSTUVWYZ\",
        \"digit\": \"0123456789\",
        \"special\": \"`1~!@#$%^&*()_+-={':[,]}|;.</>?\",
        \"hex\": \"\\u0123\\u4567\\u89AB\\uCDEF\\uabcd\\uef4A\",
        \"true\": true,
        \"false\": false,
        \"null\": null,
        \"array\":[  ],
        \"object\":{  },
        \"address\": \"50 St. James Street\",
        \"url\": \"http://www.JSON.org/\",
        \"comment\": \"// /* <!-- --\",
        \"# -- --> */\": \" \",
        \" s p a c e d \" :[1,2 , 3

,

4 , 5        ,          6           ,7        ],
        \"compact\": [1,2,3,4,5,6,7],
        \"jsontext\": \"{\\\"object with 1 member\\\":[\\\"array with 1 element\\\"]}\",
        \"quotes\": \"&#34; \\u0022 %22 0x22 034 &#x22;\",
        \"\\/\\\\\\\"\\uCAFE\\uBABE\\uAB98\\uFCDE\\ubcda\\uef4A\\b\\f\\n\\r\\t`1~!@#$%^&*()_+-=[]{}|;:',./<>?\"
: \"A key can be any string\"
    },
    0.5 ,98.6
,
99.44
,

1066


,\"rosebud\"]
";
    let obj = decode(&mut t, test);
    let arr = decode_assoc(&mut t, test);
    let partial = PARTIAL_OUTPUT_ON_ERROR as i64;
    let obj_enc = encode(&mut t, obj, partial).unwrap();
    // `23456789012E666` decodes to INF, which cannot be encoded.
    assert_eq!(last_code(&mut t), 7);
    let arr_enc = encode(&mut t, arr, partial).unwrap();
    let common = r#""JSON Test Pattern pass1",{"object with 1 member":["array with 1 element"]},"#;
    let tail = r##"-42,true,false,null,{"integer":1234567890,"real":-9876.54321,"e":1.23456789e-13,"E":1.23456789e+34,"":0,"zero":0,"one":1,"space":" ","quote":"\"","backslash":"\\","controls":"\b\f\n\r\t","slash":"\/ & \/","alpha":"abcdefghijklmnopqrstuvwyz","ALPHA":"ABCDEFGHIJKLMNOPQRSTUVWYZ","digit":"0123456789","special":"`1~!@#$%^&*()_+-={':[,]}|;.<\/>?","hex":"\u0123\u4567\u89ab\ucdef\uabcd\uef4a","true":true,"false":false,"null":null,"array":[],"object":%OBJ%,"address":"50 St. James Street","url":"http:\/\/www.JSON.org\/","comment":"\/\/ \/* <!-- --","# -- --> *\/":" "," s p a c e d ":[1,2,3,4,5,6,7],"compact":[1,2,3,4,5,6,7],"jsontext":"{\"object with 1 member\":[\"array with 1 element\"]}","quotes":"&#34; \" %22 0x22 034 &#x22;","\/\\\"\ucafe\ubabe\uab98\ufcde\ubcda\uef4a\b\f\n\r\t`1~!@#$%^&*()_+-=[]{}|;:',.\/<>?":"A key can be any string"},0.5,98.6,99.44,1066,"rosebud"]"##;
    assert_eq!(
        obj_enc,
        format!("[{common}{{}},[],{}", tail.replace("%OBJ%", "{}"))
    );
    assert_eq!(
        arr_enc,
        format!("[{common}[],[],{}", tail.replace("%OBJ%", "[]"))
    );
    // Decode again and compare.
    let again = decode(&mut t, &obj_enc);
    let again_enc = encode(&mut t, again, partial).unwrap();
    assert_eq!(again_enc, obj_enc);
}

/// pass002.phpt: nesting of 19 is fine, and the object ids of nested objects
/// follow php-src's creation order (an object is created when its first member
/// is reduced).
#[test]
fn decode_nesting_and_object_ids() {
    let mut t = TestVm::new();
    let nested = format!("{}\"Not too deep\"{}", "[".repeat(19), "]".repeat(19));
    let v = decode(&mut t, &nested);
    assert_eq!(enc(&mut t, v), nested); // round trip
    assert_eq!(last_code(&mut t), 0);

    let Value::Object(outer) = decode(&mut t, r#"{"a":{"b":{"c":1}}}"#) else {
        panic!()
    };
    let Some(Value::Object(mid)) = outer.get_prop(b"a") else {
        panic!()
    };
    let Some(Value::Object(inner)) = mid.get_prop(b"b") else {
        panic!()
    };
    // Innermost object was created first, outermost last.
    assert!(inner.id() < mid.id() && mid.id() < outer.id());
    // An empty object is created when `}` is seen.
    let Value::Array(a) = decode_assoc(&mut t, r#"[{}, {"k": {}}]"#) else {
        panic!()
    };
    assert_eq!(a.len(), 2);
}

/// `json_decode()` depth handling: parser depth starts at 1.
#[test]
fn decode_depth() {
    let mut t = TestVm::new();
    let d =
        |t: &mut TestVm, j: &str, depth: i64| repr(&decode_with(t, j.as_bytes(), None, depth, 0));
    assert_eq!(d(&mut t, "[1]", 512), "[0 => 1]");
    assert_eq!(d(&mut t, "[[1]]", 2), "NULL");
    assert_eq!(
        last_error(&mut t),
        (
            1,
            "Maximum stack depth exceeded near location 1:2".to_string()
        )
    );
    assert_eq!(d(&mut t, "[[1]]", 3), "[0 => [0 => 1]]");
    assert_eq!(d(&mut t, "[1]", 2), "[0 => 1]");
    assert_eq!(d(&mut t, "[]", 1), "NULL");
    assert_eq!(d(&mut t, "1", 1), "1");
    assert_eq!(d(&mut t, "{}", 1), "NULL");
    assert_eq!(d(&mut t, "[]", 2), "[]");

    // Argument validation.
    let (class, msg) = thrown(t.call(
        "json_decode",
        vec![s(r#""abc""#), Value::Bool(true), Value::Int(-1)],
    ));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "json_decode(): Argument #3 ($depth) must be greater than 0"
    );
    let (_, msg) = thrown(t.call(
        "json_decode",
        vec![s("[]"), Value::Bool(false), Value::Int(0x100000000)],
    ));
    assert_eq!(
        msg,
        "json_decode(): Argument #3 ($depth) must be less than 2147483647"
    );
    // INT_MAX itself is accepted.
    assert_eq!(d(&mut t, "[]", i32::MAX as i64), "[]");
    // An empty string is a syntax error before the depth is validated.
    assert!(matches!(
        t.call("json_decode", vec![s(""), Value::Null, Value::Int(0)])
            .unwrap(),
        Value::Null
    ));
    assert_eq!(last_code(&mut t), 4);
}

/// bison's `YYMAXDEPTH` limit applies to very deep inputs.
#[test]
fn decode_bison_stack_limit() {
    with_big_stack(|| {
        let mut t = TestVm::new();
        // 4000 levels need 8002 stack entries: fine.
        let ok = format!("{}{}", "[".repeat(4000), "]".repeat(4000));
        let v = decode_with(&mut t, ok.as_bytes(), None, 100_000, 0);
        assert!(matches!(v, Value::Array(_)));
        assert_eq!(last_code(&mut t), 0);
        // 6000 levels exhaust the 10000-entry stack: reported as a syntax error.
        let deep = format!("{}{}", "[".repeat(6000), "]".repeat(6000));
        let v = decode_with(&mut t, deep.as_bytes(), None, 100_000, 0);
        assert!(matches!(v, Value::Null));
        assert_eq!(last_code(&mut t), 4);
        // json_validate() has the same limit.
        assert!(!validate(&mut t, deep.as_bytes(), 100_000, 0).unwrap());
        assert_eq!(last_code(&mut t), 4);
        assert!(validate(&mut t, ok.as_bytes(), 100_000, 0).unwrap());
    });
}

/// json_decode_exceptions.phpt, json_exceptions_error_clearing.phpt
#[test]
fn decode_throw_on_error() {
    let mut t = TestVm::new();
    let throw = THROW_ON_ERROR as i64;
    let r = t.call(
        "json_decode",
        vec![
            s("{"),
            Value::Bool(false),
            Value::Int(512),
            Value::Int(throw),
        ],
    );
    let Err(Flow::Exception(e)) = r else {
        panic!("expected exception")
    };
    assert_eq!(e.class().name, "JsonException");
    assert_eq!(Vm::exception_message(&e), "Syntax error near location 1:2");
    assert_eq!(e.get_prop(b"code").and_then(|v| v.as_int()), Some(4));

    // json_exceptions_error_clearing.phpt
    let mut t = TestVm::new();
    assert_eq!(last_code(&mut t), 0);
    decode_with(&mut t, b"\xFF", None, 512, 0);
    assert_eq!(last_code(&mut t), 5);
    let call = |t: &mut TestVm, j: &str| {
        thrown(t.call(
            "json_decode",
            vec![s(j), Value::Bool(false), Value::Int(512), Value::Int(throw)],
        ))
    };
    let (class, msg) = call(&mut t, "");
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("JsonException", "Syntax error")
    );
    assert_eq!(last_code(&mut t), 5);
    call(&mut t, "{");
    assert_eq!(last_code(&mut t), 5);
    // Success with THROW_ON_ERROR leaves the state alone too.
    let r = t
        .call(
            "json_decode",
            vec![
                s("[1]"),
                Value::Bool(false),
                Value::Int(512),
                Value::Int(throw),
            ],
        )
        .unwrap();
    assert!(matches!(r, Value::Array(_)));
    assert_eq!(last_code(&mut t), 5);
    // Without the flag a successful call resets it.
    decode(&mut t, "[1]");
    assert_eq!(last_code(&mut t), 0);

    // Errors with a depth/state mismatch message carry locations too.
    let (_, msg) = call(&mut t, "[1}");
    assert_eq!(
        msg,
        "State mismatch (invalid or malformed JSON) near location 1:3"
    );
}

/// bug54484.phpt, bug68938.phpt, bug69187.phpt
#[test]
fn decode_error_state_is_reset() {
    let mut t = TestVm::new();
    decode(&mut t, r#"{"test":"test"}"#);
    assert_eq!(last_code(&mut t), 0);
    decode(&mut t, "");
    assert_eq!(last_error(&mut t), (4, "Syntax error".to_string()));
    decode(&mut t, "invalid json");
    assert_eq!(last_code(&mut t), 4);
    decode(&mut t, "\"\u{1} invalid json\"");
    assert_eq!(last_code(&mut t), 3);
    decode(&mut t, "");
    assert_eq!(last_code(&mut t), 4);
    // json_decode(FALSE) / json_decode(0) / json_decode(1) / json_decode(TRUE)
    let r = t.call("json_decode", vec![Value::Bool(false)]).unwrap();
    assert!(matches!(r, Value::Null));
    assert_eq!(last_code(&mut t), 4);
    let r = t.call("json_decode", vec![Value::Int(0)]).unwrap();
    assert!(matches!(r, Value::Int(0)));
    assert_eq!(last_code(&mut t), 0);
    let r = t.call("json_decode", vec![Value::Bool(true)]).unwrap();
    assert!(matches!(r, Value::Int(1)));
    assert_eq!(last_code(&mut t), 0);
}

/// Error codes and locations from php-src's `json_last_error_msg_error_location_*.phpt`.
#[test]
fn error_locations_match_php_src() {
    let mut t = TestVm::new();
    let mut checked = 0;
    for &(file, input, depth, flags, valid, code, msg) in location_cases::LOCATION_CASES {
        let ok = validate(&mut t, input, depth, flags).unwrap();
        assert_eq!(
            ok,
            valid,
            "[{file}] validate {:?}",
            String::from_utf8_lossy(input)
        );
        assert_eq!(
            last_error(&mut t),
            (code, msg.to_string()),
            "[{file}] validate {:?}",
            String::from_utf8_lossy(input)
        );
        // json_decode() reports the same error (the property-name check does
        // not exist for json_validate(), but none of these cases hit it).
        decode_with(&mut t, input, None, depth, flags);
        assert_eq!(
            last_error(&mut t),
            (code, msg.to_string()),
            "[{file}] decode {:?}",
            String::from_utf8_lossy(input)
        );
        checked += 1;
    }
    assert!(checked > 100);

    // Cases built with PHP expressions in the originals.
    let long_value = format!("{{\"key\": \"{}\"", "a".repeat(1000));
    assert!(!validate(&mut t, long_value.as_bytes(), 512, 0).unwrap());
    assert_eq!(last_error(&mut t).1, "Syntax error near location 1:1011");
    let long_key = format!("{{\"{}\": \"value}}", "x".repeat(500));
    assert!(!validate(&mut t, long_key.as_bytes(), 512, 0).unwrap());
    assert_eq!(last_error(&mut t).1, "Syntax error near location 1:506");
    assert!(!validate(&mut t, b"{\"test\": \"\n\"}", 512, 0).unwrap());
    assert_eq!(
        last_error(&mut t).1,
        "Control character error, possibly incorrectly encoded near location 1:10"
    );
}

// ---------------------------------------------------------------------------
// json_validate
// ---------------------------------------------------------------------------

/// json_validate_001.phpt, json_validate_005.phpt
#[test]
fn validate_general() {
    let mut t = TestVm::new();
    for bad in [
        "",
        ".",
        "<?>",
        ";",
        "руссиш",
        "blah",
        r#"{ "": "": "" } }"#,
        r#"{ "": { "": "" }"#,
        r#"{ "test": {} "foo": "bar" }, "test2": {"foo" : "bar" }, "test2": {"foo" : "bar" } }"#,
        "-",
    ] {
        assert!(!validate(&mut t, bad.as_bytes(), 512, 0).unwrap(), "{bad}");
    }
    for good in [
        r#"{ "test": { "foo": "bar" } }"#,
        r#"{ "test": { "foo": "" } }"#,
        r#"{ "": { "foo": "" } }"#,
        r#"{ "": { "": "" } }"#,
        r#"{ "test": {"foo": "bar"}, "test2": {"foo" : "bar" }, "test2": {"foo" : "bar" } }"#,
        r#""""#,
        r#""string""#,
        "1234",
        "123.45",
        "-123",
        "null",
        "true",
        "false",
        "{}",
        "[]",
    ] {
        assert!(validate(&mut t, good.as_bytes(), 512, 0).unwrap(), "{good}");
        assert_eq!(last_code(&mut t), 0);
    }
}

/// json_validate_002.phpt, json_validate_003.phpt, json_validate_004.phpt
#[test]
fn validate_errors_and_flags() {
    let mut t = TestVm::new();
    assert!(!validate(&mut t, b"", 512, 0).unwrap());
    assert_eq!(last_error(&mut t), (4, "Syntax error".to_string()));
    assert!(!validate(&mut t, b"-", 512, 0).unwrap());
    assert_eq!(last_error(&mut t).1, "Syntax error near location 1:1");
    assert!(!validate(&mut t, b"", -1, 0).unwrap());
    assert_eq!(last_error(&mut t), (4, "Syntax error".to_string()));
    let two_keys = br#"{"key1":"value1", "key2":"value2"}"#;
    assert!(!validate(&mut t, two_keys, 1, 0).unwrap());
    assert_eq!(
        last_error(&mut t),
        (
            1,
            "Maximum stack depth exceeded near location 1:1".to_string()
        )
    );
    assert!(validate(&mut t, two_keys, 2, 0).unwrap());
    assert_eq!(last_code(&mut t), 0);

    // Argument errors do not touch the error state.
    let e = thrown(t.call("json_validate", vec![s("-"), Value::Int(0)]));
    assert_eq!(
        e,
        (
            "ValueError".to_string(),
            "json_validate(): Argument #2 ($depth) must be greater than 0".to_string()
        )
    );
    assert_eq!(last_code(&mut t), 0);
    for flags in [
        BIGINT_AS_STRING,
        BIGINT_AS_STRING | INVALID_UTF8_IGNORE,
        INVALID_UTF8_SUBSTITUTE,
    ] {
        let e = thrown(t.call(
            "json_validate",
            vec![s("-"), Value::Int(512), Value::Int(flags as i64)],
        ));
        assert_eq!(
            e.1,
            "json_validate(): Argument #3 ($flags) must be a valid flag (allowed flags: JSON_INVALID_UTF8_IGNORE)"
        );
        assert_eq!(last_code(&mut t), 0);
    }
    assert!(!validate(&mut t, b"-", 512, INVALID_UTF8_IGNORE as i64).unwrap());
    assert_eq!(last_error(&mut t).1, "Syntax error near location 1:1");
    assert!(validate(&mut t, b"{}", 512, INVALID_UTF8_IGNORE as i64).unwrap());
    assert_eq!(last_code(&mut t), 0);

    let e = thrown(t.call("json_validate", vec![s("-"), Value::Int(i64::MAX)]));
    assert_eq!(
        e.1,
        "json_validate(): Argument #2 ($depth) must be less than 2147483647"
    );
    assert_eq!(last_code(&mut t), 0);

    // Invalid UTF-8 (json_validate_004.phpt)
    let ignore = INVALID_UTF8_IGNORE as i64;
    for (input, loc) in [
        (&b"\"a\xb0b\""[..], "1:1"),
        (b"\"a\xd0\xf2b\"", "1:1"),
        (b"\"\x61\xf0\x80\x80\x41\"", "1:1"),
        (b"[\"\xc1\xc1\",\"a\"]", "1:2"),
    ] {
        assert!(!validate(&mut t, input, 512, 0).unwrap());
        assert_eq!(
            last_error(&mut t),
            (
                5,
                format!(
                    "Malformed UTF-8 characters, possibly incorrectly encoded near location {loc}"
                )
            )
        );
        assert!(validate(&mut t, input, 512, ignore).unwrap());
        assert_eq!(last_code(&mut t), 0);
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn constants_and_classes() {
    let t = TestVm::new();
    let c = |n: &str| t.vm.constant(n).unwrap_or_else(|| panic!("{n} missing"));
    let expected: &[(&str, i64)] = &[
        ("JSON_HEX_TAG", 1),
        ("JSON_HEX_AMP", 2),
        ("JSON_HEX_APOS", 4),
        ("JSON_HEX_QUOT", 8),
        ("JSON_FORCE_OBJECT", 16),
        ("JSON_NUMERIC_CHECK", 32),
        ("JSON_UNESCAPED_SLASHES", 64),
        ("JSON_PRETTY_PRINT", 128),
        ("JSON_UNESCAPED_UNICODE", 256),
        ("JSON_PARTIAL_OUTPUT_ON_ERROR", 512),
        ("JSON_PRESERVE_ZERO_FRACTION", 1024),
        ("JSON_UNESCAPED_LINE_TERMINATORS", 2048),
        ("JSON_OBJECT_AS_ARRAY", 1),
        ("JSON_BIGINT_AS_STRING", 2),
        ("JSON_INVALID_UTF8_IGNORE", 1048576),
        ("JSON_INVALID_UTF8_SUBSTITUTE", 2097152),
        ("JSON_THROW_ON_ERROR", 4194304),
        ("JSON_ERROR_NONE", 0),
        ("JSON_ERROR_DEPTH", 1),
        ("JSON_ERROR_STATE_MISMATCH", 2),
        ("JSON_ERROR_CTRL_CHAR", 3),
        ("JSON_ERROR_SYNTAX", 4),
        ("JSON_ERROR_UTF8", 5),
        ("JSON_ERROR_RECURSION", 6),
        ("JSON_ERROR_INF_OR_NAN", 7),
        ("JSON_ERROR_UNSUPPORTED_TYPE", 8),
        ("JSON_ERROR_INVALID_PROPERTY_NAME", 9),
        ("JSON_ERROR_UTF16", 10),
        ("JSON_ERROR_NON_BACKED_ENUM", 11),
    ];
    for (name, value) in expected {
        assert_eq!(c(name).as_int(), Some(*value), "{name}");
    }

    let iface = t.vm.find_class(b"JsonSerializable").unwrap();
    assert!(iface.is_interface());
    assert!(iface.is_internal());
    let m = iface.find_method(b"jsonSerialize").unwrap();
    assert!(m.has_flag(member_flags::ABSTRACT));
    assert_eq!(
        m.signature().return_type.as_ref().unwrap().to_type_string(),
        "mixed"
    );

    let exc = t.vm.find_class(b"JsonException").unwrap();
    assert!(exc.instance_of_name(b"Exception"));
    assert!(exc.instance_of_name(b"Throwable"));
    assert!(!exc.is_interface());
}

#[test]
fn error_messages() {
    for (code, msg) in [
        (ErrorCode::None, "No error"),
        (ErrorCode::Depth, "Maximum stack depth exceeded"),
        (
            ErrorCode::StateMismatch,
            "State mismatch (invalid or malformed JSON)",
        ),
        (
            ErrorCode::CtrlChar,
            "Control character error, possibly incorrectly encoded",
        ),
        (ErrorCode::Syntax, "Syntax error"),
        (
            ErrorCode::Utf8,
            "Malformed UTF-8 characters, possibly incorrectly encoded",
        ),
        (ErrorCode::Recursion, "Recursion detected"),
        (ErrorCode::InfOrNan, "Inf and NaN cannot be JSON encoded"),
        (ErrorCode::UnsupportedType, "Type is not supported"),
        (
            ErrorCode::InvalidPropertyName,
            "The decoded property name is invalid",
        ),
        (
            ErrorCode::Utf16,
            "Single unpaired UTF-16 surrogate in unicode escape",
        ),
        (
            ErrorCode::NonBackedEnum,
            "Non-backed enums have no default serialization",
        ),
    ] {
        assert_eq!(code.message(), msg);
    }
}
