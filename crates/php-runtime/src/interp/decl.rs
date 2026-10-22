//! Function and class declaration (`zend_compile.c` binding,
//! `zend_inheritance.c` linking): inheritance, interfaces, traits, enums,
//! abstract-method and signature-compatibility checks, lazy constant
//! expressions (class constants, property defaults, static properties),
//! autoloading.

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use crate::class::{
    ClassConstant, ClassEntry, ClassRef, ConstValue, EnumBacking, Function, PropertyInfo, class_flags, fn_flags,
    member_flags,
};
use crate::compile::ir::*;
use crate::compile::{CResult, LowerError, Lowerer};
use crate::errors::level;
use crate::native::{DefaultValue, Param, Signature};
use crate::object::ObjRef;
use crate::types::{ClassType, TypeDecl, mask};
use crate::value::{PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

use super::{Ctx, Env, UserFn};

/// Class names as shown in messages (anonymous class names are cut at NUL).
pub fn display_class_name(name: &[u8]) -> String {
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).into_owned()
}

/// A compile-time fatal error at a location (`zend_error_at(E_COMPILE_ERROR, ...)`).
fn fatal_at(vm: &mut Vm, file: &PhpStr, line: u32, msg: String) -> Flow {
    match vm.zend_error_at(level::E_COMPILE_ERROR, file.clone(), line, PhpStr::from(msg)) {
        Err(f) => f,
        Ok(()) => Flow::Fatal,
    }
}

