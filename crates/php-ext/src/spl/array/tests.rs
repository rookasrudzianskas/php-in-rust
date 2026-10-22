//! Tests of `ArrayObject`, `ArrayIterator` and `RecursiveArrayIterator`,
//! ported from `tests/phpt/ext/spl/tests/ArrayObject/*.phpt` and the related
//! `bug*.phpt` / `gh*.phpt` files.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{
    ClassEntry, DimFetch, PropPurpose, PropertyInfo, class_flags, fn_flags, member_flags,
};
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Value, Vm,
};

use crate::spl::engine;
use crate::spl::testutil::{assoc, call, closure, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

const DEP_AO: &str = "\nDeprecated: ArrayObject::__construct(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n";
const DEP_AI: &str = "\nDeprecated: ArrayIterator::__construct(): Using an object as a backing array for ArrayIterator is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn obj(o: &ObjRef) -> Value {
    Value::Object(o.clone())
}

fn new(t: &mut TestVm, class: &str, args: Vec<Value>) -> ObjRef {
    new_obj(&mut t.vm, class, args).unwrap()
}

fn m(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> Value {
    call(&mut t.vm, o, name, args).unwrap()
}

/// `$o[$k]`.
fn rd(t: &mut TestVm, o: &ObjRef, k: Value) -> Value {
    let h = o.class().handlers.read_dimension.unwrap();
    h(&mut t.vm, o, Some(&k), DimFetch::Read).unwrap()
}

/// `$o[$k] = $v` (`None`: `$o[] = $v`).
fn wr(t: &mut TestVm, o: &ObjRef, k: Option<Value>, v: Value) {
    let h = o.class().handlers.write_dimension.unwrap();
    h(&mut t.vm, o, k.as_ref(), v).unwrap()
}

/// `isset($o[$k])` / `!empty($o[$k])`.
fn has(t: &mut TestVm, o: &ObjRef, k: Value, empty: bool) -> bool {
    let h = o.class().handlers.has_dimension.unwrap();
    h(&mut t.vm, o, &k, empty).unwrap()
}

/// `unset($o[$k])`.
fn del(t: &mut TestVm, o: &ObjRef, k: Value) {
    let h = o.class().handlers.unset_dimension.unwrap();
    h(&mut t.vm, o, &k).unwrap()
}

fn debug(t: &mut TestVm, o: &ObjRef) -> PhpArray {
    let h = o.class().handlers.get_debug_info.unwrap();
    h(&mut t.vm, o, PropPurpose::Debug).unwrap().unwrap()
}

/// Compact rendering for assertions: `[0=>1,"a"=>"x"]`, objects as
/// `Class{...}` with their debug table.
fn render(t: &mut TestVm, v: &Value) -> String {
    match v {
        Value::Undef => "undef".into(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f:?}"),
        Value::String(s) => format!("\"{}\"", esc(s.as_bytes())),
        Value::Array(a) => render_array(t, a),
        Value::Object(o) => {
            if o.class().handlers.get_debug_info.is_some() {
                let d = debug(t, o);
                format!("{}{}", o.class().name, render_array(t, &d))
            } else {
                let table = engine::std_properties(o);
                format!("{}{}", o.class().name, render_array(t, &table))
            }
        }
        Value::Resource(_) => "resource".into(),
        Value::Ref(r) if r.refcount() > 1 => format!("&{}", render(t, &r.get())),
        Value::Ref(r) => render(t, &r.get()),
    }
}

fn esc(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace('\0', "\\0")
}

fn render_array(t: &mut TestVm, a: &PhpArray) -> String {
    let mut parts = Vec::new();
    for (k, v) in a.iter() {
        let k = match k {
            ArrayKey::Int(i) => i.to_string(),
            ArrayKey::Str(s) => format!("\"{}\"", esc(s.as_bytes())),
        };
        parts.push(format!("{k}=>{}", render(t, v)));
    }
    format!("[{}]", parts.join(","))
}

fn show(t: &mut TestVm, o: &ObjRef) -> String {
    render(t, &obj(o))
}

fn show_value(t: &mut TestVm, v: &Value) -> String {
    render(t, v)
}

/// A class with declared properties `(name, flags, default)`.
fn class_with_props(
    vm: &mut Vm,
    name: &str,
    parent: Option<&str>,
    props: &[(&str, u32, Value)],
) -> ClassRef {
    let mut ce = match parent {
        Some(p) => ClassEntry::new_subclass(name, 0, &vm.find_class(p.as_bytes()).unwrap()),
        None => ClassEntry::new(name, 0),
    };
    for (n, flags, default) in props {
        let mut info = PropertyInfo::new(*n, name, *flags);
        info.default = default.clone();
        ce.declare_property(info);
    }
    ce.link_magic_methods();
    let ce = Rc::new(ce);
    vm.register_class(ce.clone());
    ce
}

const PUB: u32 = member_flags::PUBLIC;
const PRO: u32 = member_flags::PROTECTED;
const PRI: u32 = member_flags::PRIVATE;

fn std_object(t: &mut TestVm, props: &[(&str, Value)]) -> ObjRef {
    let ce = t.vm.find_class(b"stdClass").unwrap();
    let o = ObjRef::new(ce);
    for (k, v) in props {
        o.set_prop(k.as_bytes(), v.clone());
    }
    o
}

/// `foreach ($it as $k => $v)` through the Iterator methods.
fn walk(t: &mut TestVm, it: &ObjRef) -> Vec<(String, String)> {
    let mut out = Vec::new();
    m(t, it, "rewind", vec![]);
    while m(t, it, "valid", vec![]).identical(&Value::Bool(true)) {
        let v = m(t, it, "current", vec![]);
        let k = m(t, it, "key", vec![]);
        let (k, v) = (render(t, &k), render(t, &v));
        out.push((k, v));
        m(t, it, "next", vec![]);
        if out.len() > 100 {
            panic!("runaway iteration");
        }
    }
    out
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn classes_constants_and_interfaces() {
    let mut t = TestVm::new();
    let r = t.call("class_implements", vec![s("ArrayObject")]).unwrap();
    let names: Vec<String> = crate::spl::testutil::entries(&r)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        names,
        [
            "IteratorAggregate",
            "Traversable",
            "ArrayAccess",
            "Serializable",
            "Countable"
        ]
    );
    let r = t
        .call("class_implements", vec![s("RecursiveArrayIterator")])
        .unwrap();
    let names: Vec<String> = crate::spl::testutil::entries(&r)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        names,
        [
            "SeekableIterator",
            "Traversable",
            "Iterator",
            "ArrayAccess",
            "Serializable",
            "Countable",
            "RecursiveIterator"
        ]
    );
    for (class, name, value) in [
        ("ArrayObject", "STD_PROP_LIST", 1),
        ("ArrayObject", "ARRAY_AS_PROPS", 2),
        ("ArrayIterator", "STD_PROP_LIST", 1),
        ("ArrayIterator", "ARRAY_AS_PROPS", 2),
        // bug75242.phpt: inherited by RecursiveArrayIterator.
        ("RecursiveArrayIterator", "STD_PROP_LIST", 1),
        ("RecursiveArrayIterator", "ARRAY_AS_PROPS", 2),
        ("RecursiveArrayIterator", "CHILD_ARRAYS_ONLY", 4),
    ] {
        let v = t.vm.class_constant(class, name).unwrap().unwrap();
        assert!(v.identical(&int(value)), "{class}::{name}");
    }
    let ai = t.vm.find_class(b"ArrayIterator").unwrap();
    for (name, deprecated) in [
        ("getflags", true),
        ("asort", true),
        ("serialize", true),
        ("__serialize", false),
        ("count", false),
        ("seek", false),
    ] {
        let f = ai.find_method(name.as_bytes()).unwrap();
        assert_eq!(f.has_flag(fn_flags::DEPRECATED), deprecated, "{name}");
        assert_eq!(f.scope.as_ref().unwrap().to_string(), "ArrayIterator");
    }
    let ao = t.vm.find_class(b"ArrayObject").unwrap();
    assert!(
        !ao.find_method(b"asort")
            .unwrap()
            .has_flag(fn_flags::DEPRECATED)
    );
    // bug62262.phpt: RecursiveArrayIterator is Countable.
    let rai = t.vm.find_class(b"RecursiveArrayIterator").unwrap();
    assert!(rai.instance_of_name(b"Countable"));
    assert_eq!(
        rai.parent.as_ref().unwrap().name.to_string(),
        "ArrayIterator"
    );
}

// ---------------------------------------------------------------------------
// Dimensions
// ---------------------------------------------------------------------------

#[test]
fn array_001_dimension_handlers() {
    let mut t = TestVm::new();
    let ar = new(&mut t, "ArrayObject", vec![list(vec![int(0), int(1)])]);
    assert_eq!(
        show(&mut t, &ar),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>0,1=>1]]"
    );
    wr(&mut t, &ar, Some(int(2)), int(2));
    assert!(rd(&mut t, &ar, int(2)).identical(&int(2)));
    wr(&mut t, &ar, Some(s("3")), int(3));
    wr(&mut t, &ar, Some(s("a")), s("a"));
    assert_eq!(
        show(&mut t, &ar),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>0,1=>1,2=>2,3=>3,\"a\"=>\"a\"]]"
    );
    assert!(rd(&mut t, &ar, int(0)).identical(&int(0)));
    assert!(rd(&mut t, &ar, int(6)).is_null());
    assert!(rd(&mut t, &ar, s("b")).is_null());
    assert_eq!(
        t.output(),
        "\nWarning: Undefined array key 6 in /t.php on line 3\n\
         \nWarning: Undefined array key \"b\" in /t.php on line 3\n"
    );
    for k in [int(1), s("3"), s("a"), int(7), s("c")] {
        del(&mut t, &ar, k);
    }
    assert_eq!(
        show(&mut t, &ar),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>0,2=>2]]"
    );
    wr(&mut t, &ar, None, s("3"));
    wr(&mut t, &ar, None, int(4));
    assert_eq!(
        show(&mut t, &ar),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>0,2=>2,4=>\"3\",5=>4]]"
    );
    // (array) cast.
    let h = ar.class().handlers.get_properties_for.unwrap();
    let cast = h(&mut t.vm, &ar, PropPurpose::ArrayCast).unwrap().unwrap();
    assert_eq!(render_array(&mut t, &cast), "[0=>0,2=>2,4=>\"3\",5=>4]");
    assert!(t.output().is_empty());
}

#[test]
fn array_010_isset_empty_and_offset_methods() {
    let mut t = TestVm::new();
    let a = assoc(vec![
        (ArrayKey::Int(0), s("1st")),
        (ArrayKey::Int(1), int(1)),
        (ArrayKey::Int(2), s("3rd")),
        (skey("4th"), int(4)),
    ]);
    let o = new(&mut t, "ArrayObject", vec![a]);
    let keys = [int(0), int(1), int(2), s("4th"), s("5th"), int(6)];
    let empty: Vec<bool> = keys
        .iter()
        .map(|k| !has(&mut t, &o, k.clone(), true))
        .collect();
    assert_eq!(empty, [false, false, false, false, true, true]);
    let isset: Vec<bool> = keys
        .iter()
        .map(|k| has(&mut t, &o, k.clone(), false))
        .collect();
    assert_eq!(isset, [true, true, true, true, false, false]);
    assert!(m(&mut t, &o, "offsetGet", vec![s("4th")]).identical(&int(4)));
    m(&mut t, &o, "offsetSet", vec![int(1), s("Changed 1")]);
    m(&mut t, &o, "offsetSet", vec![s("5th"), s("Added 5th")]);
    m(&mut t, &o, "offsetSet", vec![int(6), s("Added 6")]);
    m(&mut t, &o, "offsetUnset", vec![int(2)]);
    m(&mut t, &o, "offsetUnset", vec![s("4th")]);
    m(&mut t, &o, "offsetUnset", vec![int(7)]);
    let copy = m(&mut t, &o, "getArrayCopy", vec![]);
    assert_eq!(
        show_value(&mut t, &copy),
        "[0=>\"1st\",1=>\"Changed 1\",\"5th\"=>\"Added 5th\",6=>\"Added 6\"]"
    );
    // arrayObject_offsetExists_nullcheck.phpt / bug28822.phpt
    let o = new(
        &mut t,
        "ArrayObject",
        vec![assoc(vec![(skey("foo"), Value::Null)])],
    );
    assert!(m(&mut t, &o, "offsetExists", vec![s("foo")]).identical(&Value::Bool(true)));
    assert!(m(&mut t, &o, "offsetExists", vec![s("nokey")]).identical(&Value::Bool(false)));
    assert!(!has(&mut t, &o, s("foo"), false));
    assert!(t.output().is_empty());
}

