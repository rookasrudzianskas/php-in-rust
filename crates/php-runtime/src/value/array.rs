//! `PhpArray`: PHP's ordered hash map (port of the semantics of `zend_hash.c`).
//!
//! Design notes:
//!
//! * Two storage modes, like php-src: a *packed* vector for list-like arrays
//!   (key == index, holes are `Value::Undef`), and a *hash* mode with an
//!   insertion-ordered bucket vector plus an index table.
//! * **Positions** ([`Pos`]) are stable logical positions, not bucket indexes:
//!   every element gets a position when inserted and keeps it through
//!   deletions of other elements, compaction, packed→hash conversion and
//!   copy-on-write duplication. Positions are never reused, so an iterator
//!   holding a position (foreach by reference, the internal pointer) always
//!   sees elements appended later — exactly what php-src achieves by updating
//!   its registered `HashTableIterator`s. A position that refers to a deleted
//!   element resolves to the next live element (`_zend_hash_get_valid_pos`).
//! * `Clone` implements `zend_array_dup`: references with refcount 1 are
//!   copied as plain values, and a past-the-end internal pointer is reset.

use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use hashbrown::HashTable;

use super::string::{PhpStr, hash_bytes, hash_int};
use super::{PhpRef, Value};

/// A stable iteration position (see module docs).
pub type Pos = i64;

/// An array key: PHP normalises decimal integer strings to `Int`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArrayKey {
    Int(i64),
    Str(PhpStr),
}

/// A borrowed array key used for lookups without allocating.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyRef<'a> {
    Int(i64),
    Str(&'a [u8]),
}

impl ArrayKey {
    /// `ZEND_HANDLE_NUMERIC_STR`: `"8"` → 8, while `"08"`, `"-0"`, `" 8"`,
    /// `"8 "` and integers outside the i64 range stay strings.
    pub fn parse_numeric(b: &[u8]) -> Option<i64> {
        let first = *b.first()?;
        if first > b'9' {
            return None;
        }
        let digits = if first == b'-' { &b[1..] } else { b };
        match digits.first() {
            Some(c) if c.is_ascii_digit() => {}
            _ => return None,
        }
        if (digits[0] == b'0' && b.len() > 1) || digits.len() > 19 {
            return None;
        }
        let mut acc: u64 = 0;
        for &c in digits {
            if !c.is_ascii_digit() {
                return None;
            }
            acc = acc * 10 + (c - b'0') as u64;
        }
        if first == b'-' {
            if acc > i64::MAX as u64 + 1 {
                None
            } else {
                Some((acc as i64).wrapping_neg())
            }
        } else if acc > i64::MAX as u64 {
            None
        } else {
            Some(acc as i64)
        }
    }

    /// Builds a key from string bytes, applying numeric normalisation.
    pub fn from_bytes(b: &[u8]) -> ArrayKey {
        match Self::parse_numeric(b) {
            Some(i) => ArrayKey::Int(i),
            None => ArrayKey::Str(PhpStr::from_bytes(b)),
        }
    }

    /// Builds a key from a string, applying numeric normalisation (no copy for string keys).
    pub fn from_php_str(s: PhpStr) -> ArrayKey {
        match Self::parse_numeric(s.as_bytes()) {
            Some(i) => ArrayKey::Int(i),
            None => ArrayKey::Str(s),
        }
    }

    #[inline]
    pub fn as_key_ref(&self) -> KeyRef<'_> {
        match self {
            ArrayKey::Int(i) => KeyRef::Int(*i),
            ArrayKey::Str(s) => KeyRef::Str(s.as_bytes()),
        }
    }

    #[inline]
    pub fn hash_code(&self) -> u64 {
        match self {
            ArrayKey::Int(i) => hash_int(*i),
            ArrayKey::Str(s) => s.hash_code(),
        }
    }

    /// The key as a PHP value (`int` or `string`).
    pub fn to_value(&self) -> Value {
        match self {
            ArrayKey::Int(i) => Value::Int(*i),
            ArrayKey::Str(s) => Value::String(s.clone()),
        }
    }

    pub fn into_value(self) -> Value {
        match self {
            ArrayKey::Int(i) => Value::Int(i),
            ArrayKey::Str(s) => Value::String(s),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            ArrayKey::Int(i) => Some(*i),
            ArrayKey::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&PhpStr> {
        match self {
            ArrayKey::Str(s) => Some(s),
            ArrayKey::Int(_) => None,
        }
    }

    pub fn is_int(&self) -> bool {
        matches!(self, ArrayKey::Int(_))
    }

    /// The key rendered as a string (`(string)$key`).
    pub fn to_php_str(&self) -> PhpStr {
        match self {
            ArrayKey::Int(i) => PhpStr::from(i.to_string()),
            ArrayKey::Str(s) => s.clone(),
        }
    }
}

impl KeyRef<'_> {
    #[inline]
    pub fn hash_code(&self) -> u64 {
        match self {
            KeyRef::Int(i) => hash_int(*i),
            KeyRef::Str(s) => hash_bytes(s),
        }
    }

    #[inline]
    fn matches(&self, key: &ArrayKey) -> bool {
        match (self, key) {
            (KeyRef::Int(a), ArrayKey::Int(b)) => a == b,
            (KeyRef::Str(a), ArrayKey::Str(b)) => *a == b.as_bytes(),
            _ => false,
        }
    }

    /// Converts to an owned key (no numeric normalisation is applied).
    pub fn to_owned_key(self) -> ArrayKey {
        match self {
            KeyRef::Int(i) => ArrayKey::Int(i),
            KeyRef::Str(s) => ArrayKey::Str(PhpStr::from_bytes(s)),
        }
    }
}

