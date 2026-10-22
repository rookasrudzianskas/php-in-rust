//! CachingIterator and RecursiveCachingIterator.

use std::rc::Rc;

use php_runtime::class::ClassEntry;
use php_runtime::{Args, ArrayKey, Flow, NativeFunction, ObjRef, PResult, Value, Vm};

use super::helpers::*;
use crate::spl::engine;
use crate::spl::testutil::{assoc, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

const FULL_CACHE: i64 = 256;

fn l(v: Vec<Value>) -> Value {
    list(v)
}

fn call(vm: &mut Vm, o: &ObjRef, m: &str, a: Vec<Value>) -> PResult<Value> {
    vm.call_method(o, m.as_bytes(), a)
}

fn print_exception(vm: &mut Vm, r: PResult<Value>) {
    match r {
        Ok(_) => {}
        Err(Flow::Exception(e)) => {
            let msg = Vm::exception_message(&e);
            echo(vm, &format!("{}: {msg}", e.class().name)).unwrap();
        }
        Err(other) => panic!("{other:?}"),
    }
}

/// A class `name` whose `__toString()` returns the `value` property.
fn stringable_class(vm: &mut Vm, name: &str) {
    fn to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let v = engine::this(args).get_prop(b"value").unwrap_or(Value::Null);
        Ok(Value::String(vm.to_php_string(&v)?))
    }
    static TO_STRING: NativeFunction = NativeFunction {
        sig: "function __toString(): string {}",
        handler: to_string,
    };
    let mut ce = ClassEntry::new(name, 0);
    engine::add_methods(&mut ce, &[&TO_STRING]);
    ce.link_magic_methods();
    vm.register_class(Rc::new(ce));
}

fn stringable(vm: &mut Vm, class: &str, value: Value) -> Value {
    let ce = vm.find_class(class.as_bytes()).unwrap();
    let o = ObjRef::new(ce);
    o.set_prop(b"value", value);
    Value::Object(o)
}

#[test]
fn to_string_of_cached_elements() {
    // iterator_003.phpt (the CachingIterator part)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    stringable_class(vm, "Student");
    let students = l(vec![
        stringable(vm, "Student", s("01234123, Joe")),
        stringable(vm, "Student", s("00000014, Bob")),
    ]);
    let inner = plain(vm, &students);
    let it = new_obj(vm, "CachingIterator", vec![Value::Object(inner)]).unwrap();
    foreach_with(vm, &it, false, |vm, _, _| {
        let s = call(vm, &it, "__toString", vec![])?;
        let s = vm.to_php_string(&s)?;
        echo(vm, &s.to_string())?;
        Ok(true)
    })
    .unwrap();
    assert_phpt(&t.output(), phpt!("iterator_003.phpt"));
}

#[test]
fn to_string_requires_a_string_flag() {
    // iterator_036.phpt, spl_cachingiterator___toString_basic.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![1.into(), 2.into(), 3.into()]));
    let it = new_obj(vm, "CachingIterator", vec![Value::Object(inner), 0.into()]).unwrap();
    call(vm, &it, "rewind", vec![]).unwrap();
    assert_eq!(
        thrown(vm.to_php_string(&Value::Object(it)).map(Value::String)),
        (
            "BadMethodCallException".into(),
            "CachingIterator does not fetch string value (see CachingIterator::__construct)".into()
        )
    );
    let std = vm.find_class(b"stdClass").unwrap();
    let objs = l(vec![
        Value::Object(ObjRef::new(std.clone())),
        Value::Object(ObjRef::new(std)),
    ]);
    let inner = plain(vm, &objs);
    let it = new_obj(vm, "CachingIterator", vec![Value::Object(inner)]).unwrap();
    assert!(
        call(vm, &it, "__toString", vec![])
            .unwrap()
            .identical(&s(""))
    );
}

fn key_current_to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let k = vm.call_method(&this, b"key", vec![])?;
    let c = vm.call_method(&this, b"current", vec![])?;
    let k = vm.to_php_string(&k)?;
    let c = vm.to_php_string(&c)?;
    Ok(Value::str(&format!("{k}:{c}")))
}
static KEY_CURRENT_TO_STRING: NativeFunction = NativeFunction {
    sig: "function __toString(): string {}",
    handler: key_current_to_string,
};

