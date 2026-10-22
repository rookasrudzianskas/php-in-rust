//! `SplObjectStorage`: registration, attach/detach, ArrayAccess and the
//! dimension handlers, iteration, `getHash()` overrides, compare, clone.

use php_runtime::class::{DimFetch, fn_flags, member_flags};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpStr, Value, Vm};

use super::super::super::engine::{self, ObjIterator, this};
use super::super::super::functions::object_hash;
use super::super::super::testutil::{array_of, call, entries, list, new_obj, s, user_class};
use super::super::reset_get_hash_depth;
use super::{
    assert_expectf, dim_empty, dim_isset, dim_read, dim_unset, dim_write, dump, int_of, log, obj,
    parent_call, state, std_obj, storage, take_log,
};
use crate::testing::{TestVm, thrown};

fn names(v: &Value) -> Vec<String> {
    entries(v).into_iter().map(|(k, _)| k).collect()
}

const CONTAINS_DEPRECATION: &str = "\nDeprecated: Method SplObjectStorage::contains() is deprecated since 8.5, use method SplObjectStorage::offsetExists() instead in /t.php on line 3\n";

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn interfaces_and_class_shape() {
    let mut t = TestVm::new();
    let r = t
        .call("class_implements", vec![s("SplObjectStorage")])
        .unwrap();
    assert_eq!(
        names(&r),
        [
            "Countable",
            "SeekableIterator",
            "Traversable",
            "Iterator",
            "Serializable",
            "ArrayAccess"
        ]
    );
    let r = t
        .call("class_implements", vec![s("MultipleIterator")])
        .unwrap();
    assert_eq!(names(&r), ["Iterator", "Traversable"]);

    let observer = t.vm.find_class(b"SplObserver").unwrap();
    assert!(observer.is_interface());
    let update = observer.find_method(b"update").unwrap();
    assert!(update.has_flag(member_flags::ABSTRACT));
    assert_eq!(update.signature().params[0].type_string(), "SplSubject");
    let subject = t.vm.find_class(b"SplSubject").unwrap();
    let methods: Vec<String> = subject
        .methods
        .values()
        .map(|m| m.name.to_string())
        .collect();
    assert_eq!(methods, ["attach", "detach", "notify"]);

    let ce = t.vm.find_class(b"SplObjectStorage").unwrap();
    let methods: Vec<String> = ce.methods.values().map(|m| m.name.to_string()).collect();
    assert_eq!(methods[..3], ["attach", "detach", "contains"]);
    assert_eq!(methods.len(), 25);
    assert_eq!(methods[24], "__debugInfo");
    for m in ["attach", "detach", "contains"] {
        assert!(
            ce.find_method(m.as_bytes())
                .unwrap()
                .has_flag(fn_flags::DEPRECATED),
            "{m}"
        );
    }
    for m in ["offsetSet", "offsetUnset", "offsetExists", "addAll"] {
        assert!(
            !ce.find_method(m.as_bytes())
                .unwrap()
                .has_flag(fn_flags::DEPRECATED),
            "{m}"
        );
    }
    assert!(ce.magic.debug_info.is_some());
    assert!(ce.magic.serialize.is_some() && ce.magic.unserialize.is_some());
}

// ---------------------------------------------------------------------------
// attach / detach / contains / ArrayAccess
// ---------------------------------------------------------------------------

#[test]
fn deprecated_methods_still_work() {
    let mut t = TestVm::new();
    let s1 = storage(&mut t);
    let o = std_obj(&t.vm);
    call(&mut t.vm, &s1, "attach", vec![obj(&o), s("info")]).unwrap();
    let r = call(&mut t.vm, &s1, "contains", vec![obj(&o)]).unwrap();
    assert!(r.identical(&Value::Bool(true)));
    call(&mut t.vm, &s1, "detach", vec![obj(&o)]).unwrap();
    let r = call(&mut t.vm, &s1, "contains", vec![obj(&o)]).unwrap();
    assert!(r.identical(&Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nDeprecated: Method SplObjectStorage::attach() is deprecated since 8.5, use method SplObjectStorage::offsetSet() instead in /t.php on line 3\n"
            .to_string()
            + CONTAINS_DEPRECATION
            + "\nDeprecated: Method SplObjectStorage::detach() is deprecated since 8.5, use method SplObjectStorage::offsetUnset() instead in /t.php on line 3\n"
            + CONTAINS_DEPRECATION
    );
    // The ArrayAccess aliases are not deprecated.
    call(&mut t.vm, &s1, "offsetSet", vec![obj(&o)]).unwrap();
    let r = call(&mut t.vm, &s1, "offsetExists", vec![obj(&o)]).unwrap();
    assert!(r.identical(&Value::Bool(true)));
    call(&mut t.vm, &s1, "offsetUnset", vec![obj(&o)]).unwrap();
    assert_eq!(int_of(call(&mut t.vm, &s1, "count", vec![])), 0);
    assert_eq!(t.output(), "");
}

#[test]
fn coalesce_isset_empty_and_debug_info() {
    // SplObjectStorage_coalesce.phpt
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let o1 = std_obj(&t.vm);
    let o2 = std_obj(&t.vm);
    let vm = &mut t.vm;
    // $s[$o1] ?? 'default'
    let v = dim_read(vm, &st, &obj(&o1), DimFetch::IsSet).unwrap();
    assert!(v.is_null());
    // $s[$o1] ??= $i
    dim_write(vm, &st, &obj(&o1), s("dynamic")).unwrap();
    let v = dim_read(vm, &st, &obj(&o1), DimFetch::IsSet).unwrap();
    assert!(v.identical(&s("dynamic")));
    assert!(dim_isset(vm, &st, &obj(&o1)).unwrap());
    assert!(!dim_empty(vm, &st, &obj(&o1)).unwrap());
    // o2
    assert!(!dim_isset(vm, &st, &obj(&o2)).unwrap());
    assert!(dim_empty(vm, &st, &obj(&o2)).unwrap());
    dim_write(vm, &st, &obj(&o2), Value::Null).unwrap();
    assert!(
        dim_read(vm, &st, &obj(&o2), DimFetch::IsSet)
            .unwrap()
            .is_null()
    );
    // isset/empty/contains/offsetExists for null: the entry is there.
    assert!(dim_isset(vm, &st, &obj(&o2)).unwrap());
    assert!(dim_empty(vm, &st, &obj(&o2)).unwrap());
    let r = call(vm, &st, "contains", vec![obj(&o2)]).unwrap();
    assert!(r.identical(&Value::Bool(true)));
    let r = call(vm, &st, "offsetExists", vec![obj(&o2)]).unwrap();
    assert!(r.identical(&Value::Bool(true)));
    // ... and for false.
    dim_write(vm, &st, &obj(&o2), Value::Bool(false)).unwrap();
    assert!(dim_isset(vm, &st, &obj(&o2)).unwrap());
    assert!(dim_empty(vm, &st, &obj(&o2)).unwrap());
    assert_eq!(t.output(), CONTAINS_DEPRECATION);

    let vm = &mut t.vm;
    // Non-object offsets go through offsetSet()/offsetExists().
    let (class, msg) =
        thrown(dim_write(vm, &st, &s("invalid"), Value::Int(123)).map(|_| Value::Null));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplObjectStorage::offsetSet(): Argument #1 ($object) must be of type object, string given"
    );
    let (class, msg) = thrown(dim_isset(vm, &st, &s("invalid")).map(Value::Bool));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplObjectStorage::offsetExists(): Argument #1 ($object) must be of type object, string given"
    );
    // $a = &$s[$o1]: a plain (non-reference) value comes back.
    let v = dim_read(vm, &st, &obj(&o1), DimFetch::Write).unwrap();
    assert!(!v.is_ref() && v.identical(&s("dynamic")));

    let out = dump(vm, &obj(&st));
    assert_eq!(
        out,
        format!(
            "object(SplObjectStorage)#{} (1) {{\n  [\"storage\":\"SplObjectStorage\":private]=>\n  array(2) {{\n    [0]=>\n    array(2) {{\n      [\"obj\"]=>\n      object(stdClass)#{} (0) {{\n      }}\n      [\"inf\"]=>\n      string(7) \"dynamic\"\n    }}\n    [1]=>\n    array(2) {{\n      [\"obj\"]=>\n      object(stdClass)#{} (0) {{\n      }}\n      [\"inf\"]=>\n      bool(false)\n    }}\n  }}\n}}\n",
            st.id(),
            o1.id(),
            o2.id()
        )
    );
}

