//! The storage shared by `SplObjectStorage` and `MultipleIterator`
//! (`spl_SplObjectStorage` in spl_observer.c) and the operations on it that
//! both classes use: `getHash()` dispatch, attach/detach/contains, the
//! object handlers (dimensions, compare, clone) and the debug table.
//!
//! The `HashTable storage` of php-src is a [`Table`]: insertion-ordered
//! elements keyed by object handle (or by the string a user `getHash()`
//! returned), addressed by stable logical positions like
//! [`php_runtime::PhpArray`]. A position whose element was deleted resolves
//! to the next live element (`_zend_hash_get_valid_pos`), so the iteration
//! cursor (`intern->pos`) and the internal pointer used by `removeAll()`
//! behave like php-src's `HashPosition`s.

use std::collections::HashMap;

use php_runtime::class::{ClassRef, DimFetch};
use php_runtime::{ArrayKey, Flow, ObjRef, PResult, PhpArray, PhpStr, Pos, Value, Vm};

use super::super::engine::{self, is_overridden, set_private_debug_info_property};
use super::{SPL_OBJECT_STORAGE, globals};

/// `SOS_OVERRIDDEN_READ_DIMENSION`: `getHash`, `offsetExists` or `offsetGet`
/// may be user code, so `isset($s[$o])` / `$s[$o]` call the methods.
pub const SOS_OVERRIDDEN_READ_DIMENSION: i64 = 1;
/// `SOS_OVERRIDDEN_WRITE_DIMENSION`: `getHash` or `offsetSet`.
pub const SOS_OVERRIDDEN_WRITE_DIMENSION: i64 = 2;
/// `SOS_OVERRIDDEN_UNSET_DIMENSION`: `getHash` or `offsetUnset`.
pub const SOS_OVERRIDDEN_UNSET_DIMENSION: i64 = 4;

// ---------------------------------------------------------------------------
// The ordered table
// ---------------------------------------------------------------------------

/// Key of an element: the object handle, or the string returned by an
/// overridden `getHash()` (an exact string key, never normalised).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Handle(u32),
    Hash(PhpStr),
}

/// `spl_SplObjectStorageElement`.
#[derive(Clone)]
pub struct Element {
    pub obj: ObjRef,
    pub inf: Value,
}

struct Slot {
    pos: Pos,
    key: Key,
    /// `None` marks a deleted bucket.
    elem: Option<Element>,
}

/// Insertion-ordered elements with stable positions (see the module docs).
#[derive(Default)]
pub struct Table {
    /// Ordered by `pos`.
    slots: Vec<Slot>,
    map: HashMap<Key, usize>,
    next_pos: Pos,
    len: usize,
    dead: usize,
}

