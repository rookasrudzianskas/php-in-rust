//! Malformed data: error offsets, warnings, options and depth limits.

use php_runtime::value::ResRef;
use php_runtime::{PhpArray, Value};

use super::harness::*;

/// Unserializes `data` expecting `false` and returns the output.
fn fails(t: &mut TestVm, data: &[u8]) -> String {
    let v = unser(t, data);
    assert!(v.identical(&Value::Bool(false)), "{data:?} gave {v:?}");
    t.output()
}

#[track_caller]
fn error_at(t: &mut TestVm, data: &str, offset: usize) {
    let out = fails(t, data.as_bytes());
    expectf(
        &out,
        &format!(
            "Warning: unserialize(): Error at offset {offset} of {} bytes in %s on line %d",
            data.len()
        ),
    );
}

#[test]
fn invalid_signs_in_lengths() {
    // invalid_signs_in_lengths.phpt
    let mut t = TestVm::new();
    for (data, off) in [
        ("s:+1:\"x\";", 0),
        ("s:-1:\"x\";", 0),
        ("a:+0:{}", 0),
        ("a:-0:{}", 0),
        ("O:+8:\"stdClass\":0:{}", 0),
        ("O:-8:\"stdClass\":0:{}", 0),
        ("C:+11:\"ArrayObject\":0:{}", 0),
        ("C:-11:\"ArrayObject\":0:{}", 0),
        ("a:1:{i:0;r:+1;}", 9),
        ("a:1:{i:0;r:-1;}", 9),
        ("a:1:{i:0;R:+1;}", 9),
        ("a:1:{i:0;R:-1;}", 9),
    ] {
        error_at(&mut t, data, off);
    }
}

#[test]
fn error_offsets_from_bug_reports() {
    let mut t = TestVm::new();
    // gh23780.phpt
    error_at(&mut t, "", 0);
    // bug71840.phpt
    error_at(&mut t, "a:1:{s:0:\"\"0a:0:{}}", 11);
    // bug74111.phpt
    error_at(&mut t, "O:8:\"stdClass\":00000000", 23);
    // oss_fuzz_433303828.phpt
    error_at(&mut t, "O:2:\"yy\": ", 9);
    error_at(&mut t, "O:2:\"yy\":: ", 10);
    // bug74103.phpt
    error_at(
        &mut t,
        "a:7:{i:0;i:04;s:1:\"a\";i:2;i:9617006;i:4;s:1:\"a\";i:4;s:1:\"a\";R:5;s:1:\"7\";R:3;s:1:\"a\";R:5;;s:18;}}",
        64,
    );
    // bug75054.phpt
    let poc = "a:9:{i:0;s:4:\"0000\";i:0;s:4:\"0000\";i:0;R:2;s:4:\"5003\";R:2;s:4:\"0000\";R:2;s:4:\"0000\";R:2;s:4:\"\x06000\";R:2;s:4:\"0000\";d:0;s:4:\"0000\";a:9:{s:4:\"0000\";";
    error_at(&mut t, poc, 43);
    // bug68545.phpt
    let data = "a:6:{a:6:{s:3:\"322\";s:3:\"bar\";s:3:\"bar\";s:3:\"foo\";a:6:{a:6:{s:3:\"322\";s:3:\"bar\";s:3:\"bar\";s:3:\"foo\";s:3:\"bar\";a:6:{a:6:{s:3:\"322\";s:3:\"bar\";s:3:\"bar\";s:3:\"foo\";a:6:{a:6:{s:3:\"322\";s:3:\"bar\";s:3:\"b22\";s:3:\"bar\";s:3:\"bar\";s:3:\"foo\";s:3:\"bar\";a:6:{a:6:{s:3:\"322\";s:3:\"bar\";s:3:\"bar\";s:3:\"foo\";s:3:\"bar\";s:3:\"bar\";";
    error_at(&mut t, data, 10);
    // Strings with a wrong length or terminator.
    error_at(&mut t, "s:6:\"abc\";", 2);
    error_at(&mut t, "s:5:\"abc\";", 10);
    error_at(&mut t, "s:2:\"abc\";", 7);
    error_at(&mut t, "s:3:\"abc\"x", 9);
    error_at(&mut t, "i:1", 0);
    error_at(&mut t, "b:2;", 0);
    error_at(&mut t, "x", 0);
    error_at(&mut t, "N", 0);
    // Array element counts beyond the remaining data are fake.
    error_at(&mut t, "a:5:{i:0;i:0;}", 5);
    // Back references need a target.
    error_at(&mut t, "R:1;", 4);
    error_at(&mut t, "r:0;", 4);
    error_at(&mut t, "a:1:{i:0;r:1;}", 13);
    // Keys must be ints or strings.
    error_at(&mut t, "a:1:{N;i:0;}", 7);
    error_at(&mut t, "a:1:{d:1;i:0;}", 9);
    error_at(&mut t, "a:1:{a:0:{}i:0;}", 10);
    // Missing closing brace.
    error_at(&mut t, "a:1:{i:0;i:0;", 13);
    error_at(&mut t, "a:1:{i:0;i:0;]", 13);
}

