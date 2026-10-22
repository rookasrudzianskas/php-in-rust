//! IteratorIterator, FilterIterator & co., NoRewindIterator,
//! InfiniteIterator, EmptyIterator, LimitIterator.

use php_runtime::{Args, NativeFunction, PResult, Value, Vm};

use super::helpers::*;
use crate::spl::iterators::dual;
use crate::spl::testutil::{assoc, closure, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

fn ints(r: std::ops::RangeInclusive<i64>) -> Value {
    list(r.map(Value::Int).collect())
}

fn echo_opts(verbose_valid: bool) -> Opts {
    Opts {
        echo: true,
        verbose_valid,
        ..Opts::default()
    }
}

fn print_values(vm: &mut Vm, it: &php_runtime::ObjRef) -> PResult<()> {
    foreach_with(vm, it, false, |vm, _, v| {
        let v = vm.to_php_string(&v)?;
        vm.echo_str(&format!("{v}\n"))?;
        Ok(true)
    })
}

fn var_dump_int(vm: &mut Vm, v: &Value) -> PResult<()> {
    vm.echo_str(&format!("int({})\n", int(v)))
}

// ---------------------------------------------------------------------------
// LimitIterator
// ---------------------------------------------------------------------------

#[test]
fn limit_iterator_call_order() {
    // iterator_004.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let plain = iter_class(vm, "NumericArrayIterator", Kind::Plain, None);
    let seekable = iter_class(
        vm,
        "SeekableNumericArrayIterator",
        Kind::Seekable,
        Some("NumericArrayIterator"),
    );
    let inner = new_iter(&plain, &ints(1..=5), echo_opts(true));
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inner), 1.into(), 3.into()],
    )
    .unwrap();
    print_values(vm, &it).unwrap();
    vm.echo_str("===SEEKABLE===\n").unwrap();
    let inner = new_iter(&seekable, &ints(1..=5), echo_opts(true));
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inner), 1.into(), 3.into()],
    )
    .unwrap();
    print_values(vm, &it).unwrap();
    vm.echo_str("===SEEKING===\n").unwrap();
    let inner = new_iter(&seekable, &ints(1..=5), echo_opts(true));
    let l = new_obj(vm, "LimitIterator", vec![Value::Object(inner)]).unwrap();
    for i in 1..4 {
        vm.call_method(&l, b"seek", vec![Value::Int(i)]).unwrap();
        let v = vm.call_method(&l, b"current", vec![]).unwrap();
        vm.echo_str(&format!("{}\n", int(&v))).unwrap();
    }
    let mut expected = String::from(
        "NumericArrayIterator::rewind\n\
         NumericArrayIterator::valid(true)\n\
         NumericArrayIterator::next\n\
         NumericArrayIterator::valid(true)\n\
         NumericArrayIterator::valid(true)\n\
         NumericArrayIterator::current\n\
         NumericArrayIterator::key\n\
         2\n",
    );
    for v in [3, 4] {
        expected.push_str(&format!(
            "NumericArrayIterator::next\n\
             NumericArrayIterator::valid(true)\n\
             NumericArrayIterator::current\n\
             NumericArrayIterator::key\n\
             {v}\n"
        ));
    }
    expected.push_str(
        "NumericArrayIterator::next\n\
         ===SEEKABLE===\n\
         NumericArrayIterator::rewind\n\
         SeekableNumericArrayIterator::seek(1)\n\
         NumericArrayIterator::valid(true)\n\
         NumericArrayIterator::current\n\
         NumericArrayIterator::key\n\
         2\n",
    );
    for v in [3, 4] {
        expected.push_str(&format!(
            "NumericArrayIterator::next\n\
             NumericArrayIterator::valid(true)\n\
             NumericArrayIterator::current\n\
             NumericArrayIterator::key\n\
             {v}\n"
        ));
    }
    expected.push_str("NumericArrayIterator::next\n===SEEKING===\n");
    for (i, v) in [(1, 2), (2, 3), (3, 4)] {
        expected.push_str(&format!(
            "SeekableNumericArrayIterator::seek({i})\n\
             NumericArrayIterator::valid(true)\n\
             NumericArrayIterator::current\n\
             NumericArrayIterator::key\n\
             {v}\n"
        ));
    }
    assert_eq!(t.output(), expected);
}