impl Hash for ArrayKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash_code());
    }
}

impl From<i64> for ArrayKey {
    fn from(i: i64) -> Self {
        ArrayKey::Int(i)
    }
}

impl From<i32> for ArrayKey {
    fn from(i: i32) -> Self {
        ArrayKey::Int(i as i64)
    }
}

impl From<usize> for ArrayKey {
    fn from(i: usize) -> Self {
        ArrayKey::Int(i as i64)
    }
}

/// Normalising conversion (`"5"` becomes `Int(5)`).
impl From<&str> for ArrayKey {
    fn from(s: &str) -> Self {
        ArrayKey::from_bytes(s.as_bytes())
    }
}

/// Normalising conversion (`"5"` becomes `Int(5)`).
impl From<PhpStr> for ArrayKey {
    fn from(s: PhpStr) -> Self {
        ArrayKey::from_php_str(s)
    }
}

/// Normalising conversion (`"5"` becomes `Int(5)`).
impl From<String> for ArrayKey {
    fn from(s: String) -> Self {
        ArrayKey::from_php_str(PhpStr::from(s))
    }
}

impl fmt::Display for ArrayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrayKey::Int(i) => write!(f, "{i}"),
            ArrayKey::Str(s) => write!(f, "{s}"),
        }
    }
}

impl fmt::Debug for ArrayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrayKey::Int(i) => write!(f, "{i}"),
            ArrayKey::Str(s) => write!(f, "{s:?}"),
        }
    }
}

/// Result of converting an arbitrary value to an array offset; the
/// `Vm`-aware wrapper emits the corresponding diagnostics.
#[derive(Debug, Clone)]
pub enum KeyCast {
    /// Clean conversion (int, string, bool).
    Key(ArrayKey),
    /// `null` → `""` ("Using null as an array offset is deprecated...").
    Null,
    /// A float with a fractional part ("Implicit conversion from float ... loses precision").
    FloatFractional(i64, f64),
    /// NaN/Inf/out-of-range float ("The float ... is not representable as an int, cast occurred").
    FloatNotRepresentable(i64, f64),
    /// A resource ("Resource ID#n used as offset, casting to integer (n)").
    Resource(i64),
    /// Arrays and objects are illegal offsets (TypeError).
    Illegal,
}

/// Pure part of PHP's offset conversion (`slow_index_convert`).
pub fn cast_key(v: &Value) -> KeyCast {
    match v {
        Value::Int(i) => KeyCast::Key(ArrayKey::Int(*i)),
        Value::String(s) => KeyCast::Key(ArrayKey::from_php_str(s.clone())),
        Value::Bool(b) => KeyCast::Key(ArrayKey::Int(*b as i64)),
        Value::Null | Value::Undef => KeyCast::Null,
        Value::Float(d) => {
            let d = *d;
            let fits = super::numeric::double_fits_long(d);
            let l = super::numeric::dval_to_lval_silent(d);
            if !d.is_finite() || !fits {
                KeyCast::FloatNotRepresentable(l, d)
            } else if (l as f64) != d {
                KeyCast::FloatFractional(l, d)
            } else {
                KeyCast::Key(ArrayKey::Int(l))
            }
        }
        Value::Resource(r) => KeyCast::Resource(r.id()),
        Value::Array(_) | Value::Object(_) => KeyCast::Illegal,
        Value::Ref(r) => cast_key(&r.borrow()),
    }
}

/// Error returned when appending to an array whose next index is occupied
/// ("Cannot add element to the array as the next element is already occupied").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextElementOccupied;

impl fmt::Display for NextElementOccupied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cannot add element to the array as the next element is already occupied")
    }
}

struct Bucket {
    pos: Pos,
    key: ArrayKey,
    /// `Value::Undef` marks a deleted bucket.
    val: Value,
}

struct PackedStore {
    vals: Vec<Value>,
    /// Position of `vals[0]`.
    offset: Pos,
}

struct HashStore {
    entries: Vec<Bucket>,
    table: HashTable<u32>,
    next_pos: Pos,
    dead: usize,
}

enum Store {
    Packed(PackedStore),
    Hash(HashStore),
}

/// PHP array (`zend_array`). Shared via [`super::PhpArrayRef`] (`Rc`).
pub struct PhpArray {
    store: Store,
    len: usize,
    /// `nNextFreeElement`; `i64::MIN` until an integer key is inserted.
    next_free: i64,
    /// Internal pointer (`nInternalPointer`) as a position.
    ptr: Pos,
    /// Highest position handed out to callers; packed slots below it are never reused.
    hwm: Cell<Pos>,
}

impl HashStore {
    fn new(cap: usize) -> Self {
        HashStore {
            entries: Vec::with_capacity(cap),
            table: HashTable::with_capacity(cap),
            next_pos: 0,
            dead: 0,
        }
    }

    #[inline]
    fn find(&self, k: KeyRef<'_>) -> Option<usize> {
        let entries = &self.entries;
        self.table
            .find(k.hash_code(), |&i| k.matches(&entries[i as usize].key))
            .map(|&i| i as usize)
    }

