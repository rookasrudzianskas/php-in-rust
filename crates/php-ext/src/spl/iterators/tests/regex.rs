//! RegexIterator and RecursiveRegexIterator (over test iterators instead
//! of ArrayIterator).

use php_runtime::{Args, ArrayKey, Flow, NativeFunction, ObjRef, PResult, PhpRef, Value, Vm};

use super::helpers::*;
use crate::spl::engine;
use crate::spl::testutil::{assoc, list, new_obj, s, skey, user_class};
use crate::testing::{TestVm, thrown};

const MATCH: i64 = 0;
const GET_MATCH: i64 = 1;
const ALL_MATCHES: i64 = 2;
const SPLIT: i64 = 3;
const REPLACE: i64 = 4;
const USE_KEY: i64 = 1;
const INVERT_MATCH: i64 = 2;

fn l(v: Vec<Value>) -> Value {
    list(v)
}

fn regex(vm: &mut Vm, inner: &ObjRef, pattern: &str, more: Vec<Value>) -> ObjRef {
    let mut a = vec![Value::Object(inner.clone()), s(pattern)];
    a.extend(more);
    new_obj(vm, "RegexIterator", a).unwrap()
}

fn show(vm: &mut Vm, it: &ObjRef) {
    foreach_with(vm, it, true, |vm, k, v| {
        var_dump(vm, &k)?;
        var_dump(vm, &v)?;
        Ok(true)
    })
    .unwrap();
}

/// The phpt expectation up to the final `var_dump($ar)` of the ArrayIterator.
fn expect_before_dump(src: &str) -> String {
    let (expected, _) = phpt_expect(src);
    let end = expected
        .find("object(ArrayIterator)")
        .unwrap_or(expected.len());
    expected[..end].replace("%s(", "string(")
}

/// array('1','1,2','1,2,3','',NULL,array(),'FooBar',',',',,')
fn subjects() -> Value {
    l(vec![
        s("1"),
        s("1,2"),
        s("1,2,3"),
        s(""),
        Value::Null,
        l(vec![]),
        s("FooBar"),
        s(","),
        s(",,"),
    ])
}

/// array('1'=>0,'1,2'=>1,'1,2,3'=>2,0=>3,'FooBar'=>4,','=>5,',,'=>6)
fn keyed_subjects() -> Value {
    assoc(vec![
        (ArrayKey::Int(1), 0.into()),
        (skey("1,2"), 1.into()),
        (skey("1,2,3"), 2.into()),
        (ArrayKey::Int(0), 3.into()),
        (skey("FooBar"), 4.into()),
        (skey(","), 5.into()),
        (skey(",,"), 6.into()),
    ])
}

#[test]
fn match_values_and_keys() {
    // iterator_029.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ar = assoc(vec![
        (ArrayKey::Int(0), 0.into()),
        (ArrayKey::Int(1), s("123")),
        (ArrayKey::Int(2), 123.into()),
        (ArrayKey::Int(22), s("abc")),
        (ArrayKey::Int(23), s("a2b")),
        (ArrayKey::Int(24), 22.into()),
        (skey("a2d"), 7.into()),
        (ArrayKey::Int(25), 42.into()),
    ]);
    let print = |vm: &mut Vm, it: &ObjRef| {
        let pairs = foreach_kv(vm, it).unwrap();
        let lines = kv_lines(vm, &pairs);
        vm.echo_str(&lines).unwrap();
    };
    let inner = plain(vm, &ar);
    let it = regex(vm, &inner, "/2/", vec![]);
    print(vm, &it);
    echo(vm, "===KEY===").unwrap();
    let it = regex(vm, &inner, "/2/", vec![0.into(), USE_KEY.into()]);
    print(vm, &it);
    assert_phpt(&t.output(), phpt!("iterator_029.phpt"));
}

