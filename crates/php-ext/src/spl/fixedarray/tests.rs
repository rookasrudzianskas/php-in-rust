//! Tests ported from the SplFixedArray `.phpt` files (`fixedarray_*`,
//! `SplFixedArray_*`, `splfixedarray_*` and the related `bug*`/`gh*`).

use std::cell::RefCell;

use php_runtime::class::{DimFetch, PropPurpose, class_flags, fn_flags, member_flags};
use php_runtime::{Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, ResRef, Value, Vm};

use super::test_helpers::{
    Prop, class_with_props, drain_pending_destructors, echo, parent_call, probe, weak,
};
use crate::spl::engine::{self, ObjIterator};
use crate::spl::testutil::{assoc, call, call_static, entries, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

fn fa(t: &mut TestVm, size: i64) -> ObjRef {
    new_obj(&mut t.vm, "SplFixedArray", vec![Value::Int(size)]).unwrap()
}

fn read(t: &mut TestVm, o: &ObjRef, off: Value) -> PResult<Value> {
    let f = o.class().handlers.read_dimension.unwrap();
    f(&mut t.vm, o, Some(&off), DimFetch::Read)
}

fn read_is(t: &mut TestVm, o: &ObjRef, off: Value) -> PResult<Value> {
    let f = o.class().handlers.read_dimension.unwrap();
    f(&mut t.vm, o, Some(&off), DimFetch::IsSet)
}

fn write(t: &mut TestVm, o: &ObjRef, off: Value, v: Value) -> PResult<()> {
    let f = o.class().handlers.write_dimension.unwrap();
    f(&mut t.vm, o, Some(&off), v)
}

fn append(t: &mut TestVm, o: &ObjRef, v: Value) -> PResult<()> {
    let f = o.class().handlers.write_dimension.unwrap();
    f(&mut t.vm, o, None, v)
}

fn isset(t: &mut TestVm, o: &ObjRef, off: Value) -> PResult<bool> {
    let f = o.class().handlers.has_dimension.unwrap();
    f(&mut t.vm, o, &off, false)
}

fn empty(t: &mut TestVm, o: &ObjRef, off: Value) -> PResult<bool> {
    let f = o.class().handlers.has_dimension.unwrap();
    Ok(!f(&mut t.vm, o, &off, true)?)
}

fn unset(t: &mut TestVm, o: &ObjRef, off: Value) -> PResult<()> {
    let f = o.class().handlers.unset_dimension.unwrap();
    f(&mut t.vm, o, &off)
}

fn count(t: &mut TestVm, o: &ObjRef) -> PResult<i64> {
    let f = o.class().handlers.count_elements.unwrap();
    Ok(f(&mut t.vm, o)?.expect("count_elements succeeds"))
}

fn props_for(t: &mut TestVm, o: &ObjRef, purpose: PropPurpose) -> Value {
    let f = o.class().handlers.get_properties_for.unwrap();
    Value::array(f(&mut t.vm, o, purpose).unwrap().unwrap())
}

fn to_array(t: &mut TestVm, o: &ObjRef) -> Value {
    call(&mut t.vm, o, "toArray", vec![]).unwrap()
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn same(a: &Value, b: &Value) -> bool {
    a.identical(b)
}

/// `foreach ($it as $k => $v)` over an Iterator object.
fn walk(t: &mut TestVm, it: &ObjRef, mut body: impl FnMut(&mut Vm, Value, Value)) {
    let mut iter = ObjIterator::over(it.clone());
    iter.rewind(&mut t.vm).unwrap();
    while iter.valid(&mut t.vm).unwrap() {
        let v = iter.current(&mut t.vm).unwrap();
        let k = iter.key(&mut t.vm).unwrap();
        body(&mut t.vm, k, v);
        iter.next(&mut t.vm).unwrap();
    }
}

fn get_iterator(t: &mut TestVm, o: &ObjRef) -> ObjRef {
    match call(&mut t.vm, o, "getIterator", vec![]).unwrap() {
        Value::Object(it) => it,
        v => panic!("getIterator() returned {v:?}"),
    }
}

// ---------------------------------------------------------------------------
// Class shape
// ---------------------------------------------------------------------------

#[test]
fn class_shape() {
    let mut t = TestVm::new();
    let r = t
        .call("class_implements", vec![s("SplFixedArray")])
        .unwrap();
    let names: Vec<String> = entries(&r).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        names,
        [
            "IteratorAggregate",
            "Traversable",
            "ArrayAccess",
            "Countable",
            "JsonSerializable"
        ]
    );
    let ce = t.vm.find_class(b"SplFixedArray").unwrap();
    let from = ce.find_method(b"fromarray").unwrap();
    assert!(from.is_static());
    let wakeup = ce.find_method(b"__wakeup").unwrap();
    assert!(wakeup.has_flag(fn_flags::DEPRECATED));

    let ii = t.vm.find_class(b"InternalIterator").unwrap();
    assert!(ii.has_flag(class_flags::FINAL));
    assert!(ii.has_flag(class_flags::NOT_SERIALIZABLE));
    assert!(ii.instance_of_name(b"Iterator"));
    assert_eq!(
        ii.find_method(b"__construct").unwrap().visibility(),
        member_flags::PRIVATE
    );
    assert_eq!(ii.module, Some("Core"));
}

// ---------------------------------------------------------------------------
// fixedarray_001 / fixedarray_002
// ---------------------------------------------------------------------------

#[test]
fn std_operations() {
    // fixedarray_001.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 0);
    let r = write(&mut t, &a, i(0), s("value1"));
    assert_eq!(
        thrown(r.map(|_| Value::Null)),
        (
            "OutOfBoundsException".into(),
            "Index invalid or out of range".into()
        )
    );
    assert_eq!(
        thrown(read(&mut t, &a, s("asdf"))),
        (
            "TypeError".into(),
            "Cannot access offset of type string on SplFixedArray".into()
        )
    );
    assert_eq!(
        thrown(unset(&mut t, &a, i(-1)).map(|_| Value::Null)),
        (
            "OutOfBoundsException".into(),
            "Index invalid or out of range".into()
        )
    );
    call(&mut t.vm, &a, "setSize", vec![i(10)]).unwrap();
    for n in 0..4 {
        write(&mut t, &a, i(n), s(&format!("value{n}"))).unwrap();
    }
    // $ref2 =& $ref; $a[4] = $ref; $ref = "value5";
    let r = php_runtime::PhpRef::new(s("value4"));
    write(&mut t, &a, i(4), Value::Ref(r.clone())).unwrap();
    r.set(s("value5"));
    unset(&mut t, &a, i(1)).unwrap();
    for (n, want) in [(0, "value0"), (2, "value2"), (3, "value3"), (4, "value4")] {
        assert!(same(&read(&mut t, &a, i(n)).unwrap(), &s(want)));
    }
    assert!(read(&mut t, &a, i(1)).unwrap().is_null());
    assert_eq!(count(&mut t, &a).unwrap(), 10);
    assert!(same(
        &call(&mut t.vm, &a, "getSize", vec![]).unwrap(),
        &i(10)
    ));
    // clonable
    let clone = a.class().handlers.clone_obj.unwrap();
    let b = clone(&mut t.vm, &a).unwrap();
    write(&mut t, &a, i(0), s("valueNew")).unwrap();
    assert!(same(&read(&mut t, &b, i(0)).unwrap(), &s("value0")));
    assert_eq!(t.output(), "");
}

fn log_method(vm: &mut Vm, args: &mut Args, name: &str, parent: &str) -> PResult<Value> {
    echo(vm, &format!("A::{name}\n"))?;
    let this = engine::this(args);
    parent_call(vm, &this, "SplFixedArray", parent, args.values().to_vec())
}

fn a_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log_method(vm, args, "offsetGet", "offsetGet")
}
fn a_offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log_method(vm, args, "offsetSet", "offsetSet")
}
fn a_offset_unset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log_method(vm, args, "offsetUnset", "offsetUnset")
}
fn a_offset_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    log_method(vm, args, "offsetExists", "offsetExists")
}
fn a_count(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(2))
}

static A_OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($n): mixed {}",
    handler: a_offset_get,
};
static A_OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet($n, $v): void {}",
    handler: a_offset_set,
};
static A_OFFSET_UNSET: NativeFunction = NativeFunction {
    sig: "public function offsetUnset($n): void {}",
    handler: a_offset_unset,
};
static A_OFFSET_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists($n): bool {}",
    handler: a_offset_exists,
};
static A_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: a_count,
};

