//! Objects: property tables, visibility, typed properties, `__sleep()`,
//! `__wakeup()`, `__serialize()` and `__unserialize()`.

use php_runtime::class::{PropPurpose, class_flags, member_flags};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, Value, Vm};

use super::harness::*;

fn this(args: &Args) -> ObjRef {
    args.this().cloned().expect("method called without $this")
}

const PROTECTED: u32 = member_flags::PROTECTED;
const PRIVATE: u32 = member_flags::PRIVATE;

#[test]
fn stdclass_numeric_properties() {
    // bug55798.phpt
    let mut t = TestVm::new();
    let a = new_obj(&mut t, "stdClass");
    a.set_prop(b"0", s("X"));
    a.set_prop(b"1", s("Y"));
    let data = ser(&mut t, Value::Object(a));
    assert_eq!(data, "O:8:\"stdClass\":2:{s:1:\"0\";s:1:\"X\";s:1:\"1\";s:1:\"Y\";}");
    let b = unser(&mut t, data.as_bytes());
    assert_eq!(ser(&mut t, b.clone()), data);
    assert!(b.as_object().unwrap().get_prop(b"0").unwrap().identical(&s("X")));
    // Integer property keys are converted to strings.
    let o = unser(&mut t, b"O:8:\"stdClass\":1:{i:5;s:1:\"Z\";}");
    assert!(o.as_object().unwrap().get_prop(b"5").unwrap().identical(&s("Z")));
}

#[test]
fn exception_properties_are_mangled() {
    let mut t = TestVm::new();
    let e = new_obj(&mut t, "Exception");
    e.set_prop(b"message", s("msg"));
    let data = ser_bytes(&mut t, Value::Object(e));
    let want: &[u8] = b"O:9:\"Exception\":7:{s:10:\"\0*\0message\";s:3:\"msg\";s:17:\"\0Exception\0string\";s:0:\"\";s:7:\"\0*\0code\";i:0;s:7:\"\0*\0file\";s:6:\"/t.php\";s:7:\"\0*\0line\";i:3;s:16:\"\0Exception\0trace\";a:0:{}s:19:\"\0Exception\0previous\";N;}";
    assert_eq!(data, want);
    let back = unser(&mut t, &data);
    let o = back.as_object().unwrap();
    assert_eq!(o.class().name, "Exception");
    assert!(o.get_prop(b"message").unwrap().identical(&s("msg")));
    assert!(o.get_prop(b"line").unwrap().identical(&Value::Int(3)));
    assert_eq!(ser_bytes(&mut t, back.clone()), want);
    // A subclass keeps the parent's private names.
    let te = new_obj(&mut t, "TypeError");
    let data = ser(&mut t, Value::Object(te));
    assert!(data.starts_with("O:9:\"TypeError\":7:{s:10:\"\0*\0message\";s:0:\"\";s:13:\"\0Error\0string\";"));
}

#[test]
fn typed_internal_properties_reject_bad_values() {
    // bug69793.phpt, bug69152.phpt, bug70963.phpt
    let mut t = TestVm::new();
    let data = b"O:9:\"Exception\":7:{s:17:\"\0Exception\0string\";s:1:\"a\";s:7:\"\0*\0code\";i:0;s:7:\"\0*\0file\";s:0:\"\";s:7:\"\0*\0line\";i:1337;s:16:\"\0Exception\0trace\";a:0:{}s:19:\"\0Exception\0previous\";i:10;s:10:\"\0*\0message\";N;}";
    assert_eq!(
        thrown(t.call("unserialize", vec![Value::bytes(data)])),
        (
            "TypeError".to_string(),
            "Cannot assign int to property Exception::$previous of type ?Throwable".to_string()
        )
    );
    let data = b"O:9:\"exception\":1:{s:16:\"\0Exception\0trace\";s:4:\"ryat\";}";
    assert_eq!(
        thrown(t.call("unserialize", vec![Value::bytes(data)])),
        (
            "TypeError".to_string(),
            "Cannot assign string to property Exception::$trace of type array".to_string()
        )
    );
    for r in ["R", "r"] {
        let data = format!(
            "a:2:{{i:0;O:9:\"exception\":1:{{s:16:\"\0Exception\0trace\";s:4:\"test\";}}i:1;{r}:3;}}"
        );
        assert_eq!(
            thrown(t.call("unserialize", vec![s(&data)])),
            (
                "TypeError".to_string(),
                "Cannot assign string to property Exception::$trace of type array".to_string()
            )
        );
    }
    assert_eq!(t.output(), "");
}