#[test]
fn numeric_string_and_special_keys() {
    let mut t = TestVm::new();
    // bug40872.phpt
    let it = new(&mut t, "ArrayIterator", vec![]);
    m(&mut t, &it, "offsetSet", vec![s("1"), s("a")]);
    m(&mut t, &it, "offsetSet", vec![int(2), s("b")]);
    assert!(m(&mut t, &it, "offsetExists", vec![int(1)]).identical(&Value::Bool(true)));
    assert!(m(&mut t, &it, "offsetExists", vec![s("2")]).identical(&Value::Bool(true)));
    // bug71028.phpt
    let a = new(&mut t, "ArrayIterator", vec![]);
    wr(&mut t, &a, Some(int(-1)), int(123));
    assert!(has(&mut t, &a, int(-1), false));
    wr(&mut t, &a, Some(int(-1)), s("okey"));
    assert!(rd(&mut t, &a, int(-1)).identical(&s("okey")));
    del(&mut t, &a, int(-1));
    assert!(!has(&mut t, &a, int(-1), false));
    // array_018.phpt
    let foo = new(&mut t, "ArrayObject", vec![]);
    m(&mut t, &foo, "offsetSet", vec![s("\0"), s("Foo")]);
    assert_eq!(
        show(&mut t, &foo),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[\"\\0\"=>\"Foo\"]]"
    );
    // bool and float keys
    let o = new(&mut t, "ArrayObject", vec![]);
    wr(&mut t, &o, Some(Value::Bool(true)), s("t"));
    wr(&mut t, &o, Some(Value::Float(2.0)), s("f"));
    assert!(rd(&mut t, &o, int(1)).identical(&s("t")));
    assert!(rd(&mut t, &o, int(2)).identical(&s("f")));
    assert!(t.output().is_empty());
}

#[test]
fn null_and_resource_offsets() {
    // bug62978.phpt, bug70852.phpt
    let mut t = TestVm::new();
    let a = new(&mut t, "ArrayObject", vec![]);
    assert!(rd(&mut t, &a, Value::Null).is_null());
    assert_eq!(
        t.output(),
        "\nDeprecated: Using null as an array offset is deprecated, use an empty string instead in /t.php on line 3\n\
         \nWarning: Undefined array key \"\" in /t.php on line 3\n"
    );
    // $a[null]['hurr'] = 'durr'
    let h = a.class().handlers.read_dimension.unwrap();
    let slot = h(&mut t.vm, &a, Some(&Value::Null), DimFetch::Write).unwrap();
    let Value::Ref(r) = slot else {
        panic!("write fetch must return a reference")
    };
    r.set(assoc(vec![(skey("hurr"), s("durr"))]));
    drop(r);
    assert_eq!(
        show(&mut t, &a),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[\"\"=>[\"hurr\"=>\"durr\"]]]"
    );
    assert!(!has(&mut t, &a, s("epic_magic"), false));
    let res = php_runtime::ResRef::new("stream", Box::new(()));
    let id = res.id();
    assert!(rd(&mut t, &a, Value::Resource(res)).is_null());
    assert_eq!(
        t.output(),
        format!(
            "\nDeprecated: Using null as an array offset is deprecated, use an empty string instead in /t.php on line 3\n\
             \nWarning: Resource ID#{id} used as offset, casting to integer ({id}) in /t.php on line 3\n\
             \nWarning: Undefined array key {id} in /t.php on line 3\n"
        )
    );
}

#[test]
fn write_fetches_return_references() {
    // array_026.phpt: $test['d1']['d2'] = 'hello'
    let mut t = TestVm::new();
    let o = new(&mut t, "ArrayObject", vec![]);
    let h = o.class().handlers.read_dimension.unwrap();
    for (k2, v) in [("d2", "hello"), ("d3", "world")] {
        let slot = h(&mut t.vm, &o, Some(&s("d1")), DimFetch::Write).unwrap();
        let Value::Ref(r) = slot else { panic!() };
        let mut inner = match r.get() {
            Value::Array(a) => (*a).clone(),
            _ => PhpArray::new(),
        };
        inner.insert(skey(k2), s(v));
        r.set(Value::array(inner));
    }
    assert_eq!(
        show(&mut t, &o),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[\"d1\"=>[\"d2\"=>\"hello\",\"d3\"=>\"world\"]]]"
    );
    // RW fetch of a missing key warns and creates it.
    let slot = h(&mut t.vm, &o, Some(&s("x")), DimFetch::ReadWrite).unwrap();
    assert!(matches!(slot, Value::Ref(_)));
    assert_eq!(
        t.output(),
        "\nWarning: Undefined array key \"x\" in /t.php on line 3\n"
    );
    // `$o[][] = 1` has no element to write to.
    let slot = h(&mut t.vm, &o, None, DimFetch::Write).unwrap();
    assert!(slot.is_undef());
    // bug66127.phpt: unset of missing keys is silent.
    del(&mut t, &o, int(0));
    assert!(t.output().is_empty());
}

#[test]
fn illegal_offsets() {
    // ArrayObject_illegal_offset.phpt
    let mut t = TestVm::new();
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(1), int(2), int(3)])],
    );
    let arr = Value::empty_array();
    let rdh = ao.class().handlers.read_dimension.unwrap();
    let wrh = ao.class().handlers.write_dimension.unwrap();
    let hash = ao.class().handlers.has_dimension.unwrap();
    let unh = ao.class().handlers.unset_dimension.unwrap();
    let r = rdh(&mut t.vm, &ao, Some(&arr), DimFetch::Read);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot access offset of type array on ArrayObject".into()
        )
    );
    let std = std_object(&mut t, &[]);
    let r = wrh(&mut t.vm, &ao, Some(&arr), obj(&std)).map(|_| Value::Null);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot access offset of type array on ArrayObject".into()
        )
    );
    let r = rdh(&mut t.vm, &ao, Some(&arr), DimFetch::Write);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot access offset of type array on ArrayObject".into()
        )
    );
    let r = hash(&mut t.vm, &ao, &arr, false).map(Value::Bool);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot access offset of type array in isset or empty".into()
        )
    );
    let r = unh(&mut t.vm, &ao, &arr).map(|_| Value::Null);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot unset offset of type array on ArrayObject".into()
        )
    );
    let r = rdh(&mut t.vm, &ao, Some(&obj(&std)), DimFetch::Read);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot access offset of type stdClass on ArrayObject".into()
        )
    );
}

// ---------------------------------------------------------------------------
// Construction, flags, iterator class
// ---------------------------------------------------------------------------

#[test]
fn copy_constructor_uses_other_storage() {
    // array_002.phpt
    let mut t = TestVm::new();
    let a = assoc(vec![
        (ArrayKey::Int(1), s("one")),
        (ArrayKey::Int(2), s("two")),
        (ArrayKey::Int(3), s("three")),
    ]);
    let object = new(&mut t, "ArrayObject", vec![a]);
    wr(&mut t, &object, None, s("four"));
    let outer = new(&mut t, "ArrayObject", vec![obj(&object)]);
    wr(&mut t, &outer, None, s("five"));
    assert_eq!(t.output(), DEP_AO);
    assert_eq!(
        show(&mut t, &outer),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>ArrayObject[\"\\0ArrayObject\\0storage\"=>\
         [1=>\"one\",2=>\"two\",3=>\"three\",4=>\"four\",5=>\"five\"]]]"
    );
}

#[test]
fn flags_are_masked_and_inherited() {
    // arrayObject_getFlags_basic2.phpt
    let mut t = TestVm::new();
    let ao = new(&mut t, "ArrayObject", vec![Value::empty_array(), int(3)]);
    assert!(m(&mut t, &ao, "getFlags", vec![]).identical(&int(3)));
    let ao2 = new(&mut t, "ArrayObject", vec![obj(&ao)]);
    assert!(m(&mut t, &ao2, "getFlags", vec![]).identical(&int(3)));
    let it = m(&mut t, &ao2, "getIterator", vec![]);
    let it = it.as_object().unwrap().clone();
    assert!(m(&mut t, &it, "getFlags", vec![]).identical(&int(3)));
    let ai = new(&mut t, "ArrayIterator", vec![obj(&ao)]);
    assert!(m(&mut t, &ai, "getFlags", vec![]).identical(&int(3)));
    let ao3 = new(&mut t, "ArrayObject", vec![obj(&ao), int(0)]);
    assert!(m(&mut t, &ao3, "getFlags", vec![]).identical(&int(0)));
    let dep_flags = "\nDeprecated: Method ArrayIterator::getFlags() is deprecated since 8.6 in /t.php on line 3\n";
    assert_eq!(
        t.output(),
        format!("{DEP_AO}{dep_flags}{DEP_AI}{dep_flags}{DEP_AO}")
    );
    // Only the low 16 bits are user flags.
    m(&mut t, &ao, "setFlags", vec![int(-1)]);
    assert!(m(&mut t, &ao, "getFlags", vec![]).identical(&int(0xFFFF)));
    let ao4 = new(
        &mut t,
        "ArrayObject",
        vec![Value::empty_array(), int(0x7_0001)],
    );
    assert!(m(&mut t, &ao4, "getFlags", vec![]).identical(&int(1)));
}

#[test]
fn iterator_class_validation() {
    // arrayObject___construct_error1.phpt, arrayObject_setIteratorClass_error1.phpt,
    // bug80719.phpt
    let mut t = TestVm::new();
    let a = std_object(&mut t, &[("p", int(1))]);
    let r = new_obj(
        &mut t.vm,
        "ArrayObject",
        vec![obj(&a), int(0), s("Exception")],
    )
    .map(Value::Object);
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "ArrayObject::__construct(): Argument #3 ($iteratorClass) must be a class name derived from ArrayIterator, \"Exception\" given".into()
        )
    );
    let r = new_obj(
        &mut t.vm,
        "ArrayObject",
        vec![obj(&a), int(0), s("nonExistentClassName")],
    )
    .map(Value::Object);
    assert_eq!(
        thrown(r).1,
        "ArrayObject::__construct(): Argument #3 ($iteratorClass) must be a class name derived from ArrayIterator, \"nonExistentClassName\" given"
    );
    let array = new(&mut t, "ArrayObject", vec![list(vec![int(42)])]);
    let r = call(
        &mut t.vm,
        &array,
        "setIteratorClass",
        vec![s("FilterIterator")],
    );
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "ArrayObject::setIteratorClass(): Argument #1 ($iteratorClass) must be a class name derived from ArrayIterator, \"FilterIterator\" given".into()
        )
    );
    let it = m(&mut t, &array, "getIterator", vec![]);
    let it = it.as_object().unwrap().clone();
    assert_eq!(it.class().name.to_string(), "ArrayIterator");
    assert_eq!(walk(&mut t, &it), pairs(&[("0", "42")]));
    // arrayObject_getIteratorClass_basic1.phpt
    user_class(&mut t.vm, "MyArrayIterator", "ArrayIterator", &[]);
    m(
        &mut t,
        &array,
        "setIteratorClass",
        vec![s("MyArrayIterator")],
    );
    assert!(m(&mut t, &array, "getIteratorClass", vec![]).identical(&s("MyArrayIterator")));
    let it = m(&mut t, &array, "getIterator", vec![]);
    assert_eq!(
        it.as_object().unwrap().class().name.to_string(),
        "MyArrayIterator"
    );
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![Value::empty_array(), int(0), s("RecursiveArrayIterator")],
    );
    assert!(m(&mut t, &ao, "getIteratorClass", vec![]).identical(&s("RecursiveArrayIterator")));
}

