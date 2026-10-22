//! `Serializable`, nested (un)serialization contexts, enums, class lookup
//! (autoloading and `unserialize_callback_func`).

use std::cell::Cell;
use std::rc::Rc;

use php_runtime::class::{
    ClassConstant, ClassEntry, ClassRef, ConstValue, EnumBacking, PropertyInfo, class_flags,
    member_flags,
};
use php_runtime::types::TypeDecl;
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpArray, Value, Vm};

use super::harness::*;

fn this(args: &Args) -> ObjRef {
    args.this().cloned().expect("method called without $this")
}

// ----- Serializable -----------------------------------------------------------------

thread_local! {
    static CHECK: Cell<u32> = const { Cell::new(0) };
}

fn new_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("TestNew::serialize()\n")?;
    let n = CHECK.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    Ok(match n {
        1 => Value::Null,
        _ => s("2"),
    })
}
static NEW_SERIALIZE: NativeFunction = NativeFunction {
    sig: "serialize()",
    handler: new_serialize,
};

fn new_unserialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("TestNew::unserialize()\n")?;
    Ok(Value::Null)
}
static NEW_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize($serialized)",
    handler: new_unserialize,
};

#[test]
fn serializable_005() {
    // 005.phpt (the Serializable parts)
    let mut t = TestVm::new();
    ClassBuilder::new("TestNew")
        .implements(&t, "Serializable")
        .method(&NEW_SERIALIZE)
        .method(&NEW_UNSERIALIZE)
        .register(&mut t);
    ClassBuilder::extends(&t, "TestNANew", "TestNew").register(&mut t);
    ClassBuilder::extends(&t, "TestNANew2", "TestNew").register(&mut t);

    // ===N1===
    let o = Value::Object(new_obj(&mut t, "TestNew"));
    let data = ser(&mut t, o);
    assert_eq!(data, "N;");
    let v = unser(&mut t, data.as_bytes());
    expectf(&dump_out(&t, &v), "TestNew::serialize()\nNULL");
    // ===N2===
    let o = Value::Object(new_obj(&mut t, "TestNew"));
    let data = ser(&mut t, o);
    assert_eq!(data, "C:7:\"TestNew\":1:{2}");
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump_out(&t, &v),
        "TestNew::serialize()\nTestNew::unserialize()\nobject(TestNew)#%d (0) {\n}",
    );
    // ===NANew===
    let v = unser(&mut t, b"O:9:\"TestNANew\":0:{}");
    expectf(
        &dump_out(&t, &v),
        "Warning: Erroneous data format for unserializing 'TestNANew' in %s on line %d\n\nWarning: unserialize(): Error at offset 19 of 20 bytes in %s on line %d\nbool(false)",
    );
    // ===NANew2===
    let v = unser(&mut t, b"C:10:\"TestNANew2\":0:{}");
    expectf(
        &dump_out(&t, &v),
        "TestNew::unserialize()\nobject(TestNANew2)#%d (0) {\n}",
    );
    // A class without Serializable cannot read C: data.
    let v = unser(&mut t, b"C:8:\"stdClass\":0:{}");
    expectf(
        &dump_out(&t, &v),
        "Warning: Class stdClass has no unserializer in %s on line %d\n\nWarning: unserialize(): Error at offset 18 of 19 bytes in %s on line %d\nbool(false)",
    );
}

fn null_serialize(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}
static NULL_SERIALIZE: NativeFunction = NativeFunction {
    sig: "serialize()",
    handler: null_serialize,
};
fn int_serialize(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(1))
}
static INT_SERIALIZE: NativeFunction = NativeFunction {
    sig: "serialize()",
    handler: int_serialize,
};
fn noop_unserialize(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}
static NOOP_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize($serialized)",
    handler: noop_unserialize,
};