    fn push(&mut self, key: ArrayKey, val: Value) -> usize {
        let idx = self.entries.len();
        let h = key.hash_code();
        self.entries.push(Bucket {
            pos: self.next_pos,
            key,
            val,
        });
        self.next_pos += 1;
        let entries = &self.entries;
        self.table
            .insert_unique(h, idx as u32, |&i| entries[i as usize].key.hash_code());
        idx
    }

    fn rebuild_table(&mut self) {
        self.table.clear();
        let entries = &self.entries;
        for (i, b) in entries.iter().enumerate() {
            if !b.val.is_undef() {
                self.table.insert_unique(b.key.hash_code(), i as u32, |&j| {
                    entries[j as usize].key.hash_code()
                });
            }
        }
    }

    fn remove_at(&mut self, idx: usize) -> Value {
        let h = self.entries[idx].key.hash_code();
        if let Ok(e) = self.table.find_entry(h, |&i| i as usize == idx) {
            e.remove();
        }
        let b = &mut self.entries[idx];
        let val = std::mem::take(&mut b.val);
        b.key = ArrayKey::Int(0);
        self.dead += 1;
        while let Some(last) = self.entries.last() {
            if last.val.is_undef() {
                self.entries.pop();
                self.dead -= 1;
            } else {
                break;
            }
        }
        if self.dead > 8 && self.dead * 2 > self.entries.len() {
            self.compact();
        }
        val
    }

    fn compact(&mut self) {
        self.entries.retain(|b| !b.val.is_undef());
        self.dead = 0;
        self.rebuild_table();
    }

    /// Index of the first bucket with `pos >= pos`.
    fn lower_bound(&self, pos: Pos) -> usize {
        let Some(first) = self.entries.first() else {
            return 0;
        };
        if pos <= first.pos {
            return 0;
        }
        let guess = (pos - first.pos) as u64;
        if guess < self.entries.len() as u64 && self.entries[guess as usize].pos == pos {
            return guess as usize;
        }
        self.entries.partition_point(|b| b.pos < pos)
    }

    fn end_pos(&self) -> Pos {
        self.next_pos
    }
}

impl PackedStore {
    #[inline]
    fn lower_bound(&self, pos: Pos) -> usize {
        if pos <= self.offset {
            0
        } else {
            let d = (pos - self.offset) as u64;
            d.min(self.vals.len() as u64) as usize
        }
    }

    fn end_pos(&self) -> Pos {
        self.offset + self.vals.len() as Pos
    }
}

#[inline]
fn sanitize(v: Value) -> Value {
    if v.is_undef() { Value::Null } else { v }
}

impl Default for PhpArray {
    fn default() -> Self {
        Self::new()
    }
}

impl PhpArray {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(cap: usize) -> Self {
        PhpArray {
            store: Store::Packed(PackedStore {
                vals: Vec::with_capacity(cap),
                offset: 0,
            }),
            len: 0,
            next_free: i64::MIN,
            ptr: 0,
            hwm: Cell::new(Pos::MIN),
        }
    }

    /// A list `[v0, v1, ...]`.
    pub fn from_values(vals: Vec<Value>) -> Self {
        let vals: Vec<Value> = vals.into_iter().map(sanitize).collect();
        let len = vals.len();
        PhpArray {
            next_free: if len == 0 { i64::MIN } else { len as i64 },
            store: Store::Packed(PackedStore { vals, offset: 0 }),
            len,
            ptr: 0,
            hwm: Cell::new(Pos::MIN),
        }
    }

    /// Builds an array by inserting the pairs in order (later duplicates overwrite).
    pub fn from_pairs<K: Into<ArrayKey>, V: Into<Value>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut a = PhpArray::new();
        for (k, v) in pairs {
            a.insert(k.into(), v.into());
        }
        a
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the array uses the packed (list) representation.
    pub fn is_packed(&self) -> bool {
        matches!(self.store, Store::Packed(_))
    }

    /// Raw `nNextFreeElement` (`i64::MIN` if no integer key was ever inserted).
    pub fn next_free_raw(&self) -> i64 {
        self.next_free
    }

    /// The key the next append (`$a[] = ...`) would use, or `None` if that key is occupied.
    pub fn next_index(&self) -> Option<i64> {
        let k = if self.next_free == i64::MIN {
            0
        } else {
            self.next_free
        };
        if self.contains_int(k) { None } else { Some(k) }
    }

    /// Overrides `nNextFreeElement` (used by e.g. `array_pop`, `array_splice`).
    pub fn set_next_free(&mut self, v: i64) {
        self.next_free = v;
    }

    #[inline]
    fn bump_next_free(&mut self, i: i64) {
        if i >= self.next_free {
            self.next_free = if i < i64::MAX { i + 1 } else { i64::MAX };
        }
    }

    #[inline]
    fn note_pos(&self, p: Pos) -> Pos {
        if p > self.hwm.get() {
            self.hwm.set(p);
        }
        p
    }

    fn find(&self, k: KeyRef<'_>) -> Option<usize> {
        match &self.store {
            Store::Packed(p) => match k {
                KeyRef::Int(i) if i >= 0 && (i as u64) < p.vals.len() as u64 => {
                    let idx = i as usize;
                    (!p.vals[idx].is_undef()).then_some(idx)
                }
                _ => None,
            },
            Store::Hash(h) => h.find(k),
        }
    }

