//! Test helpers: a `var_dump()` emulation, an EXPECTF matcher, a host with
//! Rust callables/autoloader and a builder for internal test classes.

use std::cell::RefCell;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, ClassRef, Function, PropertyInfo, member_flags};
use php_runtime::types::TypeDecl;
use php_runtime::value::float_fmt;
use php_runtime::vm::{Host, default_call_callable, default_check_callable};
use php_runtime::{ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Value, Vm};

pub use crate::testing::{TestVm, thrown};

/// `serialize($v)` as text (lossy for binary data).
pub fn ser(t: &mut TestVm, v: Value) -> String {
    String::from_utf8_lossy(&ser_bytes(t, v)).into_owned()
}

pub fn ser_bytes(t: &mut TestVm, v: Value) -> Vec<u8> {
    match t.call("serialize", vec![v]) {
        Ok(Value::String(s)) => s.to_vec(),
        other => panic!("serialize() failed: {other:?}"),
    }
}

/// `unserialize(serialize($v))`.
pub fn round_trip(t: &mut TestVm, v: Value) -> Value {
    let data = ser_bytes(t, v);
    unser(t, &data)
}

/// `unserialize($s)`; panics on exceptions.
pub fn unser(t: &mut TestVm, s: &[u8]) -> Value {
    t.call("unserialize", vec![Value::bytes(s)])
        .unwrap_or_else(|e| panic!("unserialize() threw: {e:?}"))
}

/// `unserialize($s, $options)`.
pub fn unser_opts(t: &mut TestVm, s: &[u8], opts: Vec<(&str, Value)>) -> PResult<Value> {
    let mut o = PhpArray::new();
    for (k, v) in opts {
        o.insert_str(k, v);
    }
    t.call("unserialize", vec![Value::bytes(s), Value::array(o)])
}

/// `var_dump($v)` and the output emitted so far, as one string.
pub fn dump_out(t: &TestVm, v: &Value) -> String {
    let mut s = t.output();
    s.push_str(&dump(t, v));
    s
}

/// `var_dump($v)` (`php_var_dump`) with the VM's `serialize_precision`.
pub fn dump(t: &TestVm, v: &Value) -> String {
    dump_prec(t.vm.serialize_precision, v)
}

/// `var_dump($v)` with an explicit `serialize_precision`.
pub fn dump_prec(prec: i64, v: &Value) -> String {
    let mut d = Dumper {
        out: Vec::new(),
        prec,
        arrays: Vec::new(),
        objects: Vec::new(),
    };
    d.dump(v, 1);
    String::from_utf8_lossy(&d.out).into_owned()
}

struct Dumper {
    out: Vec<u8>,
    prec: i64,
    arrays: Vec<*const PhpArray>,
    objects: Vec<u32>,
}

impl Dumper {
    fn indent(&mut self, n: usize) {
        self.out.extend(std::iter::repeat_n(b' ', n));
    }