#[test]
fn overloading_dispatches_to_user_methods() {
    // fixedarray_002.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "A",
        "SplFixedArray",
        0,
        vec![
            Prop::public("prop1", Value::Null),
            Prop::public("prop2", Value::Null),
        ],
        &[
            &A_COUNT,
            &A_OFFSET_GET,
            &A_OFFSET_SET,
            &A_OFFSET_UNSET,
            &A_OFFSET_EXISTS,
        ],
    );
    let a = new_obj(&mut t.vm, "A", vec![]).unwrap();
    let e = write(&mut t, &a, i(0), s("value1")).map(|_| Value::Null);
    assert_eq!(thrown(e).0, "OutOfBoundsException");
    let e = read(&mut t, &a, s("asdf"));
    assert_eq!(
        thrown(e),
        (
            "TypeError".into(),
            "Cannot access offset of type string on SplFixedArray".into()
        )
    );
    let e = unset(&mut t, &a, i(-1)).map(|_| Value::Null);
    assert_eq!(thrown(e).0, "OutOfBoundsException");
    call(&mut t.vm, &a, "setSize", vec![i(10)]).unwrap();
    for n in 0..5 {
        write(&mut t, &a, i(n), s(&format!("value{n}"))).unwrap();
    }
    unset(&mut t, &a, i(1)).unwrap();
    let results = [
        isset(&mut t, &a, i(1)).unwrap(),
        isset(&mut t, &a, i(2)).unwrap(),
        empty(&mut t, &a, i(1)).unwrap(),
        empty(&mut t, &a, i(2)).unwrap(),
    ];
    assert_eq!(results, [false, true, true, false]);
    for (n, want) in [(0, "value0"), (2, "value2"), (3, "value3"), (4, "value4")] {
        assert!(same(&read(&mut t, &a, i(n)).unwrap(), &s(want)));
    }
    assert_eq!(count(&mut t, &a).unwrap(), 2);
    assert!(same(
        &call(&mut t.vm, &a, "getSize", vec![]).unwrap(),
        &i(10)
    ));
    assert_eq!(
        t.output(),
        "A::offsetSet\nA::offsetGet\nA::offsetUnset\n\
         A::offsetSet\nA::offsetSet\nA::offsetSet\nA::offsetSet\nA::offsetSet\n\
         A::offsetUnset\n\
         A::offsetExists\nA::offsetExists\nA::offsetExists\nA::offsetExists\n\
         A::offsetGet\nA::offsetGet\nA::offsetGet\nA::offsetGet\n"
    );
}

// ---------------------------------------------------------------------------
// fixedarray_003: offset conversion
// ---------------------------------------------------------------------------

#[test]
fn non_integer_offsets() {
    // fixedarray_003.phpt
    let mut t = TestVm::new();
    let res = ResRef::new("stream", Box::new(()));
    let id = res.id();
    let o = fa(&mut t, 10.max(id + 1));
    let r = Value::Resource(res);
    let deprecated = "\nDeprecated: Implicit conversion from float 2.5 to int loses precision in /t.php on line 3\n";
    let warning = format!(
        "\nWarning: Resource ID#{id} used as offset, casting to integer ({id}) in /t.php on line 3\n"
    );
    let type_err = |ty: &str| format!("Cannot access offset of type {ty} on SplFixedArray");
    let obj = Value::Object(new_obj(&mut t.vm, "stdClass", vec![]).unwrap());

    // Write context
    write(&mut t, &o, Value::Bool(false), s("a")).unwrap();
    write(&mut t, &o, Value::Bool(true), s("b")).unwrap();
    write(&mut t, &o, Value::Float(2.5), s("c")).unwrap();
    assert_eq!(t.output(), deprecated);
    let e = write(&mut t, &o, Value::empty_array(), s("d")).map(|_| Value::Null);
    assert_eq!(thrown(e), ("TypeError".into(), type_err("array")));
    let e = write(&mut t, &o, obj.clone(), s("e")).map(|_| Value::Null);
    assert_eq!(thrown(e), ("TypeError".into(), type_err("stdClass")));
    write(&mut t, &o, r.clone(), s("f")).unwrap();
    assert_eq!(t.output(), warning);
    write(&mut t, &o, s("3"), s("g")).unwrap();
    for bad in ["3.5", "03", " 3"] {
        let e = write(&mut t, &o, s(bad), s("h")).map(|_| Value::Null);
        assert_eq!(thrown(e), ("TypeError".into(), type_err("string")));
    }

    // Read context
    assert!(same(
        &read(&mut t, &o, Value::Bool(false)).unwrap(),
        &s("a")
    ));
    assert!(same(&read(&mut t, &o, Value::Bool(true)).unwrap(), &s("b")));
    assert!(same(&read(&mut t, &o, Value::Float(2.5)).unwrap(), &s("c")));
    assert_eq!(t.output(), deprecated);
    assert_eq!(
        thrown(read(&mut t, &o, Value::empty_array())).1,
        type_err("array")
    );
    assert_eq!(
        thrown(read(&mut t, &o, obj.clone())).1,
        type_err("stdClass")
    );
    assert!(same(&read(&mut t, &o, r.clone()).unwrap(), &s("f")));
    assert_eq!(t.output(), warning);
    assert!(same(&read(&mut t, &o, s("3")).unwrap(), &s("g")));
    for bad in ["3.5", "03", " 3"] {
        assert_eq!(thrown(read(&mut t, &o, s(bad))).1, type_err("string"));
    }

    // isset()
    assert!(isset(&mut t, &o, Value::Bool(false)).unwrap());
    assert!(isset(&mut t, &o, Value::Bool(true)).unwrap());
    assert!(isset(&mut t, &o, Value::Float(2.5)).unwrap());
    assert_eq!(t.output(), deprecated);
    let e = isset(&mut t, &o, Value::empty_array()).map(Value::Bool);
    assert_eq!(thrown(e).1, type_err("array"));
    assert!(isset(&mut t, &o, r.clone()).unwrap());
    assert_eq!(t.output(), warning);
    assert!(isset(&mut t, &o, s("3")).unwrap());
    let e = isset(&mut t, &o, s("03")).map(Value::Bool);
    assert_eq!(thrown(e).1, type_err("string"));

    // empty()
    assert!(!empty(&mut t, &o, Value::Bool(false)).unwrap());
    assert!(!empty(&mut t, &o, Value::Float(2.5)).unwrap());
    assert_eq!(t.output(), deprecated);
    let e = empty(&mut t, &o, obj).map(Value::Bool);
    assert_eq!(thrown(e).1, type_err("stdClass"));
    assert!(!empty(&mut t, &o, s("3")).unwrap());
    let e = empty(&mut t, &o, s(" 3")).map(Value::Bool);
    assert_eq!(thrown(e).1, type_err("string"));
}

#[test]
fn reference_offsets() {
    // bug70853.phpt
    let mut t = TestVm::new();
    let o = fa(&mut t, 10);
    let r = php_runtime::PhpRef::new(i(1));
    write(&mut t, &o, Value::Ref(r.clone()), i(123)).unwrap();
    assert!(same(&read(&mut t, &o, i(1)).unwrap(), &i(123)));
}

// ---------------------------------------------------------------------------
// [] operator
// ---------------------------------------------------------------------------

#[test]
fn append_is_not_supported() {
    // fixedarray_004.phpt, fixedarray_006.phpt, fixedarray_012.phpt, fixedarray_013.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 10);
    let msg = (
        "Error".to_string(),
        "[] operator not supported for SplFixedArray".to_string(),
    );
    assert_eq!(thrown(append(&mut t, &a, i(1)).map(|_| Value::Null)), msg);
    let f = a.class().handlers.read_dimension.unwrap();
    assert_eq!(thrown(f(&mut t.vm, &a, None, DimFetch::Write)), msg);
    assert_eq!(thrown(f(&mut t.vm, &a, None, DimFetch::Read)), msg);
}

/// `var_dump()` of scalars.
fn dump_scalar(vm: &mut Vm, v: &Value) -> PResult<()> {
    let text = match v {
        Value::Null => "NULL".to_string(),
        Value::Int(n) => format!("int({n})"),
        v => format!("{}", vm.to_php_string(v)?),
    };
    echo(vm, &format!("{text}\n"))
}

fn so_offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let off = args.get(0).clone();
    dump_scalar(vm, &off)?;
    Ok(Value::Null)
}
static SO_OFFSET_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet($offset, $value): void {}",
    handler: so_offset_set,
};

fn mfa_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let off = args.get(0).clone();
    dump_scalar(vm, &off)?;
    Ok(Value::Null)
}
static MFA_OFFSET_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($offset): mixed {}",
    handler: mfa_offset_get,
};

