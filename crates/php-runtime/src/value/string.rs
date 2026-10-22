//! `PhpStr`: PHP's binary string (`zend_string`).
//!
//! PHP strings are arbitrary byte sequences; they are never assumed to be
//! UTF-8. A `PhpStr` is a cheaply clonable handle (`Rc`) with copy-on-write
//! mutation: appending to a uniquely owned string reuses its buffer, so a
//! `.=` loop is amortised O(1) per append. The hash used by arrays is cached.

use std::borrow::{Borrow, Cow};
use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;

/// A PHP string value: immutable-by-default shared bytes with COW mutation.
#[derive(Clone)]
pub struct PhpStr(Rc<StrBuf>);

#[derive(Clone)]
struct StrBuf {
    /// Cached hash; `0` means "not computed yet".
    hash: Cell<u64>,
    bytes: Vec<u8>,
}

impl PhpStr {
    /// The empty string.
    pub fn new() -> Self {
        Self::from_vec(Vec::new())
    }

    /// Alias for [`PhpStr::new`].
    pub fn empty() -> Self {
        Self::new()
    }

    pub fn from_vec(bytes: Vec<u8>) -> Self {
        PhpStr(Rc::new(StrBuf {
            hash: Cell::new(0),
            bytes,
        }))
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_vec(bytes.to_vec())
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self::from_vec(Vec::with_capacity(cap))
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.bytes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.bytes.is_empty()
    }

    /// The bytes as `&str` if they are valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0.bytes).ok()
    }

    /// Lossy UTF-8 view for diagnostics and Rust-side formatting.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0.bytes)
    }

    /// Mutable access to the bytes, cloning the buffer first if it is shared.
    pub fn make_mut(&mut self) -> &mut Vec<u8> {
        let buf = Rc::make_mut(&mut self.0);
        buf.hash.set(0);
        &mut buf.bytes
    }

    /// Appends bytes in place (amortised O(1) when uniquely owned).
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if Rc::strong_count(&self.0) == 1 && Rc::weak_count(&self.0) == 0 {
            self.make_mut().extend_from_slice(bytes);
        } else {
            let mut v = Vec::with_capacity((self.len() + bytes.len()).max(16));
            v.extend_from_slice(self.as_bytes());
            v.extend_from_slice(bytes);
            *self = Self::from_vec(v);
        }
    }

    pub fn push_str(&mut self, s: &str) {
        self.push_bytes(s.as_bytes());
    }

    pub fn push(&mut self, b: u8) {
        self.push_bytes(&[b]);
    }

    /// `a . b` as a new string.
    pub fn concat(&self, other: &[u8]) -> PhpStr {
        let mut v = Vec::with_capacity(self.len() + other.len());
        v.extend_from_slice(self.as_bytes());
        v.extend_from_slice(other);
        Self::from_vec(v)
    }

    /// Extracts the bytes, avoiding a copy when uniquely owned.
    pub fn into_vec(self) -> Vec<u8> {
        match Rc::try_unwrap(self.0) {
            Ok(buf) => buf.bytes,
            Err(rc) => rc.bytes.clone(),
        }
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.bytes.clone()
    }

    /// Whether both handles point at the same buffer.
    pub fn ptr_eq(a: &PhpStr, b: &PhpStr) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }

    pub fn is_unique(&self) -> bool {
        Rc::strong_count(&self.0) == 1
    }

    /// The (cached) hash used by `PhpArray`.
    #[inline]
    pub fn hash_code(&self) -> u64 {
        let h = self.0.hash.get();
        if h != 0 {
            return h;
        }
        let h = hash_bytes(&self.0.bytes);
        self.0.hash.set(h);
        h
    }

    /// ASCII lowercase copy (`zend_string_tolower`); shares the buffer if unchanged.
    pub fn to_ascii_lowercase(&self) -> PhpStr {
        if self.as_bytes().iter().any(|b| b.is_ascii_uppercase()) {
            Self::from_vec(self.as_bytes().to_ascii_lowercase())
        } else {
            self.clone()
        }
    }

    pub fn to_ascii_uppercase(&self) -> PhpStr {
        if self.as_bytes().iter().any(|b| b.is_ascii_lowercase()) {
            Self::from_vec(self.as_bytes().to_ascii_uppercase())
        } else {
            self.clone()
        }
    }

    pub fn eq_ignore_ascii_case(&self, other: &[u8]) -> bool {
        self.as_bytes().eq_ignore_ascii_case(other)
    }
}

/// Hash function for array keys and cached string hashes. Never returns 0.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut h: u64 = bytes.len() as u64;
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().expect("chunk of 8"));
        h = (h.rotate_left(5) ^ w).wrapping_mul(K);
    }
    let rem = chunks.remainder();
    if !rem.is_empty() {
        let mut buf = [0u8; 8];
        buf[..rem.len()].copy_from_slice(rem);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(buf)).wrapping_mul(K);
    }
    let h = fold_mul(h ^ 0x2d35_8dcc_aa6c_78a5, 0x8bb8_4b93_962e_acc9);
    if h == 0 { 1 } else { h }
}