#[test]
fn declared_and_dynamic_properties() {
    let mut t = TestVm::new();
    ClassBuilder::new("Base")
        .prop("priv", PRIVATE, None, s("base"))
        .prop("prot", PROTECTED, None, Value::Int(1))
        .register(&mut t);
    ClassBuilder::extends(&t, "Child", "Base")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .prop("priv", member_flags::PUBLIC, None, s("child"))
        .register(&mut t);
    let o = new_obj(&mut t, "Child");
    o.set_prop(b"dyn", Value::Bool(true));
    let data = ser_bytes(&mut t, Value::Object(o));
    let want: &[u8] = b"O:5:\"Child\":4:{s:10:\"\0Base\0priv\";s:4:\"base\";s:7:\"\0*\0prot\";i:1;s:4:\"priv\";s:5:\"child\";s:3:\"dyn\";b:1;}";
    assert_eq!(data, want);
    let back = unser(&mut t, &data);
    expectf(
        &dump(&t, &back),
        "object(Child)#%d (4) {\n  [\"priv\":\"Base\":private]=>\n  string(4) \"base\"\n  [\"prot\":protected]=>\n  int(1)\n  [\"priv\"]=>\n  string(5) \"child\"\n  [\"dyn\"]=>\n  bool(true)\n}",
    );
    assert_eq!(ser_bytes(&mut t, back), want);
}

#[test]
fn changed_visibility_is_followed() {
    // is_property_visibility_changed()
    let mut t = TestVm::new();
    ClassBuilder::new("Vis")
        .prop("a", PROTECTED, None, Value::Null)
        .prop("b", member_flags::PUBLIC, None, Value::Null)
        .prop("c", PRIVATE, None, Value::Null)
        .register(&mut t);
    let v = unser(
        &mut t,
        b"O:3:\"Vis\":3:{s:1:\"a\";i:1;s:4:\"\0*\0b\";i:2;s:6:\"\0vis\0c\";i:3;}",
    );
    expectf(
        &dump_out(&t, &v),
        "object(Vis)#%d (3) {\n  [\"a\":protected]=>\n  int(1)\n  [\"b\"]=>\n  int(2)\n  [\"c\":\"Vis\":private]=>\n  int(3)\n}",
    );
    // A private name of another class does not match.
    let v = unser(&mut t, b"O:3:\"Vis\":1:{s:8:\"\0Other\0a\";i:1;}");
    expectf(
        &dump_out(&t, &v),
        "Deprecated: Creation of dynamic property Vis::$a is deprecated in %s on line %d\nobject(Vis)#%d (4) {\n  [\"a\":protected]=>\n  NULL\n  [\"b\"]=>\n  NULL\n  [\"c\":\"Vis\":private]=>\n  NULL\n  [\"a\":\"Other\":private]=>\n  int(1)\n}",
    );
    // Malformed mangled names.
    let v = unser(&mut t, b"O:3:\"Vis\":1:{s:1:\"\0\";i:1;}");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Notice: Illegal member variable name in %s on line %d\n\nWarning: unserialize(): Error at offset 21 of 26 bytes in %s on line %d",
    );
    let v = unser(&mut t, b"O:3:\"Vis\":1:{s:3:\"\0ab\";i:1;}");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Notice: Corrupt member variable name in %s on line %d\n\nWarning: unserialize(): Error at offset 23 of 28 bytes in %s on line %d",
    );
}

#[test]
fn dynamic_property_creation() {
    // unserialize_overwrite_undeclared_protected.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test").public("foo").register(&mut t);
    let v = unser(&mut t, b"O:4:\"Test\":2:{s:4:\"\0*\0x\";N;s:4:\"\0*\0x\";N;}");
    expectf(
        &dump_out(&t, &v),
        "Deprecated: Creation of dynamic property Test::$x is deprecated in %s on line %d\nobject(Test)#%d (2) {\n  [\"foo\"]=>\n  NULL\n  [\"x\":protected]=>\n  NULL\n}",
    );
    // Readonly classes have no dynamic properties.
    ClassBuilder::new("Ro")
        .flags(class_flags::READONLY)
        .register(&mut t);
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:2:\"Ro\":1:{s:1:\"y\";i:1;}")])),
        (
            "Error".to_string(),
            "Cannot create dynamic property Ro::$y".to_string()
        )
    );
    // A virtual property cannot be unserialized.
    ClassBuilder::new("Hooked")
        .prop(
            "v",
            member_flags::PUBLIC | member_flags::VIRTUAL,
            Some("int"),
            Value::Undef,
        )
        .register(&mut t);
    let v = unser(&mut t, b"O:6:\"Hooked\":1:{s:1:\"v\";i:1;}");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Cannot unserialize value for virtual property Hooked::$v in %s on line %d\n\nWarning: unserialize(): Error at offset 24 of 29 bytes in %s on line %d",
    );
}