    fn convert_to_hash(&mut self) {
        let Store::Packed(p) = &mut self.store else {
            return;
        };
        let end = p.end_pos();
        let offset = p.offset;
        let vals = std::mem::take(&mut p.vals);
        let mut h = HashStore::new(self.len + 1);
        for (i, v) in vals.into_iter().enumerate() {
            if !v.is_undef() {
                h.entries.push(Bucket {
                    pos: offset + i as Pos,
                    key: ArrayKey::Int(i as i64),
                    val: v,
                });
            }
        }
        h.next_pos = end;
        h.rebuild_table();
        self.store = Store::Hash(h);
    }

    // ----- lookup -------------------------------------------------------

    pub fn get(&self, key: &ArrayKey) -> Option<&Value> {
        self.get_by(key.as_key_ref())
    }

    pub fn get_by(&self, k: KeyRef<'_>) -> Option<&Value> {
        let idx = self.find(k)?;
        Some(match &self.store {
            Store::Packed(p) => &p.vals[idx],
            Store::Hash(h) => &h.entries[idx].val,
        })
    }

    pub fn get_int(&self, i: i64) -> Option<&Value> {
        self.get_by(KeyRef::Int(i))
    }

    /// Lookup by exact string key (no numeric normalisation).
    pub fn get_str(&self, s: &[u8]) -> Option<&Value> {
        self.get_by(KeyRef::Str(s))
    }

    /// Lookup with PHP symbol-table normalisation (`zend_symtable_find`).
    pub fn get_sym(&self, s: &[u8]) -> Option<&Value> {
        match ArrayKey::parse_numeric(s) {
            Some(i) => self.get_int(i),
            None => self.get_str(s),
        }
    }

    pub fn contains_key(&self, key: &ArrayKey) -> bool {
        self.find(key.as_key_ref()).is_some()
    }

    /// Alias of [`PhpArray::contains_key`].
    pub fn contains(&self, key: &ArrayKey) -> bool {
        self.contains_key(key)
    }

    pub fn contains_int(&self, i: i64) -> bool {
        self.find(KeyRef::Int(i)).is_some()
    }

    pub fn get_mut(&mut self, key: &ArrayKey) -> Option<&mut Value> {
        self.get_mut_by(key.as_key_ref())
    }

    pub fn get_mut_by(&mut self, k: KeyRef<'_>) -> Option<&mut Value> {
        let idx = self.find(k)?;
        Some(match &mut self.store {
            Store::Packed(p) => &mut p.vals[idx],
            Store::Hash(h) => &mut h.entries[idx].val,
        })
    }

    // ----- insertion ----------------------------------------------------

    /// `zend_hash_update`: inserts or overwrites (an existing key keeps its position).
    /// Returns the previous value.
    pub fn insert(&mut self, key: ArrayKey, val: Value) -> Option<Value> {
        let val = sanitize(val);
        match key {
            ArrayKey::Int(i) => self.insert_int(i, val),
            ArrayKey::Str(s) => {
                self.convert_to_hash();
                let Store::Hash(h) = &mut self.store else {
                    unreachable!()
                };
                if let Some(idx) = h.find(KeyRef::Str(s.as_bytes())) {
                    return Some(std::mem::replace(&mut h.entries[idx].val, val));
                }
                h.push(ArrayKey::Str(s), val);
                self.len += 1;
                None
            }
        }
    }

    /// Convenience: insert with a string key (normalised).
    pub fn insert_str(&mut self, key: &str, val: impl Into<Value>) -> Option<Value> {
        self.insert(ArrayKey::from(key), val.into())
    }

    fn insert_int(&mut self, i: i64, val: Value) -> Option<Value> {
        if let Store::Packed(p) = &mut self.store {
            if i >= 0 {
                let len = p.vals.len();
                let idx = i as u64;
                if idx < len as u64 {
                    let idx = idx as usize;
                    if !p.vals[idx].is_undef() {
                        return Some(std::mem::replace(&mut p.vals[idx], val));
                    }
                    let tail_free = p.vals[idx..].iter().all(Value::is_undef);
                    if tail_free && p.offset + idx as Pos >= self.hwm.get() {
                        p.vals.truncate(idx);
                        p.vals.push(val);
                        self.len += 1;
                        self.bump_next_free(i);
                        return None;
                    }
                } else if idx == len as u64 {
                    p.vals.push(val);
                    self.len += 1;
                    self.bump_next_free(i);
                    return None;
                } else if idx < (2 * (self.len as u64 + 1)).max(8) {
                    p.vals.resize(idx as usize, Value::Undef);
                    p.vals.push(val);
                    self.len += 1;
                    self.bump_next_free(i);
                    return None;
                }
            }
            self.convert_to_hash();
        }
        let Store::Hash(h) = &mut self.store else {
            unreachable!()
        };
        if let Some(idx) = h.find(KeyRef::Int(i)) {
            return Some(std::mem::replace(&mut h.entries[idx].val, val));
        }
        h.push(ArrayKey::Int(i), val);
        self.len += 1;
        self.bump_next_free(i);
        None
    }

    /// `$a[] = $v` (`zend_hash_next_index_insert`). Returns the new key.
    pub fn append(&mut self, val: impl Into<Value>) -> Result<i64, NextElementOccupied> {
        let k = self.next_index().ok_or(NextElementOccupied)?;
        self.insert_int(k, sanitize(val.into()));
        Ok(k)
    }

    /// Appends and returns a mutable reference to the new slot (`$a[][...] = ...`).
    pub fn append_mut(&mut self) -> Result<&mut Value, NextElementOccupied> {
        let k = self.append(Value::Null)?;
        Ok(self.get_mut_by(KeyRef::Int(k)).expect("just inserted"))
    }