fn fatal(vm: &mut Vm, msg: String) -> Flow {
    let f = vm.current_file.clone();
    let l = vm.current_line;
    fatal_at(vm, &f, l, msg)
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Builds the native-style signature of a userland function (for named
/// arguments, by-reference flags and Reflection).
pub fn user_signature(code: &FuncCode) -> Signature {
    let params = code
        .params
        .iter()
        .map(|p| {
            let mut q = Param::new(p.name.clone(), p.ty.as_ref().map(|t| t.decl.clone()));
            q.by_ref = p.by_ref;
            q.variadic = p.variadic;
            q.default = if p.default.is_some() { Some(DefaultValue::Unknown) } else { None };
            q
        })
        .collect();
    Signature::new(
        &code.name.to_string_lossy(),
        params,
        code.return_type.as_ref().map(|t| t.decl.clone()),
        code.returns_ref(),
    )
}

/// Creates the `Function` object for compiled code.
pub fn make_function(code: &Rc<FuncCode>, name: PhpStr, flags: u32, scope: Option<PhpStr>) -> Function {
    let ufn = UserFn::new(code.clone());
    make_function_from(ufn, name, flags, scope)
}

pub fn make_function_from(ufn: UserFn, name: PhpStr, flags: u32, scope: Option<PhpStr>) -> Function {
    let code = ufn.code.clone();
    let sig = Rc::new(user_signature(&code));
    let mut f_flags = flags;
    if code.returns_ref() {
        f_flags |= fn_flags::RETURNS_REF;
    }
    if code.params.last().is_some_and(|p| p.variadic) {
        f_flags |= fn_flags::VARIADIC;
    }
    if code.is_generator() {
        f_flags |= fn_flags::GENERATOR;
    }
    if code.strict() {
        f_flags |= fn_flags::STRICT_TYPES;
    }
    if code.return_type.is_some() {
        f_flags |= fn_flags::HAS_RETURN_TYPE;
    }
    if code.has(code_flags::DEPRECATED) {
        f_flags |= fn_flags::DEPRECATED;
    }
    if code.has(code_flags::NODISCARD) {
        f_flags |= fn_flags::NODISCARD;
    }
    let mut f = Function::user(name, Rc::new(ufn), sig, f_flags);
    f.scope = scope;
    f.doc_comment = code.doc_comment.clone();
    f.location = Some((code.file.clone(), code.start_line, code.end_line));
    f.attributes = code.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect();
    f
}

fn redeclare_msg(vm: &Vm, def: &FuncDeclDef) -> String {
    let old = vm.functions.get(&def.lc_name);
    match old.and_then(|f| f.location.clone()) {
        Some((file, line, _)) => format!(
            "Cannot redeclare function {}() (previously declared in {}:{})",
            def.name, file, line
        ),
        None => format!("Cannot redeclare function {}()", def.name),
    }
}

/// Top-level function declared while compiling (early binding).
pub(crate) fn declare_function_compile_time(l: &mut Lowerer<'_>, def: &Rc<FuncDeclDef>) -> CResult<()> {
    if l.vm.functions.contains_key(&def.lc_name) {
        let msg = redeclare_msg(l.vm, def);
        return Err(LowerError::Compile(crate::compile::CompileError {
            message: msg,
            line: def.code.start_line,
        }));
    }
    let f = Rc::new(make_function(&def.code, def.name.clone(), member_flags::PUBLIC, None));
    let _ = def.func.set(f.clone());
    l.vm.functions.insert(def.lc_name.clone(), f);
    Ok(())
}

/// `ZEND_DECLARE_FUNCTION` (conditional / nested declarations).
pub fn declare_function_runtime(vm: &mut Vm, def: &Rc<FuncDeclDef>) -> PResult<()> {
    if vm.functions.contains_key(&def.lc_name) {
        vm.current_line = def.code.start_line;
        let msg = redeclare_msg(vm, def);
        return Err(fatal(vm, msg).into_runtime());
    }
    let f = def
        .func
        .get_or_init(|| Rc::new(make_function(&def.code, def.name.clone(), member_flags::PUBLIC, None)))
        .clone();
    vm.functions.insert(def.lc_name.clone(), f);
    Ok(())
}

trait IntoRuntime {
    fn into_runtime(self) -> Flow;
}

impl IntoRuntime for Flow {
    fn into_runtime(self) -> Flow {
        self
    }
}

/// `#[\Deprecated]` on a userland function: E_USER_DEPRECATED at call time.
pub fn emit_user_deprecation(vm: &mut Vm, func: &Function) -> PResult<()> {
    let mut suffix = String::new();
    if let Some(ufn) = super::user_fn(func) {
        if let Some(a) = ufn.code.attributes.iter().find(|a| a.name.eq_ignore_ascii_case(b"Deprecated")) {
            let (msg, since) = deprecated_args(vm, a)?;
            if let Some(s) = since.filter(|s| !s.is_empty()) {
                suffix.push_str(&format!(" since {s}"));
            }
            if let Some(m) = msg.filter(|m| !m.is_empty()) {
                suffix.push_str(&format!(", {m}"));
            }
        }
    }
    let msg = match &func.scope {
        Some(c) => format!("Method {}::{}() is deprecated{suffix}", display_class_name(c), func.name),
        None => format!("Function {}() is deprecated{suffix}", func.name),
    };
    vm.zend_error(level::E_USER_DEPRECATED, msg)
}

/// Evaluates `#[\Deprecated(message, since)]` arguments.
pub fn deprecated_args(vm: &mut Vm, a: &AttrDef) -> PResult<(Option<String>, Option<String>)> {
    let mut message = None;
    let mut since = None;
    for (i, (name, e)) in a.args.iter().enumerate() {
        let v = eval_detached(vm, e, None)?;
        let s = match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        };
        match name.as_deref() {
            Some(b"message") => message = s,
            Some(b"since") => since = s,
            None if i == 0 => message = s,
            None if i == 1 => since = s,
            _ => {}
        }
    }
    Ok((message, since))
}

// ---------------------------------------------------------------------------
// Constant expressions
// ---------------------------------------------------------------------------

/// A lazily evaluated class constant (`ConstValue::Unevaluated` payload).
pub struct ConstPayload {
    pub expr: Rc<ConstExpr>,
    /// Declaring class (for `self::`).
    pub scope: PhpStr,
    pub ty: Option<TypeDecl>,
    pub name: PhpStr,
}

thread_local! {
    static DETACHED: (Rc<FuncCode>, Rc<UserFn>) = {
        let code = Rc::new(FuncCode {
            name: PhpStr::new(),
            kind: FuncKind::Main,
            flags: 0,
            cvs: Rc::new(CvTable::default()),
            params: Vec::new(),
            body: Vec::new(),
            return_type: None,
            file: PhpStr::new(),
            start_line: 0,
            end_line: 0,
            uses: Vec::new(),
            num_statics: 0,
            class_name: None,
            in_trait: false,
            doc_comment: None,
            attributes: Vec::new(),
            labels: Vec::new(),
            namespace: PhpStr::new(),
        });
        let ufn = Rc::new(UserFn::new(code.clone()));
        (code, ufn)
    };
}

/// Evaluates a constant expression outside any function, in `scope`.
pub fn eval_detached(vm: &mut Vm, e: &Expr, scope: Option<&ClassRef>) -> PResult<Value> {
    let (code, ufn) = DETACHED.with(|d| (d.0.clone(), d.1.clone()));
    let mut env = Env::new(code.cvs.clone());
    env.scope = scope.cloned();
    env.called = scope.cloned();
    let env = Rc::new(env);
    let cx = Ctx {
        env: &env,
        code: &code,
        ufn: &ufn,
        map: None,
        goto: Cell::new(None),
        generator: None,
    };
    super::eval::eval(vm, &cx, e).map(Value::into_unref)
}

/// Evaluates a [`ConstExpr`] in the scope of class `scope`.
pub fn eval_const_expr(vm: &mut Vm, ce: &ConstExpr, scope: Option<&ClassRef>) -> PResult<Value> {
    if let Some(v) = &ce.literal {
        return Ok(v.clone());
    }
    let saved_file = std::mem::replace(&mut vm.current_file, ce.file.clone());
    let saved_line = std::mem::replace(&mut vm.current_line, ce.line);
    let r = eval_detached(vm, &ce.expr, scope);
    vm.current_file = saved_file;
    vm.current_line = saved_line;
    r
}

/// Host hook: evaluates an `Unevaluated` class constant payload.
pub fn eval_const_payload(vm: &mut Vm, class: &ClassRef, payload: &Rc<dyn Any>) -> PResult<Value> {
    if let Some(init) = payload.downcast_ref::<crate::enums::EnumCaseInit>() {
        let value = match &init.expr {
            Some(e) => match e.downcast_ref::<ConstPayload>() {
                Some(p) => {
                    let scope = vm.find_class(&p.scope).unwrap_or_else(|| class.clone());
                    Some(eval_const_expr(vm, &p.expr, Some(&scope))?)
                }
                None => None,
            },
            None => None,
        };
        // Enum cases belong to the enum class, not to `class` (a subclass never exists).
        let owner = class.clone();
        let obj = crate::enums::new_case_object(&owner, init, value);
        return Ok(Value::Object(obj));
    }
    let Some(p) = payload.downcast_ref::<ConstPayload>() else {
        return Err(vm.throw_error("Error", format!("Cannot evaluate constant of class {}", class.name)));
    };
    let scope = vm.find_class(&p.scope).unwrap_or_else(|| class.clone());
    let v = eval_const_expr(vm, &p.expr, Some(&scope))?;
    if let Some(ty) = &p.ty {
        if !ty.is_mixed() && !super::call::type_matches(vm, ty, &v, Some(&scope))? {
            let ok_float = matches!(v, Value::Int(_)) && ty.mask & mask::FLOAT != 0;
            if !ok_float {
                return Err(vm.throw_type_error(format!(
                    "Cannot assign {} to class constant {}::{} of type {}",
                    v.value_name(),
                    display_class_name(&p.scope),
                    p.name,
                    ty.to_type_string()
                )));
            }
            return Ok(Value::Float(match v {
                Value::Int(i) => i as f64,
                _ => unreachable!(),
            }));
        }
    }
    Ok(v)
}

/// `A::NAME` with visibility checks (`zend_get_class_constant_ex`).
pub fn class_constant_value(vm: &mut Vm, class: &ClassRef, name: &PhpStr, scope: Option<&ClassRef>) -> PResult<Value> {
    if name.eq_ignore_ascii_case(b"class") {
        return Ok(Value::String(class.name.clone()));
    }
    let Some(c) = class.find_constant(name).cloned() else {
        return Err(vm.throw_error(
            "Error",
            format!("Undefined constant {}::{}", display_class_name(&class.name), name),
        ));
    };
    if c.flags & member_flags::PRIVATE != 0 {
        if !scope.is_some_and(|s| s.name.eq_ignore_ascii_case(&c.declaring_class)) {
            return Err(vm.throw_error(
                "Error",
                format!("Cannot access private constant {}::{}", display_class_name(&class.name), name),
            ));
        }
    } else if c.flags & member_flags::PROTECTED != 0 && !super::objects::protected_ok(&c.declaring_class, scope, vm) {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot access protected constant {}::{}", display_class_name(&class.name), name),
        ));
    }
    if c.flags & member_flags::DEPRECATED != 0 {
        vm.engine_deprecated(format!(
            "Constant {}::{} is deprecated",
            display_class_name(&class.name),
            name
        ))?;
    }
    if let Some(v) = c.evaluated() {
        return Ok(v);
    }
    // Evaluate through the runtime (recursion detection + caching).
    let state = c.value.borrow().clone();
    match state {
        ConstValue::Value(v) => Ok(v),
        ConstValue::Evaluating => Err(vm.throw_error(
            "Error",
            format!(
                "Cannot declare self-referencing constant {}::{}",
                display_class_name(&c.declaring_class),
                c.name
            ),
        )),
        ConstValue::Unevaluated(expr) => {
            *c.value.borrow_mut() = ConstValue::Evaluating;
            let owner = vm.find_class(&c.declaring_class).unwrap_or_else(|| class.clone());
            match eval_const_payload(vm, &owner, &expr) {
                Ok(v) => {
                    *c.value.borrow_mut() = ConstValue::Value(v.clone());
                    Ok(v)
                }
                Err(e) => {
                    *c.value.borrow_mut() = ConstValue::Unevaluated(expr);
                    Err(e)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Class data
// ---------------------------------------------------------------------------

/// Hooks of a property (`PropertyInfo::hooks` payload).
pub struct HookSet {
    pub get: Option<Rc<Function>>,
    pub set: Option<Rc<Function>>,
    /// No backing storage.
    pub virtual_: bool,
}

pub fn prop_hooks(info: &PropertyInfo) -> Option<&HookSet> {
    info.hooks.as_ref().and_then(|h| h.downcast_ref::<HookSet>())
}

/// Whether code is currently running inside a hook of `prop` on `obj`.
pub fn in_hook_of(vm: &Vm, obj: &ObjRef, prop: &[u8]) -> bool {
    for f in vm.frames.iter().rev() {
        if f.is_internal {
            continue;
        }
        let Some(func) = &f.function else { continue };
        if let Some(ufn) = super::user_fn(func) {
            if ufn.code.kind == FuncKind::Hook {
                let n = &ufn.code.name;
                // `$prop::get` / `$prop::set`
                let matches = n.len() > prop.len() + 1 && n[0] == b'$' && &n[1..1 + prop.len()] == prop && n[1 + prop.len()] == b':';
                if matches && f.this.as_ref().is_some_and(|t| ObjRef::ptr_eq(t, obj)) {
                    return true;
                }
            }
        }
        return false;
    }
    false
}

/// Interpreter data of a userland class (`ClassEntry::ext`).
pub struct ClassData {
    pub def: Option<Rc<ClassDef>>,
    /// Instance property slots with constant-expression defaults: (slot, expr, scope class).
    pub const_defaults: Vec<(usize, Rc<ConstExpr>, PhpStr)>,
    /// Static properties with constant-expression defaults declared by this class.
    pub static_inits: Vec<(usize, Rc<ConstExpr>, PhpStr, Option<TypeDecl>, PhpStr)>,
    pub defaults: RefCell<Option<Rc<Vec<Value>>>>,
    pub statics_done: Cell<bool>,
    /// Slots declared by userland classes (vs. an internal parent).
    pub user_slots: Vec<bool>,
    /// Weak self reference (set after registration).
    pub this: OnceCell<Weak<ClassEntry>>,
}

pub fn class_data(class: &ClassEntry) -> Option<&ClassData> {
    class.ext.as_ref().and_then(|e| e.downcast_ref::<ClassData>())
}

/// Default property values with constant expressions evaluated (`None` for
/// classes without such defaults).
pub fn resolved_defaults(vm: &mut Vm, class: &ClassRef) -> PResult<Option<Rc<Vec<Value>>>> {
    let Some(data) = class_data(class) else { return Ok(None) };
    if data.const_defaults.is_empty() {
        return Ok(None);
    }
    if let Some(d) = data.defaults.borrow().as_ref() {
        return Ok(Some(d.clone()));
    }
    let mut d = class.default_properties.clone();
    for (slot, expr, scope) in &data.const_defaults {
        let sc = vm.find_class(scope).unwrap_or_else(|| class.clone());
        let v = eval_const_expr(vm, expr, Some(&sc))?;
        // Typed defaults are checked when evaluated.
        if let Some(info) = class.properties.values().find(|p| p.slot == *slot && !p.is_static()) {
            if let Some(ty) = &info.ty {
                if !ty.is_mixed() && !super::call::type_matches(vm, ty, &v, Some(&sc))? {
                    if !(matches!(v, Value::Int(_)) && ty.mask & mask::FLOAT != 0) {
                        return Err(vm.throw_type_error(format!(
                            "Cannot assign {} to property {}::${} of type {}",
                            v.value_name(),
                            display_class_name(&info.declaring_class),
                            info.name,
                            ty.to_type_string()
                        )));
                    }
                }
            }
        }
        d[*slot] = v;
    }
    let d = Rc::new(d);
    *data.defaults.borrow_mut() = Some(d.clone());
    Ok(Some(d))
}

pub fn slot_is_user(class: &ClassEntry, slot: usize) -> bool {
    class_data(class).is_some_and(|d| d.user_slots.get(slot).copied().unwrap_or(false))
}

/// Evaluates static property defaults on first access.
pub fn init_statics(vm: &mut Vm, class: &ClassRef) -> PResult<()> {
    let Some(data) = class_data(class) else { return Ok(()) };
    if data.statics_done.get() {
        return Ok(());
    }
    data.statics_done.set(true);
    if let Some(p) = &class.parent {
        init_statics(vm, p)?;
    }
    for (slot, expr, scope, ty, name) in &data.static_inits {
        let sc = vm.find_class(scope).unwrap_or_else(|| class.clone());
        let v = match eval_const_expr(vm, expr, Some(&sc)) {
            Ok(v) => v,
            Err(e) => {
                data.statics_done.set(false);
                return Err(e);
            }
        };
        if let Some(ty) = ty {
            if !ty.is_mixed() && !super::call::type_matches(vm, ty, &v, Some(&sc))? && !(matches!(v, Value::Int(_)) && ty.mask & mask::FLOAT != 0) {
                return Err(vm.throw_type_error(format!(
                    "Cannot assign {} to property {}::${} of type {}",
                    v.value_name(),
                    display_class_name(scope),
                    name,
                    ty.to_type_string()
                )));
            }
        }
        if let Some(r) = class.static_members.borrow().get(*slot) {
            r.set(v);
        }
    }
    Ok(())
}

/// `zend_update_class_constants`.
pub fn update_class_constants(vm: &mut Vm, class: &ClassRef) -> PResult<()> {
    let names: Vec<PhpStr> = class.constants.keys().cloned().collect();
    for n in names {
        class_constant_value(vm, class, &n, Some(class))?;
    }
    resolved_defaults(vm, class)?;
    init_statics(vm, class)
}

// ---------------------------------------------------------------------------
// Autoloading
// ---------------------------------------------------------------------------

/// `spl_autoload_call`: runs the registered autoloaders.
pub fn autoload(vm: &mut Vm, name: &[u8]) -> PResult<Option<ClassRef>> {
    if vm.interp.autoload_functions.is_empty() {
        return Ok(None);
    }
    if !valid_class_name(name) {
        return Ok(None);
    }
    let lc = PhpStr::from_bytes(name).to_ascii_lowercase();
    if vm.interp.autoloading.contains(&lc) {
        return Ok(None);
    }
    vm.interp.autoloading.push(lc.clone());
    let loaders = vm.interp.autoload_functions.clone();
    let mut result = Ok(None);
    for l in loaders {
        let r = vm.call(&l, vec![Value::String(PhpStr::from_bytes(name))]);
        if let Err(e) = r {
            result = Err(e);
            break;
        }
        if let Some(c) = vm.find_class(name) {
            result = Ok(Some(c));
            break;
        }
    }
    vm.interp.autoloading.retain(|n| *n != lc);
    result
}

fn valid_class_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80)
}

// ---------------------------------------------------------------------------
// Magic method checks (zend_check_magic_method_implementation)
// ---------------------------------------------------------------------------

pub(crate) fn check_magic_method(l: &mut Lowerer<'_>, code: &FuncCode, flags: u32, class_name: &PhpStr) -> CResult<()> {
    let lc = code.name.to_ascii_lowercase();
    let name = lc.as_bytes();
    if !name.starts_with(b"__") {
        return Ok(());
    }
    let cname = display_class_name(class_name);
    let n_params = code.params.len();
    let err = |msg: String| -> CResult<()> { crate::compile::err(msg, code.start_line) };
    let must_args = |n: usize| -> Option<String> {
        if n_params != n {
            Some(format!(
                "Method {}::{}() must take exactly {} argument{}",
                cname,
                code.name,
                n,
                if n == 1 { "" } else { "s" }
            ))
        } else {
            None
        }
    };
    let is_static = flags & member_flags::STATIC != 0;
    let must_static = |want: bool| -> Option<String> {
        if is_static != want {
            Some(format!(
                "Method {}::{}() {} be static",
                cname,
                code.name,
                if want { "must" } else { "cannot" }
            ))
        } else {
            None
        }
    };
    let by_ref_check = || -> Option<String> {
        code.params.iter().find(|p| p.by_ref).map(|_| {
            format!("Method {}::{}() cannot take arguments by reference", cname, code.name)
        })
    };
    let ret = |want: &str| -> Option<String> {
        match &code.return_type {
            Some(t) if t.text.as_bytes() != want.as_bytes() && !(want == "void" && false) => {
                if want == "string" && t.text.as_bytes() == b"string" {
                    return None;
                }
                Some(format!("{}::{}(): Return type must be {} when declared", cname, code.name, want))
            }
            _ => None,
        }
    };
    let checks: Vec<Option<String>> = match name {
        b"__construct" => {
            if is_static {
                vec![Some(format!("Method {}::{}() cannot be static", cname, code.name))]
            } else if code.return_type.is_some() {
                vec![Some(format!("Method {}::{}() cannot declare a return type", cname, code.name))]
            } else {
                vec![]
            }
        }
        b"__destruct" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            if code.return_type.is_some() {
                Some(format!("Method {}::{}() cannot declare a return type", cname, code.name))
            } else {
                None
            },
        ],
        b"__clone" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            ret("void"),
        ],
        b"__get" => vec![must_args(1), must_static(false), by_ref_check()],
        b"__set" => vec![must_args(2), must_static(false), by_ref_check(), ret("void")],
        b"__unset" => vec![must_args(1), must_static(false), by_ref_check(), ret("void")],
        b"__isset" => vec![must_args(1), must_static(false), by_ref_check(), ret("bool")],
        b"__call" => vec![must_args(2), must_static(false), by_ref_check()],
        b"__callstatic" => vec![must_args(2), must_static(true), by_ref_check()],
        b"__tostring" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            ret("string"),
        ],
        b"__debuginfo" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            ret("?array"),
        ],
        b"__serialize" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            ret("array"),
        ],
        b"__unserialize" => vec![must_args(1), must_static(false), ret("void")],
        b"__set_state" => vec![must_args(1), must_static(true), ret("object")],
        b"__invoke" => vec![must_static(false)],
        b"__sleep" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            ret("array"),
        ],
        b"__wakeup" => vec![
            must_args(0).map(|_| format!("Method {}::{}() cannot take arguments", cname, code.name)),
            must_static(false),
            ret("void"),
        ],
        _ => vec![],
    };
    let _ = &l;
    for c in checks.into_iter().flatten() {
        return err(c);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Class declaration
// ---------------------------------------------------------------------------

/// Tries to declare a top-level class while compiling (`zend_try_early_bind`).
pub(crate) fn try_early_bind(l: &mut Lowerer<'_>, def: &Rc<ClassDef>) -> CResult<bool> {
    if !def.interfaces.is_empty() || !def.traits.is_empty() || def.kind == ClassKindIr::Enum {
        return Ok(false);
    }
    if l.vm.classes.contains_key(&def.lc_name) {
        // Declared at runtime (where the "already in use" error is raised).
        return Ok(false);
    }
    if let Some(p) = &def.parent {
        if l.vm.find_class(p).is_none() {
            return Ok(false);
        }
    }
    match link_class(l.vm, def) {
        Ok(_) => {
            def.early_bound.set(true);
            Ok(true)
        }
        Err(f) => Err(LowerError::Flow(f)),
    }
}

/// `ZEND_DECLARE_CLASS` at runtime.
pub fn declare_class_runtime(vm: &mut Vm, def: &Rc<ClassDef>) -> PResult<ClassRef> {
    if def.early_bound.get() {
        if let Some(c) = vm.find_class(&def.name) {
            return Ok(c);
        }
    }
    if let Some(existing) = vm.find_class(&def.name) {
        let _ = existing;
        vm.current_line = def.start_line;
        let kind = match def.kind {
            ClassKindIr::Class => "class",
            ClassKindIr::Interface => "interface",
            ClassKindIr::Trait => "trait",
            ClassKindIr::Enum => "enum",
        };
        return Err(fatal(vm, format!(
            "Cannot declare {kind} {}, because the name is already in use",
            display_class_name(&def.name)
        )));
    }
    link_class(vm, def)
}

/// Declares an anonymous class (once).
pub fn declare_anon_class(vm: &mut Vm, def: &Rc<ClassDef>) -> PResult<ClassRef> {
    if let Some(c) = def.declared.borrow().as_ref() {
        return Ok(c.clone());
    }
    let c = link_class(vm, def)?;
    *def.declared.borrow_mut() = Some(c.clone());
    Ok(c)
}

fn kind_word(k: ClassKindIr) -> &'static str {
    match k {
        ClassKindIr::Class => "Class",
        ClassKindIr::Interface => "Interface",
        ClassKindIr::Trait => "Trait",
        ClassKindIr::Enum => "Enum",
    }
}

