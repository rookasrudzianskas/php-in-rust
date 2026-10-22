//! Helpers for the iterator tests.
//!
//! * [`TestIter`] — configurable stand-ins for userland iterators: classes
//!   with any name implementing `Iterator`, `SeekableIterator` or
//!   `RecursiveIterator`, whose methods can echo `Class::method` (like the
//!   `echo __METHOD__` lines of the phpt files), throw from a given method,
//!   or run a hook (to re-enter the outer iterator).
//! * [`foreach_kv`] / [`foreach_values`] — `foreach` through the `Iterator`
//!   methods, in php-src's call order.
//! * [`parent_call`] — `parent::method()` from a simulated user override.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function};
use php_runtime::{Args, CallArgs, ClassRef, NativeFunction, ObjRef, PResult, Value, Vm};

use crate::spl::engine;
use crate::spl::testutil::array_items;

/// Hook run at the start of a test-iterator method:
/// `(vm, method, this, depth, pos, len)`.
pub type Hook = Rc<dyn Fn(&mut Vm, &str, &ObjRef, usize, usize, usize) -> PResult<()>>;

/// Options shared by a test iterator and its children.
#[derive(Clone, Default)]
pub struct Opts {
    /// Echo `Class::method` for every call.
    pub echo: bool,
    /// Echo `Class::valid(true)` / `(false)` instead of `Class::valid`.
    pub verbose_valid: bool,
    /// Echo only `getChildren` and `valid = false` (iterator_014 style).
    pub echo_children: bool,
    /// Methods echoed even without `echo`.
    pub echo_only: &'static [&'static str],
    /// Echo `method()` instead of `method` (iterator_034 style).
    pub parens: bool,
    /// Method that throws `Exception("Class::method()")`; shared so tests
    /// can change it between runs.
    pub fail: Rc<RefCell<Option<String>>>,
    pub hook: Option<Hook>,
}

/// State of a test iterator.
pub struct TestIter {
    pub items: Vec<(Value, Value)>,
    pub pos: usize,
    pub depth: usize,
    pub opts: Opts,
}

fn this(args: &Args) -> ObjRef {
    engine::this(args)
}

fn scope(args: &Args) -> String {
    args.function()
        .scope
        .as_ref()
        .map_or_else(String::new, |s| s.to_string())
}

/// Common prologue: hook, echo, failure.
fn enter(vm: &mut Vm, args: &Args, method: &str) -> PResult<ObjRef> {
    let o = this(args);
    let (opts, depth, pos, len) = {
        let st = o.native::<TestIter>().expect("test iterator state");
        (st.opts.clone(), st.depth, st.pos, st.items.len())
    };
    if method != "next" {
        if let Some(h) = &opts.hook {
            h(vm, method, &o, depth, pos, len)?;
        }
    }
    let p = if opts.parens { "()" } else { "" };
    if (opts.echo && !(method == "valid" && opts.verbose_valid)) || opts.echo_only.contains(&method)
    {
        vm.echo_str(&format!("{}::{method}{p}\n", scope(args)))?;
    }
    if opts.fail.borrow().as_deref() == Some(method) {
        return Err(vm.throw_error("Exception", format!("{}::{method}()", scope(args))));
    }
    Ok(o)
}

fn st<R>(o: &ObjRef, f: impl FnOnce(&mut TestIter) -> R) -> R {
    f(&mut o.native_mut::<TestIter>().expect("test iterator state"))
}

fn t_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "rewind")?;
    st(&o, |s| s.pos = 0);
    Ok(Value::Null)
}

fn t_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "valid")?;
    let (ok, opts) = st(&o, |s| (s.pos < s.items.len(), s.opts.clone()));
    if opts.echo && opts.verbose_valid {
        vm.echo_str(&format!("{}::valid({ok})\n", scope(args)))?;
    }
    if opts.echo_children && !ok {
        let p = if opts.parens { "()" } else { "" };
        vm.echo_str(&format!("{}::valid{p} = false\n", scope(args)))?;
    }
    Ok(Value::Bool(ok))
}

fn t_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "current")?;
    Ok(st(&o, |s| {
        s.items.get(s.pos).map_or(Value::Null, |(_, v)| v.clone())
    }))
}

fn t_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "key")?;
    Ok(st(&o, |s| {
        s.items.get(s.pos).map_or(Value::Null, |(k, _)| k.clone())
    }))
}