#[test]
fn failed_serializable_results() {
    // ref_to_failed_serialize.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("NotSerializable")
        .implements(&t, "Serializable")
        .method(&NULL_SERIALIZE)
        .method(&NOOP_UNSERIALIZE)
        .register(&mut t);
    let o = Value::Object(new_obj(&mut t, "NotSerializable"));
    let data = ser(&mut t, list(vec![o.clone(), o]));
    assert_eq!(data, "a:2:{i:0;N;i:1;N;}");
    let v = unser(&mut t, data.as_bytes());
    assert_eq!(
        dump(&t, &v),
        "array(2) {\n  [0]=>\n  NULL\n  [1]=>\n  NULL\n}\n"
    );

    ClassBuilder::new("BadSer")
        .implements(&t, "Serializable")
        .method(&INT_SERIALIZE)
        .method(&NOOP_UNSERIALIZE)
        .register(&mut t);
    let o = Value::Object(new_obj(&mut t, "BadSer"));
    assert_eq!(
        thrown(t.call("serialize", vec![o])),
        (
            "Exception".to_string(),
            "BadSer::serialize() must return a string or NULL".to_string()
        )
    );
}

fn vars_serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // return serialize(get_object_vars($this));
    let o = this(args);
    let vars = o.obj().dyn_props.borrow().clone().unwrap_or_default();
    vm.call_native("serialize", vec![Value::array(vars)])
}
static VARS_SERIALIZE: NativeFunction = NativeFunction {
    sig: "serialize()",
    handler: vars_serialize,
};

fn vars_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // foreach (unserialize($s) as $p => $v) $this->$p = $v;
    let o = this(args);
    let data = vm.call_native("unserialize", vec![args.get(0).clone()])?;
    if let Value::Array(a) = data {
        for (k, v) in a.iter() {
            o.set_prop(&k.to_php_str(), v.unref());
        }
    }
    Ok(Value::Null)
}
static VARS_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize($s)",
    handler: vars_unserialize,
};

#[test]
fn nested_contexts_share_back_references() {
    // bug36424.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("a")
        .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
        .implements(&t, "Serializable")
        .method(&VARS_SERIALIZE)
        .method(&VARS_UNSERIALIZE)
        .register(&mut t);
    ClassBuilder::extends(&t, "b", "a").register(&mut t);
    ClassBuilder::extends(&t, "c", "b").register(&mut t);
    let c = new_obj(&mut t, "c");
    let a = new_obj(&mut t, "a");
    let b = new_obj(&mut t, "b");
    c.set_prop(b"a", Value::Object(a.clone()));
    a.set_prop(b"b", Value::Object(b.clone()));
    b.set_prop(b"c", Value::Object(c.clone()));
    a.set_prop(b"c", Value::Object(c.clone()));
    b.set_prop(b"a", Value::Object(a.clone()));
    a.set_prop(b"a", Value::Object(a.clone()));
    let data = ser(&mut t, Value::Object(c));
    assert_eq!(
        data,
        "C:1:\"c\":108:{a:1:{s:1:\"a\";C:1:\"a\":81:{a:3:{s:1:\"b\";C:1:\"b\":30:{a:2:{s:1:\"c\";r:1;s:1:\"a\";r:3;}}s:1:\"c\";r:1;s:1:\"a\";r:3;}}}}"
    );
    let d = unser(&mut t, data.as_bytes());
    assert_eq!(t.output(), "");
    let get = |v: &Value, p: &str| v.as_object().unwrap().get_prop(p.as_bytes()).unwrap();
    let da = get(&d, "a");
    let dab = get(&da, "b");
    assert!(d.identical(&get(&dab, "c")));
    assert!(get(&da, "a").identical(&da));
    assert!(get(&dab, "a").identical(&da));
    assert!(get(&da, "c").identical(&d));
    assert_eq!(d.as_object().unwrap().class().name, "c");
    assert_eq!(dab.as_object().unwrap().class().name, "b");
}

