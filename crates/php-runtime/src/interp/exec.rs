//! Statement execution: control flow, `foreach` (arrays, by reference,
//! objects, iterators, generators), `switch`, `try/catch/finally`, `goto`,
//! `global`, `static`, declarations and ticks.

use std::rc::Rc;

use crate::compile::ir::*;
use crate::object::{self, ObjRef};
use crate::value::array::ArrayKey;
use crate::value::{PhpRef, PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::eval::{self, eval, eval_bool};
use super::{Ctx, GOTO, place};

/// Executes a statement list.
pub fn exec_block(vm: &mut Vm, cx: &Ctx, stmts: &[Stmt]) -> PResult<()> {
    let mut i = 0;
    while i < stmts.len() {
        match exec_stmt(vm, cx, &stmts[i]) {
            Ok(()) => i += 1,
            Err(Flow::Break(GOTO)) => {
                let label = cx.goto.get().expect("pending goto");
                match stmts.iter().position(|s| matches!(s.kind, StmtKind::Label(l) if l == label)) {
                    Some(pos) => {
                        cx.goto.set(None);
                        i = pos + 1;
                    }
                    None => return Err(Flow::Break(GOTO)),
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Runs destructors queued by values dropped during the statement.
#[inline]
fn after_stmt(vm: &mut Vm) -> PResult<()> {
    if object::has_pending_destructors() {
        vm.run_pending_destructors()?;
    }
    if !vm.hooks.statement_boundary.is_empty() {
        vm.run_statement_hooks()?;
    }
    Ok(())
}

/// Loop body outcome.
enum LoopCtl {
    Next,
    Break,
    Exit(Flow),
}

#[inline]
fn loop_body(vm: &mut Vm, cx: &Ctx, body: &[Stmt]) -> LoopCtl {
    match exec_block(vm, cx, body) {
        Ok(()) => LoopCtl::Next,
        Err(Flow::Break(1)) => LoopCtl::Break,
        Err(Flow::Continue(1)) => LoopCtl::Next,
        Err(Flow::Break(n)) if n < super::UNWIND => LoopCtl::Exit(Flow::Break(n - 1)),
        Err(Flow::Continue(n)) => LoopCtl::Exit(Flow::Continue(n - 1)),
        Err(e) => LoopCtl::Exit(e),
    }
}

pub fn exec_stmt(vm: &mut Vm, cx: &Ctx, s: &Stmt) -> PResult<()> {
    vm.current_line = s.line;
    match &s.kind {
        StmtKind::Expr(e) => {
            let v = eval(vm, cx, e)?;
            drop(v);
            after_stmt(vm)?;
        }
        StmtKind::Echo(es) => {
            for e in es {
                let v = eval(vm, cx, e)?;
                match &v {
                    Value::String(s) => vm.echo(s)?,
                    Value::Int(i) => vm.echo(i.to_string().as_bytes())?,
                    _ => {
                        let s = vm.to_php_string(&v)?;
                        vm.echo(&s)?;
                    }
                }
            }
            after_stmt(vm)?;
        }
        StmtKind::InlineHtml(h) => vm.echo(h)?,
        StmtKind::If(branches, els) => {
            for (c, body) in branches {
                if eval_bool(vm, cx, c)? {
                    return exec_block(vm, cx, body);
                }
            }
            if let Some(b) = els {
                return exec_block(vm, cx, b);
            }
        }
        StmtKind::While(c, body) => loop {
            vm.current_line = s.line;
            if !eval_bool(vm, cx, c)? {
                break;
            }
            match loop_body(vm, cx, body) {
                LoopCtl::Next => {}
                LoopCtl::Break => break,
                LoopCtl::Exit(f) => return Err(f),
            }
        },
        StmtKind::DoWhile(body, c) => loop {
            match loop_body(vm, cx, body) {
                LoopCtl::Next => {}
                LoopCtl::Break => break,
                LoopCtl::Exit(f) => return Err(f),
            }
            if !eval_bool(vm, cx, c)? {
                break;
            }
        },
        StmtKind::For(f) => {
            for e in &f.init {
                eval(vm, cx, e)?;
            }
            loop {
                let mut ok = true;
                let n = f.cond.len();
                for (i, c) in f.cond.iter().enumerate() {
                    if i + 1 == n {
                        ok = eval_bool(vm, cx, c)?;
                    } else {
                        eval(vm, cx, c)?;
                    }
                }
                if !ok {
                    break;
                }
                match loop_body(vm, cx, &f.body) {
                    LoopCtl::Next => {}
                    LoopCtl::Break => break,
                    LoopCtl::Exit(fl) => return Err(fl),
                }
                for e in &f.step {
                    eval(vm, cx, e)?;
                }
            }
        }
        StmtKind::Foreach(f) => return foreach(vm, cx, f),
        StmtKind::Switch(sw) => return switch(vm, cx, sw),
        StmtKind::Break(n) => return Err(Flow::Break(*n)),
        StmtKind::Continue(n) => return Err(Flow::Continue(*n)),
        StmtKind::Return(e) => {
            let v = match e {
                None => Value::Null,
                Some(e) => {
                    let v = eval(vm, cx, e)?;
                    if cx.code.returns_ref() {
                        match v {
                            Value::Ref(r) => Value::Ref(r),
                            v => {
                                if !matches!(e, Expr::MakeRef(..)) && !matches!(e, Expr::Call(_) | Expr::MethodCall(_) | Expr::StaticCall(_)) {
                                    vm.engine_notice("Only variable references should be returned by reference")?;
                                }
                                Value::Ref(PhpRef::new(v))
                            }
                        }
                    } else {
                        v.into_unref()
                    }
                }
            };
            return Err(Flow::Return(v));
        }
        StmtKind::Global(vars) => {
            let g = vm.global_env();
            for gv in vars {
                let (name, slot) = match gv {
                    GlobalVar::Name(n, cv) => (n.clone(), Some(cx.slot(*cv))),
                    GlobalVar::Dynamic(e) => {
                        let v = eval(vm, cx, e)?;
                        (eval::to_str(vm, &v)?, None)
                    }
                };
                let gs = g.slot_for(&name);
                let r = g.make_ref(gs);
                let local = slot.unwrap_or_else(|| cx.env.slot_for(&name));
                // Binding the global scope to itself is a no-op.
                if Rc::ptr_eq(cx.env, &g) && local == gs {
                    continue;
                }
                let old = cx.env.bind_ref(local, r);
                drop(old);
            }
        }
        StmtKind::Static(decls) => {
            for d in decls {
                let existing = cx.ufn.statics.borrow().get(d.index as usize).cloned().flatten();
                let r = match existing {
                    Some(r) => r,
                    None => {
                        let v = match &d.init {
                            Some(e) => eval(vm, cx, e)?.into_unref(),
                            None => Value::Null,
                        };
                        // The initializer may have bound the variable recursively.
                        let again = cx.ufn.statics.borrow().get(d.index as usize).cloned().flatten();
                        match again {
                            Some(r) => r,
                            None => {
                                let r = PhpRef::new(v);
                                let mut st = cx.ufn.statics.borrow_mut();
                                if st.len() <= d.index as usize {
                                    st.resize(d.index as usize + 1, None);
                                }
                                st[d.index as usize] = Some(r.clone());
                                r
                            }
                        }
                    }
                };
                let old = cx.env.bind_ref(cx.slot(d.slot), r);
                drop(old);
            }
        }
        StmtKind::Unset(lvs) => {
            for lv in lvs {
                place::unset(vm, cx, lv)?;
            }
            after_stmt(vm)?;
        }
        StmtKind::Block(b) => return exec_block(vm, cx, b),
        StmtKind::Try(t) => return try_stmt(vm, cx, t),
        StmtKind::Goto(l) => {
            cx.goto.set(Some(*l));
            return Err(Flow::Break(GOTO));
        }
        StmtKind::Label(_) | StmtKind::Nop => {}
        StmtKind::FuncDecl(def) => super::decl::declare_function_runtime(vm, def)?,
        StmtKind::ClassDecl(def) => {
            super::decl::declare_class_runtime(vm, def)?;
        }
        StmtKind::Const(list) => {
            for (name, e) in list {
                let v = eval(vm, cx, e)?;
                vm.current_line = s.line;
                if !vm.define_constant(name.clone(), v.into_unref()) {
                    vm.engine_warning(format!("Constant {name} already defined"))?;
                }
            }
        }
        StmtKind::Ticks(n, body) => {
            let _ = n;
            for st in body {
                exec_stmt(vm, cx, st)?;
                super::builtins::run_ticks(vm)?;
            }
        }
        StmtKind::HaltCompiler => return Err(Flow::Return(Value::Int(1))),
    }
    Ok(())
}

fn switch(vm: &mut Vm, cx: &Ctx, sw: &SwitchStmt) -> PResult<()> {
    let subject = eval(vm, cx, &sw.subject)?;
    let mut start = None;
    for (i, c) in sw.cases.iter().enumerate() {
        if let Some(cond) = &c.cond {
            vm.current_line = c.line;
            let v = eval(vm, cx, cond)?;
            if vm.is_equal(&subject, &v)? {
                start = Some(i);
                break;
            }
        }
    }
    let Some(start) = start.or(sw.default) else {
        return Ok(());
    };
    for c in &sw.cases[start..] {
        match exec_block(vm, cx, &c.body) {
            Ok(()) => {}
            Err(Flow::Break(1)) | Err(Flow::Continue(1)) => return Ok(()),
            Err(Flow::Break(n)) if n < super::UNWIND => return Err(Flow::Break(n - 1)),
            Err(Flow::Continue(n)) => return Err(Flow::Continue(n - 1)),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Whether exception `ex` matches a catch type.
fn catch_matches(vm: &mut Vm, cx: &Ctx, ex: &ObjRef, ty: &PhpStr) -> bool {
    if ty.eq_ignore_ascii_case(b"self") {
        return cx.scope().is_some_and(|s| ex.class().instance_of(s));
    }
    if ty.eq_ignore_ascii_case(b"parent") {
        return cx.scope().and_then(|s| s.parent.as_ref()).is_some_and(|p| ex.class().instance_of(p));
    }
    if ex.class().instance_of_name(ty) {
        return true;
    }
    match vm.find_class(ty) {
        Some(c) => ex.class().instance_of(&c),
        None => false,
    }
}

fn try_stmt(vm: &mut Vm, cx: &Ctx, t: &TryStmt) -> PResult<()> {
    let r = exec_block(vm, cx, &t.body);
    let r = match r {
        Err(Flow::Exception(ex)) => {
            let mut handled = None;
            for c in &t.catches {
                if c.types.iter().any(|ty| catch_matches(vm, cx, &ex, ty)) {
                    vm.current_line = c.line;
                    if let Some(slot) = c.var {
                        let old = cx.env.set(cx.slot(slot), Value::Object(ex.clone()));
                        drop(old);
                    }
                    handled = Some(exec_block(vm, cx, &c.body));
                    break;
                }
            }
            match handled {
                Some(r) => r,
                None => Err(Flow::Exception(ex)),
            }
        }
        other => other,
    };
    let Some(fin) = &t.finally else { return r };
    // `exit` and fatal errors skip finally blocks.
    if matches!(r, Err(Flow::Exit(_)) | Err(Flow::Fatal)) {
        return r;
    }
    let pending_goto = cx.goto.take();
    match exec_block(vm, cx, fin) {
        Ok(()) => {
            if pending_goto.is_some() {
                cx.goto.set(pending_goto);
            }
            r
        }
        Err(Flow::Exception(new)) => {
            if let Err(Flow::Exception(old)) = r {
                chain_previous(&new, old);
            }
            Err(Flow::Exception(new))
        }
        Err(e) => Err(e),
    }
}

/// `zend_exception_set_previous`: appends `prev` at the end of `ex`'s chain,
/// unless that would create a cycle (`ex`'s chain meets `prev`'s).
pub fn chain_previous(ex: &ObjRef, prev: ObjRef) {
    fn previous_of(o: &ObjRef) -> Option<ObjRef> {
        match o.get_prop(b"previous").map(Value::into_unref) {
            Some(Value::Object(p)) => Some(p),
            _ => None,
        }
    }
    if ObjRef::ptr_eq(ex, &prev) {
        return;
    }
    let mut cur = ex.clone();
    loop {
        let mut ancestor = previous_of(&prev);
        while let Some(a) = ancestor {
            if ObjRef::ptr_eq(&a, &cur) {
                return;
            }
            ancestor = previous_of(&a);
        }
        match previous_of(&cur) {
            None => {
                cur.set_prop(b"previous", Value::Object(prev));
                return;
            }
            Some(p) => {
                if ObjRef::ptr_eq(&p, &prev) {
                    return;
                }
                cur = p;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// foreach
// ---------------------------------------------------------------------------

fn assign_target(vm: &mut Vm, cx: &Ctx, lv: &LVal, v: Value) -> PResult<()> {
    match lv {
        LVal::Var(cv) => {
            let old = cx.env.set(cx.slot(*cv), v);
            drop(old);
            Ok(())
        }
        _ => place::assign_value(vm, cx, lv, v),
    }
}

fn foreach(vm: &mut Vm, cx: &Ctx, f: &ForeachStmt) -> PResult<()> {
    if f.by_ref {
        if let Some(subject) = &f.subject_place {
            return foreach_ref(vm, cx, f, subject);
        }
    }
    let v = eval(vm, cx, &f.expr)?;
    match v.into_unref() {
        Value::Array(a) => {
            if a.is_empty() {
                return Ok(());
            }
            if f.by_ref {
                // By-reference iteration of a temporary array.
                let r = PhpRef::new(Value::Array(a));
                return foreach_ref_root(vm, cx, f, r);
            }
            for (k, v) in a.iter() {
                assign_target(vm, cx, &f.value, v.unref())?;
                if let Some(kt) = &f.key {
                    assign_target(vm, cx, kt, k.to_value())?;
                }
                match loop_body(vm, cx, &f.body) {
                    LoopCtl::Next => {}
                    LoopCtl::Break => break,
                    LoopCtl::Exit(fl) => return Err(fl),
                }
            }
            Ok(())
        }
        Value::Object(o) => foreach_object(vm, cx, f, &o),
        other => {
            vm.current_line = f.line;
            vm.engine_warning(format!(
                "foreach() argument must be of type array|object, {} given",
                other.value_name()
            ))
        }
    }
}

/// `foreach ($var as &$v)`.
fn foreach_ref(vm: &mut Vm, cx: &Ctx, f: &ForeachStmt, subject: &LVal) -> PResult<()> {
    // Fetch the subject for writing: make it a reference so the loop sees changes.
    let cur = place::read_place(vm, cx, subject, true)?;
    match &cur {
        Value::Object(o) => {
            let o = o.clone();
            drop(cur);
            return foreach_object(vm, cx, f, &o);
        }
        Value::Array(_) | Value::Null => {}
        other => {
            vm.current_line = f.line;
            return vm.engine_warning(format!(
                "foreach() argument must be of type array|object, {} given",
                other.value_name()
            ));
        }
    }
    if cur.is_null() {
        vm.current_line = f.line;
        return vm.engine_warning("foreach() argument must be of type array|object, null given");
    }
    drop(cur);
    let r = place::make_ref(vm, cx, subject)?;
    foreach_ref_root(vm, cx, f, r)
}

fn foreach_ref_root(vm: &mut Vm, cx: &Ctx, f: &ForeachStmt, r: PhpRef) -> PResult<()> {
    let mut pos: crate::value::Pos = i64::MIN;
    loop {
        // Find the next element at or after `pos` and make it a reference.
        let next = {
            let mut b = r.borrow_mut();
            let Value::Array(arr) = &mut *b else { break };
            let start = if pos == i64::MIN {
                arr.first_pos()
            } else {
                arr.valid_pos(arr.pos_after(pos))
            };
            match start {
                None => None,
                Some(p) => {
                    let a = Rc::make_mut(arr);
                    let key = a.key_at(p).expect("valid pos");
                    let er = crate::value::array::make_element_ref(a, key.clone());
                    Some((p, key, er))
                }
            }
        };
        let Some((p, key, er)) = next else { break };
        pos = p;
        place::bind_ref(vm, cx, &f.value, er)?;
        if let Some(kt) = &f.key {
            assign_target(vm, cx, kt, key.to_value())?;
        }
        match loop_body(vm, cx, &f.body) {
            LoopCtl::Next => {}
            LoopCtl::Break => break,
            LoopCtl::Exit(fl) => return Err(fl),
        }
    }
    Ok(())
}

fn foreach_object(vm: &mut Vm, cx: &Ctx, f: &ForeachStmt, o: &ObjRef) -> PResult<()> {
    let by_ref = f.by_ref;
    let mut exit: Option<Flow> = None;
    let has_key = f.key.is_some();
    let scope = cx.scope().cloned();
    let r = iterate_object_ex(vm, o, by_ref, scope.as_ref(), has_key, &mut |vm, k, v| {
        if by_ref {
            let r = match v {
                Value::Ref(r) => r,
                v => PhpRef::new(v),
            };
            place::bind_ref(vm, cx, &f.value, r)?;
        } else {
            assign_target(vm, cx, &f.value, v.into_unref())?;
        }
        if let Some(kt) = &f.key {
            assign_target(vm, cx, kt, k)?;
        }
        match loop_body(vm, cx, &f.body) {
            LoopCtl::Next => Ok(true),
            LoopCtl::Break => Ok(false),
            LoopCtl::Exit(fl) => {
                exit = Some(fl);
                Ok(false)
            }
        }
    });
    r?;
    match exit {
        Some(fl) => Err(fl),
        None => Ok(()),
    }
}

/// Iterates an object like `foreach`: the callback returns `false` to stop.
pub fn iterate_object(
    vm: &mut Vm,
    o: &ObjRef,
    by_ref: bool,
    f: &mut dyn FnMut(&mut Vm, Value, Value) -> PResult<bool>,
) -> PResult<()> {
    let scope = super::call::calling_scope(vm);
    iterate_object_ex(vm, o, by_ref, scope.as_ref(), true, f)
}

fn iterate_object_ex(
    vm: &mut Vm,
    o: &ObjRef,
    by_ref: bool,
    scope: Option<&crate::class::ClassRef>,
    want_key: bool,
    f: &mut dyn FnMut(&mut Vm, Value, Value) -> PResult<bool>,
) -> PResult<()> {
    let class = o.class().clone();
    if super::generator::is_generator(o) {
        return super::generator::iterate(vm, o, by_ref, f);
    }
    if class.instance_of_name(b"Iterator") {
        if by_ref {
            return Err(vm.throw_error("Error", "An iterator cannot be used with foreach by reference"));
        }
        vm.call_method(o, b"rewind", Vec::new())?;
        loop {
            let valid = vm.call_method(o, b"valid", Vec::new())?;
            if !eval::to_bool(vm, &valid)? {
                break;
            }
            let v = vm.call_method(o, b"current", Vec::new())?;
            let k = if want_key {
                vm.call_method(o, b"key", Vec::new())?.into_unref()
            } else {
                Value::Null
            };
            if !f(vm, k, v.into_unref())? {
                return Ok(());
            }
            vm.call_method(o, b"next", Vec::new())?;
        }
        return Ok(());
    }
    if class.instance_of_name(b"IteratorAggregate") {
        let it = vm.call_method(o, b"getIterator", Vec::new())?;
        return match it.into_unref() {
            Value::Object(i) if i.class().instance_of_name(b"Traversable") => {
                iterate_object_ex(vm, &i, by_ref, scope, want_key, f)
            }
            _ => Err(vm.throw_error(
                "Exception",
                format!(
                    "Objects returned by {}::getIterator() must be traversable or implement interface Iterator",
                    class.name
                ),
            )),
        };
    }
    if let Some(gi) = class.handlers.get_iterator {
        let it = gi(vm, o, by_ref)?;
        if let Some(cell) = it.downcast_ref::<std::cell::RefCell<Box<dyn crate::class::InternalIterator>>>() {
            cell.borrow_mut().rewind(vm)?;
            loop {
                if !cell.borrow_mut().valid(vm)? {
                    break;
                }
                let v = cell.borrow_mut().current(vm)?;
                let k = if want_key { cell.borrow_mut().key(vm)? } else { Value::Null };
                if !f(vm, k, v)? {
                    return Ok(());
                }
                cell.borrow_mut().next(vm)?;
            }
            return Ok(());
        }
    }
    // Plain object: visible properties.
    let names: Vec<(PhpStr, bool)> = {
        let mut v = Vec::new();
        let table = super::objects::property_table(o);
        for (k, _) in table.iter() {
            let ArrayKey::Str(key) = k else { continue };
            let (cls, name) = super::objects::unmangle(&key);
            let visible = match cls {
                None => true,
                Some(c) if c.as_slice() == b"*" => {
                    let decl = class.properties.get(&name).map(|p| p.declaring_class.clone()).unwrap_or_else(|| class.name.clone());
                    super::objects::protected_ok(&decl, scope, vm)
                }
                Some(c) => scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&c)),
            };
            if visible {
                let is_dynamic = !class.properties.get(&name).is_some_and(|p| !p.is_static());
                v.push((PhpStr::from_vec(name), is_dynamic));
            }
        }
        v
    };
    for (name, _dynamic) in names {
        let k = match ArrayKey::from_php_str(name.clone()) {
            ArrayKey::Int(i) => Value::Int(i),
            ArrayKey::Str(s) => Value::String(s),
        };
        if by_ref {
            let r = super::objects::make_prop_ref(vm, scope, o, &name)?;
            if !f(vm, k, Value::Ref(r))? {
                return Ok(());
            }
        } else {
            // Properties unset during iteration are skipped.
            if !super::objects::isset_prop(vm, scope, o, &name, false)? && o.get_prop(&name).is_none() {
                continue;
            }
            let v = o.get_prop(&name).unwrap_or(Value::Null);
            if !f(vm, k, v.unref())? {
                return Ok(());
            }
        }
    }
    Ok(())
}
