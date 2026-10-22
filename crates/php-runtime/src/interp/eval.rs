//! Expression evaluation (the `zend_vm_def.h` handlers for expression opcodes).

use std::rc::Rc;

use crate::class::{CastKind, ClassRef};
use crate::compile::ir::*;
use crate::errors::level;
use crate::object::ObjRef;
use crate::value::array::ArrayKey;
use crate::value::convert::OffsetKind;
use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::{Ctx, SHORT_CIRCUIT, call, objects, place};

/// Evaluates `e` to a value (never `Undef`; a `Ref` only for `MakeRef`).
pub fn eval(vm: &mut Vm, cx: &Ctx, e: &Expr) -> PResult<Value> {
    match e {
        Expr::Lit(v) => Ok(v.clone()),
        Expr::Var(cv, line) => read_var(vm, cx, *cv, *line),
        Expr::Bin(op, a, b, line) => {
            let l = eval(vm, cx, a)?;
            let r = eval(vm, cx, b)?;
            vm.current_line = *line;
            binary_op(vm, *op, &l, &r)
        }
        Expr::Assign(lv, v, line) => place::assign_expr(vm, cx, lv, v, *line),
        Expr::Call(c) => call::eval_call(vm, cx, c),
        Expr::Place(lv) => place::read_place(vm, cx, lv, false),
        Expr::PlaceIs(lv) => place::read_place(vm, cx, lv, true),
        Expr::IncDec(op, lv, line) => place::incdec(vm, cx, *op, lv, *line),
        Expr::AssignOp(op, lv, v, line) => place::assign_op(vm, cx, *op, lv, v, *line),
        Expr::MethodCall(m) => call::eval_method_call(vm, cx, m),
        Expr::StaticCall(s) => call::eval_static_call(vm, cx, s),
        Expr::New(n) => call::eval_new(vm, cx, n),
        Expr::Interp(parts) => interp_string(vm, cx, parts).map(Value::String),
        Expr::This(line) => match &cx.env.this {
            Some(o) => Ok(Value::Object(o.clone())),
            None => {
                vm.current_line = *line;
                Err(vm.throw_error("Error", "Using $this when not in object context"))
            }
        },
        Expr::Globals => {
            let g = vm.global_env();
            Ok(Value::array(g.to_array()))
        }
        Expr::Array(items, line) => build_array(vm, cx, items, *line),
        Expr::Const(c, line) => fetch_const(vm, c, *line),
        Expr::ClassMagic => Ok(match cx.scope() {
            Some(c) => Value::String(c.name.clone()),
            None => Value::empty_string(),
        }),
        Expr::ClassConst(cc, line) => {
            let (cs, member) = &**cc;
            class_constant(vm, cx, cs, member, *line)
        }
        Expr::ClassName(cs, line) => class_name_of(vm, cx, cs, *line),
        Expr::CallableConvert(c, line) => call::callable_convert(vm, cx, c, *line),
        Expr::AssignRef(lv, src, line) => place::assign_ref(vm, cx, lv, src, *line),
        Expr::AssignCoalesce(lv, v, line) => place::assign_coalesce(vm, cx, lv, v, *line),
        Expr::And(a, b) => {
            if !eval_bool(vm, cx, a)? {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(eval_bool(vm, cx, b)?))
        }
        Expr::Or(a, b) => {
            if eval_bool(vm, cx, a)? {
                return Ok(Value::Bool(true));
            }
            Ok(Value::Bool(eval_bool(vm, cx, b)?))
        }
        Expr::Coalesce(a, b) => {
            let v = match eval(vm, cx, a) {
                Ok(v) => v,
                Err(Flow::Break(SHORT_CIRCUIT)) => Value::Null,
                Err(e) => return Err(e),
            };
            if v.is_null() {
                eval(vm, cx, b)
            } else {
                Ok(v)
            }
        }
        Expr::Unary(op, a, line) => {
            let v = eval(vm, cx, a)?;
            vm.current_line = *line;
            unary_op(vm, *op, &v)
        }
        Expr::Cast(op, a, line) => {
            let v = eval(vm, cx, a)?;
            vm.current_line = *line;
            cast(vm, *op, v)
        }
        Expr::Silence(inner) => {
            let tok = vm.begin_silence();
            let r = eval(vm, cx, inner);
            vm.end_silence(tok);
            r
        }
        Expr::Isset(lvs) => {
            for lv in lvs {
                if !place::isset(vm, cx, lv)? {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        Expr::Empty(inner) => {
            let v = match &**inner {
                Expr::PlaceIs(lv) => place::empty_place(vm, cx, lv)?,
                Expr::Chain(c) => match &**c {
                    Expr::PlaceIs(lv) => match place::empty_place(vm, cx, lv) {
                        Ok(b) => b,
                        Err(Flow::Break(SHORT_CIRCUIT)) => true,
                        Err(e) => return Err(e),
                    },
                    other => {
                        let v = eval(vm, cx, other)?;
                        !vm.to_bool(&v)?
                    }
                },
                other => {
                    let v = eval(vm, cx, other)?;
                    !vm.to_bool(&v)?
                }
            };
            Ok(Value::Bool(v))
        }
        Expr::Include(op, path, line) => {
            let p = eval(vm, cx, path)?;
            vm.current_line = *line;
            super::include::include(vm, cx, *op, p)
        }
        Expr::Eval(code, line) => {
            let c = eval(vm, cx, code)?;
            vm.current_line = *line;
            super::include::eval_string(vm, cx, c)
        }
        Expr::Print(inner) => {
            let v = eval(vm, cx, inner)?;
            let s = to_str(vm, &v)?;
            vm.echo(&s)?;
            Ok(Value::Int(1))
        }
        Expr::Exit(arg, line) => {
            let v = match arg {
                Some(a) => Some(eval(vm, cx, a)?),
                None => None,
            };
            vm.current_line = *line;
            do_exit(vm, v)
        }
        Expr::Clone(obj, with, line) => {
            let o = eval(vm, cx, obj)?;
            let w = match with {
                Some(w) => Some(eval(vm, cx, w)?),
                None => None,
            };
            vm.current_line = *line;
            objects::clone_value(vm, cx.scope().cloned(), o, w)
        }
        Expr::Closure(c, line) => {
            vm.current_line = *line;
            super::closure::create_closure(vm, cx, c)
        }
        Expr::Ternary(c, t, el) => {
            match t {
                Some(t) => {
                    if eval_bool(vm, cx, c)? {
                        eval(vm, cx, t)
                    } else {
                        eval(vm, cx, el)
                    }
                }
                None => {
                    let v = eval(vm, cx, c)?;
                    if vm.to_bool(&v)? {
                        Ok(v)
                    } else {
                        eval(vm, cx, el)
                    }
                }
            }
        }
        Expr::Match(subject, arms, line) => eval_match(vm, cx, subject, arms, *line),
        Expr::Instanceof(v, cs, line) => {
            let v = eval(vm, cx, v)?;
            vm.current_line = *line;
            instance_of(vm, cx, &v, cs).map(Value::Bool)
        }
        Expr::Throw(v, line) => {
            let v = eval(vm, cx, v)?;
            vm.current_line = *line;
            throw_value(vm, v)
        }
        Expr::Yield(k, v, line) => super::generator::eval_yield(vm, cx, k.as_deref(), v.as_deref(), *line),
        Expr::YieldFrom(v, line) => super::generator::eval_yield_from(vm, cx, v, *line),
        Expr::ShellExec(parts, line) => {
            let cmd = interp_string(vm, cx, parts)?;
            vm.current_line = *line;
            match vm.find_function(b"shell_exec") {
                Some(f) => vm.call_function(&f, crate::native::CallArgs::new(vec![Value::String(cmd)])),
                None => Err(vm.throw_error("Error", "Call to undefined function shell_exec()")),
            }
        }
        Expr::Assert(args, text, line) => super::builtins::eval_assert(vm, cx, args, text, *line),
        Expr::Pipe(lhs, rhs, line) => {
            let arg = eval(vm, cx, lhs)?;
            let f = eval(vm, cx, rhs)?;
            vm.current_line = *line;
            call::call_value(vm, &f, vec![arg], Vec::new(), false)
        }
        Expr::Void(inner) => {
            eval(vm, cx, inner)?;
            Ok(Value::Null)
        }
        Expr::Chain(inner) => match eval(vm, cx, inner) {
            Err(Flow::Break(SHORT_CIRCUIT)) => Ok(Value::Null),
            r => r,
        },
        Expr::MakeRef(lv, line) => {
            vm.current_line = *line;
            Ok(Value::Ref(place::make_ref(vm, cx, lv)?))
        }
    }
}

/// Evaluates a condition (`zend_is_true`).
#[inline]
pub fn eval_bool(vm: &mut Vm, cx: &Ctx, e: &Expr) -> PResult<bool> {
    // Fast paths for comparisons of scalars.
    if let Expr::Bin(op, a, b, line) = e {
        let l = eval(vm, cx, a)?;
        let r = eval(vm, cx, b)?;
        if let (Value::Int(x), Value::Int(y)) = (&l, &r) {
            match op {
                BinOp::Less => return Ok(x < y),
                BinOp::LessEqual => return Ok(x <= y),
                BinOp::Greater => return Ok(x > y),
                BinOp::GreaterEqual => return Ok(x >= y),
                BinOp::Equal | BinOp::Identical => return Ok(x == y),
                BinOp::NotEqual | BinOp::NotIdentical => return Ok(x != y),
                _ => {}
            }
        }
        vm.current_line = *line;
        let v = binary_op(vm, *op, &l, &r)?;
        return to_bool(vm, &v);
    }
    let v = eval(vm, cx, e)?;
    to_bool(vm, &v)
}

#[inline]
pub fn to_bool(vm: &mut Vm, v: &Value) -> PResult<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Int(i) => Ok(*i != 0),
        Value::Null | Value::Undef => Ok(false),
        _ => vm.to_bool(v),
    }
}

/// `(string)` conversion with the engine's diagnostics.
#[inline]
pub fn to_str(vm: &mut Vm, v: &Value) -> PResult<PhpStr> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => vm.to_php_string(v),
    }
}

