//! A minimal PHP evaluator for running the ext/date `.phpt` scripts in unit
//! tests, until the real interpreter lands. It walks the `php-syntax` AST and
//! drives the runtime exactly like compiled code would: native functions and
//! methods go through `Vm::invoke_native` (so ZPP, warnings and exceptions
//! are the real ones); userland functions, classes (methods, properties,
//! constants, `parent::` calls) are supported through a [`Host`]
//! implementation. Only the subset of PHP the date tests use is covered;
//! anything else reports `unsupported`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use php_runtime::class::{
    ClassConstant, ClassEntry, Function, FunctionKind, PropPurpose, PropertyInfo, class_flags,
    member_flags,
};
use php_runtime::native::Signature;
use php_runtime::types::TypeDecl;
use php_runtime::value::convert::{OffsetKind, object_property_table};
use php_runtime::vm::{Frame, Host, default_call_callable};
use php_runtime::{
    ArrayKey, CallArgs, ClassRef, Flow, ObjRef, PResult, PhpArray, PhpRef, PhpStr, Value, Vm,
};
use php_syntax::ast::{
    Arg, ArgList, ArrayItem, BinaryOp, Block, Callee, CastKind, ClassConstName, ClassDecl,
    ClassMemberKind, ClassRef as AstClassRef, Expr, ExprKind, IncDecOp, MagicConst, MemberName,
    Modifiers, Name, NewClass, Param as AstParam, Stmt, StmtKind, StrPart, TypeHint, UnaryOp,
    VarName,
};

use super::support::{P, print_r_into, read_phpt, section, var_dump_into, var_export_into};

/// Interpreter state shared with the [`Host`].
#[derive(Default)]
pub struct Shared {
    globals: Rc<RefCell<HashMap<Vec<u8>, PhpRef>>>,
    statics: RefCell<HashMap<(usize, Vec<u8>), PhpRef>>,
    dir: String,
}

/// The body of a userland function or method (`FunctionKind::User` payload).
struct UserFn {
    params: Vec<AstParam>,
    body: Block,
    /// Declaring class (for `self::`).
    class: Option<Vec<u8>>,
}

/// A variable scope.
#[derive(Clone)]
struct Scope {
    vars: Rc<RefCell<HashMap<Vec<u8>, PhpRef>>>,
    this: Option<ObjRef>,
    self_class: Option<ClassRef>,
    static_class: Option<ClassRef>,
    func_name: Option<String>,
    fn_id: usize,
}

struct MiniHost {
    shared: Rc<Shared>,
}

impl Host for MiniHost {
    fn call_user_function(
        &self,
        vm: &mut Vm,
        func: &Rc<Function>,
        call: CallArgs,
    ) -> PResult<Value> {
        call_user(vm, &self.shared, func, call)
    }

    fn call_callable(&self, vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
        default_call_callable(vm, callable, args)
    }
}

fn unsupported(vm: &mut Vm, what: &str) -> Flow {
    vm.throw_error("Error", format!("minphp: unsupported {what}"))
}

/// Runs a phpt's `--FILE--` section in `p`.
pub fn run_phpt(name: &str) -> P {
    let mut p = P::phpt(name);
    let file = read_phpt(name);
    let code = section(&file, "FILE").expect("phpt without --FILE--");
    let shared = Rc::new(Shared {
        dir: format!(
            "{}/../../tests/phpt/ext/date/tests",
            env!("CARGO_MANIFEST_DIR")
        ),
        ..Shared::default()
    });
    p.t.vm.set_host(Rc::new(MiniHost {
        shared: shared.clone(),
    }));
    run_code(&mut p.t.vm, &shared, code.as_bytes());
    p
}

/// Parses and runs a script at the top level.
pub fn run_code(vm: &mut Vm, shared: &Rc<Shared>, code: &[u8]) {
    let prog = match php_syntax::parse(code, "t.php") {
        Ok(p) => p,
        Err(e) => {
            let _ = vm.echo(format!("minphp: parse error: {e}").as_bytes());
            return;
        }
    };
    let scope = Scope {
        vars: shared.globals.clone(),
        this: None,
        self_class: None,
        static_class: None,
        func_name: None,
        fn_id: 0,
    };
    hoist(vm, shared, &prog.stmts);
    let r = exec_block(vm, shared, &scope, &prog.stmts);
    match r {
        Ok(()) | Err(Flow::Exit(_)) | Err(Flow::Return(_)) => {}
        Err(Flow::Exception(e)) => {
            let _ = vm.report_uncaught_exception(&e);
        }
        Err(Flow::Fatal) => {}
        Err(other) => {
            let _ = vm.echo(format!("minphp: stray {other:?}").as_bytes());
        }
    }
    let _ = vm.run_pending_destructors();
}

/// Declares top-level functions (and classes whose parents exist) before
/// execution, like php-src's early binding.
fn hoist(vm: &mut Vm, shared: &Rc<Shared>, stmts: &[Stmt]) {
    for s in stmts {
        if let StmtKind::Function(f) = &s.kind {
            declare_function(vm, &f.name, &f.common.params, &f.body);
        }
    }
    for s in stmts {
        if let StmtKind::Class(c) = &s.kind {
            let parent_ok = c
                .extends
                .as_ref()
                .is_none_or(|n| vm.find_class(n.resolved.as_bytes()).is_some());
            if parent_ok
                && let Some(n) = &c.name
                && vm.find_class(n.as_bytes()).is_none()
            {
                let _ = declare_class(vm, shared, c);
            }
        }
    }
}

fn user_fn(params: &[AstParam], body: &Block, class: Option<Vec<u8>>) -> Rc<dyn std::any::Any> {
    Rc::new(UserFn {
        params: params.to_vec(),
        body: body.clone(),
        class,
    })
}

fn signature(name: &str, params: &[AstParam]) -> Rc<Signature> {
    let ps = params
        .iter()
        .map(|p| {
            let mut np = php_runtime::native::Param::new(PhpStr::from_bytes(&p.name), None);
            np.by_ref = p.by_ref;
            np.variadic = p.variadic;
            if p.default.is_some() {
                np.default = Some(php_runtime::native::DefaultValue::Unknown);
            }
            np
        })
        .collect();
    Rc::new(Signature::new(name, ps, None, false))
}

fn declare_function(vm: &mut Vm, name: &[u8], params: &[AstParam], body: &Block) {
    let n = String::from_utf8_lossy(name).into_owned();
    let f = Function::user(
        PhpStr::from_bytes(name),
        user_fn(params, body, None),
        signature(&n, params),
        member_flags::PUBLIC,
    );
    vm.register_function(Rc::new(f));
}

fn type_decl(t: &TypeHint) -> Option<TypeDecl> {
    fn text(t: &TypeHint) -> String {
        match t {
            TypeHint::Builtin { source, .. } => source.to_string_lossy(),
            TypeHint::Class(n) => n.resolved.to_string_lossy(),
            TypeHint::Array => "array".into(),
            TypeHint::Callable => "callable".into(),
            TypeHint::Static => "static".into(),
            TypeHint::Nullable(i) => format!("?{}", text(i)),
            TypeHint::Union(v) => v.iter().map(text).collect::<Vec<_>>().join("|"),
            TypeHint::Intersection(v) => v.iter().map(text).collect::<Vec<_>>().join("&"),
        }
    }
    TypeDecl::parse(&text(t)).ok()
}

fn member_flags_of(m: Modifiers) -> u32 {
    let mut f = 0;
    if m.0 & Modifiers::PRIVATE.0 != 0 {
        f |= member_flags::PRIVATE;
    } else if m.0 & Modifiers::PROTECTED.0 != 0 {
        f |= member_flags::PROTECTED;
    } else {
        f |= member_flags::PUBLIC;
    }
    if m.0 & Modifiers::STATIC.0 != 0 {
        f |= member_flags::STATIC;
    }
    if m.0 & Modifiers::ABSTRACT.0 != 0 {
        f |= member_flags::ABSTRACT;
    }
    if m.0 & Modifiers::FINAL.0 != 0 {
        f |= member_flags::FINAL;
    }
    f
}

