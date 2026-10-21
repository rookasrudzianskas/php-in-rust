//! Global constants (`Zend/zend_constants.c`).
//!
//! Constant names are case-sensitive, except that the namespace part of a
//! namespaced name is case-insensitive, and `true`/`false`/`null` are
//! recognised in any case.

use crate::util::NameMap;
use crate::value::{PhpStr, Value};

/// `CONST_*` flags.
pub mod const_flags {
    /// Registered by an extension (survives requests).
    pub const PERSISTENT: u32 = 1 << 0;
    /// Using the constant emits a deprecation.
    pub const DEPRECATED: u32 = 1 << 1;
    pub const NO_FILE_CACHE: u32 = 1 << 2;
}

/// A global constant.
#[derive(Clone, Debug)]
pub struct Constant {
    pub name: PhpStr,
    pub value: Value,
    pub flags: u32,
    /// Owning extension (`get_defined_constants(true)` categories); `None` = "user".
    pub module: Option<&'static str>,
    /// Deprecation message suffix for `#[\Deprecated]` constants.
    pub deprecation: Option<&'static str>,
}

/// The constant table.
#[derive(Default)]
pub struct ConstantTable {
    map: NameMap<Constant>,
}

/// Normalises a constant name for lookup: the namespace part is lowercased.
pub fn normalize_name(name: &[u8]) -> PhpStr {
    let name = name.strip_prefix(b"\\").unwrap_or(name);
    match name.iter().rposition(|&c| c == b'\\') {
        Some(i) => {
            let mut v = name[..i].to_ascii_lowercase();
            v.extend_from_slice(&name[i..]);
            PhpStr::from_vec(v)
        }
        None => PhpStr::from_bytes(name),
    }
}

impl ConstantTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Defines a constant; returns `false` if it already exists.
    pub fn define(&mut self, c: Constant) -> bool {
        let key = normalize_name(&c.name);
        if self.map.contains_key(&key) {
            return false;
        }
        self.map.insert(key, c);
        true
    }

    /// Looks a constant up (`zend_get_constant_str`).
    pub fn get(&self, name: &[u8]) -> Option<&Constant> {
        let direct = name.strip_prefix(b"\\").unwrap_or(name);
        if let Some(c) = self.map.get(direct) {
            return Some(c);
        }
        if direct.contains(&b'\\') {
            let key = normalize_name(direct);
            if let Some(c) = self.map.get(&key) {
                return Some(c);
            }
        }
        if direct.len() == 4 || direct.len() == 5 {
            let lc = direct.to_ascii_lowercase();
            if matches!(lc.as_slice(), b"true" | b"false" | b"null") {
                return self.map.get(&lc.to_ascii_uppercase());
            }
        }
        None
    }

    pub fn contains(&self, name: &[u8]) -> bool {
        self.get(name).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Constant> {
        self.map.values()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, v: i64) -> Constant {
        Constant {
            name: PhpStr::from(name),
            value: Value::Int(v),
            flags: 0,
            module: None,
            deprecation: None,
        }
    }

    #[test]
    fn case_rules() {
        let mut t = ConstantTable::new();
        assert!(t.define(c("FOO", 1)));
        assert!(!t.define(c("FOO", 2)));
        assert!(t.define(c("foo", 3)));
        assert!(t.define(c("NS\\Sub\\BAR", 4)));
        t.define(Constant {
            name: "TRUE".into(),
            value: Value::Bool(true),
            flags: 0,
            module: None,
            deprecation: None,
        });
        assert_eq!(t.get(b"FOO").unwrap().value.as_int(), Some(1));
        assert_eq!(t.get(b"foo").unwrap().value.as_int(), Some(3));
        assert!(t.get(b"Foo").is_none());
        assert_eq!(t.get(b"ns\\sub\\BAR").unwrap().value.as_int(), Some(4));
        assert_eq!(t.get(b"\\NS\\SUB\\BAR").unwrap().value.as_int(), Some(4));
        assert!(t.get(b"ns\\sub\\bar").is_none());
        assert!(t.get(b"tRuE").is_some());
    }
}