impl Table {
    /// `zend_hash_num_elements`.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, key: &Key) -> Option<&Element> {
        let idx = *self.map.get(key)?;
        self.slots[idx].elem.as_ref()
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.map.contains_key(key)
    }

    /// Adds `obj => inf` under `key`, or replaces the info of the element
    /// already stored under `key` (its object is kept). Returns the
    /// replaced info, to be dropped by the caller.
    pub fn attach(&mut self, key: Key, obj: &ObjRef, inf: Value) -> Option<Value> {
        if let Some(&idx) = self.map.get(&key) {
            let elem = self.slots[idx].elem.as_mut().expect("indexed slot is live");
            return Some(std::mem::replace(&mut elem.inf, inf));
        }
        let idx = self.slots.len();
        self.slots.push(Slot {
            pos: self.next_pos,
            key: key.clone(),
            elem: Some(Element {
                obj: obj.clone(),
                inf,
            }),
        });
        self.next_pos += 1;
        self.len += 1;
        self.map.insert(key, idx);
        None
    }

    /// `zend_hash_del`: the element's position becomes a hole.
    pub fn remove(&mut self, key: &Key) -> Option<Element> {
        let idx = self.map.remove(key)?;
        let elem = self.slots[idx].elem.take();
        self.len -= 1;
        self.dead += 1;
        if self.dead > 8 && self.dead * 2 > self.slots.len() {
            self.compact();
        }
        elem
    }

    fn compact(&mut self) {
        self.slots.retain(|s| s.elem.is_some());
        self.dead = 0;
        self.map.clear();
        for (i, s) in self.slots.iter().enumerate() {
            self.map.insert(s.key.clone(), i);
        }
    }

    /// Index of the first slot with `pos >= pos`.
    fn lower_bound(&self, pos: Pos) -> usize {
        self.slots.partition_point(|s| s.pos < pos)
    }

    /// The past-the-end position: elements attached later are found from it.
    pub fn end_pos(&self) -> Pos {
        self.next_pos
    }

    /// `_zend_hash_get_valid_pos`: the first live position `>= pos`.
    pub fn valid_pos(&self, pos: Pos) -> Option<Pos> {
        let start = self.lower_bound(pos);
        self.slots[start..]
            .iter()
            .find(|s| s.elem.is_some())
            .map(|s| s.pos)
    }

    /// The first live position, or the end (`zend_hash_internal_pointer_reset_ex`).
    pub fn first_pos(&self) -> Pos {
        self.valid_pos(Pos::MIN).unwrap_or(self.next_pos)
    }

    /// The last live position strictly before `pos`.
    fn pos_before(&self, pos: Pos) -> Option<Pos> {
        let end = self.lower_bound(pos);
        self.slots[..end]
            .iter()
            .rev()
            .find(|s| s.elem.is_some())
            .map(|s| s.pos)
    }

    /// The live element exactly at `pos`.
    pub fn at(&self, pos: Pos) -> Option<&Element> {
        let idx = self.lower_bound(pos);
        self.slots
            .get(idx)
            .filter(|s| s.pos == pos)
            .and_then(|s| s.elem.as_ref())
    }

    fn at_mut(&mut self, pos: Pos) -> Option<&mut Element> {
        let idx = self.lower_bound(pos);
        self.slots
            .get_mut(idx)
            .filter(|s| s.pos == pos)
            .and_then(|s| s.elem.as_mut())
    }

    /// The element `zend_hash_get_current_data_ex(&pos)` sees.
    pub fn current(&self, pos: Pos) -> Option<&Element> {
        self.valid_pos(pos).and_then(|p| self.at(p))
    }

    pub fn current_mut(&mut self, pos: Pos) -> Option<&mut Element> {
        let p = self.valid_pos(pos)?;
        self.at_mut(p)
    }

    /// `zend_hash_move_forward_ex`: `false` (and `pos` unchanged) when it
    /// was already past the end.
    pub fn move_forward(&self, pos: &mut Pos) -> bool {
        match self.valid_pos(*pos) {
            Some(p) => {
                *pos = self.valid_pos(p + 1).unwrap_or(self.next_pos);
                true
            }
            None => false,
        }
    }

    /// `zend_hash_move_backwards_ex`.
    pub fn move_backward(&self, pos: &mut Pos) -> bool {
        if self.valid_pos(*pos).is_none() {
            return false;
        }
        *pos = self.pos_before(*pos).unwrap_or(self.next_pos);
        true
    }

    /// The first live element at or after `pos` with its position
    /// (`SPL_SAFE_HASH_FOREACH_PTR`, which tolerates modifications).
    pub fn next_from(&self, pos: Pos) -> Option<(Pos, Element)> {
        let p = self.valid_pos(pos)?;
        self.at(p).map(|e| (p, e.clone()))
    }

    /// Live elements in order (`ZEND_HASH_FOREACH_PTR`).
    pub fn elements(&self) -> impl Iterator<Item = &Element> + '_ {
        self.slots.iter().filter_map(|s| s.elem.as_ref())
    }

    /// Live `(key, element)` pairs in order.
    pub fn entries(&self) -> impl Iterator<Item = (&Key, &Element)> + '_ {
        self.slots
            .iter()
            .filter_map(|s| s.elem.as_ref().map(|e| (&s.key, e)))
    }
}

// ---------------------------------------------------------------------------
// Per-object state
// ---------------------------------------------------------------------------

