//! Type declarations (`zend_type`) and PHP's type coercion rules
//! (`zend_verify_scalar_type_hint`, `zend_parse_arg_*_weak` in `zend_API.c` /
//! `zend_execute.c`). Shared by native-function ZPP and (later) userland
//! parameter, return and property type checks.

use std::fmt;

use crate::class::{CastKind, ClassEntry};
use crate::value::numeric::{Numeric, double_fits_long, is_long_compatible, is_numeric_str};
use crate::value::{PhpStr, Value, float_fmt};
use crate::vm::{PResult, Vm};

/// Type mask bits (`MAY_BE_*`).
pub mod mask {
    pub const NULL: u32 = 1 << 0;
    pub const FALSE: u32 = 1 << 1;
    pub const TRUE: u32 = 1 << 2;
    pub const INT: u32 = 1 << 3;
    pub const FLOAT: u32 = 1 << 4;
    pub const STRING: u32 = 1 << 5;
    pub const ARRAY: u32 = 1 << 6;
    pub const OBJECT: u32 = 1 << 7;
    pub const RESOURCE: u32 = 1 << 8;
    pub const CALLABLE: u32 = 1 << 9;
    pub const VOID: u32 = 1 << 10;
    pub const NEVER: u32 = 1 << 11;
    pub const STATIC: u32 = 1 << 12;
    pub const BOOL: u32 = FALSE | TRUE;
    pub const SCALAR: u32 = BOOL | INT | FLOAT | STRING;
    /// `mixed` (`MAY_BE_ANY`).
    pub const ANY: u32 = NULL | BOOL | INT | FLOAT | STRING | ARRAY | OBJECT | RESOURCE;
}

/// A class part of a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassType {
    Name(PhpStr),
    /// `A&B` (all must match).
    Intersection(Vec<PhpStr>),
}

/// A (possibly union / nullable / intersection) type declaration.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TypeDecl {
    pub mask: u32,
    pub classes: Vec<ClassType>,
}

impl TypeDecl {
    pub fn from_mask(mask: u32) -> TypeDecl {
        TypeDecl {
            mask,
            classes: Vec::new(),
        }
    }

    pub fn mixed() -> TypeDecl {
        Self::from_mask(mask::ANY)
    }

    pub fn class(name: &str) -> TypeDecl {
        TypeDecl {
            mask: 0,
            classes: vec![ClassType::Name(PhpStr::from(name))],
        }
    }

    pub fn is_mixed(&self) -> bool {
        self.mask & mask::ANY == mask::ANY
    }

    pub fn allows_null(&self) -> bool {
        self.mask & mask::NULL != 0
    }

    /// Parses stub/source type syntax: `?int`, `int|string|null`, `A&B`,
    /// `(A&B)|null`, `iterable` (→ `Traversable|array`), `mixed`, class names.
    pub fn parse(src: &str) -> Result<TypeDecl, String> {
        let s = src.trim();
        if s.is_empty() {
            return Err("empty type".into());
        }
        let (nullable, s) = match s.strip_prefix('?') {
            Some(rest) => (true, rest.trim()),
            None => (false, s),
        };
        let mut t = TypeDecl::default();
        for part in split_top(s, '|') {
            let part = part.trim();
            let inner = part
                .strip_prefix('(')
                .and_then(|p| p.strip_suffix(')'))
                .unwrap_or(part);
            if inner.contains('&') {
                let names = inner
                    .split('&')
                    .map(|n| PhpStr::from(n.trim().trim_start_matches('\\')))
                    .collect::<Vec<_>>();
                if names.iter().any(|n| n.is_empty()) {
                    return Err(format!("invalid intersection type {part:?}"));
                }
                t.classes.push(ClassType::Intersection(names));
                continue;
            }
            t.add_simple(inner)?;
        }
        if nullable {
            if t.is_mixed() {
                return Err("mixed cannot be nullable".into());
            }
            t.mask |= mask::NULL;
        }
        Ok(t)
    }

