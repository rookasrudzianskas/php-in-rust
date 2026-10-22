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

use super::variance::{self, Inh};
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

/// `#[\Deprecated]` on a userland function: E_USER_DEPRECATED at call time
/// (`zend_deprecated_function`).
pub fn emit_user_deprecation(vm: &mut Vm, func: &Function) -> PResult<()> {
    let mut suffix = String::new();
    if let Some(ufn) = super::user_fn(func) {
        if let Some(a) = ufn.code.attributes.iter().find(|a| a.name.eq_ignore_ascii_case(b"Deprecated")) {
            let scope = ufn.scope_class().or_else(|| func.scope.as_ref().and_then(|s| vm.find_class(s)));
            suffix = deprecation_suffix_in(vm, a, scope.as_ref(), ufn.code.strict())?;
        }
    }
    let msg = match &func.scope {
        Some(c) => format!("Method {}::{}() is deprecated{suffix}", display_class_name(c), func.name),
        None => format!("Function {}() is deprecated{suffix}", func.name),
    };
    vm.zend_error(level::E_USER_DEPRECATED, msg)
}

/// Evaluates `#[\Deprecated(message, since)]` arguments by constructing the
/// attribute object (`zend_get_attribute_object`), so that
/// `Deprecated::__construct()` validates the argument types.
pub fn deprecated_args_in(
    vm: &mut Vm,
    a: &AttrDef,
    scope: Option<&ClassRef>,
    strict: bool,
) -> PResult<(Option<String>, Option<String>)> {
    if a.args.is_empty() {
        return Ok((None, None));
    }
    let mut positional = Vec::new();
    let mut named = Vec::new();
    for (name, e) in &a.args {
        let v = eval_detached(vm, e, scope)?;
        match name {
            Some(n) => named.push((n.clone(), v)),
            None => positional.push(v),
        }
    }
    let Some(class) = vm.find_class(b"Deprecated") else { return Ok((None, None)) };
    let obj = super::call::new_object(vm, &class)?;
    if let Some(ctor) = class.methods.get(b"__construct".as_slice()).cloned() {
        super::call::call_function_args(
            vm,
            &ctor,
            crate::native::CallArgs {
                positional,
                named,
                this: Some(obj.clone()),
                scope: Some(class.clone()),
                strict,
                dynamic: false,
            },
        )?;
    }
    let get = |n: &[u8]| match obj.get_prop(n) {
        Some(Value::String(s)) => Some(s.to_string()),
        _ => None,
    };
    Ok((get(b"message"), get(b"since")))
}

/// Evaluates `#[\Deprecated(message, since)]` arguments.
pub fn deprecated_args(vm: &mut Vm, a: &AttrDef) -> PResult<(Option<String>, Option<String>)> {
    deprecated_args_in(vm, a, None, false)
}

/// `get_deprecation_suffix_from_attribute`: " since X, message".
pub fn deprecation_suffix_in(vm: &mut Vm, a: &AttrDef, scope: Option<&ClassRef>, strict: bool) -> PResult<String> {
    let (msg, since) = deprecated_args_in(vm, a, scope, strict)?;
    let mut suffix = String::new();
    if let Some(s) = since.filter(|s| !s.is_empty()) {
        suffix.push_str(&format!(" since {s}"));
    }
    if let Some(m) = msg.filter(|m| !m.is_empty()) {
        suffix.push_str(&format!(", {m}"));
    }
    Ok(suffix)
}

thread_local! {
    /// Class constants whose deprecation is being reported (`CONST_PROTECT_RECURSION`).
    static DEPRECATING: RefCell<Vec<*const ClassConstant>> = const { RefCell::new(Vec::new()) };
}

/// `zend_deprecated_class_constant`.
fn deprecated_class_constant(vm: &mut Vm, c: &Rc<ClassConstant>, name: &[u8]) -> PResult<()> {
    let ptr = Rc::as_ptr(c);
    if DEPRECATING.with(|d| d.borrow().contains(&ptr)) {
        return Ok(());
    }
    let owner = vm.find_class(&c.declaring_class);
    let user = owner.as_ref().is_some_and(|o| class_data(o).is_some());
    DEPRECATING.with(|d| d.borrow_mut().push(ptr));
    let r = (|| -> PResult<String> {
        let mut suffix = String::new();
        for a in &c.attributes {
            if let Some(a) = a.downcast_ref::<AttrDef>() {
                if a.name.eq_ignore_ascii_case(b"Deprecated") {
                    let strict = owner
                        .as_ref()
                        .and_then(|o| class_data(o))
                        .and_then(|d| d.def.as_ref())
                        .is_some_and(|d| d.strict);
                    suffix = deprecation_suffix_in(vm, a, owner.as_ref(), strict)?;
                }
            } else if let Some(n) = a.downcast_ref::<crate::class::DeprecationNote>() {
                suffix = n.suffix();
            }
        }
        Ok(suffix)
    })();
    DEPRECATING.with(|d| d.borrow_mut().retain(|p| *p != ptr));
    let suffix = r?;
    let kind = if c.flags & member_flags::ENUM_CASE != 0 { "Enum case" } else { "Constant" };
    let msg = format!(
        "{kind} {}::{} is deprecated{suffix}",
        display_class_name(&c.declaring_class),
        String::from_utf8_lossy(name)
    );
    vm.zend_error(if user { level::E_USER_DEPRECATED } else { level::E_DEPRECATED }, msg)
}

/// Checks the cases of a backed user enum (`zend_enum_build_backed_enum_table`):
/// backing value types and duplicates. Runs until it succeeds once.
fn verify_backed_enum(vm: &mut Vm, class: &ClassRef) -> PResult<()> {
    let Some(data) = class_data(class) else { return Ok(()) };
    let Some(def) = data.def.clone() else { return Ok(()) };
    let backing = match def.enum_backing {
        Some(EnumBacking::Int) => EnumBacking::Int,
        Some(EnumBacking::String) => EnumBacking::String,
        _ => return Ok(()),
    };
    if data.enum_state.get() != 0 {
        return Ok(());
    }
    data.enum_state.set(1);
    let r = (|| -> PResult<()> {
        let mut seen: Vec<(Value, PhpStr)> = Vec::new();
        for case in &def.cases {
            let Some(v) = &case.value else { continue };
            let v = eval_const_expr(vm, v, Some(class))?;
            let ok = match backing {
                EnumBacking::Int => matches!(v, Value::Int(_)),
                _ => matches!(v, Value::String(_)),
            };
            if !ok {
                return Err(vm.throw_type_error(format!(
                    "Enum case type {} does not match enum backing type {}",
                    v.type_name(),
                    if backing == EnumBacking::Int { "int" } else { "string" }
                )));
            }
            if let Some((_, prev)) = seen.iter().find(|(x, _)| x.identical(&v)) {
                return Err(vm.throw_error(
                    "Error",
                    format!(
                        "Duplicate value in enum {} for cases {} and {}",
                        display_class_name(&class.name),
                        prev,
                        case.name
                    ),
                ));
            }
            seen.push((v, case.name.clone()));
        }
        Ok(())
    })();
    data.enum_state.set(if r.is_ok() { 2 } else { 0 });
    r
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
        verify_backed_enum(vm, class)?;
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
    if class.is_trait() {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot access trait constant {}::{} directly", display_class_name(&class.name), name),
        ));
    }
    if c.flags & member_flags::DEPRECATED != 0 {
        deprecated_class_constant(vm, &c, name)?;
    }
    if class.is_enum() && class_data(class).is_some() {
        verify_backed_enum(vm, class)?;
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
#[derive(Clone)]
pub struct HookSet {
    pub get: Option<Rc<Function>>,
    pub set: Option<Rc<Function>>,
    /// No backing storage.
    pub virtual_: bool,
}

pub fn prop_hooks(info: &PropertyInfo) -> Option<&HookSet> {
    info.hooks.as_ref().and_then(|h| h.downcast_ref::<HookSet>())
}

/// Whether the running function (`EG(current_execute_data)`) is a hook of
/// property `prop` on `obj` (`zend_is_in_hook` + the `$this` check of
/// `zend_should_call_hook`). Closures, included files and internal functions
/// called from a hook are not "in" the hook.
pub fn in_hook_of(vm: &Vm, obj: &ObjRef, prop: &[u8]) -> bool {
    let Some(f) = vm.frames.last() else { return false };
    let Some(func) = &f.function else { return false };
    let n: &[u8] = match super::user_fn(func) {
        Some(ufn) if ufn.code.kind == FuncKind::Hook => &ufn.code.name,
        Some(_) => return false,
        // The parent hook trampolines (`parent::$prop::get()`).
        None if f.is_internal && func.name.first() == Some(&b'$') => &func.name,
        None => return false,
    };
    // `$prop::get` / `$prop::set`
    let matches = n.len() > prop.len() + 1 && n[0] == b'$' && &n[1..1 + prop.len()] == prop && n[1 + prop.len()] == b':';
    matches && f.this.as_ref().is_some_and(|t| ObjRef::ptr_eq(t, obj))
}


// ---------------------------------------------------------------------------
// Properties (zend_declare_typed_property, do_inherit_property,
// inherit_property_hook, zend_verify_hooked_property)
// ---------------------------------------------------------------------------

const SET_MASK: u32 = member_flags::PUBLIC_SET | member_flags::PROTECTED_SET | member_flags::PRIVATE_SET;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PropVariance {
    Invariant,
    Covariant,
    Contravariant,
}

fn hook_of(h: &HookSet, get: bool) -> Option<&Rc<Function>> {
    if get { h.get.as_ref() } else { h.set.as_ref() }
}

fn hook_slot(h: &mut HookSet, get: bool) -> &mut Option<Rc<Function>> {
    if get { &mut h.get } else { &mut h.set }
}

/// `property_has_operation`: backed properties implement get, and set when
/// they are not readonly.
fn property_has_operation(info: &PropertyInfo, hooks: Option<&HookSet>, get: bool) -> bool {
    (info.flags & member_flags::VIRTUAL == 0 && (get || !info.is_readonly())) || hooks.is_some_and(|h| hook_of(h, get).is_some())
}

/// `prop_get_variance`.
fn prop_variance(info: &PropertyInfo) -> PropVariance {
    if info.flags & member_flags::VIRTUAL != 0 {
        if let Some(h) = prop_hooks(info) {
            if h.set.is_none() {
                return PropVariance::Covariant;
            }
            if h.get.is_none() {
                return PropVariance::Contravariant;
            }
        }
    }
    PropVariance::Invariant
}

fn set_visibility_word(flags: u32) -> &'static str {
    if flags & member_flags::PRIVATE_SET != 0 {
        "private(set)"
    } else if flags & member_flags::PROTECTED_SET != 0 {
        "protected(set)"
    } else {
        "omitted"
    }
}

/// The parameter type of a set hook.
fn set_hook_type(f: &Function) -> Option<TypeDecl> {
    f.signature().params.first().and_then(|p| p.ty.clone())
}

/// The function objects of a property's hooks.
fn build_hooks(def: &ClassDef, p: &PropDef) -> Option<HookSet> {
    if p.hooks.is_empty() {
        return None;
    }
    let mut set = HookSet {
        get: None,
        set: None,
        virtual_: p.flags & member_flags::VIRTUAL != 0,
    };
    for h in &p.hooks {
        let Some(code) = &h.code else { continue };
        let mut flags = h.flags;
        if code.attributes.iter().any(|a| a.name.eq_ignore_ascii_case(b"Override")) {
            flags |= member_flags::OVERRIDE;
        }
        let f = Rc::new(make_function(code, code.name.clone(), flags, Some(def.name.clone())));
        match h.kind {
            HookKind::Get => set.get = Some(f),
            HookKind::Set => set.set = Some(f),
        }
    }
    Some(set)
}

