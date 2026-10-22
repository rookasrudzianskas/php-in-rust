//! Helpers for the ext/date unit tests: a PHP "session" around [`TestVm`]
//! with shorthands for `new`, method calls and static calls, PHP's
//! `var_dump()` / `var_export()` / `print_r()` renderings (objects included)
//! and run-tests' `--EXPECTF--` matching, so `.phpt` scripts can be
//! transliterated statement by statement and compared with their expected
//! output verbatim.

#![allow(dead_code)]

use php_runtime::class::{ClassEntry, PropPurpose};
use php_runtime::value::convert::object_property_table;
use php_runtime::value::float_fmt;
use php_runtime::{
    ArrayKey, CallArgs, ClassRef, Flow, ObjRef, PResult, PhpArray, PhpStr, Value, Vm,
};

use crate::testing::TestVm;

/// A string value.
pub fn s(v: &str) -> Value {
    Value::str(v)
}

/// A binary string value.
pub fn bs(v: &[u8]) -> Value {
    Value::bytes(v)
}

/// An int value.
pub fn i(v: i64) -> Value {
    Value::Int(v)
}

/// A float value.
pub fn f(v: f64) -> Value {
    Value::Float(v)
}

/// A bool value.
pub fn b(v: bool) -> Value {
    Value::Bool(v)
}

/// `null`.
pub fn null() -> Value {
    Value::Null
}

/// A list `[a, b, ...]`.
pub fn list(vals: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(vals))
}

/// An array with string keys `['k' => v, ...]`.
pub fn assoc(pairs: Vec<(&str, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(ArrayKey::from_bytes(k.as_bytes()), v);
    }
    Value::array(a)
}

/// A PHP session.
pub struct P {
    pub t: TestVm,
    /// The phpt this session transliterates (see [`P::phpt`]).
    name: String,
}