#[test]
fn unexpected_end_of_data() {
    // __serialize_006.phpt shape with an array
    let mut t = TestVm::new();
    let out = fails(&mut t, b"a:2:{i:0;i:1;}");
    expectf(
        &out,
        "Warning: unserialize(): Unexpected end of serialized data in %s on line %d\n\nWarning: unserialize(): Error at offset 13 of 14 bytes in %s on line %d",
    );
}

#[test]
fn numerical_result_out_of_range() {
    // bug74101.phpt, bug74614.phpt
    let mut t = TestVm::new();
    let out = fails(
        &mut t,
        b"O:9:\"Exception\":799999999999999999999999999997:0i:0;a:0:{}i:2;i:0;i:0;R:2;",
    );
    expectf(
        &out,
        "Warning: unserialize(): Numerical result out of range in %s on line %d\n\nWarning: unserialize(): Error at offset 46 of 74 bytes in %s on line %d",
    );
    let out = fails(
        &mut t,
        b"a:3020000000000000000000000000000001:{i:0;a:0:{}i:1;i:2;i:2;i:3;i:3;i:4;i:4;i:5;i:5;i:6;i:6;i:7;i:7;i:8;i:8;R:2;}",
    );
    expectf(
        &out,
        "Warning: unserialize(): Numerical result out of range in %s on line %d\n\nWarning: unserialize(): Error at offset 38 of 113 bytes in %s on line %d",
    );
}

#[test]
fn insufficient_data_for_custom_objects() {
    // bug68044.phpt
    let mut t = TestVm::new();
    let out = fails(&mut t, b"C:3:\"XYZ\":18446744075857035259:{}");
    expectf(
        &out,
        "Warning: unserialize(): Numerical result out of range in %s on line %d\n\nWarning: Insufficient data for unserializing - 9223372036854775807 required, 1 present in %s on line %d\n\nWarning: unserialize(): Error at offset 32 of 33 bytes in %s on line %d",
    );
}

#[test]
fn bad_unserialize_data() {
    // bug73825.phpt
    let mut t = TestVm::new();
    let out = fails(&mut t, b"O:8:\"00000000\":");
    expectf(
        &out,
        "Warning: Bad unserialize data in %s on line %d\n\nWarning: unserialize(): Error at offset 13 of 15 bytes in %s on line %d",
    );
}

#[test]
fn extra_data() {
    // unserialize_extra_data_001.phpt
    let mut t = TestVm::new();
    let v = unser(&mut t, b"i:5;i:6;");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Extra data starting at offset 4 of 8 bytes in %s on line %d\nint(5)",
    );
    let v = unser(&mut t, b"N;i:6;");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Extra data starting at offset 2 of 6 bytes in %s on line %d\nNULL",
    );
    let v = unser(&mut t, b"b:1;i:6;");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Extra data starting at offset 4 of 8 bytes in %s on line %d\nbool(true)",
    );
    let v = unser(&mut t, b"a:1:{s:3:\"foo\";b:1;}i:6;");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Extra data starting at offset 20 of 24 bytes in %s on line %d\narray(1) {\n  [\"foo\"]=>\n  bool(true)\n}",
    );
}