#[test]
fn append_reaches_overridden_methods_with_null() {
    // bug53362.phpt, bug64106.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "obj", "SplFixedArray", &[&SO_OFFSET_SET]);
    let o = new_obj(&mut t.vm, "obj", vec![]).unwrap();
    for _ in 0..3 {
        append(&mut t, &o, i(2)).unwrap();
    }
    assert_eq!(t.output(), "NULL\nNULL\nNULL\n");

    user_class(
        &mut t.vm,
        "MyFixedArray",
        "SplFixedArray",
        &[&MFA_OFFSET_GET],
    );
    let o = new_obj(&mut t.vm, "MyFixedArray", vec![i(10)]).unwrap();
    let f = o.class().handlers.read_dimension.unwrap();
    let r = f(&mut t.vm, &o, None, DimFetch::Write).unwrap();
    assert!(r.is_null());
    assert_eq!(t.output(), "NULL\n");
}

// ---------------------------------------------------------------------------
// Self references, nested writes, read-modify-write
// ---------------------------------------------------------------------------

#[test]
fn self_reference_and_nested_write() {
    // fixedarray_007.phpt, fixedarray_008.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 3);
    write(&mut t, &a, i(0), i(1)).unwrap();
    write(&mut t, &a, i(1), i(2)).unwrap();
    write(&mut t, &a, i(2), Value::Object(a.clone())).unwrap();
    // $a[2][0] = 3;
    let f = a.class().handlers.read_dimension.unwrap();
    let inner = f(&mut t.vm, &a, Some(&i(2)), DimFetch::Write).unwrap();
    let Value::Object(inner) = inner else {
        panic!()
    };
    write(&mut t, &inner, i(0), i(3)).unwrap();
    let it = get_iterator(&mut t, &a);
    let mut seen = Vec::new();
    walk(&mut t, &it, |_, _, v| seen.push(v));
    assert!(same(&seen[0], &i(3)));
    assert!(same(&seen[1], &i(2)));
    assert!(matches!(&seen[2], Value::Object(o) if ObjRef::ptr_eq(o, &a)));
    assert_eq!(size_of(&mut t, &a), 3);
    // Break the cycle.
    write(&mut t, &a, i(2), Value::Null).unwrap();
}

fn size_of(t: &mut TestVm, o: &ObjRef) -> i64 {
    call(&mut t.vm, o, "getSize", vec![])
        .unwrap()
        .as_int()
        .unwrap()
}

#[test]
fn read_write_fetch_of_missing_index() {
    // fixedarray_014.phpt: $a[0]++ on an empty array
    let mut t = TestVm::new();
    let a = fa(&mut t, 0);
    let f = a.class().handlers.read_dimension.unwrap();
    assert_eq!(
        thrown(f(&mut t.vm, &a, Some(&i(0)), DimFetch::ReadWrite)),
        (
            "OutOfBoundsException".into(),
            "Index invalid or out of range".into()
        )
    );
}

// ---------------------------------------------------------------------------
// Sizing
// ---------------------------------------------------------------------------

#[test]
fn set_size() {
    // fixedarray_010.phpt, bug54970.phpt, SplFixedArray_setsize_*.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 3);
    write(&mut t, &a, i(0), i(1)).unwrap();
    for n in [2, 3, 0] {
        call(&mut t.vm, &a, "setSize", vec![i(n)]).unwrap();
        assert_eq!(size_of(&mut t, &a), n);
    }
    for (init, new) in [(0, 0), (10, 10), (1, 5), (20, 3)] {
        let a = fa(&mut t, init);
        let r = call(&mut t.vm, &a, "setSize", vec![i(new)]).unwrap();
        assert!(same(&r, &Value::Bool(true)));
        assert_eq!(size_of(&mut t, &a), new);
    }

    let a = fa(&mut t, 2);
    write(&mut t, &a, i(0), s("Hello")).unwrap();
    write(&mut t, &a, i(1), s("World")).unwrap();
    call(&mut t.vm, &a, "setSize", vec![i(3)]).unwrap();
    write(&mut t, &a, i(2), s("!")).unwrap();
    assert!(same(
        &to_array(&mut t, &a),
        &list(vec![s("Hello"), s("World"), s("!")])
    ));
    call(&mut t.vm, &a, "setSize", vec![i(2)]).unwrap();
    assert!(same(
        &to_array(&mut t, &a),
        &list(vec![s("Hello"), s("World")])
    ));

    // SplFixedArray_setSize_param_null.phpt
    let a = fa(&mut t, 2);
    call(&mut t.vm, &a, "setSize", vec![Value::Null]).unwrap();
    assert_eq!(size_of(&mut t, &a), 0);
    assert_eq!(
        t.output(),
        "\nDeprecated: SplFixedArray::setSize(): Passing null to parameter #1 ($size) of type int is deprecated in /t.php on line 3\n"
    );
    // fixedarray_021.phpt
    let e = call(&mut t.vm, &a, "setSize", vec![i(-5)]);
    assert_eq!(
        thrown(e),
        (
            "ValueError".into(),
            "SplFixedArray::setSize(): Argument #1 ($size) must be greater than or equal to 0"
                .into()
        )
    );
}

#[test]
fn set_size_reports_allocation_overflow() {
    // bug67247.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 1);
    let r = call(&mut t.vm, &a, "setSize", vec![i(0x2000000000000001)]);
    assert!(matches!(r, Err(php_runtime::Flow::Fatal)));
    assert!(t.output().starts_with(
        "\nFatal error: Possible integer overflow in memory allocation (2305843009213693953 * 16 + 0) in /t.php on line 3\n"
    ));
}

#[test]
fn set_size_keeps_declared_properties() {
    // bug64264.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "MyFixedArray",
        "SplFixedArray",
        0,
        vec![
            Prop {
                name: "foo",
                flags: member_flags::PROTECTED,
                default: Value::Null,
                ty: None,
            },
            Prop {
                name: "bar",
                flags: member_flags::PROTECTED,
                default: Value::Null,
                ty: None,
            },
        ],
        &[],
    );
    let a = new_obj(&mut t.vm, "MyFixedArray", vec![i(1)]).unwrap();
    write(&mut t, &a, i(0), s("foo")).unwrap();
    call(&mut t.vm, &a, "setSize", vec![i(2)]).unwrap();
    write(&mut t, &a, i(1), s("bar")).unwrap();
    call(&mut t.vm, &a, "setSize", vec![i(5)]).unwrap();
    let _array = to_array(&mut t, &a);
    call(&mut t.vm, &a, "setSize", vec![i(2)]).unwrap();
    assert!(same(&to_array(&mut t, &a), &list(vec![s("foo"), s("bar")])));
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn constructor_arguments() {
    // SplFixedArray__construct_param_*.phpt, SplFixedArray_construct_param_SplFixedArray.phpt
    let mut t = TestVm::new();
    let msg = |ty: &str| {
        format!("SplFixedArray::__construct(): Argument #1 ($size) must be of type int, {ty} given")
    };
    let e = new_obj(
        &mut t.vm,
        "SplFixedArray",
        vec![list(vec![s("string"), i(1)])],
    );
    assert_eq!(
        thrown(e.map(Value::Object)),
        ("TypeError".into(), msg("array"))
    );
    let e = new_obj(&mut t.vm, "SplFixedArray", vec![s("string")]);
    assert_eq!(
        thrown(e.map(Value::Object)),
        ("TypeError".into(), msg("string"))
    );
    let inner = fa(&mut t, 3);
    let e = new_obj(&mut t.vm, "SplFixedArray", vec![Value::Object(inner)]);
    assert_eq!(
        thrown(e.map(Value::Object)),
        ("TypeError".into(), msg("SplFixedArray"))
    );

    let a = new_obj(&mut t.vm, "SplFixedArray", vec![Value::Null]).unwrap();
    assert_eq!(size_of(&mut t, &a), 0);
    assert_eq!(
        t.output(),
        "\nDeprecated: SplFixedArray::__construct(): Passing null to parameter #1 ($size) of type int is deprecated in /t.php on line 3\n"
    );
}