/// Reads a compiled variable (`ZEND_FETCH_R` on a CV).
#[inline]
pub fn read_var(vm: &mut Vm, cx: &Ctx, cv: u32, line: u32) -> PResult<Value> {
    let slot = cx.slot(cv);
    let v = {
        let vars = cx.env.vars.borrow();
        match &vars[slot as usize] {
            Value::Ref(r) => r.get(),
            v => v.clone(),
        }
    };
    if v.is_undef() {
        vm.current_line = line;
        undefined_var(vm, &cx.cv_name(cv))?;
        return Ok(Value::Null);
    }
    Ok(v)
}

/// "Undefined variable $x".
pub fn undefined_var(vm: &mut Vm, name: &[u8]) -> PResult<()> {
    let mut msg = b"Undefined variable $".to_vec();
    msg.extend_from_slice(name);
    vm.zend_error(level::E_WARNING, PhpStr::from_vec(msg))
}

/// String interpolation (`ZEND_ROPE_*`).
pub fn interp_string(vm: &mut Vm, cx: &Ctx, parts: &[InterpPart]) -> PResult<PhpStr> {
    let mut out: Vec<u8> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Lit(s) => out.extend_from_slice(s),
            InterpPart::Expr(e) => {
                let v = eval(vm, cx, e)?;
                match &v {
                    Value::String(s) => out.extend_from_slice(s),
                    Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    _ => {
                        let s = vm.to_php_string(&v)?;
                        out.extend_from_slice(&s);
                    }
                }
            }
        }
    }
    Ok(PhpStr::from_vec(out))
}

