//! The hash table behind an `ArrayObject` / `ArrayIterator`
//! (`spl_array_get_hash_table` and the `zend_hash_*` calls made on it).
//!
//! php-src stores either an array, another SPL array object
//! (`SPL_ARRAY_USE_OTHER`), a plain object whose property table is used, or
//! nothing (`SPL_ARRAY_IS_SELF`: the object's own property table). [`resolve`]
//! follows that chain and returns an [`Ht`]:
//!
//! * [`Ht::Array`] — the `PhpArray` kept in the native state of the object at
//!   the end of the chain (copy-on-write through `Rc::make_mut`).
//! * [`Ht::Props`] — the property table of an object, built on
//!   [`php_runtime::object::PhpObject::props`] (declared slots, keyed by their
//!   mangled names like `zend_std_get_properties`) followed by
//!   [`php_runtime::object::PhpObject::dyn_props`]. Declared slots behave like
//!   php-src's `IS_INDIRECT` buckets: an unset slot stays in the table as
//!   [`Found::Unset`].
//! * [`Ht::Empty`] — the sentinel used when no table can be reached (a
//!   `USE_OTHER` cycle); reads find nothing and writes are dropped.
//!
//! Positions: [`Ht::Array`] uses the stable [`Pos`] of `PhpArray`. In a
//! property table, declared slot `n` has position `n` and the dynamic
//! property at `PhpArray` position `p` has position `slots + p`, so positions
//! survive insertions and deletions exactly like php-src's bucket indexes
//! (`HashTableIterator`).

use std::rc::Rc;

use php_runtime::class::{PropertyInfo, member_flags};
use php_runtime::value::KeyRef;
use php_runtime::value::convert::mangle_property_name;
use php_runtime::{ArrayKey, ObjRef, PhpArray, PhpArrayRef, PhpRef, PhpStr, Pos, Value};

use super::{IS_SELF, SplArray, USE_OTHER};

/// The resolved backing table (see the module docs).
#[derive(Clone)]
pub(crate) enum Ht {
    /// The array held by this object's [`SplArray`] state.
    Array(ObjRef),
    /// The property table of this object.
    Props(ObjRef),
    /// No reachable table (`intern->sentinel_array`).
    Empty,
}

/// An offset after `get_hash_key()`: integer keys of property tables are
/// already converted to strings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HKey {
    Int(i64),
    Str(PhpStr),
}

impl HKey {
    pub(crate) fn key_ref(&self) -> KeyRef<'_> {
        match self {
            HKey::Int(i) => KeyRef::Int(*i),
            HKey::Str(s) => KeyRef::Str(s.as_bytes()),
        }
    }

    pub(crate) fn to_array_key(&self) -> ArrayKey {
        match self {
            HKey::Int(i) => ArrayKey::Int(*i),
            HKey::Str(s) => ArrayKey::Str(s.clone()),
        }
    }

    /// `Undefined array key "x"` / `Undefined array key 5`.
    pub(crate) fn undefined_message(&self) -> PhpStr {
        let mut msg = b"Undefined array key ".to_vec();
        match self {
            HKey::Int(i) => msg.extend_from_slice(i.to_string().as_bytes()),
            HKey::Str(s) => {
                msg.push(b'"');
                msg.extend_from_slice(s.as_bytes());
                msg.push(b'"');
            }
        }
        PhpStr::from_vec(msg)
    }
}

/// Result of a lookup (`zend_hash_find` + `IS_INDIRECT` handling).
pub(crate) enum Found {
    Missing,
    /// A declared property slot that is unset (`IS_INDIRECT` to `IS_UNDEF`).
    Unset,
    Value(Value),
}

/// Result of [`Ht::delete`].
pub(crate) enum Deleted {
    Nothing,
    Element,
    /// A declared property slot was unset (php-src then moves the iterator
    /// of the object forward).
    DeclaredSlot,
}