fn declare_class(vm: &mut Vm, shared: &Rc<Shared>, c: &ClassDecl) -> PResult<ClassRef> {
    let name = c
        .name
        .as_ref()
        .map(|n| n.to_string_lossy())
        .unwrap_or_else(|| "class@anonymous".into());
    let mut flags = 0;
    if c.modifiers.is_abstract {
        flags |= class_flags::ABSTRACT;
    }
    if c.modifiers.is_final {
        flags |= class_flags::FINAL;
    }
    match c.kind {
        php_syntax::ast::ClassKind::Interface => flags |= class_flags::INTERFACE,
        php_syntax::ast::ClassKind::Trait => flags |= class_flags::TRAIT,
        php_syntax::ast::ClassKind::Enum => return Err(unsupported(vm, "enum")),
        php_syntax::ast::ClassKind::Class => {}
    }
    let mut ce = match &c.extends {
        Some(parent) => {
            let Some(pce) = vm.lookup_class(parent.resolved.as_bytes())? else {
                return Err(vm.fatal_error(format!(
                    "Class \"{}\" not found",
                    parent.resolved.to_string_lossy()
                )));
            };
            let mut ce = ClassEntry::new_subclass(name.as_str(), flags, &pce);
            ce.interfaces = pce.interfaces.clone();
            ce
        }
        None => ClassEntry::new(name.as_str(), flags),
    };
    for iface in &c.implements {
        let Some(ice) = vm.lookup_class(iface.resolved.as_bytes())? else {
            return Err(vm.fatal_error(format!(
                "Interface \"{}\" not found",
                iface.resolved.to_string_lossy()
            )));
        };
        if ice.lc_name.as_bytes() == b"datetimeinterface"
            && !ce.instance_of_name(b"DateTime")
            && !ce.instance_of_name(b"DateTimeImmutable")
        {
            // implement_date_interface_handler
            return Err(vm.fatal_error("DateTimeInterface can't be implemented by user classes"));
        }
        for p in ice.interfaces.iter() {
            if !ce.interfaces.iter().any(|i| i.lc_name == p.lc_name) {
                ce.interfaces.push(p.clone());
            }
        }
        if !ce.interfaces.iter().any(|i| i.lc_name == ice.lc_name) {
            ce.interfaces.push(ice.clone());
        }
    }
    let scope = Scope {
        vars: Rc::new(RefCell::new(HashMap::new())),
        this: None,
        self_class: None,
        static_class: None,
        func_name: None,
        fn_id: 0,
    };
    for m in &c.members {
        match &m.kind {
            ClassMemberKind::Const(g) => {
                for k in &g.consts {
                    let v = eval(vm, shared, &scope, &k.value)?;
                    ce.add_constant(ClassConstant::new(
                        PhpStr::from_bytes(&k.name),
                        v,
                        name.as_str(),
                    ));
                }
            }
            ClassMemberKind::Property(g) => {
                for pe in &g.props {
                    let mut info = PropertyInfo::new(
                        PhpStr::from_bytes(&pe.name),
                        name.as_str(),
                        member_flags_of(g.modifiers),
                    );
                    info.ty = g.ty.as_ref().and_then(type_decl);
                    info.default = match &pe.default {
                        Some(d) => eval(vm, shared, &scope, d)?,
                        None if info.ty.is_some() => Value::Undef,
                        None => Value::Null,
                    };
                    ce.declare_property(info);
                }
            }
            ClassMemberKind::Method(md) => {
                let mname = md.name.to_string_lossy();
                let mut f = Function::user(
                    PhpStr::from_bytes(&md.name),
                    user_fn(
                        &md.common.params,
                        md.body.as_ref().unwrap_or(&Vec::new()),
                        Some(name.as_bytes().to_vec()),
                    ),
                    signature(&mname, &md.common.params),
                    member_flags_of(md.modifiers),
                );
                f.scope = Some(PhpStr::from(name.as_str()));
                ce.add_method(f);
            }
            _ => return Err(unsupported(vm, "class member")),
        }
    }
    ce.link_magic_methods();
    let ce = Rc::new(ce);
    if !vm.register_class(ce.clone()) {
        return Err(vm.fatal_error(format!(
            "Cannot declare class {name}, because the name is already in use"
        )));
    }
    Ok(ce)
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

fn call_user(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    func: &Rc<Function>,
    call: CallArgs,
) -> PResult<Value> {
    let FunctionKind::User(payload) = &func.kind else {
        return vm.invoke_native(func, call);
    };
    let Some(uf) = payload.downcast_ref::<UserFn>() else {
        return Err(unsupported(vm, "foreign user function"));
    };
    let self_class = match &uf.class {
        Some(c) => vm.find_class(c),
        None => None,
    };
    let static_class = call
        .this
        .as_ref()
        .map(|o| o.class().clone())
        .or(call.scope.clone())
        .or(self_class.clone());
    let scope = Scope {
        vars: Rc::new(RefCell::new(HashMap::new())),
        this: if func.is_static() {
            None
        } else {
            call.this.clone()
        },
        self_class,
        static_class,
        func_name: Some(func.name.to_string()),
        fn_id: Rc::as_ptr(func) as *const u8 as usize,
    };
    let mut positional = call.positional.into_iter();
    let mut named = call.named;
    for p in &uf.params {
        let pname = p.name.as_bytes().to_vec();
        if p.variadic {
            let rest: Vec<Value> = positional.by_ref().collect();
            let mut arr = PhpArray::from_values(rest);
            for (n, v) in named.drain(..) {
                arr.insert(ArrayKey::Str(n), v);
            }
            set_var(&scope, &pname, Value::array(arr));
            continue;
        }
        let v = match positional.next() {
            Some(v) => Some(v),
            None => named
                .iter()
                .position(|(n, _)| n.as_bytes() == p.name.as_bytes())
                .map(|i| named.remove(i).1),
        };
        let v = match v {
            Some(v) => v,
            None => match &p.default {
                Some(d) => eval(vm, shared, &scope, d)?,
                None => {
                    return Err(vm.throw_error(
                        "ArgumentCountError",
                        format!(
                            "Too few arguments to function {}(), 0 passed and at least 1 expected",
                            func.display_name()
                        ),
                    ));
                }
            },
        };
        if p.by_ref {
            let r = match v {
                Value::Ref(r) => r,
                other => PhpRef::new(other),
            };
            scope.vars.borrow_mut().insert(pname, r);
        } else {
            let v = v.into_unref();
            let v = match p.ty.as_ref().and_then(type_decl) {
                Some(ty) if !ty.is_mixed() => {
                    match vm.coerce_to_type(&ty, v.clone(), Default::default())? {
                        Some(c) => c,
                        None if v.is_null() && ty.allows_null() => v,
                        None => {
                            return Err(vm.throw_type_error(format!(
                                "{}(): Argument #{} (${}) must be of type {}, {} given",
                                func.display_name(),
                                uf.params.iter().position(|q| q.name == p.name).unwrap_or(0) + 1,
                                p.name.to_string_lossy(),
                                ty.to_type_string(),
                                v.value_name()
                            )));
                        }
                    }
                }
                _ => v,
            };
            set_var(&scope, &pname, v);
        }
    }
    let frame = Frame::user(vm, func.clone(), scope.this.clone(), Vec::new());
    vm.push_frame(frame);
    let r = exec_block(vm, shared, &scope, &uf.body);
    vm.pop_frame();
    match r {
        Ok(()) => Ok(Value::Null),
        Err(Flow::Return(v)) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Evaluates an argument list for `func` (by-reference parameters get the
/// variable's reference).
fn eval_args(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    func: Option<&Rc<Function>>,
    args: &ArgList,
) -> PResult<(Vec<Value>, Vec<(PhpStr, Value)>)> {
    let sig = func.map(|f| f.signature().clone());
    let mut positional = Vec::new();
    let mut named = Vec::new();
    for a in &args.args {
        match a {
            Arg::Positional(e) => {
                let idx = positional.len();
                let by_ref = sig.as_ref().is_some_and(|s| {
                    s.params
                        .get(idx)
                        .or_else(|| s.params.last().filter(|p| p.variadic))
                        .is_some_and(|p| p.by_ref)
                });
                if by_ref && is_lvalue(e) {
                    let r = lvalue_ref(vm, shared, scope, e)?;
                    positional.push(Value::Ref(r));
                } else {
                    positional.push(eval(vm, shared, scope, e)?);
                }
            }
            Arg::Unpack(e) => {
                let v = eval(vm, shared, scope, e)?;
                if let Value::Array(a) = v {
                    for (k, v) in a.iter() {
                        match k {
                            ArrayKey::Int(_) => positional.push(v.unref()),
                            ArrayKey::Str(s) => named.push((s, v.unref())),
                        }
                    }
                }
            }
            Arg::Named { name, value } => {
                let v = eval(vm, shared, scope, value)?;
                named.push((PhpStr::from_bytes(name), v));
            }
            _ => return Err(unsupported(vm, "callable placeholder")),
        }
    }
    Ok((positional, named))
}

fn call_function_value(
    vm: &mut Vm,
    f: &Rc<Function>,
    positional: Vec<Value>,
    named: Vec<(PhpStr, Value)>,
    this: Option<ObjRef>,
    scope: Option<ClassRef>,
) -> PResult<Value> {
    vm.call_function(
        f,
        CallArgs {
            positional,
            named,
            this,
            scope,
            strict: false,
            dynamic: false,
        },
    )
}

fn resolve_class(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    c: &AstClassRef,
) -> PResult<ClassRef> {
    match c {
        AstClassRef::Name(n) => resolve_class_name(vm, scope, n),
        AstClassRef::Expr(e) => {
            let v = eval(vm, shared, scope, e)?;
            match v {
                Value::Object(o) => Ok(o.class().clone()),
                Value::String(s) => match vm.lookup_class(&s)? {
                    Some(c) => Ok(c),
                    None => Err(vm.throw_error("Error", format!("Class \"{s}\" not found"))),
                },
                _ => Err(vm.throw_error("Error", "Cannot use value as class name")),
            }
        }
    }
}

fn resolve_class_name(vm: &mut Vm, scope: &Scope, n: &Name) -> PResult<ClassRef> {
    use php_syntax::ast::SpecialClass;
    match n.special_class() {
        Some(SpecialClass::Self_) => scope.self_class.clone().ok_or_else(|| {
            vm.throw_error("Error", "Cannot use \"self\" when no class scope is active")
        }),
        Some(SpecialClass::Static) => scope.static_class.clone().ok_or_else(|| {
            vm.throw_error(
                "Error",
                "Cannot use \"static\" when no class scope is active",
            )
        }),
        Some(SpecialClass::Parent) => scope
            .self_class
            .as_ref()
            .and_then(|c| c.parent.clone())
            .ok_or_else(|| {
                vm.throw_error(
                    "Error",
                    "Cannot use \"parent\" when current class scope has no parent",
                )
            }),
        None => match vm.lookup_class(n.resolved.as_bytes())? {
            Some(c) => Ok(c),
            None => Err(vm.throw_error(
                "Error",
                format!("Class \"{}\" not found", n.resolved.to_string_lossy()),
            )),
        },
    }
}

fn member_name(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, m: &MemberName) -> PResult<PhpStr> {
    match m {
        MemberName::Ident(s) => Ok(PhpStr::from_bytes(s)),
        MemberName::Expr(e) => {
            let v = eval(vm, shared, scope, e)?;
            vm.to_php_string(&v)
        }
    }
}

/// `new $class(...$args)`.
fn instantiate(
    vm: &mut Vm,
    ce: &ClassRef,
    positional: Vec<Value>,
    named: Vec<(PhpStr, Value)>,
) -> PResult<Value> {
    let what = if ce.is_interface() {
        Some("interface")
    } else if ce.is_trait() {
        Some("trait")
    } else if ce.is_enum() {
        Some("enum")
    } else if ce.is_abstract() {
        Some("abstract class")
    } else {
        None
    };
    if let Some(w) = what {
        return Err(vm.throw_error("Error", format!("Cannot instantiate {w} {}", ce.name)));
    }
    let obj = vm.instantiate(ce)?;
    if let Some(ctor) = ce.magic.constructor.clone() {
        if ctor.visibility() != member_flags::PUBLIC {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Call to private {}::__construct() from global scope",
                    ce.name
                ),
            ));
        }
        call_function_value(
            vm,
            &ctor,
            positional,
            named,
            Some(obj.clone()),
            Some(ce.clone()),
        )?;
    }
    Ok(Value::Object(obj))
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

fn exec_block(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, stmts: &[Stmt]) -> PResult<()> {
    for s in stmts {
        exec(vm, shared, scope, s)?;
    }
    Ok(())
}

fn exec(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, s: &Stmt) -> PResult<()> {
    vm.current_line = s.line;
    let r = exec_inner(vm, shared, scope, s);
    vm.run_pending_destructors()?;
    r
}

fn loop_flow(r: PResult<()>) -> Result<bool, Flow> {
    // Ok(true) = continue looping, Ok(false) = break.
    match r {
        Ok(()) => Ok(true),
        Err(Flow::Break(n)) if n <= 1 => Ok(false),
        Err(Flow::Break(n)) => Err(Flow::Break(n - 1)),
        Err(Flow::Continue(n)) if n <= 1 => Ok(true),
        Err(Flow::Continue(n)) => Err(Flow::Continue(n - 1)),
        Err(e) => Err(e),
    }
}

fn level(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, e: &Option<Expr>) -> PResult<u32> {
    Ok(match e {
        Some(e) => eval(vm, shared, scope, e)?.as_int().unwrap_or(1) as u32,
        None => 1,
    })
}

fn exec_inner(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, s: &Stmt) -> PResult<()> {
    match &s.kind {
        StmtKind::Expr(e) => {
            eval(vm, shared, scope, e)?;
        }
        StmtKind::Echo(es) => {
            for e in es {
                let v = eval(vm, shared, scope, e)?;
                let s = vm.to_php_string(&v)?;
                vm.echo(&s)?;
            }
        }
        StmtKind::InlineHtml(h) => vm.echo(h)?,
        StmtKind::If(i) => {
            for br in &i.branches {
                let c = eval(vm, shared, scope, &br.cond)?;
                if vm.to_bool(&c)? {
                    return exec_block(vm, shared, scope, &br.body);
                }
            }
            if let Some(e) = &i.else_branch {
                exec_block(vm, shared, scope, e)?;
            }
        }
        StmtKind::While { cond, body } => loop {
            let c = eval(vm, shared, scope, cond)?;
            if !vm.to_bool(&c)? {
                break;
            }
            if !loop_flow(exec_block(vm, shared, scope, body))? {
                break;
            }
        },
        StmtKind::DoWhile { body, cond } => loop {
            if !loop_flow(exec_block(vm, shared, scope, body))? {
                break;
            }
            let c = eval(vm, shared, scope, cond)?;
            if !vm.to_bool(&c)? {
                break;
            }
        },
        StmtKind::For(f) => {
            for e in &f.init {
                eval(vm, shared, scope, e)?;
            }
            loop {
                let mut ok = true;
                for (k, e) in f.cond.iter().enumerate() {
                    let v = eval(vm, shared, scope, e)?;
                    if k == f.cond.len() - 1 {
                        ok = vm.to_bool(&v)?;
                    }
                }
                if !ok {
                    break;
                }
                if !loop_flow(exec_block(vm, shared, scope, &f.body))? {
                    break;
                }
                for e in &f.step {
                    eval(vm, shared, scope, e)?;
                }
            }
        }
        StmtKind::Foreach(f) => exec_foreach(vm, shared, scope, f)?,
        StmtKind::Switch { subject, cases } => {
            let v = eval(vm, shared, scope, subject)?;
            let mut matched = None;
            for (k, c) in cases.iter().enumerate() {
                if let Some(ce) = &c.cond {
                    let cv = eval(vm, shared, scope, ce)?;
                    if vm.is_equal(&v, &cv)? {
                        matched = Some(k);
                        break;
                    }
                }
            }
            if matched.is_none() {
                matched = cases.iter().position(|c| c.cond.is_none());
            }
            if let Some(start) = matched {
                for c in &cases[start..] {
                    match exec_block(vm, shared, scope, &c.body) {
                        Ok(()) => {}
                        Err(Flow::Break(n)) if n <= 1 => break,
                        Err(Flow::Break(n)) => return Err(Flow::Break(n - 1)),
                        Err(Flow::Continue(n)) if n <= 1 => break,
                        Err(Flow::Continue(n)) => return Err(Flow::Continue(n - 1)),
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        StmtKind::Break(e) => return Err(Flow::Break(level(vm, shared, scope, e)?)),
        StmtKind::Continue(e) => return Err(Flow::Continue(level(vm, shared, scope, e)?)),
        StmtKind::Return(e) => {
            let v = match e {
                Some(e) => eval(vm, shared, scope, e)?,
                None => Value::Null,
            };
            return Err(Flow::Return(v));
        }
        StmtKind::Global(vars) => {
            for v in vars {
                if let ExprKind::Var(VarName::Name(n)) = &v.kind {
                    let r = shared
                        .globals
                        .borrow_mut()
                        .entry(n.as_bytes().to_vec())
                        .or_insert_with(|| PhpRef::new(Value::Null))
                        .clone();
                    scope.vars.borrow_mut().insert(n.as_bytes().to_vec(), r);
                }
            }
        }
        StmtKind::Static(vars) => {
            for sv in vars {
                let key = (scope.fn_id, sv.name.as_bytes().to_vec());
                let existing = shared.statics.borrow().get(&key).cloned();
                let r = match existing {
                    Some(r) => r,
                    None => {
                        let v = match &sv.default {
                            Some(d) => eval(vm, shared, scope, d)?,
                            None => Value::Null,
                        };
                        let r = PhpRef::new(v);
                        shared.statics.borrow_mut().insert(key, r.clone());
                        r
                    }
                };
                scope
                    .vars
                    .borrow_mut()
                    .insert(sv.name.as_bytes().to_vec(), r);
            }
        }
        StmtKind::Unset(es) => {
            for e in es {
                unset(vm, shared, scope, e)?;
            }
        }
        StmtKind::Block(b) => exec_block(vm, shared, scope, b)?,
        StmtKind::Try(t) => {
            let r = exec_block(vm, shared, scope, &t.body);
            let r = match r {
                Err(Flow::Exception(ex)) => {
                    let mut handled = None;
                    for c in &t.catches {
                        let hit = c
                            .types
                            .iter()
                            .any(|ty| ex.class().instance_of_name(ty.resolved.as_bytes()));
                        if hit {
                            if let Some(var) = &c.var {
                                set_var(scope, var.as_bytes(), Value::Object(ex.clone()));
                            }
                            handled = Some(exec_block(vm, shared, scope, &c.body));
                            break;
                        }
                    }
                    handled.unwrap_or(Err(Flow::Exception(ex)))
                }
                other => other,
            };
            if let Some(fin) = &t.finally {
                exec_block(vm, shared, scope, fin)?;
            }
            r?;
        }
        StmtKind::Function(f) => {
            if vm.find_function(f.name.as_bytes()).is_none() {
                declare_function(vm, &f.name, &f.common.params, &f.body);
            }
        }
        StmtKind::Class(c) => {
            if let Some(n) = &c.name
                && vm.find_class(n.as_bytes()).is_none()
            {
                declare_class(vm, shared, c)?;
            }
        }
        StmtKind::Const(c) => {
            for k in &c.consts {
                let v = eval(vm, shared, scope, &k.value)?;
                vm.define_constant(PhpStr::from_bytes(&k.fq_name), v);
            }
        }
        StmtKind::Declare { body, .. } => {
            if let Some(b) = body {
                exec_block(vm, shared, scope, b)?;
            }
        }
        StmtKind::Namespace { body, .. } => {
            if let Some(b) = body {
                exec_block(vm, shared, scope, b)?;
            }
        }
        StmtKind::Use { .. } | StmtKind::GroupUse { .. } | StmtKind::Nop => {}
        StmtKind::HaltCompiler { .. } => return Err(Flow::Exit(0)),
        _ => return Err(unsupported(vm, "statement")),
    }
    Ok(())
}

fn exec_foreach(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    f: &php_syntax::ast::ForeachStmt,
) -> PResult<()> {
    if f.value.by_ref {
        return Err(unsupported(vm, "foreach by reference"));
    }
    let subject = eval(vm, shared, scope, &f.expr)?;
    let run = |vm: &mut Vm, k: Value, v: Value| -> Result<bool, Flow> {
        if let Some(key) = &f.key {
            assign_to(vm, shared, scope, &key.expr, k)?;
        }
        assign_to(vm, shared, scope, &f.value.expr, v)?;
        loop_flow(exec_block(vm, shared, scope, &f.body))
    };
    match subject {
        Value::Array(a) => {
            for (k, v) in a.iter() {
                if !run(vm, k.into_value(), v.unref())? {
                    break;
                }
            }
        }
        Value::Object(o) => {
            let it = if o.class().instance_of_name(b"Iterator") {
                o.clone()
            } else if o.class().instance_of_name(b"IteratorAggregate") {
                let mut cur = o.clone();
                loop {
                    let r = vm.call_method(&cur, b"getIterator", Vec::new())?;
                    match r {
                        Value::Object(n) if n.class().instance_of_name(b"Iterator") => break n,
                        Value::Object(n) if n.class().instance_of_name(b"IteratorAggregate") => {
                            cur = n
                        }
                        _ => {
                            return Err(vm.throw_error(
                                "Exception",
                                format!(
                                    "Objects returned by {}::getIterator() must be traversable or implement interface Iterator",
                                    cur.class().name
                                ),
                            ));
                        }
                    }
                }
            } else {
                let props = match o.class().handlers.get_properties_for {
                    Some(h) => h(vm, &o, PropPurpose::GetObjectVars)?
                        .unwrap_or_else(|| object_property_table(&o)),
                    None => object_property_table(&o),
                };
                for (k, v) in props.iter() {
                    if let ArrayKey::Str(s) = &k
                        && s.first() == Some(&0)
                    {
                        continue;
                    }
                    if !run(vm, k.into_value(), v.unref())? {
                        break;
                    }
                }
                return Ok(());
            };
            vm.call_method(&it, b"rewind", Vec::new())?;
            loop {
                let valid = vm.call_method(&it, b"valid", Vec::new())?;
                if !vm.to_bool(&valid)? {
                    break;
                }
                let v = vm.call_method(&it, b"current", Vec::new())?;
                let k = if f.key.is_some() {
                    vm.call_method(&it, b"key", Vec::new())?
                } else {
                    Value::Null
                };
                if !run(vm, k, v)? {
                    break;
                }
                vm.call_method(&it, b"next", Vec::new())?;
            }
        }
        other => {
            vm.engine_warning(format!(
                "foreach() argument must be of type array|object, {} given",
                other.type_name()
            ))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Variables and lvalues
// ---------------------------------------------------------------------------

fn set_var(scope: &Scope, name: &[u8], v: Value) {
    let existing = scope.vars.borrow().get(name).cloned();
    match existing {
        Some(r) => {
            r.set(v.into_unref());
        }
        None => {
            scope
                .vars
                .borrow_mut()
                .insert(name.to_vec(), PhpRef::new(v.into_unref()));
        }
    }
}

fn var_name(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, n: &VarName) -> PResult<Vec<u8>> {
    match n {
        VarName::Name(n) => Ok(n.as_bytes().to_vec()),
        VarName::Expr(e) => {
            let v = eval(vm, shared, scope, e)?;
            Ok(vm.to_php_string(&v)?.to_vec())
        }
    }
}

fn read_var(vm: &mut Vm, scope: &Scope, name: &[u8], quiet: bool) -> PResult<Value> {
    if name == b"this" {
        return Ok(scope.this.clone().map_or(Value::Null, Value::Object));
    }
    if name == b"GLOBALS" {
        return Err(unsupported(vm, "$GLOBALS"));
    }
    let r = scope.vars.borrow().get(name).cloned();
    match r {
        Some(r) => Ok(r.get()),
        None => {
            if !quiet {
                vm.engine_warning(format!(
                    "Undefined variable ${}",
                    String::from_utf8_lossy(name)
                ))?;
            }
            Ok(Value::Null)
        }
    }
}

fn is_lvalue(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Var(_) | ExprKind::Dim { .. } | ExprKind::Prop { .. }
    )
}

/// A reference to a variable (for by-reference arguments).
fn lvalue_ref(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, e: &Expr) -> PResult<PhpRef> {
    match &e.kind {
        ExprKind::Var(n) => {
            let name = var_name(vm, shared, scope, n)?;
            let r = scope
                .vars
                .borrow_mut()
                .entry(name)
                .or_insert_with(|| PhpRef::new(Value::Null))
                .clone();
            Ok(r)
        }
        _ => {
            // Array elements / properties: pass a copy and write it back is
            // not needed by the date tests.
            let v = eval(vm, shared, scope, e)?;
            Ok(PhpRef::new(v))
        }
    }
}

/// `$target = $v` for any lvalue.
fn assign_to(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    target: &Expr,
    v: Value,
) -> PResult<()> {
    match &target.kind {
        ExprKind::Var(n) => {
            let name = var_name(vm, shared, scope, n)?;
            if name == b"this" {
                return Err(vm.throw_error("Error", "Cannot re-assign $this"));
            }
            set_var(scope, &name, v);
            Ok(())
        }
        ExprKind::Array { items, .. } => {
            // list() / [] destructuring
            let mut idx = 0i64;
            for item in items {
                let Some(item) = item else {
                    idx += 1;
                    continue;
                };
                let key = match &item.key {
                    Some(k) => eval(vm, shared, scope, k)?,
                    None => {
                        let k = Value::Int(idx);
                        idx += 1;
                        k
                    }
                };
                let elem = match &v {
                    Value::Array(a) => {
                        let k = vm.to_array_key(&key, OffsetKind::Read)?;
                        a.get(&k).map(Value::unref).unwrap_or(Value::Null)
                    }
                    _ => Value::Null,
                };
                assign_to(vm, shared, scope, &item.value, elem)?;
            }
            Ok(())
        }
        ExprKind::Dim { .. } => {
            let mut path = Vec::new();
            let root = dim_path(vm, shared, scope, target, &mut path)?;
            match root {
                Root::Var(name) => {
                    let r = scope
                        .vars
                        .borrow_mut()
                        .entry(name)
                        .or_insert_with(|| PhpRef::new(Value::Null))
                        .clone();
                    let mut cur = r.get();
                    r.set(Value::Null);
                    let res = set_path(vm, &mut cur, &path, v);
                    r.set(cur);
                    res
                }
                Root::Prop(obj, name) => {
                    let mut cur = read_prop(vm, &obj, &name, true)?;
                    set_path(vm, &mut cur, &path, v)?;
                    write_prop(vm, &obj, &name, cur)
                }
            }
        }
        ExprKind::Prop { object, name, .. } => {
            let o = eval(vm, shared, scope, object)?;
            let name = member_name(vm, shared, scope, name)?;
            let Value::Object(o) = o else {
                return Err(vm.throw_error(
                    "Error",
                    format!("Attempt to assign property \"{name}\" on {}", o.type_name()),
                ));
            };
            write_prop(vm, &o, &name, v)
        }
        ExprKind::StaticProp { class, name, .. } => {
            let ce = resolve_class(vm, shared, scope, class)?;
            let name = var_name(vm, shared, scope, name)?;
            let Some(info) = ce.find_property(&name).filter(|p| p.is_static()).cloned() else {
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Access to undeclared static property {}::${}",
                        ce.name,
                        String::from_utf8_lossy(&name)
                    ),
                ));
            };
            ce.static_members.borrow()[info.slot].set(v);
            Ok(())
        }
        _ => Err(unsupported(vm, "assignment target")),
    }
}

enum Root {
    Var(Vec<u8>),
    Prop(ObjRef, PhpStr),
}

/// Collects the keys of a nested `$a[..][..]` lvalue (innermost last).
fn dim_path(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    e: &Expr,
    path: &mut Vec<Option<Value>>,
) -> PResult<Root> {
    match &e.kind {
        ExprKind::Dim { base, index } => {
            let root = dim_path(vm, shared, scope, base, path)?;
            let k = match index {
                Some(i) => Some(eval(vm, shared, scope, i)?),
                None => None,
            };
            path.push(k);
            Ok(root)
        }
        ExprKind::Var(n) => Ok(Root::Var(var_name(vm, shared, scope, n)?)),
        ExprKind::Prop { object, name, .. } => {
            let o = eval(vm, shared, scope, object)?;
            let name = member_name(vm, shared, scope, name)?;
            match o {
                Value::Object(o) => Ok(Root::Prop(o, name)),
                _ => Err(unsupported(vm, "dim on non-object property")),
            }
        }
        _ => Err(unsupported(vm, "dim base")),
    }
}

fn set_path(vm: &mut Vm, cur: &mut Value, path: &[Option<Value>], v: Value) -> PResult<()> {
    let Some((first, rest)) = path.split_first() else {
        *cur = v;
        return Ok(());
    };
    if let Value::Ref(r) = cur {
        let mut inner = r.get();
        set_path(vm, &mut inner, path, v)?;
        r.set(inner);
        return Ok(());
    }
    if matches!(cur, Value::Null | Value::Undef) {
        *cur = Value::empty_array();
    }
    match cur {
        Value::Array(a) => {
            let arr = Rc::make_mut(a);
            match first {
                None => {
                    if rest.is_empty() {
                        if arr.append(v).is_err() {
                            return Err(vm.throw_error(
                                "Error",
                                "Cannot add element to the array as the next element is already occupied",
                            ));
                        }
                    } else {
                        let mut inner = Value::Null;
                        set_path(vm, &mut inner, rest, v)?;
                        let _ = arr.append(inner);
                    }
                }
                Some(k) => {
                    let key = vm.to_array_key(k, OffsetKind::Write)?;
                    let slot = arr.entry_mut(key);
                    set_path(vm, slot, rest, v)?;
                }
            }
            Ok(())
        }
        Value::Object(o) if o.class().instance_of_name(b"ArrayAccess") && rest.is_empty() => {
            let o = o.clone();
            vm.call_method(
                &o,
                b"offsetSet",
                vec![first.clone().unwrap_or(Value::Null), v],
            )?;
            Ok(())
        }
        _ => Err(vm.throw_error("Error", "Cannot use a scalar value as an array")),
    }
}

fn unset(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, e: &Expr) -> PResult<()> {
    match &e.kind {
        ExprKind::Var(n) => {
            let name = var_name(vm, shared, scope, n)?;
            scope.vars.borrow_mut().remove(&name);
            Ok(())
        }
        ExprKind::Dim {
            base,
            index: Some(index),
        } => {
            let k = eval(vm, shared, scope, index)?;
            let mut path = Vec::new();
            let root = dim_path(vm, shared, scope, base, &mut path)?;
            let Root::Var(name) = root else {
                return Err(unsupported(vm, "unset of property dim"));
            };
            let r = scope.vars.borrow().get(&name).cloned();
            if let Some(r) = r {
                let mut cur = r.get();
                unset_path(vm, &mut cur, &path, &k)?;
                r.set(cur);
            }
            Ok(())
        }
        ExprKind::Prop { object, name, .. } => {
            let o = eval(vm, shared, scope, object)?;
            let name = member_name(vm, shared, scope, name)?;
            if let Value::Object(o) = o {
                if o.class().lc_name.as_bytes() == b"dateperiod"
                    && matches!(
                        name.as_bytes(),
                        b"start"
                            | b"current"
                            | b"end"
                            | b"interval"
                            | b"recurrences"
                            | b"include_start_date"
                            | b"include_end_date"
                    )
                {
                    // date_period_unset_property (no unset hook in the runtime)
                    return Err(vm.throw_error(
                        "Error",
                        format!("Cannot unset {}::${name}", o.class().name),
                    ));
                }
                if let Some(info) = o.class().find_property(&name)
                    && !info.is_static()
                {
                    o.obj().props.borrow_mut()[info.slot] = Value::Undef;
                } else if let Some(d) = o.obj().dyn_props.borrow_mut().as_mut() {
                    d.remove(&ArrayKey::Str(name.clone()));
                }
            }
            Ok(())
        }
        _ => Err(unsupported(vm, "unset target")),
    }
}

fn unset_path(vm: &mut Vm, cur: &mut Value, path: &[Option<Value>], k: &Value) -> PResult<()> {
    if let Value::Ref(r) = cur {
        let mut inner = r.get();
        unset_path(vm, &mut inner, path, k)?;
        r.set(inner);
        return Ok(());
    }
    let Value::Array(a) = cur else {
        return Ok(());
    };
    let arr = Rc::make_mut(a);
    match path.split_first() {
        None => {
            let key = vm.to_array_key(k, OffsetKind::Unset)?;
            arr.remove(&key);
        }
        Some((Some(first), rest)) => {
            let key = vm.to_array_key(first, OffsetKind::Unset)?;
            if let Some(slot) = arr.get_mut(&key) {
                unset_path(vm, slot, rest, k)?;
            }
        }
        Some((None, _)) => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

fn read_prop(vm: &mut Vm, o: &ObjRef, name: &PhpStr, quiet: bool) -> PResult<Value> {
    if let Some(h) = o.class().handlers.read_property
        && let Some(v) = h(vm, o, name)?
    {
        return Ok(v);
    }
    if let Some(info) = o.class().find_property(name).filter(|p| !p.is_static()) {
        let v = o.obj().props.borrow().get(info.slot).cloned();
        match v {
            Some(v) if !v.is_undef() => return Ok(v.unref()),
            _ => {
                if info.ty.is_some() {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Typed property {}::${} must not be accessed before initialization",
                            info.declaring_class, info.name
                        ),
                    ));
                }
            }
        }
    }
    match o.get_prop(name) {
        Some(v) => Ok(v.unref()),
        None => {
            if !quiet {
                vm.engine_warning(format!("Undefined property: {}::${}", o.class().name, name))?;
            }
            Ok(Value::Null)
        }
    }
}

fn write_prop(vm: &mut Vm, o: &ObjRef, name: &PhpStr, v: Value) -> PResult<()> {
    if let Some(h) = o.class().handlers.write_property
        && h(vm, o, name, v.clone())?
    {
        return Ok(());
    }
    if let Some(info) = o
        .class()
        .find_property(name)
        .filter(|p| !p.is_static())
        .cloned()
    {
        let mut v = v.into_unref();
        if let Some(ty) = &info.ty {
            match vm.coerce_to_type(ty, v.clone(), Default::default())? {
                Some(c) => v = c,
                None => {
                    return Err(vm.throw_type_error(format!(
                        "Cannot assign {} to property {}::${} of type {}",
                        v.value_name(),
                        info.declaring_class,
                        info.name,
                        ty.to_type_string()
                    )));
                }
            }
        }
        o.set_prop(name, v);
        return Ok(());
    }
    let exists = o
        .obj()
        .dyn_props
        .borrow()
        .as_ref()
        .is_some_and(|d| d.get_str(name).is_some());
    if !exists && !o.class().has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
        vm.engine_deprecated(format!(
            "Creation of dynamic property {}::${} is deprecated",
            o.class().name,
            name
        ))?;
    }
    o.set_prop(name, v.into_unref());
    Ok(())
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

fn eval(vm: &mut Vm, shared: &Rc<Shared>, scope: &Scope, e: &Expr) -> PResult<Value> {
    eval_ex(vm, shared, scope, e, false)
}

/// `quiet`: no undefined variable/index/property diagnostics (`isset`, `??`).
fn eval_ex(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    e: &Expr,
    quiet: bool,
) -> PResult<Value> {
    Ok(match &e.kind {
        ExprKind::Int(i) => Value::Int(*i),
        ExprKind::Float(f) => Value::Float(*f),
        ExprKind::String(s) => Value::bytes(s),
        ExprKind::Interpolated(parts) => {
            let mut out = Vec::new();
            for p in parts {
                match p {
                    StrPart::Literal(l) => out.extend_from_slice(l),
                    StrPart::Expr { expr, .. } => {
                        let v = eval(vm, shared, scope, expr)?;
                        out.extend_from_slice(&vm.to_php_string(&v)?);
                    }
                }
            }
            Value::from(out)
        }
        ExprKind::Const(n) => {
            let lc = n.resolved.to_ascii_lowercase();
            match lc.as_bytes() {
                b"true" => Value::Bool(true),
                b"false" => Value::Bool(false),
                b"null" => Value::Null,
                _ => {
                    let mut r = vm.fetch_constant(n.resolved.as_bytes())?;
                    if r.is_none() && n.fallback {
                        r = vm.fetch_constant(n.name.as_bytes())?;
                    }
                    match r {
                        Some(v) => v,
                        None => {
                            return Err(vm.throw_error(
                                "Error",
                                format!("Undefined constant \"{}\"", n.name.to_string_lossy()),
                            ));
                        }
                    }
                }
            }
        }
        ExprKind::MagicConst(m) => match m {
            MagicConst::Line => Value::Int(i64::from(e.line)),
            MagicConst::File => Value::String(vm.current_file.clone()),
            MagicConst::Dir => {
                let f = vm.current_file.to_string();
                Value::from(
                    f.rsplit_once('/')
                        .map_or(".".to_string(), |(d, _)| d.to_string()),
                )
            }
            MagicConst::Class => Value::String(
                scope
                    .self_class
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_default(),
            ),
            MagicConst::Function => Value::from(scope.func_name.clone().unwrap_or_default()),
            MagicConst::Method => {
                let f = scope.func_name.clone().unwrap_or_default();
                match &scope.self_class {
                    Some(c) => Value::from(format!("{}::{f}", c.name)),
                    None => Value::from(f),
                }
            }
            _ => Value::empty_string(),
        },
        ExprKind::ClassConst { class, name } => {
            let ce = resolve_class(vm, shared, scope, class)?;
            let cname = match name {
                ClassConstName::Ident(s) => PhpStr::from_bytes(s),
                ClassConstName::Expr(x) => {
                    let v = eval(vm, shared, scope, x)?;
                    vm.to_php_string(&v)?
                }
            };
            if let Some(c) = ce.find_constant(&cname)
                && c.flags & member_flags::DEPRECATED != 0
                && ce.instance_of_name(b"DateTimeInterface")
            {
                // #[\Deprecated] class constants (only DateTimeInterface::RFC7231 here)
                vm.engine_deprecated(format!(
                    "Constant {}::{} is deprecated {}",
                    c.declaring_class,
                    cname,
                    super::super::datetime::RFC7231_DEPRECATION
                ))?;
            }
            match vm.class_constant(ce.name.as_bytes(), cname.as_bytes())? {
                Some(v) => v,
                None => {
                    return Err(
                        vm.throw_error("Error", format!("Undefined constant {}::{cname}", ce.name))
                    );
                }
            }
        }
        ExprKind::ClassName(c) => {
            let ce = resolve_class(vm, shared, scope, c)?;
            Value::String(ce.name.clone())
        }
        ExprKind::Var(n) => {
            let name = var_name(vm, shared, scope, n)?;
            read_var(vm, scope, &name, quiet)?
        }
        ExprKind::Array { items, .. } => {
            let mut a = PhpArray::new();
            for item in items.iter().flatten() {
                eval_array_item(vm, shared, scope, item, &mut a)?;
            }
            Value::array(a)
        }
        ExprKind::Dim { base, index } => {
            let b = eval_ex(vm, shared, scope, base, quiet)?;
            let Some(index) = index else {
                return Err(vm.throw_error("Error", "Cannot use [] for reading"));
            };
            let k = eval(vm, shared, scope, index)?;
            read_dim(vm, &b, &k, quiet)?
        }
        ExprKind::Prop {
            object,
            name,
            nullsafe,
        } => {
            let o = eval_ex(vm, shared, scope, object, quiet)?;
            if *nullsafe && o.is_null() {
                return Ok(Value::Null);
            }
            let name = member_name(vm, shared, scope, name)?;
            match o {
                Value::Object(o) => read_prop(vm, &o, &name, quiet)?,
                other => {
                    if !quiet {
                        vm.engine_warning(format!(
                            "Attempt to read property \"{name}\" on {}",
                            other.type_name()
                        ))?;
                    }
                    Value::Null
                }
            }
        }
        ExprKind::StaticProp { class, name, .. } => {
            let ce = resolve_class(vm, shared, scope, class)?;
            let name = var_name(vm, shared, scope, name)?;
            match ce.find_property(&name).filter(|p| p.is_static()) {
                Some(info) => ce.static_members.borrow()[info.slot].get(),
                None => {
                    return Err(vm.throw_error(
                        "Error",
                        format!(
                            "Access to undeclared static property {}::${}",
                            ce.name,
                            String::from_utf8_lossy(&name)
                        ),
                    ));
                }
            }
        }
        ExprKind::Call { callee, args } => eval_call(vm, shared, scope, callee, args)?,
        ExprKind::MethodCall {
            object,
            name,
            args,
            nullsafe,
        } => {
            let o = eval(vm, shared, scope, object)?;
            if *nullsafe && o.is_null() {
                return Ok(Value::Null);
            }
            let name = member_name(vm, shared, scope, name)?;
            let Value::Object(o) = o else {
                return Err(vm.throw_error(
                    "Error",
                    format!("Call to a member function {name}() on {}", o.type_name()),
                ));
            };
            let Some(m) = o.class().find_method(&name).cloned() else {
                return Err(vm.throw_error(
                    "Error",
                    format!("Call to undefined method {}::{name}()", o.class().name),
                ));
            };
            let (pos, named) = eval_args(vm, shared, scope, Some(&m), args)?;
            let this = if m.is_static() { None } else { Some(o.clone()) };
            call_function_value(vm, &m, pos, named, this, Some(o.class().clone()))?
        }
        ExprKind::StaticCall { class, name, args } => {
            let ce = resolve_class(vm, shared, scope, class)?;
            let name = member_name(vm, shared, scope, name)?;
            let Some(m) = ce.find_method(&name).cloned() else {
                return Err(vm.throw_error(
                    "Error",
                    format!("Call to undefined method {}::{name}()", ce.name),
                ));
            };
            let (pos, named) = eval_args(vm, shared, scope, Some(&m), args)?;
            let forwarding = matches!(class, AstClassRef::Name(n) if n.special_class().is_some());
            if m.is_static() {
                let called = if forwarding {
                    scope.static_class.clone().unwrap_or(ce.clone())
                } else {
                    ce.clone()
                };
                call_function_value(vm, &m, pos, named, None, Some(called))?
            } else {
                match &scope.this {
                    Some(this) if this.class().instance_of(&ce) => call_function_value(
                        vm,
                        &m,
                        pos,
                        named,
                        Some(this.clone()),
                        Some(ce.clone()),
                    )?,
                    _ => {
                        return Err(vm.throw_error(
                            "Error",
                            format!(
                                "Non-static method {}::{}() cannot be called statically",
                                m.scope.clone().unwrap_or_default(),
                                m.name
                            ),
                        ));
                    }
                }
            }
        }
        ExprKind::New { class, args } => {
            let ce = match class {
                NewClass::Name(n) => resolve_class_name(vm, scope, n)?,
                NewClass::Expr(x) => {
                    resolve_class(vm, shared, scope, &AstClassRef::Expr(x.clone()))?
                }
                NewClass::Anonymous(c) => declare_class(vm, shared, c)?,
            };
            let ctor = ce.magic.constructor.clone();
            let (pos, named) = eval_args(vm, shared, scope, ctor.as_ref(), args)?;
            instantiate(vm, &ce, pos, named)?
        }
        ExprKind::Assign { target, value } => {
            let v = eval(vm, shared, scope, value)?;
            let v = v.into_unref();
            assign_to(vm, shared, scope, target, v.clone())?;
            v
        }
        ExprKind::AssignRef { target, value } => {
            let (ExprKind::Var(t), ExprKind::Var(_)) = (&target.kind, &value.kind) else {
                return Err(unsupported(vm, "reference assignment"));
            };
            let r = lvalue_ref(vm, shared, scope, value)?;
            let name = var_name(vm, shared, scope, t)?;
            scope.vars.borrow_mut().insert(name, r.clone());
            r.get()
        }
        ExprKind::AssignOp { op, target, value } => {
            let cur = eval(vm, shared, scope, target)?;
            let rhs = eval(vm, shared, scope, value)?;
            let v = binary(vm, *op, &cur, &rhs)?;
            assign_to(vm, shared, scope, target, v.clone())?;
            v
        }
        ExprKind::AssignCoalesce { target, value } => {
            let cur = eval_ex(vm, shared, scope, target, true)?;
            if cur.is_null() {
                let v = eval(vm, shared, scope, value)?;
                assign_to(vm, shared, scope, target, v.clone())?;
                v
            } else {
                cur
            }
        }
        ExprKind::IncDec { op, target } => {
            let cur = eval(vm, shared, scope, target)?;
            let new = match op {
                IncDecOp::PreInc | IncDecOp::PostInc => vm.increment(&cur)?,
                IncDecOp::PreDec | IncDecOp::PostDec => vm.decrement(&cur)?,
            };
            assign_to(vm, shared, scope, target, new.clone())?;
            match op {
                IncDecOp::PreInc | IncDecOp::PreDec => new,
                _ => cur,
            }
        }
        ExprKind::Binary { op, left, right } => match op {
            BinaryOp::BooleanAnd => {
                let l = eval(vm, shared, scope, left)?;
                if !vm.to_bool(&l)? {
                    Value::Bool(false)
                } else {
                    let r = eval(vm, shared, scope, right)?;
                    Value::Bool(vm.to_bool(&r)?)
                }
            }
            BinaryOp::BooleanOr => {
                let l = eval(vm, shared, scope, left)?;
                if vm.to_bool(&l)? {
                    Value::Bool(true)
                } else {
                    let r = eval(vm, shared, scope, right)?;
                    Value::Bool(vm.to_bool(&r)?)
                }
            }
            BinaryOp::Coalesce => {
                let l = eval_ex(vm, shared, scope, left, true)?;
                if l.is_null() {
                    eval(vm, shared, scope, right)?
                } else {
                    l
                }
            }
            op => {
                let l = eval(vm, shared, scope, left)?;
                let r = eval(vm, shared, scope, right)?;
                binary(vm, *op, &l, &r)?
            }
        },
        ExprKind::Unary { op, expr } => {
            let v = eval(vm, shared, scope, expr)?;
            match op {
                UnaryOp::Not => Value::Bool(!vm.to_bool(&v)?),
                UnaryOp::BitNot => vm.bitwise_not(&v)?,
                UnaryOp::Plus => vm.unary_plus(&v)?,
                UnaryOp::Minus => vm.negate(&v)?,
            }
        }
        ExprKind::Cast { kind, expr } => {
            let v = eval(vm, shared, scope, expr)?;
            match kind {
                CastKind::Int => Value::Int(vm.to_int(&v)?),
                CastKind::Float => Value::Float(vm.to_float(&v)?),
                CastKind::String => Value::String(vm.to_php_string(&v)?),
                CastKind::Bool => Value::Bool(vm.to_bool(&v)?),
                CastKind::Array => Value::array(vm.cast_to_array(&v)?),
                CastKind::Object => Value::Object(vm.cast_to_object(&v)?),
                CastKind::Unset => Value::Null,
            }
        }
        ExprKind::VoidCast(x) => {
            eval(vm, shared, scope, x)?;
            Value::Null
        }
        ExprKind::Silence(x) => {
            let tok = vm.begin_silence();
            let r = eval(vm, shared, scope, x);
            vm.end_silence(tok);
            r?
        }
        ExprKind::Isset(es) => {
            for x in es {
                let v = eval_ex(vm, shared, scope, x, true)?;
                if v.is_null() {
                    return Ok(Value::Bool(false));
                }
            }
            Value::Bool(true)
        }
        ExprKind::Empty(x) => {
            let v = eval_ex(vm, shared, scope, x, true)?;
            Value::Bool(!vm.to_bool(&v)?)
        }
        ExprKind::Include { expr, .. } => {
            let v = eval(vm, shared, scope, expr)?;
            let path = vm.to_php_string(&v)?.to_string();
            let file = path.rsplit('/').next().unwrap_or(&path).to_string();
            let full = format!("{}/{file}", shared.dir);
            let Ok(code) = std::fs::read(&full) else {
                return Err(unsupported(vm, &format!("include of {path}")));
            };
            let prog = match php_syntax::parse(&code, &file) {
                Ok(p) => p,
                Err(err) => return Err(unsupported(vm, &format!("include parse error {err}"))),
            };
            hoist(vm, shared, &prog.stmts);
            match exec_block(vm, shared, scope, &prog.stmts) {
                Ok(()) => Value::Int(1),
                Err(Flow::Return(v)) => v,
                Err(e) => return Err(e),
            }
        }
        ExprKind::Print(x) => {
            let v = eval(vm, shared, scope, x)?;
            let s = vm.to_php_string(&v)?;
            vm.echo(&s)?;
            Value::Int(1)
        }
        ExprKind::Ternary {
            cond, then, else_, ..
        } => {
            let c = eval(vm, shared, scope, cond)?;
            if vm.to_bool(&c)? {
                match then {
                    Some(t) => eval(vm, shared, scope, t)?,
                    None => c,
                }
            } else {
                eval(vm, shared, scope, else_)?
            }
        }
        ExprKind::Match { subject, arms } => {
            let v = eval(vm, shared, scope, subject)?;
            for arm in arms {
                match &arm.conds {
                    None => return eval(vm, shared, scope, &arm.body),
                    Some(cs) => {
                        for c in cs {
                            let cv = eval(vm, shared, scope, c)?;
                            if vm.is_identical(&v, &cv)? {
                                return eval(vm, shared, scope, &arm.body);
                            }
                        }
                    }
                }
            }
            return Err(vm.throw_error("UnhandledMatchError", "Unhandled match case"));
        }
        ExprKind::Instanceof { expr, class } => {
            let v = eval(vm, shared, scope, expr)?;
            let Value::Object(o) = v else {
                return Ok(Value::Bool(false));
            };
            match class {
                AstClassRef::Name(n) if n.special_class().is_none() => {
                    Value::Bool(o.class().instance_of_name(n.resolved.as_bytes()))
                }
                c => {
                    let ce = resolve_class(vm, shared, scope, c)?;
                    Value::Bool(o.class().instance_of(&ce))
                }
            }
        }
        ExprKind::Throw(x) => {
            let v = eval(vm, shared, scope, x)?;
            match v {
                Value::Object(o) => return Err(Flow::Exception(o)),
                _ => return Err(vm.throw_error("Error", "Can only throw objects")),
            }
        }
        _ => return Err(unsupported(vm, "expression")),
    })
}

fn eval_array_item(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    item: &ArrayItem,
    a: &mut PhpArray,
) -> PResult<()> {
    if item.unpack {
        let v = eval(vm, shared, scope, &item.value)?;
        if let Value::Array(src) = v {
            for (k, v) in src.iter() {
                match k {
                    ArrayKey::Int(_) => {
                        let _ = a.append(v.unref());
                    }
                    k => {
                        a.insert(k, v.unref());
                    }
                }
            }
        }
        return Ok(());
    }
    let v = eval(vm, shared, scope, &item.value)?;
    match &item.key {
        Some(k) => {
            let k = eval(vm, shared, scope, k)?;
            let key = vm.to_array_key(&k, OffsetKind::Write)?;
            a.insert(key, v);
        }
        None => {
            if a.append(v).is_err() {
                return Err(vm.throw_error(
                    "Error",
                    "Cannot add element to the array as the next element is already occupied",
                ));
            }
        }
    }
    Ok(())
}

fn read_dim(vm: &mut Vm, b: &Value, k: &Value, quiet: bool) -> PResult<Value> {
    match b {
        Value::Array(a) => {
            let key = vm.to_array_key(k, OffsetKind::Read)?;
            match a.get(&key) {
                Some(v) => Ok(v.unref()),
                None => {
                    if !quiet {
                        match &key {
                            ArrayKey::Int(i) => {
                                vm.engine_warning(format!("Undefined array key {i}"))?
                            }
                            ArrayKey::Str(s) => {
                                vm.engine_warning(format!("Undefined array key \"{s}\""))?
                            }
                        }
                    }
                    Ok(Value::Null)
                }
            }
        }
        Value::String(s) => {
            let i = vm.to_int(k)?;
            let len = s.len() as i64;
            let idx = if i < 0 { len + i } else { i };
            if idx < 0 || idx >= len {
                if !quiet {
                    vm.engine_warning(format!("Uninitialized string offset {i}"))?;
                }
                return Ok(Value::empty_string());
            }
            Ok(Value::bytes(&s[idx as usize..idx as usize + 1]))
        }
        Value::Object(o) if o.class().instance_of_name(b"ArrayAccess") => {
            let o = o.clone();
            vm.call_method(&o, b"offsetGet", vec![k.clone()])
        }
        Value::Null | Value::Undef => Ok(Value::Null),
        other => {
            if !quiet {
                vm.engine_warning(format!(
                    "Trying to access array offset on {}",
                    other.type_name()
                ))?;
            }
            Ok(Value::Null)
        }
    }
}

fn binary(vm: &mut Vm, op: BinaryOp, l: &Value, r: &Value) -> PResult<Value> {
    Ok(match op {
        BinaryOp::Add => vm.add(l, r)?,
        BinaryOp::Sub => vm.sub(l, r)?,
        BinaryOp::Mul => vm.mul(l, r)?,
        BinaryOp::Div => vm.div(l, r)?,
        BinaryOp::Mod => vm.modulo(l, r)?,
        BinaryOp::Pow => vm.pow(l, r)?,
        BinaryOp::Concat => vm.concat(l, r)?,
        BinaryOp::BitAnd => vm.bitwise_and(l, r)?,
        BinaryOp::BitOr => vm.bitwise_or(l, r)?,
        BinaryOp::BitXor => vm.bitwise_xor(l, r)?,
        BinaryOp::ShiftLeft => vm.shift_left(l, r)?,
        BinaryOp::ShiftRight => vm.shift_right(l, r)?,
        BinaryOp::LogicalXor => vm.bool_xor(l, r)?,
        BinaryOp::Identical => Value::Bool(vm.is_identical(l, r)?),
        BinaryOp::NotIdentical => Value::Bool(!vm.is_identical(l, r)?),
        BinaryOp::Equal => Value::Bool(vm.is_equal(l, r)?),
        BinaryOp::NotEqual => Value::Bool(vm.is_not_equal(l, r)?),
        BinaryOp::Less => Value::Bool(vm.is_smaller(l, r)?),
        BinaryOp::LessEqual => Value::Bool(vm.is_smaller_or_equal(l, r)?),
        BinaryOp::Greater => Value::Bool(vm.is_smaller(r, l)?),
        BinaryOp::GreaterEqual => Value::Bool(vm.is_smaller_or_equal(r, l)?),
        BinaryOp::Spaceship => vm.spaceship(l, r)?,
        BinaryOp::BooleanAnd => Value::Bool(vm.to_bool(l)? && vm.to_bool(r)?),
        BinaryOp::BooleanOr => Value::Bool(vm.to_bool(l)? || vm.to_bool(r)?),
        BinaryOp::Coalesce => {
            if l.is_null() {
                r.clone()
            } else {
                l.clone()
            }
        }
        BinaryOp::Pipe => return Err(unsupported(vm, "pipe operator")),
    })
}

// ---------------------------------------------------------------------------
// Function calls and the interpreter-provided built-ins
// ---------------------------------------------------------------------------

fn eval_call(
    vm: &mut Vm,
    shared: &Rc<Shared>,
    scope: &Scope,
    callee: &Callee,
    args: &ArgList,
) -> PResult<Value> {
    let name = match callee {
        Callee::Name(n) => n,
        Callee::Expr(e) => {
            let f = eval(vm, shared, scope, e)?;
            let (pos, named) = eval_args(vm, shared, scope, None, args)?;
            if !named.is_empty() {
                return Err(unsupported(vm, "named args to dynamic call"));
            }
            return vm.call(&f, pos);
        }
    };
    let lc = name.resolved.to_ascii_lowercase();
    let lc_short = name.name.to_ascii_lowercase();
    // Language constructs parsed as calls.
    match lc_short.as_bytes() {
        b"exit" | b"die" => {
            if let Some(Arg::Positional(e)) = args.args.first() {
                let v = eval(vm, shared, scope, e)?;
                if let Value::String(s) = &v {
                    vm.echo(s)?;
                }
            }
            return Err(Flow::Exit(0));
        }
        b"clone" => {
            let Some(Arg::Positional(e)) = args.args.first() else {
                return Err(unsupported(vm, "clone"));
            };
            let v = eval(vm, shared, scope, e)?;
            return clone_value(vm, &v);
        }
        _ => {}
    }
    let func = vm
        .find_function(lc.as_bytes())
        .or_else(|| vm.find_function(lc_short.as_bytes()));
    if let Some(f) = func {
        let (pos, named) = eval_args(vm, shared, scope, Some(&f), args)?;
        return call_function_value(vm, &f, pos, named, None, None);
    }
    let (pos, named) = eval_args(vm, shared, scope, None, args)?;
    if !named.is_empty() {
        return Err(unsupported(vm, "named args to a minphp built-in"));
    }
    builtin(vm, shared, scope, lc_short.as_bytes(), pos)
}

fn clone_value(vm: &mut Vm, v: &Value) -> PResult<Value> {
    let Value::Object(o) = v else {
        return Err(vm.throw_error("Error", "__clone method called on non-object"));
    };
    let new = match o.class().handlers.clone_obj {
        Some(h) => h(vm, o)?,
        None => {
            let n = ObjRef::new(o.class().clone());
            *n.obj().props.borrow_mut() = o.obj().props.borrow().clone();
            *n.obj().dyn_props.borrow_mut() = o.obj().dyn_props.borrow().clone();
            n
        }
    };
    if let Some(m) = o.class().magic.clone.clone()
        && !m.is_native()
    {
        call_function_value(vm, &m, Vec::new(), Vec::new(), Some(new.clone()), None)?;
    }
    Ok(Value::Object(new))
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null).into_unref()
}

fn builtin(
    vm: &mut Vm,
    _shared: &Rc<Shared>,
    scope: &Scope,
    name: &[u8],
    args: Vec<Value>,
) -> PResult<Value> {
    Ok(match name {
        b"var_dump" => {
            for a in &args {
                let mut out = Vec::new();
                var_dump_into(vm, &mut out, &a.unref(), 0)?;
                vm.echo(&out)?;
            }
            Value::Null
        }
        b"var_export" => {
            let mut out = Vec::new();
            var_export_into(vm, &mut out, &arg(&args, 0), 1)?;
            if vm.to_bool(&arg(&args, 1))? {
                Value::from(out)
            } else {
                vm.echo(&out)?;
                Value::Null
            }
        }
        b"print_r" => {
            let mut out = Vec::new();
            print_r_into(vm, &mut out, &arg(&args, 0), 0)?;
            if vm.to_bool(&arg(&args, 1))? {
                Value::from(out)
            } else {
                vm.echo(&out)?;
                Value::Bool(true)
            }
        }
        b"get_class" => match arg(&args, 0) {
            Value::Object(o) => Value::String(o.class().name.clone()),
            Value::Null if args.is_empty() => match &scope.self_class {
                Some(c) => Value::String(c.name.clone()),
                None => {
                    return Err(vm.throw_error(
                        "Error",
                        "get_class() without arguments must be called from within a class",
                    ));
                }
            },
            other => {
                return Err(vm.throw_type_error(format!(
                    "get_class(): Argument #1 ($object) must be of type object, {} given",
                    other.type_name()
                )));
            }
        },
        b"get_parent_class" => match arg(&args, 0) {
            Value::Object(o) => o
                .class()
                .parent
                .as_ref()
                .map_or(Value::Bool(false), |p| Value::String(p.name.clone())),
            _ => Value::Bool(false),
        },
        b"define" => {
            let n = vm.to_php_string(&arg(&args, 0))?;
            Value::Bool(vm.define_constant(n, arg(&args, 1)))
        }
        b"defined" => {
            let n = vm.to_php_string(&arg(&args, 0))?;
            Value::Bool(vm.constant(n.as_bytes()).is_some())
        }
        b"constant" => {
            let n = vm.to_php_string(&arg(&args, 0))?;
            if let Some(pos) = n.windows(2).position(|w| w == b"::") {
                vm.class_constant(&n[..pos], &n[pos + 2..])?
                    .unwrap_or(Value::Null)
            } else {
                vm.fetch_constant(n.as_bytes())?.unwrap_or(Value::Null)
            }
        }
        b"strlen" => Value::Int(vm.to_php_string(&arg(&args, 0))?.len() as i64),
        b"property_exists" => {
            let n = vm.to_php_string(&arg(&args, 1))?;
            match arg(&args, 0) {
                Value::Object(o) => Value::Bool(
                    o.class().find_property(&n).is_some()
                        || o.obj()
                            .dyn_props
                            .borrow()
                            .as_ref()
                            .is_some_and(|d| d.get_str(&n).is_some()),
                ),
                Value::String(c) => match vm.find_class(&c) {
                    Some(ce) => Value::Bool(ce.find_property(&n).is_some()),
                    None => Value::Bool(false),
                },
                _ => Value::Bool(false),
            }
        }
        b"method_exists" => {
            let n = vm.to_php_string(&arg(&args, 1))?;
            let ce = match arg(&args, 0) {
                Value::Object(o) => Some(o.class().clone()),
                Value::String(c) => vm.find_class(&c),
                _ => None,
            };
            Value::Bool(ce.is_some_and(|c| c.find_method(&n).is_some()))
        }
        b"get_object_vars" => match arg(&args, 0) {
            Value::Object(o) => {
                let props = match o.class().handlers.get_properties_for {
                    Some(h) => h(vm, &o, PropPurpose::GetObjectVars)?
                        .unwrap_or_else(|| object_property_table(&o)),
                    None => object_property_table(&o),
                };
                let mut out = PhpArray::new();
                for (k, v) in props.iter() {
                    if let ArrayKey::Str(s) = &k
                        && s.first() == Some(&0)
                    {
                        continue;
                    }
                    out.insert(k, v.unref());
                }
                Value::array(out)
            }
            _ => Value::Null,
        },
        b"class_exists" => {
            let n = vm.to_php_string(&arg(&args, 0))?;
            Value::Bool(vm.find_class(&n).is_some_and(|c| !c.is_interface()))
        }
        b"function_exists" => {
            let n = vm.to_php_string(&arg(&args, 0))?;
            Value::Bool(vm.find_function(&n).is_some())
        }
        b"spl_object_id" => match arg(&args, 0) {
            Value::Object(o) => Value::Int(i64::from(super::support::php_id(&o))),
            _ => Value::Null,
        },
        b"error_reporting" => {
            let old = vm.errors.reporting;
            if !args.is_empty() {
                vm.errors.reporting = vm.to_int(&arg(&args, 0))?;
            }
            Value::Int(old)
        }
        b"gc_collect_cycles" => Value::Int(0),
        b"func_get_args" => return Err(unsupported(vm, "func_get_args")),
        _ => {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Call to undefined function {}()",
                    String::from_utf8_lossy(name)
                ),
            ));
        }
    })
}