#[test]
fn get_match_mode() {
    // iterator_050.phpt, iterator_051.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &subjects());
    let it = regex(vm, &inner, "/(\\d),(\\d)/", vec![GET_MATCH.into()]);
    show(vm, &it);
    let it = regex(vm, &inner, "/(\\d)/", vec![GET_MATCH.into()]);
    show(vm, &it);
    assert_eq!(t.output(), expect_before_dump(phpt!("iterator_050.phpt")));

    let vm = &mut t.vm;
    let inner = plain(vm, &keyed_subjects());
    let it = regex(
        vm,
        &inner,
        "/(\\d),(\\d)/",
        vec![GET_MATCH.into(), USE_KEY.into()],
    );
    show(vm, &it);
    let it = regex(
        vm,
        &inner,
        "/(\\d)/",
        vec![GET_MATCH.into(), USE_KEY.into()],
    );
    show(vm, &it);
    assert_eq!(t.output(), expect_before_dump(phpt!("iterator_051.phpt")));
}

/// `accept()` of iterator_052/053: compares the matches of
/// `preg_match_all()` with what `RegexIterator::accept()` stored.
fn all_matches_accept(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let uk = this
        .get_prop(b"uk")
        .is_some_and(|v| v.identical(&Value::Int(1)));
    let re = this.get_prop(b"re").unwrap_or(Value::Null);
    let subject = if uk {
        vm.call_method(&this, b"key", vec![])?
    } else {
        vm.call_method(&this, b"current", vec![])?
    };
    let sub = PhpRef::new(Value::Null);
    let token = vm.begin_silence();
    let subject = vm.to_php_string(&subject);
    let r = subject.and_then(|subject| {
        vm.call_native(
            "preg_match_all",
            vec![re, Value::String(subject), Value::Ref(sub.clone())],
        )
    });
    vm.end_silence(token);
    r?;
    let ret = parent_call(vm, args, "RegexIterator", "accept", vec![])?;
    let cur = vm.call_method(&this, b"current", vec![])?;
    let same = vm.is_equal(&sub.get(), &cur)?;
    var_dump(vm, &Value::Bool(same))?;
    Ok(ret)
}
static ALL_MATCHES_ACCEPT: NativeFunction = NativeFunction {
    sig: "function accept(): bool {}",
    handler: all_matches_accept,
};

fn my_regex(vm: &mut Vm, inner: &ObjRef, re: &str, flags: i64) -> ObjRef {
    let it = new_obj(
        vm,
        "MyRegexIterator",
        vec![
            Value::Object(inner.clone()),
            s(re),
            ALL_MATCHES.into(),
            flags.into(),
        ],
    )
    .unwrap();
    it.set_prop(b"uk", Value::Int(flags & USE_KEY));
    it.set_prop(b"re", s(re));
    it
}

#[test]
fn all_matches_mode() {
    // iterator_052.phpt, iterator_053.phpt
    for (flags, data, file) in [
        (0, subjects(), phpt!("iterator_052.phpt")),
        (USE_KEY, subjects(), phpt!("iterator_053.phpt")),
    ] {
        let mut t = TestVm::new();
        let vm = &mut t.vm;
        user_class(
            vm,
            "MyRegexIterator",
            "RegexIterator",
            &[&ALL_MATCHES_ACCEPT],
        );
        let inner = plain(vm, &data);
        let it = my_regex(vm, &inner, "/(\\d),(\\d)/", flags);
        show(vm, &it);
        let it = my_regex(vm, &inner, "/(\\d)/", flags);
        show(vm, &it);
        assert_eq!(t.output(), expect_before_dump(file));
    }
}

#[test]
fn split_mode() {
    // iterator_054.phpt, iterator_055.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &subjects());
    let it = regex(vm, &inner, "/,/", vec![SPLIT.into()]);
    show(vm, &it);
    assert_eq!(t.output(), expect_before_dump(phpt!("iterator_054.phpt")));

    let vm = &mut t.vm;
    let inner = plain(vm, &keyed_subjects());
    let it = regex(
        vm,
        &inner,
        "/(\\d),(\\d)/",
        vec![SPLIT.into(), USE_KEY.into()],
    );
    show(vm, &it);
    assert_eq!(t.output(), expect_before_dump(phpt!("iterator_055.phpt")));
}