#[test]
fn misc_small_tests() {
    // fixedarray_021.phpt
    let mut t = TestVm::new();
    let a = new_obj(&mut t.vm, "SplFixedArray", vec![]).unwrap();
    assert_eq!(count(&mut t, &a).unwrap(), 0);
    assert!(same(&call(&mut t.vm, &a, "count", vec![]).unwrap(), &i(0)));
    let e = new_obj(&mut t.vm, "SplFixedArray", vec![i(-10)]);
    assert_eq!(
        thrown(e.map(Value::Object)),
        (
            "ValueError".into(),
            "SplFixedArray::__construct(): Argument #1 ($size) must be greater than or equal to 0"
                .into()
        )
    );
    // calling __construct() twice
    let c = fa(&mut t, 0);
    assert!(
        call(&mut t.vm, &c, "__construct", vec![])
            .unwrap()
            .is_null()
    );
    let c = fa(&mut t, 3);
    call(&mut t.vm, &c, "__construct", vec![i(7)]).unwrap();
    assert_eq!(size_of(&mut t, &c), 3);
    // fromArray() from an empty array
    let d = call_static(
        &mut t.vm,
        "SplFixedArray",
        "fromArray",
        vec![Value::empty_array()],
    )
    .unwrap();
    assert_eq!(size_of(&mut t, d.as_object().unwrap()), 0);
    // foreach by ref
    let e = super::get_iterator(&mut t.vm, &a, true).map(|_| Value::Null);
    assert_eq!(
        thrown(e),
        (
            "Error".into(),
            "An iterator cannot be used with foreach by reference".into()
        )
    );
    // non-long indexes
    let a = fa(&mut t, 4);
    write(&mut t, &a, s("2"), s("foo")).unwrap();
    write(&mut t, &a, s("1"), s("foo")).unwrap();
    write(&mut t, &a, s("3"), s("0")).unwrap();
    assert!(!isset(&mut t, &a, s("0")).unwrap());
    assert!(!isset(&mut t, &a, i(-1)).unwrap());
    assert!(same(&read(&mut t, &a, s("1")).unwrap(), &s("foo")));
    assert!(empty(&mut t, &a, s("3")).unwrap());
}

static UNCONSTRUCTED_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct() {}",
    handler: |_vm, _args| Ok(Value::Null),
};

fn late_init(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let size = args.get(0).clone();
    parent_call(vm, &this, "SplFixedArray", "__construct", vec![size])?;
    Ok(Value::Null)
}
static LATE_INIT: NativeFunction = NativeFunction {
    sig: "public function init(int $size): void {}",
    handler: late_init,
};

#[test]
fn subclass_without_parent_constructor() {
    // SplFixedArray_subclass_without_parent_construct.phpt, bug62904.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "Unconstructed",
        "SplFixedArray",
        &[&UNCONSTRUCTED_CONSTRUCT],
    );
    let a = new_obj(&mut t.vm, "Unconstructed", vec![]).unwrap();
    assert_eq!(size_of(&mut t, &a), 0);
    call(&mut t.vm, &a, "setSize", vec![i(3)]).unwrap();
    assert_eq!(size_of(&mut t, &a), 3);
    write(&mut t, &a, i(0), s("x")).unwrap();
    write(&mut t, &a, i(2), s("z")).unwrap();
    assert!(same(
        &to_array(&mut t, &a),
        &list(vec![s("x"), Value::Null, s("z")])
    ));
    call(&mut t.vm, &a, "setSize", vec![i(1)]).unwrap();
    assert_eq!(size_of(&mut t, &a), 1);
    call(&mut t.vm, &a, "setSize", vec![i(0)]).unwrap();
    assert_eq!(size_of(&mut t, &a), 0);

    user_class(
        &mut t.vm,
        "LateInit",
        "SplFixedArray",
        &[&UNCONSTRUCTED_CONSTRUCT, &LATE_INIT],
    );
    let b = new_obj(&mut t.vm, "LateInit", vec![]).unwrap();
    call(&mut t.vm, &b, "init", vec![i(2)]).unwrap();
    assert_eq!(size_of(&mut t, &b), 2);

    let fresh = new_obj(&mut t.vm, "Unconstructed", vec![]).unwrap();
    let clone = fresh.class().handlers.clone_obj.unwrap();
    let c = clone(&mut t.vm, &fresh).unwrap();
    call(&mut t.vm, &c, "setSize", vec![i(2)]).unwrap();
    assert_eq!(size_of(&mut t, &c), 2);
    assert_eq!(c.class().name.to_string(), "Unconstructed");
}

// ---------------------------------------------------------------------------
// fromArray / toArray
// ---------------------------------------------------------------------------

fn from_array(t: &mut TestVm, a: Value, preserve: Option<bool>) -> PResult<Value> {
    let mut args = vec![a];
    if let Some(p) = preserve {
        args.push(Value::Bool(p));
    }
    call_static(&mut t.vm, "SplFixedArray", "fromArray", args)
}

#[test]
fn from_array_and_to_array() {
    // fixedarray_020.phpt
    let mut t = TestVm::new();
    let a = assoc(vec![
        (ArrayKey::Int(1), s("foo")),
        (ArrayKey::Int(2), s("bar")),
        (ArrayKey::Int(0), s("gee")),
    ]);
    let fa1 = from_array(&mut t, a.clone(), Some(false)).unwrap();
    let fa1 = fa1.as_object().unwrap().clone();
    assert_eq!(count(&mut t, &fa1).unwrap(), 3);
    assert!(same(
        &to_array(&mut t, &fa1),
        &list(vec![s("foo"), s("bar"), s("gee")])
    ));

    let fa2 = from_array(&mut t, a.clone(), Some(true)).unwrap();
    let fa2 = fa2.as_object().unwrap().clone();
    assert_eq!(count(&mut t, &fa2).unwrap(), 3);
    let arr = to_array(&mut t, &fa2);
    assert!(!same(&arr, &a));
    let cast = Value::array(t.vm.cast_to_array(&Value::Object(fa2.clone())).unwrap());
    assert!(same(&arr, &cast));
    assert!(same(&arr, &list(vec![s("gee"), s("foo"), s("bar")])));

    from_array(&mut t, assoc(vec![(skey("foo"), s("bar"))]), Some(false)).unwrap();
    let e = from_array(&mut t, assoc(vec![(skey("foo"), s("bar"))]), Some(true));
    assert_eq!(
        thrown(e),
        (
            "InvalidArgumentException".into(),
            "array must contain only positive integer keys".into()
        )
    );
    let e = from_array(&mut t, assoc(vec![(ArrayKey::Int(-1), s("x"))]), None);
    assert_eq!(thrown(e).1, "array must contain only positive integer keys");
    let e = from_array(&mut t, assoc(vec![(ArrayKey::Int(i64::MAX), s("x"))]), None);
    assert_eq!(
        thrown(e),
        (
            "InvalidArgumentException".into(),
            "integer overflow detected".into()
        )
    );
}

#[test]
fn from_array_indexes() {
    // SplFixedArray_fromarray_indexes.phpt, _non_indexes.phpt, _param_multiarray.phpt
    let mut t = TestVm::new();
    let a = assoc(vec![
        (ArrayKey::Int(1), i(1)),
        (ArrayKey::Int(2), s("2")),
        (ArrayKey::Int(3), Value::Bool(false)),
    ]);
    let r = from_array(&mut t, a.clone(), None).unwrap();
    let r = r.as_object().unwrap().clone();
    assert!(same(
        &to_array(&mut t, &r),
        &list(vec![Value::Null, i(1), s("2"), Value::Bool(false)])
    ));
    let r = from_array(&mut t, a, Some(false)).unwrap();
    let r = r.as_object().unwrap().clone();
    assert!(same(
        &to_array(&mut t, &r),
        &list(vec![i(1), s("2"), Value::Bool(false)])
    ));
    let nested = list(vec![list(vec![s("1")])]);
    let r = from_array(&mut t, nested.clone(), None).unwrap();
    let r = r.as_object().unwrap().clone();
    assert!(same(&to_array(&mut t, &r), &nested));
    assert_eq!(r.class().name.to_string(), "SplFixedArray");
}

#[test]
fn to_array_of_empty_array() {
    // SplFixedArray_toArray_empty.phpt
    let mut t = TestVm::new();
    let a = new_obj(&mut t.vm, "SplFixedArray", vec![]).unwrap();
    assert!(same(&to_array(&mut t, &a), &Value::empty_array()));
}

// ---------------------------------------------------------------------------
// offsetExists / offsetUnset / empty()
// ---------------------------------------------------------------------------

#[test]
fn offset_methods() {
    // SplFixedArray_offsetExists_less_than_zero.phpt, splfixedarray_offsetExists_larger.phpt,
    // SplFixedArray_offsetUnset_string.phpt, fixedarray_016.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 5);
    let r = call(&mut t.vm, &a, "offsetExists", vec![i(-10)]).unwrap();
    assert!(same(&r, &Value::Bool(false)));
    for n in 0..5 {
        call(&mut t.vm, &a, "offsetSet", vec![i(n), s("PHPNW Testfest")]).unwrap();
    }
    let r = call(&mut t.vm, &a, "offsetExists", vec![i(4)]).unwrap();
    assert!(same(&r, &Value::Bool(true)));
    let r = call(&mut t.vm, &a, "offsetExists", vec![i(5)]).unwrap();
    assert!(same(&r, &Value::Bool(false)));
    call(&mut t.vm, &a, "offsetUnset", vec![s("4")]).unwrap();
    let arr = to_array(&mut t, &a);
    assert!(array_get(&arr, 4).is_null());
    assert!(same(&array_get(&arr, 3), &s("PHPNW Testfest")));
    let e = call(&mut t.vm, &a, "offsetGet", vec![Value::Null]);
    assert_eq!(
        thrown(e).1,
        "Cannot access offset of type null on SplFixedArray"
    );
    let e = call(&mut t.vm, &a, "offsetGet", vec![i(5)]);
    assert_eq!(thrown(e).1, "Index invalid or out of range");

    let b = fa(&mut t, 2);
    write(&mut t, &b, i(0), s("foo")).unwrap();
    assert!(!empty(&mut t, &b, i(0)).unwrap());
    assert!(empty(&mut t, &b, i(1)).unwrap());
    let dump = props_for(&mut t, &b, PropPurpose::Debug);
    assert!(same(&dump, &list(vec![s("foo"), Value::Null])));
}

