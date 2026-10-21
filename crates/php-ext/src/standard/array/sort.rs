//! Sorting: `zend_sort` (Zend/zend_sort.c), the comparison functions of
//! array.c and `sort`, `rsort`, `usort`, `asort`, `arsort`, `uasort`, `ksort`,
//! `krsort`, `uksort`, `natsort`, `natcasesort` and `array_multisort`.

use std::rc::Rc;

use php_runtime::value::numeric::strtod_value;
use php_runtime::value::ops::smart_str_cmp;
use php_runtime::{Args, ArrayKey, Flow, NativeFunction, PResult, PhpArray, PhpRef, Value, Vm};

use super::util::{
    binary_strcasecmp, binary_strcmp, c_strcoll, call, cmp_f64, php_get_long, set_array, sign,
};
use crate::standard::strnatcmp::strnatcmp_ex;

// ---------------------------------------------------------------------------
// zend_sort
// ---------------------------------------------------------------------------

/// `zend_sort`: the hybrid insertion sort / quick sort PHP uses for all array
/// sorting. `cmp` returns `> 0` when its first argument sorts after the second.
/// Porting the algorithm (instead of using `slice::sort_by`) keeps the exact
/// sequence of comparator calls, which is observable with user callbacks and
/// inconsistent comparison functions.
pub fn zend_sort<T, C: FnMut(&T, &T) -> i32>(v: &mut [T], cmp: &mut C) {
    let n = v.len();
    sort_rec(v, 0, n, cmp);
}

fn sort_2<T, C: FnMut(&T, &T) -> i32>(v: &mut [T], a: usize, b: usize, cmp: &mut C) {
    if cmp(&v[a], &v[b]) > 0 {
        v.swap(a, b);
    }
}

fn sort_3<T, C: FnMut(&T, &T) -> i32>(v: &mut [T], a: usize, b: usize, c: usize, cmp: &mut C) {
    if cmp(&v[a], &v[b]) <= 0 {
        if cmp(&v[b], &v[c]) <= 0 {
            return;
        }
        v.swap(b, c);
        if cmp(&v[a], &v[b]) > 0 {
            v.swap(a, b);
        }
        return;
    }
    if cmp(&v[c], &v[b]) <= 0 {
        v.swap(a, c);
        return;
    }
    v.swap(a, b);
    if cmp(&v[b], &v[c]) > 0 {
        v.swap(b, c);
    }
}

fn sort_4<T, C: FnMut(&T, &T) -> i32>(
    v: &mut [T],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    cmp: &mut C,
) {
    sort_3(v, a, b, c, cmp);
    if cmp(&v[c], &v[d]) > 0 {
        v.swap(c, d);
        if cmp(&v[b], &v[c]) > 0 {
            v.swap(b, c);
            if cmp(&v[a], &v[b]) > 0 {
                v.swap(a, b);
            }
        }
    }
}

fn sort_5<T, C: FnMut(&T, &T) -> i32>(
    v: &mut [T],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
    cmp: &mut C,
) {
    sort_4(v, a, b, c, d, cmp);
    if cmp(&v[d], &v[e]) > 0 {
        v.swap(d, e);
        if cmp(&v[c], &v[d]) > 0 {
            v.swap(c, d);
            if cmp(&v[b], &v[c]) > 0 {
                v.swap(b, c);
                if cmp(&v[a], &v[b]) > 0 {
                    v.swap(a, b);
                }
            }
        }
    }
}

