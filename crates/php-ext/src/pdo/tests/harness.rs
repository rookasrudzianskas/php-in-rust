//! Helpers that let the PDO tests read like their `.phpt` originals: a
//! `var_dump()` writing to the VM's output, method calls, PHP-like user
//! classes built from native methods, and `--EXPECT--`/`--EXPECTF--`
//! comparison (run-tests.php trims both sides).

use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, PropertyInfo, member_flags};
use php_runtime::value::float_fmt;
use php_runtime::{
    Args, ArrayKey, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, Value, Vm,
};

use crate::pdo::consts::*;
use crate::testing::TestVm;

pub fn s(x: &str) -> Value {
    Value::from(x)
}

pub fn i(n: i64) -> Value {
    Value::Int(n)
}

pub fn arr(vals: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(vals))
}

pub fn assoc(pairs: Vec<(&str, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(ArrayKey::from(k), v);
    }
    Value::array(a)
}

pub fn iassoc(pairs: Vec<(i64, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(ArrayKey::Int(k), v);
    }
    Value::array(a)
}

/// The `--EXPECT--`/`--EXPECTF--` section of a phpt file.
pub fn expect_section(src: &str) -> (bool, String) {
    let mut lines = src.lines();
    let mut out = None;
    let mut expectf = false;
    for l in lines.by_ref() {
        if l == "--EXPECT--" || l == "--EXPECTF--" {
            expectf = l == "--EXPECTF--";
            out = Some(String::new());
            break;
        }
    }
    let mut out = out.expect("phpt has an EXPECT section");
    for l in lines {
        if l.starts_with("--") && l.ends_with("--") && l.len() > 4 && l[2..l.len() - 2].chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            break;
        }
        out.push_str(l);
        out.push('\n');
    }
    (expectf, out)
}

/// run-tests.php's `%` placeholders (`%s`, `%S`, `%a`, `%A`, `%d`, `%i`,
/// `%w`, `%c`, `%x`, `%f`, `%e`).
pub fn expectf_match(pat: &[u8], s: &[u8]) -> bool {
    if pat.is_empty() {
        return s.is_empty();
    }
    if pat[0] == b'%' && pat.len() > 1 {
        let rest = &pat[2..];
        let class: &dyn Fn(u8) -> bool = match pat[1] {
            b's' | b'S' => &|c| c != b'\n',
            b'a' | b'A' => &|_| true,
            b'd' => &|c: u8| c.is_ascii_digit(),
            b'i' => &|c: u8| c.is_ascii_digit() || c == b'-' || c == b'+',
            b'x' => &|c: u8| c.is_ascii_hexdigit(),
            b'f' => &|c: u8| c.is_ascii_digit() || b".-+eE".contains(&c),
            b'w' => &|c: u8| c.is_ascii_whitespace(),
            b'c' => {
                return !s.is_empty() && expectf_match(rest, &s[1..]);
            }
            b'e' => {
                return s.first() == Some(&b'/') && expectf_match(rest, &s[1..]);
            }
            b'%' => {
                return s.first() == Some(&b'%') && expectf_match(rest, &s[1..]);
            }
            _ => return !s.is_empty() && s[0] == pat[0] && expectf_match(&pat[1..], &s[1..]),
        };
        let min = if matches!(pat[1], b'S' | b'A' | b'w') { 0 } else { 1 };
        let mut n = 0;
        while n < s.len() && class(s[n]) {
            n += 1;
        }
        for k in (min..=n).rev() {
            if expectf_match(rest, &s[k..]) {
                return true;
            }
        }
        return false;
    }
    !s.is_empty() && s[0] == pat[0] && expectf_match(&pat[1..], &s[1..])
}

/// A PHP-like test session.
pub struct P {
    pub t: TestVm,
}

impl P {
    pub fn new() -> P {
        P { t: TestVm::new() }
    }

    pub fn vm(&mut self) -> &mut Vm {
        &mut self.t.vm
    }

    pub fn echo(&mut self, s: impl AsRef<[u8]>) {
        self.t.vm.echo(s.as_ref()).unwrap();
    }

    /// `new $class(...$args)`.
    pub fn try_new(&mut self, class: &str, args: Vec<Value>) -> PResult<ObjRef> {
        let ce = self.t.vm.find_class(class).unwrap_or_else(|| panic!("class {class}"));
        let obj = self.t.vm.instantiate(&ce)?;
        if ce.find_method(b"__construct").is_some() {
            self.t.vm.call_method(&obj, b"__construct", args)?;
        }
        Ok(obj)
    }

    pub fn new_obj(&mut self, class: &str, args: Vec<Value>) -> ObjRef {
        match self.try_new(class, args) {
            Ok(o) => o,
            Err(e) => panic!("new {class} threw {}", describe(&e)),
        }
    }

    /// `$obj->name(...$args)`.
    pub fn tm(&mut self, obj: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
        self.t.vm.call_method(obj, name.as_bytes(), args)
    }