fn lookup_dep(vm: &mut Vm, name: &PhpStr, what: &str) -> PResult<ClassRef> {
    match vm.lookup_class(name)? {
        Some(c) => Ok(c),
        None => Err(vm.throw_error(
            "Error",
            format!("{what} \"{}\" not found", name.to_string_lossy().trim_start_matches('\\')),
        )),
    }
}

fn vis_name(flags: u32) -> &'static str {
    if flags & member_flags::PRIVATE != 0 {
        "private"
    } else if flags & member_flags::PROTECTED != 0 {
        "protected"
    } else {
        "public"
    }
}

fn vis_rank(flags: u32) -> u8 {
    if flags & member_flags::PRIVATE != 0 {
        2
    } else if flags & member_flags::PROTECTED != 0 {
        1
    } else {
        0
    }
}

/// Links a class definition and registers it (`zend_do_link_class`).
pub fn link_class(vm: &mut Vm, def: &Rc<ClassDef>) -> PResult<ClassRef> {
    let saved_line = vm.current_line;
    vm.current_line = def.start_line;
    let r = link_class_inner(vm, def);
    vm.current_line = saved_line;
    r
}

fn link_class_inner(vm: &mut Vm, def: &Rc<ClassDef>) -> PResult<ClassRef> {
    let file = def.file.clone();
    let cname = display_class_name(&def.name);
    // Parent.
    let parent = match &def.parent {
        Some(p) => {
            let pc = lookup_dep(vm, p, "Class")?;
            if pc.is_interface() {
                return Err(fatal(vm, format!("Class {cname} cannot extend interface {}", pc.name)));
            }
            if pc.is_trait() {
                return Err(fatal(vm, format!("Class {cname} cannot extend trait {}", pc.name)));
            }
            if pc.has_flag(class_flags::FINAL) {
                return Err(fatal(vm, format!(
                    "Class {cname} cannot extend final class {}",
                    pc.name
                )));
            }
            let child_ro = def.flags & class_flags::READONLY != 0;
            let parent_ro = pc.has_flag(class_flags::READONLY);
            if child_ro != parent_ro {
                return Err(fatal(vm, if child_ro {
                    format!("Readonly class {cname} cannot extend non-readonly class {}", pc.name)
                } else {
                    format!("Non-readonly class {cname} cannot extend readonly class {}", pc.name)
                }));
            }
            Some(pc)
        }
        None => None,
    };
    // Interfaces.
    let mut ifaces: Vec<ClassRef> = Vec::new();
    for i in &def.interfaces {
        let ic = lookup_dep(vm, i, "Interface")?;
        if !ic.is_interface() {
            if def.kind == ClassKindIr::Interface {
                return Err(fatal(vm, format!("{cname} cannot implement {} - it is not an interface", ic.name)));
            }
            return Err(fatal(vm, format!("{cname} cannot implement {} - it is not an interface", ic.name)));
        }
        ifaces.push(ic);
    }
    // Traits.
    let mut traits: Vec<ClassRef> = Vec::new();
    for tu in &def.traits {
        for t in &tu.traits {
            let tc = lookup_dep(vm, t, "Trait")?;
            if !tc.is_trait() {
                return Err(fatal(vm, format!(
                    "{cname} cannot use {} - it is not a trait",
                    tc.name
                )));
            }
            traits.push(tc);
        }
    }

    let mut ce = match &parent {
        Some(p) => ClassEntry::new_subclass(def.name.clone(), def.flags, p),
        None => ClassEntry::new(def.name.clone(), def.flags),
    };
    ce.flags = def.flags | class_flags::LINKED;
    ce.location = Some((file.clone(), def.start_line, def.end_line));
    ce.doc_comment = def.doc_comment.clone();
    ce.attributes = def.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect();
    ce.enum_backing = def.enum_backing;
    if let Some(p) = &parent {
        if p.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
            ce.flags |= class_flags::ALLOW_DYNAMIC_PROPERTIES;
        }
        if p.has_flag(class_flags::NO_DYNAMIC_PROPERTIES) {
            ce.flags |= class_flags::NO_DYNAMIC_PROPERTIES;
        }
    }

    let mut data = ClassData {
        def: Some(def.clone()),
        const_defaults: Vec::new(),
        static_inits: Vec::new(),
        defaults: RefCell::new(None),
        statics_done: Cell::new(false),
        user_slots: Vec::new(),
        this: OnceCell::new(),
    };
    if let Some(pd) = parent.as_ref().and_then(|p| class_data(p)) {
        data.const_defaults = pd.const_defaults.clone();
        data.user_slots = pd.user_slots.clone();
    }

    // ----- constants -----
    for c in &def.consts {
        if let Some(existing) = ce.constants.get(&c.name).cloned() {
            if existing.flags & member_flags::FINAL != 0 && !existing.declaring_class.eq_ignore_ascii_case(&def.name) {
                return Err(fatal(vm, format!(
                    "{}::{} cannot override final constant {}::{}",
                    cname, c.name, existing.declaring_class, c.name
                )));
            }
            if vis_rank(c.flags) > vis_rank(existing.flags) && existing.flags & member_flags::PRIVATE == 0 {
                return Err(fatal(vm, format!(
                    "Access level to {}::{} must be {} (as in class {}){}",
                    cname,
                    c.name,
                    vis_name(existing.flags),
                    existing.declaring_class,
                    if existing.flags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
                )));
            }
        }
        let value = match &c.value.literal {
            Some(v) => {
                if let Some(t) = &c.ty {
                    let _ = t;
                }
                ConstValue::Value(v.clone())
            }
            None => ConstValue::Unevaluated(Rc::new(ConstPayload {
                expr: c.value.clone(),
                scope: def.name.clone(),
                ty: c.ty.as_ref().map(|t| t.decl.clone()),
                name: c.name.clone(),
            })),
        };
        let mut flags = c.flags;
        if c.attributes.iter().any(|a| a.name.eq_ignore_ascii_case(b"Deprecated")) {
            flags |= member_flags::DEPRECATED;
        }
        ce.add_constant(ClassConstant {
            name: c.name.clone(),
            value: RefCell::new(value),
            flags,
            declaring_class: def.name.clone(),
            ty: c.ty.as_ref().map(|t| t.decl.clone()),
            doc_comment: c.doc_comment.clone(),
            attributes: c.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect(),
        });
    }

    // ----- enum cases -----
    if def.kind == ClassKindIr::Enum {
        let backing = def.enum_backing.unwrap_or(EnumBacking::Pure);
        crate::enums::declare_enum_props(&mut ce, backing);
        for case in &def.cases {
            let (value, expr) = match &case.value {
                Some(v) => match &v.literal {
                    Some(lit) => (Some(lit.clone()), None),
                    None => (
                        None,
                        Some(Rc::new(ConstPayload {
                            expr: v.clone(),
                            scope: def.name.clone(),
                            ty: None,
                            name: case.name.clone(),
                        }) as Rc<dyn Any>),
                    ),
                },
                None => (None, None),
            };
            let init = crate::enums::EnumCaseInit {
                name: case.name.clone(),
                value,
                expr,
            };
            ce.add_constant(ClassConstant {
                name: case.name.clone(),
                value: RefCell::new(ConstValue::Unevaluated(Rc::new(init))),
                flags: member_flags::PUBLIC | member_flags::ENUM_CASE,
                declaring_class: def.name.clone(),
                ty: None,
                doc_comment: case.doc_comment.clone(),
                attributes: case.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect(),
            });
        }
        crate::enums::add_enum_methods(&mut ce, backing);
        ce.flags |= class_flags::NO_DYNAMIC_PROPERTIES;
    }

    // ----- properties -----
    for p in &def.props {
        let mut flags = p.flags;
        let virtual_ = !p.hooks.is_empty() && p.default.is_none() && hooks_are_virtual(p);
        if virtual_ {
            flags |= member_flags::VIRTUAL;
        }
        if let Some(existing) = ce.properties.get(&p.name).cloned() {
            let parent_private = existing.flags & member_flags::PRIVATE != 0;
            if !parent_private {
                if existing.is_static() != (flags & member_flags::STATIC != 0) {
                    return Err(fatal(vm, if existing.is_static() {
                        format!(
                            "Cannot redeclare static {}::${} as non static {}::${}",
                            existing.declaring_class, p.name, cname, p.name
                        )
                    } else {
                        format!(
                            "Cannot redeclare non static {}::${} as static {}::${}",
                            existing.declaring_class, p.name, cname, p.name
                        )
                    }));
                }
                if existing.is_readonly() != (flags & member_flags::READONLY != 0) {
                    return Err(fatal(vm, if existing.is_readonly() {
                        format!(
                            "Cannot redeclare readonly property {}::${} as non-readonly {}::${}",
                            existing.declaring_class, p.name, cname, p.name
                        )
                    } else {
                        format!(
                            "Cannot redeclare non-readonly property {}::${} as readonly {}::${}",
                            existing.declaring_class, p.name, cname, p.name
                        )
                    }));
                }
                if existing.flags & member_flags::FINAL != 0 {
                    return Err(fatal(vm, format!(
                        "Cannot override final property {}::${}",
                        existing.declaring_class, p.name
                    )));
                }
                if vis_rank(flags) > vis_rank(existing.flags) {
                    return Err(fatal(vm, format!(
                        "Access level to {}::${} must be {} (as in class {}){}",
                        cname,
                        p.name,
                        vis_name(existing.flags),
                        existing.declaring_class,
                        if existing.flags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
                    )));
                }
                // Type invariance.
                let pt = existing.ty.clone();
                let ct = p.ty.as_ref().map(|t| t.decl.clone());
                match (&pt, &ct) {
                    (Some(a), Some(b)) => {
                        if !types_equivalent(vm, a, b, &existing.declaring_class, &def.name) {
                            return Err(fatal(vm, format!(
                                "Type of {}::${} must be {} (as in class {})",
                                cname,
                                p.name,
                                a.to_type_string(),
                                existing.declaring_class
                            )));
                        }
                    }
                    (Some(a), None) => {
                        return Err(fatal(vm, format!(
                            "Type of {}::${} must be {} (as in class {})",
                            cname,
                            p.name,
                            a.to_type_string(),
                            existing.declaring_class
                        )));
                    }
                    (None, Some(_)) => {
                        return Err(fatal(vm, format!(
                            "Type of {}::${} must not be defined (as in class {})",
                            cname, p.name, existing.declaring_class
                        )));
                    }
                    (None, None) => {}
                }
            } else {
                flags |= member_flags::CHANGED;
            }
        }
        let mut info = PropertyInfo::new(p.name.clone(), def.name.clone(), flags);
        info.ty = p.ty.as_ref().map(|t| t.decl.clone());
        info.doc_comment = p.doc_comment.clone();
        info.attributes = p.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect();
        let mut needs_eval = None;
        info.default = match &p.default {
            Some(d) => match &d.literal {
                Some(v) => v.clone(),
                None => {
                    needs_eval = Some(d.clone());
                    Value::Undef
                }
            },
            None => {
                if p.ty.is_some() || !p.hooks.is_empty() {
                    Value::Undef
                } else {
                    Value::Null
                }
            }
        };
        if !p.hooks.is_empty() {
            let mut set = HookSet {
                get: None,
                set: None,
                virtual_,
            };
            for h in &p.hooks {
                if let Some(code) = &h.code {
                    let f = Rc::new(make_function(code, code.name.clone(), member_flags::PUBLIC | h.flags, Some(def.name.clone())));
                    match h.kind {
                        HookKind::Get => set.get = Some(f),
                        HookKind::Set => set.set = Some(f),
                    }
                }
            }
            info.hooks = Some(Rc::new(set));
        }
        let is_static = info.is_static();
        let ty = info.ty.clone();
        ce.declare_property(info);
        let slot = ce.properties.get(&p.name).map(|i| i.slot).unwrap_or(0);
        if !is_static {
            if data.user_slots.len() <= slot {
                data.user_slots.resize(slot + 1, false);
            }
            data.user_slots[slot] = true;
            data.const_defaults.retain(|(s, _, _)| *s != slot);
            if let Some(e) = needs_eval {
                data.const_defaults.push((slot, e, def.name.clone()));
            }
        } else if let Some(e) = needs_eval {
            data.static_inits.push((slot, e, def.name.clone(), ty, p.name.clone()));
        }
    }
    if data.user_slots.len() < ce.default_properties.len() {
        let parent_internal = parent.as_ref().is_some_and(|p| class_data(p).is_none());
        data.user_slots.resize(ce.default_properties.len(), !parent_internal);
    }

    // ----- methods -----
    let mut own_methods: Vec<Rc<Function>> = Vec::new();
    for m in &def.methods {
        let lc = m.name.to_ascii_lowercase();
        let mut flags = m.flags;
        if m.code.attributes.iter().any(|a| a.name.eq_ignore_ascii_case(b"Override")) {
            flags |= member_flags::OVERRIDE;
        }
        if lc.as_bytes() == b"__construct" {
            flags |= fn_flags::CTOR;
        }
        if let Some(pm) = ce.methods.get(&lc).cloned() {
            check_method_override(vm, &ce, &def.name, m, &pm)?;
            if pm.flags & member_flags::PRIVATE != 0 {
                flags |= member_flags::CHANGED;
            }
        }
        let f = make_function(&m.code, m.name.clone(), flags, Some(def.name.clone()));
        let f = ce.add_method(f);
        own_methods.push(f);
    }

    // ----- traits -----
    if !traits.is_empty() {
        apply_traits(vm, &mut ce, def, &traits, &mut data, &mut own_methods)?;
    }

    // ----- interfaces -----
    let mut all_ifaces: Vec<ClassRef> = ce.interfaces.clone();
    for i in &ifaces {
        for sub in i.interfaces.iter().chain(std::iter::once(i)) {
            if !all_ifaces.iter().any(|x| x.lc_name == sub.lc_name) {
                all_ifaces.push(sub.clone());
            }
        }
    }
    // Interface constants.
    for i in &all_ifaces {
        for (n, c) in i.constants.iter() {
            match ce.constants.get(n) {
                Some(existing) => {
                    if !existing.declaring_class.eq_ignore_ascii_case(&c.declaring_class)
                        && existing.declaring_class.eq_ignore_ascii_case(&def.name)
                        && c.flags & member_flags::FINAL != 0
                    {
                        return Err(fatal(vm, format!(
                            "{}::{} cannot override final constant {}::{}",
                            cname, n, display_class_name(&c.declaring_class), n
                        )));
                    }
                }
                None => {
                    ce.constants.insert(n.clone(), c.clone());
                }
            }
        }
    }
    // Interface methods: inherited as abstract when not implemented; checked otherwise.
    for i in &all_ifaces {
        for (lc, im) in i.methods.iter() {
            match ce.methods.get(lc).cloned() {
                Some(m) => {
                    if !Rc::ptr_eq(&m, im) {
                        check_signature(vm, &m, im)?;
                    }
                }
                None => {
                    ce.methods.insert(lc.clone(), im.clone());
                }
            }
        }
    }
    ce.interfaces = all_ifaces.clone();

    // ----- abstract check -----
    if def.kind == ClassKindIr::Class && def.flags & class_flags::ABSTRACT == 0 || def.kind == ClassKindIr::Enum {
        let abstracts: Vec<String> = ce
            .methods
            .values()
            .filter(|m| m.has_flag(member_flags::ABSTRACT))
            .map(|m| format!("{}::{}", display_class_name(m.scope.as_deref().unwrap_or(b"")), m.name))
            .collect();
        if !abstracts.is_empty() {
            let n = abstracts.len();
            let shown: Vec<&str> = abstracts.iter().take(3).map(String::as_str).collect();
            let more = if n > 3 { ", ..." } else { "" };
            let what = if def.kind == ClassKindIr::Enum { "Enum" } else { "Class" };
            return Err(fatal(vm, format!(
                "{what} {cname} contains {n} abstract method{} and must therefore be declared abstract or implement the remaining methods ({}{more})",
                if n == 1 { "" } else { "s" },
                shown.join(", ")
            )));
        }
    }
    // UnitEnum only for enums.
    if def.kind != ClassKindIr::Enum {
        for i in &ifaces {
            if i.lc_name.as_bytes() == b"unitenum" || i.lc_name.as_bytes() == b"backedenum" {
                return Err(fatal(vm, format!("Non-enum class {cname} cannot implement interface {}", i.name)));
            }
        }
    }
    // Traversable must come via Iterator or IteratorAggregate.
    if def.kind == ClassKindIr::Class
        && ce.interfaces.iter().any(|i| i.lc_name.as_bytes() == b"traversable")
        && !ce.interfaces.iter().any(|i| matches!(i.lc_name.as_bytes(), b"iterator" | b"iteratoraggregate"))
        && !parent.as_ref().is_some_and(|p| p.instance_of_name(b"Traversable"))
    {
        return Err(fatal(vm, format!(
            "Class {cname} must implement interface Traversable as part of either Iterator or IteratorAggregate"
        )));
    }
    // #[\Override].
    for f in &own_methods {
        if f.has_flag(member_flags::OVERRIDE) {
            let lc = f.name.to_ascii_lowercase();
            let has_parent = parent.as_ref().is_some_and(|p| p.methods.get(&lc).is_some_and(|m| m.flags & member_flags::PRIVATE == 0))
                || all_ifaces.iter().any(|i| i.methods.contains_key(&lc));
            if !has_parent {
                let (ffile, fline) = f.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or((file.clone(), def.start_line));
                return Err(fatal_at(vm, &ffile, fline, format!(
                    "{}::{}() has #[\\Override] attribute, but no matching parent method exists",
                    cname, f.name
                )));
            }
        }
    }

    ce.link_magic_methods();
    if ce.magic.tostring.is_some() && !ce.interfaces.iter().any(|i| i.lc_name.as_bytes() == b"stringable") {
        if let Some(s) = vm.find_class(b"Stringable") {
            ce.interfaces.push(s);
        }
    }
    let has_user_data = true;
    if has_user_data {
        ce.ext = Some(Rc::new(data));
    }
    let lc = ce.lc_name.clone();
    if vm.classes.contains_key(&lc) {
        return Err(fatal(vm, format!(
            "Cannot declare {} {}, because the name is already in use",
            kind_word(def.kind).to_ascii_lowercase(),
            cname
        )));
    }
    let rc = Rc::new(ce);
    if let Some(d) = class_data(&rc) {
        let _ = d.this.set(Rc::downgrade(&rc));
    }
    // Bind method scopes.
    for f in rc.methods.values() {
        if let Some(u) = super::user_fn(f) {
            if f.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&rc.name)) {
                let _ = u.scope.set(Rc::downgrade(&rc));
            }
        }
    }
    for info in rc.properties.values() {
        if let Some(h) = prop_hooks(info) {
            for f in h.get.iter().chain(h.set.iter()) {
                if let Some(u) = super::user_fn(f) {
                    let _ = u.scope.set(Rc::downgrade(&rc));
                }
            }
        }
    }
    vm.register_class(rc.clone());
    // interface_gets_implemented hooks.
    let hooks = vm.hooks.interface_implemented.clone();
    if !hooks.is_empty() {
        for i in rc.interfaces.clone() {
            for (name, h) in &hooks {
                if i.lc_name == *name {
                    h(vm, &i, &rc)?;
                }
            }
        }
    }
    Ok(rc)
}