fn array_get(v: &Value, n: i64) -> Value {
    v.as_array().unwrap().get_int(n).cloned().unwrap()
}

#[test]
fn null_coalescing_uses_isset_fetch() {
    // bug79393.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 5);
    let vals = [s("bar1"), s("bar2"), i(0), Value::Bool(false), s("")];
    for (n, v) in vals.iter().enumerate() {
        write(&mut t, &a, i(n as i64), v.clone()).unwrap();
    }
    for (n, v) in vals.iter().enumerate() {
        assert!(same(&read_is(&mut t, &a, i(n as i64)).unwrap(), v));
    }
    assert!(read_is(&mut t, &a, i(5)).unwrap().is_null());
}

fn log_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let off = vm.to_php_string(args.get(0))?;
    echo(vm, &format!("offsetExists({off})\n"))?;
    let this = engine::this(args);
    parent_call(
        vm,
        &this,
        "SplFixedArray",
        "offsetExists",
        args.values().to_vec(),
    )
}
fn log_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let off = vm.to_php_string(args.get(0))?;
    echo(vm, &format!("offsetGet({off})\n"))?;
    let this = engine::this(args);
    parent_call(
        vm,
        &this,
        "SplFixedArray",
        "offsetGet",
        args.values().to_vec(),
    )
}
fn log_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let off = vm.to_php_string(args.get(0))?;
    echo(vm, &format!("offsetSet({off})\n"))?;
    let this = engine::this(args);
    parent_call(
        vm,
        &this,
        "SplFixedArray",
        "offsetSet",
        args.values().to_vec(),
    )
}
static LOG_EXISTS: NativeFunction = NativeFunction {
    sig: "public function offsetExists($name): bool {}",
    handler: log_exists,
};
static LOG_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($name): mixed {}",
    handler: log_get,
};
static LOG_SET: NativeFunction = NativeFunction {
    sig: "public function offsetSet($name, $value): void {}",
    handler: log_set,
};

#[test]
fn isset_chains_call_offset_exists_then_offset_get() {
    // bug74478.phpt (the fetches the interpreter performs)
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyFixedArray",
        "SplFixedArray",
        &[&LOG_EXISTS, &LOG_GET, &LOG_SET],
    );
    let fixed = new_obj(&mut t.vm, "MyFixedArray", vec![i(10)]).unwrap();
    // isset($fixedData[0][1][2]): FETCH_DIM_IS on [0] yields null.
    assert!(read_is(&mut t, &fixed, i(0)).unwrap().is_null());
    assert_eq!(t.output(), "offsetExists(0)\n");
    let inner = new_obj(&mut t.vm, "MyFixedArray", vec![i(10)]).unwrap();
    write(&mut t, &fixed, i(0), Value::Object(inner)).unwrap();
    // $fixedData[0][1] = ...: a W fetch of [0], then a write of [1]
    let f = fixed.class().handlers.read_dimension.unwrap();
    let Value::Object(inner) = f(&mut t.vm, &fixed, Some(&i(0)), DimFetch::Write).unwrap() else {
        panic!()
    };
    let inner2 = new_obj(&mut t.vm, "MyFixedArray", vec![i(10)]).unwrap();
    write(&mut t, &inner, i(1), Value::Object(inner2)).unwrap();
    assert_eq!(t.output(), "offsetSet(0)\noffsetGet(0)\noffsetSet(1)\n");
    // isset($fixedData[0][1][2])
    let Value::Object(a) = read_is(&mut t, &fixed, i(0)).unwrap() else {
        panic!()
    };
    let Value::Object(b) = read_is(&mut t, &a, i(1)).unwrap() else {
        panic!()
    };
    assert!(!isset(&mut t, &b, i(2)).unwrap());
    assert_eq!(
        t.output(),
        "offsetExists(0)\noffsetGet(0)\noffsetExists(1)\noffsetGet(1)\noffsetExists(2)\n"
    );
}

fn prefix_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let v = parent_call(
        vm,
        &this,
        "SplFixedArray",
        "offsetGet",
        args.values().to_vec(),
    )?;
    let v = vm.to_php_string(&v)?;
    Ok(s(&format!("prefix_{v}")))
}
static PREFIX_GET: NativeFunction = NativeFunction {
    sig: "public function offsetGet($key): mixed {}",
    handler: prefix_get,
};

#[test]
fn overriding_only_offset_get() {
    // SplFixedArray_override_offsetGet_only.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "MyArray", "SplFixedArray", &[&PREFIX_GET]);
    let arr = new_obj(&mut t.vm, "MyArray", vec![i(1)]).unwrap();
    assert!(!isset(&mut t, &arr, i(0)).unwrap());
    write(&mut t, &arr, i(0), s("abc")).unwrap();
    assert!(isset(&mut t, &arr, i(0)).unwrap());
    assert!(same(&read(&mut t, &arr, i(0)).unwrap(), &s("prefix_abc")));
}

// ---------------------------------------------------------------------------
// count()
// ---------------------------------------------------------------------------

fn negative_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let n = parent_call(vm, &this, "SplFixedArray", "count", vec![])?;
    Ok(Value::Int(-n.as_int().unwrap()))
}
static NEGATIVE_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: negative_count,
};

#[test]
fn count_override() {
    // fixedarray_018.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 2);
    assert!(same(
        &t.call("count", vec![Value::Object(a)]).unwrap(),
        &i(2)
    ));
    user_class(
        &mut t.vm,
        "SplFixedArray2",
        "SplFixedArray",
        &[&NEGATIVE_COUNT],
    );
    let b = new_obj(&mut t.vm, "SplFixedArray2", vec![i(2)]).unwrap();
    assert!(same(
        &t.call("count", vec![Value::Object(b)]).unwrap(),
        &i(-2)
    ));
}

// ---------------------------------------------------------------------------
// Properties, serialization, JSON
// ---------------------------------------------------------------------------

#[test]
fn json_encode() {
    // splfixedarray_json_encode.phpt
    let mut t = TestVm::new();
    let enc = |t: &mut TestVm, o: &ObjRef| {
        let r = t
            .call("json_encode", vec![Value::Object(o.clone())])
            .unwrap();
        r.as_str().unwrap().to_string_lossy().into_owned()
    };
    let a = new_obj(&mut t.vm, "SplFixedArray", vec![]).unwrap();
    assert_eq!(enc(&mut t, &a), "[]");
    let a = fa(&mut t, 1);
    assert_eq!(enc(&mut t, &a), "[null]");
    let a = fa(&mut t, 3);
    write(&mut t, &a, i(0), i(0)).unwrap();
    write(&mut t, &a, i(2), i(2)).unwrap();
    assert_eq!(enc(&mut t, &a), "[0,null,2]");
}

fn ser(t: &mut TestVm, v: Value) -> String {
    let r = t.call("serialize", vec![v]).unwrap();
    r.as_str().unwrap().to_string_lossy().into_owned()
}

fn unser(t: &mut TestVm, s_: &str) -> Value {
    t.call("unserialize", vec![s(s_)]).unwrap()
}

