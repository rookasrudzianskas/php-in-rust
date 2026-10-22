//! The internal array pointer functions (`current`, `key`, `next`, `prev`,
//! `reset`, `end`) and `array_walk()` / `array_walk_recursive()`.

use std::collections::HashMap;
use std::rc::Rc;

use php_runtime::object::WeakObjRef;
use php_runtime::value::convert::{mangle_property_name, object_property_table};
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
/// table is rebuilt (`object_property_table`) for every call and the pointer
/// position is remembered here, keyed by object handle.
// TODO(runtime): expose the object's real property table so that the internal
// pointer lives in it (and survives property additions exactly like php-src).
#[derive(Default)]
struct ObjectIap {
    map: HashMap<u32, (WeakObjRef, Pos)>,
}

/// Runs `f` on the object's property table with its internal pointer
/// restored, then stores the pointer back.
fn with_object_table<R>(vm: &mut Vm, o: &ObjRef, f: impl FnOnce(&mut PhpArray) -> R) -> R {
    let mut table = object_property_table(o);
    let st = vm.ext_data::<ObjectIap>();
    if let Some((w, pos)) = st.map.get(&o.id()) {
        if w.upgrade().is_some_and(|x| ObjRef::ptr_eq(&x, o)) {
            table.set_internal_pointer(*pos);
        }
    }
    let r = f(&mut table);
    let pos = table.internal_pointer();
    let st = vm.ext_data::<ObjectIap>();
    st.map.insert(o.id(), (o.downgrade(), pos));
    r
}

/// `get_ht_for_iap` for objects: the deprecation.
fn deprecate_object_iap(vm: &mut Vm) -> PResult<()> {
    let name = vm.current_function_name();
    vm.deprecated(format!("Calling {name}() on an object is deprecated"))
}

fn current_value(a: &PhpArray) -> Value {
    match a.current() {
        Some(v) => v.unref(),
        None => Value::Bool(false),
    }
}

fn current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.get(0).clone() {
        Value::Array(a) => Ok(current_value(&a)),
        Value::Object(o) => {
            deprecate_object_iap(vm)?;
            Ok(with_object_table(vm, &o, |t| current_value(t)))
        }
        _ => Ok(Value::Bool(false)),
    }
}

fn key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    fn key_of(a: &PhpArray) -> Value {
        match a.current_key() {
            Some(k) => k.into_value(),
            None => Value::Null,
        }
    }
    match args.get(0).clone() {
        Value::Array(a) => Ok(key_of(&a)),
        Value::Object(o) => {
            deprecate_object_iap(vm)?;
            Ok(with_object_table(vm, &o, |t| key_of(t)))
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

fn apply_move(a: &mut PhpArray, m: Move) -> Value {
    if a.is_empty() {
        // The internal pointer of an empty array is already 0.
        return Value::Bool(false);
    }
    match m {
        Move::End => a.end(),
        Move::Prev => {
            a.move_backward();
        }
        Move::Next => {
            a.move_forward();
        }
        Move::Reset => a.reset(),
    }
    current_value(a)
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
        None => Ok(with_array_mut(vm, args, 0, |a| apply_move(a, m))),
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

/// The initialised properties of an object in property-table order, with the
/// (mangled) key `array_walk()` passes to the callback.
fn object_props(o: &ObjRef) -> Vec<(PhpStr, PropLoc)> {
    let mut out: Vec<(PhpStr, PropLoc)> = Vec::new();
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
            if props.get(slot).is_some_and(|v| !v.is_undef()) {
                out.push((key, PropLoc::Slot(slot)));
            }
        }
    }
    if let Some(d) = o.obj().dyn_props.borrow().as_ref() {
        for (k, _) in d.iter() {
            let name = k.to_php_str();
            out.push((name.clone(), PropLoc::Dynamic(name)));
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
        let props = object_props(o);
        for (key, loc) in props {
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
