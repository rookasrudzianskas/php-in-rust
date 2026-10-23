//! RecursiveIteratorIterator and RecursiveTreeIterator.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::{Args, ArrayKey, Flow, NativeFunction, ObjRef, PResult, Value, Vm};

use super::helpers::*;
use crate::spl::engine;
use crate::spl::iterators::recursive;
use crate::spl::testutil::{assoc, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

const RII: &str = "RecursiveIteratorIterator";
const NAME: &str = "RecursiveArrayIteratorIterator";

fn l(v: Vec<Value>) -> Value {
    list(v)
}

/// array("a", array("ba", array("bba", "bbb"), array(array("bcaa"))), array("ca"), "d")
fn tree_014() -> Value {
    l(vec![
        s("a"),
        l(vec![
            s("ba"),
            l(vec![s("bba"), s("bbb")]),
            l(vec![l(vec![s("bcaa")])]),
        ]),
        l(vec![s("ca")]),
        s("d"),
    ])
}

/// The same with `array("bcba")` added (iterator_021 & co.).
fn tree_021() -> Value {
    l(vec![
        s("a"),
        l(vec![
            s("ba"),
            l(vec![s("bba"), s("bbb")]),
            l(vec![l(vec![s("bcaa")]), l(vec![s("bcba")])]),
        ]),
        l(vec![s("ca")]),
        s("d"),
    ])
}

fn this(args: &Args) -> ObjRef {
    engine::this(args)
}

fn depth(vm: &mut Vm, args: &Args) -> PResult<i64> {
    Ok(int(&vm.call_method(&this(args), b"getDepth", vec![])?))
}

fn flag(o: &ObjRef, name: &str) -> bool {
    matches!(o.get_prop(name.as_bytes()), Some(Value::Bool(true)))
}

fn num(o: &ObjRef, name: &str) -> i64 {
    o.get_prop(name.as_bytes()).map_or(0, |v| int(&v))
}

/// `echo "$k=>$v\n"` with `join('', $v)` for arrays.
fn print_kv(vm: &mut Vm, k: Value, v: Value) -> PResult<bool> {
    let v = match v.unref() {
        Value::Array(a) => {
            let mut out = String::new();
            for x in a.values() {
                out.push_str(&vm.to_php_string(x)?.to_string());
            }
            out
        }
        other => vm.to_php_string(&other)?.to_string(),
    };
    let k = vm.to_php_string(&k)?;
    echo(vm, &format!("{k}=>{v}"))?;
    Ok(true)
}

/// `echo $e::class, ': ', $e->getMessage(), "\n"` for a thrown exception.
fn print_exception(vm: &mut Vm, r: PResult<()>) {
    match r {
        Ok(()) => {}
        Err(Flow::Exception(e)) => {
            let msg = Vm::exception_message(&e);
            echo(vm, &format!("{}: {msg}", e.class().name)).unwrap();
        }
        Err(other) => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// User subclass `RecursiveArrayIteratorIterator` (iterator_014/021/022/023)
// ---------------------------------------------------------------------------

fn r_echo(vm: &mut Vm, args: &Args, m: &str) -> PResult<Value> {
    echo(vm, &format!("{NAME}::{m}"))?;
    parent_call(vm, args, RII, m, vec![])
}
fn r_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, &format!("{NAME}::rewind"))?;
    this(args).set_prop(b"skip", Value::Bool(false));
    parent_call(vm, args, RII, "rewind", vec![])
}
fn r_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, &format!("{NAME}::valid"))?;
    let o = this(args);
    if flag(&o, "skip") {
        o.set_prop(b"skip", Value::Bool(false));
        vm.call_method(&o, b"next", vec![])?;
    }
    parent_call(vm, args, RII, "valid", vec![])
}
fn r_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    r_echo(vm, args, "current")
}
fn r_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    r_echo(vm, args, "key")
}
fn r_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    r_echo(vm, args, "next")
}
fn r_begin_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    echo(vm, &format!("{NAME}::beginChildren({d})"))?;
    Ok(Value::Null)
}
fn r_end_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    echo(vm, &format!("{NAME}::endChildren({d})"))?;
    Ok(Value::Null)
}
fn r_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    o.set_prop(b"max_depth", args.get(1).clone());
    o.set_prop(b"over", Value::Int(0));
    o.set_prop(b"skip", Value::Bool(false));
    parent_call(vm, args, RII, "__construct", vec![args.get(0).clone()])
}
fn r_construct_catch(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    o.set_prop(b"max_depth", args.get(1).clone());
    o.set_prop(b"over", Value::Int(0));
    o.set_prop(b"skip", Value::Bool(false));
    parent_call(
        vm,
        args,
        RII,
        "__construct",
        vec![args.get(0).clone(), 0.into(), 16.into()],
    )
}
fn r_call_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    o.set_prop(b"skip", Value::Bool(false));
    let has = parent_call(vm, args, RII, "callHasChildren", vec![])?;
    let has = vm.to_bool(&has)?;
    let d = depth(vm, args)?;
    let res = d < num(&o, "max_depth") && has;
    let yn = |b: bool| if b { "yes" } else { "no" };
    echo(
        vm,
        &format!("{NAME}::callHasChildren({d}) = {}/{}", yn(res), yn(has)),
    )?;
    if has && !res {
        let over = num(&o, "over") + 1;
        o.set_prop(b"over", Value::Int(over));
        if over == 2 {
            o.set_prop(b"skip", Value::Bool(true));
        }
    }
    Ok(Value::Bool(res))
}
fn my_rai(vm: &mut Vm, items: &Value) -> ObjRef {
    let ce = iter_class(vm, "MyRecursiveArrayIterator", Kind::Recursive, None);
    new_iter(
        &ce,
        items,
        Opts {
            echo_children: true,
            ..Opts::default()
        },
    )
}
fn r_call_get_children_null(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let over = num(&o, "over");
    if over == 2 {
        echo(vm, &format!("{NAME}::callGetChildren(skip)"))?;
        return Ok(Value::Null);
    }
    echo(vm, &format!("{NAME}::callGetChildren(ok:{over})"))?;
    let cur = vm.call_method(&o, b"current", vec![])?;
    Ok(Value::Object(my_rai(vm, &cur)))
}
fn r_call_get_children_throw(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let over = num(&o, "over");
    if over == 2 {
        echo(vm, &format!("{NAME}::callGetChildren(throw)"))?;
        return Err(vm.throw_error("Exception", "Thrown in callGetChildren()"));
    }
    echo(vm, &format!("{NAME}::callGetChildren(ok:{over})"))?;
    let cur = vm.call_method(&o, b"current", vec![])?;
    Ok(Value::Object(my_rai(vm, &cur)))
}