#[test]
fn incompatible_backing_objects() {
    // ArrayObject_enum.phpt
    let mut t = TestVm::new();
    let mut e = ClassEntry::new("Foo", class_flags::ENUM);
    e.link_magic_methods();
    let e = Rc::new(e);
    t.vm.register_class(e.clone());
    let case = ObjRef::new(e);
    let r = new_obj(&mut t.vm, "ArrayObject", vec![obj(&case)]).map(Value::Object);
    assert_eq!(
        thrown(r),
        (
            "InvalidArgumentException".into(),
            "Enums are not compatible with ArrayObject".into()
        )
    );
    assert_eq!(t.output(), DEP_AO);
    // ArrayObject_overloaded_object_incompatible.phpt / gh15918.phpt: objects
    // with their own property table handler.
    fn props_for(_: &mut Vm, _: &ObjRef, _: PropPurpose) -> PResult<Option<PhpArray>> {
        Ok(None)
    }
    let mut c = ClassEntry::new("Overloaded", 0);
    c.handlers.get_properties_for = Some(props_for);
    let c = Rc::new(c);
    t.vm.register_class(c.clone());
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(1), int(2), int(3)])],
    );
    let r = call(&mut t.vm, &ao, "exchangeArray", vec![obj(&ObjRef::new(c))]);
    assert_eq!(
        thrown(r),
        (
            "InvalidArgumentException".into(),
            "Overloaded object of type Overloaded is not compatible with ArrayObject".into()
        )
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: ArrayObject::exchangeArray(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n"
    );
    assert_eq!(
        show(&mut t, &ao),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>1,1=>2,2=>3]]"
    );
}

#[test]
fn constructor_argument_count() {
    // arrayObject___construct_error2.phpt
    let mut t = TestVm::new();
    let a = std_object(&mut t, &[]);
    let r = new_obj(
        &mut t.vm,
        "ArrayObject",
        vec![obj(&a), int(0), s("ArrayIterator"), s("extra")],
    )
    .map(Value::Object);
    assert_eq!(
        thrown(r),
        (
            "ArgumentCountError".into(),
            "ArrayObject::__construct() expects at most 3 arguments, 4 given".into()
        )
    );
}

// ---------------------------------------------------------------------------
// Object storage
// ---------------------------------------------------------------------------

fn visibility_test_class(t: &mut TestVm) -> ClassRef {
    class_with_props(
        &mut t.vm,
        "test",
        None,
        &[
            ("zero", PUB, int(0)),
            ("pro", PRO, Value::Null),
            ("one", PUB, int(1)),
            ("pri", PRI, Value::Null),
            ("two", PUB, int(2)),
        ],
    )
}

#[test]
fn object_storage_iteration_skips_non_public() {
    // array_012.phpt, array_003.phpt
    let mut t = TestVm::new();
    let ce = visibility_test_class(&mut t);
    let o = t.vm.instantiate(&ce).unwrap();
    o.set_prop(b"dyn", s("dynamic"));
    let it = new(&mut t, "ArrayIterator", vec![obj(&o)]);
    assert_eq!(t.output(), DEP_AI);
    assert!(m(&mut t, &it, "count", vec![]).identical(&int(4)));
    assert_eq!(
        walk(&mut t, &it),
        pairs(&[
            ("\"zero\"", "0"),
            ("\"one\"", "1"),
            ("\"two\"", "2"),
            ("\"dyn\"", "\"dynamic\"")
        ])
    );
    // getArrayCopy() duplicates the whole table, mangled names included.
    let copy = m(&mut t, &it, "getArrayCopy", vec![]);
    assert_eq!(
        show_value(&mut t, &copy),
        "[\"zero\"=>0,\"\\0*\\0pro\"=>null,\"one\"=>1,\"\\0test\\0pri\"=>null,\"two\"=>2,\"dyn\"=>\"dynamic\"]"
    );
    // array_013.phpt
    let r = call(&mut t.vm, &it, "append", vec![s("three")]);
    assert_eq!(
        thrown(r),
        (
            "Error".into(),
            "Cannot append properties to objects, use ArrayIterator::offsetSet() instead".into()
        )
    );
}

#[test]
fn object_storage_dimensions_use_the_property_table() {
    // ArrayObject_proptable_canonicalization.phpt
    let mut t = TestVm::new();
    let o = std_object(&mut t, &[]);
    let ao = new(&mut t, "ArrayObject", vec![obj(&o)]);
    assert_eq!(t.output(), DEP_AO);
    wr(&mut t, &ao, Some(int(0)), int(1));
    assert_eq!(show(&mut t, &o), "stdClass[\"0\"=>1]");
    let v = rd(&mut t, &ao, int(0));
    wr(&mut t, &ao, Some(int(0)), int(v.as_int().unwrap() + 1));
    assert_eq!(show(&mut t, &o), "stdClass[\"0\"=>2]");
    assert!(has(&mut t, &ao, int(0), false));
    let cast = t.vm.cast_to_array(&obj(&ao)).unwrap();
    assert_eq!(render_array(&mut t, &cast), "[0=>2]");
    del(&mut t, &ao, int(0));
    assert_eq!(show(&mut t, &o), "stdClass[]");
    // arrayObject___construct_basic2.phpt: declared properties of the
    // wrapped object.
    let c = class_with_props(&mut t.vm, "C", None, &[("prop", PUB, s("C::prop.orig"))]);
    let c = t.vm.instantiate(&c).unwrap();
    let ao = new(&mut t, "ArrayObject", vec![obj(&c)]);
    t.output();
    assert!(rd(&mut t, &ao, s("prop")).identical(&s("C::prop.orig")));
    wr(&mut t, &ao, Some(s("prop")), s("changed2"));
    assert!(c.get_prop(b"prop").unwrap().identical(&s("changed2")));
    assert!(has(&mut t, &ao, s("prop"), false));
    del(&mut t, &ao, s("prop"));
    assert!(rd(&mut t, &ao, s("prop")).is_null());
    assert_eq!(
        t.output(),
        "\nWarning: Undefined array key \"prop\" in /t.php on line 3\n"
    );
    assert_eq!(show(&mut t, &c), "C[]");
    assert!(m(&mut t, &ao, "count", vec![]).identical(&int(0)));
    // Writing re-initialises the unset declared slot.
    wr(&mut t, &ao, Some(s("prop")), s("again"));
    assert_eq!(show(&mut t, &c), "C[\"prop\"=>\"again\"]");
}

#[test]
fn object_storage_unset_moves_the_iterator() {
    // bug45614.phpt
    let mut t = TestVm::new();
    let ce = class_with_props(
        &mut t.vm,
        "C",
        None,
        &[
            ("priv1", PRI, s("secret1")),
            ("priv2", PRI, s("secret2")),
            ("pub1", PUB, s("public1")),
            ("pub2", PUB, s("public2")),
            ("pub3", PUB, s("public3")),
            ("pub4", PUB, s("public4")),
        ],
    );
    let c = t.vm.instantiate(&ce).unwrap();
    let ao = new(&mut t, "ArrayObject", vec![obj(&c)]);
    let ai = m(&mut t, &ao, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    let first_two = |t: &mut TestVm| {
        let mut out = Vec::new();
        for i in 0..2 {
            if i == 1 {
                m(t, &ai, "next", vec![]);
            }
            let k = m(t, &ai, "key", vec![]);
            let v = m(t, &ai, "current", vec![]);
            out.push(format!("{} => {}", render(t, &k), render(t, &v)));
        }
        out
    };
    assert_eq!(
        first_two(&mut t),
        ["\"pub1\" => \"public1\"", "\"pub2\" => \"public2\""]
    );
    m(&mut t, &ai, "rewind", vec![]);
    assert_eq!(
        first_two(&mut t),
        ["\"pub1\" => \"public1\"", "\"pub2\" => \"public2\""]
    );
    // Invalidate the current position.
    let k = m(&mut t, &ai, "key", vec![]);
    del(&mut t, &ai, k);
    m(&mut t, &ai, "current", vec![]);
    assert_eq!(
        first_two(&mut t),
        ["\"pub3\" => \"public3\"", "\"pub4\" => \"public4\""]
    );
    m(&mut t, &ai, "rewind", vec![]);
    m(&mut t, &ai, "seek", vec![int(0)]);
    assert_eq!(
        first_two(&mut t),
        ["\"pub1\" => \"public1\"", "\"pub3\" => \"public3\""]
    );
    // bug31346.phpt: deleting the current dynamic property.
    let o = std_object(&mut t, &[("var1", int(1))]);
    let ao = new(&mut t, "ArrayObject", vec![obj(&o)]);
    let i = m(&mut t, &ao, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    let k = m(&mut t, &i, "key", vec![]);
    m(&mut t, &ao, "offsetUnset", vec![k]);
    m(&mut t, &i, "next", vec![]);
    assert!(m(&mut t, &i, "valid", vec![]).identical(&Value::Bool(false)));
}

#[test]
fn object_storage_sorting_and_self_storage() {
    // ArrayObject_sort_different_backing_storage.phpt
    let mut t = TestVm::new();
    let o = std_object(&mut t, &[("a", int(2)), ("b", int(1))]);
    let ao = new(&mut t, "ArrayObject", vec![obj(&o)]);
    let asc = closure(&mut t.vm, |vm, a| Ok(int(vm.compare(&a[0], &a[1])? as i64)));
    let desc = closure(&mut t.vm, |vm, a| Ok(int(vm.compare(&a[1], &a[0])? as i64)));
    assert!(m(&mut t, &ao, "uasort", vec![asc.clone()]).identical(&Value::Bool(true)));
    assert_eq!(show(&mut t, &o), "stdClass[\"b\"=>1,\"a\"=>2]");
    let ao2 = new(&mut t, "ArrayObject", vec![obj(&ao)]);
    m(&mut t, &ao2, "uasort", vec![desc]);
    assert_eq!(show(&mut t, &o), "stdClass[\"a\"=>2,\"b\"=>1]");
    let ao3 = new(&mut t, "ArrayObject", vec![]);
    m(&mut t, &ao3, "exchangeArray", vec![obj(&ao3)]);
    ao3.set_prop(b"a", int(2));
    ao3.set_prop(b"b", int(1));
    m(&mut t, &ao3, "uasort", vec![asc.clone()]);
    assert_eq!(show(&mut t, &ao3), "ArrayObject[\"b\"=>1,\"a\"=>2]");
    let ao4 = new(&mut t, "ArrayObject", vec![Value::empty_array()]);
    m(&mut t, &ao4, "uasort", vec![asc.clone()]);
    assert_eq!(
        show(&mut t, &ao4),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[]]"
    );
    // arrayObject___construct_basic7.phpt
    let o = std_object(&mut t, &[("0", int(2)), ("1", int(1))]);
    let ao = new(&mut t, "ArrayObject", vec![obj(&o)]);
    m(&mut t, &ao, "asort", vec![]);
    assert_eq!(show(&mut t, &o), "stdClass[\"1\"=>1,\"0\"=>2]");
    assert_eq!(
        t.output(),
        format!(
            "{DEP_AO}{DEP_AO}\nDeprecated: ArrayObject::exchangeArray(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n{DEP_AO}"
        )
    );
}

#[test]
fn self_storage_and_exchange_array() {
    // array_017.phpt (exchangeArray($this)), bug41691.phpt
    let mut t = TestVm::new();
    let ce = class_with_props(
        &mut t.vm,
        "A",
        Some("ArrayObject"),
        &[
            ("a", PUB, Value::Null),
            ("b", PUB, Value::Null),
            ("c", PUB, Value::Null),
        ],
    );
    let a = t.vm.instantiate(&ce).unwrap();
    m(&mut t, &a, "__construct", vec![obj(&a), int(2)]);
    let old = m(
        &mut t,
        &a,
        "exchangeArray",
        vec![assoc(vec![
            (skey("a"), int(1)),
            (skey("b"), int(1)),
            (skey("c"), int(1)),
        ])],
    );
    assert_eq!(
        show_value(&mut t, &old),
        "[\"a\"=>null,\"b\"=>null,\"c\"=>null]"
    );
    assert_eq!(t.output(), DEP_AO);
    // array_022.phpt: IS_SELF survives cloning.
    let my = user_class(&mut t.vm, "MyArrayObject", "ArrayObject", &[]);
    let a = t.vm.instantiate(&my).unwrap();
    m(&mut t, &a, "__construct", vec![obj(&a)]);
    wr(&mut t, &a, Some(s("bar")), s("baz"));
    let b = (a.class().handlers.clone_obj.unwrap())(&mut t.vm, &a).unwrap();
    wr(&mut t, &b, Some(s("baz")), s("Foo"));
    assert_eq!(show(&mut t, &a), "MyArrayObject[\"bar\"=>\"baz\"]");
    assert_eq!(
        show(&mut t, &b),
        "MyArrayObject[\"bar\"=>\"baz\",\"baz\"=>\"Foo\"]"
    );
    let myi = user_class(&mut t.vm, "MyArrayIterator", "ArrayIterator", &[]);
    let a = t.vm.instantiate(&myi).unwrap();
    m(&mut t, &a, "__construct", vec![obj(&a)]);
    wr(&mut t, &a, Some(s("bar")), s("baz"));
    let b = (a.class().handlers.clone_obj.unwrap())(&mut t.vm, &a).unwrap();
    wr(&mut t, &b, Some(s("baz")), s("Foo"));
    assert_eq!(show(&mut t, &a), "MyArrayIterator[\"bar\"=>\"baz\"]");
    assert_eq!(
        show(&mut t, &b),
        "MyArrayIterator[\"bar\"=>\"baz\",\"baz\"=>\"Foo\"]"
    );
    assert_eq!(t.output(), format!("{DEP_AO}{DEP_AI}"));
    // ArrayObject_sort_different_backing_storage.phpt: own dynamic properties.
    let selfish = new(&mut t, "ArrayObject", vec![]);
    m(&mut t, &selfish, "exchangeArray", vec![obj(&selfish)]);
    t.output();
    wr(&mut t, &selfish, Some(s("x")), int(1));
    assert_eq!(show(&mut t, &selfish), "ArrayObject[\"x\"=>1]");
    assert!(selfish.get_prop(b"x").unwrap().identical(&int(1)));
}

#[test]
fn std_prop_list_and_array_cast() {
    // ArrayObject_std_props_no_recursion.phpt
    let mut t = TestVm::new();
    let a = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(1), int(2), int(3)]), int(1)],
    );
    a.set_prop(b"prop", s("a"));
    let b = new(&mut t, "ArrayObject", vec![obj(&a), int(0)]);
    b.set_prop(b"prop", s("b"));
    let cast = t.vm.cast_to_array(&obj(&b)).unwrap();
    assert_eq!(render_array(&mut t, &cast), "[0=>1,1=>2,2=>3]");
    let c = new(&mut t, "ArrayObject", vec![obj(&a)]);
    c.set_prop(b"prop", s("c"));
    let cast = t.vm.cast_to_array(&obj(&c)).unwrap();
    assert_eq!(render_array(&mut t, &cast), "[\"prop\"=>\"c\"]");
    // arrayObject___construct_basic6.phpt: STD_PROP_LIST does not change var_dump.
    let ao = new(&mut t, "ArrayObject", vec![list(vec![int(1)]), int(1)]);
    ao.set_prop(b"p", int(1));
    assert_eq!(
        show(&mut t, &ao),
        "ArrayObject[\"p\"=>1,\"\\0ArrayObject\\0storage\"=>[0=>1]]"
    );
    // ArrayObject_get_object_vars.phpt: other purposes use the real properties.
    let h = ao.class().handlers.get_properties_for.unwrap();
    assert!(
        h(&mut t.vm, &ao, PropPurpose::GetObjectVars)
            .unwrap()
            .is_none()
    );
    let plain = new(&mut t, "ArrayObject", vec![list(vec![int(5)])]);
    assert!(
        h(&mut t.vm, &plain, PropPurpose::GetObjectVars)
            .unwrap()
            .is_none()
    );
    let json = h(&mut t.vm, &plain, PropPurpose::Json).unwrap().unwrap();
    assert_eq!(render_array(&mut t, &json), "[0=>5]");
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[test]
fn array_object_iterators_have_their_own_position() {
    // array_004.phpt
    let mut t = TestVm::new();
    let o = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(0), int(1), int(2)])],
    );
    let outer = m(&mut t, &o, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    let mut seen = Vec::new();
    m(&mut t, &outer, "rewind", vec![]);
    while m(&mut t, &outer, "valid", vec![]).identical(&Value::Bool(true)) {
        let ak = m(&mut t, &outer, "key", vec![]).as_int().unwrap();
        let inner = m(&mut t, &o, "getIterator", vec![])
            .as_object()
            .unwrap()
            .clone();
        for (bk, _) in walk(&mut t, &inner) {
            seen.push(format!("{ak}-{bk}"));
        }
        m(&mut t, &outer, "next", vec![]);
    }
    assert_eq!(seen.len(), 9);
    // array_006.phpt: a single ArrayIterator shares its position.
    let it = new(
        &mut t,
        "ArrayIterator",
        vec![list(vec![int(0), int(1), int(2)])],
    );
    let mut seen = Vec::new();
    m(&mut t, &it, "rewind", vec![]);
    while m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(true)) {
        let ak = m(&mut t, &it, "key", vec![]).as_int().unwrap();
        for (bk, _) in walk(&mut t, &it) {
            seen.push(format!("{ak}-{bk}"));
        }
        m(&mut t, &it, "next", vec![]);
    }
    assert_eq!(seen, ["0-0", "0-1", "0-2"]);
}