#[test]
fn limit_iterator_backward_seek() {
    // iterator_039.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = iter_class(vm, "NumericArrayIterator", Kind::Plain, None);
    let inner = new_iter(
        &ce,
        &list(vec![12.into(), 25.into(), 42.into(), 56.into()]),
        echo_opts(false),
    );
    let it = new_obj(vm, "LimitIterator", vec![Value::Object(inner)]).unwrap();
    foreach_with(vm, &it, true, |vm, k, v| {
        var_dump_int(vm, &k)?;
        var_dump_int(vm, &v)?;
        Ok(true)
    })
    .unwrap();
    vm.echo_str("===SEEK===\n").unwrap();
    vm.call_method(&it, b"seek", vec![2.into()]).unwrap();
    vm.echo_str("===LOOP===\n").unwrap();
    let nr = new_obj(vm, "NoRewindIterator", vec![Value::Object(it)]).unwrap();
    foreach_with(vm, &nr, true, |vm, k, v| {
        var_dump_int(vm, &k)?;
        var_dump_int(vm, &v)?;
        Ok(true)
    })
    .unwrap();
    let step = |k: i64, v: i64| {
        format!(
            "NumericArrayIterator::next\nNumericArrayIterator::valid\n\
             NumericArrayIterator::current\nNumericArrayIterator::key\nint({k})\nint({v})\n"
        )
    };
    let mut expected = String::from(
        "NumericArrayIterator::rewind\nNumericArrayIterator::valid\nNumericArrayIterator::valid\n\
         NumericArrayIterator::current\nNumericArrayIterator::key\nint(0)\nint(12)\n",
    );
    expected += &step(1, 25);
    expected += &step(2, 42);
    expected += &step(3, 56);
    expected += "NumericArrayIterator::next\nNumericArrayIterator::valid\n===SEEK===\n\
         NumericArrayIterator::rewind\nNumericArrayIterator::valid\nNumericArrayIterator::next\n\
         NumericArrayIterator::valid\nNumericArrayIterator::next\nNumericArrayIterator::valid\n\
         NumericArrayIterator::valid\nNumericArrayIterator::current\nNumericArrayIterator::key\n\
         ===LOOP===\nint(2)\nint(42)\n";
    expected += &step(3, 56);
    expected += "NumericArrayIterator::next\nNumericArrayIterator::valid\n";
    assert_eq!(t.output(), expected);
}

#[test]
fn limit_iterator_position_and_seek_errors() {
    // iterator_032.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = seekable(vm, &ints(1..=4));
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inner), 1.into(), 2.into()],
    )
    .unwrap();
    let mut positions = Vec::new();
    let pairs = {
        let mut out = Vec::new();
        foreach_with(vm, &it, true, |vm, k, v| {
            out.push((k, v));
            positions.push(int(&vm.call_method(&it, b"getPosition", vec![])?));
            Ok(true)
        })
        .unwrap();
        out
    };
    assert_eq!(kv_lines(vm, &pairs), "1=>2\n2=>3\n");
    assert_eq!(positions, [1, 2]);
    assert_eq!(
        thrown(vm.call_method(&it, b"seek", vec![0.into()])),
        (
            "OutOfBoundsException".into(),
            "Cannot seek to 0 which is below the offset 1".into()
        )
    );
    vm.call_method(&it, b"seek", vec![2.into()]).unwrap();
    assert_eq!(int(&vm.call_method(&it, b"current", vec![]).unwrap()), 3);
    assert_eq!(
        thrown(vm.call_method(&it, b"seek", vec![3.into()])),
        (
            "OutOfBoundsException".into(),
            "Cannot seek to 3 which is behind offset 1 plus count 2".into()
        )
    );
    vm.call_method(&it, b"next", vec![]).unwrap();
    assert!(
        vm.call_method(&it, b"valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
}

#[test]
fn limit_iterator_argument_checks() {
    // spl_limit_iterator_check_limits.phpt, bug51119.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &list(vec![s("a"), s("b"), s("c")]));
    assert_eq!(
        thrown(
            new_obj(vm, "LimitIterator", vec![Value::Object(inner.clone()), (-1).into()])
                .map(Value::Object)
        ),
        (
            "ValueError".into(),
            "LimitIterator::__construct(): Argument #2 ($offset) must be greater than or equal to 0"
                .into()
        )
    );
    assert_eq!(
        thrown(
            new_obj(
                vm,
                "LimitIterator",
                vec![Value::Object(inner.clone()), 0.into(), (-2).into()]
            )
            .map(Value::Object)
        ),
        (
            "ValueError".into(),
            "LimitIterator::__construct(): Argument #3 ($limit) must be greater than or equal to -1"
                .into()
        )
    );
    let it = new_obj(vm, "LimitIterator", vec![Value::Object(inner), 0.into()]).unwrap();
    print_values(vm, &it).unwrap();
    assert_eq!(t.output(), "a\nb\nc\n");
}

#[test]
fn limit_iterator_large_counts() {
    // gh19577.phpt, bug49723.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &list(vec![s("A"), s("B"), s("C"), s("D")]));
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inner), 2.into(), i64::MAX.into()],
    )
    .unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(kv_lines(vm, &pairs), "2=>C\n3=>D\n");
    let empty = plain(vm, &list(vec![]));
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(empty), 0.into(), 5.into()],
    )
    .unwrap();
    assert!(foreach_values(vm, &it).unwrap().is_empty());
}

#[test]
fn limit_iterator_over_empty_and_infinite() {
    // iterator_010.phpt, iterator_011.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let empty = new_obj(vm, "EmptyIterator", vec![]).unwrap();
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(empty), 0.into(), 3.into()],
    )
    .unwrap();
    assert!(foreach_kv(vm, &it).unwrap().is_empty());

    let empty = new_obj(vm, "EmptyIterator", vec![]).unwrap();
    let inf = new_obj(vm, "InfiniteIterator", vec![Value::Object(empty)]).unwrap();
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inf), 0.into(), 3.into()],
    )
    .unwrap();
    assert!(foreach_kv(vm, &it).unwrap().is_empty());

    let abcd = list(vec![s("A"), s("B"), s("C"), s("D")]);
    let inner = plain(vm, &abcd);
    let inf = new_obj(vm, "InfiniteIterator", vec![Value::Object(inner)]).unwrap();
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inf), 2.into(), 5.into()],
    )
    .unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(kv_lines(vm, &pairs), "2=>C\n3=>D\n0=>A\n1=>B\n2=>C\n");

    let inner = plain(vm, &abcd);
    let lim = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inner), 1.into(), 2.into()],
    )
    .unwrap();
    let inf = new_obj(vm, "InfiniteIterator", vec![Value::Object(lim)]).unwrap();
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(inf), 2.into(), 5.into()],
    )
    .unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(kv_lines(vm, &pairs), "1=>B\n2=>C\n1=>B\n2=>C\n1=>B\n");
}