static R_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: r_rewind,
};
static R_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): bool {}",
    handler: r_valid,
};
static R_CURRENT: NativeFunction = NativeFunction {
    sig: "function current(): mixed {}",
    handler: r_current,
};
static R_KEY: NativeFunction = NativeFunction {
    sig: "function key(): mixed {}",
    handler: r_key,
};
static R_NEXT: NativeFunction = NativeFunction {
    sig: "function next(): void {}",
    handler: r_next,
};
static R_BEGIN_CHILDREN: NativeFunction = NativeFunction {
    sig: "function beginChildren(): void {}",
    handler: r_begin_children,
};
static R_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "function endChildren(): void {}",
    handler: r_end_children,
};
static R_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "function __construct($it, $max_depth) {}",
    handler: r_construct,
};
static R_CONSTRUCT_CATCH: NativeFunction = NativeFunction {
    sig: "function __construct($it, $max_depth) {}",
    handler: r_construct_catch,
};
static R_CALL_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "function callHasChildren(): bool {}",
    handler: r_call_has_children,
};
static R_CALL_GET_CHILDREN_NULL: NativeFunction = NativeFunction {
    sig: "function callGetChildren(): ?MyRecursiveArrayIterator {}",
    handler: r_call_get_children_null,
};
static R_CALL_GET_CHILDREN_THROW: NativeFunction = NativeFunction {
    sig: "function callGetChildren(): MyRecursiveArrayIterator {}",
    handler: r_call_get_children_throw,
};

#[test]
fn begin_and_end_children_hooks() {
    // iterator_014.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        NAME,
        RII,
        &[
            &R_REWIND,
            &R_VALID,
            &R_CURRENT,
            &R_KEY,
            &R_NEXT,
            &R_BEGIN_CHILDREN,
            &R_END_CHILDREN,
        ],
    );
    let inner = my_rai(vm, &tree_014());
    let it = new_obj(vm, NAME, vec![Value::Object(inner)]).unwrap();
    foreach_with(vm, &it, true, print_kv).unwrap();
    assert_phpt(&t.output(), phpt!("iterator_014.phpt"));
}

#[test]
fn call_has_children_hook() {
    // iterator_021.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        NAME,
        RII,
        &[
            &R_CONSTRUCT,
            &R_REWIND,
            &R_VALID,
            &R_CURRENT,
            &R_KEY,
            &R_NEXT,
            &R_CALL_HAS_CHILDREN,
            &R_BEGIN_CHILDREN,
            &R_END_CHILDREN,
        ],
    );
    let inner = my_rai(vm, &tree_021());
    let it = new_obj(vm, NAME, vec![Value::Object(inner), 2.into()]).unwrap();
    foreach_with(vm, &it, true, print_kv).unwrap();
    assert_phpt(&t.output(), phpt!("iterator_021.phpt"));
}

#[test]
fn call_get_children_returning_null() {
    // iterator_022.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        NAME,
        RII,
        &[
            &R_CONSTRUCT,
            &R_REWIND,
            &R_VALID,
            &R_CURRENT,
            &R_KEY,
            &R_NEXT,
            &R_CALL_HAS_CHILDREN,
            &R_CALL_GET_CHILDREN_NULL,
            &R_BEGIN_CHILDREN,
            &R_END_CHILDREN,
        ],
    );
    let inner = my_rai(vm, &tree_021());
    let it = new_obj(vm, NAME, vec![Value::Object(inner), 2.into()]).unwrap();
    let r = foreach_with(vm, &it, true, print_kv);
    print_exception(vm, r);
    assert_phpt(&t.output(), phpt!("iterator_022.phpt"));
}

#[test]
fn call_get_children_exception_is_caught() {
    // iterator_023.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        NAME,
        RII,
        &[
            &R_CONSTRUCT_CATCH,
            &R_REWIND,
            &R_VALID,
            &R_CURRENT,
            &R_KEY,
            &R_NEXT,
            &R_CALL_HAS_CHILDREN,
            &R_CALL_GET_CHILDREN_THROW,
            &R_BEGIN_CHILDREN,
            &R_END_CHILDREN,
        ],
    );
    let inner = my_rai(vm, &tree_021());
    let it = new_obj(vm, NAME, vec![Value::Object(inner), 2.into()]).unwrap();
    let r = foreach_with(vm, &it, true, print_kv);
    print_exception(vm, r);
    assert_phpt(&t.output(), phpt!("iterator_023.phpt"));
}

// ---------------------------------------------------------------------------
// iterator_034: break deep and rewind
// ---------------------------------------------------------------------------

fn b_echo(vm: &mut Vm, args: &Args, m: &str) -> PResult<Value> {
    echo(vm, &format!("{NAME}::{m}()"))?;
    parent_call(vm, args, RII, m, vec![])
}
fn b_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, &format!("{NAME}::rewind() - BEGIN"))?;
    parent_call(vm, args, RII, "rewind", vec![])?;
    echo(vm, &format!("{NAME}::rewind() - DONE"))?;
    Ok(Value::Null)
}
fn b_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    b_echo(vm, args, "valid")
}
fn b_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    b_echo(vm, args, "current")
}
fn b_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    b_echo(vm, args, "key")
}
fn b_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    b_echo(vm, args, "next")
}
fn b_call_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let has = parent_call(vm, args, RII, "callHasChildren", vec![])?;
    let has = vm.to_bool(&has)?;
    let d = depth(vm, args)?;
    let res = d < num(&o, "max_depth") && has;
    let yn = |b: bool| if b { "yes" } else { "no" };
    echo(
        vm,
        &format!("{NAME}::callHasChildren({d}) = {}/{}", yn(res), yn(has)),
    )?;
    Ok(Value::Bool(res))
}
fn b_begin_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    echo(vm, &format!("{NAME}::beginChildren({d})"))?;
    parent_call(vm, args, RII, "beginChildren", vec![])
}
fn b_end_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    echo(vm, &format!("{NAME}::endChildren({d})"))?;
    parent_call(vm, args, RII, "endChildren", vec![])
}
static B_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: b_rewind,
};
static B_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): bool {}",
    handler: b_valid,
};
static B_CURRENT: NativeFunction = NativeFunction {
    sig: "function current(): mixed {}",
    handler: b_current,
};
static B_KEY: NativeFunction = NativeFunction {
    sig: "function key(): int {}",
    handler: b_key,
};
static B_NEXT: NativeFunction = NativeFunction {
    sig: "function next(): void {}",
    handler: b_next,
};
static B_CALL_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "function callHasChildren(): bool {}",
    handler: b_call_has_children,
};
static B_BEGIN_CHILDREN: NativeFunction = NativeFunction {
    sig: "function beginChildren(): void {}",
    handler: b_begin_children,
};
static B_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "function endChildren(): void {}",
    handler: b_end_children,
};