thread_local! {
    static ID_BASE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The handle PHP would show for `o` (`#N` in var_dump): handles taken by
/// objects alive before the session started are not counted.
pub fn php_id(o: &ObjRef) -> u32 {
    o.id() - ID_BASE.with(|b| b.get()).min(o.id().saturating_sub(1))
}

/// The text of `tests/phpt/ext/date/tests/<name>.phpt`.
pub fn read_phpt(name: &str) -> String {
    let path = format!(
        "{}/../../tests/phpt/ext/date/tests/{name}.phpt",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The body of a `--NAME--` section of a phpt file.
pub fn section(file: &str, name: &str) -> Option<String> {
    let header = format!("--{name}--");
    let mut out: Option<String> = None;
    for line in file.split_inclusive('\n') {
        let trimmed = line.trim_end();
        let is_header = trimmed.len() > 4
            && trimmed.starts_with("--")
            && trimmed.ends_with("--")
            && trimmed[2..trimmed.len() - 2]
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c == b'_');
        if is_header {
            if out.is_some() {
                break;
            }
            if trimmed == header {
                out = Some(String::new());
            }
            continue;
        }
        if let Some(o) = out.as_mut() {
            o.push_str(line);
        }
    }
    out
}

impl Default for P {
    fn default() -> Self {
        Self::new()
    }
}

impl P {
    /// A session with `date.timezone=UTC`.
    pub fn new() -> P {
        let mut t = TestVm::new();
        t.vm.current_file = PhpStr::from("/t.php");
        // Objects the other modules keep alive take the first handles.
        let live = php_runtime::object::live_objects().len();
        let base = php_runtime::object::store_top().saturating_sub(1);
        ID_BASE.with(|b| b.set(if live == base { base as u32 } else { 0 }));
        P {
            t,
            name: String::new(),
        }
    }

    /// A session with `date.timezone=<tz>` (the phpt `--INI--` section).
    pub fn tz(tz: &str) -> P {
        let mut p = P::new();
        p.ini("date.timezone", tz);
        p
    }

    /// A session transliterating `tests/phpt/ext/date/tests/<name>.phpt`:
    /// its `--INI--` settings are applied and [`P::finish`] compares the
    /// output with its `--EXPECT--` / `--EXPECTF--` section.
    pub fn phpt(name: &str) -> P {
        let mut p = P::new();
        p.t.vm.current_file = PhpStr::from(format!("/phpt/{name}.php"));
        p.name = name.to_string();
        let file = read_phpt(name);
        if let Some(ini) = section(&file, "INI") {
            for line in ini.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with(';') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let (k, v) = (k.trim(), v.trim().trim_matches('"'));
                    if p.t.vm.ini.get(k.as_bytes()).is_some() {
                        p.ini(k, v);
                    }
                }
            }
        }
        p
    }

    /// Compares the output with the phpt's expected section.
    pub fn finish(&mut self) {
        let file = read_phpt(&self.name);
        if let Some(e) = section(&file, "EXPECT") {
            self.expect(&e);
        } else if let Some(e) = section(&file, "EXPECTF") {
            self.expectf(&e);
        } else {
            panic!("{}: no EXPECT/EXPECTF section", self.name);
        }
    }

    /// A startup ini setting (`-d name=value`).
    pub fn ini(&mut self, name: &str, value: &str) {
        assert!(
            self.t.vm.ini_set_system(name, value),
            "unknown ini entry {name}"
        );
    }

    /// Writes raw bytes to the output.
    pub fn write(&mut self, bytes: &[u8]) {
        if let Err(e) = self.t.vm.echo(bytes) {
            panic!("echo failed: {e:?}");
        }
    }

    pub fn vm(&mut self) -> &mut Vm {
        &mut self.t.vm
    }

    fn unwrap(&mut self, r: PResult<Value>, what: &str) -> Value {
        match r {
            Ok(v) => v,
            Err(Flow::Exception(e)) => panic!(
                "{what}: uncaught {}: {}\noutput so far:\n{}",
                e.class().name,
                Vm::exception_message(&e),
                self.t.output()
            ),
            Err(other) => panic!("{what}: {other:?}"),
        }
    }

    /// Calls a function (panics on an exception).
    pub fn call(&mut self, name: &str, args: Vec<Value>) -> Value {
        let r = self.try_call(name, args);
        self.unwrap(r, name)
    }

    pub fn try_call(&mut self, name: &str, args: Vec<Value>) -> PResult<Value> {
        self.t.vm.call_native(name, args)
    }

    /// Calls a function with named arguments.
    pub fn call_named(
        &mut self,
        name: &str,
        args: Vec<Value>,
        named: Vec<(&str, Value)>,
    ) -> PResult<Value> {
        self.t.vm.call_native_named(name, args, named)
    }

    pub fn class(&self, name: &str) -> ClassRef {
        self.t
            .vm
            .find_class(name.as_bytes())
            .unwrap_or_else(|| panic!("no class {name}"))
    }

    /// `new Class(...$args)`.
    pub fn new_obj(&mut self, class: &str, args: Vec<Value>) -> Value {
        let r = self.try_new(class, args);
        self.unwrap(r, class)
    }

    pub fn try_new(&mut self, class: &str, args: Vec<Value>) -> PResult<Value> {
        let ce = self.class(class);
        let obj = self.t.vm.instantiate(&ce)?;
        if let Some(ctor) = ce.magic.constructor.clone() {
            self.t.vm.call_function(
                &ctor,
                CallArgs {
                    positional: args,
                    this: Some(obj.clone()),
                    scope: Some(ce.clone()),
                    ..CallArgs::default()
                },
            )?;
        }
        Ok(Value::Object(obj))
    }

    /// An object of `class` without calling its constructor
    /// (`ReflectionClass::newInstanceWithoutConstructor()`).
    pub fn new_without_ctor(&mut self, class: &str) -> Value {
        let ce = self.class(class);
        Value::Object(self.t.vm.instantiate(&ce).unwrap())
    }

    /// `$obj->method(...$args)`.
    pub fn m(&mut self, obj: &Value, method: &str, args: Vec<Value>) -> Value {
        let r = self.try_m(obj, method, args);
        self.unwrap(r, method)
    }

    pub fn try_m(&mut self, obj: &Value, method: &str, args: Vec<Value>) -> PResult<Value> {
        let o = obj
            .as_object()
            .expect("method call on a non-object")
            .clone();
        self.t.vm.call_method(&o, method.as_bytes(), args)
    }

    /// `Class::method(...$args)` (late static binding scope = `class`).
    pub fn sm(&mut self, class: &str, method: &str, args: Vec<Value>) -> Value {
        let r = self.try_sm(class, method, args);
        self.unwrap(r, method)
    }

    pub fn try_sm(&mut self, class: &str, method: &str, args: Vec<Value>) -> PResult<Value> {
        let ce = self.class(class);
        let m = ce
            .find_method(method.as_bytes())
            .unwrap_or_else(|| panic!("no method {class}::{method}"))
            .clone();
        self.t.vm.call_function(
            &m,
            CallArgs {
                positional: args,
                scope: Some(ce.clone()),
                ..CallArgs::default()
            },
        )
    }

    /// `clone $obj`.
    pub fn clone_obj(&mut self, obj: &Value) -> Value {
        let o = obj.as_object().unwrap().clone();
        let r = match o.class().handlers.clone_obj {
            Some(h) => h(&mut self.t.vm, &o).map(Value::Object),
            None => panic!("no clone handler"),
        };
        self.unwrap(r, "clone")
    }

    /// `class Name extends Parent {}`.
    pub fn subclass(&mut self, name: &str, parent: &str) -> ClassRef {
        let parent = self.class(parent);
        let mut ce = ClassEntry::new_subclass(name, 0, &parent);
        ce.interfaces = parent.interfaces.clone();
        ce.link_magic_methods();
        let ce = std::rc::Rc::new(ce);
        self.t.vm.register_class(ce.clone());
        ce
    }

    /// `$obj->name` (through the class's read_property handler).
    pub fn prop(&mut self, obj: &Value, name: &str) -> Value {
        let o = obj.as_object().unwrap().clone();
        if let Some(h) = o.class().handlers.read_property {
            let r = h(&mut self.t.vm, &o, &PhpStr::from(name));
            let v = self.unwrap(r.map(|v| v.unwrap_or(Value::Undef)), name);
            if !v.is_undef() {
                return v;
            }
        }
        o.get_prop(name.as_bytes()).unwrap_or(Value::Null)
    }

    /// `$obj->name = $v` (through the class's write_property handler).
    pub fn try_set_prop(&mut self, obj: &Value, name: &str, v: Value) -> PResult<()> {
        let o = obj.as_object().unwrap().clone();
        if let Some(h) = o.class().handlers.write_property
            && h(&mut self.t.vm, &o, &PhpStr::from(name), v.clone())?
        {
            return Ok(());
        }
        o.set_prop(name.as_bytes(), v);
        Ok(())
    }

    pub fn set_prop(&mut self, obj: &Value, name: &str, v: Value) {
        let r = self.try_set_prop(obj, name, v).map(|_| Value::Null);
        self.unwrap(r, name);
    }

    /// `echo $a, $b, ...;`
    pub fn echo(&mut self, parts: &[Value]) {
        for p in parts {
            let r = self.t.vm.to_php_string(p);
            let s = self.unwrap(r.map(Value::String), "echo");
            if let Value::String(s) = s {
                self.write(&s);
            }
        }
    }

    /// `echo "text";`
    pub fn print(&mut self, text: &str) {
        self.write(text.as_bytes());
    }

    /// `var_dump($v)`.
    pub fn dump(&mut self, v: &Value) {
        let mut out = Vec::new();
        let r = var_dump_into(&mut self.t.vm, &mut out, v, 0);
        self.unwrap(r.map(|_| Value::Null), "var_dump");
        self.write(&out);
    }

    /// `var_export($v)`.
    pub fn export(&mut self, v: &Value) {
        let text = self.export_str(v);
        self.write(&text);
    }

    /// `var_export($v, true)`.
    pub fn export_str(&mut self, v: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        let r = var_export_into(&mut self.t.vm, &mut out, v, 1);
        self.unwrap(r.map(|_| Value::Null), "var_export");
        out
    }

    /// `print_r($v)`.
    pub fn print_r(&mut self, v: &Value) {
        let mut out = Vec::new();
        let r = print_r_into(&mut self.t.vm, &mut out, v, 0);
        self.unwrap(r.map(|_| Value::Null), "print_r");
        self.write(&out);
    }

    /// A script ending with an uncaught exception: prints PHP's
    /// `Fatal error: Uncaught ...` report (the stack trace of a Rust-driven
    /// call differs from the script's, compare it with `%a`).
    pub fn uncaught(&mut self, r: PResult<Value>) {
        match r {
            Err(Flow::Exception(e)) => {
                let r = self.t.vm.report_uncaught_exception(&e);
                self.unwrap(r.map(|_| Value::Null), "uncaught");
            }
            Ok(v) => panic!("expected an uncaught exception, got {v:?}"),
            Err(other) => panic!("unexpected {other:?}"),
        }
    }

    /// `foreach ($aggregate as $k => $v)` over an `IteratorAggregate` (or an
    /// `Iterator`): the `(key, value)` pairs.
    pub fn foreach(&mut self, obj: &Value) -> Vec<(Value, Value)> {
        let o = obj.as_object().expect("foreach over a non-object").clone();
        let it = if o.class().instance_of_name(b"IteratorAggregate") {
            self.m(obj, "getIterator", vec![])
        } else {
            obj.clone()
        };
        let mut out = Vec::new();
        self.m(&it, "rewind", vec![]);
        while self.m(&it, "valid", vec![]).as_bool() == Some(true) {
            let v = self.m(&it, "current", vec![]);
            let k = self.m(&it, "key", vec![]);
            out.push((k, v));
            self.m(&it, "next", vec![]);
        }
        out
    }

    /// `try { ... } catch (Throwable $e) { echo $e::class, ': ', $e->getMessage(), "\n"; }`
    pub fn catch(&mut self, r: PResult<Value>) -> Option<Value> {
        match r {
            Ok(v) => Some(v),
            Err(Flow::Exception(e)) => {
                let line = format!("{}: {}\n", e.class().name, Vm::exception_message(&e));
                self.write(line.as_bytes());
                None
            }
            Err(other) => panic!("unexpected {other:?}"),
        }
    }

    /// Like [`P::catch`] but prints only the message (`echo $e->getMessage(), "\n"`).
    pub fn catch_msg(&mut self, r: PResult<Value>) -> Option<Value> {
        match r {
            Ok(v) => Some(v),
            Err(Flow::Exception(e)) => {
                let line = format!("{}\n", Vm::exception_message(&e));
                self.write(line.as_bytes());
                None
            }
            Err(other) => panic!("unexpected {other:?}"),
        }
    }

    /// The output so far (drained).
    pub fn out(&mut self) -> String {
        self.t.output()
    }

    /// Asserts the output equals `expected` (`--EXPECT--`, trailing
    /// whitespace ignored like run-tests).
    pub fn expect(&mut self, expected: &str) {
        let got = self.out();
        assert_eq!(
            got.trim_end().replace("\r\n", "\n"),
            expected.trim_end().replace("\r\n", "\n"),
            "\n--- got ---\n{got}\n--- expected ---\n{expected}"
        );
    }

    /// Asserts the output matches the `--EXPECTF--` pattern.
    pub fn expectf(&mut self, pattern: &str) {
        let got = self.out();
        let g = got.trim_end();
        let p = pattern.trim_end();
        assert!(
            expectf_match(p.as_bytes(), g.as_bytes()),
            "\n--- got ---\n{got}\n--- expected (EXPECTF) ---\n{pattern}"
        );
    }
}

