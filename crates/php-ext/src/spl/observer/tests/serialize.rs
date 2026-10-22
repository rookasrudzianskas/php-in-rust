//! `SplObjectStorage` serialization: the `Serializable` format, `serialize()`
//! / `unserialize()` through `__serialize()` / `__unserialize()`, and their
//! errors.

use php_runtime::class::member_flags;
use php_runtime::{ArrayKey, NativeFunction, PResult, PhpArray, Value, Vm};

use super::super::super::testutil::{array_of, call, list, new_obj, s, user_class};
use super::{
    assert_phpt, declared_class, dim_read, dim_write, dump, dump_count, int_of, obj, state,
    std_obj, storage,
};
use crate::testing::{TestVm, thrown};
use php_runtime::class::DimFetch;

fn serialize(vm: &mut Vm, v: &Value) -> String {
    match vm.call_native("serialize", vec![v.clone()]).unwrap() {
        Value::String(s) => s.to_string_lossy().into_owned(),
        other => panic!("serialize() returned {other:?}"),
    }
}

fn unserialize(vm: &mut Vm, data: &str) -> PResult<Value> {
    vm.call_native("unserialize", vec![s(data)])
}

/// `catch (Exception $e) { echo $e::class, ': ', $e->getMessage(), "\n"; }`
fn caught(r: PResult<Value>) -> String {
    let (class, msg) = thrown(r);
    format!("{class}: {msg}\n")
}

// ---------------------------------------------------------------------------
// The Serializable format
// ---------------------------------------------------------------------------

#[test]
fn serializable_format_round_trip() {
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    a.set_prop(b"x", Value::Int(1));
    st.set_prop(b"member", s("m"));
    let vm = &mut t.vm;
    dim_write(vm, &st, &obj(&a), s("a")).unwrap();
    dim_write(vm, &st, &obj(&b), list(vec![obj(&a)])).unwrap();
    let data = call(vm, &st, "serialize", vec![]).unwrap();
    assert_eq!(
        data.as_str().unwrap().to_string(),
        "x:i:2;O:8:\"stdClass\":1:{s:1:\"x\";i:1;},s:1:\"a\";;O:8:\"stdClass\":0:{},a:1:{i:0;r:2;};m:a:1:{s:6:\"member\";s:1:\"m\";}"
    );
    let copy = new_obj(vm, "SplObjectStorage", vec![]).unwrap();
    call(vm, &copy, "unserialize", vec![data]).unwrap();
    assert_eq!(int_of(call(vm, &copy, "count", vec![])), 2);
    assert!(copy.get_prop(b"member").unwrap().identical(&s("m")));
    // The back-reference r:2 resolved to the first object.
    call(vm, &copy, "rewind", vec![]).unwrap();
    let first = call(vm, &copy, "current", vec![]).unwrap();
    call(vm, &copy, "next", vec![]).unwrap();
    let info = call(vm, &copy, "getInfo", vec![]).unwrap();
    assert!(array_of(&info).get_int(0).unwrap().identical(&first));
    // An empty storage.
    let empty = new_obj(vm, "SplObjectStorage", vec![]).unwrap();
    let data = call(vm, &empty, "serialize", vec![]).unwrap();
    assert_eq!(data.as_str().unwrap().to_string(), "x:i:0;m:a:0:{}");
    // The old format without infos is still accepted.
    call(
        vm,
        &empty,
        "unserialize",
        vec![s("x:i:1;O:8:\"stdClass\":0:{};m:a:0:{}")],
    )
    .unwrap();
    call(vm, &empty, "rewind", vec![]).unwrap();
    assert!(call(vm, &empty, "getInfo", vec![]).unwrap().is_null());
}