#[test]
fn replace_mode() {
    // bug50579.phpt, bug54304.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let data = assoc(vec![
        (skey("test1"), s("test888")),
        (skey("test2"), s("what?")),
        (skey("test3"), s("test999")),
    ]);
    let print = |vm: &mut Vm, it: &ObjRef| {
        let pairs = foreach_kv(vm, it).unwrap();
        let lines = kv_lines(vm, &pairs);
        vm.echo_str(&lines).unwrap();
    };
    let h = plain(vm, &data);
    let i = regex(vm, &h, "/^test(.*)/", vec![REPLACE.into()]);
    i.set_prop(b"replacement", s("[$0]"));
    print(vm, &i);
    i.set_prop(b"replacement", s("$1"));
    print(vm, &i);
    let h = plain(vm, &data);
    let i = regex(vm, &h, "/^test(.*)/", vec![REPLACE.into()]);
    i.set_prop(b"replacement", s("[$1]"));
    print(vm, &i);
    assert_phpt(&t.output(), phpt!("bug50579.phpt"));

    let vm = &mut t.vm;
    let h = plain(vm, &assoc(vec![(skey("test3"), s("test999"))]));
    let i = regex(vm, &h, "/^test(.*)/", vec![REPLACE.into()]);
    i.set_prop(b"replacement", s("42"));
    let r = i.get_prop(b"replacement").unwrap();
    var_dump(vm, &r).unwrap();
    show(vm, &i);
    let r = i.get_prop(b"replacement").unwrap();
    var_dump(vm, &r).unwrap();
    assert_phpt(&t.output(), phpt!("bug54304.phpt"));
}

#[test]
fn replace_mode_with_reference_and_default() {
    // RegexIterator_with_reference_replacement.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![s("test1"), s("test2"), s("test3")]));
    let it = regex(vm, &inner, "/^(test)(\\d+)/", vec![REPLACE.into()]);
    // `$i->replacement =& $r`
    let r = PhpRef::new(s("$2:$1"));
    let slot = it.class().find_property(b"replacement").unwrap().slot;
    it.obj().props.borrow_mut()[slot] = Value::Ref(r);
    let arr = vm
        .call_native("iterator_to_array", vec![Value::Object(it)])
        .unwrap();
    var_dump(vm, &arr).unwrap();
    assert_phpt(
        &t.output(),
        phpt!("RegexIterator_with_reference_replacement.phpt"),
    );

    // Without a replacement (null) matches are removed.
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![s("test1"), s("other")]));
    let it = regex(vm, &inner, "/^test/", vec![REPLACE.into()]);
    assert!(it.get_prop(b"replacement").unwrap().is_null());
    let pairs = foreach_kv(vm, &it).unwrap();
    assert_eq!(kv_lines(vm, &pairs), "0=>1\n");
}

#[test]
fn replace_with_backtrack_limit_error() {
    // gh14290.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    vm.ini_set("pcre.backtrack_limit", "2");
    vm.ini_set("pcre.jit", "0");
    let h = plain(vm, &assoc(vec![(skey("test"), s("test1"))]));
    let i = regex(vm, &h, "/^test(.*)/", vec![REPLACE.into()]);
    assert!(foreach_kv(vm, &i).unwrap().is_empty());
}

