//! The `array_diff*` and `array_intersect*` families (18 functions).
//!
//! Three algorithms mirror php-src:
//! * `php_array_diff_key` / `php_array_intersect_key`: hash lookups driven by
//!   the first array (`*_key`, `*_assoc`, `u*_assoc`);
//! * `php_array_diff` / `php_array_intersect`: sort every argument and walk
//!   the sorted lists in parallel (all variants with a user key callback, and
//!   the user data callback variants `array_udiff`, `array_uintersect`);
//! * hash-set implementations of `array_diff` and `array_intersect`.
//!
//! Comparison callbacks run through `zend_sort` so the order and number of
//! user callback invocations match php-src.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use php_runtime::{Args, ArrayKey, Flow, NativeFunction, PResult, PhpArray, PhpStr, Value, Vm};

use super::sort::{Kind, data_cmp, key_cmp, user_compare, zend_sort};
use super::util::{HT_MAX_SIZE, call, copy_elem, php_get_long, sign};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    /// `DIFF_NORMAL` / `INTERSECT_NORMAL`: compare values.
    Normal,
    /// `DIFF_KEY` / `INTERSECT_KEY`: compare keys only.
    Key,
    /// `DIFF_ASSOC` / `INTERSECT_ASSOC`: compare keys and values.
    Assoc,
}

impl Behavior {
    /// `behavior & DIFF_ASSOC`: true for `Key` and `Assoc`.
    fn keyed(self) -> bool {
        self != Behavior::Normal
    }
}

struct Bucket {
    key: ArrayKey,
    val: Value,
}

fn type_error_array(vm: &mut Vm, arg_num: usize, v: &Value) -> Flow {
    let msg = format!("must be of type array, {} given", v.value_name());
    vm.argument_type_error(arg_num, &msg)
}

// ---------------------------------------------------------------------------
// Argument parsing ("+", "+f", "+ff")
// ---------------------------------------------------------------------------

struct Parsed {
    arrays: Vec<Rc<PhpArray>>,
    /// The data comparison callback (first `f`).
    data_cb: Option<Value>,
    /// The key comparison callback: the second `f`, or the only one when the
    /// function takes a key callback only.
    key_cb: Option<Value>,
}

/// `zend_parse_parameters("+", ...)`, `"+f"` or `"+ff"`.
///
/// `which` says what the callbacks are: `(data, key)`.
fn parse(vm: &mut Vm, args: &Args, data_user: bool, key_user: bool) -> PResult<Parsed> {
    let n_callbacks = data_user as usize + key_user as usize;
    let mut all: Vec<Value> = vec![args.get(0).clone()];
    all.extend(args.rest(1).iter().cloned());
    let min = 1 + n_callbacks;
    if all.len() < min {
        let name = vm.current_function_display_name();
        let msg = format!(
            "{name}() expects at least {min} argument{}, {} given",
            if min == 1 { "" } else { "s" },
            all.len()
        );
        return Err(vm.argument_count_error(msg));
    }
    let arr_argc = all.len() - n_callbacks;
    let host = vm.host.clone();
    let mut callbacks: Vec<Value> = Vec::new();
    for (j, v) in all[arr_argc..].iter().enumerate() {
        if let Err(e) = host.check_callable(vm, v) {
            let idx = arr_argc + j;
            let msg = format!("must be a valid callback, {e}");
            return Err(vm.argument_type_error(idx + 1, &msg));
        }
        callbacks.push(v.clone());
    }
    let (data_cb, key_cb) = match (data_user, key_user) {
        (true, true) => (Some(callbacks[0].clone()), Some(callbacks[1].clone())),
        (true, false) => (Some(callbacks[0].clone()), None),
        (false, true) => (None, Some(callbacks[0].clone())),
        (false, false) => (None, None),
    };
    let mut arrays = Vec::with_capacity(arr_argc);
    for (i, v) in all[..arr_argc].iter().enumerate() {
        match v {
            Value::Array(a) => arrays.push(a.clone()),
            other => return Err(type_error_array(vm, i + 1, other)),
        }
    }
    Ok(Parsed {
        arrays,
        data_cb,
        key_cb,
    })
}

// ---------------------------------------------------------------------------
// Comparison state (`BG(user_compare_fci)`)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Bg {
    Data,
    Key,
}