/// `zend_insert_sort` on `v[base..base + nmemb]`.
fn insert_sort<T, C: FnMut(&T, &T) -> i32>(v: &mut [T], base: usize, nmemb: usize, cmp: &mut C) {
    match nmemb {
        0 | 1 => {}
        2 => sort_2(v, base, base + 1, cmp),
        3 => sort_3(v, base, base + 1, base + 2, cmp),
        4 => sort_4(v, base, base + 1, base + 2, base + 3, cmp),
        5 => sort_5(v, base, base + 1, base + 2, base + 3, base + 4, cmp),
        _ => {
            let start = base;
            let end = base + nmemb;
            let sentry = start + 6;
            let mut i = start + 1;
            while i < sentry {
                let mut j = i - 1;
                if cmp(&v[j], &v[i]) <= 0 {
                    i += 1;
                    continue;
                }
                while j != start {
                    j -= 1;
                    if cmp(&v[j], &v[i]) <= 0 {
                        j += 1;
                        break;
                    }
                }
                let mut k = i;
                while k > j {
                    v.swap(k, k - 1);
                    k -= 1;
                }
                i += 1;
            }
            let mut i = sentry;
            while i < end {
                let mut j = i - 1;
                if cmp(&v[j], &v[i]) <= 0 {
                    i += 1;
                    continue;
                }
                loop {
                    j -= 2;
                    if cmp(&v[j], &v[i]) <= 0 {
                        j += 1;
                        if cmp(&v[j], &v[i]) <= 0 {
                            j += 1;
                        }
                        break;
                    }
                    if j == start {
                        break;
                    }
                    if j == start + 1 {
                        j -= 1;
                        if cmp(&v[i], &v[j]) > 0 {
                            j += 1;
                        }
                        break;
                    }
                }
                let mut k = i;
                while k > j {
                    v.swap(k, k - 1);
                    k -= 1;
                }
                i += 1;
            }
        }
    }
}