#[test]
fn limit_iterator_seeks_seekable_inner() {
    // gh18421.phpt: the inner seek() decides (ArrayIterator throws).
    let mut t = TestVm::new();
    if t.vm.find_class(b"ArrayIterator").is_none() {
        return;
    }
    let vm = &mut t.vm;
    let ce = vm.find_class(b"ArrayIterator").unwrap();
    let ai = vm.instantiate(&ce).unwrap();
    vm.call_method(
        &ai,
        b"__construct",
        vec![assoc(vec![
            (skey("zero"), 0.into()),
            (skey("one"), 1.into()),
        ])],
    )
    .unwrap();
    let it = new_obj(
        vm,
        "LimitIterator",
        vec![Value::Object(ai), i64::MAX.into(), i64::MAX.into()],
    )
    .unwrap();
    let (class, msg) = thrown(foreach_kv(vm, &it).map(|_| Value::Null));
    assert_eq!(class, "OutOfBoundsException");
    assert!(msg.starts_with("Seek position "), "{msg}");
}

// ---------------------------------------------------------------------------
// NoRewindIterator / InfiniteIterator / EmptyIterator
// ---------------------------------------------------------------------------

fn nr_ex(vm: &mut Vm, args: &Args, m: &str) -> PResult<Value> {
    vm.echo_str(&format!("NoRewindIteratorEx::{m}\n"))?;
    parent_call(vm, args, "NoRewindIterator", m, vec![])
}
fn nr_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    nr_ex(vm, args, "rewind")
}
fn nr_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    nr_ex(vm, args, "valid")
}
fn nr_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    nr_ex(vm, args, "current")
}
fn nr_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    nr_ex(vm, args, "key")
}
fn nr_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    nr_ex(vm, args, "next")
}
static NR_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: nr_rewind,
};
static NR_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): bool {}",
    handler: nr_valid,
};
static NR_CURRENT: NativeFunction = NativeFunction {
    sig: "function current(): mixed {}",
    handler: nr_current,
};
static NR_KEY: NativeFunction = NativeFunction {
    sig: "function key(): mixed {}",
    handler: nr_key,
};
static NR_NEXT: NativeFunction = NativeFunction {
    sig: "function next(): void {}",
    handler: nr_next,
};

#[test]
fn no_rewind_iterator_call_order() {
    // iterator_007.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner_ce = iter_class(vm, "ArrayIteratorEx", Kind::Plain, None);
    user_class(
        vm,
        "NoRewindIteratorEx",
        "NoRewindIterator",
        &[&NR_REWIND, &NR_VALID, &NR_CURRENT, &NR_KEY, &NR_NEXT],
    );
    let dump = |vm: &mut Vm, _: Value, v: Value| -> PResult<bool> {
        var_dump_int(vm, &v)?;
        Ok(true)
    };
    let inner = new_iter(&inner_ce, &ints(0..=3), echo_opts(false));
    let it = new_obj(vm, "NoRewindIteratorEx", vec![Value::Object(inner)]).unwrap();
    vm.echo_str("===0===\n").unwrap();
    let inner = vm.call_method(&it, b"getInnerIterator", vec![]).unwrap();
    foreach_with(vm, inner.as_object().unwrap(), false, dump).unwrap();
    vm.echo_str("===1===\n").unwrap();
    foreach_with(vm, &it, false, dump).unwrap();

    let inner = new_iter(&inner_ce, &ints(0..=3), echo_opts(false));
    let it = new_obj(vm, "NoRewindIteratorEx", vec![Value::Object(inner)]).unwrap();
    vm.echo_str("===2===\n").unwrap();
    let mut pos = 0;
    foreach_with(vm, &it, false, |vm, _, v| {
        var_dump_int(vm, &v)?;
        let more = pos <= 1;
        pos += 1;
        Ok(more)
    })
    .unwrap();
    vm.echo_str("===3===\n").unwrap();
    foreach_with(vm, &it, false, dump).unwrap();
    vm.echo_str("===4===\n").unwrap();
    foreach_with(vm, &it, false, dump).unwrap();

    let mut expected = String::from("===0===\nArrayIteratorEx::rewind\n");
    for i in 0..4 {
        expected += &format!(
            "ArrayIteratorEx::valid\nArrayIteratorEx::current\nint({i})\nArrayIteratorEx::next\n"
        );
    }
    expected += "ArrayIteratorEx::valid\n===1===\nNoRewindIteratorEx::rewind\n\
                 NoRewindIteratorEx::valid\nArrayIteratorEx::valid\n===2===\n\
                 NoRewindIteratorEx::rewind\n";
    for i in 0..3 {
        if i > 0 {
            expected += "NoRewindIteratorEx::next\nArrayIteratorEx::next\n";
        }
        expected += &format!(
            "NoRewindIteratorEx::valid\nArrayIteratorEx::valid\nNoRewindIteratorEx::current\n\
             ArrayIteratorEx::current\nint({i})\n"
        );
    }
    expected += "===3===\nNoRewindIteratorEx::rewind\nNoRewindIteratorEx::valid\n\
                 ArrayIteratorEx::valid\nNoRewindIteratorEx::current\nint(2)\n\
                 NoRewindIteratorEx::next\nArrayIteratorEx::next\nNoRewindIteratorEx::valid\n\
                 ArrayIteratorEx::valid\nNoRewindIteratorEx::current\nArrayIteratorEx::current\n\
                 int(3)\nNoRewindIteratorEx::next\nArrayIteratorEx::next\n\
                 NoRewindIteratorEx::valid\nArrayIteratorEx::valid\n===4===\n\
                 NoRewindIteratorEx::rewind\nNoRewindIteratorEx::valid\nArrayIteratorEx::valid\n";
    assert_eq!(t.output(), expected);
}