struct Cmp {
    data_user: bool,
    key_user: bool,
    data_cb: Option<Value>,
    key_cb: Option<Value>,
    /// Which callback the shared `BG(user_compare_fci)` currently holds. Both
    /// the value and the key user comparison use it, exactly as php-src does.
    bg: Bg,
    deprecation_thrown: bool,
}

impl Cmp {
    fn new(p: &Parsed, data_user: bool, key_user: bool) -> Cmp {
        Cmp {
            data_user,
            key_user,
            data_cb: p.data_cb.clone(),
            key_cb: p.key_cb.clone(),
            bg: Bg::Data,
            deprecation_thrown: false,
        }
    }

    fn bg_callable(&self) -> Value {
        match self.bg {
            Bg::Data => self.data_cb.clone(),
            Bg::Key => self.key_cb.clone(),
        }
        .expect("callback configured")
    }

    /// `*_data_compare_func`: `php_array_data_compare_string_unstable` or
    /// `php_array_user_compare_unstable`.
    fn data(&mut self, vm: &mut Vm, a: &Bucket, b: &Bucket) -> PResult<i32> {
        if self.data_user {
            let cb = self.bg_callable();
            user_compare(vm, &cb, &mut self.deprecation_thrown, &a.val, &b.val)
        } else {
            data_cmp(vm, Kind::String, &a.val, &b.val)
        }
    }

    /// `*_key_compare_func`: `php_array_key_compare_string_unstable` or
    /// `php_array_user_key_compare_unstable`.
    fn key(&mut self, vm: &mut Vm, a: &Bucket, b: &Bucket) -> PResult<i32> {
        if self.key_user {
            let cb = self.bg_callable();
            user_compare(
                vm,
                &cb,
                &mut self.deprecation_thrown,
                &a.key.to_value(),
                &b.key.to_value(),
            )
        } else {
            key_cmp(vm, Kind::String, &a.key, &b.key)
        }
    }
}

fn make_list(a: &PhpArray) -> Vec<Bucket> {
    a.iter()
        .map(|(key, v)| Bucket {
            key,
            val: v.clone(),
        })
        .collect()
}