fn interop_sleep(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("__sleep() called\n")?;
    Ok(Value::Null)
}
static INTEROP_SLEEP: NativeFunction = NativeFunction {
    sig: "__sleep()",
    handler: interop_sleep,
};
fn interop_wakeup(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("__wakeup() called\n")?;
    Ok(Value::Null)
}
static INTEROP_WAKEUP: NativeFunction = NativeFunction {
    sig: "__wakeup()",
    handler: interop_wakeup,
};
fn interop_magic_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("__serialize() called\n")?;
    Ok(assoc(vec![("key", s("value"))]))
}
static INTEROP_MAGIC_SERIALIZE: NativeFunction = NativeFunction {
    sig: "__serialize()",
    handler: interop_magic_serialize,
};
fn interop_magic_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.echo_str("__unserialize() called\n")?;
    let d = dump_prec(vm.serialize_precision, args.get(0));
    vm.echo_str(&d)?;
    Ok(Value::Null)
}
static INTEROP_MAGIC_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "__unserialize(array $data)",
    handler: interop_magic_unserialize,
};
fn interop_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("serialize() called\n")?;
    Ok(s("payload"))
}
static INTEROP_SERIALIZE: NativeFunction = NativeFunction {
    sig: "serialize()",
    handler: interop_serialize,
};
fn interop_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.echo_str("unserialize() called\n")?;
    let d = dump_prec(vm.serialize_precision, args.get(0));
    vm.echo_str(&d)?;
    Ok(Value::Null)
}
static INTEROP_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize($payload)",
    handler: interop_unserialize,
};

#[test]
fn serialization_mechanisms_interoperate() {
    // __serialize_003.phpt
    let mut t = TestVm::new();
    let ce = ClassBuilder::new("Test")
        .implements(&t, "Serializable")
        .method(&INTEROP_SLEEP)
        .method(&INTEROP_WAKEUP)
        .method(&INTEROP_MAGIC_SERIALIZE)
        .method(&INTEROP_MAGIC_UNSERIALIZE)
        .method(&INTEROP_SERIALIZE)
        .method(&INTEROP_UNSERIALIZE)
        .register(&mut t);
    // Having both __serialize() and __unserialize() avoids the deprecation.
    super::super::implement_serializable(&mut t.vm, &ce).unwrap();
    assert_eq!(t.output(), "");
    let o = Value::Object(new_obj(&mut t, "Test"));
    let data = ser(&mut t, o);
    assert_eq!(data, "O:4:\"Test\":1:{s:3:\"key\";s:5:\"value\";}");
    let v = unser(&mut t, data.as_bytes());
    let v2 = unser(&mut t, b"C:4:\"Test\":7:{payload}");
    let out = t.output();
    expectf(
        &format!("{out}{}{}", dump(&t, &v), dump(&t, &v2)),
        "__serialize() called\n__unserialize() called\narray(1) {\n  [\"key\"]=>\n  string(5) \"value\"\n}\nunserialize() called\nstring(7) \"payload\"\nobject(Test)#%d (0) {\n}\nobject(Test)#%d (0) {\n}",
    );
}

#[test]
fn serializable_deprecation() {
    // Zend/tests/serialize/serializable_deprecation.phpt
    let mut t = TestVm::new();
    let ce = ClassBuilder::new("C1")
        .implements(&t, "Serializable")
        .method(&NULL_SERIALIZE)
        .method(&NOOP_UNSERIALIZE)
        .build();
    super::super::implement_serializable(&mut t.vm, &ce).unwrap();
    expectf(
        &t.output(),
        "Deprecated: C1 implements the Serializable interface, which is deprecated. Implement __serialize() and __unserialize() instead (or in addition, if support for old PHP versions is necessary) in %s on line %d",
    );
    let abs = ClassBuilder::new("C2")
        .flags(class_flags::ABSTRACT)
        .implements(&t, "Serializable")
        .build();
    super::super::implement_serializable(&mut t.vm, &abs).unwrap();
    assert_eq!(t.output(), "");
}

