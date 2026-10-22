//! Places: reads, writes, references, `isset`/`empty` and `unset` of
//! variables, array elements, properties and static properties
//! (`zend_fetch_dimension_address_*`, `ZEND_ASSIGN_*`, `ZEND_FETCH_*_W`).
//!
//! A write is done in two phases, like php-src's delayed oplines: first the
//! dynamic parts of the place (keys, property names, temporaries) are
//! evaluated in source order ([`prepare`]), then the container chain is
//! walked and modified. While walking arrays the walker holds a `RefCell`
//! borrow of the root slot, so it never runs user code there: whenever a
//! diagnostic, an object or a reference is met it returns, the borrow is
//! released, the event is handled, and the walk restarts (idempotently).

use std::rc::Rc;

use crate::class::ClassRef;
use crate::compile::ir::*;
use crate::errors::level;
use crate::object::ObjRef;
use crate::value::array::{ArrayKey, KeyCast, cast_key, make_element_ref};
use crate::value::convert::OffsetKind;
use crate::value::numeric::is_numeric_string_ex;
use crate::value::{PhpArray, PhpRef, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::eval::{self, eval, read_var, undefined_var};
use super::{Ctx, Env, SHORT_CIRCUIT, objects};

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// Reads a place (`BP_VAR_R`, or `BP_VAR_IS` when `is`).
pub fn read_place(vm: &mut Vm, cx: &Ctx, lv: &LVal, is: bool) -> PResult<Value> {
    match lv {
        LVal::Var(cv) => {
            if is {
                Ok(cx.env.get(cx.slot(*cv)).or_null())
            } else {
                read_var(vm, cx, *cv, vm.current_line)
            }
        }
        LVal::VarVar(name) => {
            let n = eval(vm, cx, name)?;
            let n = eval::to_str(vm, &n)?;
            read_named(vm, cx, cx.env, &n, is, false)
        }
        LVal::Global(name) => {
            let n = eval(vm, cx, name)?;
            let n = eval::to_str(vm, &n)?;
            let g = vm.global_env();
            read_named(vm, cx, &g, &n, is, true)
        }
        LVal::This => match &cx.env.this {
            Some(o) => Ok(Value::Object(o.clone())),
            None => {
                if is {
                    return Ok(Value::Null);
                }
                Err(vm.throw_error("Error", "Using $this when not in object context"))
            }
        },
        LVal::Dim(base, idx, line) => {
            let (container, key) = if let LVal::Var(cv) = &**base {
                let key = match idx {
                    Some(i) => Some(eval(vm, cx, i)?),
                    None => None,
                };
                let c = if is {
                    cx.env.get(cx.slot(*cv)).or_null()
                } else {
                    read_var(vm, cx, *cv, *line)?
                };
                (c, key)
            } else {
                let c = read_place(vm, cx, base, is)?;
                let key = match idx {
                    Some(i) => Some(eval(vm, cx, i)?),
                    None => None,
                };
                (c, key)
            };
            vm.current_line = *line;
            let Some(key) = key else {
                return Err(vm.throw_error("Error", "Cannot use [] for reading"));
            };
            dim_read(vm, &container, &key, is)
        }
        LVal::Prop(base, member, nullsafe, line) => {
            let (obj, name) = if let LVal::Var(cv) = &**base {
                let name = member_name(vm, cx, member)?;
                let o = if is {
                    cx.env.get(cx.slot(*cv)).or_null()
                } else {
                    read_var(vm, cx, *cv, *line)?
                };
                (o, name)
            } else {
                let o = read_place(vm, cx, base, is)?;
                if *nullsafe && o.is_null() {
                    return Err(Flow::Break(SHORT_CIRCUIT));
                }
                let name = member_name(vm, cx, member)?;
                (o, name)
            };
            if *nullsafe && obj.is_null() {
                return Err(Flow::Break(SHORT_CIRCUIT));
            }
            vm.current_line = *line;
            match obj {
                Value::Object(o) => objects::read_prop(vm, cx.scope(), &o, &name, is),
                other => {
                    if !is {
                        vm.engine_warning(format!(
                            "Attempt to read property \"{}\" on {}",
                            name,
                            other.value_name()
                        ))?;
                    }
                    Ok(Value::Null)
                }
            }
        }
        LVal::StaticProp(cs, member, line) => {
            let class = eval::resolve_class(vm, cx, cs, *line)?;
            let name = member_name(vm, cx, member)?;
            vm.current_line = *line;
            if is {
                match objects::static_prop_ref_opt(vm, cx.scope(), &class, &name)? {
                    Some(r) => Ok(r.get().or_null()),
                    None => Ok(Value::Null),
                }
            } else {
                let r = objects::static_prop_ref(vm, cx.scope(), &class, &name)?;
                let v = r.get();
                if v.is_undef() {
                    return Err(objects::uninit_static_error(vm, &class, &name));
                }
                Ok(v)
            }
        }
        LVal::Temp(e) => eval(vm, cx, e),
        LVal::List(_, _) => Err(vm.throw_error("Error", "Cannot use list() as standalone expression")),
    }
}

/// Reads a variable by name (`$$name`, `$GLOBALS['name']`).
fn read_named(vm: &mut Vm, cx: &Ctx, env: &Rc<Env>, name: &PhpStr, is: bool, global: bool) -> PResult<Value> {
    if name.as_bytes() == b"this" && !global {
        return match &cx.env.this {
            Some(o) => Ok(Value::Object(o.clone())),
            None => {
                if !is {
                    vm.engine_warning("Undefined variable $this")?;
                }
                Ok(Value::Null)
            }
        };
    }
    if !global && crate::compile::is_auto_global(name) {
        let g = vm.global_env();
        if let Some(s) = g.lookup(name) {
            return Ok(g.get(s).or_null());
        }
    }
    match env.lookup(name) {
        Some(s) if env.is_set(s) => Ok(env.get(s)),
        _ => {
            if !is {
                if global {
                    let mut msg = b"Undefined global variable $".to_vec();
                    msg.extend_from_slice(name);
                    vm.engine_warning(PhpStr::from_vec(msg))?;
                } else {
                    undefined_var(vm, name)?;
                }
            }
            Ok(Value::Null)
        }
    }
}

pub(crate) fn member_name(vm: &mut Vm, cx: &Ctx, m: &Member) -> PResult<PhpStr> {
    match m {
        Member::Name(n) => Ok(n.clone()),
        Member::Expr(e) => {
            let v = eval(vm, cx, e)?;
            eval::to_str(vm, &v)
        }
    }
}

fn undefined_key_warning(vm: &mut Vm, k: &ArrayKey) -> PResult<()> {
    match k {
        ArrayKey::Int(i) => vm.engine_warning(format!("Undefined array key {i}")),
        ArrayKey::Str(s) => {
            let mut m = b"Undefined array key \"".to_vec();
            m.extend_from_slice(s);
            m.push(b'"');
            vm.engine_warning(PhpStr::from_vec(m))
        }
    }
}

/// `$container[$key]` in read context.
pub fn dim_read(vm: &mut Vm, container: &Value, key: &Value, is: bool) -> PResult<Value> {
    match container {
        Value::Array(a) => {
            let found = match key {
                Value::Int(i) => a.get_int(*i).cloned(),
                Value::String(s) => a.get_sym(s).cloned(),
                _ => {
                    let k = if is {
                        match cast_key(key) {
                            KeyCast::Key(k) => k,
                            KeyCast::Illegal => {
                                return Err(vm.throw_type_error(format!(
                                    "Cannot access offset of type {} in isset or empty",
                                    key.type_name()
                                )));
                            }
                            _ => vm.to_array_key(key, OffsetKind::IsSet)?,
                        }
                    } else {
                        vm.to_array_key(key, OffsetKind::Read)?
                    };
                    match a.get(&k).cloned() {
                        Some(v) => Some(v),
                        None => {
                            if !is {
                                undefined_key_warning(vm, &k)?;
                            }
                            return Ok(Value::Null);
                        }
                    }
                }
            };
            match found {
                Some(v) => Ok(v.into_unref()),
                None => {
                    if !is {
                        let k = match key {
                            Value::Int(i) => ArrayKey::Int(*i),
                            Value::String(s) => ArrayKey::from_php_str(s.clone()),
                            _ => unreachable!(),
                        };
                        undefined_key_warning(vm, &k)?;
                    }
                    Ok(Value::Null)
                }
            }
        }
        Value::String(s) => string_offset_read(vm, s, key, is),
        Value::Object(o) => objects::dim_read_obj(vm, o, key, is),
        Value::Ref(r) => {
            let inner = r.get();
            dim_read(vm, &inner, key, is)
        }
        other => {
            if !is {
                vm.engine_warning(format!(
                    "Trying to access array offset on {}",
                    other.value_name()
                ))?;
            }
            Ok(Value::Null)
        }
    }
}

fn string_offset_read(vm: &mut Vm, s: &PhpStr, key: &Value, is: bool) -> PResult<Value> {
    let off = match key {
        Value::Int(i) => *i,
        Value::String(k) => match is_numeric_string_ex(k, true) {
            Some(info) if matches!(info.num, crate::value::numeric::Numeric::Int(_)) => {
                let crate::value::numeric::Numeric::Int(i) = info.num else {
                    unreachable!()
                };
                if info.trailing_data {
                    if is {
                        return Ok(Value::Null);
                    }
                    let mut m = b"Illegal string offset \"".to_vec();
                    m.extend_from_slice(k);
                    m.push(b'"');
                    vm.engine_warning(PhpStr::from_vec(m))?;
                }
                i
            }
            _ => {
                if is {
                    return Ok(Value::Null);
                }
                return Err(vm.throw_type_error(format!(
                    "Cannot access offset of type {} on string",
                    key.type_name()
                )));
            }
        },
        Value::Float(d) => {
            if !is {
                vm.engine_warning("String offset cast occurred")?;
            }
            crate::value::numeric::dval_to_lval_silent(*d)
        }
        Value::Null | Value::Undef | Value::Bool(_) => {
            if !is {
                vm.engine_warning("String offset cast occurred")?;
            }
            crate::value::convert::to_int_silent(key)
        }
        Value::Ref(r) => {
            let k = r.get();
            return string_offset_read(vm, s, &k, is);
        }
        _ => {
            if is {
                return Err(vm.throw_type_error(format!(
                    "Cannot access offset of type {} in isset or empty",
                    key.type_name()
                )));
            }
            return Err(vm.throw_type_error(format!(
                "Cannot access offset of type {} on string",
                key.type_name()
            )));
        }
    };
    let len = s.len() as i64;
    if off >= len || off < -len {
        if is {
            return Ok(Value::Null);
        }
        vm.engine_warning(format!("Uninitialized string offset {off}"))?;
        return Ok(Value::empty_string());
    }
    let real = if off < 0 { len + off } else { off } as usize;
    Ok(Value::String(PhpStr::from_bytes(&s[real..real + 1])))
}

// ---------------------------------------------------------------------------
// isset / empty
// ---------------------------------------------------------------------------

/// `isset($place)`.
pub fn isset(vm: &mut Vm, cx: &Ctx, lv: &LVal) -> PResult<bool> {
    match isset_inner(vm, cx, lv, false) {
        Err(Flow::Break(SHORT_CIRCUIT)) => Ok(false),
        r => r,
    }
}

/// `empty($place)`.
pub fn empty_place(vm: &mut Vm, cx: &Ctx, lv: &LVal) -> PResult<bool> {
    Ok(!isset_inner(vm, cx, lv, true)?)
}

/// `ZEND_ISSET_ISEMPTY_*`: with `empty`, returns whether the value is set and truthy.
fn isset_inner(vm: &mut Vm, cx: &Ctx, lv: &LVal, empty: bool) -> PResult<bool> {
    let check = |vm: &mut Vm, v: Value| -> PResult<bool> {
        if empty {
            if v.is_null() {
                return Ok(false);
            }
            eval::to_bool(vm, &v)
        } else {
            Ok(!v.is_null())
        }
    };
    match lv {
        LVal::Var(cv) => {
            let v = cx.env.get(cx.slot(*cv));
            check(vm, v)
        }
        LVal::This => Ok(cx.env.this.is_some()),
        LVal::Dim(base, idx, line) => {
            let container = read_place(vm, cx, base, true)?;
            let Some(idx) = idx else {
                return Err(vm.throw_error("Error", "Cannot use [] for reading"));
            };
            let key = eval(vm, cx, idx)?;
            vm.current_line = *line;
            match &container {
                Value::Object(o) => objects::dim_isset_obj(vm, o, &key, empty),
                Value::String(s) => {
                    let ok = string_offset_isset(s, &key);
                    if !ok {
                        if !matches!(
                            key,
                            Value::Int(_) | Value::String(_) | Value::Float(_) | Value::Bool(_) | Value::Null
                        ) {
                            return Err(vm.throw_type_error(format!(
                                "Cannot access offset of type {} in isset or empty",
                                key.type_name()
                            )));
                        }
                        return Ok(false);
                    }
                    if empty {
                        let v = string_offset_read(vm, s, &key, true)?;
                        return check(vm, v);
                    }
                    Ok(true)
                }
                Value::Array(_) => {
                    let v = dim_read(vm, &container, &key, true)?;
                    check(vm, v)
                }
                _ => Ok(false),
            }
        }
        LVal::Prop(base, member, nullsafe, line) => {
            let obj = read_place(vm, cx, base, true)?;
            if *nullsafe && obj.is_null() {
                return Err(Flow::Break(SHORT_CIRCUIT));
            }
            let name = member_name(vm, cx, member)?;
            vm.current_line = *line;
            match obj {
                Value::Object(o) => objects::isset_prop(vm, cx.scope(), &o, &name, empty),
                _ => Ok(false),
            }
        }
        LVal::StaticProp(cs, member, line) => {
            let class = match cs.as_ref() {
                ClassSpec::Named(n) => match vm.find_class(n) {
                    Some(c) => c,
                    None => match vm.lookup_class(n)? {
                        Some(c) => c,
                        None => return Ok(false),
                    },
                },
                other => eval::resolve_class(vm, cx, other, *line)?,
            };
            let name = member_name(vm, cx, member)?;
            match objects::static_prop_ref_opt(vm, cx.scope(), &class, &name)? {
                Some(r) => {
                    let v = r.get();
                    check(vm, v)
                }
                None => Ok(false),
            }
        }
        LVal::VarVar(_) | LVal::Global(_) | LVal::Temp(_) => {
            let v = read_place(vm, cx, lv, true)?;
            check(vm, v)
        }
        LVal::List(_, _) => Ok(false),
    }
}

fn string_offset_isset(s: &PhpStr, key: &Value) -> bool {
    let off = match key {
        Value::Int(i) => *i,
        Value::String(k) => match is_numeric_string_ex(k, false) {
            Some(info) => match info.num {
                crate::value::numeric::Numeric::Int(i) if !info.trailing_data => {
                    // Leading/trailing whitespace is not allowed in isset().
                    if k.first().is_some_and(|c| c.is_ascii_whitespace())
                        || k.last().is_some_and(|c| c.is_ascii_whitespace())
                    {
                        return false;
                    }
                    i
                }
                _ => return false,
            },
            None => return false,
        },
        Value::Float(d) => *d as i64,
        Value::Bool(b) => *b as i64,
        Value::Null => 0,
        _ => return false,
    };
    let len = s.len() as i64;
    off < len && off >= -len
}

// ---------------------------------------------------------------------------
// Write targets
// ---------------------------------------------------------------------------

/// A resolved path segment.
#[derive(Clone)]
pub(crate) enum Seg {
    Dim(Option<Value>),
    Prop(PhpStr),
}

/// The root of a write path.
pub(crate) enum Root {
    /// A variable slot of an environment.
    Var(Rc<Env>, u32),
    /// A reference (static property, array element reference, ...).
    Ref(PhpRef),
    /// `$this`.
    This(Option<ObjRef>),
    /// An object (from a temporary or `$this`): the next segment applies to it.
    Obj(ObjRef),
    /// A temporary value: writes have no lasting effect.
    Temp(Value),
}

/// The final operation applied at the end of a path.
pub(crate) enum Op {
    Assign(Value),
    AssignRef(PhpRef),
    MakeRef,
    /// Read for read-modify-write (`BP_VAR_RW`): warns on undefined keys and
    /// creates them as `null`.
    Fetch,
    /// Read without warnings or creation (`??=`).
    FetchIs,
    Unset,
    /// `.=` with a string right operand (in place when possible).
    Concat(PhpStr),
}

impl Op {
    fn is_unset(&self) -> bool {
        matches!(self, Op::Unset)
    }
    fn is_fetch_is(&self) -> bool {
        matches!(self, Op::FetchIs)
    }
    fn is_rw(&self) -> bool {
        matches!(self, Op::Fetch | Op::Concat(_))
    }
}

/// Outcome of a write: the value produced, and the key used for appends.
pub(crate) struct Done {
    pub value: Value,
    pub key: Option<ArrayKey>,
}

/// Evaluates the dynamic parts of a place in source order.
pub(crate) fn prepare(vm: &mut Vm, cx: &Ctx, lv: &LVal, segs: &mut Vec<(Seg, u32)>) -> PResult<Root> {
    match lv {
        LVal::Var(cv) => Ok(Root::Var(cx.env.clone(), cx.slot(*cv))),
        LVal::VarVar(name) => {
            let n = eval(vm, cx, name)?;
            let n = eval::to_str(vm, &n)?;
            if n.as_bytes() == b"this" {
                return Ok(Root::This(cx.env.this.clone()));
            }
            if crate::compile::is_auto_global(&n) {
                let g = vm.global_env();
                let s = g.slot_for(&n);
                return Ok(Root::Var(g, s));
            }
            let s = cx.env.slot_for(&n);
            Ok(Root::Var(cx.env.clone(), s))
        }
        LVal::Global(name) => {
            let n = eval(vm, cx, name)?;
            let n = eval::to_str(vm, &n)?;
            let g = vm.global_env();
            let s = g.slot_for(&n);
            Ok(Root::Var(g, s))
        }
        LVal::This => Ok(Root::This(cx.env.this.clone())),
        LVal::Dim(base, idx, line) => {
            let root = prepare(vm, cx, base, segs)?;
            let k = match idx {
                Some(i) => Some(eval(vm, cx, i)?),
                None => None,
            };
            segs.push((Seg::Dim(k), *line));
            Ok(root)
        }
        LVal::Prop(base, member, _, line) => {
            let root = prepare(vm, cx, base, segs)?;
            let n = member_name(vm, cx, member)?;
            segs.push((Seg::Prop(n), *line));
            Ok(root)
        }
        LVal::StaticProp(cs, member, line) => {
            let class = eval::resolve_class(vm, cx, cs, *line)?;
            let name = member_name(vm, cx, member)?;
            vm.current_line = *line;
            let r = objects::static_prop_ref(vm, cx.scope(), &class, &name)?;
            Ok(Root::Ref(r))
        }
        LVal::Temp(e) => {
            let v = eval(vm, cx, e)?;
            match v {
                Value::Object(o) => Ok(Root::Obj(o)),
                Value::Ref(r) => Ok(Root::Ref(r)),
                v => Ok(Root::Temp(v)),
            }
        }
        LVal::List(_, line) => {
            vm.current_line = *line;
            Err(vm.throw_error("Error", "Cannot use list() as standalone expression"))
        }
    }
}

/// Why the array walker stopped.
enum Stop {
    Done(Done),
    /// Emit the key-conversion diagnostic of segment `i`.
    KeyDiag(usize),
    /// `false` container at segment `i`.
    FalseToArray(usize),
    /// Undefined key at segment `i` in RW mode.
    Undefined(usize, ArrayKey),
    /// Undefined variable (RW / unset on an undefined CV root).
    UndefinedVar,
    /// Container at segment `i` is an object.
    Object(usize, ObjRef),
    /// Container slot at segment `i` is a reference.
    Ref(usize, PhpRef),
    /// Container at segment `i` is a string.
    Str(usize, PhpStr),
    /// Container at segment `i` is a scalar.
    Scalar(usize, Value),
    /// Throw (message, is_type_error).
    Error(String, bool),
    /// `.=` could not be done in place; fall back to fetch + assign.
    Slow,
}

const DIAG_DONE: u8 = 1;
const FALSE_DONE: u8 = 2;
const UNDEF_DONE: u8 = 4;
const UNDEF_VAR_DONE: u8 = 8;

/// Converts a key without diagnostics (`None` when it needs a diagnostic).
fn quick_key(k: &Value) -> Option<Result<ArrayKey, ()>> {
    match k {
        Value::Int(i) => Some(Ok(ArrayKey::Int(*i))),
        Value::String(s) => Some(Ok(ArrayKey::from_php_str(s.clone()))),
        Value::Bool(b) => Some(Ok(ArrayKey::Int(*b as i64))),
        Value::Float(_) | Value::Null | Value::Undef | Value::Resource(_) => match cast_key(k) {
            KeyCast::Key(k) => Some(Ok(k)),
            _ => None,
        },
        Value::Ref(r) => quick_key(&r.get()),
        _ => Some(Err(())),
    }
}

/// Walks `segs` inside `slot` (borrowed), stopping at the first event that
/// needs the borrow released.
fn walk(slot: &mut Value, segs: &[(Seg, u32)], op: &mut Option<Op>, flags: &[u8], root_is_cv: bool) -> Stop {
    let mut cur: &mut Value = slot;
    let n = segs.len();
    let mut i = 0;
    loop {
        // Dereference (switch root at references, except at the root itself).
        if let Value::Ref(r) = cur {
            if i > 0 || n > 0 {
                return Stop::Ref(i, r.clone());
            }
        }
        if i == n {
            // Apply the final op to the slot itself.
            let o = op.take().expect("op");
            return Stop::Done(apply_slot(cur, o));
        }
        let (seg, _line) = &segs[i];
        let Seg::Dim(key) = seg else {
            // Property segment: container must be an object.
            match cur {
                Value::Object(o) => return Stop::Object(i, o.clone()),
                other => return Stop::Scalar(i, other.clone()),
            }
        };
        let o = op.as_ref().expect("op");
        let last = i + 1 == n;
        // Container conversion.
        match cur {
            Value::Array(_) => {}
            Value::Undef | Value::Null => {
                if o.is_unset() || o.is_fetch_is() {
                    if matches!(cur, Value::Undef) && i == 0 && root_is_cv && o.is_unset() && flags[0] & UNDEF_VAR_DONE == 0 {
                        return Stop::UndefinedVar;
                    }
                    return Stop::Done(Done {
                        value: Value::Undef,
                        key: None,
                    });
                }
                if matches!(cur, Value::Undef) && i == 0 && root_is_cv && o.is_rw() && flags[0] & UNDEF_VAR_DONE == 0 {
                    return Stop::UndefinedVar;
                }
                *cur = Value::empty_array();
            }
            Value::Bool(false) => {
                if o.is_unset() || o.is_fetch_is() {
                    return Stop::Done(Done {
                        value: Value::Undef,
                        key: None,
                    });
                }
                if flags[i] & FALSE_DONE == 0 {
                    return Stop::FalseToArray(i);
                }
                *cur = Value::empty_array();
            }
            Value::Object(obj) => return Stop::Object(i, obj.clone()),
            Value::String(s) => return Stop::Str(i, s.clone()),
            other => {
                if o.is_fetch_is() {
                    return Stop::Done(Done {
                        value: Value::Undef,
                        key: None,
                    });
                }
                return Stop::Scalar(i, other.clone());
            }
        }
        let Value::Array(arr) = cur else { unreachable!() };
        // Key conversion.
        let akey = match key {
            None => None,
            Some(k) => match quick_key(k) {
                Some(Ok(k)) => Some(k),
                Some(Err(())) => {
                    let ty = k.type_name();
                    let msg = if o.is_unset() {
                        format!("Cannot unset offset of type {ty} on array")
                    } else if o.is_fetch_is() {
                        format!("Cannot access offset of type {ty} in isset or empty")
                    } else {
                        format!("Cannot access offset of type {ty} on array")
                    };
                    return Stop::Error(msg, true);
                }
                None => {
                    if flags[i] & DIAG_DONE == 0 && !(o.is_unset() && matches!(k, Value::Null)) {
                        return Stop::KeyDiag(i);
                    }
                    match cast_key(k) {
                        KeyCast::Key(k) => Some(k),
                        KeyCast::Null => Some(ArrayKey::Str(PhpStr::new())),
                        KeyCast::FloatFractional(_, d) | KeyCast::FloatNotRepresentable(_, d) => {
                            Some(ArrayKey::Int(crate::value::numeric::dval_to_lval_silent(d)))
                        }
                        KeyCast::Resource(id) => Some(ArrayKey::Int(id)),
                        KeyCast::Illegal => unreachable!(),
                    }
                }
            },
        };
        if last {
            let a = Rc::make_mut(arr);
            let o = op.take().expect("op");
            return match (akey, o) {
                (None, Op::Assign(v)) => match a.append(v) {
                    Ok(k) => Stop::Done(Done {
                        value: a.get_int(k).cloned().unwrap_or_default().into_unref(),
                        key: Some(ArrayKey::Int(k)),
                    }),
                    Err(_) => Stop::Error(
                        "Cannot add element to the array as the next element is already occupied".into(),
                        false,
                    ),
                },
                (None, Op::AssignRef(r)) => match a.append(Value::Ref(r.clone())) {
                    Ok(k) => Stop::Done(Done {
                        value: Value::Ref(r),
                        key: Some(ArrayKey::Int(k)),
                    }),
                    Err(_) => Stop::Error(
                        "Cannot add element to the array as the next element is already occupied".into(),
                        false,
                    ),
                },
                (None, Op::MakeRef) => match a.append(Value::Null) {
                    Ok(k) => {
                        let r = make_element_ref(a, ArrayKey::Int(k));
                        Stop::Done(Done {
                            value: Value::Ref(r),
                            key: Some(ArrayKey::Int(k)),
                        })
                    }
                    Err(_) => Stop::Error(
                        "Cannot add element to the array as the next element is already occupied".into(),
                        false,
                    ),
                },
                (None, Op::Fetch) | (None, Op::Concat(_)) => match a.append(Value::Null) {
                    Ok(k) => Stop::Done(Done {
                        value: Value::Null,
                        key: Some(ArrayKey::Int(k)),
                    }),
                    Err(_) => Stop::Error(
                        "Cannot add element to the array as the next element is already occupied".into(),
                        false,
                    ),
                },
                (None, Op::FetchIs) | (None, Op::Unset) => Stop::Error("Cannot use [] for reading".into(), false),
                (Some(k), Op::Assign(v)) => {
                    match a.get_mut(&k) {
                        Some(Value::Ref(r)) => {
                            r.set(v.clone());
                        }
                        Some(slot) => *slot = v.clone(),
                        None => {
                            a.insert(k.clone(), v.clone());
                        }
                    }
                    Stop::Done(Done {
                        value: v,
                        key: Some(k),
                    })
                }
                (Some(k), Op::AssignRef(r)) => {
                    a.insert(k.clone(), Value::Ref(r.clone()));
                    Stop::Done(Done {
                        value: Value::Ref(r),
                        key: Some(k),
                    })
                }
                (Some(k), Op::MakeRef) => {
                    let r = make_element_ref(a, k.clone());
                    Stop::Done(Done {
                        value: Value::Ref(r),
                        key: Some(k),
                    })
                }
                (Some(k), Op::Fetch) => match a.get(&k) {
                    Some(v) => Stop::Done(Done {
                        value: v.unref(),
                        key: Some(k),
                    }),
                    None => {
                        if flags[i] & UNDEF_DONE == 0 {
                            *op = Some(Op::Fetch);
                            return Stop::Undefined(i, k);
                        }
                        a.insert(k.clone(), Value::Null);
                        Stop::Done(Done {
                            value: Value::Null,
                            key: Some(k),
                        })
                    }
                },
                (Some(k), Op::Concat(s)) => match a.get_mut(&k) {
                    Some(Value::String(cur)) => {
                        cur.push_bytes(&s);
                        Stop::Done(Done {
                            value: Value::String(cur.clone()),
                            key: Some(k),
                        })
                    }
                    Some(Value::Ref(r)) => {
                        let mut b = r.borrow_mut();
                        if let Value::String(cur) = &mut *b {
                            cur.push_bytes(&s);
                            let v = Value::String(cur.clone());
                            drop(b);
                            Stop::Done(Done { value: v, key: Some(k) })
                        } else {
                            drop(b);
                            *op = Some(Op::Concat(s));
                            Stop::Slow
                        }
                    }
                    _ => {
                        *op = Some(Op::Concat(s));
                        Stop::Slow
                    }
                },
                (Some(k), Op::FetchIs) => Stop::Done(Done {
                    value: a.get(&k).map(Value::unref).unwrap_or(Value::Undef),
                    key: Some(k),
                }),
                (Some(k), Op::Unset) => {
                    let old = a.remove(&k);
                    Stop::Done(Done {
                        value: old.unwrap_or_default(),
                        key: Some(k),
                    })
                }
            };
        }
        // Intermediate segment: descend.
        let next_is_obj = matches!(segs[i + 1].0, Seg::Prop(_));
        let a = Rc::make_mut(arr);
        match akey {
            None => {
                if o.is_unset() || o.is_fetch_is() {
                    return Stop::Error("Cannot use [] for reading".into(), false);
                }
                match a.append(if next_is_obj { Value::Null } else { Value::Null }) {
                    Ok(k) => {
                        cur = a.get_mut(&ArrayKey::Int(k)).expect("appended");
                    }
                    Err(_) => {
                        return Stop::Error(
                            "Cannot add element to the array as the next element is already occupied".into(),
                            false,
                        );
                    }
                }
            }
            Some(k) => {
                if !a.contains_key(&k) {
                    if o.is_unset() || o.is_fetch_is() {
                        return Stop::Done(Done {
                            value: Value::Undef,
                            key: None,
                        });
                    }
                    if o.is_rw() && flags[i] & UNDEF_DONE == 0 {
                        return Stop::Undefined(i, k);
                    }
                    a.insert(k.clone(), Value::Null);
                }
                cur = a.get_mut(&k).expect("present");
            }
        }
        i += 1;
    }
}

/// Applies the final op to a slot (the path is empty).
fn apply_slot(slot: &mut Value, op: Op) -> Done {
    let value = match op {
        Op::Assign(v) => {
            match slot {
                Value::Ref(r) => {
                    r.set(v.clone());
                }
                _ => *slot = v.clone(),
            }
            v
        }
        Op::AssignRef(r) => {
            *slot = Value::Ref(r.clone());
            Value::Ref(r)
        }
        Op::MakeRef => match slot {
            Value::Ref(r) => Value::Ref(r.clone()),
            other => {
                let v = std::mem::take(other);
                let r = PhpRef::new(if v.is_undef() { Value::Null } else { v });
                *other = Value::Ref(r.clone());
                Value::Ref(r)
            }
        },
        Op::Fetch | Op::FetchIs => slot.unref(),
        Op::Unset => std::mem::take(slot),
        Op::Concat(s) => {
            match slot {
                Value::String(cur) => {
                    cur.push_bytes(&s);
                    Value::String(cur.clone())
                }
                Value::Ref(r) => {
                    let mut b = r.borrow_mut();
                    if let Value::String(cur) = &mut *b {
                        cur.push_bytes(&s);
                        Value::String(cur.clone())
                    } else {
                        // Not a string: caller falls back (unreachable via exec_path).
                        b.clone()
                    }
                }
                other => other.clone(),
            }
        }
    };
    Done { value, key: None }
}

/// Runs `f` on the root slot.
fn with_root<R>(root: &mut Root, f: impl FnOnce(&mut Value) -> R) -> R {
    match root {
        Root::Var(env, s) => {
            let mut vars = env.vars.borrow_mut();
            let slot = &mut vars[*s as usize];
            match slot {
                Value::Ref(r) => {
                    let r = r.clone();
                    drop(vars);
                    let mut b = r.borrow_mut();
                    f(&mut b)
                }
                _ => f(slot),
            }
        }
        Root::Ref(r) => {
            let mut b = r.borrow_mut();
            f(&mut b)
        }
        Root::Temp(v) => f(v),
        Root::This(o) => {
            let mut v = match o {
                Some(o) => Value::Object(o.clone()),
                None => Value::Null,
            };
            f(&mut v)
        }
        Root::Obj(o) => {
            let mut v = Value::Object(o.clone());
            f(&mut v)
        }
    }
}

/// Executes a write path.
pub(crate) fn exec_path(vm: &mut Vm, cx: &Ctx, mut root: Root, mut segs: Vec<(Seg, u32)>, op: Op) -> PResult<Done> {
    let mut op = Some(op);
    let mut flags: Vec<u8> = vec![0; segs.len().max(1)];
    let mut root_is_cv = matches!(root, Root::Var(..));
    loop {
        // `$this` root: writes to `$this` itself are errors; properties go to the object.
        if let Root::This(o) = &root {
            match o {
                Some(o) => {
                    if segs.is_empty() {
                        return Err(vm.throw_error("Error", "Cannot re-assign $this"));
                    }
                    root = Root::Obj(o.clone());
                }
                None => {
                    if segs.is_empty() {
                        return Err(vm.throw_error("Error", "Cannot re-assign $this"));
                    }
                    return Err(vm.throw_error("Error", "Using $this when not in object context"));
                }
            }
        }
        if let Root::Obj(obj) = &root {
            let obj = obj.clone();
            if segs.is_empty() {
                let o = op.take().expect("op");
                return Ok(Done {
                    value: match o {
                        Op::Assign(v) => v,
                        _ => Value::Object(obj),
                    },
                    key: None,
                });
            }
            let (seg, line) = segs[0].clone();
            vm.current_line = line;
            match seg {
                Seg::Prop(name) => {
                    if segs.len() == 1 {
                        let o = op.take().expect("op");
                        return final_prop_op(vm, cx, &obj, &name, o);
                    }
                    let mode = if op.as_ref().is_some_and(|o| o.is_unset() || o.is_fetch_is()) {
                        objects::FetchMode::Unset
                    } else if op.as_ref().is_some_and(Op::is_rw) {
                        objects::FetchMode::ReadWrite
                    } else {
                        objects::FetchMode::Write
                    };
                    match objects::prop_for_write(vm, cx, &obj, &name, mode, matches!(segs[1].0, Seg::Dim(_)))? {
                        objects::PropTarget::Ref(r) => root = Root::Ref(r),
                        objects::PropTarget::Value(v) => {
                            root = match v {
                                Value::Object(o) => Root::Obj(o),
                                v => Root::Temp(v),
                            }
                        }
                        objects::PropTarget::Missing => {
                            return Ok(Done {
                                value: Value::Undef,
                                key: None,
                            });
                        }
                    }
                    segs.remove(0);
                    flags = vec![0; segs.len().max(1)];
                    root_is_cv = false;
                    continue;
                }
                Seg::Dim(key) => {
                    if segs.len() == 1 {
                        let o = op.take().expect("op");
                        return final_obj_dim_op(vm, &obj, key, o);
                    }
                    // `$obj[k][...]`: offsetGet, then write into the result.
                    let mode = op.as_ref().map(|o| o.is_unset() || o.is_fetch_is()).unwrap_or(false);
                    let v = objects::dim_fetch_obj_for_write(vm, &obj, key, mode)?;
                    root = match v {
                        Value::Object(o) => Root::Obj(o),
                        Value::Ref(r) => Root::Ref(r),
                        v => Root::Temp(v),
                    };
                    segs.remove(0);
                    flags = vec![0; segs.len().max(1)];
                    root_is_cv = false;
                    continue;
                }
            }
        }
        let stop = with_root(&mut root, |slot| walk(slot, &segs, &mut op, &flags, root_is_cv));
        match stop {
            Stop::Done(d) => return Ok(d),
            Stop::KeyDiag(i) => {
                vm.current_line = segs[i].1;
                let kind = if op.as_ref().is_some_and(Op::is_unset) {
                    OffsetKind::Unset
                } else {
                    OffsetKind::Write
                };
                if let Seg::Dim(Some(k)) = &segs[i].0 {
                    let k = k.clone();
                    let key = vm.to_array_key(&k, kind)?;
                    segs[i].0 = Seg::Dim(Some(key.to_value()));
                }
                flags[i] |= DIAG_DONE;
            }
            Stop::FalseToArray(i) => {
                vm.current_line = segs[i].1;
                vm.engine_deprecated("Automatic conversion of false to array is deprecated")?;
                flags[i] |= FALSE_DONE;
            }
            Stop::Undefined(i, k) => {
                vm.current_line = segs[i].1;
                undefined_key_warning(vm, &k)?;
                flags[i] |= UNDEF_DONE;
            }
            Stop::UndefinedVar => {
                if let Root::Var(env, s) = &root {
                    let name = env.name_of(*s);
                    undefined_var(vm, &name)?;
                }
                flags[0] |= UNDEF_VAR_DONE;
            }
            Stop::Ref(i, r) => {
                root = Root::Ref(r);
                segs.drain(..i);
                flags = vec![0; segs.len().max(1)];
                root_is_cv = false;
            }
            Stop::Object(i, o) => {
                root = Root::Obj(o);
                segs.drain(..i);
                flags = vec![0; segs.len().max(1)];
                root_is_cv = false;
            }
            Stop::Str(i, s) => {
                vm.current_line = segs[i].1;
                let last = i + 1 == segs.len();
                let o = op.take().expect("op");
                if !last {
                    let msg = match &segs[i + 1].0 {
                        Seg::Prop(_) => "Cannot use string offset as an object",
                        Seg::Dim(_) => "Cannot use string offset as an array",
                    };
                    return Err(vm.throw_error("Error", msg));
                }
                return string_offset_op(vm, &mut root, &segs, i, s, o);
            }
            Stop::Scalar(i, v) => {
                vm.current_line = segs[i].1;
                let o = op.take().expect("op");
                return scalar_container_error(vm, &segs[i].0, &v, o);
            }
            Stop::Error(msg, type_error) => {
                return Err(if type_error {
                    vm.throw_type_error(msg)
                } else {
                    vm.throw_error("Error", msg)
                });
            }
            Stop::Slow => {
                // `.=` on a non-string: fetch (RW), concat, assign.
                let Some(Op::Concat(s)) = op.take() else { unreachable!() };
                let mut segs2 = segs.clone();
                let root2 = clone_root(&root);
                let d = exec_path(vm, cx, root2, segs2.clone(), Op::Fetch)?;
                if let (Some(k), Some((Seg::Dim(None), _))) = (&d.key, segs2.last_mut().map(|x| (x.0.clone(), x.1))) {
                    let l = segs2.len() - 1;
                    segs2[l].0 = Seg::Dim(Some(k.to_value()));
                }
                let new = vm.concat(&d.value, &Value::String(s))?;
                return exec_path(vm, cx, root, segs2, Op::Assign(new));
            }
        }
    }
}

fn clone_root(r: &Root) -> Root {
    match r {
        Root::Var(e, s) => Root::Var(e.clone(), *s),
        Root::Ref(r) => Root::Ref(r.clone()),
        Root::This(o) => Root::This(o.clone()),
        Root::Obj(o) => Root::Obj(o.clone()),
        Root::Temp(v) => Root::Temp(v.clone()),
    }
}

/// Final property segment on an object.
fn final_prop_op(vm: &mut Vm, cx: &Ctx, obj: &ObjRef, name: &PhpStr, op: Op) -> PResult<Done> {
    let scope = cx.scope().cloned();
    let value = match op {
        Op::Assign(v) => objects::write_prop(vm, scope.as_ref(), obj, name, v, cx.strict())?,
        Op::AssignRef(r) => {
            objects::assign_prop_ref(vm, scope.as_ref(), obj, name, r.clone())?;
            Value::Ref(r)
        }
        Op::MakeRef => Value::Ref(objects::make_prop_ref(vm, scope.as_ref(), obj, name)?),
        Op::Fetch => objects::read_prop(vm, scope.as_ref(), obj, name, false)?,
        Op::FetchIs => {
            if objects::isset_prop(vm, scope.as_ref(), obj, name, false)? {
                objects::read_prop(vm, scope.as_ref(), obj, name, true)?
            } else {
                Value::Undef
            }
        }
        Op::Unset => {
            objects::unset_prop(vm, scope.as_ref(), obj, name)?;
            Value::Null
        }
        Op::Concat(s) => {
            let old = objects::read_prop(vm, scope.as_ref(), obj, name, false)?;
            let new = vm.concat(&old, &Value::String(s))?;
            objects::write_prop(vm, scope.as_ref(), obj, name, new, cx.strict())?
        }
    };
    Ok(Done { value, key: None })
}

/// Final dimension segment on an object (`ArrayAccess`).
fn final_obj_dim_op(vm: &mut Vm, obj: &ObjRef, key: Option<Value>, op: Op) -> PResult<Done> {
    let value = match op {
        Op::Assign(v) => {
            objects::dim_write_obj(vm, obj, key.as_ref(), v.clone())?;
            v
        }
        Op::Unset => {
            let k = key.unwrap_or(Value::Null);
            objects::dim_unset_obj(vm, obj, &k)?;
            Value::Null
        }
        Op::Fetch => {
            let k = key.unwrap_or(Value::Null);
            objects::dim_read_obj(vm, obj, &k, false)?
        }
        Op::FetchIs => {
            let k = key.unwrap_or(Value::Null);
            if objects::dim_isset_obj(vm, obj, &k, false)? {
                objects::dim_read_obj(vm, obj, &k, true)?
            } else {
                Value::Undef
            }
        }
        Op::Concat(s) => {
            let k = key.clone().unwrap_or(Value::Null);
            let old = objects::dim_read_obj(vm, obj, &k, false)?;
            let new = vm.concat(&old, &Value::String(s))?;
            objects::dim_write_obj(vm, obj, key.as_ref(), new.clone())?;
            new
        }
        Op::MakeRef | Op::AssignRef(_) => {
            let v = objects::dim_fetch_obj_for_write(vm, obj, key, false)?;
            match v {
                Value::Ref(r) => Value::Ref(r),
                v => Value::Ref(PhpRef::new(v)),
            }
        }
    };
    Ok(Done { value, key: None })
}

/// Errors for scalar containers (`zend_wrong_*`).
fn scalar_container_error(vm: &mut Vm, seg: &Seg, v: &Value, op: Op) -> PResult<Done> {
    match seg {
        Seg::Prop(name) => {
            let what = match op {
                Op::Fetch | Op::Concat(_) => "modify",
                Op::Unset | Op::FetchIs => {
                    return Ok(Done {
                        value: Value::Undef,
                        key: None,
                    });
                }
                _ => "assign",
            };
            let msg = if what == "assign" && matches!(op, Op::MakeRef) {
                format!("Attempt to modify property \"{name}\" on {}", v.value_name())
            } else {
                format!("Attempt to {what} property \"{name}\" on {}", v.value_name())
            };
            Err(vm.throw_error("Error", msg))
        }
        Seg::Dim(_) => {
            if op.is_unset() {
                return Err(vm.throw_error("Error", "Cannot unset offset in a non-array variable"));
            }
            Err(vm.throw_error("Error", "Cannot use a scalar value as an array"))
        }
    }
}

/// Operations on a string offset (`$str[i] = 'c'`).
fn string_offset_op(vm: &mut Vm, root: &mut Root, segs: &[(Seg, u32)], i: usize, s: PhpStr, op: Op) -> PResult<Done> {
    let Seg::Dim(key) = &segs[i].0 else { unreachable!() };
    let v = match op {
        Op::Assign(v) => v,
        Op::Fetch | Op::Concat(_) => {
            return Err(vm.throw_error("Error", "Cannot use assign-op operators with string offsets"));
        }
        Op::MakeRef | Op::AssignRef(_) => {
            return Err(vm.throw_error("Error", "Cannot create references to/from string offsets"));
        }
        Op::Unset => return Err(vm.throw_error("Error", "Cannot unset string offsets")),
        Op::FetchIs => {
            let k = key.clone().unwrap_or(Value::Null);
            let v = string_offset_read(vm, &s, &k, true)?;
            return Ok(Done { value: v, key: None });
        }
    };
    let Some(key) = key else {
        return Err(vm.throw_error("Error", "[] operator not supported for strings"));
    };
    let offset = match key {
        Value::Int(i) => *i,
        Value::String(k) => match is_numeric_string_ex(k, true) {
            Some(info) if matches!(info.num, crate::value::numeric::Numeric::Int(_)) => {
                let crate::value::numeric::Numeric::Int(n) = info.num else { unreachable!() };
                if info.trailing_data {
                    let mut m = b"Illegal string offset \"".to_vec();
                    m.extend_from_slice(k);
                    m.push(b'"');
                    vm.engine_warning(PhpStr::from_vec(m))?;
                }
                n
            }
            _ => {
                return Err(vm.throw_type_error(format!(
                    "Cannot access offset of type {} on string",
                    key.type_name()
                )));
            }
        },
        Value::Float(d) => {
            vm.engine_warning("String offset cast occurred")?;
            crate::value::numeric::dval_to_lval_silent(*d)
        }
        Value::Null | Value::Bool(_) | Value::Undef => {
            vm.engine_warning("String offset cast occurred")?;
            crate::value::convert::to_int_silent(key)
        }
        other => {
            return Err(vm.throw_type_error(format!(
                "Cannot access offset of type {} on string",
                other.type_name()
            )));
        }
    };
    let len = s.len() as i64;
    if offset < -len {
        vm.engine_warning(format!("Illegal string offset {offset}"))?;
        return Ok(Done {
            value: Value::Null,
            key: None,
        });
    }
    let offset = if offset < 0 { offset + len } else { offset } as usize;
    let c = match &v {
        Value::String(x) => x.clone(),
        other => vm.to_php_string(other)?,
    };
    if c.len() != 1 {
        if c.is_empty() {
            return Err(vm.throw_error("Error", "Cannot assign an empty string to a string offset"));
        }
        vm.engine_warning("Only the first byte will be assigned to the string offset")?;
    }
    let byte = c[0];
    let mut bytes = s.to_vec();
    if offset >= bytes.len() {
        bytes.resize(offset, b' ');
        bytes.push(byte);
    } else {
        bytes[offset] = byte;
    }
    // Write the modified string back to its container.
    let new = Value::String(PhpStr::from_vec(bytes));
    let prefix: Vec<(Seg, u32)> = segs[..i].to_vec();
    let mut op = Some(Op::Assign(new));
    let flags = vec![DIAG_DONE | FALSE_DONE | UNDEF_DONE; prefix.len().max(1)];
    let _ = with_root(root, |slot| walk(slot, &prefix, &mut op, &flags, false));
    Ok(Done {
        value: Value::String(PhpStr::from_bytes(&[byte])),
        key: None,
    })
}

// ---------------------------------------------------------------------------
// Assignment expressions
// ---------------------------------------------------------------------------

/// `$place = expr`.
pub fn assign_expr(vm: &mut Vm, cx: &Ctx, lv: &LVal, value: &Expr, line: u32) -> PResult<Value> {
    match lv {
        LVal::Var(cv) => {
            let v = eval(vm, cx, value)?.into_unref();
            let old = cx.env.set(cx.slot(*cv), v.clone());
            drop(old);
            Ok(v)
        }
        LVal::List(items, l) => {
            let v = eval(vm, cx, value)?;
            destructure(vm, cx, items, v.clone(), *l)?;
            Ok(v.into_unref())
        }
        _ => {
            let mut segs = Vec::new();
            let root = prepare(vm, cx, lv, &mut segs)?;
            let v = eval(vm, cx, value)?.into_unref();
            vm.current_line = line;
            Ok(exec_path(vm, cx, root, segs, Op::Assign(v))?.value)
        }
    }
}

/// Assigns an already computed value (foreach targets, list elements, catch).
pub fn assign_value(vm: &mut Vm, cx: &Ctx, lv: &LVal, v: Value) -> PResult<()> {
    match lv {
        LVal::Var(cv) => {
            let old = cx.env.set(cx.slot(*cv), v.into_unref());
            drop(old);
            Ok(())
        }
        LVal::List(items, l) => destructure(vm, cx, items, v, *l),
        _ => {
            let mut segs = Vec::new();
            let root = prepare(vm, cx, lv, &mut segs)?;
            exec_path(vm, cx, root, segs, Op::Assign(v.into_unref()))?;
            Ok(())
        }
    }
}

/// Binds a place to a reference (`$place = &$ref`).
pub fn bind_ref(vm: &mut Vm, cx: &Ctx, lv: &LVal, r: PhpRef) -> PResult<()> {
    match lv {
        LVal::Var(cv) => {
            let old = cx.env.bind_ref(cx.slot(*cv), r);
            drop(old);
            Ok(())
        }
        LVal::List(items, l) => destructure(vm, cx, items, Value::Ref(r), *l),
        _ => {
            let mut segs = Vec::new();
            let root = prepare(vm, cx, lv, &mut segs)?;
            exec_path(vm, cx, root, segs, Op::AssignRef(r))?;
            Ok(())
        }
    }
}

/// `list(...) = $v` / `[...] = $v` (`ZEND_FETCH_LIST_R` / `_W`).
pub fn destructure(vm: &mut Vm, cx: &Ctx, items: &[Option<ListItem>], v: Value, line: u32) -> PResult<()> {
    let by_ref_source = match &v {
        Value::Ref(r) => Some(r.clone()),
        _ => None,
    };
    let container = v.unref();
    for (idx, item) in items.iter().enumerate() {
        let Some(item) = item else { continue };
        let key = match &item.key {
            Some(k) => eval(vm, cx, k)?,
            None => Value::Int(idx as i64),
        };
        vm.current_line = line;
        if item.by_ref {
            // Reference into the source array.
            let Some(src) = &by_ref_source else {
                return Err(vm.throw_error("Error", "Cannot assign reference to non referenceable value"));
            };
            let mut segs = vec![(Seg::Dim(Some(key)), line)];
            let _ = &mut segs;
            let d = exec_path(vm, cx, Root::Ref(src.clone()), segs, Op::MakeRef)?;
            let Value::Ref(r) = d.value else { unreachable!() };
            match &item.target {
                LVal::List(inner, l) => destructure(vm, cx, inner, Value::Ref(r), *l)?,
                t => bind_ref(vm, cx, t, r)?,
            }
            continue;
        }
        let elem = match &container {
            Value::Array(_) => dim_read(vm, &container, &key, false)?,
            Value::Object(o) => objects::dim_read_obj(vm, o, &key, false)?,
            Value::Null | Value::Undef => Value::Null,
            other => {
                // `[$a] = 1;` assigns null silently for scalars except strings.
                let _ = other;
                Value::Null
            }
        };
        match &item.target {
            LVal::List(inner, l) => destructure(vm, cx, inner, elem, *l)?,
            t => assign_value(vm, cx, t, elem)?,
        }
    }
    Ok(())
}

/// `$place op= expr`.
pub fn assign_op(vm: &mut Vm, cx: &Ctx, op: BinOp, lv: &LVal, value: &Expr, line: u32) -> PResult<Value> {
    if let LVal::Var(cv) = lv {
        let slot = cx.slot(*cv);
        let rhs = eval(vm, cx, value)?;
        // `.=` on a string: append in place.
        if op == BinOp::Concat {
            if let Value::String(r) = &rhs {
                let mut vars = cx.env.vars.borrow_mut();
                match &mut vars[slot as usize] {
                    Value::String(s) => {
                        s.push_bytes(r);
                        return Ok(Value::String(s.clone()));
                    }
                    Value::Ref(rf) => {
                        let mut b = rf.borrow_mut();
                        if let Value::String(s) = &mut *b {
                            s.push_bytes(r);
                            return Ok(Value::String(s.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
        let cur = cx.env.get(slot);
        let cur = if cur.is_undef() {
            vm.current_line = line;
            undefined_var(vm, &cx.cv_name(*cv))?;
            Value::Null
        } else {
            cur
        };
        vm.current_line = line;
        let new = eval::binary_op(vm, op, &cur, &rhs)?;
        drop(cur);
        cx.env.set(slot, new.clone());
        return Ok(new);
    }
    let mut segs = Vec::new();
    let root = prepare(vm, cx, lv, &mut segs)?;
    let rhs = eval(vm, cx, value)?;
    vm.current_line = line;
    if op == BinOp::Concat {
        if let Value::String(s) = &rhs {
            let d = exec_path(vm, cx, root, segs, Op::Concat(s.clone()))?;
            return Ok(d.value);
        }
    }
    let root2 = clone_root(&root);
    let d = exec_path(vm, cx, root2, segs.clone(), Op::Fetch)?;
    if let (Some(k), Some((Seg::Dim(None), _))) = (&d.key, segs.last()) {
        let l = segs.len() - 1;
        segs[l].0 = Seg::Dim(Some(k.to_value()));
    }
    vm.current_line = line;
    let new = eval::binary_op(vm, op, &d.value, &rhs)?;
    drop(d);
    Ok(exec_path(vm, cx, root, segs, Op::Assign(new))?.value)
}

/// `$place ??= expr`.
pub fn assign_coalesce(vm: &mut Vm, cx: &Ctx, lv: &LVal, value: &Expr, line: u32) -> PResult<Value> {
    if let LVal::Var(cv) = lv {
        let slot = cx.slot(*cv);
        let cur = cx.env.get(slot);
        if !cur.is_null() {
            return Ok(cur);
        }
        let v = eval(vm, cx, value)?.into_unref();
        cx.env.set(slot, v.clone());
        return Ok(v);
    }
    let mut segs = Vec::new();
    let root = prepare(vm, cx, lv, &mut segs)?;
    vm.current_line = line;
    let root2 = clone_root(&root);
    let d = exec_path(vm, cx, root2, segs.clone(), Op::FetchIs)?;
    if !d.value.is_null() {
        return Ok(d.value);
    }
    let v = eval(vm, cx, value)?.into_unref();
    vm.current_line = line;
    Ok(exec_path(vm, cx, root, segs, Op::Assign(v))?.value)
}

/// `++$x`, `$x--`, ...
pub fn incdec(vm: &mut Vm, cx: &Ctx, op: IncDec, lv: &LVal, line: u32) -> PResult<Value> {
    let inc = matches!(op, IncDec::PreInc | IncDec::PostInc);
    let post = matches!(op, IncDec::PostInc | IncDec::PostDec);
    if let LVal::Var(cv) = lv {
        let slot = cx.slot(*cv);
        {
            // Fast path: integers.
            let mut vars = cx.env.vars.borrow_mut();
            let s = &mut vars[slot as usize];
            let target = match s {
                Value::Ref(_) => None,
                other => Some(other),
            };
            if let Some(Value::Int(i)) = target {
                let old = *i;
                let res = if inc { old.checked_add(1) } else { old.checked_sub(1) };
                if let Some(n) = res {
                    *i = n;
                    return Ok(Value::Int(if post { old } else { n }));
                }
            }
        }
        let cur = cx.env.get(slot);
        let cur = if cur.is_undef() {
            vm.current_line = line;
            undefined_var(vm, &cx.cv_name(*cv))?;
            Value::Null
        } else {
            cur
        };
        vm.current_line = line;
        let new = if inc { vm.increment(&cur)? } else { vm.decrement(&cur)? };
        cx.env.set(slot, new.clone());
        return Ok(if post { cur } else { new });
    }
    let mut segs = Vec::new();
    let root = prepare(vm, cx, lv, &mut segs)?;
    vm.current_line = line;
    // Properties: typed-property range checks live in the object layer.
    if let Some((Seg::Prop(name), _)) = segs.last() {
        if segs.len() == 1 {
            let obj = match &root {
                Root::This(Some(o)) | Root::Obj(o) => Some(o.clone()),
                _ => None,
            };
            if let Some(o) = obj {
                return objects::incdec_prop(vm, cx, &o, &name.clone(), inc, post);
            }
        }
    }
    let root2 = clone_root(&root);
    let d = match exec_path(vm, cx, root2, segs.clone(), Op::Fetch) {
        Ok(d) => d,
        Err(Flow::Exception(e)) => {
            // php-src reports string offsets specially for inc/dec.
            if crate::vm::Vm::exception_message(&e) == "Cannot use assign-op operators with string offsets" {
                return Err(vm.throw_error("Error", "Cannot increment/decrement string offsets"));
            }
            if crate::vm::Vm::exception_message(&e).starts_with("Attempt to modify property") {
                let m = crate::vm::Vm::exception_message(&e).replacen("Attempt to modify", "Attempt to increment/decrement", 1);
                return Err(vm.throw_error("Error", m));
            }
            return Err(Flow::Exception(e));
        }
        Err(f) => return Err(f),
    };
    if let (Some(k), Some((Seg::Dim(None), _))) = (&d.key, segs.last()) {
        let l = segs.len() - 1;
        segs[l].0 = Seg::Dim(Some(k.to_value()));
    }
    vm.current_line = line;
    let old = d.value;
    let new = if inc { vm.increment(&old)? } else { vm.decrement(&old)? };
    exec_path(vm, cx, root, segs, Op::Assign(new.clone()))?;
    Ok(if post { old } else { new })
}

/// `$place = &$source` (or `= &f()`).
pub fn assign_ref(vm: &mut Vm, cx: &Ctx, lv: &LVal, src: &Expr, line: u32) -> PResult<Value> {
    // Evaluate the target's dynamic parts first, like php-src's delayed oplines.
    let mut segs = Vec::new();
    let root = match lv {
        LVal::Var(_) | LVal::List(..) => None,
        _ => Some(prepare(vm, cx, lv, &mut segs)?),
    };
    let r = match src {
        Expr::MakeRef(slv, l) => {
            vm.current_line = *l;
            make_ref(vm, cx, slv)?
        }
        other => {
            let v = eval(vm, cx, other)?;
            match v {
                Value::Ref(r) => r,
                v => {
                    // A function not returning by reference.
                    vm.current_line = line;
                    if matches!(other, Expr::Call(_) | Expr::MethodCall(_) | Expr::StaticCall(_)) {
                        vm.engine_notice("Only variables should be assigned by reference")?;
                    }
                    PhpRef::new(v)
                }
            }
        }
    };
    vm.current_line = line;
    match root {
        None => {
            bind_ref(vm, cx, lv, r.clone())?;
        }
        Some(root) => {
            exec_path(vm, cx, root, segs, Op::AssignRef(r.clone()))?;
        }
    }
    Ok(r.get())
}

/// Fetches a place for reference-taking (`&$place`).
pub fn make_ref(vm: &mut Vm, cx: &Ctx, lv: &LVal) -> PResult<PhpRef> {
    match lv {
        LVal::Var(cv) => Ok(cx.env.make_ref(cx.slot(*cv))),
        LVal::This => Err(vm.throw_error("Error", "Cannot re-assign $this")),
        _ => {
            let mut segs = Vec::new();
            let root = prepare(vm, cx, lv, &mut segs)?;
            if segs.is_empty() {
                if let Root::Temp(v) = root {
                    return Ok(PhpRef::new(v));
                }
            }
            let d = exec_path(vm, cx, root, segs, Op::MakeRef)?;
            match d.value {
                Value::Ref(r) => Ok(r),
                v => Ok(PhpRef::new(v)),
            }
        }
    }
}

/// `unset($place)`.
pub fn unset(vm: &mut Vm, cx: &Ctx, lv: &LVal) -> PResult<()> {
    match lv {
        LVal::Var(cv) => {
            let old = cx.env.unset(cx.slot(*cv));
            drop(old);
            Ok(())
        }
        LVal::VarVar(name) => {
            let n = eval(vm, cx, name)?;
            let n = eval::to_str(vm, &n)?;
            if n.as_bytes() == b"this" {
                return Err(vm.throw_error("Error", "Cannot unset $this"));
            }
            if let Some(s) = cx.env.lookup(&n) {
                let old = cx.env.unset(s);
                drop(old);
            }
            Ok(())
        }
        LVal::Global(name) => {
            let n = eval(vm, cx, name)?;
            let n = eval::to_str(vm, &n)?;
            let g = vm.global_env();
            if let Some(s) = g.lookup(&n) {
                let old = g.unset(s);
                drop(old);
            }
            Ok(())
        }
        LVal::StaticProp(_, _, line) => {
            vm.current_line = *line;
            Err(vm.throw_error("Error", "Attempt to unset static property"))
        }
        _ => {
            let mut segs = Vec::new();
            let root = prepare(vm, cx, lv, &mut segs)?;
            let d = exec_path(vm, cx, root, segs, Op::Unset)?;
            drop(d);
            Ok(())
        }
    }
}

/// The class of a static property place (for `isset`), without autoloading.
#[allow(dead_code)]
pub(crate) fn class_for_isset(vm: &mut Vm, name: &[u8]) -> Option<ClassRef> {
    vm.find_class(name)
}

/// Converts an array key back to a value for re-walking.
#[allow(dead_code)]
pub(crate) fn key_value(k: &ArrayKey) -> Value {
    k.to_value()
}