/// `next()`; the hook runs after the position moved.
fn t_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "next")?;
    let (hook, depth, pos, len) = st(&o, |s| {
        s.pos += 1;
        (s.opts.hook.clone(), s.depth, s.pos, s.items.len())
    });
    if let Some(h) = hook {
        h(vm, "next", &o, depth, pos, len)?;
    }
    Ok(Value::Null)
}

/// `seek($index)`: moves if in range, then echoes `Class::seek(N)`.
fn t_seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    let index = args.get(0).unref();
    let i = match index {
        Value::Int(i) => i,
        _ => 0,
    };
    let echo = st(&o, |s| {
        if i >= 0 && (i as usize) < s.items.len() {
            s.pos = i as usize;
        }
        s.opts.echo
    });
    if echo {
        vm.echo_str(&format!("{}::seek({i})\n", scope(args)))?;
    }
    Ok(Value::Null)
}

fn t_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "hasChildren")?;
    Ok(Value::Bool(st(&o, |s| {
        matches!(
            s.items.get(s.pos).map(|(_, v)| v.unref()),
            Some(Value::Array(_))
        )
    })))
}

fn t_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = enter(vm, args, "getChildren")?;
    let (child, depth, opts) = st(&o, |s| {
        let child = s.items.get(s.pos).map_or(Value::Null, |(_, v)| v.unref());
        (child, s.depth + 1, s.opts.clone())
    });
    if opts.echo_children {
        let p = if opts.parens { "()" } else { "" };
        vm.echo_str(&format!("{}::getChildren{p}\n", scope(args)))?;
    }
    let ce = o.class().clone();
    Ok(Value::Object(make(&ce, array_items(&child), depth, opts)))
}

static T_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void {}",
    handler: t_rewind,
};
static T_VALID: NativeFunction = NativeFunction {
    sig: "public function valid(): bool {}",
    handler: t_valid,
};
static T_CURRENT: NativeFunction = NativeFunction {
    sig: "public function current(): mixed {}",
    handler: t_current,
};
static T_KEY: NativeFunction = NativeFunction {
    sig: "public function key(): mixed {}",
    handler: t_key,
};
static T_NEXT: NativeFunction = NativeFunction {
    sig: "public function next(): void {}",
    handler: t_next,
};
static T_SEEK: NativeFunction = NativeFunction {
    sig: "public function seek($index): void {}",
    handler: t_seek,
};
static T_HAS_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function hasChildren(): bool {}",
    handler: t_has_children,
};
static T_GET_CHILDREN: NativeFunction = NativeFunction {
    sig: "public function getChildren(): ?RecursiveIterator {}",
    handler: t_get_children,
};

/// Kind of test iterator class.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Seekable,
    Recursive,
}

/// Declares (or finds) a test iterator class `name`. A `Seekable` class
/// named `name` extends the plain class `base` when given (so inherited
/// methods echo the base name, like `SeekableNumericArrayIterator`).
pub fn iter_class(vm: &mut Vm, name: &str, kind: Kind, base: Option<&str>) -> ClassRef {
    if let Some(ce) = vm.find_class(name.as_bytes()) {
        return ce;
    }
    let mut ce = match base {
        Some(b) => {
            let parent = iter_class(vm, b, Kind::Plain, None);
            ClassEntry::new_subclass(name, 0, &parent)
        }
        None => ClassEntry::new(name, 0),
    };
    let iface = match kind {
        Kind::Plain => "Iterator",
        Kind::Seekable => "SeekableIterator",
        Kind::Recursive => "RecursiveIterator",
    };
    let iface = vm.find_class(iface.as_bytes()).expect("interface");
    engine::implement(&mut ce, &iface);
    if base.is_none() {
        engine::add_methods(&mut ce, &[&T_REWIND, &T_VALID, &T_CURRENT, &T_KEY, &T_NEXT]);
    }
    match kind {
        Kind::Seekable => engine::add_methods(&mut ce, &[&T_SEEK]),
        Kind::Recursive => engine::add_methods(&mut ce, &[&T_HAS_CHILDREN, &T_GET_CHILDREN]),
        Kind::Plain => {}
    }
    ce.link_magic_methods();
    let ce = Rc::new(ce);
    vm.register_class(ce.clone());
    ce
}

fn make(ce: &ClassRef, items: Vec<(Value, Value)>, depth: usize, opts: Opts) -> ObjRef {
    let o = ObjRef::new(ce.clone());
    o.set_native(TestIter {
        items,
        pos: 0,
        depth,
        opts,
    });
    o
}

/// A test iterator of class `ce` over an array value.
pub fn new_iter(ce: &ClassRef, array: &Value, opts: Opts) -> ObjRef {
    make(ce, array_items(array), 0, opts)
}