#[test]
fn break_deep_and_rewind() {
    // iterator_034.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        NAME,
        RII,
        &[
            &R_CONSTRUCT,
            &B_REWIND,
            &B_VALID,
            &B_CURRENT,
            &B_KEY,
            &B_NEXT,
            &B_CALL_HAS_CHILDREN,
            &B_BEGIN_CHILDREN,
            &B_END_CHILDREN,
        ],
    );
    let ce = iter_class(vm, "MyRecursiveArrayIterator", Kind::Recursive, None);
    let inner = new_iter(
        &ce,
        &tree_021(),
        Opts {
            echo_children: true,
            echo_only: &["rewind"],
            parens: true,
            ..Opts::default()
        },
    );
    let it = new_obj(vm, NAME, vec![Value::Object(inner), 2.into()]).unwrap();
    let mut p = 0;
    foreach_with(vm, &it, true, |vm, k, v| {
        print_kv(vm, k, v)?;
        let stop = p == 5;
        p += 1;
        if stop {
            echo(vm, "===BREAK===")?;
        }
        Ok(!stop)
    })
    .unwrap();
    echo(vm, "===FOREND===").unwrap();
    vm.call_method(&it, b"rewind", vec![]).unwrap();
    echo(vm, "===CHECK===").unwrap();
    let valid = vm.call_method(&it, b"valid", vec![]).unwrap();
    let valid = vm.to_bool(&valid).unwrap();
    echo(vm, &format!("bool({valid})")).unwrap();
    let cur = vm.call_method(&it, b"current", vec![]).unwrap();
    echo(vm, &format!("bool({})", cur.identical(&s("a")))).unwrap();
    assert_phpt(&t.output(), phpt!("iterator_034.phpt"));
}

// ---------------------------------------------------------------------------
// iterator_015 / iterator_016: HTML lists
// ---------------------------------------------------------------------------

fn h_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, "<ul>")?;
    parent_call(vm, args, RII, "rewind", vec![])
}
fn h_begin_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    echo(vm, &format!("{}<ul>", "  ".repeat(d as usize)))?;
    Ok(Value::Null)
}
fn h_end_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    echo(vm, &format!("{}</ul>", "  ".repeat(d as usize)))?;
    Ok(Value::Null)
}
fn h_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = parent_call(vm, args, RII, "valid", vec![])?;
    if !vm.to_bool(&v)? {
        echo(vm, "<ul>")?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}
fn h_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    parent_call(vm, args, RII, "__construct", vec![args.get(0).clone()])
}
fn menu_get_iterator(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "Menu::getIterator")?;
    Ok(Value::Object(super::helpers::recursive(vm, &tree_014())))
}
static H_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: h_rewind,
};
static H_BEGIN_CHILDREN: NativeFunction = NativeFunction {
    sig: "function beginChildren(): void {}",
    handler: h_begin_children,
};
static H_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "function endChildren(): void {}",
    handler: h_end_children,
};
static H_VALID: NativeFunction = NativeFunction {
    sig: "function valid(): bool {}",
    handler: h_valid,
};
static H_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "function __construct(Menu $it) {}",
    handler: h_construct,
};
static MENU_GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "function getIterator(): RecursiveArrayIterator {}",
    handler: menu_get_iterator,
};

fn print_html(vm: &mut Vm, it: &ObjRef) {
    foreach_with(vm, it, true, |vm, k, v| {
        let d = int(&vm.call_method(it, b"getDepth", vec![])?);
        let k = vm.to_php_string(&k)?;
        let v = vm.to_php_string(&v)?;
        echo(vm, &format!("{}{k}=>{v}", "  ".repeat(d as usize + 1)))?;
        Ok(true)
    })
    .unwrap();
}

#[test]
fn html_list_from_hooks() {
    // iterator_015.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        NAME,
        RII,
        &[&H_REWIND, &H_BEGIN_CHILDREN, &H_END_CHILDREN, &H_VALID],
    );
    let inner = super::helpers::recursive(vm, &tree_014());
    let it = new_obj(vm, NAME, vec![Value::Object(inner)]).unwrap();
    print_html(vm, &it);
    assert_phpt(&t.output(), phpt!("iterator_015.phpt"));
}

#[test]
fn html_list_from_aggregate() {
    // iterator_016.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mut ce = php_runtime::class::ClassEntry::new("Menu", 0);
    let iface = vm.find_class(b"IteratorAggregate").unwrap();
    engine::implement(&mut ce, &iface);
    engine::add_methods(&mut ce, &[&MENU_GET_ITERATOR]);
    vm.register_class(Rc::new(ce));
    user_class(
        vm,
        "MenuOutput",
        RII,
        &[
            &H_CONSTRUCT,
            &H_REWIND,
            &H_BEGIN_CHILDREN,
            &H_END_CHILDREN,
            &H_VALID,
        ],
    );
    let menu = new_obj(vm, "Menu", vec![]).unwrap();
    let it = new_obj(vm, "MenuOutput", vec![Value::Object(menu)]).unwrap();
    print_html(vm, &it);
    assert_phpt(&t.output(), phpt!("iterator_016.phpt"));
}

// ---------------------------------------------------------------------------
// beginIteration / endIteration / nextElement
// ---------------------------------------------------------------------------

fn bi_echo(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "MyRecursiveIteratorIterator::beginIteration()")?;
    Ok(Value::Null)
}
fn ei_echo(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "MyRecursiveIteratorIterator::endIteration()")?;
    Ok(Value::Null)
}
static BI_ECHO: NativeFunction = NativeFunction {
    sig: "function beginIteration(): void {}",
    handler: bi_echo,
};
static EI_ECHO: NativeFunction = NativeFunction {
    sig: "function endIteration(): void {}",
    handler: ei_echo,
};

fn ar_025() -> Value {
    l(vec![
        1.into(),
        2.into(),
        l(vec![31.into(), 32.into(), l(vec![331.into()])]),
        4.into(),
    ])
}

fn print_lines(vm: &mut Vm, it: &ObjRef) {
    foreach_with(vm, it, false, |vm, _, v| {
        let v = vm.to_php_string(&v)?;
        echo(vm, &v.to_string())?;
        Ok(true)
    })
    .unwrap();
}

#[test]
fn begin_and_end_iteration() {
    // iterator_025.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        "MyRecursiveIteratorIterator",
        RII,
        &[&BI_ECHO, &EI_ECHO],
    );
    let inner = super::helpers::recursive(vm, &ar_025());
    let it = new_obj(
        vm,
        "MyRecursiveIteratorIterator",
        vec![Value::Object(inner)],
    )
    .unwrap();
    print_lines(vm, &it);
    echo(vm, "===MORE===").unwrap();
    print_lines(vm, &it);
    echo(vm, "===MORE===").unwrap();
    vm.call_method(&it, b"rewind", vec![]).unwrap();
    print_lines(vm, &it);
    let valid = vm.call_method(&it, b"valid", vec![]).unwrap();
    let valid = vm.to_bool(&valid).unwrap();
    echo(vm, &format!("bool({valid})")).unwrap();
    echo(vm, "===MANUAL===").unwrap();
    vm.call_method(&it, b"rewind", vec![]).unwrap();
    let valid = vm.call_method(&it, b"valid", vec![]).unwrap();
    if vm.to_bool(&valid).unwrap() {
        let cur = vm.call_method(&it, b"current", vec![]).unwrap();
        let cur = vm.to_php_string(&cur).unwrap();
        echo(vm, &cur.to_string()).unwrap();
        vm.call_method(&it, b"next", vec![]).unwrap();
    }
    vm.call_method(&it, b"rewind", vec![]).unwrap();
    loop {
        let valid = vm.call_method(&it, b"valid", vec![]).unwrap();
        if !vm.to_bool(&valid).unwrap() {
            break;
        }
        let cur = vm.call_method(&it, b"current", vec![]).unwrap();
        let cur = vm.to_php_string(&cur).unwrap();
        echo(vm, &cur.to_string()).unwrap();
        vm.call_method(&it, b"next", vec![]).unwrap();
    }
    assert_phpt(&t.output(), phpt!("iterator_025.phpt"));
}

