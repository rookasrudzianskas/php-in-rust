//! Tests ported from `dllist_*.phpt`, `SplDoublyLinkedList_*.phpt`,
//! `SplQueue_*.phpt`, `SplStack_*.phpt` and the related `bug*`/`gh*` tests.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{class_flags, member_flags};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpStr, Value, Vm};

use super::{flags, foreach_iterator, values};
use crate::spl::engine;
use crate::spl::fixedarray::internal_iterator::ObjectIterator;
use crate::spl::fixedarray::test_helpers::{
    Prop, class_with_props, echo, parent_call, probe, weak,
};
use crate::spl::testutil::{call, entries, list, new_obj, s, user_class};
use crate::testing::{TestVm, thrown};

fn dll(t: &mut TestVm, class: &str) -> ObjRef {
    new_obj(&mut t.vm, class, vec![]).unwrap()
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn m(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    call(&mut t.vm, o, name, args)
}

fn ok(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> Value {
    m(t, o, name, args).unwrap()
}

fn push_all(t: &mut TestVm, o: &ObjRef, vals: &[Value]) {
    for v in vals {
        ok(t, o, "push", vec![v.clone()]);
    }
}

fn same(a: &Value, b: &Value) -> bool {
    a.identical(b)
}

fn count(t: &mut TestVm, o: &ObjRef) -> PResult<i64> {
    let f = o.class().handlers.count_elements.unwrap();
    Ok(f(&mut t.vm, o)?.unwrap())
}

fn text(vm: &mut Vm, v: &Value) -> String {
    vm.to_php_string(v).unwrap().to_string_lossy().into_owned()
}

/// `foreach ($o as $k => $v)` through the `get_iterator` handler.
fn foreach(t: &mut TestVm, o: &ObjRef, mut body: impl FnMut(&mut Vm, Value, Value)) {
    let it = foreach_iterator(&mut t.vm, o, false)
        .unwrap()
        .expect("internal iterator");
    foreach_with(&mut t.vm, &it, &mut body);
}

fn foreach_with(
    vm: &mut Vm,
    it: &Rc<dyn ObjectIterator>,
    body: &mut dyn FnMut(&mut Vm, Value, Value),
) {
    it.rewind(vm).unwrap();
    while it.valid(vm).unwrap() {
        let Some(v) = it.current(vm).unwrap() else {
            break;
        };
        let k = it.key(vm).unwrap().unwrap();
        body(vm, k, v);
        it.move_forward(vm).unwrap();
    }
}

/// `"$k=>$v"` lines of a `foreach`.
fn pairs(t: &mut TestVm, o: &ObjRef) -> Vec<String> {
    let mut out = Vec::new();
    foreach(t, o, |vm, k, v| {
        let line = format!("{}=>{}", text(vm, &k), text(vm, &v));
        out.push(line);
    });
    out
}

fn debug(t: &mut TestVm, o: &ObjRef) -> Vec<(String, Value)> {
    let r = m(t, o, "__debugInfo", vec![]).unwrap();
    entries(&r)
}

fn ints(v: &[i64]) -> Vec<Value> {
    v.iter().map(|n| i(*n)).collect()
}

// ---------------------------------------------------------------------------
// Class shape
// ---------------------------------------------------------------------------

#[test]
fn class_shape() {
    let mut t = TestVm::new();
    for (name, value) in [
        ("IT_MODE_LIFO", 2),
        ("IT_MODE_FIFO", 0),
        ("IT_MODE_DELETE", 1),
        ("IT_MODE_KEEP", 0),
    ] {
        let v = t.vm.class_constant("SplStack", name).unwrap().unwrap();
        assert!(same(&v, &i(value)), "{name}");
    }
    let r = t.call("class_implements", vec![s("SplQueue")]).unwrap();
    let names: Vec<String> = entries(&r).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        names,
        [
            "Iterator",
            "Traversable",
            "Countable",
            "ArrayAccess",
            "Serializable"
        ]
    );
    let q = t.vm.find_class(b"SplQueue").unwrap();
    assert_eq!(
        q.parent.as_ref().unwrap().name.to_string(),
        "SplDoublyLinkedList"
    );
    assert_eq!(
        q.find_method(b"enqueue").unwrap().display_name(),
        "SplQueue::enqueue"
    );
    assert!(!q.has_flag(class_flags::FINAL));
    let st = t.vm.find_class(b"SplStack").unwrap();
    assert!(st.find_method(b"enqueue").is_none());
}

// ---------------------------------------------------------------------------
// dllist_001 / 004 / 005: push, pop, shift, top, bottom, count, clone
// ---------------------------------------------------------------------------

#[test]
fn std_operations() {
    // dllist_001.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    assert_eq!(
        thrown(m(&mut t, &d, "pop", vec![])),
        (
            "RuntimeException".into(),
            "Can't pop from an empty datastructure".into()
        )
    );
    assert_eq!(
        thrown(m(&mut t, &d, "shift", vec![])),
        (
            "RuntimeException".into(),
            "Can't shift from an empty datastructure".into()
        )
    );
    ok(&mut t, &d, "push", vec![i(2)]);
    assert!(same(&ok(&mut t, &d, "pop", vec![]), &i(2)));
    ok(&mut t, &d, "unshift", vec![i(2)]);
    assert!(same(&ok(&mut t, &d, "shift", vec![]), &i(2)));
    push_all(&mut t, &d, &ints(&[1, 2]));
    assert!(same(&ok(&mut t, &d, "top", vec![]), &i(2)));
    assert!(same(&ok(&mut t, &d, "bottom", vec![]), &i(1)));
    ok(&mut t, &d, "pop", vec![]);
    ok(&mut t, &d, "pop", vec![]);
    push_all(&mut t, &d, &[Value::Null, Value::Null]);
    assert_eq!(count(&mut t, &d).unwrap(), 2);
    assert!(same(&ok(&mut t, &d, "count", vec![]), &i(2)));
    assert!(ok(&mut t, &d, "pop", vec![]).is_null());
    assert!(ok(&mut t, &d, "pop", vec![]).is_null());
    ok(&mut t, &d, "push", vec![i(2)]);
    let clone = d.class().handlers.clone_obj.unwrap();
    let c = clone(&mut t.vm, &d).unwrap();
    ok(&mut t, &c, "pop", vec![]);
    assert_eq!(count(&mut t, &d).unwrap(), 1);
    assert_eq!(count(&mut t, &c).unwrap(), 0);
}