#[test]
fn missing_objects_and_empty_storage() {
    // SplObjectStorage_offsetGet_missing_object.phpt,
    // SplObjectStorage_current_empty_storage.phpt,
    // SplObjectStorage_getInfo_empty_storage.phpt,
    // SplObjectStorage_setInfo_empty_storage.phpt
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let o1 = std_obj(&t.vm);
    let vm = &mut t.vm;
    let (class, msg) = thrown(call(vm, &st, "offsetGet", vec![obj(&o1)]));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("UnexpectedValueException", "Object not found")
    );
    let (class, msg) = thrown(dim_read(vm, &st, &obj(&o1), DimFetch::Read));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("UnexpectedValueException", "Object not found")
    );
    assert!(
        call(vm, &st, "valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
    let (class, msg) = thrown(call(vm, &st, "current", vec![]));
    assert_eq!(class, "RuntimeException");
    assert_eq!(msg, "Called current() on invalid iterator");
    assert!(call(vm, &st, "getInfo", vec![]).unwrap().is_null());
    assert!(
        call(vm, &st, "setInfo", vec![s("some_value")])
            .unwrap()
            .is_null()
    );
    // offsetGet.phpt
    dim_write(vm, &st, &obj(&o1), s("some_value")).unwrap();
    let v = call(vm, &st, "offsetGet", vec![obj(&o1)]).unwrap();
    assert!(v.identical(&s("some_value")));
    let (class, msg) = thrown(call(vm, &st, "offsetGet", vec![Value::Int(1)]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplObjectStorage::offsetGet(): Argument #1 ($object) must be of type object, int given"
    );
    let (class, msg) = thrown(call(vm, &st, "offsetUnset", vec![Value::Null]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplObjectStorage::offsetUnset(): Argument #1 ($object) must be of type object, null given"
    );
}

#[test]
fn info_follows_the_iterator() {
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    let vm = &mut t.vm;
    call(vm, &st, "offsetSet", vec![obj(&a), Value::Int(1)]).unwrap();
    call(vm, &st, "offsetSet", vec![obj(&b), Value::Int(2)]).unwrap();
    call(vm, &st, "rewind", vec![]).unwrap();
    assert!(
        call(vm, &st, "getInfo", vec![])
            .unwrap()
            .identical(&Value::Int(1))
    );
    call(vm, &st, "next", vec![]).unwrap();
    call(vm, &st, "setInfo", vec![s("two")]).unwrap();
    assert_eq!(int_of(call(vm, &st, "key", vec![])), 1);
    let v = call(vm, &st, "offsetGet", vec![obj(&b)]).unwrap();
    assert!(v.identical(&s("two")));
    // Re-attaching keeps the position and replaces the info.
    call(vm, &st, "offsetSet", vec![obj(&a), s("one")]).unwrap();
    call(vm, &st, "rewind", vec![]).unwrap();
    let cur = call(vm, &st, "current", vec![]).unwrap();
    assert!(cur.identical(&obj(&a)));
    assert!(
        call(vm, &st, "getInfo", vec![])
            .unwrap()
            .identical(&s("one"))
    );
    call(vm, &st, "next", vec![]).unwrap();
    call(vm, &st, "next", vec![]).unwrap();
    assert!(
        call(vm, &st, "valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
    assert_eq!(int_of(call(vm, &st, "key", vec![])), 2);
    assert!(call(vm, &st, "setInfo", vec![s("x")]).unwrap().is_null());
}

#[test]
fn unset_removes_the_entry() {
    // SplObjectStorage_unset.phpt (destructors run at statement boundaries)
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let o = std_obj(&t.vm);
    let inf = std_obj(&t.vm);
    let vm = &mut t.vm;
    dim_write(vm, &st, &obj(&o), obj(&inf)).unwrap();
    dim_unset(vm, &st, &obj(&o)).unwrap();
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 0);
    // The storage released its reference to the info.
    assert_eq!(inf.refcount(), 1);
    dim_write(vm, &st, &obj(&o), obj(&inf)).unwrap();
    call(vm, &st, "offsetUnset", vec![obj(&o)]).unwrap();
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 0);
    let (class, msg) = thrown(dim_unset(vm, &st, &s("x")).map(|_| Value::Null));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplObjectStorage::offsetUnset(): Argument #1 ($object) must be of type object, string given"
    );
}

#[test]
fn many_objects_are_identified_correctly() {
    // bug61453.phpt
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let mut keep = Vec::new();
    for _ in 0..1000 {
        let o = std_obj(&t.vm);
        assert!(!dim_isset(&mut t.vm, &st, &obj(&o)).unwrap());
        dim_write(&mut t.vm, &st, &obj(&o), Value::Int(1)).unwrap();
        keep.push(o);
    }
    assert_eq!(int_of(call(&mut t.vm, &st, "count", vec![])), 1000);
}

#[test]
fn count_modes_and_handler() {
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    let vm = &mut t.vm;
    dim_write(vm, &st, &obj(&a), Value::empty_array()).unwrap();
    dim_write(vm, &st, &obj(&b), Value::Int(1)).unwrap();
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 2);
    // The elements themselves are never arrays: COUNT_RECURSIVE is the same.
    assert_eq!(int_of(call(vm, &st, "count", vec![Value::Int(1)])), 2);
    let h = st.class().handlers.count_elements.unwrap();
    assert_eq!(h(vm, &st).unwrap(), Some(2));
    assert!(
        vm.find_class(b"MultipleIterator")
            .unwrap()
            .handlers
            .count_elements
            .is_none()
    );
}

static COUNT_OVERRIDE: NativeFunction = NativeFunction {
    sig: "public function count($mode = COUNT_NORMAL): int {}",
    handler: |_vm, _args| Ok(Value::str("42")),
};

#[test]
fn count_elements_calls_a_count_override() {
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "CountingStorage",
        "SplObjectStorage",
        &[&COUNT_OVERRIDE],
    );
    let st = new_obj(&mut t.vm, "CountingStorage", vec![]).unwrap();
    let h = st.class().handlers.count_elements.unwrap();
    assert_eq!(h(&mut t.vm, &st).unwrap(), None);
    let n = t.vm.call_native("count", vec![obj(&st)]);
    assert_eq!(int_of(n), 42);
    let plain = storage(&mut t);
    assert_eq!(
        int_of(t.vm.call_native("count", vec![obj(&plain), Value::Int(1)])),
        0
    );
}

