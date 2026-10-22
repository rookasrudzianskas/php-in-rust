//! The internal array pointer functions (`current`, `key`, `next`, `prev`,
//! `reset`, `end`) and `array_walk()` / `array_walk_recursive()`.

use std::collections::HashMap;
use std::rc::Rc;

use php_runtime::object::WeakObjRef;
use php_runtime::value::convert::mangle_property_name;
use php_runtime::{
    Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, PhpStr, Pos, Value, Vm,
};

use super::util::{call, with_array_mut};

// ---------------------------------------------------------------------------
// Internal pointer of objects
// ---------------------------------------------------------------------------

/// Internal-pointer positions of objects' property tables.
///
/// The runtime keeps no persistent property hash table for objects, so the
/// table is rebuilt for every call (see [`iap_table`]) and the pointer
/// position is remembered here, keyed by object handle.
// TODO(runtime): expose the object's real property table so that the internal
// pointer lives in it (and survives property additions exactly like php-src).
#[derive(Default)]
struct ObjectIap {
    map: HashMap<u32, (WeakObjRef, Pos)>,
}

/// An object's property table as `get_properties()` shows it to the internal
/// pointer functions: one entry per declared slot (uninitialised typed
/// properties are `INDIRECT` entries pointing at `UNDEF`, listed in
/// `uninit`) followed by the dynamic properties.
struct IapTable {
    table: PhpArray,
    uninit: Vec<ArrayKey>,
}

fn iap_table(o: &ObjRef) -> IapTable {
    let mut table = PhpArray::new();
    let mut uninit = Vec::new();
    for (key, loc, initialised) in object_props(o, true) {
        let k = ArrayKey::Str(key);
        let value = match &loc {
            PropLoc::Slot(slot) => o
                .obj()
                .props
                .borrow()
                .get(*slot)
                .filter(|v| !v.is_undef())
                .cloned()
                .unwrap_or(Value::Null),
            PropLoc::Dynamic(name) => o
                .obj()
                .dyn_props
                .borrow()
                .as_ref()
                .and_then(|d| d.get(&ArrayKey::Str(name.clone())).cloned())
                .unwrap_or(Value::Null),
        };
        if !initialised {
            uninit.push(k.clone());
        }
        table.insert(k, value);
    }
    IapTable { table, uninit }
}

/// Runs `f` on the object's property table with its internal pointer
/// restored, then stores the pointer back.
fn with_object_table<R>(vm: &mut Vm, o: &ObjRef, f: impl FnOnce(&mut IapTable) -> R) -> R {
    let mut t = iap_table(o);
    let st = vm.ext_data::<ObjectIap>();
    if let Some((w, pos)) = st.map.get(&o.id()) {
        if w.upgrade().is_some_and(|x| ObjRef::ptr_eq(&x, o)) {
            t.table.set_internal_pointer(*pos);
        }
    }
    let r = f(&mut t);
    let pos = t.table.internal_pointer();
    let st = vm.ext_data::<ObjectIap>();
    st.map.insert(o.id(), (o.downgrade(), pos));
    r
}

/// `get_ht_for_iap` for objects: the deprecation.
fn deprecate_object_iap(vm: &mut Vm) -> PResult<()> {
    let name = vm.current_function_name();
    vm.deprecated(format!("Calling {name}() on an object is deprecated"))
}

/// `php_array_iter_seek_current`: the entry at the internal pointer, moving
/// over uninitialised properties in the direction of travel.
fn seek_current(a: &mut PhpArray, uninit: &[ArrayKey], forward: bool) -> Option<(ArrayKey, Value)> {
    loop {
        let pos = a.valid_pos(a.internal_pointer())?;
        let (key, v) = a.entry_at(pos)?;
        if !uninit.contains(&key) {
            return Some((key, v.unref()));
        }
        let moved = if forward {
            a.move_forward()
        } else {
            a.move_backward()
        };
        if !moved {
            return None;
        }
    }
}

fn current_value(t: &mut IapTable, forward: bool) -> Value {
    match seek_current(&mut t.table, &t.uninit, forward) {
        Some((_, v)) => v,
        None => Value::Bool(false),
    }
}

fn current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.get(0).clone() {
        Value::Array(a) => Ok(match a.current() {
            Some(v) => v.unref(),
            None => Value::Bool(false),
        }),
        Value::Object(o) => {
            deprecate_object_iap(vm)?;
            Ok(with_object_table(vm, &o, |t| current_value(t, true)))
        }
        _ => Ok(Value::Bool(false)),
    }
}

fn key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.get(0).clone() {
        Value::Array(a) => Ok(match a.current_key() {
            Some(k) => k.into_value(),
            None => Value::Null,
        }),
        Value::Object(o) => {
            deprecate_object_iap(vm)?;
            Ok(with_object_table(vm, &o, |t| {
                match seek_current(&mut t.table, &t.uninit, true) {
                    Some((k, _)) => k.into_value(),
                    None => Value::Null,
                }
            }))
        }
        _ => Ok(Value::Null),
    }
}