#[test]
fn stack() {
    // dllist_004.phpt
    let mut t = TestVm::new();
    let st = dll(&mut t, "SplStack");
    assert_eq!(
        thrown(m(&mut t, &st, "pop", vec![])).1,
        "Can't pop from an empty datastructure"
    );
    assert_eq!(
        thrown(m(&mut t, &st, "shift", vec![])).1,
        "Can't shift from an empty datastructure"
    );
    ok(&mut t, &st, "push", vec![i(2)]);
    assert!(same(&ok(&mut t, &st, "pop", vec![]), &i(2)));
    push_all(&mut t, &st, &ints(&[1, 2]));
    assert!(same(&ok(&mut t, &st, "top", vec![]), &i(2)));
    let mut out = Vec::new();
    foreach(&mut t, &st, |vm, _, v| out.push(text(vm, &v)));
    assert_eq!(out, ["2", "1"]);
    push_all(&mut t, &st, &[Value::Null, Value::Null]);
    assert_eq!(count(&mut t, &st).unwrap(), 4);
    assert!(ok(&mut t, &st, "pop", vec![]).is_null());
    assert!(ok(&mut t, &st, "pop", vec![]).is_null());
    ok(&mut t, &st, "push", vec![i(2)]);
    let clone = st.class().handlers.clone_obj.unwrap();
    let c = clone(&mut t.vm, &st).unwrap();
    ok(&mut t, &c, "pop", vec![]);
    assert_eq!(count(&mut t, &st).unwrap(), 3);
    assert_eq!(flags(&c), 6);
}

#[test]
fn queue() {
    // dllist_005.phpt
    let mut t = TestVm::new();
    let q = dll(&mut t, "SplQueue");
    for method in ["dequeue", "shift"] {
        assert_eq!(
            thrown(m(&mut t, &q, method, vec![])),
            (
                "RuntimeException".into(),
                "Can't shift from an empty datastructure".into()
            )
        );
    }
    ok(&mut t, &q, "enqueue", vec![i(2)]);
    assert!(same(&ok(&mut t, &q, "dequeue", vec![]), &i(2)));
    ok(&mut t, &q, "enqueue", vec![i(1)]);
    ok(&mut t, &q, "enqueue", vec![i(2)]);
    assert!(same(&ok(&mut t, &q, "top", vec![]), &i(2)));
    let mut out = Vec::new();
    foreach(&mut t, &q, |vm, _, v| out.push(text(vm, &v)));
    assert_eq!(out, ["1", "2"]);
    ok(&mut t, &q, "enqueue", vec![Value::Null]);
    ok(&mut t, &q, "enqueue", vec![Value::Null]);
    assert_eq!(count(&mut t, &q).unwrap(), 4);
    assert!(same(&ok(&mut t, &q, "dequeue", vec![]), &i(1)));
    assert!(same(&ok(&mut t, &q, "dequeue", vec![]), &i(2)));
    ok(&mut t, &q, "enqueue", vec![i(2)]);
    let clone = q.class().handlers.clone_obj.unwrap();
    let c = clone(&mut t.vm, &q).unwrap();
    ok(&mut t, &c, "dequeue", vec![]);
    assert_eq!(count(&mut t, &q).unwrap(), 3);
}

#[test]
fn empty_list_peeks() {
    // SplDoublyLinkedList_top_empty.phpt, _bottom_empty.phpt, _current*.phpt, _key.phpt,
    // _isEmpty_*.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    for method in ["top", "bottom"] {
        assert_eq!(
            thrown(m(&mut t, &d, method, vec![])),
            (
                "RuntimeException".into(),
                "Can't peek at an empty datastructure".into()
            )
        );
    }
    assert!(ok(&mut t, &d, "current", vec![]).is_null());
    assert!(same(&ok(&mut t, &d, "key", vec![]), &i(0)));
    assert!(same(&ok(&mut t, &d, "isEmpty", vec![]), &Value::Bool(true)));
    push_all(&mut t, &d, &ints(&[1, 2, 3]));
    assert!(same(
        &ok(&mut t, &d, "isEmpty", vec![]),
        &Value::Bool(false)
    ));
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[test]
fn nested_and_modified_iteration() {
    // dllist_002.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[2, 3, 4]));
    let clone = d.class().handlers.clone_obj.unwrap();
    let d2 = clone(&mut t.vm, &d).unwrap();

    let mut out = Vec::new();
    let outer = foreach_iterator(&mut t.vm, &d, false).unwrap().unwrap();
    let mut body = |vm: &mut Vm, k: Value, v: Value| {
        out.push(format!("{}=>{}", text(vm, &k), text(vm, &v)));
        let inner = foreach_iterator(vm, &d, false).unwrap().unwrap();
        foreach_with(vm, &inner, &mut |vm, k2, v2| {
            out.push(format!("->{}=>{}", text(vm, &k2), text(vm, &v2)));
        });
    };
    foreach_with(&mut t.vm, &outer, &mut body);
    assert_eq!(
        out,
        [
            "0=>2", "->0=>2", "->1=>3", "->2=>4", "1=>3", "->0=>2", "->1=>3", "->2=>4", "2=>4",
            "->0=>2", "->1=>3", "->2=>4"
        ]
    );

    // while popping
    let mut out = Vec::new();
    foreach(&mut t, &d2, |vm, k, v| {
        out.push(format!("{}=>{}", text(vm, &k), text(vm, &v)));
        let popped = call(vm, &d2, "pop", vec![]).unwrap();
        out.push(format!("popped {}", text(vm, &popped)));
    });
    assert_eq!(out, ["0=>2", "popped 4", "1=>3", "popped 3"]);
}

