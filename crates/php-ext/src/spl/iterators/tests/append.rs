//! AppendIterator. Its iterator list is an `ArrayIterator`, which is ported
//! separately: until ext/spl's `ArrayIterator` is registered these tests
//! run against a minimal stand-in class of the same name (list semantics:
//! `__construct(array)`, `append()`, `rewind()`, `valid()`, `current()`,
//! `key()`, `next()`, `count()`); with the real class they test the
//! integration.

use std::rc::Rc;

use php_runtime::class::ClassEntry;
use php_runtime::{Args, ArrayKey, Flow, NativeFunction, ObjRef, PResult, Value, Vm};

use super::helpers::*;
use crate::spl::engine;
use crate::spl::testutil::{array_items, list, new_obj, s, user_class};
use crate::testing::{TestVm, thrown};

struct StandIn {
    items: Vec<(Value, Value)>,
    pos: usize,
    next_key: i64,
}

fn si<R>(args: &Args, f: impl FnOnce(&mut StandIn) -> R) -> R {
    let this = engine::this(args);
    let mut st = this.native_mut::<StandIn>().expect("stand-in state");
    f(&mut st)
}

fn si_create(_vm: &mut Vm, ce: &php_runtime::ClassRef) -> PResult<ObjRef> {
    let o = ObjRef::new(ce.clone());
    o.set_native(StandIn {
        items: Vec::new(),
        pos: 0,
        next_key: 0,
    });
    Ok(o)
}
fn si_construct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let items = array_items(args.get(0));
    si(args, |s| {
        s.next_key = items
            .iter()
            .filter_map(|(k, _)| k.as_int())
            .map(|k| k + 1)
            .max()
            .unwrap_or(0);
        s.items = items;
        s.pos = 0;
    });
    Ok(Value::Null)
}
fn si_append(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).clone();
    si(args, |s| {
        s.items.push((Value::Int(s.next_key), v));
        s.next_key += 1;
    });
    Ok(Value::Null)
}
fn si_rewind(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    si(args, |s| s.pos = 0);
    Ok(Value::Null)
}
fn si_valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(si(args, |s| s.pos < s.items.len())))
}
fn si_current(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(si(args, |s| {
        s.items.get(s.pos).map_or(Value::Null, |(_, v)| v.clone())
    }))
}
fn si_key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(si(args, |s| {
        s.items.get(s.pos).map_or(Value::Null, |(k, _)| k.clone())
    }))
}
fn si_next(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    si(args, |s| s.pos += 1);
    Ok(Value::Null)
}
fn si_count(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(si(args, |s| s.items.len() as i64)))
}
static SI_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct(array|object $array = [], int $flags = 0) {}",
    handler: si_construct,
};
static SI_APPEND: NativeFunction = NativeFunction {
    sig: "public function append(mixed $value): void {}",
    handler: si_append,
};
static SI_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: si_rewind,
};
static SI_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: si_valid,
};
static SI_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: si_current,
};
static SI_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): string|int|null {}",
    handler: si_key,
};
static SI_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: si_next,
};
static SI_COUNT: NativeFunction = NativeFunction {
    sig: "public function count(): int {}",
    handler: si_count,
};

/// A VM with `ArrayIterator` available (the stand-in if ext/spl's is not
/// registered yet).
fn vm_with_array_iterator() -> TestVm {
    let mut t = TestVm::new();
    if t.vm.find_class(b"ArrayIterator").is_none() {
        let mut ce = ClassEntry::new("ArrayIterator", 0);
        for iface in ["Iterator", "Countable"] {
            let iface = t.vm.find_class(iface.as_bytes()).unwrap();
            engine::implement(&mut ce, &iface);
        }
        engine::add_methods(
            &mut ce,
            &[
                &SI_CONSTRUCT,
                &SI_APPEND,
                &SI_REWIND,
                &SI_VALID,
                &SI_CURRENT,
                &SI_KEY,
                &SI_NEXT,
                &SI_COUNT,
            ],
        );
        ce.handlers.create_object = Some(si_create);
        ce.link_magic_methods();
        t.vm.register_class(Rc::new(ce));
    }
    t
}

fn ai(vm: &mut Vm, items: Value) -> ObjRef {
    new_obj(vm, "ArrayIterator", vec![items]).unwrap()
}

fn append(vm: &mut Vm, it: &ObjRef, inner: &ObjRef) {
    vm.call_method(it, b"append", vec![Value::Object(inner.clone())])
        .unwrap();
}

fn print_pairs(vm: &mut Vm, it: &ObjRef) {
    foreach_with(vm, it, true, |vm, k, v| {
        let line = kv_lines(vm, &[(k, v)]);
        vm.echo_str(&line)?;
        Ok(true)
    })
    .unwrap();
}