#[test]
fn iterator_positions_survive_modification() {
    // array_015.phpt
    let mut t = TestVm::new();
    let ar = new(&mut t, "ArrayObject", vec![]);
    for i in 0..5 {
        wr(&mut t, &ar, Some(int(i)), int(i + 1));
    }
    let it = m(&mut t, &ar, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    let k = m(&mut t, &it, "key", vec![]);
    m(&mut t, &ar, "offsetUnset", vec![k]);
    m(&mut t, &it, "next", vec![]);
    assert!(m(&mut t, &it, "current", vec![]).identical(&int(3)));
    let mut out = Vec::new();
    m(&mut t, &it, "rewind", vec![]);
    while m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(true)) {
        let k = m(&mut t, &it, "key", vec![]).as_int().unwrap();
        let v = m(&mut t, &it, "current", vec![]).as_int().unwrap();
        m(&mut t, &ar, "offsetUnset", vec![int(k + 1)]);
        out.push(format!("{k}=>{v}"));
        m(&mut t, &it, "next", vec![]);
    }
    assert_eq!(out, ["1=>2", "3=>4"]);
    let mut out = Vec::new();
    m(&mut t, &it, "rewind", vec![]);
    while m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(true)) {
        let k = m(&mut t, &it, "key", vec![]).as_int().unwrap();
        let v = m(&mut t, &it, "current", vec![]).as_int().unwrap();
        m(&mut t, &ar, "offsetUnset", vec![int(k)]);
        out.push(format!("{k}=>{v}"));
        m(&mut t, &it, "next", vec![]);
    }
    assert_eq!(out, ["1=>2"]);
    assert_eq!(
        show(&mut t, &ar),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[3=>4]]"
    );
}

#[test]
fn bug61527_positions_at_the_end() {
    let mut t = TestVm::new();
    let ao = new(&mut t, "ArrayObject", vec![Value::empty_array()]);
    let ai = m(&mut t, &ao, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    m(&mut t, &ai, "next", vec![]);
    assert!(m(&mut t, &ai, "key", vec![]).is_null());
    assert!(m(&mut t, &ai, "current", vec![]).is_null());
    let a = assoc((1..=5).map(|i| (ArrayKey::Int(i), int(i))).collect());
    let ao2 = new(&mut t, "ArrayObject", vec![a]);
    let ai2 = m(&mut t, &ao2, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    let k = m(&mut t, &ai2, "key", vec![]);
    m(&mut t, &ao2, "offsetUnset", vec![k]);
    m(&mut t, &ai2, "next", vec![]);
    let k = m(&mut t, &ai2, "key", vec![]);
    m(&mut t, &ao2, "offsetUnset", vec![k]);
    assert!(m(&mut t, &ai2, "key", vec![]).identical(&int(4)));
    let k = m(&mut t, &ai2, "key", vec![]);
    m(&mut t, &ao2, "offsetUnset", vec![k]);
    assert!(m(&mut t, &ai2, "current", vec![]).identical(&int(5)));
    m(&mut t, &ai2, "next", vec![]);
    assert!(m(&mut t, &ai2, "key", vec![]).is_null());
    assert!(m(&mut t, &ai2, "current", vec![]).is_null());
    m(&mut t, &ai2, "next", vec![]);
    assert!(m(&mut t, &ai2, "key", vec![]).is_null());
    m(&mut t, &ai2, "rewind", vec![]);
    m(&mut t, &ai2, "next", vec![]);
    m(&mut t, &ai2, "next", vec![]);
    assert!(m(&mut t, &ai2, "next", vec![]).is_null());
    assert!(m(&mut t, &ai2, "key", vec![]).is_null());
    let ao3 = new(
        &mut t,
        "ArrayObject",
        vec![Value::empty_array(), int(0), s("RecursiveArrayIterator")],
    );
    let ai3 = m(&mut t, &ao3, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    assert!(m(&mut t, &ai3, "getChildren", vec![]).is_null());
    let ao4 = new(
        &mut t,
        "ArrayObject",
        vec![
            list(vec![int(1), int(2)]),
            int(0),
            s("RecursiveArrayIterator"),
        ],
    );
    let ai4 = m(&mut t, &ao4, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    for _ in 0..3 {
        m(&mut t, &ai4, "next", vec![]);
    }
    assert!(m(&mut t, &ai4, "hasChildren", vec![]).identical(&Value::Bool(false)));
    m(&mut t, &ai4, "rewind", vec![]);
    let k = m(&mut t, &ai4, "key", vec![]);
    m(&mut t, &ao4, "offsetUnset", vec![k]);
    assert!(m(&mut t, &ai4, "hasChildren", vec![]).identical(&Value::Bool(false)));
    let k = m(&mut t, &ai4, "key", vec![]);
    m(&mut t, &ao4, "offsetUnset", vec![k]);
    assert!(m(&mut t, &ai4, "getChildren", vec![]).is_null());
    assert!(t.output().is_empty());
}

#[test]
fn appended_elements_become_visible() {
    // bug77903.phpt
    let mut t = TestVm::new();
    let a = new(&mut t, "ArrayIterator", vec![]);
    let state = |t: &mut TestVm| {
        let v = m(t, &a, "valid", vec![]);
        let c = m(t, &a, "current", vec![]);
        format!("{}:{}", render(t, &v), render(t, &c))
    };
    m(&mut t, &a, "rewind", vec![]);
    assert_eq!(state(&mut t), "false:null");
    m(&mut t, &a, "offsetSet", vec![int(1), int(1)]);
    assert_eq!(state(&mut t), "true:1");
    m(&mut t, &a, "next", vec![]);
    assert_eq!(state(&mut t), "false:null");
    m(&mut t, &a, "offsetSet", vec![int(4), int(4)]);
    assert_eq!(state(&mut t), "true:4");
    m(&mut t, &a, "next", vec![]);
    assert_eq!(state(&mut t), "false:null");
    m(&mut t, &a, "next", vec![]);
    assert_eq!(state(&mut t), "false:null");
    m(&mut t, &a, "offsetSet", vec![int(2), int(2)]);
    assert_eq!(state(&mut t), "true:2");
    m(&mut t, &a, "next", vec![]);
    assert_eq!(state(&mut t), "false:null");
    // array_013.phpt: appends after the end are seen by a non-rewinding loop.
    let it = new(
        &mut t,
        "ArrayIterator",
        vec![list(vec![s("zero"), s("one"), s("two")])],
    );
    assert_eq!(walk(&mut t, &it).len(), 3);
    m(&mut t, &it, "append", vec![s("three")]);
    m(&mut t, &it, "append", vec![s("four")]);
    let mut rest = Vec::new();
    while m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(true)) {
        let k = m(&mut t, &it, "key", vec![]).as_int().unwrap();
        rest.push(k);
        m(&mut t, &it, "next", vec![]);
    }
    assert_eq!(rest, [3, 4]);
    // bug55157_variation.phpt: unsetting the current element.
    let it = new(
        &mut t,
        "ArrayIterator",
        vec![list((0..4).map(int).collect())],
    );
    let mut out = Vec::new();
    m(&mut t, &it, "rewind", vec![]);
    while m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(true)) {
        let k = m(&mut t, &it, "key", vec![]);
        out.push(k.as_int().unwrap());
        m(&mut t, &it, "offsetUnset", vec![k]);
    }
    assert_eq!(out, [0, 1, 2, 3]);
}

#[test]
fn seek() {
    // array_014.phpt
    let mut t = TestVm::new();
    let it = new(
        &mut t,
        "ArrayIterator",
        vec![list((0..=10).map(int).collect())],
    );
    assert!(m(&mut t, &it, "count", vec![]).identical(&int(11)));
    m(&mut t, &it, "seek", vec![int(5)]);
    assert!(m(&mut t, &it, "current", vec![]).identical(&int(5)));
    m(&mut t, &it, "seek", vec![int(4)]);
    assert!(m(&mut t, &it, "current", vec![]).identical(&int(4)));
    for p in [-1, 12, 11] {
        let r = call(&mut t.vm, &it, "seek", vec![int(p)]);
        assert_eq!(
            thrown(r),
            (
                "OutOfBoundsException".into(),
                format!("Seek position {p} is out of range")
            )
        );
    }
    m(&mut t, &it, "seek", vec![int(10)]);
    assert!(m(&mut t, &it, "current", vec![]).identical(&int(10)));
}

#[test]
fn by_reference_iteration() {
    // array_008.phpt
    let mut t = TestVm::new();
    let o = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(0), int(1), int(2)])],
    );
    let outer = m(&mut t, &o, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    m(&mut t, &outer, "rewind", vec![]);
    let av = super::current_ref(&mut t.vm, &outer).unwrap().unwrap();
    let inner = m(&mut t, &o, "getIterator", vec![])
        .as_object()
        .unwrap()
        .clone();
    m(&mut t, &inner, "rewind", vec![]);
    let bv = super::current_ref(&mut t.vm, &inner).unwrap().unwrap();
    bv.set(s("modify"));
    assert!(av.get().identical(&s("modify")));
    drop((av, bv));
    assert_eq!(
        show(&mut t, &o),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>\"modify\",1=>1,2=>2]]"
    );
    // gh11178.phpt: a wrapped object's dynamic property.
    let a = std_object(&mut t, &[("x", int(1))]);
    let it = new(&mut t, "ArrayIterator", vec![obj(&a)]);
    m(&mut t, &it, "rewind", vec![]);
    let v = super::current_ref(&mut t.vm, &it).unwrap().unwrap();
    v.set(int(3));
    m(&mut t, &it, "next", vec![]);
    assert!(super::current_ref(&mut t.vm, &it).unwrap().is_none());
    assert_eq!(show(&mut t, &a), "stdClass[\"x\"=>&3]");
    // Readonly properties cannot be iterated by reference.
    let mut ce = ClassEntry::new("RO", 0);
    let mut info = PropertyInfo::new("p", "RO", PUB | member_flags::READONLY);
    info.ty = Some(php_runtime::types::TypeDecl::parse("int").unwrap());
    info.default = int(1);
    ce.declare_property(info);
    let ce = Rc::new(ce);
    t.vm.register_class(ce.clone());
    let ro = t.vm.instantiate(&ce).unwrap();
    let it = new(&mut t, "ArrayIterator", vec![obj(&ro)]);
    m(&mut t, &it, "rewind", vec![]);
    let r = super::current_ref(&mut t.vm, &it).map(|_| Value::Null);
    assert_eq!(
        thrown(r),
        (
            "Error".into(),
            "Cannot acquire reference to readonly property RO::$p".into()
        )
    );
}