/// `spl_SplObjectStorage`: the native state of `SplObjectStorage` and
/// `MultipleIterator` objects (and their subclasses).
pub struct ObjectStorage {
    pub table: Table,
    /// `intern->index`: the iteration key.
    pub index: i64,
    /// `intern->pos`: the iteration cursor.
    pub pos: Pos,
    /// `SOS_OVERRIDDEN_*` bits for `SplObjectStorage`; the user-settable
    /// `MIT_*` flags for `MultipleIterator` (the dimension handlers test the
    /// same bits for both, exactly like php-src).
    pub flags: i64,
    /// `intern->fptr_get_hash != NULL`: a subclass overrides `getHash()`.
    pub get_hash_overridden: bool,
    /// `nInternalPointer` of the table (only `removeAll()` uses it).
    pub internal_pointer: Pos,
    /// `GC_PROTECT_RECURSION` of the table while it is being compared.
    pub compare_guard: bool,
}

impl ObjectStorage {
    /// `spl_object_storage_new_ex`: subclasses of `SplObjectStorage` get the
    /// overridden-method bits. Every user subclass has ArrayAccess function
    /// pointers (`zend_implement_arrayaccess` runs for inherited
    /// interfaces), so php-src always sets all three bits for them.
    pub fn new(class: &ClassRef) -> ObjectStorage {
        let mut st = ObjectStorage {
            table: Table::default(),
            index: 0,
            pos: 0,
            flags: 0,
            get_hash_overridden: false,
            internal_pointer: 0,
            compare_guard: false,
        };
        let mut parent = Some(class.clone());
        while let Some(c) = parent {
            if c.lc_name.as_bytes() == b"splobjectstorage" {
                if class.lc_name.as_bytes() != b"splobjectstorage" {
                    st.get_hash_overridden = is_overridden(class, b"gethash", SPL_OBJECT_STORAGE);
                    st.flags |= SOS_OVERRIDDEN_READ_DIMENSION
                        | SOS_OVERRIDDEN_WRITE_DIMENSION
                        | SOS_OVERRIDDEN_UNSET_DIMENSION;
                }
                break;
            }
            parent = c.parent.clone();
        }
        st
    }

    /// `zend_hash_internal_pointer_reset_ex(&storage, &pos)`.
    pub fn reset_pos(&mut self) {
        self.pos = self.table.first_pos();
    }

    /// `zend_hash_move_forward_ex(&storage, &pos)`.
    pub fn move_forward(&mut self) -> bool {
        let mut pos = self.pos;
        let r = self.table.move_forward(&mut pos);
        self.pos = pos;
        r
    }

    /// `zend_hash_move_backwards_ex(&storage, &pos)`.
    pub fn move_backward(&mut self) -> bool {
        let mut pos = self.pos;
        let r = self.table.move_backward(&mut pos);
        self.pos = pos;
        r
    }

    /// The element at the cursor.
    pub fn current(&self) -> Option<&Element> {
        self.table.current(self.pos)
    }
}

/// Runs `f` on the storage state of `obj`, creating it if the object was
/// allocated without the class's `create_object` handler. `f` must not run
/// PHP code; values it removes should be returned and dropped by the caller.
pub fn with_state<R>(obj: &ObjRef, f: impl FnOnce(&mut ObjectStorage) -> R) -> R {
    if obj.native::<ObjectStorage>().is_none() {
        obj.set_native(ObjectStorage::new(obj.class()));
    }
    let mut st = obj
        .native_mut::<ObjectStorage>()
        .expect("SplObjectStorage state");
    f(&mut st)
}

/// `zend_hash_num_elements(&intern->storage)`.
pub fn len(obj: &ObjRef) -> usize {
    with_state(obj, |s| s.table.len())
}

/// `create_object` of `SplObjectStorage` and `MultipleIterator`
/// (`spl_SplObjectStorage_new`).
pub fn create_object(_vm: &mut Vm, class: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(class.clone());
    obj.set_native(ObjectStorage::new(class));
    Ok(obj)
}

// ---------------------------------------------------------------------------
// Core operations
// ---------------------------------------------------------------------------