    pub fn m(&mut self, obj: &ObjRef, name: &str, args: Vec<Value>) -> Value {
        match self.tm(obj, name, args) {
            Ok(v) => v,
            Err(e) => panic!("{}::{name}() threw {}", obj.class().name, describe(&e)),
        }
    }

    /// `Class::name(...$args)`.
    pub fn ts(&mut self, class: &str, name: &str, args: Vec<Value>) -> PResult<Value> {
        let callable = Value::from(format!("{class}::{name}"));
        self.t.vm.call(&callable, args)
    }

    pub fn obj(&mut self, o: &ObjRef, name: &str, args: Vec<Value>) -> ObjRef {
        match self.m(o, name, args) {
            Value::Object(x) => x,
            other => panic!("{name}() returned {other:?}"),
        }
    }

    /// `PDOTest::factory()` for `PDOTEST_DSN=sqlite::memory:`.
    pub fn factory(&mut self) -> ObjRef {
        self.factory_class("PDO")
    }

    pub fn factory_class(&mut self, class: &str) -> ObjRef {
        let db = self.new_obj(class, vec![s("sqlite::memory:")]);
        self.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(0)]);
        self.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(1)]);
        self.m(&db, "setAttribute", vec![i(ATTR_CASE), i(2)]);
        self.m(&db, "setAttribute", vec![i(ATTR_STRINGIFY_FETCHES), Value::Bool(true)]);
        db
    }

    pub fn exec(&mut self, db: &ObjRef, sql: &str) -> Value {
        self.m(db, "exec", vec![s(sql)])
    }

    /// `echo $e::class, ': ', $e->getMessage(), "\n"` for a thrown exception.
    pub fn catch(&mut self, r: PResult<Value>) -> Option<Value> {
        match r {
            Ok(v) => Some(v),
            Err(Flow::Exception(e)) => {
                let msg = Vm::exception_message(&e);
                self.echo(format!("{}: {msg}\n", e.class().name));
                None
            }
            Err(other) => panic!("unexpected flow {other:?}"),
        }
    }

    /// `var_dump($v)`.
    pub fn dump(&mut self, v: &Value) {
        let mut out = Vec::new();
        var_dump_into(&mut self.t.vm, &mut out, v, 0);
        self.echo(out);
    }

    pub fn output(&self) -> String {
        self.t.output()
    }

    /// Compares the output with the phpt expectation.
    pub fn check(&self, phpt: &str) {
        let (expectf, want) = expect_section(phpt);
        let got = self.output();
        let (w, g) = (want.trim(), got.trim());
        let ok = if expectf {
            expectf_match(w.as_bytes(), g.as_bytes())
        } else {
            w == g
        };
        assert!(ok, "output mismatch\n--- expected\n{w}\n--- got\n{g}\n");
    }
}

pub fn describe(e: &Flow) -> String {
    match e {
        Flow::Exception(o) => format!("{}: {}", o.class().name, Vm::exception_message(o)),
        other => format!("{other:?}"),
    }
}

/// `var_dump()` (`php_var_dump`).
pub fn var_dump_into(vm: &mut Vm, out: &mut Vec<u8>, v: &Value, level: usize) {
    let pad = " ".repeat(level);
    out.extend_from_slice(pad.as_bytes());
    match v {
        Value::Ref(r) => {
            out.truncate(out.len() - pad.len());
            let inner = r.get();
            var_dump_into(vm, out, &inner, level);
        }
        Value::Undef | Value::Null => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
        Value::Int(n) => out.extend_from_slice(format!("int({n})\n").as_bytes()),
        Value::Float(d) => {
            out.extend_from_slice(format!("float({})\n", float_fmt::repr(*d, -1)).as_bytes())
        }
        Value::String(x) => {
            out.extend_from_slice(format!("string({}) \"", x.len()).as_bytes());
            out.extend_from_slice(x);
            out.extend_from_slice(b"\"\n");
        }
        Value::Resource(r) => out.extend_from_slice(
            format!("resource({}) of type ({})\n", r.id(), r.type_name()).as_bytes(),
        ),
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, v) in a.iter() {
                dump_key(out, &k, level, None);
                var_dump_into(vm, out, v, level + 2);
            }
            out.extend_from_slice(pad.as_bytes());
            out.extend_from_slice(b"}\n");
        }
        Value::Object(o) => {
            let class = o.class().clone();
            if let Some(debug) = class.handlers.get_debug_info {
                if let Ok(Some(props)) = debug(vm, o, php_runtime::class::PropPurpose::Debug) {
                    out.extend_from_slice(
                        format!("object({})#{} ({}) {{\n", class.name, o.id(), props.len()).as_bytes(),
                    );
                    for (k, v) in props.iter() {
                        dump_key(out, &k, level, None);
                        var_dump_into(vm, out, v, level + 2);
                    }
                    out.extend_from_slice(pad.as_bytes());
                    out.extend_from_slice(b"}\n");
                    return;
                }
            }
            let mut entries: Vec<(ArrayKey, Option<String>, Option<Value>)> = Vec::new();
            {
                let props = o.obj().props.borrow();
                for info in class.properties.values() {
                    if info.is_static() {
                        continue;
                    }
                    let vis = if info.flags & member_flags::PRIVATE != 0 {
                        Some(format!("\"{}\":private", info.declaring_class))
                    } else if info.flags & member_flags::PROTECTED != 0 {
                        Some("protected".to_string())
                    } else {
                        None
                    };
                    let val = props.get(info.slot).cloned().unwrap_or(Value::Undef);
                    if val.is_undef() {
                        if info.ty.is_some() {
                            entries.push((ArrayKey::Str(info.name.clone()), vis, None));
                        }
                        continue;
                    }
                    entries.push((ArrayKey::Str(info.name.clone()), vis, Some(val)));
                }
            }
            if let Some(d) = o.obj().dyn_props.borrow().as_ref() {
                for (k, v) in d.iter() {
                    entries.push((k, None, Some(v.clone())));
                }
            }
            let count = entries.iter().filter(|e| e.2.is_some()).count();
            out.extend_from_slice(
                format!("object({})#{} ({count}) {{\n", class.name, o.id()).as_bytes(),
            );
            for (k, vis, val) in entries {
                dump_key(out, &k, level, vis.as_deref());
                match val {
                    Some(v) => var_dump_into(vm, out, &v, level + 2),
                    None => {
                        let ty = class
                            .find_property(k.to_php_str().as_bytes())
                            .and_then(|i| i.ty.as_ref().map(|t| t.to_type_string()))
                            .unwrap_or_default();
                        out.extend_from_slice(" ".repeat(level + 2).as_bytes());
                        out.extend_from_slice(format!("uninitialized({ty})\n").as_bytes());
                    }
                }
            }
            out.extend_from_slice(pad.as_bytes());
            out.extend_from_slice(b"}\n");
        }
    }
}