#[test]
fn recursive_array_iterator_children() {
    // array_009.phpt
    let mut t = TestVm::new();
    let array = assoc(vec![
        (ArrayKey::Int(0), int(1)),
        (
            ArrayKey::Int(2),
            assoc(vec![
                (ArrayKey::Int(0), int(21)),
                (ArrayKey::Int(22), list(vec![int(221), int(222)])),
                (ArrayKey::Int(23), list(vec![int(231)])),
            ]),
        ),
        (ArrayKey::Int(3), int(3)),
    ]);
    fn leaves(t: &mut TestVm, it: &ObjRef, out: &mut Vec<i64>) {
        m(t, it, "rewind", vec![]);
        while m(t, it, "valid", vec![]).identical(&Value::Bool(true)) {
            if m(t, it, "hasChildren", vec![]).identical(&Value::Bool(true)) {
                let c = m(t, it, "getChildren", vec![]).as_object().unwrap().clone();
                assert_eq!(c.class().name.to_string(), it.class().name.to_string());
                leaves(t, &c, out);
            } else {
                out.push(m(t, it, "current", vec![]).as_int().unwrap());
            }
            m(t, it, "next", vec![]);
        }
    }
    let rai = new(&mut t, "RecursiveArrayIterator", vec![array.clone()]);
    let mut out = Vec::new();
    leaves(&mut t, &rai, &mut out);
    assert_eq!(out, [1, 21, 221, 222, 231, 3]);
    // Children of a user subclass are constructed through its constructor.
    let my = user_class(&mut t.vm, "MyRAI", "RecursiveArrayIterator", &[]);
    let rai = t.vm.instantiate(&my).unwrap();
    m(&mut t, &rai, "__construct", vec![array]);
    let mut out = Vec::new();
    leaves(&mut t, &rai, &mut out);
    assert_eq!(out, [1, 21, 221, 222, 231, 3]);
    assert!(t.output().is_empty());
}

#[test]
fn recursive_array_iterator_child_arrays_only() {
    // bug44615.phpt
    let mut t = TestVm::new();
    let a = std_object(&mut t, &[]);
    let array = list(vec![
        list(vec![s("z"), obj(&a)]),
        list(vec![s("q"), s("s")]),
    ]);
    let rai = new(
        &mut t,
        "RecursiveArrayIterator",
        vec![array.clone(), int(4)],
    );
    let first = m(&mut t, &rai, "getChildren", vec![])
        .as_object()
        .unwrap()
        .clone();
    m(&mut t, &first, "next", vec![]);
    assert!(m(&mut t, &first, "hasChildren", vec![]).identical(&Value::Bool(false)));
    assert!(m(&mut t, &first, "getChildren", vec![]).is_null());
    assert!(m(&mut t, &first, "getFlags", vec![]).identical(&int(4)));
    t.output();
    let rai = new(&mut t, "RecursiveArrayIterator", vec![array]);
    let first = m(&mut t, &rai, "getChildren", vec![])
        .as_object()
        .unwrap()
        .clone();
    m(&mut t, &first, "next", vec![]);
    assert!(m(&mut t, &first, "hasChildren", vec![]).identical(&Value::Bool(true)));
    let child = m(&mut t, &first, "getChildren", vec![])
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(t.output(), DEP_AI);
    assert!(m(&mut t, &child, "valid", vec![]).identical(&Value::Bool(false)));
    // An element that already is an instance of the class is returned as is.
    let inner = new(&mut t, "RecursiveArrayIterator", vec![list(vec![int(1)])]);
    let outer = new(
        &mut t,
        "RecursiveArrayIterator",
        vec![list(vec![obj(&inner)])],
    );
    let c = m(&mut t, &outer, "getChildren", vec![]);
    assert!(ObjRef::ptr_eq(c.as_object().unwrap(), &inner));
}

#[test]
fn array_iterator_internal_use_by_append_iterator() {
    // What AppendIterator does with its internal ArrayIterator.
    let mut t = TestVm::new();
    let it = new(&mut t, "ArrayIterator", vec![]);
    let (i1, _) = crate::spl::testutil::test_iterator(&mut t.vm, vec![]);
    let (i2, _) = crate::spl::testutil::test_iterator(&mut t.vm, vec![]);
    m(&mut t, &it, "rewind", vec![]);
    m(&mut t, &it, "append", vec![obj(&i1)]);
    assert!(m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(true)));
    m(&mut t, &it, "append", vec![obj(&i2)]);
    assert!(m(&mut t, &it, "count", vec![]).identical(&int(2)));
    assert!(ObjRef::ptr_eq(
        m(&mut t, &it, "current", vec![]).as_object().unwrap(),
        &i1
    ));
    m(&mut t, &it, "next", vec![]);
    assert!(m(&mut t, &it, "key", vec![]).identical(&int(1)));
    assert!(ObjRef::ptr_eq(
        m(&mut t, &it, "offsetGet", vec![int(1)])
            .as_object()
            .unwrap(),
        &i2
    ));
    m(&mut t, &it, "next", vec![]);
    assert!(m(&mut t, &it, "valid", vec![]).identical(&Value::Bool(false)));
    // bug67360.phpt
    let a = assoc(vec![
        (skey(""), int(1)),
        (ArrayKey::Int(1), int(2)),
        (ArrayKey::Int(3), int(4)),
    ]);
    let ao = new(&mut t, "ArrayObject", vec![a]);
    let it = m(&mut t, &ao, "getIterator", vec![]);
    let n = t.call("count", vec![it.clone()]).unwrap();
    assert!(n.identical(&int(3)));
    let arr = t.call("iterator_to_array", vec![it]).unwrap();
    assert_eq!(show_value(&mut t, &arr), "[\"\"=>1,1=>2,3=>4]");
}

// ---------------------------------------------------------------------------
// User overrides
// ---------------------------------------------------------------------------

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn log(s: String) {
    LOG.with(|l| l.borrow_mut().push(s));
}

fn take_log() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

fn arg_text(v: &Value) -> String {
    match v.unref() {
        Value::String(s) => s.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

fn user_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log(format!("offsetGet({})", arg_text(args.get(0))));
    let this = engine::this(args);
    let parent = vm.find_class(b"ArrayObject").unwrap();
    let f = parent.find_method(b"offsetget").unwrap().clone();
    vm.call_function(
        &f,
        php_runtime::CallArgs {
            positional: vec![args.get(0).clone()],
            this: Some(this),
            ..Default::default()
        },
    )
}

fn user_offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log(format!(
        "offsetSet({},{})",
        arg_text(args.get(0)),
        arg_text(args.get(1))
    ));
    let this = engine::this(args);
    let parent = vm.find_class(b"ArrayObject").unwrap();
    let f = parent.find_method(b"offsetset").unwrap().clone();
    vm.call_function(
        &f,
        php_runtime::CallArgs {
            positional: vec![args.get(0).clone(), args.get(1).clone()],
            this: Some(this),
            ..Default::default()
        },
    )
}

fn user_offset_exists(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log(format!("offsetExists({})", arg_text(args.get(0))));
    Ok(Value::Bool(arg_text(args.get(0)) != "missing"))
}

fn user_offset_unset(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log(format!("offsetUnset({})", arg_text(args.get(0))));
    Ok(Value::Null)
}

fn user_count(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(int(99))
}

fn user_debug_info(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(assoc(vec![(skey("custom"), Value::Bool(true))]))
}

static U_OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet(mixed $key): mixed {}",
    handler: user_offset_get,
};
static U_OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet(mixed $key, mixed $value): void {}",
    handler: user_offset_set,
};
static U_OFFSET_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists(mixed $key): bool {}",
    handler: user_offset_exists,
};
static U_OFFSET_UNSET: NativeFunction = NativeFunction {
    sig: "public function offsetUnset(mixed $key): void {}",
    handler: user_offset_unset,
};
static U_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: user_count,
};
static U_DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: user_debug_info,
};

#[test]
fn handlers_dispatch_to_user_overrides() {
    // bug32134.phpt, bug33136.phpt, bug34548.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyAO",
        "ArrayObject",
        &[
            &U_OFFSET_GET,
            &U_OFFSET_SET,
            &U_OFFSET_EXISTS,
            &U_OFFSET_UNSET,
        ],
    );
    let o = new(&mut t, "MyAO", vec![assoc(vec![(skey("a"), int(0))])]);
    take_log();
    wr(&mut t, &o, Some(s("b")), int(2));
    wr(&mut t, &o, None, int(3));
    m(&mut t, &o, "append", vec![int(4)]);
    assert!(rd(&mut t, &o, s("b")).identical(&int(2)));
    assert!(has(&mut t, &o, s("a"), false));
    assert!(!has(&mut t, &o, s("a"), true));
    assert!(!has(&mut t, &o, s("missing"), false));
    del(&mut t, &o, s("a"));
    assert_eq!(
        take_log(),
        [
            "offsetSet(b,2)",
            "offsetSet(null,3)",
            "offsetSet(null,4)",
            "offsetGet(b)",
            "offsetExists(a)",
            "offsetExists(a)",
            "offsetGet(a)",
            "offsetExists(missing)"
        ]
        .iter()
        .chain(["offsetUnset(a)"].iter())
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
    );
    // offsetUnset was overridden (and did nothing): 'a' is still there.
    assert_eq!(
        show(&mut t, &o),
        "MyAO[\"\\0ArrayObject\\0storage\"=>[\"a\"=>0,\"b\"=>2,0=>3,1=>4]]"
    );
    assert!(take_log().is_empty());
    // `$o['k'] ?? x` with offsetExists + offsetGet overrides.
    let h = o.class().handlers.read_dimension.unwrap();
    let v = h(&mut t.vm, &o, Some(&s("missing")), DimFetch::IsSet).unwrap();
    assert!(v.is_null());
    let v = h(&mut t.vm, &o, Some(&s("b")), DimFetch::IsSet).unwrap();
    assert!(v.identical(&int(2)));
    assert_eq!(
        take_log(),
        ["offsetExists(missing)", "offsetExists(b)", "offsetGet(b)"]
    );
    // A user offsetGet() result is not writable by itself.
    let v = h(&mut t.vm, &o, Some(&s("b")), DimFetch::Write).unwrap();
    assert!(v.identical(&int(2)));
}