#[test]
fn no_rewind_iterator_basics() {
    // iterator_012.phpt, iterator_038.phpt, bug76367.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &list(vec![s("A"), s("B"), s("C")]));
    let it = new_obj(vm, "NoRewindIterator", vec![Value::Object(inner)]).unwrap();
    let k = vm.call_method(&it, b"key", vec![]).unwrap();
    let v = vm.call_method(&it, b"current", vec![]).unwrap();
    assert_eq!(kv_lines(vm, &[(k, v)]), "0=>A\n");
    vm.call_method(&it, b"next", vec![]).unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(kv_lines(vm, &pairs), "1=>B\n2=>C\n");

    let inner = plain(
        vm,
        &assoc(vec![(skey("Hello"), 0.into()), (skey("World"), 1.into())]),
    );
    let it = new_obj(vm, "NoRewindIterator", vec![Value::Object(inner)]).unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert!(pairs[0].0.identical(&s("Hello")) && pairs[1].0.identical(&s("World")));
    assert!(pairs[0].1.identical(&Value::Int(0)) && pairs[1].1.identical(&Value::Int(1)));

    let inner = plain(vm, &ints(1..=6));
    let it = new_obj(vm, "NoRewindIterator", vec![Value::Object(inner)]).unwrap();
    while vm
        .call_method(&it, b"valid", vec![])
        .unwrap()
        .identical(&Value::Bool(true))
    {
        vm.call_method(&it, b"next", vec![]).unwrap();
    }
    assert!(vm.call_method(&it, b"current", vec![]).unwrap().is_null());
}

#[test]
fn infinite_iterator_call_order() {
    // iterator_008.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = iter_class(vm, "ArrayIteratorEx", Kind::Plain, None);
    let inner = new_iter(&ce, &ints(0..=2), echo_opts(false));
    let it = new_obj(vm, "InfiniteIterator", vec![Value::Object(inner)]).unwrap();
    let mut pos = 0;
    foreach_with(vm, &it, false, |vm, _, v| {
        var_dump_int(vm, &v)?;
        let more = pos <= 5;
        pos += 1;
        Ok(more)
    })
    .unwrap();
    let fetch = |i: i64| {
        format!(
            "ArrayIteratorEx::valid\nArrayIteratorEx::current\nArrayIteratorEx::key\nint({i})\n"
        )
    };
    let mut expected = String::from("ArrayIteratorEx::rewind\n");
    for round in 0..3 {
        let n = if round == 2 { 1 } else { 3 };
        for i in 0..n {
            if i > 0 {
                expected += "ArrayIteratorEx::next\n";
            }
            expected += &fetch(i);
        }
        if round < 2 {
            expected += "ArrayIteratorEx::next\nArrayIteratorEx::valid\nArrayIteratorEx::rewind\n";
        }
    }
    assert_eq!(t.output(), expected);
}

fn ei_ex(vm: &mut Vm, args: &Args, m: &str) -> PResult<Value> {
    vm.echo_str(&format!("EmptyIteratorEx::{m}\n"))?;
    parent_call(vm, args, "EmptyIterator", m, vec![])
}
fn ei_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    ei_ex(vm, args, "rewind")
}
fn ei_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    ei_ex(vm, args, "valid")
}
fn ei_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    ei_ex(vm, args, "current")
}
static EI_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: ei_rewind,
};
static EI_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): false {}",
    handler: ei_valid,
};
static EI_CURRENT: NativeFunction = NativeFunction {
    sig: "function current(): never {}",
    handler: ei_current,
};

#[test]
fn empty_iterator() {
    // iterator_009.phpt, iterator_030.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        "EmptyIteratorEx",
        "EmptyIterator",
        &[&EI_REWIND, &EI_VALID, &EI_CURRENT],
    );
    let it = new_obj(vm, "EmptyIteratorEx", vec![]).unwrap();
    assert!(foreach_values(vm, &it).unwrap().is_empty());
    assert_eq!(
        t.output(),
        "EmptyIteratorEx::rewind\nEmptyIteratorEx::valid\n"
    );

    let vm = &mut t.vm;
    let it = new_obj(vm, "EmptyIterator", vec![]).unwrap();
    let f = Value::Bool(false);
    assert!(vm.call_method(&it, b"valid", vec![]).unwrap().identical(&f));
    vm.call_method(&it, b"rewind", vec![]).unwrap();
    vm.call_method(&it, b"next", vec![]).unwrap();
    assert!(vm.call_method(&it, b"valid", vec![]).unwrap().identical(&f));
    assert_eq!(
        thrown(vm.call_method(&it, b"key", vec![])),
        (
            "BadMethodCallException".into(),
            "Accessing the key of an EmptyIterator".into()
        )
    );
    assert_eq!(
        thrown(vm.call_method(&it, b"current", vec![])),
        (
            "BadMethodCallException".into(),
            "Accessing the value of an EmptyIterator".into()
        )
    );
}