fn test_037(vm: &mut Vm, ar: &ObjRef, flags: i64) {
    echo(vm, &format!("==={flags}===")).unwrap();
    let it = new_obj(
        vm,
        "CachingIterator",
        vec![Value::Object(ar.clone()), 0.into()],
    )
    .unwrap();
    if let Err(Flow::Exception(e)) = call(vm, &it, "setFlags", vec![flags.into()]) {
        let msg = Vm::exception_message(&e);
        echo(vm, &format!("{}: {msg}", e.class().name)).unwrap();
        let f = call(vm, &it, "getFlags", vec![]).unwrap();
        var_dump(vm, &f).unwrap();
        return;
    }
    let f = call(vm, &it, "getFlags", vec![]).unwrap();
    var_dump(vm, &f).unwrap();
    let r = foreach_with(vm, &it, false, |vm, _, _| {
        let s = vm.to_php_string(&Value::Object(it.clone()))?;
        var_dump(vm, &Value::String(s))?;
        Ok(true)
    });
    print_exception(vm, r.map(|_| Value::Null));
}

#[test]
fn to_string_modes_and_set_flags() {
    // iterator_037.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    iter_class(vm, "TIter", Kind::Plain, None);
    user_class(vm, "MyArrayIterator", "TIter", &[&KEY_CURRENT_TO_STRING]);
    stringable_class(vm, "MyItem");
    let ce = vm.find_class(b"MyArrayIterator").unwrap();
    let ar = new_iter(&ce, &l(vec![1.into(), 2.into(), 3.into()]), Opts::default());
    for flags in [1, 2, 4] {
        test_037(vm, &ar, flags);
    }
    let items = l(vec![
        stringable(vm, "MyItem", 1.into()),
        stringable(vm, "MyItem", 2.into()),
        stringable(vm, "MyItem", 3.into()),
    ]);
    let ar = new_iter(&ce, &items, Opts::default());
    for flags in [8, 1 | 2, 1 | 4, 1 | 8, 2 | 4, 2 | 8] {
        test_037(vm, &ar, flags);
    }
    echo(vm, "===X===").unwrap();
    let it = new_obj(
        vm,
        "CachingIterator",
        vec![Value::Object(ar.clone()), 1.into()],
    )
    .unwrap();
    let r = call(vm, &it, "setFlags", vec![0.into()]);
    print_exception(vm, r);
    let it = new_obj(vm, "CachingIterator", vec![Value::Object(ar), 8.into()]).unwrap();
    let r = call(vm, &it, "setFlags", vec![0.into()]);
    print_exception(vm, r);
    assert_phpt(&t.output(), phpt!("iterator_037.phpt"));
}

#[test]
fn constructor_flag_checks() {
    // spl_caching_iterator_constructor_flags.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![1.into()]));
    for flags in [0, 1, 2, 4, 8] {
        new_obj(
            vm,
            "CachingIterator",
            vec![Value::Object(inner.clone()), flags.into()],
        )
        .unwrap();
    }
    let r = new_obj(vm, "CachingIterator", vec![Value::Object(inner), 3.into()]).map(Value::Object);
    print_exception(vm, r);
    assert_phpt(
        &t.output(),
        phpt!("spl_caching_iterator_constructor_flags.phpt"),
    );
}

#[test]
fn cache_accessors_need_full_cache() {
    // iterator_044.phpt / iterator_045.phpt (first part),
    // spl_iterator_caching_count_error.phpt, spl_iterator_caching_getcache_error.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(vm, "MyCachingIterator", "CachingIterator", &[]);
    let inner = plain(vm, &l(vec![0.into(), 1.into()]));
    let it = new_obj(vm, "MyCachingIterator", vec![Value::Object(inner)]).unwrap();
    let err = (
        "BadMethodCallException".to_string(),
        "MyCachingIterator does not use a full cache (see CachingIterator::__construct)"
            .to_string(),
    );
    for (m, a) in [
        ("offsetExists", vec![0.into()]),
        ("offsetGet", vec![0.into()]),
        ("offsetSet", vec![0.into(), 0.into()]),
        ("offsetUnset", vec![0.into()]),
        ("getCache", vec![]),
        ("count", vec![]),
    ] {
        assert_eq!(thrown(call(vm, &it, m, a)), err, "{m}");
    }
}