#[test]
fn serialization() {
    // SplFixedArray_serialize.phpt
    let mut t = TestVm::new();
    let array = fa(&mut t, 5);
    let obj = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    obj.set_prop(b"prop", s("value"));
    write(&mut t, &array, i(0), s("foo")).unwrap();
    write(&mut t, &array, i(2), i(42)).unwrap();
    write(&mut t, &array, i(3), Value::Object(obj)).unwrap();
    write(&mut t, &array, i(4), list((1..=5).map(i).collect())).unwrap();
    array.set_prop(b"foo", s("bar"));
    let ser_ = ser(&mut t, Value::Object(array));
    assert_eq!(
        ser_,
        "O:13:\"SplFixedArray\":6:{i:0;s:3:\"foo\";i:1;N;i:2;i:42;i:3;O:8:\"stdClass\":1:{s:4:\"prop\";s:5:\"value\";}i:4;a:5:{i:0;i:1;i:1;i:2;i:2;i:3;i:3;i:4;i:4;i:5;}s:3:\"foo\";s:3:\"bar\";}"
    );
    let unser_ = unser(&mut t, &ser_);
    assert_eq!(
        t.output(),
        "\nDeprecated: Creation of dynamic property SplFixedArray::$foo is deprecated in /t.php on line 3\n"
    );
    let u = unser_.as_object().unwrap().clone();
    assert_eq!(count(&mut t, &u).unwrap(), 5);
    assert_eq!(size_of(&mut t, &u), 5);
    assert!(same(&read(&mut t, &u, i(0)).unwrap(), &s("foo")));
    assert!(read(&mut t, &u, i(1)).unwrap().is_null());
    assert!(same(&read(&mut t, &u, i(2)).unwrap(), &i(42)));
    let Value::Object(o3) = read(&mut t, &u, i(3)).unwrap() else {
        panic!()
    };
    assert!(same(&o3.get_prop(b"prop").unwrap(), &s("value")));
    write(&mut t, &u, i(4), s("quux")).unwrap();
    assert!(same(&read(&mut t, &u, i(4)).unwrap(), &s("quux")));
    assert!(same(&u.get_prop(b"foo").unwrap(), &s("bar")));

    // __unserialize is a no-op on a non-empty SplFixedArray
    let a = fa(&mut t, 1);
    let data = list(vec![list(vec![i(1)]), assoc(vec![(skey("foo"), s("bar"))])]);
    call(&mut t.vm, &a, "__unserialize", vec![data]).unwrap();
    assert!(same(&to_array(&mut t, &a), &list(vec![Value::Null])));
    assert!(a.get_prop(b"foo").is_none());

    let empty_ = new_obj(&mut t.vm, "SplFixedArray", vec![]).unwrap();
    let s0 = ser(&mut t, Value::Object(empty_));
    assert_eq!(s0, "O:13:\"SplFixedArray\":0:{}");
    let u = unser(&mut t, &s0);
    assert_eq!(size_of(&mut t, u.as_object().unwrap()), 0);
    assert_eq!(t.output(), "");
}

#[test]
fn unserialized_array_can_be_resized() {
    // gh13531.phpt, gh10907.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 5);
    write(&mut t, &a, i(4), i(1)).unwrap();
    let ser_ = ser(&mut t, Value::Object(a));
    let u = unser(&mut t, &ser_);
    let u = u.as_object().unwrap().clone();
    call(&mut t.vm, &u, "setSize", vec![i(6)]).unwrap();
    assert!(same(
        &to_array(&mut t, &u),
        &list(vec![
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            i(1),
            Value::Null
        ])
    ));

    let a = fa(&mut t, 2);
    write(&mut t, &a, i(0), s("test value 1")).unwrap();
    write(&mut t, &a, i(1), s("test value 2")).unwrap();
    let _ = props_for(&mut t, &a, PropPurpose::Debug);
    assert_eq!(
        ser(&mut t, Value::Object(a)),
        "O:13:\"SplFixedArray\":2:{i:0;s:12:\"test value 1\";i:1;s:12:\"test value 2\";}"
    );
}

#[test]
fn subclass_properties_are_serialized() {
    // gh10925.phpt, gh10907.phpt (members)
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "MySplFixedArray",
        "SplFixedArray",
        class_flags::ALLOW_DYNAMIC_PROPERTIES,
        vec![
            Prop::public("x", Value::Null),
            Prop {
                name: "y",
                flags: member_flags::PUBLIC,
                default: i(3),
                ty: Some("int"),
            },
        ],
        &[],
    );
    let x = new_obj(&mut t.vm, "MySplFixedArray", vec![i(2)]).unwrap();
    x.set_prop(b"y", i(2));
    let s1 = ser(&mut t, Value::Object(x.clone()));
    assert_eq!(
        s1,
        "O:15:\"MySplFixedArray\":4:{i:0;N;i:1;N;s:1:\"x\";N;s:1:\"y\";i:2;}"
    );
    let u = unser(&mut t, &s1);
    let u = u.as_object().unwrap().clone();
    let dump = props_for(&mut t, &u, PropPurpose::Debug);
    assert_eq!(
        entries(&dump)
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
        ["0", "1", "x", "y"]
    );
    assert!(same(&u.get_prop(b"y").unwrap(), &i(2)));

    x.set_prop(b"dynamic_property", s("dynamic_property_value"));
    let s2 = ser(&mut t, Value::Object(x.clone()));
    assert_eq!(
        s2,
        "O:15:\"MySplFixedArray\":5:{i:0;N;i:1;N;s:1:\"x\";N;s:1:\"y\";i:2;s:16:\"dynamic_property\";s:22:\"dynamic_property_value\";}"
    );
    let u = unser(&mut t, &s2);
    let u = u.as_object().unwrap().clone();
    assert!(same(
        &u.get_prop(b"dynamic_property").unwrap(),
        &s("dynamic_property_value")
    ));
    assert_eq!(t.output(), "");

    // A typed property gets strict verification.
    let fresh = new_obj(&mut t.vm, "MySplFixedArray", vec![]).unwrap();
    let e = call(
        &mut t.vm,
        &fresh,
        "__unserialize",
        vec![assoc(vec![(skey("y"), s("nope"))])],
    );
    assert_eq!(
        thrown(e),
        (
            "TypeError".into(),
            "Cannot assign string to property MySplFixedArray::$y of type int".into()
        )
    );
}

#[test]
fn properties_for_each_purpose() {
    // SplFixedArray_get_properties_for.phpt, gh17198.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "MySplFixedArray",
        "SplFixedArray",
        class_flags::ALLOW_DYNAMIC_PROPERTIES,
        vec![
            Prop::public("x", Value::Null),
            Prop {
                name: "y",
                flags: member_flags::PUBLIC,
                default: Value::Undef,
                ty: Some("int"),
            },
        ],
        &[],
    );
    let array = new_obj(&mut t.vm, "MySplFixedArray", vec![i(2)]).unwrap();
    let vars = props_for(&mut t, &array, PropPurpose::GetObjectVars);
    assert_eq!(
        entries(&vars)
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
        ["x"]
    );

    let std = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    let y = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    let inner = new_obj(&mut t.vm, "SplFixedArray", vec![]).unwrap();
    let x = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    write(&mut t, &array, i(0), Value::Object(std)).unwrap();
    write(&mut t, &array, i(1), Value::Object(y)).unwrap();
    array.set_prop(b"x", Value::Object(inner));
    array.set_prop(b"0", Value::Object(x.clone()));

    let dump = props_for(&mut t, &array, PropPurpose::Debug);
    let keys: Vec<ArrayKey> = dump.as_array().unwrap().keys().collect();
    assert_eq!(
        keys,
        [ArrayKey::Int(0), ArrayKey::Int(1), skey("x"), skey("0")]
    );
    let vars = props_for(&mut t, &array, PropPurpose::GetObjectVars);
    let keys: Vec<ArrayKey> = vars.as_array().unwrap().keys().collect();
    assert_eq!(keys, [skey("x"), skey("0")]);

    // (array): elements, then properties ("0" overwrites element 0).
    let cast = t.vm.cast_to_array(&Value::Object(array.clone())).unwrap();
    let keys: Vec<ArrayKey> = cast.keys().collect();
    assert_eq!(keys, [ArrayKey::Int(0), ArrayKey::Int(1), skey("x")]);
    assert!(matches!(cast.get_int(0), Some(Value::Object(o)) if ObjRef::ptr_eq(o, &x)));

    let enc = t
        .call("json_encode", vec![Value::Object(array.clone())])
        .unwrap();
    assert!(same(&enc, &s("[{},{}]")));

    let ser_ = ser(&mut t, Value::Object(array.clone()));
    assert_eq!(
        ser_,
        "O:15:\"MySplFixedArray\":4:{i:0;O:8:\"stdClass\":0:{}i:1;O:8:\"stdClass\":0:{}s:1:\"x\";O:13:\"SplFixedArray\":0:{}s:1:\"0\";O:8:\"stdClass\":0:{}}"
    );
    // unserialize() turns the "0" key into an element.
    let u = unser(&mut t, &ser_);
    let u = u.as_object().unwrap().clone();
    assert_eq!(size_of(&mut t, &u), 2);
    let dump = props_for(&mut t, &u, PropPurpose::Debug);
    let keys: Vec<ArrayKey> = dump.as_array().unwrap().keys().collect();
    assert_eq!(keys, [ArrayKey::Int(0), ArrayKey::Int(1), skey("x")]);
}