// ---------------------------------------------------------------------------
// IteratorIterator
// ---------------------------------------------------------------------------

fn agg_get_iterator(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("ArrayObjectEx::getIterator\n")?;
    Ok(Value::Object(plain(vm, &ints(0..=3))))
}
static AGG_GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "function getIterator(): Iterator {}",
    handler: agg_get_iterator,
};
fn bad_get_iterator(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}
static BAD_GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "function getIterator() {}",
    handler: bad_get_iterator,
};

/// A class `name` implementing `IteratorAggregate` with `get_iterator`.
fn aggregate_class(vm: &mut Vm, name: &str, get_iterator: &'static NativeFunction) {
    let mut ce = php_runtime::class::ClassEntry::new(name, 0);
    let iface = vm.find_class(b"IteratorAggregate").unwrap();
    crate::spl::engine::implement(&mut ce, &iface);
    crate::spl::engine::add_methods(&mut ce, &[get_iterator]);
    vm.register_class(std::rc::Rc::new(ce));
}

#[test]
fn iterator_iterator_over_aggregate() {
    // iterator_005.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    aggregate_class(vm, "ArrayObjectEx", &AGG_GET_ITERATOR);
    let agg = new_obj(vm, "ArrayObjectEx", vec![]).unwrap();
    let it = new_obj(vm, "IteratorIterator", vec![Value::Object(agg)]).unwrap();
    let values: Vec<i64> = foreach_values(vm, &it).unwrap().iter().map(int).collect();
    assert_eq!(values, [0, 1, 2, 3]);
    assert_eq!(t.output(), "ArrayObjectEx::getIterator\n");
    let inner = t.vm.call_method(&it, b"getInnerIterator", vec![]).unwrap();
    assert_eq!(inner.as_object().unwrap().class().name.to_string(), "TIter");
}

#[test]
fn iterator_iterator_aggregate_errors() {
    // RecursiveIteratorIterator_invalid_aggregate.phpt (same helper)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    aggregate_class(vm, "MyIteratorAggregate", &BAD_GET_ITERATOR);
    let agg = new_obj(vm, "MyIteratorAggregate", vec![]).unwrap();
    for class in ["IteratorIterator", "RecursiveIteratorIterator"] {
        assert_eq!(
            thrown(new_obj(vm, class, vec![Value::Object(agg.clone())]).map(Value::Object)),
            (
                "LogicException".into(),
                "MyIteratorAggregate::getIterator() must return an object that implements Traversable"
                    .into()
            )
        );
    }
    // A downcast class that is not a parent.
    assert_eq!(
        thrown(
            new_obj(
                vm,
                "IteratorIterator",
                vec![Value::Object(agg.clone()), s("ArrayObjectX")]
            )
            .map(Value::Object)
        ),
        (
            "LogicException".into(),
            "Class to downcast to not found or not base class or does not implement Traversable"
                .into()
        )
    );
    // The class argument is ignored for Iterators; at most 2 arguments.
    let inner = plain(vm, &ints(1..=2));
    new_obj(
        vm,
        "IteratorIterator",
        vec![Value::Object(inner.clone()), s("1")],
    )
    .unwrap();
    assert_eq!(
        thrown(
            new_obj(
                vm,
                "IteratorIterator",
                vec![Value::Object(inner), 1.into(), 1.into()]
            )
            .map(Value::Object)
        ),
        (
            "ArgumentCountError".into(),
            "IteratorIterator::__construct() expects at most 2 arguments, 3 given".into()
        )
    );
}

#[test]
fn iterator_iterator_exception_in_current() {
    // bug42703.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = iter_class(vm, "BlaIterator", Kind::Plain, None);
    let opts = Opts::default();
    *opts.fail.borrow_mut() = Some("current".into());
    let inner = new_iter(&ce, &ints(1..=1), opts);
    let it = new_obj(vm, "IteratorIterator", vec![Value::Object(inner)]).unwrap();
    assert_eq!(
        thrown(foreach_kv(vm, &it).map(|_| Value::Null)),
        ("Exception".into(), "BlaIterator::current()".into())
    );
    assert!(vm.call_method(&it, b"current", vec![]).unwrap().is_null());
    assert!(vm.call_method(&it, b"key", vec![]).unwrap().is_null());
}

#[test]
fn dual_iterators_are_not_cloneable_and_forward_methods() {
    // iterator_068.phpt, bug62616.phpt (forwarding is done by the interpreter)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &ints(1..=2));
    let it = new_obj(vm, "IteratorIterator", vec![Value::Object(inner.clone())]).unwrap();
    let clone = it.class().handlers.clone_obj.unwrap();
    assert_eq!(
        thrown(clone(vm, &it).map(Value::Object)),
        (
            "Error".into(),
            "Trying to clone an uncloneable object of class IteratorIterator".into()
        )
    );
    let (target, ce) = dual::dual_it_forward_target(&it).unwrap();
    assert!(php_runtime::ObjRef::ptr_eq(&target, &inner));
    assert_eq!(ce.name.to_string(), "TIter");
    // Not constructed: nothing to forward to.
    let ce = vm.find_class(b"IteratorIterator").unwrap();
    let raw = vm.instantiate(&ce).unwrap();
    assert!(dual::dual_it_forward_target(&raw).is_none());
}