// ---------------------------------------------------------------------------
// addAll / removeAll / removeAllExcept
// ---------------------------------------------------------------------------

#[test]
fn add_all_remove_all_and_remove_all_except() {
    // SplObjectStorage_removeAllExcept_basic.phpt, bug53144.phpt
    let mut t = TestVm::new();
    let (a, b, c) = (std_obj(&t.vm), std_obj(&t.vm), std_obj(&t.vm));
    let foo = storage(&mut t);
    let bar = storage(&mut t);
    let vm = &mut t.vm;
    call(vm, &foo, "offsetSet", vec![obj(&a)]).unwrap();
    call(vm, &foo, "offsetSet", vec![obj(&b)]).unwrap();
    call(vm, &bar, "offsetSet", vec![obj(&b)]).unwrap();
    call(vm, &bar, "offsetSet", vec![obj(&c), s("c")]).unwrap();
    assert_eq!(
        int_of(call(vm, &foo, "removeAllExcept", vec![obj(&bar)])),
        1
    );
    let r = call(vm, &foo, "offsetExists", vec![obj(&a)]).unwrap();
    assert!(r.identical(&Value::Bool(false)));
    let r = call(vm, &foo, "offsetExists", vec![obj(&b)]).unwrap();
    assert!(r.identical(&Value::Bool(true)));

    assert_eq!(int_of(call(vm, &foo, "addAll", vec![obj(&bar)])), 2);
    let v = call(vm, &foo, "offsetGet", vec![obj(&c)]).unwrap();
    assert!(v.identical(&s("c")));
    assert_eq!(int_of(call(vm, &foo, "removeAll", vec![obj(&bar)])), 0);
    assert_eq!(int_of(call(vm, &bar, "count", vec![])), 2);

    // bug53144: removeAll() of itself.
    assert_eq!(int_of(call(vm, &bar, "removeAll", vec![obj(&bar)])), 0);
    let (class, msg) = thrown(call(vm, &bar, "addAll", vec![obj(&a)]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplObjectStorage::addAll(): Argument #1 ($storage) must be of type SplObjectStorage, stdClass given"
    );
}

#[test]
fn add_all_resets_the_key_only() {
    let mut t = TestVm::new();
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    let s1 = storage(&mut t);
    let s2 = storage(&mut t);
    let vm = &mut t.vm;
    call(vm, &s1, "offsetSet", vec![obj(&a)]).unwrap();
    call(vm, &s2, "offsetSet", vec![obj(&b)]).unwrap();
    call(vm, &s1, "rewind", vec![]).unwrap();
    call(vm, &s1, "next", vec![]).unwrap();
    assert_eq!(int_of(call(vm, &s1, "key", vec![])), 1);
    call(vm, &s1, "addAll", vec![obj(&s2)]).unwrap();
    assert_eq!(int_of(call(vm, &s1, "key", vec![])), 0);
    // The cursor was past the end, so it now sees the added element.
    assert!(
        call(vm, &s1, "current", vec![])
            .unwrap()
            .identical(&obj(&b))
    );
}

// ---------------------------------------------------------------------------
// Iteration and seek
// ---------------------------------------------------------------------------

static MARKED_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(string $marker) {}",
    handler: |_vm, args| {
        let this = this(args);
        this.set_prop(b"marker", args.get(0).clone());
        Ok(Value::Null)
    },
};