#[test]
fn count_uses_user_override() {
    // array_024.phpt, arrayObject_count_basic1.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "C", "ArrayObject", &[&U_COUNT]);
    user_class(&mut t.vm, "D", "ArrayIterator", &[&U_COUNT]);
    for class in ["C", "D"] {
        let c = new(&mut t, class, vec![]);
        let base = if class == "C" {
            "ArrayObject"
        } else {
            "ArrayIterator"
        };
        let plain = new(&mut t, base, vec![]);
        let cnt = |t: &mut TestVm, o: &ObjRef| t.call("count", vec![obj(o)]).unwrap();
        assert!(cnt(&mut t, &c).identical(&int(99)));
        assert!(cnt(&mut t, &plain).identical(&int(0)));
        for v in ["a", "b"] {
            wr(&mut t, &c, None, s(v));
            wr(&mut t, &plain, None, s(v));
        }
        assert!(cnt(&mut t, &c).identical(&int(99)));
        assert!(cnt(&mut t, &plain).identical(&int(2)));
        del(&mut t, &c, int(0));
        del(&mut t, &plain, int(0));
        assert!(m(&mut t, &c, "count", vec![]).identical(&int(99)));
        assert!(m(&mut t, &plain, "count", vec![]).identical(&int(1)));
    }
    // The handler reaches the internal count through user_class's method:
    let h =
        t.vm.find_class(b"C")
            .unwrap()
            .handlers
            .count_elements
            .unwrap();
    let c = new(&mut t, "C", vec![]);
    assert_eq!(h(&mut t.vm, &c).unwrap(), Some(99));
}

#[test]
fn debug_info_override_is_used() {
    // bug69264.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "Dbg", "ArrayObject", &[&U_DEBUG_INFO]);
    let o = new(&mut t, "Dbg", vec![]);
    let d = debug(&mut t, &o);
    assert_eq!(render_array(&mut t, &d), "[\"custom\"=>true]");
    let d = m(&mut t, &o, "__debugInfo", vec![]);
    assert_eq!(show_value(&mut t, &d), "[\"custom\"=>true]");
    // The internal __debugInfo of a subclass with declared properties.
    let ce = class_with_props(
        &mut t.vm,
        "Name",
        Some("ArrayObject"),
        &[
            ("var", PUB, s("a")),
            ("bar", PRO, s("b")),
            ("foo", PRI, s("c")),
        ],
    );
    let n = t.vm.instantiate(&ce).unwrap();
    assert_eq!(
        show(&mut t, &n),
        "Name[\"var\"=>\"a\",\"\\0*\\0bar\"=>\"b\",\"\\0Name\\0foo\"=>\"c\",\"\\0ArrayObject\\0storage\"=>[]]"
    );
    let ai = new(&mut t, "RecursiveArrayIterator", vec![]);
    assert_eq!(
        show(&mut t, &ai),
        "RecursiveArrayIterator[\"\\0ArrayIterator\\0storage\"=>[]]"
    );
}

// ---------------------------------------------------------------------------
// ARRAY_AS_PROPS
// ---------------------------------------------------------------------------

fn read_prop(t: &mut TestVm, o: &ObjRef, name: &str) -> Option<Value> {
    let h = o.class().handlers.read_property.unwrap();
    h(&mut t.vm, o, &PhpStr::from(name)).unwrap()
}

fn write_prop(t: &mut TestVm, o: &ObjRef, name: &str, v: Value) -> bool {
    let h = o.class().handlers.write_property.unwrap();
    h(&mut t.vm, o, &PhpStr::from(name), v).unwrap()
}

#[test]
fn array_as_props_routes_properties_to_elements() {
    // arrayObject___construct_basic4.phpt
    let mut t = TestVm::new();
    let c = class_with_props(&mut t.vm, "C", None, &[("prop", PUB, s("C::prop.orig"))]);
    let c = t.vm.instantiate(&c).unwrap();
    let ao = new(&mut t, "ArrayObject", vec![obj(&c), int(2)]);
    t.output();
    assert!(
        read_prop(&mut t, &ao, "prop")
            .unwrap()
            .identical(&s("C::prop.orig"))
    );
    assert!(write_prop(&mut t, &ao, "prop", s("changed1")));
    assert!(c.get_prop(b"prop").unwrap().identical(&s("changed1")));
    let name = PhpStr::from("prop");
    assert_eq!(
        super::has_property(&mut t.vm, &ao, &name, 0).unwrap(),
        Some(true)
    );
    assert!(super::unset_property(&mut t.vm, &ao, &name).unwrap());
    assert!(read_prop(&mut t, &ao, "prop").unwrap().is_null());
    assert_eq!(
        t.output(),
        "\nWarning: Undefined array key \"prop\" in /t.php on line 3\n"
    );
    // A declared property of the ArrayObject subclass wins.
    let my = class_with_props(
        &mut t.vm,
        "MyArrayObject",
        Some("ArrayObject"),
        &[("prop", PUB, s("MyArrayObject::prop.orig"))],
    );
    let ao = t.vm.instantiate(&my).unwrap();
    m(&mut t, &ao, "__construct", vec![list(vec![]), int(2)]);
    assert!(read_prop(&mut t, &ao, "prop").is_none());
    assert!(!write_prop(&mut t, &ao, "prop", s("x")));
    assert_eq!(super::has_property(&mut t.vm, &ao, &name, 0).unwrap(), None);
    // Without the flag nothing is redirected.
    let plain = new(
        &mut t,
        "ArrayObject",
        vec![assoc(vec![(skey("a"), int(1))])],
    );
    assert!(read_prop(&mut t, &plain, "a").is_none());
    m(&mut t, &plain, "setFlags", vec![int(2)]);
    assert!(read_prop(&mut t, &plain, "a").unwrap().identical(&int(1)));
    // `??`: no warning for a missing element.
    let v = super::read_property_fetch(&mut t.vm, &plain, &PhpStr::from("zz"), DimFetch::IsSet)
        .unwrap();
    assert!(v.unwrap().is_null());
    assert!(t.output().is_empty());
}

#[test]
fn array_as_props_isset_empty_and_property_exists() {
    // bug53515.phpt, bug40036.phpt, bug69845.phpt
    let mut t = TestVm::new();
    let a = assoc(vec![
        (skey("a"), int(1)),
        (skey("b"), Value::Bool(true)),
        (skey("c"), int(0)),
        (skey("d"), Value::Null),
        (skey("e"), Value::Bool(false)),
        (skey("f"), Value::empty_array()),
    ]);
    let o = new(&mut t, "ArrayObject", vec![a, int(2)]);
    for k in ["a", "b", "c", "d", "e", "f"] {
        let r = super::has_property(&mut t.vm, &o, &PhpStr::from(k), 2).unwrap();
        assert_eq!(r, Some(true), "property_exists {k}");
    }
    for k in ["z", ""] {
        let r = super::has_property(&mut t.vm, &o, &PhpStr::from(k), 2).unwrap();
        assert_eq!(r, Some(false), "property_exists {k}");
    }
    assert_eq!(
        super::has_property(&mut t.vm, &o, &PhpStr::from("d"), 0).unwrap(),
        Some(false)
    );
    assert_eq!(
        super::has_property(&mut t.vm, &o, &PhpStr::from("e"), 1).unwrap(),
        Some(false)
    );
    let view = new(&mut t, "ArrayObject", vec![Value::empty_array(), int(2)]);
    for (k, v) in [
        ("foo", Value::Bool(false)),
        ("bar", Value::Null),
        ("baz", s("")),
    ] {
        assert!(write_prop(&mut t, &view, k, v));
        assert!(!has(&mut t, &view, s(k), true));
        let r = super::has_property(&mut t.vm, &view, &PhpStr::from(k), 1).unwrap();
        assert_eq!(r, Some(false));
    }
    let data = std_object(&mut t, &[]);
    let data = new(&mut t, "ArrayObject", vec![obj(&data), int(2)]);
    assert!(write_prop(&mut t, &data, "itemType", s("bulletin")));
    assert!(rd(&mut t, &data, s("itemType")).identical(&s("bulletin")));
}

#[test]
fn array_as_props_respects_visibility() {
    // arrayObject_setFlags_basic2.phpt: from outside the class, a private
    // property falls back to the element.
    let mut t = TestVm::new();
    let ce = class_with_props(
        &mut t.vm,
        "C",
        Some("ArrayObject"),
        &[("x", PRI, s("secret"))],
    );
    let c = t.vm.instantiate(&ce).unwrap();
    m(
        &mut t,
        &c,
        "__construct",
        vec![assoc(vec![(skey("x"), s("public"))])],
    );
    m(&mut t, &c, "setFlags", vec![int(2)]);
    assert!(read_prop(&mut t, &c, "x").unwrap().identical(&s("public")));
    // Inside C's scope the private property is the standard one.
    let f = Rc::new(php_runtime::Function::native_method(&U_COUNT, "C", PUB));
    t.vm.push_frame(php_runtime::vm::Frame {
        function: Some(f),
        name: PhpStr::from("go"),
        class: Some(PhpStr::from("C")),
        call_type: Some("::"),
        this: None,
        call_site: None,
        args: Vec::new(),
        is_internal: false,
    });
    assert!(read_prop(&mut t, &c, "x").is_none());
    t.vm.pop_frame();
}

#[test]
fn property_ptr_semantics() {
    let mut t = TestVm::new();
    let o = new(&mut t, "ArrayObject", vec![Value::empty_array(), int(2)]);
    let name = PhpStr::from("list");
    match super::property_ptr(&mut t.vm, &o, &name, DimFetch::Write).unwrap() {
        super::PropertyPtr::Element(Value::Ref(r)) => {
            r.set(list(vec![int(1)]));
        }
        _ => panic!("expected an element reference"),
    }
    assert!(
        rd(&mut t, &o, s("list")).identical(&list(vec![int(1)])) || {
            let v = rd(&mut t, &o, s("list"));
            show_value(&mut t, &v) == "[0=>1]"
        }
    );
    let plain = new(&mut t, "ArrayObject", vec![]);
    assert!(matches!(
        super::property_ptr(&mut t.vm, &plain, &name, DimFetch::Write).unwrap(),
        super::PropertyPtr::Standard
    ));
    user_class(&mut t.vm, "WithGet", "ArrayObject", &[&U_OFFSET_GET]);
    let w = new(&mut t, "WithGet", vec![Value::empty_array(), int(2)]);
    assert!(matches!(
        super::property_ptr(&mut t.vm, &w, &name, DimFetch::Write).unwrap(),
        super::PropertyPtr::ReadWrite
    ));
}

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

#[test]
fn sort_methods() {
    // arrayObject_asort_basic1.phpt, arrayObject_ksort_basic1.phpt,
    // arrayObject_uasort_basic1.phpt, arrayObject_natsort_basic1.phpt
    let mut t = TestVm::new();
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![assoc(vec![
            (skey("c"), int(3)),
            (skey("a"), int(1)),
            (skey("b"), int(2)),
        ])],
    );
    assert!(m(&mut t, &ao, "asort", vec![]).identical(&Value::Bool(true)));
    assert_eq!(
        show(&mut t, &ao),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[\"a\"=>1,\"b\"=>2,\"c\"=>3]]"
    );
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![assoc(vec![
            (ArrayKey::Int(3), int(1)),
            (ArrayKey::Int(1), int(3)),
        ])],
    );
    m(&mut t, &ao, "ksort", vec![]);
    let copy = m(&mut t, &ao, "getArrayCopy", vec![]);
    assert_eq!(show_value(&mut t, &copy), "[1=>3,3=>1]");
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(2), int(3), int(1)])],
    );
    let rev = closure(&mut t.vm, |vm, a| Ok(int(vm.compare(&a[1], &a[0])? as i64)));
    m(&mut t, &ao, "uasort", vec![rev.clone()]);
    let copy = m(&mut t, &ao, "getArrayCopy", vec![]);
    assert_eq!(show_value(&mut t, &copy), "[1=>3,0=>2,2=>1]");
    m(&mut t, &ao, "uksort", vec![rev]);
    let copy = m(&mut t, &ao, "getArrayCopy", vec![]);
    assert_eq!(show_value(&mut t, &copy), "[2=>1,1=>3,0=>2]");
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![
            s("img12.png"),
            s("img10.png"),
            s("IMG2.png"),
            s("img1.png"),
        ])],
    );
    m(&mut t, &ao, "natsort", vec![]);
    let copy = m(&mut t, &ao, "getArrayCopy", vec![]);
    assert_eq!(
        show_value(&mut t, &copy),
        "[2=>\"IMG2.png\",3=>\"img1.png\",1=>\"img10.png\",0=>\"img12.png\"]"
    );
    m(&mut t, &ao, "natcasesort", vec![]);
    let copy = m(&mut t, &ao, "getArrayCopy", vec![]);
    assert_eq!(
        show_value(&mut t, &copy),
        "[3=>\"img1.png\",2=>\"IMG2.png\",1=>\"img10.png\",0=>\"img12.png\"]"
    );
    // arrayIterator_ksort_basic1.phpt
    let it = new(
        &mut t,
        "ArrayIterator",
        vec![assoc(vec![
            (ArrayKey::Int(3), int(1)),
            (ArrayKey::Int(2), int(2)),
            (ArrayKey::Int(1), int(3)),
        ])],
    );
    assert!(m(&mut t, &it, "ksort", vec![]).identical(&Value::Bool(true)));
    assert_eq!(
        show(&mut t, &it),
        "ArrayIterator[\"\\0ArrayIterator\\0storage\"=>[1=>3,2=>2,3=>1]]"
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Method ArrayIterator::ksort() is deprecated since 8.6 in /t.php on line 3\n"
    );
}