    fn add_simple(&mut self, name: &str) -> Result<(), String> {
        let lc = name.to_ascii_lowercase();
        let bits = match lc.as_str() {
            "null" => mask::NULL,
            "false" => mask::FALSE,
            "true" => mask::TRUE,
            "bool" => mask::BOOL,
            "int" => mask::INT,
            "float" => mask::FLOAT,
            "string" => mask::STRING,
            "array" => mask::ARRAY,
            "object" => mask::OBJECT,
            "callable" => mask::CALLABLE,
            "void" => mask::VOID,
            "never" => mask::NEVER,
            "static" => mask::STATIC,
            "mixed" => mask::ANY,
            "resource" => mask::RESOURCE,
            "iterable" => {
                self.classes
                    .push(ClassType::Name(PhpStr::from("Traversable")));
                mask::ARRAY
            }
            _ => {
                let n = name.trim_start_matches('\\');
                if n.is_empty()
                    || !n
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80)
                {
                    return Err(format!("invalid type name {name:?}"));
                }
                self.classes.push(ClassType::Name(PhpStr::from(n)));
                0
            }
        };
        self.mask |= bits;
        Ok(())
    }

    /// `zend_type_to_string`: canonical spelling used in error messages
    /// (class names first, then `static`, `callable`, `object`, `array`,
    /// `string`, `int`, `float`, `bool`/`false`/`true`, `void`, `never`, `null`;
    /// a single type plus null is written `?T`).
    pub fn to_type_string(&self) -> String {
        self.to_type_string_resolved(None)
    }

    /// Like [`TypeDecl::to_type_string`] resolving `self`/`parent` against `scope`.
    pub fn to_type_string_resolved(&self, scope: Option<&ClassEntry>) -> String {
        self.to_type_string_in(scope, None)
    }

    /// `zend_type_to_string_resolved` at runtime: `self`/`parent` resolve
    /// against `scope`, and `static` names `called_scope` when both are known.
    pub fn to_type_string_in(
        &self,
        scope: Option<&ClassEntry>,
        called_scope: Option<&ClassEntry>,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();
        for c in &self.classes {
            match c {
                ClassType::Name(n) => parts.push(resolve_name(n, scope)),
                ClassType::Intersection(ns) => {
                    let s = ns
                        .iter()
                        .map(|n| n.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("&");
                    if self.classes.len() > 1 || self.mask != 0 {
                        parts.push(format!("({s})"));
                    } else {
                        parts.push(s);
                    }
                }
            }
        }
        let m = self.mask;
        if m & mask::ANY == mask::ANY {
            parts.push("mixed".into());
            return parts.join("|");
        }
        if m & mask::STATIC != 0 {
            match (scope, called_scope) {
                (Some(_), Some(c)) => parts.push(cut_nul(&c.name)),
                _ => parts.push("static".into()),
            }
        }
        let simple: [(u32, &str); 6] = [
            (mask::CALLABLE, "callable"),
            (mask::OBJECT, "object"),
            (mask::ARRAY, "array"),
            (mask::STRING, "string"),
            (mask::INT, "int"),
            (mask::FLOAT, "float"),
        ];
        for (bit, name) in simple {
            if m & bit != 0 {
                parts.push(name.into());
            }
        }
        if m & mask::BOOL == mask::BOOL {
            parts.push("bool".into());
        } else if m & mask::FALSE != 0 {
            parts.push("false".into());
        } else if m & mask::TRUE != 0 {
            parts.push("true".into());
        }
        if m & mask::VOID != 0 {
            parts.push("void".into());
        }
        if m & mask::NEVER != 0 {
            parts.push("never".into());
        }
        if m & mask::NULL != 0 {
            let joined = parts.join("|");
            if parts.len() == 1 && !joined.contains('&') {
                return format!("?{joined}");
            }
            if parts.is_empty() {
                return "null".into();
            }
            parts.push("null".into());
        }
        parts.join("|")
    }

    /// Whether `v` satisfies the type without coercion (`zend_check_type`
    /// minus `callable`/`static`, which need runtime context).
    pub fn matches_exact(&self, v: &Value) -> bool {
        let m = self.mask;
        match v {
            Value::Undef | Value::Null => m & mask::NULL != 0,
            Value::Bool(false) => m & mask::FALSE != 0,
            Value::Bool(true) => m & mask::TRUE != 0,
            Value::Int(_) => m & mask::INT != 0,
            Value::Float(_) => m & mask::FLOAT != 0,
            Value::String(_) => m & mask::STRING != 0,
            Value::Array(_) => m & mask::ARRAY != 0,
            Value::Resource(_) => m & mask::RESOURCE != 0,
            Value::Object(o) => m & mask::OBJECT != 0 || self.matches_class(o.class()),
            Value::Ref(r) => self.matches_exact(&r.borrow()),
        }
    }

    /// Class part check (`instanceof` against named and intersection types).
    pub fn matches_class(&self, ce: &ClassEntry) -> bool {
        self.classes.iter().any(|c| match c {
            ClassType::Name(n) => ce.instance_of_name(n),
            ClassType::Intersection(ns) => ns.iter().all(|n| ce.instance_of_name(n)),
        })
    }
}

impl fmt::Display for TypeDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_type_string())
    }
}

