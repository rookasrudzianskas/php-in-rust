//! Scalars, arrays, floats and references.

use php_runtime::value::ResRef;
use php_runtime::{PhpArray, PhpRef, Value};

use super::harness::*;

#[test]
fn serialize_scalars_001() {
    // ext/standard/tests/serialize/001.phpt (serialize_precision=100)
    let mut t = TestVm::new();
    t.vm.ini_set("serialize_precision", "100");
    let cases: Vec<(Value, &str)> = vec![
        (Value::Null, "N;"),
        (Value::Bool(true), "b:1;"),
        (Value::Bool(false), "b:0;"),
        (Value::Int(1), "i:1;"),
        (Value::Int(0), "i:0;"),
        (Value::Int(-1), "i:-1;"),
        (Value::Int(2147483647), "i:2147483647;"),
        (Value::Int(-2147483647), "i:-2147483647;"),
        (
            Value::Float(1.123456789),
            "d:1.123456789000000011213842299184761941432952880859375;",
        ),
        (Value::Float(1.0), "d:1;"),
        (Value::Float(0.0), "d:0;"),
        (Value::Float(-1.0), "d:-1;"),
        (
            Value::Float(-1.123456789),
            "d:-1.123456789000000011213842299184761941432952880859375;",
        ),
        (s("hallo"), "s:5:\"hallo\";"),
        (
            list(vec![
                Value::Int(1),
                Value::Float(1.1),
                s("hallo"),
                Value::Null,
                Value::Bool(true),
                Value::empty_array(),
            ]),
            "a:6:{i:0;i:1;i:1;d:1.100000000000000088817841970012523233890533447265625;i:2;s:5:\"hallo\";i:3;N;i:4;b:1;i:5;a:0:{}}",
        ),
    ];
    for (v, want) in cases {
        assert_eq!(ser(&mut t, v), want);
    }
}

#[test]
fn reference_round_trip_001() {
    // 001.phpt: $a["b"] = &$a["a"]
    let mut t = TestVm::new();
    let r = PhpRef::new(s("test"));
    let mut a = PhpArray::new();
    a.insert_str("a", Value::Ref(r.clone()));
    a.insert_str("b", Value::Ref(r));
    let a = Value::array(a);
    let expected =
        "array(2) {\n  [\"a\"]=>\n  &string(4) \"test\"\n  [\"b\"]=>\n  &string(4) \"test\"\n}\n";
    assert_eq!(dump(&t, &a), expected);
    let data = ser(&mut t, a);
    assert_eq!(data, "a:2:{s:1:\"a\";s:4:\"test\";s:1:\"b\";R:2;}");
    let back = unser(&mut t, data.as_bytes());
    assert_eq!(dump(&t, &back), expected);
    // The two elements really alias.
    let arr = back.as_array().unwrap();
    let (Some(Value::Ref(x)), Some(Value::Ref(y))) = (arr.get_str(b"a"), arr.get_str(b"b")) else {
        panic!("expected references");
    };
    assert!(PhpRef::ptr_eq(x, y));
}

#[test]
fn references_are_not_counted_twice() {
    // counting_of_references.phpt
    let mut t = TestVm::new();
    let r1 = PhpRef::new(Value::Int(1));
    let r2 = PhpRef::new(Value::Int(2));
    let arr = list(vec![
        Value::Ref(r1.clone()),
        Value::Ref(r1),
        Value::Ref(r2.clone()),
        Value::Ref(r2),
    ]);
    assert_eq!(ser(&mut t, arr), "a:4:{i:0;i:1;i:1;R:2;i:2;i:2;i:3;R:3;}");
}

#[test]
fn unshared_references_are_values() {
    // A reference with refcount 1 is serialized as its value.
    let mut t = TestVm::new();
    let arr = list(vec![Value::Ref(PhpRef::new(Value::Int(7))), Value::Int(8)]);
    assert_eq!(ser(&mut t, arr), "a:2:{i:0;i:7;i:1;i:8;}");
}