    /// `zend_hash_lookup`: returns the slot for `key`, inserting `null` if absent.
    pub fn entry_mut(&mut self, key: ArrayKey) -> &mut Value {
        if self.find(key.as_key_ref()).is_none() {
            self.insert(key.clone(), Value::Null);
        }
        self.get_mut(&key).expect("present")
    }

    // ----- removal ------------------------------------------------------

    /// `zend_hash_del`. The removed element's position becomes a hole.
    pub fn remove(&mut self, key: &ArrayKey) -> Option<Value> {
        self.remove_by(key.as_key_ref())
    }

    pub fn remove_by(&mut self, k: KeyRef<'_>) -> Option<Value> {
        let idx = self.find(k)?;
        self.len -= 1;
        Some(match &mut self.store {
            Store::Packed(p) => std::mem::take(&mut p.vals[idx]),
            Store::Hash(h) => h.remove_at(idx),
        })
    }

    /// Removes all elements (positions are not reused).
    pub fn clear(&mut self) {
        let end = self.end_pos_raw();
        self.store = Store::Packed(PackedStore {
            vals: Vec::new(),
            offset: end,
        });
        self.len = 0;
        self.next_free = i64::MIN;
        self.ptr = end;
    }

    /// `array_pop` semantics: removes the last element, decrements the next
    /// free index if it was the last integer key, and resets the internal pointer.
    pub fn pop(&mut self) -> Option<Value> {
        if self.len == 0 {
            return None;
        }
        let val = match &mut self.store {
            Store::Packed(p) => {
                let idx = p.vals.iter().rposition(|v| !v.is_undef())?;
                let val = std::mem::take(&mut p.vals[idx]);
                if idx as i64 == self.next_free.wrapping_sub(1) {
                    self.next_free -= 1;
                }
                let live_end = p.vals[..idx]
                    .iter()
                    .rposition(|v| !v.is_undef())
                    .map_or(0, |j| j + 1);
                let floor = self
                    .hwm
                    .get()
                    .saturating_sub(p.offset)
                    .clamp(0, p.vals.len() as i64) as usize;
                p.vals.truncate(live_end.max(floor));
                val
            }
            Store::Hash(h) => {
                let idx = h.entries.iter().rposition(|b| !b.val.is_undef())?;
                if let ArrayKey::Int(k) = h.entries[idx].key {
                    if k == self.next_free.wrapping_sub(1) {
                        self.next_free -= 1;
                    }
                }
                h.remove_at(idx)
            }
        };
        self.len -= 1;
        self.reset();
        Some(val.into_unref())
    }

    /// `array_shift` semantics: removes the first element, renumbers integer
    /// keys from 0 and resets the internal pointer.
    pub fn shift(&mut self) -> Option<Value> {
        if self.len == 0 {
            return None;
        }
        if let Store::Packed(p) = &mut self.store {
            let first = p.vals.iter().position(|v| !v.is_undef())?;
            let last = p.vals.iter().rposition(|v| !v.is_undef())?;
            let holes_inside = (last - first + 1) != self.len;
            if !holes_inside {
                let val = std::mem::take(&mut p.vals[first]);
                p.vals.drain(..=first);
                p.offset += first as Pos + 1;
                self.len -= 1;
                self.next_free = self.len as i64;
                self.reset();
                return Some(val.into_unref());
            }
            self.convert_to_hash();
        }
        let Store::Hash(h) = &mut self.store else {
            unreachable!()
        };
        let idx = h.entries.iter().position(|b| !b.val.is_undef())?;
        let val = h.remove_at(idx);
        self.len -= 1;
        let mut k = 0i64;
        let mut changed = false;
        for b in h.entries.iter_mut() {
            if b.val.is_undef() {
                continue;
            }
            if let ArrayKey::Int(old) = &mut b.key {
                if *old != k {
                    *old = k;
                    changed = true;
                }
                k += 1;
            }
        }
        if changed {
            h.rebuild_table();
        }
        self.next_free = k;
        self.reset();
        Some(val.into_unref())
    }

    /// `array_unshift` semantics: prepends `vals` (keys 0..n), renumbers the
    /// existing integer keys, keeps string keys and resets the internal pointer.
    pub fn unshift(&mut self, vals: Vec<Value>) {
        let n = vals.len();
        if n == 0 {
            // PHP still renumbers.
            self.renumber_int_keys();
            self.reset();
            return;
        }
        let vals: Vec<Value> = vals.into_iter().map(sanitize).collect();
        if let Store::Packed(p) = &mut self.store {
            let live_end = p
                .vals
                .iter()
                .rposition(|v| !v.is_undef())
                .map_or(0, |j| j + 1);
            let holes = live_end != self.len;
            if !holes {
                let mut new_vals = vals;
                new_vals.append(&mut p.vals);
                p.vals = new_vals;
                p.offset -= n as Pos;
                self.len += n;
                self.next_free = self.len as i64;
                self.reset();
                return;
            }
            self.convert_to_hash();
        }
        let Store::Hash(h) = &mut self.store else {
            unreachable!()
        };
        let base = h
            .entries
            .iter()
            .find(|b| !b.val.is_undef())
            .map_or(h.next_pos, |b| b.pos)
            - n as Pos;
        let mut entries = Vec::with_capacity(n + self.len);
        for (i, v) in vals.into_iter().enumerate() {
            entries.push(Bucket {
                pos: base + i as Pos,
                key: ArrayKey::Int(i as i64),
                val: v,
            });
        }
        let mut k = n as i64;
        for b in std::mem::take(&mut h.entries) {
            if b.val.is_undef() {
                continue;
            }
            let key = match b.key {
                ArrayKey::Int(_) => {
                    k += 1;
                    ArrayKey::Int(k - 1)
                }
                s => s,
            };
            entries.push(Bucket { key, ..b });
        }
        h.entries = entries;
        h.dead = 0;
        h.rebuild_table();
        self.len += n;
        self.next_free = k;
        self.reset();
    }