fn foo_magic_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // $this->foo = unserialize($foo['bar']);
    let bar = args.array(0).get_str(b"bar").cloned().unwrap_or_default();
    let v = vm.call_native("unserialize", vec![bar])?;
    this(args).set_prop(b"foo", v);
    Ok(Value::Null)
}
static FOO_MAGIC_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "__unserialize(array $foo)",
    handler: foo_magic_unserialize,
};
fn foo_magic_serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let foo = this(args).get_prop(b"foo").unwrap_or_default();
    let mut data = vm
        .call_native("serialize", vec![foo])?
        .as_str()
        .unwrap()
        .clone();
    data.push_str("garbage");
    Ok(assoc(vec![("bar", Value::String(data))]))
}
static FOO_MAGIC_SERIALIZE: NativeFunction = NativeFunction {
    sig: "__serialize(): array",
    handler: foo_magic_serialize,
};
fn foo_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = vm.call_native("unserialize", vec![args.get(0).clone()])?;
    this(args).set_prop(b"foo", v);
    Ok(Value::Null)
}
static FOO_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize(string $foo)",
    handler: foo_unserialize,
};
fn foo_serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let foo = this(args).get_prop(b"foo").unwrap_or_default();
    let mut data = vm
        .call_native("serialize", vec![foo])?
        .as_str()
        .unwrap()
        .clone();
    data.push_str("garbage");
    Ok(Value::String(data))
}
static FOO_SERIALIZE: NativeFunction = NativeFunction {
    sig: "serialize(): string",
    handler: foo_serialize,
};

#[test]
fn extra_data_in_nested_calls() {
    let abc = || list(vec![s("a"), s("b"), s("c")]);
    let want = "object(Foo)#%d (1) {\n  [\"foo\"]=>\n  array(3) {\n    [0]=>\n    string(1) \"a\"\n    [1]=>\n    string(1) \"b\"\n    [2]=>\n    string(1) \"c\"\n  }\n}";
    // unserialize_extra_data_002.phpt: __unserialize() runs after the outer
    // call's own warning.
    let mut t = TestVm::new();
    ClassBuilder::new("Foo")
        .public("foo")
        .method(&FOO_MAGIC_UNSERIALIZE)
        .method(&FOO_MAGIC_SERIALIZE)
        .register(&mut t);
    let f = new_obj(&mut t, "Foo");
    f.set_prop(b"foo", abc());
    let mut data = ser(&mut t, Value::Object(f));
    data.push_str("garbage");
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump_out(&t, &v),
        &format!(
            "Warning: unserialize(): Extra data starting at offset 81 of 88 bytes in %s on line %d\n\nWarning: unserialize(): Extra data starting at offset 42 of 49 bytes in %s on line %d\n{want}"
        ),
    );
    // unserialize_extra_data_003.phpt: Serializable::unserialize() runs
    // during the outer parse.
    let mut t = TestVm::new();
    ClassBuilder::new("Foo")
        .implements(&t, "Serializable")
        .public("foo")
        .method(&FOO_UNSERIALIZE)
        .method(&FOO_SERIALIZE)
        .register(&mut t);
    let f = new_obj(&mut t, "Foo");
    f.set_prop(b"foo", abc());
    let mut data = ser(&mut t, Value::Object(f));
    data.push_str("garbage");
    let v = unser(&mut t, data.as_bytes());
    expectf(
        &dump_out(&t, &v),
        &format!(
            "Warning: unserialize(): Extra data starting at offset 42 of 49 bytes in %s on line %d\n\nWarning: unserialize(): Extra data starting at offset 64 of 71 bytes in %s on line %d\n{want}"
        ),
    );
}

fn nested(depth: usize, inner: &str) -> String {
    format!("{}{inner}{}", "a:1:{i:0;".repeat(depth), "}".repeat(depth))
}

fn depth_unserialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // Should fail, due to combined nesting level.
    let r = vm.call_native("unserialize", vec![s(&nested(129, "i:0;"))])?;
    let d = dump_prec(-1, &r);
    vm.echo_str(&d)?;
    // Should succeed, below combined nesting level.
    let r = vm.call_native("unserialize", vec![s(&nested(128, "i:0;"))])?;
    vm.echo_str(&format!("bool({})\n", !r.identical(&Value::Bool(false))))?;
    Ok(Value::Null)
}
static DEPTH_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize($str)",
    handler: depth_unserialize,
};