/// Borrows the state of an SPL array object.
pub(crate) fn state(obj: &ObjRef) -> Option<std::cell::Ref<'_, SplArray>> {
    obj.native::<SplArray>()
}

/// `spl_array_get_hash_table`: follows `USE_OTHER` links to the table.
pub(crate) fn resolve(obj: &ObjRef) -> Ht {
    let mut cur = obj.clone();
    let mut seen: Vec<ObjRef> = Vec::new();
    loop {
        let next = {
            let Some(st) = state(&cur) else {
                return Ht::Empty;
            };
            if st.ar_flags & IS_SELF != 0 {
                drop(st);
                return Ht::Props(cur);
            }
            match &st.array {
                Value::Object(o) if st.ar_flags & USE_OTHER != 0 => o.clone(),
                Value::Object(o) => return Ht::Props(o.clone()),
                Value::Array(_) => {
                    drop(st);
                    return Ht::Array(cur);
                }
                _ => return Ht::Empty,
            }
        };
        if ObjRef::ptr_eq(&next, obj) || seen.iter().any(|o| ObjRef::ptr_eq(o, &next)) {
            return Ht::Empty;
        }
        seen.push(std::mem::replace(&mut cur, next));
    }
}

/// `spl_array_is_object`: whether the table is a property table.
pub(crate) fn is_object(obj: &ObjRef) -> bool {
    matches!(resolve(obj), Ht::Props(_))
}

/// A declared property of a property table.
pub(crate) struct DeclProp {
    pub slot: usize,
    /// Mangled name (the property-table key).
    pub key: PhpStr,
    pub info: PropertyInfo,
}

/// Declared instance properties with storage, in slot order
/// (`rebuild_object_properties`); virtual (hooked) properties have none.
pub(crate) fn declared_props(obj: &ObjRef) -> Vec<DeclProp> {
    let mut out: Vec<DeclProp> = Vec::new();
    let mut ce = Some(obj.class().clone());
    while let Some(c) = ce {
        for info in c.properties.values() {
            if info.is_static()
                || info.flags & member_flags::VIRTUAL != 0
                || out.iter().any(|d| d.slot == info.slot)
            {
                continue;
            }
            out.push(DeclProp {
                slot: info.slot,
                key: mangle_property_name(&info.name, info.flags, &info.declaring_class),
                info: info.clone(),
            });
        }
        ce = c.parent.clone();
    }
    out.sort_by_key(|d| d.slot);
    out
}

/// Where a key lives in a property table.
enum PropLoc {
    Slot(usize),
    Dyn,
    Missing,
}

fn prop_loc(obj: &ObjRef, decl: &[DeclProp], key: &[u8]) -> PropLoc {
    if let Some(d) = decl.iter().find(|d| d.key.as_bytes() == key) {
        return PropLoc::Slot(d.slot);
    }
    let dyn_props = obj.obj().dyn_props.borrow();
    match dyn_props.as_ref() {
        Some(t) if t.get_by(KeyRef::Str(key)).is_some() => PropLoc::Dyn,
        _ => PropLoc::Missing,
    }
}

/// Property-table keys are strings; integer keys only reach this through
/// appends (`zend_hash_next_index_insert`), which use their decimal form.
fn prop_key(key: &HKey) -> PhpStr {
    match key {
        HKey::Str(s) => s.clone(),
        HKey::Int(i) => PhpStr::from(i.to_string()),
    }
}

/// `zval_add_ref`-style copy used when duplicating tables: a reference only
/// held by the table is copied as a plain value.
fn dup_value(v: &Value) -> Value {
    match v {
        Value::Ref(r) if r.refcount() == 1 => r.get(),
        v => v.clone(),
    }
}

impl Ht {
    fn with_array<R>(owner: &ObjRef, f: impl FnOnce(&PhpArray) -> R) -> Option<R> {
        let st = state(owner)?;
        match &st.array {
            Value::Array(a) => Some(f(a)),
            _ => None,
        }
    }