fn marker(o: &Value) -> String {
    match o {
        Value::Object(o) => o.get_prop(b"marker").unwrap().as_str().unwrap().to_string(),
        other => panic!("not an object: {other:?}"),
    }
}

#[test]
fn seek() {
    // SplObjectStorage_seek.phpt
    let mut t = TestVm::new();
    super::declared_class(
        &mut t.vm,
        "Test",
        None,
        &[("marker", member_flags::PUBLIC, Value::Undef)],
        &[&MARKED_CONSTRUCT],
    );
    let objs: Vec<ObjRef> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|m| new_obj(&mut t.vm, "Test", vec![s(m)]).unwrap())
        .collect();
    let st = storage(&mut t);
    let vm = &mut t.vm;
    for (i, o) in objs.iter().enumerate() {
        dim_write(vm, &st, &obj(o), Value::Int(i as i64 + 1)).unwrap();
    }
    for bad in [-1, 5] {
        let (class, msg) = thrown(call(vm, &st, "seek", vec![Value::Int(bad)]));
        assert_eq!(class, "OutOfBoundsException");
        assert_eq!(msg, format!("Seek position {bad} is out of range"));
    }
    assert_eq!(int_of(call(vm, &st, "key", vec![])), 0);
    assert_eq!(marker(&call(vm, &st, "current", vec![]).unwrap()), "a");
    for (pos, m) in [(2, "c"), (1, "b"), (4, "e"), (0, "a"), (3, "d"), (3, "d")] {
        call(vm, &st, "seek", vec![Value::Int(pos)]).unwrap();
        assert_eq!(int_of(call(vm, &st, "key", vec![])), pos);
        assert_eq!(marker(&call(vm, &st, "current", vec![]).unwrap()), m);
    }
    // With holes.
    call(vm, &st, "offsetUnset", vec![obj(&objs[1])]).unwrap();
    call(vm, &st, "offsetUnset", vec![obj(&objs[3])]).unwrap();
    for (pos, m) in [(0, "a"), (1, "c"), (2, "e")] {
        call(vm, &st, "seek", vec![Value::Int(pos)]).unwrap();
        assert_eq!(int_of(call(vm, &st, "key", vec![])), pos);
        assert_eq!(marker(&call(vm, &st, "current", vec![]).unwrap()), m);
    }
    // Backwards from the end (closer than from the start).
    call(vm, &st, "seek", vec![Value::Int(1)]).unwrap();
    assert_eq!(marker(&call(vm, &st, "current", vec![]).unwrap()), "c");
    let (class, msg) = thrown(call(vm, &st, "seek", vec![Value::Int(3)]));
    assert_eq!(class, "OutOfBoundsException");
    assert_eq!(msg, "Seek position 3 is out of range");
}

#[test]
fn removing_the_current_element_moves_the_cursor_on() {
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let objs: Vec<ObjRef> = (0..3).map(|_| std_obj(&t.vm)).collect();
    let vm = &mut t.vm;
    for o in &objs {
        dim_write(vm, &st, &obj(o), Value::Null).unwrap();
    }
    call(vm, &st, "rewind", vec![]).unwrap();
    call(vm, &st, "next", vec![]).unwrap();
    // unset($s[$o]) does not reset the iterator (detach() does).
    dim_unset(vm, &st, &obj(&objs[1])).unwrap();
    assert!(
        call(vm, &st, "current", vec![])
            .unwrap()
            .identical(&obj(&objs[2]))
    );
    assert_eq!(int_of(call(vm, &st, "key", vec![])), 1);
    call(vm, &st, "offsetUnset", vec![obj(&objs[2])]).unwrap();
    assert_eq!(int_of(call(vm, &st, "key", vec![])), 0);
    assert!(
        call(vm, &st, "current", vec![])
            .unwrap()
            .identical(&obj(&objs[0]))
    );
}

fn log_and_parent(vm: &mut Vm, args: &mut Args, method: &str) -> PResult<Value> {
    let this = this(args);
    let r = parent_call(vm, "SplObjectStorage", &this, method, vec![])?;
    let shown = match (method, &r) {
        ("valid", Value::Bool(b)) => format!("({})", *b as i32),
        ("key", Value::Int(i)) => format!("({i})"),
        ("current", Value::Object(o)) => {
            format!("({})", o.get_prop(b"name").unwrap().as_str().unwrap())
        }
        _ => "()".into(),
    };
    log(vm, format!("MyObjectStorage::{method}{shown}"));
    Ok(r)
}

static MY_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: |vm, args| log_and_parent(vm, args, "rewind"),
};
static MY_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): bool {}",
    handler: |vm, args| log_and_parent(vm, args, "valid"),
};
static MY_KEY: NativeFunction = NativeFunction {
    sig: "function key(): int {}",
    handler: |vm, args| log_and_parent(vm, args, "key"),
};
static MY_CURRENT: NativeFunction = NativeFunction {
    sig: "function current(): object {}",
    handler: |vm, args| log_and_parent(vm, args, "current"),
};
static MY_NEXT: NativeFunction = NativeFunction {
    sig: "function next(): void {}",
    handler: |vm, args| log_and_parent(vm, args, "next"),
};