// ---------------------------------------------------------------------------
// var_dump / var_export / print_r
// ---------------------------------------------------------------------------

/// The properties an object shows for `purpose`.
fn object_props(vm: &mut Vm, o: &ObjRef, purpose: PropPurpose) -> PResult<PhpArray> {
    if let Some(h) = o.class().handlers.get_properties_for
        && let Some(t) = h(vm, o, purpose)?
    {
        return Ok(t);
    }
    Ok(object_property_table(o))
}

/// A property-table key as var_dump/print_r show it.
fn prop_label(key: &[u8], quote: bool) -> Vec<u8> {
    let q: &[u8] = if quote { b"\"" } else { b"" };
    let mut out = Vec::new();
    if key.first() == Some(&0)
        && let Some(end) = key[1..].iter().position(|c| *c == 0)
    {
        let class = &key[1..1 + end];
        let name = &key[2 + end..];
        out.extend_from_slice(q);
        out.extend_from_slice(name);
        out.extend_from_slice(q);
        if class == b"*" {
            out.extend_from_slice(b":protected");
        } else {
            out.push(b':');
            out.extend_from_slice(q);
            out.extend_from_slice(class);
            out.extend_from_slice(q);
            out.extend_from_slice(b":private");
        }
        return out;
    }
    out.extend_from_slice(q);
    out.extend_from_slice(key);
    out.extend_from_slice(q);
    out
}