fn depth2_unserialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // An explicit max_depth counts from zero again.
    let mut o = PhpArray::new();
    o.insert_str("max_depth", Value::Int(256));
    let o = Value::array(o);
    let r = vm.call_native("unserialize", vec![s(&nested(257, "i:0;")), o.clone()])?;
    let d = dump_prec(-1, &r);
    vm.echo_str(&d)?;
    let r = vm.call_native("unserialize", vec![s(&nested(256, "i:0;")), o])?;
    vm.echo_str(&format!("bool({})\n", !r.identical(&Value::Bool(false))))?;
    Ok(Value::Null)
}
static DEPTH2_UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "unserialize($str)",
    handler: depth2_unserialize,
};

#[test]
fn nested_max_depth() {
    // max_depth.phpt (nested parts)
    let mut t = TestVm::new();
    ClassBuilder::new("Test")
        .implements(&t, "Serializable")
        .method(&NULL_SERIALIZE)
        .method(&DEPTH_UNSERIALIZE)
        .register(&mut t);
    ClassBuilder::new("Test2")
        .implements(&t, "Serializable")
        .method(&NULL_SERIALIZE)
        .method(&DEPTH2_UNSERIALIZE)
        .register(&mut t);
    let v = unser_opts(
        &mut t,
        nested(128, "C:4:\"Test\":0:{}").as_bytes(),
        vec![("max_depth", Value::Int(256))],
    )
    .unwrap();
    assert!(v.as_array().is_some());
    expectf(
        &t.output(),
        "Warning: unserialize(): Maximum depth of 256 exceeded. The depth limit can be changed using the max_depth unserialize() option or the unserialize_max_depth ini setting in %s on line %d\n\nWarning: unserialize(): Error at offset 1157 of 1294 bytes in %s on line %d\nbool(false)\nbool(true)",
    );
    let v = unser_opts(
        &mut t,
        nested(64, "C:5:\"Test2\":0:{}").as_bytes(),
        vec![("max_depth", Value::Int(128))],
    )
    .unwrap();
    assert!(v.as_array().is_some());
    expectf(
        &t.output(),
        "Warning: unserialize(): Maximum depth of 256 exceeded. The depth limit can be changed using the max_depth unserialize() option or the unserialize_max_depth ini setting in %s on line %d\n\nWarning: unserialize(): Error at offset 2309 of 2574 bytes in %s on line %d\nbool(false)\nbool(true)",
    );
}

// ----- enums ---------------------------------------------------------------------------

/// A pure enum with `cases`, plus constants aliasing cases.
fn make_enum(t: &mut TestVm, name: &str, cases: &[&str], aliases: &[(&str, &str)]) -> ClassRef {
    let mut ce = ClassEntry::new(name, class_flags::ENUM | class_flags::FINAL);
    ce.enum_backing = Some(EnumBacking::Pure);
    let mut p = PropertyInfo::new("name", name, member_flags::PUBLIC | member_flags::READONLY);
    p.ty = Some(TypeDecl::parse("string").unwrap());
    p.default = Value::Undef;
    ce.declare_property(p);
    for c in cases {
        let mut cc = ClassConstant::new(*c, Value::Null, name);
        cc.flags |= member_flags::ENUM_CASE;
        ce.add_constant(cc);
    }
    for (c, _) in aliases {
        ce.add_constant(ClassConstant::new(*c, Value::Null, name));
    }
    let ce = Rc::new(ce);
    for c in cases {
        let obj = ObjRef::new(ce.clone());
        obj.set_prop(b"name", s(c));
        *ce.find_constant(c.as_bytes()).unwrap().value.borrow_mut() =
            ConstValue::Value(Value::Object(obj));
    }
    for (c, target) in aliases {
        let v = ce
            .find_constant(target.as_bytes())
            .unwrap()
            .evaluated()
            .unwrap();
        *ce.find_constant(c.as_bytes()).unwrap().value.borrow_mut() = ConstValue::Value(v);
    }
    t.vm.register_class(ce.clone());
    ce
}

