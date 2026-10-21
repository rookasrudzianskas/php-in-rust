//! Conversions (`zend_operators.c`): (bool), (int), (float), (string), (array), (object).

use super::*;
use crate::value::PhpArray;

#[test]
fn to_bool() {
    let mut t = TestVm::new();
    let cases: Vec<(Value, bool)> = vec![
        (Value::Null, false),
        (false.into(), false),
        (true.into(), true),
        (0.into(), false),
        ((-1).into(), true),
        (0.0.into(), false),
        ((-0.0).into(), false),
        (0.1.into(), true),
        (s(""), false),
        (s("0"), false),
        (s("0.0"), true),
        (s(" "), true),
        (s("00"), true),
        (s("false"), true),
        (Value::empty_array(), false),
        (Value::array(PhpArray::from_values(vec![Value::Null])), true),
    ];
    for (v, want) in cases {
        assert_eq!(t.vm.to_bool(&v).unwrap(), want, "{v:?}");
    }
    assert_eq!(t.output(), "");
    assert!(t.vm.to_bool(&Value::Float(f64::NAN)).unwrap());
    assert_eq!(
        t.output(),
        "\nWarning: unexpected NAN value was coerced to bool in /t.php on line 3\n"
    );
    let o = t.vm.cast_to_object(&Value::Null).unwrap();
    assert!(t.vm.to_bool(&Value::Object(o)).unwrap());
}

#[test]
fn to_int_casts() {
    let mut t = TestVm::new();
    let cases: Vec<(Value, i64)> = vec![
        (Value::Null, 0),
        (true.into(), 1),
        (1.9.into(), 1),
        ((-1.9).into(), -1),
        (s("12abc"), 12),
        (s("abc"), 0),
        (s(" 12"), 12),
        (s("12 "), 12),
        (s("1e3"), 1000),
        (s("1.9e1"), 19),
        (s("0x1A"), 0),
        (s("012"), 12),
        (s("-0"), 0),
        (s("9999999999999999999999"), i64::MAX),
        (s("-9999999999999999999999"), i64::MIN),
        (s("1e1000"), 0),
        (s("NAN"), 0),
        (Value::empty_array(), 0),
        (Value::array(PhpArray::from_values(vec![0.into()])), 1),
        ((i64::MAX as f64 - 1024.0).into(), 9223372036854774784),
        ((-9223372036854775808.0f64).into(), i64::MIN),
    ];
    for (v, want) in cases {
        assert_eq!(t.vm.to_int(&v).unwrap(), want, "{v:?}");
    }
    assert_eq!(t.output(), "");
    // Out-of-range floats: modular arithmetic with a warning (PHP 8.5).
    assert_eq!(
        t.vm.to_int(&Value::Float(1e19)).unwrap(),
        -8446744073709551616
    );
    assert_eq!(
        t.output(),
        "\nWarning: The float 1.0E+19 is not representable as an int, cast occurred in /t.php on line 3\n"
    );
    assert_eq!(
        t.vm.to_int(&Value::Float(9223372036854775808.0)).unwrap(),
        i64::MIN
    );
    assert!(
        t.output()
            .contains("The float 9.223372036854776E+18 is not representable")
    );
    assert_eq!(t.vm.to_int(&Value::Float(f64::NAN)).unwrap(), 0);
    assert!(
        t.output()
            .contains("The float NAN is not representable as an int")
    );
    assert_eq!(t.vm.to_int(&Value::Float(f64::NEG_INFINITY)).unwrap(), 0);
    assert!(
        t.output()
            .contains("The float -INF is not representable as an int")
    );
    assert_eq!(t.vm.to_int(&Value::Float(1e121)).unwrap(), 0);
    assert!(
        t.output()
            .contains("The float 1.0E+121 is not representable as an int")
    );
    assert_eq!(
        t.vm.to_int(&Value::Float(36893488147419111424.0)).unwrap(),
        8192
    );
}

#[test]
fn to_float_casts() {
    let mut t = TestVm::new();
    let cases: Vec<(Value, f64)> = vec![
        (s("1.5abc"), 1.5),
        (s("abc"), 0.0),
        (s(".5"), 0.5),
        (s(" 2e3x"), 2000.0),
        (s("0x10"), 0.0),
        (s("1e999"), f64::INFINITY),
        (s("-1e999"), f64::NEG_INFINITY),
        (true.into(), 1.0),
        (Value::Null, 0.0),
        (7.into(), 7.0),
    ];
    for (v, want) in cases {
        assert_eq!(t.vm.to_float(&v).unwrap(), want, "{v:?}");
    }
}