#[test]
fn unserialize_bad_blobs() {
    // SplObjectStorage_unserialize_bad.phpt
    let mut t = TestVm::new();
    let blobs = [
        "x:i:2;i:0;,i:1;;i:0;,i:2;;m:a:0:{}",
        "x:i:3;O:8:\"stdClass\":0:{},O:8:\"stdClass\":0:{};R:2;,i:1;;O:8:\"stdClass\":0:{},r:2;;m:a:0:{}",
        "x:i:3;O:8:\"stdClass\":0:{},O:8:\"stdClass\":0:{};r:2;,i:1;;O:8:\"stdClass\":0:{},r:2;;m:a:0:{}",
        "x:i:1;O:8:\"stdClass\":0:{},N;;m:s:40:\"1234567890123456789012345678901234567890\"",
    ];
    let mut out = String::new();
    for blob in blobs {
        let so = storage(&mut t);
        match call(&mut t.vm, &so, "unserialize", vec![s(blob)]) {
            Ok(_) => out += &dump(&mut t.vm, &obj(&so)),
            Err(e) => out += &caught(Err(e)),
        }
    }
    out += "DONE\n";
    assert_phpt(
        "SplObjectStorage/SplObjectStorage_unserialize_bad.phpt",
        &out,
    );
    // The handles of the third blob: the first object is attached twice
    // (its info replaced), the third one refers back to it.
    let so = storage(&mut t);
    call(&mut t.vm, &so, "unserialize", vec![s(blobs[2])]).unwrap();
    call(&mut t.vm, &so, "rewind", vec![]).unwrap();
    let first = call(&mut t.vm, &so, "current", vec![]).unwrap();
    call(&mut t.vm, &so, "next", vec![]).unwrap();
    let inf = call(&mut t.vm, &so, "getInfo", vec![]).unwrap();
    assert!(inf.identical(&first));
}

#[test]
fn unserialize_invalid_parameters() {
    // SplObjectStorage_unserialize_invalid_parameter2.phpt, ..._parameter3.phpt
    let mut t = TestVm::new();
    let inputs = [
        Value::Int(12345),
        Value::Float(1.2345),
        Value::Int(i64::MAX),
        s("x:rubbish"),
        s("x:i:2;O:8:\"stdClass\":0:{},s:5:\"value\";;m:a:0:{}"),
    ];
    let mut out = String::new();
    let mut offsets = Vec::new();
    for input in inputs {
        let so = storage(&mut t);
        let (class, msg) = thrown(call(&mut t.vm, &so, "unserialize", vec![input]));
        out += &format!("{class}: {msg}\n");
        offsets.push(msg);
    }
    assert_phpt(
        "SplObjectStorage/SplObjectStorage_unserialize_invalid_parameter2.phpt",
        &out,
    );
    assert_eq!(
        offsets,
        [
            "Error at offset 0 of 5 bytes",
            "Error at offset 0 of 6 bytes",
            "Error at offset 0 of 19 bytes",
            "Error at offset 2 of 9 bytes",
            "Error at offset 39 of 47 bytes",
        ]
    );
    let so = storage(&mut t);
    assert!(
        call(&mut t.vm, &so, "unserialize", vec![s("")])
            .unwrap()
            .is_null()
    );
    for (blob, msg) in [
        ("y", "Error at offset 0 of 1 bytes"),
        ("xy", "Error at offset 1 of 2 bytes"),
        ("x:s:1:\"a\";", "Error at offset 10 of 10 bytes"),
        ("x:i:-1;m:a:0:{}", "Error at offset 6 of 15 bytes"),
        ("x:i:0;", "Error at offset 6 of 6 bytes"),
        ("x:i:0;n:a:0:{}", "Error at offset 6 of 14 bytes"),
        ("x:i:0;mx", "Error at offset 7 of 8 bytes"),
        ("x:i:0;m:i:1;", "Error at offset 12 of 12 bytes"),
        ("x:i:1;i:1;,N;;m:a:0:{}", "Error at offset 6 of 22 bytes"),
    ] {
        let (class, m) = thrown(call(&mut t.vm, &so, "unserialize", vec![s(blob)]));
        assert_eq!(class, "UnexpectedValueException", "{blob}");
        assert_eq!(m, msg, "{blob}");
    }
}