#[test]
fn overridden_iteration_methods() {
    // observer_002.phpt (the notify() loop over MyObjectStorage)
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyObjectStorage",
        "SplObjectStorage",
        &[&MY_REWIND, &MY_VALID, &MY_KEY, &MY_CURRENT, &MY_NEXT],
    );
    let st = new_obj(&mut t.vm, "MyObjectStorage", vec![]).unwrap();
    let observers: Vec<ObjRef> = ["$ob1", "$ob2", "$ob3"]
        .iter()
        .map(|n| {
            let o = std_obj(&t.vm);
            o.set_prop(b"name", s(n));
            o
        })
        .collect();
    let vm = &mut t.vm;
    for o in &observers {
        call(vm, &st, "offsetSet", vec![obj(o)]).unwrap();
    }
    call(vm, &st, "offsetSet", vec![obj(&observers[0])]).unwrap();
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 3);
    let notify = |vm: &mut Vm| -> Vec<String> {
        let mut it = ObjIterator::over(st.clone());
        it.rewind(vm).unwrap();
        while it.valid(vm).unwrap() {
            let o = it.current(vm).unwrap();
            it.key(vm).unwrap();
            let name = marker_name(&o);
            log(vm, format!("{name}->update"));
            it.next(vm).unwrap();
        }
        take_log(vm)
    };
    assert_eq!(
        notify(vm),
        [
            "MyObjectStorage::rewind()",
            "MyObjectStorage::valid(1)",
            "MyObjectStorage::current($ob1)",
            "MyObjectStorage::key(0)",
            "$ob1->update",
            "MyObjectStorage::next()",
            "MyObjectStorage::valid(1)",
            "MyObjectStorage::current($ob2)",
            "MyObjectStorage::key(1)",
            "$ob2->update",
            "MyObjectStorage::next()",
            "MyObjectStorage::valid(1)",
            "MyObjectStorage::current($ob3)",
            "MyObjectStorage::key(2)",
            "$ob3->update",
            "MyObjectStorage::next()",
            "MyObjectStorage::valid(0)",
        ]
    );
    call(vm, &st, "offsetUnset", vec![obj(&observers[2])]).unwrap();
    call(vm, &st, "offsetUnset", vec![obj(&observers[1])]).unwrap();
    call(vm, &st, "offsetUnset", vec![obj(&observers[0])]).unwrap();
    assert_eq!(
        notify(vm),
        ["MyObjectStorage::rewind()", "MyObjectStorage::valid(0)"]
    );
}

fn marker_name(o: &Value) -> String {
    match o {
        Value::Object(o) => o.get_prop(b"name").unwrap().as_str().unwrap().to_string(),
        other => panic!("not an object: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// getHash()
// ---------------------------------------------------------------------------

static HASH_INT: NativeFunction = NativeFunction {
    sig: "public function getHash($obj) {}",
    handler: |_vm, _args| Ok(Value::Int(2)),
};
static HASH_THROWS: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, _args| Err(vm.throw_error("Exception", "foo")),
};
static HASH_CONST: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |_vm, _args| Ok(Value::str("asd")),
};

#[test]
fn get_hash_overrides() {
    // SplObjectStorage_getHash.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MySplObjectStorage1",
        "SplObjectStorage",
        &[&HASH_INT],
    );
    user_class(
        &mut t.vm,
        "MySplObjectStorage2",
        "SplObjectStorage",
        &[&HASH_THROWS],
    );
    user_class(
        &mut t.vm,
        "MySplObjectStorage3",
        "SplObjectStorage",
        &[&HASH_CONST],
    );
    let o1 = std_obj(&t.vm);
    let o2 = std_obj(&t.vm);
    let mut out = Vec::new();
    for class in [
        "SplObjectStorage",
        "MySplObjectStorage1",
        "MySplObjectStorage2",
        "MySplObjectStorage3",
    ] {
        let st = new_obj(&mut t.vm, class, vec![]).unwrap();
        let vm = &mut t.vm;
        out.push(format!("Instance as {class}"));
        let r = (|| -> PResult<()> {
            dim_write(vm, &st, &obj(&o1), s("foo"))?;
            let v = call(vm, &st, "offsetGet", vec![obj(&o1)])?;
            out.push(dump(vm, &v));
            let v = dim_read(vm, &st, &obj(&o1), DimFetch::Read)?;
            out.push(dump(vm, &v));
            dim_write(vm, &st, &obj(&o2), obj(&o2))?;
            let a = dim_read(vm, &st, &obj(&o1), DimFetch::Read)?;
            let b = dim_read(vm, &st, &obj(&o2), DimFetch::Read)?;
            out.push(dump(vm, &Value::Bool(a.identical(&b))));
            Ok(())
        })();
        if let Err(e) = r {
            let (class, msg) = thrown(Err(e));
            out.push(format!("{class}: {msg}"));
        }
    }
    let out: String = out
        .iter()
        .map(|l| l.trim_end().to_string() + "\n")
        .collect();
    assert_eq!(
        out,
        "Instance as SplObjectStorage\nstring(3) \"foo\"\nstring(3) \"foo\"\nbool(false)\n\
         Instance as MySplObjectStorage1\nTypeError: MySplObjectStorage1::getHash(): Return value must be of type string, int returned\n\
         Instance as MySplObjectStorage2\nException: foo\n\
         Instance as MySplObjectStorage3\nstring(3) \"foo\"\nstring(3) \"foo\"\nbool(true)\n"
    );
}

#[test]
fn get_hash_string_keys_keep_the_first_object() {
    let mut t = TestVm::new();
    user_class(&mut t.vm, "OneBucket", "SplObjectStorage", &[&HASH_CONST]);
    let st = new_obj(&mut t.vm, "OneBucket", vec![]).unwrap();
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    let vm = &mut t.vm;
    call(vm, &st, "offsetSet", vec![obj(&a), Value::Int(1)]).unwrap();
    call(vm, &st, "offsetSet", vec![obj(&b), Value::Int(2)]).unwrap();
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 1);
    call(vm, &st, "rewind", vec![]).unwrap();
    assert!(
        call(vm, &st, "current", vec![])
            .unwrap()
            .identical(&obj(&a))
    );
    assert!(
        call(vm, &st, "getInfo", vec![])
            .unwrap()
            .identical(&Value::Int(2))
    );
    // The default getHash() is php_spl_object_hash().
    let plain = storage(&mut t);
    let h = call(&mut t.vm, &plain, "getHash", vec![obj(&a)]).unwrap();
    assert_eq!(h.as_str().unwrap().to_string(), object_hash(&a).to_string());
    assert_eq!(t.output(), "");
}

static HASH_CLASS: NativeFunction = NativeFunction {
    sig: "public function getHash($object): string {}",
    handler: |_vm, args| Ok(Value::String(args.object(0).class().name.clone())),
};