fn skip_first_begin_iteration(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    echo(vm, "::beginIteration() was invoked")?;
    vm.call_method(&this(args), b"next", vec![])?;
    Ok(Value::Null)
}
static SKIP_FIRST: NativeFunction = NativeFunction {
    sig: "public function beginIteration(): void {}",
    handler: skip_first_begin_iteration,
};
fn end_iteration_invoked(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "::endIteration() was invoked")?;
    Ok(Value::Null)
}
static END_ITERATION_INVOKED: NativeFunction = NativeFunction {
    sig: "public function endIteration(): void {}",
    handler: end_iteration_invoked,
};
fn next_element_invoked(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "::nextElement() was invoked")?;
    Ok(Value::Null)
}
static NEXT_ELEMENT_INVOKED: NativeFunction = NativeFunction {
    sig: "public function nextElement(): void {}",
    handler: next_element_invoked,
};

fn dump_ints(vm: &mut Vm, it: &ObjRef) {
    foreach_with(vm, it, false, |vm, _, v| {
        echo(vm, &format!("int({})", int(&v)))?;
        Ok(true)
    })
    .unwrap();
}

#[test]
fn hooks_basic() {
    // recursiveiteratoriterator_{beginiteration,enditeration,nextelement}_basic.phpt
    for (name, hook, data, file) in [
        (
            "SkipsFirstElementRecursiveIteratorIterator",
            &SKIP_FIRST,
            l(vec![1.into(), 2.into()]),
            phpt!("recursiveiteratoriterator_beginiteration_basic.phpt"),
        ),
        (
            "EndIterationRecursiveIteratorIterator",
            &END_ITERATION_INVOKED,
            l(vec![1.into(), 2.into()]),
            phpt!("recursiveiteratoriterator_enditeration_basic.phpt"),
        ),
        (
            "NextElementRecursiveIteratorIterator",
            &NEXT_ELEMENT_INVOKED,
            l(vec![1.into(), 2.into(), l(vec![3.into(), 4.into()])]),
            phpt!("recursiveiteratoriterator_nextelement_basic.phpt"),
        ),
    ] {
        let mut t = TestVm::new();
        let vm = &mut t.vm;
        let sub = super::helpers::recursive(vm, &data);
        let it = new_obj(vm, RII, vec![Value::Object(sub.clone())]).unwrap();
        dump_ints(vm, &it);
        user_class(vm, name, RII, &[hook]);
        let it = new_obj(vm, name, vec![Value::Object(sub)]).unwrap();
        dump_ints(vm, &it);
        assert_phpt(&t.output(), file);
    }
}

fn nextelement_lc(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "MyRecursiveIteratorIterator::nextelement")?;
    Ok(Value::Null)
}
static NEXTELEMENT_LC: NativeFunction = NativeFunction {
    sig: "function nextelement(): void {}",
    handler: nextelement_lc,
};

#[test]
fn child_first_calls_next_element_for_parents() {
    // iterator_071.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(vm, "MyRecursiveIteratorIterator", RII, &[&NEXTELEMENT_LC]);
    let data = l(vec![l(vec![1.into(), 2.into()]), l(vec![s("a"), s("b")])]);
    let inner = super::helpers::recursive(vm, &data);
    let it = new_obj(
        vm,
        "MyRecursiveIteratorIterator",
        vec![Value::Object(inner), 2.into()],
    )
    .unwrap();
    foreach_with(vm, &it, true, |vm, k, _| {
        echo(vm, &format!("{}", int(&k)))?;
        Ok(true)
    })
    .unwrap();
    assert_phpt(&t.output(), phpt!("iterator_071.phpt"));
}

#[test]
fn base_hooks_return_null() {
    // iterator_070.phpt, iterator_069.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = super::helpers::recursive(vm, &l(vec![]));
    let it = new_obj(vm, RII, vec![Value::Object(inner)]).unwrap();
    for m in [
        "beginIteration",
        "endIteration",
        "nextElement",
        "beginChildren",
        "endChildren",
    ] {
        assert!(vm.call_method(&it, m.as_bytes(), vec![]).unwrap().is_null());
    }
    assert_eq!(
        thrown(recursive::recursive_it_check_foreach(vm, &it, true).map(|_| Value::Null)),
        (
            "Error".into(),
            "An iterator cannot be used with foreach by reference".into()
        )
    );
    // Not a recursive iterator: no checks.
    let plain = super::helpers::plain(vm, &l(vec![]));
    recursive::recursive_it_check_foreach(vm, &plain, true).unwrap();
}

// ---------------------------------------------------------------------------
// Exceptions thrown by hooks
// ---------------------------------------------------------------------------

fn throw_exception(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", ""))
}
static THROW_BEGIN_CHILDREN: NativeFunction = NativeFunction {
    sig: "function beginchildren(): void {}",
    handler: throw_exception,
};
static THROW_CALL_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "function callHasChildren(): bool {}",
    handler: throw_exception,
};
static THROW_NEXT_ELEMENT: NativeFunction = NativeFunction {
    sig: "function nextelement(): void {}",
    handler: throw_exception,
};
static THROW_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "function endchildren(): void {}",
    handler: throw_exception,
};

fn nested_ab() -> Value {
    l(vec![l(vec![1.into(), 2.into()]), l(vec![s("a"), s("b")])])
}

#[test]
fn hook_exceptions_and_catch_get_child() {
    // recursiveIteratorIterator_{beginchildren,callHasChildren,nextelement}_error.phpt
    for hook in [
        &THROW_BEGIN_CHILDREN,
        &THROW_CALL_HAS_CHILDREN,
        &THROW_NEXT_ELEMENT,
    ] {
        let mut t = TestVm::new();
        let vm = &mut t.vm;
        user_class(vm, "MyRecursiveIteratorIterator", RII, &[hook]);
        let inner = super::helpers::recursive(vm, &nested_ab());
        let it = new_obj(
            vm,
            "MyRecursiveIteratorIterator",
            vec![Value::Object(inner.clone()), 0.into(), 16.into()],
        )
        .unwrap();
        assert!(vm.call_method(&it, b"next", vec![]).unwrap().is_null());
        let it = new_obj(
            vm,
            "MyRecursiveIteratorIterator",
            vec![Value::Object(inner), 0.into()],
        )
        .unwrap();
        assert_eq!(thrown(vm.call_method(&it, b"next", vec![])).0, "Exception");
    }
}

