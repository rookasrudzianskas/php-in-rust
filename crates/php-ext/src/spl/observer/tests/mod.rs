//! Tests of spl_observer.c, ported from ext/spl/tests/SplObjectStorage/*.phpt,
//! multiple_iterator_001.phpt, spl_object_storage_gethash_bailout.phpt and
//! the related bug/gh tests.
//!
//! User classes are emulated with [`user_class`] (native overrides) and
//! [`declared_class`] (declared properties); `var_dump()` output is
//! reproduced by [`dump`] so that `.phpt` expectations can be compared
//! verbatim (with `%d`/`%s` placeholders, see [`expectf`]).

mod multiple;
mod serialize;
mod storage;

use std::rc::Rc;

use php_runtime::class::{ClassEntry, DimFetch, PropertyInfo};
use php_runtime::value::float_fmt;
use php_runtime::{
    ArrayKey, CallArgs, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, Value, Vm,
};

use super::super::engine;
use super::super::testutil::new_obj;
use crate::testing::TestVm;

// ---------------------------------------------------------------------------
// Objects and handlers
// ---------------------------------------------------------------------------

/// `new stdClass`.
fn std_obj(vm: &Vm) -> ObjRef {
    ObjRef::new(vm.find_class(b"stdClass").expect("stdClass"))
}

fn obj(o: &ObjRef) -> Value {
    Value::Object(o.clone())
}

/// `new SplObjectStorage`.
fn storage(t: &mut TestVm) -> ObjRef {
    new_obj(&mut t.vm, "SplObjectStorage", vec![]).unwrap()
}

/// `$o[$offset]` (`DimFetch::Read`) / `$o[$offset] ?? ...` (`IsSet`).
fn dim_read(vm: &mut Vm, o: &ObjRef, offset: &Value, fetch: DimFetch) -> PResult<Value> {
    let h = o.class().handlers.read_dimension.expect("read_dimension");
    h(vm, o, Some(offset), fetch)
}

/// `$o[$offset] = $value`.
fn dim_write(vm: &mut Vm, o: &ObjRef, offset: &Value, value: Value) -> PResult<()> {
    let h = o.class().handlers.write_dimension.expect("write_dimension");
    h(vm, o, Some(offset), value)
}

/// `isset($o[$offset])`.
fn dim_isset(vm: &mut Vm, o: &ObjRef, offset: &Value) -> PResult<bool> {
    let h = o.class().handlers.has_dimension.expect("has_dimension");
    h(vm, o, offset, false)
}

/// `empty($o[$offset])`.
fn dim_empty(vm: &mut Vm, o: &ObjRef, offset: &Value) -> PResult<bool> {
    let h = o.class().handlers.has_dimension.expect("has_dimension");
    Ok(!h(vm, o, offset, true)?)
}

/// `unset($o[$offset])`.
fn dim_unset(vm: &mut Vm, o: &ObjRef, offset: &Value) -> PResult<()> {
    let h = o.class().handlers.unset_dimension.expect("unset_dimension");
    h(vm, o, offset)
}

/// `$obj->$method(...$args)` as an integer.
fn int_of(v: PResult<Value>) -> i64 {
    match v.unwrap().unref() {
        Value::Int(i) => i,
        other => panic!("expected an int, got {other:?}"),
    }
}

/// `var_dump($o->$method(...$args))`.
fn dump_call(vm: &mut Vm, o: &ObjRef, method: &str, args: Vec<Value>) -> String {
    let v = vm.call_method(o, method.as_bytes(), args).unwrap();
    dump(vm, &v)
}

/// `var_dump(count($v))`.
fn dump_count(vm: &mut Vm, v: &Value) -> String {
    let n = vm.call_native("count", vec![v.clone()]).unwrap();
    dump(vm, &n)
}

/// `parent::$method(...$args)` from an override in a subclass of `parent`.
fn parent_call(
    vm: &mut Vm,
    parent: &str,
    this: &ObjRef,
    method: &str,
    args: Vec<Value>,
) -> PResult<Value> {
    let ce = vm.find_class(parent.as_bytes()).expect("parent class");
    let m = ce.find_method(method.as_bytes()).expect("method").clone();
    vm.call_function(
        &m,
        CallArgs {
            positional: args,
            this: Some(this.clone()),
            scope: Some(ce),
            ..CallArgs::default()
        },
    )
}