#[test]
fn typed_property_refs() {
    // typed_property_refs.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("A").typed("a", "int").public("b").register(&mut t);
    ClassBuilder::new("B").public("a").typed("b", "int").register(&mut t);
    ClassBuilder::new("E").public("a").typed("b", "int").register(&mut t);
    ClassBuilder::new("C").typed("a", "int").typed("b", "string").register(&mut t);
    ClassBuilder::new("D").typed("a", "int").typed("b", "float").register(&mut t);
    for (class, a, b) in [("A", "a", "b"), ("B", "a", "b"), ("E", "a", "b")] {
        let data = format!("O:1:\"{class}\":2:{{s:1:\"{a}\";i:1;s:1:\"{b}\";R:2;}}");
        let v = unser(&mut t, data.as_bytes());
        expectf(
            &dump_out(&t, &v),
            &format!(
                "object({class})#%d (2) {{\n  [\"a\"]=>\n  &int(1)\n  [\"b\"]=>\n  &int(1)\n}}"
            ),
        );
    }
    let cases = [
        (
            "O:1:\"A\":2:{s:1:\"a\";N;s:1:\"b\";R:2;}",
            "Cannot assign null to property A::$a of type int",
        ),
        (
            "O:1:\"B\":2:{s:1:\"a\";N;s:1:\"b\";R:2;}",
            "Cannot assign null to property B::$b of type int",
        ),
        (
            "O:1:\"C\":2:{s:1:\"a\";i:1;s:1:\"b\";R:2;}",
            "Cannot assign int to property C::$b of type string",
        ),
        (
            "O:1:\"C\":2:{s:1:\"b\";s:1:\"x\";s:1:\"a\";R:2;}",
            "Cannot assign string to property C::$a of type int",
        ),
        (
            "O:1:\"D\":2:{s:1:\"a\";i:1;s:1:\"b\";R:2;}",
            "Reference with value of type int held by property D::$a of type int is not compatible with property D::$b of type float",
        ),
    ];
    for (data, msg) in cases {
        assert_eq!(
            thrown(t.call("unserialize", vec![s(data)])),
            ("TypeError".to_string(), msg.to_string()),
            "{data}"
        );
    }
    // Strict mode still converts int to float.
    let v = unser(&mut t, b"O:1:\"D\":2:{s:1:\"a\";i:1;s:1:\"b\";i:5;}");
    expectf(
        &dump(&t, &v),
        "object(D)#%d (2) {\n  [\"a\"]=>\n  int(1)\n  [\"b\"]=>\n  float(5)\n}",
    );
    // ... but not string to int.
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:1:\"D\":1:{s:1:\"a\";s:1:\"1\";}")])),
        (
            "TypeError".to_string(),
            "Cannot assign string to property D::$a of type int".to_string()
        )
    );
}

#[test]
fn typed_property_ref_overwrites() {
    // typed_property_ref_overwrite.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test").typed("prop", "?object").register(&mut t);
    let v = unser(&mut t, b"O:4:\"Test\":2:{s:4:\"prop\";R:1;s:4:\"prop\";N;}");
    expectf(&dump_out(&t, &v), "object(Test)#%d (1) {\n  [\"prop\"]=>\n  NULL\n}");

    // typed_property_ref_overwrite2.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test").typed("prop", "?Test").register(&mut t);
    let v = unser(
        &mut t,
        b"O:4:\"Test\":2:{s:4:\"prop\";N;s:4:\"prop\";O:4:\"Test\":1:{s:4:\"prop\";R:2;}}",
    );
    expectf(
        &dump_out(&t, &v),
        "object(Test)#%d (1) {\n  [\"prop\"]=>\n  &object(Test)#%d (1) {\n    [\"prop\"]=>\n    *RECURSION*\n  }\n}",
    );

    // overwrite_untyped_ref.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test").public("prop").register(&mut t);
    let v = unser(&mut t, b"O:4:\"Test\":2:{s:4:\"prop\";R:1;s:4:\"prop\";i:0;}");
    expectf(&dump_out(&t, &v), "object(Test)#%d (1) {\n  [\"prop\"]=>\n  int(0)\n}");
}