fn resolve_name(n: &PhpStr, scope: Option<&ClassEntry>) -> String {
    if let Some(ce) = scope {
        if n.eq_ignore_ascii_case(b"self") {
            return cut_nul(&ce.name);
        }
        if n.eq_ignore_ascii_case(b"parent") {
            if let Some(p) = &ce.parent {
                return cut_nul(&p.name);
            }
        }
    }
    cut_nul(n)
}

/// Anonymous class names contain a NUL byte; messages cut them there.
fn cut_nul(n: &PhpStr) -> String {
    String::from_utf8_lossy(crate::value::numeric::c_str(n)).into_owned()
}

/// Splits on `sep` outside parentheses.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// How a value is being coerced.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoerceMode {
    /// `declare(strict_types=1)` in the calling code.
    pub strict: bool,
    /// Whether deprecations for lossy conversions are emitted (php-src passes
    /// `arg_num == (uint32_t)-1` to suppress them).
    pub silent: bool,
}

impl Vm {
    /// `zend_parse_arg_long_weak` (without the `null` deprecation, which the
    /// caller emits): floats must be integral-ish and in range; strings must be
    /// fully numeric. Emits "Implicit conversion ... loses precision".
    pub fn parse_arg_long_weak(&mut self, v: &Value, silent: bool) -> PResult<Option<i64>> {
        Ok(match v {
            Value::Float(d) => {
                let d = *d;
                if d.is_nan() || !double_fits_long(d) {
                    return Ok(None);
                }
                let l = d as i64;
                if !is_long_compatible(d, l) && !silent {
                    self.engine_deprecated(format!(
                        "Implicit conversion from float {} to int loses precision",
                        float_fmt::format_h(d, -1)
                    ))?;
                }
                Some(l)
            }
            Value::String(s) => match is_numeric_str(s) {
                Some(Numeric::Int(l)) => Some(l),
                Some(Numeric::Float(d)) => {
                    if d.is_nan() || !double_fits_long(d) {
                        return Ok(None);
                    }
                    let l = d as i64;
                    if !is_long_compatible(d, l) && !silent {
                        let mut msg = b"Implicit conversion from float-string \"".to_vec();
                        msg.extend_from_slice(crate::value::numeric::c_str(s));
                        msg.extend_from_slice(b"\" to int loses precision");
                        self.engine_deprecated(msg)?;
                    }
                    Some(l)
                }
                None => None,
            },
            Value::Null | Value::Undef | Value::Bool(false) => Some(0),
            Value::Bool(true) => Some(1),
            Value::Int(i) => Some(*i),
            Value::Ref(r) => {
                let inner = r.get();
                return self.parse_arg_long_weak(&inner, silent);
            }
            _ => None,
        })
    }

