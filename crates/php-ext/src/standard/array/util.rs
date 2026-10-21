//! Helpers shared by the array functions.

use std::cmp::Ordering;
use std::rc::Rc;

use php_runtime::{Args, Flow, PResult, PhpArray, PhpArrayRef, PhpRef, Value, Vm};

/// `zval_add_ref` for a value that is copied into another array: a reference
/// held only by the source array is copied as a plain value, other references
/// stay shared.
pub fn copy_elem(v: &Value) -> Value {
    match v {
        Value::Ref(r) if r.refcount() == 1 => r.get(),
        other => other.clone(),
    }
}

/// `ZEND_NORMALIZE_BOOL`.
#[inline]
pub fn sign(i: i64) -> i32 {
    match i.cmp(&0) {
        Ordering::Greater => 1,
        Ordering::Less => -1,
        Ordering::Equal => 0,
    }
}

#[inline]
pub fn ord(o: Ordering) -> i32 {
    match o {
        Ordering::Greater => 1,
        Ordering::Less => -1,
        Ordering::Equal => 0,
    }
}

/// `ZEND_THREEWAY_COMPARE` for doubles (NaN compares as 1).
#[inline]
pub fn cmp_f64(a: f64, b: f64) -> i32 {
    if a == b {
        0
    } else if a < b {
        -1
    } else {
        1
    }
}

/// `zend_binary_strcmp`.
pub fn binary_strcmp(a: &[u8], b: &[u8]) -> i32 {
    ord(a.cmp(b))
}

/// `zend_binary_strcasecmp[_l]` (ASCII case folding).
pub fn binary_strcasecmp(a: &[u8], b: &[u8]) -> i32 {
    let n = a.len().min(b.len());
    for i in 0..n {
        let (c1, c2) = (a[i].to_ascii_lowercase(), b[i].to_ascii_lowercase());
        if c1 != c2 {
            return (c1 as i32 - c2 as i32).signum();
        }
    }
    ord(a.len().cmp(&b.len()))
}

/// `strcoll()` in the "C" locale: `strcmp()` of the NUL-terminated prefixes.
pub fn c_strcoll(a: &[u8], b: &[u8]) -> i32 {
    let cut = |s: &[u8]| s.iter().position(|&c| c == 0).unwrap_or(s.len());
    ord(a[..cut(a)].cmp(&b[..cut(b)]))
}

/// `php_get_long`: the integer value of a callback's return value.
pub fn php_get_long(vm: &mut Vm, v: &Value) -> PResult<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        other => vm.to_int(other),
    }
}

/// Calls a PHP callable and unwraps a returned reference.
pub fn call(vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
    Ok(vm.call(callable, args)?.into_unref())
}

/// The array stored in the by-reference parameter `i` (a cheap `Rc` clone).
pub fn ref_array(args: &Args, i: usize) -> Option<PhpArrayRef> {
    match &*args.by_ref(i).borrow() {
        Value::Array(a) => Some(a.clone()),
        _ => None,
    }
}

/// Runs `f` with exclusive access to the array held by the by-reference
/// parameter `i`, separating it first when it is shared (`SEPARATE_ARRAY`).
/// No PHP code may run inside `f`.
pub fn with_array_mut<R>(args: &Args, i: usize, f: impl FnOnce(&mut PhpArray) -> R) -> R {
    let r = args.by_ref(i);
    let mut slot = r.borrow_mut();
    match &mut *slot {
        Value::Array(a) => f(Rc::make_mut(a)),
        other => panic!("array expected after ZPP, got {other:?}"),
    }
}

/// Replaces the array held by a reference.
pub fn set_array(r: &PhpRef, a: PhpArray) {
    r.set(Value::Array(Rc::new(a)));
}

/// Appends `v`, mapping the "next element occupied" failure to PHP's Error.
pub fn append_or_throw(vm: &mut Vm, out: &mut PhpArray, v: Value) -> PResult<()> {
    match out.append(v) {
        Ok(_) => Ok(()),
        Err(_) => Err(cannot_add_element(vm)),
    }
}

/// `zend_cannot_add_element`.
pub fn cannot_add_element(vm: &mut Vm) -> Flow {
    vm.throw_error(
        "Error",
        "Cannot add element to the array as the next element is already occupied",
    )
}

/// `HT_MAX_SIZE` on 64-bit builds.
pub const HT_MAX_SIZE: i64 = 0x4000_0000;
