//! Objects (`zend_object`) and the object store (`zend_objects_API.c`).
//!
//! * [`ObjRef`] is a shared handle (`Rc<PhpObject>`); `===` compares identity.
//! * Every object gets a handle id (`spl_object_id`); freed ids are reused
//!   most-recently-freed first, like the object store free list.
//! * Destructors: when the last `ObjRef` of an object whose class has a
//!   destructor is dropped (and the destructor has not run yet), the object is
//!   pushed onto a thread-local pending queue instead of being freed. The
//!   interpreter drains the queue at safe points with
//!   [`crate::vm::Vm::run_pending_destructors`], calling `__destruct`. At
//!   shutdown, [`live_objects`] lists the remaining objects in handle order.
//! * There is no cycle collector: reference cycles are never freed (but the
//!   shutdown sequence still runs their destructors).

use std::any::Any;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::VecDeque;
use std::fmt;
use std::rc::{Rc, Weak};

use crate::class::ClassRef;
use crate::value::{ArrayKey, KeyRef, PhpArray, PhpStr, Value};

/// Object flags (subset of `IS_OBJ_*`).
pub mod obj_flags {
    /// `__destruct` was called (or must not be called any more).
    pub const DESTRUCTOR_CALLED: u32 = 1 << 0;
    /// The object is queued for destruction.
    pub const DESTRUCTOR_QUEUED: u32 = 1 << 1;
    /// Free-for-interpreter use: lazy object, uninitialised readonly clone, ...
    pub const USER_1: u32 = 1 << 8;
    pub const USER_2: u32 = 1 << 9;
    pub const USER_3: u32 = 1 << 10;
}

/// A PHP object.
pub struct PhpObject {
    id: u32,
    class: ClassRef,
    flags: Cell<u32>,
    /// Declared property slots (`properties_table`); `Undef` means
    /// uninitialised (typed) or unset.
    pub props: RefCell<Vec<Value>>,
    /// Dynamic properties (`properties`), in insertion order. Keys are always
    /// `ArrayKey::Str` (property tables never use integer keys).
    pub dyn_props: RefCell<Option<PhpArray>>,
    /// State of internal classes (ArrayObject storage, DateTime, Closure, ...).
    pub native: RefCell<Option<Box<dyn Any>>>,
    /// Interpreter-private per-object data (property guards, lazy state, ...).
    pub extra: RefCell<Option<Box<dyn Any>>>,
}

/// Shared handle to an object.
pub struct ObjRef(Rc<PhpObject>);

struct ObjectStore {
    slots: Vec<Weak<PhpObject>>,
    free: Vec<u32>,
}

thread_local! {
    static STORE: RefCell<ObjectStore> = RefCell::new(ObjectStore {
        // Handle 0 is never used (php-src starts at 1).
        slots: vec![Weak::new()],
        free: Vec::new(),
    });
    static PENDING_DESTRUCTORS: RefCell<VecDeque<ObjRef>> = const { RefCell::new(VecDeque::new()) };
}

impl ObjRef {
    /// Allocates an object of `class` with the class's default property values
    /// (`zend_objects_new` + `object_properties_init`).
    pub fn new(class: ClassRef) -> ObjRef {
        let props = class.default_properties.clone();
        Self::with_props(class, props)
    }

    /// Allocates an object with explicit declared-property slot values.
    pub fn with_props(class: ClassRef, props: Vec<Value>) -> ObjRef {
        let rc = Rc::new_cyclic(|weak: &Weak<PhpObject>| {
            let id = STORE.with(|s| {
                let mut s = s.borrow_mut();
                match s.free.pop() {
                    Some(id) => {
                        s.slots[id as usize] = weak.clone();
                        id
                    }
                    None => {
                        s.slots.push(weak.clone());
                        (s.slots.len() - 1) as u32
                    }
                }
            });
            PhpObject {
                id,
                class,
                flags: Cell::new(0),
                props: RefCell::new(props),
                dyn_props: RefCell::new(None),
                native: RefCell::new(None),
                extra: RefCell::new(None),
            }
        });
        ObjRef(rc)
    }