#[derive(Clone, Copy)]
enum Move {
    End,
    Prev,
    Next,
    Reset,
}

/// Moves the pointer and returns the current element (`false` if none).
fn apply_move(t: &mut IapTable, m: Move) -> Value {
    if t.table.is_empty() {
        // The internal pointer of an empty array is already 0.
        return Value::Bool(false);
    }
    let forward = match m {
        Move::End => {
            t.table.end();
            false
        }
        Move::Prev => {
            t.table.move_backward();
            false
        }
        Move::Next => {
            t.table.move_forward();
            true
        }
        Move::Reset => {
            t.table.reset();
            true
        }
    };
    current_value(t, forward)
}

fn move_pointer(vm: &mut Vm, args: &mut Args, m: Move) -> PResult<Value> {
    let r = args.by_ref(0);
    let obj = match &*r.borrow() {
        Value::Object(o) => Some(o.clone()),
        _ => None,
    };
    match obj {
        Some(o) => {
            deprecate_object_iap(vm)?;
            Ok(with_object_table(vm, &o, |t| apply_move(t, m)))
        }
        None => Ok(with_array_mut(vm, args, 0, |a| {
            // Arrays have no uninitialised slots: move the array itself.
            let mut t = IapTable {
                table: std::mem::take(a),
                uninit: Vec::new(),
            };
            let v = apply_move(&mut t, m);
            *a = t.table;
            v
        })),
    }
}

fn end(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    move_pointer(vm, args, Move::End)
}

fn prev(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    move_pointer(vm, args, Move::Prev)
}

fn next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    move_pointer(vm, args, Move::Next)
}

fn reset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    move_pointer(vm, args, Move::Reset)
}

// ---------------------------------------------------------------------------
// array_walk
// ---------------------------------------------------------------------------

/// Where an object's property lives.
enum PropLoc {
    Slot(usize),
    Dynamic(PhpStr),
}

/// The properties of an object in property-table order, with the (mangled)
/// key `array_walk()` passes to the callback. With `include_uninit`,
/// uninitialised declared slots are listed too (flagged `false`).
fn object_props(o: &ObjRef, include_uninit: bool) -> Vec<(PhpStr, PropLoc, bool)> {
    let mut out: Vec<(PhpStr, PropLoc, bool)> = Vec::new();
    let mut infos: Vec<(usize, PhpStr)> = Vec::new();
    let mut ce = Some(o.class().clone());
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
    {
        let props = o.obj().props.borrow();
        for (slot, key) in infos {
            let initialised = props.get(slot).is_some_and(|v| !v.is_undef());
            if initialised || include_uninit {
                out.push((key, PropLoc::Slot(slot), initialised));
            }
        }
    }
    if let Some(d) = o.obj().dyn_props.borrow().as_ref() {
        for (k, _) in d.iter() {
            let name = k.to_php_str();
            out.push((name.clone(), PropLoc::Dynamic(name), true));
        }
    }
    out
}

/// Turns a slot into a reference (`ZVAL_MAKE_REF`) and returns it.
fn make_ref(slot: &mut Value) -> PhpRef {
    match slot {
        Value::Ref(r) => r.clone(),
        other => {
            let r = PhpRef::new(std::mem::take(other));
            *other = Value::Ref(r.clone());
            r
        }
    }
}

struct Walk<'a> {
    callback: &'a Value,
    userdata: Option<&'a Value>,
    recursive: bool,
    /// Arrays currently being descended into (`GC_PROTECT_RECURSION`).
    guard: Vec<usize>,
}