/// Declares the class's own properties and checks them against inherited
/// ones (`zend_compile_prop_decl` + `do_inherit_property`).
fn link_own_properties(vm: &mut Vm, ce: &mut ClassEntry, def: &ClassDef, data: &mut ClassData, cname: &str) -> PResult<()> {
    for p in &def.props {
        let mut info = PropertyInfo::new(p.name.clone(), def.name.clone(), p.flags);
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
        let has_default = p.default.is_some();
        let mut hooks = build_hooks(def, p);
        if info.is_static() {
            if let Some(existing) = ce.properties.get(&p.name).cloned() {
                check_redeclared_property(vm, ce, &existing, &mut info, None, cname, false)?;
            }
            ce.declare_property(info);
            let slot = ce.properties.get(&p.name).map(|i| i.slot).unwrap_or(0);
            if let Some(e) = needs_eval {
                data.static_inits.push((slot, e, def.name.clone(), p.ty.as_ref().map(|t| t.decl.clone()), p.name.clone()));
            }
            continue;
        }
        // Own slot unless the property shares the parent's.
        let mut slot: Option<usize> = None;
        let mut parent_slot_default: Option<(usize, Value)> = None;
        if let Some(existing) = ce.properties.get(&p.name).cloned() {
            let shared = check_redeclared_property(vm, ce, &existing, &mut info, hooks.as_mut(), cname, false)?;
            if !existing.is_static() && existing.flags & member_flags::PRIVATE == 0 && existing.flags & member_flags::VIRTUAL == 0 {
                match shared {
                    SlotShare::UseChild => {
                        parent_slot_default = Some((existing.slot, Value::Undef));
                    }
                    SlotShare::Parent => {
                        let is_virtual_no_default = p.flags & member_flags::VIRTUAL != 0 && !has_default;
                        let d = if is_virtual_no_default {
                            if info.ty.is_some() { Value::Undef } else { Value::Null }
                        } else {
                            info.default.clone()
                        };
                        if is_virtual_no_default {
                            info.default = d.clone();
                        }
                        slot = Some(existing.slot);
                        parent_slot_default = Some((existing.slot, d));
                    }
                }
            }
        }
        if let Some((s, d)) = parent_slot_default {
            ce.default_properties[s] = d;
        }
        let slot = match slot {
            Some(s) => s,
            None => {
                ce.default_properties.push(info.default.clone());
                ce.default_properties.len() - 1
            }
        };
        info.slot = slot;
        if let Some(h) = hooks {
            info.hooks = Some(Rc::new(HookSet {
                virtual_: info.flags & member_flags::VIRTUAL != 0,
                ..h
            }));
        }
        ce.properties.insert(p.name.clone(), info);
        if data.user_slots.len() <= slot {
            data.user_slots.resize(slot + 1, false);
        }
        data.user_slots[slot] = true;
        data.const_defaults.retain(|(s, _, _)| *s != slot);
        if let Some(e) = needs_eval {
            data.const_defaults.push((slot, e, def.name.clone()));
        }
    }
    if data.user_slots.len() < ce.default_properties.len() {
        data.user_slots.resize(ce.default_properties.len(), true);
    }
    Ok(())
}

enum SlotShare {
    /// The child uses the parent's slot.
    Parent,
    /// The child added hooks: it uses its own slot.
    UseChild,
}

/// `do_inherit_property` for a property redeclared in the child.
fn check_redeclared_property(
    vm: &mut Vm,
    ce: &ClassEntry,
    parent: &PropertyInfo,
    child: &mut PropertyInfo,
    mut child_hooks: Option<&mut HookSet>,
    cname: &str,
    parent_is_iface: bool,
) -> PResult<SlotShare> {
    let key = child.name.clone();
    let pname = display_class_name(&parent.declaring_class);
    if parent.flags & (member_flags::PRIVATE | member_flags::CHANGED) != 0 {
        child.flags |= member_flags::CHANGED;
    }
    if parent.flags & member_flags::FINAL != 0 {
        return Err(fatal(vm, format!("Cannot override final property {pname}::${key}")));
    }
    if parent.flags & member_flags::PRIVATE != 0 {
        return Ok(SlotShare::UseChild);
    }
    if parent.is_static() != child.is_static() {
        return Err(fatal(vm, format!(
            "Cannot redeclare {}{pname}::${key} as {}{cname}::${key}",
            if parent.is_static() { "static " } else { "non static " },
            if child.is_static() { "static " } else { "non static " },
        )));
    }
    if parent.is_readonly() != child.is_readonly() && parent.flags & member_flags::ABSTRACT == 0 {
        return Err(fatal(vm, format!(
            "Cannot redeclare {} property {pname}::${key} as {} {cname}::${key}",
            if parent.is_readonly() { "readonly" } else { "non-readonly" },
            if child.is_readonly() { "readonly" } else { "non-readonly" },
        )));
    }
    let parent_hooks = prop_hooks(parent);
    if child.flags & SET_MASK != 0
        && !(parent_hooks.is_some_and(|h| h.set.is_none()) && parent.flags & member_flags::VIRTUAL != 0)
    {
        let mut parent_set = parent.flags & SET_MASK;
        if parent_set == 0 {
            parent_set = (parent.flags & member_flags::PPP_MASK) << 7;
        }
        if child.flags & SET_MASK > parent_set {
            return Err(fatal(vm, format!(
                "Set access level of {cname}::${key} must be {} (as in class {pname}){}",
                set_visibility_word(parent.flags),
                if parent.flags & SET_MASK == 0 { "" } else { " or weaker" }
            )));
        }
    }
    if child.flags & member_flags::PPP_MASK > parent.flags & member_flags::PPP_MASK {
        return Err(fatal(vm, format!(
            "Access level to {cname}::${key} must be {} (as in class {pname}){}",
            vis_name(parent.flags),
            if parent.flags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
        )));
    }
    let mut share = SlotShare::UseChild;
    if !child.is_static() && parent.flags & member_flags::VIRTUAL == 0 {
        // php-src gives a child that adds hooks its own slot (left UNDEF in
        // the parent's), but lists it at the parent's slot position
        // (properties_info_table); sharing the slot is equivalent.
        share = SlotShare::Parent;
        child.flags &= !member_flags::VIRTUAL;
    }
    // Hooks.
    if parent_hooks.is_some() || child_hooks.is_some() {
        let mut new_hooks: Option<HookSet> = None;
        for get in [true, false] {
            let parent_hook = parent_hooks.and_then(|h| hook_of(h, get)).cloned();
            let child_hook = child_hooks.as_deref().and_then(|h| hook_of(h, get)).cloned();
            let Some(ph) = parent_hook else { continue };
            match child_hook {
                None => {
                    if ph.has_flag(member_flags::ABSTRACT) && property_has_operation(child, child_hooks.as_deref(), get) {
                        continue;
                    }
                    match child_hooks.as_deref_mut() {
                        Some(h) => *hook_slot(h, get) = Some(ph.clone()),
                        None => {
                            let h = new_hooks.get_or_insert(HookSet {
                                get: None,
                                set: None,
                                virtual_: false,
                            });
                            *hook_slot(h, get) = Some(ph.clone());
                        }
                    }
                }
                Some(chk) => {
                    if ph.has_flag(member_flags::PRIVATE) {
                        continue;
                    }
                    if ph.has_flag(member_flags::FINAL) {
                        let (file, line) = chk.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or_default();
                        return Err(fatal_at(vm, &file, line, format!(
                            "Cannot override final property hook {}::{}()",
                            display_class_name(ph.scope.as_deref().unwrap_or(b"")),
                            ph.name
                        )));
                    }
                    check_signature(vm, &chk, &ph)?;
                }
            }
        }
        if let Some(h) = new_hooks {
            // A plain property overriding a hooked one keeps the parent's hooks.
            child.hooks = Some(Rc::new(h));
        }
    }
    // Types.
    let variance = prop_variance(parent);
    let child_scope = ce.name.clone();
    match (&parent.ty, &child.ty) {
        (Some(pt), ct) => {
            let ok = match ct {
                None => false,
                Some(ct) => match variance {
                    PropVariance::Invariant => types_equivalent(vm, pt, ct, &parent.declaring_class, &child_scope),
                    PropVariance::Covariant => is_subtype(vm, ct, &child_scope, pt, &parent.declaring_class),
                    PropVariance::Contravariant => is_subtype(vm, pt, &parent.declaring_class, ct, &child_scope),
                },
            };
            if !ok {
                return Err(fatal(vm, format!(
                    "Type of {cname}::${key} must be {}{} (as in class {pname})",
                    match variance {
                        PropVariance::Invariant => "",
                        PropVariance::Covariant => "subtype of ",
                        PropVariance::Contravariant => "supertype of ",
                    },
                    pt.to_type_string()
                )));
            }
            if parent.flags & member_flags::ABSTRACT != 0 {
                if let Some(ps) = parent_hooks.and_then(|h| h.set.as_ref()) {
                    let child_has_set = child_hooks.as_deref().is_some_and(|h| h.set.is_some());
                    if !child_has_set {
                        if let (Some(st), Some(ct)) = (set_hook_type(ps), &child.ty) {
                            if !is_subtype(vm, &st, &parent.declaring_class, ct, &child_scope) {
                                let kind = vm.find_class(&parent.declaring_class).map(|c| c.kind_name()).unwrap_or("class");
                                return Err(fatal(vm, format!(
                                    "Set type of {cname}::${key} must be supertype of {} (as in {} {pname})",
                                    st.to_type_string(),
                                    kind
                                )));
                            }
                        }
                    }
                }
            }
        }
        (None, Some(_)) => {
            return Err(fatal(vm, format!(
                "Type of {cname}::${key} must be omitted to match the parent definition in class {pname}"
            )));
        }
        (None, None) => {}
    }
    let _ = parent_is_iface;
    child.flags &= !member_flags::OVERRIDE;
    Ok(share)
}

/// Interface properties (`do_interface_implementation` →
/// `do_inherit_property`).
fn inherit_interface_properties(vm: &mut Vm, ce: &mut ClassEntry, iface: &ClassRef, cname: &str) -> PResult<()> {
    for (key, prop) in iface.properties.iter() {
        if prop.is_static() {
            continue;
        }
        match ce.properties.get(key).cloned() {
            Some(existing) => {
                if existing.declaring_class.eq_ignore_ascii_case(&prop.declaring_class) {
                    continue;
                }
                let mut child = existing.clone();
                let mut hooks: Option<HookSet> = prop_hooks(&existing).cloned();
                let had_hooks = hooks.is_some();
                check_redeclared_property(vm, ce, prop, &mut child, hooks.as_mut(), cname, true)?;
                if had_hooks {
                    child.hooks = hooks.map(|h| Rc::new(h) as Rc<dyn Any>);
                }
                // Interface properties never share slots.
                child.slot = existing.slot;
                ce.properties.insert(key.clone(), child);
            }
            None => {
                let mut p = prop.clone();
                ce.default_properties.push(Value::Undef);
                p.slot = ce.default_properties.len() - 1;
                ce.properties.insert(key.clone(), p);
            }
        }
    }
    Ok(())
}