    /// `zend_parse_arg_double_weak`.
    pub fn parse_arg_double_weak(&mut self, v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(d) => Some(*d),
            Value::String(s) => is_numeric_str(s).map(Numeric::to_f64),
            Value::Null | Value::Undef | Value::Bool(false) => Some(0.0),
            Value::Bool(true) => Some(1.0),
            Value::Ref(r) => {
                let inner = r.get();
                self.parse_arg_double_weak(&inner)
            }
            _ => None,
        }
    }

    /// `zend_parse_arg_str_weak`: scalars are converted; objects via `__toString`.
    pub fn parse_arg_str_weak(&mut self, v: &Value) -> PResult<Option<PhpStr>> {
        match v {
            Value::String(s) => Ok(Some(s.clone())),
            Value::Null | Value::Undef | Value::Bool(_) | Value::Int(_) | Value::Float(_) => {
                Ok(Some(self.to_php_string(v)?))
            }
            Value::Object(o) => match self.object_cast(o, CastKind::String)? {
                Some(Value::String(s)) => Ok(Some(s)),
                _ => Ok(None),
            },
            Value::Ref(r) => {
                let inner = r.get();
                self.parse_arg_str_weak(&inner)
            }
            _ => Ok(None),
        }
    }

    /// `zend_parse_arg_bool_weak`.
    pub fn parse_arg_bool_weak(&mut self, v: &Value) -> PResult<Option<bool>> {
        match v {
            Value::Null
            | Value::Undef
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::String(_) => Ok(Some(self.to_bool(v)?)),
            Value::Ref(r) => {
                let inner = r.get();
                self.parse_arg_bool_weak(&inner)
            }
            _ => Ok(None),
        }
    }

    /// `zend_verify_weak_scalar_type_hint`: preference order int → float →
    /// string → bool (int|float with a string follows numeric-string rules).
    pub fn verify_weak_scalar(
        &mut self,
        type_mask: u32,
        v: &Value,
        silent: bool,
    ) -> PResult<Option<Value>> {
        if type_mask & mask::INT != 0 {
            if let (true, Value::String(s)) = (type_mask & mask::FLOAT != 0, v) {
                if let Some(n) = is_numeric_str(s) {
                    return Ok(Some(n.into_value()));
                }
            } else if let Some(l) = self.parse_arg_long_weak(v, silent)? {
                return Ok(Some(Value::Int(l)));
            }
        }
        if type_mask & mask::FLOAT != 0 {
            if let Some(d) = self.parse_arg_double_weak(v) {
                if !d.is_nan() || matches!(v, Value::Float(_)) {
                    return Ok(Some(Value::Float(d)));
                }
            }
        }
        if type_mask & mask::STRING != 0 {
            if let Some(s) = self.parse_arg_str_weak(v)? {
                return Ok(Some(Value::String(s)));
            }
        }
        if type_mask & mask::BOOL == mask::BOOL {
            if let Some(b) = self.parse_arg_bool_weak(v)? {
                return Ok(Some(Value::Bool(b)));
            }
        }
        Ok(None)
    }

    /// Checks `v` against `ty`, applying weak/strict scalar coercion
    /// (`zend_check_type` + `zend_verify_scalar_type_hint`). `null` is only
    /// accepted by nullable types here; the internal-function `null`
    /// deprecation path is handled by ZPP. Returns `None` if incompatible.
    pub fn coerce_to_type(
        &mut self,
        ty: &TypeDecl,
        v: Value,
        mode: CoerceMode,
    ) -> PResult<Option<Value>> {
        let v = v.into_unref();
        if ty.matches_exact(&v) {
            return Ok(Some(v));
        }
        if ty.mask & mask::CALLABLE != 0 {
            let host = self.host.clone();
            if host.check_callable(self, &v).is_ok() {
                return Ok(Some(v));
            }
        }
        if matches!(
            v,
            Value::Null | Value::Undef | Value::Array(_) | Value::Resource(_)
        ) {
            return Ok(None);
        }
        if mode.strict {
            if ty.mask & mask::FLOAT != 0 {
                if let Value::Int(i) = v {
                    return Ok(Some(Value::Float(i as f64)));
                }
            }
            return Ok(None);
        }
        if matches!(v, Value::Object(_)) && ty.mask & mask::STRING == 0 {
            return Ok(None);
        }
        if ty.mask & mask::FLOAT != 0 && ty.mask & mask::INT == 0 {
            if let Value::Int(i) = v {
                return Ok(Some(Value::Float(i as f64)));
            }
        }
        self.verify_weak_scalar(ty.mask, &v, mode.silent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> String {
        TypeDecl::parse(s).unwrap().to_type_string()
    }

    #[test]
    fn canonical_type_strings() {
        assert_eq!(ts("int"), "int");
        assert_eq!(ts("?int"), "?int");
        assert_eq!(ts("int|null"), "?int");
        assert_eq!(ts("int|string|null"), "string|int|null");
        assert_eq!(ts("float|array"), "array|float");
        assert_eq!(ts("bool|array"), "array|bool");
        assert_eq!(ts("int|float|false"), "int|float|false");
        assert_eq!(ts("array|string"), "array|string");
        assert_eq!(ts("mixed"), "mixed");
        assert_eq!(ts("iterable"), "Traversable|array");
        assert_eq!(ts("?Throwable"), "?Throwable");
        assert_eq!(ts("Foo|Bar|null"), "Foo|Bar|null");
        assert_eq!(ts("A&B"), "A&B");
        assert_eq!(ts("(A&B)|null"), "(A&B)|null");
        assert_eq!(ts("null"), "null");
        assert_eq!(ts("static"), "static");
        assert_eq!(ts("\\Foo\\Bar"), "Foo\\Bar");
        assert_eq!(ts("true|string"), "string|true");
        assert_eq!(ts("bool|int"), "int|bool");
    }

    #[test]
    fn exact_matching() {
        let t = TypeDecl::parse("?int").unwrap();
        assert!(t.matches_exact(&Value::Int(1)));
        assert!(t.matches_exact(&Value::Null));
        assert!(!t.matches_exact(&Value::Float(1.0)));
        let t = TypeDecl::parse("int|false").unwrap();
        assert!(t.matches_exact(&Value::Bool(false)));
        assert!(!t.matches_exact(&Value::Bool(true)));
        assert!(TypeDecl::mixed().matches_exact(&Value::Null));
    }
}