#[test]
fn clone_with_get_hash_override() {
    // bug67582.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyObjectStorage",
        "SplObjectStorage",
        &[&HASH_CLASS],
    );
    let list = new_obj(&mut t.vm, "MyObjectStorage", vec![]).unwrap();
    let o = std_obj(&t.vm);
    let vm = &mut t.vm;
    call(vm, &list, "offsetSet", vec![obj(&o)]).unwrap();
    let clone = (list.class().handlers.clone_obj.unwrap())(vm, &list).unwrap();
    assert_eq!(clone.class().name.to_string(), "MyObjectStorage");
    for st in [&list, &clone] {
        let mut it = ObjIterator::over(st.clone());
        it.rewind(vm).unwrap();
        while it.valid(vm).unwrap() {
            let x = it.current(vm).unwrap();
            let r = call(vm, st, "offsetExists", vec![x]).unwrap();
            assert!(r.identical(&Value::Bool(true)));
            it.next(vm).unwrap();
        }
    }
}

#[test]
fn clone_copies_storage_and_properties() {
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    st.set_prop(b"extra", Value::Int(7));
    let vm = &mut t.vm;
    dim_write(vm, &st, &obj(&a), s("a")).unwrap();
    let copy = (st.class().handlers.clone_obj.unwrap())(vm, &st).unwrap();
    assert!(!ObjRef::ptr_eq(&copy, &st));
    dim_write(vm, &copy, &obj(&b), s("b")).unwrap();
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 1);
    assert_eq!(int_of(call(vm, &copy, "count", vec![])), 2);
    assert!(copy.get_prop(b"extra").unwrap().identical(&Value::Int(7)));
    let v = dim_read(vm, &copy, &obj(&a), DimFetch::Read).unwrap();
    assert!(v.identical(&s("a")));
}

// ---------------------------------------------------------------------------
// Modification during getHash()
// ---------------------------------------------------------------------------

const MUTATION: &str = "Modification of SplObjectStorage during getHash() is prohibited";

/// `EvilStorage::getHash()` of concurrent_deletion.phpt: writes to the
/// global `$victim` when `$this->mutate`.
static EVIL_WRITE_HASH: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, args| {
        let o = args.object(0).clone();
        if state(vm).mutate {
            let victim = state(vm).target.clone().unwrap();
            let fresh = std_obj(vm);
            dim_write(vm, &victim, &obj(&fresh), Value::Null)?;
        }
        Ok(Value::String(PhpStr::from(o.id().to_string())))
    },
};

#[test]
fn mutation_during_get_hash_is_prohibited() {
    // concurrent_deletion.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "EvilStorage",
        "SplObjectStorage",
        &[&EVIL_WRITE_HASH],
    );
    for scenario in ["removeAllExcept", "addAll"] {
        let victim = storage(&mut t);
        let other = new_obj(&mut t.vm, "EvilStorage", vec![]).unwrap();
        let vm = &mut t.vm;
        state(vm).target = Some(victim.clone());
        state(vm).mutate = false;
        let mut keep = Vec::new();
        for _ in 0..64 {
            let o = std_obj(vm);
            dim_write(vm, &victim, &obj(&o), Value::Null).unwrap();
            dim_write(vm, &other, &obj(&o), Value::Null).unwrap();
            keep.push(o);
        }
        state(vm).mutate = true;
        let r = if scenario == "removeAllExcept" {
            call(vm, &victim, "removeAllExcept", vec![obj(&other)])
        } else {
            call(vm, &other, "addAll", vec![obj(&victim)])
        };
        let (class, msg) = thrown(r);
        assert_eq!(
            (class.as_str(), msg.as_str()),
            ("Error", MUTATION),
            "{scenario}"
        );
        assert_eq!(int_of(call(vm, &victim, "count", vec![])), 64);
        assert_eq!(int_of(call(vm, &other, "count", vec![])), 64);
        state(vm).mutate = false;
    }
}

/// `EvilStorage::getHash()` of concurrent_deletion_addall.phpt and
/// concurrent_deletion_removeexcept.phpt: `unset($storage[$obj])`.
static EVIL_UNSET_HASH: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, args| {
        let o = args.object(0).clone();
        let target = state(vm).target.clone().unwrap();
        dim_unset(vm, &target, &obj(&o))?;
        Ok(Value::String(object_hash(&o)))
    },
};

#[test]
fn unset_during_get_hash_is_prohibited() {
    // concurrent_deletion_addall.phpt, concurrent_deletion_removeexcept.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "EvilStorage",
        "SplObjectStorage",
        &[&EVIL_UNSET_HASH],
    );
    for scenario in ["addAll", "removeAllExcept"] {
        let st = storage(&mut t);
        let o = std_obj(&t.vm);
        let evil = new_obj(&mut t.vm, "EvilStorage", vec![]).unwrap();
        let vm = &mut t.vm;
        dim_write(vm, &st, &obj(&o), s("foo")).unwrap();
        state(vm).target = Some(st.clone());
        let r = if scenario == "addAll" {
            call(vm, &evil, "addAll", vec![obj(&st)])
        } else {
            call(vm, &st, "removeAllExcept", vec![obj(&evil)])
        };
        let (class, msg) = thrown(r);
        assert_eq!(
            (class.as_str(), msg.as_str()),
            ("Error", MUTATION),
            "{scenario}"
        );
        assert_eq!(int_of(call(vm, &evil, "count", vec![])), 0);
        assert_eq!(int_of(call(vm, &st, "count", vec![])), 1);
    }
    // The depth counter went back to zero.
    let st = storage(&mut t);
    let o = std_obj(&t.vm);
    dim_write(&mut t.vm, &st, &obj(&o), Value::Null).unwrap();
}

/// `FilterStorage::getHash()` of gh21831.phpt.
static FILTER_HASH: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, args| {
        let o = args.object(0).clone();
        if let Some(other) = state(vm).target.take() {
            call(vm, &other, "offsetUnset", vec![obj(&o)])?;
        }
        Ok(Value::str("x"))
    },
};