    fn renumber_int_keys(&mut self) {
        if let Store::Hash(h) = &mut self.store {
            let mut k = 0i64;
            for b in h.entries.iter_mut().filter(|b| !b.val.is_undef()) {
                if let ArrayKey::Int(old) = &mut b.key {
                    *old = k;
                    k += 1;
                }
            }
            h.rebuild_table();
            self.next_free = k;
        } else {
            self.next_free = self.len as i64;
        }
    }

    // ----- iteration ----------------------------------------------------

    /// Iterates `(key, value)` in insertion order.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            inner: match &self.store {
                Store::Packed(p) => IterInner::Packed(p.vals.iter().enumerate()),
                Store::Hash(h) => IterInner::Hash(h.entries.iter()),
            },
            remaining: self.len,
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = ArrayKey> + '_ {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> + '_ {
        self.iter().map(|(_, v)| v)
    }

    /// Mutable access to all values in order.
    pub fn values_mut(&mut self) -> Box<dyn Iterator<Item = &mut Value> + '_> {
        match &mut self.store {
            Store::Packed(p) => Box::new(p.vals.iter_mut().filter(|v| !v.is_undef())),
            Store::Hash(h) => Box::new(
                h.entries
                    .iter_mut()
                    .filter(|b| !b.val.is_undef())
                    .map(|b| &mut b.val),
            ),
        }
    }

    /// Iterates `(position, key, value)`.
    pub fn iter_pos(&self) -> impl Iterator<Item = (Pos, ArrayKey, &Value)> + '_ {
        let it: Box<dyn Iterator<Item = (Pos, ArrayKey, &Value)>> = match &self.store {
            Store::Packed(p) => Box::new(
                p.vals
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| !v.is_undef())
                    .map(move |(i, v)| (p.offset + i as Pos, ArrayKey::Int(i as i64), v)),
            ),
            Store::Hash(h) => Box::new(
                h.entries
                    .iter()
                    .filter(|b| !b.val.is_undef())
                    .map(|b| (b.pos, b.key.clone(), &b.val)),
            ),
        };
        it
    }

    /// Iterates `(position, key, value)` starting at the first live element at
    /// or after `pos`.
    pub fn iter_from(&self, pos: Pos) -> impl Iterator<Item = (Pos, ArrayKey, &Value)> + '_ {
        let mut next = self.valid_pos(pos);
        std::iter::from_fn(move || {
            let p = next?;
            let (k, v) = self.entry_at(p)?;
            next = self.valid_pos(p + 1);
            Some((p, k, v))
        })
    }

    pub fn first(&self) -> Option<(ArrayKey, &Value)> {
        self.iter().next()
    }

    pub fn last(&self) -> Option<(ArrayKey, &Value)> {
        let p = self.last_pos()?;
        self.entry_at(p)
    }

    /// `array_is_list()`: keys are exactly 0, 1, 2, ... in order.
    pub fn is_list(&self) -> bool {
        match &self.store {
            Store::Packed(p) => {
                let live_end = p
                    .vals
                    .iter()
                    .rposition(|v| !v.is_undef())
                    .map_or(0, |j| j + 1);
                live_end == self.len
            }
            Store::Hash(_) => self
                .iter()
                .enumerate()
                .all(|(i, (k, _))| k == ArrayKey::Int(i as i64)),
        }
    }

    // ----- positions ----------------------------------------------------

    fn end_pos_raw(&self) -> Pos {
        match &self.store {
            Store::Packed(p) => p.end_pos(),
            Store::Hash(h) => h.end_pos(),
        }
    }

    /// The past-the-end position. Elements inserted later get positions at
    /// or after it, so an iterator parked here sees them.
    pub fn end_pos(&self) -> Pos {
        self.note_pos(self.end_pos_raw())
    }

    /// `_zend_hash_get_valid_pos`: the first live position `>= pos`.
    pub fn valid_pos(&self, pos: Pos) -> Option<Pos> {
        match &self.store {
            Store::Packed(p) => {
                let start = p.lower_bound(pos);
                p.vals[start..]
                    .iter()
                    .position(|v| !v.is_undef())
                    .map(|j| p.offset + (start + j) as Pos)
            }
            Store::Hash(h) => {
                let start = h.lower_bound(pos);
                h.entries[start..]
                    .iter()
                    .find(|b| !b.val.is_undef())
                    .map(|b| b.pos)
            }
        }
    }

    /// First live position.
    pub fn first_pos(&self) -> Option<Pos> {
        self.valid_pos(Pos::MIN)
    }

    /// Last live position.
    pub fn last_pos(&self) -> Option<Pos> {
        match &self.store {
            Store::Packed(p) => p
                .vals
                .iter()
                .rposition(|v| !v.is_undef())
                .map(|j| p.offset + j as Pos),
            Store::Hash(h) => h
                .entries
                .iter()
                .rev()
                .find(|b| !b.val.is_undef())
                .map(|b| b.pos),
        }
    }

    /// Position used by a foreach iterator after visiting the element at
    /// `pos` (php-src stores `pos + 1`).
    pub fn pos_after(&self, pos: Pos) -> Pos {
        self.note_pos(pos + 1)
    }

    /// Last live position strictly before `pos`.
    pub fn pos_before(&self, pos: Pos) -> Option<Pos> {
        match &self.store {
            Store::Packed(p) => {
                let end = p.lower_bound(pos);
                p.vals[..end]
                    .iter()
                    .rposition(|v| !v.is_undef())
                    .map(|j| p.offset + j as Pos)
            }
            Store::Hash(h) => {
                let end = h.lower_bound(pos);
                h.entries[..end]
                    .iter()
                    .rev()
                    .find(|b| !b.val.is_undef())
                    .map(|b| b.pos)
            }
        }
    }

    fn slot_at(&self, pos: Pos) -> Option<(ArrayKey, &Value)> {
        match &self.store {
            Store::Packed(p) => {
                let i = pos.checked_sub(p.offset)?;
                let v = p.vals.get(usize::try_from(i).ok()?)?;
                (!v.is_undef()).then_some((ArrayKey::Int(i), v))
            }
            Store::Hash(h) => {
                let idx = h.lower_bound(pos);
                let b = h.entries.get(idx)?;
                (b.pos == pos && !b.val.is_undef()).then(|| (b.key.clone(), &b.val))
            }
        }
    }

    /// The element exactly at `pos`, if it is live.
    pub fn entry_at(&self, pos: Pos) -> Option<(ArrayKey, &Value)> {
        self.slot_at(pos)
    }

    pub fn key_at(&self, pos: Pos) -> Option<ArrayKey> {
        self.slot_at(pos).map(|(k, _)| k)
    }

    pub fn value_at_mut(&mut self, pos: Pos) -> Option<&mut Value> {
        match &mut self.store {
            Store::Packed(p) => {
                let i = pos.checked_sub(p.offset)?;
                let v = p.vals.get_mut(usize::try_from(i).ok()?)?;
                (!v.is_undef()).then_some(v)
            }
            Store::Hash(h) => {
                let idx = h.lower_bound(pos);
                let b = h.entries.get_mut(idx)?;
                (b.pos == pos && !b.val.is_undef()).then_some(&mut b.val)
            }
        }
    }

    /// Position of `key`, if present.
    pub fn pos_of(&self, key: &ArrayKey) -> Option<Pos> {
        let idx = self.find(key.as_key_ref())?;
        Some(match &self.store {
            Store::Packed(p) => p.offset + idx as Pos,
            Store::Hash(h) => h.entries[idx].pos,
        })
    }

    // ----- internal pointer (current/key/next/prev/reset/end) -----------

    pub fn internal_pointer(&self) -> Pos {
        self.ptr
    }

    pub fn set_internal_pointer(&mut self, pos: Pos) {
        self.ptr = pos;
    }

    /// `current()`.
    pub fn current(&self) -> Option<&Value> {
        let p = self.valid_pos(self.ptr)?;
        self.slot_at(p).map(|(_, v)| v)
    }

    /// `key()`.
    pub fn current_key(&self) -> Option<ArrayKey> {
        let p = self.valid_pos(self.ptr)?;
        self.key_at(p)
    }

    /// `next()` / `zend_hash_move_forward`; returns `false` if already past the end.
    pub fn move_forward(&mut self) -> bool {
        match self.valid_pos(self.ptr) {
            Some(p) => {
                self.ptr = match self.valid_pos(p + 1) {
                    Some(n) => n,
                    None => self.end_pos(),
                };
                true
            }
            None => false,
        }
    }

    /// `prev()` / `zend_hash_move_backwards`; returns `false` if already past the end.
    pub fn move_backward(&mut self) -> bool {
        if self.valid_pos(self.ptr).is_none() {
            return false;
        }
        self.ptr = match self.pos_before(self.ptr) {
            Some(p) => p,
            None => self.end_pos(),
        };
        true
    }

    /// `zend_hash_internal_pointer_reset`.
    pub fn reset(&mut self) {
        self.ptr = match self.first_pos() {
            Some(p) => p,
            None => self.end_pos(),
        };
    }

    /// `zend_hash_internal_pointer_end`.
    pub fn end(&mut self) {
        self.ptr = match self.last_pos() {
            Some(p) => p,
            None => self.end_pos(),
        };
    }

    // ----- bulk ---------------------------------------------------------

    /// Removes and returns all `(key, value)` pairs in order.
    pub fn take_entries(&mut self) -> Vec<(ArrayKey, Value)> {
        let out = match &mut self.store {
            Store::Packed(p) => std::mem::take(&mut p.vals)
                .into_iter()
                .enumerate()
                .filter(|(_, v)| !v.is_undef())
                .map(|(i, v)| (ArrayKey::Int(i as i64), v))
                .collect(),
            Store::Hash(h) => std::mem::take(&mut h.entries)
                .into_iter()
                .filter(|b| !b.val.is_undef())
                .map(|b| (b.key, b.val))
                .collect(),
        };
        self.clear();
        out
    }

    /// Stable sort (`zend_hash_sort`). With `renumber`, keys become 0..n
    /// (`sort`, `usort`); otherwise keys are kept (`asort`, `ksort`, `uasort`).
    /// The comparator may fail; the first error is returned and the array is
    /// left in an unspecified (but valid) order.
    pub fn sort_by<E>(
        &mut self,
        renumber: bool,
        mut cmp: impl FnMut((&ArrayKey, &Value), (&ArrayKey, &Value)) -> Result<Ordering, E>,
    ) -> Result<(), E> {
        let end = self.end_pos_raw();
        let mut items: Vec<(Pos, ArrayKey, Value)> = match &mut self.store {
            Store::Packed(p) => {
                let off = p.offset;
                std::mem::take(&mut p.vals)
                    .into_iter()
                    .enumerate()
                    .filter(|(_, v)| !v.is_undef())
                    .map(|(i, v)| (off + i as Pos, ArrayKey::Int(i as i64), v))
                    .collect()
            }
            Store::Hash(h) => std::mem::take(&mut h.entries)
                .into_iter()
                .filter(|b| !b.val.is_undef())
                .map(|b| (b.pos, b.key, b.val))
                .collect(),
        };
        let positions: Vec<Pos> = items.iter().map(|(p, _, _)| *p).collect();
        let mut err = None;
        items.sort_by(|a, b| {
            if err.is_some() {
                return Ordering::Equal;
            }
            match cmp((&a.1, &a.2), (&b.1, &b.2)) {
                Ok(o) => o,
                Err(e) => {
                    err = Some(e);
                    Ordering::Equal
                }
            }
        });
        let n = items.len();
        if renumber {
            let vals: Vec<Value> = items.into_iter().map(|(_, _, v)| v).collect();
            self.store = Store::Packed(PackedStore {
                vals,
                offset: end - n as Pos,
            });
            self.next_free = n as i64;
        } else {
            let mut h = HashStore::new(n);
            for ((_, k, v), pos) in items.into_iter().zip(positions) {
                h.entries.push(Bucket {
                    pos,
                    key: k,
                    val: v,
                });
            }
            h.next_pos = end;
            h.rebuild_table();
            self.store = Store::Hash(h);
        }
        self.reset();
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Duplicates the array following `zend_array_dup` (same as `clone()`).
    pub fn dup(&self) -> PhpArray {
        self.clone()
    }
}