/// `spl_object_storage_is_mutating_within_get_hash_call`.
pub fn check_not_in_get_hash(vm: &mut Vm) -> PResult<()> {
    if globals(vm).get_hash_depth > 0 {
        return Err(vm.throw_error(
            "Error",
            "Modification of SplObjectStorage during getHash() is prohibited",
        ));
    }
    Ok(())
}

/// `spl_object_storage_get_hash`: the handle, or the result of the user's
/// `getHash()` (counted in the per-VM `getHash()` depth).
pub fn get_hash(vm: &mut Vm, this: &ObjRef, obj: &ObjRef) -> PResult<Key> {
    if !with_state(this, |s| s.get_hash_overridden) {
        return Ok(Key::Handle(obj.id()));
    }
    globals(vm).get_hash_depth += 1;
    let r = vm.call_method(this, b"getHash", vec![Value::Object(obj.clone())]);
    globals(vm).get_hash_depth -= 1;
    match r?.into_unref() {
        Value::String(s) => Ok(Key::Hash(s)),
        other => {
            let msg = format!(
                "{}::getHash(): Return value must be of type string, {} returned",
                this.class().name,
                other.value_name()
            );
            Err(vm.throw_type_error(msg))
        }
    }
}

/// `spl_object_storage_attach`.
pub fn attach(vm: &mut Vm, this: &ObjRef, obj: &ObjRef, inf: Option<Value>) -> PResult<()> {
    check_not_in_get_hash(vm)?;
    let key = get_hash(vm, this, obj)?;
    let inf = inf.map_or(Value::Null, Value::into_unref);
    // The replaced info is released last, once the table is consistent.
    let old = with_state(this, |s| s.table.attach(key, obj, inf));
    drop(old);
    Ok(())
}

/// `spl_object_storage_attach_handle`: the fast path of the dimension
/// handlers (no `getHash()` override).
fn attach_handle(this: &ObjRef, obj: &ObjRef, inf: Value) {
    let old = with_state(this, |s| {
        s.table.attach(Key::Handle(obj.id()), obj, inf.into_unref())
    });
    drop(old);
}

/// `spl_object_storage_detach`: `Ok(false)` when the object was not stored.
pub fn detach(vm: &mut Vm, this: &ObjRef, obj: &ObjRef) -> PResult<bool> {
    check_not_in_get_hash(vm)?;
    let key = get_hash(vm, this, obj)?;
    let removed = with_state(this, |s| s.table.remove(&key));
    Ok(removed.is_some())
}

/// `spl_object_storage_contains`.
pub fn contains(vm: &mut Vm, this: &ObjRef, obj: &ObjRef) -> PResult<bool> {
    let key = get_hash(vm, this, obj)?;
    Ok(with_state(this, |s| s.table.contains(&key)))
}