/// Binary operators.
pub fn binary_op(vm: &mut Vm, op: BinOp, l: &Value, r: &Value) -> PResult<Value> {
    // Integer fast paths.
    if let (Value::Int(a), Value::Int(b)) = (l, r) {
        let (a, b) = (*a, *b);
        match op {
            BinOp::Add => {
                return Ok(match a.checked_add(b) {
                    Some(v) => Value::Int(v),
                    None => Value::Float(a as f64 + b as f64),
                });
            }
            BinOp::Sub => {
                return Ok(match a.checked_sub(b) {
                    Some(v) => Value::Int(v),
                    None => Value::Float(a as f64 - b as f64),
                });
            }
            BinOp::Mul => {
                return Ok(match a.checked_mul(b) {
                    Some(v) => Value::Int(v),
                    None => Value::Float(a as f64 * b as f64),
                });
            }
            BinOp::Less => return Ok(Value::Bool(a < b)),
            BinOp::LessEqual => return Ok(Value::Bool(a <= b)),
            BinOp::Greater => return Ok(Value::Bool(a > b)),
            BinOp::GreaterEqual => return Ok(Value::Bool(a >= b)),
            BinOp::Equal | BinOp::Identical => return Ok(Value::Bool(a == b)),
            BinOp::NotEqual | BinOp::NotIdentical => return Ok(Value::Bool(a != b)),
            BinOp::BitAnd => return Ok(Value::Int(a & b)),
            BinOp::BitOr => return Ok(Value::Int(a | b)),
            BinOp::BitXor => return Ok(Value::Int(a ^ b)),
            BinOp::Spaceship => return Ok(Value::Int((a > b) as i64 - (a < b) as i64)),
            _ => {}
        }
    }
    match op {
        BinOp::Add => vm.add(l, r),
        BinOp::Sub => vm.sub(l, r),
        BinOp::Mul => vm.mul(l, r),
        BinOp::Div => vm.div(l, r),
        BinOp::Mod => vm.modulo(l, r),
        BinOp::Pow => vm.pow(l, r),
        BinOp::Concat => concat(vm, l, r),
        BinOp::BitAnd => vm.bitwise_and(l, r),
        BinOp::BitOr => vm.bitwise_or(l, r),
        BinOp::BitXor => vm.bitwise_xor(l, r),
        BinOp::Shl => vm.shift_left(l, r),
        BinOp::Shr => vm.shift_right(l, r),
        BinOp::Xor => vm.bool_xor(l, r),
        BinOp::Identical => Ok(Value::Bool(crate::value::ops::is_identical(l, r))),
        BinOp::NotIdentical => Ok(Value::Bool(!crate::value::ops::is_identical(l, r))),
        BinOp::Equal => Ok(Value::Bool(vm.is_equal(l, r)?)),
        BinOp::NotEqual => Ok(Value::Bool(vm.is_not_equal(l, r)?)),
        BinOp::Less => Ok(Value::Bool(vm.is_smaller(l, r)?)),
        BinOp::LessEqual => Ok(Value::Bool(vm.is_smaller_or_equal(l, r)?)),
        BinOp::Greater => Ok(Value::Bool(vm.is_smaller(r, l)?)),
        BinOp::GreaterEqual => Ok(Value::Bool(vm.is_smaller_or_equal(r, l)?)),
        BinOp::Spaceship => vm.spaceship(l, r),
    }
}