#[test]
fn offset_get_and_exists() {
    // iterator_044.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    stringable_class(vm, "MyFoo");
    let data = assoc(vec![
        (ArrayKey::Int(0), 0.into()),
        (skey("foo"), 1.into()),
        (ArrayKey::Int(1), 2.into()),
        (skey("bar"), 3.into()),
        (ArrayKey::Int(2), 4.into()),
    ]);
    let inner = plain(vm, &data);
    let it = new_obj(
        vm,
        "CachingIterator",
        vec![Value::Object(inner), FULL_CACHE.into()],
    )
    .unwrap();
    let std = vm.find_class(b"stdClass").unwrap();
    let checks = vec![
        Value::Int(0),
        Value::Object(ObjRef::new(std)),
        stringable(vm, "MyFoo", s("foo")),
        Value::Null,
        Value::Int(2),
        s("foo"),
        Value::Int(3),
    ];
    let run = |vm: &mut Vm, out: &mut Vec<String>| {
        for v in &checks {
            for m in ["offsetExists", "offsetGet"] {
                match call(vm, &it, m, vec![v.clone()]) {
                    Ok(r) => out.push(dump(&r).trim_end().to_string()),
                    Err(Flow::Exception(e)) => out.push(Vm::exception_message(&e)),
                    Err(other) => panic!("{other:?}"),
                }
            }
        }
    };
    let mut before = Vec::new();
    run(vm, &mut before);
    let type_err = |m: &str| {
        format!("CachingIterator::{m}(): Argument #1 ($key) must be of type string, stdClass given")
    };
    assert_eq!(
        before,
        [
            "bool(false)".to_string(),
            "NULL".into(),
            type_err("offsetExists"),
            type_err("offsetGet"),
            "bool(false)".into(),
            "NULL".into(),
            "bool(false)".into(),
            "NULL".into(),
            "bool(false)".into(),
            "NULL".into(),
            "bool(false)".into(),
            "NULL".into(),
            "bool(false)".into(),
            "NULL".into(),
        ]
    );
    let out = t.output();
    assert!(out.contains("\nWarning: Undefined array key \"0\" in /t.php on line 3\n"));
    assert!(out.contains("\nWarning: Undefined array key \"foo\" in /t.php on line 3\n"));
    assert!(out.contains(
        "\nDeprecated: CachingIterator::offsetExists(): Passing null to parameter #1 ($key) of type string is deprecated in /t.php on line 3\n"
    ));
    assert!(out.contains("\nWarning: Undefined array key \"\" in /t.php on line 3\n"));

    let vm = &mut t.vm;
    foreach_values(vm, &it).unwrap(); // read all into cache
    let mut after = Vec::new();
    run(vm, &mut after);
    assert_eq!(
        after,
        [
            "bool(true)".to_string(),
            "int(0)".into(),
            type_err("offsetExists"),
            type_err("offsetGet"),
            "bool(true)".into(),
            "int(1)".into(),
            "bool(false)".into(),
            "NULL".into(),
            "bool(true)".into(),
            "int(4)".into(),
            "bool(true)".into(),
            "int(1)".into(),
            "bool(false)".into(),
            "NULL".into(),
        ]
    );
}

#[test]
fn offset_set_unset_and_get_cache() {
    // iterator_045.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![0.into(), 1.into(), 2.into(), 3.into()]));
    let it = new_obj(
        vm,
        "CachingIterator",
        vec![Value::Object(inner), FULL_CACHE.into()],
    )
    .unwrap();
    let test_set = |vm: &mut Vm| {
        echo(vm, "MyCachingIterator::testSet()").unwrap();
        for (k, v) in [(0, Value::Int(25)), (1, Value::Int(42)), (3, s("FooBar"))] {
            let vs = vm.to_php_string(&v).unwrap();
            echo(vm, &format!("set({k},{vs})")).unwrap();
            call(vm, &it, "offsetSet", vec![k.into(), v]).unwrap();
        }
    };
    let test_unset = |vm: &mut Vm| {
        echo(vm, "MyCachingIterator::testUnset()").unwrap();
        for k in [0, 2] {
            echo(vm, &format!("unset({k})")).unwrap();
            call(vm, &it, "offsetUnset", vec![k.into()]).unwrap();
        }
    };
    let show = |vm: &mut Vm| {
        echo(vm, "MyCachingIterator::show()").unwrap();
        let c = call(vm, &it, "getCache", vec![]).unwrap();
        var_dump(vm, &c).unwrap();
    };
    echo(
        vm,
        "BadMethodCallException: MyCachingIterator does not use a full cache (see CachingIterator::__construct)",
    )
    .unwrap();
    echo(
        vm,
        "BadMethodCallException: MyCachingIterator does not use a full cache (see CachingIterator::__construct)",
    )
    .unwrap();
    test_set(vm);
    show(vm);
    test_unset(vm);
    show(vm);
    echo(vm, "MyCachingIterator::fill()").unwrap();
    foreach_values(vm, &it).unwrap();
    show(vm);
    test_set(vm);
    show(vm);
    test_unset(vm);
    show(vm);
    assert_phpt(&t.output(), phpt!("iterator_045.phpt"));
}