#[test]
fn sort_with_disabled_function() {
    // asort_disabled.phpt, uasort_disabled.phpt, ...
    let mut t = TestVm::new();
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![s("hello"), s("world")])],
    );
    for f in [
        "asort",
        "ksort",
        "natsort",
        "natcasesort",
        "uasort",
        "uksort",
    ] {
        let old = std::mem::take(&mut t.vm.functions);
        for (k, v) in old.iter() {
            if k.as_bytes() != f.as_bytes() {
                t.vm.functions.insert(k.clone(), v.clone());
            }
        }
        let args = if f.starts_with('u') {
            vec![closure(&mut t.vm, |_, _| Ok(int(0)))]
        } else {
            vec![]
        };
        let r = call(&mut t.vm, &ao, f, args);
        assert_eq!(
            thrown(r),
            (
                "Error".into(),
                format!("Cannot call method {f} when function {f} is disabled")
            )
        );
    }
}

#[test]
fn modification_during_sorting_is_prohibited() {
    // ArrayObject_exchange_array_during_sorting.phpt,
    // ArrayObject_construct_during_sorting.phpt, ArrayObject_dump_during_sort.phpt
    let mut t = TestVm::new();
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(1), int(2), int(3)])],
    );
    let other = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(4), int(5), int(6)])],
    );
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let (ao2, seen2) = (ao.clone(), seen.clone());
    let cb = closure(&mut t.vm, move |vm, a| {
        if seen2.borrow().is_empty() {
            let attempts: Vec<(&str, Vec<Value>)> = vec![
                ("exchangeArray", vec![list(vec![int(4)])]),
                ("__construct", vec![Value::Object(other.clone())]),
                ("offsetSet", vec![int(0), int(9)]),
                ("offsetUnset", vec![int(0)]),
                ("append", vec![int(9)]),
                ("unserialize", vec![s("x:i:0;a:0:{};m:a:0:{}")]),
            ];
            for (name, args) in attempts {
                let r = vm.call_method(&ao2, name.as_bytes(), args);
                let (class, msg) = thrown(r);
                seen2.borrow_mut().push(format!("{name}: {class}: {msg}"));
            }
            let h = ao2.class().handlers.read_dimension.unwrap();
            let r = h(vm, &ao2, Some(&int(0)), DimFetch::Write);
            seen2.borrow_mut().push(format!("fetch-w: {}", thrown(r).1));
            // Reads still work and see the unsorted storage.
            let v = vm.call_method(&ao2, b"getArrayCopy", vec![])?;
            seen2
                .borrow_mut()
                .push(format!("{:?}", v.as_array().unwrap().len()));
        }
        Ok(int(vm.compare(&a[0], &a[1])? as i64))
    });
    m(&mut t, &ao, "uasort", vec![cb]);
    let msg = "Error: Modification of ArrayObject during sorting is prohibited";
    assert_eq!(
        *seen.borrow(),
        [
            format!("exchangeArray: {msg}"),
            format!("__construct: {msg}"),
            format!("offsetSet: {msg}"),
            format!("offsetUnset: {msg}"),
            format!("append: {msg}"),
            format!("unserialize: {msg}"),
            "fetch-w: Modification of ArrayObject during sorting is prohibited".to_string(),
            "3".to_string(),
        ]
    );
    assert_eq!(
        show(&mut t, &ao),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>1,1=>2,2=>3]]"
    );
    // The guard is released afterwards.
    m(&mut t, &ao, "offsetSet", vec![int(3), int(4)]);
    assert!(m(&mut t, &ao, "count", vec![]).identical(&int(4)));
}

#[test]
fn bug67539_unserialize_during_sort() {
    let mut t = TestVm::new();
    let it = new(
        &mut t,
        "ArrayIterator",
        vec![list(vec![s("X"), s("X")]), int(1)],
    );
    let out = Rc::new(RefCell::new(Vec::new()));
    let (it2, out2) = (it.clone(), out.clone());
    let cb = closure(&mut t.vm, move |vm, _| {
        let ser = vm.call_method(&it2, b"serialize", vec![])?;
        let r = vm.call_method(&it2, b"unserialize", vec![ser]);
        out2.borrow_mut().push(thrown(r));
        Ok(int(0))
    });
    m(&mut t, &it, "uksort", vec![cb]);
    assert_eq!(
        out.borrow()[0],
        (
            "Error".to_string(),
            "Modification of ArrayObject during sorting is prohibited".to_string()
        )
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Method ArrayIterator::uksort() is deprecated since 8.6 in /t.php on line 3\n\
         \nDeprecated: Method ArrayIterator::serialize() is deprecated since 8.6 in /t.php on line 3\n\
         \nDeprecated: Method ArrayIterator::unserialize() is deprecated since 8.6 in /t.php on line 3\n"
    );
}

// ---------------------------------------------------------------------------
// Cloning and comparison
// ---------------------------------------------------------------------------

fn clone(t: &mut TestVm, o: &ObjRef) -> ObjRef {
    (o.class().handlers.clone_obj.unwrap())(&mut t.vm, o).unwrap()
}

#[test]
fn clone_semantics() {
    // bug36941.phpt
    let mut t = TestVm::new();
    let a = new(&mut t, "ArrayObject", vec![]);
    wr(&mut t, &a, None, int(1));
    let b = clone(&mut t, &a);
    wr(&mut t, &b, Some(int(0)), int(2));
    assert!(rd(&mut t, &a, int(0)).identical(&int(1)));
    assert!(rd(&mut t, &b, int(0)).identical(&int(2)));
    let a = new(&mut t, "ArrayIterator", vec![]);
    wr(&mut t, &a, None, int(1));
    let b = clone(&mut t, &a);
    wr(&mut t, &b, Some(int(0)), int(2));
    assert!(rd(&mut t, &a, int(0)).identical(&int(2)));
    assert!(rd(&mut t, &b, int(0)).identical(&int(2)));
    // arrayObject_clone_basic2.phpt: an ArrayObject wrapping an object is
    // cloned into an array of its properties.
    let c = std_object(&mut t, &[]);
    let ao1 = new(&mut t, "ArrayObject", vec![obj(&c)]);
    c.set_prop(b"p1", s("before"));
    let ao2 = clone(&mut t, &ao1);
    c.set_prop(b"p2", s("after"));
    wr(&mut t, &ao1, Some(s("new.ao1")), s("ao1"));
    wr(&mut t, &ao2, Some(s("new.ao2")), s("ao2"));
    assert_eq!(
        show(&mut t, &c),
        "stdClass[\"p1\"=>\"before\",\"p2\"=>\"after\",\"new.ao1\"=>\"ao1\"]"
    );
    assert_eq!(
        show(&mut t, &ao2),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[\"p1\"=>\"before\",\"new.ao2\"=>\"ao2\"]]"
    );
    // ArrayObject_clone_other_std_props.phpt
    let a = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(1), int(2), int(3)]), int(1)],
    );
    let b = new(&mut t, "ArrayObject", vec![obj(&a)]);
    let c = clone(&mut t, &b);
    assert_eq!(
        show(&mut t, &c),
        "ArrayObject[\"\\0ArrayObject\\0storage\"=>[0=>1,1=>2,2=>3]]"
    );
    assert!(m(&mut t, &c, "getFlags", vec![]).identical(&int(1)));
    // arrayObject_clone_basic1.phpt: the original array is not shared.
    let arr = list(vec![int(1), int(2)]);
    let aa1 = new(&mut t, "ArrayObject", vec![arr.clone()]);
    let aa2 = clone(&mut t, &aa1);
    wr(&mut t, &aa1, Some(s("new.aa1")), s("x"));
    assert!(m(&mut t, &aa2, "count", vec![]).identical(&int(2)));
    assert_eq!(show_value(&mut t, &arr), "[0=>1,1=>2]");
}

