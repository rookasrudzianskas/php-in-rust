use super::*;
use crate::testing::{TestVm, thrown};

fn arr(pairs: Vec<(Option<&str>, Value)>) -> PhpArray {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        match k {
            Some(k) => {
                a.insert(ArrayKey::from_bytes(k.as_bytes()), v);
            }
            None => {
                let _ = a.append(v);
            }
        }
    }
    a
}

fn build(t: &mut TestVm, args: Vec<Value>) -> String {
    match t.call("http_build_query", args).unwrap() {
        Value::String(s) => s.to_string_lossy().into_owned(),
        v => panic!("string expected, got {v:?}"),
    }
}

#[test]
fn basic_array_phpt() {
    // http_build_query.phpt
    let mut t = TestVm::new();
    let a = Value::array(arr(vec![
        (Some("foo"), Value::str("bar")),
        (Some("baz"), Value::Int(1)),
        (Some("test"), Value::str("a ' \" ")),
        (None, Value::str("abc")),
        (Some("float"), Value::Float(10.42)),
        (Some("true"), Value::Bool(true)),
        (Some("false"), Value::Bool(false)),
    ]));
    assert_eq!(
        build(&mut t, vec![a.clone()]),
        "foo=bar&baz=1&test=a+%27+%22+&0=abc&float=10.42&true=1&false=0"
    );
    assert_eq!(
        build(&mut t, vec![a.clone(), Value::str("foo")]),
        "foo=bar&baz=1&test=a+%27+%22+&foo0=abc&float=10.42&true=1&false=0"
    );
    assert_eq!(
        build(&mut t, vec![a, Value::str("foo"), Value::str(";")]),
        "foo=bar;baz=1;test=a+%27+%22+;foo0=abc;float=10.42;true=1;false=0"
    );
}

#[test]
fn empty_null_resource_and_float_encoding() {
    let mut t = TestVm::new();
    assert_eq!(build(&mut t, vec![Value::empty_array()]), "");
    assert_eq!(
        build(&mut t, vec![Value::array(arr(vec![(None, Value::Null)]))]),
        ""
    );
    // bug77608: "+" in a float is encoded
    let a = arr(vec![
        (Some("x"), Value::Float(1e14)),
        (Some("y"), Value::str("1E+14")),
    ]);
    assert_eq!(
        build(&mut t, vec![Value::array(a)]),
        "x=1.0E%2B14&y=1E%2B14"
    );
    // references are followed
    let r = php_runtime::PhpRef::new(Value::str("value"));
    let a = arr(vec![(None, Value::Ref(r))]);
    assert_eq!(build(&mut t, vec![Value::array(a)]), "0=value");
}

#[test]
fn multidimensional_arrays() {
    // http_build_query_variation2.phpt
    let mut t = TestVm::new();
    let admin2 = arr(vec![
        (Some("who"), Value::str("admin2")),
        (Some("2"), Value::str("test")),
    ]);
    let admin = arr(vec![
        (None, Value::str("admin1")),
        (Some("admin2"), Value::array(admin2)),
    ]);
    let sort = arr(vec![
        (None, Value::str("desc")),
        (Some("admin"), Value::array(admin)),
    ]);
    let nine = arr(vec![
        (Some("1"), Value::str("val1")),
        (Some("3"), Value::str("val2")),
        (Some("string"), Value::str("string")),
    ]);
    let m = Value::array(arr(vec![
        (None, Value::Int(20)),
        (Some("5"), Value::Int(13)),
        (Some("9"), Value::array(nine)),
        (Some("name"), Value::str("homepage")),
        (Some("page"), Value::Int(10)),
        (Some("sort"), Value::array(sort)),
    ]));
    assert_eq!(
        build(&mut t, vec![m.clone()]),
        "0=20&5=13&9%5B1%5D=val1&9%5B3%5D=val2&9%5Bstring%5D=string&name=homepage&page=10&sort%5B0%5D=desc&sort%5Badmin%5D%5B0%5D=admin1&sort%5Badmin%5D%5Badmin2%5D%5Bwho%5D=admin2&sort%5Badmin%5D%5Badmin2%5D%5B2%5D=test"
    );
    assert_eq!(
        build(&mut t, vec![m, Value::str("prefix_")]),
        "prefix_0=20&prefix_5=13&prefix_9%5B1%5D=val1&prefix_9%5B3%5D=val2&prefix_9%5Bstring%5D=string&name=homepage&page=10&sort%5B0%5D=desc&sort%5Badmin%5D%5B0%5D=admin1&sort%5Badmin%5D%5Badmin2%5D%5Bwho%5D=admin2&sort%5Badmin%5D%5Badmin2%5D%5B2%5D=test"
    );
}