fn hooks_are_virtual(p: &PropDef) -> bool {
    // A property is virtual when no hook references its backing value
    // (approximation: get-only or both hooks with explicit bodies).
    let has_get = p.hooks.iter().any(|h| h.kind == HookKind::Get && h.code.is_some());
    let has_set = p.hooks.iter().any(|h| h.kind == HookKind::Set);
    has_get && !has_set
}

/// Checks a method override (`do_inheritance_check_on_method`).
fn check_method_override(vm: &mut Vm, ce: &ClassEntry, class_name: &PhpStr, m: &MethodDef, pm: &Rc<Function>) -> PResult<()> {
    let cname = display_class_name(class_name);
    let pflags = pm.flags;
    let cflags = m.flags;
    let (ffile, fline) = (m.code.file.clone(), m.code.start_line);
    if pflags & member_flags::PRIVATE != 0 && pflags & member_flags::ABSTRACT == 0 {
        return Ok(());
    }
    let pscope = display_class_name(pm.scope.as_deref().unwrap_or(b""));
    if pflags & member_flags::FINAL != 0 {
        return Err(fatal_at(vm, &ffile, fline, format!("Cannot override final method {}::{}()", pscope, pm.name)));
    }
    let p_static = pflags & member_flags::STATIC != 0;
    let c_static = cflags & member_flags::STATIC != 0;
    if p_static != c_static {
        return Err(fatal_at(vm, &ffile, fline, if c_static {
            format!("Cannot make non static method {}::{}() static in class {}", pscope, pm.name, cname)
        } else {
            format!("Cannot make static method {}::{}() non static in class {}", pscope, pm.name, cname)
        }));
    }
    if cflags & member_flags::ABSTRACT != 0 && pflags & member_flags::ABSTRACT == 0 {
        return Err(fatal_at(vm, &ffile, fline, format!(
            "Cannot make non abstract method {}::{}() abstract in class {}",
            pscope, pm.name, cname
        )));
    }
    if vis_rank(cflags) > vis_rank(pflags) {
        return Err(fatal_at(vm, &ffile, fline, format!(
            "Access level to {}::{}() must be {} (as in class {}){}",
            cname,
            m.name,
            vis_name(pflags),
            pscope,
            if pflags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
        )));
    }
    // Constructors are only checked against abstract/interface prototypes.
    let is_ctor = m.name.eq_ignore_ascii_case(b"__construct");
    if is_ctor && pflags & member_flags::ABSTRACT == 0 {
        let from_iface = vm.find_class(pm.scope.as_deref().unwrap_or(b"")).is_some_and(|c| c.is_interface());
        if !from_iface {
            return Ok(());
        }
    }
    let _ = ce;
    let child = make_function(&m.code, m.name.clone(), cflags, Some(class_name.clone()));
    check_signature(vm, &child, pm)
}

