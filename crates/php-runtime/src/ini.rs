//! INI directives (`Zend/zend_ini.c`).
//!
//! Entries are registered by modules with a default value and a modifiable
//! mask. `ini_set` (`PHP_INI_USER`) fails for unknown entries, for entries not
//! modifiable at runtime and when the entry's validator rejects the value.
//! The `Vm` mirrors a few hot entries into typed fields (see `Vm::ini_set`).

use crate::util::NameMap;
use crate::value::PhpStr;

/// `PHP_INI_*` modifiable masks.
pub mod modifiable {
    pub const USER: u32 = 1;
    pub const PERDIR: u32 = 2;
    pub const SYSTEM: u32 = 4;
    pub const ALL: u32 = USER | PERDIR | SYSTEM;
}

/// Validates a new value (`on_modify` returning FAILURE rejects it).
pub type IniValidator = fn(&[u8]) -> bool;

/// Static description of an ini entry, as registered by a module.
#[derive(Clone, Copy, Debug)]
pub struct IniDef {
    pub name: &'static str,
    /// `None` is php-src's `NULL` default (reads as `""`).
    pub default: Option<&'static str>,
    pub modifiable: u32,
    pub validator: Option<IniValidator>,
}

impl IniDef {
    pub const fn new(name: &'static str, default: Option<&'static str>, modifiable: u32) -> IniDef {
        IniDef {
            name,
            default,
            modifiable,
            validator: None,
        }
    }

    pub const fn validated(mut self, v: IniValidator) -> IniDef {
        self.validator = Some(v);
        self
    }
}

/// A registered ini entry.
#[derive(Clone, Debug)]
pub struct IniEntry {
    pub name: PhpStr,
    pub value: Option<PhpStr>,
    /// Value before runtime modification (restored by `ini_restore` / request end).
    pub orig_value: Option<Option<PhpStr>>,
    pub modifiable: u32,
    pub validator: Option<IniValidator>,
    pub module: &'static str,
}

impl IniEntry {
    /// The current value as `ini_get` returns it (`NULL` reads as `""`).
    pub fn value_str(&self) -> PhpStr {
        self.value.clone().unwrap_or_default()
    }

    pub fn is_modified(&self) -> bool {
        self.orig_value.is_some()
    }
}

/// Why an `ini_set` failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IniError {
    Unknown,
    NotModifiable,
    Invalid,
}

/// All registered ini entries.
#[derive(Default)]
pub struct IniRegistry {
    entries: NameMap<IniEntry>,
}