#[test]
fn encoding_types() {
    // http_build_query_variation3.phpt
    let mut t = TestVm::new();
    let a = Value::array(arr(vec![
        (Some("name"), Value::str("main page")),
        (Some("sort"), Value::str("desc,admin")),
        (Some("equation"), Value::str("10 + 10 - 5")),
    ]));
    assert_eq!(
        build(
            &mut t,
            vec![
                a.clone(),
                Value::str(""),
                Value::str("&"),
                Value::Int(PHP_QUERY_RFC1738)
            ]
        ),
        "name=main+page&sort=desc%2Cadmin&equation=10+%2B+10+-+5"
    );
    assert_eq!(
        build(
            &mut t,
            vec![
                a,
                Value::str(""),
                Value::str("&"),
                Value::Int(PHP_QUERY_RFC3986)
            ]
        ),
        "name=main%20page&sort=desc%2Cadmin&equation=10%20%2B%2010%20-%205"
    );
    // gh12745: the default separator with a named encoding_type
    let a = Value::array(arr(vec![
        (Some("hello"), Value::str("world")),
        (Some("foo"), Value::str("bar")),
    ]));
    let r =
        t.vm.call_native_named(
            "http_build_query",
            vec![a],
            vec![("encoding_type", Value::Int(PHP_QUERY_RFC3986))],
        )
        .unwrap();
    assert_eq!(r.as_str().unwrap(), "hello=world&foo=bar");
    // rawurlencode keeps "~"
    let a = Value::array(arr(vec![(Some("k~"), Value::str("v~ "))]));
    assert_eq!(
        build(
            &mut t,
            vec![a.clone(), Value::str(""), Value::Null, Value::Int(2)]
        ),
        "k~=v~%20"
    );
    assert_eq!(build(&mut t, vec![a]), "k%7E=v%7E+");
}

#[test]
fn arg_separator_follows_the_ini_setting() {
    let mut t = TestVm::new();
    let a = Value::array(arr(vec![
        (Some("a"), Value::Int(1)),
        (Some("b"), Value::Int(2)),
    ]));
    t.vm.ini_set("arg_separator.output", "&amp;");
    assert_eq!(build(&mut t, vec![a]), "a=1&amp;b=2");
}

fn std_object(t: &mut TestVm, props: &[(&str, Value)]) -> Value {
    let ce = t.vm.find_class("stdClass").unwrap();
    let o = t.vm.instantiate(&ce).unwrap();
    for (k, v) in props {
        o.set_prop(k.as_bytes(), v.clone());
    }
    Value::Object(o)
}

#[test]
fn objects_are_deprecated_but_encoded() {
    let mut t = TestVm::new();
    let o = std_object(
        &mut t,
        &[
            ("name", Value::str("homepage")),
            ("page", Value::Int(1)),
            ("sort", Value::str("desc,name")),
        ],
    );
    assert_eq!(
        build(&mut t, vec![o]),
        "name=homepage&page=1&sort=desc%2Cname"
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: http_build_query(): Passing an object for argument #1 $data to http_build_query() is deprecated, call get_object_vars() first instead in /t.php on line 3\n"
    );
    // empty object
    let o = std_object(&mut t, &[]);
    assert_eq!(build(&mut t, vec![o]), "");
    let _ = t.output();
}

#[test]
fn nested_objects_and_recursion() {
    let mut t = TestVm::new();
    let inner = std_object(&mut t, &[("public", Value::str("input"))]);
    let outer = std_object(&mut t, &[("public", inner)]);
    assert_eq!(build(&mut t, vec![outer]), "public%5Bpublic%5D=input");
    let out = t.output();
    assert!(out.contains("Passing an object for argument #1"), "{out}");
    assert!(out.contains("object values within argument #1 $data to http_build_query() being interpreted as arrays is deprecated"), "{out}");

    // a self-referencing object produces nothing
    let o = std_object(&mut t, &[]);
    if let Value::Object(obj) = &o {
        obj.set_prop(b"public", o.clone());
    }
    assert_eq!(build(&mut t, vec![o.clone()]), "");
    let _ = t.output();
    if let Value::Object(obj) = &o {
        obj.set_prop(b"public", Value::Null); // break the Rc cycle
    }
}

#[test]
fn array_with_object_values() {
    let mut t = TestVm::new();
    let o = std_object(&mut t, &[("a", Value::Int(1))]);
    let a = Value::array(arr(vec![(Some("o"), o)]));
    assert_eq!(build(&mut t, vec![a]), "o%5Ba%5D=1");
    assert!(t.output().contains("object values within argument #1"));
}