#[test]
fn append_and_rewind() {
    // iterator_013.phpt
    let mut t = vm_with_array_iterator();
    let vm = &mut t.vm;
    echo(vm, "===Empty===").unwrap();
    let it = new_obj(vm, "AppendIterator", vec![]).unwrap();
    print_pairs(vm, &it);
    echo(vm, "===Append===").unwrap();
    let a = ai(vm, list(vec![s("A"), s("B")]));
    append(vm, &it, &a);
    print_pairs(vm, &it);
    echo(vm, "===Rewind===").unwrap();
    print_pairs(vm, &it);
    echo(vm, "===Append===").unwrap();
    let mut items = php_runtime::PhpArray::new();
    items.insert(ArrayKey::Int(2), s("C"));
    items.insert(ArrayKey::Int(3), s("D"));
    let b = ai(vm, Value::array(items));
    append(vm, &it, &b);
    let nr = new_obj(vm, "NoRewindIterator", vec![Value::Object(it.clone())]).unwrap();
    print_pairs(vm, &nr);
    echo(vm, "===Rewind===").unwrap();
    print_pairs(vm, &it);
    assert_phpt(&t.output(), phpt!("iterator_013.phpt"));
}

fn my_ai_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, "MyArrayIterator::rewind")?;
    parent_call(vm, args, "ArrayIterator", "rewind", vec![])
}
static MY_AI_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: my_ai_rewind,
};
fn my_ap_construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "MyAppendIterator::__construct")?;
    Ok(Value::Null)
}
fn my_ap_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, "MyAppendIterator::rewind")?;
    parent_call(vm, args, "AppendIterator", "rewind", vec![])
}
fn my_ap_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, "MyAppendIterator::valid")?;
    parent_call(vm, args, "AppendIterator", "valid", vec![])
}
fn my_ap_append(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, "MyAppendIterator::append")?;
    let what = args.get(0).clone();
    parent_call(vm, args, "AppendIterator", "append", vec![what])
}
fn my_ap_parent_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    parent_call(vm, args, "AppendIterator", "__construct", vec![])
}
static MY_AP_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "function __construct() {}",
    handler: my_ap_construct,
};
static MY_AP_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: my_ap_rewind,
};
static MY_AP_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): bool {}",
    handler: my_ap_valid,
};
static MY_AP_APPEND: NativeFunction = NativeFunction {
    sig: "function append(Iterator $what): void {}",
    handler: my_ap_append,
};
static MY_AP_PARENT_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "function parent__construct(mixed ...$args) {}",
    handler: my_ap_parent_construct,
};

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

#[test]
fn append_rewinds_when_necessary() {
    // iterator_031.phpt
    let mut t = vm_with_array_iterator();
    let vm = &mut t.vm;
    let ap = new_obj(vm, "AppendIterator", vec![]).unwrap();
    let r = vm.call_method(&ap, b"__construct", vec![]);
    print_exception(vm, r);
    user_class(vm, "MyArrayIterator", "ArrayIterator", &[&MY_AI_REWIND]);
    let it = new_obj(vm, "MyArrayIterator", vec![list(vec![1.into(), 2.into()])]).unwrap();
    print_pairs(vm, &it);
    user_class(
        vm,
        "MyAppendIterator",
        "AppendIterator",
        &[
            &MY_AP_CONSTRUCT,
            &MY_AP_REWIND,
            &MY_AP_VALID,
            &MY_AP_APPEND,
            &MY_AP_PARENT_CONSTRUCT,
        ],
    );
    let ap = new_obj(vm, "MyAppendIterator", vec![]).unwrap();
    let r = vm.call_method(&ap, b"append", vec![Value::Object(it.clone())]);
    print_exception(vm, r);
    vm.call_method(&ap, b"parent__construct", vec![]).unwrap();
    let r = vm.call_method(&ap, b"parent__construct", vec![Value::Object(it.clone())]);
    print_exception(vm, r);
    for _ in 0..3 {
        append(vm, &ap, &it);
    }
    print_pairs(vm, &ap);
    assert_phpt(&t.output(), phpt!("iterator_031.phpt"));
}

