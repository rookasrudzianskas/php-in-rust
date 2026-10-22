//! `MultipleIterator`: multiple_iterator_001.phpt, bug70053.phpt,
//! gh19094.phpt, gh21927.phpt and the remaining error paths.

use std::rc::Rc;

use php_runtime::class::{ClassEntry, DimFetch};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, Value, Vm};

use super::super::super::engine::{self, ObjIterator, this};
use super::super::super::testutil::{array_of, call, new_obj, s};
use super::{
    assert_phpt, dim_isset, dim_read, dim_unset, dim_write, dump, dump_call, int_of, log, obj,
    std_obj, take_log,
};
use crate::testing::{TestVm, thrown};

// ---------------------------------------------------------------------------
// A scriptable Iterator
// ---------------------------------------------------------------------------

/// State of `ListIter`, an `Iterator` over `(key, value)` pairs whose
/// methods can detach the iterator from a `MultipleIterator`, throw, or
/// report a fixed `valid()` result (the user iterators of the tests).
#[derive(Default)]
struct ListIter {
    items: Vec<(Value, Value)>,
    pos: usize,
    /// Method that calls `$parent->detachIterator($this)` and logs
    /// "<method>: still alive" (gh21927.phpt).
    detach_on: Option<&'static str>,
    parent: Option<ObjRef>,
    /// Method that throws `Exception("boom")`.
    throw_on: Option<&'static str>,
    /// What `valid()` returns instead of the computed `bool`.
    valid: Option<Value>,
}

fn hook(vm: &mut Vm, this: &ObjRef, method: &'static str) -> PResult<()> {
    let (detach, parent, throws) = {
        let st = this.native::<ListIter>().unwrap();
        (
            st.detach_on == Some(method),
            st.parent.clone(),
            st.throw_on == Some(method),
        )
    };
    if detach {
        let parent = parent.expect("parent MultipleIterator");
        call(vm, &parent, "detachIterator", vec![obj(this)])?;
        log(vm, format!("{method}: still alive"));
    }
    if throws {
        return Err(vm.throw_error("Exception", "boom"));
    }
    Ok(())
}

fn li_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    hook(vm, &this, "rewind")?;
    this.native_mut::<ListIter>().unwrap().pos = 0;
    Ok(Value::Null)
}

fn li_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    hook(vm, &this, "valid")?;
    let st = this.native::<ListIter>().unwrap();
    Ok(st
        .valid
        .clone()
        .unwrap_or(Value::Bool(st.pos < st.items.len())))
}

fn li_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    hook(vm, &this, "current")?;
    let st = this.native::<ListIter>().unwrap();
    Ok(st.items.get(st.pos).map_or(Value::Null, |(_, v)| v.clone()))
}

fn li_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    hook(vm, &this, "key")?;
    let st = this.native::<ListIter>().unwrap();
    Ok(st.items.get(st.pos).map_or(Value::Null, |(k, _)| k.clone()))
}

fn li_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    hook(vm, &this, "next")?;
    this.native_mut::<ListIter>().unwrap().pos += 1;
    Ok(Value::Null)
}

static LI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: li_rewind,
};
static LI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): mixed {}",
    handler: li_valid,
};
static LI_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: li_current,
};
static LI_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: li_key,
};
static LI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: li_next,
};

fn list_iter_with(vm: &mut Vm, state: ListIter) -> ObjRef {
    let ce = match vm.find_class(b"ListIter") {
        Some(ce) => ce,
        None => {
            let mut ce = ClassEntry::new("ListIter", 0);
            let iface = vm.find_class(b"Iterator").unwrap();
            engine::implement(&mut ce, &iface);
            engine::add_methods(
                &mut ce,
                &[&LI_REWIND, &LI_VALID, &LI_CURRENT, &LI_KEY, &LI_NEXT],
            );
            let ce = Rc::new(ce);
            vm.register_class(ce.clone());
            ce
        }
    };
    let o = ObjRef::new(ce);
    o.set_native(state);
    o
}

/// `new ArrayIterator($values)`.
fn list_iter(vm: &mut Vm, values: Vec<Value>) -> ObjRef {
    let items = values
        .into_iter()
        .enumerate()
        .map(|(i, v)| (Value::Int(i as i64), v))
        .collect();
    list_iter_with(
        vm,
        ListIter {
            items,
            ..ListIter::default()
        },
    )
}

/// `$iter[] = $v`.
fn push(it: &ObjRef, v: Value) {
    let mut st = it.native_mut::<ListIter>().unwrap();
    let k = Value::Int(st.items.len() as i64);
    st.items.push((k, v));
}