    fn with_array_mut<R>(owner: &ObjRef, f: impl FnOnce(&mut PhpArray) -> R) -> Option<R> {
        let mut st = owner.native_mut::<SplArray>()?;
        match &mut st.array {
            Value::Array(a) => Some(f(Rc::make_mut(a))),
            _ => None,
        }
    }

    /// Whether this is the property table of `obj` itself (`IS_SELF`).
    pub(crate) fn is_props_of(&self, obj: &ObjRef) -> bool {
        matches!(self, Ht::Props(o) if ObjRef::ptr_eq(o, obj))
    }

    /// `zend_hash_find` / `zend_hash_index_find`.
    pub(crate) fn find(&self, key: &HKey) -> Found {
        match self {
            Ht::Array(owner) => Self::with_array(owner, |a| match a.get_by(key.key_ref()) {
                Some(v) => Found::Value(v.clone()),
                None => Found::Missing,
            })
            .unwrap_or(Found::Missing),
            Ht::Props(obj) => {
                let k = prop_key(key);
                let decl = declared_props(obj);
                match prop_loc(obj, &decl, &k) {
                    PropLoc::Slot(slot) => match obj.obj().props.borrow().get(slot) {
                        Some(Value::Undef) | None => Found::Unset,
                        Some(v) => Found::Value(v.clone()),
                    },
                    PropLoc::Dyn => obj
                        .obj()
                        .dyn_props
                        .borrow()
                        .as_ref()
                        .and_then(|t| t.get_by(KeyRef::Str(&k)).cloned())
                        .map_or(Found::Missing, Found::Value),
                    PropLoc::Missing => Found::Missing,
                }
            }
            Ht::Empty => Found::Missing,
        }
    }

    /// `zend_hash_update_ind` / `zend_hash_index_update`: replaces the value
    /// (a reference in the slot is replaced, not written through).
    pub(crate) fn update(&self, key: &HKey, value: Value) {
        match self {
            Ht::Array(owner) => {
                Self::with_array_mut(owner, |a| {
                    a.insert(key.to_array_key(), value);
                });
            }
            Ht::Props(obj) => {
                let k = prop_key(key);
                let decl = declared_props(obj);
                if let PropLoc::Slot(slot) = prop_loc(obj, &decl, &k) {
                    let old = {
                        let mut props = obj.obj().props.borrow_mut();
                        props.get_mut(slot).map(|s| std::mem::replace(s, value))
                    };
                    drop(old);
                    return;
                }
                let old = {
                    let mut d = obj.obj().dyn_props.borrow_mut();
                    d.get_or_insert_with(PhpArray::new)
                        .insert(ArrayKey::Str(k), value)
                };
                drop(old);
            }
            Ht::Empty => {}
        }
    }

    /// `zend_hash_next_index_insert`; fails silently when the next index is
    /// taken.
    pub(crate) fn append(&self, value: Value) {
        match self {
            Ht::Array(owner) => {
                Self::with_array_mut(owner, |a| {
                    let _ = a.append(value);
                });
            }
            Ht::Props(obj) => {
                let mut d = obj.obj().dyn_props.borrow_mut();
                let t = d.get_or_insert_with(PhpArray::new);
                let next = t
                    .keys()
                    .filter_map(|k| match k {
                        ArrayKey::Int(i) => Some(i),
                        ArrayKey::Str(s) => ArrayKey::parse_numeric(&s),
                    })
                    .filter(|i| *i >= 0)
                    .max()
                    .map_or(0, |i| i.saturating_add(1));
                t.insert(ArrayKey::Str(PhpStr::from(next.to_string())), value);
            }
            Ht::Empty => {}
        }
    }