#[test]
fn array_containing_reference_to_itself() {
    // $a = [1]; $a[] = &$a;  (the outer variable is the reference)
    let mut t = TestVm::new();
    let r = PhpRef::new(Value::Null);
    let mut a = PhpArray::new();
    a.append(Value::Int(1)).unwrap();
    a.append(Value::Ref(r.clone())).unwrap();
    r.set(Value::array(a));
    let s1 = ser(&mut t, r.get());
    assert_eq!(s1, "a:2:{i:0;i:1;i:1;a:2:{i:0;i:1;i:1;R:3;}}");
    // Unserializing a self reference yields an array whose element refers
    // back to the array itself (once the result is unwrapped, only the
    // element holds the reference).
    let v = unser(&mut t, b"a:2:{i:0;i:1;i:1;R:1;}");
    expectf(
        &dump(&t, &v),
        "array(2) {\n  [0]=>\n  int(1)\n  [1]=>\n  *RECURSION*\n}\n",
    );
}

#[test]
fn objects_in_arrays_get_back_references() {
    // __serialize_005.phpt numbering with plain objects; r: vs R:
    let mut t = TestVm::new();
    let o = Value::Object(new_obj(&mut t, "stdClass"));
    let arr = list(vec![o.clone(), o.clone(), list(vec![o])]);
    assert_eq!(
        ser(&mut t, arr),
        "a:3:{i:0;O:8:\"stdClass\":0:{}i:1;r:2;i:2;a:1:{i:0;r:2;}}"
    );
    // A reference to an object counts as the object itself.
    let o = Value::Object(new_obj(&mut t, "stdClass"));
    let r = PhpRef::new(o.clone());
    let arr = list(vec![o, Value::Ref(r.clone()), Value::Ref(r)]);
    assert_eq!(
        ser(&mut t, arr),
        "a:3:{i:0;O:8:\"stdClass\":0:{}i:1;R:2;i:2;R:2;}"
    );
    let v = unser(&mut t, b"a:3:{i:0;O:8:\"stdClass\":0:{}i:1;r:2;i:2;R:2;}");
    let a = v.as_array().unwrap();
    let first = a.get_int(0).unwrap().unref();
    let second = a.get_int(1).unwrap().unref();
    assert!(first.identical(&second));
    assert!(a.get_int(2).unwrap().is_ref());
}

#[test]
fn floats_and_precision() {
    // 003.phpt, 004.phpt, bug24063.phpt, bug23298.phpt, precision.phpt,
    // serialization_precision_001.phpt
    let mut t = TestVm::new();
    t.vm.ini_set("serialize_precision", "100");
    assert_eq!(ser(&mut t, Value::Float(1e2)), "d:100;");
    assert_eq!(
        ser(&mut t, list(vec![Value::Float(4.5)])),
        "a:1:{i:0;d:4.5;}"
    );
    let s1 = ser(&mut t, Value::Float(85.29e-23));
    assert!(s1.starts_with("d:8.52") && s1.ends_with("E-22;"), "{s1}");
    #[allow(clippy::excessive_precision)]
    let f = 1.428571428571428647642857142;
    let back = round_trip(&mut t, Value::Float(f));
    assert!(back.identical(&Value::Float(f)));
    let mut v = 1.0f64;
    for _ in 1..10 {
        v /= 10.0;
        let back = round_trip(&mut t, Value::Float(v));
        assert!(back.identical(&Value::Float(v)));
    }

    t.vm.ini_set("serialize_precision", "-1");
    for bits in [
        0x0000000000000000u64,
        0x3f2a36e2eb1c432d,
        0x3f2a36e2eb1c432e,
        0x0010000000000000,
        0x0010000000000001,
        0x7fefffffffffffff,
        0x7feffffffffffffe,
        0x0000000000000001,
        0x0000000000000002,
        0x000ffffffffffff0,
        0x000ffffffffffff0 - 1,
        0x7f0f000000000000,
        0xff0f000000000000,
    ] {
        let d = f64::from_bits(bits);
        let back = round_trip(&mut t, Value::Float(d));
        assert_eq!(back.as_float().unwrap().to_bits(), bits);
    }
    assert_eq!(ser(&mut t, Value::Float(0.1)), "d:0.1;");
    assert_eq!(ser(&mut t, Value::Float(f64::INFINITY)), "d:INF;");
    assert_eq!(ser(&mut t, Value::Float(f64::NEG_INFINITY)), "d:-INF;");
    assert_eq!(ser(&mut t, Value::Float(f64::NAN)), "d:NAN;");
    assert_eq!(ser(&mut t, Value::Float(-0.0)), "d:-0;");
    assert_eq!(ser(&mut t, Value::Float(1e25)), "d:1.0E+25;");
    t.vm.ini_set("serialize_precision", "10");
    assert_eq!(ser(&mut t, Value::Float(0.1)), "d:0.1;");
    t.vm.ini_set("serialize_precision", "2");
    // snprintf(buf, ndigit + 1, "INF") truncates.
    assert_eq!(ser(&mut t, Value::Float(f64::INFINITY)), "d:IN;");
    assert_eq!(t.output(), "");
}