fn noop_construct(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}
static NOOP_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "function __construct() {}",
    handler: noop_construct,
};
fn accept_true(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(true))
}
static ACCEPT_TRUE: NativeFunction = NativeFunction {
    sig: "function accept(): bool {}",
    handler: accept_true,
};

#[test]
fn parent_constructor_not_called() {
    // bug54384.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    for (class, method) in [
        ("IteratorIterator", "rewind"),
        ("FilterIterator", "rewind"),
        ("RecursiveFilterIterator", "hasChildren"),
        ("ParentIterator", "accept"),
        ("LimitIterator", "rewind"),
        ("CachingIterator", "rewind"),
        ("RecursiveCachingIterator", "rewind"),
        ("NoRewindIterator", "valid"),
        ("RegexIterator", "rewind"),
        ("RecursiveRegexIterator", "hasChildren"),
        ("AppendIterator", "rewind"),
        ("InfiniteIterator", "rewind"),
        ("CallbackFilterIterator", "accept"),
        ("IteratorIterator", "getInnerIterator"),
    ] {
        let name = format!("{class}Test");
        let methods: &[&'static NativeFunction] =
            if class == "FilterIterator" || class == "RecursiveFilterIterator" {
                &[&NOOP_CONSTRUCT, &ACCEPT_TRUE]
            } else {
                &[&NOOP_CONSTRUCT]
            };
        if vm.find_class(name.as_bytes()).is_none() {
            user_class(vm, &name, class, methods);
        }
        let o = new_obj(vm, &name, vec![]).unwrap();
        assert_eq!(
            thrown(vm.call_method(&o, method.as_bytes(), vec![])),
            ("Error".into(), dual::INVALID_STATE.into()),
            "{class}::{method}"
        );
    }
    // NoRewindIterator::rewind() does not check.
    let o = new_obj(vm, "NoRewindIteratorTest", vec![]).unwrap();
    vm.call_method(&o, b"rewind", vec![]).unwrap();
}

#[test]
fn constructors_without_arguments() {
    // iterator_056.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    for (class, msg) in [
        (
            "FilterIterator",
            "FilterIterator::__construct() expects exactly 1 argument, 0 given",
        ),
        (
            "CachingIterator",
            "CachingIterator::__construct() expects at least 1 argument, 0 given",
        ),
        (
            "RecursiveCachingIterator",
            "RecursiveCachingIterator::__construct() expects at least 1 argument, 0 given",
        ),
        (
            "ParentIterator",
            "ParentIterator::__construct() expects exactly 1 argument, 0 given",
        ),
        (
            "LimitIterator",
            "LimitIterator::__construct() expects at least 1 argument, 0 given",
        ),
        (
            "NoRewindIterator",
            "NoRewindIterator::__construct() expects exactly 1 argument, 0 given",
        ),
    ] {
        let name = format!("my{class}");
        let methods: &[&'static NativeFunction] = if class == "FilterIterator" {
            &[&ACCEPT_TRUE]
        } else {
            &[]
        };
        user_class(vm, &name, class, methods);
        assert_eq!(
            thrown(new_obj(vm, &name, vec![]).map(Value::Object)),
            ("ArgumentCountError".into(), msg.into())
        );
    }
}

#[test]
fn constructors_called_twice() {
    // iterator_031.phpt (first part), gh16604
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &ints(1..=2));
    let it = new_obj(vm, "IteratorIterator", vec![Value::Object(inner.clone())]).unwrap();
    assert_eq!(
        thrown(vm.call_method(&it, b"__construct", vec![Value::Object(inner)])),
        (
            "BadMethodCallException".into(),
            "IteratorIterator::getIterator() must be called exactly once per instance".into()
        )
    );
}

// ---------------------------------------------------------------------------
// FilterIterator & co.
// ---------------------------------------------------------------------------

fn throwing_accept(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo_str("TestFilter::accept\n")?;
    Err(vm.throw_error("Exception", "Failure in Accept"))
}
static THROWING_ACCEPT: NativeFunction = NativeFunction {
    sig: "function accept(): bool {}",
    handler: throwing_accept,
};

#[test]
fn filter_iterator_exception_in_accept() {
    // bug37457.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(vm, "TestFilter", "FilterIterator", &[&THROWING_ACCEPT]);
    let ce = iter_class(vm, "Collection", Kind::Plain, None);
    let inner = new_iter(&ce, &ints(0..=0), echo_opts(false));
    let it = new_obj(vm, "TestFilter", vec![Value::Object(inner)]).unwrap();
    assert_eq!(
        thrown(foreach_values(vm, &it).map(|_| Value::Null)),
        ("Exception".into(), "Failure in Accept".into())
    );
    assert_eq!(
        t.output(),
        "Collection::rewind\nCollection::valid\nCollection::current\nCollection::key\n\
         TestFilter::accept\n"
    );
}

fn odd_accept(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = crate::spl::engine::this(args);
    let v = vm.call_method(&this, b"current", vec![])?;
    Ok(Value::Bool(int(&v) % 2 == 1))
}
static ODD_ACCEPT: NativeFunction = NativeFunction {
    sig: "function accept(): bool {}",
    handler: odd_accept,
};