/// Hash for integer array keys (well mixed in both high and low bits).
#[inline]
pub fn hash_int(i: i64) -> u64 {
    let h = fold_mul((i as u64) ^ 0x243f_6a88_85a3_08d3, 0x9e37_79b9_7f4a_7c15);
    if h == 0 { 1 } else { h }
}

#[inline]
fn fold_mul(a: u64, b: u64) -> u64 {
    let r = (a as u128).wrapping_mul(b as u128);
    (r as u64) ^ ((r >> 64) as u64)
}

impl Default for PhpStr {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for PhpStr {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl AsRef<[u8]> for PhpStr {
    fn as_ref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl Borrow<[u8]> for PhpStr {
    fn borrow(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl PartialEq for PhpStr {
    fn eq(&self, other: &Self) -> bool {
        PhpStr::ptr_eq(self, other) || self.as_bytes() == other.as_bytes()
    }
}

impl Eq for PhpStr {}

impl PartialEq<[u8]> for PhpStr {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl PartialEq<&[u8]> for PhpStr {
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_bytes() == *other
    }
}

impl PartialEq<str> for PhpStr {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for PhpStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialOrd for PhpStr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Byte-wise ordering, identical to `zend_binary_strcmp`.
impl Ord for PhpStr {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

// Must agree with `Borrow<[u8]>` so `HashMap<PhpStr, _>` can be queried
// with `&[u8]` keys.
impl Hash for PhpStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl From<&str> for PhpStr {
    fn from(s: &str) -> Self {
        Self::from_bytes(s.as_bytes())
    }
}

impl From<String> for PhpStr {
    fn from(s: String) -> Self {
        Self::from_vec(s.into_bytes())
    }
}

impl From<&String> for PhpStr {
    fn from(s: &String) -> Self {
        Self::from_bytes(s.as_bytes())
    }
}

impl From<Vec<u8>> for PhpStr {
    fn from(v: Vec<u8>) -> Self {
        Self::from_vec(v)
    }
}

impl From<&[u8]> for PhpStr {
    fn from(v: &[u8]) -> Self {
        Self::from_bytes(v)
    }
}

impl<const N: usize> From<&[u8; N]> for PhpStr {
    fn from(v: &[u8; N]) -> Self {
        Self::from_bytes(v)
    }
}

impl From<Cow<'_, str>> for PhpStr {
    fn from(s: Cow<'_, str>) -> Self {
        Self::from_bytes(s.as_bytes())
    }
}

impl From<&PhpStr> for PhpStr {
    fn from(s: &PhpStr) -> Self {
        s.clone()
    }
}

impl fmt::Display for PhpStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl fmt::Debug for PhpStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        for chunk in self.as_bytes().utf8_chunks() {
            for c in chunk.valid().chars() {
                write!(f, "{}", c.escape_debug())?;
            }
            for b in chunk.invalid() {
                write!(f, "\\x{b:02x}")?;
            }
        }
        f.write_str("\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cow_append_is_in_place_when_unique() {
        let mut s = PhpStr::from("ab");
        let before = s.as_bytes().as_ptr();
        s.make_mut().reserve(1024);
        let reserved = s.as_bytes().as_ptr();
        for _ in 0..100 {
            s.push_bytes(b"x");
        }
        assert_eq!(s.len(), 102);
        assert_eq!(s.as_bytes().as_ptr(), reserved);
        let _ = before;
    }

    #[test]
    fn cow_append_separates_shared() {
        let a = PhpStr::from("ab");
        let mut b = a.clone();
        b.push_bytes(b"cd");
        assert_eq!(a, "ab");
        assert_eq!(b, "abcd");
    }

    #[test]
    fn hash_is_cached_and_reset_on_mutation() {
        let mut s = PhpStr::from("hello");
        let h1 = s.hash_code();
        assert_eq!(h1, hash_bytes(b"hello"));
        s.push_bytes(b"!");
        assert_eq!(s.hash_code(), hash_bytes(b"hello!"));
        assert_ne!(hash_bytes(b"a"), hash_bytes(b"a\0"));
    }

    #[test]
    fn ordering_matches_binary_strcmp() {
        let s = |x: &str| PhpStr::from(x);
        assert!(s("abc") < s("abd"));
        assert!(s("ab") < s("abc"));
        assert!(s("b") > s("abc"));
    }

    #[test]
    fn debug_escapes_binary() {
        let s = PhpStr::from_bytes(b"a\xffb\n");
        assert_eq!(format!("{s:?}"), "\"a\\xffb\\n\"");
    }
}