/// A silent iterator (class `TIter`) over an array value.
pub fn plain(vm: &mut Vm, array: &Value) -> ObjRef {
    let ce = iter_class(vm, "TIter", Kind::Plain, None);
    new_iter(&ce, array, Opts::default())
}

/// A silent recursive iterator (class `TRecIter`) over an array value:
/// array elements have children.
pub fn recursive(vm: &mut Vm, array: &Value) -> ObjRef {
    let ce = iter_class(vm, "TRecIter", Kind::Recursive, None);
    new_iter(&ce, array, Opts::default())
}

/// A silent seekable iterator (class `TSeekIter`).
pub fn seekable(vm: &mut Vm, array: &Value) -> ObjRef {
    let ce = iter_class(vm, "TSeekIter", Kind::Seekable, None);
    new_iter(&ce, array, Opts::default())
}

// ---------------------------------------------------------------------------
// foreach and parent calls
// ---------------------------------------------------------------------------

/// `foreach ($it as $k => $v)` through the Iterator methods.
pub fn foreach_kv(vm: &mut Vm, it: &ObjRef) -> PResult<Vec<(Value, Value)>> {
    let mut out = Vec::new();
    foreach_with(vm, it, true, |_, k, v| {
        out.push((k, v));
        Ok(true)
    })?;
    Ok(out)
}

/// `foreach ($it as $v)`.
pub fn foreach_values(vm: &mut Vm, it: &ObjRef) -> PResult<Vec<Value>> {
    let mut out = Vec::new();
    foreach_with(vm, it, false, |_, _, v| {
        out.push(v);
        Ok(true)
    })?;
    Ok(out)
}

/// `foreach` with a body; the body returns `Ok(false)` to `break`.
pub fn foreach_with(
    vm: &mut Vm,
    it: &ObjRef,
    with_key: bool,
    mut body: impl FnMut(&mut Vm, Value, Value) -> PResult<bool>,
) -> PResult<()> {
    super::super::recursive::recursive_it_check_foreach(vm, it, false)?;
    vm.call_method(it, b"rewind", Vec::new())?;
    loop {
        let valid = vm.call_method(it, b"valid", Vec::new())?;
        if !vm.to_bool(&valid)? {
            break;
        }
        let v = vm.call_method(it, b"current", Vec::new())?;
        let k = if with_key {
            vm.call_method(it, b"key", Vec::new())?
        } else {
            Value::Null
        };
        if !body(vm, k, v)? {
            break;
        }
        vm.call_method(it, b"next", Vec::new())?;
    }
    Ok(())
}

/// `parent::$method(...$a)` where the parent class is `class`.
pub fn parent_call(
    vm: &mut Vm,
    args: &Args,
    class: &str,
    method: &str,
    a: Vec<Value>,
) -> PResult<Value> {
    let this = engine::this(args);
    let ce = vm.find_class(class.as_bytes()).expect("parent class");
    let m: Rc<Function> = ce
        .find_method(method.as_bytes())
        .cloned()
        .expect("parent method");
    vm.call_function(
        &m,
        CallArgs {
            positional: a,
            this: Some(this),
            scope: Some(ce),
            ..CallArgs::default()
        },
    )
}

/// `echo "$k=>$v\n"` for a list of pairs (values converted like `echo`).
pub fn kv_lines(vm: &mut Vm, pairs: &[(Value, Value)]) -> String {
    let mut out = String::new();
    for (k, v) in pairs {
        let k = vm.to_php_string(k).unwrap();
        let v = vm.to_php_string(v).unwrap();
        out.push_str(&format!("{k}=>{v}\n"));
    }
    out
}

/// Keys and values as `(String, String)` (via `echo` conversion).
pub fn kv_strings(vm: &mut Vm, pairs: &[(Value, Value)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| {
            (
                vm.to_php_string(k).unwrap().to_string(),
                vm.to_php_string(v).unwrap().to_string(),
            )
        })
        .collect()
}

/// A per-test counter usable from hooks.
pub fn counter() -> Rc<Cell<i64>> {
    Rc::new(Cell::new(0))
}

/// Integer value of a `Value`.
pub fn int(v: &Value) -> i64 {
    match v.unref() {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    }
}

/// `echo "$s\n"`.
pub fn echo(vm: &mut Vm, s: &str) -> PResult<()> {
    vm.echo_str(s)?;
    vm.echo_str("\n")
}