#[test]
fn comparison() {
    // bug61326.phpt
    let mut t = TestVm::new();
    let a1 = new(&mut t, "ArrayObject", vec![list(vec![int(0)])]);
    let a2 = new(&mut t, "ArrayObject", vec![list(vec![int(1)])]);
    assert!(!t.vm.is_equal(&obj(&a1), &obj(&a2)).unwrap());
    let a3 = new(&mut t, "ArrayObject", vec![list(vec![int(0)])]);
    assert!(t.vm.is_equal(&obj(&a1), &obj(&a3)).unwrap());
    a3.set_prop(b"foo", s("bar"));
    assert!(!t.vm.is_equal(&obj(&a1), &obj(&a3)).unwrap());
    // Different classes compare through zend_std_compare_objects.
    let it = new(&mut t, "ArrayIterator", vec![list(vec![int(0)])]);
    assert!(!t.vm.is_equal(&obj(&a1), &obj(&it)).unwrap());
    let std = std_object(&mut t, &[]);
    assert!(!t.vm.is_equal(&obj(&a1), &obj(&std)).unwrap());
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn ser(t: &mut TestVm, v: Value) -> String {
    let r = t.call("serialize", vec![v]).unwrap();
    r.as_str().unwrap().to_string_lossy().into_owned()
}

fn unser(t: &mut TestVm, data: &str) -> PResult<Value> {
    t.call("unserialize", vec![s(data)])
}

#[test]
fn serialize_formats() {
    // bug45826.phpt
    let mut t = TestVm::new();
    let o = new(&mut t, "ArrayObject", vec![]);
    let y = std_object(&mut t, &[]);
    m(&mut t, &o, "append", vec![obj(&y)]);
    m(&mut t, &o, "append", vec![obj(&y)]);
    m(&mut t, &o, "append", vec![obj(&o)]);
    let s1 = ser(&mut t, obj(&o));
    let s2 = m(&mut t, &o, "serialize", vec![]);
    assert_eq!(
        s1,
        "O:11:\"ArrayObject\":4:{i:0;i:0;i:1;a:3:{i:0;O:8:\"stdClass\":0:{}i:1;r:4;i:2;r:1;}i:2;a:0:{}i:3;N;}"
    );
    assert_eq!(
        s2.as_str().unwrap().to_string(),
        "x:i:0;a:3:{i:0;O:8:\"stdClass\":0:{}i:1;r:3;i:2;O:11:\"ArrayObject\":4:{i:0;i:0;i:1;a:3:{i:0;r:3;i:1;r:3;i:2;r:5;}i:2;a:0:{}i:3;N;}};m:a:0:{}"
    );
    let o1 = unser(&mut t, &s1).unwrap();
    let o1 = o1.as_object().unwrap().clone();
    let e0 = rd(&mut t, &o1, int(0));
    let e1 = rd(&mut t, &o1, int(1));
    assert!(ObjRef::ptr_eq(
        e0.as_object().unwrap(),
        e1.as_object().unwrap()
    ));
    let e2 = rd(&mut t, &o1, int(2));
    assert!(ObjRef::ptr_eq(e2.as_object().unwrap(), &o1));
    let o2 = new(&mut t, "ArrayObject", vec![]);
    m(&mut t, &o2, "unserialize", vec![s2]);
    let e0 = rd(&mut t, &o2, int(0));
    let e1 = rd(&mut t, &o2, int(1));
    assert!(ObjRef::ptr_eq(
        e0.as_object().unwrap(),
        e1.as_object().unwrap()
    ));
    let e2 = rd(&mut t, &o2, int(2)).as_object().unwrap().clone();
    assert!(!ObjRef::ptr_eq(&e2, &o2));
    let inner = rd(&mut t, &e2, int(2));
    assert!(ObjRef::ptr_eq(inner.as_object().unwrap(), &e2));
    // array_025.phpt
    let inner = new(&mut t, "ArrayObject", vec![list(vec![int(1), int(2)])]);
    let obj1 = new(&mut t, "ArrayObject", vec![obj(&inner)]);
    let s = ser(&mut t, obj(&obj1));
    assert_eq!(
        s,
        "O:11:\"ArrayObject\":4:{i:0;i:0;i:1;O:11:\"ArrayObject\":4:{i:0;i:0;i:1;a:2:{i:0;i:1;i:1;i:2;}i:2;a:0:{}i:3;N;}i:2;a:0:{}i:3;N;}"
    );
    t.output();
    let obj2 = unser(&mut t, &s).unwrap().as_object().unwrap().clone();
    assert_eq!(
        t.output(),
        "\nDeprecated: ArrayObject::__unserialize(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n"
    );
    assert_eq!(show(&mut t, &obj2), show(&mut t, &obj1));
}

#[test]
fn serialize_round_trips_properties_and_iterator_class() {
    let mut t = TestVm::new();
    // array_023.phpt, bug70959.phpt, bug71617.phpt
    let ce = class_with_props(
        &mut t.vm,
        "Name",
        Some("ArrayObject"),
        &[
            ("var", PUB, s("a")),
            ("bar", PRO, s("b")),
            ("foo", PRI, s("c")),
        ],
    );
    let a = t.vm.instantiate(&ce).unwrap();
    a.set_prop(b"bar", s("changed"));
    wr(&mut t, &a, Some(s("k")), s("v"));
    let data = ser(&mut t, obj(&a));
    let b = unser(&mut t, &data).unwrap().as_object().unwrap().clone();
    assert_eq!(
        show(&mut t, &b),
        "Name[\"var\"=>\"a\",\"\\0*\\0bar\"=>\"changed\",\"\\0Name\\0foo\"=>\"c\",\"\\0ArrayObject\\0storage\"=>[\"k\"=>\"v\"]]"
    );
    // ArrayObject__serialize_saves_iterator_class.phpt
    user_class(&mut t.vm, "MyArrayIterator", "ArrayIterator", &[]);
    let ao = new(
        &mut t,
        "ArrayObject",
        vec![list(vec![int(1), int(2), int(3)])],
    );
    m(&mut t, &ao, "setIteratorClass", vec![s("MyArrayIterator")]);
    let data = ser(&mut t, obj(&ao));
    let back = unser(&mut t, &data).unwrap().as_object().unwrap().clone();
    assert!(m(&mut t, &back, "getIteratorClass", vec![]).identical(&s("MyArrayIterator")));
    // bug74669.phpt: self-referencing storage.
    let selfish = user_class(&mut t.vm, "SelfArray", "ArrayObject", &[]);
    let sa = t.vm.instantiate(&selfish).unwrap();
    m(&mut t, &sa, "__construct", vec![obj(&sa)]);
    wr(&mut t, &sa, Some(s("foo")), s("bar"));
    let data = ser(&mut t, obj(&sa));
    assert_eq!(
        data,
        "O:9:\"SelfArray\":4:{i:0;i:16777216;i:1;N;i:2;a:1:{s:3:\"foo\";s:3:\"bar\";}i:3;N;}"
    );
    t.output();
    let back = unser(&mut t, &data).unwrap().as_object().unwrap().clone();
    assert_eq!(
        t.output(),
        "\nDeprecated: Creation of dynamic property SelfArray::$foo is deprecated in /t.php on line 3\n"
    );
    assert_eq!(show(&mut t, &back), "SelfArray[\"foo\"=>\"bar\"]");
    assert!(rd(&mut t, &back, s("foo")).identical(&s("bar")));
    // An ArrayIterator over an ArrayObject (bug62672.phpt).
    let ao = new(&mut t, "ArrayObject", vec![]);
    let it = m(&mut t, &ao, "getIterator", vec![]);
    let data = ser(&mut t, it);
    let back = unser(&mut t, &data).unwrap();
    assert_eq!(
        t.output(),
        "\nDeprecated: ArrayIterator::__unserialize(): Using an object as a backing array for ArrayIterator is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n"
    );
    let fresh = m(&mut t, &ao, "getIterator", vec![]);
    assert!(t.vm.is_equal(&back, &fresh).unwrap());
}

#[test]
fn serializable_unserialize_errors() {
    let mut t = TestVm::new();
    // bug73029.phpt
    let r = unser(&mut t, "C:11:\"ArrayObject\":19:{x:i:0;r:2;;m:a:0:{}}");
    assert_eq!(
        thrown(r),
        (
            "UnexpectedValueException".into(),
            "Error at offset 10 of 19 bytes".into()
        )
    );
    let ao = new(&mut t, "ArrayObject", vec![]);
    for (data, offset) in [
        ("y", 0),
        ("xx", 1),
        ("x:s:1:\"a\";", 10),
        ("x:i:0;b:1;", 6),
        ("x:i:0;a:0:{}", 12),
        ("x:i:0;a:0:{};q", 13),
        ("x:i:0;a:0:{};m:i:1;", 19),
    ] {
        let r = call(&mut t.vm, &ao, "unserialize", vec![s(data)]);
        assert_eq!(
            thrown(r),
            (
                "UnexpectedValueException".into(),
                format!("Error at offset {offset} of {} bytes", data.len())
            ),
            "{data}"
        );
    }
    // ArrayObject_unserialize_empty_string.phpt
    m(&mut t, &ao, "unserialize", vec![s("")]);
    // bug74669.phpt: USE_OTHER is not a serializable flag.
    let r = unser(
        &mut t,
        "C:11:\"ArrayObject\":41:{x:i:33554432;O:8:\"stdClass\":0:{};m:a:0:{}}",
    )
    .unwrap();
    let r = r.as_object().unwrap().clone();
    assert_eq!(
        t.output(),
        "\nDeprecated: ArrayObject::unserialize(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n"
    );
    assert!(rd(&mut t, &r, s("foo")).is_null());
    assert!(m(&mut t, &r, "getFlags", vec![]).identical(&int(0)));
    t.output();
    // Flags, storage and members are restored.
    let ao = new(&mut t, "ArrayObject", vec![]);
    m(
        &mut t,
        &ao,
        "unserialize",
        vec![s("x:i:2;a:1:{s:1:\"k\";i:5;};m:a:1:{s:1:\"p\";i:7;}")],
    );
    assert!(m(&mut t, &ao, "getFlags", vec![]).identical(&int(2)));
    assert_eq!(
        show(&mut t, &ao),
        "ArrayObject[\"p\"=>7,\"\\0ArrayObject\\0storage\"=>[\"k\"=>5]]"
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Creation of dynamic property ArrayObject::$p is deprecated in /t.php on line 3\n"
    );
}

#[test]
fn unserialize_continues_after_incompatible_storage() {
    // bug70155.phpt with a stand-in for DateInterval.
    let mut t = TestVm::new();
    fn props_for(_: &mut Vm, _: &ObjRef, _: PropPurpose) -> PResult<Option<PhpArray>> {
        Ok(None)
    }
    let mut c = ClassEntry::new("Overloaded", 0);
    c.handlers.get_properties_for = Some(props_for);
    t.vm.register_class(Rc::new(c));
    let inner = "x:i:0;O:10:\"Overloaded\":0:{};m:a:1:{i:0;R:2;}";
    let data = format!("C:11:\"ArrayObject\":{}:{{{inner}}}", inner.len());
    let r = unser(&mut t, &data);
    assert_eq!(
        thrown(r),
        (
            "InvalidArgumentException".into(),
            "Overloaded object of type Overloaded is not compatible with ArrayObject".into()
        )
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: ArrayObject::unserialize(): Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants in /t.php on line 3\n\
         \nDeprecated: Creation of dynamic property ArrayObject::$0 is deprecated in /t.php on line 3\n"
    );
}

#[test]
fn magic_unserialize_errors() {
    let mut t = TestVm::new();
    let ao = new(&mut t, "ArrayObject", vec![]);
    for data in [
        list(vec![]),
        list(vec![s("0"), Value::empty_array(), Value::empty_array()]),
        list(vec![int(0), Value::empty_array()]),
        list(vec![int(0), Value::empty_array(), int(1)]),
        list(vec![
            int(0),
            Value::empty_array(),
            Value::empty_array(),
            int(5),
        ]),
    ] {
        let r = call(&mut t.vm, &ao, "__unserialize", vec![data]);
        assert_eq!(
            thrown(r),
            (
                "UnexpectedValueException".into(),
                "Incomplete or ill-typed serialization data".into()
            )
        );
    }
    let r = call(
        &mut t.vm,
        &ao,
        "__unserialize",
        vec![list(vec![int(0), int(1), Value::empty_array()])],
    );
    assert_eq!(
        thrown(r),
        (
            "InvalidArgumentException".into(),
            "Passed variable is not an array or object".into()
        )
    );
    let r = call(
        &mut t.vm,
        &ao,
        "__unserialize",
        vec![list(vec![
            int(0),
            Value::empty_array(),
            Value::empty_array(),
            s("NoSuchClass"),
        ])],
    );
    assert_eq!(
        thrown(r),
        (
            "UnexpectedValueException".into(),
            "Cannot deserialize ArrayObject with iterator class 'NoSuchClass'; no such class exists"
                .into()
        )
    );
    let r = call(
        &mut t.vm,
        &ao,
        "__unserialize",
        vec![list(vec![
            int(0),
            Value::empty_array(),
            Value::empty_array(),
            s("stdClass"),
        ])],
    );
    assert_eq!(
        thrown(r).1,
        "Cannot deserialize ArrayObject with iterator class 'stdClass'; this class is not derived from ArrayIterator"
    );
    // bug78409.phpt
    let u = list(vec![int(0), Value::empty_array(), Value::empty_array()]);
    m(&mut t, &ao, "__unserialize", vec![u]);
    assert!(m(&mut t, &ao, "count", vec![]).identical(&int(0)));
}

#[test]
fn unserialize_verifies_typed_properties() {
    // gh_9708_unserialize.phpt
    let mut t = TestVm::new();
    let mut ce = ClassEntry::new_subclass("Foo", 0, &t.vm.find_class(b"ArrayObject").unwrap());
    let mut a = PropertyInfo::new("a", "Foo", PUB);
    a.ty = Some(php_runtime::types::TypeDecl::parse("int").unwrap());
    a.default = int(5);
    ce.declare_property(a);
    let mut b = PropertyInfo::new("b", "Foo", PUB);
    b.ty = Some(php_runtime::types::TypeDecl::parse("string").unwrap());
    b.default = s("10");
    ce.declare_property(b);
    ce.link_magic_methods();
    t.vm.register_class(Rc::new(ce));
    let r = unser(
        &mut t,
        "O:3:\"Foo\":4:{i:0;i:0;i:1;a:0:{}i:2;a:2:{s:1:\"a\";s:2:\"10\";s:1:\"b\";i:5;}i:3;N;}",
    );
    assert_eq!(
        thrown(r),
        (
            "TypeError".into(),
            "Cannot assign string to property Foo::$a of type int".into()
        )
    );
    // gh_9707_unserialize.phpt: references between typed properties survive.
    let mut ce = ClassEntry::new_subclass("Foo2", 0, &t.vm.find_class(b"ArrayObject").unwrap());
    for n in ["a", "b"] {
        let mut p = PropertyInfo::new(n, "Foo2", PUB);
        p.ty = Some(php_runtime::types::TypeDecl::parse("int").unwrap());
        p.default = int(0);
        ce.declare_property(p);
    }
    ce.link_magic_methods();
    t.vm.register_class(Rc::new(ce));
    let o = unser(
        &mut t,
        "O:4:\"Foo2\":4:{i:0;i:0;i:1;a:0:{}i:2;a:2:{s:1:\"a\";i:1;s:1:\"b\";R:5;}i:3;N;}",
    )
    .unwrap();
    let o = o.as_object().unwrap().clone();
    let Some(Value::Ref(a)) = o.obj().props.borrow().first().cloned() else {
        panic!("a must be a reference")
    };
    a.set(int(2));
    assert!(o.get_prop(b"b").unwrap().identical(&int(2)));
}