#[test]
fn typed_property_ref_assignment_failure() {
    // typed_property_ref_assignment_failure.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test").typed("prop", "int").register(&mut t);
    assert_eq!(
        thrown(t.call(
            "unserialize",
            vec![s("O:4:\"Test\":1:{s:4:\"prop\";O:8:\"stdClass\":1:{s:1:\"y\";R:2;}}")]
        )),
        (
            "TypeError".to_string(),
            "Cannot assign stdClass to property Test::$prop of type int".to_string()
        )
    );
    let data = "O:4:\"Test\":1:{s:4:\"prop\";a:1:{i:0;R:2;";
    let v = unser(&mut t, data.as_bytes());
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Error at offset 38 of 38 bytes in %s on line %d",
    );
}

// ----- __sleep() / __wakeup() -------------------------------------------------

fn sleep_a_c(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("__sleep called\n")?;
    Ok(list(vec![s("a"), s("c")]))
}
static SLEEP_A_C: NativeFunction = NativeFunction {
    sig: "__sleep(): array",
    handler: sleep_a_c,
};

fn wakeup_echo(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    vm.echo_str(&format!("{}::__wakeup called\n", o.class().name))?;
    Ok(Value::Null)
}
static WAKEUP_ECHO: NativeFunction = NativeFunction {
    sig: "__wakeup(): void",
    handler: wakeup_echo,
};

#[test]
fn sleep_and_wakeup_001() {
    // 001.phpt (classes t and s)
    let mut t = TestVm::new();
    ClassBuilder::new("t")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .register(&mut t);
    let o = new_obj(&mut t, "t");
    o.set_prop(b"a", s("hallo"));
    let data = ser(&mut t, Value::Object(o));
    assert_eq!(data, "O:1:\"t\":1:{s:1:\"a\";s:5:\"hallo\";}");
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump(&t, &v),
        "object(t)#%d (1) {\n  [\"a\"]=>\n  string(5) \"hallo\"\n}",
    );

    ClassBuilder::new("s")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .public("a")
        .public("b")
        .public("c")
        .method(&SLEEP_A_C)
        .method(&WAKEUP_ECHO)
        .register(&mut t);
    let o = new_obj(&mut t, "s");
    o.set_prop(b"a", s("hallo"));
    o.set_prop(b"b", s("php"));
    o.set_prop(b"c", s("world"));
    o.set_prop(b"d", s("!"));
    let data = ser(&mut t, Value::Object(o));
    expectf(&t.output(), "__sleep called");
    assert_eq!(data, "O:1:\"s\":2:{s:1:\"a\";s:5:\"hallo\";s:1:\"c\";s:5:\"world\";}");
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump_out(&t, &v),
        "s::__wakeup called\nobject(s)#%d (3) {\n  [\"a\"]=>\n  string(5) \"hallo\"\n  [\"b\"]=>\n  NULL\n  [\"c\"]=>\n  string(5) \"world\"\n}",
    );
}

fn sleep_clash(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(list(vec![Value::bytes(b"\0Test\0priv"), s("priv")]))
}
static SLEEP_CLASH: NativeFunction = NativeFunction {
    sig: "__sleep(): array",
    handler: sleep_clash,
};

fn sleep_pub_prot_priv(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(list(vec![s("pub"), s("prot"), s("priv")]))
}
static SLEEP_PPP: NativeFunction = NativeFunction {
    sig: "__sleep(): array",
    handler: sleep_pub_prot_priv,
};

fn sleep_xyz(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(list(vec![s("x"), s("y"), s("z")]))
}
static SLEEP_XYZ: NativeFunction = NativeFunction {
    sig: "__sleep(): array",
    handler: sleep_xyz,
};

fn sleep_bad(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(42))
}
static SLEEP_BAD: NativeFunction = NativeFunction {
    sig: "__sleep()",
    handler: sleep_bad,
};

fn sleep_names(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let name = PhpRef::new(s("x"));
    Ok(list(vec![Value::Ref(name), Value::Int(1)]))
}
static SLEEP_NAMES: NativeFunction = NativeFunction {
    sig: "__sleep(): array",
    handler: sleep_names,
};