/// `var_dump()` of null, bools, ints, strings, arrays and objects without
/// properties (enough for the iterator tests).
pub fn dump(v: &Value) -> String {
    fn go(v: &Value, indent: usize, out: &mut String) {
        let pad = " ".repeat(indent);
        match v.unref() {
            Value::Null | Value::Undef => out.push_str(&format!("{pad}NULL\n")),
            Value::Bool(b) => out.push_str(&format!("{pad}bool({b})\n")),
            Value::Int(i) => out.push_str(&format!("{pad}int({i})\n")),
            Value::String(s) => out.push_str(&format!("{pad}string({}) \"{s}\"\n", s.len())),
            Value::Array(a) => {
                out.push_str(&format!("{pad}array({}) {{\n", a.len()));
                for (k, v) in a.iter() {
                    match k {
                        php_runtime::ArrayKey::Int(i) => {
                            out.push_str(&format!("{pad}  [{i}]=>\n"));
                        }
                        php_runtime::ArrayKey::Str(s) => {
                            out.push_str(&format!("{pad}  [\"{s}\"]=>\n"));
                        }
                    }
                    go(v, indent + 2, out);
                }
                out.push_str(&format!("{pad}}}\n"));
            }
            Value::Object(o) => out.push_str(&format!(
                "{pad}object({})#{} (0) {{\n{pad}}}\n",
                o.class().name,
                o.id()
            )),
            other => panic!("cannot dump {other:?}"),
        }
    }
    let mut out = String::new();
    go(v, 0, &mut out);
    out
}

/// `var_dump($v)` to the output.
pub fn var_dump(vm: &mut Vm, v: &Value) -> PResult<()> {
    vm.echo_str(&dump(v))
}

// ---------------------------------------------------------------------------
// phpt expectations
// ---------------------------------------------------------------------------

/// The `--EXPECT--` / `--EXPECTF--` section of a phpt file and whether it
/// is an `EXPECTF` pattern.
pub fn phpt_expect(src: &str) -> (&str, bool) {
    for (marker, f) in [("--EXPECTF--\n", true), ("--EXPECT--\n", false)] {
        if let Some(i) = src.find(marker) {
            let rest = &src[i + marker.len()..];
            let end = rest.find("\n--CLEAN--").unwrap_or(rest.len());
            return (&rest[..end], f);
        }
    }
    panic!("no EXPECT section");
}

/// `run-tests.php` matching of an `EXPECTF` pattern (`%s`, `%S`, `%a`,
/// `%A`, `%d`, `%i`, `%e`, `%0`, `%%`).
pub fn expectf_matches(pattern: &str, out: &str) -> bool {
    fn go(p: &[u8], o: &[u8]) -> bool {
        let Some(&c) = p.first() else {
            return o.is_empty();
        };
        if c != b'%' || p.len() < 2 {
            return o.first() == Some(&c) && go(&p[1..], &o[1..]);
        }
        let rest = &p[2..];
        match p[1] {
            b's' | b'S' | b'a' | b'A' => {
                let min = usize::from(p[1] == b's' || p[1] == b'a');
                let multiline = p[1] == b'a' || p[1] == b'A';
                let mut n = 0;
                loop {
                    if n >= min && go(rest, &o[n..]) {
                        return true;
                    }
                    if n >= o.len() || (!multiline && (o[n] == b'\n' || o[n] == b'\r')) {
                        return false;
                    }
                    n += 1;
                }
            }
            b'd' | b'i' => {
                let mut n = usize::from(
                    p[1] == b'i' && o.first().is_some_and(|c| *c == b'-' || *c == b'+'),
                );
                while n < o.len() && o[n].is_ascii_digit() {
                    n += 1;
                    if go(rest, &o[n..]) {
                        return true;
                    }
                }
                false
            }
            b'e' => o.first() == Some(&b'/') && go(rest, &o[1..]),
            b'0' => o.first() == Some(&0) && go(rest, &o[1..]),
            b'%' => o.first() == Some(&b'%') && go(rest, &o[1..]),
            _ => o.first() == Some(&c) && go(&p[1..], &o[1..]),
        }
    }
    go(pattern.trim_end().as_bytes(), out.trim_end().as_bytes())
}

/// Asserts that `out` is what the phpt file `src` expects.
pub fn assert_phpt(out: &str, src: &str) {
    let (expected, is_f) = phpt_expect(src);
    if is_f {
        assert!(
            expectf_matches(expected, out),
            "output does not match EXPECTF\n--- output ---\n{out}\n--- expected ---\n{expected}"
        );
    } else {
        assert_eq!(out.trim_end(), expected.trim_end());
    }
}