fn sort_rec<T, C: FnMut(&T, &T) -> i32>(
    v: &mut [T],
    mut base: usize,
    mut nmemb: usize,
    cmp: &mut C,
) {
    loop {
        if nmemb <= 16 {
            insert_sort(v, base, nmemb, cmp);
            return;
        }
        let start = base;
        let end = start + nmemb;
        let offset = nmemb >> 1;
        let mut pivot = start + offset;
        if nmemb >> 10 != 0 {
            let delta = offset >> 1;
            sort_5(v, start, start + delta, pivot, pivot + delta, end - 1, cmp);
        } else {
            sort_3(v, start, pivot, end - 1, cmp);
        }
        v.swap(start + 1, pivot);
        pivot = start + 1;
        let mut i = pivot + 1;
        let mut j = end - 1;
        'partition: loop {
            while cmp(&v[pivot], &v[i]) > 0 {
                i += 1;
                if i == j {
                    break 'partition;
                }
            }
            j -= 1;
            if j == i {
                break 'partition;
            }
            while cmp(&v[j], &v[pivot]) > 0 {
                j -= 1;
                if j == i {
                    break 'partition;
                }
            }
            v.swap(i, j);
            i += 1;
            if i == j {
                break 'partition;
            }
        }
        v.swap(pivot, i - 1);
        if (i - 1) - start < end - i {
            sort_rec(v, start, (i - start) - 1, cmp);
            base = i;
            nmemb = end - i;
        } else {
            sort_rec(v, i, end - i, cmp);
            nmemb = (i - start) - 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Comparison functions
// ---------------------------------------------------------------------------

/// `PHP_SORT_*` flag values.
pub mod flags {
    pub const SORT_REGULAR: i64 = 0;
    pub const SORT_NUMERIC: i64 = 1;
    pub const SORT_STRING: i64 = 2;
    pub const SORT_DESC: i64 = 3;
    pub const SORT_ASC: i64 = 4;
    pub const SORT_LOCALE_STRING: i64 = 5;
    pub const SORT_NATURAL: i64 = 6;
    pub const SORT_FLAG_CASE: i64 = 8;
}

/// The comparison selected by a `$flags` argument (`php_get_data_compare_func`
/// and `php_get_key_compare_func`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Regular,
    Numeric,
    String,
    StringCase,
    Natural,
    NaturalCase,
    Locale,
}

impl Kind {
    pub fn from_flags(sort_type: i64) -> Kind {
        let case = sort_type & flags::SORT_FLAG_CASE != 0;
        match sort_type & !flags::SORT_FLAG_CASE {
            flags::SORT_NUMERIC => Kind::Numeric,
            flags::SORT_STRING => {
                if case {
                    Kind::StringCase
                } else {
                    Kind::String
                }
            }
            flags::SORT_NATURAL => {
                if case {
                    Kind::NaturalCase
                } else {
                    Kind::Natural
                }
            }
            flags::SORT_LOCALE_STRING => Kind::Locale,
            _ => Kind::Regular,
        }
    }
}

/// `php_array_data_compare_*_unstable_i`: compares two element values.
pub fn data_cmp(vm: &mut Vm, kind: Kind, a: &Value, b: &Value) -> PResult<i32> {
    match kind {
        Kind::Regular => {
            let result = vm.compare(a, b)?;
            // Enums are uncomparable with each other; array_unique() (and
            // sorting) just needs equal enum cases to group together.
            if result == 1 {
                let rhs = b.unref();
                if let Value::Object(ro) = &rhs {
                    if ro.class().is_enum() {
                        let lhs = a.unref();
                        if let Value::Object(lo) = &lhs {
                            if lo.class().is_enum() {
                                return Ok(match lo.id().cmp(&ro.id()) {
                                    std::cmp::Ordering::Equal => 0,
                                    std::cmp::Ordering::Less => -1,
                                    std::cmp::Ordering::Greater => 1,
                                });
                            }
                        }
                        return Ok(-1);
                    }
                }
            }
            Ok(result)
        }
        Kind::Numeric => {
            let d1 = vm.to_float(a)?;
            let d2 = vm.to_float(b)?;
            Ok(cmp_f64(d1, d2))
        }
        Kind::String | Kind::StringCase | Kind::Natural | Kind::NaturalCase | Kind::Locale => {
            let s1 = vm.to_php_string(a)?;
            let s2 = vm.to_php_string(b)?;
            Ok(match kind {
                Kind::String => binary_strcmp(&s1, &s2),
                Kind::StringCase => binary_strcasecmp(&s1, &s2),
                Kind::Natural => strnatcmp_ex(&s1, &s2, false),
                Kind::NaturalCase => strnatcmp_ex(&s1, &s2, true),
                _ => c_strcoll(&s1, &s2),
            })
        }
    }
}

/// `php_array_key_compare_*_unstable_i`: compares two array keys.
pub fn key_cmp(vm: &mut Vm, kind: Kind, a: &ArrayKey, b: &ArrayKey) -> PResult<i32> {
    Ok(match kind {
        Kind::Regular => match (a, b) {
            (ArrayKey::Int(x), ArrayKey::Int(y)) => {
                if x > y {
                    1
                } else {
                    -1
                }
            }
            (ArrayKey::Str(x), ArrayKey::Str(y)) => smart_str_cmp(x, y),
            _ => vm.compare(&a.to_value(), &b.to_value())?,
        },
        Kind::Numeric => match (a, b) {
            (ArrayKey::Int(x), ArrayKey::Int(y)) => {
                if x > y {
                    1
                } else {
                    -1
                }
            }
            _ => {
                let d = |k: &ArrayKey| match k {
                    ArrayKey::Str(s) => strtod_value(s),
                    ArrayKey::Int(i) => *i as f64,
                };
                cmp_f64(d(a), d(b))
            }
        },
        Kind::String => binary_strcmp(&a.to_php_str(), &b.to_php_str()),
        Kind::StringCase => binary_strcasecmp(&a.to_php_str(), &b.to_php_str()),
        Kind::Natural => strnatcmp_ex(&a.to_php_str(), &b.to_php_str(), false),
        Kind::NaturalCase => strnatcmp_ex(&a.to_php_str(), &b.to_php_str(), true),
        Kind::Locale => c_strcoll(&a.to_php_str(), &b.to_php_str()),
    })
}

// ---------------------------------------------------------------------------
// User comparison callbacks
// ---------------------------------------------------------------------------

const BOOL_DEPRECATION: &str = "Returning bool from comparison function is deprecated, return an integer less than, equal to, or greater than zero";

/// `php_array_user_compare_unstable` / `php_array_user_key_compare_unstable`:
/// calls the callback with `(x, y)`; a `bool` result is deprecated and `false`
/// is retried with swapped operands.
pub fn user_compare(
    vm: &mut Vm,
    callback: &Value,
    deprecation_thrown: &mut bool,
    x: &Value,
    y: &Value,
) -> PResult<i32> {
    let ret = call(vm, callback, vec![x.clone(), y.clone()])?;
    if let Value::Bool(b) = ret {
        if !*deprecation_thrown {
            vm.deprecated(BOOL_DEPRECATION)?;
            *deprecation_thrown = true;
        }
        if !b {
            // Retry with swapped operands.
            let ret = call(vm, callback, vec![y.clone(), x.clone()])?;
            let r = php_get_long(vm, &ret)?;
            return Ok(-sign(r));
        }
    }
    let r = php_get_long(vm, &ret)?;
    Ok(sign(r))
}

// ---------------------------------------------------------------------------
// Sorting arrays
// ---------------------------------------------------------------------------

/// A bucket being sorted; `idx` is the original position (`Z_EXTRA`) used by
/// the stable-sort fallback.
pub struct Entry {
    pub key: ArrayKey,
    pub val: Value,
    pub idx: usize,
}

/// Copies the elements of `a` in order.
pub fn snapshot(a: &PhpArray) -> Vec<Entry> {
    a.iter()
        .enumerate()
        .map(|(idx, (key, v))| Entry {
            key,
            val: v.clone(),
            idx,
        })
        .collect()
}

/// A comparison of two entries (`bucket_compare_func_t` without the stable
/// fallback).
pub type EntryCmp<'a> = dyn FnMut(&mut Vm, &Entry, &Entry) -> PResult<i32> + 'a;

/// Sorts `items` with `zend_sort`. The comparison is `f`, optionally reversed
/// (`* -1`) and with the stable-sort fallback on ties. After the first error
/// no further comparisons are made (PHP's pending exception makes callbacks
/// return nothing) and the error is returned once the sort has finished.
pub fn sort_entries(
    vm: &mut Vm,
    items: &mut [Entry],
    stable: bool,
    reverse: bool,
    f: &mut EntryCmp<'_>,
) -> Option<Flow> {
    let mut err: Option<Flow> = None;
    let mut cmp = |a: &Entry, b: &Entry| -> i32 {
        let mut r = 0;
        if err.is_none() {
            match f(vm, a, b) {
                Ok(x) => r = x,
                Err(e) => err = Some(e),
            }
        }
        if reverse {
            r = -r;
        }
        if r == 0 && stable {
            super::util::ord(a.idx.cmp(&b.idx))
        } else {
            r
        }
    };
    zend_sort(items, &mut cmp);
    err
}

/// Builds the sorted array (`zend_hash_sort`'s post-processing): with
/// `renumber` keys become `0..n`, otherwise keys and `nNextFreeElement` stay.
fn rebuild(items: Vec<Entry>, renumber: bool, next_free: i64) -> PhpArray {
    if renumber {
        return PhpArray::from_values(items.into_iter().map(|e| e.val).collect());
    }
    let mut out = PhpArray::with_capacity(items.len());
    for e in items {
        out.insert(e.key, e.val);
    }
    out.set_next_free(next_free);
    out
}

/// `zend_array_sort` on the array held by `r`.
pub fn sort_ref(
    vm: &mut Vm,
    r: &PhpRef,
    renumber: bool,
    stable: bool,
    reverse: bool,
    f: &mut EntryCmp<'_>,
) -> PResult<()> {
    let arr = match &*r.borrow() {
        Value::Array(a) => a.clone(),
        _ => return Ok(()),
    };
    let n = arr.len();
    // `zend_hash_sort_internal`: nothing to do for 0 elements (or 1 element
    // when the keys are kept).
    if n == 0 || (n == 1 && !renumber) {
        return Ok(());
    }
    let next_free = arr.next_free_raw();
    let mut items = snapshot(&arr);
    drop(arr);
    let err = sort_entries(vm, &mut items, stable, reverse, f);
    set_array(r, rebuild(items, renumber, next_free));
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn ok_true() -> PResult<Value> {
    Ok(Value::Bool(true))
}

/// `php_sort`: `sort`, `rsort`, `asort`, `arsort`, `ksort`, `krsort`.
fn php_sort(
    vm: &mut Vm,
    args: &mut Args,
    by_key: bool,
    reverse: bool,
    renumber: bool,
) -> PResult<Value> {
    let kind = Kind::from_flags(args.int(1));
    let r = args.by_ref(0);
    if by_key {
        sort_ref(vm, &r, renumber, true, reverse, &mut |vm, a, b| {
            key_cmp(vm, kind, &a.key, &b.key)
        })?;
    } else {
        sort_ref(vm, &r, renumber, true, reverse, &mut |vm, a, b| {
            data_cmp(vm, kind, &a.val, &b.val)
        })?;
    }
    ok_true()
}

fn sort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sort(vm, args, false, false, true)
}

fn rsort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sort(vm, args, false, true, true)
}