/// LSP check between an implementation and its prototype.
fn check_signature(vm: &mut Vm, child: &Function, proto: &Function) -> PResult<()> {
    if proto.flags & member_flags::PRIVATE != 0 && proto.flags & member_flags::ABSTRACT == 0 {
        return Ok(());
    }
    if child.name.eq_ignore_ascii_case(b"__construct") && proto.flags & member_flags::ABSTRACT == 0 {
        let from_iface = vm.find_class(proto.scope.as_deref().unwrap_or(b"")).is_some_and(|c| c.is_interface());
        if !from_iface {
            return Ok(());
        }
    }
    let status = implementation_check(vm, child, proto);
    match status {
        Compat::Ok => Ok(()),
        Compat::Warning => {
            let (file, line) = child.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or_default();
            let has_rtwc = child.attributes.iter().any(|a| {
                a.downcast_ref::<AttrDef>().is_some_and(|a| a.name.eq_ignore_ascii_case(b"ReturnTypeWillChange"))
            });
            if !has_rtwc {
                let msg = format!(
                    "Return type of {} should either be compatible with {}, or the #[\\ReturnTypeWillChange] attribute should be used to temporarily suppress the notice",
                    function_declaration(child),
                    function_declaration(proto)
                );
                vm.zend_error_at(level::E_DEPRECATED, file, line, PhpStr::from(msg))?;
            }
            Ok(())
        }
        Compat::Error => {
            let (file, line) = child.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or_default();
            Err(fatal_at(vm, &file, line, format!(
                "Declaration of {} must be compatible with {}",
                function_declaration(child),
                function_declaration(proto)
            )))
        }
    }
}