/// `spl_object_storage_addall`: attaches every element of `other`
/// (elements removed or added meanwhile are skipped or visited).
pub fn addall(vm: &mut Vm, this: &ObjRef, other: &ObjRef) -> PResult<()> {
    let mut cursor = Pos::MIN;
    while let Some((pos, elem)) = with_state(other, |s| s.table.next_from(cursor)) {
        attach(vm, this, &elem.obj, Some(elem.inf))?;
        cursor = pos + 1;
    }
    with_state(this, |s| s.index = 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// Debug info
// ---------------------------------------------------------------------------

/// `spl_object_storage_debug_info`: the property table plus the private
/// `SplObjectStorage::$storage` list of `["obj" => ..., "inf" => ...]`
/// (also for `MultipleIterator`).
pub fn debug_info(obj: &ObjRef) -> PhpArray {
    let mut table = engine::std_properties(obj);
    let elements: Vec<Element> = with_state(obj, |s| s.table.elements().cloned().collect());
    let mut storage = PhpArray::with_capacity(elements.len());
    for e in elements {
        let mut pair = PhpArray::with_capacity(2);
        pair.insert_str("obj", Value::Object(e.obj));
        pair.insert_str("inf", e.inf);
        let _ = storage.append(Value::array(pair));
    }
    set_private_debug_info_property(
        &mut table,
        SPL_OBJECT_STORAGE,
        "storage",
        Value::array(storage),
    );
    table
}

// ---------------------------------------------------------------------------
// Object handlers
// ---------------------------------------------------------------------------

fn flags(obj: &ObjRef) -> i64 {
    with_state(obj, |s| s.flags)
}

/// The offset object when a dimension handler may take its fast path (a
/// handle lookup): the offset is an object and `bit` is clear.
fn fast_path_key(obj: &ObjRef, offset: Option<&Value>, bit: i64) -> Option<ObjRef> {
    match offset {
        Some(Value::Object(key)) if flags(obj) & bit == 0 => Some(key.clone()),
        _ => None,
    }
}

/// The info stored for `key` under its handle.
fn stored_info(obj: &ObjRef, key: &ObjRef) -> Option<Value> {
    with_state(obj, |s| {
        s.table.get(&Key::Handle(key.id())).map(|e| e.inf.clone())
    })
}

/// `zend_bad_array_access`.
fn bad_array_access(vm: &mut Vm, obj: &ObjRef) -> Flow {
    let msg = format!("Cannot use object of type {} as array", obj.class().name);
    vm.throw_error("Error", msg)
}

fn implements_array_access(obj: &ObjRef) -> bool {
    obj.class().instance_of_name(b"ArrayAccess")
}

/// `zend_std_read_dimension`: `offsetExists()` (for `isset`-style fetches)
/// and `offsetGet()` of the object's class.
fn std_read_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<Value>,
    fetch: DimFetch,
) -> PResult<Value> {
    if !implements_array_access(obj) {
        return Err(bad_array_access(vm, obj));
    }
    let offset = offset.unwrap_or(Value::Null);
    if fetch == DimFetch::IsSet {
        let exists = vm.call_method(obj, b"offsetExists", vec![offset.clone()])?;
        if !vm.to_bool(&exists)? {
            return Ok(Value::Null);
        }
    }
    let v = vm.call_method(obj, b"offsetGet", vec![offset])?;
    if v.is_undef() {
        let msg = format!(
            "Undefined offset for object of type {} used as array",
            obj.class().name
        );
        return Err(vm.throw_error("Error", msg));
    }
    Ok(v)
}

/// `zend_std_write_dimension`: `offsetSet()`.
fn std_write_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<Value>,
    value: Value,
) -> PResult<()> {
    if !implements_array_access(obj) {
        return Err(bad_array_access(vm, obj));
    }
    let offset = offset.unwrap_or(Value::Null);
    vm.call_method(obj, b"offsetSet", vec![offset, value])?;
    Ok(())
}

/// `zend_std_has_dimension`: `offsetExists()`, then `offsetGet()` for
/// `empty()`.
fn std_has_dimension(vm: &mut Vm, obj: &ObjRef, offset: Value, check_empty: bool) -> PResult<bool> {
    if !implements_array_access(obj) {
        return Err(bad_array_access(vm, obj));
    }
    let exists = vm.call_method(obj, b"offsetExists", vec![offset.clone()])?;
    let mut result = vm.to_bool(&exists)?;
    if check_empty && result {
        let v = vm.call_method(obj, b"offsetGet", vec![offset])?;
        result = vm.to_bool(&v)?;
    }
    Ok(result)
}

/// `zend_std_unset_dimension`: `offsetUnset()`.
fn std_unset_dimension(vm: &mut Vm, obj: &ObjRef, offset: Value) -> PResult<()> {
    if !implements_array_access(obj) {
        return Err(bad_array_access(vm, obj));
    }
    vm.call_method(obj, b"offsetUnset", vec![offset])?;
    Ok(())
}

/// `read_dimension` (`spl_object_storage_read_dimension`): `$s[$obj]` and
/// `$s[$obj] ?? ...` (`DimFetch::IsSet`, which yields `null` for a missing
/// object). The value is never a reference, even for write fetches (the
/// interpreter reports "Indirect modification of overloaded element").
pub fn read_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    fetch: DimFetch,
) -> PResult<Value> {
    let offset = offset.map(Value::unref);
    if let Some(key) = fast_path_key(obj, offset.as_ref(), SOS_OVERRIDDEN_READ_DIMENSION) {
        return match stored_info(obj, &key) {
            Some(inf) => Ok(inf.into_unref()),
            None if fetch == DimFetch::IsSet => Ok(Value::Null),
            None => Err(vm.throw_error("UnexpectedValueException", "Object not found")),
        };
    }
    std_read_dimension(vm, obj, offset, fetch)
}