static HASH_THROWS: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, _args| Err(vm.throw_error("RuntimeException", "no hash")),
};

#[test]
fn unserialize_propagates_get_hash_exceptions() {
    let mut t = TestVm::new();
    user_class(&mut t.vm, "NoHash", "SplObjectStorage", &[&HASH_THROWS]);
    let so = new_obj(&mut t.vm, "NoHash", vec![]).unwrap();
    let r = call(
        &mut t.vm,
        &so,
        "unserialize",
        vec![s("x:i:1;O:8:\"stdClass\":0:{},N;;m:a:0:{}")],
    );
    let (class, msg) = thrown(r);
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("RuntimeException", "no hash")
    );
}

#[test]
fn unserialize_members_create_dynamic_properties() {
    let mut t = TestVm::new();
    let so = storage(&mut t);
    call(
        &mut t.vm,
        &so,
        "unserialize",
        vec![s("x:i:0;m:a:2:{s:3:\"foo\";i:1;i:5;s:1:\"x\";}")],
    )
    .unwrap();
    assert!(so.get_prop(b"foo").unwrap().identical(&Value::Int(1)));
    assert!(so.get_prop(b"5").unwrap().identical(&s("x")));
    assert_eq!(
        t.output(),
        "\nDeprecated: Creation of dynamic property SplObjectStorage::$foo is deprecated in /t.php on line 3\n\
         \nDeprecated: Creation of dynamic property SplObjectStorage::$5 is deprecated in /t.php on line 3\n"
    );
}

// ---------------------------------------------------------------------------
// serialize() / unserialize() (__serialize / __unserialize)
// ---------------------------------------------------------------------------

#[test]
fn nested_and_linked_storage() {
    // SplObjectStorage_unserialize_nested.phpt
    let mut t = TestVm::new();
    let o = std_obj(&t.vm);
    let a = std_obj(&t.vm);
    o.set_prop(b"a", obj(&a));
    let so = storage(&mut t);
    let vm = &mut t.vm;
    dim_write(vm, &so, &obj(&o), Value::Int(1)).unwrap();
    dim_write(vm, &so, &obj(&a), Value::Int(2)).unwrap();
    let ser = serialize(vm, &obj(&so));
    let so1 = unserialize(vm, &ser).unwrap();
    let out = format!("{ser}\n{}", dump(vm, &so1));
    assert_phpt(
        "SplObjectStorage/SplObjectStorage_unserialize_nested.phpt",
        &out,
    );
}

#[test]
fn serialization_references() {
    // bug49263.phpt
    let mut t = TestVm::new();
    let o1 = std_obj(&t.vm);
    let o2 = std_obj(&t.vm);
    let s1 = storage(&mut t);
    let vm = &mut t.vm;
    let mut inf1 = PhpArray::new();
    inf1.insert_str("prev", Value::Int(2));
    inf1.insert_str("next", obj(&o2));
    call(vm, &s1, "offsetSet", vec![obj(&o1), Value::array(inf1)]).unwrap();
    let mut inf2 = PhpArray::new();
    inf2.insert_str("prev", obj(&o1));
    call(vm, &s1, "offsetSet", vec![obj(&o2), Value::array(inf2)]).unwrap();
    let ss = serialize(vm, &obj(&s1));
    drop((s1, o1, o2));
    let v = unserialize(vm, &ss).unwrap();
    let out = format!("{ss}\n{}", dump(vm, &v));
    assert_phpt("SplObjectStorage/bug49263.phpt", &out);
}