fn asort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sort(vm, args, false, false, false)
}

fn arsort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sort(vm, args, false, true, false)
}

fn ksort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sort(vm, args, true, false, false)
}

fn krsort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_sort(vm, args, true, true, false)
}

/// `php_natsort`.
fn php_natsort(vm: &mut Vm, args: &mut Args, kind: Kind) -> PResult<Value> {
    let r = args.by_ref(0);
    sort_ref(vm, &r, false, true, false, &mut |vm, a, b| {
        data_cmp(vm, kind, &a.val, &b.val)
    })?;
    ok_true()
}

fn natsort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_natsort(vm, args, Kind::Natural)
}

fn natcasesort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_natsort(vm, args, Kind::NaturalCase)
}

/// `php_usort`.
fn php_usort(vm: &mut Vm, args: &mut Args, by_key: bool, renumber: bool) -> PResult<Value> {
    let r = args.by_ref(0);
    let callback = args.get(1).clone();
    let mut deprecation_thrown = false;
    if by_key {
        sort_ref(vm, &r, renumber, true, false, &mut |vm, a, b| {
            user_compare(
                vm,
                &callback,
                &mut deprecation_thrown,
                &a.key.to_value(),
                &b.key.to_value(),
            )
        })?;
    } else {
        sort_ref(vm, &r, renumber, true, false, &mut |vm, a, b| {
            user_compare(vm, &callback, &mut deprecation_thrown, &a.val, &b.val)
        })?;
    }
    ok_true()
}