#[test]
fn iterator_index() {
    // iterator_042.phpt (without the var_dump() of the ArrayIterator)
    let mut t = vm_with_array_iterator();
    let vm = &mut t.vm;
    let it = new_obj(vm, "AppendIterator", vec![]).unwrap();
    assert_eq!(
        thrown(vm.call_method(&it, b"append", vec![list(vec![])])),
        (
            "TypeError".into(),
            "AppendIterator::append(): Argument #1 ($iterator) must be of type Iterator, array given"
                .into()
        )
    );
    for items in [
        vec![1.into()],
        vec![21.into(), 22.into()],
        vec![31.into(), 32.into(), 33.into()],
    ] {
        let a = ai(vm, list(items));
        append(vm, &it, &a);
    }
    let list_it = vm.call_method(&it, b"getArrayIterator", vec![]).unwrap();
    let count = vm
        .call_method(list_it.as_object().unwrap(), b"count", vec![])
        .unwrap();
    assert_eq!(int(&count), 3);
    let mut seen = Vec::new();
    foreach_with(vm, &it, true, |vm, k, v| {
        let idx = vm.call_method(&it, b"getIteratorIndex", vec![])?;
        seen.push((int(&idx), int(&k), int(&v)));
        Ok(true)
    })
    .unwrap();
    assert_eq!(
        seen,
        [
            (0, 0, 1),
            (1, 0, 21),
            (1, 1, 22),
            (2, 0, 31),
            (2, 1, 32),
            (2, 2, 33)
        ]
    );
    assert!(
        vm.call_method(&it, b"getIteratorIndex", vec![])
            .unwrap()
            .is_null()
    );
}

#[test]
fn edge_cases() {
    // bug46031.phpt, bug74519.phpt, bug75155.phpt, bug73471.phpt,
    // bug75173.phpt, iterator_067.phpt
    let mut t = vm_with_array_iterator();
    let vm = &mut t.vm;
    let x = new_obj(vm, "AppendIterator", vec![]).unwrap();
    assert!(vm.call_method(&x, b"next", vec![]).unwrap().is_null());
    assert!(
        vm.call_method(&x, b"valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );

    // bug74519: the inner iterator moved after it was appended.
    let iterator = new_obj(vm, "AppendIterator", vec![]).unwrap();
    let events = ai(vm, list((1..=5).map(Value::Int).collect()));
    append(vm, &iterator, &events);
    vm.call_method(&events, b"next", vec![]).unwrap();
    let mut out = Vec::new();
    loop {
        let valid = vm.call_method(&iterator, b"valid", vec![]).unwrap();
        if !vm.to_bool(&valid).unwrap() {
            break;
        }
        out.push(int(&vm.call_method(&iterator, b"current", vec![]).unwrap()));
        vm.call_method(&iterator, b"next", vec![]).unwrap();
    }
    assert_eq!(out, [2, 3, 4, 5]);

    // bug75155: an AppendIterator inside an AppendIterator.
    let a = ai(vm, list(vec![s("a"), s("b"), s("c")]));
    let b = ai(vm, list(vec![s("d"), s("e"), s("f")]));
    let it1 = new_obj(vm, "AppendIterator", vec![]).unwrap();
    append(vm, &it1, &a);
    let it2 = new_obj(vm, "AppendIterator", vec![]).unwrap();
    append(vm, &it2, &it1);
    append(vm, &it2, &b);
    let mut joined = String::new();
    for v in foreach_values(vm, &it2).unwrap() {
        joined.push_str(&vm.to_php_string(&v).unwrap().to_string());
    }
    assert_eq!(joined, "abcdef");

    // bug73471: appending after the first iterator was exhausted.
    let iterator = new_obj(vm, "AppendIterator", vec![]).unwrap();
    let events = ai(vm, list((1..=5).map(Value::Int).collect()));
    let events2 = ai(vm, list(vec![s("a"), s("b"), s("c")]));
    append(vm, &iterator, &events);
    foreach_values(vm, &events).unwrap();
    append(vm, &iterator, &events2);

    // bug75173: appending during iteration.
    let it = new_obj(vm, "AppendIterator", vec![]).unwrap();
    let foo = ai(vm, list(vec![s("foo")]));
    append(vm, &it, &foo);
    let mut seen = Vec::new();
    foreach_with(vm, &it, false, |vm, _, v| {
        if v.identical(&s("foo")) {
            let bar = ai(vm, list(vec![s("bar")]));
            vm.call_method(&it, b"append", vec![Value::Object(bar)])?;
        }
        seen.push(vm.to_php_string(&v)?.to_string());
        Ok(true)
    })
    .unwrap();
    assert_eq!(seen, ["foo", "bar"]);

    // iterator_067: a subclass without its own constructor.
    user_class(vm, "myAppendIterator", "AppendIterator", &[]);
    new_obj(vm, "myAppendIterator", vec![]).unwrap();
}

#[test]
fn constructor_takes_no_arguments() {
    // AppendIterator_invalid_ctor.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(
        thrown(new_obj(vm, "AppendIterator", vec![Value::Null]).map(Value::Object)),
        (
            "ArgumentCountError".into(),
            "AppendIterator::__construct() expects exactly 0 arguments, 1 given".into()
        )
    );
}
