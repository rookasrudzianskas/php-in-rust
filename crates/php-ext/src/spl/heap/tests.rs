//! Tests ported from `heap_*.phpt`, `pqueue_*.phpt`, `SplHeap_*.phpt`,
//! `SplMinHeap_*`, `SplMaxHeap_*`, `SplPriorityQueue_*`, `spl_heap_*`,
//! `spl_pq_*`, `spl_pqueue_*` and the related `bug*`/`gh*` tests.

use std::cell::RefCell;

use php_runtime::class::{class_flags, member_flags};
use php_runtime::{Args, Flow, NativeFunction, ObjRef, PResult, PhpArray, Value, Vm};

use super::{foreach_iterator, is_corrupted};
use crate::spl::engine;
use crate::spl::fixedarray::test_helpers::{Prop, class_with_props, echo, parent_call};
use crate::spl::testutil::{assoc, call, entries, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn same(a: &Value, b: &Value) -> bool {
    a.identical(b)
}

fn heap(t: &mut TestVm, class: &str) -> ObjRef {
    new_obj(&mut t.vm, class, vec![]).unwrap()
}

fn m(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    call(&mut t.vm, o, name, args)
}

fn ok(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> Value {
    m(t, o, name, args).unwrap()
}

fn insert_all(t: &mut TestVm, h: &ObjRef, vals: &[i64]) {
    for v in vals {
        ok(t, h, "insert", vec![i(*v)]);
    }
}

fn pq_insert(t: &mut TestVm, h: &ObjRef, data: Value, priority: Value) {
    ok(t, h, "insert", vec![data, priority]);
}

fn text(vm: &mut Vm, v: &Value) -> String {
    vm.to_php_string(v).unwrap().to_string_lossy().into_owned()
}

fn count(t: &mut TestVm, o: &ObjRef) -> PResult<i64> {
    let f = o.class().handlers.count_elements.unwrap();
    Ok(f(&mut t.vm, o)?.unwrap())
}

const CORRUPTED: &str = "Heap is corrupted, heap properties are no longer ensured.";
const LOCKED: &str = "Heap cannot be changed when it is already being modified.";

fn runtime(msg: &str) -> (String, String) {
    ("RuntimeException".into(), msg.into())
}

/// `foreach ($h as $k => $v)` through the `get_iterator` handler.
fn foreach(t: &mut TestVm, o: &ObjRef, mut body: impl FnMut(&mut Vm, Value, Value)) -> PResult<()> {
    let it = foreach_iterator(&mut t.vm, o, false)?.expect("internal iterator");
    let vm = &mut t.vm;
    it.rewind(vm)?;
    while it.valid(vm)? {
        let Some(v) = it.current(vm)? else { break };
        let k = it.key(vm)?.unwrap();
        body(vm, k, v);
        it.move_forward(vm)?;
    }
    Ok(())
}

fn pairs(t: &mut TestVm, o: &ObjRef) -> Vec<String> {
    let mut out = Vec::new();
    foreach(t, o, |vm, k, v| {
        let line = format!("{} => {}", text(vm, &k), text(vm, &v));
        out.push(line);
    })
    .unwrap();
    out
}

fn debug(t: &mut TestVm, o: &ObjRef) -> Vec<(String, Value)> {
    let r = ok(t, o, "__debugInfo", vec![]);
    entries(&r)
}

fn heap_values(t: &mut TestVm, o: &ObjRef) -> Value {
    debug(t, o).last().unwrap().1.clone()
}

fn ints(v: &[i64]) -> Value {
    list(v.iter().map(|n| i(*n)).collect())
}

/// `['data' => $d, 'priority' => $p]`.
fn both(d: Value, p: Value) -> Value {
    assoc(vec![(skey("data"), d), (skey("priority"), p)])
}

fn shuffled(n: i64) -> Vec<i64> {
    // A fixed permutation (the order must not matter).
    let mut v: Vec<i64> = (0..n).map(|k| (k * 37 + 11) % n).collect();
    v.dedup();
    v
}

// ---------------------------------------------------------------------------
// Class shape
// ---------------------------------------------------------------------------

#[test]
fn class_shape() {
    let mut t = TestVm::new();
    let h = t.vm.find_class(b"SplHeap").unwrap();
    assert!(h.has_flag(class_flags::ABSTRACT));
    let cmp = h.find_method(b"compare").unwrap();
    assert!(cmp.has_flag(member_flags::ABSTRACT));
    assert_eq!(cmp.visibility(), member_flags::PROTECTED);
    let min = t.vm.find_class(b"SplMinHeap").unwrap();
    let cmp = min.find_method(b"compare").unwrap();
    assert_eq!(cmp.visibility(), member_flags::PROTECTED);
    assert!(!cmp.has_flag(member_flags::ABSTRACT));
    assert_eq!(
        min.find_method(b"count").unwrap().display_name(),
        "SplHeap::count"
    );
    for (name, v) in [("EXTR_BOTH", 3), ("EXTR_PRIORITY", 2), ("EXTR_DATA", 1)] {
        let c =
            t.vm.class_constant("SplPriorityQueue", name)
                .unwrap()
                .unwrap();
        assert!(same(&c, &i(v)));
    }
    let r = t
        .call("class_implements", vec![s("SplPriorityQueue")])
        .unwrap();
    let names: Vec<String> = entries(&r).into_iter().map(|(k, _)| k).collect();
    assert_eq!(names, ["Iterator", "Traversable", "Countable"]);
    let pq = t.vm.find_class(b"SplPriorityQueue").unwrap();
    assert!(pq.parent.is_none());
    assert_eq!(
        pq.find_method(b"count").unwrap().display_name(),
        "SplPriorityQueue::count"
    );
}

// ---------------------------------------------------------------------------
// Basic operations
// ---------------------------------------------------------------------------

#[test]
fn max_heap_operations() {
    // heap_001.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMaxHeap");
    assert_eq!(
        thrown(m(&mut t, &h, "extract", vec![])),
        runtime("Can't extract from an empty heap")
    );
    insert_all(&mut t, &h, &[1, 2, 3, 3, 3]);
    assert!(same(&ok(&mut t, &h, "count", vec![]), &i(5)));
    for want in [3, 3, 3, 2, 1] {
        assert!(same(&ok(&mut t, &h, "extract", vec![]), &i(want)));
    }
    assert!(same(&ok(&mut t, &h, "count", vec![]), &i(0)));
    ok(&mut t, &h, "insert", vec![i(4)]);
    let clone = h.class().handlers.clone_obj.unwrap();
    let h2 = clone(&mut t.vm, &h).unwrap();
    assert!(same(&ok(&mut t, &h, "extract", vec![]), &i(4)));
    assert!(same(&ok(&mut t, &h2, "extract", vec![]), &i(4)));
}

#[test]
fn min_heap_operations() {
    // heap_002.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMinHeap");
    assert_eq!(
        thrown(m(&mut t, &h, "extract", vec![])).1,
        "Can't extract from an empty heap"
    );
    insert_all(&mut t, &h, &[1, 2, 3, 3, 3]);
    assert!(same(&ok(&mut t, &h, "count", vec![]), &i(5)));
    for want in [1, 2, 3, 3, 3] {
        assert!(same(&ok(&mut t, &h, "extract", vec![]), &i(want)));
    }
    assert_eq!(count(&mut t, &h).unwrap(), 0);
    ok(&mut t, &h, "insert", vec![i(4)]);
    assert!(same(&ok(&mut t, &h, "extract", vec![]), &i(4)));
}

#[test]
fn empty_heaps() {
    // heap_current_variation_001.phpt, heap_it_current_empty.phpt, heap_top_variation_003.phpt,
    // SplHeap_isEmpty.phpt, spl_heap_isempty.phpt, pqueue_current_error.phpt,
    // spl_pq_top_error_empty.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMinHeap");
    assert!(ok(&mut t, &h, "current", vec![]).is_null());
    assert!(pairs(&mut t, &h).is_empty());
    assert_eq!(
        thrown(m(&mut t, &h, "top", vec![])),
        runtime("Can't peek at an empty heap")
    );
    let h = heap(&mut t, "SplMaxHeap");
    assert!(same(&ok(&mut t, &h, "isEmpty", vec![]), &Value::Bool(true)));
    ok(&mut t, &h, "insert", vec![i(2)]);
    assert!(same(
        &ok(&mut t, &h, "isEmpty", vec![]),
        &Value::Bool(false)
    ));
    ok(&mut t, &h, "extract", vec![]);
    assert!(same(&ok(&mut t, &h, "isEmpty", vec![]), &Value::Bool(true)));
    let pq = heap(&mut t, "SplPriorityQueue");
    assert!(ok(&mut t, &pq, "current", vec![]).is_null());
    assert_eq!(
        thrown(m(&mut t, &pq, "top", vec![])),
        runtime("Can't peek at an empty heap")
    );
    assert_eq!(
        thrown(m(&mut t, &pq, "extract", vec![])),
        runtime("Can't extract from an empty heap")
    );
}

#[test]
fn iteration_extracts() {
    // heap_005.phpt, heap_006.phpt, bug62073.phpt
    let mut t = TestVm::new();
    for (class, first, last) in [("SplMinHeap", 1, 100), ("SplMaxHeap", 100, 1)] {
        let h = heap(&mut t, class);
        let input: Vec<i64> = shuffled(100).into_iter().map(|v| v + 1).collect();
        insert_all(&mut t, &h, &input);
        let out = pairs(&mut t, &h);
        assert_eq!(out.len(), 100);
        assert_eq!(out[0], format!("99 => {first}"));
        assert_eq!(out[99], format!("0 => {last}"));
        assert_eq!(count(&mut t, &h).unwrap(), 0);
    }
    let h = heap(&mut t, "SplMaxHeap");
    ok(&mut t, &h, "insert", vec![i(42)]);
    let it = foreach_iterator(&mut t.vm, &h, false).unwrap().unwrap();
    assert!(same(&it.key(&mut t.vm).unwrap().unwrap(), &i(0)));
    assert!(same(&it.current(&mut t.vm).unwrap().unwrap(), &i(42)));
    assert!(same(&ok(&mut t, &h, "key", vec![]), &i(0)));
    assert!(same(&ok(&mut t, &h, "current", vec![]), &i(42)));
}

#[test]
fn iteration_through_methods() {
    // heap_007.phpt, pqueue_003.phpt
    let mut t = TestVm::new();
    for class in ["SplMaxHeap", "SplPriorityQueue"] {
        let h = heap(&mut t, class);
        for v in [1, 5, 0, 4] {
            if class == "SplPriorityQueue" {
                pq_insert(&mut t, &h, i(v), i(v));
            } else {
                ok(&mut t, &h, "insert", vec![i(v)]);
            }
        }
        ok(&mut t, &h, "rewind", vec![]);
        assert_eq!(count(&mut t, &h).unwrap(), 4);
        assert!(same(&ok(&mut t, &h, "count", vec![]), &i(4)));
        let mut out = Vec::new();
        while ok(&mut t, &h, "valid", vec![]).as_bool().unwrap() {
            let k = ok(&mut t, &h, "key", vec![]);
            let v = ok(&mut t, &h, "current", vec![]);
            out.push(format!("{}=>{}", text(&mut t.vm, &k), text(&mut t.vm, &v)));
            ok(&mut t, &h, "next", vec![]);
        }
        assert_eq!(out, ["3=>5", "2=>4", "1=>1", "0=>0"], "{class}");
    }
}

#[test]
fn debug_info_layout() {
    // heap_008.phpt, pqueue_004.phpt, bug78436.phpt, heap_012.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMaxHeap");
    insert_all(&mut t, &h, &[1, 5, 0, 4]);
    let d = debug(&mut t, &h);
    let keys: Vec<&str> = d.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "\0SplHeap\0flags",
            "\0SplHeap\0isCorrupted",
            "\0SplHeap\0heap"
        ]
    );
    assert!(same(&d[0].1, &i(0)));
    assert!(same(&d[1].1, &Value::Bool(false)));
    assert!(same(&d[2].1, &ints(&[5, 4, 0, 1])));

    let pq = heap(&mut t, "SplPriorityQueue");
    for (data, p) in [("a", 0), ("b", 1), ("c", 5), ("d", -2)] {
        pq_insert(&mut t, &pq, s(data), i(p));
    }
    let d = debug(&mut t, &pq);
    let keys: Vec<&str> = d.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "\0SplPriorityQueue\0flags",
            "\0SplPriorityQueue\0isCorrupted",
            "\0SplPriorityQueue\0heap"
        ]
    );
    assert!(same(&d[0].1, &i(1)));
    let want = list(vec![
        both(s("c"), i(5)),
        both(s("a"), i(0)),
        both(s("b"), i(1)),
        both(s("d"), i(-2)),
    ]);
    assert!(same(&d[2].1, &want));

    let pq = heap(&mut t, "SplPriorityQueue");
    let std = Value::Object(new_obj(&mut t.vm, "stdClass", vec![]).unwrap());
    pq_insert(&mut t, &pq, std.clone(), i(1));
    for _ in 0..2 {
        assert!(same(
            &heap_values(&mut t, &pq),
            &list(vec![both(std.clone(), i(1))])
        ));
    }

    let a = heap(&mut t, "SplMaxHeap");
    ok(&mut t, &a, "insert", vec![Value::Object(a.clone())]);
    let hv = heap_values(&mut t, &a);
    assert!(
        matches!(hv.as_array().unwrap().get_int(0), Some(Value::Object(o)) if ObjRef::ptr_eq(o, &a))
    );
    ok(&mut t, &a, "extract", vec![]);
}

