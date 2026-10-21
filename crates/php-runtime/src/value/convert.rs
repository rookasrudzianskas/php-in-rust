//! Type conversions (ports of `zval_get_long`, `zval_get_double`,
//! `zval_try_get_string`, `zend_is_true`, `convert_to_*` from `zend_operators.c`).
//!
//! The `*_silent` functions are pure and never emit diagnostics. The `Vm`
//! methods implement the exact PHP 8.5 behaviour including warnings
//! ("Array to string conversion", "unexpected NAN value was coerced to ...",
//! "The float ... is not representable as an int, cast occurred") and object
//! casts through class handlers / `__toString`.

use std::rc::Rc;

use super::array::{ArrayKey, KeyCast, cast_key};
use super::float_fmt;
use super::numeric::{
    Numeric, double_fits_long, dval_to_lval_cap, dval_to_lval_silent, is_long_compatible,
    is_numeric_string_ex, strtod_value,
};
use super::{PhpArray, PhpStr, Value};
use crate::class::{CastKind, PropPurpose, member_flags};
use crate::object::ObjRef;
use crate::vm::{PResult, Vm};

/// `(string)` of an integer.
pub fn long_to_str(i: i64) -> PhpStr {
    PhpStr::from(i.to_string())
}

/// Truthiness without diagnostics (objects are `true`).
pub fn to_bool_silent(v: &Value) -> bool {
    match v {
        Value::Undef | Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        Value::Int(i) => *i != 0,
        Value::Float(d) => *d != 0.0,
        Value::String(s) => !(s.is_empty() || s.as_bytes() == b"0"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(_) => true,
        Value::Resource(r) => r.id() != 0,
        Value::Ref(r) => to_bool_silent(&r.borrow()),
    }
}

/// `zval_get_long` without diagnostics (objects count as 1).
pub fn to_int_silent(v: &Value) -> i64 {
    match v {
        Value::Undef | Value::Null | Value::Bool(false) => 0,
        Value::Bool(true) => 1,
        Value::Int(i) => *i,
        Value::Float(d) => dval_to_lval_silent(*d),
        Value::String(s) => string_to_int(s),
        Value::Array(a) => !a.is_empty() as i64,
        Value::Object(_) => 1,
        Value::Resource(r) => r.id(),
        Value::Ref(r) => to_int_silent(&r.borrow()),
    }
}

/// `(int)"..."`: leading-numeric prefix, float strings saturate (`zend_dval_to_lval_cap`).
pub fn string_to_int(s: &[u8]) -> i64 {
    match is_numeric_string_ex(s, true) {
        None => 0,
        Some(info) => match info.num {
            Numeric::Int(i) => i,
            Numeric::Float(d) => dval_to_lval_cap(d),
        },
    }
}

/// `zval_get_double` without diagnostics.
pub fn to_float_silent(v: &Value) -> f64 {
    match v {
        Value::Undef | Value::Null | Value::Bool(false) => 0.0,
        Value::Bool(true) => 1.0,
        Value::Int(i) => *i as f64,
        Value::Float(d) => *d,
        Value::String(s) => strtod_value(s),
        Value::Array(a) => !a.is_empty() as i64 as f64,
        Value::Object(_) => 1.0,
        Value::Resource(r) => r.id() as f64,
        Value::Ref(r) => to_float_silent(&r.borrow()),
    }
}

/// Context of an array offset conversion (selects the TypeError wording).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffsetKind {
    Read,
    Write,
    IsSet,
    Unset,
}

/// Mangled property-table key (`zend_mangle_property_name`): private →
/// `"\0Class\0name"`, protected → `"\0*\0name"`.
pub fn mangle_property_name(name: &[u8], flags: u32, declaring_class: &[u8]) -> PhpStr {
    let mut v = Vec::with_capacity(name.len() + declaring_class.len() + 2);
    if flags & member_flags::PRIVATE != 0 {
        v.push(0);
        v.extend_from_slice(declaring_class);
        v.push(0);
    } else if flags & member_flags::PROTECTED != 0 {
        v.extend_from_slice(b"\0*\0");
    }
    v.extend_from_slice(name);
    PhpStr::from_vec(v)
}