fn dump_key(out: &mut Vec<u8>, k: &ArrayKey, level: usize, vis: Option<&str>) {
    out.extend_from_slice(" ".repeat(level + 2).as_bytes());
    match k {
        ArrayKey::Int(n) => out.extend_from_slice(format!("[{n}]=>\n").as_bytes()),
        ArrayKey::Str(s) => {
            out.extend_from_slice(b"[\"");
            out.extend_from_slice(s);
            out.push(b'"');
            if let Some(vis) = vis {
                out.push(b':');
                out.extend_from_slice(vis.as_bytes());
            }
            out.extend_from_slice(b"]=>\n");
        }
    }
}

/// Echoes `Class::__construct(args...)` like the test classes' constructors.
fn echo_ctor(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let scope = args.function().scope.clone().unwrap_or_default();
    let mut parts = Vec::new();
    for v in args.values() {
        parts.push(vm.to_php_string(&v.unref())?.to_string_lossy().into_owned());
    }
    vm.echo(format!("{scope}::__construct({})\n", parts.join(", ")).as_bytes())?;
    Ok(Value::Null)
}

fn silent_ctor(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

/// `function __construct()` echoing `__METHOD__ . "()"`.
pub static ECHO_CTOR0: NativeFunction = NativeFunction {
    sig: "public function __construct()",
    handler: echo_ctor,
};
/// `function __construct($name = 'N/A')` echoing `__METHOD__ . "($name)"`.
pub static ECHO_CTOR_NAME: NativeFunction = NativeFunction {
    sig: "public function __construct($name = 'N/A')",
    handler: echo_ctor,
};
/// `function __construct(string $str)`.
pub static ECHO_CTOR_STR: NativeFunction = NativeFunction {
    sig: "public function __construct(string $str)",
    handler: echo_ctor,
};
/// `function __construct(string $a, string $b)`.
pub static ECHO_CTOR_AB: NativeFunction = NativeFunction {
    sig: "public function __construct(string $a, string $b)",
    handler: echo_ctor,
};
/// `function __construct() {}`.
pub static EMPTY_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct()",
    handler: silent_ctor,
};

/// Declares a userland-like class with untyped properties
/// `(name, visibility)` and an optional constructor.
pub fn user_class(
    vm: &mut Vm,
    name: &str,
    parent: Option<&str>,
    props: &[(&str, u32)],
    ctor: Option<(&'static NativeFunction, u32)>,
) -> ClassRef {
    let mut ce = match parent {
        Some(p) => {
            let parent = vm.find_class(p).expect("parent class");
            ClassEntry::new_subclass(name, 0, &parent)
        }
        None => ClassEntry::new(name, 0),
    };
    for (p, vis) in props {
        let mut info = PropertyInfo::new(*p, name, *vis);
        info.default = Value::Null;
        ce.declare_property(info);
    }
    if let Some((nf, flags)) = ctor {
        ce.add_method(Function::native_method(nf, name, flags));
    }
    ce.link_magic_methods();
    let rc = Rc::new(ce);
    vm.register_class(rc.clone());
    rc
}

pub const PUBLIC: u32 = member_flags::PUBLIC;
pub const PROTECTED: u32 = member_flags::PROTECTED;
pub const PRIVATE: u32 = member_flags::PRIVATE;