#[test]
fn foreach_by_reference_is_an_error() {
    // heap_009.phpt
    let mut t = TestVm::new();
    for class in ["SplMinHeap", "SplMaxHeap", "SplPriorityQueue"] {
        for filled in [false, true] {
            let h = heap(&mut t, class);
            if filled {
                let args = if class == "SplPriorityQueue" {
                    vec![i(1), i(2)]
                } else {
                    vec![i(1)]
                };
                ok(&mut t, &h, "insert", args);
            }
            let e = foreach_iterator(&mut t.vm, &h, true).map(|_| Value::Null);
            assert_eq!(
                thrown(e),
                (
                    "Error".into(),
                    "An iterator cannot be used with foreach by reference".into()
                )
            );
        }
    }
}

// ---------------------------------------------------------------------------
// User compare() / count()
// ---------------------------------------------------------------------------

fn sign_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (a, b) = (args.get(0).clone(), args.get(1).clone());
    Ok(i(vm.compare(&a, &b)? as i64))
}
static SIGN_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: sign_compare,
};

fn throwing_compare(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "foo"))
}
static THROWING_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: throwing_compare,
};

#[test]
fn user_compare() {
    // heap_003.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "myHeap", "SplHeap", &[&SIGN_COMPARE]);
    let h = heap(&mut t, "myHeap");
    insert_all(&mut t, &h, &shuffled(11));
    let mut out = Vec::new();
    foreach(&mut t, &h, |vm, _, v| out.push(text(vm, &v))).unwrap();
    let want: Vec<String> = (0..=10).rev().map(|n| n.to_string()).collect();
    assert_eq!(out, want);
}