/// `write_dimension` of `SplObjectStorage` (`$s[$obj] = $info`).
pub fn write_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    value: Value,
) -> PResult<()> {
    check_not_in_get_hash(vm)?;
    let offset = offset.map(Value::unref);
    if let Some(key) = fast_path_key(obj, offset.as_ref(), SOS_OVERRIDDEN_WRITE_DIMENSION) {
        attach_handle(obj, &key, value);
        return Ok(());
    }
    std_write_dimension(vm, obj, offset, value)
}

/// `write_dimension` of `MultipleIterator`
/// (`spl_multiple_iterator_write_dimension`): only `Iterator`s are stored.
pub fn multiple_iterator_write_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: Option<&Value>,
    value: Value,
) -> PResult<()> {
    check_not_in_get_hash(vm)?;
    let offset = offset.map(Value::unref);
    if let Some(key) = fast_path_key(obj, offset.as_ref(), SOS_OVERRIDDEN_WRITE_DIMENSION) {
        if !key.class().instance_of_name(b"Iterator") {
            return Err(vm.throw_type_error(
                "Can only attach objects that implement the Iterator interface",
            ));
        }
        attach_handle(obj, &key, value);
        return Ok(());
    }
    std_write_dimension(vm, obj, offset, value)
}

/// `has_dimension` (`isset($s[$obj])`, and `empty()` with `check_empty`,
/// which returns whether the info is truthy — php-src's convention).
pub fn has_dimension(
    vm: &mut Vm,
    obj: &ObjRef,
    offset: &Value,
    check_empty: bool,
) -> PResult<bool> {
    let offset = offset.unref();
    if let Some(key) = fast_path_key(obj, Some(&offset), SOS_OVERRIDDEN_READ_DIMENSION) {
        return match stored_info(obj, &key) {
            None => Ok(false),
            Some(inf) if check_empty => vm.to_bool(&inf),
            // offsetExists() is contains(): true even for a null info.
            Some(_) => Ok(true),
        };
    }
    std_has_dimension(vm, obj, offset, check_empty)
}

/// `unset_dimension` (`unset($s[$obj])`).
pub fn unset_dimension(vm: &mut Vm, obj: &ObjRef, offset: &Value) -> PResult<()> {
    check_not_in_get_hash(vm)?;
    let offset = offset.unref();
    if let Some(key) = fast_path_key(obj, Some(&offset), SOS_OVERRIDDEN_UNSET_DIMENSION) {
        let removed = with_state(obj, |s| s.table.remove(&Key::Handle(key.id())));
        drop(removed);
        return Ok(());
    }
    std_unset_dimension(vm, obj, offset)
}

/// Whether `obj` uses the storage object handlers (`SplObjectStorage`,
/// `MultipleIterator` and their subclasses).
fn has_storage_handlers(obj: &ObjRef) -> bool {
    let ce = obj.class();
    ce.instance_of_name(b"SplObjectStorage") || ce.instance_of_name(b"MultipleIterator")
}

/// `compare` (`spl_object_storage_compare_objects`): only two plain
/// `SplObjectStorage`s are comparable; they are equal when they hold the
/// same objects with equal infos (in any order). Returns -1/0/1, `1` also
/// meaning "uncomparable".
pub fn compare(vm: &mut Vm, a: &Value, b: &Value) -> PResult<i32> {
    let (Value::Object(o1), Value::Object(o2)) = (a.unref(), b.unref()) else {
        return vm.std_compare_objects(a, b);
    };
    if !has_storage_handlers(&o1) || !has_storage_handlers(&o2) {
        return vm.std_compare_objects(a, b);
    }
    let is_plain = |o: &ObjRef| o.class().lc_name.as_bytes() == b"splobjectstorage";
    if !is_plain(&o1) || !is_plain(&o2) {
        return Ok(1);
    }
    hash_compare(vm, &o1, &o2)
}

