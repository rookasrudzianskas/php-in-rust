//! Unit tests ported from `ext/ctype/tests/*.phpt`.

use php_runtime::Value;

use crate::testing::TestVm;

const FUNCS: [&str; 11] = [
    "ctype_lower",
    "ctype_upper",
    "ctype_alpha",
    "ctype_digit",
    "ctype_alnum",
    "ctype_cntrl",
    "ctype_graph",
    "ctype_print",
    "ctype_punct",
    "ctype_space",
    "ctype_xdigit",
];

fn call(t: &mut TestVm, f: &str, v: Value) -> bool {
    match t.call(f, vec![v]).expect("no exception") {
        Value::Bool(b) => b,
        other => panic!("{f} returned {other:?}"),
    }
}

/// 001.phpt: ctype on integers 0..255 (deprecations are not shown by that test).
#[test]
fn ints_0_to_255() {
    let expected = [26, 26, 52, 10, 62, 33, 94, 95, 32, 6, 22];
    let mut t = TestVm::new();
    for (f, want) in FUNCS.iter().zip(expected) {
        let n = (0..256).filter(|&a| call(&mut t, f, Value::Int(a))).count();
        assert_eq!(n, want, "{f}");
    }
}

/// 002.phpt: ctype on strings (single char, three chars, mixed with other chars).
#[test]
fn strings_0_to_127() {
    let expected = [
        (26, 26, 0),
        (26, 26, 0),
        (52, 52, 0),
        (10, 10, 0),
        (62, 62, 0),
        (33, 33, 0),
        (94, 94, 94),
        (95, 95, 95),
        (32, 32, 0),
        (6, 6, 0),
        (22, 22, 0),
    ];
    let mut t = TestVm::new();
    for (f, want) in FUNCS.iter().zip(expected) {
        let (mut n1, mut n2, mut n3) = (0, 0, 0);
        for a in 0..128u8 {
            let c = [a];
            if call(&mut t, f, Value::bytes(&c)) {
                n1 += 1;
            }
            if call(&mut t, f, Value::bytes(&[a, a, a])) {
                n2 += 1;
            }
            let mut s = b"1-".to_vec();
            s.extend_from_slice(&[a, a, a]);
            s.extend_from_slice(b"-x");
            if call(&mut t, f, Value::bytes(&s)) {
                n3 += 1;
            }
        }
        assert_eq!((n1, n2, n3), want, "{f}");
    }
}

#[test]
fn empty_string_and_high_bytes() {
    let mut t = TestVm::new();
    for f in FUNCS {
        assert!(!call(&mut t, f, Value::str("")), "{f}");
        // C locale: bytes >= 0x80 never belong to a class.
        assert!(!call(&mut t, f, Value::bytes(b"\xe9")), "{f}");
        assert!(!call(&mut t, f, Value::bytes(b"\xff")), "{f}");
    }
}

#[test]
fn string_basics() {
    let mut t = TestVm::new();
    assert!(call(&mut t, "ctype_alnum", Value::str("AbCd1zyZ9")));
    assert!(!call(&mut t, "ctype_alnum", Value::str("foo!#$bar")));
    assert!(call(&mut t, "ctype_alpha", Value::str("KjgWZC")));
    assert!(!call(&mut t, "ctype_alpha", Value::str("arf12")));
    assert!(call(&mut t, "ctype_cntrl", Value::str("\n\t\r")));
    assert!(call(&mut t, "ctype_digit", Value::str("0123456789")));
    assert!(!call(&mut t, "ctype_digit", Value::str("10.5")));
    assert!(call(&mut t, "ctype_lower", Value::str("abc")));
    assert!(!call(&mut t, "ctype_lower", Value::str("aBc")));
    assert!(call(&mut t, "ctype_graph", Value::str("arf12!")));
    assert!(!call(&mut t, "ctype_graph", Value::str("a b")));
    assert!(call(&mut t, "ctype_print", Value::str("a b")));
    assert!(call(&mut t, "ctype_punct", Value::str("!#$%&")));
    assert!(call(&mut t, "ctype_space", Value::str(" \t\n\r\x0b\x0c")));
    assert!(call(&mut t, "ctype_upper", Value::str("ABC")));
    assert!(call(&mut t, "ctype_xdigit", Value::str("AB10BC99")));
    assert!(!call(&mut t, "ctype_xdigit", Value::str("AR1012")));
    assert_eq!(t.output(), "");
}