#[test]
fn compare_exceptions_corrupt_the_heap() {
    // heap_004.phpt, pqueue_002.phpt, heap_top_variation_002.phpt
    for (class, parent) in [("myHeap", "SplHeap"), ("myPQueue", "SplPriorityQueue")] {
        let mut t = TestVm::new();
        user_class(&mut t.vm, class, parent, &[&THROWING_COMPARE]);
        let h = heap(&mut t, class);
        let pq = parent == "SplPriorityQueue";
        let ins = |t: &mut TestVm, v: i64| {
            let args = if pq { vec![i(v), i(1)] } else { vec![i(v)] };
            m(t, &h, "insert", args)
        };
        ins(&mut t, 1).unwrap();
        assert_eq!(thrown(ins(&mut t, 2)), ("Exception".into(), "foo".into()));
        assert!(is_corrupted(&h));
        assert_eq!(thrown(ins(&mut t, 4)), runtime(CORRUPTED));
        for _ in 0..2 {
            assert_eq!(thrown(m(&mut t, &h, "extract", vec![])), runtime(CORRUPTED));
        }
        assert_eq!(thrown(m(&mut t, &h, "top", vec![])), runtime(CORRUPTED));
        ok(&mut t, &h, "recoverFromCorruption", vec![]);
        assert!(same(&ok(&mut t, &h, "extract", vec![]), &i(1)));
        assert!(same(&ok(&mut t, &h, "extract", vec![]), &i(2)));
    }
}

fn guarded_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if this
        .get_prop(b"allow_compare")
        .is_some_and(|v| v.as_bool() == Some(true))
    {
        let (a, b) = (args.get(0).clone(), args.get(1).clone());
        return Ok(i(vm.compare(&a, &b)? as i64));
    }
    Err(vm.throw_error("Exception", "Compare exception"))
}
static GUARDED_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($v1, $v2): int {}",
    handler: guarded_compare,
};

#[test]
fn corruption_on_extract() {
    // heap_corruption.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "myHeap",
        "SplHeap",
        0,
        vec![Prop::public("allow_compare", Value::Bool(true))],
        &[&GUARDED_COMPARE],
    );
    let h = heap(&mut t, "myHeap");
    insert_all(&mut t, &h, &[1, 2, 3, 4]);
    h.set_prop(b"allow_compare", Value::Bool(false));
    assert!(same(
        &ok(&mut t, &h, "isCorrupted", vec![]),
        &Value::Bool(false)
    ));
    assert_eq!(
        thrown(m(&mut t, &h, "extract", vec![])),
        ("Exception".into(), "Compare exception".into())
    );
    assert_eq!(thrown(m(&mut t, &h, "top", vec![])), runtime(CORRUPTED));
    assert!(same(
        &ok(&mut t, &h, "isCorrupted", vec![]),
        &Value::Bool(true)
    ));
    ok(&mut t, &h, "recoverFromCorruption", vec![]);
    assert!(same(
        &ok(&mut t, &h, "isCorrupted", vec![]),
        &Value::Bool(false)
    ));
}

fn failing_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if this
        .get_prop(b"fail")
        .is_some_and(|v| v.as_bool() == Some(true))
    {
        let e = vm.create_exception("Exception", "Corrupting heap", 99);
        return Err(Flow::Exception(e));
    }
    Ok(i(0))
}
static FAILING_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($val1, $val2): int {}",
    handler: failing_compare,
};

#[test]
fn corruption_during_iteration() {
    // spl_heap_iteration_error.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "ext_heap",
        "SplMaxHeap",
        0,
        vec![Prop::public("fail", Value::Bool(false))],
        &[&FAILING_COMPARE],
    );
    let h = heap(&mut t, "ext_heap");
    for v in ["foobar", "foobar1", "foobar2"] {
        ok(&mut t, &h, "insert", vec![list(vec![s(v)])]);
    }
    h.set_prop(b"fail", Value::Bool(true));
    let r = foreach(&mut t, &h, |_, _, _| {});
    let Err(Flow::Exception(e)) = r else {
        panic!("expected an exception")
    };
    assert_eq!(e.get_prop(b"code").and_then(|c| c.as_int()), Some(99));
    let d = debug(&mut t, &h);
    let keys: Vec<&str> = d.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "fail",
            "\0SplHeap\0flags",
            "\0SplHeap\0isCorrupted",
            "\0SplHeap\0heap"
        ]
    );
    assert!(same(&d[2].1, &Value::Bool(true)));
    assert!(same(
        &d[3].1,
        &list(vec![list(vec![s("foobar2")]), list(vec![s("foobar1")])])
    ));
    // The iterator's current() validates the heap.
    let it = super::get_iterator(&mut t.vm, &h, false).unwrap();
    let e = it.current(&mut t.vm).map(|_| Value::Null);
    assert_eq!(thrown(e), runtime(CORRUPTED));
}

fn negative_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let n = parent_call(vm, &this, "SplHeap", "count", vec![])?;
    Ok(i(-n.as_int().unwrap()))
}
static NEGATIVE_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: negative_count,
};

fn throwing_count(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "Cause count to fail"))
}
static THROWING_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: throwing_count,
};

fn lt_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (a, b) = (args.get(0).clone(), args.get(1).clone());
    Ok(Value::Bool(vm.is_smaller(&a, &b)?))
}
static LT_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: lt_compare,
};