#[test]
fn sleep_warnings() {
    // sleep_mangled_name_clash.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test")
        .prop("priv", PRIVATE, None, Value::Null)
        .method(&SLEEP_CLASH)
        .register(&mut t);
    let o = new_obj(&mut t, "Test");
    let data = ser_bytes(&mut t, Value::Object(o));
    expectf(
        &t.output(),
        "Warning: serialize(): \"priv\" is returned from __sleep() multiple times in %s on line %d",
    );
    assert_eq!(data, b"O:4:\"Test\":1:{s:10:\"\0Test\0priv\";N;}");

    // sleep_undefined_declared_properties.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test")
        .public("pub")
        .prop("prot", PROTECTED, None, Value::Null)
        .prop("priv", PRIVATE, None, Value::Null)
        .method(&SLEEP_PPP)
        .register(&mut t);
    let o = new_obj(&mut t, "Test");
    for v in o.obj().props.borrow_mut().iter_mut() {
        *v = Value::Undef;
    }
    let data = ser(&mut t, Value::Object(o));
    expectf(
        &t.output(),
        "Warning: serialize(): \"pub\" returned as member variable from __sleep() but does not exist in %s on line %d\n\nWarning: serialize(): \"prot\" returned as member variable from __sleep() but does not exist in %s on line %d\n\nWarning: serialize(): \"priv\" returned as member variable from __sleep() but does not exist in %s on line %d",
    );
    assert_eq!(data, "O:4:\"Test\":0:{}");

    // sleep_uninitialized_typed_prop.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test")
        .typed("x", "int")
        .prop("y", PROTECTED, Some("int"), Value::Undef)
        .prop("z", PRIVATE, Some("int"), Value::Undef)
        .method(&SLEEP_XYZ)
        .register(&mut t);
    let o = new_obj(&mut t, "Test");
    assert_eq!(ser(&mut t, Value::Object(o.clone())), "O:4:\"Test\":0:{}");
    o.set_prop(b"x", Value::Int(1));
    o.set_prop(b"y", Value::Int(2));
    o.set_prop(b"z", Value::Int(3));
    let data = ser_bytes(&mut t, Value::Object(o));
    assert_eq!(
        data,
        b"O:4:\"Test\":3:{s:1:\"x\";i:1;s:4:\"\0*\0y\";i:2;s:7:\"\0Test\0z\";i:3;}"
    );
    let v = unser(&mut t, &data);
    expectf(
        &dump_out(&t, &v),
        "object(Test)#%d (3) {\n  [\"x\"]=>\n  int(1)\n  [\"y\":protected]=>\n  int(2)\n  [\"z\":\"Test\":private]=>\n  int(3)\n}",
    );

    // A non-array __sleep() result.
    let mut t = TestVm::new();
    ClassBuilder::new("Bad").method(&SLEEP_BAD).register(&mut t);
    let o = Value::Object(new_obj(&mut t, "Bad"));
    assert_eq!(ser(&mut t, list(vec![o])), "a:1:{i:0;N;}");
    expectf(
        &t.output(),
        "Warning: serialize(): Bad::__sleep() should return an array only containing the names of instance-variables to serialize in %s on line %d",
    );

    // sleep_deref.phpt + a non-string name.
    let mut t = TestVm::new();
    ClassBuilder::new("Test")
        .prop("x", member_flags::PUBLIC, None, Value::Int(42))
        .method(&SLEEP_NAMES)
        .register(&mut t);
    let o = Value::Object(new_obj(&mut t, "Test"));
    assert_eq!(ser(&mut t, o), "O:4:\"Test\":1:{s:1:\"x\";i:42;}");
    expectf(
        &t.output(),
        "Warning: serialize(): Test::__sleep() should return an array only containing the names of instance-variables to serialize in %s on line %d\n\nWarning: serialize(): \"1\" returned as member variable from __sleep() but does not exist in %s on line %d",
    );
}

fn wakeup_throw(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    vm.echo_str(&format!("{}::__wakeup\n", o.class().name))?;
    Err(vm.throw_error("Exception", "wakeup failed"))
}
static WAKEUP_THROW: NativeFunction = NativeFunction {
    sig: "__wakeup(): void",
    handler: wakeup_throw,
};

#[test]
fn delayed_wakeup_calls() {
    let mut t = TestVm::new();
    ClassBuilder::new("W")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .method(&WAKEUP_ECHO)
        .register(&mut t);
    ClassBuilder::new("Boom").method(&WAKEUP_THROW).register(&mut t);
    // Nested objects are woken up first.
    let v = unser(
        &mut t,
        b"O:1:\"W\":1:{s:1:\"x\";a:1:{i:0;O:1:\"W\":0:{}}}",
    );
    expectf(&t.output(), "W::__wakeup called\nW::__wakeup called");
    assert!(v.as_object().is_some());
    // __wakeup() still runs for complete objects when the data is invalid.
    let v = unser(&mut t, b"a:2:{i:0;O:1:\"W\":0:{}i:1;X}");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Error at offset 25 of 27 bytes in %s on line %d\nW::__wakeup called",
    );
    // An exception stops the remaining calls; their objects count as destructed.
    let r = t.call(
        "unserialize",
        vec![s("a:2:{i:0;O:4:\"Boom\":0:{}i:1;O:1:\"W\":0:{}}")],
    );
    assert_eq!(
        thrown(r),
        ("Exception".to_string(), "wakeup failed".to_string())
    );
    expectf(&t.output(), "Boom::__wakeup");
}