    fn put(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn dump(&mut self, v: &Value, level: usize) {
        if level > 1 {
            self.indent(level - 1);
        }
        let (is_ref, v) = match v {
            Value::Ref(r) => (r.refcount() > 1, r.get()),
            v => (false, v.clone()),
        };
        let c = if is_ref { "&" } else { "" };
        match v {
            Value::Bool(b) => self.put(&format!("{c}bool({b})\n")),
            Value::Null | Value::Undef => self.put(&format!("{c}NULL\n")),
            Value::Int(i) => self.put(&format!("{c}int({i})\n")),
            Value::Float(d) => {
                let f = float_fmt::format_h(d, self.prec);
                self.put(&format!("{c}float({f})\n"));
            }
            Value::String(s) => {
                self.put(&format!("{c}string({}) \"", s.len()));
                self.out.extend_from_slice(&s);
                self.put("\"\n");
            }
            Value::Array(a) => {
                let p = Rc::as_ptr(&a);
                if self.arrays.contains(&p) {
                    self.put("*RECURSION*\n");
                    return;
                }
                self.arrays.push(p);
                self.put(&format!("{c}array({}) {{\n", a.len()));
                for (k, val) in a.iter() {
                    self.indent(level + 1);
                    match k {
                        ArrayKey::Int(i) => self.put(&format!("[{i}]=>\n")),
                        ArrayKey::Str(s) => {
                            self.put("[\"");
                            self.out.extend_from_slice(&s);
                            self.put("\"]=>\n");
                        }
                    }
                    self.dump(val, level + 2);
                }
                self.arrays.pop();
                if level > 1 {
                    self.indent(level - 1);
                }
                self.put("}\n");
            }
            Value::Object(o) => self.object(&o, c, level),
            Value::Resource(r) => {
                self.put(&format!(
                    "{c}resource({}) of type ({})\n",
                    r.id(),
                    r.type_name()
                ));
            }
            Value::Ref(_) => unreachable!(),
        }
    }

    fn object(&mut self, o: &ObjRef, c: &str, level: usize) {
        let ce = o.class().clone();
        if ce.is_enum() {
            let case = o.get_prop(b"name").map(|v| v.unref()).unwrap_or_default();
            let case = case.as_str().map(|s| s.to_string_lossy().into_owned());
            self.put(&format!(
                "{c}enum({}::{})\n",
                ce.name,
                case.unwrap_or_default()
            ));
            return;
        }
        if self.objects.contains(&o.id()) {
            self.put("*RECURSION*\n");
            return;
        }
        self.objects.push(o.id());
        // (key, value, type of an uninitialized typed property)
        let mut props: Vec<(ArrayKey, Value, Option<String>)> = Vec::new();
        {
            let slots = o.obj().props.borrow();
            for d in super::super::declared_props(o) {
                let v = slots.get(d.slot).cloned().unwrap_or_default();
                if v.is_undef() {
                    if let Some(ty) = &d.info.ty {
                        props.push((ArrayKey::Str(d.key), v, Some(ty.to_type_string())));
                    }
                    continue;
                }
                props.push((ArrayKey::Str(d.key), v, None));
            }
        }
        if let Some(t) = o.obj().dyn_props.borrow().as_ref() {
            for (k, v) in t.iter() {
                props.push((k, v.clone(), None));
            }
        }
        let count = props.iter().filter(|(_, v, _)| !v.is_undef()).count();
        self.put(&format!("{c}object({})#{} ({count}) {{\n", ce.name, o.id()));
        for (k, v, uninit) in props {
            self.indent(level + 1);
            match k {
                ArrayKey::Int(i) => self.put(&format!("[{i}]=>\n")),
                ArrayKey::Str(s) => {
                    self.put("[");
                    let b = s.as_bytes();
                    let parts = (b.len() > 2 && b[0] == 0)
                        .then(|| b[1..].iter().position(|&x| x == 0).map(|i| i + 1))
                        .flatten();
                    match parts {
                        Some(end) if &b[1..end] == b"*" => {
                            self.put("\"");
                            self.out.extend_from_slice(&b[end + 1..]);
                            self.put("\":protected");
                        }
                        Some(end) => {
                            self.put("\"");
                            self.out.extend_from_slice(&b[end + 1..]);
                            self.put("\":\"");
                            self.out.extend_from_slice(&b[1..end]);
                            self.put("\":private");
                        }
                        None => {
                            self.put("\"");
                            self.out.extend_from_slice(b);
                            self.put("\"");
                        }
                    }
                    self.put("]=>\n");
                }
            }
            match uninit {
                Some(ty) => {
                    self.indent(level + 1);
                    self.put(&format!("uninitialized({ty})\n"));
                }
                None => self.dump(&v, level + 2),
            }
        }
        if level > 1 {
            self.indent(level - 1);
        }
        self.put("}\n");
        self.objects.pop();
    }
}

/// run-tests.php `--EXPECTF--` matching (`%s`, `%d`, `%a`, `%e`), with
/// trailing whitespace trimmed like run-tests does.
#[track_caller]
pub fn expectf(actual: &str, expected: &str) {
    let a = actual.trim_end();
    let e = expected.trim_end();
    let a = a.trim_start_matches('\n');
    let e = e.trim_start_matches('\n');
    assert!(
        matches(e.as_bytes(), a.as_bytes()),
        "output mismatch\n--- expected ---\n{e}\n--- actual ---\n{a}\n"
    );
}

fn matches(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0, 0);
    while p < pat.len() {
        if pat[p] == b'%' && p + 1 < pat.len() {
            let rest = &pat[p + 2..];
            let class: fn(u8) -> bool = match pat[p + 1] {
                b's' => |c| c != b'\n',
                b'd' => |c| c.is_ascii_digit(),
                b'a' => |_| true,
                b'e' => {
                    if s.get(i) != Some(&b'/') {
                        return false;
                    }
                    i += 1;
                    p += 2;
                    continue;
                }
                _ => {
                    if s.get(i) != Some(&b'%') {
                        return false;
                    }
                    i += 1;
                    p += 1;
                    continue;
                }
            };
            let mut j = i;
            while j < s.len() && class(s[j]) {
                j += 1;
            }
            // Greedy with backtracking, at least one character.
            return (i + 1..=j).rev().any(|k| matches(rest, &s[k..]));
        }
        if s.get(i) != Some(&pat[p]) {
            return false;
        }
        p += 1;
        i += 1;
    }
    i == s.len()
}

type Callable = Rc<dyn Fn(&mut Vm, Vec<Value>) -> PResult<Value>>;
type Autoloader = Rc<dyn Fn(&mut Vm, &[u8]) -> PResult<Option<ClassRef>>>;

/// A host with Rust callables (by name) and an optional autoloader.
#[derive(Default)]
pub struct TestHost {
    fns: RefCell<Vec<(String, Callable)>>,
    autoload: RefCell<Option<Autoloader>>,
}

impl TestHost {
    pub fn install(t: &mut TestVm) -> Rc<TestHost> {
        let h = Rc::new(TestHost::default());
        t.vm.set_host(h.clone());
        h
    }