#[test]
fn count_overrides() {
    // heap_010.phpt, spl_heap_count_basic.phpt, spl_heap_is_empty_basic.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMaxHeap");
    insert_all(&mut t, &h, &[1, 2]);
    assert!(same(
        &t.call("count", vec![Value::Object(h)]).unwrap(),
        &i(2)
    ));
    user_class(&mut t.vm, "SplMaxHeap2", "SplMaxHeap", &[&NEGATIVE_COUNT]);
    let h = heap(&mut t, "SplMaxHeap2");
    insert_all(&mut t, &h, &[1, 2]);
    assert!(same(
        &t.call("count", vec![Value::Object(h.clone())]).unwrap(),
        &i(-2)
    ));
    // isEmpty() ignores the override.
    assert!(same(
        &ok(&mut t, &h, "isEmpty", vec![]),
        &Value::Bool(false)
    ));

    user_class(
        &mut t.vm,
        "MyHeap",
        "SplHeap",
        &[&LT_COMPARE, &THROWING_COUNT],
    );
    let h = heap(&mut t, "MyHeap");
    ok(&mut t, &h, "insert", vec![i(1)]);
    assert_eq!(
        thrown(t.call("count", vec![Value::Object(h.clone())])),
        ("Exception".into(), "Cause count to fail".into())
    );
    assert!(same(
        &ok(&mut t, &h, "isEmpty", vec![]),
        &Value::Bool(false)
    ));
    ok(&mut t, &h, "extract", vec![]);
    assert!(same(&ok(&mut t, &h, "isEmpty", vec![]), &Value::Bool(true)));
}

fn negated_min_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let r = parent_call(vm, &this, "SplMinHeap", "compare", args.values().to_vec())?;
    Ok(i(-r.as_int().unwrap()))
}
static NEGATED_MIN_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: negated_min_compare,
};
fn negated_max_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let r = parent_call(vm, &this, "SplMaxHeap", "compare", args.values().to_vec())?;
    Ok(i(-r.as_int().unwrap()))
}
static NEGATED_MAX_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: negated_max_compare,
};

#[test]
fn compare_override_calling_parent() {
    // heap_011.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "SplMinHeap2",
        "SplMinHeap",
        &[&NEGATED_MIN_COMPARE],
    );
    let h = heap(&mut t, "SplMinHeap2");
    insert_all(&mut t, &h, &[1, 6, 5, 2]);
    assert!(same(&ok(&mut t, &h, "top", vec![]), &i(6)));
    user_class(
        &mut t.vm,
        "SplMaxHeap2",
        "SplMaxHeap",
        &[&NEGATED_MAX_COMPARE],
    );
    let h = heap(&mut t, "SplMaxHeap2");
    insert_all(&mut t, &h, &[1, 6, 5, 2]);
    assert!(same(&ok(&mut t, &h, "top", vec![]), &i(1)));
}

fn undefined_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    parent_call(vm, &this, "SplMinHeap", "compare", args.values().to_vec())?;
    Err(vm.throw_error("Error", "Call to undefined method SplMinHeap::notexist()"))
}
static UNDEFINED_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: undefined_compare,
};

#[test]
fn errors_from_compare_propagate() {
    // bug69737.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "SplMinHeap1",
        "SplMinHeap",
        &[&UNDEFINED_COMPARE],
    );
    let h = heap(&mut t, "SplMinHeap1");
    ok(&mut t, &h, "insert", vec![i(1)]);
    assert_eq!(
        thrown(m(&mut t, &h, "insert", vec![i(6)])),
        (
            "Error".into(),
            "Call to undefined method SplMinHeap::notexist()".into()
        )
    );
    assert!(is_corrupted(&h));
}

// ---------------------------------------------------------------------------
// Re-entrancy: the write lock
// ---------------------------------------------------------------------------

thread_local! {
    static TARGET: RefCell<Option<ObjRef>> = const { RefCell::new(None) };
}

fn target() -> ObjRef {
    TARGET.with(|t| t.borrow().clone()).expect("target heap")
}

fn set_target(o: Option<ObjRef>) {
    TARGET.with(|t| *t.borrow_mut() = o);
}

fn report(vm: &mut Vm, r: PResult<Value>) -> PResult<()> {
    match r {
        Ok(_) => Ok(()),
        Err(Flow::Exception(e)) => {
            let msg = format!("{}: {}\n", e.class().name, Vm::exception_message(&e));
            echo(vm, &msg)
        }
        Err(e) => Err(e),
    }
}

fn reentrant_to_string(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let h = target();
    let r = call(vm, &h, "extract", vec![]);
    report(vm, r)?;
    let r = call(vm, &h, "insert", vec![i(1)]);
    report(vm, r)?;
    let top = call(vm, &h, "top", vec![])?;
    let top = vm.to_php_string(&top)?;
    echo(vm, &format!("{top}\n"))?;
    Ok(s("0"))
}
static REENTRANT_TO_STRING: NativeFunction = NativeFunction {
    sig: "public function __toString(): string {}",
    handler: reentrant_to_string,
};

#[test]
fn modification_during_comparison_is_refused() {
    // gh16337.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "C", "stdClass", &[&REENTRANT_TO_STRING]);
    let h = heap(&mut t, "SplMinHeap");
    set_target(Some(h.clone()));
    for n in 0..100 {
        ok(&mut t, &h, "insert", vec![s(&n.to_string())]);
    }
    let c = new_obj(&mut t.vm, "C", vec![]).unwrap();
    ok(&mut t, &h, "insert", vec![Value::Object(c)]);
    let round = format!("RuntimeException: {LOCKED}\nRuntimeException: {LOCKED}\n0\n");
    assert_eq!(t.output(), round.repeat(6));
    assert!(!is_corrupted(&h));
    assert_eq!(count(&mut t, &h).unwrap(), 101);
    set_target(None);
}

fn evil_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    if this
        .get_prop(b"did")
        .is_some_and(|v| v.as_bool() == Some(false))
    {
        this.set_prop(b"did", Value::Bool(true));
        if !call(vm, &this, "isEmpty", vec![])?.as_bool().unwrap() {
            call(vm, &this, "next", vec![])?;
        }
    }
    parent_call(
        vm,
        &this,
        "SplPriorityQueue",
        "compare",
        args.values().to_vec(),
    )
}
static EVIL_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare(mixed $p1, mixed $p2): int {}",
    handler: evil_compare,
};

#[test]
fn next_respects_the_write_lock() {
    // heap_next_write_lock.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "EvilPQ",
        "SplPriorityQueue",
        0,
        vec![Prop {
            name: "did",
            flags: member_flags::PRIVATE,
            default: Value::Bool(false),
            ty: Some("bool"),
        }],
        &[&EVIL_COMPARE],
    );
    let q = heap(&mut t, "EvilPQ");
    let mut error = None;
    for n in 0..200 {
        if let Err(e) = m(&mut t, &q, "insert", vec![s(&format!("d{n}")), i(100 - n)]) {
            error = Some(e);
            break;
        }
    }
    assert_eq!(thrown(Err(error.unwrap())), runtime(LOCKED));
}

fn unserializing_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let first = TARGET.with(|t| t.borrow_mut().take());
    if let Some(Value::Array(data)) = first.map(|_| SERIALIZED.with(|s| s.borrow().clone())) {
        call(vm, &this, "__unserialize", vec![Value::Array(data)])?;
    }
    let (a, b) = (args.get(0).clone(), args.get(1).clone());
    Ok(i(vm.compare(&a, &b)? as i64))
}
static UNSERIALIZING_COMPARE: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: unserializing_compare,
};

