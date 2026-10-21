//! Operators (port of `zend_operators.c`): arithmetic, bitwise, concatenation,
//! comparison, identity and increment/decrement with PHP 8.5 semantics.
//!
//! All operators take dereferenced or reference values and return a new
//! value; diagnostics and exceptions go through the `Vm`. Objects first get
//! a chance through their class's `do_operation` handler (op1 first, then op2
//! — but op2 is not tried if op1 is an object with a `do_operation` handler).

use std::cmp::Ordering;
use std::rc::Rc;

use super::array::PhpArray;
use super::convert::long_to_str;
use super::float_fmt;
use super::numeric::{Numeric, is_numeric_str, is_numeric_string_ex};
use super::{PhpArrayRef, PhpStr, Value};
use crate::class::{CastKind, OpKind};
use crate::vm::{Flow, PResult, Vm};

/// `ZEND_THREEWAY_COMPARE` for doubles (NaN compares as 1).
#[inline]
fn cmp_f64(a: f64, b: f64) -> i32 {
    if a == b {
        0
    } else if a < b {
        -1
    } else {
        1
    }
}

#[inline]
fn cmp_i64(a: i64, b: i64) -> i32 {
    match a.cmp(&b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

#[inline]
fn normalize(i: i32) -> i32 {
    i.signum()
}

fn bytes_cmp(a: &[u8], b: &[u8]) -> i32 {
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// `zend_is_identical` (`===`), looking through references. Recursive
/// arrays compare as not identical here; [`Vm::is_identical`] throws
/// "Nesting level too deep" like php-src.
pub fn is_identical(a: &Value, b: &Value) -> bool {
    identical_guarded(a, b, &mut Vec::new()).unwrap_or(false)
}

/// `Err(())` signals recursion (`GC_IS_RECURSIVE` in `zend_hash_compare`).
fn identical_guarded(a: &Value, b: &Value, guard: &mut Vec<usize>) -> Result<bool, ()> {
    let a = a.deref();
    let b = b.deref();
    Ok(match (&*a, &*b) {
        (Value::Undef | Value::Null, Value::Undef | Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y) || arrays_identical(x, y, guard)?,
        (Value::Object(x), Value::Object(y)) => crate::object::ObjRef::ptr_eq(x, y),
        (Value::Resource(x), Value::Resource(y)) => super::ResRef::ptr_eq(x, y),
        _ => false,
    })
}

fn arrays_identical(a: &PhpArrayRef, b: &PhpArrayRef, guard: &mut Vec<usize>) -> Result<bool, ()> {
    let ptr = Rc::as_ptr(a) as usize;
    if guard.contains(&ptr) {
        return Err(());
    }
    if a.len() != b.len() {
        return Ok(false);
    }
    guard.push(ptr);
    let mut r = Ok(true);
    for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
        if ka != kb {
            r = Ok(false);
            break;
        }
        match identical_guarded(va, vb, guard) {
            Ok(true) => {}
            other => {
                r = other;
                break;
            }
        }
    }
    guard.pop();
    r
}

/// `zendi_smart_streq`: numeric strings compare numerically.
pub fn smart_str_eq(s1: &[u8], s2: &[u8]) -> bool {
    if let (Some(i1), Some(i2)) = (
        is_numeric_string_ex(s1, false),
        is_numeric_string_ex(s2, false),
    ) {
        let (o1, o2) = (i1.overflow, i2.overflow);
        let d1 = i1.num.to_f64();
        let d2 = i2.num.to_f64();
        if o1 != 0 && o1 == o2 && d1 - d2 == 0.0 {
            return s1 == s2;
        }
        match (i1.num, i2.num) {
            (Numeric::Int(l1), Numeric::Int(l2)) => l1 == l2,
            (Numeric::Int(l1), Numeric::Float(d2)) => {
                if o2 != 0 {
                    return false;
                }
                (l1 as f64) == d2
            }
            (Numeric::Float(d1), Numeric::Int(l2)) => {
                if o1 != 0 {
                    return false;
                }
                d1 == l2 as f64
            }
            (Numeric::Float(d1), Numeric::Float(d2)) => {
                if d1 == d2 && !d1.is_finite() {
                    return s1 == s2;
                }
                d1 == d2
            }
        }
    } else {
        s1 == s2
    }
}

/// `zendi_smart_strcmp`.
pub fn smart_str_cmp(s1: &[u8], s2: &[u8]) -> i32 {
    if let (Some(i1), Some(i2)) = (
        is_numeric_string_ex(s1, false),
        is_numeric_string_ex(s2, false),
    ) {
        let (o1, o2) = (i1.overflow, i2.overflow);
        let d1 = i1.num.to_f64();
        let d2 = i2.num.to_f64();
        if o1 != 0 && o1 == o2 && d1 - d2 == 0.0 {
            return normalize(bytes_cmp(s1, s2));
        }
        match (i1.num, i2.num) {
            (Numeric::Int(l1), Numeric::Int(l2)) => cmp_i64(l1, l2),
            (Numeric::Int(l1), Numeric::Float(d2)) => {
                if o2 != 0 {
                    return -(o2 as i32);
                }
                float_sign(l1 as f64 - d2)
            }
            (Numeric::Float(d1), Numeric::Int(l2)) => {
                if o1 != 0 {
                    return o1 as i32;
                }
                float_sign(d1 - l2 as f64)
            }
            (Numeric::Float(d1), Numeric::Float(d2)) => {
                if d1 == d2 && !d1.is_finite() {
                    return normalize(bytes_cmp(s1, s2));
                }
                float_sign(d1 - d2)
            }
        }
    } else {
        normalize(bytes_cmp(s1, s2))
    }
}

/// `ZEND_NORMALIZE_BOOL` for doubles (NaN → 0).
fn float_sign(d: f64) -> i32 {
    if d > 0.0 {
        1
    } else if d < 0.0 {
        -1
    } else {
        0
    }
}

/// Increments an alphanumeric string Perl-style (`increment_string`): `"a"`→`"b"`,
/// `"Az"`→`"Ba"`, `"a9"`→`"b0"`, `"zz"`→`"aaa"`, `""`→`"1"`.
pub fn increment_string(s: &[u8]) -> PhpStr {
    if s.is_empty() {
        return PhpStr::from("1");
    }
    #[derive(PartialEq)]
    enum Last {
        None,
        Lower,
        Upper,
        Numeric,
    }
    let mut v = s.to_vec();
    let mut pos = v.len() - 1;
    let mut carry;
    let mut last = Last::None;
    loop {
        let ch = v[pos];
        match ch {
            b'a'..=b'z' => {
                if ch == b'z' {
                    v[pos] = b'a';
                    carry = true;
                } else {
                    v[pos] += 1;
                    carry = false;
                }
                last = Last::Lower;
            }
            b'A'..=b'Z' => {
                if ch == b'Z' {
                    v[pos] = b'A';
                    carry = true;
                } else {
                    v[pos] += 1;
                    carry = false;
                }
                last = Last::Upper;
            }
            b'0'..=b'9' => {
                if ch == b'9' {
                    v[pos] = b'0';
                    carry = true;
                } else {
                    v[pos] += 1;
                    carry = false;
                }
                last = Last::Numeric;
            }
            _ => {
                carry = false;
                break;
            }
        }
        if !carry || pos == 0 {
            break;
        }
        pos -= 1;
    }
    if carry {
        let first = match last {
            Last::Numeric => b'1',
            Last::Upper => b'A',
            Last::Lower => b'a',
            Last::None => return PhpStr::from_vec(v),
        };
        v.insert(0, first);
    }
    PhpStr::from_vec(v)
}

/// Array union (`+`): elements of `b` whose keys are missing from `a` are
/// appended (`add_function_array` + `zend_hash_merge` without overwrite).
pub fn array_union(a: &PhpArrayRef, b: &PhpArrayRef) -> PhpArray {
    let mut r = (**a).clone();
    for (k, v) in b.iter() {
        if !r.contains_key(&k) {
            let v = match v {
                Value::Ref(rf) if rf.refcount() == 1 => rf.get(),
                v => v.clone(),
            };
            r.insert(k, v);
        }
    }
    r
}

fn bytes_bitwise(a: &[u8], b: &[u8], op: u8) -> PhpStr {
    let (longer, shorter) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out: Vec<u8> = shorter
        .iter()
        .zip(longer.iter())
        .map(|(x, y)| match op {
            b'|' => x | y,
            b'&' => x & y,
            _ => x ^ y,
        })
        .collect();
    if op == b'|' {
        out.extend_from_slice(&longer[shorter.len()..]);
    }
    PhpStr::from_vec(out)
}

/// `pow()` for doubles, deprecating `0 ** negative` (`safe_pow`).
fn safe_pow(vm: &mut Vm, base: f64, exp: f64) -> PResult<f64> {
    if base == 0.0 && exp < 0.0 {
        vm.engine_deprecated("Power of base 0 and negative exponent is deprecated")?;
    }
    Ok(base.powf(exp))
}

impl Vm {
    fn binop_error(&mut self, op: &str, a: &Value, b: &Value) -> Flow {
        let msg = format!(
            "Unsupported operand types: {} {op} {}",
            a.type_name(),
            b.type_name()
        );
        self.throw_type_error(msg)
    }

    /// `ZEND_TRY_BINARY_OBJECT_OPERATION`.
    fn try_object_op(&mut self, op: OpKind, a: &Value, b: &Value) -> PResult<Option<Value>> {
        if let Value::Object(o) = a {
            if let Some(f) = o.class().handlers.do_operation {
                return f(self, op, a, b);
            }
        }
        if let Value::Object(o) = b {
            if let Some(f) = o.class().handlers.do_operation {
                return f(self, op, a, b);
            }
        }
        Ok(None)
    }

    fn arith_operands(
        &mut self,
        sigil: &str,
        op: OpKind,
        a: &Value,
        b: &Value,
    ) -> PResult<Result<(Numeric, Numeric), Value>> {
        if let Some(r) = self.try_object_op(op, a, b)? {
            return Ok(Err(r));
        }
        let Some(na) = self.to_number(a)? else {
            return Err(self.binop_error(sigil, a, b));
        };
        let Some(nb) = self.to_number(b)? else {
            return Err(self.binop_error(sigil, a, b));
        };
        Ok(Ok((na, nb)))
    }

    /// `+`
    pub fn add(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => Ok(add_int(*x, *y)),
            (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x + y)),
            (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 + y)),
            (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x + *y as f64)),
            (Value::Array(x), Value::Array(y)) => Ok(Value::array(array_union(x, y))),
            _ => match self.arith_operands("+", OpKind::Add, &a, &b)? {
                Err(v) => Ok(v),
                Ok((x, y)) => Ok(num_binop(x, y, add_int, |p, q| p + q)),
            },
        }
    }

    /// `-`
    pub fn sub(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => Ok(sub_int(*x, *y)),
            (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x - y)),
            (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 - y)),
            (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x - *y as f64)),
            _ => match self.arith_operands("-", OpKind::Sub, &a, &b)? {
                Err(v) => Ok(v),
                Ok((x, y)) => Ok(num_binop(x, y, sub_int, |p, q| p - q)),
            },
        }
    }

    /// `*`
    pub fn mul(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => Ok(mul_int(*x, *y)),
            (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x * y)),
            (Value::Int(x), Value::Float(y)) => Ok(Value::Float(*x as f64 * y)),
            (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x * *y as f64)),
            _ => match self.arith_operands("*", OpKind::Mul, &a, &b)? {
                Err(v) => Ok(v),
                Ok((x, y)) => Ok(num_binop(x, y, mul_int, |p, q| p * q)),
            },
        }
    }

    /// Unary minus (compiled as `$x * -1`).
    pub fn negate(&mut self, a: &Value) -> PResult<Value> {
        self.mul(a, &Value::Int(-1))
    }

    /// Unary plus (compiled as `$x * 1`).
    pub fn unary_plus(&mut self, a: &Value) -> PResult<Value> {
        self.mul(a, &Value::Int(1))
    }

    /// `/`
    pub fn div(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        if let Some(r) = self.div_base(&a, &b)? {
            return Ok(r);
        }
        if let Some(r) = self.try_object_op(OpKind::Div, &a, &b)? {
            return Ok(r);
        }
        let Some(x) = self.to_number(&a)? else {
            return Err(self.binop_error("/", &a, &b));
        };
        let Some(y) = self.to_number(&b)? else {
            return Err(self.binop_error("/", &a, &b));
        };
        match self.div_base(&x.into_value(), &y.into_value())? {
            Some(r) => Ok(r),
            None => unreachable!("numbers are handled"),
        }
    }

    fn div_base(&mut self, a: &Value, b: &Value) -> PResult<Option<Value>> {
        let r = match (a, b) {
            (Value::Int(x), Value::Int(y)) => {
                if *y == 0 {
                    return Err(self.throw_division_by_zero("Division by zero"));
                }
                if *y == -1 && *x == i64::MIN {
                    Value::Float(i64::MIN as f64 / -1.0)
                } else if x % y == 0 {
                    Value::Int(x / y)
                } else {
                    Value::Float(*x as f64 / *y as f64)
                }
            }
            (Value::Float(x), Value::Float(y)) => {
                if *y == 0.0 {
                    return Err(self.throw_division_by_zero("Division by zero"));
                }
                Value::Float(x / y)
            }
            (Value::Float(x), Value::Int(y)) => {
                if *y == 0 {
                    return Err(self.throw_division_by_zero("Division by zero"));
                }
                Value::Float(x / *y as f64)
            }
            (Value::Int(x), Value::Float(y)) => {
                if *y == 0.0 {
                    return Err(self.throw_division_by_zero("Division by zero"));
                }
                Value::Float(*x as f64 / y)
            }
            _ => return Ok(None),
        };
        Ok(Some(r))
    }

    /// `convert_op1_op2_long`: integer operands for `%`, `<<`, `>>`.
    fn long_operands(
        &mut self,
        sigil: &str,
        op: OpKind,
        a: &Value,
        b: &Value,
    ) -> PResult<Result<(i64, i64), Value>> {
        let x = match a {
            Value::Int(i) => *i,
            _ => {
                if let Value::Object(o) = a {
                    if let Some(f) = o.class().handlers.do_operation {
                        if let Some(r) = f(self, op, a, b)? {
                            return Ok(Err(r));
                        }
                    }
                }
                match self.try_get_long(a)? {
                    Some(i) => i,
                    None => return Err(self.binop_error(sigil, a, b)),
                }
            }
        };
        let y = match b {
            Value::Int(i) => *i,
            _ => {
                if let Value::Object(o) = b {
                    if let Some(f) = o.class().handlers.do_operation {
                        if let Some(r) = f(self, op, a, b)? {
                            return Ok(Err(r));
                        }
                    }
                }
                match self.try_get_long(b)? {
                    Some(i) => i,
                    None => return Err(self.binop_error(sigil, a, b)),
                }
            }
        };
        Ok(Ok((x, y)))
    }

    /// `%`
    pub fn modulo(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        let (x, y) = match self.long_operands("%", OpKind::Mod, &a, &b)? {
            Ok(p) => p,
            Err(v) => return Ok(v),
        };
        if y == 0 {
            return Err(self.throw_division_by_zero("Modulo by zero"));
        }
        if y == -1 {
            return Ok(Value::Int(0));
        }
        Ok(Value::Int(x % y))
    }

    /// `**`
    pub fn pow(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        if let Some(r) = self.pow_base(&a, &b)? {
            return Ok(r);
        }
        if let Some(r) = self.try_object_op(OpKind::Pow, &a, &b)? {
            return Ok(r);
        }
        let Some(x) = self.to_number(&a)? else {
            return Err(self.binop_error("**", &a, &b));
        };
        let Some(y) = self.to_number(&b)? else {
            return Err(self.binop_error("**", &a, &b));
        };
        Ok(self
            .pow_base(&x.into_value(), &y.into_value())?
            .expect("numbers"))
    }

    fn pow_base(&mut self, a: &Value, b: &Value) -> PResult<Option<Value>> {
        Ok(Some(match (a, b) {
            (Value::Int(base), Value::Int(exp)) => {
                if *exp >= 0 {
                    let (mut l1, mut l2, mut i) = (1i64, *base, *exp);
                    if i == 0 {
                        return Ok(Some(Value::Int(1)));
                    } else if l2 == 0 {
                        return Ok(Some(Value::Int(0)));
                    }
                    while i >= 1 {
                        if i % 2 != 0 {
                            i -= 1;
                            match l1.checked_mul(l2) {
                                Some(p) => l1 = p,
                                None => {
                                    let d = l1 as f64 * l2 as f64;
                                    let p = safe_pow(self, l2 as f64, i as f64)?;
                                    return Ok(Some(Value::Float(d * p)));
                                }
                            }
                        } else {
                            i /= 2;
                            match l2.checked_mul(l2) {
                                Some(p) => l2 = p,
                                None => {
                                    let d = l2 as f64 * l2 as f64;
                                    let p = safe_pow(self, d, i as f64)?;
                                    return Ok(Some(Value::Float(l1 as f64 * p)));
                                }
                            }
                        }
                    }
                    Value::Int(l1)
                } else {
                    Value::Float(safe_pow(self, *base as f64, *exp as f64)?)
                }
            }
            (Value::Float(x), Value::Float(y)) => Value::Float(safe_pow(self, *x, *y)?),
            (Value::Int(x), Value::Float(y)) => Value::Float(safe_pow(self, *x as f64, *y)?),
            (Value::Float(x), Value::Int(y)) => Value::Float(safe_pow(self, *x, *y as f64)?),
            _ => return Ok(None),
        }))
    }

    /// `intdiv($a, $b)`.
    pub fn intdiv(&mut self, a: i64, b: i64) -> PResult<i64> {
        if b == 0 {
            return Err(self.throw_division_by_zero("Division by zero"));
        }
        if b == -1 && a == i64::MIN {
            return Err(
                self.throw_arithmetic_error("Division of PHP_INT_MIN by -1 is not an integer")
            );
        }
        Ok(a / b)
    }

    /// `<<`
    pub fn shift_left(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        let (x, y) = match self.long_operands("<<", OpKind::ShiftLeft, &a, &b)? {
            Ok(p) => p,
            Err(v) => return Ok(v),
        };
        if (y as u64) >= 64 {
            if y > 0 {
                return Ok(Value::Int(0));
            }
            return Err(self.throw_arithmetic_error("Bit shift by negative number"));
        }
        Ok(Value::Int(((x as u64) << y) as i64))
    }

    /// `>>`
    pub fn shift_right(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        let (x, y) = match self.long_operands(">>", OpKind::ShiftRight, &a, &b)? {
            Ok(p) => p,
            Err(v) => return Ok(v),
        };
        if (y as u64) >= 64 {
            if y > 0 {
                return Ok(Value::Int(if x < 0 { -1 } else { 0 }));
            }
            return Err(self.throw_arithmetic_error("Bit shift by negative number"));
        }
        Ok(Value::Int(x >> y))
    }

    fn bitwise(&mut self, a: &Value, b: &Value, sigil: u8, op: OpKind) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        let f = |x: i64, y: i64| match sigil {
            b'|' => x | y,
            b'&' => x & y,
            _ => x ^ y,
        };
        match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => return Ok(Value::Int(f(*x, *y))),
            (Value::String(x), Value::String(y)) => {
                return Ok(Value::String(bytes_bitwise(x, y, sigil)));
            }
            _ => {}
        }
        let s = (sigil as char).to_string();
        let x = match &a {
            Value::Int(i) => *i,
            _ => {
                if let Value::Object(o) = &a {
                    if let Some(h) = o.class().handlers.do_operation {
                        if let Some(r) = h(self, op, &a, &b)? {
                            return Ok(r);
                        }
                    }
                }
                match self.try_get_long(&a)? {
                    Some(i) => i,
                    None => return Err(self.binop_error(&s, &a, &b)),
                }
            }
        };
        let y = match &b {
            Value::Int(i) => *i,
            _ => {
                if let Value::Object(o) = &b {
                    if let Some(h) = o.class().handlers.do_operation {
                        if let Some(r) = h(self, op, &a, &b)? {
                            return Ok(r);
                        }
                    }
                }
                match self.try_get_long(&b)? {
                    Some(i) => i,
                    None => return Err(self.binop_error(&s, &a, &b)),
                }
            }
        };
        Ok(Value::Int(f(x, y)))
    }

    /// `|`
    pub fn bitwise_or(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        self.bitwise(a, b, b'|', OpKind::BitOr)
    }

    /// `&`
    pub fn bitwise_and(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        self.bitwise(a, b, b'&', OpKind::BitAnd)
    }

    /// `^`
    pub fn bitwise_xor(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        self.bitwise(a, b, b'^', OpKind::BitXor)
    }

    /// `~`
    pub fn bitwise_not(&mut self, a: &Value) -> PResult<Value> {
        let a = a.unref();
        match &a {
            Value::Int(i) => Ok(Value::Int(!i)),
            Value::Float(d) => Ok(Value::Int(!self.dval_to_lval_safe(*d)?)),
            Value::String(s) => Ok(Value::String(PhpStr::from_vec(
                s.iter().map(|c| !c).collect(),
            ))),
            _ => {
                if let Value::Object(o) = &a {
                    if let Some(h) = o.class().handlers.do_operation {
                        if let Some(r) = h(self, OpKind::BitNot, &a, &Value::Null)? {
                            return Ok(r);
                        }
                    }
                }
                let msg = format!("Cannot perform bitwise not on {}", a.value_name());
                Err(self.throw_type_error(msg))
            }
        }
    }

    /// `xor`
    pub fn bool_xor(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        let x = match &a {
            Value::Bool(x) => *x,
            _ => {
                if let Value::Object(o) = &a {
                    if let Some(h) = o.class().handlers.do_operation {
                        if let Some(r) = h(self, OpKind::BoolXor, &a, &b)? {
                            return Ok(r);
                        }
                    }
                }
                self.to_bool(&a)?
            }
        };
        let y = match &b {
            Value::Bool(y) => *y,
            _ => {
                if let Value::Object(o) = &b {
                    if let Some(h) = o.class().handlers.do_operation {
                        if let Some(r) = h(self, OpKind::BoolXor, &a, &b)? {
                            return Ok(r);
                        }
                    }
                }
                self.to_bool(&b)?
            }
        };
        Ok(Value::Bool(x ^ y))
    }

    /// `!`
    pub fn bool_not(&mut self, a: &Value) -> PResult<Value> {
        let a = a.unref();
        if let Value::Object(o) = &a {
            if let Some(h) = o.class().handlers.do_operation {
                if let Some(r) = h(self, OpKind::BoolNot, &a, &Value::Null)? {
                    return Ok(r);
                }
            }
        }
        Ok(Value::Bool(!self.to_bool(&a)?))
    }

    /// `.`
    pub fn concat(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        let (a, b) = (a.unref(), b.unref());
        let sa = match &a {
            Value::String(s) => s.clone(),
            _ => {
                if let Some(r) = self.try_object_op(OpKind::Concat, &a, &b)? {
                    return Ok(r);
                }
                self.to_php_string(&a)?
            }
        };
        let sb = match &b {
            Value::String(s) => s.clone(),
            _ => {
                if let Value::Object(o) = &b {
                    if let Some(h) = o.class().handlers.do_operation {
                        if let Some(r) = h(self, OpKind::Concat, &a, &b)? {
                            return Ok(r);
                        }
                    }
                }
                self.to_php_string(&b)?
            }
        };
        if sa.is_empty() {
            return Ok(Value::String(sb));
        }
        if sb.is_empty() {
            return Ok(Value::String(sa));
        }
        Ok(Value::String(sa.concat(&sb)))
    }

    /// `$s .= $x` on a string slot: appends in place when the buffer is
    /// uniquely owned (amortised O(1)).
    pub fn concat_assign(&mut self, target: &mut Value, b: &Value) -> PResult<()> {
        if let Value::Ref(r) = target {
            let r = r.clone();
            // Fast path without user code: append in place inside the reference.
            if let Value::String(rhs) = b.unref() {
                if let Value::String(s) = &mut *r.borrow_mut() {
                    s.push_bytes(&rhs);
                    return Ok(());
                }
            }
            let mut cur = r.get();
            self.concat_assign(&mut cur, b)?;
            r.set(cur);
            return Ok(());
        }
        let rhs = match &*b.deref() {
            Value::String(s) => Some(s.clone()),
            _ => None,
        };
        let rhs = match rhs {
            Some(s) => s,
            None => {
                let v = self.concat(target, b)?;
                *target = v;
                return Ok(());
            }
        };
        match target {
            Value::String(s) => {
                s.push_bytes(&rhs);
                Ok(())
            }
            _ => {
                let v = self.concat(target, &Value::String(rhs))?;
                *target = v;
                Ok(())
            }
        }
    }

    // ----- comparison --------------------------------------------------------

    /// `zend_compare`: -1, 0 or 1 (`1` also means "uncomparable").
    pub fn compare(&mut self, a: &Value, b: &Value) -> PResult<i32> {
        let mut a = a.unref();
        let mut b = b.unref();
        let mut converted = false;
        loop {
            let r = match (&a, &b) {
                (Value::Int(x), Value::Int(y)) => cmp_i64(*x, *y),
                (Value::Float(x), Value::Int(y)) => cmp_f64(*x, *y as f64),
                (Value::Int(x), Value::Float(y)) => cmp_f64(*x as f64, *y),
                (Value::Float(x), Value::Float(y)) => cmp_f64(*x, *y),
                (Value::Array(x), Value::Array(y)) => return self.compare_arrays(x, y),
                (Value::Null | Value::Undef, Value::Null | Value::Undef)
                | (Value::Null | Value::Undef, Value::Bool(false))
                | (Value::Bool(false), Value::Null | Value::Undef) => 0,
                (Value::Bool(x), Value::Bool(y)) => cmp_i64(*x as i64, *y as i64),
                (Value::Null | Value::Undef, Value::Bool(true)) => -1,
                (Value::Bool(true), Value::Null | Value::Undef) => 1,
                (Value::String(x), Value::String(y)) => {
                    if PhpStr::ptr_eq(x, y) {
                        0
                    } else {
                        smart_str_cmp(x, y)
                    }
                }
                (Value::Null | Value::Undef, Value::String(y)) => {
                    if y.is_empty() {
                        0
                    } else {
                        -1
                    }
                }
                (Value::String(x), Value::Null | Value::Undef) => {
                    if x.is_empty() {
                        0
                    } else {
                        1
                    }
                }
                (Value::Int(x), Value::String(y)) => self.compare_long_to_string(*x, y)?,
                (Value::String(x), Value::Int(y)) => -self.compare_long_to_string(*y, x)?,
                (Value::Float(x), Value::String(y)) => {
                    if x.is_nan() {
                        1
                    } else {
                        self.compare_double_to_string(*x, y)?
                    }
                }
                (Value::String(x), Value::Float(y)) => {
                    if y.is_nan() {
                        1
                    } else {
                        -self.compare_double_to_string(*y, x)?
                    }
                }
                (Value::Object(_), Value::Null | Value::Undef) => 1,
                (Value::Null | Value::Undef, Value::Object(_)) => -1,
                _ => {
                    if matches!(a, Value::Object(_)) || matches!(b, Value::Object(_)) {
                        return self.compare_objects(&a, &b);
                    }
                    if !converted {
                        let a_nan = matches!(a, Value::Float(d) if d.is_nan());
                        let b_nan = matches!(b, Value::Float(d) if d.is_nan());
                        let is_falsy = |v: &Value| {
                            matches!(v, Value::Null | Value::Undef | Value::Bool(false))
                        };
                        if a_nan || b_nan {
                            if is_falsy(&a) {
                                return Ok(-1);
                            } else if matches!(a, Value::Bool(true))
                                || matches!(b, Value::Bool(true))
                            {
                                return Ok(0);
                            } else if is_falsy(&b) {
                                return Ok(1);
                            } else if !matches!(a, Value::Float(_)) {
                                a = scalar_to_number_silent(self, &a)?;
                                converted = true;
                            } else if !matches!(b, Value::Float(_)) {
                                b = scalar_to_number_silent(self, &b)?;
                                converted = true;
                            }
                        } else if is_falsy(&a) {
                            return Ok(if self.to_bool(&b)? { -1 } else { 0 });
                        } else if matches!(a, Value::Bool(true)) {
                            return Ok(if self.to_bool(&b)? { 0 } else { 1 });
                        } else if is_falsy(&b) {
                            return Ok(if self.to_bool(&a)? { 1 } else { 0 });
                        } else if matches!(b, Value::Bool(true)) {
                            return Ok(if self.to_bool(&a)? { 0 } else { -1 });
                        } else {
                            a = scalar_to_number_silent(self, &a)?;
                            b = scalar_to_number_silent(self, &b)?;
                            converted = true;
                        }
                        continue;
                    } else if matches!(a, Value::Array(_)) {
                        1
                    } else if matches!(b, Value::Array(_)) {
                        -1
                    } else {
                        return Err(self.throw_error("Error", "Unsupported operand types"));
                    }
                }
            };
            return Ok(r);
        }
    }

    fn compare_long_to_string(&mut self, l: i64, s: &PhpStr) -> PResult<i32> {
        Ok(match is_numeric_string_ex(s, false).map(|i| i.num) {
            Some(Numeric::Int(sl)) => cmp_i64(l, sl),
            Some(Numeric::Float(sd)) => cmp_f64(l as f64, sd),
            None => normalize(bytes_cmp(long_to_str(l).as_bytes(), s)),
        })
    }

    fn compare_double_to_string(&mut self, d: f64, s: &PhpStr) -> PResult<i32> {
        Ok(match is_numeric_string_ex(s, false).map(|i| i.num) {
            Some(Numeric::Int(sl)) => cmp_f64(d, sl as f64),
            Some(Numeric::Float(sd)) => cmp_f64(d, sd),
            None => {
                let ds = float_fmt::double_to_string(d, self.precision);
                normalize(bytes_cmp(ds.as_bytes(), s))
            }
        })
    }

    /// `zend_compare_arrays` (`zend_hash_compare`, unordered): sizes first,
    /// then every key of `a` must exist in `b` (else uncomparable = 1).
    pub fn compare_arrays(&mut self, a: &PhpArrayRef, b: &PhpArrayRef) -> PResult<i32> {
        if Rc::ptr_eq(a, b) {
            return Ok(0);
        }
        let ptr = Rc::as_ptr(a) as usize;
        if self.compare_guard.contains(&ptr) {
            return Err(self.throw_error("Error", "Nesting level too deep - recursive dependency?"));
        }
        if a.len() != b.len() {
            return Ok(if a.len() > b.len() { 1 } else { -1 });
        }
        self.compare_guard.push(ptr);
        let r = (|| {
            for (k, v1) in a.iter() {
                let Some(v2) = b.get(&k) else {
                    return Ok(1);
                };
                let c = self.compare(v1, v2)?;
                if c != 0 {
                    return Ok(c);
                }
            }
            Ok(0)
        })();
        self.compare_guard.retain(|p| *p != ptr);
        r
    }

    fn compare_objects(&mut self, a: &Value, b: &Value) -> PResult<i32> {
        let (object, other, object_lhs) = match (a, b) {
            (Value::Object(_), _) => (a, b, true),
            _ => (b, a, false),
        };
        let Value::Object(obj) = object else {
            unreachable!()
        };
        if let Value::Object(o2) = other {
            if crate::object::ObjRef::ptr_eq(obj, o2) {
                return Ok(0);
            }
        } else if let Value::Bool(_) = other {
            return match self.object_cast(obj, CastKind::Bool)? {
                None => Ok(if object_lhs { 1 } else { -1 }),
                Some(casted) => {
                    if object_lhs {
                        self.compare(&casted, other)
                    } else {
                        self.compare(other, &casted)
                    }
                }
            };
        }
        if let Some(h) = obj.class().handlers.compare {
            return h(self, a, b);
        }
        self.std_compare_objects(a, b)
    }

    /// `zend_std_compare_objects`.
    pub fn std_compare_objects(&mut self, a: &Value, b: &Value) -> PResult<i32> {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                if crate::object::ObjRef::ptr_eq(x, y) {
                    return Ok(0);
                }
                if !Rc::ptr_eq(x.class(), y.class()) && x.class().lc_name != y.class().lc_name {
                    return Ok(1);
                }
                let no_dyn =
                    x.obj().dyn_props.borrow().is_none() && y.obj().dyn_props.borrow().is_none();
                let ptr = x.obj() as *const crate::object::PhpObject as usize;
                if self.compare_guard.contains(&ptr) {
                    return Err(
                        self.throw_error("Error", "Nesting level too deep - recursive dependency?")
                    );
                }
                if no_dyn {
                    let p1 = x.obj().props.borrow().clone();
                    let p2 = y.obj().props.borrow().clone();
                    self.compare_guard.push(ptr);
                    let r = (|| {
                        for (v1, v2) in p1.iter().zip(p2.iter()) {
                            match (v1.is_undef(), v2.is_undef()) {
                                (false, false) => {
                                    let c = self.compare(v1, v2)?;
                                    if c != 0 {
                                        return Ok(c);
                                    }
                                }
                                (true, true) => {}
                                _ => return Ok(1),
                            }
                        }
                        Ok(0)
                    })();
                    self.compare_guard.retain(|p| *p != ptr);
                    return r;
                }
                let ta = Rc::new(super::convert::object_property_table(x));
                let tb = Rc::new(super::convert::object_property_table(y));
                self.compare_guard.push(ptr);
                let r = self.compare_arrays(&ta, &tb);
                self.compare_guard.retain(|p| *p != ptr);
                r
            }
            _ => {
                let (obj, value, object_lhs) = match a {
                    Value::Object(o) => (o.clone(), b.clone(), true),
                    _ => match b {
                        Value::Object(o) => (o.clone(), a.clone(), false),
                        _ => unreachable!(),
                    },
                };
                let target = match &value {
                    Value::Int(_) => Some(CastKind::Int),
                    Value::Float(_) => Some(CastKind::Float),
                    Value::String(_) => Some(CastKind::String),
                    _ => None,
                };
                let casted = match target {
                    Some(t) => self.object_cast(&obj, t)?,
                    None => None,
                };
                let casted = match casted {
                    Some(c) => c,
                    None => match target {
                        Some(t @ (CastKind::Int | CastKind::Float)) => {
                            let ty = if t == CastKind::Int { "int" } else { "float" };
                            self.engine_notice(format!(
                                "Object of class {} could not be converted to {ty}",
                                obj.class().name
                            ))?;
                            if t == CastKind::Int {
                                Value::Int(1)
                            } else {
                                Value::Float(1.0)
                            }
                        }
                        _ => return Ok(if object_lhs { 1 } else { -1 }),
                    },
                };
                if object_lhs {
                    self.compare(&casted, &value)
                } else {
                    self.compare(&value, &casted)
                }
            }
        }
    }

    /// `===` with php-src's recursion error.
    pub fn is_identical(&mut self, a: &Value, b: &Value) -> PResult<bool> {
        match identical_guarded(a, b, &mut Vec::new()) {
            Ok(r) => Ok(r),
            Err(()) => {
                Err(self.throw_error("Error", "Nesting level too deep - recursive dependency?"))
            }
        }
    }

    /// `==`
    pub fn is_equal(&mut self, a: &Value, b: &Value) -> PResult<bool> {
        let (x, y) = (a.deref(), b.deref());
        match (&*x, &*y) {
            (Value::Int(p), Value::Int(q)) => return Ok(p == q),
            (Value::String(p), Value::String(q)) => {
                if PhpStr::ptr_eq(p, q) {
                    return Ok(true);
                }
                if p.first().is_some_and(|c| *c > b'9') || q.first().is_some_and(|c| *c > b'9') {
                    return Ok(p == q);
                }
                return Ok(smart_str_eq(p, q));
            }
            _ => {}
        }
        drop((x, y));
        Ok(self.compare(a, b)? == 0)
    }

    /// `!=`
    pub fn is_not_equal(&mut self, a: &Value, b: &Value) -> PResult<bool> {
        Ok(!self.is_equal(a, b)?)
    }

    /// `<`
    pub fn is_smaller(&mut self, a: &Value, b: &Value) -> PResult<bool> {
        Ok(self.compare(a, b)? < 0)
    }

    /// `<=`
    pub fn is_smaller_or_equal(&mut self, a: &Value, b: &Value) -> PResult<bool> {
        Ok(self.compare(a, b)? <= 0)
    }

    /// `<=>`
    pub fn spaceship(&mut self, a: &Value, b: &Value) -> PResult<Value> {
        Ok(Value::Int(self.compare(a, b)? as i64))
    }

    // ----- increment / decrement ----------------------------------------------

    /// `++$x`: returns the incremented value (`increment_function`).
    pub fn increment(&mut self, v: &Value) -> PResult<Value> {
        let v = v.unref();
        Ok(match v {
            Value::Int(i) => match i.checked_add(1) {
                Some(n) => Value::Int(n),
                None => Value::Float(i64::MAX as f64 + 1.0),
            },
            Value::Float(d) => Value::Float(d + 1.0),
            Value::Null | Value::Undef => Value::Int(1),
            Value::String(s) => match is_numeric_str(&s) {
                Some(Numeric::Int(l)) => {
                    if l == i64::MAX {
                        Value::Float(l as f64 + 1.0)
                    } else {
                        Value::Int(l + 1)
                    }
                }
                Some(Numeric::Float(d)) => Value::Float(d + 1.0),
                None => {
                    self.engine_deprecated("Increment on non-numeric string is deprecated, use str_increment() instead")?;
                    Value::String(increment_string(&s))
                }
            },
            Value::Bool(_) => {
                self.engine_warning("Increment on type bool has no effect, this will change in the next major version of PHP")?;
                v
            }
            Value::Object(ref o) => {
                if let Some(h) = o.class().handlers.do_operation {
                    if let Some(r) = h(self, OpKind::Add, &v, &Value::Int(1))? {
                        return Ok(r);
                    }
                }
                match self.object_cast(o, CastKind::Number)? {
                    Some(n @ (Value::Int(_) | Value::Float(_))) => return self.increment(&n),
                    _ => {
                        let msg = format!("Cannot increment {}", v.value_name());
                        return Err(self.throw_type_error(msg));
                    }
                }
            }
            Value::Array(_) | Value::Resource(_) => {
                let msg = format!("Cannot increment {}", v.value_name());
                return Err(self.throw_type_error(msg));
            }
            Value::Ref(_) => unreachable!("unref'd"),
        })
    }

    /// `--$x`: returns the decremented value (`decrement_function`).
    pub fn decrement(&mut self, v: &Value) -> PResult<Value> {
        let v = v.unref();
        Ok(match v {
            Value::Int(i) => match i.checked_sub(1) {
                Some(n) => Value::Int(n),
                None => Value::Float(i64::MIN as f64 - 1.0),
            },
            Value::Float(d) => Value::Float(d - 1.0),
            Value::String(ref s) => {
                if s.is_empty() {
                    self.engine_deprecated(
                        "Decrement on empty string is deprecated as non-numeric",
                    )?;
                    return Ok(Value::Int(-1));
                }
                match is_numeric_str(s) {
                    Some(Numeric::Int(l)) => {
                        if l == i64::MIN {
                            Value::Float(l as f64 - 1.0)
                        } else {
                            Value::Int(l - 1)
                        }
                    }
                    Some(Numeric::Float(d)) => Value::Float(d - 1.0),
                    None => {
                        self.engine_deprecated(
                            "Decrement on non-numeric string has no effect and is deprecated",
                        )?;
                        v
                    }
                }
            }
            Value::Null | Value::Undef => {
                self.engine_warning("Decrement on type null has no effect, this will change in the next major version of PHP")?;
                Value::Null
            }
            Value::Bool(_) => {
                self.engine_warning("Decrement on type bool has no effect, this will change in the next major version of PHP")?;
                v
            }
            Value::Object(ref o) => {
                if let Some(h) = o.class().handlers.do_operation {
                    if let Some(r) = h(self, OpKind::Sub, &v, &Value::Int(1))? {
                        return Ok(r);
                    }
                }
                match self.object_cast(o, CastKind::Number)? {
                    Some(n @ (Value::Int(_) | Value::Float(_))) => return self.decrement(&n),
                    _ => {
                        let msg = format!("Cannot decrement {}", v.value_name());
                        return Err(self.throw_type_error(msg));
                    }
                }
            }
            Value::Array(_) | Value::Resource(_) => {
                let msg = format!("Cannot decrement {}", v.value_name());
                return Err(self.throw_type_error(msg));
            }
            Value::Ref(_) => unreachable!("unref'd"),
        })
    }
}