#[test]
fn only_public_properties_outside_the_class() {
    // Exception has protected and private properties but no public ones.
    let mut t = TestVm::new();
    let e = t.vm.create_exception("Exception", "boom", 0);
    assert_eq!(build(&mut t, vec![Value::Object(e.clone())]), "");
    let _ = t.output();

    // visibility rules, as zend_check_property_access applies them
    let exception = t.vm.find_class("Exception").unwrap();
    let error = t.vm.find_class("Error").unwrap();
    assert_eq!(
        visible_property_name(&e, b"\0*\0message", Some(&exception)),
        Some(&b"message"[..])
    );
    assert_eq!(
        visible_property_name(&e, b"\0*\0message", Some(&error)),
        None
    );
    assert_eq!(visible_property_name(&e, b"\0*\0message", None), None);
    assert_eq!(
        visible_property_name(&e, b"\0Exception\0string", Some(&exception)),
        Some(&b"string"[..])
    );
    assert_eq!(
        visible_property_name(&e, b"\0Error\0string", Some(&exception)),
        None
    );
    assert_eq!(
        visible_property_name(&e, b"public", None),
        Some(&b"public"[..])
    );
}

#[test]
fn deep_structures_raise_an_error() {
    let mut t = TestVm::new();
    // too-deep structures raise an Error instead of overflowing the stack
    let mut a = Value::Null;
    for i in 0..3000i64 {
        let mut level = PhpArray::new();
        level.insert(ArrayKey::Int(i), a);
        a = Value::array(level);
    }
    t.vm.ini_set_system("zend.max_allowed_stack_size", "256K");
    let r = t.call("http_build_query", vec![a, Value::str("p")]);
    let (c, m) = thrown(r);
    assert_eq!(
        (c.as_str(), m.as_str()),
        ("Error", "Maximum call stack size reached.")
    );
}

#[test]
fn request_parse_body_options() {
    let mut t = TestVm::new();
    let opts = |k: Option<&str>, v: Value| Value::array(arr(vec![(k, v)]));
    let (c, m) = thrown(t.call("request_parse_body", vec![opts(Some("foo"), Value::Int(1))]));
    assert_eq!(
        (c.as_str(), m.as_str()),
        ("ValueError", "Invalid key \"foo\" in $options argument")
    );
    let (_, m) = thrown(t.call("request_parse_body", vec![opts(None, Value::Int(1))]));
    assert_eq!(m, "Invalid integer key in $options argument");
    let (_, m) = thrown(t.call("request_parse_body", vec![opts(Some(""), Value::Int(1))]));
    assert_eq!(m, "Invalid empty string key in $options argument");
    let (_, m) = thrown(t.call(
        "request_parse_body",
        vec![opts(Some("max_input_vars"), Value::empty_array())],
    ));
    assert_eq!(m, "Invalid array value in $options argument");
    // an invalid quantity warns, then the request has no content type
    let r = t.call(
        "request_parse_body",
        vec![opts(Some("upload_max_filesize"), Value::str("1GB"))],
    );
    let (c, m) = thrown(r);
    assert_eq!(
        (c.as_str(), m.as_str()),
        (
            "RequestParseBodyException",
            "Request does not provide a content type"
        )
    );
    assert_eq!(
        t.output(),
        "\nWarning: Invalid quantity \"1GB\": unknown multiplier \"B\", interpreting as \"1\" for backwards compatibility in /t.php on line 3\n"
    );
    let (c, _) = thrown(t.call("request_parse_body", vec![]));
    assert_eq!(c, "RequestParseBodyException");
}

#[test]
fn last_response_headers() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("http_get_last_response_headers", vec![]).unwrap(),
        Value::Null
    ));
    let h = Value::array(PhpArray::from_values(vec![Value::str("HTTP/1.1 200 OK")]));
    set_last_response_headers(&mut t.vm, Some(h));
    let r = t.call("http_get_last_response_headers", vec![]).unwrap();
    assert_eq!(r.as_array().unwrap().len(), 1);
    t.call("http_clear_last_response_headers", vec![]).unwrap();
    assert!(matches!(
        t.call("http_get_last_response_headers", vec![]).unwrap(),
        Value::Null
    ));
}

#[test]
fn constants() {
    let t = TestVm::new();
    assert_eq!(
        t.vm.constant("PHP_QUERY_RFC1738").unwrap().as_int(),
        Some(1)
    );
    assert_eq!(
        t.vm.constant("PHP_QUERY_RFC3986").unwrap().as_int(),
        Some(2)
    );
}