#[test]
fn end_children_exception() {
    // recursiveIteratorIterator_endchildren_error.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(
        vm,
        "MyRecursiveIteratorIterator",
        RII,
        &[&THROW_END_CHILDREN],
    );
    let inner = super::helpers::recursive(vm, &nested_ab());
    let it = new_obj(
        vm,
        "MyRecursiveIteratorIterator",
        vec![Value::Object(inner.clone()), 0.into(), 16.into()],
    )
    .unwrap();
    print_lines(vm, &it);
    echo(vm, "===NEXT LOOP===").unwrap();
    let it = new_obj(
        vm,
        "MyRecursiveIteratorIterator",
        vec![Value::Object(inner), 0.into()],
    )
    .unwrap();
    let r = foreach_with(vm, &it, false, |vm, _, v| {
        let v = vm.to_php_string(&v)?;
        echo(vm, &v.to_string())?;
        Ok(true)
    });
    assert_eq!(thrown(r.map(|_| Value::Null)).0, "Exception");
    assert_eq!(t.output(), "1\n2\na\nb\n===NEXT LOOP===\n1\n2\n");
}

// ---------------------------------------------------------------------------
// Depth, sub-iterators and limits
// ---------------------------------------------------------------------------

#[test]
fn max_depth() {
    // iterator_028.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ar = l(vec![
        1.into(),
        2.into(),
        l(vec![
            31.into(),
            32.into(),
            l(vec![
                331.into(),
                l(vec![3321.into(), l(vec![33221.into()])]),
            ]),
        ]),
        4.into(),
    ]);
    let inner = super::helpers::recursive(vm, &ar);
    let it = new_obj(vm, RII, vec![Value::Object(inner)]).unwrap();
    let show = |vm: &mut Vm| {
        let d = vm.call_method(&it, b"getMaxDepth", vec![]).unwrap();
        match d {
            Value::Bool(b) => echo(vm, &format!("bool({b})")).unwrap(),
            v => echo(vm, &format!("int({})", int(&v))).unwrap(),
        }
        foreach_with(vm, &it, false, |vm, _, v| {
            let depth = int(&vm.call_method(&it, b"getDepth", vec![])?);
            echo(vm, &format!("{depth}: {}", int(&v)))?;
            Ok(true)
        })
        .unwrap();
    };
    echo(vm, "===?===").unwrap();
    show(vm);
    for (label, arg) in [
        ("2", Some(2)),
        ("X", None),
        ("3", Some(3)),
        ("5", Some(5)),
        ("0", Some(0)),
    ] {
        echo(vm, &format!("==={label}===")).unwrap();
        let a = arg.map(Value::Int).into_iter().collect();
        vm.call_method(&it, b"setMaxDepth", a).unwrap();
        show(vm);
    }
    echo(vm, "===-1===").unwrap();
    vm.call_method(&it, b"setMaxDepth", vec![(-1).into()])
        .unwrap();
    let d = vm.call_method(&it, b"getMaxDepth", vec![]).unwrap();
    echo(vm, &format!("bool({})", d.identical(&Value::Bool(true)))).unwrap();
    vm.call_method(&it, b"setMaxDepth", vec![4.into()]).unwrap();
    let r = vm
        .call_method(&it, b"setMaxDepth", vec![(-2).into()])
        .map(|_| ());
    print_exception(vm, r);
    let d = vm.call_method(&it, b"getMaxDepth", vec![]).unwrap();
    echo(vm, &format!("int({})", int(&d))).unwrap();
    assert_phpt(&t.output(), phpt!("iterator_028.phpt"));
}

#[test]
fn sub_iterators() {
    // recursiveiteratoriterator_getsubiterator_{basic,variation,variation_002,variation_003}.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let data = l(vec![1.into(), 2.into(), l(vec![3.into(), 4.into()])]);
    let sub = super::helpers::recursive(vm, &data);
    let other = super::helpers::recursive(vm, &data);
    let it = new_obj(vm, RII, vec![Value::Object(sub.clone())]).unwrap();
    let get = |vm: &mut Vm, a: Vec<Value>| vm.call_method(&it, b"getSubIterator", a).unwrap();
    let cur = get(vm, vec![]);
    assert!(ObjRef::ptr_eq(cur.as_object().unwrap(), &sub));
    assert!(!ObjRef::ptr_eq(cur.as_object().unwrap(), &other));
    vm.call_method(&it, b"next", vec![]).unwrap();
    vm.call_method(&it, b"next", vec![]).unwrap();
    assert!(ObjRef::ptr_eq(get(vm, vec![]).as_object().unwrap(), &sub));
    vm.call_method(&it, b"next", vec![]).unwrap();
    let child = get(vm, vec![]);
    let child = child.as_object().unwrap();
    assert!(!ObjRef::ptr_eq(child, &sub));
    assert_eq!(int(&vm.call_method(child, b"current", vec![]).unwrap()), 3);
    assert!(get(vm, vec![(-1).into()]).is_null());
    assert!(ObjRef::ptr_eq(
        get(vm, vec![0.into()]).as_object().unwrap(),
        &sub
    ));
    assert!(ObjRef::ptr_eq(
        get(vm, vec![1.into()]).as_object().unwrap(),
        child
    ));
    assert!(get(vm, vec![2.into()]).is_null());
    assert!(ObjRef::ptr_eq(
        vm.call_method(&it, b"getInnerIterator", vec![])
            .unwrap()
            .as_object()
            .unwrap(),
        child
    ));

    let sub = super::helpers::recursive(vm, &l(vec![1.into()]));
    let it = new_obj(vm, RII, vec![Value::Object(sub)]).unwrap();
    for _ in 0..2 {
        vm.call_method(&it, b"next", vec![]).unwrap();
        assert!(
            !vm.call_method(&it, b"getSubIterator", vec![])
                .unwrap()
                .is_null()
        );
    }
}