#[test]
fn unserialize_floats() {
    // 003.phpt, bug74300.phpt
    let mut t = TestVm::new();
    let f = |t: &mut TestVm, s: &str| unser(t, s.as_bytes()).as_float().unwrap();
    assert_eq!(f(&mut t, "d:2e+2;"), 200.0);
    assert_eq!(f(&mut t, "d:5.2e25;"), 5.2e25);
    assert_eq!(f(&mut t, "d:.5;"), 0.5);
    assert_eq!(f(&mut t, "d:5.;"), 5.0);
    assert_eq!(f(&mut t, "d:-.5E-3;"), -0.0005);
    assert_eq!(f(&mut t, "d:+1;"), 1.0);
    assert_eq!(f(&mut t, "d:7;"), 7.0);
    assert!(f(&mut t, "d:NAN;").is_nan());
    assert_eq!(f(&mut t, "d:INF;"), f64::INFINITY);
    assert_eq!(f(&mut t, "d:-INF;"), f64::NEG_INFINITY);
    assert_eq!(t.output(), "");
    let v = unser(&mut t, b"d:2e++2;");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Error at offset 0 of 8 bytes in %s on line %d",
    );
    for bad in ["d:1e;", "d:.;", "d:+INF;", "d:1.5", "d:e5;"] {
        assert!(
            unser(&mut t, bad.as_bytes()).identical(&Value::Bool(false)),
            "{bad}"
        );
    }
}

#[test]
fn bug25378_simple_values() {
    // 002.phpt
    let mut t = TestVm::new();
    let out = [
        ("b:0;", "bool(false)\n"),
        ("b:1;", "bool(true)\n"),
        ("i:823;", "int(823)\n"),
        ("s:0:\"\";", "string(0) \"\"\n"),
        ("s:3:\"foo\";", "string(3) \"foo\"\n"),
        (
            "a:1:{i:0;s:2:\"12\";}",
            "array(1) {\n  [0]=>\n  string(2) \"12\"\n}\n",
        ),
        (
            "a:2:{i:0;a:0:{}i:1;a:0:{}}",
            "array(2) {\n  [0]=>\n  array(0) {\n  }\n  [1]=>\n  array(0) {\n  }\n}\n",
        ),
        (
            "a:3:{i:0;s:3:\"foo\";i:1;s:3:\"bar\";i:2;s:3:\"baz\";}",
            "array(3) {\n  [0]=>\n  string(3) \"foo\"\n  [1]=>\n  string(3) \"bar\"\n  [2]=>\n  string(3) \"baz\"\n}\n",
        ),
        ("O:8:\"stdClass\":0:{}", "object(stdClass)#%d (0) {\n}\n"),
    ];
    for (data, want) in out {
        let v = unser(&mut t, data.as_bytes());
        expectf(&dump_out(&t, &v), want);
    }
}