#[test]
fn iterator_modes() {
    // dllist_003.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[2, 3, 4]));
    ok(&mut t, &d, "setIteratorMode", vec![i(2)]);
    assert_eq!(pairs(&mut t, &d), ["2=>4", "1=>3", "0=>2"]);
    ok(&mut t, &d, "setIteratorMode", vec![i(0)]);
    assert_eq!(pairs(&mut t, &d), ["0=>2", "1=>3", "2=>4"]);
    ok(&mut t, &d, "setIteratorMode", vec![i(1)]);
    assert_eq!(count(&mut t, &d).unwrap(), 3);
    assert_eq!(pairs(&mut t, &d), ["0=>2", "0=>3", "0=>4"]);
    assert_eq!(count(&mut t, &d).unwrap(), 0);
}

#[test]
fn iteration_mode_is_fixed_per_loop() {
    // SplDoublyLinkedList_consistent_iterator_mode.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[1, 2, 3]));
    for (start, during, want) in [(0, 2, ["1", "2", "3"]), (2, 0, ["3", "2", "1"])] {
        ok(&mut t, &d, "setIteratorMode", vec![i(start)]);
        let mut out = Vec::new();
        foreach(&mut t, &d, |vm, _, v| {
            call(vm, &d, "setIteratorMode", vec![i(during)]).unwrap();
            out.push(text(vm, &v));
        });
        assert_eq!(out, want);
    }
}

#[test]
fn foreach_by_reference_is_an_error() {
    // SPLDoublyLinkedList_iterate_by_reference.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[2, 3]));
    let e = foreach_iterator(&mut t.vm, &d, true).map(|_| Value::Null);
    assert_eq!(
        thrown(e),
        (
            "Error".into(),
            "An iterator cannot be used with foreach by reference".into()
        )
    );
}

fn my_current(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(s("mine"))
}
static MY_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: my_current,
};

#[test]
fn overridden_iterator_methods_disable_the_internal_iterator() {
    let mut t = TestVm::new();
    user_class(&mut t.vm, "MyList", "SplDoublyLinkedList", &[&MY_CURRENT]);
    let d = dll(&mut t, "MyList");
    assert!(foreach_iterator(&mut t.vm, &d, false).unwrap().is_none());
    user_class(&mut t.vm, "PlainList", "SplQueue", &[]);
    let d = dll(&mut t, "PlainList");
    assert!(foreach_iterator(&mut t.vm, &d, false).unwrap().is_some());
}

#[test]
fn iterator_methods() {
    // dllist_007.phpt, dllist_memleak.phpt, SplDoublyLinkedList_lifoMode.phpt
    let mut t = TestVm::new();
    let a = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &a, &ints(&[1, 2, 3]));
    ok(&mut t, &a, "rewind", vec![]);
    let mut out = Vec::new();
    while ok(&mut t, &a, "valid", vec![]).as_bool().unwrap() {
        out.push(ok(&mut t, &a, "current", vec![]));
        assert!(ok(&mut t, &a, "next", vec![]).is_null());
    }
    assert!(same(&list(out), &list(ints(&[1, 2, 3]))));

    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[1, 2, 3, 4]));
    ok(&mut t, &d, "rewind", vec![]);
    assert!(same(&ok(&mut t, &d, "current", vec![]), &i(1)));
    ok(&mut t, &d, "next", vec![]);
    ok(&mut t, &d, "next", vec![]);
    assert!(same(&ok(&mut t, &d, "current", vec![]), &i(3)));

    let l = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &l, &[s("o"), s("o"), s("f")]);
    ok(&mut t, &l, "setIteratorMode", vec![i(2)]);
    ok(&mut t, &l, "rewind", vec![]);
    let mut word = String::new();
    loop {
        let v = ok(&mut t, &l, "current", vec![]);
        if !t.vm.to_bool(&v).unwrap() {
            break;
        }
        word.push_str(&text(&mut t.vm, &v));
        ok(&mut t, &l, "next", vec![]);
    }
    assert_eq!(word, "foo");
}

#[test]
fn prev() {
    // dllist_010.phpt, dllist_011.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[1, 2, 3, 4]));
    ok(&mut t, &d, "rewind", vec![]);
    ok(&mut t, &d, "prev", vec![]);
    assert!(ok(&mut t, &d, "current", vec![]).is_null());
    ok(&mut t, &d, "rewind", vec![]);
    assert!(same(&ok(&mut t, &d, "current", vec![]), &i(1)));
    ok(&mut t, &d, "next", vec![]);
    assert!(same(&ok(&mut t, &d, "current", vec![]), &i(2)));
    ok(&mut t, &d, "next", vec![]);
    ok(&mut t, &d, "next", vec![]);
    assert!(same(&ok(&mut t, &d, "current", vec![]), &i(4)));
    ok(&mut t, &d, "prev", vec![]);
    assert!(same(&ok(&mut t, &d, "current", vec![]), &i(3)));

    let e = dll(&mut t, "SplDoublyLinkedList");
    ok(&mut t, &e, "rewind", vec![]);
    ok(&mut t, &e, "prev", vec![]);
    assert!(ok(&mut t, &e, "current", vec![]).is_null());
}