/// A user class `name` extending `parent` (if any) with declared instance
/// properties `(name, flags, default)` and native `methods`.
fn declared_class(
    vm: &mut Vm,
    name: &str,
    parent: Option<&str>,
    props: &[(&str, u32, Value)],
    methods: &[&'static NativeFunction],
) -> ClassRef {
    let mut ce = match parent {
        Some(p) => {
            let p = vm.find_class(p.as_bytes()).expect("parent");
            ClassEntry::new_subclass(name, 0, &p)
        }
        None => ClassEntry::new(name, 0),
    };
    for (prop, flags, default) in props {
        let mut info = PropertyInfo::new(*prop, name, *flags);
        info.default = default.clone();
        ce.declare_property(info);
    }
    for nf in methods {
        ce.add_method(php_runtime::class::Function::native_method(
            nf,
            name,
            engine::method_flags(nf.sig),
        ));
    }
    ce.link_magic_methods();
    let ce = Rc::new(ce);
    vm.register_class(ce.clone());
    ce
}

/// Per-test state reachable from native "user" methods.
#[derive(Default)]
struct TestState {
    log: Vec<String>,
    target: Option<ObjRef>,
    mutate: bool,
}

fn state(vm: &mut Vm) -> &mut TestState {
    vm.ext_data::<TestState>()
}

fn log(vm: &mut Vm, entry: impl Into<String>) {
    state(vm).log.push(entry.into());
}

fn take_log(vm: &mut Vm) -> Vec<String> {
    std::mem::take(&mut state(vm).log)
}

// ---------------------------------------------------------------------------
// var_dump()
// ---------------------------------------------------------------------------

/// `var_dump($v)` (php_var_dump) for the value types these tests use.
/// Objects with a `__debugInfo()` method are dumped through it.
fn dump(vm: &mut Vm, v: &Value) -> String {
    let mut out = String::new();
    let mut guard = Vec::new();
    dump_into(vm, v, 1, &mut out, &mut guard);
    out
}

fn dump_into(vm: &mut Vm, v: &Value, level: usize, out: &mut String, guard: &mut Vec<u32>) {
    if level > 1 {
        out.push_str(&" ".repeat(level - 1));
    }
    let amp = match v {
        Value::Ref(r) if r.refcount() > 1 => "&",
        _ => "",
    };
    match v.unref() {
        Value::Null | Value::Undef => out.push_str(&format!("{amp}NULL\n")),
        Value::Bool(b) => out.push_str(&format!("{amp}bool({b})\n")),
        Value::Int(i) => out.push_str(&format!("{amp}int({i})\n")),
        Value::Float(d) => out.push_str(&format!(
            "{amp}float({})\n",
            float_fmt::repr(d, vm.serialize_precision)
        )),
        Value::String(s) => out.push_str(&format!(
            "{amp}string({}) \"{}\"\n",
            s.len(),
            s.to_string_lossy()
        )),
        Value::Array(a) => {
            out.push_str(&format!("{amp}array({}) {{\n", a.len()));
            for (k, v) in a.iter() {
                out.push_str(&" ".repeat(level + 1));
                match k {
                    ArrayKey::Int(i) => out.push_str(&format!("[{i}]=>\n")),
                    ArrayKey::Str(s) => out.push_str(&format!("[\"{}\"]=>\n", s.to_string_lossy())),
                }
                dump_into(vm, v, level + 2, out, guard);
            }
            if level > 1 {
                out.push_str(&" ".repeat(level - 1));
            }
            out.push_str("}\n");
        }
        Value::Object(o) => {
            if guard.contains(&o.id()) {
                out.push_str("*RECURSION*\n");
                return;
            }
            guard.push(o.id());
            let table = if o.class().find_method(b"__debuginfo").is_some() {
                match vm
                    .call_method(&o, b"__debugInfo", Vec::new())
                    .unwrap()
                    .unref()
                {
                    Value::Array(a) => (*a).clone(),
                    other => panic!("__debugInfo() returned {other:?}"),
                }
            } else {
                engine::std_properties(&o)
            };
            out.push_str(&format!(
                "{amp}object({})#{} ({}) {{\n",
                o.class().name,
                o.id(),
                table.len()
            ));
            for (k, v) in table.iter() {
                out.push_str(&" ".repeat(level + 1));
                match k {
                    ArrayKey::Int(i) => out.push_str(&format!("[{i}]=>\n")),
                    ArrayKey::Str(s) => out.push_str(&format!("[{}]=>\n", property_label(&s))),
                }
                dump_into(vm, v, level + 2, out, guard);
            }
            if level > 1 {
                out.push_str(&" ".repeat(level - 1));
            }
            out.push_str("}\n");
            guard.pop();
        }
        Value::Resource(_) => out.push_str("resource\n"),
        Value::Ref(_) => unreachable!("unref"),
    }
}

/// `php_object_property_dump`'s key: `"name"`, `"name":protected` or
/// `"name":"Class":private`.
fn property_label(key: &[u8]) -> String {
    let mangled = key
        .strip_prefix(b"\0")
        .and_then(|rest| rest.iter().position(|&c| c == 0).map(|end| (rest, end)));
    if let Some((rest, end)) = mangled {
        let class = String::from_utf8_lossy(&rest[..end]);
        let name = String::from_utf8_lossy(&rest[end + 1..]);
        return if class == "*" {
            format!("\"{name}\":protected")
        } else {
            format!("\"{name}\":\"{class}\":private")
        };
    }
    format!("\"{}\"", String::from_utf8_lossy(key))
}

/// `--EXPECTF--` matching for `%d` (digits) and `%s` (anything on a line).
fn expectf(expected: &str, actual: &str) -> bool {
    fn go(e: &[u8], a: &[u8]) -> bool {
        match e {
            [] => a.is_empty(),
            [b'%', b'd', rest @ ..] => {
                let n = a.iter().take_while(|c| c.is_ascii_digit()).count();
                (1..=n).rev().any(|i| go(rest, &a[i..]))
            }
            [b'%', b's', rest @ ..] => {
                let n = a.iter().take_while(|&&c| c != b'\n').count();
                (1..=n).rev().any(|i| go(rest, &a[i..]))
            }
            [c, rest @ ..] => a.first() == Some(c) && go(rest, &a[1..]),
        }
    }
    go(expected.as_bytes(), actual.as_bytes())
}

#[track_caller]
fn assert_expectf(expected: &str, actual: &str) {
    assert!(
        expectf(expected, actual),
        "output mismatch\n--- expected ---\n{expected}\n--- actual ---\n{actual}"
    );
}

/// The `--EXPECT--`/`--EXPECTF--` section of `tests/phpt/ext/spl/tests/<path>`,
/// trimmed like run-tests does.
fn phpt_expect(path: &str) -> String {
    let file = format!(
        "{}/../../tests/phpt/ext/spl/tests/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file}: {e}"));
    let start = ["--EXPECTF--\n", "--EXPECT--\n"]
        .iter()
        .find_map(|m| text.find(m).map(|i| i + m.len()))
        .unwrap_or_else(|| panic!("{file}: no EXPECT section"));
    let rest = &text[start..];
    let end = rest
        .match_indices("\n--")
        .find(|(i, _)| {
            let line = rest[i + 1..].lines().next().unwrap_or("");
            line.len() > 4
                && line.ends_with("--")
                && line[2..line.len() - 2]
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_')
        })
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].trim().to_string()
}

/// Compares emulated script output with a phpt expectation. Object handles
/// (`)#3 (`) are matched as `%d`: the test VM allocates objects of its own.
#[track_caller]
fn assert_phpt(path: &str, actual: &str) {
    let expected = phpt_expect(path);
    let mut out = String::with_capacity(expected.len());
    let mut rest = expected.as_str();
    while let Some(i) = rest.find(")#") {
        out.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(" (") {
            out.push_str("%d");
            rest = &rest[digits..];
        }
    }
    out.push_str(rest);
    assert_expectf(&out, actual.trim());
}

/// An array value from `(key, value)` pairs.
fn array_from(pairs: Vec<(ArrayKey, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(k, v);
    }
    Value::array(a)
}

#[test]
fn expectf_placeholders() {
    assert!(expectf(
        "object(stdClass)#%d (0)",
        "object(stdClass)#12 (0)"
    ));
    assert!(!expectf("#%d (0)", "# (0)"));
    assert!(expectf("in %s on line %d\n", "in /t.php on line 3\n"));
}
