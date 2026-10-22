//! `WeakReference` and `WeakMap` (`Zend/zend_weakrefs.c`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::class::{ClassEntry, ClassRef, DimFetch, Function, class_flags, member_flags};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::{ObjRef, WeakObjRef};
use crate::value::{PhpArray, Value};
use crate::vm::{PResult, Vm};

struct WeakRefData {
    target: WeakObjRef,
}

thread_local! {
    /// target object id → WeakReference object (one per target).
    static WEAKREFS: RefCell<HashMap<u32, (WeakObjRef, WeakObjRef)>> = RefCell::new(HashMap::new());
}

fn wr_create(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let target = a.object(0).clone();
    let existing = WEAKREFS.with(|m| {
        m.borrow().get(&target.id()).and_then(|(t, w)| {
            let t = t.upgrade()?;
            if !ObjRef::ptr_eq(&t, &target) {
                return None;
            }
            w.upgrade()
        })
    });
    if let Some(w) = existing {
        return Ok(Value::Object(w));
    }
    let class: ClassRef = vm.find_class(b"WeakReference").expect("WeakReference");
    let obj = ObjRef::new(class);
    obj.set_native(WeakRefData {
        target: target.downgrade(),
    });
    WEAKREFS.with(|m| {
        m.borrow_mut().insert(target.id(), (target.downgrade(), obj.downgrade()));
    });
    Ok(Value::Object(obj))
}

fn wr_get(_vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    let t = o.native::<WeakRefData>().and_then(|d| d.target.upgrade());
    Ok(t.map(Value::Object).unwrap_or(Value::Null))
}

fn wr_construct(vm: &mut Vm, _a: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Direct instantiation of WeakReference is not allowed, use WeakReference::create instead"))
}

static WR_CREATE: NativeFunction = NativeFunction { sig: "public static function create(object $object): WeakReference {}", handler: wr_create };
static WR_GET: NativeFunction = NativeFunction { sig: "public function get(): ?object {}", handler: wr_get };
static WR_CONSTRUCT: NativeFunction = NativeFunction { sig: "public function __construct() {}", handler: wr_construct };

/// Entries of a WeakMap.
#[derive(Default)]
pub struct WeakMapData {
    entries: Vec<(WeakObjRef, u32, Value)>,
}

impl WeakMapData {
    fn purge(&mut self) {
        self.entries.retain(|(w, id, _)| w.upgrade().is_some_and(|o| o.id() == *id));
    }

    fn find(&self, o: &ObjRef) -> Option<usize> {
        self.entries
            .iter()
            .position(|(w, _, _)| w.upgrade().is_some_and(|x| ObjRef::ptr_eq(&x, o)))
    }
}

fn map_of(o: &ObjRef) -> std::cell::RefMut<'_, WeakMapData> {
    if o.native::<RefCell<WeakMapData>>().is_none() {
        o.set_native(RefCell::new(WeakMapData::default()));
    }
    std::cell::RefMut::map(o.native_mut::<RefCell<WeakMapData>>().expect("weakmap"), |c| c.get_mut())
}

fn key_obj(vm: &mut Vm, k: Option<&Value>) -> PResult<ObjRef> {
    match k.map(Value::unref) {
        Some(Value::Object(o)) => Ok(o),
        None => Err(vm.throw_error("Error", "Cannot append to WeakMap")),
        Some(_) => Err(vm.throw_type_error("WeakMap key must be an object")),
    }
}

fn wm_read(vm: &mut Vm, o: &ObjRef, k: Option<&Value>, mode: DimFetch) -> PResult<Value> {
    let key = key_obj(vm, k)?;
    let mut m = map_of(o);
    m.purge();
    match m.find(&key) {
        Some(i) => {
            if mode == DimFetch::Write || mode == DimFetch::ReadWrite {
                let r = match &m.entries[i].2 {
                    Value::Ref(r) => r.clone(),
                    v => {
                        let r = crate::value::PhpRef::new(v.clone());
                        m.entries[i].2 = Value::Ref(r.clone());
                        r
                    }
                };
                return Ok(Value::Ref(r));
            }
            Ok(m.entries[i].2.unref())
        }
        None => {
            if mode == DimFetch::IsSet {
                return Ok(Value::Null);
            }
            if mode == DimFetch::Write {
                let r = crate::value::PhpRef::new(Value::Null);
                m.entries.push((key.downgrade(), key.id(), Value::Ref(r.clone())));
                return Ok(Value::Ref(r));
            }
            drop(m);
            Err(vm.throw_error("Error", format!("Object {}#{} not contained in WeakMap", key.class().name, key.id())))
        }
    }
}