#[test]
fn string_and_negative_keys() {
    // spl_recursive_iterator_iterator_key_case.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ar = assoc(vec![
        (skey("one"), 1.into()),
        (skey("two"), 2.into()),
        (
            skey("three"),
            assoc(vec![
                (skey("four"), 4.into()),
                (skey("five"), 5.into()),
                (skey("six"), assoc(vec![(skey("seven"), 7.into())])),
            ]),
        ),
        (skey("eight"), 8.into()),
        (ArrayKey::Int(-100), 10.into()),
        (skey(""), s("null")),
    ]);
    let inner = super::helpers::recursive(vm, &ar);
    let it = new_obj(vm, RII, vec![Value::Object(inner)]).unwrap();
    foreach_with(vm, &it, true, |vm, k, v| {
        let ks = vm.to_php_string(&k)?;
        let vs = vm.to_php_string(&v)?;
        echo(vm, &format!("{ks}=>{vs}"))?;
        match k {
            Value::Int(i) => echo(vm, &format!("int({i})"))?,
            Value::String(s) => echo(vm, &format!("string({}) \"{s}\"", s.len()))?,
            other => panic!("{other:?}"),
        }
        Ok(true)
    })
    .unwrap();
    assert_phpt(
        &t.output(),
        phpt!("spl_recursive_iterator_iterator_key_case.phpt"),
    );
}

// ---------------------------------------------------------------------------
// Construction errors and uninitialised objects
// ---------------------------------------------------------------------------

fn plain_get_iterator(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Object(super::helpers::plain(vm, &l(vec![1.into()]))))
}
static PLAIN_GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "function getIterator(): Iterator {}",
    handler: plain_get_iterator,
};
fn ignore_construct(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}
static IGNORE_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "public function __construct($str) {}",
    handler: ignore_construct,
};

#[test]
fn construction_errors() {
    // iterator_024.phpt, iterator_062.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mut ce = php_runtime::class::ClassEntry::new("PlainAggregate", 0);
    let iface = vm.find_class(b"IteratorAggregate").unwrap();
    engine::implement(&mut ce, &iface);
    engine::add_methods(&mut ce, &[&PLAIN_GET_ITERATOR]);
    vm.register_class(Rc::new(ce));
    let agg = new_obj(vm, "PlainAggregate", vec![]).unwrap();
    let invalid = (
        "InvalidArgumentException".to_string(),
        "An instance of RecursiveIterator or IteratorAggregate creating it is required".to_string(),
    );
    assert_eq!(
        thrown(new_obj(vm, RII, vec![Value::Object(agg)]).map(Value::Object)),
        invalid
    );
    let plain = super::helpers::plain(vm, &l(vec![]));
    assert_eq!(
        thrown(new_obj(vm, RII, vec![Value::Object(plain)]).map(Value::Object)),
        invalid
    );
    user_class(vm, "myRecursiveIteratorIterator", RII, &[]);
    assert_eq!(
        thrown(new_obj(vm, "myRecursiveIteratorIterator", vec![]).map(Value::Object)),
        (
            "ArgumentCountError".into(),
            "RecursiveIteratorIterator::__construct() expects at least 1 argument, 0 given".into()
        )
    );
}

#[test]
fn uninitialised_objects() {
    // RecursiveIteratorIterator_not_initialized.phpt, bug41828.phpt, bug54281.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = vm.find_class(RII.as_bytes()).unwrap();
    let raw = vm.instantiate(&ce).unwrap();
    assert_eq!(
        thrown(recursive::recursive_it_check_foreach(vm, &raw, false).map(|_| Value::Null)),
        ("Error".into(), "Object is not initialized".into())
    );
    user_class(vm, "foo", RII, &[&IGNORE_CONSTRUCT]);
    let foo = new_obj(vm, "foo", vec![s("This is bar")]).unwrap();
    assert_eq!(
        thrown(recursive::recursive_it_forward_target(vm, &foo).map(|_| Value::Null)),
        (
            "Error".into(),
            "The foo instance wasn't initialized properly".into()
        )
    );
    for m in [
        "rewind",
        "next",
        "key",
        "current",
        "getInnerIterator",
        "callGetChildren",
    ] {
        assert_eq!(
            thrown(vm.call_method(&foo, m.as_bytes(), vec![])),
            (
                "Error".into(),
                crate::spl::iterators::dual::INVALID_STATE.into()
            ),
            "{m}"
        );
    }
    assert!(
        vm.call_method(&foo, b"valid", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
    assert!(
        vm.call_method(&foo, b"callHasChildren", vec![])
            .unwrap()
            .identical(&Value::Bool(false))
    );
    // Initialised: unknown methods go to the current sub-iterator.
    let inner = super::helpers::recursive(vm, &l(vec![1.into()]));
    let it = new_obj(vm, RII, vec![Value::Object(inner.clone())]).unwrap();
    let target = recursive::recursive_it_forward_target(vm, &it)
        .unwrap()
        .unwrap();
    assert!(ObjRef::ptr_eq(&target, &inner));
    let plain = super::helpers::plain(vm, &l(vec![]));
    assert!(
        recursive::recursive_it_forward_target(vm, &plain)
            .unwrap()
            .is_none()
    );
    let clone = it.class().handlers.clone_obj.unwrap();
    assert_eq!(thrown(clone(vm, &it).map(Value::Object)).0, "Error");
}

#[test]
fn constructor_can_be_called_again() {
    // gh16604_1.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = super::helpers::recursive(vm, &l(vec![1.into()]));
    for class in [RII, "RecursiveTreeIterator"] {
        let it = new_obj(vm, class, vec![Value::Object(inner.clone())]).unwrap();
        vm.call_method(&it, b"__construct", vec![Value::Object(inner.clone())])
            .unwrap();
    }
}

// ---------------------------------------------------------------------------
// Re-entrancy
// ---------------------------------------------------------------------------

fn reenter_end_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let fired = num(&o, "fired");
    o.set_prop(b"fired", Value::Int(fired + 1));
    if fired == 0 {
        vm.call_method(&o, b"next", vec![])?;
    }
    Ok(Value::Null)
}
static REENTER_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "function endChildren(): void {}",
    handler: reenter_end_children,
};

fn seen_line(vm: &mut Vm, it: &ObjRef) -> String {
    let mut seen = Vec::new();
    foreach_with(vm, it, false, |_, _, v| {
        seen.push(match v.unref() {
            Value::Array(_) => "array".to_string(),
            Value::Int(i) => i.to_string(),
            other => format!("{other:?}"),
        });
        if seen.len() > 20 {
            seen.push("...".into());
            return Ok(false);
        }
        Ok(true)
    })
    .unwrap();
    seen.join(" ")
}

fn tree_reenter() -> Value {
    l(vec![
        l(vec![l(vec![1.into(), 2.into()]), 3.into()]),
        4.into(),
    ])
}

#[test]
fn next_during_end_children() {
    // recursiveiteratoriterator_next_during_endchildren.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(vm, "ReenterRII", RII, &[&REENTER_END_CHILDREN]);
    let inner = super::helpers::recursive(vm, &tree_reenter());
    let it = new_obj(vm, "ReenterRII", vec![Value::Object(inner), 1.into()]).unwrap();
    assert_eq!(seen_line(vm, &it), "array array 1 2 4");
}