#[test]
fn unserialize_reference_infos_become_values() {
    // SplObjectStorage_unserialize_reference.phpt
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let o = std_obj(&t.vm);
    let vm = &mut t.vm;
    let y = php_runtime::PhpRef::new(Value::Int(1));
    let x = list(vec![obj(&o), Value::Ref(y.clone())]);
    call(
        vm,
        &st,
        "__unserialize",
        vec![list(vec![x, Value::empty_array()])],
    )
    .unwrap();
    let mut out = dump(vm, &obj(&st));
    // $val = $s[$o]; $val = 123; (and the original reference changing)
    let val = dim_read(vm, &st, &obj(&o), DimFetch::Read).unwrap();
    assert!(!val.is_ref());
    y.set(Value::Int(5));
    out += &dump(vm, &obj(&st));
    assert_phpt(
        "SplObjectStorage/SplObjectStorage_unserialize_reference.phpt",
        &out,
    );
}

#[test]
fn magic_unserialize_errors() {
    // unserialize_errors.phpt (SplObjectStorage)
    let mut t = TestVm::new();
    let mut out = String::new();
    for data in [
        "O:16:\"SplObjectStorage\":0:{}",
        "O:16:\"SplObjectStorage\":2:{i:0;i:0;i:1;a:0:{}}",
        "O:16:\"SplObjectStorage\":2:{i:0;a:0:{}i:1;i:1;}",
        "O:16:\"SplObjectStorage\":2:{i:0;a:1:{i:0;i:0;}i:1;a:0:{}}",
        "O:16:\"SplObjectStorage\":2:{i:0;a:2:{i:0;i:0;i:1;i:0;}i:1;a:0:{}}",
    ] {
        out += &caught(unserialize(&mut t.vm, data));
    }
    assert_eq!(
        out,
        "UnexpectedValueException: Incomplete or ill-typed serialization data\n\
         UnexpectedValueException: Incomplete or ill-typed serialization data\n\
         UnexpectedValueException: Incomplete or ill-typed serialization data\n\
         UnexpectedValueException: Odd number of elements\n\
         UnexpectedValueException: Non-object key\n"
    );
    // A reference is not an array.
    let st = storage(&mut t);
    let r = php_runtime::PhpRef::new(Value::empty_array());
    let data = list(vec![Value::Ref(r), Value::empty_array()]);
    let (_, msg) = thrown(call(&mut t.vm, &st, "__unserialize", vec![data]));
    assert_eq!(msg, "Incomplete or ill-typed serialization data");
}

#[test]
fn serializable_c_format_and_empty_payload() {
    // unserialize.phpt (SplObjectStorage)
    let mut t = TestVm::new();
    let fresh = storage(&mut t);
    let exp = serialize(&mut t.vm, &obj(&fresh));
    assert_eq!(exp, "O:16:\"SplObjectStorage\":2:{i:0;a:0:{}i:1;a:0:{}}");
    let o = unserialize(&mut t.vm, "C:16:\"SplObjectStorage\":0:{}").unwrap();
    let out = dump(&mut t.vm, &o);
    assert!(out.starts_with("object(SplObjectStorage)#"));
    assert!(out.ends_with(
        " (1) {\n  [\"storage\":\"SplObjectStorage\":private]=>\n  array(0) {\n  }\n}\n"
    ));
    assert_eq!(serialize(&mut t.vm, &o), exp);
}

#[test]
fn nested_serializable_payload_shares_back_references() {
    // bug70168.phpt: R:3 refers to the count of the inner payload.
    let mut t = TestVm::new();
    let inner = "x:i:1;O:8:\"stdClass\":0:{};m:a:0:{}";
    let exploit = format!(
        "a:2:{{i:0;C:16:\"SplObjectStorage\":{}:{{{inner}}}i:1;R:3;}}",
        inner.len()
    );
    let data = unserialize(&mut t.vm, &exploit).unwrap();
    let out = dump(&mut t.vm, &data);
    assert_phpt("SplObjectStorage/bug70168.phpt", &out);
}

static C_SERIALIZE: NativeFunction = NativeFunction {
    sig: "function __serialize(): array {}",
    handler: |vm, _args| {
        let store = state(vm).target.clone().unwrap();
        call(vm, &store, "removeAll", vec![obj(&store)])?;
        Ok(Value::empty_array())
    },
};