    /// `spl_object_id` / the `#N` shown by `var_dump`.
    #[inline]
    pub fn id(&self) -> u32 {
        self.0.id
    }

    #[inline]
    pub fn class(&self) -> &ClassRef {
        &self.0.class
    }

    #[inline]
    pub fn obj(&self) -> &PhpObject {
        &self.0
    }

    pub fn ptr_eq(a: &ObjRef, b: &ObjRef) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }

    /// Number of live handles (`GC_REFCOUNT`).
    pub fn refcount(&self) -> usize {
        Rc::strong_count(&self.0)
    }

    pub fn downgrade(&self) -> WeakObjRef {
        WeakObjRef(Rc::downgrade(&self.0))
    }

    pub fn flags(&self) -> u32 {
        self.0.flags.get()
    }

    pub fn set_flag(&self, f: u32) {
        self.0.flags.set(self.0.flags.get() | f);
    }

    pub fn clear_flag(&self, f: u32) {
        self.0.flags.set(self.0.flags.get() & !f);
    }

    pub fn has_flag(&self, f: u32) -> bool {
        self.0.flags.get() & f != 0
    }

    pub fn destructor_called(&self) -> bool {
        self.has_flag(obj_flags::DESTRUCTOR_CALLED)
    }

    /// Marks the destructor as called (php-src sets this before invoking it).
    pub fn mark_destructor_called(&self) {
        self.set_flag(obj_flags::DESTRUCTOR_CALLED);
    }

    /// Typed accessor for [`PhpObject::native`].
    pub fn native<T: Any>(&self) -> Option<Ref<'_, T>> {
        Ref::filter_map(self.0.native.borrow(), |n| {
            n.as_ref().and_then(|b| b.downcast_ref::<T>())
        })
        .ok()
    }

    pub fn native_mut<T: Any>(&self) -> Option<RefMut<'_, T>> {
        RefMut::filter_map(self.0.native.borrow_mut(), |n| {
            n.as_mut().and_then(|b| b.downcast_mut::<T>())
        })
        .ok()
    }

    pub fn set_native<T: Any>(&self, v: T) {
        *self.0.native.borrow_mut() = Some(Box::new(v));
    }

    /// Reads a property by name: declared slot first, then dynamic properties.
    /// Visibility is not checked (that is the interpreter's job).
    pub fn get_prop(&self, name: &[u8]) -> Option<Value> {
        if let Some(info) = self.0.class.find_property(name) {
            if !info.is_static() {
                let v = self.0.props.borrow().get(info.slot).cloned()?;
                return (!v.is_undef()).then_some(v);
            }
        }
        self.0
            .dyn_props
            .borrow()
            .as_ref()
            .and_then(|d| d.get_by(KeyRef::Str(name)).cloned())
    }

    /// Writes a property by name (declared slot if present, else dynamic).
    pub fn set_prop(&self, name: &[u8], v: Value) {
        if let Some(info) = self.0.class.find_property(name) {
            if !info.is_static() {
                let mut props = self.0.props.borrow_mut();
                if info.slot < props.len() {
                    match &props[info.slot] {
                        Value::Ref(r) => {
                            r.set(v);
                        }
                        _ => props[info.slot] = v.into_unref(),
                    }
                    return;
                }
            }
        }
        let mut d = self.0.dyn_props.borrow_mut();
        let table = d.get_or_insert_with(PhpArray::new);
        let key = ArrayKey::Str(PhpStr::from_bytes(name));
        match table.get_mut(&key) {
            Some(Value::Ref(r)) => {
                r.set(v);
            }
            Some(slot) => *slot = v.into_unref(),
            None => {
                table.insert(key, v.into_unref());
            }
        }
    }

    /// All initialised properties in PHP order (declared, then dynamic),
    /// keyed by plain property name (`zend_std_get_properties` without mangling).
    pub fn properties(&self) -> Vec<(PhpStr, Value)> {
        let mut out = Vec::new();
        let props = self.0.props.borrow();
        for info in self.0.class.properties.values() {
            if info.is_static() {
                continue;
            }
            if let Some(v) = props.get(info.slot) {
                if !v.is_undef() {
                    out.push((info.name.clone(), v.clone()));
                }
            }
        }
        if let Some(d) = self.0.dyn_props.borrow().as_ref() {
            for (k, v) in d.iter() {
                out.push((k.to_php_str(), v.clone()));
            }
        }
        out
    }
}