#[test]
fn invert_match_and_use_key() {
    // bug66702.phpt, bug68128-USE_KEY.phpt, bug68128.phpt (first part)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let it = plain(vm, &l(vec![s("foo"), s("bar"), s("baz")]));
    let strs = |vm: &mut Vm, it: &ObjRef| {
        let pairs = foreach_kv(vm, it).unwrap();
        kv_strings(vm, &pairs)
    };
    let r = regex(vm, &it, "/^ba/", vec![MATCH.into()]);
    assert_eq!(
        strs(vm, &r),
        [("1".into(), "bar".into()), ("2".into(), "baz".into())]
    );
    let r = regex(vm, &it, "/^ba/", vec![MATCH.into(), INVERT_MATCH.into()]);
    assert_eq!(strs(vm, &r), [("0".to_string(), "foo".to_string())]);
    let it = plain(
        vm,
        &assoc(vec![
            (skey("foo"), 1.into()),
            (skey("bar"), 2.into()),
            (skey("baz"), 3.into()),
        ]),
    );
    let r = regex(vm, &it, "/^ba/", vec![MATCH.into(), USE_KEY.into()]);
    assert_eq!(
        strs(vm, &r),
        [("bar".into(), "2".into()), ("baz".into(), "3".into())]
    );
    let r = regex(
        vm,
        &it,
        "/^ba/",
        vec![MATCH.into(), (USE_KEY | INVERT_MATCH).into()],
    );
    assert_eq!(strs(vm, &r), [("foo".to_string(), "1".to_string())]);

    // Arrays are matched by key only (no "Array to string conversion").
    let it = plain(
        vm,
        &assoc(vec![
            (skey("key 1"), s("value 1")),
            (skey("key 2"), l(vec![s("value 2")])),
        ]),
    );
    let r = regex(vm, &it, "/^key/", vec![MATCH.into(), USE_KEY.into()]);
    show(vm, &r);
    let it = plain(vm, &l(vec![s("a"), l(vec![s("b"), s("c")])]));
    let r = regex(vm, &it, "/Array/", vec![]);
    assert!(foreach_values(vm, &r).unwrap().is_empty());
    assert_phpt(&t.output(), phpt!("bug68128-USE_KEY.phpt"));
}

#[test]
fn recursive_regex_iterator() {
    // iterator_048.phpt, bug68128.phpt (RecursiveRegexIterator part)
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let ar = l(vec![
        s("Foo"),
        l(vec![s("Bar")]),
        s("FooBar"),
        l(vec![s("Baz")]),
        s("Biz"),
    ]);
    let inner = recursive(vm, &ar);
    let it = new_obj(
        vm,
        "RecursiveRegexIterator",
        vec![Value::Object(inner), s("/Bar/")],
    )
    .unwrap();
    let rii = new_obj(vm, "RecursiveIteratorIterator", vec![Value::Object(it)]).unwrap();
    show(vm, &rii);
    assert_phpt(&t.output(), phpt!("iterator_048.phpt"));

    let vm = &mut t.vm;
    let ar = l(vec![s("test1"), l(vec![s("tet3"), s("test4"), s("test5")])]);
    let inner = recursive(vm, &ar);
    let it = new_obj(
        vm,
        "RecursiveRegexIterator",
        vec![
            Value::Object(inner),
            s("/^(t)est(\\d*)/"),
            ALL_MATCHES.into(),
            0.into(),
            1.into(),
        ],
    )
    .unwrap();
    let mut seen = Vec::new();
    foreach_with(vm, &it, true, |vm, _, v| {
        let has = vm.call_method(&it, b"hasChildren", vec![])?;
        if vm.to_bool(&has)? {
            let children = vm.call_method(&it, b"getChildren", vec![])?;
            let children = children.as_object().unwrap().clone();
            assert_eq!(children.class().name.to_string(), "RecursiveRegexIterator");
            for c in foreach_values(vm, &children)? {
                seen.push(format!("child {}", dump(&c)));
            }
        } else {
            seen.push(format!("no children {}", dump(&v)));
        }
        Ok(true)
    })
    .unwrap();
    // test1 matches; the children iterator accepts test4 and test5 (tet3
    // does not match).
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen[0].starts_with("no children array(3)"));
    assert!(seen[0].contains("string(5) \"test1\""));
    assert!(seen[1].contains("string(5) \"test4\""));
    assert!(seen[2].contains("string(5) \"test5\""));
}