thread_local! {
    static SERIALIZED: RefCell<Value> = const { RefCell::new(Value::Null) };
}

#[test]
fn unserialize_during_modification_is_refused() {
    // heap_unserialize_under_corruption_or_modification.phpt
    let mut t = TestVm::new();
    let plain = heap(&mut t, "SplMaxHeap");
    ok(&mut t, &plain, "insert", vec![i(1)]);
    let data = ok(&mut t, &plain, "__serialize", vec![]);
    SERIALIZED.with(|s| *s.borrow_mut() = data);
    user_class(&mut t.vm, "MyHeap", "SplMaxHeap", &[&UNSERIALIZING_COMPARE]);
    let h = heap(&mut t, "MyHeap");
    set_target(Some(h.clone()));
    ok(&mut t, &h, "insert", vec![i(0)]);
    assert_eq!(thrown(m(&mut t, &h, "insert", vec![i(2)])), runtime(LOCKED));
    // A corrupted heap refuses __unserialize() too.
    let data = SERIALIZED.with(|s| s.borrow().clone());
    assert_eq!(
        thrown(m(&mut t, &h, "__unserialize", vec![data])),
        runtime(CORRUPTED)
    );
}

// ---------------------------------------------------------------------------
// SplPriorityQueue
// ---------------------------------------------------------------------------

#[test]
fn priority_queue_extract_flags() {
    // pqueue_001.phpt
    let mut t = TestVm::new();
    let fill = |t: &mut TestVm, flags: Option<i64>| {
        let pq = heap(t, "SplPriorityQueue");
        if let Some(f) = flags {
            ok(t, &pq, "setExtractFlags", vec![i(f)]);
        }
        for (d, p) in [("a", 1), ("b", 2), ("c", 0)] {
            pq_insert(t, &pq, s(d), i(p));
        }
        pq
    };
    let pq = fill(&mut t, None);
    assert_eq!(pairs(&mut t, &pq), ["2 => b", "1 => a", "0 => c"]);
    let pq = fill(&mut t, Some(3));
    let mut out = Vec::new();
    foreach(&mut t, &pq, |_, _, v| out.push(v)).unwrap();
    assert!(same(
        &list(out),
        &list(vec![
            both(s("b"), i(2)),
            both(s("a"), i(1)),
            both(s("c"), i(0))
        ])
    ));
    let pq = fill(&mut t, Some(1));
    assert_eq!(pairs(&mut t, &pq), ["2 => b", "1 => a", "0 => c"]);
    let pq = fill(&mut t, Some(2));
    assert_eq!(pairs(&mut t, &pq), ["2 => 2", "1 => 1", "0 => 0"]);
}

#[test]
fn priority_queue_top_and_flags() {
    // spl_pq_top_basic.phpt, SplPriorityQueue_setExtractFlags_zero.phpt, pqueue_compare_basic.phpt
    let mut t = TestVm::new();
    let pq = heap(&mut t, "SplPriorityQueue");
    assert!(same(&ok(&mut t, &pq, "getExtractFlags", vec![]), &i(1)));
    for (d, p) in [("a", 1), ("b", 2), ("c", 0)] {
        pq_insert(&mut t, &pq, s(d), i(p));
    }
    assert!(same(&ok(&mut t, &pq, "top", vec![]), &s("b")));
    assert!(same(&ok(&mut t, &pq, "setExtractFlags", vec![i(2)]), &i(2)));
    assert!(same(&ok(&mut t, &pq, "top", vec![]), &i(2)));
    ok(&mut t, &pq, "setExtractFlags", vec![i(3)]);
    assert!(same(&ok(&mut t, &pq, "top", vec![]), &both(s("b"), i(2))));
    ok(&mut t, &pq, "setExtractFlags", vec![i(1)]);
    assert!(same(&ok(&mut t, &pq, "top", vec![]), &s("b")));
    assert!(same(&ok(&mut t, &pq, "setExtractFlags", vec![i(7)]), &i(3)));
    assert_eq!(
        thrown(m(&mut t, &pq, "setExtractFlags", vec![i(0)])),
        runtime("Must specify at least one extract flag")
    );
    assert_eq!(
        thrown(m(&mut t, &pq, "setExtractFlags", vec![i(4)])),
        runtime("Must specify at least one extract flag")
    );
    let h = heap(&mut t, "SplPriorityQueue");
    assert!(
        ok(&mut t, &h, "compare", vec![i(4), i(5)])
            .as_int()
            .unwrap()
            < 0
    );
    assert!(same(&ok(&mut t, &h, "compare", vec![i(5), i(5)]), &i(0)));
    assert!(
        ok(&mut t, &h, "compare", vec![i(5), i(4)])
            .as_int()
            .unwrap()
            > 0
    );
}

fn throwing_on_two(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let b = args.get(1).clone();
    if vm.is_equal(&b, &i(2))? {
        return Err(vm.throw_error("Exception", "ignore me"));
    }
    let this = engine::this(args);
    parent_call(
        vm,
        &this,
        "SplPriorityQueue",
        "compare",
        args.values().to_vec(),
    )
}
static THROWING_ON_TWO: NativeFunction = NativeFunction {
    sig: "public function compare($a, $b): int {}",
    handler: throwing_on_two,
};

#[test]
fn priority_queue_corruption() {
    // spl_pq_top_error_corrupt.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "myPriorityQueue",
        "SplPriorityQueue",
        &[&THROWING_ON_TWO],
    );
    let pq = heap(&mut t, "myPriorityQueue");
    pq_insert(&mut t, &pq, s("a"), i(1));
    let _ = m(&mut t, &pq, "insert", vec![s("b"), i(2)]);
    assert_eq!(thrown(m(&mut t, &pq, "top", vec![])), runtime(CORRUPTED));
}

#[test]
fn priority_queue_mixed_priorities() {
    // The int/float specialisation falls back to zend_compare on mixed types.
    let mut t = TestVm::new();
    let pq = heap(&mut t, "SplPriorityQueue");
    for (d, p) in [
        ("a", i(1)),
        ("b", Value::Float(2.5)),
        ("c", s("3")),
        ("d", i(0)),
    ] {
        pq_insert(&mut t, &pq, s(d), p);
    }
    let mut out = Vec::new();
    while !ok(&mut t, &pq, "isEmpty", vec![]).as_bool().unwrap() {
        let v = ok(&mut t, &pq, "extract", vec![]);
        out.push(text(&mut t.vm, &v));
    }
    assert_eq!(out, ["c", "b", "a", "d"]);
    let pq = heap(&mut t, "SplPriorityQueue");
    for (d, p) in [
        ("x", Value::Float(1.5)),
        ("y", Value::Float(-1.0)),
        ("z", Value::Float(9.0)),
    ] {
        pq_insert(&mut t, &pq, s(d), p);
    }
    assert!(same(&ok(&mut t, &pq, "extract", vec![]), &s("z")));
    assert!(same(&ok(&mut t, &pq, "extract", vec![]), &s("x")));
}