enum Compat {
    Ok,
    Warning,
    Error,
}

fn implementation_check(vm: &mut Vm, fe: &Function, proto: &Function) -> Compat {
    let fs = fe.signature().clone();
    let ps = proto.signature().clone();
    if ps.required < fs.required {
        return Compat::Error;
    }
    if ps.returns_ref && !fs.returns_ref {
        return Compat::Error;
    }
    let pv = ps.variadic().is_some();
    let fv = fs.variadic().is_some();
    if pv && !fv {
        return Compat::Error;
    }
    let pn = ps.params.len();
    let fnn = fs.params.len();
    let n = pn.max(fnn);
    let fe_scope = fe.scope.clone().unwrap_or_default();
    let pr_scope = proto.scope.clone().unwrap_or_default();
    for i in 0..n {
        let pa = if i < pn {
            Some(&ps.params[i])
        } else if pv {
            ps.params.last()
        } else {
            None
        };
        let fa = if i < fnn {
            Some(&fs.params[i])
        } else if fv {
            fs.params.last()
        } else {
            None
        };
        let Some(pa) = pa else { continue };
        let Some(fa) = fa else { return Compat::Error };
        // Contravariant parameter types: proto ⊆ child.
        match (&fa.ty, &pa.ty) {
            (None, _) => {}
            (Some(_), None) => return Compat::Error,
            (Some(ft), Some(pt)) => {
                if !ft.is_mixed() && !is_subtype(vm, pt, &pr_scope, ft, &fe_scope) {
                    return Compat::Error;
                }
            }
        }
        if fa.by_ref != pa.by_ref {
            return Compat::Error;
        }
    }
    if let Some(prt) = &ps.return_type {
        let tentative = proto.is_native();
        match &fs.return_type {
            None => {
                if tentative {
                    return Compat::Warning;
                }
                return Compat::Error;
            }
            Some(frt) => {
                if !prt.is_mixed() && !is_subtype(vm, frt, &fe_scope, prt, &pr_scope) {
                    if tentative {
                        return Compat::Warning;
                    }
                    return Compat::Error;
                }
                if prt.is_mixed() && frt.mask & mask::VOID != 0 {
                    return Compat::Error;
                }
            }
        }
    }
    Compat::Ok
}

fn resolve_type_name(n: &[u8], scope: &[u8], vm: &Vm) -> Option<ClassRef> {
    if n.eq_ignore_ascii_case(b"self") {
        return vm.find_class(scope);
    }
    if n.eq_ignore_ascii_case(b"parent") {
        return vm.find_class(scope).and_then(|c| c.parent.clone());
    }
    vm.find_class(n)
}

fn class_name_matches(a: &[u8], a_scope: &[u8], b: &[u8], b_scope: &[u8], vm: &Vm) -> bool {
    let an = if a.eq_ignore_ascii_case(b"self") { a_scope } else { a };
    let bn = if b.eq_ignore_ascii_case(b"self") { b_scope } else { b };
    if an.eq_ignore_ascii_case(bn) {
        return true;
    }
    match (resolve_type_name(a, a_scope, vm), resolve_type_name(b, b_scope, vm)) {
        (Some(ac), Some(bc)) => ac.instance_of(&bc),
        _ => false,
    }
}