#[test]
fn unserialize_dereferences_elements() {
    // gh20614.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 0);
    let r = php_runtime::PhpRef::new(i(1));
    let mut data = PhpArray::new();
    let _ = data.append(Value::Ref(r.clone()));
    call(&mut t.vm, &a, "__unserialize", vec![Value::array(data)]).unwrap();
    assert!(same(&to_array(&mut t, &a), &list(vec![i(1)])));
    unset(&mut t, &a, i(0)).unwrap();
    assert!(same(&to_array(&mut t, &a), &list(vec![Value::Null])));
    assert!(same(&r.get(), &i(1)));
}

#[test]
fn wakeup_is_deprecated_and_loads_properties() {
    let mut t = TestVm::new();
    let a = new_obj(&mut t.vm, "SplFixedArray", vec![]).unwrap();
    a.set_prop(b"0", s("a"));
    a.set_prop(b"1", s("b"));
    call(&mut t.vm, &a, "__wakeup", vec![]).unwrap();
    assert_eq!(
        t.output(),
        "\nDeprecated: Method SplFixedArray::__wakeup() is deprecated since 8.4, this method is obsolete, as serialization hooks are provided by __unserialize() and __serialize() in /t.php on line 3\n"
    );
    assert!(same(&to_array(&mut t, &a), &list(vec![s("a"), s("b")])));
    assert!(a.get_prop(b"0").is_none());
}