pub fn var_dump_into(vm: &mut Vm, out: &mut Vec<u8>, v: &Value, indent: usize) -> PResult<()> {
    let pad = vec![b' '; indent];
    out.extend_from_slice(&pad);
    match v {
        Value::Undef | Value::Null => out.extend_from_slice(b"NULL\n"),
        Value::Bool(x) => out.extend_from_slice(format!("bool({x})\n").as_bytes()),
        Value::Int(x) => out.extend_from_slice(format!("int({x})\n").as_bytes()),
        Value::Float(x) => {
            out.extend_from_slice(format!("float({})\n", float_fmt::format_h(*x, -1)).as_bytes())
        }
        Value::String(x) => {
            out.extend_from_slice(format!("string({}) \"", x.len()).as_bytes());
            out.extend_from_slice(x);
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, val) in a.iter() {
                out.extend_from_slice(&pad);
                match k {
                    ArrayKey::Int(n) => out.extend_from_slice(format!("  [{n}]=>\n").as_bytes()),
                    ArrayKey::Str(s) => {
                        out.extend_from_slice(b"  [\"");
                        out.extend_from_slice(&s);
                        out.extend_from_slice(b"\"]=>\n");
                    }
                }
                var_dump_into(vm, out, &val.unref(), indent + 2)?;
            }
            out.extend_from_slice(&pad);
            out.extend_from_slice(b"}\n");
        }
        Value::Object(o) => {
            let props = object_props(vm, o, PropPurpose::Debug)?;
            out.extend_from_slice(
                format!(
                    "object({})#{} ({}) {{\n",
                    o.class().name,
                    php_id(o),
                    props.len()
                )
                .as_bytes(),
            );
            for (k, val) in props.iter() {
                out.extend_from_slice(&pad);
                out.extend_from_slice(b"  [");
                match k {
                    ArrayKey::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
                    ArrayKey::Str(s) => out.extend_from_slice(&prop_label(&s, true)),
                }
                out.extend_from_slice(b"]=>\n");
                var_dump_into(vm, out, &val.unref(), indent + 2)?;
            }
            out.extend_from_slice(&pad);
            out.extend_from_slice(b"}\n");
        }
        Value::Ref(r) => {
            out.truncate(out.len() - indent);
            var_dump_into(vm, out, &r.get(), indent)?;
        }
        other => out.extend_from_slice(format!("{other:?}\n").as_bytes()),
    }
    Ok(())
}