/// Sorts `list` by value (`Normal`) or by key with `zend_sort`.
fn sort_list(vm: &mut Vm, cmp: &mut Cmp, list: &mut [Bucket], behavior: Behavior) -> PResult<()> {
    let mut err: Option<Flow> = None;
    {
        let mut f = |a: &Bucket, b: &Bucket| -> i32 {
            if err.is_some() {
                return 0;
            }
            let r = if behavior == Behavior::Normal {
                cmp.data(vm, a, b)
            } else {
                cmp.key(vm, a, b)
            };
            match r {
                Ok(x) => x,
                Err(e) => {
                    err = Some(e);
                    0
                }
            }
        };
        zend_sort(list, &mut f);
    }
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Removes the element a bucket was made from from the result array.
fn delete(result: &mut PhpArray, b: &Bucket) {
    result.remove(&b.key);
}

// ---------------------------------------------------------------------------
// php_array_diff
// ---------------------------------------------------------------------------

fn php_array_diff(
    vm: &mut Vm,
    args: &mut Args,
    behavior: Behavior,
    data_user: bool,
    key_user: bool,
) -> PResult<Value> {
    let parsed = parse(vm, args, data_user, key_user)?;
    let mut cmp = Cmp::new(&parsed, data_user, key_user);
    let n = parsed.arrays.len();

    if behavior == Behavior::Normal && data_user {
        cmp.bg = Bg::Data;
    } else if behavior.keyed() && key_user {
        cmp.bg = Bg::Key;
    }

    // For each argument, create and sort a list of its buckets.
    let mut lists: Vec<Vec<Bucket>> = Vec::with_capacity(n);
    for a in &parsed.arrays {
        let mut list = make_list(a);
        if a.len() > 1 {
            sort_list(vm, &mut cmp, &mut list, behavior)?;
        }
        lists.push(list);
    }

    // Copy the first argument.
    let mut result: PhpArray = (*parsed.arrays[0]).clone();

    // Go through the lists and look for values of ptrs[0] that are not in the others.
    let mut ptrs = vec![0usize; n];
    'outer: while ptrs[0] < lists[0].len() {
        if behavior.keyed() && key_user {
            cmp.bg = Bg::Key;
        }
        let mut c: i32 = 1;
        for i in 1..n {
            let mut ptr = ptrs[i];
            if behavior == Behavior::Normal {
                while ptrs[i] < lists[i].len() {
                    c = cmp.data(vm, &lists[0][ptrs[0]], &lists[i][ptrs[i]])?;
                    if c <= 0 {
                        break;
                    }
                    ptrs[i] += 1;
                }
            } else {
                while ptr < lists[i].len() {
                    c = cmp.key(vm, &lists[0][ptrs[0]], &lists[i][ptr])?;
                    if c == 0 {
                        break;
                    }
                    ptr += 1;
                }
            }
            if c == 0 {
                match behavior {
                    Behavior::Normal => {
                        if ptrs[i] < lists[i].len() {
                            ptrs[i] += 1;
                        }
                        break;
                    }
                    Behavior::Assoc => {
                        if ptr < lists[i].len() {
                            if data_user {
                                cmp.bg = Bg::Data;
                            }
                            if cmp.data(vm, &lists[0][ptrs[0]], &lists[i][ptr])? != 0 {
                                // The data is not the same.
                                c = -1;
                                if key_user {
                                    cmp.bg = Bg::Key;
                                }
                            } else {
                                // Found in another array: drop it from the result.
                                break;
                            }
                        }
                    }
                    Behavior::Key => break,
                }
            }
        }
        if c == 0 {
            // ptrs[0] is in one of the other arguments: delete all entries
            // with the value of ptrs[0].
            loop {
                delete(&mut result, &lists[0][ptrs[0]]);
                ptrs[0] += 1;
                if ptrs[0] >= lists[0].len() {
                    break 'outer;
                }
                if behavior == Behavior::Normal {
                    if cmp.data(vm, &lists[0][ptrs[0] - 1], &lists[0][ptrs[0]])? != 0 {
                        break;
                    }
                } else {
                    break;
                }
            }
        } else {
            // ptrs[0] is in none of the other arguments: skip all entries
            // with the value of ptrs[0].
            loop {
                ptrs[0] += 1;
                if ptrs[0] >= lists[0].len() {
                    break 'outer;
                }
                if behavior == Behavior::Normal {
                    if cmp.data(vm, &lists[0][ptrs[0] - 1], &lists[0][ptrs[0]])? != 0 {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }
    Ok(Value::array(result))
}

// ---------------------------------------------------------------------------
// php_array_intersect
// ---------------------------------------------------------------------------

fn php_array_intersect(
    vm: &mut Vm,
    args: &mut Args,
    behavior: Behavior,
    data_user: bool,
    key_user: bool,
) -> PResult<Value> {
    let parsed = parse(vm, args, data_user, key_user)?;
    let mut cmp = Cmp::new(&parsed, data_user, key_user);
    let n = parsed.arrays.len();

    if behavior == Behavior::Normal && data_user {
        cmp.bg = Bg::Data;
    } else if behavior.keyed() && key_user {
        cmp.bg = Bg::Key;
    }

    let mut lists: Vec<Vec<Bucket>> = Vec::with_capacity(n);
    for a in &parsed.arrays {
        let mut list = make_list(a);
        if a.len() > 1 {
            sort_list(vm, &mut cmp, &mut list, behavior)?;
        }
        lists.push(list);
    }

    let mut result: PhpArray = (*parsed.arrays[0]).clone();

    let mut ptrs = vec![0usize; n];
    let mut c: i32 = 0;
    'outer: while ptrs[0] < lists[0].len() {
        if behavior.keyed() && key_user {
            cmp.bg = Bg::Key;
        }
        let mut i = 1usize;
        while i < n {
            if behavior == Behavior::Normal {
                while ptrs[i] < lists[i].len() {
                    c = cmp.data(vm, &lists[0][ptrs[0]], &lists[i][ptrs[i]])?;
                    if c <= 0 {
                        break;
                    }
                    ptrs[i] += 1;
                }
            } else {
                while ptrs[i] < lists[i].len() {
                    c = cmp.key(vm, &lists[0][ptrs[0]], &lists[i][ptrs[i]])?;
                    if c <= 0 {
                        break;
                    }
                    ptrs[i] += 1;
                }
                if c == 0 && ptrs[i] < lists[i].len() && behavior == Behavior::Assoc {
                    // Only when INTERSECT_ASSOC: compare the data as well.
                    if data_user {
                        cmp.bg = Bg::Data;
                    }
                    if cmp.data(vm, &lists[0][ptrs[0]], &lists[i][ptrs[i]])? != 0 {
                        c = 1;
                        if key_user {
                            cmp.bg = Bg::Key;
                        }
                    }
                }
            }
            if ptrs[i] >= lists[i].len() {
                // Delete any values corresponding to the remains of ptrs[0]
                // and exit: they are not present in at least one of the
                // other arguments.
                loop {
                    let p = ptrs[0];
                    ptrs[0] += 1;
                    if p >= lists[0].len() {
                        break 'outer;
                    }
                    delete(&mut result, &lists[0][p]);
                }
            }
            if c != 0 {
                // Not all are equal.
                break;
            }
            ptrs[i] += 1;
            i += 1;
        }
        if c != 0 {
            // The value of ptrs[0] is not in all arguments: delete all
            // entries with a value < the value of ptrs[i].
            loop {
                delete(&mut result, &lists[0][ptrs[0]]);
                ptrs[0] += 1;
                if ptrs[0] >= lists[0].len() {
                    break 'outer;
                }
                if behavior == Behavior::Normal {
                    if cmp.data(vm, &lists[0][ptrs[0]], &lists[i][ptrs[i]])? >= 0 {
                        break;
                    }
                } else {
                    // No need to loop because the keys are unique.
                    break;
                }
            }
        } else {
            // ptrs[0] is present in all the arguments: skip all entries with
            // the same value.
            loop {
                ptrs[0] += 1;
                if ptrs[0] >= lists[0].len() {
                    break 'outer;
                }
                if behavior == Behavior::Normal {
                    if cmp.data(vm, &lists[0][ptrs[0] - 1], &lists[0][ptrs[0]])? != 0 {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }
    Ok(Value::array(result))
}

// ---------------------------------------------------------------------------
// php_array_diff_key / php_array_intersect_key
// ---------------------------------------------------------------------------

/// `zval_compare` (internal data comparison: string comparison) or
/// `zval_user_compare`.
fn data_equal(vm: &mut Vm, callback: Option<&Value>, a: &Value, b: &Value) -> PResult<bool> {
    match callback {
        Some(cb) => {
            let ret = call(vm, cb, vec![a.clone(), b.clone()])?;
            let r = php_get_long(vm, &ret)?;
            Ok(sign(r) == 0)
        }
        None => Ok(data_cmp(vm, Kind::String, a, b)? == 0),
    }
}

/// How data is compared by the hash-driven variants.
enum DataCompare {
    None,
    Internal,
    User,
}

fn php_array_diff_key(vm: &mut Vm, args: &mut Args, data: DataCompare) -> PResult<Value> {
    let user = matches!(data, DataCompare::User);
    let parsed = parse(vm, args, user, false)?;
    let compare = !matches!(data, DataCompare::None);
    let cb = parsed.data_cb.as_ref();
    let first = &parsed.arrays[0];

    let mut out = PhpArray::new();
    // Iterate over the keys of the first array, to compute the keys that
    // aren't in the other arrays.
    for (key, val) in first.iter() {
        let val = copy_elem(val);
        let mut ok = true;
        for other in &parsed.arrays[1..] {
            if let Some(d) = other.get(&key) {
                if !compare || data_equal(vm, cb, &val, d)? {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            out.insert(key, val);
        }
    }
    Ok(Value::array(out))
}

fn php_array_intersect_key(vm: &mut Vm, args: &mut Args, data: DataCompare) -> PResult<Value> {
    let user = matches!(data, DataCompare::User);
    let parsed = parse(vm, args, user, false)?;
    let compare = !matches!(data, DataCompare::None);
    let cb = parsed.data_cb.as_ref();
    let first = &parsed.arrays[0];

    let mut out = PhpArray::new();
    // Iterate over the keys of the first array, to compute the keys that are
    // in all of the other arrays.
    for (key, val) in first.iter() {
        let val = copy_elem(val);
        let mut ok = true;
        for other in &parsed.arrays[1..] {
            match other.get(&key) {
                None => {
                    ok = false;
                    break;
                }
                Some(d) => {
                    if compare && !data_equal(vm, cb, &val, d)? {
                        ok = false;
                        break;
                    }
                }
            }
        }
        if ok {
            out.insert(key, val);
        }
    }
    Ok(Value::array(out))
}

// ---------------------------------------------------------------------------
// array_diff / array_intersect (hash based)
// ---------------------------------------------------------------------------

#[allow(clippy::mutable_key_type)] // PhpStr hashes by content; the cache is not part of identity
fn array_diff(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut all: Vec<Rc<PhpArray>> = vec![args.array(0).clone()];
    for v in args.rest(1) {
        match v {
            Value::Array(a) => all.push(a.clone()),
            _ => unreachable!("array_diff() argument after ZPP"),
        }
    }
    let first = all[0].clone();
    let num = first.len();
    if num == 0 {
        return Ok(Value::empty_array());
    }
    if num == 1 {
        let (_, value) = first.first().expect("one element");
        let search = vm.to_php_string(value)?;
        let mut found = false;
        for other in &all[1..] {
            if !found {
                for v in other.values() {
                    let s = vm.to_php_string(v)?;
                    if search == s {
                        found = true;
                        break;
                    }
                }
            }
        }
        return Ok(if found {
            Value::empty_array()
        } else {
            Value::Array(first)
        });
    }

    // Count the number of elements.
    let total: u64 = all[1..].iter().map(|a| a.len() as u64).sum();
    if total == 0 {
        return Ok(Value::Array(first));
    }
    if total >= HT_MAX_SIZE as u64 {
        return Err(vm.throw_error(
            "Error",
            format!("The total number of elements must be lower than {HT_MAX_SIZE}"),
        ));
    }

    // Create the exclude map (plain string keys, no numeric normalisation).
    let mut exclude: HashSet<PhpStr> = HashSet::with_capacity(total as usize);
    for other in &all[1..] {
        for v in other.values() {
            exclude.insert(vm.to_php_string(v)?);
        }
    }

    // Copy all elements of the first array that are not in the exclude set.
    let mut out = PhpArray::with_capacity(first.len());
    for (key, v) in first.iter() {
        let s = vm.to_php_string(v)?;
        if !exclude.contains(&s) {
            out.insert(key, copy_elem(v));
        }
    }
    Ok(Value::array(out))
}

/// `php_array_intersect_get_key`: the symtable key of a value.
fn value_key(vm: &mut Vm, v: &Value) -> PResult<ArrayKey> {
    Ok(match &*v.deref() {
        Value::Int(i) => ArrayKey::Int(*i),
        Value::String(s) => ArrayKey::from_php_str(s.clone()),
        other => {
            let other = other.clone();
            ArrayKey::from_php_str(vm.to_php_string(&other)?)
        }
    })
}

/// `php_array_intersect_hash`.
#[allow(clippy::mutable_key_type)] // ArrayKey hashes by content
fn array_intersect_hash(vm: &mut Vm, arrays: &[Rc<PhpArray>]) -> PResult<Value> {
    let argc = arrays.len();
    let first = &arrays[0];

    // An empty argument makes the intersection empty, so no values need to
    // be converted to string.
    if arrays.iter().any(|a| a.is_empty()) {
        let mut result: PhpArray = (**first).clone();
        for key in first.keys() {
            result.remove(&key);
        }
        return Ok(Value::array(result));
    }

    // Map each value of args[1] to the number of consecutive arguments,
    // starting from args[1], the value has been seen in.
    let mut set: HashMap<ArrayKey, i64> = HashMap::with_capacity(arrays[1].len());
    for v in arrays[1].values() {
        let k = value_key(vm, v)?;
        set.insert(k, 1);
    }
    for (i, a) in arrays.iter().enumerate().skip(2) {
        for v in a.values() {
            let k = value_key(vm, v)?;
            if let Some(count) = set.get_mut(&k) {
                if *count == i as i64 - 1 {
                    *count = i as i64;
                }
            }
        }
    }

    // Filter a copy of the first argument; determine all entries to remove
    // before deleting any.
    let mut result: PhpArray = (**first).clone();
    let mut to_delete: Vec<ArrayKey> = Vec::new();
    for (key, entry) in first.iter() {
        let k = value_key(vm, entry)?;
        match set.get(&k) {
            Some(count) if *count == argc as i64 - 1 => {}
            _ => to_delete.push(key),
        }
    }
    for key in to_delete {
        result.remove(&key);
    }
    Ok(Value::array(result))
}

fn array_intersect(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut all: Vec<Rc<PhpArray>> = vec![args.array(0).clone()];
    for v in args.rest(1) {
        match v {
            Value::Array(a) => all.push(a.clone()),
            _ => unreachable!("array_intersect() argument after ZPP"),
        }
    }
    if all.len() >= 2 {
        return array_intersect_hash(vm, &all);
    }
    // A single array takes the generic path (and its conversion side effects).
    php_array_intersect(vm, args, Behavior::Normal, false, false)
}

// ---------------------------------------------------------------------------
// The functions
// ---------------------------------------------------------------------------

fn array_diff_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff_key(vm, args, DataCompare::None)
}

fn array_diff_ukey(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff(vm, args, Behavior::Key, false, true)
}

fn array_udiff(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff(vm, args, Behavior::Normal, true, false)
}

fn array_diff_assoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff_key(vm, args, DataCompare::Internal)
}

fn array_diff_uassoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff(vm, args, Behavior::Assoc, false, true)
}

fn array_udiff_assoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff_key(vm, args, DataCompare::User)
}

fn array_udiff_uassoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_diff(vm, args, Behavior::Assoc, true, true)
}

fn array_intersect_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect_key(vm, args, DataCompare::None)
}

fn array_intersect_ukey(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect(vm, args, Behavior::Key, false, true)
}

fn array_uintersect(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect(vm, args, Behavior::Normal, true, false)
}

fn array_intersect_assoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect_key(vm, args, DataCompare::Internal)
}

fn array_intersect_uassoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect(vm, args, Behavior::Assoc, false, true)
}

fn array_uintersect_assoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect_key(vm, args, DataCompare::User)
}