impl Clone for ObjRef {
    fn clone(&self) -> Self {
        ObjRef(Rc::clone(&self.0))
    }
}

impl Drop for ObjRef {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) != 1 {
            return;
        }
        let obj = &*self.0;
        let f = obj.flags.get();
        if f & (obj_flags::DESTRUCTOR_CALLED | obj_flags::DESTRUCTOR_QUEUED) != 0
            || !obj.class.has_destructor()
        {
            return;
        }
        obj.flags.set(f | obj_flags::DESTRUCTOR_QUEUED);
        let handle = ObjRef(Rc::clone(&self.0));
        let _ = PENDING_DESTRUCTORS.try_with(move |q| {
            if let Ok(mut q) = q.try_borrow_mut() {
                q.push_back(handle);
            }
        });
    }
}

impl Drop for PhpObject {
    fn drop(&mut self) {
        let id = self.id;
        let _ = STORE.try_with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                if let Some(slot) = s.slots.get_mut(id as usize) {
                    *slot = Weak::new();
                    s.free.push(id);
                }
            }
        });
    }
}

impl PartialEq for ObjRef {
    fn eq(&self, other: &Self) -> bool {
        ObjRef::ptr_eq(self, other)
    }
}

impl fmt::Debug for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.class().name, self.id())
    }
}

/// Weak object handle (`WeakReference`, `WeakMap`).
#[derive(Clone)]
pub struct WeakObjRef(Weak<PhpObject>);

impl WeakObjRef {
    pub fn upgrade(&self) -> Option<ObjRef> {
        self.0.upgrade().map(ObjRef)
    }
}

/// Takes the next object waiting for its destructor (FIFO).
pub fn take_pending_destructor() -> Option<ObjRef> {
    PENDING_DESTRUCTORS
        .try_with(|q| q.borrow_mut().pop_front())
        .ok()
        .flatten()
}

/// Whether objects are waiting for their destructor.
pub fn has_pending_destructors() -> bool {
    PENDING_DESTRUCTORS
        .try_with(|q| !q.borrow().is_empty())
        .unwrap_or(false)
}

/// All live objects in handle order (used by the shutdown sequence,
/// `zend_objects_store_call_destructors`).
pub fn live_objects() -> Vec<ObjRef> {
    STORE
        .try_with(|s| {
            s.borrow()
                .slots
                .iter()
                .filter_map(|w| w.upgrade().map(ObjRef))
                .collect()
        })
        .unwrap_or_default()
}

/// Marks every live object as destructed and drops the pending queue
/// (`zend_objects_store_mark_destructed`, used after fatal errors).
pub fn mark_all_destructed() {
    for obj in live_objects() {
        obj.mark_destructor_called();
    }
    let drained: Vec<ObjRef> = PENDING_DESTRUCTORS
        .try_with(|q| q.borrow_mut().drain(..).collect())
        .unwrap_or_default();
    for o in &drained {
        o.mark_destructor_called();
    }
    drop(drained);
}

/// Looks up a live object by handle.
pub fn object_by_id(id: u32) -> Option<ObjRef> {
    STORE
        .try_with(|s| {
            s.borrow()
                .slots
                .get(id as usize)
                .and_then(|w| w.upgrade())
                .map(ObjRef)
        })
        .ok()
        .flatten()
}

/// Number of handle slots in use (`EG(objects_store).top`).
pub fn store_top() -> usize {
    STORE.try_with(|s| s.borrow().slots.len()).unwrap_or(0)
}