fn concat(vm: &mut Vm, l: &Value, r: &Value) -> PResult<Value> {
    if let (Value::String(a), Value::String(b)) = (l, r) {
        let mut v = Vec::with_capacity(a.len() + b.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        return Ok(Value::String(PhpStr::from_vec(v)));
    }
    vm.concat(l, r)
}

fn unary_op(vm: &mut Vm, op: UnOp, v: &Value) -> PResult<Value> {
    match op {
        UnOp::Not => Ok(Value::Bool(!to_bool(vm, v)?)),
        UnOp::BitNot => vm.bitwise_not(v),
        UnOp::Plus => vm.unary_plus(v),
        UnOp::Minus => match v {
            Value::Int(i) if *i != i64::MIN => Ok(Value::Int(-i)),
            _ => vm.negate(v),
        },
    }
}

/// Casts (`ZEND_CAST`).
pub fn cast(vm: &mut Vm, op: CastOp, v: Value) -> PResult<Value> {
    Ok(match op {
        CastOp::Int => match v {
            Value::Int(_) => v,
            _ => Value::Int(vm.to_int(&v)?),
        },
        CastOp::Float => match v {
            Value::Float(_) => v,
            _ => Value::Float(vm.to_float(&v)?),
        },
        CastOp::String => match v {
            Value::String(_) => v,
            _ => Value::String(vm.to_php_string(&v)?),
        },
        CastOp::Bool => Value::Bool(to_bool(vm, &v)?),
        CastOp::Array => match v {
            Value::Array(_) => v,
            Value::Object(ref o) => Value::array(objects::object_to_array(vm, o)?),
            _ => Value::array(vm.cast_to_array(&v)?),
        },
        CastOp::Object => match v {
            Value::Object(_) => v,
            _ => Value::Object(vm.cast_to_object(&v)?),
        },
    })
}

/// Array literal construction (`ZEND_INIT_ARRAY` / `ZEND_ADD_ARRAY_ELEMENT` /
/// `ZEND_ADD_ARRAY_UNPACK`).
fn build_array(vm: &mut Vm, cx: &Ctx, items: &[ArrayItem], line: u32) -> PResult<Value> {
    let mut arr = PhpArray::with_capacity(items.len());
    for it in items {
        if it.spread {
            let v = eval(vm, cx, &it.value)?;
            vm.current_line = line;
            unpack_into(vm, &mut arr, v)?;
            continue;
        }
        let key = match &it.key {
            Some(k) => Some(eval(vm, cx, k)?),
            None => None,
        };
        let value = eval(vm, cx, &it.value)?;
        let value = if it.by_ref {
            value
        } else {
            value.into_unref()
        };
        match key {
            None => {
                if arr.append(value).is_err() {
                    vm.current_line = line;
                    return Err(vm.throw_error(
                        "Error",
                        "Cannot add element to the array as the next element is already occupied",
                    ));
                }
            }
            Some(k) => {
                vm.current_line = line;
                let key = array_key(vm, &k, OffsetKind::Write)?;
                arr.insert(key, value);
            }
        }
    }
    Ok(Value::array(arr))
}

/// Offset → key with diagnostics; illegal offsets in literals say "Illegal offset type".
pub fn array_key(vm: &mut Vm, k: &Value, kind: OffsetKind) -> PResult<ArrayKey> {
    match k {
        Value::Int(i) => Ok(ArrayKey::Int(*i)),
        Value::String(s) => Ok(ArrayKey::from_php_str(s.clone())),
        _ => vm.to_array_key(k, kind),
    }
}

/// `...$x` inside an array literal.
fn unpack_into(vm: &mut Vm, arr: &mut PhpArray, v: Value) -> PResult<()> {
    match v.into_unref() {
        Value::Array(a) => {
            for (k, v) in a.iter() {
                let v = v.unref();
                match k {
                    ArrayKey::Int(_) => {
                        if arr.append(v).is_err() {
                            return Err(vm.throw_error(
                                "Error",
                                "Cannot add element to the array as the next element is already occupied",
                            ));
                        }
                    }
                    ArrayKey::Str(_) => {
                        arr.insert(k, v);
                    }
                }
            }
            Ok(())
        }
        Value::Object(o) if o.class().instance_of_name(b"Traversable") => {
            let mut err = None;
            super::exec::iterate_object(vm, &o, false, &mut |vm, k, v| {
                let v = v.unref();
                match k {
                    Value::Int(_) | Value::Undef | Value::Null => {
                        if arr.append(v).is_err() {
                            err = Some(());
                            return Err(vm.throw_error(
                                "Error",
                                "Cannot add element to the array as the next element is already occupied",
                            ));
                        }
                    }
                    Value::String(s) => {
                        arr.insert(ArrayKey::from_php_str(s), v);
                    }
                    other => {
                        return Err(vm.throw_type_error(format!(
                            "Keys must be of type int|string during array unpacking, {} given",
                            other.type_name()
                        )));
                    }
                }
                Ok(true)
            })?;
            let _ = err;
            Ok(())
        }
        other => Err(vm.throw_error(
            "Error",
            format!("Only arrays and Traversables can be unpacked, {} given", other.value_name()),
        )),
    }
}

/// Constant fetch (`ZEND_FETCH_CONSTANT`).
fn fetch_const(vm: &mut Vm, c: &ConstRef, line: u32) -> PResult<Value> {
    if let Some(v) = c.cache.get() {
        return Ok(v.clone());
    }
    vm.current_line = line;
    if let Some(v) = vm.fetch_constant(&c.name)? {
        cache_const(vm, c, &c.name, &v);
        return Ok(v);
    }
    if let Some(fb) = &c.fallback {
        if let Some(v) = vm.fetch_constant(fb)? {
            cache_const(vm, c, fb, &v);
            return Ok(v);
        }
    }
    // `__COMPILER_HALT_OFFSET__` of the executing file.
    let name = c.fallback.as_ref().unwrap_or(&c.name);
    if name.as_bytes() == b"__COMPILER_HALT_OFFSET__" {
        let key = halt_offset_constant(&vm.current_file.clone());
        if let Some(v) = vm.constant(&key) {
            return Ok(v);
        }
    }
    Err(vm.throw_error("Error", format!("Undefined constant \"{}\"", c.display)))
}

fn cache_const(vm: &Vm, c: &ConstRef, name: &[u8], v: &Value) {
    if let Some(k) = vm.constants.get(name) {
        if k.flags & crate::constants::const_flags::PERSISTENT != 0
            && k.flags & crate::constants::const_flags::DEPRECATED == 0
            && !matches!(v, Value::Object(_))
        {
            let _ = c.cache.set(v.clone());
        }
    }
}

/// The mangled per-file name of `__COMPILER_HALT_OFFSET__`.
pub fn halt_offset_constant(file: &[u8]) -> PhpStr {
    let mut v = b"__COMPILER_HALT_OFFSET__\0".to_vec();
    v.extend_from_slice(file);
    PhpStr::from_vec(v)
}

/// Resolves a [`ClassSpec`] (`zend_fetch_class`).
pub fn resolve_class(vm: &mut Vm, cx: &Ctx, cs: &ClassSpec, line: u32) -> PResult<ClassRef> {
    match cs {
        ClassSpec::Named(n) => lookup_class_or_error(vm, n, line),
        ClassSpec::Self_ => match cx.scope() {
            Some(c) => Ok(c.clone()),
            None => {
                vm.current_line = line;
                Err(vm.throw_error("Error", "Cannot use \"self\" when no class scope is active"))
            }
        },
        ClassSpec::Parent => match cx.scope() {
            Some(c) => match &c.parent {
                Some(p) => Ok(p.clone()),
                None => {
                    vm.current_line = line;
                    Err(vm.throw_error(
                        "Error",
                        "Cannot use \"parent\" when current class scope has no parent",
                    ))
                }
            },
            None => {
                vm.current_line = line;
                Err(vm.throw_error("Error", "Cannot use \"parent\" when no class scope is active"))
            }
        },
        ClassSpec::Static => match &cx.env.called {
            Some(c) => Ok(c.clone()),
            None => match cx.scope() {
                Some(c) => Ok(c.clone()),
                None => {
                    vm.current_line = line;
                    Err(vm.throw_error("Error", "Cannot use \"static\" when no class scope is active"))
                }
            },
        },
        ClassSpec::Expr(e) => {
            let v = eval(vm, cx, e)?;
            vm.current_line = line;
            match v.into_unref() {
                Value::Object(o) => Ok(o.class().clone()),
                Value::String(s) => lookup_class_or_error(vm, &s, line),
                _ => Err(vm.throw_error("Error", "Cannot use value as class name")),
            }
        }
    }
}

pub fn lookup_class_or_error(vm: &mut Vm, name: &[u8], line: u32) -> PResult<ClassRef> {
    if let Some(c) = vm.find_class(name) {
        return Ok(c);
    }
    vm.current_line = line;
    match vm.lookup_class(name)? {
        Some(c) => Ok(c),
        None => {
            let n = name.strip_prefix(b"\\").unwrap_or(name);
            Err(vm.throw_error(
                "Error",
                format!("Class \"{}\" not found", String::from_utf8_lossy(n)),
            ))
        }
    }
}

/// `X::class`.
fn class_name_of(vm: &mut Vm, cx: &Ctx, cs: &ClassSpec, line: u32) -> PResult<Value> {
    match cs {
        ClassSpec::Named(n) => Ok(Value::String(n.clone())),
        ClassSpec::Expr(e) => {
            let v = eval(vm, cx, e)?;
            vm.current_line = line;
            match v.into_unref() {
                Value::Object(o) => Ok(Value::String(o.class().name.clone())),
                other => Err(vm.throw_type_error(format!(
                    "Cannot use \"::class\" on {}",
                    other.value_name()
                ))),
            }
        }
        _ => {
            let c = resolve_class(vm, cx, cs, line)?;
            Ok(Value::String(c.name.clone()))
        }
    }
}

/// `A::CONST` (`ZEND_FETCH_CLASS_CONSTANT`).
fn class_constant(vm: &mut Vm, cx: &Ctx, cs: &ClassSpec, member: &Member, line: u32) -> PResult<Value> {
    let class = resolve_class(vm, cx, cs, line)?;
    let name = match member {
        Member::Name(n) => n.clone(),
        Member::Expr(e) => {
            let v = eval(vm, cx, e)?;
            match v.into_unref() {
                Value::String(s) => s,
                other => {
                    vm.current_line = line;
                    return Err(vm.throw_type_error(format!(
                        "Cannot use value of type {} as class constant name",
                        other.type_name()
                    )));
                }
            }
        }
    };
    vm.current_line = line;
    super::decl::class_constant_value(vm, &class, &name, cx.scope())
}

/// `instanceof`.
pub fn instance_of(vm: &mut Vm, cx: &Ctx, v: &Value, cs: &ClassSpec) -> PResult<bool> {
    let Value::Object(o) = &*v.deref() else {
        // The class expression is still evaluated for dynamic classes.
        if let ClassSpec::Expr(e) = cs {
            let c = eval(vm, cx, e)?;
            if !matches!(c.deref().type_code(), crate::value::TypeCode::Object | crate::value::TypeCode::String) {
                return Err(vm.throw_error("Error", "Class name must be a valid object or a string"));
            }
        }
        return Ok(false);
    };
    let o = o.clone();
    match cs {
        ClassSpec::Named(n) => {
            // No autoloading: an unknown class is never an instance.
            Ok(match vm.find_class(n) {
                Some(c) => o.class().instance_of(&c),
                None => false,
            })
        }
        ClassSpec::Expr(e) => {
            let c = eval(vm, cx, e)?;
            match c.into_unref() {
                Value::Object(c) => Ok(o.class().instance_of(c.class())),
                Value::String(s) => Ok(match vm.find_class(&s) {
                    Some(c) => o.class().instance_of(&c),
                    None => false,
                }),
                _ => Err(vm.throw_error("Error", "Class name must be a valid object or a string")),
            }
        }
        _ => {
            let c = resolve_class(vm, cx, cs, vm.current_line)?;
            Ok(o.class().instance_of(&c))
        }
    }
}

/// `throw $v`.
pub fn throw_value(vm: &mut Vm, v: Value) -> PResult<Value> {
    match v.into_unref() {
        Value::Object(o) if o.class().instance_of_name(b"Throwable") => Err(Flow::Exception(o)),
        _ => Err(vm.throw_error("Error", "Can only throw objects")),
    }
}

/// `exit($status)`.
pub fn do_exit(vm: &mut Vm, v: Option<Value>) -> PResult<Value> {
    let code = match v.map(Value::into_unref) {
        None => 0,
        Some(Value::String(s)) => {
            vm.echo(&s)?;
            0
        }
        Some(Value::Int(i)) => i as i32,
        Some(Value::Bool(b)) if false => b as i32,
        Some(other) => {
            // exit() accepts string|int; other types are coerced (weak mode).
            match &other {
                Value::Float(_) | Value::Bool(_) => vm.to_int(&other)? as i32,
                Value::Null => {
                    vm.engine_deprecated(
                        "exit(): Passing null to parameter #1 ($status) of type string|int is deprecated",
                    )?;
                    0
                }
                _ => {
                    return Err(vm.throw_type_error(format!(
                        "exit(): Argument #1 ($status) must be of type string|int, {} given",
                        other.value_name()
                    )));
                }
            }
        }
    };
    vm.exit_status = code;
    vm.interp.exiting = true;
    Err(Flow::Exit(code))
}

/// `match` (`ZEND_MATCH` + `ZEND_MATCH_ERROR`).
fn eval_match(vm: &mut Vm, cx: &Ctx, subject: &Expr, arms: &[MatchArm], line: u32) -> PResult<Value> {
    let v = eval(vm, cx, subject)?;
    let mut default = None;
    for arm in arms {
        match &arm.conds {
            None => default = Some(&arm.body),
            Some(conds) => {
                for c in conds {
                    let cv = eval(vm, cx, c)?;
                    if crate::value::ops::is_identical(&v, &cv) {
                        return eval(vm, cx, &arm.body);
                    }
                }
            }
        }
    }
    if let Some(body) = default {
        return eval(vm, cx, body);
    }
    vm.current_line = line;
    let max_len = vm.ini_int("zend.exception_string_param_max_len").max(0) as usize;
    let ignore = vm.ini_bool("zend.exception_ignore_args");
    let printable = match &v {
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => true,
        Value::String(_) => max_len > 0,
        Value::Object(o) => o.class().is_enum(),
        _ => false,
    };
    let desc = if !ignore && printable {
        let mut out = Vec::new();
        crate::errors::append_trace_arg(&mut out, &v, max_len, vm.precision);
        String::from_utf8_lossy(&out).into_owned()
    } else {
        format!("of type {}", v.type_name())
    };
    Err(vm.throw_error("UnhandledMatchError", format!("Unhandled match case {desc}")))
}

/// Converts an object for string contexts (`__toString`).
pub fn object_to_string(vm: &mut Vm, o: &ObjRef) -> PResult<PhpStr> {
    match vm.object_cast(o, CastKind::String)? {
        Some(Value::String(s)) => Ok(s),
        _ => Err(vm.throw_error(
            "Error",
            format!("Object of class {} could not be converted to string", o.class().name),
        )),
    }
}

/// Keeps `Rc` imported for submodules that use `eval::*`.
pub(crate) type RcAlias<T> = Rc<T>;