#[test]
fn serialize_survives_modification_by_user_code() {
    // gh16588.phpt
    let mut t = TestVm::new();
    declared_class(&mut t.vm, "C", None, &[], &[&C_SERIALIZE]);
    let store = storage(&mut t);
    let c = new_obj(&mut t.vm, "C", vec![]).unwrap();
    let inf = std_obj(&t.vm);
    let vm = &mut t.vm;
    state(vm).target = Some(store.clone());
    dim_write(vm, &store, &obj(&c), obj(&inf)).unwrap();
    drop((c, inf));
    let r = call(vm, &store, "serialize", vec![]).unwrap();
    assert_phpt("gh16588.phpt", &dump(vm, &r));
}

// ---------------------------------------------------------------------------
// Subclasses with properties
// ---------------------------------------------------------------------------

fn test_class(vm: &mut Vm) {
    declared_class(
        vm,
        "TestClass",
        None,
        &[
            ("def", member_flags::PUBLIC, Value::Int(24)),
            ("pub", member_flags::PUBLIC, Value::Int(25)),
            ("pro", member_flags::PROTECTED, Value::Int(26)),
            ("pri", member_flags::PRIVATE, Value::Int(27)),
        ],
        &[],
    );
}

fn test_object(vm: &mut Vm, pub_: i64, pro: i64, pri: i64) -> Value {
    let o = new_obj(vm, "TestClass", vec![]).unwrap();
    o.set_prop(b"pub", Value::Int(pub_));
    o.set_prop(b"pro", Value::Int(pro));
    o.set_prop(b"pri", Value::Int(pri));
    Value::Object(o)
}

/// `foreach ($storage as $object) { var_dump($object); }`
fn dump_objects(vm: &mut Vm, st: &Value) -> String {
    let Value::Object(st) = st else {
        panic!("not a storage")
    };
    let mut out = String::new();
    call(vm, st, "rewind", vec![]).unwrap();
    while call(vm, st, "valid", vec![])
        .unwrap()
        .identical(&Value::Bool(true))
    {
        let o = call(vm, st, "current", vec![]).unwrap();
        out += &dump(vm, &o);
        call(vm, st, "next", vec![]).unwrap();
    }
    out
}

#[test]
fn serialization_and_visibility() {
    // observer_005.phpt
    let mut t = TestVm::new();
    test_class(&mut t.vm);
    declared_class(
        &mut t.vm,
        "MyStorage",
        Some("SplObjectStorage"),
        &[
            ("def", member_flags::PUBLIC, Value::Int(24)),
            ("pub", member_flags::PUBLIC, Value::Int(25)),
            ("pro", member_flags::PROTECTED, Value::Int(26)),
            ("pri", member_flags::PRIVATE, Value::Int(27)),
        ],
        &[],
    );
    let vm = &mut t.vm;
    let st = new_obj(vm, "MyStorage", vec![]).unwrap();
    st.set_prop(b"pub", Value::Int(1));
    st.set_prop(b"pro", Value::Int(2));
    st.set_prop(b"pri", Value::Int(3));
    for (a, b, c) in [(4, 5, 6), (7, 8, 9)] {
        let o = test_object(vm, a, b, c);
        call(vm, &st, "offsetSet", vec![o]).unwrap();
    }
    let st = obj(&st);
    let mut out = dump_count(vm, &st);
    out += &dump_objects(vm, &st);
    out += &dump(vm, &st);
    let ser = serialize(vm, &st);
    out += &dump(vm, &s(&ser));
    out += "===UNSERIALIZE===\n";
    let st2 = unserialize(vm, &ser).unwrap();
    out += &dump_count(vm, &st2);
    out += &dump_objects(vm, &st2);
    out += &dump(vm, &st2);
    assert_phpt("SplObjectStorage/observer_005.phpt", &out);
}