/// `foreach ($m as $key => $value) { var_dump($key, $value); }`
fn foreach_dump(vm: &mut Vm, m: &ObjRef) -> PResult<String> {
    let mut out = String::new();
    let mut it = ObjIterator::over(m.clone());
    it.rewind(vm)?;
    while it.valid(vm)? {
        let value = it.current(vm)?;
        let key = it.key(vm)?;
        out += &dump(vm, &key);
        out += &dump(vm, &value);
        it.next(vm)?;
    }
    Ok(out)
}

fn caught(r: PResult<Value>) -> String {
    let (class, msg) = thrown(r);
    format!("{class}: {msg}\n")
}

fn flag(vm: &mut Vm, name: &str) -> i64 {
    match vm.class_constant("MultipleIterator", name).unwrap() {
        Some(Value::Int(i)) => i,
        other => panic!("MultipleIterator::{name} = {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn constants_and_flags() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(
        [
            "MIT_NEED_ANY",
            "MIT_NEED_ALL",
            "MIT_KEYS_NUMERIC",
            "MIT_KEYS_ASSOC"
        ]
        .map(|n| flag(vm, n)),
        [0, 1, 0, 2]
    );
    let ce = vm.find_class(b"MultipleIterator").unwrap();
    let c = ce.find_constant(b"MIT_KEYS_ASSOC").unwrap();
    assert_eq!(c.ty.as_ref().unwrap().to_type_string(), "int");
    let m = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    assert_eq!(int_of(call(vm, &m, "getFlags", vec![])), 1);
    call(vm, &m, "setFlags", vec![Value::Int(3)]).unwrap();
    assert_eq!(int_of(call(vm, &m, "getFlags", vec![])), 3);
    let m = new_obj(vm, "MultipleIterator", vec![Value::Int(2)]).unwrap();
    assert_eq!(int_of(call(vm, &m, "getFlags", vec![])), 2);
    assert!(
        call(vm, &m, "valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
}

#[test]
fn multiple_iterator_001() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let iter1 = list_iter(vm, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let iter2 = list_iter(vm, vec![Value::Int(1), Value::Int(2)]);
    let o = std_obj(vm);
    let iter3 = list_iter(vm, vec![obj(&o), s("string"), Value::Int(3)]);
    let m = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    let mut out = String::new();

    out += "-- Default flags, no iterators --\n";
    out += &foreach_dump(vm, &m).unwrap();
    out += &caught(call(vm, &m, "current", vec![]));
    out += &caught(call(vm, &m, "key", vec![]));

    for it in [&iter1, &iter2, &iter3] {
        call(vm, &m, "attachIterator", vec![obj(it)]).unwrap();
    }
    out += "-- Default flags, MultipleIterator::MIT_NEED_ALL | MultipleIterator::MIT_KEYS_NUMERIC --\n";
    let want = flag(vm, "MIT_NEED_ALL") | flag(vm, "MIT_KEYS_NUMERIC");
    let flags = int_of(call(vm, &m, "getFlags", vec![]));
    out += &dump(vm, &Value::Bool(flags == want));
    out += &foreach_dump(vm, &m).unwrap();
    out += &caught(call(vm, &m, "current", vec![]));
    out += &caught(call(vm, &m, "key", vec![]));

    out += "-- Flags = MultipleIterator::MIT_NEED_ANY | MultipleIterator::MIT_KEYS_NUMERIC --\n";
    let any = flag(vm, "MIT_NEED_ANY") | flag(vm, "MIT_KEYS_NUMERIC");
    call(vm, &m, "setFlags", vec![Value::Int(any)]).unwrap();
    let flags = int_of(call(vm, &m, "getFlags", vec![]));
    out += &dump(vm, &Value::Bool(flags == any));
    out += &foreach_dump(vm, &m).unwrap();

    out += "-- Default flags, added element --\n";
    call(vm, &m, "setFlags", vec![Value::Int(want)]).unwrap();
    push(&iter2, Value::Int(3));
    out += &foreach_dump(vm, &m).unwrap();

    out += "-- Flags |= MultipleIterator::MIT_KEYS_ASSOC, with iterator associated with NULL --\n";
    let assoc = flag(vm, "MIT_NEED_ALL") | flag(vm, "MIT_KEYS_ASSOC");
    call(vm, &m, "setFlags", vec![Value::Int(assoc)]).unwrap();
    call(vm, &m, "rewind", vec![]).unwrap();
    out += &caught(call(vm, &m, "current", vec![]));

    out += "-- Flags |= MultipleIterator::MIT_KEYS_ASSOC --\n";
    call(vm, &m, "attachIterator", vec![obj(&iter1), s("iter1")]).unwrap();
    call(vm, &m, "attachIterator", vec![obj(&iter2), s("iter2")]).unwrap();
    call(vm, &m, "attachIterator", vec![obj(&iter3), Value::Int(3)]).unwrap();
    out += &foreach_dump(vm, &m).unwrap();

    out += "-- Associate with invalid value --\n";
    let bad = std_obj(vm);
    out += &caught(call(vm, &m, "attachIterator", vec![obj(&iter3), obj(&bad)]));

    out += "-- Associate with duplicate value --\n";
    out += &caught(call(
        vm,
        &m,
        "attachIterator",
        vec![obj(&iter3), s("iter1")],
    ));

    out += "-- Count, contains, detach, count, contains, iterate --\n";
    out += &dump_call(vm, &m, "countIterators", vec![]);
    out += &dump_call(vm, &m, "containsIterator", vec![obj(&iter2)]);
    out += &dump_call(vm, &m, "detachIterator", vec![obj(&iter2)]);
    out += &dump_call(vm, &m, "countIterators", vec![]);
    out += &dump_call(vm, &m, "containsIterator", vec![obj(&iter2)]);
    out += &foreach_dump(vm, &m).unwrap();

    assert_phpt("multiple_iterator_001.phpt", &out);
}

#[test]
fn infos_that_are_the_same_array_key() {
    // bug70053.phpt (EmptyIterator stand-ins)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let assoc = flag(vm, "MIT_KEYS_ASSOC");
    let mit = new_obj(vm, "MultipleIterator", vec![Value::Int(assoc)]).unwrap();
    let e1 = list_iter(vm, vec![]);
    let e2 = list_iter(vm, vec![]);
    call(vm, &mit, "attachIterator", vec![obj(&e1), s("2")]).unwrap();
    call(vm, &mit, "attachIterator", vec![obj(&e2), Value::Int(2)]).unwrap();
    let mut out = String::from(
        "-- Two empty iterators attached with infos that are different but same array key --\n",
    );
    out += &dump_call(vm, &mit, "countIterators", vec![]);
    call(vm, &mit, "rewind", vec![]).unwrap();
    out += &dump_call(vm, &mit, "current", vec![]);
    assert_phpt("bug70053.phpt", &out);
}

#[test]
fn array_writes_need_iterators() {
    // gh19094.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let cls = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    let canary = std_obj(vm);
    let mut out = String::new();
    out += &caught(dim_write(vm, &cls, &obj(&canary), Value::Int(1)).map(|_| Value::Null));
    // An IteratorAggregate is Traversable but no Iterator.
    let agg = {
        let mut ce = ClassEntry::new("MyAggregate", 0);
        let iface = vm.find_class(b"IteratorAggregate").unwrap();
        engine::implement(&mut ce, &iface);
        let ce = Rc::new(ce);
        vm.register_class(ce.clone());
        ObjRef::new(ce)
    };
    out += &caught(dim_write(vm, &cls, &obj(&agg), Value::Int(1)).map(|_| Value::Null));
    let my = list_iter_with(
        vm,
        ListIter {
            valid: Some(Value::Bool(false)),
            ..ListIter::default()
        },
    );
    dim_write(vm, &cls, &obj(&my), Value::Int(1)).unwrap();
    out += &caught(call(vm, &cls, "key", vec![]));
    assert_phpt("gh19094.phpt", &out);
    assert_eq!(int_of(call(vm, &cls, "countIterators", vec![])), 1);
}

#[test]
fn dimension_handlers_test_the_flags_word() {
    // php-src's MultipleIterator handlers test the SOS_OVERRIDDEN_* bits of
    // the flags word, which holds the MIT_* flags.
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let it = list_iter(vm, vec![Value::Int(1)]);
    let all = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    call(vm, &all, "attachIterator", vec![obj(&it), s("info")]).unwrap();
    let (class, msg) = thrown(dim_isset(vm, &all, &obj(&it)).map(Value::Bool));
    assert_eq!(class, "Error");
    assert_eq!(msg, "Cannot use object of type MultipleIterator as array");
    let (_, msg) = thrown(dim_read(vm, &all, &obj(&it), DimFetch::Read));
    assert_eq!(msg, "Cannot use object of type MultipleIterator as array");
    // MIT_NEED_ANY: the fast paths read the storage.
    let any = new_obj(vm, "MultipleIterator", vec![Value::Int(0)]).unwrap();
    call(vm, &any, "attachIterator", vec![obj(&it), s("info")]).unwrap();
    assert!(dim_isset(vm, &any, &obj(&it)).unwrap());
    let v = dim_read(vm, &any, &obj(&it), DimFetch::Read).unwrap();
    assert!(v.identical(&s("info")));
    // MIT_KEYS_ASSOC (bit 2): writes go through the (missing) offsetSet().
    let assoc = new_obj(vm, "MultipleIterator", vec![Value::Int(2)]).unwrap();
    let (_, msg) = thrown(dim_write(vm, &assoc, &obj(&it), Value::Null).map(|_| Value::Null));
    assert_eq!(msg, "Cannot use object of type MultipleIterator as array");
    // unset() always takes the fast path (bit 4 is never an MIT flag).
    dim_unset(vm, &all, &obj(&it)).unwrap();
    assert_eq!(int_of(call(vm, &all, "countIterators", vec![])), 0);
}

#[test]
fn self_detaching_sub_iterators() {
    // gh21927.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mut out = String::new();
    let cases = [
        ("-- detach inside rewind --", "rewind", 1, None),
        ("-- detach inside next --", "next", 1, None),
        ("-- detach inside valid --", "valid", 1, None),
        (
            "-- detach inside current (numeric keys) --",
            "current",
            1,
            None,
        ),
        ("-- detach inside key (numeric keys) --", "key", 1, None),
        (
            "-- detach inside current (assoc keys, refcounted inf) --",
            "current",
            3,
            Some("cur_info_string"),
        ),
        (
            "-- detach inside key (assoc keys, refcounted inf) --",
            "key",
            3,
            Some("key_info_string"),
        ),
    ];
    for (title, method, flags, info) in cases {
        out += title;
        out += "\n";
        // DetachOnRewind is never valid; the others always are and yield
        // key 'k'/'K' and value 'C'.
        let valid = Some(Value::Bool(method != "rewind"));
        let items = match method {
            "current" => vec![(s("k"), s("C"))],
            "key" => vec![(s("K"), s("C"))],
            _ => vec![],
        };
        let mi = new_obj(vm, "MultipleIterator", vec![Value::Int(flags)]).unwrap();
        let it = list_iter_with(
            vm,
            ListIter {
                items,
                detach_on: Some(method),
                parent: Some(mi.clone()),
                valid,
                ..ListIter::default()
            },
        );
        let mut args = vec![obj(&it)];
        if let Some(info) = info {
            args.push(s(info));
        }
        call(vm, &mi, "attachIterator", args).unwrap();
        drop(it);
        let shown = match method {
            "rewind" => {
                call(vm, &mi, "rewind", vec![]).unwrap();
                None
            }
            "next" => {
                call(vm, &mi, "rewind", vec![]).unwrap();
                call(vm, &mi, "next", vec![]).unwrap();
                None
            }
            m => Some(call(vm, &mi, m, vec![]).unwrap()),
        };
        for line in take_log(vm) {
            out += &line;
            out += "\n";
        }
        if let Some(v) = shown {
            out += &dump(vm, &v);
        }
        out += &dump_call(vm, &mi, "countIterators", vec![]);
    }
    assert_phpt("gh21927.phpt", &out);
}

#[test]
fn valid_needs_a_literal_true() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mi = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    let it = list_iter_with(
        vm,
        ListIter {
            items: vec![(Value::Int(0), Value::Int(0))],
            valid: Some(Value::Int(1)),
            ..ListIter::default()
        },
    );
    call(vm, &mi, "attachIterator", vec![obj(&it)]).unwrap();
    assert!(
        call(vm, &mi, "valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
    let (_, msg) = thrown(call(vm, &mi, "current", vec![]));
    assert_eq!(msg, "Called current() with non valid sub iterator");
    // MIT_NEED_ANY: one valid iterator is enough; invalid ones yield null.
    let any = new_obj(vm, "MultipleIterator", vec![Value::Int(0)]).unwrap();
    let good = list_iter(vm, vec![s("x")]);
    call(vm, &any, "attachIterator", vec![obj(&it)]).unwrap();
    call(vm, &any, "attachIterator", vec![obj(&good)]).unwrap();
    assert!(
        call(vm, &any, "valid", vec![])
            .unwrap()
            .identical(&Value::Bool(true))
    );
    let cur = call(vm, &any, "current", vec![]).unwrap();
    let a = array_of(&cur);
    assert!(a.get_int(0).unwrap().is_null());
    assert!(a.get_int(1).unwrap().identical(&s("x")));
}

#[test]
fn sub_iterator_exceptions() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mi = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    let it = list_iter_with(
        vm,
        ListIter {
            items: vec![(Value::Int(0), Value::Int(0))],
            throw_on: Some("current"),
            ..ListIter::default()
        },
    );
    call(vm, &mi, "attachIterator", vec![obj(&it)]).unwrap();
    let Err(php_runtime::Flow::Exception(e)) = call(vm, &mi, "current", vec![]) else {
        panic!("expected an exception")
    };
    assert_eq!(e.class().name.to_string(), "RuntimeException");
    assert_eq!(
        Vm::exception_message(&e),
        "Failed to call sub iterator method"
    );
    let Some(Value::Object(prev)) = e.get_prop(b"previous") else {
        panic!("no previous exception")
    };
    assert_eq!(Vm::exception_message(&prev), "boom");
    // key() still works; exceptions from rewind()/valid()/next() propagate.
    let k = call(vm, &mi, "key", vec![]).unwrap();
    assert!(array_of(&k).get_int(0).unwrap().identical(&Value::Int(0)));
    for method in ["rewind", "valid", "next"] {
        let it = list_iter_with(
            vm,
            ListIter {
                items: vec![(Value::Int(0), Value::Int(0))],
                throw_on: Some(method),
                ..ListIter::default()
            },
        );
        let mi = new_obj(vm, "MultipleIterator", vec![]).unwrap();
        call(vm, &mi, "attachIterator", vec![obj(&it)]).unwrap();
        let (class, msg) = thrown(call(vm, &mi, method, vec![]));
        assert_eq!(
            (class.as_str(), msg.as_str()),
            ("Exception", "boom"),
            "{method}"
        );
    }
}

#[test]
fn attach_iterator_errors_and_infos() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mi = new_obj(vm, "MultipleIterator", vec![]).unwrap();
    let plain = std_obj(vm);
    let (class, msg) = thrown(call(vm, &mi, "attachIterator", vec![obj(&plain)]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "MultipleIterator::attachIterator(): Argument #1 ($iterator) must be of type Iterator, stdClass given"
    );
    let a = list_iter(vm, vec![]);
    let b = list_iter(vm, vec![]);
    call(vm, &mi, "attachIterator", vec![obj(&a), Value::Int(1)]).unwrap();
    // Identity, not equality: "1" is not a duplicate of 1.
    call(vm, &mi, "attachIterator", vec![obj(&b), s("1")]).unwrap();
    let (class, msg) = thrown(call(
        vm,
        &mi,
        "attachIterator",
        vec![obj(&b), Value::Int(1)],
    ));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("InvalidArgumentException", "Key duplication error")
    );
    // Re-attaching without info keeps the position and clears the info.
    call(vm, &mi, "attachIterator", vec![obj(&a)]).unwrap();
    let table = super::super::storage::debug_info(&mi);
    let dumped = dump(vm, &Value::array(table));
    assert!(dumped.contains("[\"inf\"]=>\n      NULL\n"), "{dumped}");
    assert!(
        dumped.contains("[\"inf\"]=>\n      string(1) \"1\"\n"),
        "{dumped}"
    );
    let (class, _) = thrown(call(vm, &mi, "detachIterator", vec![obj(&plain)]));
    assert_eq!(class, "TypeError");
}

#[test]
fn debug_info_and_clone() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mi = new_obj(vm, "MultipleIterator", vec![Value::Int(2)]).unwrap();
    let it = list_iter(vm, vec![Value::Int(1)]);
    call(vm, &mi, "attachIterator", vec![obj(&it), s("a")]).unwrap();
    let table = call(vm, &mi, "__debugInfo", vec![]).unwrap();
    let keys: Vec<Vec<u8>> = array_of(&table)
        .keys()
        .map(|k| k.to_php_str().to_vec())
        .collect();
    assert_eq!(keys, [b"\0SplObjectStorage\0storage".to_vec()]);
    // A clone gets fresh flags (php-src recomputes them for the class).
    let copy = (mi.class().handlers.clone_obj.unwrap())(vm, &mi).unwrap();
    assert_eq!(int_of(call(vm, &copy, "getFlags", vec![])), 0);
    assert_eq!(int_of(call(vm, &copy, "countIterators", vec![])), 1);
    assert!(
        call(vm, &copy, "containsIterator", vec![obj(&it)])
            .unwrap()
            .identical(&Value::Bool(true))
    );
    // MultipleIterator is not Countable.
    let (class, msg) = thrown(vm.call_native("count", vec![obj(&mi)]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "count(): Argument #1 ($value) must be of type Countable|array, MultipleIterator given"
    );
}