#[test]
fn delete_mode_with_concurrent_modification() {
    // bug79151.phpt
    let mut t = TestVm::new();
    let a = dll(&mut t, "SplDoublyLinkedList");
    ok(&mut t, &a, "setIteratorMode", vec![i(2 | 1)]);
    ok(&mut t, &a, "push", vec![i(1)]);
    ok(&mut t, &a, "rewind", vec![]);
    ok(&mut t, &a, "unshift", vec![i(2)]);
    assert!(same(&ok(&mut t, &a, "pop", vec![]), &i(1)));
    assert!(ok(&mut t, &a, "next", vec![]).is_null());

    let a = dll(&mut t, "SplDoublyLinkedList");
    ok(&mut t, &a, "setIteratorMode", vec![i(1)]);
    ok(&mut t, &a, "unshift", vec![i(1)]);
    ok(&mut t, &a, "rewind", vec![]);
    ok(&mut t, &a, "push", vec![i(2)]);
    assert!(same(&ok(&mut t, &a, "shift", vec![]), &i(1)));
    assert!(ok(&mut t, &a, "next", vec![]).is_null());
}

#[test]
fn unset_of_the_traverse_pointer() {
    // bug67538.phpt
    let mut t = TestVm::new();
    let l = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &l, &[s("a"), s("b")]);
    ok(&mut t, &l, "rewind", vec![]);
    ok(&mut t, &l, "offsetUnset", vec![i(0)]);
    ok(&mut t, &l, "push", vec![s("b")]);
    ok(&mut t, &l, "offsetUnset", vec![i(0)]);
    ok(&mut t, &l, "next", vec![]);
    assert!(ok(&mut t, &l, "current", vec![]).is_null());
    assert!(same(&list(values(&l)), &list(vec![s("b")])));
}

#[test]
fn unset_during_stack_iteration() {
    // gh20856.phpt
    let mut t = TestVm::new();
    let st = dll(&mut t, "SplStack");
    for _ in 0..2 {
        let o = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
        ok(
            &mut t,
            &st,
            "offsetSet",
            vec![Value::Null, Value::Object(o)],
        );
    }
    foreach(&mut t, &st, |vm, _, _| {
        call(vm, &st, "offsetUnset", vec![i(0)]).unwrap();
        call(vm, &st, "offsetUnset", vec![i(0)]).unwrap();
    });
    let dbg = debug(&mut t, &st);
    assert_eq!(dbg[0].0, "\0SplDoublyLinkedList\0flags");
    assert!(same(&dbg[0].1, &i(6)));
    assert!(same(&dbg[1].1, &Value::empty_array()));
}

// ---------------------------------------------------------------------------
// ArrayAccess
// ---------------------------------------------------------------------------

#[test]
fn array_access() {
    // dllist_006.phpt
    let mut t = TestVm::new();
    let a = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &a, &ints(&[1, 2, 3]));
    ok(&mut t, &a, "offsetSet", vec![Value::Null, s("foo")]);
    ok(&mut t, &a, "offsetSet", vec![i(3), i(4)]);
    for n in 0..4 {
        assert!(same(&ok(&mut t, &a, "offsetGet", vec![i(n)]), &i(n + 1)));
    }
    assert!(same(&ok(&mut t, &a, "offsetGet", vec![i(2)]), &i(3)));
    ok(&mut t, &a, "offsetUnset", vec![i(2)]);
    assert!(same(&ok(&mut t, &a, "offsetGet", vec![i(2)]), &i(4)));
    assert!(same(&ok(&mut t, &a, "offsetGet", vec![s("1")]), &i(2)));
    assert_eq!(
        thrown(m(&mut t, &a, "offsetGet", vec![s("a")])),
        (
            "TypeError".into(),
            "SplDoublyLinkedList::offsetGet(): Argument #1 ($index) must be of type int, string given"
                .into()
        )
    );
    assert!(same(&ok(&mut t, &a, "offsetGet", vec![s("0")]), &i(1)));
    assert_eq!(
        thrown(m(&mut t, &a, "offsetGet", vec![s("9")])),
        (
            "OutOfRangeException".into(),
            "SplDoublyLinkedList::offsetGet(): Argument #1 ($index) is out of range".into()
        )
    );
}

#[test]
fn offset_argument_errors() {
    // SplDoublyLinkedList_offsetGet_param_*.phpt, _offsetUnset_*.phpt, _offsetExists_success.phpt,
    // _add_*.phpt, bug71735.phpt
    let mut t = TestVm::new();
    let a = dll(&mut t, "SplDoublyLinkedList");
    let e = m(&mut t, &a, "offsetGet", vec![list(vec![s("fail")])]);
    assert_eq!(
        thrown(e).1,
        "SplDoublyLinkedList::offsetGet(): Argument #1 ($index) must be of type int, array given"
    );
    push_all(&mut t, &a, &[s("1"), s("2"), s("3")]);
    for bad in [4, -1, 3] {
        assert_eq!(
            thrown(m(&mut t, &a, "offsetUnset", vec![i(bad)])),
            (
                "OutOfRangeException".into(),
                "SplDoublyLinkedList::offsetUnset(): Argument #1 ($index) is out of range".into()
            )
        );
    }
    for (n, want) in [(0, true), (1, true), (3, false), (-1, false)] {
        assert!(same(
            &ok(&mut t, &a, "offsetExists", vec![i(n)]),
            &Value::Bool(want)
        ));
    }
    let e = m(
        &mut t,
        &a,
        "add",
        vec![i(12), s("Offset 12 should not exist")],
    );
    assert_eq!(
        thrown(e),
        (
            "OutOfRangeException".into(),
            "SplDoublyLinkedList::add(): Argument #1 ($index) is out of range".into()
        )
    );
    let e = m(&mut t, &a, "add", vec![Value::empty_array(), i(2)]);
    assert_eq!(
        thrown(e),
        (
            "TypeError".into(),
            "SplDoublyLinkedList::add(): Argument #1 ($index) must be of type int, array given"
                .into()
        )
    );
    let st = dll(&mut t, "SplStack");
    let date = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    let e = m(&mut t, &st, "offsetSet", vec![i(100), Value::Object(date)]);
    assert_eq!(
        thrown(e),
        (
            "OutOfRangeException".into(),
            "SplDoublyLinkedList::offsetSet(): Argument #1 ($index) is out of range".into()
        )
    );
    // A null index is deprecated for offsetGet() (ZPP "l").
    let v = ok(&mut t, &a, "offsetGet", vec![Value::Null]);
    assert!(same(&v, &s("1")));
    assert_eq!(
        t.output(),
        "\nDeprecated: SplDoublyLinkedList::offsetGet(): Passing null to parameter #1 ($index) of type int is deprecated in /t.php on line 3\n"
    );
}