#[test]
fn filter_iterator_skips_rejected_elements() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(vm, "OddFilter", "FilterIterator", &[&ODD_ACCEPT]);
    let inner = plain(vm, &ints(1..=6));
    let it = new_obj(vm, "OddFilter", vec![Value::Object(inner)]).unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(kv_lines(vm, &pairs), "0=>1\n2=>3\n4=>5\n");
    // Exhausted: current/key are null.
    assert!(vm.call_method(&it, b"current", vec![]).unwrap().is_null());
}

/// `printf("%s / %s / %d / %d\n", ...)` of CallbackFilterIteratorTest.
fn report(vm: &mut Vm, value: &Value, key: &Value, inner: &Value) -> PResult<()> {
    let inner = inner.as_object().expect("inner iterator").clone();
    let cur = vm.call_method(&inner, b"current", vec![])?;
    let k = vm.call_method(&inner, b"key", vec![])?;
    let same_value = vm.is_equal(value, &cur)?;
    let same_key = vm.is_equal(key, &k)?;
    let value_s = vm.to_php_string(value)?;
    let key_s = vm.to_php_string(key)?;
    vm.echo_str(&format!(
        "{value_s} / {key_s} / {} / {}\n",
        same_value as i32, same_key as i32
    ))
}

fn callback_test(vm: &mut Vm, args: Vec<Value>) -> PResult<Value> {
    let inner = args[2].clone();
    if let Some(o) = inner.as_object() {
        if o.class().instance_of_name(b"RecursiveIterator") {
            let has = vm.call_method(o, b"hasChildren", vec![])?;
            if vm.to_bool(&has)? {
                return Ok(Value::Bool(true));
            }
        }
    }
    report(vm, &args[0], &args[1], &inner)?;
    let v = &args[0];
    Ok(Value::Bool(
        v.identical(&Value::Int(1)) || v.identical(&Value::Int(4)),
    ))
}

#[test]
fn callback_filter_iterator() {
    // CallbackFilterIteratorTest.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let cb = closure(vm, callback_test);
    let inner = plain(vm, &ints(1..=5));
    let it = new_obj(vm, "CallbackFilterIterator", vec![Value::Object(inner), cb]).unwrap();
    print_values(vm, &it).unwrap();
    assert_eq!(
        t.output(),
        "1 / 0 / 1 / 1\n1\n2 / 1 / 1 / 1\n3 / 2 / 1 / 1\n4 / 3 / 1 / 1\n4\n5 / 4 / 1 / 1\n"
    );
}

#[test]
fn callback_filter_iterator_errors() {
    // CallbackFilterIteratorTest-002.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(
        thrown(new_obj(vm, "CallbackFilterIterator", vec![]).map(Value::Object)),
        (
            "ArgumentCountError".into(),
            "CallbackFilterIterator::__construct() expects exactly 2 arguments, 0 given".into()
        )
    );
    assert_eq!(
        thrown(new_obj(vm, "CallbackFilterIterator", vec![Value::Null]).map(Value::Object)),
        (
            "ArgumentCountError".into(),
            "CallbackFilterIterator::__construct() expects exactly 2 arguments, 1 given".into()
        )
    );
    let inner = plain(vm, &list(vec![]));
    assert_eq!(
        thrown(
            new_obj(
                vm,
                "CallbackFilterIterator",
                vec![Value::Object(inner.clone()), Value::Null]
            )
            .map(Value::Object)
        ),
        (
            "TypeError".into(),
            "CallbackFilterIterator::__construct(): Argument #2 ($callback) must be a valid callback, no array or string given".into()
        )
    );
    assert_eq!(
        thrown(
            new_obj(
                vm,
                "CallbackFilterIterator",
                vec![Value::Object(inner), list(vec![])]
            )
            .map(Value::Object)
        ),
        (
            "TypeError".into(),
            "CallbackFilterIterator::__construct(): Argument #2 ($callback) must be a valid callback, array callback must have exactly two members".into()
        )
    );
    let cb = closure(vm, |vm, _| Err(vm.throw_error("Exception", "some message")));
    let inner = plain(vm, &ints(1..=1));
    let it = new_obj(vm, "CallbackFilterIterator", vec![Value::Object(inner), cb]).unwrap();
    assert_eq!(
        thrown(foreach_values(vm, &it).map(|_| Value::Null)),
        ("Exception".into(), "some message".into())
    );
}

#[test]
fn recursive_callback_filter_iterator() {
    // RecursiveCallbackFilterIteratorTest.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let cb = closure(vm, callback_test);
    let data = list(vec![
        1.into(),
        list(vec![2.into(), 3.into()]),
        list(vec![4.into(), 5.into()]),
    ]);
    let inner = recursive(vm, &data);
    let f = new_obj(
        vm,
        "RecursiveCallbackFilterIterator",
        vec![Value::Object(inner), cb],
    )
    .unwrap();
    let it = new_obj(vm, "RecursiveIteratorIterator", vec![Value::Object(f)]).unwrap();
    foreach_with(vm, &it, false, |vm, _, v| {
        let v = vm.to_php_string(&v)?;
        vm.echo_str(&format!("=> {v}\n"))?;
        Ok(true)
    })
    .unwrap();
    assert_eq!(
        t.output(),
        "1 / 0 / 1 / 1\n=> 1\n2 / 0 / 1 / 1\n3 / 1 / 1 / 1\n4 / 0 / 1 / 1\n=> 4\n5 / 1 / 1 / 1\n"
    );
}