/// The object's property table with mangled keys (`zend_std_get_properties`):
/// declared slots in order, then dynamic properties.
pub fn object_property_table(obj: &ObjRef) -> PhpArray {
    let mut out = PhpArray::new();
    let props = obj.obj().props.borrow();
    let mut infos: Vec<(usize, PhpStr)> = Vec::new();
    let mut ce = Some(obj.class().clone());
    while let Some(c) = ce {
        for info in c.properties.values() {
            if info.is_static() || infos.iter().any(|(s, _)| *s == info.slot) {
                continue;
            }
            infos.push((
                info.slot,
                mangle_property_name(&info.name, info.flags, &info.declaring_class),
            ));
        }
        ce = c.parent.clone();
    }
    infos.sort_by_key(|(slot, _)| *slot);
    for (slot, key) in infos {
        if let Some(v) = props.get(slot) {
            if !v.is_undef() {
                out.insert(ArrayKey::Str(key), v.clone());
            }
        }
    }
    drop(props);
    if let Some(d) = obj.obj().dyn_props.borrow().as_ref() {
        for (k, v) in d.iter() {
            out.insert(ArrayKey::Str(k.to_php_str()), v.clone());
        }
    }
    out
}

impl Vm {
    /// `zend_is_true`: emits the NaN warning; objects go through `cast_object`.
    pub fn to_bool(&mut self, v: &Value) -> PResult<bool> {
        match v {
            Value::Float(d) => {
                if d.is_nan() {
                    self.nan_coerced("bool")?;
                }
                Ok(*d != 0.0)
            }
            Value::Object(o) => self.object_is_true(o),
            Value::Ref(r) => {
                let inner = r.get();
                self.to_bool(&inner)
            }
            v => Ok(to_bool_silent(v)),
        }
    }

    /// `zend_object_is_true`.
    pub fn object_is_true(&mut self, o: &ObjRef) -> PResult<bool> {
        match self.object_cast(o, CastKind::Bool)? {
            Some(v) => Ok(matches!(v, Value::Bool(true))),
            None => {
                self.zend_error(
                    crate::errors::level::E_RECOVERABLE_ERROR,
                    format!(
                        "Object of class {} could not be converted to bool",
                        o.class().name
                    ),
                )?;
                Ok(false)
            }
        }
    }

    /// "unexpected NAN value was coerced to {type}" (`zend_nan_coerced_to_type_warning`).
    pub fn nan_coerced(&mut self, ty: &str) -> PResult<()> {
        self.engine_warning(format!("unexpected NAN value was coerced to {ty}"))
    }

    /// `zend_dval_to_lval`: NaN/Inf → 0 and out-of-range → modular, with the
    /// "is not representable as an int" warning.
    pub fn dval_to_lval(&mut self, d: f64) -> PResult<i64> {
        if !d.is_finite() || !double_fits_long(d) {
            self.engine_warning(format!(
                "The float {} is not representable as an int, cast occurred",
                float_fmt::format_h(d, -1)
            ))?;
        }
        Ok(dval_to_lval_silent(d))
    }

    /// `zend_dval_to_lval_safe`: also deprecates lossy (fractional) conversions.
    pub fn dval_to_lval_safe(&mut self, d: f64) -> PResult<i64> {
        let l = self.dval_to_lval(d)?;
        if !is_long_compatible(d, l) && double_fits_long(d) {
            self.engine_deprecated(format!(
                "Implicit conversion from float {} to int loses precision",
                float_fmt::format_h(d, -1)
            ))?;
        }
        Ok(l)
    }

    /// `(int)$v` / `intval($v)` (`zval_get_long`).
    pub fn to_int(&mut self, v: &Value) -> PResult<i64> {
        match v {
            Value::Float(d) => self.dval_to_lval(*d),
            Value::Object(o) => match self.object_cast(o, CastKind::Int)? {
                Some(Value::Int(i)) => Ok(i),
                _ => {
                    self.engine_warning(format!(
                        "Object of class {} could not be converted to int",
                        o.class().name
                    ))?;
                    Ok(1)
                }
            },
            Value::Ref(r) => {
                let inner = r.get();
                self.to_int(&inner)
            }
            v => Ok(to_int_silent(v)),
        }
    }

    /// `(float)$v` (`zval_get_double`).
    pub fn to_float(&mut self, v: &Value) -> PResult<f64> {
        match v {
            Value::Object(o) => match self.object_cast(o, CastKind::Float)? {
                Some(Value::Float(d)) => Ok(d),
                _ => {
                    self.engine_warning(format!(
                        "Object of class {} could not be converted to float",
                        o.class().name
                    ))?;
                    Ok(1.0)
                }
            },
            Value::Ref(r) => {
                let inner = r.get();
                self.to_float(&inner)
            }
            v => Ok(to_float_silent(v)),
        }
    }