#[test]
fn offset_unset_first_and_last() {
    // SplDoublylinkedlist_offsetunset_first.phpt, _first002.phpt, _last.phpt
    let mut t = TestVm::new();
    let words = [s("oh"), s("hai"), s("thar")];
    let a = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &a, &words);
    assert!(same(&ok(&mut t, &a, "bottom", vec![]), &s("oh")));
    ok(&mut t, &a, "offsetUnset", vec![i(0)]);
    assert!(same(&ok(&mut t, &a, "bottom", vec![]), &s("hai")));
    assert!(same(&list(values(&a)), &list(vec![s("hai"), s("thar")])));
    let b = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &b, &words);
    ok(&mut t, &b, "offsetUnset", vec![i(2)]);
    assert!(same(&list(values(&b)), &list(vec![s("oh"), s("hai")])));
    assert!(same(&ok(&mut t, &b, "top", vec![]), &s("hai")));
    // LIFO offsets count from the tail.
    let c = dll(&mut t, "SplStack");
    push_all(&mut t, &c, &words);
    assert!(same(&ok(&mut t, &c, "offsetGet", vec![i(0)]), &s("thar")));
    ok(&mut t, &c, "offsetUnset", vec![i(1)]);
    assert!(same(&list(values(&c)), &list(vec![s("oh"), s("thar")])));
}

#[test]
fn add() {
    // dllist_013.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    assert_eq!(
        thrown(m(&mut t, &d, "add", vec![i(2), i(5)])).1,
        "SplDoublyLinkedList::add(): Argument #1 ($index) is out of range"
    );
    for (index, v) in [(0, 6), (0, 3), (1, 4), (2, 5)] {
        ok(&mut t, &d, "add", vec![i(index), i(v)]);
    }
    ok(&mut t, &d, "unshift", vec![i(2)]);
    ok(&mut t, &d, "add", vec![i(0), i(1)]);
    ok(&mut t, &d, "add", vec![i(6), i(7)]);
    assert_eq!(count(&mut t, &d).unwrap(), 7);
    let mut popped = Vec::new();
    for _ in 0..7 {
        popped.push(ok(&mut t, &d, "pop", vec![]).as_int().unwrap());
    }
    assert_eq!(popped, [7, 6, 5, 4, 3, 2, 1]);
    let foobar = s("foobar");
    ok(&mut t, &d, "add", vec![i(0), foobar.clone()]);
    ok(&mut t, &d, "add", vec![i(0), foobar]);
    assert!(same(&ok(&mut t, &d, "shift", vec![]), &s("foobar")));
    assert!(same(&ok(&mut t, &d, "shift", vec![]), &s("foobar")));
}

#[test]
fn iterator_mode_flags() {
    // SplDoublyLinkedList_getIteratorMode.phpt, SplQueue_setIteratorMode*.phpt,
    // SplStack_setIteratorMode.phpt, bug73629.phpt
    let mut t = TestVm::new();
    let l = dll(&mut t, "SplDoublyLinkedList");
    assert!(same(&ok(&mut t, &l, "setIteratorMode", vec![i(0)]), &i(0)));
    assert!(same(&ok(&mut t, &l, "getIteratorMode", vec![]), &i(0)));
    ok(&mut t, &l, "setIteratorMode", vec![i(2)]);
    assert!(same(&ok(&mut t, &l, "getIteratorMode", vec![]), &i(2)));
    let frozen = (
        "RuntimeException".to_string(),
        "Iterators' LIFO/FIFO modes for SplStack/SplQueue objects are frozen".to_string(),
    );
    let q = dll(&mut t, "SplQueue");
    assert!(same(&ok(&mut t, &q, "setIteratorMode", vec![i(0)]), &i(4)));
    assert!(same(&ok(&mut t, &q, "setIteratorMode", vec![i(1)]), &i(5)));
    assert_eq!(thrown(m(&mut t, &q, "setIteratorMode", vec![i(2)])), frozen);
    let st = dll(&mut t, "SplStack");
    assert!(same(&ok(&mut t, &st, "getIteratorMode", vec![]), &i(6)));
    assert_eq!(
        thrown(m(&mut t, &st, "setIteratorMode", vec![i(0)])),
        frozen
    );
    assert!(same(&ok(&mut t, &st, "setIteratorMode", vec![i(3)]), &i(7)));
}

// ---------------------------------------------------------------------------
// count(), __debugInfo
// ---------------------------------------------------------------------------

fn negative_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let n = parent_call(vm, &this, "SplDoublyLinkedList", "count", vec![])?;
    Ok(Value::Int(-n.as_int().unwrap()))
}
static NEGATIVE_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: negative_count,
};

#[test]
fn count_override() {
    // dllist_008.phpt
    let mut t = TestVm::new();
    let o = dll(&mut t, "SplDoublyLinkedList");
    ok(&mut t, &o, "offsetSet", vec![Value::Null, i(1)]);
    ok(&mut t, &o, "offsetSet", vec![Value::Null, i(2)]);
    assert!(same(
        &t.call("count", vec![Value::Object(o)]).unwrap(),
        &i(2)
    ));
    user_class(
        &mut t.vm,
        "SplDoublyLinkedList2",
        "SplDoublyLinkedList",
        &[&NEGATIVE_COUNT],
    );
    let o = dll(&mut t, "SplDoublyLinkedList2");
    ok(&mut t, &o, "offsetSet", vec![Value::Null, i(1)]);
    ok(&mut t, &o, "offsetSet", vec![Value::Null, i(2)]);
    assert!(same(
        &t.call("count", vec![Value::Object(o.clone())]).unwrap(),
        &i(-2)
    ));
    // isEmpty() goes through count_elements too.
    assert!(same(
        &ok(&mut t, &o, "isEmpty", vec![]),
        &Value::Bool(false)
    ));
}