/// `zend_array_dup_value`: a reference held only by this array is copied as a
/// plain value, unless it points back at the array being duplicated.
fn dup_value(v: &Value, src: *const PhpArray) -> Value {
    match v {
        Value::Ref(r) if r.refcount() == 1 => {
            let inner = r.borrow();
            match &*inner {
                Value::Array(a) if std::ptr::eq(Rc::as_ptr(a), src) => v.clone(),
                other => other.clone(),
            }
        }
        _ => v.clone(),
    }
}

impl Clone for PhpArray {
    fn clone(&self) -> Self {
        let src: *const PhpArray = self;
        let store = match &self.store {
            Store::Packed(p) => Store::Packed(PackedStore {
                vals: p.vals.iter().map(|v| dup_value(v, src)).collect(),
                offset: p.offset,
            }),
            Store::Hash(h) => {
                let mut nh = HashStore::new(self.len);
                for b in h.entries.iter().filter(|b| !b.val.is_undef()) {
                    nh.entries.push(Bucket {
                        pos: b.pos,
                        key: b.key.clone(),
                        val: dup_value(&b.val, src),
                    });
                }
                nh.next_pos = h.next_pos;
                nh.rebuild_table();
                Store::Hash(nh)
            }
        };
        let mut out = PhpArray {
            store,
            len: self.len,
            next_free: self.next_free,
            ptr: self.ptr,
            hwm: Cell::new(self.hwm.get()),
        };
        if self.valid_pos(self.ptr).is_none() {
            // php-src resets a past-the-end internal pointer to the first bucket.
            out.ptr = out.first_pos().unwrap_or(Pos::MIN);
        }
        out
    }
}