// ----- __serialize() / __unserialize() ------------------------------------------

fn ser_value_42(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let mut a = PhpArray::new();
    a.insert_str("value", o.get_prop(b"prop").unwrap_or_default());
    a.insert(
        php_runtime::ArrayKey::Int(42),
        o.get_prop(b"prop2").unwrap_or_default(),
    );
    Ok(Value::array(a))
}
static SER_VALUE_42: NativeFunction = NativeFunction {
    sig: "__serialize(): array",
    handler: ser_value_42,
};

fn unser_value_42(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let data = args.array(0).clone();
    o.set_prop(b"prop", data.get_str(b"value").cloned().unwrap_or_default());
    o.set_prop(b"prop2", data.get_int(42).cloned().unwrap_or_default());
    Ok(Value::Null)
}
static UNSER_VALUE_42: NativeFunction = NativeFunction {
    sig: "__unserialize(array $data): void",
    handler: unser_value_42,
};

fn ser_this(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Object(this(args)))
}
static SER_THIS: NativeFunction = NativeFunction {
    sig: "__serialize()",
    handler: ser_this,
};

#[test]
fn magic_serialize_001_002() {
    // __serialize_001.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test")
        .public("prop")
        .public("prop2")
        .method(&SER_VALUE_42)
        .method(&UNSER_VALUE_42)
        .register(&mut t);
    let o = new_obj(&mut t, "Test");
    o.set_prop(b"prop", s("foobar"));
    o.set_prop(b"prop2", s("barfoo"));
    let data = ser(&mut t, Value::Object(o));
    assert_eq!(
        data,
        "O:4:\"Test\":2:{s:5:\"value\";s:6:\"foobar\";i:42;s:6:\"barfoo\";}"
    );
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump(&t, &v),
        "object(Test)#%d (2) {\n  [\"prop\"]=>\n  string(6) \"foobar\"\n  [\"prop2\"]=>\n  string(6) \"barfoo\"\n}",
    );

    // __serialize_002.phpt
    ClassBuilder::new("Test2").method(&SER_THIS).register(&mut t);
    let o = Value::Object(new_obj(&mut t, "Test2"));
    assert_eq!(
        thrown(t.call("serialize", vec![o])),
        (
            "TypeError".to_string(),
            "Test2::__serialize() must return an array".to_string()
        )
    );
}

fn ser_data(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(this(args).get_prop(b"data").unwrap_or_default())
}
static SER_DATA: NativeFunction = NativeFunction {
    sig: "__serialize(): array",
    handler: ser_data,
};

fn unser_data(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let data = args.get(0).clone();
    o.set_prop(b"data", data.clone());
    vm.echo_str("__unserialize() called\n")?;
    let mut dumped = String::new();
    dumped.push_str(&dump_value(vm, &data));
    vm.echo_str(&dumped)?;
    o.set_prop(b"unserialized", Value::Bool(true));
    Ok(Value::Null)
}
static UNSER_DATA: NativeFunction = NativeFunction {
    sig: "__unserialize(array $data): void",
    handler: unser_data,
};

fn wakeup_dump(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    vm.echo_str("__wakeup() called\n")?;
    let d = dump_value(vm, &o.get_prop(b"data").unwrap_or_default());
    vm.echo_str(&d)?;
    o.set_prop(b"woken_up", Value::Bool(true));
    Ok(Value::Null)
}
static WAKEUP_DUMP: NativeFunction = NativeFunction {
    sig: "__wakeup(): void",
    handler: wakeup_dump,
};

/// `var_dump()` from inside a native method.
fn dump_value(vm: &mut Vm, v: &Value) -> String {
    dump_prec(vm.serialize_precision, v)
}