#[test]
fn clone_priority_queue() {
    // bug70573.phpt, spl_pqueue_gc.phpt
    let mut t = TestVm::new();
    let q1 = heap(&mut t, "SplPriorityQueue");
    pq_insert(&mut t, &q1, list(vec![i(1)]), i(1));
    pq_insert(&mut t, &q1, list(vec![i(1)]), i(2));
    let clone = q1.class().handlers.clone_obj.unwrap();
    let q2 = clone(&mut t.vm, &q1).unwrap();
    assert_eq!(count(&mut t, &q2).unwrap(), 2);
    ok(&mut t, &q2, "extract", vec![]);
    assert_eq!(count(&mut t, &q1).unwrap(), 2);

    let pq = heap(&mut t, "SplPriorityQueue");
    pq_insert(&mut t, &pq, Value::Object(pq.clone()), i(1));
    let pq2 = heap(&mut t, "SplPriorityQueue");
    pq_insert(&mut t, &pq2, i(1), Value::Object(pq.clone()));
    ok(&mut t, &pq, "extract", vec![]);
}

// ---------------------------------------------------------------------------
// __debugInfo overrides
// ---------------------------------------------------------------------------

fn my_heap_debug_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let parent = parent_call(vm, &this, "SplHeap", "__debugInfo", vec![])?;
    let mut out = PhpArray::new();
    out.insert_str("child", i(42));
    out.insert_str("parent", i(parent.as_array().unwrap().len() as i64));
    Ok(Value::array(out))
}
static MY_HEAP_DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: my_heap_debug_info,
};
fn my_pq_debug_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let parent = parent_call(vm, &this, "SplPriorityQueue", "__debugInfo", vec![])?;
    let mut out = PhpArray::new();
    out.insert_str("child", i(42));
    out.insert_str("parent", i(parent.as_array().unwrap().len() as i64));
    Ok(Value::array(out))
}
static MY_PQ_DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: my_pq_debug_info,
};

#[test]
fn subclass_debug_info() {
    // bug69264.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "MyMaxHeap", "SplMaxHeap", &[&MY_HEAP_DEBUG_INFO]);
    user_class(
        &mut t.vm,
        "MyPriorityQueue",
        "SplPriorityQueue",
        &[&MY_PQ_DEBUG_INFO],
    );
    for class in ["MyMaxHeap", "MyPriorityQueue"] {
        let h = heap(&mut t, class);
        assert!(h.class().handlers.get_debug_info.is_none());
        let d = debug(&mut t, &h);
        assert_eq!(d.len(), 2);
        assert!(same(&d[0].1, &i(42)));
        assert!(same(&d[1].1, &i(3)), "{class}");
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn ser(t: &mut TestVm, v: &ObjRef) -> PResult<String> {
    let r = t.call("serialize", vec![Value::Object(v.clone())])?;
    Ok(r.as_str().unwrap().to_string_lossy().into_owned())
}

fn unser(t: &mut TestVm, data: &str) -> ObjRef {
    let r = t.call("unserialize", vec![s(data)]).unwrap();
    r.as_object().unwrap().clone()
}

#[test]
fn serialize_format() {
    // SplHeap_serialize_format.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMaxHeap");
    let s0 = ser(&mut t, &h).unwrap();
    assert_eq!(
        s0,
        "O:10:\"SplMaxHeap\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:0;s:13:\"heap_elements\";a:0:{}}}"
    );
    let u = unser(&mut t, &s0);
    assert!(same(&heap_values(&mut t, &u), &Value::empty_array()));

    let h = heap(&mut t, "SplMaxHeap");
    insert_all(&mut t, &h, &[100, 50, 200]);
    let s1 = ser(&mut t, &h).unwrap();
    assert_eq!(
        s1,
        "O:10:\"SplMaxHeap\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:0;s:13:\"heap_elements\";a:3:{i:0;i:200;i:1;i:50;i:2;i:100;}}}"
    );
    let u = unser(&mut t, &s1);
    assert!(same(&heap_values(&mut t, &u), &ints(&[200, 50, 100])));

    let h = heap(&mut t, "SplMinHeap");
    insert_all(&mut t, &h, &[30, 10, 20]);
    let s2 = ser(&mut t, &h).unwrap();
    assert_eq!(
        s2,
        "O:10:\"SplMinHeap\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:0;s:13:\"heap_elements\";a:3:{i:0;i:10;i:1;i:30;i:2;i:20;}}}"
    );
    let u = unser(&mut t, &s2);
    assert!(same(&heap_values(&mut t, &u), &ints(&[10, 30, 20])));

    let pq = heap(&mut t, "SplPriorityQueue");
    let s3 = ser(&mut t, &pq).unwrap();
    assert_eq!(
        s3,
        "O:16:\"SplPriorityQueue\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:1;s:13:\"heap_elements\";a:0:{}}}"
    );
    let u = unser(&mut t, &s3);
    assert!(same(&debug(&mut t, &u)[0].1, &i(1)));

    let pq = heap(&mut t, "SplPriorityQueue");
    pq_insert(&mut t, &pq, s("low"), i(1));
    pq_insert(&mut t, &pq, s("high"), i(10));
    pq_insert(&mut t, &pq, s("medium"), i(5));
    ok(&mut t, &pq, "setExtractFlags", vec![i(3)]);
    let s4 = ser(&mut t, &pq).unwrap();
    assert_eq!(
        s4,
        "O:16:\"SplPriorityQueue\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:3;s:13:\"heap_elements\";a:3:{i:0;a:2:{s:4:\"data\";s:4:\"high\";s:8:\"priority\";i:10;}i:1;a:2:{s:4:\"data\";s:3:\"low\";s:8:\"priority\";i:1;}i:2;a:2:{s:4:\"data\";s:6:\"medium\";s:8:\"priority\";i:5;}}}}"
    );
    let u = unser(&mut t, &s4);
    let d = debug(&mut t, &u);
    assert!(same(&d[0].1, &i(3)));
    assert!(same(
        &d[2].1,
        &list(vec![
            both(s("high"), i(10)),
            both(s("low"), i(1)),
            both(s("medium"), i(5))
        ])
    ));
    assert_eq!(t.output(), "");
}

#[test]
fn serialize_indexed_format() {
    // SplHeap_serialize_indexed_format.phpt, gh20101.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMaxHeap");
    insert_all(&mut t, &h, &[100, 50, 200]);
    let data = ok(&mut t, &h, "__serialize", vec![]);
    let want = list(vec![
        Value::empty_array(),
        assoc(vec![
            (skey("flags"), i(0)),
            (skey("heap_elements"), ints(&[200, 50, 100])),
        ]),
    ]);
    assert!(same(&data, &want));

    let pq = heap(&mut t, "SplPriorityQueue");
    pq_insert(&mut t, &pq, s("data"), i(10));
    ok(&mut t, &pq, "setExtractFlags", vec![i(1)]);
    let data = ok(&mut t, &pq, "__serialize", vec![]);
    let want = list(vec![
        Value::empty_array(),
        assoc(vec![
            (skey("flags"), i(1)),
            (skey("heap_elements"), list(vec![both(s("data"), i(10))])),
        ]),
    ]);
    assert!(same(&data, &want));

    class_with_props(
        &mut t.vm,
        "CustomHeap",
        "SplMaxHeap",
        0,
        vec![
            Prop::public("flags", s("user_property")),
            Prop::public("heap_elements", s("user_property")),
        ],
        &[],
    );
    let c = heap(&mut t, "CustomHeap");
    ok(&mut t, &c, "insert", vec![i(42)]);
    let data = ok(&mut t, &c, "__serialize", vec![]);
    let want = list(vec![
        assoc(vec![
            (skey("flags"), s("user_property")),
            (skey("heap_elements"), s("user_property")),
        ]),
        assoc(vec![
            (skey("flags"), i(0)),
            (skey("heap_elements"), ints(&[42])),
        ]),
    ]);
    assert!(same(&data, &want));

    class_with_props(
        &mut t.vm,
        "CustomPriorityQueue",
        "SplPriorityQueue",
        0,
        vec![Prop::public("field", i(0))],
        &[],
    );
    let c = heap(&mut t, "CustomPriorityQueue");
    let data = ok(&mut t, &c, "__serialize", vec![]);
    let want = list(vec![
        assoc(vec![(skey("field"), i(0))]),
        assoc(vec![
            (skey("flags"), i(1)),
            (skey("heap_elements"), Value::empty_array()),
        ]),
    ]);
    assert!(same(&data, &want));
}

#[test]
fn serialize_inheritance_safety() {
    // SplHeap_serialize_inheritance_safety.phpt
    let mut t = TestVm::new();
    let prop = |name: &'static str, flags: u32, v: &str| Prop {
        name,
        flags,
        default: s(v),
        ty: None,
    };
    class_with_props(
        &mut t.vm,
        "CustomHeap",
        "SplMaxHeap",
        0,
        vec![
            prop("flags", member_flags::PUBLIC, "user_flags_property"),
            prop(
                "heap_elements",
                member_flags::PUBLIC,
                "user_heap_elements_property",
            ),
            prop("custom_prop", member_flags::PUBLIC, "custom_value"),
            prop("protected_prop", member_flags::PROTECTED, "protected_value"),
            prop("private_prop", member_flags::PRIVATE, "private_value"),
        ],
        &[],
    );
    let h = heap(&mut t, "CustomHeap");
    h.set_prop(b"flags", s("modified_user_flags"));
    h.set_prop(b"heap_elements", s("modified_user_elements"));
    h.set_prop(b"custom_prop", s("modified_custom"));
    insert_all(&mut t, &h, &[100, 50, 200]);
    let ss = ser(&mut t, &h).unwrap();
    assert_eq!(
        ss,
        "O:10:\"CustomHeap\":2:{i:0;a:5:{s:5:\"flags\";s:19:\"modified_user_flags\";s:13:\"heap_elements\";s:22:\"modified_user_elements\";s:11:\"custom_prop\";s:15:\"modified_custom\";s:17:\"\0*\0protected_prop\";s:15:\"protected_value\";s:24:\"\0CustomHeap\0private_prop\";s:13:\"private_value\";}i:1;a:2:{s:5:\"flags\";i:0;s:13:\"heap_elements\";a:3:{i:0;i:200;i:1;i:50;i:2;i:100;}}}"
    );
    let u = unser(&mut t, &ss);
    let d = debug(&mut t, &u);
    let keys: Vec<&str> = d.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "flags",
            "heap_elements",
            "custom_prop",
            "\0*\0protected_prop",
            "\0CustomHeap\0private_prop",
            "\0SplHeap\0flags",
            "\0SplHeap\0isCorrupted",
            "\0SplHeap\0heap"
        ]
    );
    assert!(same(&d[0].1, &s("modified_user_flags")));
    assert!(same(&d[7].1, &ints(&[200, 50, 100])));
    assert_eq!(t.output(), "");
}