fn crash_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = crate::spl::engine::this(args);
    let inner = vm.call_method(&this, b"getInnerIterator", vec![])?;
    vm.call_method(inner.as_object().unwrap(), b"hasChildren", vec![])
}
fn crash_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = crate::spl::engine::this(args);
    let inner = vm.call_method(&this, b"getInnerIterator", vec![])?;
    let cur = vm.call_method(inner.as_object().unwrap(), b"current", vec![])?;
    Ok(Value::Object(recursive(vm, &cur)))
}
static CRASH_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "function hasChildren(): bool {}",
    handler: crash_has_children,
};
static CRASH_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "function getChildren(): RecursiveIterator {}",
    handler: crash_get_children,
};

#[test]
fn filter_iterator_as_recursive_iterator() {
    // iterator_002.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = user_class(
        vm,
        "CrashIterator",
        "FilterIterator",
        &[&ACCEPT_TRUE, &CRASH_HAS_CHILDREN, &CRASH_GET_CHILDREN],
    );
    // `implements RecursiveIterator`
    let mut ce2 = php_runtime::class::ClassEntry::new_subclass("CrashIterator2", 0, &ce);
    let iface = vm.find_class(b"RecursiveIterator").unwrap();
    crate::spl::engine::implement(&mut ce2, &iface);
    ce2.link_magic_methods();
    vm.register_class(std::rc::Rc::new(ce2));
    let data = assoc(vec![
        (php_runtime::ArrayKey::Int(0), 1.into()),
        (
            php_runtime::ArrayKey::Int(2),
            assoc(vec![
                (php_runtime::ArrayKey::Int(0), 21.into()),
                (
                    php_runtime::ArrayKey::Int(22),
                    list(vec![221.into(), 222.into()]),
                ),
                (php_runtime::ArrayKey::Int(23), list(vec![231.into()])),
            ]),
        ),
        (php_runtime::ArrayKey::Int(3), 3.into()),
    ]);
    let inner = recursive(vm, &data);
    let crash = new_obj(vm, "CrashIterator2", vec![Value::Object(inner)]).unwrap();
    let it = new_obj(
        vm,
        "RecursiveIteratorIterator",
        vec![Value::Object(crash), 0.into()],
    )
    .unwrap();
    let values: Vec<i64> = foreach_values(vm, &it).unwrap().iter().map(int).collect();
    assert_eq!(values, [1, 21, 221, 222, 231, 3]);
}

fn rfi_accept_true(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(true))
}
static RFI_ACCEPT_TRUE: NativeFunction = NativeFunction {
    sig: "function accept(): bool {}",
    handler: rfi_accept_true,
};

#[test]
fn recursive_filter_iterator_children() {
    // iterator_040.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        "MyRecursiveFilterIterator",
        "RecursiveFilterIterator",
        &[&RFI_ACCEPT_TRUE],
    );
    let data = list(vec![1.into(), list(vec![21.into(), 22.into()]), 3.into()]);
    let inner = recursive(vm, &data);
    let f = new_obj(vm, "MyRecursiveFilterIterator", vec![Value::Object(inner)]).unwrap();
    let it = new_obj(vm, "RecursiveIteratorIterator", vec![Value::Object(f)]).unwrap();
    let mut seen = Vec::new();
    foreach_with(vm, &it, true, |vm, k, v| {
        let depth = int(&vm.call_method(&it, b"getDepth", vec![])?);
        seen.push((depth, int(&k), int(&v)));
        Ok(true)
    })
    .unwrap();
    assert_eq!(seen, [(0, 0, 1), (1, 0, 21), (1, 1, 22), (0, 2, 3)]);
    // getChildren() wraps the children in the same class.
    let inner = recursive(vm, &data);
    let f2 = new_obj(vm, "MyRecursiveFilterIterator", vec![Value::Object(inner)]).unwrap();
    vm.call_method(&f2, b"rewind", vec![]).unwrap();
    vm.call_method(&f2, b"next", vec![]).unwrap();
    let c = vm.call_method(&f2, b"getChildren", vec![]).unwrap();
    assert_eq!(
        c.as_object().unwrap().class().name.to_string(),
        "MyRecursiveFilterIterator"
    );
}

#[test]
fn parent_iterator_only_shows_parents() {
    // iterator_033.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let data = list(vec![
        1.into(),
        list(vec![21.into(), 22.into(), list(vec![231.into()])]),
        3.into(),
    ]);
    let inner = recursive(vm, &data);
    let p = new_obj(vm, "ParentIterator", vec![Value::Object(inner)]).unwrap();
    let it = new_obj(
        vm,
        "RecursiveIteratorIterator",
        vec![Value::Object(p.clone())],
    )
    .unwrap();
    assert!(foreach_kv(vm, &it).unwrap().is_empty());
    let it = new_obj(
        vm,
        "RecursiveIteratorIterator",
        vec![Value::Object(p), 1.into()],
    )
    .unwrap();
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(int(&pairs[0].0), 1);
    assert_eq!(crate::spl::testutil::array_of(&pairs[0].1).len(), 3);
    assert_eq!(int(&pairs[1].0), 2);
    assert_eq!(crate::spl::testutil::array_of(&pairs[1].1).len(), 1);
}