    /// Float to string with the `precision` ini (`zend_double_to_str`), warning on NaN.
    pub fn float_to_string(&mut self, d: f64) -> PResult<PhpStr> {
        let s = PhpStr::from(float_fmt::double_to_string(d, self.precision));
        if d.is_nan() {
            self.nan_coerced("string")?;
        }
        Ok(s)
    }

    /// `(string)$v` (`zval_try_get_string`): `"Array"` with a warning for
    /// arrays, `__toString` for objects (Error if unsupported).
    pub fn to_php_string(&mut self, v: &Value) -> PResult<PhpStr> {
        match v {
            Value::String(s) => Ok(s.clone()),
            Value::Undef | Value::Null | Value::Bool(false) => Ok(PhpStr::new()),
            Value::Bool(true) => Ok(PhpStr::from("1")),
            Value::Int(i) => Ok(long_to_str(*i)),
            Value::Float(d) => self.float_to_string(*d),
            Value::Array(_) => {
                self.engine_warning("Array to string conversion")?;
                Ok(PhpStr::from("Array"))
            }
            Value::Object(o) => match self.object_cast(o, CastKind::String)? {
                Some(Value::String(s)) => Ok(s),
                _ => Err(self.throw_error(
                    "Error",
                    format!(
                        "Object of class {} could not be converted to string",
                        o.class().name
                    ),
                )),
            },
            Value::Resource(r) => Ok(PhpStr::from(format!("Resource id #{}", r.id()))),
            Value::Ref(r) => {
                let inner = r.get();
                self.to_php_string(&inner)
            }
        }
    }

    /// Converts a value used as an array offset, emitting PHP's diagnostics
    /// (`slow_index_convert` / `zend_illegal_container_offset`).
    pub fn to_array_key(&mut self, v: &Value, kind: OffsetKind) -> PResult<ArrayKey> {
        match cast_key(v) {
            KeyCast::Key(k) => Ok(k),
            KeyCast::Null => {
                self.engine_deprecated(
                    "Using null as an array offset is deprecated, use an empty string instead",
                )?;
                Ok(ArrayKey::Str(PhpStr::new()))
            }
            KeyCast::FloatFractional(_, d) | KeyCast::FloatNotRepresentable(_, d) => {
                Ok(ArrayKey::Int(self.dval_to_lval_safe(d)?))
            }
            KeyCast::Resource(id) => {
                self.engine_warning(format!(
                    "Resource ID#{id} used as offset, casting to integer ({id})"
                ))?;
                Ok(ArrayKey::Int(id))
            }
            KeyCast::Illegal => {
                let ty = v.type_name();
                let msg = match kind {
                    OffsetKind::IsSet => {
                        format!("Cannot access offset of type {ty} in isset or empty")
                    }
                    OffsetKind::Unset => format!("Cannot unset offset of type {ty} on array"),
                    _ => format!("Cannot access offset of type {ty} on array"),
                };
                Err(self.throw_type_error(msg))
            }
        }
    }

    /// `zendi_try_convert_scalar_to_number` (arithmetic operands): `None`
    /// means "unsupported operand" (TypeError by the caller). Leading-numeric
    /// strings warn "A non-numeric value encountered".
    pub fn to_number(&mut self, v: &Value) -> PResult<Option<Numeric>> {
        Ok(match v {
            Value::Int(i) => Some(Numeric::Int(*i)),
            Value::Float(d) => Some(Numeric::Float(*d)),
            Value::Null | Value::Undef | Value::Bool(false) => Some(Numeric::Int(0)),
            Value::Bool(true) => Some(Numeric::Int(1)),
            Value::String(s) => match is_numeric_string_ex(s, true) {
                None => None,
                Some(info) => {
                    if info.trailing_data {
                        self.engine_warning("A non-numeric value encountered")?;
                    }
                    Some(info.num)
                }
            },
            Value::Object(o) => match self.object_cast(o, CastKind::Number)? {
                Some(Value::Int(i)) => Some(Numeric::Int(i)),
                Some(Value::Float(d)) => Some(Numeric::Float(d)),
                _ => None,
            },
            Value::Array(_) | Value::Resource(_) => None,
            Value::Ref(r) => {
                let inner = r.get();
                return self.to_number(&inner);
            }
        })
    }