#[test]
fn magic_unserialize_is_delayed_004() {
    // __serialize_004.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Wakeup")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .public("data")
        .method(&WAKEUP_DUMP)
        .register(&mut t);
    ClassBuilder::new("Unserialize")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .public("data")
        .method(&SER_DATA)
        .method(&UNSER_DATA)
        .register(&mut t);
    let mk = |t: &mut TestVm, class: &str, data: Value| {
        let o = new_obj(t, class);
        o.set_prop(b"data", data);
        Value::Object(o)
    };
    let u2 = mk(&mut t, "Unserialize", Value::empty_array());
    let w2 = mk(&mut t, "Wakeup", list(vec![u2]));
    let u1 = mk(&mut t, "Unserialize", list(vec![w2]));
    let w1 = mk(&mut t, "Wakeup", list(vec![u1]));
    let data = ser(&mut t, w1);
    assert_eq!(
        data,
        "O:6:\"Wakeup\":1:{s:4:\"data\";a:1:{i:0;O:11:\"Unserialize\":1:{i:0;O:6:\"Wakeup\":1:{s:4:\"data\";a:1:{i:0;O:11:\"Unserialize\":0:{}}}}}}"
    );
    let v = unser(&mut t, data.as_bytes());
    let expected = r#"__unserialize() called
array(0) {
}
__wakeup() called
array(1) {
  [0]=>
  object(Unserialize)#%d (2) {
    ["data"]=>
    array(0) {
    }
    ["unserialized"]=>
    bool(true)
  }
}
__unserialize() called
array(1) {
  [0]=>
  object(Wakeup)#%d (2) {
    ["data"]=>
    array(1) {
      [0]=>
      object(Unserialize)#%d (2) {
        ["data"]=>
        array(0) {
        }
        ["unserialized"]=>
        bool(true)
      }
    }
    ["woken_up"]=>
    bool(true)
  }
}
__wakeup() called
array(1) {
  [0]=>
  object(Unserialize)#%d (2) {
    ["data"]=>
    array(1) {
      [0]=>
      object(Wakeup)#%d (2) {
        ["data"]=>
        array(1) {
          [0]=>
          object(Unserialize)#%d (2) {
            ["data"]=>
            array(0) {
            }
            ["unserialized"]=>
            bool(true)
          }
        }
        ["woken_up"]=>
        bool(true)
      }
    }
    ["unserialized"]=>
    bool(true)
  }
}
object(Wakeup)#%d (2) {
  ["data"]=>
  array(1) {
    [0]=>
    object(Unserialize)#%d (2) {
      ["data"]=>
      array(1) {
        [0]=>
        object(Wakeup)#%d (2) {
          ["data"]=>
          array(1) {
            [0]=>
            object(Unserialize)#%d (2) {
              ["data"]=>
              array(0) {
              }
              ["unserialized"]=>
              bool(true)
            }
          }
          ["woken_up"]=>
          bool(true)
        }
      }
      ["unserialized"]=>
      bool(true)
    }
  }
  ["woken_up"]=>
  bool(true)
}"#;
    expectf(&dump_out(&t, &v), expected);
}

fn a_ser(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(this(args).get_prop(b"data").unwrap_or_default())
}
static A_SER: NativeFunction = NativeFunction {
    sig: "__serialize(): array",
    handler: a_ser,
};
fn a_unser(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    this(args).set_prop(b"data", args.get(0).clone());
    Ok(Value::Null)
}
static A_UNSER: NativeFunction = NativeFunction {
    sig: "__unserialize(array $data): void",
    handler: a_unser,
};
fn b_ser(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    // [$this->data2, parent::__serialize()]
    let data2 = o.obj().props.borrow()[1].clone();
    let data = o.obj().props.borrow()[0].clone();
    Ok(list(vec![data2, data]))
}
static B_SER: NativeFunction = NativeFunction {
    sig: "__serialize(): array",
    handler: b_ser,
};
fn b_unser(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let payload = args.array(0).clone();
    let data2 = payload.get_int(0).cloned().unwrap_or_default();
    let data = payload.get_int(1).cloned().unwrap_or_default();
    o.obj().props.borrow_mut()[0] = data;
    o.obj().props.borrow_mut()[1] = data2;
    Ok(Value::Null)
}
static B_UNSER: NativeFunction = NativeFunction {
    sig: "__unserialize(array $payload): void",
    handler: b_unser,
};

#[test]
fn magic_serialize_back_references_005() {
    // __serialize_005.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("A")
        .prop("data", PRIVATE, None, Value::Null)
        .method(&A_SER)
        .method(&A_UNSER)
        .register(&mut t);
    ClassBuilder::extends(&t, "B", "A")
        .prop("data2", PRIVATE, None, Value::Null)
        .method(&B_SER)
        .method(&B_UNSER)
        .register(&mut t);
    let common = Value::Object(new_obj(&mut t, "stdClass"));
    let b = new_obj(&mut t, "B");
    b.obj().props.borrow_mut()[0] = list(vec![common.clone()]);
    b.obj().props.borrow_mut()[1] = list(vec![common]);
    let data = ser(&mut t, Value::Object(b));
    assert_eq!(
        data,
        "O:1:\"B\":2:{i:0;a:1:{i:0;O:8:\"stdClass\":0:{}}i:1;a:1:{i:0;r:3;}}"
    );
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump(&t, &v),
        "object(B)#%d (2) {\n  [\"data\":\"A\":private]=>\n  array(1) {\n    [0]=>\n    object(stdClass)#%d (0) {\n    }\n  }\n  [\"data2\":\"B\":private]=>\n  array(1) {\n    [0]=>\n    object(stdClass)#%d (0) {\n    }\n  }\n}",
    );
    let o = v.as_object().unwrap();
    let props = o.obj().props.borrow();
    let x = props[0].as_array().unwrap().get_int(0).unwrap().clone();
    let y = props[1].as_array().unwrap().get_int(0).unwrap().clone();
    assert!(x.identical(&y));
}