#[test]
fn enums() {
    // Zend/tests/enum/serialize.phpt, unserialize.phpt
    let mut t = TestVm::new();
    let ce = make_enum(&mut t, "Foo", &["Bar", "Quux"], &[("Baz", "Bar")]);
    let bar = ce.find_constant(b"Bar").unwrap().evaluated().unwrap();
    assert_eq!(ser(&mut t, bar.clone()), "E:7:\"Foo:Bar\";");
    let back = unser(&mut t, b"E:7:\"Foo:Bar\";");
    assert!(back.identical(&bar));
    let arr = ser(&mut t, list(vec![bar.clone(), bar.clone()]));
    // Case objects are shared, so repetitions are back references.
    assert_eq!(arr, "a:2:{i:0;E:7:\"Foo:Bar\";i:1;r:2;}");
    let back = unser(&mut t, arr.as_bytes());
    let a = back.as_array().unwrap();
    assert!(a.get_int(0).unwrap().identical(&bar));
    assert!(a.get_int(1).unwrap().identical(&bar));
    expectf(&dump(&t, &bar), "enum(Foo::Bar)");

    // unserialize-missing-colon.phpt
    let v = unser(&mut t, b"E:6:\"FooBar\";");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Invalid enum name 'FooBar' (missing colon) in %s on line %d\n\nWarning: unserialize(): Error at offset 0 of 13 bytes in %s on line %d\nbool(false)",
    );
    // unserialize-non-enum.phpt
    ClassBuilder::new("NotEnum").register(&mut t);
    let v = unser(&mut t, b"E:11:\"NotEnum:Bar\";");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Class 'NotEnum' is not an enum in %s on line %d\n\nWarning: unserialize(): Error at offset 0 of 19 bytes in %s on line %d\nbool(false)",
    );
    // unserialize-non-existent-case.phpt
    let v = unser(&mut t, b"E:7:\"Foo:Bat\";");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Undefined constant Foo::Bat in %s on line %d\n\nWarning: unserialize(): Error at offset 14 of 14 bytes in %s on line %d\nbool(false)",
    );
    // unserialize-const.phpt
    let v = unser(&mut t, b"E:7:\"Foo:Baz\";");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Foo::Baz is not an enum case in %s on line %d\n\nWarning: unserialize(): Error at offset 14 of 14 bytes in %s on line %d\nbool(false)",
    );
    // Unknown classes.
    let v = unser(&mut t, b"E:8:\"Nope:Bar\";");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Class 'Nope' not found in %s on line %d\n\nWarning: unserialize(): Error at offset 0 of 15 bytes in %s on line %d\nbool(false)",
    );
    // Enums cannot be instantiated from O: data.
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:3:\"Foo\":0:{}")])),
        (
            "Error".to_string(),
            "Cannot instantiate enum Foo".to_string()
        )
    );
    // Not as keys either.
    let v = unser(&mut t, b"a:1:{E:7:\"Foo:Bar\";i:0;}");
    assert!(v.identical(&Value::Bool(false)));
    assert!(t.output().contains("Error at offset 5 of"));
}

// ----- class lookup --------------------------------------------------------------------

fn incomplete_foo() -> &'static str {
    "object(__PHP_Incomplete_Class)#%d (1) {\n  [\"__PHP_Incomplete_Class_Name\"]=>\n  string(3) \"FOO\"\n}"
}