#[test]
fn serialization_with_public_members() {
    // observer_004.phpt
    let mut t = TestVm::new();
    declared_class(
        &mut t.vm,
        "TestClass",
        None,
        &[("test", member_flags::PUBLIC, Value::Int(25))],
        &[],
    );
    declared_class(
        &mut t.vm,
        "MyStorage",
        Some("SplObjectStorage"),
        &[("bla", member_flags::PUBLIC, Value::Int(25))],
        &[],
    );
    let vm = &mut t.vm;
    let st = new_obj(vm, "MyStorage", vec![]).unwrap();
    st.set_prop(b"bla", Value::Int(26));
    for v in [1, 2] {
        let o = new_obj(vm, "TestClass", vec![]).unwrap();
        o.set_prop(b"test", Value::Int(v));
        call(vm, &st, "offsetSet", vec![obj(&o)]).unwrap();
    }
    let st = obj(&st);
    let test_values = |vm: &mut Vm, st: &Value| -> String {
        let Value::Object(st) = st else {
            unreachable!()
        };
        let mut out = String::new();
        call(vm, st, "rewind", vec![]).unwrap();
        while call(vm, st, "valid", vec![])
            .unwrap()
            .identical(&Value::Bool(true))
        {
            let Value::Object(o) = call(vm, st, "current", vec![]).unwrap() else {
                unreachable!()
            };
            out += &dump(vm, &o.get_prop(b"test").unwrap());
            call(vm, st, "next", vec![]).unwrap();
        }
        out
    };
    let mut out = dump_count(vm, &st);
    out += &test_values(vm, &st);
    out += &dump(vm, &st);
    let ser = serialize(vm, &st);
    out += &dump(vm, &s(&ser));
    out += "===UNSERIALIZE===\n";
    let st2 = unserialize(vm, &ser).unwrap();
    out += &dump_count(vm, &st2);
    out += &test_values(vm, &st2);
    out += &dump(vm, &st2);
    assert_phpt("SplObjectStorage/observer_004.phpt", &out);
}

#[test]
fn serialization_of_scalar_members() {
    // observer_003.phpt
    let mut t = TestVm::new();
    declared_class(
        &mut t.vm,
        "TestClass",
        None,
        &[("test", member_flags::PUBLIC, Value::Int(25))],
        &[],
    );
    let st = storage(&mut t);
    let vm = &mut t.vm;
    for v in [Value::Int(1), s("2"), s("foo"), Value::Bool(true)] {
        let o = new_obj(vm, "TestClass", vec![]).unwrap();
        o.set_prop(b"test", v);
        call(vm, &st, "offsetSet", vec![obj(&o)]).unwrap();
    }
    let ser = serialize(vm, &obj(&st));
    assert_eq!(
        ser,
        "O:16:\"SplObjectStorage\":2:{i:0;a:8:{i:0;O:9:\"TestClass\":1:{s:4:\"test\";i:1;}i:1;N;\
         i:2;O:9:\"TestClass\":1:{s:4:\"test\";s:1:\"2\";}i:3;N;\
         i:4;O:9:\"TestClass\":1:{s:4:\"test\";s:3:\"foo\";}i:5;N;\
         i:6;O:9:\"TestClass\":1:{s:4:\"test\";b:1;}i:7;N;}i:1;a:0:{}}"
    );
    let st2 = unserialize(vm, &ser).unwrap();
    let Value::Object(st2) = st2 else {
        panic!("not an object")
    };
    let mut values = Vec::new();
    call(vm, &st2, "rewind", vec![]).unwrap();
    while call(vm, &st2, "valid", vec![])
        .unwrap()
        .identical(&Value::Bool(true))
    {
        let Value::Object(o) = call(vm, &st2, "current", vec![]).unwrap() else {
            unreachable!()
        };
        values.push(o.get_prop(b"test").unwrap());
        call(vm, &st2, "next", vec![]).unwrap();
    }
    assert_eq!(values.len(), 4);
    for (got, want) in values
        .iter()
        .zip([Value::Int(1), s("2"), s("foo"), Value::Bool(true)])
    {
        assert!(got.identical(&want));
    }
}