#[test]
fn next_during_valid() {
    // recursiveiteratoriterator_next_during_valid.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let rii: Rc<RefCell<Option<ObjRef>>> = Rc::default();
    let fired = counter();
    let hook: Hook = {
        let rii = rii.clone();
        Rc::new(move |vm, method, _this, depth, pos, len| {
            if method == "valid" && depth == 2 && pos == len && fired.get() == 0 {
                fired.set(1);
                let rii = rii.borrow().clone().expect("rii");
                vm.call_method(&rii, b"next", vec![])?;
            }
            Ok(())
        })
    };
    let ce = iter_class(vm, "RIt", Kind::Recursive, None);
    let root = new_iter(
        &ce,
        &tree_reenter(),
        Opts {
            hook: Some(hook),
            ..Opts::default()
        },
    );
    let it = new_obj(vm, RII, vec![Value::Object(root), 1.into()]).unwrap();
    *rii.borrow_mut() = Some(it.clone());
    assert_eq!(seen_line(vm, &it), "array array 1 2 3 4");
}

#[test]
fn rewind_during_next() {
    // recursiveiteratoriterator_rewind_during_next.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let rii: Rc<RefCell<Option<ObjRef>>> = Rc::default();
    let fired = counter();
    let hook: Hook = {
        let rii = rii.clone();
        Rc::new(move |vm, method, _this, depth, pos, _len| {
            if method == "next" && depth == 1 && pos == 1 && fired.get() == 0 {
                fired.set(1);
                let rii = rii.borrow().clone().expect("rii");
                vm.call_method(&rii, b"rewind", vec![])?;
            }
            Ok(())
        })
    };
    let ce = iter_class(vm, "Reenter", Kind::Recursive, None);
    let root = new_iter(
        &ce,
        &l(vec![
            l(vec![10.into(), 11.into()]),
            l(vec![20.into(), 21.into()]),
        ]),
        Opts {
            hook: Some(hook),
            ..Opts::default()
        },
    );
    let it = new_obj(vm, RII, vec![Value::Object(root), 1.into()]).unwrap();
    *rii.borrow_mut() = Some(it.clone());
    let mut seen = Vec::new();
    foreach_with(vm, &it, false, |vm, _, v| {
        seen.push(match v.unref() {
            Value::Array(a) => {
                let parts: Vec<String> = a
                    .values()
                    .map(|x| vm.to_php_string(x).unwrap().to_string())
                    .collect();
                format!("[{}]", parts.join(","))
            }
            other => vm.to_php_string(&other)?.to_string(),
        });
        Ok(seen.len() <= 20)
    })
    .unwrap();
    assert_eq!(seen.join(" "), "[10,11] 10 [10,11] 10 11 [20,21] 20 21");
}

fn dummy_rewind(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    echo(vm, "dummy")?;
    Ok(Value::Null)
}
fn rewinding_end_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let d = depth(vm, args)?;
    vm.echo_str(&d.to_string())?;
    let o = this(args);
    let count = num(&o, "count") - 1;
    o.set_prop(b"count", Value::Int(count));
    if count > 0 {
        parent_call(vm, args, RII, "rewind", vec![])?;
    }
    Ok(Value::Null)
}
static DUMMY_REWIND: NativeFunction = NativeFunction {
    sig: "function rewind(): void {}",
    handler: dummy_rewind,
};
static REWINDING_END_CHILDREN: NativeFunction = NativeFunction {
    sig: "function endChildren(): void {}",
    handler: rewinding_end_children,
};

#[test]
fn rewind_from_end_children() {
    // bug69970.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    user_class(vm, NAME, RII, &[&DUMMY_REWIND, &REWINDING_END_CHILDREN]);
    let arr = l(vec![s("a"), l(vec![s("ba"), l(vec![s("bba"), s("bbb")])])]);
    let inner = super::helpers::recursive(vm, &arr);
    let it = new_obj(vm, NAME, vec![Value::Object(inner)]).unwrap();
    it.set_prop(b"count", Value::Int(10));
    foreach_with(vm, &it, true, |vm, k, v| {
        let d = int(&vm.call_method(&it, b"getDepth", vec![])?);
        let k = vm.to_php_string(&k)?;
        let v = vm.to_php_string(&v)?;
        echo(vm, &format!("{d}{k}=>{v}"))?;
        Ok(true)
    })
    .unwrap();
    assert_phpt(&t.output(), phpt!("bug69970.phpt"));
}

#[test]
fn call_get_children_of_leaf() {
    // spl_iterator_getcallchildren.phpt (RecursiveArrayIterator children)
    let mut t = TestVm::new();
    if t.vm.find_class(b"RecursiveArrayIterator").is_none() {
        return;
    }
    let vm = &mut t.vm;
    let data = l(vec![
        l(vec![7.into(), 8.into(), 9.into()]),
        1.into(),
        2.into(),
        3.into(),
        l(vec![4.into(), 5.into(), 6.into()]),
    ]);
    let rai = new_obj(vm, "RecursiveArrayIterator", vec![data]).unwrap();
    let it = new_obj(vm, RII, vec![Value::Object(rai)]).unwrap();
    let cur = vm.call_method(&it, b"current", vec![]).unwrap();
    assert_eq!(crate::spl::testutil::array_of(&cur).len(), 3);
    vm.call_method(&it, b"next", vec![]).unwrap();
    assert_eq!(int(&vm.call_method(&it, b"current", vec![]).unwrap()), 7);
    // php-src parses the argument with ZPP "A": "must be of type array".
    assert_eq!(
        thrown(vm.call_method(&it, b"callGetChildren", vec![])),
        (
            "TypeError".into(),
            "ArrayIterator::__construct(): Argument #1 ($array) must be of type array, int given"
                .into()
        )
    );
}

// ---------------------------------------------------------------------------
// RecursiveTreeIterator
// ---------------------------------------------------------------------------

fn tree_ary() -> Value {
    assoc(vec![
        (ArrayKey::Int(0), l(vec![s("a"), 1.into()])),
        (
            skey("a"),
            assoc(vec![
                (ArrayKey::Int(0), 2.into()),
                (ArrayKey::Int(1), s("b")),
                (ArrayKey::Int(3), l(vec![4.into(), s("c")])),
            ]),
        ),
    ])
}

fn print_tree(vm: &mut Vm, it: &ObjRef) -> PResult<()> {
    foreach_with(vm, it, true, |vm, k, v| {
        let k = vm.to_php_string(&k)?;
        let v = vm.to_php_string(&v)?;
        echo(vm, &format!("[{k}] => {v}"))?;
        Ok(true)
    })
}

fn tree_flag_runs(vm: &mut Vm, source: &dyn Fn(&mut Vm) -> Value) {
    for (label, args) in [
        ("BYPASS_KEY", vec![]),
        ("BYPASS_CURRENT", vec![Value::Int(4)]),
        ("BYPASS_KEY|BYPASS_KEY", vec![Value::Int(12)]),
        ("0", vec![Value::Int(0)]),
        (
            "0, caching_it_flags = CachingIterator::CATCH_GET_CHILD",
            vec![Value::Int(0), Value::Int(16)],
        ),
    ] {
        echo(vm, &format!("-- flags = {label} --")).unwrap();
        let mut a = vec![source(vm)];
        a.extend(args);
        let it = new_obj(vm, "RecursiveTreeIterator", a).unwrap();
        print_tree(vm, &it).unwrap();
    }
}