#[test]
fn integers() {
    // bug31442.phpt, bug46882.phpt, unserialize_neg_iv_edge_cases.phpt
    let mut t = TestVm::new();
    let v = round_trip(&mut t, Value::Int(2147483648));
    assert!(v.identical(&Value::Int(2147483648)));
    assert!(unser(&mut t, b"i:5000000000;").identical(&Value::Int(5000000000)));
    assert!(unser(&mut t, b"i:-9223372036854775808;").identical(&Value::Int(i64::MIN)));
    assert!(unser(&mut t, b"i:-0;").identical(&Value::Int(0)));
    assert!(unser(&mut t, b"i:+5;").identical(&Value::Int(5)));
    assert!(unser(&mut t, b"i:007;").identical(&Value::Int(7)));
    assert_eq!(t.output(), "");
    // Overflow saturates with a warning.
    let v = unser(&mut t, b"i:9223372036854775808;");
    assert!(v.identical(&Value::Int(i64::MAX)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Numerical result out of range in %s on line %d",
    );
    let v = unser(&mut t, b"i:-99999999999999999999;");
    assert!(v.identical(&Value::Int(i64::MIN)));
    assert!(t.output().contains("Numerical result out of range"));
}

#[test]
fn numeric_string_keys() {
    // bug43614.phpt
    let mut t = TestVm::new();
    let v = unser(&mut t, b"a:2:{s:2:\"10\";i:1;s:2:\"01\";i:2;}");
    assert_eq!(
        dump(&t, &v),
        "array(2) {\n  [10]=>\n  int(1)\n  [\"01\"]=>\n  int(2)\n}\n"
    );
}

#[test]
fn duplicate_keys_overwrite() {
    let mut t = TestVm::new();
    let v = unser(&mut t, b"a:2:{i:0;i:1;i:0;i:2;}");
    assert_eq!(dump(&t, &v), "array(1) {\n  [0]=>\n  int(2)\n}\n");
    // A reference to the slot that is being overwritten is rejected.
    let v = unser(&mut t, b"a:2:{i:0;i:1;i:0;R:2;}");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Error at offset 21 of 22 bytes in %s on line %d",
    );
}

#[test]
fn exotic_bytes() {
    // 006.phpt
    let mut t = TestVm::new();
    let k = "\u{e4}\u{f6}\u{fc}";
    let arr = assoc(vec![(k, s(k))]);
    let data = ser_bytes(&mut t, arr.clone());
    let v = unser(&mut t, &data);
    assert_eq!(dump(&t, &v), dump(&t, &arr));
    let bin = Value::bytes(b"\x00\xff\"\\");
    let data = ser_bytes(&mut t, bin.clone());
    assert_eq!(data, b"s:4:\"\x00\xff\"\\\";");
    assert!(unser(&mut t, &data).identical(&bin));
}

#[test]
fn resources_serialize_as_zero() {
    // serialization_resources_001.phpt
    let mut t = TestVm::new();
    let r = ResRef::new("stream", Box::new(()));
    assert_eq!(ser(&mut t, Value::Resource(r)), "i:0;");
}

#[test]
fn uppercase_s_format() {
    // unserialize_uppercase_s.phpt, unserializeS.phpt
    let mut t = TestVm::new();
    let v = unser(&mut t, b"S:1:\"e\";");
    expectf(
        &dump_out(&t, &v),
        "Deprecated: unserialize(): Unserializing the 'S' format is deprecated in %s on line %d\nstring(1) \"e\"",
    );
    let v = unser(&mut t, b"S:1:\"\\65\";");
    expectf(
        &dump_out(&t, &v),
        "Deprecated: unserialize(): Unserializing the 'S' format is deprecated in %s on line %d\nstring(1) \"e\"",
    );
    let data = format!("S:300:\"{}\"", "\\61".repeat(100));
    t.vm.ini_set("error_reporting", "0");
    assert!(unser(&mut t, data.as_bytes()).identical(&Value::Bool(false)));
}

#[test]
fn nested_arrays_round_trip() {
    // serialization_arrays_00x.phpt
    let mut t = TestVm::new();
    let inner = list(vec![s("a"), Value::Int(2)]);
    let mut a = PhpArray::new();
    a.insert_str("x", inner.clone());
    a.insert(php_runtime::ArrayKey::Int(-5), Value::Bool(false));
    a.insert_str("y", inner);
    let a = Value::array(a);
    let data = ser(&mut t, a.clone());
    assert_eq!(
        data,
        "a:3:{s:1:\"x\";a:2:{i:0;s:1:\"a\";i:1;i:2;}i:-5;b:0;s:1:\"y\";a:2:{i:0;s:1:\"a\";i:1;i:2;}}"
    );
    let back = unser(&mut t, data.as_bytes());
    assert_eq!(dump(&t, &back), dump(&t, &a));
}