fn export_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'\'');
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\'' => out.extend_from_slice(b"\\'"),
            0 => out.extend_from_slice(b"' . \"\\0\" . '"),
            c => out.push(c),
        }
        i += 1;
    }
    out.push(b'\'');
}

pub fn var_export_into(vm: &mut Vm, out: &mut Vec<u8>, v: &Value, level: usize) -> PResult<()> {
    let spaces = |out: &mut Vec<u8>, n: usize| out.extend(std::iter::repeat_n(b' ', n));
    match v {
        Value::Undef | Value::Null => out.extend_from_slice(b"NULL"),
        Value::Bool(x) => out.extend_from_slice(if *x { b"true" } else { b"false" }),
        Value::Int(x) => out.extend_from_slice(x.to_string().as_bytes()),
        Value::Float(x) => float_fmt::append_double(out, *x, vm.serialize_precision, true),
        Value::String(x) => export_string(out, x),
        Value::Array(a) => {
            if level > 1 {
                out.push(b'\n');
                spaces(out, level - 1);
            }
            out.extend_from_slice(b"array (\n");
            for (k, val) in a.iter() {
                spaces(out, level + 1);
                match k {
                    ArrayKey::Int(n) => out.extend_from_slice(format!("{n} => ").as_bytes()),
                    ArrayKey::Str(s) => {
                        export_string(out, &s);
                        out.extend_from_slice(b" => ");
                    }
                }
                var_export_into(vm, out, &val.unref(), level + 2)?;
                out.extend_from_slice(b",\n");
            }
            if level > 1 {
                spaces(out, level - 1);
            }
            out.push(b')');
        }
        Value::Object(o) => {
            if level > 1 {
                out.push(b'\n');
                spaces(out, level - 1);
            }
            let props = object_props(vm, o, PropPurpose::VarExport)?;
            out.push(b'\\');
            out.extend_from_slice(&o.class().name);
            out.extend_from_slice(b"::__set_state(array(\n");
            for (k, val) in props.iter() {
                spaces(out, level + 2);
                match k {
                    ArrayKey::Int(n) => out.extend_from_slice(format!("{n} => ").as_bytes()),
                    ArrayKey::Str(s) => {
                        let name = match s.iter().rposition(|c| *c == 0) {
                            Some(p) => &s[p + 1..],
                            None => &s[..],
                        };
                        export_string(out, name);
                        out.extend_from_slice(b" => ");
                    }
                }
                var_export_into(vm, out, &val.unref(), level + 2)?;
                out.extend_from_slice(b",\n");
            }
            if level > 1 {
                spaces(out, level - 1);
            }
            out.extend_from_slice(b"))");
        }
        Value::Ref(r) => var_export_into(vm, out, &r.get(), level)?,
        other => out.extend_from_slice(format!("{other:?}").as_bytes()),
    }
    Ok(())
}