/// Whether type `a` (declared in `a_scope`) is a subtype of `b`.
fn is_subtype(vm: &Vm, a: &TypeDecl, a_scope: &[u8], b: &TypeDecl, b_scope: &[u8]) -> bool {
    if b.is_mixed() {
        return true;
    }
    let am = a.mask;
    let bm = b.mask;
    // Scalar/builtin part.
    let simple = am & !(mask::STATIC | mask::OBJECT | mask::CALLABLE | mask::VOID | mask::NEVER);
    let mut bm_eff = bm;
    if bm & mask::BOOL == mask::BOOL {
        bm_eff |= mask::BOOL;
    }
    if am & mask::NEVER != 0 {
        return true;
    }
    if simple & !bm_eff & mask::ANY != 0 {
        // `iterable` (array|Traversable) is handled with the class part.
        return false;
    }
    if am & mask::VOID != 0 && bm & mask::VOID == 0 {
        return false;
    }
    if am & mask::CALLABLE != 0 && bm & mask::CALLABLE == 0 {
        return false;
    }
    if am & mask::OBJECT != 0 && bm & mask::OBJECT == 0 {
        return false;
    }
    if am & mask::STATIC != 0 && bm & (mask::STATIC | mask::OBJECT) == 0 {
        // static ⊆ self class.
        let ok = b.classes.iter().any(|c| match c {
            ClassType::Name(n) => class_name_matches(b"self", a_scope, n, b_scope, vm),
            _ => false,
        });
        if !ok {
            return false;
        }
    }
    for c in &a.classes {
        let ok = match c {
            ClassType::Name(n) => {
                bm & mask::OBJECT != 0
                    || b.classes.iter().any(|bc| match bc {
                        ClassType::Name(bn) => class_name_matches(n, a_scope, bn, b_scope, vm),
                        ClassType::Intersection(bns) => bns.iter().all(|bn| class_name_matches(n, a_scope, bn, b_scope, vm)),
                    })
                    || (bm & mask::CALLABLE != 0 && n.eq_ignore_ascii_case(b"Closure"))
            }
            ClassType::Intersection(ns) => {
                bm & mask::OBJECT != 0
                    || b.classes.iter().any(|bc| match bc {
                        ClassType::Name(bn) => ns.iter().any(|n| class_name_matches(n, a_scope, bn, b_scope, vm)),
                        ClassType::Intersection(bns) => bns.iter().all(|bn| ns.iter().any(|n| class_name_matches(n, a_scope, bn, b_scope, vm))),
                    })
            }
        };
        if !ok {
            return false;
        }
    }
    true
}

fn types_equivalent(vm: &Vm, a: &TypeDecl, b: &TypeDecl, a_scope: &[u8], b_scope: &[u8]) -> bool {
    is_subtype(vm, a, a_scope, b, b_scope) && is_subtype(vm, b, b_scope, a, a_scope)
}

/// `zend_get_function_declaration`.
pub fn function_declaration(f: &Function) -> String {
    let mut s = String::new();
    let sig = f.signature().clone();
    if sig.returns_ref {
        s.push('&');
    }
    if let Some(c) = &f.scope {
        s.push_str(&display_class_name(c));
        s.push_str("::");
    }
    s.push_str(&f.name.to_string_lossy());
    s.push('(');
    let scope_ce: Option<ClassRef> = None;
    let _ = scope_ce;
    let user = super::user_fn(f);
    for (i, p) in sig.params.iter().enumerate() {
        if let Some(t) = &p.ty {
            s.push_str(&type_string_for_decl(t, f));
            s.push(' ');
        }
        if p.by_ref {
            s.push('&');
        }
        if p.variadic {
            s.push_str("...");
        }
        s.push('$');
        s.push_str(&p.name.to_string_lossy());
        if i >= sig.required && !p.variadic {
            s.push_str(" = ");
            match user {
                Some(u) => {
                    let pi = &u.code.params[i];
                    s.push_str(&default_text(pi));
                }
                None => s.push_str(&native_default_text(p)),
            }
        }
        if i + 1 < sig.params.len() {
            s.push_str(", ");
        }
    }
    s.push(')');
    if let Some(rt) = &sig.return_type {
        s.push_str(": ");
        s.push_str(&type_string_for_decl(rt, f));
    }
    s
}

fn type_string_for_decl(t: &TypeDecl, f: &Function) -> String {
    let _ = f;
    t.to_type_string()
}

fn default_text(p: &ParamInfo) -> String {
    match &p.default_const {
        Some(Value::Bool(false)) => "false".into(),
        Some(Value::Bool(true)) => "true".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => {
            let n = s.len().min(10);
            let mut out = format!("'{}", String::from_utf8_lossy(&s[..n]));
            if s.len() > 10 {
                out.push_str("...");
            }
            out.push('\'');
            out
        }
        Some(Value::Array(a)) => {
            if a.is_empty() {
                "[]".into()
            } else {
                "[...]".into()
            }
        }
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Float(d)) => crate::value::float_fmt::double_to_string(*d, 17),
        Some(_) => "<expression>".into(),
        None => match &p.default {
            Some(Expr::Const(c, _)) => c.display.to_string(),
            Some(Expr::ClassConst(cc, _)) => {
                let (cs, m) = &**cc;
                match (cs, m) {
                    (ClassSpec::Named(n), Member::Name(m)) => format!("{n}::{m}"),
                    (ClassSpec::Self_, Member::Name(m)) => format!("self::{m}"),
                    (ClassSpec::Parent, Member::Name(m)) => format!("parent::{m}"),
                    (ClassSpec::Static, Member::Name(m)) => format!("static::{m}"),
                    _ => "<expression>".into(),
                }
            }
            Some(Expr::Array(items, _)) if items.is_empty() => "[]".into(),
            Some(Expr::Array(..)) => "[...]".into(),
            _ => "<expression>".into(),
        },
    }
}

fn native_default_text(p: &Param) -> String {
    match &p.default {
        Some(DefaultValue::Expr(e)) => const_expr_text(e),
        _ => "<default>".into(),
    }
}