#[test]
fn to_string_conversions() {
    let mut t = TestVm::new();
    let cases: Vec<(Value, &str)> = vec![
        (Value::Null, ""),
        (true.into(), "1"),
        (false.into(), ""),
        (0.into(), "0"),
        ((-42).into(), "-42"),
        (i64::MIN.into(), "-9223372036854775808"),
        (1.0.into(), "1"),
        ((-0.0).into(), "-0"),
        (0.1.into(), "0.1"),
        ((0.1 + 0.2).into(), "0.3"),
        (1e100.into(), "1.0E+100"),
        (1e15.into(), "1.0E+15"),
        (123456789012345.0.into(), "1.2345678901234E+14"),
        (0.00001.into(), "1.0E-5"),
        ((-1.5).into(), "-1.5"),
        (f64::INFINITY.into(), "INF"),
        (f64::NEG_INFINITY.into(), "-INF"),
        ((i64::MAX as f64).into(), "9.2233720368548E+18"),
    ];
    for (v, want) in cases {
        assert_eq!(t.vm.to_php_string(&v).unwrap(), want, "{v:?}");
    }
    assert_eq!(t.output(), "");
    assert_eq!(t.vm.to_php_string(&Value::Float(f64::NAN)).unwrap(), "NAN");
    assert_eq!(
        t.output(),
        "\nWarning: unexpected NAN value was coerced to string in /t.php on line 3\n"
    );
    assert_eq!(t.vm.to_php_string(&Value::empty_array()).unwrap(), "Array");
    assert_eq!(
        t.output(),
        "\nWarning: Array to string conversion in /t.php on line 3\n"
    );
    let o = t.vm.cast_to_object(&Value::Null).unwrap();
    let r = t.vm.to_php_string(&Value::Object(o)).map(Value::String);
    assert_eq!(
        thrown(r),
        (
            "Error".into(),
            "Object of class stdClass could not be converted to string".into()
        )
    );
    let res = crate::value::ResRef::new("stream", Box::new(()));
    let id = res.id();
    assert_eq!(
        t.vm.to_php_string(&Value::Resource(res.clone())).unwrap(),
        format!("Resource id #{id}").as_str()
    );
    assert_eq!(Value::Resource(res.clone()).legacy_type_name(), "resource");
    res.close();
    assert_eq!(Value::Resource(res).legacy_type_name(), "resource (closed)");

    // Exceptions have __toString.
    let ex = t.vm.create_exception("Exception", "m", 0);
    let s = t.vm.to_php_string(&Value::Object(ex)).unwrap();
    assert!(s.to_string().starts_with("Exception: m in /t.php:3"));
}

#[test]
fn array_and_object_casts() {
    let mut t = TestVm::new();
    let a = t.vm.cast_to_array(&Value::Int(5)).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a.get_int(0).unwrap().as_int(), Some(5));
    assert!(t.vm.cast_to_array(&Value::Null).unwrap().is_empty());
    t.vm.cast_to_array(&Value::Float(f64::NAN)).unwrap();
    assert_eq!(
        t.output(),
        "\nWarning: unexpected NAN value was coerced to array in /t.php on line 3\n"
    );

    let arr = Value::array(PhpArray::from_pairs([("a", 1), ("5", 2)]));
    let o = t.vm.cast_to_object(&arr).unwrap();
    assert_eq!(o.class().name, "stdClass");
    assert_eq!(o.get_prop(b"a").unwrap().as_int(), Some(1));
    assert_eq!(o.get_prop(b"5").unwrap().as_int(), Some(2));
    let back = t.vm.cast_to_array(&Value::Object(o)).unwrap();
    assert_eq!(back.get_int(5).unwrap().as_int(), Some(2));

    let so = t.vm.cast_to_object(&Value::str("x")).unwrap();
    assert_eq!(so.get_prop(b"scalar").unwrap().as_str().unwrap(), "x");

    // Mangled names for protected/private declared properties.
    let ex = t.vm.create_exception("Exception", "msg", 0);
    let props = t.vm.cast_to_array(&Value::Object(ex)).unwrap();
    let keys: Vec<String> = props.keys().map(|k| format!("{k:?}")).collect();
    assert_eq!(
        keys,
        [
            "\"\\0*\\0message\"",
            "\"\\0Exception\\0string\"",
            "\"\\0*\\0code\"",
            "\"\\0*\\0file\"",
            "\"\\0*\\0line\"",
            "\"\\0Exception\\0trace\"",
            "\"\\0Exception\\0previous\""
        ]
    );
}

#[test]
fn type_names() {
    let mut t = TestVm::new();
    assert_eq!(Value::Null.type_name(), "null");
    assert_eq!(Value::Bool(true).type_name(), "bool");
    assert_eq!(Value::Bool(true).value_name(), "true");
    assert_eq!(Value::Bool(false).value_name(), "false");
    assert_eq!(Value::Int(1).type_name(), "int");
    assert_eq!(Value::Float(1.0).type_name(), "float");
    assert_eq!(s("").type_name(), "string");
    assert_eq!(Value::empty_array().type_name(), "array");
    let o = t.vm.cast_to_object(&Value::Null).unwrap();
    assert_eq!(Value::Object(o.clone()).type_name(), "stdClass");
    assert_eq!(
        Value::Ref(crate::value::PhpRef::new(Value::Int(1))).type_name(),
        "int"
    );
    assert_eq!(Value::Undef.type_name(), "null");
    assert_eq!(Value::Float(1.0).legacy_type_name(), "double");
    assert_eq!(Value::Int(1).legacy_type_name(), "integer");
    assert_eq!(Value::Bool(true).legacy_type_name(), "boolean");
    assert_eq!(Value::Null.legacy_type_name(), "NULL");
    assert_eq!(Value::Object(o).legacy_type_name(), "object");
}