    /// `zend_hash_del` / `zend_hash_index_del`; a declared property slot is
    /// set to `IS_UNDEF` instead.
    pub(crate) fn delete(&self, key: &HKey) -> Deleted {
        match self {
            Ht::Array(owner) => {
                let old = Self::with_array_mut(owner, |a| a.remove_by(key.key_ref())).flatten();
                if old.is_some() {
                    Deleted::Element
                } else {
                    Deleted::Nothing
                }
            }
            Ht::Props(obj) => {
                let k = prop_key(key);
                let decl = declared_props(obj);
                match prop_loc(obj, &decl, &k) {
                    PropLoc::Slot(slot) => {
                        let old = {
                            let mut props = obj.obj().props.borrow_mut();
                            match props.get_mut(slot) {
                                Some(v) if !v.is_undef() => Some(std::mem::take(v)),
                                _ => None,
                            }
                        };
                        if old.is_some() {
                            Deleted::DeclaredSlot
                        } else {
                            Deleted::Nothing
                        }
                    }
                    PropLoc::Dyn => {
                        let old = obj
                            .obj()
                            .dyn_props
                            .borrow_mut()
                            .as_mut()
                            .and_then(|t| t.remove_by(KeyRef::Str(&k)));
                        drop(old);
                        Deleted::Element
                    }
                    PropLoc::Missing => Deleted::Nothing,
                }
            }
            Ht::Empty => Deleted::Nothing,
        }
    }

    /// The element as a reference (`ZVAL_NEW_REF` on the bucket), for write
    /// fetches. A missing element is created as `null` when `create` is set;
    /// an unset declared slot is always re-initialised to `null`.
    pub(crate) fn element_ref(&self, key: &HKey, create: bool) -> Option<PhpRef> {
        fn make_ref(slot: &mut Value) -> PhpRef {
            match slot {
                Value::Ref(r) => r.clone(),
                other => {
                    let v = std::mem::take(other);
                    let v = if v.is_undef() { Value::Null } else { v };
                    let r = PhpRef::new(v);
                    *other = Value::Ref(r.clone());
                    r
                }
            }
        }
        match self {
            Ht::Array(owner) => Self::with_array_mut(owner, |a| {
                let k = key.to_array_key();
                if a.get(&k).is_none() {
                    if !create {
                        return None;
                    }
                    a.insert(k.clone(), Value::Null);
                }
                a.get_mut(&k).map(make_ref)
            })
            .flatten(),
            Ht::Props(obj) => {
                let k = prop_key(key);
                let decl = declared_props(obj);
                match prop_loc(obj, &decl, &k) {
                    PropLoc::Slot(slot) => {
                        let mut props = obj.obj().props.borrow_mut();
                        props.get_mut(slot).map(make_ref)
                    }
                    PropLoc::Dyn => {
                        let mut d = obj.obj().dyn_props.borrow_mut();
                        d.as_mut()
                            .and_then(|t| t.get_mut_by(KeyRef::Str(&k)))
                            .map(make_ref)
                    }
                    PropLoc::Missing => {
                        if !create {
                            return None;
                        }
                        let mut d = obj.obj().dyn_props.borrow_mut();
                        let t = d.get_or_insert_with(PhpArray::new);
                        let ak = ArrayKey::Str(k);
                        t.insert(ak.clone(), Value::Null);
                        t.get_mut(&ak).map(make_ref)
                    }
                }
            }
            Ht::Empty => None,
        }
    }

    /// `zend_hash_num_elements` (unset declared slots included).
    pub(crate) fn num_elements(&self) -> usize {
        match self {
            Ht::Array(owner) => Self::with_array(owner, PhpArray::len).unwrap_or(0),
            Ht::Props(obj) => {
                declared_props(obj).len()
                    + obj
                        .obj()
                        .dyn_props
                        .borrow()
                        .as_ref()
                        .map_or(0, PhpArray::len)
            }
            Ht::Empty => 0,
        }
    }

    /// `spl_array_object_count_elements_helper`: property tables count
    /// public declared properties that are set, and every dynamic one.
    pub(crate) fn visible_count(&self) -> i64 {
        match self {
            Ht::Props(obj) => {
                let decl = declared_props(obj);
                let props = obj.obj().props.borrow();
                let declared = decl
                    .iter()
                    .filter(|d| {
                        props.get(d.slot).is_some_and(|v| !v.is_undef())
                            && d.key.as_bytes().first() != Some(&0)
                    })
                    .count();
                drop(props);
                let dynamic = obj
                    .obj()
                    .dyn_props
                    .borrow()
                    .as_ref()
                    .map_or(0, PhpArray::len);
                (declared + dynamic) as i64
            }
            ht => ht.num_elements() as i64,
        }
    }