    pub fn function(
        &self,
        name: &str,
        f: impl Fn(&mut Vm, Vec<Value>) -> PResult<Value> + 'static,
    ) {
        self.fns.borrow_mut().push((name.to_string(), Rc::new(f)));
    }

    pub fn autoloader(&self, f: impl Fn(&mut Vm, &[u8]) -> PResult<Option<ClassRef>> + 'static) {
        *self.autoload.borrow_mut() = Some(Rc::new(f));
    }

    fn find(&self, v: &Value) -> Option<Callable> {
        let Value::String(s) = v.unref() else {
            return None;
        };
        self.fns
            .borrow()
            .iter()
            .find(|(n, _)| n.as_bytes() == s.as_bytes())
            .map(|(_, f)| f.clone())
    }
}

impl Host for TestHost {
    fn call_callable(&self, vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
        match self.find(callable) {
            Some(f) => f(vm, args),
            None => default_call_callable(vm, callable, args),
        }
    }

    fn check_callable(&self, vm: &mut Vm, callable: &Value) -> Result<PhpStr, String> {
        match self.find(callable) {
            Some(_) => Ok(callable.unref().as_str().cloned().unwrap_or_default()),
            None => default_check_callable(vm, callable),
        }
    }

    fn autoload(&self, vm: &mut Vm, name: &[u8]) -> PResult<Option<ClassRef>> {
        let f = self.autoload.borrow().clone();
        match f {
            Some(f) => f(vm, name),
            None => Ok(None),
        }
    }
}

/// Builds an internal class standing in for a userland test class.
pub struct ClassBuilder {
    ce: ClassEntry,
}

impl ClassBuilder {
    pub fn new(name: &str) -> ClassBuilder {
        ClassBuilder {
            ce: ClassEntry::new(name, 0),
        }
    }

    pub fn extends(t: &TestVm, name: &str, parent: &str) -> ClassBuilder {
        let parent = t.vm.find_class(parent.as_bytes()).expect("parent class");
        ClassBuilder {
            ce: ClassEntry::new_subclass(name, 0, &parent),
        }
    }

    pub fn flags(mut self, f: u32) -> Self {
        self.ce.flags |= f;
        self
    }

    pub fn implements(mut self, t: &TestVm, iface: &str) -> Self {
        let i = t.vm.find_class(iface.as_bytes()).expect("interface");
        for sub in i.interfaces.clone() {
            self.ce.interfaces.push(sub);
        }
        self.ce.interfaces.push(i);
        self
    }

    /// Declares a property; `ty` is a type declaration (`None` untyped).
    pub fn prop(mut self, name: &str, flags: u32, ty: Option<&str>, default: Value) -> Self {
        let mut p = PropertyInfo::new(name, self.ce.name.clone(), flags);
        p.ty = ty.map(|t| TypeDecl::parse(t).expect("valid type"));
        p.default = default;
        self.ce.declare_property(p);
        self
    }

    pub fn public(self, name: &str) -> Self {
        self.prop(name, member_flags::PUBLIC, None, Value::Null)
    }

    pub fn typed(self, name: &str, ty: &str) -> Self {
        self.prop(name, member_flags::PUBLIC, Some(ty), Value::Undef)
    }

    pub fn method(mut self, nf: &'static NativeFunction) -> Self {
        let name = self.ce.name.to_string();
        self.ce
            .add_method(Function::native_method(nf, &name, member_flags::PUBLIC));
        self
    }

    pub fn build(mut self) -> ClassRef {
        self.ce.link_magic_methods();
        Rc::new(self.ce)
    }

    pub fn register(self, t: &mut TestVm) -> ClassRef {
        let ce = self.build();
        assert!(t.vm.register_class(ce.clone()), "class registered twice");
        ce
    }
}

/// A new instance (`new $class` without constructor).
pub fn new_obj(t: &mut TestVm, class: &str) -> ObjRef {
    let ce = t.vm.find_class(class.as_bytes()).expect("class");
    t.vm.instantiate(&ce).expect("instantiate")
}

/// Shorthand for `Value::str`.
pub fn s(v: &str) -> Value {
    Value::str(v)
}

/// An array literal from values.
pub fn list(vals: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(vals))
}

/// An array literal from string keys.
pub fn assoc(pairs: Vec<(&str, Value)>) -> Value {
    let mut a = PhpArray::new();
    for (k, v) in pairs {
        a.insert(ArrayKey::from_bytes(k.as_bytes()), v);
    }
    Value::array(a)
}

#[test]
fn expectf_matcher() {
    assert!(matches(b"a%sb", b"axxb"));
    assert!(!matches(b"a%sb", b"ab"));
    assert!(!matches(b"a%sb", b"a\nb"));
    assert!(matches(b"#%d (", b"#12 ("));
    assert!(matches(b"in %s on line %d", b"in /t.php on line 3"));
    assert!(matches(b"x%ay", b"x\n\ny"));
}