fn const_expr_text(e: &crate::native::ConstExpr) -> String {
    use crate::native::ConstExpr as C;
    match e {
        C::Literal(Value::Null) => "null".into(),
        C::Literal(Value::Bool(b)) => if *b { "true".into() } else { "false".into() },
        C::Literal(Value::Int(i)) => i.to_string(),
        C::Literal(Value::Float(d)) => crate::value::float_fmt::double_to_string(*d, 17),
        C::Literal(Value::String(s)) => format!("\"{}\"", s.to_string_lossy()),
        C::EmptyArray => "[]".into(),
        C::Const(n) => n.clone(),
        C::ClassConst(c, n) => format!("{c}::{n}"),
        C::ClassName(c) => format!("{c}::class"),
        _ => "<expression>".into(),
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

fn apply_traits(
    vm: &mut Vm,
    ce: &mut ClassEntry,
    def: &ClassDef,
    traits: &[ClassRef],
    data: &mut ClassData,
    own_methods: &mut Vec<Rc<Function>>,
) -> PResult<()> {
    let cname = display_class_name(&def.name);
    // Collect rules.
    let mut excludes: Vec<(PhpStr, PhpStr)> = Vec::new(); // (trait lc, method lc) excluded
    let mut aliases: Vec<(Option<PhpStr>, PhpStr, u32, Option<PhpStr>, u32)> = Vec::new();
    for tu in &def.traits {
        for r in &tu.rules {
            match r {
                TraitRule::Precedence {
                    trait_name,
                    method,
                    insteadof,
                    line,
                } => {
                    let t = lookup_dep(vm, trait_name, "Trait")?;
                    if !traits.iter().any(|x| x.lc_name == t.lc_name) {
                        vm.current_line = *line;
                        return Err(fatal(vm, format!(
                            "Required Trait {} wasn't added to {}",
                            t.name, cname
                        )));
                    }
                    if t.find_method(method).is_none() {
                        vm.current_line = *line;
                        return Err(fatal(vm, format!(
                            "A precedence rule was defined for {}::{} but this method does not exist",
                            t.name, method
                        )));
                    }
                    for io in insteadof {
                        let x = lookup_dep(vm, io, "Trait")?;
                        if !traits.iter().any(|y| y.lc_name == x.lc_name) {
                            vm.current_line = *line;
                            return Err(fatal(vm, format!(
                                "Required Trait {} wasn't added to {}",
                                x.name, cname
                            )));
                        }
                        if x.lc_name == t.lc_name {
                            vm.current_line = *line;
                            return Err(fatal(vm, format!(
                                "Inconsistent insteadof definition. The method {} is to be used from {}, but {} is also on the exclude list",
                                method, t.name, t.name
                            )));
                        }
                        excludes.push((x.lc_name.clone(), method.to_ascii_lowercase()));
                    }
                }
                TraitRule::Alias {
                    trait_name,
                    method,
                    flags,
                    alias,
                    line,
                } => {
                    let t = match trait_name {
                        Some(tn) => {
                            let t = lookup_dep(vm, tn, "Trait")?;
                            if !traits.iter().any(|x| x.lc_name == t.lc_name) {
                                vm.current_line = *line;
                                return Err(fatal(vm, format!(
                                    "Required Trait {} wasn't added to {}",
                                    t.name, cname
                                )));
                            }
                            if t.find_method(method).is_none() {
                                vm.current_line = *line;
                                return Err(fatal(vm, format!(
                                    "An alias was defined for {}::{} but this method does not exist",
                                    t.name, method
                                )));
                            }
                            Some(t.lc_name.clone())
                        }
                        None => {
                            let found: Vec<&ClassRef> = traits.iter().filter(|t| t.find_method(method).is_some()).collect();
                            if found.is_empty() {
                                vm.current_line = *line;
                                return Err(fatal(vm, format!(
                                    "An alias was defined for {} but this method does not exist",
                                    method
                                )));
                            }
                            if found.len() > 1 && alias.is_some() {
                                let unique: Vec<_> = found.iter().map(|t| t.lc_name.clone()).collect();
                                if unique.len() > 1 {
                                    vm.current_line = *line;
                                    return Err(fatal(vm, format!(
                                        "An alias was defined for method {}(), which exists in both {} and {}. Use {}::{} or {}::{} to resolve the ambiguity",
                                        method, found[0].name, found[1].name, found[0].name, method, found[1].name, method
                                    )));
                                }
                            }
                            None
                        }
                    };
                    aliases.push((t, method.to_ascii_lowercase(), *flags, alias.clone(), *line));
                }
            }
        }
    }
    // Methods.
    let mut added: Vec<(PhpStr, PhpStr)> = Vec::new(); // (method lc, trait name)
    for t in traits {
        for (lc, m) in t.methods.iter() {
            if excludes.iter().any(|(tl, ml)| *tl == t.lc_name && ml == lc) {
                // Still apply aliases of excluded methods.
            }
            let excluded = excludes.iter().any(|(tl, ml)| *tl == t.lc_name && ml == lc);
            // Aliases.
            for (at, am, aflags, alias, _line) in &aliases {
                if am != lc || at.as_ref().is_some_and(|x| *x != t.lc_name) {
                    continue;
                }
                if let Some(alias) = alias {
                    let mut flags = m.flags;
                    if aflags & member_flags::PPP_MASK != 0 {
                        flags = (flags & !member_flags::PPP_MASK) | (aflags & member_flags::PPP_MASK);
                    }
                    if aflags & member_flags::FINAL != 0 {
                        flags |= member_flags::FINAL;
                    }
                    add_trait_method(vm, ce, def, t, m, alias.clone(), flags, own_methods, &mut added)?;
                }
            }
            if excluded {
                continue;
            }
            let mut flags = m.flags;
            for (at, am, aflags, alias, _) in &aliases {
                if am == lc && alias.is_none() && at.as_ref().is_none_or(|x| *x == t.lc_name) {
                    if aflags & member_flags::PPP_MASK != 0 {
                        flags = (flags & !member_flags::PPP_MASK) | (aflags & member_flags::PPP_MASK);
                    }
                    if aflags & member_flags::FINAL != 0 {
                        flags |= member_flags::FINAL;
                    }
                }
            }
            add_trait_method(vm, ce, def, t, m, m.name.clone(), flags, own_methods, &mut added)?;
        }
    }
    // Constants.
    for t in traits {
        for (n, c) in t.constants.iter() {
            match ce.constants.get(n).cloned() {
                Some(existing) => {
                    let same = existing.flags & (member_flags::PPP_MASK | member_flags::FINAL) == c.flags & (member_flags::PPP_MASK | member_flags::FINAL)
                        && match (existing.evaluated(), c.evaluated()) {
                            (Some(a), Some(b)) => a.identical(&b),
                            _ => true,
                        };
                    if !same {
                        return Err(fatal(vm, format!(
                            "{} and {} define the same constant ({}) in the composition of {}. However, the definition differs and is considered incompatible. Class was composed",
                            display_class_name(&existing.declaring_class),
                            t.name,
                            n,
                            cname
                        )));
                    }
                }
                None => {
                    let nc = ClassConstant {
                        name: c.name.clone(),
                        value: RefCell::new(c.value.borrow().clone()),
                        flags: c.flags,
                        declaring_class: def.name.clone(),
                        ty: c.ty.clone(),
                        doc_comment: c.doc_comment.clone(),
                        attributes: c.attributes.clone(),
                    };
                    ce.add_constant(nc);
                }
            }
        }
    }
    // Properties.
    for t in traits {
        let tdata = class_data(t);
        for info in t.properties.values() {
            if let Some(existing) = ce.properties.get(&info.name).cloned() {
                let compatible = existing.flags & (member_flags::PPP_MASK | member_flags::STATIC | member_flags::READONLY)
                    == info.flags & (member_flags::PPP_MASK | member_flags::STATIC | member_flags::READONLY)
                    && existing.ty == info.ty
                    && existing.default.identical(&info.default);
                if !compatible {
                    return Err(fatal(vm, format!(
                        "{} and {} define the same property (${}) in the composition of {}. However, the definition differs and is considered incompatible. Class was composed",
                        display_class_name(&existing.declaring_class),
                        t.name,
                        info.name,
                        cname
                    )));
                }
                if existing.declaring_class.eq_ignore_ascii_case(&def.name) {
                    continue;
                }
            }
            let mut ni = info.clone();
            ni.declaring_class = def.name.clone();
            let default_expr = if info.is_static() {
                tdata.and_then(|d| d.static_inits.iter().find(|(s, ..)| *s == info.slot).map(|x| x.1.clone()))
            } else {
                tdata.and_then(|d| d.const_defaults.iter().find(|(s, ..)| *s == info.slot).map(|x| x.1.clone()))
            };
            if info.is_static() {
                // Fresh storage per using class.
                let v = t.static_members.borrow().get(info.slot).map(|r| r.get()).unwrap_or_default();
                ni.default = v;
            }
            let is_static = ni.is_static();
            let ty = ni.ty.clone();
            let name = ni.name.clone();
            ce.declare_property(ni);
            let slot = ce.properties.get(&name).map(|i| i.slot).unwrap_or(0);
            if !is_static {
                if data.user_slots.len() <= slot {
                    data.user_slots.resize(slot + 1, false);
                }
                data.user_slots[slot] = true;
                if let Some(e) = default_expr {
                    data.const_defaults.push((slot, e, def.name.clone()));
                }
            } else if let Some(e) = default_expr {
                data.static_inits.push((slot, e, def.name.clone(), ty, name));
            }
        }
    }
    if data.user_slots.len() < ce.default_properties.len() {
        data.user_slots.resize(ce.default_properties.len(), true);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add_trait_method(
    vm: &mut Vm,
    ce: &mut ClassEntry,
    def: &ClassDef,
    t: &ClassRef,
    m: &Rc<Function>,
    name: PhpStr,
    flags: u32,
    own_methods: &mut Vec<Rc<Function>>,
    added: &mut Vec<(PhpStr, PhpStr)>,
) -> PResult<()> {
    let lc = name.to_ascii_lowercase();
    let cname = display_class_name(&def.name);
    // A method declared in the class itself wins.
    if own_methods.iter().any(|f| f.name.eq_ignore_ascii_case(&lc) && f.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&def.name)) && super::user_fn(f).is_none_or(|u| u.from_trait.is_none())) {
        if m.has_flag(member_flags::ABSTRACT) {
            let existing = own_methods.iter().find(|f| f.name.eq_ignore_ascii_case(&lc)).cloned().expect("present");
            check_signature(vm, &existing, m)?;
        }
        return Ok(());
    }
    if let Some((_, other)) = added.iter().find(|(l, _)| *l == lc) {
        let other_trait = other.clone();
        // An abstract trait method is satisfied by another trait's method.
        if m.has_flag(member_flags::ABSTRACT) {
            return Ok(());
        }
        let existing = ce.methods.get(&lc).cloned();
        if existing.as_ref().is_some_and(|e| e.has_flag(member_flags::ABSTRACT)) {
            // Replace the abstract one below.
        } else {
            return Err(fatal(vm, format!(
                "Trait method {}::{} has not been applied as {}::{}, because of collision with {}::{}",
                t.name, name, cname, name, other_trait, name
            )));
        }
    }
    let Some(src) = super::user_fn(m) else {
        // Internal trait methods (none in practice).
        return Ok(());
    };
    let mut u = src.fork();
    u.from_trait = Some(t.name.clone());
    let mut f = make_function_from(u, name.clone(), flags, Some(def.name.clone()));
    f.flags = flags | (m.flags & !(member_flags::PPP_MASK | member_flags::FINAL)) & !member_flags::CHANGED;
    if flags & member_flags::PPP_MASK != 0 {
        f.flags = (f.flags & !member_flags::PPP_MASK) | (flags & member_flags::PPP_MASK);
    }
    // Inherited parent method: signature check against a non-private parent.
    if let Some(pm) = ce.methods.get(&lc).cloned() {
        let from_parent = pm.scope.as_ref().is_some_and(|s| !s.eq_ignore_ascii_case(&def.name));
        if from_parent && !m.has_flag(member_flags::ABSTRACT) {
            if pm.flags & member_flags::FINAL != 0 && pm.flags & member_flags::PRIVATE == 0 {
                let (ffile, fline) = f.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or_default();
                return Err(fatal_at(vm, &ffile, fline, format!(
                    "Cannot override final method {}::{}()",
                    display_class_name(pm.scope.as_deref().unwrap_or(b"")),
                    pm.name
                )));
            }
            if pm.flags & member_flags::PRIVATE == 0 {
                check_signature(vm, &f, &pm)?;
            }
        } else if from_parent && m.has_flag(member_flags::ABSTRACT) {
            check_signature(vm, &pm, m)?;
            return Ok(());
        }
    }
    let rc = ce.add_method(f);
    own_methods.push(rc);
    added.push((lc, t.name.clone()));
    Ok(())
}

/// Stack frames referencing a class (silences unused warnings for helpers).
#[allow(dead_code)]
fn _unused(_: &Vm, _: Option<&ObjRef>) {}