#[test]
fn autoload_is_called_twice_with_callback() {
    // unserialize_callback_func/autoload_must_be_called_twice.phpt
    let mut t = TestVm::new();
    let host = TestHost::install(&mut t);
    host.autoloader(|vm, name| {
        vm.echo_str(&format!("in autoload({})\n", String::from_utf8_lossy(name)))?;
        Ok(None)
    });
    host.function("check", |vm, args| {
        let name = args[0].as_str().unwrap().to_string_lossy().into_owned();
        vm.echo_str(&format!("in check({name})\n"))?;
        Ok(Value::Null)
    });
    t.vm.ini_set("unserialize_callback_func", "check");
    let v = unser(&mut t, b"O:3:\"FOO\":0:{}");
    expectf(
        &dump_out(&t, &v),
        &format!(
            "in autoload(FOO)\nin check(FOO)\nin autoload(FOO)\n\nWarning: unserialize(): Function check() hasn't defined the class it was called for in %s on line %d\n{}",
            incomplete_foo()
        ),
    );
}

#[test]
fn callback_defines_the_class() {
    let mut t = TestVm::new();
    let host = TestHost::install(&mut t);
    host.function("define", |vm, _args| {
        vm.register_class(ClassBuilder::new("Foo").public("a").build());
        Ok(Value::Null)
    });
    t.vm.ini_set("unserialize_callback_func", "define");
    let v = unser(&mut t, b"O:3:\"FOO\":1:{s:1:\"a\";i:1;}");
    expectf(
        &dump_out(&t, &v),
        "object(Foo)#%d (1) {\n  [\"a\"]=>\n  int(1)\n}",
    );
}

#[test]
fn invalid_callbacks() {
    // unserialize_callback_func/non_existing.phpt, fn_with_null_bytes.phpt
    let mut t = TestVm::new();
    t.vm.ini_set("unserialize_callback_func", "Nonexistent");
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:3:\"FOO\":0:{}")])),
        (
            "Error".to_string(),
            "Invalid callback Nonexistent, function \"Nonexistent\" not found or invalid function name".to_string()
        )
    );
    t.vm.ini_set("unserialize_callback_func", "foo\0butno");
    assert_eq!(
        thrown(t.call("unserialize", vec![s("O:3:\"FOO\":0:{}")])),
        (
            "Error".to_string(),
            "Invalid callback foo, function \"foo\" not found or invalid function name".to_string()
        )
    );
    // An empty value disables the callback (gh23082.phpt).
    t.vm.ini_set("unserialize_callback_func", "");
    let v = unser(&mut t, b"O:3:\"FOO\":0:{}");
    expectf(&dump_out(&t, &v), incomplete_foo());
}

#[test]
fn callback_changing_the_ini_setting() {
    // unserialize_callback_func_INI_modifications.phpt
    let mut t = TestVm::new();
    let host = TestHost::install(&mut t);
    host.function("unserialize_cb_original", |vm, _args| {
        vm.ini_set("unserialize_callback_func", "unserialize_cb_changed");
        Ok(Value::Null)
    });
    host.function("unserialize_cb_changed", |_vm, _args| Ok(Value::Null));
    t.vm.ini_set("unserialize_callback_func", "unserialize_cb_original");
    unser(&mut t, b"O:3:\"FOO\":0:{}");
    expectf(
        &t.output(),
        "Warning: unserialize(): Function unserialize_cb_original() hasn't defined the class it was called for in %s on line %d",
    );
    assert_eq!(
        t.vm.ini_get("unserialize_callback_func").unwrap(),
        "unserialize_cb_changed"
    );
}

#[test]
fn autoloader_exceptions_propagate() {
    // bug64354_1.phpt, unserialize_callback_func/bug26762.phpt
    let mut t = TestVm::new();
    let host = TestHost::install(&mut t);
    host.autoloader(|vm, _name| Err(vm.throw_error("Exception", "Failed")));
    for data in [
        "O:1:\"A\":0:{}",
        "a:2:{i:0;O:1:\"A\":0:{}i:1;O:1:\"A\":0:{}}",
    ] {
        assert_eq!(
            thrown(t.call("unserialize", vec![s(data)])),
            ("Exception".to_string(), "Failed".to_string())
        );
    }
    assert_eq!(t.output(), "");
}