#[test]
fn modes_flags_and_accessors() {
    // regexIterator_flags_basic.phpt, regexIterator_mode_basic.phpt,
    // regexIterator_setMode_error.phpt, regexiterator_getregex.phpt,
    // regexiterator_{get,set}pregflags.phpt, bug68175.phpt, bug46088.phpt
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![s("foo"), s("bar"), s("baz")]));
    let it = regex(vm, &inner, "/f/", vec![MATCH.into(), USE_KEY.into()]);
    let get = |vm: &mut Vm, m: &str| int(&vm.call_method(&it, m.as_bytes(), vec![]).unwrap());
    assert_eq!(get(vm, "getFlags"), USE_KEY);
    vm.call_method(&it, b"setFlags", vec![3.into()]).unwrap();
    assert_eq!(get(vm, "getFlags"), 3);
    for mode in [MATCH, GET_MATCH, ALL_MATCHES, SPLIT, REPLACE] {
        vm.call_method(&it, b"setMode", vec![mode.into()]).unwrap();
        assert_eq!(get(vm, "getMode"), mode);
    }
    let (class, msg) = thrown(vm.call_method(&it, b"setMode", vec![7.into()]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "RegexIterator::setMode(): Argument #1 ($mode) must be RegexIterator::MATCH, RegexIterator::GET_MATCH, RegexIterator::ALL_MATCHES, RegexIterator::SPLIT, or RegexIterator::REPLACE"
    );
    vm.call_method(&it, b"setPregFlags", vec![256.into()])
        .unwrap();
    assert_eq!(get(vm, "getPregFlags"), 256);
    for re in [
        "/.at/",
        "//",
        "|\\b[A-Z0-9._%-]+@[A-Z0-9.-]+\\.[A-Z]{2,4}\\b|",
    ] {
        let r = regex(vm, &inner, re, vec![]);
        let got = vm.call_method(&r, b"getRegex", vec![]).unwrap();
        assert!(got.identical(&s(re)));
        assert_eq!(int(&vm.call_method(&r, b"getMode", vec![]).unwrap()), 0);
        assert_eq!(int(&vm.call_method(&r, b"getFlags", vec![]).unwrap()), 0);
        assert_eq!(
            int(&vm.call_method(&r, b"getPregFlags", vec![]).unwrap()),
            0
        );
        // accept() before rewind(): no current element.
        assert!(
            vm.call_method(&r, b"accept", vec![])
                .unwrap()
                .identical(&Value::Bool(false))
        );
    }
    // The $replacement property is declared.
    let r = regex(vm, &inner, "/a/", vec![]);
    assert!(r.class().find_property(b"replacement").is_some());
    assert!(r.get_prop(b"replacement").unwrap().is_null());
}

#[test]
fn construction_errors() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let inner = plain(vm, &l(vec![s("a")]));
    let (class, msg) = thrown(
        new_obj(
            vm,
            "RegexIterator",
            vec![Value::Object(inner.clone()), s("/a/"), 5.into()],
        )
        .map(Value::Object),
    );
    assert_eq!(class, "ValueError");
    assert!(msg.starts_with(
        "RegexIterator::__construct(): Argument #3 ($mode) must be RegexIterator::MATCH"
    ));
    let (class, msg) = thrown(
        new_obj(
            vm,
            "RegexIterator",
            vec![Value::Object(inner.clone()), s("abc")],
        )
        .map(Value::Object),
    );
    assert_eq!(class, "InvalidArgumentException");
    assert_eq!(
        msg,
        "RegexIterator::__construct(): Delimiter must not be alphanumeric, backslash, or NUL byte"
    );
    // No warning was displayed (EH_THROW).
    assert_eq!(t.output(), "");
    let vm = &mut t.vm;
    let (class, msg) = thrown(
        new_obj(
            vm,
            "RecursiveRegexIterator",
            vec![Value::Object(inner), s("/a/")],
        )
        .map(Value::Object),
    );
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "RecursiveRegexIterator::__construct(): Argument #1 ($iterator) must be of type RecursiveIterator, TIter given"
    );
    // A bad PREG_* order in GET_MATCH mode is reported by accept().
    let inner = plain(vm, &l(vec![s("a")]));
    let it = regex(
        vm,
        &inner,
        "/a/",
        vec![GET_MATCH.into(), 0.into(), 3.into()],
    );
    let r = foreach_kv(vm, &it).map(|_| Value::Null);
    match r {
        Err(Flow::Exception(e)) => {
            assert_eq!(e.class().name.to_string(), "ValueError");
            assert_eq!(
                Vm::exception_message(&e),
                "RegexIterator::accept(): Argument #4 must be a PREG_* constant"
            );
        }
        other => panic!("{other:?}"),
    }
}