fn wm_write(vm: &mut Vm, o: &ObjRef, k: Option<&Value>, v: Value) -> PResult<()> {
    let key = key_obj(vm, k)?;
    let mut m = map_of(o);
    m.purge();
    match m.find(&key) {
        Some(i) => {
            let old = std::mem::replace(&mut m.entries[i].2, v);
            drop(m);
            drop(old);
        }
        None => m.entries.push((key.downgrade(), key.id(), v)),
    }
    Ok(())
}

fn wm_has(vm: &mut Vm, o: &ObjRef, k: &Value, empty: bool) -> PResult<bool> {
    let key = key_obj(vm, Some(k))?;
    let v = {
        let mut m = map_of(o);
        m.purge();
        m.find(&key).map(|i| m.entries[i].2.unref())
    };
    match v {
        None => Ok(false),
        Some(v) => {
            if empty {
                super::eval::to_bool(vm, &v)
            } else {
                Ok(!v.is_null())
            }
        }
    }
}

fn wm_unset(vm: &mut Vm, o: &ObjRef, k: &Value) -> PResult<()> {
    let key = key_obj(vm, Some(k))?;
    let removed = {
        let mut m = map_of(o);
        m.purge();
        m.find(&key).map(|i| m.entries.remove(i))
    };
    drop(removed);
    Ok(())
}

fn wm_count(_vm: &mut Vm, o: &ObjRef) -> PResult<Option<i64>> {
    let mut m = map_of(o);
    m.purge();
    Ok(Some(m.entries.len() as i64))
}

fn wm_offset_get(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    let k = a.get(0).clone();
    wm_read(vm, &o, Some(&k), DimFetch::Read)
}

fn wm_offset_set(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    let k = a.get(0).clone();
    wm_write(vm, &o, Some(&k), a.get(1).clone())?;
    Ok(Value::Null)
}

fn wm_offset_exists(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    let k = a.get(0).clone();
    Ok(Value::Bool(wm_has(vm, &o, &k, false)?))
}

fn wm_offset_unset(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    let k = a.get(0).clone();
    wm_unset(vm, &o, &k)?;
    Ok(Value::Null)
}

fn wm_count_m(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    Ok(Value::Int(wm_count(vm, &o)?.unwrap_or(0)))
}

fn wm_get_iterator(vm: &mut Vm, a: &mut Args) -> PResult<Value> {
    let o = a.this().cloned().expect("this");
    // Materialise as a generator-like array iterator: [key objects, values].
    let pairs: Vec<(ObjRef, Value)> = {
        let mut m = map_of(&o);
        m.purge();
        m.entries.iter().filter_map(|(w, _, v)| w.upgrade().map(|k| (k, v.unref()))).collect()
    };
    let class = vm.find_class(b"InternalIterator").or_else(|| vm.find_class(b"ArrayIterator"));
    let _ = class;
    let mut arr = PhpArray::new();
    for (k, v) in pairs {
        let mut pair = PhpArray::new();
        let _ = pair.append(Value::Object(k));
        let _ = pair.append(v);
        let _ = arr.append(Value::array(pair));
    }
    Ok(Value::array(arr))
}

fn wm_debug_info(vm: &mut Vm, o: &ObjRef, _p: crate::class::PropPurpose) -> PResult<Option<PhpArray>> {
    let _ = vm;
    let mut m = map_of(o);
    m.purge();
    let mut out = PhpArray::new();
    for (w, _, v) in &m.entries {
        if let Some(k) = w.upgrade() {
            let mut e = PhpArray::new();
            e.insert_str("key", Value::Object(k));
            e.insert_str("value", v.unref());
            let _ = out.append(Value::array(e));
        }
    }
    Ok(Some(out))
}