#[test]
// The phpt serializes the literal 3.14159.
#[allow(clippy::approx_constant)]
fn serialize_complex_values() {
    // SplMaxHeap_serialize_complex.phpt, SplMinHeap_serialize_complex.phpt,
    // SplPriorityQueue_serialize_complex.phpt
    let mut t = TestVm::new();
    let h = heap(&mut t, "SplMaxHeap");
    let item = |ty: &str, v: i64| assoc(vec![(skey("type"), s(ty)), (skey("value"), i(v))]);
    for (ty, v) in [("array1", 10), ("array2", 20), ("array3", 5)] {
        ok(&mut t, &h, "insert", vec![item(ty, v)]);
    }
    let ss = ser(&mut t, &h).unwrap();
    assert_eq!(
        ss,
        "O:10:\"SplMaxHeap\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:0;s:13:\"heap_elements\";a:3:{i:0;a:2:{s:4:\"type\";s:6:\"array3\";s:5:\"value\";i:5;}i:1;a:2:{s:4:\"type\";s:6:\"array1\";s:5:\"value\";i:10;}i:2;a:2:{s:4:\"type\";s:6:\"array2\";s:5:\"value\";i:20;}}}}"
    );
    let u = unser(&mut t, &ss);
    assert!(same(
        &heap_values(&mut t, &u),
        &list(vec![
            item("array3", 5),
            item("array1", 10),
            item("array2", 20)
        ])
    ));

    let h2 = heap(&mut t, "SplMaxHeap");
    for v in [30, 10, 20] {
        let o = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
        o.set_prop(b"val", i(v));
        ok(&mut t, &h2, "insert", vec![Value::Object(o)]);
    }
    let ss = ser(&mut t, &h2).unwrap();
    assert_eq!(
        ss,
        "O:10:\"SplMaxHeap\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:0;s:13:\"heap_elements\";a:3:{i:0;O:8:\"stdClass\":1:{s:3:\"val\";i:30;}i:1;O:8:\"stdClass\":1:{s:3:\"val\";i:10;}i:2;O:8:\"stdClass\":1:{s:3:\"val\";i:20;}}}}"
    );

    let q = heap(&mut t, "SplPriorityQueue");
    ok(&mut t, &q, "setExtractFlags", vec![i(3)]);
    let hobbies = list(vec![s("reading"), s("gaming")]);
    let array1 = assoc(vec![(skey("name"), s("John")), (skey("hobbies"), hobbies)]);
    pq_insert(&mut t, &q, array1, i(10));
    let obj = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    obj.set_prop(b"prop", s("test"));
    pq_insert(&mut t, &q, Value::Object(obj), i(15));
    pq_insert(&mut t, &q, Value::Float(3.14159), i(12));
    pq_insert(&mut t, &q, Value::Bool(true), i(20));
    pq_insert(&mut t, &q, Value::Null, i(3));
    let ss = ser(&mut t, &q).unwrap();
    assert_eq!(
        ss,
        "O:16:\"SplPriorityQueue\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:3;s:13:\"heap_elements\";a:5:{i:0;a:2:{s:4:\"data\";b:1;s:8:\"priority\";i:20;}i:1;a:2:{s:4:\"data\";O:8:\"stdClass\":1:{s:4:\"prop\";s:4:\"test\";}s:8:\"priority\";i:15;}i:2;a:2:{s:4:\"data\";d:3.14159;s:8:\"priority\";i:12;}i:3;a:2:{s:4:\"data\";a:2:{s:4:\"name\";s:4:\"John\";s:7:\"hobbies\";a:2:{i:0;s:7:\"reading\";i:1;s:6:\"gaming\";}}s:8:\"priority\";i:10;}i:4;a:2:{s:4:\"data\";N;s:8:\"priority\";i:3;}}}}"
    );
    let u = unser(&mut t, &ss);
    assert_eq!(count(&mut t, &u).unwrap(), 5);
    assert!(same(
        &ok(&mut t, &u, "top", vec![]),
        &both(Value::Bool(true), i(20))
    ));

    let q2 = heap(&mut t, "SplPriorityQueue");
    ok(&mut t, &q2, "setExtractFlags", vec![i(2)]);
    pq_insert(&mut t, &q2, s("data"), i(42));
    let ss = ser(&mut t, &q2).unwrap();
    assert_eq!(
        ss,
        "O:16:\"SplPriorityQueue\":2:{i:0;a:0:{}i:1;a:2:{s:5:\"flags\";i:2;s:13:\"heap_elements\";a:1:{i:0;a:2:{s:4:\"data\";s:4:\"data\";s:8:\"priority\";i:42;}}}}"
    );
    let u = unser(&mut t, &ss);
    assert!(same(&ok(&mut t, &u, "top", vec![]), &i(42)));
}