#[test]
fn tree_iterator_flags() {
    // recursive_tree_iterator_001.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = super::helpers::recursive(vm, &tree_ary());
    tree_flag_runs(vm, &|_| Value::Object(inner.clone()));
    assert_phpt(&t.output(), phpt!("recursive_tree_iterator_001.phpt"));
}

fn tree_get_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match this(args).get_prop(b"it") {
        Some(v) => Ok(v),
        None => Ok(Value::Object(super::helpers::recursive(vm, &tree_ary()))),
    }
}
static TREE_GET_ITERATOR: NativeFunction = NativeFunction {
    sig: "function getIterator(): Traversable {}",
    handler: tree_get_iterator,
};

#[test]
fn tree_iterator_over_aggregate() {
    // recursive_tree_iterator_006.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let mut ce = php_runtime::class::ClassEntry::new("RecursiveArrayIteratorAggregated", 0);
    let iface = vm.find_class(b"IteratorAggregate").unwrap();
    engine::implement(&mut ce, &iface);
    engine::add_methods(&mut ce, &[&TREE_GET_ITERATOR]);
    vm.register_class(Rc::new(ce));
    let agg = new_obj(vm, "RecursiveArrayIteratorAggregated", vec![]).unwrap();
    let inner = super::helpers::recursive(vm, &tree_ary());
    agg.set_prop(b"it", Value::Object(inner));
    tree_flag_runs(vm, &|_| Value::Object(agg.clone()));
    assert_phpt(&t.output(), phpt!("recursive_tree_iterator_006.phpt"));
}

#[test]
fn tree_iterator_parts() {
    // recursive_tree_iterator_004.phpt, recursive_tree_iterator_setpostfix.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = super::helpers::recursive(vm, &tree_ary());
    let it = new_obj(vm, "RecursiveTreeIterator", vec![Value::Object(inner)]).unwrap();
    foreach_with(vm, &it, true, |vm, _, _| {
        let k = vm.call_method(&it, b"key", vec![])?;
        let k = vm.to_php_string(&k)?;
        let mut line = format!("[{k}] => ");
        for m in ["getPrefix", "getEntry", "getPostfix"] {
            let part = vm.call_method(&it, m.as_bytes(), vec![])?;
            line.push_str(&vm.to_php_string(&part)?.to_string());
        }
        echo(vm, &line)?;
        Ok(true)
    })
    .unwrap();
    assert_phpt(&t.output(), phpt!("recursive_tree_iterator_004.phpt"));

    let vm = &mut t.vm;
    let inner = super::helpers::recursive(vm, &tree_ary());
    let it = new_obj(vm, "RecursiveTreeIterator", vec![Value::Object(inner)]).unwrap();
    let postfix = |vm: &mut Vm| {
        echo(vm, "----").unwrap();
        let p = vm.call_method(&it, b"getPostfix", vec![]).unwrap();
        let p = vm.to_php_string(&p).unwrap();
        vm.echo_str(&p.to_string()).unwrap();
        vm.echo_str("\n\n").unwrap();
    };
    postfix(vm);
    vm.call_method(&it, b"setPostfix", vec![s("POSTFIX")])
        .unwrap();
    postfix(vm);
    echo(vm, "----").unwrap();
    print_tree(vm, &it).unwrap();
    vm.call_method(&it, b"setPostfix", vec![s("")]).unwrap();
    postfix(vm);
    echo(vm, "----").unwrap();
    print_tree(vm, &it).unwrap();
    assert_phpt(
        &t.output(),
        phpt!("recursive_tree_iterator_setpostfix.phpt"),
    );
}

#[test]
fn tree_iterator_prefix_parts() {
    // recursive_tree_iterator_008.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ary = assoc(vec![
        (skey("a"), l(vec![s("b")])),
        (skey("c"), l(vec![s("d")])),
    ]);
    let inner = super::helpers::recursive(vm, &ary);
    let it = new_obj(vm, "RecursiveTreeIterator", vec![Value::Object(inner)]).unwrap();
    for i in 0..6 {
        vm.call_method(&it, b"setPrefixPart", vec![i.into(), s(&i.to_string())])
            .unwrap();
    }
    print_tree(vm, &it).unwrap();
    for part in [-1, 6] {
        let r = vm
            .call_method(&it, b"setPrefixPart", vec![part.into(), s("")])
            .map(|_| ());
        print_exception(vm, r);
    }
    assert_phpt(&t.output(), phpt!("recursive_tree_iterator_008.phpt"));
}

#[test]
fn tree_iterator_errors() {
    // recursive_tree_iterator_002.phpt, recursive_tree_iterator_007.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(
        thrown(new_obj(vm, "RecursiveTreeIterator", vec![]).map(Value::Object)),
        (
            "ArgumentCountError".into(),
            "RecursiveTreeIterator::__construct() expects at least 1 argument, 0 given".into()
        )
    );
    // RecursiveCachingIterator::__construct() requires a RecursiveIterator.
    let plain = super::helpers::plain(vm, &l(vec![]));
    assert_eq!(
        thrown(
            new_obj(vm, "RecursiveCachingIterator", vec![Value::Object(plain)])
                .map(Value::Object)
        ),
        (
            "TypeError".into(),
            "RecursiveCachingIterator::__construct(): Argument #1 ($iterator) must be of type RecursiveIterator, TIter given".into()
        )
    );
    // An entry that cannot be converted to string.
    let std = vm.find_class(b"stdClass").unwrap();
    let obj = Value::Object(ObjRef::new(std));
    let inner = super::helpers::recursive(vm, &l(vec![l(vec![s("string")]), l(vec![obj])]));
    let it = new_obj(vm, "RecursiveTreeIterator", vec![Value::Object(inner)]).unwrap();
    let r = print_tree(vm, &it);
    print_exception(vm, r);
    assert_eq!(
        t.output(),
        "[0] => |-Array\n[0] => | \\-string\n[1] => \\-Array\n\
         Error: Object of class stdClass could not be converted to string\n"
    );
}

#[test]
fn tree_iterator_uninitialised() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ce = vm.find_class(b"RecursiveTreeIterator").unwrap();
    let raw = vm.instantiate(&ce).unwrap();
    for m in ["getPrefix", "getEntry", "getPostfix", "current", "key"] {
        assert_eq!(
            thrown(vm.call_method(&raw, m.as_bytes(), vec![])).0,
            "Error",
            "{m}"
        );
    }
    // The setters work without the constructor.
    vm.call_method(&raw, b"setPostfix", vec![s("x")]).unwrap();
    vm.call_method(&raw, b"setPrefixPart", vec![0.into(), s("x")])
        .unwrap();
}