impl IniRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers (or re-registers, replacing the default of) an entry.
    pub fn register(&mut self, def: &IniDef, module: &'static str) {
        let entry = IniEntry {
            name: PhpStr::from(def.name),
            value: def.default.map(PhpStr::from),
            orig_value: None,
            modifiable: def.modifiable,
            validator: def.validator,
            module,
        };
        self.entries.insert(entry.name.clone(), entry);
    }

    pub fn get(&self, name: &[u8]) -> Option<&IniEntry> {
        self.entries.get(name)
    }

    /// `zend_ini_get_value`: `None` for unknown entries, `""` for NULL values.
    pub fn value(&self, name: &[u8]) -> Option<PhpStr> {
        self.entries.get(name).map(IniEntry::value_str)
    }

    /// `zend_alter_ini_entry_ex` with the given modify type. Returns the old value.
    pub fn alter(
        &mut self,
        name: &[u8],
        value: PhpStr,
        modify_type: u32,
        force: bool,
    ) -> Result<PhpStr, IniError> {
        let e = self.entries.get_mut(name).ok_or(IniError::Unknown)?;
        if !force && e.modifiable & modify_type == 0 {
            return Err(IniError::NotModifiable);
        }
        if let Some(v) = e.validator {
            if !v(&value) {
                return Err(IniError::Invalid);
            }
        }
        let old = e.value_str();
        if e.orig_value.is_none() {
            e.orig_value = Some(e.value.clone());
        }
        e.value = Some(value);
        Ok(old)
    }

    /// Startup-time change (`-d name=value`, php.ini): becomes the new default.
    pub fn set_default(&mut self, name: &[u8], value: Option<PhpStr>) -> bool {
        match self.entries.get_mut(name) {
            Some(e) => {
                e.value = value;
                e.orig_value = None;
                true
            }
            None => false,
        }
    }

    /// `ini_restore`: back to the value before runtime modification.
    pub fn restore(&mut self, name: &[u8]) -> bool {
        match self.entries.get_mut(name) {
            Some(e) => {
                if let Some(orig) = e.orig_value.take() {
                    e.value = orig;
                }
                true
            }
            None => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &IniEntry> {
        self.entries.values()
    }

    pub fn names_modified(&self) -> Vec<PhpStr> {
        self.entries
            .values()
            .filter(|e| e.is_modified())
            .map(|e| e.name.clone())
            .collect()
    }
}

/// `zend_ini_parse_bool`: "true"/"yes"/"on" (any case) or a non-zero integer.
pub fn parse_bool(v: &[u8]) -> bool {
    if v.eq_ignore_ascii_case(b"true")
        || v.eq_ignore_ascii_case(b"yes")
        || v.eq_ignore_ascii_case(b"on")
    {
        true
    } else {
        atoi(v) != 0
    }
}

/// C `atoi`: optional whitespace and sign, then decimal digits.
pub fn atoi(v: &[u8]) -> i64 {
    strtol(v, 10).0
}

/// C `strtol` (with `base` 0 auto-detecting `0x` and leading-`0` octal).
/// Returns the value (saturated on overflow) and the bytes consumed.
pub fn strtol(v: &[u8], base: u32) -> (i64, usize) {
    let mut i = 0;
    while i < v.len() && (v[i] == b' ' || (0x09..=0x0d).contains(&v[i])) {
        i += 1;
    }
    let mut neg = false;
    if i < v.len() && (v[i] == b'+' || v[i] == b'-') {
        neg = v[i] == b'-';
        i += 1;
    }
    let mut base = base;
    if (base == 0 || base == 16)
        && i + 1 < v.len()
        && v[i] == b'0'
        && (v[i + 1] | 0x20) == b'x'
        && v.get(i + 2).is_some_and(|c| c.is_ascii_hexdigit())
    {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if i < v.len() && v[i] == b'0' { 8 } else { 10 };
    }
    let start = i;
    let mut acc: i128 = 0;
    let mut overflow = false;
    while i < v.len() {
        let d = match v[i] {
            c @ b'0'..=b'9' => (c - b'0') as u32,
            c @ b'a'..=b'z' => (c - b'a') as u32 + 10,
            c @ b'A'..=b'Z' => (c - b'A') as u32 + 10,
            _ => break,
        };
        if d >= base {
            break;
        }
        acc = acc * base as i128 + d as i128;
        if acc > i64::MAX as i128 + 1 {
            overflow = true;
            acc = i64::MAX as i128 + 1;
        }
        i += 1;
    }
    if i == start {
        return (0, 0);
    }
    let v = if neg { -acc } else { acc };
    let v = if overflow || v > i64::MAX as i128 || v < i64::MIN as i128 {
        if neg { i64::MIN } else { i64::MAX }
    } else {
        v as i64
    };
    (v, i)
}

/// `zend_atol`: `strtol(base 0)` followed by an optional `K`/`M`/`G` suffix
/// (wrapping multiplication like php-src).
pub fn atol_quantity(v: &[u8]) -> i64 {
    let (n, _) = strtol(v, 0);
    let mut r = n as u64;
    match v.last() {
        Some(b'g' | b'G') => r = r.wrapping_mul(1024 * 1024 * 1024),
        Some(b'm' | b'M') => r = r.wrapping_mul(1024 * 1024),
        Some(b'k' | b'K') => r = r.wrapping_mul(1024),
        _ => {}
    }
    r as i64
}

/// Validator for `precision`/`serialize_precision` (`OnSetPrecision`): `>= -1`.
pub fn validate_precision(v: &[u8]) -> bool {
    atol_quantity(v) >= -1
}

/// `display_errors` parsing (`php_get_display_errors_mode`): 0 off, 1 stdout, 2 stderr.
pub fn display_errors_mode(v: &[u8]) -> i64 {
    if v.eq_ignore_ascii_case(b"on")
        || v.eq_ignore_ascii_case(b"yes")
        || v.eq_ignore_ascii_case(b"true")
    {
        1
    } else if v.eq_ignore_ascii_case(b"stderr") {
        2
    } else if v.eq_ignore_ascii_case(b"stdout") {
        1
    } else {
        let n = atoi(v);
        if n == 2 {
            2
        } else if n != 0 {
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities() {
        assert_eq!(atol_quantity(b"128M"), 128 * 1024 * 1024);
        assert_eq!(atol_quantity(b"2k"), 2048);
        assert_eq!(atol_quantity(b"-1"), -1);
        assert_eq!(atol_quantity(b"0x10"), 16);
        assert_eq!(atol_quantity(b"010"), 8);
        assert_eq!(atoi(b"  42abc"), 42);
        assert_eq!(atoi(b"abc"), 0);
    }

    #[test]
    fn bools() {
        assert!(parse_bool(b"On"));
        assert!(parse_bool(b"yes"));
        assert!(parse_bool(b"1"));
        assert!(!parse_bool(b"off"));
        assert!(!parse_bool(b""));
        assert_eq!(display_errors_mode(b"stderr"), 2);
        assert_eq!(display_errors_mode(b"1"), 1);
        assert_eq!(display_errors_mode(b"0"), 0);
    }

    #[test]
    fn alter_and_restore() {
        let mut r = IniRegistry::new();
        r.register(
            &IniDef::new("precision", Some("14"), modifiable::ALL).validated(validate_precision),
            "Core",
        );
        r.register(&IniDef::new("sys", Some("x"), modifiable::SYSTEM), "Core");
        assert_eq!(
            r.alter(b"precision", "10".into(), modifiable::USER, false),
            Ok("14".into())
        );
        assert_eq!(r.value(b"precision").unwrap(), "10");
        assert_eq!(
            r.alter(b"precision", "-2".into(), modifiable::USER, false),
            Err(IniError::Invalid)
        );
        assert_eq!(
            r.alter(b"sys", "y".into(), modifiable::USER, false),
            Err(IniError::NotModifiable)
        );
        assert_eq!(
            r.alter(b"nope", "y".into(), modifiable::USER, false),
            Err(IniError::Unknown)
        );
        assert!(r.restore(b"precision"));
        assert_eq!(r.value(b"precision").unwrap(), "14");
    }
}