/// `_zendi_convert_scalar_to_number_silent` (comparison fallback).
fn scalar_to_number_silent(vm: &mut Vm, v: &Value) -> PResult<Value> {
    Ok(match v {
        Value::Null | Value::Undef | Value::Bool(false) => Value::Int(0),
        Value::Bool(true) => Value::Int(1),
        Value::String(s) => match is_numeric_string_ex(s, true) {
            Some(i) => i.num.into_value(),
            None => Value::Int(0),
        },
        Value::Resource(r) => Value::Int(r.id()),
        Value::Object(o) => match vm.object_cast(o, CastKind::Number)? {
            Some(n @ (Value::Int(_) | Value::Float(_))) => n,
            _ => Value::Int(1),
        },
        v => v.clone(),
    })
}

#[inline]
fn add_int(x: i64, y: i64) -> Value {
    match x.checked_add(y) {
        Some(r) => Value::Int(r),
        None => Value::Float(x as f64 + y as f64),
    }
}

#[inline]
fn sub_int(x: i64, y: i64) -> Value {
    match x.checked_sub(y) {
        Some(r) => Value::Int(r),
        None => Value::Float(x as f64 - y as f64),
    }
}

#[inline]
fn mul_int(x: i64, y: i64) -> Value {
    match x.checked_mul(y) {
        Some(r) => Value::Int(r),
        None => Value::Float(x as f64 * y as f64),
    }
}

fn num_binop(
    x: Numeric,
    y: Numeric,
    int_op: fn(i64, i64) -> Value,
    float_op: fn(f64, f64) -> f64,
) -> Value {
    match (x, y) {
        (Numeric::Int(a), Numeric::Int(b)) => int_op(a, b),
        (a, b) => Value::Float(float_op(a.to_f64(), b.to_f64())),
    }
}