#[test]
fn to_string_use_key() {
    // iterator_046.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let data = assoc(vec![
        (ArrayKey::Int(0), 0.into()),
        (skey("foo"), 1.into()),
        (skey("bar"), 2.into()),
    ]);
    let inner = plain(vm, &data);
    let it = new_obj(vm, "CachingIterator", vec![Value::Object(inner), 2.into()]).unwrap();
    echo(vm, "MyCachingIterator::fill()").unwrap();
    foreach_values(vm, &it).unwrap();
    echo(vm, "MyCachingIterator::show()").unwrap();
    foreach_with(vm, &it, false, |vm, _, _| {
        let s = vm.to_php_string(&Value::Object(it.clone()))?;
        var_dump(vm, &Value::String(s)).map(|_| true)
    })
    .unwrap();
    assert_phpt(&t.output(), phpt!("iterator_046.phpt"));
}

#[test]
fn count_and_cache() {
    // spl_iterator_caching_count_basic.phpt, iterator_027.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![1.into(); 5]));
    let it = new_obj(
        vm,
        "CachingIterator",
        vec![Value::Object(inner), FULL_CACHE.into()],
    )
    .unwrap();
    let mut counts = Vec::new();
    foreach_values(vm, &it).unwrap();
    call(vm, &it, "rewind", vec![]).unwrap();
    loop {
        let valid = call(vm, &it, "valid", vec![]).unwrap();
        if !vm.to_bool(&valid).unwrap() {
            break;
        }
        counts.push(int(&call(vm, &it, "count", vec![]).unwrap()));
        call(vm, &it, "next", vec![]).unwrap();
    }
    assert_eq!(counts, [1, 2, 3, 4, 5]);
}

#[test]
fn full_cache_over_recursive_iterator() {
    // iterator_027.phpt (ArrayAccess through the offset*() methods)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ar = l(vec![
        1.into(),
        2.into(),
        l(vec![31.into(), 32.into(), l(vec![331.into()])]),
        4.into(),
    ]);
    let inner = recursive(vm, &ar);
    let rii = new_obj(vm, "RecursiveIteratorIterator", vec![Value::Object(inner)]).unwrap();
    let it = new_obj(
        vm,
        "CachingIterator",
        vec![Value::Object(rii), FULL_CACHE.into()],
    )
    .unwrap();
    foreach_with(vm, &it, true, |vm, k, v| {
        let k = vm.to_php_string(&k)?;
        let v = vm.to_php_string(&v)?;
        echo(vm, &format!("{k}=>{v}"))?;
        Ok(true)
    })
    .unwrap();
    echo(vm, "===CHECK===").unwrap();
    let isset = |vm: &mut Vm, k: Value| {
        let r = call(vm, &it, "offsetExists", vec![k]).unwrap();
        vm.to_bool(&r).unwrap()
    };
    for i in 0..4 {
        if isset(vm, i.into()) {
            var_dump(vm, &i.into()).unwrap();
            let v = call(vm, &it, "offsetGet", vec![i.into()]).unwrap();
            var_dump(vm, &v).unwrap();
        }
    }
    call(vm, &it, "offsetSet", vec![2.into(), s("foo")]).unwrap();
    call(vm, &it, "offsetSet", vec![3.into(), s("bar")]).unwrap();
    call(vm, &it, "offsetSet", vec![s("baz"), s("25")]).unwrap();
    for k in [Value::Int(2), Value::Int(3), s("baz")] {
        let v = call(vm, &it, "offsetGet", vec![k]).unwrap();
        var_dump(vm, &v).unwrap();
    }
    for k in [Value::Int(0), Value::Int(2), s("baz")] {
        call(vm, &it, "offsetUnset", vec![k]).unwrap();
    }
    for k in [0.into(), 1.into(), 2.into(), 3.into(), s("baz")] {
        let b = isset(vm, k);
        var_dump(vm, &Value::Bool(b)).unwrap();
    }
    echo(vm, "===REWIND===").unwrap();
    call(vm, &it, "rewind", vec![]).unwrap();
    for k in 0..4 {
        let b = isset(vm, k.into());
        var_dump(vm, &Value::Bool(b)).unwrap();
    }
    assert_phpt(&t.output(), phpt!("iterator_027.phpt"));
}