#[test]
fn class_lookup_runs_with_a_fresh_context() {
    // unserialize_callback_func/bug70213.phpt
    let mut t = TestVm::new();
    let host = TestHost::install(&mut t);
    let h = host.clone();
    host.function("evil", move |_vm, _args| {
        h.autoloader(|vm, _name| {
            let r = vm.call_native("unserialize", vec![s("R:1;")])?;
            let d = dump_prec(-1, &r);
            vm.echo_str(&d)?;
            Ok(None)
        });
        Ok(Value::Null)
    });
    t.vm.ini_set("unserialize_callback_func", "evil");
    let v = unser(&mut t, b"a:2:{i:0;i:42;i:1;O:4:\"evil\":0:{}}");
    expectf(
        &dump_out(&t, &v),
        "Warning: unserialize(): Error at offset 4 of 4 bytes in %s on line %d\nbool(false)\n\nWarning: unserialize(): Function evil() hasn't defined the class it was called for in %s on line %d\narray(2) {\n  [0]=>\n  int(42)\n  [1]=>\n  object(__PHP_Incomplete_Class)#%d (1) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(4) \"evil\"\n  }\n}",
    );

    // bug62836_1.phpt: the autoloader unserializes too.
    let mut t = TestVm::new();
    let host = TestHost::install(&mut t);
    host.autoloader(|vm, name| {
        vm.call_native("unserialize", vec![s("i:4;")])?;
        let name = String::from_utf8_lossy(name).into_owned();
        let ce = ClassBuilder::new(&name)
            .flags(class_flags::ALLOW_DYNAMIC_PROPERTIES)
            .build();
        vm.register_class(ce.clone());
        Ok(Some(ce))
    });
    let v = unser(
        &mut t,
        b"O:1:\"A\":4:{s:1:\"b\";O:1:\"B\":0:{}s:2:\"b1\";r:2;s:1:\"c\";O:1:\"B\":0:{}s:2:\"c1\";r:4;}",
    );
    let o = v.as_object().unwrap();
    let p = |n: &str| o.get_prop(n.as_bytes()).unwrap();
    assert!(p("b").identical(&p("b1")));
    assert!(p("c").identical(&p("c1")));
    assert!(!p("b").identical(&p("c")));
    assert_eq!(p("c").as_object().unwrap().class().name, "B");
    assert_eq!(t.output(), "");
}

#[test]
fn allowed_classes_and_custom_format() {
    // incomplete_class_c_format_allowed_classes.phpt
    let mut t = TestVm::new();
    ClassBuilder::new("Kept")
        .flags(class_flags::FINAL)
        .prop("a", member_flags::PUBLIC, None, Value::Int(1))
        .register(&mut t);
    ClassBuilder::new("Blocked")
        .flags(class_flags::FINAL)
        .implements(&t, "Serializable")
        .method(&NEW_UNSERIALIZE)
        .register(&mut t);
    let payload = "a:4:{i:0;O:4:\"Kept\":1:{s:1:\"a\";i:2;}i:1;C:7:\"Blocked\":4:{abcd}i:2;C:7:\"Missing\":4:{abcd}i:3;r:4;}";
    let v = unser_opts(
        &mut t,
        payload.as_bytes(),
        vec![("allowed_classes", list(vec![s("Kept")]))],
    )
    .unwrap();
    expectf(
        &dump_out(&t, &v),
        "Warning: Class __PHP_Incomplete_Class has no unserializer in %s on line %d\n\nWarning: Class __PHP_Incomplete_Class has no unserializer in %s on line %d\narray(4) {\n  [0]=>\n  object(Kept)#%d (1) {\n    [\"a\"]=>\n    int(2)\n  }\n  [1]=>\n  object(__PHP_Incomplete_Class)#%d (1) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(7) \"Blocked\"\n  }\n  [2]=>\n  object(__PHP_Incomplete_Class)#%d (1) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(7) \"Missing\"\n  }\n  [3]=>\n  object(__PHP_Incomplete_Class)#%d (1) {\n    [\"__PHP_Incomplete_Class_Name\"]=>\n    string(7) \"Blocked\"\n  }\n}",
    );
}