#[test]
fn protected_members_round_trip() {
    // bug71838.phpt
    let mut t = TestVm::new();
    declared_class(
        &mut t.vm,
        "A",
        Some("SplObjectStorage"),
        &[("a", member_flags::PROTECTED, Value::Null)],
        &[],
    );
    let a = new_obj(&mut t.vm, "A", vec![]).unwrap();
    a.set_prop(b"a", s("123"));
    let ser = serialize(&mut t.vm, &obj(&a));
    let Value::Object(back) = unserialize(&mut t.vm, &ser).unwrap() else {
        panic!("not an object")
    };
    assert!(back.get_prop(b"a").unwrap().identical(&s("123")));
    assert_eq!(t.output(), "");
}

#[test]
fn magic_serialize_uses_the_property_table() {
    // serialize_property_tables.phpt (MySplObjectStorage)
    let mut t = TestVm::new();
    declared_class(
        &mut t.vm,
        "MySplObjectStorage",
        Some("SplObjectStorage"),
        &[("unused", member_flags::PRIVATE, Value::Int(123))],
        &[],
    );
    let x = new_obj(&mut t.vm, "MySplObjectStorage", vec![]).unwrap();
    let r = call(&mut t.vm, &x, "__serialize", vec![]).unwrap();
    let out = dump(&mut t.vm, &r).replace('\0', "%0");
    assert_eq!(
        out,
        "array(2) {\n  [0]=>\n  array(0) {\n  }\n  [1]=>\n  array(1) {\n    [\"%0MySplObjectStorage%0unused\"]=>\n    int(123)\n  }\n}\n"
    );
    // Numeric property names become integer keys (zend_proptable_to_symtable).
    let st = storage(&mut t);
    st.set_prop(b"7", Value::Int(1));
    let r = call(&mut t.vm, &st, "__serialize", vec![]).unwrap();
    let members = array_of(&r).get_int(1).unwrap().clone();
    assert!(array_of(&members).get_int(7).is_some());
}

#[test]
fn magic_unserialize_loads_declared_and_dynamic_members() {
    let mut t = TestVm::new();
    declared_class(
        &mut t.vm,
        "Typed",
        Some("SplObjectStorage"),
        &[("n", member_flags::PRIVATE, Value::Int(0))],
        &[],
    );
    let st = new_obj(&mut t.vm, "Typed", vec![]).unwrap();
    let mut members = PhpArray::new();
    members.insert(ArrayKey::Str("\0Typed\0n".into()), Value::Int(5));
    // A private property seen from another scope is a (mangled) dynamic one.
    members.insert(ArrayKey::Str("n".into()), Value::Int(6));
    let data = list(vec![Value::empty_array(), Value::array(members)]);
    call(&mut t.vm, &st, "__unserialize", vec![data]).unwrap();
    assert!(st.get_prop(b"n").unwrap().identical(&Value::Int(5)));
    let dyn_keys: Vec<String> = st
        .obj()
        .dyn_props
        .borrow()
        .as_ref()
        .unwrap()
        .keys()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(dyn_keys, ["n"]);
    assert_eq!(
        t.output(),
        "\nDeprecated: Creation of dynamic property Typed::$ is deprecated in /t.php on line 3\n"
    );
}

#[test]
fn many_objects_round_trip() {
    // bug69108.phpt (smaller)
    let mut t = TestVm::new();
    let b = storage(&mut t);
    let mut a = Vec::new();
    for _ in 0..1000 {
        let o = std_obj(&t.vm);
        call(&mut t.vm, &b, "offsetSet", vec![obj(&o)]).unwrap();
        a.push(obj(&o));
    }
    let c = serialize(&mut t.vm, &list(vec![list(a), obj(&b)]));
    let d = unserialize(&mut t.vm, &c).unwrap();
    let st = array_of(&d).get_int(1).unwrap().clone();
    assert_eq!(int_of(t.vm.call_native("count", vec![st])), 1000);
}