#[test]
fn allowed_classes_option_validation() {
    // unserialize_allowed_classes_option_invalid_value.phpt
    let mut t = TestVm::new();
    let data = b"a:3:{i:0;O:8:\"stdClass\":1:{s:1:\"x\";s:3:\"bar\";}i:1;i:2;i:2;s:1:\"3\";}";
    for (v, given) in [
        (Value::Null, "null"),
        (Value::Int(0), "int"),
        (Value::Int(1), "int"),
    ] {
        assert_eq!(
            thrown(unser_opts(&mut t, data, vec![("allowed_classes", v)])),
            (
                "TypeError".to_string(),
                format!(
                    "unserialize(): Option \"allowed_classes\" must be of type array|bool, {given} given"
                )
            )
        );
    }
    // unserialize_allowed_classes_option_invalid_array.phpt
    let res = ResRef::new("stream", Box::new(()));
    for (v, given) in [
        (Value::Null, "null"),
        (Value::Bool(false), "false"),
        (Value::Bool(true), "true"),
        (Value::Int(42), "int"),
        (Value::Float(15.2), "float"),
        (Value::empty_array(), "array"),
        (Value::Resource(res), "resource"),
    ] {
        assert_eq!(
            thrown(unser_opts(
                &mut t,
                data,
                vec![("allowed_classes", list(vec![v]))]
            )),
            (
                "TypeError".to_string(),
                format!(
                    "unserialize(): Option \"allowed_classes\" must be an array of class names, {given} given"
                )
            )
        );
    }
    let std = Value::Object(new_obj(&mut t, "stdClass"));
    assert_eq!(
        thrown(unser_opts(
            &mut t,
            data,
            vec![("allowed_classes", list(vec![std]))]
        )),
        (
            "Error".to_string(),
            "Object of class stdClass could not be converted to string".to_string()
        )
    );
    // unserialize_allowed_classes_option_invalid_class_names.phpt
    for name in ["", "245blerg"] {
        assert!(unser_opts(&mut t, data, vec![("allowed_classes", list(vec![s(name)]))]).is_ok());
    }
    for (name, shown) in [
        ("  whitespace  ", "  whitespace  "),
        ("name\nwith whitespace", "name\nwith whitespace"),
        ("$dollars", "$dollars"),
        ("have\0nul_byte", "have"),
    ] {
        assert_eq!(
            thrown(unser_opts(
                &mut t,
                data,
                vec![("allowed_classes", list(vec![s(name)]))]
            )),
            (
                "ValueError".to_string(),
                format!(
                    "unserialize(): Option \"allowed_classes\" must be an array of class names, \"{shown}\" given"
                )
            )
        );
    }
    assert_eq!(t.output(), "");
}

#[test]
fn allowed_classes_make_incomplete_objects() {
    // unserialize_classes.phpt
    let mut t = TestVm::new();
    let data = b"a:2:{i:0;O:8:\"stdClass\":1:{s:1:\"x\";i:1;}i:1;r:2;}";
    let v = unser_opts(&mut t, data, vec![("allowed_classes", Value::Bool(false))]).unwrap();
    expectf(
        &dump(&t, &v),
        "array(2) {\n  [0]=>\n  object(__PHP_Incomplete_Class)#%d (2) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(8) \"stdClass\"\n    [\"x\"]=>\n    int(1)\n  }\n  [1]=>\n  object(__PHP_Incomplete_Class)#%d (2) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(8) \"stdClass\"\n    [\"x\"]=>\n    int(1)\n  }\n}",
    );
    // Allowed by (case-insensitive) name.
    let v = unser_opts(
        &mut t,
        data,
        vec![("allowed_classes", list(vec![s("STDCLASS")]))],
    )
    .unwrap();
    let a = v.as_array().unwrap();
    assert_eq!(
        a.get_int(0).unwrap().as_object().unwrap().class().name,
        "stdClass"
    );
    // Another list does not allow it.
    let v = unser_opts(
        &mut t,
        data,
        vec![("allowed_classes", list(vec![s("Foo")]))],
    )
    .unwrap();
    let o = v
        .as_array()
        .unwrap()
        .get_int(0)
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(o.class().name, "__PHP_Incomplete_Class");
    // true allows everything.
    let v = unser_opts(&mut t, data, vec![("allowed_classes", Value::Bool(true))]).unwrap();
    let o = v
        .as_array()
        .unwrap()
        .get_int(0)
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(o.class().name, "stdClass");
    assert_eq!(t.output(), "");
}

fn nested(depth: usize, prefix: &str, suffix: &str) -> String {
    format!("{}i:0;{}", prefix.repeat(depth), suffix.repeat(depth))
}