#[test]
fn offset_unset_during_get_hash_is_prohibited() {
    // gh21831.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "FilterStorage",
        "SplObjectStorage",
        &[&FILTER_HASH],
    );
    let st = storage(&mut t);
    let o = std_obj(&t.vm);
    let filter = new_obj(&mut t.vm, "FilterStorage", vec![]).unwrap();
    let vm = &mut t.vm;
    dim_write(vm, &st, &obj(&o), Value::Null).unwrap();
    state(vm).target = Some(st.clone());
    let (class, msg) = thrown(call(vm, &st, "removeAllExcept", vec![obj(&filter)]));
    assert_eq!((class.as_str(), msg.as_str()), ("Error", MUTATION));
    assert_eq!(int_of(call(vm, &st, "count", vec![])), 1);
}

/// A getHash() that runs `$code` on the target storage.
static MUTATING_HASH: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, args| {
        let o = args.object(0).clone();
        let target = state(vm).target.clone().unwrap();
        let r = match state(vm).log.pop().as_deref() {
            Some("setInfo") => call(vm, &target, "setInfo", vec![Value::Int(1)]),
            Some("attach") => call(vm, &target, "offsetSet", vec![obj(&o)]),
            Some("unserialize") => {
                let pairs = list(vec![obj(&o), Value::Null]);
                let data = list(vec![pairs, Value::empty_array()]);
                call(vm, &target, "__unserialize", vec![data])
            }
            _ => Ok(Value::Null),
        };
        r?;
        Ok(Value::str("h"))
    },
};

#[test]
fn every_mutation_checks_the_get_hash_depth() {
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MutatingStorage",
        "SplObjectStorage",
        &[&MUTATING_HASH],
    );
    let target = storage(&mut t);
    let o = std_obj(&t.vm);
    let st = new_obj(&mut t.vm, "MutatingStorage", vec![]).unwrap();
    let vm = &mut t.vm;
    state(vm).target = Some(target.clone());
    for op in ["setInfo", "attach", "unserialize"] {
        log(vm, op);
        let (class, msg) = thrown(call(vm, &st, "offsetExists", vec![obj(&o)]));
        assert_eq!((class.as_str(), msg.as_str()), ("Error", MUTATION), "{op}");
    }
    // Reads are allowed, and the counter is back at zero afterwards.
    assert!(
        call(vm, &st, "offsetExists", vec![obj(&o)])
            .unwrap()
            .identical(&Value::Bool(false))
    );
    dim_write(vm, &target, &obj(&o), Value::Null).unwrap();
    // PHP_RINIT resets the counter (spl_object_storage_gethash_bailout.phpt).
    super::super::globals(vm).get_hash_depth = 3;
    let (_, msg) = thrown(dim_write(vm, &target, &obj(&o), Value::Null).map(|_| Value::Null));
    assert_eq!(msg, MUTATION);
    reset_get_hash_depth(vm);
    dim_write(vm, &target, &obj(&o), Value::Null).unwrap();
    assert_eq!(int_of(call(vm, &target, "count", vec![])), 1);
}

static LOGGING_HASH: NativeFunction = NativeFunction {
    sig: "public function getHash($obj): string {}",
    handler: |vm, args| {
        let o = args.object(0).clone();
        log(vm, format!("getHash(#{})", o.id()));
        Ok(Value::String(object_hash(&o)))
    },
};

#[test]
fn remove_all_retries_detached_elements() {
    // php-src's removeAll() walks the other storage with its internal
    // pointer and only advances when a detach fails, so a successful
    // detach hashes the same object twice.
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "LoggingStorage",
        "SplObjectStorage",
        &[&LOGGING_HASH],
    );
    let st = new_obj(&mut t.vm, "LoggingStorage", vec![]).unwrap();
    let other = storage(&mut t);
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    let vm = &mut t.vm;
    call(vm, &st, "offsetSet", vec![obj(&a)]).unwrap();
    call(vm, &other, "offsetSet", vec![obj(&a)]).unwrap();
    call(vm, &other, "offsetSet", vec![obj(&b)]).unwrap();
    take_log(vm);
    assert_eq!(int_of(call(vm, &st, "removeAll", vec![obj(&other)])), 0);
    assert_eq!(
        take_log(vm),
        [
            format!("getHash(#{})", a.id()),
            format!("getHash(#{})", a.id()),
            format!("getHash(#{})", b.id()),
        ]
    );
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

#[test]
fn compare_handler() {
    let mut t = TestVm::new();
    let (a, b) = (std_obj(&t.vm), std_obj(&t.vm));
    let s1 = storage(&mut t);
    let s2 = storage(&mut t);
    let vm = &mut t.vm;
    let eq = |vm: &mut Vm, x: &ObjRef, y: &ObjRef| vm.compare(&obj(x), &obj(y)).unwrap();
    assert_eq!(eq(vm, &s1, &s2), 0);
    dim_write(vm, &s1, &obj(&a), Value::Int(1)).unwrap();
    assert_eq!(eq(vm, &s1, &s2), 1);
    assert_eq!(eq(vm, &s2, &s1), -1);
    dim_write(vm, &s2, &obj(&a), Value::Int(2)).unwrap();
    assert_eq!(eq(vm, &s1, &s2), -1);
    dim_write(vm, &s2, &obj(&a), s("1")).unwrap();
    assert_eq!(eq(vm, &s1, &s2), 0);
    // Same size, different objects: uncomparable.
    dim_write(vm, &s1, &obj(&b), Value::Null).unwrap();
    let c = std_obj(vm);
    dim_write(vm, &s2, &obj(&c), Value::Null).unwrap();
    assert_eq!(eq(vm, &s1, &s2), 1);
    assert_eq!(eq(vm, &s2, &s1), 1);
    // Insertion order does not matter.
    let s3 = new_obj(vm, "SplObjectStorage", vec![]).unwrap();
    dim_write(vm, &s3, &obj(&b), Value::Null).unwrap();
    dim_write(vm, &s3, &obj(&a), Value::Int(1)).unwrap();
    assert_eq!(eq(vm, &s1, &s3), 0);
    assert!(vm.is_equal(&obj(&s1), &obj(&s3)).unwrap());
    // Subclasses and MultipleIterators are uncomparable.
    user_class(vm, "SubStorage", "SplObjectStorage", &[]);
    let sub = new_obj(vm, "SubStorage", vec![]).unwrap();
    let sub2 = new_obj(vm, "SubStorage", vec![]).unwrap();
    assert_eq!(eq(vm, &sub, &sub2), 1);
    assert_eq!(eq(vm, &sub, &sub), 0);
    let m1 = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    let m2 = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    assert_eq!(eq(vm, &m1, &m2), 1);
    // Against another class: the standard comparison.
    let plain = std_obj(vm);
    assert_eq!(eq(vm, &s1, &plain), 1);
}