fn array_uintersect_uassoc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_array_intersect(vm, args, Behavior::Assoc, true, true)
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "array_intersect_key(array $array, array ...$arrays): array",
        handler: array_intersect_key,
    },
    NativeFunction {
        sig: "array_intersect_ukey(array $array, ...$rest): array",
        handler: array_intersect_ukey,
    },
    NativeFunction {
        sig: "array_intersect(array $array, array ...$arrays): array",
        handler: array_intersect,
    },
    NativeFunction {
        sig: "array_uintersect(array $array, ...$rest): array",
        handler: array_uintersect,
    },
    NativeFunction {
        sig: "array_intersect_assoc(array $array, array ...$arrays): array",
        handler: array_intersect_assoc,
    },
    NativeFunction {
        sig: "array_uintersect_assoc(array $array, ...$rest): array",
        handler: array_uintersect_assoc,
    },
    NativeFunction {
        sig: "array_intersect_uassoc(array $array, ...$rest): array",
        handler: array_intersect_uassoc,
    },
    NativeFunction {
        sig: "array_uintersect_uassoc(array $array, ...$rest): array",
        handler: array_uintersect_uassoc,
    },
    NativeFunction {
        sig: "array_diff_key(array $array, array ...$arrays): array",
        handler: array_diff_key,
    },
    NativeFunction {
        sig: "array_diff_ukey(array $array, ...$rest): array",
        handler: array_diff_ukey,
    },
    NativeFunction {
        sig: "array_diff(array $array, array ...$arrays): array",
        handler: array_diff,
    },
    NativeFunction {
        sig: "array_udiff(array $array, ...$rest): array",
        handler: array_udiff,
    },
    NativeFunction {
        sig: "array_diff_assoc(array $array, array ...$arrays): array",
        handler: array_diff_assoc,
    },
    NativeFunction {
        sig: "array_diff_uassoc(array $array, ...$rest): array",
        handler: array_diff_uassoc,
    },
    NativeFunction {
        sig: "array_udiff_assoc(array $array, ...$rest): array",
        handler: array_udiff_assoc,
    },
    NativeFunction {
        sig: "array_udiff_uassoc(array $array, ...$rest): array",
        handler: array_udiff_uassoc,
    },
];
