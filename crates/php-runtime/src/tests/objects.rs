//! Object handles, destructor queue and class data model.

use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::class::{ClassEntry, Function, PropertyInfo, member_flags};
use crate::object::{self, ObjRef};

thread_local! {
    static DESTRUCTED: Cell<u32> = const { Cell::new(0) };
}

fn destruct(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    DESTRUCTED.with(|c| c.set(c.get() + 1));
    Ok(Value::Null)
}

static DESTRUCT: NativeFunction = NativeFunction {
    sig: "__destruct(): void",
    handler: destruct,
};

fn class_with_destructor() -> crate::class::ClassRef {
    let mut ce = ClassEntry::new("WithDtor", 0);
    ce.add_method(Function::native_method(
        &DESTRUCT,
        "WithDtor",
        member_flags::PUBLIC,
    ));
    ce.declare_property(PropertyInfo::new("p", "WithDtor", member_flags::PUBLIC));
    ce.link_magic_methods();
    Rc::new(ce)
}

#[test]
fn handle_ids_are_reused_lifo() {
    let t = TestVm::new();
    let std = t.vm.find_class(b"stdClass").unwrap();
    let a = ObjRef::new(std.clone());
    let b = ObjRef::new(std.clone());
    let c = ObjRef::new(std.clone());
    let (ia, ib, ic) = (a.id(), b.id(), c.id());
    assert_eq!(ib, ia + 1);
    assert_eq!(ic, ib + 1);
    drop(a);
    drop(c);
    // Most recently freed first.
    let d = ObjRef::new(std.clone());
    assert_eq!(d.id(), ic);
    let e = ObjRef::new(std.clone());
    assert_eq!(e.id(), ia);
    let f = ObjRef::new(std);
    assert_eq!(f.id(), ic + 1);
    assert!(object::object_by_id(ib).is_some());
    drop(b);
    assert!(object::object_by_id(ib).is_none());
}

#[test]
fn destructors_are_queued_and_run_once() {
    let mut t = TestVm::new();
    DESTRUCTED.with(|c| c.set(0));
    let ce = class_with_destructor();
    assert!(ce.has_destructor());
    let o = ObjRef::new(ce.clone());
    let o2 = o.clone();
    drop(o);
    assert!(!object::has_pending_destructors());
    drop(o2);
    assert!(object::has_pending_destructors());
    t.vm.run_pending_destructors().unwrap();
    assert_eq!(DESTRUCTED.with(Cell::get), 1);
    assert!(!object::has_pending_destructors());

    // Objects freed by a destructor-less container are queued in order.
    let a = ObjRef::new(ce.clone());
    let b = ObjRef::new(ce.clone());
    a.set_prop(b"p", Value::Object(b));
    drop(a);
    t.vm.run_pending_destructors().unwrap();
    assert_eq!(DESTRUCTED.with(Cell::get), 3);

    // Shutdown destructs remaining objects.
    let keep = ObjRef::new(ce.clone());
    t.vm.call_remaining_destructors().unwrap();
    assert_eq!(DESTRUCTED.with(Cell::get), 4);
    assert!(keep.destructor_called());
    drop(keep);
    t.vm.run_pending_destructors().unwrap();
    assert_eq!(DESTRUCTED.with(Cell::get), 4);

    // Plain classes never queue.
    let std = t.vm.find_class(b"stdClass").unwrap();
    drop(ObjRef::new(std));
    assert!(!object::has_pending_destructors());
}

#[test]
fn properties_and_inheritance() {
    let mut base = ClassEntry::new("Base", 0);
    let mut p = PropertyInfo::new("x", "Base", member_flags::PRIVATE);
    p.default = Value::Int(1);
    base.declare_property(p);
    let mut q = PropertyInfo::new("y", "Base", member_flags::PUBLIC);
    q.default = Value::Int(2);
    base.declare_property(q);
    let base = Rc::new(base);
    let mut child = ClassEntry::new_subclass("Child", 0, &base);
    let mut px = PropertyInfo::new("x", "Child", member_flags::PUBLIC);
    px.default = Value::Int(3);
    child.declare_property(px);
    let mut py = PropertyInfo::new("y", "Child", member_flags::PUBLIC);
    py.default = Value::Int(4);
    child.declare_property(py);
    let child = Rc::new(child);
    // The private parent property keeps its own slot; the public one is shared.
    assert_eq!(child.default_properties.len(), 3);
    assert!(child.instance_of(&base));
    assert!(!base.instance_of(&child));
    let o = ObjRef::new(child.clone());
    assert_eq!(o.get_prop(b"x").unwrap().as_int(), Some(3));
    assert_eq!(o.get_prop(b"y").unwrap().as_int(), Some(4));
    let mut t = TestVm::new();
    let arr = t.vm.cast_to_array(&Value::Object(o.clone())).unwrap();
    let keys: Vec<String> = arr.keys().map(|k| format!("{k:?}")).collect();
    assert_eq!(keys, ["\"\\0Base\\0x\"", "\"y\"", "\"x\""]);
    o.set_prop(b"dyn", Value::Int(9));
    assert_eq!(o.get_prop(b"dyn").unwrap().as_int(), Some(9));
    assert_eq!(o.properties().len(), 3);
}

#[test]
fn native_storage() {
    let t = TestVm::new();
    let std = t.vm.find_class(b"stdClass").unwrap();
    let o = ObjRef::new(std);
    o.set_native(vec![1u8, 2, 3]);
    assert_eq!(o.native::<Vec<u8>>().unwrap().len(), 3);
    o.native_mut::<Vec<u8>>().unwrap().push(4);
    assert_eq!(o.native::<Vec<u8>>().unwrap().len(), 4);
    assert!(o.native::<String>().is_none());
}