/// `zend_hash_compare(&s1->storage, &s2->storage, compare_info, 0)` with its
/// recursion protection.
fn hash_compare(vm: &mut Vm, o1: &ObjRef, o2: &ObjRef) -> PResult<i32> {
    if ObjRef::ptr_eq(o1, o2) {
        return Ok(0);
    }
    if with_state(o1, |s| s.compare_guard) {
        return Err(vm.throw_error("Error", "Nesting level too deep - recursive dependency?"));
    }
    with_state(o1, |s| s.compare_guard = true);
    let r = hash_compare_impl(vm, o1, o2);
    with_state(o1, |s| s.compare_guard = false);
    r
}

fn hash_compare_impl(vm: &mut Vm, o1: &ObjRef, o2: &ObjRef) -> PResult<i32> {
    let (n1, n2) = (len(o1), len(o2));
    if n1 != n2 {
        return Ok(if n1 > n2 { 1 } else { -1 });
    }
    let entries: Vec<(Key, Value)> = with_state(o1, |s| {
        s.table
            .entries()
            .map(|(k, e)| (k.clone(), e.inf.clone()))
            .collect()
    });
    for (key, inf1) in entries {
        let Some(inf2) = with_state(o2, |s| s.table.get(&key).map(|e| e.inf.clone())) else {
            return Ok(1);
        };
        let c = vm.compare(&inf1, &inf2)?;
        if c != 0 {
            return Ok(c);
        }
    }
    Ok(0)
}

/// `clone_obj` (`spl_object_storage_clone`): a new object of the same class
/// (its storage state computed afresh, so a `MultipleIterator`'s flags are
/// not copied), filled with `addAll()` semantics (a user `getHash()` runs),
/// then the properties are copied. The interpreter calls `__clone()`
/// afterwards.
pub fn clone_obj(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let class = old.class().clone();
    let new = vm.instantiate(&class)?;
    addall(vm, &new, old)?;
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    Ok(new)
}

/// `zend_throw_exception(class, msg)` while `pending` is being thrown: the
/// new exception gets `pending` as its previous exception
/// (`zend_exception_set_previous`). Exit/fatal unwinding is not replaced.
pub fn throw_chained(vm: &mut Vm, pending: Flow, class: &str, msg: &str) -> Flow {
    match pending {
        Flow::Exception(previous) => {
            let ex = vm.create_exception(class, msg, 0);
            ex.set_prop(b"previous", Value::Object(previous));
            Flow::Exception(ex)
        }
        other => other,
    }
}

/// `object_properties_load()` (Zend/zend_API.c): writes `props` (a property
/// table with mangled keys) into the declared slots of `obj`; other keys
/// become dynamic properties (with the dynamic-property deprecation).
pub fn object_properties_load(vm: &mut Vm, obj: &ObjRef, props: &PhpArray) -> PResult<()> {
    use php_runtime::class::{class_flags, member_flags};
    let class = obj.class().clone();
    for (key, prop) in props.iter() {
        let key = match key {
            ArrayKey::Str(k) => k,
            ArrayKey::Int(h) => {
                dynamic_property_deprecation(vm, obj, &h.to_string())?;
                set_dynamic(obj, PhpStr::from(h.to_string()), prop.clone());
                continue;
            }
        };
        match lookup_property(&class, &key) {
            Lookup::Found(info) => {
                if info.flags & member_flags::VIRTUAL != 0 {
                    let msg = format!(
                        "Cannot unserialize value for virtual property {}::${}",
                        class.name, info.name
                    );
                    return Err(vm.throw_error("Error", msg));
                }
                let mut val = prop.clone();
                if let Some(ty) = &info.ty {
                    if val.is_ref() {
                        if info.is_readonly() {
                            let msg = format!(
                                "Cannot indirectly modify readonly property {}::${}",
                                info.declaring_class, info.name
                            );
                            return Err(vm.throw_error("Error", msg));
                        }
                    } else {
                        let initialized = obj
                            .obj()
                            .props
                            .borrow()
                            .get(info.slot)
                            .is_some_and(|v| !v.is_undef());
                        if info.is_readonly() && initialized {
                            let msg = format!(
                                "Cannot modify readonly property {}::${}",
                                info.declaring_class, info.name
                            );
                            return Err(vm.throw_error("Error", msg));
                        }
                    }
                    // Strict verification: only int -> float widening.
                    let inner = val.unref();
                    if !ty.matches_exact(&inner) {
                        match (&inner, ty.mask & php_runtime::types::mask::FLOAT != 0) {
                            (Value::Int(i), true) if !val.is_ref() => val = Value::Float(*i as f64),
                            _ => {
                                let msg = format!(
                                    "Cannot assign {} to property {}::${} of type {}",
                                    inner.value_name(),
                                    info.declaring_class,
                                    info.name,
                                    ty.to_type_string()
                                );
                                return Err(vm.throw_type_error(msg));
                            }
                        }
                    }
                }
                let mut slots = obj.obj().props.borrow_mut();
                if let Some(slot) = slots.get_mut(info.slot) {
                    *slot = val;
                }
            }
            lookup => {
                let name = match &lookup {
                    Lookup::Dynamic(n) => n.to_string(),
                    _ => String::new(),
                };
                // ZEND_ACC_NO_DYNAMIC_PROPERTIES (readonly classes).
                if class.has_flag(class_flags::READONLY) {
                    let msg = format!("Cannot create dynamic property {}::${name}", class.name);
                    return Err(vm.throw_error("Error", msg));
                }
                dynamic_property_deprecation(vm, obj, &name)?;
                set_dynamic(obj, key, prop.clone());
            }
        }
    }
    Ok(())
}