static WM_GET: NativeFunction = NativeFunction { sig: "public function offsetGet($object): mixed {}", handler: wm_offset_get };
static WM_SET: NativeFunction = NativeFunction { sig: "public function offsetSet($object, mixed $value): void {}", handler: wm_offset_set };
static WM_EXISTS: NativeFunction = NativeFunction { sig: "public function offsetExists($object): bool {}", handler: wm_offset_exists };
static WM_UNSET: NativeFunction = NativeFunction { sig: "public function offsetUnset($object): void {}", handler: wm_offset_unset };
static WM_COUNT: NativeFunction = NativeFunction { sig: "public function count(): int {}", handler: wm_count_m };
static WM_ITER: NativeFunction = NativeFunction { sig: "public function getIterator(): Iterator {}", handler: wm_get_iterator };

/// Iteration over a WeakMap yields object keys.
struct WeakMapIter {
    pairs: Vec<(ObjRef, Value)>,
    pos: usize,
}

impl crate::class::InternalIterator for WeakMapIter {
    fn rewind(&mut self, _vm: &mut Vm) -> PResult<()> {
        self.pos = 0;
        Ok(())
    }
    fn valid(&mut self, _vm: &mut Vm) -> PResult<bool> {
        Ok(self.pos < self.pairs.len())
    }
    fn current(&mut self, _vm: &mut Vm) -> PResult<Value> {
        Ok(self.pairs[self.pos].1.clone())
    }
    fn key(&mut self, _vm: &mut Vm) -> PResult<Value> {
        Ok(Value::Object(self.pairs[self.pos].0.clone()))
    }
    fn next(&mut self, _vm: &mut Vm) -> PResult<()> {
        self.pos += 1;
        Ok(())
    }
}

fn wm_iterator(_vm: &mut Vm, o: &ObjRef, _by_ref: bool) -> PResult<Rc<dyn std::any::Any>> {
    let pairs: Vec<(ObjRef, Value)> = {
        let mut m = map_of(o);
        m.purge();
        m.entries.iter().filter_map(|(w, _, v)| w.upgrade().map(|k| (k, v.unref()))).collect()
    };
    let it: Box<dyn crate::class::InternalIterator> = Box::new(WeakMapIter { pairs, pos: 0 });
    Ok(Rc::new(RefCell::new(it)))
}

pub fn register(r: &mut Registry<'_>) {
    let mut wr = ClassEntry::new("WeakReference", class_flags::FINAL | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES);
    wr.add_method(Function::native_method(&WR_CONSTRUCT, "WeakReference", member_flags::PUBLIC));
    wr.add_method(Function::native_method(&WR_CREATE, "WeakReference", member_flags::PUBLIC | member_flags::STATIC));
    wr.add_method(Function::native_method(&WR_GET, "WeakReference", member_flags::PUBLIC));
    r.class(wr);
    let mut wm = ClassEntry::new("WeakMap", class_flags::FINAL | class_flags::NOT_SERIALIZABLE | class_flags::NO_DYNAMIC_PROPERTIES);
    for i in ["ArrayAccess", "Countable", "IteratorAggregate"] {
        if let Some(c) = r.find_class(i) {
            for sub in c.interfaces.clone() {
                if !wm.interfaces.iter().any(|x| x.lc_name == sub.lc_name) {
                    wm.interfaces.push(sub);
                }
            }
            wm.interfaces.push(c);
        }
    }
    for nf in [&WM_GET, &WM_SET, &WM_EXISTS, &WM_UNSET, &WM_COUNT, &WM_ITER] {
        wm.add_method(Function::native_method(nf, "WeakMap", member_flags::PUBLIC));
    }
    wm.handlers.read_dimension = Some(wm_read);
    wm.handlers.write_dimension = Some(wm_write);
    wm.handlers.has_dimension = Some(wm_has);
    wm.handlers.unset_dimension = Some(wm_unset);
    wm.handlers.count_elements = Some(wm_count);
    wm.handlers.get_debug_info = Some(wm_debug_info);
    wm.handlers.get_iterator = Some(wm_iterator);
    r.class(wm);
}