    /// `zendi_try_get_long` (operands of `%`, `<<`, `>>`, `|`, `&`, `^`, `~`):
    /// `None` means "unsupported operand".
    pub fn try_get_long(&mut self, v: &Value) -> PResult<Option<i64>> {
        Ok(match v {
            Value::Int(i) => Some(*i),
            Value::Null | Value::Undef | Value::Bool(false) => Some(0),
            Value::Bool(true) => Some(1),
            Value::Float(d) => Some(self.dval_to_lval_safe(*d)?),
            Value::String(s) => match is_numeric_string_ex(s, true) {
                None => None,
                Some(info) => {
                    if info.trailing_data {
                        self.engine_warning("A non-numeric value encountered")?;
                    }
                    match info.num {
                        Numeric::Int(i) => Some(i),
                        Numeric::Float(d) => {
                            let l = dval_to_lval_cap(d);
                            if !is_long_compatible(d, l) {
                                let mut msg = b"Implicit conversion from float-string \"".to_vec();
                                msg.extend_from_slice(super::numeric::c_str(s));
                                msg.extend_from_slice(b"\" to int loses precision");
                                self.engine_deprecated(msg)?;
                            }
                            Some(l)
                        }
                    }
                }
            },
            Value::Object(o) => match self.object_cast(o, CastKind::Int)? {
                Some(Value::Int(i)) => Some(i),
                _ => None,
            },
            Value::Array(_) | Value::Resource(_) => None,
            Value::Ref(r) => {
                let inner = r.get();
                return self.try_get_long(&inner);
            }
        })
    }

    /// `(array)$v` (`convert_to_array`).
    pub fn cast_to_array(&mut self, v: &Value) -> PResult<PhpArray> {
        match v {
            Value::Array(a) => Ok((**a).clone()),
            Value::Undef | Value::Null => Ok(PhpArray::new()),
            Value::Object(o) => {
                if o.class().lc_name.as_bytes() == b"closure" {
                    return Ok(PhpArray::from_values(vec![v.clone()]));
                }
                if let Some(f) = o.class().handlers.get_properties_for {
                    if let Some(t) = f(self, o, PropPurpose::ArrayCast)? {
                        return Ok(proptable_to_symtable(&t));
                    }
                }
                Ok(proptable_to_symtable(&object_property_table(o)))
            }
            Value::Ref(r) => {
                let inner = r.get();
                self.cast_to_array(&inner)
            }
            Value::Float(d) => {
                if d.is_nan() {
                    self.nan_coerced("array")?;
                }
                Ok(PhpArray::from_values(vec![v.clone()]))
            }
            v => Ok(PhpArray::from_values(vec![v.clone()])),
        }
    }

    /// `(object)$v` (`convert_to_object`): arrays become `stdClass` properties,
    /// scalars are stored in `->scalar`.
    pub fn cast_to_object(&mut self, v: &Value) -> PResult<ObjRef> {
        let std = self
            .find_class(b"stdClass")
            .expect("stdClass is registered");
        match v {
            Value::Object(o) => Ok(o.clone()),
            Value::Ref(r) => {
                let inner = r.get();
                self.cast_to_object(&inner)
            }
            Value::Undef | Value::Null => Ok(ObjRef::new(std)),
            Value::Array(a) => {
                let obj = ObjRef::new(std);
                let mut props = PhpArray::new();
                for (k, v) in a.iter() {
                    props.insert(ArrayKey::Str(k.to_php_str()), v.clone());
                }
                *obj.obj().dyn_props.borrow_mut() = Some(props);
                Ok(obj)
            }
            v => {
                if let Value::Float(d) = v {
                    if d.is_nan() {
                        self.nan_coerced("object")?;
                    }
                }
                let obj = ObjRef::new(std);
                obj.set_prop(b"scalar", v.clone());
                Ok(obj)
            }
        }
    }

    /// `settype($v, "null")` / `(unset)`-style conversion diagnostics.
    pub fn cast_to_null(&mut self, v: &Value) -> PResult<()> {
        if let Value::Float(d) = &*v.deref() {
            if d.is_nan() {
                self.nan_coerced("null")?;
            }
        }
        Ok(())
    }
}

/// `zend_proptable_to_symtable`: numeric string keys become integer keys.
pub fn proptable_to_symtable(t: &PhpArray) -> PhpArray {
    let mut out = PhpArray::with_capacity(t.len());
    for (k, v) in t.iter() {
        let k = match k {
            ArrayKey::Str(s) => ArrayKey::from_php_str(s),
            k => k,
        };
        out.insert(k, v.clone());
    }
    out
}

/// Wraps an array in a fresh handle (`Rc`).
pub fn array_value(a: PhpArray) -> Value {
    Value::Array(Rc::new(a))
}