/// Result of `zend_get_property_info(ce, name, silent)` for a table key.
enum Lookup {
    Found(php_runtime::class::PropertyInfo),
    /// Not a declared (accessible) property: dynamic, with this name.
    Dynamic(PhpStr),
    /// `ZEND_WRONG_PROPERTY_INFO`: a private property of the class itself
    /// seen from another scope (or a malformed mangled name).
    Wrong,
}

/// `object_properties_load`'s lookup: the scope is the class named in a
/// mangled key, else the executing `SplObjectStorage` method.
fn lookup_property(class: &ClassRef, key: &PhpStr) -> Lookup {
    use php_runtime::class::member_flags;
    let b = key.as_bytes();
    let (scope, name): (Option<&[u8]>, &[u8]) = if b.first() == Some(&0) {
        // zend_unmangle_property_name_ex
        let Some(end) = b[1..].iter().position(|&c| c == 0) else {
            return Lookup::Wrong;
        };
        let class_part = &b[1..1 + end];
        let name = &b[2 + end..];
        if b.len() < 3 || b[1] == 0 || name.is_empty() {
            return Lookup::Wrong;
        }
        (Some(class_part), name)
    } else {
        (None, b)
    };
    let Some(info) = class.find_property(name).filter(|i| !i.is_static()) else {
        return Lookup::Dynamic(PhpStr::from_bytes(name));
    };
    if info.flags & member_flags::PRIVATE != 0 {
        let scope = scope
            .filter(|s| *s != b"*")
            .unwrap_or(SPL_OBJECT_STORAGE.as_bytes());
        if !info.declaring_class.eq_ignore_ascii_case(scope) {
            return if info.declaring_class.eq_ignore_ascii_case(&class.name) {
                Lookup::Wrong
            } else {
                Lookup::Dynamic(PhpStr::from_bytes(name))
            };
        }
    }
    Lookup::Found(info.clone())
}

fn dynamic_property_deprecation(vm: &mut Vm, obj: &ObjRef, name: &str) -> PResult<()> {
    use php_runtime::class::class_flags;
    if obj.class().has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
        return Ok(());
    }
    let msg = format!(
        "Creation of dynamic property {}::${name} is deprecated",
        obj.class().name
    );
    vm.engine_deprecated(msg)
}

fn set_dynamic(obj: &ObjRef, key: PhpStr, value: Value) {
    let mut d = obj.obj().dyn_props.borrow_mut();
    d.get_or_insert_with(PhpArray::new)
        .insert(ArrayKey::Str(key), value);
}