#[test]
fn debug_info() {
    // SplDoublyLinkedList_debug-info.phpt, dllist_012.phpt
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    push_all(&mut t, &d, &ints(&[1, 2, 3]));
    let dbg = debug(&mut t, &d);
    assert_eq!(
        dbg.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        [
            "\0SplDoublyLinkedList\0flags",
            "\0SplDoublyLinkedList\0dllist"
        ]
    );
    assert!(same(&dbg[0].1, &i(0)));
    assert!(same(&dbg[1].1, &list(ints(&[1, 2, 3]))));

    let a = dll(&mut t, "SplDoublyLinkedList");
    ok(
        &mut t,
        &a,
        "offsetSet",
        vec![Value::Null, Value::Object(a.clone())],
    );
    let dbg = debug(&mut t, &a);
    let inner = dbg[1].1.as_array().unwrap().get_int(0).cloned().unwrap();
    assert!(matches!(inner, Value::Object(o) if ObjRef::ptr_eq(&o, &a)));
    ok(&mut t, &a, "pop", vec![]);
}

fn my_debug_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let parent = parent_call(vm, &this, "SplDoublyLinkedList", "__debugInfo", vec![])?;
    let mut out = php_runtime::PhpArray::new();
    out.insert_str("child", i(42));
    out.insert_str("parent", i(parent.as_array().unwrap().len() as i64));
    Ok(Value::array(out))
}
static MY_DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: my_debug_info,
};

#[test]
fn subclass_debug_info_wins() {
    // bug69264.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyDoublyLinkedList",
        "SplDoublyLinkedList",
        &[&MY_DEBUG_INFO],
    );
    let d = dll(&mut t, "MyDoublyLinkedList");
    let ce = d.class().clone();
    assert!(ce.handlers.get_debug_info.is_none());
    assert_eq!(
        ce.magic.debug_info.as_ref().unwrap().display_name(),
        "MyDoublyLinkedList::__debugInfo"
    );
    let r = m(&mut t, &d, "__debugInfo", vec![]).unwrap();
    let e = entries(&r);
    assert_eq!(e.len(), 2);
    assert_eq!((e[0].0.as_str(), e[1].0.as_str()), ("child", "parent"));
    assert!(same(&e[0].1, &i(42)) && same(&e[1].1, &i(2)));
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
fn serialization_of_queue_and_stack() {
    // SplDoublyLinkedList_serialization.phpt, bug78456.phpt
    let mut t = TestVm::new();
    for (class, flags_) in [("SplQueue", 4), ("SplStack", 6)] {
        let q = dll(&mut t, class);
        push_all(&mut t, &q, &[s("a"), s("b")]);
        let ss = ser(&mut t, Value::Object(q));
        assert_eq!(
            ss,
            format!(
                "O:{}:\"{class}\":3:{{i:0;i:{flags_};i:1;a:2:{{i:0;s:1:\"a\";i:1;s:1:\"b\";}}i:2;a:0:{{}}}}",
                class.len()
            )
        );
        let u = unser(&mut t, &ss).unwrap();
        let u = u.as_object().unwrap().clone();
        assert_eq!(u.class().name.to_string(), class);
        assert_eq!(flags(&u), flags_);
        assert!(same(&list(values(&u)), &list(vec![s("a"), s("b")])));
    }

    let d = dll(&mut t, "SplDoublyLinkedList");
    let std = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    ok(&mut t, &d, "push", vec![Value::Object(std)]);
    let ss = ser(&mut t, Value::Object(d));
    assert_eq!(
        ss,
        "O:19:\"SplDoublyLinkedList\":3:{i:0;i:0;i:1;a:1:{i:0;O:8:\"stdClass\":0:{}}i:2;a:0:{}}"
    );
    let u = unser(&mut t, &ss).unwrap();
    let vals = values(u.as_object().unwrap());
    assert!(matches!(&vals[..], [Value::Object(o)] if o.class().name.as_bytes() == b"stdClass"));
}

#[test]
fn serializable_format() {
    // bug75673.phpt, unserialize.phpt
    let mut t = TestVm::new();
    let st = dll(&mut t, "SplStack");
    push_all(&mut t, &st, &[s("one"), s("two")]);
    let serialized = ok(&mut t, &st, "serialize", vec![]);
    assert!(same(&serialized, &s("i:6;:s:3:\"one\";:s:3:\"two\";")));
    assert_eq!(count(&mut t, &st).unwrap(), 2);
    for _ in 0..2 {
        ok(&mut t, &st, "unserialize", vec![serialized.clone()]);
        assert_eq!(count(&mut t, &st).unwrap(), 2);
    }
    assert_eq!(flags(&st), 6);
    // An empty string leaves the list alone.
    ok(&mut t, &st, "unserialize", vec![s("")]);
    assert_eq!(count(&mut t, &st).unwrap(), 2);

    let u = unser(&mut t, "C:19:\"SplDoublyLinkedList\":0:{}").unwrap();
    let u = u.as_object().unwrap().clone();
    assert_eq!(flags(&u), 0);
    assert!(values(&u).is_empty());
    let fresh = dll(&mut t, "SplDoublyLinkedList");
    assert_eq!(
        ser(&mut t, Value::Object(u)),
        ser(&mut t, Value::Object(fresh))
    );
}

#[test]
fn serializable_errors() {
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    for (data, msg) in [
        ("x", "Error at offset 0 of 1 bytes"),
        ("s:1:\"a\";", "Error at offset 8 of 8 bytes"),
        ("i:0;x", "Error at offset 4 of 5 bytes"),
    ] {
        assert_eq!(
            thrown(m(&mut t, &d, "unserialize", vec![s(data)])),
            ("UnexpectedValueException".into(), msg.into()),
            "{data}"
        );
    }
    // Parsing stops at a NUL byte.
    let data = Value::String(PhpStr::from_bytes(b"i:2;:i:1;\0junk"));
    ok(&mut t, &d, "unserialize", vec![data]);
    assert_eq!(flags(&d), 2);
    assert!(same(&list(values(&d)), &list(vec![i(1)])));
}