impl fmt::Debug for PhpArray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Iterator over `(key, &value)` pairs.
pub struct Iter<'a> {
    inner: IterInner<'a>,
    remaining: usize,
}

enum IterInner<'a> {
    Packed(std::iter::Enumerate<std::slice::Iter<'a, Value>>),
    Hash(std::slice::Iter<'a, Bucket>),
}

impl<'a> Iterator for Iter<'a> {
    type Item = (ArrayKey, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        let item = match &mut self.inner {
            IterInner::Packed(it) => it
                .find(|(_, v)| !v.is_undef())
                .map(|(i, v)| (ArrayKey::Int(i as i64), v)),
            IterInner::Hash(it) => it
                .find(|b| !b.val.is_undef())
                .map(|b| (b.key.clone(), &b.val)),
        };
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a> IntoIterator for &'a PhpArray {
    type Item = (ArrayKey, &'a Value);
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<K: Into<ArrayKey>, V: Into<Value>> FromIterator<(K, V)> for PhpArray {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        PhpArray::from_pairs(iter)
    }
}

impl FromIterator<Value> for PhpArray {
    fn from_iter<T: IntoIterator<Item = Value>>(iter: T) -> Self {
        PhpArray::from_values(iter.into_iter().collect())
    }
}

/// Helper: wraps the element at `key` in a reference (creating it if needed)
/// and returns the reference (`$x = &$a[key]`).
pub fn make_element_ref(arr: &mut PhpArray, key: ArrayKey) -> PhpRef {
    let slot = arr.entry_mut(key);
    match slot {
        Value::Ref(r) => r.clone(),
        other => {
            let r = PhpRef::new(std::mem::take(other));
            *other = Value::Ref(r.clone());
            r
        }
    }
}