    /// `zend_array_dup` of the table (unset declared slots are skipped).
    pub(crate) fn to_array(&self) -> PhpArray {
        match self {
            Ht::Array(owner) => Self::with_array(owner, |a| a.clone()).unwrap_or_default(),
            Ht::Props(obj) => {
                let mut out = PhpArray::new();
                for (k, v) in self.prop_entries(obj) {
                    out.insert(k, dup_value(&v));
                }
                out
            }
            Ht::Empty => PhpArray::new(),
        }
    }

    /// The table as an array value without duplication where possible
    /// (comparison, sorting, debug output).
    pub(crate) fn shared_array(&self) -> PhpArrayRef {
        match self {
            Ht::Array(owner) => state(owner)
                .and_then(|st| match &st.array {
                    Value::Array(a) => Some(a.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            Ht::Props(obj) => {
                let mut out = PhpArray::new();
                for (k, v) in self.prop_entries(obj) {
                    out.insert(k, v);
                }
                Rc::new(out)
            }
            Ht::Empty => Rc::new(PhpArray::new()),
        }
    }

    /// Set entries of a property table in order (mangled keys).
    fn prop_entries(&self, obj: &ObjRef) -> Vec<(ArrayKey, Value)> {
        let decl = declared_props(obj);
        let mut out = Vec::new();
        {
            let props = obj.obj().props.borrow();
            for d in &decl {
                if let Some(v) = props.get(d.slot)
                    && !v.is_undef()
                {
                    out.push((ArrayKey::Str(d.key.clone()), v.clone()));
                }
            }
        }
        if let Some(t) = obj.obj().dyn_props.borrow().as_ref() {
            for (k, v) in t.iter() {
                out.push((ArrayKey::Str(k.to_php_str()), v.clone()));
            }
        }
        out
    }

    /// Replaces the table with `new` (after sorting through a reference,
    /// `spl_array_method`). Property tables keep their declared slots in
    /// place; the dynamic properties take the new order.
    pub(crate) fn replace(&self, new: PhpArrayRef) {
        match self {
            Ht::Array(owner) => {
                let old = owner
                    .native_mut::<SplArray>()
                    .map(|mut st| std::mem::replace(&mut st.array, Value::Array(new)));
                drop(old);
            }
            Ht::Props(obj) => {
                let decl = declared_props(obj);
                let mut dynamic = PhpArray::new();
                let mut garbage = Vec::new();
                {
                    let mut props = obj.obj().props.borrow_mut();
                    for (k, v) in new.iter() {
                        let key = k.to_php_str();
                        match decl.iter().find(|d| d.key == key) {
                            Some(d) => {
                                if let Some(slot) = props.get_mut(d.slot) {
                                    garbage.push(std::mem::replace(slot, v.clone()));
                                }
                            }
                            None => {
                                dynamic.insert(ArrayKey::Str(key), v.clone());
                            }
                        }
                    }
                }
                let old = {
                    let mut d = obj.obj().dyn_props.borrow_mut();
                    if d.is_some() || !dynamic.is_empty() {
                        d.replace(dynamic)
                    } else {
                        None
                    }
                };
                drop(old);
                drop(garbage);
            }
            Ht::Empty => {}
        }
    }

    // ----- positions (`HashPosition`) ------------------------------------

    /// Number of declared slots of a property table (dynamic positions
    /// start there).
    fn slot_base(obj: &ObjRef) -> Pos {
        obj.obj().props.borrow().len() as Pos
    }

    /// `_zend_hash_get_valid_pos`: the first element at or after `pos`.
    pub(crate) fn valid_pos(&self, pos: Pos) -> Option<Pos> {
        match self {
            Ht::Array(owner) => Self::with_array(owner, |a| a.valid_pos(pos)).flatten(),
            Ht::Props(obj) => {
                let base = Self::slot_base(obj);
                if pos < base {
                    let decl = declared_props(obj);
                    if let Some(d) = decl.iter().find(|d| d.slot as Pos >= pos) {
                        return Some(d.slot as Pos);
                    }
                }
                let d = obj.obj().dyn_props.borrow();
                let t = d.as_ref()?;
                t.valid_pos(pos.saturating_sub(base)).map(|p| p + base)
            }
            Ht::Empty => None,
        }
    }

    /// The past-the-end position (`ht->nNumUsed`).
    pub(crate) fn end_pos(&self) -> Pos {
        match self {
            Ht::Array(owner) => Self::with_array(owner, PhpArray::end_pos).unwrap_or(0),
            Ht::Props(obj) => {
                let base = Self::slot_base(obj);
                base + obj
                    .obj()
                    .dyn_props
                    .borrow()
                    .as_ref()
                    .map_or(0, PhpArray::end_pos)
            }
            Ht::Empty => 0,
        }
    }

    /// `zend_hash_internal_pointer_reset_ex`.
    pub(crate) fn first_pos(&self) -> Pos {
        self.valid_pos(Pos::MIN).unwrap_or_else(|| self.end_pos())
    }

    /// Position stored after visiting `pos`; elements added later at the
    /// end stay visible.
    fn pos_after(&self, pos: Pos) -> Pos {
        match self {
            Ht::Array(owner) => Self::with_array(owner, |a| a.pos_after(pos)).unwrap_or(pos + 1),
            Ht::Props(obj) => {
                let base = Self::slot_base(obj);
                if pos < base {
                    return pos + 1;
                }
                obj.obj()
                    .dyn_props
                    .borrow()
                    .as_ref()
                    .map_or(pos + 1, |t| t.pos_after(pos - base) + base)
            }
            Ht::Empty => pos + 1,
        }
    }

    /// `zend_hash_move_forward_ex`: `None` (FAILURE) if `pos` is past the end.
    pub(crate) fn move_forward(&self, pos: Pos) -> Option<Pos> {
        let cur = self.valid_pos(pos)?;
        let next = self.pos_after(cur);
        Some(self.valid_pos(next).unwrap_or(next))
    }

    /// Key and value of the element at the valid position `pos`.
    pub(crate) fn entry_at(&self, pos: Pos) -> Option<(ArrayKey, Found)> {
        match self {
            Ht::Array(owner) => Self::with_array(owner, |a| {
                a.entry_at(pos).map(|(k, v)| (k, Found::Value(v.clone())))
            })
            .flatten(),
            Ht::Props(obj) => {
                let base = Self::slot_base(obj);
                if pos < base {
                    let decl = declared_props(obj);
                    let d = decl.iter().find(|d| d.slot as Pos == pos)?;
                    let v = obj.obj().props.borrow().get(d.slot).cloned()?;
                    let found = if v.is_undef() {
                        Found::Unset
                    } else {
                        Found::Value(v)
                    };
                    return Some((ArrayKey::Str(d.key.clone()), found));
                }
                let d = obj.obj().dyn_props.borrow();
                let (k, v) = d.as_ref()?.entry_at(pos - base)?;
                Some((ArrayKey::Str(k.to_php_str()), Found::Value(v.clone())))
            }
            Ht::Empty => None,
        }
    }

    /// The element at the valid position `pos` as a reference
    /// (`ZEND_FE_FETCH_RW`).
    pub(crate) fn ref_at(&self, pos: Pos) -> Option<PhpRef> {
        let (key, _) = self.entry_at(pos)?;
        let key = match key {
            ArrayKey::Int(i) => HKey::Int(i),
            ArrayKey::Str(s) => HKey::Str(s),
        };
        self.element_ref(&key, false)
    }
}