#[test]
fn serialize_refuses_corrupted_heaps() {
    // SplHeap_serialize_corrupted.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "ThrowingHeap",
        "SplMaxHeap",
        &[&THROWING_COMPARE],
    );
    let h = heap(&mut t, "ThrowingHeap");
    ok(&mut t, &h, "insert", vec![i(1)]);
    let _ = m(&mut t, &h, "insert", vec![i(2)]);
    assert!(is_corrupted(&h));
    assert_eq!(
        thrown(ser(&mut t, &h).map(|_| Value::Null)),
        runtime(CORRUPTED)
    );
    user_class(
        &mut t.vm,
        "ThrowingPQ",
        "SplPriorityQueue",
        &[&THROWING_COMPARE],
    );
    let pq = heap(&mut t, "ThrowingPQ");
    pq_insert(&mut t, &pq, s("data1"), i(1));
    let _ = m(&mut t, &pq, "insert", vec![s("data2"), i(2)]);
    assert_eq!(
        thrown(ser(&mut t, &pq).map(|_| Value::Null)),
        runtime(CORRUPTED)
    );
}

#[test]
fn unserialize_error_handling() {
    // SplHeap_serialize_error_handling.phpt, SplPriorityQueue_unserialize_invalid_flags.phpt
    let mut t = TestVm::new();
    let e = Value::empty_array;
    let st =
        |pairs: Vec<(&str, Value)>| assoc(pairs.into_iter().map(|(k, v)| (skey(k), v)).collect());
    let cases = vec![
        list(vec![]),
        list(vec![e(), e(), e()]),
        list(vec![s("not_array"), e()]),
        list(vec![i(123), e()]),
        list(vec![e(), s("not_array")]),
        list(vec![e(), i(123)]),
        list(vec![e(), e()]),
        list(vec![e(), st(vec![("flags", s("not_int"))])]),
        list(vec![e(), st(vec![("flags", i(0))])]),
        list(vec![
            e(),
            st(vec![("flags", i(0)), ("heap_elements", s("not_array"))]),
        ]),
        list(vec![
            e(),
            st(vec![("flags", i(0)), ("heap_elements", i(123))]),
        ]),
        list(vec![e(), st(vec![("flags", i(1)), ("heap_elements", e())])]),
    ];
    for (n, case) in cases.into_iter().enumerate() {
        let h = heap(&mut t, "SplMaxHeap");
        assert_eq!(
            thrown(m(&mut t, &h, "__unserialize", vec![case])),
            (
                "Exception".into(),
                "Invalid serialization data for SplMaxHeap object".into()
            ),
            "case {n}"
        );
    }
    let pq_cases = vec![
        list(vec![e(), st(vec![("flags", i(0))])]),
        list(vec![e(), st(vec![("flags", i(4)), ("heap_elements", e())])]),
        list(vec![
            e(),
            st(vec![
                ("flags", i(1)),
                ("heap_elements", list(vec![s("not_array")])),
            ]),
        ]),
        list(vec![
            e(),
            st(vec![
                ("flags", i(1)),
                ("heap_elements", list(vec![st(vec![("data", s("test"))])])),
            ]),
        ]),
        list(vec![
            e(),
            st(vec![
                ("flags", i(1)),
                ("heap_elements", list(vec![st(vec![("priority", i(1))])])),
            ]),
        ]),
        list(vec![
            e(),
            st(vec![("flags", i(1)), ("heap_elements", list(vec![e()]))]),
        ]),
        list(vec![
            e(),
            st(vec![
                ("flags", i(1)),
                (
                    "heap_elements",
                    list(vec![st(vec![("data", i(1)), ("x", i(1))])]),
                ),
            ]),
        ]),
    ];
    for (n, case) in pq_cases.into_iter().enumerate() {
        let pq = heap(&mut t, "SplPriorityQueue");
        assert_eq!(
            thrown(m(&mut t, &pq, "__unserialize", vec![case])),
            (
                "Exception".into(),
                "Invalid serialization data for SplPriorityQueue object".into()
            ),
            "pq case {n}"
        );
    }
    // Valid flags, and flags masked to EXTR_BOTH.
    for (flags, want) in [(1, 1), (999, 3)] {
        let pq = heap(&mut t, "SplPriorityQueue");
        let data = list(vec![
            e(),
            st(vec![("flags", i(flags)), ("heap_elements", e())]),
        ]);
        ok(&mut t, &pq, "__unserialize", vec![data]);
        assert!(same(&ok(&mut t, &pq, "getExtractFlags", vec![]), &i(want)));
    }
    // Property errors become the previous exception.
    class_with_props(
        &mut t.vm,
        "TypedHeap",
        "SplMaxHeap",
        0,
        vec![Prop {
            name: "n",
            flags: member_flags::PUBLIC,
            default: i(0),
            ty: Some("int"),
        }],
        &[],
    );
    let h = heap(&mut t, "TypedHeap");
    let data = list(vec![
        st(vec![("n", s("x"))]),
        st(vec![("flags", i(0)), ("heap_elements", e())]),
    ]);
    let r = m(&mut t, &h, "__unserialize", vec![data]);
    let Err(Flow::Exception(ex)) = r else {
        panic!()
    };
    assert_eq!(
        Vm::exception_message(&ex),
        "Invalid serialization data for TypedHeap object"
    );
    assert_eq!(
        crate::spl::fixedarray::test_helpers::previous(&ex),
        Some((
            "TypeError".into(),
            "Cannot assign string to property TypedHeap::$n of type int".into()
        ))
    );
}