/// ctype_*_variation*.phpt: integer arguments, ranges and the deprecation.
#[test]
fn int_deprecation_and_ranges() {
    let mut t = TestVm::new();
    assert!(!call(&mut t, "ctype_alnum", Value::Int(0)));
    assert_eq!(
        t.output(),
        "\nDeprecated: ctype_alnum(): Argument of type int will be interpreted as string in the future in /t.php on line 3\n"
    );

    // 48 is '0'.
    assert!(call(&mut t, "ctype_digit", Value::Int(48)));
    t.output();
    // > 255: allow_digits (digit/alnum/xdigit/graph/print).
    assert!(call(&mut t, "ctype_digit", Value::Int(256)));
    assert!(call(&mut t, "ctype_alnum", Value::Int(12345)));
    assert!(call(&mut t, "ctype_xdigit", Value::Int(12345)));
    assert!(call(&mut t, "ctype_graph", Value::Int(12345)));
    assert!(call(&mut t, "ctype_print", Value::Int(12345)));
    assert!(!call(&mut t, "ctype_alpha", Value::Int(12345)));
    assert!(!call(&mut t, "ctype_punct", Value::Int(12345)));
    assert!(!call(&mut t, "ctype_space", Value::Int(12345)));
    // Below -128: allow_minus (graph/print only).
    assert!(call(&mut t, "ctype_graph", Value::Int(-2345)));
    assert!(call(&mut t, "ctype_print", Value::Int(-2345)));
    assert!(!call(&mut t, "ctype_digit", Value::Int(-2345)));
    assert!(!call(&mut t, "ctype_alnum", Value::Int(-2345)));
    // -128..-1 map to 128..255 (never in a class in the C locale).
    assert!(!call(&mut t, "ctype_print", Value::Int(-1)));
    assert!(!call(&mut t, "ctype_graph", Value::Int(-128)));
    // -129 is below the wrap range.
    assert!(call(&mut t, "ctype_graph", Value::Int(-129)));
    t.output();
}

#[test]
fn other_types_are_false_with_deprecation() {
    let mut t = TestVm::new();
    let cases: Vec<(Value, &str)> = vec![
        (Value::Float(10.5), "float"),
        (Value::Null, "null"),
        (Value::Bool(true), "bool"),
        (Value::Bool(false), "bool"),
        (Value::empty_array(), "array"),
    ];
    for (v, ty) in cases {
        assert!(!call(&mut t, "ctype_alnum", v));
        assert_eq!(
            t.output(),
            format!(
                "\nDeprecated: ctype_alnum(): Argument of type {ty} will be interpreted as string in the future in /t.php on line 3\n"
            )
        );
    }
}

#[test]
fn object_argument_reports_class_name() {
    let mut t = TestVm::new();
    let std = t.vm.find_class(b"stdClass").unwrap();
    let obj = Value::Object(php_runtime::ObjRef::new(std));
    assert!(!call(&mut t, "ctype_digit", obj));
    assert_eq!(
        t.output(),
        "\nDeprecated: ctype_digit(): Argument of type stdClass will be interpreted as string in the future in /t.php on line 3\n"
    );
}

#[test]
fn arity_errors() {
    let mut t = TestVm::new();
    let r = t.call("ctype_alpha", vec![]);
    let (class, msg) = crate::testing::thrown(r);
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "ctype_alpha() expects exactly 1 argument, 0 given");
}