#[test]
fn compare_detects_recursion() {
    let mut t = TestVm::new();
    let s1 = storage(&mut t);
    let s2 = storage(&mut t);
    let k = std_obj(&t.vm);
    let vm = &mut t.vm;
    dim_write(vm, &s1, &obj(&k), obj(&s1)).unwrap();
    dim_write(vm, &s2, &obj(&k), obj(&s2)).unwrap();
    let (class, msg) = thrown(
        vm.compare(&obj(&s1), &obj(&s2))
            .map(|c| Value::Int(c.into())),
    );
    assert_eq!(class, "Error");
    assert_eq!(msg, "Nesting level too deep - recursive dependency?");
    // The guard was released.
    let s3 = new_obj(vm, "SplObjectStorage", vec![]).unwrap();
    assert_eq!(vm.compare(&obj(&s3), &obj(&s1)).unwrap(), -1);
}

// ---------------------------------------------------------------------------
// __debugInfo / var_dump
// ---------------------------------------------------------------------------

#[test]
fn recursive_var_dump() {
    // SplObjectStorage_var_dump.phpt
    let mut t = TestVm::new();
    let o = storage(&mut t);
    let k = std_obj(&t.vm);
    dim_write(&mut t.vm, &o, &obj(&k), obj(&o)).unwrap();
    let out = dump(&mut t.vm, &obj(&o));
    assert_expectf(
        "object(SplObjectStorage)#%d (1) {\n  [\"storage\":\"SplObjectStorage\":private]=>\n  array(1) {\n    [0]=>\n    array(2) {\n      [\"obj\"]=>\n      object(stdClass)#%d (0) {\n      }\n      [\"inf\"]=>\n      *RECURSION*\n    }\n  }\n}\n",
        &out,
    );
}

#[test]
fn debug_info_is_a_copy() {
    // gh18322.phpt
    let mut t = TestVm::new();
    let st = storage(&mut t);
    let o = std_obj(&t.vm);
    let vm = &mut t.vm;
    dim_write(vm, &st, &obj(&o), Value::Int(1)).unwrap();
    let tmp = call(vm, &st, "__debugInfo", vec![]).unwrap();
    let tmp2 = array_of(&tmp).values().next().unwrap().clone();
    let Value::Array(mut tmp2) = tmp2 else {
        panic!("storage list")
    };
    drop(tmp);
    let fresh = std_obj(vm);
    let inner = std::rc::Rc::make_mut(&mut tmp2);
    let first = inner.get_int(0).unwrap().clone();
    let Value::Array(mut first) = first else {
        panic!("pair")
    };
    std::rc::Rc::make_mut(&mut first).insert_str("obj", obj(&fresh));
    inner.insert(php_runtime::ArrayKey::Int(0), Value::Array(first));
    assert_expectf(
        "array(1) {\n  [0]=>\n  array(2) {\n    [\"obj\"]=>\n    object(stdClass)#%d (0) {\n    }\n    [\"inf\"]=>\n    int(1)\n  }\n}\n",
        &dump(vm, &Value::Array(tmp2)),
    );
    // The storage still holds the original object.
    let v = call(vm, &st, "current", vec![]).unwrap();
    assert!(v.identical(&obj(&o)));
}

static CHILD_DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: |vm, args| {
        let this = this(args);
        let parent = if this.class().instance_of_name(b"MultipleIterator") {
            "MultipleIterator"
        } else {
            "SplObjectStorage"
        };
        let p = parent_call(vm, parent, &this, "__debugInfo", vec![])?;
        let n = array_of(&p).len() as i64;
        Ok(super::array_from(vec![
            (php_runtime::ArrayKey::from("child"), Value::Int(42)),
            (php_runtime::ArrayKey::from("parent"), Value::Int(n)),
        ]))
    },
};

#[test]
fn debug_info_overrides() {
    // bug69264.phpt (SplObjectStorage and MultipleIterator)
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyObjectStorage",
        "SplObjectStorage",
        &[&CHILD_DEBUG_INFO],
    );
    user_class(
        &mut t.vm,
        "MyMultipleIterator",
        "MultipleIterator",
        &[&CHILD_DEBUG_INFO],
    );
    let a = new_obj(&mut t.vm, "MyObjectStorage", vec![]).unwrap();
    let b = new_obj(&mut t.vm, "MyMultipleIterator", vec![]).unwrap();
    let mut out = dump(&mut t.vm, &obj(&a));
    out += &dump(&mut t.vm, &obj(&b));
    assert_expectf(
        "object(MyObjectStorage)#%d (2) {\n  [\"child\"]=>\n  int(42)\n  [\"parent\"]=>\n  int(1)\n}\n\
         object(MyMultipleIterator)#%d (2) {\n  [\"child\"]=>\n  int(42)\n  [\"parent\"]=>\n  int(1)\n}\n",
        &out,
    );
}

#[test]
fn debug_info_includes_properties() {
    let mut t = TestVm::new();
    let st = storage(&mut t);
    st.set_prop(b"extra", Value::Int(1));
    let o = std_obj(&t.vm);
    dim_write(&mut t.vm, &st, &obj(&o), Value::Null).unwrap();
    let table = super::super::storage::debug_info(&st);
    let keys: Vec<Vec<u8>> = table.keys().map(|k| k.to_php_str().to_vec()).collect();
    assert_eq!(
        keys,
        [b"extra".to_vec(), b"\0SplObjectStorage\0storage".to_vec()]
    );
    assert_eq!(engine::std_properties(&st).len(), 1);
}