#[test]
fn nested_serializable_unserialize_shares_back_references() {
    // bug70169.phpt
    let mut t = TestVm::new();
    let inner = "i:1;";
    let exploit = format!(
        "a:2:{{i:0;C:19:\"SplDoublyLinkedList\":{}:{{{inner}}}i:1;R:3;}}",
        inner.len()
    );
    let data = unser(&mut t, &exploit).unwrap();
    let a = data.as_array().unwrap();
    let Some(Value::Object(l)) = a.get_int(0) else {
        panic!()
    };
    assert_eq!(flags(l), 1);
    assert!(values(l).is_empty());
    assert!(same(&a.get_int(1).unwrap().unref(), &i(1)));
}

#[test]
fn magic_serialization() {
    // serialize_property_tables.phpt, unserialize_errors.phpt
    let mut t = TestVm::new();
    class_with_props(
        &mut t.vm,
        "MySplDoublyLinkedList",
        "SplDoublyLinkedList",
        0,
        vec![Prop {
            name: "unused",
            flags: member_flags::PRIVATE,
            default: i(123),
            ty: None,
        }],
        &[],
    );
    let x = dll(&mut t, "MySplDoublyLinkedList");
    let r = ok(&mut t, &x, "__serialize", vec![]);
    let e = entries(&r);
    assert!(same(&e[0].1, &i(0)));
    assert!(same(&e[1].1, &Value::empty_array()));
    let members = entries(&e[2].1);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].0, "\0MySplDoublyLinkedList\0unused");
    assert!(same(&members[0].1, &i(123)));

    for data in [
        "O:19:\"SplDoublyLinkedList\":0:{}",
        "O:19:\"SplDoublyLinkedList\":3:{i:0;b:1;i:1;a:0:{}i:2;a:0:{}}",
        "O:19:\"SplDoublyLinkedList\":3:{i:0;i:0;i:1;a:0:{}i:2;i:0;}",
        "O:19:\"SplDoublyLinkedList\":3:{i:0;i:0;i:1;i:0;i:2;a:0:{}}",
    ] {
        assert_eq!(
            thrown(unser(&mut t, data)),
            (
                "UnexpectedValueException".into(),
                "Incomplete or ill-typed serialization data".into()
            )
        );
    }

    // Round trip of a private property of the subclass.
    x.obj().props.borrow_mut()[0] = i(7);
    ok(&mut t, &x, "push", vec![s("v")]);
    let ss = ser(&mut t, Value::Object(x));
    let u = unser(&mut t, &ss).unwrap();
    let u = u.as_object().unwrap().clone();
    assert!(same(&u.get_prop(b"unused").unwrap(), &i(7)));
    assert!(same(&list(values(&u)), &list(vec![s("v")])));
    assert_eq!(t.output(), "");
}

// ---------------------------------------------------------------------------
// Modification from destructors and serialization hooks
// ---------------------------------------------------------------------------

thread_local! {
    static TARGET: RefCell<Option<ObjRef>> = const { RefCell::new(None) };
}

fn target() -> ObjRef {
    TARGET.with(|t| t.borrow().clone()).expect("target list")
}

fn set_target(o: Option<ObjRef>) {
    TARGET.with(|t| *t.borrow_mut() = o);
}

#[test]
fn destructor_during_offset_unset() {
    // bug80111.phpt
    let mut t = TestVm::new();
    let list_ = dll(&mut t, "SplDoublyLinkedList");
    let w = weak(&list_);
    let trigger = probe(&mut t.vm, move |vm| {
        let s_ = w.upgrade().unwrap();
        call(vm, &s_, "offsetUnset", vec![i(0)])?;
        let a = call(vm, &s_, "current", vec![])?;
        call(vm, &s_, "next", vec![])?;
        let b = call(vm, &s_, "current", vec![])?;
        echo(vm, &format!("{:?} {:?} DONE\n", a.is_null(), b.is_null()))
    });
    ok(&mut t, &list_, "push", vec![Value::Object(trigger)]);
    ok(&mut t, &list_, "push", vec![i(3)]);
    ok(&mut t, &list_, "rewind", vec![]);
    ok(&mut t, &list_, "offsetUnset", vec![i(0)]);
    assert_eq!(t.output(), "true true DONE\n");
    assert_eq!(count(&mut t, &list_).unwrap(), 0);
}

#[test]
fn destructor_during_offset_set() {
    // gh16464.phpt
    let mut t = TestVm::new();
    let list_ = dll(&mut t, "SplDoublyLinkedList");
    let w = weak(&list_);
    let c = probe(&mut t.vm, move |vm| {
        let l = w.upgrade().unwrap();
        let v = call(vm, &l, "pop", vec![])?;
        let v = vm.to_php_string(&v)?;
        echo(vm, &format!("{v}\n"))
    });
    ok(&mut t, &list_, "add", vec![i(0), Value::Object(c)]);
    ok(&mut t, &list_, "offsetSet", vec![i(0), i(42)]);
    assert_eq!(t.output(), "42\n");
    assert!(values(&list_).is_empty());
}

#[test]
fn destructor_unsetting_the_next_node_in_delete_mode() {
    // gh22062.phpt
    let mut t = TestVm::new();
    let list_ = dll(&mut t, "SplDoublyLinkedList");
    ok(&mut t, &list_, "setIteratorMode", vec![i(1)]);
    let w = weak(&list_);
    let p = probe(&mut t.vm, move |vm| {
        let l = w.upgrade().unwrap();
        if call(vm, &l, "count", vec![])?.as_int().unwrap() > 0 {
            call(vm, &l, "offsetUnset", vec![i(0)])?;
        }
        Ok(())
    });
    ok(&mut t, &list_, "push", vec![Value::Object(p)]);
    let std = new_obj(&mut t.vm, "stdClass", vec![]).unwrap();
    ok(&mut t, &list_, "push", vec![Value::Object(std)]);
    let mut seen = 0;
    foreach(&mut t, &list_, |_, _, _| seen += 1);
    assert_eq!(seen, 1);
    assert_eq!(count(&mut t, &list_).unwrap(), 0);
}