#[test]
fn max_depth() {
    // max_depth.phpt
    let mut t = TestVm::new();
    assert_eq!(
        thrown(unser_opts(&mut t, b"i:0;", vec![("max_depth", s("foo"))])),
        (
            "TypeError".to_string(),
            "unserialize(): Option \"max_depth\" must be of type int, string given".to_string()
        )
    );
    assert_eq!(
        thrown(unser_opts(
            &mut t,
            b"i:0;",
            vec![("max_depth", Value::Int(-1))]
        )),
        (
            "ValueError".to_string(),
            "unserialize(): Option \"max_depth\" must be greater than or equal to 0".to_string()
        )
    );
    let depth_warning = |n: i64, off: usize, len: usize| {
        format!(
            "Warning: unserialize(): Maximum depth of {n} exceeded. The depth limit can be changed using the max_depth unserialize() option or the unserialize_max_depth ini setting in %s on line %d\n\nWarning: unserialize(): Error at offset {off} of {len} bytes in %s on line %d"
        )
    };
    let md = |n: i64| vec![("max_depth", Value::Int(n))];
    // Arrays.
    let ok = nested(128, "a:1:{i:0;", "}");
    assert!(
        !unser_opts(&mut t, ok.as_bytes(), md(128))
            .unwrap()
            .identical(&Value::Bool(false))
    );
    let deep = nested(129, "a:1:{i:0;", "}");
    let v = unser_opts(&mut t, deep.as_bytes(), md(128)).unwrap();
    assert!(v.identical(&Value::Bool(false)));
    expectf(&t.output(), &depth_warning(128, 1157, 1294));
    // Objects.
    let ok = nested(128, "O:8:\"stdClass\":1:{i:0;", "}");
    assert!(
        !unser_opts(&mut t, ok.as_bytes(), md(128))
            .unwrap()
            .identical(&Value::Bool(false))
    );
    let deep = nested(129, "O:8:\"stdClass\":1:{i:0;", "}");
    let v = unser_opts(&mut t, deep.as_bytes(), md(128)).unwrap();
    assert!(v.identical(&Value::Bool(false)));
    expectf(&t.output(), &depth_warning(128, 2834, 2971));
    // The ini setting.
    t.vm.ini_set("unserialize_max_depth", "128");
    let ok = nested(128, "a:1:{i:0;", "}");
    assert!(!unser(&mut t, ok.as_bytes()).identical(&Value::Bool(false)));
    let deep = nested(129, "a:1:{i:0;", "}");
    assert!(unser(&mut t, deep.as_bytes()).identical(&Value::Bool(false)));
    expectf(&t.output(), &depth_warning(128, 1157, 1294));
    // An explicit option takes precedence.
    let ok = nested(256, "a:1:{i:0;", "}");
    assert!(
        !unser_opts(&mut t, ok.as_bytes(), md(256))
            .unwrap()
            .identical(&Value::Bool(false))
    );
    let deep = nested(257, "a:1:{i:0;", "}");
    assert!(
        unser_opts(&mut t, deep.as_bytes(), md(256))
            .unwrap()
            .identical(&Value::Bool(false))
    );
    expectf(&t.output(), &depth_warning(256, 2309, 2574));
    // 0 disables the limit.
    t.vm.ini_set("unserialize_max_depth", "0");
    let deep = nested(300, "a:1:{i:0;", "}");
    assert!(!unser(&mut t, deep.as_bytes()).identical(&Value::Bool(false)));
    assert_eq!(t.output(), "");
}