pub fn print_r_into(vm: &mut Vm, out: &mut Vec<u8>, v: &Value, indent: usize) -> PResult<()> {
    let print_hash = |vm: &mut Vm, out: &mut Vec<u8>, a: &PhpArray, object: bool| -> PResult<()> {
        out.extend(std::iter::repeat_n(b' ', indent));
        out.extend_from_slice(b"(\n");
        for (k, val) in a.iter() {
            out.extend(std::iter::repeat_n(b' ', indent + 4));
            out.push(b'[');
            match k {
                ArrayKey::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
                ArrayKey::Str(s) => {
                    if object {
                        out.extend_from_slice(&prop_label(&s, false));
                    } else {
                        out.extend_from_slice(&s);
                    }
                }
            }
            out.extend_from_slice(b"] => ");
            print_r_into(vm, out, &val.unref(), indent + 8)?;
            out.push(b'\n');
        }
        out.extend(std::iter::repeat_n(b' ', indent));
        out.extend_from_slice(b")\n");
        Ok(())
    };
    match v {
        Value::Array(a) => {
            out.extend_from_slice(b"Array\n");
            print_hash(vm, out, a, false)?;
        }
        Value::Object(o) => {
            out.extend_from_slice(&o.class().name);
            out.extend_from_slice(b" Object\n");
            let props = object_props(vm, o, PropPurpose::Debug)?;
            print_hash(vm, out, &props, true)?;
        }
        Value::Ref(r) => print_r_into(vm, out, &r.get(), indent)?,
        other => {
            let s = vm.to_php_string(other)?;
            out.extend_from_slice(&s);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// --EXPECTF-- matching
// ---------------------------------------------------------------------------

/// run-tests' `--EXPECTF--` placeholders: `%s %S %a %A %w %i %d %x %f %c %e %%`.
pub fn expectf_match(pattern: &[u8], text: &[u8]) -> bool {
    fn class_len(kind: u8, text: &[u8], start: usize) -> Vec<usize> {
        // All possible match lengths (longest first) for one placeholder.
        let rest = &text[start..];
        let ends: Vec<usize> = match kind {
            b's' | b'S' => {
                let n = rest
                    .iter()
                    .position(|c| *c == b'\n' || *c == b'\r')
                    .unwrap_or(rest.len());
                let min = usize::from(kind == b's');
                (min..=n).rev().collect()
            }
            b'a' | b'A' => {
                let min = usize::from(kind == b'a');
                (min..=rest.len()).rev().collect()
            }
            b'w' => {
                let n = rest.iter().take_while(|c| c.is_ascii_whitespace()).count();
                (0..=n).rev().collect()
            }
            b'd' => {
                let n = rest.iter().take_while(|c| c.is_ascii_digit()).count();
                (1..=n).rev().collect()
            }
            b'i' => {
                let sign = usize::from(matches!(rest.first(), Some(b'+' | b'-')));
                let n = rest[sign..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                (1..=n).rev().map(|k| k + sign).collect()
            }
            b'x' => {
                let n = rest.iter().take_while(|c| c.is_ascii_hexdigit()).count();
                (1..=n).rev().collect()
            }
            b'f' => {
                let n = rest
                    .iter()
                    .take_while(|c| {
                        c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'+' | b'e' | b'E')
                    })
                    .count();
                (1..=n).rev().collect()
            }
            b'c' => {
                if rest.is_empty() {
                    vec![]
                } else {
                    vec![1]
                }
            }
            _ => vec![],
        };
        ends
    }
    /// A literal run or a placeholder.
    enum Tok {
        Lit(Vec<u8>),
        Ph(u8),
    }
    let mut toks: Vec<Tok> = Vec::new();
    let mut lit = Vec::new();
    let mut k = 0;
    while k < pattern.len() {
        if pattern[k] == b'%' && k + 1 < pattern.len() {
            match pattern[k + 1] {
                b'%' => {
                    lit.push(b'%');
                    k += 2;
                    continue;
                }
                b'e' => {
                    lit.push(b'/');
                    k += 2;
                    continue;
                }
                kind @ (b's' | b'S' | b'a' | b'A' | b'w' | b'd' | b'i' | b'x' | b'f' | b'c') => {
                    if !lit.is_empty() {
                        toks.push(Tok::Lit(std::mem::take(&mut lit)));
                    }
                    toks.push(Tok::Ph(kind));
                    k += 2;
                    continue;
                }
                _ => {}
            }
        }
        lit.push(pattern[k]);
        k += 1;
    }
    if !lit.is_empty() {
        toks.push(Tok::Lit(lit));
    }
    // Backtracking over tokens (recursion depth = number of tokens).
    fn go(toks: &[Tok], t: &[u8], ti: usize) -> bool {
        let Some(tok) = toks.first() else {
            return ti == t.len();
        };
        match tok {
            Tok::Lit(l) => t[ti..].starts_with(l) && go(&toks[1..], t, ti + l.len()),
            Tok::Ph(kind) => class_len(*kind, t, ti)
                .into_iter()
                .any(|len| go(&toks[1..], t, ti + len)),
        }
    }
    go(&toks, text, 0)
}

#[test]
fn expectf_matcher() {
    assert!(expectf_match(
        b"object(DateTime)#%d (3)",
        b"object(DateTime)#12 (3)"
    ));
    assert!(expectf_match(b"a %s b", b"a xyz b"));
    assert!(!expectf_match(b"a %s b", b"a  b"));
    assert!(expectf_match(b"%a\nend", b"x\ny\nend"));
    assert!(expectf_match(b"int(%i)", b"int(-5)"));
    assert!(expectf_match(b"float(%f)", b"float(1.5E+5)"));
    assert!(expectf_match(b"100%%", b"100%"));
}