fn pop_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    call(vm, &target(), "pop", vec![])?;
    Ok(Value::empty_array())
}
fn unset0_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    call(vm, &target(), "offsetUnset", vec![i(0)])?;
    Ok(Value::empty_array())
}
fn unset1_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    call(vm, &target(), "offsetUnset", vec![i(1)])?;
    Ok(Value::empty_array())
}
fn remove_all_serialize(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let l = target();
    while !call(vm, &l, "isEmpty", vec![])?.as_bool().unwrap() {
        call(vm, &l, "pop", vec![])?;
    }
    Ok(Value::empty_array())
}
static POP_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: pop_serialize,
};
static UNSET0_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: unset0_serialize,
};
static UNSET1_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: unset1_serialize,
};
static REMOVE_ALL_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: remove_all_serialize,
};

fn serialized(t: &mut TestVm, l: &ObjRef) -> String {
    let v = ok(t, l, "serialize", vec![]);
    v.as_str().unwrap().to_string_lossy().into_owned()
}

#[test]
fn serialize_while_the_list_changes() {
    // gh16589.phpt, gh23385.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "C", "stdClass", &[&POP_SERIALIZE]);
    user_class(&mut t.vm, "RemoveSelf", "stdClass", &[&UNSET0_SERIALIZE]);
    user_class(&mut t.vm, "RemoveNext", "stdClass", &[&UNSET1_SERIALIZE]);
    user_class(&mut t.vm, "RemoveAll", "stdClass", &[&REMOVE_ALL_SERIALIZE]);
    user_class(&mut t.vm, "RemoveHolder", "stdClass", &[&UNSET0_SERIALIZE]);
    class_with_props(
        &mut t.vm,
        "Holder",
        "stdClass",
        0,
        vec![
            Prop::public("first", Value::Null),
            Prop::public("second", s("second")),
            Prop::public("third", s("third")),
        ],
        &[],
    );
    let obj = |t: &mut TestVm, c: &str| Value::Object(new_obj(&mut t.vm, c, vec![]).unwrap());

    let l = dll(&mut t, "SplDoublyLinkedList");
    set_target(Some(l.clone()));
    let c = obj(&mut t, "C");
    ok(&mut t, &l, "add", vec![i(0), c]);
    ok(&mut t, &l, "add", vec![i(1), i(1)]);
    assert_eq!(serialized(&mut t, &l), "i:0;:O:1:\"C\":0:{}");

    let l = dll(&mut t, "SplDoublyLinkedList");
    set_target(Some(l.clone()));
    let rs = obj(&mut t, "RemoveSelf");
    ok(
        &mut t,
        &l,
        "push",
        vec![list(vec![rs, list(ints(&[1, 2, 3]))])],
    );
    ok(&mut t, &l, "push", vec![s("tail")]);
    assert_eq!(
        serialized(&mut t, &l),
        "i:0;:a:2:{i:0;O:10:\"RemoveSelf\":0:{}i:1;a:3:{i:0;i:1;i:1;i:2;i:2;i:3;}}:s:4:\"tail\";"
    );
    assert_eq!(count(&mut t, &l).unwrap(), 1);

    let l = dll(&mut t, "SplDoublyLinkedList");
    set_target(Some(l.clone()));
    let rn = obj(&mut t, "RemoveNext");
    push_all(&mut t, &l, &[rn, s("removed"), s("after")]);
    assert_eq!(serialized(&mut t, &l), "i:0;:O:10:\"RemoveNext\":0:{}");
    assert_eq!(count(&mut t, &l).unwrap(), 2);

    let l = dll(&mut t, "SplDoublyLinkedList");
    set_target(Some(l.clone()));
    let ra = obj(&mut t, "RemoveAll");
    push_all(
        &mut t,
        &l,
        &[list(vec![ra, list(ints(&[1, 2]))]), s("x"), s("y")],
    );
    assert_eq!(
        serialized(&mut t, &l),
        "i:0;:a:2:{i:0;O:9:\"RemoveAll\":0:{}i:1;a:2:{i:0;i:1;i:1;i:2;}}"
    );
    assert_eq!(count(&mut t, &l).unwrap(), 0);

    let l = dll(&mut t, "SplDoublyLinkedList");
    set_target(Some(l.clone()));
    let holder = new_obj(&mut t.vm, "Holder", vec![]).unwrap();
    let rh = obj(&mut t, "RemoveHolder");
    holder.set_prop(b"first", rh);
    push_all(&mut t, &l, &[Value::Object(holder), s("tail")]);
    assert_eq!(
        serialized(&mut t, &l),
        "i:0;:O:6:\"Holder\":3:{s:5:\"first\";O:12:\"RemoveHolder\":0:{}s:6:\"second\";s:6:\"second\";s:5:\"third\";s:5:\"third\";}:s:4:\"tail\";"
    );
    assert_eq!(count(&mut t, &l).unwrap(), 1);
    set_target(None);
}

#[test]
fn unserialize_releases_the_old_elements() {
    let mut t = TestVm::new();
    let d = dll(&mut t, "SplDoublyLinkedList");
    let p = probe(&mut t.vm, |vm| echo(vm, "released\n"));
    ok(&mut t, &d, "push", vec![Value::Object(p)]);
    ok(&mut t, &d, "unserialize", vec![s("i:0;:i:5;")]);
    assert_eq!(t.output(), "released\n");
    assert!(same(&list(values(&d)), &list(vec![i(5)])));
}
