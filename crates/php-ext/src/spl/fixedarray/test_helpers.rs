//! Test helpers shared by the `fixedarray`, `dllist` and `heap` tests.

#![allow(dead_code)]

use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, PropertyInfo, member_flags};
use php_runtime::object::WeakObjRef;
use php_runtime::types::TypeDecl;
use php_runtime::{Args, CallArgs, ClassRef, NativeFunction, ObjRef, PResult, PhpStr, Value, Vm};

use crate::spl::engine;

/// `parent::$method(...$args)` from a method of `this` (`class` is the
/// internal class whose implementation is called).
pub fn parent_call(
    vm: &mut Vm,
    this: &ObjRef,
    class: &str,
    method: &str,
    args: Vec<Value>,
) -> PResult<Value> {
    let ce = vm.find_class(class.as_bytes()).expect("parent class");
    let m = ce
        .find_method(method.as_bytes())
        .cloned()
        .expect("parent method");
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

/// A declared property for [`class_with_props`].
pub struct Prop {
    pub name: &'static str,
    pub flags: u32,
    pub default: Value,
    pub ty: Option<&'static str>,
}

impl Prop {
    pub fn public(name: &'static str, default: Value) -> Prop {
        Prop {
            name,
            flags: member_flags::PUBLIC,
            default,
            ty: None,
        }
    }
}

/// A "user" subclass of `parent` with declared properties and native
/// methods (class flags `flags`, e.g. `ALLOW_DYNAMIC_PROPERTIES`).
pub fn class_with_props(
    vm: &mut Vm,
    name: &str,
    parent: &str,
    flags: u32,
    props: Vec<Prop>,
    methods: &[&'static NativeFunction],
) -> ClassRef {
    let parent = vm.find_class(parent.as_bytes()).expect("parent class");
    let mut ce = ClassEntry::new_subclass(name, flags, &parent);
    for p in props {
        let mut info = PropertyInfo::new(p.name, name, p.flags);
        info.default = p.default;
        info.ty = p.ty.map(|t| TypeDecl::parse(t).expect("type"));
        ce.declare_property(info);
    }
    for nf in methods {
        ce.add_method(Function::native_method(
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

// ---------------------------------------------------------------------------
// Destructor probes
// ---------------------------------------------------------------------------

type DtorFn = Rc<dyn Fn(&mut Vm) -> PResult<()>>;

struct Probe(DtorFn);

fn probe_destruct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = engine::this(args);
    let f = this.native::<Probe>().map(|p| p.0.clone()).expect("probe");
    f(vm)?;
    Ok(Value::Null)
}

static PROBE_DESTRUCT: NativeFunction = NativeFunction {
    sig: "public function __destruct() {}",
    handler: probe_destruct,
};

/// An object whose `__destruct()` runs `f` (class `SplTestDtorProbe`).
pub fn probe(vm: &mut Vm, f: impl Fn(&mut Vm) -> PResult<()> + 'static) -> ObjRef {
    let ce = match vm.find_class(b"SplTestDtorProbe") {
        Some(ce) => ce,
        None => class_with_props(
            vm,
            "SplTestDtorProbe",
            "stdClass",
            0,
            Vec::new(),
            &[&PROBE_DESTRUCT],
        ),
    };
    let obj = ObjRef::new(ce);
    obj.set_native(Probe(Rc::new(f)));
    obj
}

/// Discards objects waiting for their destructor (left over by an earlier
/// VM on this thread).
pub fn drain_pending_destructors() {
    while let Some(o) = php_runtime::object::take_pending_destructor() {
        o.mark_destructor_called();
    }
}

/// A weak handle to `obj` for use inside probe closures (no cycles).
pub fn weak(obj: &ObjRef) -> WeakObjRef {
    obj.downgrade()
}

/// `echo $s;`
pub fn echo(vm: &mut Vm, s: &str) -> PResult<()> {
    vm.echo_str(s)
}

/// The class name and message of `e`'s `previous` exception.
pub fn previous(e: &ObjRef) -> Option<(String, String)> {
    match e.get_prop(b"previous") {
        Some(Value::Object(p)) => Some((p.class().name.to_string(), Vm::exception_message(&p))),
        _ => None,
    }
}

/// A string value from bytes.
pub fn bytes(b: &[u8]) -> Value {
    Value::String(PhpStr::from_bytes(b))
}