#[test]
fn clone_and_cast_of_non_empty_array() {
    // fixedarray_024.phpt
    let mut t = TestVm::new();
    let x = fa(&mut t, 1);
    let z = t.vm.cast_to_array(&Value::Object(x.clone())).unwrap();
    assert_eq!(z.len(), 1);
    let clone = x.class().handlers.clone_obj.unwrap();
    let y = clone(&mut t.vm, &x).unwrap();
    assert_eq!(size_of(&mut t, &y), 1);
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[test]
fn get_iterator_returns_internal_iterator() {
    // SplFixedArray_override_getIterator.phpt (first part), SplFixedArray_nested_foreach.phpt
    let mut t = TestVm::new();
    let a = fa(&mut t, 3);
    for (n, v) in ["a", "b", "c"].iter().enumerate() {
        write(&mut t, &a, i(n as i64), s(v)).unwrap();
    }
    let it = get_iterator(&mut t, &a);
    assert_eq!(it.class().name.to_string(), "InternalIterator");
    let mut out = Vec::new();
    walk(&mut t, &it, |vm, k, v| {
        let k = vm.to_php_string(&k).unwrap();
        let v = vm.to_php_string(&v).unwrap();
        out.push(format!("{k} => {v}"));
    });
    assert_eq!(out, ["0 => a", "1 => b", "2 => c"]);

    let arr = from_array(&mut t, list(vec![i(0), i(1)]), None).unwrap();
    let arr = arr.as_object().unwrap().clone();
    let outer = get_iterator(&mut t, &arr);
    let mut pairs = Vec::new();
    let mut o_iter = ObjIterator::over(outer);
    o_iter.rewind(&mut t.vm).unwrap();
    while o_iter.valid(&mut t.vm).unwrap() {
        let v1 = o_iter.current(&mut t.vm).unwrap();
        let inner = get_iterator(&mut t, &arr);
        walk(&mut t, &inner, |_, _, v2| {
            pairs.push((v1.as_int().unwrap(), v2.as_int().unwrap()))
        });
        o_iter.next(&mut t.vm).unwrap();
    }
    assert_eq!(pairs, [(0, 0), (0, 1), (1, 0), (1, 1)]);
}

#[test]
fn changing_size_during_iteration() {
    // SplFixedArray_change_size_during_iteration.phpt
    for (shrink_at, want) in [
        (0, vec!["0 => a", "1 => b"]),
        (1, vec!["0 => a", "1 => b"]),
        (2, vec!["0 => a", "1 => b", "2 => c"]),
    ] {
        let mut t = TestVm::new();
        let arr = from_array(&mut t, list(vec![s("a"), s("b"), s("c")]), None).unwrap();
        let arr = arr.as_object().unwrap().clone();
        let it = get_iterator(&mut t, &arr);
        let mut out = Vec::new();
        walk(&mut t, &it, |vm, k, v| {
            let k = k.as_int().unwrap();
            out.push(format!("{k} => {}", v.as_str().unwrap()));
            if k == shrink_at {
                call(vm, &arr, "setSize", vec![i(2)]).unwrap();
            }
        });
        assert_eq!(out, want);
    }
}

#[test]
fn internal_iterator_methods() {
    let mut t = TestVm::new();
    let a = from_array(&mut t, list(vec![s("x"), s("y")]), None).unwrap();
    let a = a.as_object().unwrap().clone();
    let it = get_iterator(&mut t, &a);
    // The first call rewinds implicitly.
    assert!(same(&call(&mut t.vm, &it, "key", vec![]).unwrap(), &i(0)));
    assert!(same(
        &call(&mut t.vm, &it, "current", vec![]).unwrap(),
        &s("x")
    ));
    call(&mut t.vm, &it, "next", vec![]).unwrap();
    assert!(same(
        &call(&mut t.vm, &it, "current", vec![]).unwrap(),
        &s("y")
    ));
    call(&mut t.vm, &it, "next", vec![]).unwrap();
    assert!(same(
        &call(&mut t.vm, &it, "valid", vec![]).unwrap(),
        &Value::Bool(false)
    ));
    // get_current_data past the end throws (spl_fixedarray_it_get_current_data).
    let e = call(&mut t.vm, &it, "current", vec![]);
    assert_eq!(
        thrown(e),
        (
            "OutOfBoundsException".into(),
            "Index invalid or out of range".into()
        )
    );
    call(&mut t.vm, &it, "rewind", vec![]).unwrap();
    assert!(same(
        &call(&mut t.vm, &it, "current", vec![]).unwrap(),
        &s("x")
    ));

    // Not constructible, not cloneable, not initialized when created raw.
    let ce = t.vm.find_class(b"InternalIterator").unwrap();
    let raw = t.vm.instantiate(&ce).unwrap();
    assert_eq!(
        thrown(call(&mut t.vm, &raw, "valid", vec![])),
        (
            "Error".into(),
            "The InternalIterator object has not been properly initialized".into()
        )
    );
    assert_eq!(
        thrown(call(&mut t.vm, &raw, "__construct", vec![])),
        (
            "Error".into(),
            "Cannot manually construct InternalIterator".into()
        )
    );
    let clone = ce.handlers.clone_obj.unwrap();
    assert_eq!(
        thrown(clone(&mut t.vm, &it).map(Value::Object)),
        (
            "Error".into(),
            "Trying to clone an uncloneable object of class InternalIterator".into()
        )
    );
}

// ---------------------------------------------------------------------------
// Destructors during modification
// ---------------------------------------------------------------------------

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[test]
fn destructor_during_offset_set() {
    // SplFixedArray_setSize_destruct.phpt
    let mut t = TestVm::new();
    let values = fa(&mut t, 1);
    let w = weak(&values);
    let dtor = move |vm: &mut Vm| -> PResult<()> {
        let values = w.upgrade().unwrap();
        let before = super::elements(&values).len();
        let n = super::size(&values);
        call(vm, &values, "setSize", vec![i(n - 1)])?;
        LOG.with(|l| {
            l.borrow_mut()
                .push(format!("{before} -> {}", super::size(&values)))
        });
        Ok(())
    };
    let p = probe(&mut t.vm, dtor.clone());
    call(
        &mut t.vm,
        &values,
        "offsetSet",
        vec![i(0), Value::Object(p)],
    )
    .unwrap();
    call(
        &mut t.vm,
        &values,
        "offsetSet",
        vec![i(0), Value::Bool(false)],
    )
    .unwrap();
    assert_eq!(LOG.with(|l| l.take()), ["1 -> 0"]);

    call(&mut t.vm, &values, "setSize", vec![i(5)]).unwrap();
    let p = probe(&mut t.vm, dtor);
    call(
        &mut t.vm,
        &values,
        "offsetSet",
        vec![i(4), Value::Object(p)],
    )
    .unwrap();
    assert_eq!(size_of(&mut t, &values), 5);
    // (At shutdown) the probe's destructor shrinks the array to 4.
    unset(&mut t, &values, i(4)).unwrap();
    assert_eq!(LOG.with(|l| l.take()), ["5 -> 4"]);
    assert_eq!(size_of(&mut t, &values), 4);
}

#[test]
fn destructor_resizing_during_shrink() {
    // bug81992b.phpt
    for (desired, want_size, logs) in [
        (
            1,
            1,
            vec!["In destructor", "still 2", "logger 4", "logger 1"],
        ),
        (2, 2, vec!["In destructor", "still 2", "logger 4"]),
        (10, 10, vec!["In destructor", "still 2", "logger 4"]),
    ] {
        drain_pending_destructors();
        LOG.with(|l| l.take());
        let mut t = TestVm::new();
        let obj = fa(&mut t, 5);
        let w = weak(&obj);
        write(&mut t, &obj, i(0), s(&"A".repeat(10))).unwrap();
        let l1 = probe(&mut t.vm, |_| {
            LOG.with(|l| l.borrow_mut().push("logger 1".into()));
            Ok(())
        });
        write(&mut t, &obj, i(1), Value::Object(l1)).unwrap();
        write(&mut t, &obj, i(2), s(&"B".repeat(10))).unwrap();
        let invalid = probe(&mut t.vm, move |vm| {
            LOG.with(|l| l.borrow_mut().push("In destructor".into()));
            let obj = w.upgrade().unwrap();
            call(vm, &obj, "setSize", vec![i(desired)])?;
            let n = super::size(&obj);
            LOG.with(|l| l.borrow_mut().push(format!("still {n}")));
            Ok(())
        });
        write(&mut t, &obj, i(3), Value::Object(invalid)).unwrap();
        let l4 = probe(&mut t.vm, |_| {
            LOG.with(|l| l.borrow_mut().push("logger 4".into()));
            Ok(())
        });
        write(&mut t, &obj, i(4), Value::Object(l4)).unwrap();
        call(&mut t.vm, &obj, "setSize", vec![i(2)]).unwrap();
        assert_eq!(size_of(&mut t, &obj), want_size);
        assert_eq!(LOG.with(|l| l.take()), logs, "desired size {desired}");
    }
}

#[test]
fn destructor_reading_during_shrink() {
    // bug81992.phpt
    let mut t = TestVm::new();
    let obj = fa(&mut t, 5);
    let w = weak(&obj);
    write(&mut t, &obj, i(0), s(&"A".repeat(10))).unwrap();
    write(&mut t, &obj, i(2), s(&"B".repeat(10))).unwrap();
    let p = probe(&mut t.vm, move |vm| {
        let obj = w.upgrade().unwrap();
        let mut out = Vec::new();
        for n in [0, 2, 4] {
            let f = obj.class().handlers.read_dimension.unwrap();
            match f(vm, &obj, Some(&i(n)), DimFetch::Read) {
                Ok(v) => out.push(vm.to_php_string(&v)?.to_string_lossy().into_owned()),
                Err(php_runtime::Flow::Exception(e)) => {
                    out.push(format!("{}: {}", e.class().name, Vm::exception_message(&e)))
                }
                Err(e) => return Err(e),
            }
        }
        LOG.with(|l| *l.borrow_mut() = out);
        Ok(())
    });
    write(&mut t, &obj, i(3), Value::Object(p)).unwrap();
    write(&mut t, &obj, i(4), s(&"C".repeat(10))).unwrap();
    call(&mut t.vm, &obj, "setSize", vec![i(2)]).unwrap();
    assert_eq!(
        LOG.with(|l| l.take()),
        [
            "AAAAAAAAAA",
            "OutOfBoundsException: Index invalid or out of range",
            "OutOfBoundsException: Index invalid or out of range"
        ]
    );
}

#[test]
fn recursive_clear_from_destructor() {
    // bug80663.phpt, gh16478.phpt
    let mut t = TestVm::new();
    let obj = fa(&mut t, 1000);
    let w = weak(&obj);
    let p = probe(&mut t.vm, move |vm| {
        let obj = w.upgrade().unwrap();
        call(vm, &obj, "setSize", vec![i(0)])?;
        Ok(())
    });
    write(&mut t, &obj, i(0), Value::Object(p)).unwrap();
    call(&mut t.vm, &obj, "setSize", vec![i(0)]).unwrap();
    assert_eq!(size_of(&mut t, &obj), 0);

    let arr = fa(&mut t, 2);
    let w = weak(&arr);
    let p = probe(&mut t.vm, move |vm| {
        let arr = w.upgrade().unwrap();
        call(vm, &arr, "setSize", vec![i(0)])?;
        Ok(())
    });
    write(&mut t, &arr, i(0), Value::Object(p)).unwrap();
    unset(&mut t, &arr, i(0)).unwrap();
    assert_eq!(size_of(&mut t, &arr), 0);
}

#[test]
fn reentrant_resize_during_clear() {
    // SplFixedArray_setSize_destruct_grow_during_clear.phpt
    let mut t = TestVm::new();
    let arr = fa(&mut t, 2);
    let w = weak(&arr);
    let r = probe(&mut t.vm, move |vm| {
        let arr = w.upgrade().unwrap();
        call(vm, &arr, "setSize", vec![i(5)])?;
        Ok(())
    });
    write(&mut t, &arr, i(0), Value::Object(r)).unwrap();
    write(&mut t, &arr, i(1), s("tail")).unwrap();
    call(&mut t.vm, &arr, "setSize", vec![i(0)]).unwrap();
    assert_eq!(size_of(&mut t, &arr), 5);
    write(&mut t, &arr, i(0), s("ok")).unwrap();
    assert!(same(&read(&mut t, &arr, i(0)).unwrap(), &s("ok")));
}

#[test]
fn reinitialisation_during_clear_is_refused() {
    // SplFixedArray_setSize_destruct_reinit_during_clear.phpt
    type Action = fn(&mut Vm, &ObjRef) -> PResult<()>;
    let cases: [(Action, i64); 4] = [
        (|vm, a| call(vm, a, "__construct", vec![i(5)]).map(drop), 0),
        (
            |vm, a| {
                call(vm, a, "__construct", vec![i(7)])?;
                call(vm, a, "setSize", vec![i(3)]).map(drop)
            },
            3,
        ),
        (
            |vm, a| {
                call(
                    vm,
                    a,
                    "__unserialize",
                    vec![list(vec![s("a"), s("b"), s("c")])],
                )
                .map(drop)
            },
            0,
        ),
        (
            |vm, a| {
                let tok = vm.begin_silence();
                let r = call(vm, a, "__wakeup", vec![]);
                vm.end_silence(tok);
                r.map(drop)
            },
            0,
        ),
    ];
    for (action, want) in cases {
        drain_pending_destructors();
        let mut t = TestVm::new();
        let arr = fa(&mut t, 2);
        let w = weak(&arr);
        let r = probe(&mut t.vm, move |vm| {
            let arr = w.upgrade().unwrap();
            action(vm, &arr)
        });
        write(&mut t, &arr, i(0), Value::Object(r)).unwrap();
        write(&mut t, &arr, i(1), s("tail")).unwrap();
        call(&mut t.vm, &arr, "setSize", vec![i(0)]).unwrap();
        assert_eq!(size_of(&mut t, &arr), want);
        call(&mut t.vm, &arr, "setSize", vec![i(1)]).unwrap();
        write(&mut t, &arr, i(0), s("ok")).unwrap();
        assert!(same(&read(&mut t, &arr, i(0)).unwrap(), &s("ok")));
        assert_eq!(t.output(), "");
    }
}

#[test]
fn replaced_element_is_destroyed_immediately() {
    // SplFixedArray_immediate_gc.phpt
    let mut t = TestVm::new();
    let p = probe(&mut t.vm, |vm| echo(vm, "In destructor\n"));
    let array = from_array(&mut t, list(vec![Value::Object(p)]), None).unwrap();
    let array = array.as_object().unwrap().clone();
    echo(&mut t.vm, "Replacing\n").unwrap();
    let std = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    write(&mut t, &array, i(0), Value::Object(std)).unwrap();
    echo(&mut t.vm, "Dumping again\n").unwrap();
    assert_eq!(t.output(), "Replacing\nIn destructor\nDumping again\n");
    let vars = props_for(&mut t, &array, PropPurpose::GetObjectVars);
    assert!(same(&vars, &Value::empty_array()));
}

#[test]
fn destructor_exceptions_are_chained() {
    let mut t = TestVm::new();
    let arr = fa(&mut t, 2);
    for n in 0..2 {
        let p = probe(&mut t.vm, move |vm| {
            Err(vm.throw_error("Exception", format!("dtor {n}")))
        });
        write(&mut t, &arr, i(n), Value::Object(p)).unwrap();
    }
    // setSize(0) destroys the elements last to first.
    let e = call(&mut t.vm, &arr, "setSize", vec![i(0)]);
    let Err(php_runtime::Flow::Exception(e)) = e else {
        panic!()
    };
    assert_eq!(Vm::exception_message(&e), "dtor 0");
    assert_eq!(
        super::test_helpers::previous(&e),
        Some(("Exception".into(), "dtor 1".into()))
    );
    assert_eq!(size_of(&mut t, &arr), 0);
}