/// `zend_verify_hooked_property` (and the set hook variance check) for the
/// class's own hooked properties, after inheritance.
fn verify_own_hooked_properties(vm: &mut Vm, ce: &mut ClassEntry, def: &ClassDef, data: &ClassData) -> PResult<()> {
    let cname = display_class_name(&def.name);
    // foreach over objects with hooked properties (zend_hooked_object_get_iterator).
    if def.kind == ClassKindIr::Class && ce.properties.values().any(|p| prop_hooks(p).is_some()) {
        if ce.handlers.get_iterator.is_none() {
            ce.handlers.get_iterator = Some(super::objects::hooked_get_iterator);
        }
        if ce.handlers.get_properties_for.is_none() {
            ce.handlers.get_properties_for = Some(super::objects::hooked_properties_for);
        }
    }
    for p in &def.props {
        let Some(info) = ce.properties.get(&p.name).cloned() else { continue };
        if !info.declaring_class.eq_ignore_ascii_case(&def.name) {
            continue;
        }
        let Some(hooks) = prop_hooks(&info).cloned() else { continue };
        if p.hooks.is_empty() {
            continue;
        }
        let is_virtual = info.flags & member_flags::VIRTUAL != 0;
        let has_default = p.default.is_some() || data.const_defaults.iter().any(|(s, _, _)| *s == info.slot);
        if is_virtual && has_default {
            return Err(fatal(vm, format!(
                "Cannot specify default value for virtual hooked property {cname}::${}",
                p.name
            )));
        }
        if !is_virtual && info.ty.is_none() && ce.default_properties.get(info.slot).is_some_and(Value::is_undef) {
            ce.default_properties[info.slot] = Value::Null;
            if let Some(i) = ce.properties.get_mut(&p.name) {
                i.default = Value::Null;
            }
        }
        let mut abstract_error = info.flags & member_flags::ABSTRACT != 0;
        for (get, f) in [(true, &hooks.get), (false, &hooks.set)] {
            let Some(f) = f else { continue };
            if get && f.has_flag(fn_flags::RETURNS_REF) && !is_virtual && hooks.set.is_some() {
                return Err(fatal(vm, format!(
                    "Get hook of backed property {cname}::{} with set hook may not return by reference",
                    p.name
                )));
            }
            if f.has_flag(member_flags::ABSTRACT) {
                abstract_error = false;
            }
        }
        if abstract_error {
            return Err(fatal(vm, format!(
                "Abstract property {cname}::${} must specify at least one abstract hook",
                p.name
            )));
        }
        if is_virtual && info.flags & SET_MASK != 0 && (hooks.get.is_none() || hooks.set.is_none()) {
            return Err(fatal(vm, format!(
                "{} virtual property {cname}::${} must not specify asymmetric visibility",
                if hooks.get.is_none() { "set-only" } else { "get-only" },
                p.name
            )));
        }
        // zend_verify_property_hook_variance
        if let Some(set) = &hooks.set {
            if set.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&def.name)) {
                if let Some(pt) = set_hook_type(set) {
                    let ok = match &info.ty {
                        None => false,
                        Some(t) => {
                            !type_classes_known(vm, t, &def.name)
                                || !type_classes_known(vm, &pt, &def.name)
                                || is_subtype(vm, t, &def.name, &pt, &def.name)
                        }
                    };
                    if !ok {
                        let pname = set.signature().params.first().map(|p| p.name.clone()).unwrap_or_default();
                        let (file, line) = set.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or_default();
                        return Err(fatal_at(vm, &file, line, format!(
                            "Type of parameter ${pname} of hook {cname}::${}::set must be compatible with property type",
                            p.name
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether every class named in `t` is declared (else the compatibility
/// check is unresolved and deferred, `INHERITANCE_UNRESOLVED`).
fn type_classes_known(vm: &Vm, t: &TypeDecl, scope: &[u8]) -> bool {
    t.classes.iter().all(|c| match c {
        ClassType::Name(n) => resolve_type_name(n, scope, vm).is_some(),
        ClassType::Intersection(ns) => ns.iter().all(|n| resolve_type_name(n, scope, vm).is_some()),
    })
}

/// Early binding is not possible while property types or set hook parameter
/// types name undeclared classes (unresolved variance obligations).
fn props_have_unresolved_types(vm: &Vm, def: &ClassDef) -> bool {
    def.props.iter().any(|p| {
        let ty_unknown = p.ty.as_ref().is_some_and(|t| !type_classes_known(vm, &t.decl, &def.name));
        let set_unknown = p.hooks.iter().any(|h| {
            h.kind == HookKind::Set
                && h.code.as_ref().is_some_and(|c| {
                    c.params.first().and_then(|p| p.ty.as_ref()).is_some_and(|t| !type_classes_known(vm, &t.decl, &def.name))
                })
        });
        let parent_typed = def
            .parent
            .as_ref()
            .and_then(|pn| vm.find_class(pn))
            .is_some_and(|pc| pc.properties.get(&p.name).is_some_and(|pp| pp.ty.is_some()));
        (ty_unknown && (!p.hooks.is_empty() || parent_typed)) || set_unknown
    })
}

/// Hooks of a trait property imported into `class` are copies scoped to the
/// using class (`zend_do_traits_property_binding`).
fn rebind_trait_hooks(info: &mut PropertyInfo, class: &PhpStr) {
    let Some(h) = prop_hooks(info).cloned() else { return };
    let rebind = |f: &Option<Rc<Function>>| -> Option<Rc<Function>> {
        let f = f.as_ref()?;
        match super::user_fn(f) {
            Some(u) => {
                let mut g = make_function_from(u.fork(), f.name.clone(), f.flags, Some(class.clone()));
                g.attributes = f.attributes.clone();
                Some(Rc::new(g))
            }
            None => Some(f.clone()),
        }
    };
    let new = HookSet {
        get: rebind(&h.get),
        set: rebind(&h.set),
        virtual_: h.virtual_,
    };
    info.hooks = Some(Rc::new(new));
}

/// Abstract hooks of the class's properties, for the abstract-class check
/// (`zend_verify_abstract_class`).
fn abstract_hooks(ce: &ClassEntry) -> Vec<String> {
    let mut out = Vec::new();
    for info in ce.properties.values() {
        if let Some(h) = prop_hooks(info) {
            for f in h.get.iter().chain(h.set.iter()) {
                if f.has_flag(member_flags::ABSTRACT) {
                    out.push(format!("{}::{}", display_class_name(f.scope.as_deref().unwrap_or(b"")), f.name));
                }
            }
        }
    }
    out
}

/// `#[\Override]` on properties and hooks without a parent.
fn check_property_overrides(vm: &mut Vm, ce: &ClassEntry, def: &ClassDef) -> PResult<()> {
    if def.kind == ClassKindIr::Trait {
        return Ok(());
    }
    let cname = display_class_name(&def.name);
    for p in &def.props {
        let Some(info) = ce.properties.get(&p.name) else { continue };
        if !info.declaring_class.eq_ignore_ascii_case(&def.name) {
            continue;
        }
        if info.flags & member_flags::OVERRIDE != 0 {
            vm.current_line = p.line;
            return Err(fatal(vm, format!(
                "{cname}::${} has #[\\Override] attribute, but no matching parent property exists",
                p.name
            )));
        }
        if let Some(h) = prop_hooks(info) {
            // The parent property (class or interface) providing each operation.
            let parents: Vec<PropertyInfo> = ce
                .parent
                .iter()
                .chain(ce.interfaces.iter())
                .filter_map(|c| c.properties.get(&p.name).cloned())
                .filter(|pp| pp.flags & member_flags::PRIVATE == 0)
                .collect();
            for (get, f) in [(true, &h.get), (false, &h.set)] {
                let Some(f) = f else { continue };
                let has_parent = parents.iter().any(|pp| property_has_operation(pp, prop_hooks(pp), get));
                if f.has_flag(member_flags::OVERRIDE) && !has_parent && f.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&def.name)) {
                    let (file, line) = f.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or_default();
                    return Err(fatal_at(vm, &file, line, format!(
                        "{cname}::{}() has #[\\Override] attribute, but no matching parent method exists",
                        f.name
                    )));
                }
            }
        }
    }
    Ok(())
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
    /// Prototypes of the methods of this class (`common.prototype`), by lowercase name.
    pub protos: Vec<(PhpStr, Rc<Function>)>,
    /// Backed enum case check: 0 = pending, 1 = running, 2 = done.
    pub enum_state: Cell<u8>,
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
    if let Some(h) = vm.hooks.autoload {
        if !valid_class_name(name) {
            return Ok(None);
        }
        let lc = PhpStr::from_bytes(name).to_ascii_lowercase();
        if vm.interp.autoloading.contains(&lc) {
            return Ok(None);
        }
        vm.interp.autoloading.push(lc.clone());
        let r = h(vm, name);
        vm.interp.autoloading.retain(|n| *n != lc);
        r?;
        return Ok(vm.find_class(name));
    }
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
    // zend_check_magic_method_implementation (E_COMPILE_ERROR).
    let lc = code.name.to_ascii_lowercase();
    let name = lc.as_bytes();
    if !name.starts_with(b"__") {
        return Ok(());
    }
    enum Step {
        Check(Option<String>),
        Public,
    }
    use Step::{Check, Public};
    let cname = display_class_name(class_name);
    let fname = code.name.to_string();
    let num_args = code.params.iter().filter(|p| !p.variadic).count();
    let args = |n: usize| -> Step {
        if num_args != n {
            return Check(Some(match n {
                0 => format!("Method {cname}::{fname}() cannot take arguments"),
                1 => format!("Method {cname}::{fname}() must take exactly 1 argument"),
                _ => format!("Method {cname}::{fname}() must take exactly {n} arguments"),
            }));
        }
        if code.params.iter().take(n).any(|p| p.by_ref) {
            return Check(Some(format!("Method {cname}::{fname}() cannot take arguments by reference")));
        }
        Check(None)
    };
    let is_static = flags & member_flags::STATIC != 0;
    let non_static = || Check(is_static.then(|| format!("Method {cname}::{fname}() cannot be static")));
    let must_static = || Check((!is_static).then(|| format!("Method {cname}::{fname}() must be static")));
    let nodiscard = code.has(code_flags::NODISCARD);
    let no_return_type = || -> Step {
        if code.return_type.is_some() {
            return Check(Some(format!("Method {cname}::{fname}() cannot declare a return type")));
        }
        if nodiscard {
            return Check(Some(format!("Method {cname}::{fname} cannot be #[\\NoDiscard]")));
        }
        Check(None)
    };
    let arg_type = |i: usize, want: u32| -> Step {
        let Some(t) = code.params.get(i).and_then(|p| p.ty.as_ref()) else { return Check(None) };
        if t.decl.mask & want != 0 {
            return Check(None);
        }
        Check(Some(format!(
            "{cname}::{fname}(): Parameter #{} (${}) must be of type {} when declared",
            i + 1,
            code.params[i].name,
            TypeDecl::from_mask(want).to_type_string()
        )))
    };
    let ret = |want: u32| -> Step {
        if want == mask::VOID && nodiscard {
            return Check(Some(format!("Method {cname}::{fname} cannot be #[\\NoDiscard]")));
        }
        let Some(t) = code.return_type.as_ref().map(|t| &t.decl) else { return Check(None) };
        if t.mask & mask::NEVER != 0 {
            return Check(None);
        }
        let mut complex = !t.classes.is_empty();
        let mut extra = t.mask & !want;
        if extra & mask::STATIC != 0 {
            extra &= !mask::STATIC;
            complex = true;
        }
        if extra != 0 || (complex && want != mask::OBJECT) {
            return Check(Some(format!(
                "{cname}::{fname}(): Return type must be {} when declared",
                TypeDecl::from_mask(want).to_type_string()
            )));
        }
        Check(None)
    };
    let steps: Vec<Step> = match name {
        b"__construct" => vec![non_static(), no_return_type()],
        b"__destruct" => vec![args(0), non_static(), no_return_type()],
        b"__clone" => vec![args(0), non_static(), ret(mask::VOID)],
        b"__get" => vec![args(1), non_static(), Public, arg_type(0, mask::STRING)],
        b"__set" => vec![args(2), non_static(), Public, arg_type(0, mask::STRING), ret(mask::VOID)],
        b"__unset" => vec![args(1), non_static(), Public, arg_type(0, mask::STRING), ret(mask::VOID)],
        b"__isset" => vec![args(1), non_static(), Public, arg_type(0, mask::STRING), ret(mask::BOOL)],
        b"__call" => vec![args(2), non_static(), Public, arg_type(0, mask::STRING), arg_type(1, mask::ARRAY)],
        b"__callstatic" => vec![args(2), must_static(), Public, arg_type(0, mask::STRING), arg_type(1, mask::ARRAY)],
        b"__tostring" => vec![args(0), non_static(), Public, ret(mask::STRING)],
        b"__debuginfo" => vec![args(0), non_static(), Public, ret(mask::ARRAY | mask::NULL)],
        b"__serialize" => vec![args(0), non_static(), Public, ret(mask::ARRAY)],
        b"__unserialize" => vec![args(1), non_static(), Public, arg_type(0, mask::ARRAY), ret(mask::VOID)],
        b"__set_state" => vec![args(1), must_static(), Public, arg_type(0, mask::ARRAY), ret(mask::OBJECT)],
        b"__invoke" => vec![non_static(), Public],
        b"__sleep" => vec![args(0), non_static(), Public, ret(mask::ARRAY)],
        b"__wakeup" => vec![args(0), non_static(), Public, ret(mask::VOID)],
        _ => vec![],
    };
    for s in steps {
        match s {
            Check(Some(m)) => return crate::compile::err(m, code.start_line),
            Check(None) => {}
            Public => {
                if flags & member_flags::PUBLIC == 0 {
                    let msg = format!("The magic method {cname}::{fname}() must have public visibility");
                    l.vm.zend_error_at(level::E_WARNING, code.file.clone(), code.start_line, PhpStr::from(msg))?;
                }
            }
        }
    }
    if name == b"__debuginfo" && code.return_type.as_ref().is_some_and(|t| t.decl.mask & mask::NULL != 0) {
        let msg = format!(
            "Returning null from {cname}::__debugInfo() is deprecated, make the return type non-nullable and return an empty array instead"
        );
        l.vm.zend_error_at(level::E_DEPRECATED, code.file.clone(), code.start_line, PhpStr::from(msg))?;
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
        // Declared at runtime (where the redeclaration error is raised).
        return Ok(false);
    }
    if let Some(p) = &def.parent {
        let Some(pc) = l.vm.find_class(p) else { return Ok(false) };
        if can_early_bind(l.vm, def, &pc) == Inh::Unresolved {
            return Ok(false);
        }
    }
    if props_have_unresolved_types(l.vm, def) {
        return Ok(false);
    }
    match link_class(l.vm, def) {
        Ok(_) => {
            def.early_bound.set(true);
            Ok(true)
        }
        Err(f) => Err(LowerError::Flow(f)),
    }
}

/// `zend_class_redeclaration_error`.
fn redeclaration_msg(old: &ClassEntry) -> String {
    match &old.location {
        Some((file, line, _)) if !old.is_internal() => format!(
            "Cannot redeclare {} {} (previously declared in {}:{})",
            old.kind_name(),
            display_class_name(&old.name),
            file,
            line
        ),
        _ => format!("Cannot redeclare {} {}", old.kind_name(), display_class_name(&old.name)),
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
        vm.current_line = def.start_line;
        let msg = redeclaration_msg(&existing);
        return Err(fatal(vm, msg));
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

/// `zend_get_object_type_uc`.
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

/// `zend_visibility_string`.
fn vis_name(flags: u32) -> &'static str {
    if flags & member_flags::PUBLIC != 0 {
        "public"
    } else if flags & member_flags::PRIVATE != 0 {
        "private"
    } else {
        "protected"
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

fn has_attr(attrs: &[Rc<AttrDef>], name: &str) -> bool {
    attrs.iter().any(|a| a.name.eq_ignore_ascii_case(name.as_bytes()))
}

fn fn_has_attr(f: &Function, name: &str) -> bool {
    f.attributes
        .iter()
        .any(|a| a.downcast_ref::<AttrDef>().is_some_and(|a| a.name.eq_ignore_ascii_case(name.as_bytes())))
}

/// Kind keyword of a class known by name ("class" when unknown).
fn object_type_of(vm: &Vm, name: &[u8], cur: &ClassDef) -> &'static str {
    if cur.name.eq_ignore_ascii_case(name) {
        return match cur.kind {
            ClassKindIr::Class => "class",
            ClassKindIr::Interface => "interface",
            ClassKindIr::Trait => "trait",
            ClassKindIr::Enum => "enum",
        };
    }
    vm.find_class(name).map_or("class", |c| c.kind_name())
}

/// Links a class definition and registers it (`zend_do_link_class`).
pub fn link_class(vm: &mut Vm, def: &Rc<ClassDef>) -> PResult<ClassRef> {
    let saved_line = vm.current_line;
    vm.current_line = def.start_line;
    let r = link_class_outer(vm, def);
    vm.current_line = saved_line;
    r
}

fn link_class_outer(vm: &mut Vm, def: &Rc<ClassDef>) -> PResult<ClassRef> {
    let cname = display_class_name(&def.name);
    // Load the parent, traits and interfaces first.
    let parent = match &def.parent {
        Some(p) => Some(lookup_dep(vm, p, "Class")?),
        None => None,
    };
    let mut traits: Vec<ClassRef> = Vec::new();
    for tu in &def.traits {
        for t in &tu.traits {
            let tc = lookup_dep(vm, t, "Trait")?;
            if !tc.is_trait() {
                return Err(vm.throw_error("Error", format!("{cname} cannot use {} - it is not a trait", tc.name)));
            }
            if let Some(attr) = class_attr(&tc, "Deprecated") {
                let strict = class_data(&tc).and_then(|d| d.def.as_ref()).is_some_and(|d| d.strict);
                let suffix = deprecation_suffix_in(vm, &attr, Some(&tc), strict)?;
                let msg = format!("Trait {} used by {} is deprecated{suffix}", tc.name, cname);
                vm.zend_error(level::E_USER_DEPRECATED, msg)?;
            }
            if !traits.iter().any(|x| Rc::ptr_eq(x, &tc)) {
                traits.push(tc);
            }
        }
    }
    let mut ifaces: Vec<ClassRef> = Vec::new();
    for i in &def.interfaces {
        ifaces.push(lookup_dep(vm, i, "Interface")?);
    }

    // Warnings raised while linking are delayed (`zend_begin_record_errors`).
    let recording = vm.errors.recorded.is_none();
    if recording {
        vm.errors.recorded = Some(Vec::new());
    }
    variance::push(variance::Linking {
        name: def.name.clone(),
        parent: parent.clone(),
        interfaces: ifaces.clone(),
        flags: def.flags,
        unresolved: Vec::new(),
        obligations: Vec::new(),
    });
    let r = link_class_inner(vm, def, parent, traits, ifaces);
    let ctx = variance::pop();
    let r = match r {
        Ok((rc, st)) => resolve_obligations(vm, def, &rc, ctx).and_then(|_| check_override(vm, def, &rc, &st)).map(|_| rc),
        Err(e) => Err(e),
    };
    if recording {
        let rec = vm.errors.recorded.take().unwrap_or_default();
        if r.is_ok() {
            for e in rec {
                vm.zend_error_at(e.level, e.file, e.line, e.message)?;
            }
        }
    }
    let rc = r?;
    // interface_gets_implemented hooks of extensions.
    let hooks = vm.hooks.interface_implemented.clone();
    if !hooks.is_empty() && !rc.is_interface() {
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

/// Per-link bookkeeping.
#[derive(Default)]
struct LinkState {
    /// Lowercase names of methods whose `#[\Override]` was satisfied.
    override_ok: Vec<PhpStr>,
    /// Constants whose `#[\Override]` was satisfied.
    const_override_ok: Vec<PhpStr>,
    /// Methods declared in the class body or imported from traits.
    declared: Vec<Rc<Function>>,
}

/// The attribute `name` of a userland class.
fn class_attr(c: &ClassEntry, name: &str) -> Option<Rc<AttrDef>> {
    class_data(c)
        .and_then(|d| d.def.as_ref())
        .and_then(|d| d.attributes.iter().find(|a| a.name.eq_ignore_ascii_case(name.as_bytes())).cloned())
}


fn filtered_constants(ce: &ClassEntry) -> crate::util::NameMap<Rc<ClassConstant>> {
    let mut m = crate::util::NameMap::new();
    for (k, v) in ce.constants.iter() {
        if v.flags & member_flags::PRIVATE == 0 {
            m.insert(k.clone(), v.clone());
        }
    }
    m
}

/// `do_inherit_constant_check`: `child` (already in the class) against an
/// inherited `parent` constant. Returns whether an `#[\Override]` on the
/// child is satisfied.
fn inherit_constant_check(
    vm: &mut Vm,
    def: &ClassDef,
    parent_c: &ClassConstant,
    child: &ClassConstant,
) -> PResult<bool> {
    let cname = display_class_name(&def.name);
    let name = &child.name;
    if !parent_c.declaring_class.eq_ignore_ascii_case(&child.declaring_class) && parent_c.flags & member_flags::FINAL != 0 {
        return Err(fatal(vm, format!(
            "{}::{} cannot override final constant {}::{}",
            display_class_name(&child.declaring_class),
            name,
            display_class_name(&parent_c.declaring_class),
            name
        )));
    }
    if !child.declaring_class.eq_ignore_ascii_case(&parent_c.declaring_class)
        && !child.declaring_class.eq_ignore_ascii_case(&def.name)
    {
        return Err(fatal(vm, format!(
            "{} {} inherits both {}::{} and {}::{}, which is ambiguous",
            kind_word(def.kind),
            cname,
            display_class_name(&child.declaring_class),
            name,
            display_class_name(&parent_c.declaring_class),
            name
        )));
    }
    if vis_rank(child.flags) > vis_rank(parent_c.flags) {
        let kind = object_type_of(vm, &parent_c.declaring_class, def);
        return Err(fatal(vm, format!(
            "Access level to {}::{} must be {} (as in {} {}){}",
            cname,
            name,
            vis_name(parent_c.flags),
            kind,
            display_class_name(&parent_c.declaring_class),
            if parent_c.flags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
        )));
    }
    let mut ok = false;
    if parent_c.flags & member_flags::PRIVATE == 0 {
        if child.declaring_class.eq_ignore_ascii_case(&def.name) {
            ok = true;
        }
        if let Some(pt) = &parent_c.ty {
            let status = match &child.ty {
                None => Inh::Error,
                Some(ct) => {
                    let mut ck = variance::Checker::new(vm);
                    let st = ck.covariant(&child.declaring_class, ct, &parent_c.declaring_class, pt);
                    ck.finish();
                    st
                }
            };
            match status {
                Inh::Error => return Err(constant_type_error(vm, child, parent_c)),
                Inh::Unresolved => variance::with(|l| {
                    l.obligations.push(variance::Obligation::Constant {
                        child_class: child.declaring_class.clone(),
                        child_ty: child.ty.clone(),
                        parent_class: parent_c.declaring_class.clone(),
                        parent_ty: pt.clone(),
                        name: name.clone(),
                    })
                })
                .unwrap_or(()),
                _ => {}
            }
        }
    }
    Ok(ok)
}

fn constant_type_error(vm: &mut Vm, child: &ClassConstant, parent_c: &ClassConstant) -> Flow {
    let ty = {
        let ck = variance::Checker::new(vm);
        parent_c.ty.as_ref().map(|t| ck.type_string(t, &parent_c.declaring_class)).unwrap_or_default()
    };
    fatal(vm, format!(
        "Type of {}::{} must be compatible with {}::{} of type {}",
        display_class_name(&child.declaring_class),
        child.name,
        display_class_name(&parent_c.declaring_class),
        child.name,
        ty
    ))
}

fn link_class_inner(
    vm: &mut Vm,
    def: &Rc<ClassDef>,
    parent: Option<ClassRef>,
    traits: Vec<ClassRef>,
    ifaces: Vec<ClassRef>,
) -> PResult<(ClassRef, LinkState)> {
    let file = def.file.clone();
    let cname = display_class_name(&def.name);
    let _ = &file;
    let mut st = LinkState::default();
    // Parent checks (`zend_do_inheritance_ex`).
    if let Some(pc) = &parent {
        let pname = display_class_name(&pc.name);
        if pc.is_enum() {
            return Err(fatal(vm, format!("Class {cname} cannot extend enum {pname}")));
        }
        if pc.has_flag(class_flags::FINAL) {
            return Err(fatal(vm, format!("Class {cname} cannot extend final class {pname}")));
        }
        if pc.is_interface() || pc.is_trait() {
            return Err(fatal(vm, format!(
                "Class {cname} cannot extend {} {pname}",
                if pc.is_interface() { "interface" } else { "trait" }
            )));
        }
        let child_ro = def.flags & class_flags::READONLY != 0;
        let parent_ro = pc.has_flag(class_flags::READONLY);
        if child_ro != parent_ro {
            return Err(fatal(vm, format!(
                "{} class {cname} cannot extend {} class {pname}",
                if child_ro { "Readonly" } else { "Non-readonly" },
                if parent_ro { "readonly" } else { "non-readonly" }
            )));
        }
    }

    let mut ce = match &parent {
        Some(p) => {
            let mut ce = ClassEntry::new_subclass(def.name.clone(), def.flags, p);
            // Private constants are not inherited.
            ce.constants = filtered_constants(p);
            ce
        }
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
        protos: Vec::new(),
        enum_state: Cell::new(0),
    };
    if let Some(pd) = parent.as_ref().and_then(|p| class_data(p)) {
        data.const_defaults = pd.const_defaults.clone();
        data.user_slots = pd.user_slots.clone();
    }

    // ----- enum members (zend_enum_register_props / zend_enum_register_funcs) -----
    if def.kind == ClassKindIr::Enum {
        let backing = def.enum_backing.unwrap_or(EnumBacking::Pure);
        crate::enums::declare_enum_props(&mut ce, backing);
        let mut builtin: Vec<&str> = vec!["cases"];
        if backing != EnumBacking::Pure {
            builtin.push("from");
            builtin.push("tryfrom");
        }
        for b in builtin {
            if def.methods.iter().any(|m| m.name.eq_ignore_ascii_case(b.as_bytes())) {
                return Err(fatal(vm, format!("Cannot redeclare {cname}::{b}()")));
            }
        }
        crate::enums::add_enum_methods(&mut ce, backing);
        ce.flags |= class_flags::NO_DYNAMIC_PROPERTIES;
    }

    // ----- constants -----
    for c in &def.consts {
        let value = match &c.value.literal {
            // Integers are allowed as initializers for floats (`zend_is_valid_default_value`).
            Some(Value::Int(i))
                if c.ty.as_ref().is_some_and(|t| t.decl.mask & mask::FLOAT != 0 && t.decl.mask & mask::INT == 0) =>
            {
                ConstValue::Value(Value::Float(*i as f64))
            }
            Some(v) => ConstValue::Value(v.clone()),
            None => ConstValue::Unevaluated(Rc::new(ConstPayload {
                expr: c.value.clone(),
                scope: def.name.clone(),
                ty: c.ty.as_ref().map(|t| t.decl.clone()),
                name: c.name.clone(),
            })),
        };
        let mut flags = c.flags;
        if has_attr(&c.attributes, "Deprecated") {
            flags |= member_flags::DEPRECATED;
        }
        if has_attr(&c.attributes, "Override") {
            flags |= member_flags::OVERRIDE;
        }
        let nc = ClassConstant {
            name: c.name.clone(),
            value: RefCell::new(value),
            flags,
            declaring_class: def.name.clone(),
            ty: c.ty.as_ref().map(|t| t.decl.clone()),
            doc_comment: c.doc_comment.clone(),
            attributes: c.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect(),
        };
        if let Some(existing) = ce.constants.get(&c.name).cloned() {
            if !existing.declaring_class.eq_ignore_ascii_case(&def.name) && inherit_constant_check(vm, def, &existing, &nc)? {
                st.const_override_ok.push(c.name.clone());
            }
        }
        ce.add_constant(nc);
    }

    // ----- enum cases -----
    if def.kind == ClassKindIr::Enum {
        for case in &def.cases {
            let (value, expr) = match &case.value {
                Some(v) => match &v.literal {
                    Some(lit) => (
                        Some(lit.clone()),
                        // Through the interpreter, which checks the backing values.
                        Some(Rc::new(ConstPayload {
                            expr: v.clone(),
                            scope: def.name.clone(),
                            ty: None,
                            name: case.name.clone(),
                        }) as Rc<dyn Any>),
                    ),
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
            let mut flags = member_flags::PUBLIC | member_flags::ENUM_CASE;
            if has_attr(&case.attributes, "Deprecated") {
                flags |= member_flags::DEPRECATED;
            }
            if has_attr(&case.attributes, "Override") {
                flags |= member_flags::OVERRIDE;
            }
            ce.add_constant(ClassConstant {
                name: case.name.clone(),
                value: RefCell::new(ConstValue::Unevaluated(Rc::new(init))),
                flags,
                declaring_class: def.name.clone(),
                ty: None,
                doc_comment: case.doc_comment.clone(),
                attributes: case.attributes.iter().map(|a| a.clone() as Rc<dyn Any>).collect(),
            });
        }
    }

    // ----- properties -----
    link_own_properties(vm, &mut ce, def, &mut data, &cname)?;


    // ----- methods -----
    for m in &def.methods {
        let lc = m.name.to_ascii_lowercase();
        let mut flags = m.flags;
        if m.code.attributes.iter().any(|a| a.name.eq_ignore_ascii_case(b"Override")) {
            flags |= member_flags::OVERRIDE;
        }
        if lc.as_bytes() == b"__construct" {
            flags |= fn_flags::CTOR;
        }
        let mut f = make_function(&m.code, m.name.clone(), flags, Some(def.name.clone()));
        let inherited = ce.methods.get(&lc).cloned();
        let mut pending = None;
        if let Some(pm) = &inherited {
            let pscope = pm.scope.clone().unwrap_or_default();
            let r = inheritance_check_on_method(vm, &f, &def.name, pm, &pscope, parent.as_ref(), INH_CHILD_DEFAULT)?;
            if r.changed {
                f.flags |= member_flags::CHANGED;
            }
            if r.reset_override {
                st.override_ok.push(lc.clone());
            }
            if let Some(p) = r.proto {
                data.protos.push((lc.clone(), p));
            }
            pending = r.check;
        }
        let f = ce.add_method(f);
        if let Some((target, tscope)) = pending {
            perform_delayable_check(vm, &f, &def.name, &target, &tscope)?;
        }
        st.declared.push(f);
    }

    // ----- traits -----
    if !traits.is_empty() {
        apply_traits(vm, &mut ce, def, &traits, &mut data, &mut st)?;
    }

    // ----- interfaces (zend_do_implement_interfaces) -----
    let num_parent = ce.interfaces.len();
    let mut new_ifaces: Vec<ClassRef> = Vec::new();
    for iface in &ifaces {
        if !iface.is_interface() {
            return Err(vm.fatal_error(format!("{cname} cannot implement {} - it is not an interface", iface.name)));
        }
        if ce.interfaces[..num_parent].iter().any(|x| x.lc_name == iface.lc_name) {
            // Already implemented by the parent: only check constant redeclarations.
            for (n, c) in iface.constants.iter() {
                if let Some(existing) = ce.constants.get(n).cloned() {
                    if inherit_constant_check(vm, def, c, &existing)? {
                        st.const_override_ok.push(n.clone());
                    }
                }
            }
            continue;
        }
        if new_ifaces.iter().any(|x| x.lc_name == iface.lc_name) {
            return Err(fatal(vm, format!(
                "{} {cname} cannot implement previously implemented interface {}",
                kind_word(def.kind),
                iface.name
            )));
        }
        new_ifaces.push(iface.clone());
    }
    for i in &new_ifaces {
        ce.interfaces.push(i.clone());
    }
    for i in ce.interfaces[..num_parent].to_vec() {
        implement_interface_hook(vm, def, &ce, &i)?;
    }
    for i in new_ifaces.clone() {
        interface_implementation(vm, &mut ce, def, &i, &mut st, &mut data.protos)?;
    }
    // Stringable for a __toString imported from a trait.
    if def.kind != ClassKindIr::Trait
        && ce.methods.contains_key(b"__tostring".as_slice())
        && !ce.interfaces.iter().any(|i| i.lc_name.as_bytes() == b"stringable")
    {
        if let Some(s) = vm.find_class(b"Stringable") {
            ce.interfaces.push(s.clone());
            variance::add_interface(&s);
            interface_implementation(vm, &mut ce, def, &s, &mut st, &mut data.protos)?;
        }
    }

    verify_own_hooked_properties(vm, &mut ce, def, &data)?;

    // ----- abstract methods (zend_verify_abstract_class) -----
    if matches!(def.kind, ClassKindIr::Class | ClassKindIr::Enum) {
        verify_abstract_class(vm, &ce, def)?;
    }
    // ----- enums (zend_verify_enum) -----
    if def.kind == ClassKindIr::Enum {
        verify_enum(vm, &ce, def)?;
    }

    if def.kind != ClassKindIr::Trait {
        check_property_overrides(vm, &ce, def)?;
    }
    ce.link_magic_methods();
    ce.ext = Some(Rc::new(data));
    let lc = ce.lc_name.clone();
    if let Some(existing) = vm.find_class(&lc) {
        let msg = redeclaration_msg(&existing);
        return Err(fatal(vm, msg));
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
                    if f.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&rc.name)) {
                        let _ = u.scope.set(Rc::downgrade(&rc));
                    }
                }
            }
        }
    }
    vm.register_class(rc.clone());
    Ok((rc, st))
}

/// `do_interface_implementation`.
fn interface_implementation(
    vm: &mut Vm,
    ce: &mut ClassEntry,
    def: &ClassDef,
    iface: &ClassRef,
    st: &mut LinkState,
    protos: &mut Vec<(PhpStr, Rc<Function>)>,
) -> PResult<()> {
    // zend_do_inherit_interfaces: the interface's parents (in reverse order).
    let mut added = Vec::new();
    for sub in iface.interfaces.iter().rev() {
        if !ce.interfaces.iter().any(|x| x.lc_name == sub.lc_name) {
            ce.interfaces.push(sub.clone());
            added.push(sub.clone());
        }
    }
    for sub in &added {
        variance::add_interface(sub);
        implement_interface_hook(vm, def, ce, sub)?;
    }
    // Constants.
    for (n, c) in iface.constants.iter() {
        match ce.constants.get(n).cloned() {
            Some(existing) => {
                if inherit_constant_check(vm, def, c, &existing)? {
                    st.const_override_ok.push(n.clone());
                }
            }
            None => {
                ce.constants.insert(n.clone(), c.clone());
            }
        }
    }
    // Methods.
    let is_iface = def.kind == ClassKindIr::Interface;
    for (lc, im) in iface.methods.iter() {
        match ce.methods.get(lc).cloned() {
            Some(m) => {
                if Rc::ptr_eq(&m, im) {
                    continue;
                }
                let mscope = m.scope.clone().unwrap_or_default();
                let iscope = im.scope.clone().unwrap_or_default();
                let mut flags = INH_CHECK_PROTO | INH_CHECK_VISIBILITY | INH_RESET_OVERRIDE;
                if !is_iface {
                    flags |= INH_SET_CHILD_PROTO;
                }
                let r = inheritance_check_on_method(vm, &m, &mscope, im, &iscope, None, flags)?;
                if r.reset_override && mscope.eq_ignore_ascii_case(&def.name) {
                    st.override_ok.push(lc.clone());
                }
                if let Some(p) = r.proto {
                    protos.push((lc.clone(), p));
                }
                if let Some((target, tscope)) = r.check {
                    perform_delayable_check(vm, &m, &mscope, &target, &tscope)?;
                }
            }
            None => {
                ce.methods.insert(lc.clone(), im.clone());
            }
        }
    }
    // Properties.
    let cname = display_class_name(&def.name);
    inherit_interface_properties(vm, ce, iface, &cname)?;
    implement_interface_hook(vm, def, ce, iface)
}

/// Built-in `interface_gets_implemented` handlers that run during linking
/// (`do_implement_interface`).
fn implement_interface_hook(vm: &mut Vm, def: &ClassDef, ce: &ClassEntry, iface: &ClassRef) -> PResult<()> {
    if def.kind == ClassKindIr::Interface {
        return Ok(());
    }
    let cname = display_class_name(&def.name);
    match iface.lc_name.as_bytes() {
        b"unitenum" => {
            if def.kind != ClassKindIr::Enum {
                return Err(vm.fatal_error(format!("Non-enum class {cname} cannot implement interface {}", iface.name)));
            }
        }
        b"backedenum" => {
            if def.kind != ClassKindIr::Enum {
                return Err(vm.fatal_error(format!("Non-enum class {cname} cannot implement interface {}", iface.name)));
            }
            if def.enum_backing.is_none_or(|b| b == EnumBacking::Pure) {
                return Err(vm.fatal_error(format!("Non-backed enum {cname} cannot implement interface {}", iface.name)));
            }
        }
        b"traversable" => {
            // zend_implement_traversable: only via Iterator or IteratorAggregate.
            if def.flags & class_flags::ABSTRACT == 0 {
                let ok = ce
                    .interfaces
                    .iter()
                    .any(|i| matches!(i.lc_name.as_bytes(), b"iterator" | b"iteratoraggregate"));
                if !ok {
                    let msg = format!(
                        "{} {cname} must implement interface Traversable as part of either Iterator or IteratorAggregate",
                        kind_word(def.kind)
                    );
                    return Err(match vm.zend_error(level::E_CORE_ERROR, msg) {
                        Err(f) => f,
                        Ok(()) => Flow::Fatal,
                    });
                }
            }
        }
        b"throwable" => {
            // zend_implement_throwable: only via Exception or Error.
            let mut root = ce.parent.clone();
            while let Some(p) = root.as_ref().and_then(|r| r.parent.clone()) {
                root = Some(p);
            }
            let ok = root.is_some_and(|r| matches!(r.name.as_bytes(), b"Exception" | b"Error"));
            if !ok {
                let msg = if def.kind == ClassKindIr::Enum {
                    format!("{} {cname} cannot implement interface {}", kind_word(def.kind), iface.name)
                } else {
                    format!(
                        "{} {cname} cannot implement interface {}, extend Exception or Error instead",
                        kind_word(def.kind),
                        iface.name
                    )
                };
                return Err(vm.fatal_error(msg));
            }
        }
        b"serializable" => {
            // zend_implement_serializable.
            let has = |n: &[u8]| ce.methods.contains_key(n);
            if def.flags & class_flags::ABSTRACT == 0 && !(has(b"__serialize") && has(b"__unserialize")) {
                vm.engine_deprecated(format!(
                    "{cname} implements the Serializable interface, which is deprecated. Implement __serialize() and __unserialize() instead (or in addition, if support for old PHP versions is necessary)"
                ))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `zend_verify_abstract_class`.
fn verify_abstract_class(vm: &mut Vm, ce: &ClassEntry, def: &ClassDef) -> PResult<()> {
    let explicit = def.flags & class_flags::ABSTRACT != 0;
    let can_be_abstract = def.kind != ClassKindIr::Enum && def.flags & class_flags::ANON == 0;
    let mut names: Vec<String> = Vec::new();
    let mut cnt = 0usize;
    for m in ce.methods.values() {
        if m.has_flag(member_flags::ABSTRACT) && (!explicit || m.has_flag(member_flags::PRIVATE)) {
            if cnt < 3 {
                names.push(format!("{}::{}", display_class_name(m.scope.as_deref().unwrap_or(b"")), m.name));
            }
            cnt += 1;
        }
    }
    if !explicit {
        for h in abstract_hooks(ce) {
            if cnt < 3 {
                names.push(h);
            }
            cnt += 1;
        }
    }
    if cnt == 0 {
        return Ok(());
    }
    let cname = display_class_name(&def.name);
    let mut list = names.join(", ");
    if cnt > 3 {
        list.push_str(", ...");
    }
    let s = if cnt > 1 { "s" } else { "" };
    let msg = if !explicit && can_be_abstract {
        format!(
            "{} {cname} contains {cnt} abstract method{s} and must therefore be declared abstract or implement the remaining method{s} ({list})",
            kind_word(def.kind)
        )
    } else {
        format!("{} {cname} must implement {cnt} abstract method{s} ({list})", kind_word(def.kind))
    };
    Err(vm.fatal_error(msg))
}

/// `zend_verify_enum`.
fn verify_enum(vm: &mut Vm, ce: &ClassEntry, def: &ClassDef) -> PResult<()> {
    let cname = display_class_name(&def.name);
    let backed = def.enum_backing.is_some_and(|b| b != EnumBacking::Pure);
    for p in ce.properties.values() {
        if p.name.as_bytes() == b"name" || (backed && p.name.as_bytes() == b"value") {
            continue;
        }
        return Err(fatal(vm, format!("Enum {cname} cannot include properties")));
    }
    for (lc, shown) in [
        ("__construct", "__construct"),
        ("__destruct", "__destruct"),
        ("__clone", "__clone"),
        ("__get", "__get"),
        ("__set", "__set"),
        ("__unset", "__unset"),
        ("__isset", "__isset"),
        ("__tostring", "__toString"),
        ("__serialize", "__serialize"),
        ("__unserialize", "__unserialize"),
        ("__sleep", "__sleep"),
        ("__wakeup", "__wakeup"),
        ("__set_state", "__set_state"),
    ] {
        if ce.methods.contains_key(lc.as_bytes()) {
            return Err(fatal(vm, format!("Enum {cname} cannot include magic method {shown}")));
        }
    }
    if ce.interfaces.iter().any(|i| i.lc_name.as_bytes() == b"serializable") {
        return Err(fatal(vm, format!("Enum {cname} cannot implement the Serializable interface")));
    }
    Ok(())
}

/// Re-checks delayed variance obligations after loading the classes they
/// were waiting for (`load_delayed_classes` + `resolve_delayed_variance_obligations`).
fn resolve_obligations(vm: &mut Vm, def: &ClassDef, rc: &ClassRef, ctx: Option<variance::Linking>) -> PResult<()> {
    let Some(ctx) = ctx else { return Ok(()) };
    if ctx.obligations.is_empty() {
        return Ok(());
    }
    for name in &ctx.unresolved {
        if vm.find_class(name).is_none() {
            match autoload(vm, name) {
                Ok(_) => {}
                Err(Flow::Exception(e)) => {
                    let _ = e;
                    let msg = format!(
                        "During inheritance of {}, while autoloading {}",
                        display_class_name(&def.name),
                        name
                    );
                    return Err(vm.fatal_error(msg));
                }
                Err(f) => return Err(f),
            }
        }
    }
    // The class is registered now: checks see it as a normal class.
    variance::push(variance::Linking {
        name: PhpStr::new(),
        parent: rc.parent.clone(),
        interfaces: Vec::new(),
        flags: rc.flags,
        unresolved: Vec::new(),
        obligations: Vec::new(),
    });
    let r = (|| -> PResult<()> {
        for o in &ctx.obligations {
            match o {
                variance::Obligation::Method { child, child_scope, parent, parent_scope } => {
                    let mut ck = variance::Checker::new(vm);
                    let st = ck.implementation_check(child, child_scope, parent, parent_scope);
                    ck.finish();
                    if st != Inh::Success {
                        emit_incompatible_method_error(vm, child, child_scope, parent, parent_scope, st)?;
                    }
                }
                variance::Obligation::Property { class, prop, child_ty, parent_ty, parent_class } => {
                    let mut ck = variance::Checker::new(vm);
                    let a = ck.covariant(class, child_ty, parent_class, parent_ty);
                    let b = ck.covariant(parent_class, parent_ty, class, child_ty);
                    let ty = ck.type_string(parent_ty, parent_class);
                    ck.finish();
                    if a != Inh::Success || b != Inh::Success {
                        let prop = if prop.is_empty() {
                            rc.properties
                                .values()
                                .find(|p| {
                                    p.declaring_class.eq_ignore_ascii_case(class)
                                        && p.ty.as_ref() == Some(child_ty)
                                        && rc.parent.as_ref().is_some_and(|pc| {
                                            pc.properties.get(&p.name).is_some_and(|pp| pp.ty.as_ref() == Some(parent_ty))
                                        })
                                })
                                .map(|p| p.name.clone())
                                .unwrap_or_default()
                        } else {
                            prop.clone()
                        };
                        return Err(fatal(vm, format!(
                            "Type of {}::${} must be {} (as in class {})",
                            display_class_name(class),
                            prop,
                            ty,
                            display_class_name(parent_class)
                        )));
                    }
                }
                variance::Obligation::Constant { child_class, child_ty, parent_class, parent_ty, name } => {
                    let mut ck = variance::Checker::new(vm);
                    let st = match child_ty {
                        Some(ct) => ck.covariant(child_class, ct, parent_class, parent_ty),
                        None => Inh::Error,
                    };
                    let ty = ck.type_string(parent_ty, parent_class);
                    ck.finish();
                    if st != Inh::Success {
                        return Err(fatal(vm, format!(
                            "Type of {}::{} must be compatible with {}::{} of type {}",
                            display_class_name(child_class),
                            name,
                            display_class_name(parent_class),
                            name,
                            ty
                        )));
                    }
                }
            }
        }
        Ok(())
    })();
    variance::pop();
    r
}

/// `zend_inheritance_check_override`.
fn check_override(vm: &mut Vm, def: &ClassDef, rc: &ClassRef, st: &LinkState) -> PResult<()> {
    if def.kind == ClassKindIr::Trait {
        return Ok(());
    }
    let cname = display_class_name(&def.name);
    for f in rc.methods.values() {
        if !f.has_flag(member_flags::OVERRIDE) {
            continue;
        }
        if !st.declared.iter().any(|d| Rc::ptr_eq(d, f)) {
            continue;
        }
        let lc = f.name.to_ascii_lowercase();
        if st.override_ok.iter().any(|x| *x == lc) {
            continue;
        }
        let (ffile, fline) = f.location.as_ref().map(|l| (l.0.clone(), l.1)).unwrap_or((def.file.clone(), def.start_line));
        return Err(fatal_at(vm, &ffile, fline, format!(
            "{}::{}() has #[\\Override] attribute, but no matching parent method exists",
            display_class_name(f.scope.as_deref().unwrap_or(b"")),
            f.name
        )));
    }
    for c in rc.constants.values() {
        if c.flags & member_flags::OVERRIDE != 0
            && c.declaring_class.eq_ignore_ascii_case(&def.name)
            && !st.const_override_ok.iter().any(|x| *x == c.name)
        {
            return Err(fatal(vm, format!(
                "{cname}::{} has #[\\Override] attribute, but no matching parent constant exists",
                c.name
            )));
        }
    }
    for p in rc.properties.values() {
        // Own properties flagged OVERRIDE are checked by
        // `check_property_overrides`; this covers promoted and trait properties.
        if !p.declaring_class.eq_ignore_ascii_case(&def.name)
            || def.props.iter().any(|d| d.name == p.name && d.flags & member_flags::OVERRIDE != 0)
        {
            continue;
        }
        let has = p
            .attributes
            .iter()
            .any(|a| a.downcast_ref::<AttrDef>().is_some_and(|a| a.name.eq_ignore_ascii_case(b"Override")));
        if !has {
            continue;
        }
        let in_parent = rc.parent.as_ref().is_some_and(|pc| {
            pc.properties.get(&p.name).is_some_and(|pp| pp.flags & member_flags::PRIVATE == 0)
        });
        let in_iface = rc.interfaces.iter().any(|i| i.properties.contains_key(&p.name));
        if !in_parent && !in_iface {
            return Err(fatal(vm, format!(
                "{cname}::${} has #[\\Override] attribute, but no matching parent property exists",
                p.name
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Early binding
// ---------------------------------------------------------------------------

/// `zend_can_early_bind`: whether inheritance checks against `parent` can be
/// decided now (`Unresolved` when a class they need is not declared yet).
fn can_early_bind(vm: &Vm, def: &ClassDef, parent: &ClassRef) -> Inh {
    variance::push(variance::Linking {
        name: def.name.clone(),
        parent: Some(parent.clone()),
        interfaces: Vec::new(),
        flags: def.flags,
        unresolved: Vec::new(),
        obligations: Vec::new(),
    });
    let mut overall = Inh::Success;
    let r = (|| {
        for m in &def.methods {
            let lc = m.name.to_ascii_lowercase();
            let Some(pm) = parent.methods.get(&lc) else { continue };
            let child = make_function(&m.code, m.name.clone(), m.flags, Some(def.name.clone()));
            let pflags = pm.flags;
            if pflags & (member_flags::PRIVATE | member_flags::ABSTRACT) == member_flags::PRIVATE && !is_ctor(pm) {
                continue;
            }
            if pflags & member_flags::FINAL != 0
                || (pflags & member_flags::STATIC) != (m.flags & member_flags::STATIC)
                || (m.flags & member_flags::ABSTRACT != 0 && pflags & member_flags::ABSTRACT == 0)
            {
                return Inh::Error;
            }
            let proto = prototype_of(Some(parent), &lc).unwrap_or_else(|| pm.clone());
            let target = if is_ctor(pm) {
                if !proto.has_flag(member_flags::ABSTRACT) {
                    continue;
                }
                proto
            } else {
                pm.clone()
            };
            if vis_rank(m.flags) > vis_rank(pflags) {
                return Inh::Error;
            }
            let tscope = target.scope.clone().unwrap_or_default();
            let mut ck = variance::Checker::new(vm);
            let st = ck.implementation_check(&child, &def.name, &target, &tscope);
            match st {
                Inh::Success => {}
                Inh::Warning => overall = Inh::Warning,
                other => return other,
            }
        }
        for p in &def.props {
            let Some(pp) = parent.properties.get(&p.name) else { continue };
            if pp.flags & member_flags::PRIVATE != 0 {
                continue;
            }
            let (Some(pt), Some(ct)) = (&pp.ty, &p.ty) else { continue };
            if *pt == ct.decl {
                continue;
            }
            let mut ck = variance::Checker::new(vm);
            let a = ck.covariant(&def.name, &ct.decl, &pp.declaring_class, pt);
            let b = ck.covariant(&pp.declaring_class, pt, &def.name, &ct.decl);
            if a == Inh::Error || b == Inh::Error {
                return Inh::Error;
            }
            if a == Inh::Unresolved || b == Inh::Unresolved {
                return Inh::Unresolved;
            }
        }
        for c in &def.consts {
            let Some(pc) = parent.constants.get(&c.name) else { continue };
            if pc.flags & member_flags::PRIVATE != 0 {
                continue;
            }
            let (Some(pt), Some(ct)) = (&pc.ty, &c.ty) else { continue };
            let mut ck = variance::Checker::new(vm);
            let st = ck.covariant(&def.name, &ct.decl, &pc.declaring_class, pt);
            if st != Inh::Success {
                return st;
            }
        }
        overall
    })();
    variance::pop();
    r
}

// ---------------------------------------------------------------------------
// Method inheritance (do_inheritance_check_on_method)
// ---------------------------------------------------------------------------

const INH_CHECK_PROTO: u32 = 1 << 0;
const INH_CHECK_VISIBILITY: u32 = 1 << 1;
const INH_SET_CHILD_CHANGED: u32 = 1 << 2;
const INH_SET_CHILD_PROTO: u32 = 1 << 3;
const INH_RESET_OVERRIDE: u32 = 1 << 4;
/// Flags used by `zend_do_inheritance_ex` for parent methods.
const INH_CHILD_DEFAULT: u32 =
    INH_CHECK_PROTO | INH_CHECK_VISIBILITY | INH_SET_CHILD_CHANGED | INH_SET_CHILD_PROTO | INH_RESET_OVERRIDE;

#[derive(Default)]
struct MethodCheck {
    changed: bool,
    reset_override: bool,
    proto: Option<Rc<Function>>,
    /// Prototype to check the signature against (and its scope).
    check: Option<(Rc<Function>, PhpStr)>,
}

/// The prototype recorded for method `lc` as seen in `class` (`common.prototype`):
/// the nearest class up the chain that recorded one, stopping at the class
/// that declares the method.
fn prototype_of(class: Option<&ClassRef>, lc: &[u8]) -> Option<Rc<Function>> {
    let mut c = class.cloned();
    while let Some(cl) = c {
        if let Some(d) = class_data(&cl) {
            if let Some((_, p)) = d.protos.iter().find(|(n, _)| n.as_bytes() == lc) {
                return Some(p.clone());
            }
        }
        let declared_here = cl
            .methods
            .get(lc)
            .is_none_or(|m| m.scope.as_ref().is_none_or(|s| s.eq_ignore_ascii_case(&cl.name)));
        if declared_here {
            return None;
        }
        c = cl.parent.clone();
    }
    None
}

fn is_ctor(f: &Function) -> bool {
    f.flags & fn_flags::CTOR != 0 || f.name.eq_ignore_ascii_case(b"__construct")
}

fn fn_location(vm: &Vm, f: &Function) -> (PhpStr, u32) {
    match &f.location {
        Some(l) => (l.0.clone(), l.1),
        None => (vm.current_file.clone(), vm.current_line),
    }
}

/// `do_inheritance_check_on_method` without the signature check, which the
/// caller performs with [`perform_delayable_check`] on `check`.
fn inheritance_check_on_method(
    vm: &mut Vm,
    child: &Function,
    child_scope: &[u8],
    parent: &Rc<Function>,
    parent_scope: &[u8],
    parent_class: Option<&ClassRef>,
    flags: u32,
) -> PResult<MethodCheck> {
    let mut r = MethodCheck::default();
    let pflags = parent.flags;
    let parent_is_ctor = is_ctor(parent);
    if pflags & (member_flags::PRIVATE | member_flags::ABSTRACT) == member_flags::PRIVATE && !parent_is_ctor {
        if flags & INH_SET_CHILD_CHANGED != 0 {
            r.changed = true;
        }
        return Ok(r);
    }
    let _ = (child_scope, parent_scope);
    let (file, line) = fn_location(vm, child);
    let pscope_name = display_class_name(parent.scope.as_deref().unwrap_or(b""));
    let cscope_name = display_class_name(child.scope.as_deref().unwrap_or(b""));
    if flags & INH_CHECK_PROTO != 0 && pflags & member_flags::FINAL != 0 {
        return Err(fatal_at(vm, &file, line, format!("Cannot override final method {}::{}()", pscope_name, child.name)));
    }
    let cflags = child.flags;
    if flags & INH_CHECK_PROTO != 0 && (cflags & member_flags::STATIC) != (pflags & member_flags::STATIC) {
        return Err(fatal_at(vm, &file, line, if cflags & member_flags::STATIC != 0 {
            format!("Cannot make non static method {}::{}() static in class {}", pscope_name, child.name, cscope_name)
        } else {
            format!("Cannot make static method {}::{}() non static in class {}", pscope_name, child.name, cscope_name)
        }));
    }
    if flags & INH_CHECK_PROTO != 0 && cflags & member_flags::ABSTRACT != 0 && pflags & member_flags::ABSTRACT == 0 {
        return Err(fatal_at(vm, &file, line, format!(
            "Cannot make non abstract method {}::{}() abstract in class {}",
            pscope_name, child.name, cscope_name
        )));
    }
    if flags & INH_SET_CHILD_CHANGED != 0 && pflags & (member_flags::PRIVATE | member_flags::CHANGED) != 0 {
        r.changed = true;
    }
    let proto = prototype_of(parent_class, &parent.name.to_ascii_lowercase()).unwrap_or_else(|| parent.clone());
    let mut target = parent.clone();
    let mut target_scope = PhpStr::from_bytes(parent_scope);
    if parent_is_ctor {
        if !proto.has_flag(member_flags::ABSTRACT) {
            return Ok(r);
        }
        target_scope = proto.scope.clone().unwrap_or_default();
        target = proto.clone();
    }
    if flags & INH_SET_CHILD_PROTO != 0 {
        r.proto = Some(proto);
    }
    if flags & INH_CHECK_VISIBILITY != 0 && vis_rank(cflags) > vis_rank(pflags) {
        return Err(fatal_at(vm, &file, line, format!(
            "Access level to {}::{}() must be {} (as in class {}){}",
            cscope_name,
            child.name,
            vis_name(pflags),
            display_class_name(target.scope.as_deref().unwrap_or(b"")),
            if pflags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
        )));
    }
    if flags & INH_CHECK_PROTO != 0 {
        r.check = Some((target, target_scope));
    }
    if flags & INH_RESET_OVERRIDE != 0 {
        r.reset_override = true;
    }
    Ok(r)
}

/// `perform_delayable_implementation_check`.
fn perform_delayable_check(
    vm: &mut Vm,
    child: &Rc<Function>,
    child_scope: &[u8],
    parent: &Rc<Function>,
    parent_scope: &[u8],
) -> PResult<()> {
    let mut ck = variance::Checker::new(vm);
    let status = ck.implementation_check(child, child_scope, parent, parent_scope);
    ck.finish();
    match status {
        Inh::Success => Ok(()),
        Inh::Unresolved => {
            let pushed = variance::with(|l| {
                l.obligations.push(variance::Obligation::Method {
                    child: child.clone(),
                    child_scope: PhpStr::from_bytes(child_scope),
                    parent: parent.clone(),
                    parent_scope: PhpStr::from_bytes(parent_scope),
                })
            });
            if pushed.is_none() {
                return emit_incompatible_method_error(vm, child, child_scope, parent, parent_scope, status);
            }
            Ok(())
        }
        _ => emit_incompatible_method_error(vm, child, child_scope, parent, parent_scope, status),
    }
}

/// `emit_incompatible_method_error`.
fn emit_incompatible_method_error(
    vm: &mut Vm,
    child: &Function,
    child_scope: &[u8],
    parent: &Function,
    parent_scope: &[u8],
    status: Inh,
) -> PResult<()> {
    let (parent_proto, child_proto) = {
        let ck = variance::Checker::new(vm);
        (function_declaration_in(&ck, parent, parent_scope), function_declaration_in(&ck, child, child_scope))
    };
    let (file, line) = fn_location(vm, child);
    match status {
        Inh::Unresolved => {
            let cls = variance::first_unresolved().map(|n| n.to_string()).unwrap_or_default();
            Err(fatal_at(vm, &file, line, format!(
                "Could not check compatibility between {child_proto} and {parent_proto}, because class {cls} is not available"
            )))
        }
        Inh::Warning => {
            if !fn_has_attr(child, "ReturnTypeWillChange") {
                let msg = format!(
                    "Return type of {child_proto} should either be compatible with {parent_proto}, or the #[\\ReturnTypeWillChange] attribute should be used to temporarily suppress the notice"
                );
                vm.zend_error_at(level::E_DEPRECATED, file, line, PhpStr::from(msg))?;
            }
            Ok(())
        }
        _ => Err(fatal_at(vm, &file, line, format!(
            "Declaration of {child_proto} must be compatible with {parent_proto}"
        ))),
    }
}

/// `zend_get_function_declaration` with types resolved against `scope`.
fn function_declaration_in(ck: &variance::Checker<'_>, f: &Function, scope: &[u8]) -> String {
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
    let user = super::user_fn(f);
    for (i, p) in sig.params.iter().enumerate() {
        if let Some(t) = &p.ty {
            s.push_str(&ck.type_string(t, scope));
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
        s.push_str(&ck.type_string(rt, scope));
    }
    s
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

/// Signature check of a hook against the parent's hook
/// (`do_inheritance_check_on_method` + `perform_delayable_implementation_check`).
fn check_signature(vm: &mut Vm, child: &Rc<Function>, proto: &Rc<Function>) -> PResult<()> {
    let cs = child.scope.clone().unwrap_or_default();
    let ps = proto.scope.clone().unwrap_or_default();
    perform_delayable_check(vm, child, &cs, proto, &ps)
}

/// Whether `a` (in `a_scope`) is a subtype of `b` (in `b_scope`); checks
/// that cannot be decided yet (undeclared classes) pass.
fn is_subtype(vm: &Vm, a: &TypeDecl, a_scope: &[u8], b: &TypeDecl, b_scope: &[u8]) -> bool {
    let mut ck = variance::Checker::new(vm);
    let st = ck.covariant(a_scope, a, b_scope, b);
    st != Inh::Error
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

/// Property type invariance used by the property inheritance checks.
/// Unresolvable types are recorded as delayed obligations (checked once the
/// class is registered) and treated as compatible here.
fn types_equivalent(vm: &Vm, a: &TypeDecl, b: &TypeDecl, a_scope: &[u8], b_scope: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let mut ck = variance::Checker::new(vm);
    let s1 = ck.covariant(b_scope, b, a_scope, a);
    let s2 = ck.covariant(a_scope, a, b_scope, b);
    ck.finish();
    if s1 == Inh::Error || s2 == Inh::Error {
        return false;
    }
    if s1 == Inh::Unresolved || s2 == Inh::Unresolved {
        let recorded = variance::with(|l| {
            l.obligations.push(variance::Obligation::Property {
                class: PhpStr::from_bytes(b_scope),
                prop: PhpStr::new(),
                child_ty: b.clone(),
                parent_ty: a.clone(),
                parent_class: PhpStr::from_bytes(a_scope),
            })
        });
        return recorded.is_some();
    }
    true
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Resolved `insteadof`/`as` rules (`zend_traits_init_trait_structures`).
struct TraitRules {
    /// Per trait (index into `traits`): excluded lowercase method names.
    excludes: Vec<Vec<PhpStr>>,
    /// Alias rules: (trait index, method lc, modifiers, alias).
    aliases: Vec<(usize, PhpStr, u32, Option<PhpStr>)>,
}

fn find_linked_trait(vm: &mut Vm, name: &PhpStr) -> PResult<ClassRef> {
    match vm.find_class(name) {
        Some(c) => Ok(c),
        None => Err(fatal(vm, format!("Could not find trait {}", name.to_string_lossy().trim_start_matches('\\')))),
    }
}

/// `zend_check_trait_usage`.
fn check_trait_usage(vm: &mut Vm, def: &ClassDef, trait_: &ClassRef, traits: &[ClassRef]) -> PResult<usize> {
    if !trait_.is_trait() {
        return Err(fatal(vm, format!(
            "Class {} is not a trait, Only traits may be used in 'as' and 'insteadof' statements",
            trait_.name
        )));
    }
    match traits.iter().position(|t| Rc::ptr_eq(t, trait_)) {
        Some(i) => Ok(i),
        None => Err(fatal(vm, format!(
            "Required Trait {} wasn't added to {}",
            trait_.name,
            display_class_name(&def.name)
        ))),
    }
}

fn init_trait_rules(vm: &mut Vm, def: &ClassDef, traits: &[ClassRef]) -> PResult<TraitRules> {
    let mut rules = TraitRules {
        excludes: vec![Vec::new(); traits.len()],
        aliases: Vec::new(),
    };
    // Precedences first.
    for tu in &def.traits {
        for r in &tu.rules {
            let TraitRule::Precedence { trait_name, method, insteadof, .. } = r else { continue };
            let t = find_linked_trait(vm, trait_name)?;
            check_trait_usage(vm, def, &t, traits)?;
            let lc = method.to_ascii_lowercase();
            if !t.methods.contains_key(&lc) {
                return Err(fatal(vm, format!(
                    "A precedence rule was defined for {}::{} but this method does not exist",
                    t.name, method
                )));
            }
            for io in insteadof {
                let x = find_linked_trait(vm, io)?;
                let idx = check_trait_usage(vm, def, &x, traits)?;
                if rules.excludes[idx].contains(&lc) {
                    return Err(fatal(vm, format!(
                        "Failed to evaluate a trait precedence ({}). Method of trait {} was defined to be excluded multiple times",
                        method, x.name
                    )));
                }
                rules.excludes[idx].push(lc.clone());
                if Rc::ptr_eq(&x, &t) {
                    return Err(fatal(vm, format!(
                        "Inconsistent insteadof definition. The method {} is to be used from {}, but {} is also on the exclude list",
                        method, t.name, t.name
                    )));
                }
            }
        }
    }
    for tu in &def.traits {
        for r in &tu.rules {
            let TraitRule::Alias { trait_name, method, flags, alias, .. } = r else { continue };
            let lc = method.to_ascii_lowercase();
            let idx = match trait_name {
                Some(tn) => {
                    let t = find_linked_trait(vm, tn)?;
                    let idx = check_trait_usage(vm, def, &t, traits)?;
                    if !t.methods.contains_key(&lc) {
                        return Err(fatal(vm, format!(
                            "An alias was defined for {}::{} but this method does not exist",
                            t.name, method
                        )));
                    }
                    idx
                }
                None => {
                    let mut found: Option<usize> = None;
                    for (j, t) in traits.iter().enumerate() {
                        if t.methods.contains_key(&lc) {
                            if let Some(f) = found {
                                return Err(fatal(vm, format!(
                                    "An alias was defined for method {}(), which exists in both {} and {}. Use {}::{} or {}::{} to resolve the ambiguity",
                                    method, traits[f].name, t.name, traits[f].name, method, t.name, method
                                )));
                            }
                            found = Some(j);
                        }
                    }
                    match found {
                        Some(f) => f,
                        None => {
                            return Err(fatal(vm, match alias {
                                Some(a) => format!(
                                    "An alias ({a}) was defined for method {method}(), but this method does not exist"
                                ),
                                None => format!(
                                    "The modifiers of the trait method {method}() are changed, but this method does not exist. Error"
                                ),
                            }));
                        }
                    }
                }
            };
            rules.aliases.push((idx, lc, *flags, alias.clone()));
        }
    }
    Ok(rules)
}

fn apply_modifiers(fn_flags_v: u32, modifiers: u32) -> u32 {
    if modifiers & member_flags::PPP_MASK != 0 {
        modifiers | (fn_flags_v & !member_flags::PPP_MASK)
    } else {
        modifiers | fn_flags_v
    }
}

/// `zend_traits_check_private_final_inheritance`.
fn check_private_final(vm: &mut Vm, orig: u32, new: u32, name: &[u8]) -> PResult<()> {
    if orig & member_flags::FINAL == 0
        && new & (member_flags::PRIVATE | member_flags::FINAL) == (member_flags::PRIVATE | member_flags::FINAL)
        && !name.eq_ignore_ascii_case(b"__construct")
    {
        vm.zend_error(level::E_COMPILE_WARNING, "Private methods cannot be final as they are never overridden by other classes")?;
    }
    Ok(())
}

fn apply_traits(
    vm: &mut Vm,
    ce: &mut ClassEntry,
    def: &ClassDef,
    traits: &[ClassRef],
    data: &mut ClassData,
    st: &mut LinkState,
) -> PResult<()> {
    let rules = init_trait_rules(vm, def, traits)?;
    // Non-abstract methods first, abstract ones after (they are checked
    // against the inherited methods).
    for pass_abstract in [false, true] {
        for (i, t) in traits.iter().enumerate() {
            for (lc, m) in t.methods.iter() {
                if m.has_flag(member_flags::ABSTRACT) != pass_abstract {
                    continue;
                }
                // Aliases with a new name.
                for (ai, am, mods, alias) in &rules.aliases {
                    let Some(alias) = alias else { continue };
                    if *ai != i || am != lc {
                        continue;
                    }
                    let flags = apply_modifiers(m.flags, *mods);
                    check_private_final(vm, m.flags, flags, alias)?;
                    add_trait_method(vm, ce, def, t, m, alias.clone(), flags, data, st)?;
                }
                if rules.excludes[i].contains(lc) {
                    continue;
                }
                let mut flags = m.flags;
                for (ai, am, mods, alias) in &rules.aliases {
                    if alias.is_none() && *mods != 0 && *ai == i && am == lc {
                        flags = apply_modifiers(m.flags, *mods);
                    }
                }
                check_private_final(vm, m.flags, flags, &m.name)?;
                add_trait_method(vm, ce, def, t, m, m.name.clone(), flags, data, st)?;
            }
        }
    }
    // Constants (zend_do_traits_constant_binding).
    for (i, t) in traits.iter().enumerate() {
        for (n, c) in t.constants.iter() {
            // The value is evaluated in the scope of the using class.
            let value = match &*c.value.borrow() {
                ConstValue::Unevaluated(p) => match p.downcast_ref::<ConstPayload>() {
                    Some(p) => ConstValue::Unevaluated(Rc::new(ConstPayload {
                        expr: p.expr.clone(),
                        scope: def.name.clone(),
                        ty: p.ty.clone(),
                        name: p.name.clone(),
                    })),
                    None => ConstValue::Unevaluated(p.clone()),
                },
                ConstValue::Evaluating => ConstValue::Evaluating,
                ConstValue::Value(v) => ConstValue::Value(v.clone()),
            };
            let nc = ClassConstant {
                name: c.name.clone(),
                value: RefCell::new(value),
                flags: c.flags,
                declaring_class: def.name.clone(),
                ty: c.ty.clone(),
                doc_comment: c.doc_comment.clone(),
                attributes: c.attributes.clone(),
            };
            match ce.constants.get(n).cloned() {
                Some(existing) if existing.flags & member_flags::ENUM_CASE != 0 => {
                    return Err(fatal(vm, format!(
                        "Cannot use trait {}, because {}::{} conflicts with enum case {}::{}",
                        t.name,
                        t.name,
                        n,
                        display_class_name(&def.name),
                        n
                    )));
                }
                Some(existing) if !existing.declaring_class.eq_ignore_ascii_case(&def.name) => {
                    // Inherited: the trait constant overrides it like a constant of the class.
                    if inherit_constant_check(vm, def, &existing, &nc)? {
                        st.const_override_ok.push(n.clone());
                    }
                    ce.add_constant(nc);
                }
                Some(existing) => {
                    let mask = member_flags::PPP_MASK | member_flags::FINAL;
                    let mut same = existing.flags & mask == c.flags & mask && existing.ty.is_some() == c.ty.is_some();
                    if same {
                        if let (Some(a), Some(b)) = (&existing.ty, &c.ty) {
                            let mut ck = variance::Checker::new(vm);
                            let s1 = ck.covariant(&def.name, a, &t.name, b);
                            let s2 = ck.covariant(&t.name, b, &def.name, a);
                            if s1 == Inh::Error || s2 == Inh::Error {
                                same = false;
                            }
                        }
                    }
                    if same {
                        same = match (existing.evaluated(), c.evaluated()) {
                            (Some(a), Some(b)) => a.identical(&b),
                            _ => true,
                        };
                    }
                    if !same {
                        let first = traits[..i]
                            .iter()
                            .find(|x| x.constants.contains_key(n))
                            .map(|x| x.name.to_string())
                            .unwrap_or_else(|| display_class_name(&def.name));
                        return Err(fatal(vm, format!(
                            "{} and {} define the same constant ({}) in the composition of {}. However, the definition differs and is considered incompatible. Class was composed",
                            first,
                            t.name,
                            n,
                            display_class_name(&def.name)
                        )));
                    }
                }
                None => ce.add_constant(nc),
            }
        }
    }
    // Properties (zend_do_traits_property_binding).
    for (i, t) in traits.iter().enumerate() {
        let tdata = class_data(t);
        for info in t.properties.values() {
            if let Some(existing) = ce.properties.get(&info.name).cloned() {
                let inherited = !existing.declaring_class.eq_ignore_ascii_case(&def.name);
                if inherited {
                    if existing.flags & member_flags::PRIVATE == 0 {
                        trait_prop_inherit_check(vm, def, &existing, info)?;
                    }
                } else {
                    let first = traits[..i]
                        .iter()
                        .find(|x| x.properties.contains_key(&info.name))
                        .map(|x| x.name.to_string())
                        .unwrap_or_else(|| display_class_name(&def.name));
                    if existing.hooks.is_some() || info.hooks.is_some() {
                        return Err(fatal(vm, format!(
                            "{} and {} define the same hooked property (${}) in the composition of {}. Conflict resolution between hooked properties is currently not supported. Class was composed",
                            first,
                            t.name,
                            info.name,
                            display_class_name(&def.name)
                        )));
                    }
                    let mask = member_flags::PPP_MASK | member_flags::STATIC | member_flags::READONLY;
                    let mut compatible = existing.flags & mask == info.flags & mask
                        && match (&existing.ty, &info.ty) {
                            (None, None) => true,
                            (Some(a), Some(b)) => {
                                a == b || {
                                    let mut ck = variance::Checker::new(vm);
                                    ck.covariant(&def.name, a, &t.name, b) == Inh::Success
                                        && ck.covariant(&t.name, b, &def.name, a) == Inh::Success
                                }
                            }
                            _ => false,
                        };
                    if compatible {
                        let ev = if info.is_static() {
                            t.static_members.borrow().get(info.slot).map(|r| r.get()).unwrap_or_default()
                        } else {
                            info.default.clone()
                        };
                        let ex = if existing.is_static() {
                            ce.static_members.borrow().get(existing.slot).map(|r| r.get()).unwrap_or_default()
                        } else {
                            existing.default.clone()
                        };
                        compatible = ev.identical(&ex);
                    }
                    if !compatible {
                        return Err(fatal(vm, format!(
                            "{} and {} define the same property (${}) in the composition of {}. However, the definition differs and is considered incompatible. Class was composed",
                            first,
                            t.name,
                            info.name,
                            display_class_name(&def.name)
                        )));
                    }
                    continue;
                }
            }
            if def.flags & class_flags::READONLY != 0 && !info.is_readonly() {
                return Err(fatal(vm, format!(
                    "Readonly class {} cannot use trait with a non-readonly property {}::${}",
                    display_class_name(&def.name),
                    t.name,
                    info.name
                )));
            }
            let mut ni = info.clone();
            ni.declaring_class = def.name.clone();
            rebind_trait_hooks(&mut ni, &def.name);
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
                data.const_defaults.retain(|(s, _, _)| *s != slot);
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

/// `do_inherit_property` checks for a trait property redeclaring an
/// inherited one.
fn trait_prop_inherit_check(vm: &mut Vm, def: &ClassDef, parent: &PropertyInfo, child: &PropertyInfo) -> PResult<()> {
    let cname = display_class_name(&def.name);
    let pname = display_class_name(&parent.declaring_class);
    let n = &child.name;
    if parent.flags & member_flags::FINAL != 0 {
        return Err(fatal(vm, format!("Cannot override final property {pname}::${n}")));
    }
    if parent.is_static() != child.is_static() {
        return Err(fatal(vm, format!(
            "Cannot redeclare {}{pname}::${n} as {}{cname}::${n}",
            if parent.is_static() { "static " } else { "non static " },
            if child.is_static() { "static " } else { "non static " }
        )));
    }
    if parent.is_readonly() != child.is_readonly() && parent.flags & member_flags::ABSTRACT == 0 {
        return Err(fatal(vm, format!(
            "Cannot redeclare {} property {pname}::${n} as {} {cname}::${n}",
            if parent.is_readonly() { "readonly" } else { "non-readonly" },
            if child.is_readonly() { "readonly" } else { "non-readonly" }
        )));
    }
    if vis_rank(child.flags) > vis_rank(parent.flags) {
        return Err(fatal(vm, format!(
            "Access level to {cname}::${n} must be {} (as in class {pname}){}",
            vis_name(parent.flags),
            if parent.flags & member_flags::PUBLIC != 0 { "" } else { " or weaker" }
        )));
    }
    match (&parent.ty, &child.ty) {
        (Some(a), _) => {
            let ok = match &child.ty {
                Some(b) => types_equivalent(vm, a, b, &parent.declaring_class, &def.name),
                None => false,
            };
            if !ok {
                let ty = variance::Checker::new(vm).type_string(a, &parent.declaring_class);
                return Err(fatal(vm, format!("Type of {cname}::${n} must be {ty} (as in class {pname})")));
            }
        }
        (None, Some(_)) => {
            return Err(fatal(vm, format!(
                "Type of {cname}::${n} must be omitted to match the parent definition in class {pname}"
            )));
        }
        (None, None) => {}
    }
    Ok(())
}

/// `zend_add_trait_method`.
#[allow(clippy::too_many_arguments)]
fn add_trait_method(
    vm: &mut Vm,
    ce: &mut ClassEntry,
    def: &ClassDef,
    t: &ClassRef,
    m: &Rc<Function>,
    name: PhpStr,
    flags: u32,
    data: &mut ClassData,
    st: &mut LinkState,
) -> PResult<()> {
    let lc = name.to_ascii_lowercase();
    let cname = display_class_name(&def.name);
    let is_abstract = m.has_flag(member_flags::ABSTRACT);
    let existing = ce.methods.get(&lc).cloned();
    let mut inherited: Option<Rc<Function>> = None;
    if let Some(ex) = &existing {
        let ex_own_class = ex.scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&def.name));
        let ex_trait = if ex_own_class { super::user_fn(ex).and_then(|u| u.from_trait.clone()) } else { None };
        // The same trait method imported twice.
        if let (Some(a), Some(b)) = (super::user_fn(ex), super::user_fn(m)) {
            if ex_trait.is_some()
                && Rc::ptr_eq(&a.code, &b.code)
                && ex.flags & member_flags::PPP_MASK == flags & member_flags::PPP_MASK
            {
                return Ok(());
            }
        }
        if is_abstract {
            // The abstract trait method's signature must be satisfied.
            let ex_scope = ex.scope.clone().unwrap_or_default();
            let mut pf = Function::user(
                m.name.clone(),
                match &m.kind {
                    crate::class::FunctionKind::User(p) => p.clone(),
                    crate::class::FunctionKind::Native(_) => return Ok(()),
                },
                m.signature().clone(),
                flags,
            );
            pf.scope = m.scope.clone();
            pf.location = m.location.clone();
            pf.attributes = m.attributes.clone();
            let pf = Rc::new(pf);
            let r = inheritance_check_on_method(vm, ex, &ex_scope, &pf, &def.name, None, INH_CHECK_PROTO | INH_RESET_OVERRIDE)?;
            if r.reset_override && ex_own_class {
                st.override_ok.push(lc.clone());
            }
            if let Some((target, tscope)) = r.check {
                let tscope = if Rc::ptr_eq(&target, &pf) { def.name.clone() } else { tscope };
                perform_delayable_check(vm, ex, &ex_scope, &target, &tscope)?;
            }
            return Ok(());
        }
        if ex_own_class && ex_trait.is_none() {
            // Members of the class itself win over trait methods.
            return Ok(());
        }
        if let Some(other) = ex_trait {
            if !ex.has_flag(member_flags::ABSTRACT) {
                return Err(fatal(vm, format!(
                    "Trait method {}::{} has not been applied as {}::{}, because of collision with {}::{}",
                    t.name, m.name, cname, name, other, ex.name
                )));
            }
        }
        if !ex_own_class {
            inherited = Some(ex.clone());
        }
    }
    let Some(src) = super::user_fn(m) else {
        return Ok(());
    };
    let mut u = src.fork();
    u.from_trait = Some(t.name.clone());
    let mut f = make_function_from(u, name.clone(), flags, Some(def.name.clone()));
    f.flags = flags & !member_flags::CHANGED;
    if lc.as_bytes() == b"__construct" {
        f.flags |= fn_flags::CTOR;
    } else {
        f.flags &= !fn_flags::CTOR;
    }
    let mut pending = None;
    if let Some(pm) = &inherited {
        // The trait method overrides an inherited method.
        let pscope = pm.scope.clone().unwrap_or_default();
        let r = inheritance_check_on_method(vm, &f, &def.name, pm, &pscope, ce.parent.as_ref(), INH_CHILD_DEFAULT)?;
        if r.changed {
            f.flags |= member_flags::CHANGED;
        }
        if r.reset_override {
            st.override_ok.push(lc.clone());
        }
        if let Some(p) = r.proto {
            data.protos.push((lc.clone(), p));
        }
        pending = r.check;
    }
    let rc = ce.add_method(f);
    if let Some((target, tscope)) = pending {
        perform_delayable_check(vm, &rc, &def.name, &target, &tscope)?;
    }
    st.declared.push(rc);
    Ok(())
}