#[test]
fn incomplete_classes() {
    // bug27469.phpt
    let mut t = TestVm::new();
    let o = unser(&mut t, b"O:9:\"TestClass\":0:{}");
    let want = "object(__PHP_Incomplete_Class)#%d (1) {\n  [\"__PHP_Incomplete_Class_Name\"]=>\n  string(9) \"TestClass\"\n}";
    expectf(&dump(&t, &o), want);
    assert_eq!(ser(&mut t, o.clone()), "O:9:\"TestClass\":0:{}");
    expectf(&dump(&t, &o), want);
    assert_eq!(ser(&mut t, o), "O:9:\"TestClass\":0:{}");

    // incomplete_class.phpt
    let ic = Value::Object(new_obj(&mut t, "__PHP_Incomplete_Class"));
    let d = ser(&mut t, ic);
    assert_eq!(d, "O:22:\"__PHP_Incomplete_Class\":0:{}");
    let o = unser(&mut t, d.as_bytes());
    expectf(&dump(&t, &o), "object(__PHP_Incomplete_Class)#%d (0) {\n}");
    let obj = o.as_object().unwrap().clone();
    let write = obj.class().handlers.write_property.unwrap();
    let r = write(&mut t.vm, &obj, &php_runtime::PhpStr::from("test"), s("a"));
    assert_eq!(
        thrown(r.map(Value::Bool)),
        (
            "Error".to_string(),
            "The script tried to modify a property on an incomplete object. Please ensure that the class definition \"unknown\" of the object you are trying to operate on was loaded _before_ unserialize() gets called or provide an autoloader to load the class definition".to_string()
        )
    );
    let read = obj.class().handlers.read_property.unwrap();
    t.vm.executing = true;
    let v = read(&mut t.vm, &obj, &php_runtime::PhpStr::from("test")).unwrap();
    assert!(v.unwrap().identical(&Value::Null));
    expectf(
        &t.output(),
        "Warning: main(): The script tried to access a property on an incomplete object. Please ensure that the class definition \"unknown\" of the object you are trying to operate on was loaded _before_ unserialize() gets called or provide an autoloader to load the class definition in %s on line %d",
    );

    // incomplete_class_magic.phpt
    let data = b"O:8:\"Missing_\":1:{s:33:\"__PHP_Incomplete_Class_Name\0other\";i:123;}";
    let o = unser(&mut t, data);
    let want = "object(__PHP_Incomplete_Class)#%d (2) {\n  [\"__PHP_Incomplete_Class_Name\"]=>\n  string(8) \"Missing_\"\n  [\"__PHP_Incomplete_Class_Name\0other\"]=>\n  int(123)\n}";
    expectf(&dump(&t, &o), want);
    let re = ser_bytes(&mut t, o);
    assert_eq!(&re[..], &data[..]);
    let o = unser(&mut t, &re);
    expectf(&dump(&t, &o), want);
}

#[test]
fn incomplete_class_c_format() {
    // incomplete_class_c_format.phpt (first part)
    let mut t = TestVm::new();
    let v = unser(
        &mut t,
        b"a:2:{i:0;C:7:\"Missing\":4:{abcd}i:1;s:4:\"tail\";}",
    );
    expectf(
        &dump_out(&t, &v),
        "Warning: Class __PHP_Incomplete_Class has no unserializer in %s on line %d\narray(2) {\n  [0]=>\n  object(__PHP_Incomplete_Class)#%d (1) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(7) \"Missing\"\n  }\n  [1]=>\n  string(4) \"tail\"\n}",
    );
}

#[test]
fn not_serializable_classes() {
    // bug67072.phpt shape: NOT_SERIALIZABLE internal classes
    let mut t = TestVm::new();
    ClassBuilder::new("NoSer")
        .flags(php_runtime::class::class_flags::NOT_SERIALIZABLE)
        .register(&mut t);
    let o = Value::Object(new_obj(&mut t, "NoSer"));
    assert_eq!(
        thrown(t.call("serialize", vec![list(vec![o])])),
        (
            "Exception".to_string(),
            "Serialization of 'NoSer' is not allowed".to_string()
        )
    );
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:5:\"NoSer\":0:{}")])),
        (
            "Exception".to_string(),
            "Unserialization of 'NoSer' is not allowed".to_string()
        )
    );
    // No "Error at offset" warning when an exception is pending.
    assert_eq!(t.output(), "");
}

#[test]
fn abstract_classes_cannot_be_instantiated() {
    // unserialize_abstract_class.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("AbstractThing")
        .flags(php_runtime::class::class_flags::ABSTRACT)
        .register(&mut t);
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:13:\"AbstractThing\":0:{}")])),
        (
            "Error".to_string(),
            "Cannot instantiate abstract class AbstractThing".to_string()
        )
    );
    let iface = "O:9:\"Countable\":0:{}";
    assert_eq!(
        thrown(t.call("unserialize", vec![s(iface)])),
        (
            "Error".to_string(),
            "Cannot instantiate interface Countable".to_string()
        )
    );
}

#[test]
fn class_name_validation() {
    let mut t = TestVm::new();
    // Leading namespace separator, NUL and invalid characters are rejected
    // before any lookup.
    error_at(&mut t, "O:9:\"\\stdClass\":0:{}", 0);
    error_at(&mut t, "O:4:\"\0abc\":0:{}", 0);
    error_at(&mut t, "O:3:\"a-b\":0:{}", 0);
    error_at(&mut t, "O:0:\"\":0:{}", 2);
    let mut opts = PhpArray::new();
    opts.insert_str("allowed_classes", Value::Bool(false));
    let v = t
        .call(
            "unserialize",
            vec![s("O:3:\"a-b\":0:{}"), Value::array(opts)],
        )
        .unwrap();
    assert!(v.identical(&Value::Bool(false)));
    assert!(t.output().contains("Error at offset 0 of 14 bytes"));
}