#[test]
fn magic_unserialize_failures() {
    // __serialize_006.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Test").method(&A_UNSER).register(&mut t);
    let v = unser(&mut t, b"O:4:\"Test\":1:{}");
    assert!(v.identical(&Value::Bool(false)));
    expectf(
        &t.output(),
        "Warning: unserialize(): Unexpected end of serialized data in %s on line %d\n\nWarning: unserialize(): Error at offset 14 of 15 bytes in %s on line %d",
    );
}

// ----- get_properties_for ---------------------------------------------------------

fn props_for(_vm: &mut Vm, obj: &ObjRef, purpose: PropPurpose) -> PResult<Option<PhpArray>> {
    if purpose != PropPurpose::Serialize {
        return Ok(None);
    }
    let mut t = PhpArray::new();
    t.insert_str("virtual", obj.get_prop(b"real").unwrap_or_default());
    Ok(Some(t))
}

#[test]
fn get_properties_for_handler() {
    let mut t = TestVm::new();
    let mut ce = ClassBuilder::new("Custom").public("real").build();
    std::rc::Rc::get_mut(&mut ce).unwrap().handlers.get_properties_for = Some(props_for);
    t.vm.register_class(ce);
    let o = new_obj(&mut t, "Custom");
    o.set_prop(b"real", Value::Int(5));
    assert_eq!(
        ser(&mut t, Value::Object(o)),
        "O:6:\"Custom\":1:{s:7:\"virtual\";i:5;}"
    );
}

#[test]
fn mangled_names_round_trip_through_incomplete_classes() {
    // incomplete_class_raw_payload_markers.phpt
    let mut t = TestVm::new();
    let raw = "x:i:0;a:1:{i:0;s:3:\"pwn\";};m:a:0:{}";
    let untrusted = format!(
        "a:2:{{i:0;O:11:\"ArrayObject\":1:{{s:33:\"__PHP_Incomplete_Class_Serialized\";s:{}:\"{raw}\";}}i:1;O:4:\"Suit\":1:{{s:26:\"__PHP_Incomplete_Enum_Case\";s:6:\"Hearts\";}}}}",
        raw.len()
    );
    let v = unser_opts(
        &mut t,
        untrusted.as_bytes(),
        vec![("allowed_classes", Value::Bool(false))],
    )
    .unwrap();
    assert_eq!(ser(&mut t, v), untrusted);
}

#[test]
fn incomplete_classes_only_mode() {
    use super::super::*;
    let mut t = TestVm::new();
    let data: &[u8] =
        b"a:3:{i:0;C:7:\"Missing\":4:{abcd}i:1;E:8:\"Suit:Ace\";i:2;O:8:\"stdClass\":1:{s:1:\"a\";i:1;}}";
    let d = var_unserialize_init(&mut t.vm);
    var_unserialize_set_incomplete_classes_only(&d, true);
    let mut pos = 0;
    let v = var_unserialize(&mut t.vm, &d, data, &mut pos)
        .unwrap()
        .expect("valid data");
    var_unserialize_destroy(&mut t.vm, d).unwrap();
    assert_eq!(pos, data.len());
    for i in 0..3 {
        let o = v.as_array().unwrap().get_int(i).unwrap().unref();
        assert_eq!(o.as_object().unwrap().class().name, "__PHP_Incomplete_Class");
    }
    let sd = var_serialize_init(&mut t.vm);
    var_serialize_set_incomplete_classes_only(&sd, true);
    let mut buf = Vec::new();
    var_serialize(&mut t.vm, &mut buf, &v, &sd).unwrap();
    var_serialize_destroy(&mut t.vm, sd);
    assert_eq!(buf, data);
    // Reserved members are rejected in this mode.
    let d = var_unserialize_init(&mut t.vm);
    var_unserialize_set_incomplete_classes_only(&d, true);
    let bad = b"O:1:\"X\":1:{s:27:\"__PHP_Incomplete_Class_Name\";s:1:\"Y\";}";
    let mut pos = 0;
    assert!(var_unserialize(&mut t.vm, &d, bad, &mut pos).unwrap().is_none());
    var_unserialize_destroy(&mut t.vm, d).unwrap();
}