fn usort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_usort(vm, args, false, true)
}

fn uasort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_usort(vm, args, false, false)
}

fn uksort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_usort(vm, args, true, false)
}

// ---------------------------------------------------------------------------
// array_multisort
// ---------------------------------------------------------------------------

struct MultiCmp {
    kind: Kind,
    reverse: bool,
}

/// One row of `array_multisort`'s indirection table: the element of every
/// array at the same position, plus the original position.
struct Row {
    cols: Vec<(ArrayKey, Value)>,
    idx: usize,
}

fn array_multisort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // `array_multisort(&$array, &...$rest)`: every argument is a reference.
    let mut all: Vec<Value> = vec![args.get(0).clone()];
    all.extend(args.rest(1).iter().cloned());

    let mut refs: Vec<PhpRef> = Vec::new();
    let mut funcs: Vec<MultiCmp> = Vec::new();
    // parse_state: whether an order / type flag is still allowed.
    let mut order_allowed = false;
    let mut type_allowed = false;
    let mut sort_order = flags::SORT_ASC;
    let mut sort_type = flags::SORT_REGULAR;

    for (i, arg) in all.iter().enumerate() {
        let arg_ref: Option<PhpRef> = match arg {
            Value::Ref(r) => Some(r.clone()),
            _ => None,
        };
        let value = arg.unref();
        match &value {
            Value::Array(_) => {
                // A new array: finish the flags of the previous one.
                if i > 0 {
                    funcs.push(MultiCmp {
                        kind: Kind::from_flags(sort_type),
                        reverse: sort_order != flags::SORT_ASC,
                    });
                    sort_order = flags::SORT_ASC;
                    sort_type = flags::SORT_REGULAR;
                }
                refs.push(arg_ref.unwrap_or_else(|| PhpRef::new(value.clone())));
                order_allowed = true;
                type_allowed = true;
            }
            Value::Int(n) => match *n & !flags::SORT_FLAG_CASE {
                flags::SORT_ASC | flags::SORT_DESC => {
                    if order_allowed {
                        sort_order = if *n == flags::SORT_DESC {
                            flags::SORT_DESC
                        } else {
                            flags::SORT_ASC
                        };
                        order_allowed = false;
                    } else {
                        return Err(vm.argument_type_error(
                            i + 1,
                            "must be an array or a sort flag that has not already been specified",
                        ));
                    }
                }
                flags::SORT_REGULAR
                | flags::SORT_NUMERIC
                | flags::SORT_STRING
                | flags::SORT_NATURAL
                | flags::SORT_LOCALE_STRING => {
                    if type_allowed {
                        sort_type = *n;
                        type_allowed = false;
                    } else {
                        return Err(vm.argument_type_error(
                            i + 1,
                            "must be an array or a sort flag that has not already been specified",
                        ));
                    }
                }
                _ => return Err(vm.argument_value_error(i + 1, "must be a valid sort flag")),
            },
            _ => return Err(vm.argument_type_error(i + 1, "must be an array or a sort flag")),
        }
    }

    // Make sure the arrays are of the same size.
    let arrays: Vec<Rc<PhpArray>> = refs
        .iter()
        .map(|r| match &*r.borrow() {
            Value::Array(a) => a.clone(),
            _ => Rc::new(PhpArray::new()),
        })
        .collect();
    let array_size = arrays[0].len();
    if arrays.iter().skip(1).any(|a| a.len() != array_size) {
        return Err(vm.throw_value_error("Array sizes are inconsistent"));
    }

    // If all arrays are empty we don't need to do anything.
    if array_size < 1 {
        return ok_true();
    }

    // The last array's flags.
    funcs.push(MultiCmp {
        kind: Kind::from_flags(sort_type),
        reverse: sort_order != flags::SORT_ASC,
    });

    let mut rows: Vec<Row> = (0..array_size)
        .map(|idx| Row {
            cols: Vec::with_capacity(arrays.len()),
            idx,
        })
        .collect();
    for a in &arrays {
        for (k, (key, v)) in a.iter().enumerate() {
            rows[k].cols.push((key, v.clone()));
        }
    }
    drop(arrays);

    // `php_multisort_compare`.
    let mut err: Option<Flow> = None;
    let mut cmp = |a: &Row, b: &Row| -> i32 {
        if err.is_none() {
            for (r, f) in funcs.iter().enumerate() {
                match data_cmp(vm, f.kind, &a.cols[r].1, &b.cols[r].1) {
                    Ok(x) => {
                        let x = if f.reverse { -x } else { x };
                        if x != 0 {
                            return if x > 0 { 1 } else { -1 };
                        }
                    }
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
        }
        super::util::ord(a.idx.cmp(&b.idx))
    };
    zend_sort(&mut rows, &mut cmp);
    if let Some(e) = err {
        return Err(e);
    }

    // Restructure the arrays based on the sorted rows: integer keys are
    // renumbered, string keys are kept, `nNextFreeElement` is the size.
    for (i, r) in refs.iter().enumerate() {
        let mut out = PhpArray::with_capacity(array_size);
        let mut n = 0i64;
        for row in rows.iter_mut() {
            let (key, val) = std::mem::replace(&mut row.cols[i], (ArrayKey::Int(0), Value::Null));
            match key {
                ArrayKey::Int(_) => {
                    out.insert(ArrayKey::Int(n), val);
                    n += 1;
                }
                s => {
                    out.insert(s, val);
                }
            }
        }
        out.set_next_free(array_size as i64);
        set_array(r, out);
    }
    ok_true()
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "krsort(array &$array, int $flags = SORT_REGULAR): true",
        handler: krsort,
    },
    NativeFunction {
        sig: "ksort(array &$array, int $flags = SORT_REGULAR): true",
        handler: ksort,
    },
    NativeFunction {
        sig: "natsort(array &$array): true",
        handler: natsort,
    },
    NativeFunction {
        sig: "natcasesort(array &$array): true",
        handler: natcasesort,
    },
    NativeFunction {
        sig: "asort(array &$array, int $flags = SORT_REGULAR): true",
        handler: asort,
    },
    NativeFunction {
        sig: "arsort(array &$array, int $flags = SORT_REGULAR): true",
        handler: arsort,
    },
    NativeFunction {
        sig: "sort(array &$array, int $flags = SORT_REGULAR): true",
        handler: sort,
    },
    NativeFunction {
        sig: "rsort(array &$array, int $flags = SORT_REGULAR): true",
        handler: rsort,
    },
    NativeFunction {
        sig: "usort(array &$array, callable $callback): true",
        handler: usort,
    },
    NativeFunction {
        sig: "uasort(array &$array, callable $callback): true",
        handler: uasort,
    },
    NativeFunction {
        sig: "uksort(array &$array, callable $callback): true",
        handler: uksort,
    },
    NativeFunction {
        sig: "/** @prefer-ref $array @prefer-ref $rest */ function array_multisort(&$array, &...$rest): true",
        handler: array_multisort,
    },
];