#[test]
fn has_next_in_recursive_caching_iterator() {
    // iterator_026.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ar = l(vec![
        1.into(),
        2.into(),
        l(vec![31.into(), 32.into(), l(vec![331.into()])]),
        4.into(),
    ]);
    let inner = recursive(vm, &ar);
    let rci = new_obj(vm, "RecursiveCachingIterator", vec![Value::Object(inner)]).unwrap();
    let it = new_obj(vm, "RecursiveIteratorIterator", vec![Value::Object(rci)]).unwrap();
    foreach_with(vm, &it, true, |vm, k, v| {
        let k = vm.to_php_string(&k)?;
        let v = vm.to_php_string(&v)?;
        echo(vm, &format!("{k}=>{v}"))?;
        let inner = vm.call_method(&it, b"getInnerIterator", vec![])?;
        let has = vm.call_method(inner.as_object().unwrap(), b"hasNext", vec![])?;
        let yn = if vm.to_bool(&has)? { "yes" } else { "no" };
        echo(vm, &format!("hasNext: {yn}"))?;
        Ok(true)
    })
    .unwrap();
    assert_phpt(&t.output(), phpt!("iterator_026.phpt"));
}

#[test]
fn recursive_caching_children_before_iteration() {
    // iterator_043.phpt, bug31348.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = recursive(vm, &l(vec![1.into(), 2.into()]));
    let it = new_obj(vm, "RecursiveCachingIterator", vec![Value::Object(inner)]).unwrap();
    assert!(call(vm, &it, "getChildren", vec![]).unwrap().is_null());
    call(vm, &it, "rewind", vec![]).unwrap();
    assert!(call(vm, &it, "getChildren", vec![]).unwrap().is_null());
    assert!(
        call(vm, &it, "hasChildren", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
}

#[test]
fn recursive_caching_exceptions_in_children() {
    // iterator_047.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = iter_class(vm, "MyRecursiveArrayIterator", Kind::Recursive, None);
    let opts = Opts {
        echo_only: &["hasChildren", "getChildren"],
        parens: true,
        ..Opts::default()
    };
    let fail = opts.fail.clone();
    let data = l(vec![
        0.into(),
        l(vec![10.into()]),
        2.into(),
        l(vec![30.into()]),
        4.into(),
    ]);
    let inner = new_iter(&ce, &data, opts);
    let rci = new_obj(vm, "RecursiveCachingIterator", vec![Value::Object(inner)]).unwrap();
    for state in 0..4 {
        echo(vm, &format!("==={state}===")).unwrap();
        *fail.borrow_mut() = match state {
            1 => Some("hasChildren".into()),
            2 => Some("getChildren".into()),
            _ => None,
        };
        let it = new_obj(
            vm,
            "RecursiveIteratorIterator",
            vec![Value::Object(rci.clone())],
        )
        .unwrap();
        let r = foreach_with(vm, &it, true, |vm, k, v| {
            var_dump(vm, &k)?;
            var_dump(vm, &v)?;
            Ok(true)
        });
        if let Err(Flow::Exception(e)) = r {
            let msg = Vm::exception_message(&e);
            echo(
                vm,
                &format!("{}: State {state}: {msg} in /t.php", e.class().name),
            )
            .unwrap();
        }
    }
    assert_phpt(&t.output(), phpt!("iterator_047.phpt"));
}