impl Walk<'_> {
    fn call_for(&mut self, vm: &mut Vm, r: PhpRef, key: Value) -> PResult<()> {
        let mut cargs = vec![Value::Ref(r), key];
        if let Some(u) = self.userdata {
            cargs.push(u.clone());
        }
        call(vm, self.callback, cargs)?;
        Ok(())
    }

    /// Descends into the array held by `r` (`array_walk_recursive`).
    fn descend(&mut self, vm: &mut Vm, r: &PhpRef) -> PResult<()> {
        // SEPARATE_ARRAY
        let ptr = {
            let mut b = r.borrow_mut();
            match &mut *b {
                Value::Array(rc) => {
                    Rc::make_mut(rc);
                    Rc::as_ptr(rc) as usize
                }
                _ => 0,
            }
        };
        if self.guard.contains(&ptr) {
            return Err(vm.throw_error("Error", "Recursion detected"));
        }
        self.guard.push(ptr);
        let res = self.walk(vm, r);
        self.guard.pop();
        res
    }

    /// `php_array_walk` on the array or object held by `var`.
    fn walk(&mut self, vm: &mut Vm, var: &PhpRef) -> PResult<()> {
        let obj = match &*var.borrow() {
            Value::Object(o) => Some(o.clone()),
            _ => None,
        };
        if let Some(o) = obj {
            return self.walk_object(vm, var, &o);
        }
        let is_empty = match &*var.borrow() {
            Value::Array(a) => a.is_empty(),
            _ => true,
        };
        if is_empty {
            return Ok(());
        }
        let mut pos: Pos = Pos::MIN;
        loop {
            // Retrieve the element at the iterator position and make it a
            // reference ("the location of the value may be freed otherwise").
            let (r, key) = {
                let mut b = var.borrow_mut();
                let Value::Array(rc) = &mut *b else {
                    break;
                };
                let Some(p) = rc.valid_pos(pos) else {
                    break;
                };
                let key = rc.key_at(p).expect("live position");
                // Move to the next element already now (like foreach).
                pos = rc.pos_after(p);
                let arr = Rc::make_mut(rc);
                let slot = arr.value_at_mut(p).expect("live position");
                (make_ref(slot), key)
            };
            let nested_array = self.recursive && matches!(&*r.borrow(), Value::Array(_));
            if nested_array {
                self.descend(vm, &r)?;
            } else {
                self.call_for(vm, r, key.into_value())?;
            }
            // Reload the array: it may have changed.
            let still = matches!(&*var.borrow(), Value::Array(_) | Value::Object(_));
            if !still {
                return Err(vm.throw_type_error("Iterated value is no longer an array or object"));
            }
        }
        Ok(())
    }

    fn walk_object(&mut self, vm: &mut Vm, var: &PhpRef, o: &ObjRef) -> PResult<()> {
        let props = object_props(o, false);
        for (key, loc, _) in props {
            let r = match &loc {
                PropLoc::Slot(slot) => {
                    let mut p = o.obj().props.borrow_mut();
                    match p.get_mut(*slot) {
                        Some(v) if !v.is_undef() => make_ref(v),
                        _ => continue,
                    }
                }
                PropLoc::Dynamic(name) => {
                    let mut d = o.obj().dyn_props.borrow_mut();
                    let Some(t) = d.as_mut() else { continue };
                    match t.get_mut(&ArrayKey::Str(name.clone())) {
                        Some(v) => make_ref(v),
                        None => continue,
                    }
                }
            };
            let nested_array = self.recursive && matches!(&*r.borrow(), Value::Array(_));
            if nested_array {
                self.descend(vm, &r)?;
            } else {
                self.call_for(vm, r, Value::String(key))?;
            }
            let still = matches!(&*var.borrow(), Value::Array(_) | Value::Object(_));
            if !still {
                return Err(vm.throw_type_error("Iterated value is no longer an array or object"));
            }
        }
        Ok(())
    }
}

fn walk_impl(vm: &mut Vm, args: &mut Args, recursive: bool, fname: &str) -> PResult<Value> {
    let var = args.by_ref(0);
    if matches!(&*var.borrow(), Value::Object(_)) {
        vm.deprecated(format!(
            "Passing an object for argument #1 $array to {fname}() is deprecated, call get_object_vars() first instead"
        ))?;
    }
    let callback = args.get(1).clone();
    let userdata = if args.is_passed(2) {
        Some(args.get(2).clone())
    } else {
        None
    };
    let mut w = Walk {
        callback: &callback,
        userdata: userdata.as_ref(),
        recursive,
        guard: Vec::new(),
    };
    w.walk(vm, &var)?;
    Ok(Value::Bool(true))
}

fn array_walk(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    walk_impl(vm, args, false, "array_walk")
}

fn array_walk_recursive(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    walk_impl(vm, args, true, "array_walk_recursive")
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "end(array|object &$array): mixed",
        handler: end,
    },
    NativeFunction {
        sig: "prev(array|object &$array): mixed",
        handler: prev,
    },
    NativeFunction {
        sig: "next(array|object &$array): mixed",
        handler: next,
    },
    NativeFunction {
        sig: "reset(array|object &$array): mixed",
        handler: reset,
    },
    NativeFunction {
        sig: "current(array|object $array): mixed",
        handler: current,
    },
    NativeFunction {
        sig: "pos(array|object $array): mixed",
        handler: current,
    },
    NativeFunction {
        sig: "key(array|object $array): int|string|null",
        handler: key,
    },
    NativeFunction {
        sig: "array_walk(array|object &$array, callable $callback, mixed $arg = UNKNOWN): true",
        handler: array_walk,
    },
    NativeFunction {
        sig: "array_walk_recursive(array|object &$array, callable $callback, mixed $arg = UNKNOWN): true",
        handler: array_walk_recursive,
    },
];
