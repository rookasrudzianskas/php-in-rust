//! Lowering of declarations: functions, closures, parameters, types,
//! classes (members, traits, enums), attributes and constant expressions.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use php_syntax::ast;

use super::ir::*;
use super::*;
use crate::class::{EnumBacking, class_flags, member_flags};
use crate::errors::level;
use crate::types::{ClassType, TypeDecl, mask};
use crate::value::Value;

pub(crate) fn type_is_void(t: &ast::TypeHint) -> bool {
    matches!(t, ast::TypeHint::Builtin { ty: ast::BuiltinType::Void, .. })
}

pub(crate) fn type_is_never(t: &ast::TypeHint) -> bool {
    matches!(t, ast::TypeHint::Builtin { ty: ast::BuiltinType::Never, .. })
}

fn modifiers_to_flags(m: ast::Modifiers) -> u32 {
    let mut f = 0;
    if m.contains(ast::Modifiers::PUBLIC) {
        f |= member_flags::PUBLIC;
    }
    if m.contains(ast::Modifiers::PROTECTED) {
        f |= member_flags::PROTECTED;
    }
    if m.contains(ast::Modifiers::PRIVATE) {
        f |= member_flags::PRIVATE;
    }
    if m.contains(ast::Modifiers::STATIC) {
        f |= member_flags::STATIC;
    }
    if m.contains(ast::Modifiers::ABSTRACT) {
        f |= member_flags::ABSTRACT;
    }
    if m.contains(ast::Modifiers::FINAL) {
        f |= member_flags::FINAL;
    }
    if m.contains(ast::Modifiers::READONLY) {
        f |= member_flags::READONLY;
    }
    if m.contains(ast::Modifiers::PUBLIC_SET) {
        f |= member_flags::PUBLIC_SET;
    }
    if m.contains(ast::Modifiers::PROTECTED_SET) {
        f |= member_flags::PROTECTED_SET;
    }
    if m.contains(ast::Modifiers::PRIVATE_SET) {
        f |= member_flags::PRIVATE_SET;
    }
    f
}

/// Implicit-binding scan for arrow functions (`find_implicit_binds`).
fn find_implicit_binds(params: &[ast::Param], body: &ast::Expr, out: &mut Vec<Vec<u8>>) {
    let mut names = Vec::new();
    scan_expr(body, &mut names);
    for n in names {
        if params.iter().any(|p| p.name.as_bytes() == n.as_slice()) {
            continue;
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
}

fn add_name(out: &mut Vec<Vec<u8>>, n: &[u8]) {
    if n == b"this" || is_auto_global(n) {
        return;
    }
    if !out.iter().any(|x| x.as_slice() == n) {
        out.push(n.to_vec());
    }
}

fn scan_stmts(stmts: &[ast::Stmt], out: &mut Vec<Vec<u8>>) {
    for s in stmts {
        scan_stmt(s, out);
    }
}

fn scan_stmt(s: &ast::Stmt, out: &mut Vec<Vec<u8>>) {
    use ast::StmtKind as S;
    match &s.kind {
        S::Expr(e) => scan_expr(e, out),
        S::Echo(es) | S::Unset(es) | S::Global(es) => es.iter().for_each(|e| scan_expr(e, out)),
        S::If(i) => {
            for b in &i.branches {
                scan_expr(&b.cond, out);
                scan_stmts(&b.body, out);
            }
            if let Some(b) = &i.else_branch {
                scan_stmts(b, out);
            }
        }
        S::While { cond, body } | S::DoWhile { body, cond } => {
            scan_expr(cond, out);
            scan_stmts(body, out);
        }
        S::For(f) => {
            f.init.iter().chain(&f.cond).chain(&f.step).for_each(|e| scan_expr(e, out));
            scan_stmts(&f.body, out);
        }
        S::Foreach(f) => {
            scan_expr(&f.expr, out);
            if let Some(k) = &f.key {
                scan_expr(&k.expr, out);
            }
            scan_expr(&f.value.expr, out);
            scan_stmts(&f.body, out);
        }
        S::Switch { subject, cases } => {
            scan_expr(subject, out);
            for c in cases {
                if let Some(e) = &c.cond {
                    scan_expr(e, out);
                }
                scan_stmts(&c.body, out);
            }
        }
        S::Break(Some(e)) | S::Continue(Some(e)) | S::Return(Some(e)) => scan_expr(e, out),
        S::Static(vs) => {
            for v in vs {
                add_name(out, &v.name);
                if let Some(d) = &v.default {
                    scan_expr(d, out);
                }
            }
        }
        S::Block(b) => scan_stmts(b, out),
        S::Try(t) => {
            scan_stmts(&t.body, out);
            for c in &t.catches {
                if let Some(v) = &c.var {
                    add_name(out, v);
                }
                scan_stmts(&c.body, out);
            }
            if let Some(f) = &t.finally {
                scan_stmts(f, out);
            }
        }
        S::Declare { body: Some(b), .. } => scan_stmts(b, out),
        _ => {}
    }
}

fn scan_args(a: &ast::ArgList, out: &mut Vec<Vec<u8>>) {
    for arg in &a.args {
        match arg {
            ast::Arg::Positional(e) | ast::Arg::Unpack(e) | ast::Arg::Named { value: e, .. } => {
                scan_expr(e, out)
            }
            _ => {}
        }
    }
}

fn scan_class_ref(c: &ast::ClassRef, out: &mut Vec<Vec<u8>>) {
    if let ast::ClassRef::Expr(e) = c {
        scan_expr(e, out);
    }
}

fn scan_expr(e: &ast::Expr, out: &mut Vec<Vec<u8>>) {
    use ast::ExprKind as K;
    match &e.kind {
        K::Var(ast::VarName::Name(n)) => add_name(out, n),
        K::Var(ast::VarName::Expr(e)) => scan_expr(e, out),
        K::Interpolated(parts) | K::ShellExec(parts) => {
            for p in parts {
                if let ast::StrPart::Expr { expr, .. } = p {
                    scan_expr(expr, out);
                }
            }
        }
        K::ClassConst { class, name } => {
            scan_class_ref(class, out);
            if let ast::ClassConstName::Expr(e) = name {
                scan_expr(e, out);
            }
        }
        K::ClassName(c) => scan_class_ref(c, out),
        K::Array { items, .. } => {
            for it in items.iter().flatten() {
                if let Some(k) = &it.key {
                    scan_expr(k, out);
                }
                scan_expr(&it.value, out);
            }
        }
        K::Dim { base, index } => {
            scan_expr(base, out);
            if let Some(i) = index {
                scan_expr(i, out);
            }
        }
        K::Prop { object, name, .. } => {
            scan_expr(object, out);
            if let ast::MemberName::Expr(e) = name {
                scan_expr(e, out);
            }
        }
        K::StaticProp { class, name, .. } => {
            scan_class_ref(class, out);
            if let ast::VarName::Expr(e) = name {
                scan_expr(e, out);
            }
        }
        K::Call { callee, args } => {
            if let ast::Callee::Expr(e) = callee {
                scan_expr(e, out);
            }
            scan_args(args, out);
        }
        K::MethodCall {
            object, name, args, ..
        } => {
            scan_expr(object, out);
            if let ast::MemberName::Expr(e) = name {
                scan_expr(e, out);
            }
            scan_args(args, out);
        }
        K::StaticCall { class, name, args } => {
            scan_class_ref(class, out);
            if let ast::MemberName::Expr(e) = name {
                scan_expr(e, out);
            }
            scan_args(args, out);
        }
        K::New { class, args } => {
            if let ast::NewClass::Expr(e) = class {
                scan_expr(e, out);
            }
            scan_args(args, out);
        }
        K::Assign { target, value }
        | K::AssignRef { target, value }
        | K::AssignOp { target, value, .. }
        | K::AssignCoalesce { target, value } => {
            scan_expr(target, out);
            scan_expr(value, out);
        }
        K::IncDec { target, .. } => scan_expr(target, out),
        K::Binary { left, right, .. } => {
            scan_expr(left, out);
            scan_expr(right, out);
        }
        K::Unary { expr, .. }
        | K::Cast { expr, .. }
        | K::VoidCast(expr)
        | K::Silence(expr)
        | K::Empty(expr)
        | K::Include { expr, .. }
        | K::Eval(expr)
        | K::Print(expr)
        | K::Throw(expr)
        | K::YieldFrom(expr) => scan_expr(expr, out),
        K::Isset(es) => es.iter().for_each(|e| scan_expr(e, out)),
        K::Closure(c) => {
            for u in &c.uses {
                add_name(out, &u.name);
            }
        }
        K::ArrowFn(a) => {
            let mut inner = Vec::new();
            find_implicit_binds(&a.common.params, &a.body, &mut inner);
            for n in inner {
                add_name(out, &n);
            }
        }
        K::Ternary {
            cond, then, else_, ..
        } => {
            scan_expr(cond, out);
            if let Some(t) = then {
                scan_expr(t, out);
            }
            scan_expr(else_, out);
        }
        K::Match { subject, arms } => {
            scan_expr(subject, out);
            for a in arms {
                if let Some(cs) = &a.conds {
                    cs.iter().for_each(|c| scan_expr(c, out));
                }
                scan_expr(&a.body, out);
            }
        }
        K::Instanceof { expr, class } => {
            scan_expr(expr, out);
            scan_class_ref(class, out);
        }
        K::Yield { key, value } => {
            if let Some(k) = key {
                scan_expr(k, out);
            }
            if let Some(v) = value {
                scan_expr(v, out);
            }
        }
        _ => {}
    }
}

impl Lowerer<'_> {
    // ----- types -------------------------------------------------------------------

    pub(crate) fn type_info(&mut self, t: &ast::TypeHint) -> CResult<TypeInfo> {
        let decl = self.type_decl(t)?;
        let text = PhpStr::from(decl.to_type_string());
        Ok(TypeInfo { decl, text })
    }

    fn scope_known(&self) -> bool {
        match &self.class {
            Some(c) => c.kind != ClassKindIr::Trait && !self.in_closure(),
            None => false,
        }
    }

    fn single_type(&mut self, t: &ast::TypeHint, out: &mut TypeDecl) -> CResult<()> {
        match t {
            ast::TypeHint::Builtin { ty, .. } => {
                use ast::BuiltinType as B;
                let bits = match ty {
                    B::Null => mask::NULL,
                    B::True => mask::TRUE,
                    B::False => mask::FALSE,
                    B::Int => mask::INT,
                    B::Float => mask::FLOAT,
                    B::String => mask::STRING,
                    B::Bool => mask::BOOL,
                    B::Void => mask::VOID,
                    B::Never => mask::NEVER,
                    B::Iterable => {
                        out.classes.push(ClassType::Name(PhpStr::from("Traversable")));
                        mask::ARRAY
                    }
                    B::Object => mask::OBJECT,
                    B::Mixed => mask::ANY,
                };
                let overlap = out.mask & bits;
                if overlap != 0 && *ty != B::Iterable {
                    let s = TypeDecl::from_mask(overlap).to_type_string();
                    return self.error(format!("Duplicate type {s} is redundant"));
                }
                if (out.mask & mask::TRUE != 0 && bits == mask::FALSE)
                    || (out.mask & mask::FALSE != 0 && bits == mask::TRUE)
                {
                    return self.error("Type contains both true and false, bool must be used instead");
                }
                out.mask |= bits;
            }
            ast::TypeHint::Array => {
                if out.mask & mask::ARRAY != 0 {
                    return self.error("Duplicate type array is redundant");
                }
                out.mask |= mask::ARRAY
            }
            ast::TypeHint::Callable => out.mask |= mask::CALLABLE,
            ast::TypeHint::Static => {
                if self.class.is_none() && !self.in_closure() && self.fs.kind != FuncKind::Main {
                    return self.error("Cannot use \"static\" when no class scope is active");
                }
                out.mask |= mask::STATIC
            }
            ast::TypeHint::Class(n) => {
                let name = match n.special_class() {
                    Some(ast::SpecialClass::Self_) => {
                        if self.class.is_none() && !self.in_closure() {
                            return self.error("Cannot use \"self\" when no class scope is active");
                        }
                        match &self.class {
                            Some(c) if self.scope_known() && !c.is_anon => c.name.clone(),
                            _ => PhpStr::from("self"),
                        }
                    }
                    Some(ast::SpecialClass::Parent) => {
                        if self.class.is_none() && !self.in_closure() {
                            return self.error("Cannot use \"parent\" when no class scope is active");
                        }
                        match &self.class {
                            Some(c) if self.scope_known() && !c.is_anon => match &c.parent {
                                Some(p) => p.clone(),
                                None => {
                                    return self.error(
                                        "Cannot use \"parent\" when current class scope has no parent",
                                    );
                                }
                            },
                            _ => PhpStr::from("parent"),
                        }
                    }
                    _ => pstr(&n.resolved),
                };
                if out
                    .classes
                    .iter()
                    .any(|c| matches!(c, ClassType::Name(x) if x.eq_ignore_ascii_case(&name)))
                {
                    return self.error(format!("Duplicate type {name} is redundant"));
                }
                out.classes.push(ClassType::Name(name));
            }
            ast::TypeHint::Nullable(inner) => {
                self.single_type(inner, out)?;
                out.mask |= mask::NULL;
            }
            ast::TypeHint::Union(_) | ast::TypeHint::Intersection(_) => {
                let d = self.type_decl(t)?;
                out.mask |= d.mask;
                out.classes.extend(d.classes);
            }
        }
        Ok(())
    }

    pub(crate) fn type_decl(&mut self, t: &ast::TypeHint) -> CResult<TypeDecl> {
        let mut d = TypeDecl::default();
        match t {
            ast::TypeHint::Nullable(inner) => {
                if let ast::TypeHint::Builtin { ty, .. } = &**inner {
                    match ty {
                        ast::BuiltinType::Null => return self.error("null cannot be marked as nullable"),
                        ast::BuiltinType::Mixed => {
                            return self.error(
                                "Type mixed cannot be marked as nullable since mixed already includes null",
                            );
                        }
                        ast::BuiltinType::Void => return self.error("Void can only be used as a standalone type"),
                        ast::BuiltinType::Never => {
                            return self.error("never can only be used as a standalone type");
                        }
                        _ => {}
                    }
                }
                self.single_type(inner, &mut d)?;
                d.mask |= mask::NULL;
            }
            ast::TypeHint::Union(types) => {
                for ty in types {
                    match ty {
                        ast::TypeHint::Intersection(parts) => {
                            let mut names = Vec::new();
                            for p in parts {
                                let mut x = TypeDecl::default();
                                self.single_type(p, &mut x)?;
                                for c in x.classes {
                                    if let ClassType::Name(n) = c {
                                        names.push(n);
                                    }
                                }
                            }
                            d.classes.push(ClassType::Intersection(names));
                        }
                        ast::TypeHint::Builtin {
                            ty: ast::BuiltinType::Mixed,
                            ..
                        } => return self.error("Type mixed can only be used as a standalone type"),
                        ast::TypeHint::Builtin {
                            ty: ast::BuiltinType::Void,
                            ..
                        } => return self.error("Void can only be used as a standalone type"),
                        ast::TypeHint::Builtin {
                            ty: ast::BuiltinType::Never,
                            ..
                        } => return self.error("never can only be used as a standalone type"),
                        _ => self.single_type(ty, &mut d)?,
                    }
                }
                if d.mask & mask::OBJECT != 0
                    && (d.classes.iter().any(|c| match c {
                        ClassType::Name(n) => !n.eq_ignore_ascii_case(b"Traversable"),
                        _ => true,
                    }) || d.mask & mask::STATIC != 0)
                {
                    let s = d.to_type_string();
                    return self.error(format!(
                        "Type {s} contains both object and a class type, which is redundant"
                    ));
                }
            }
            ast::TypeHint::Intersection(parts) => {
                let mut names = Vec::new();
                for p in parts {
                    match p {
                        ast::TypeHint::Class(n) if n.special_class().is_none() => {
                            let name = pstr(&n.resolved);
                            if names.iter().any(|x: &PhpStr| x.eq_ignore_ascii_case(&name)) {
                                return self.error(format!("Duplicate type {name} is redundant"));
                            }
                            names.push(name);
                        }
                        ast::TypeHint::Class(_) | ast::TypeHint::Static => {
                            let mut x = TypeDecl::default();
                            self.single_type(p, &mut x)?;
                            let s = x.to_type_string();
                            return self.error(format!("Type {s} cannot be part of an intersection type"));
                        }
                        other => {
                            let mut x = TypeDecl::default();
                            self.single_type(other, &mut x)?;
                            let s = x.to_type_string();
                            return self.error(format!("Type {s} cannot be part of an intersection type"));
                        }
                    }
                }
                d.classes.push(ClassType::Intersection(names));
            }
            other => self.single_type(other, &mut d)?,
        }
        Ok(d)
    }

    // ----- attributes --------------------------------------------------------------

    pub(crate) fn attributes(&mut self, groups: &[ast::AttributeGroup]) -> CResult<Vec<Rc<AttrDef>>> {
        let mut out = Vec::new();
        for g in groups {
            for a in g {
                self.line = a.line;
                let mut args = Vec::new();
                if let Some(al) = &a.args {
                    for arg in &al.args {
                        match arg {
                            ast::Arg::Positional(e) => {
                                self.check_const_expr(e)?;
                                let v = self.const_lower(e)?;
                                args.push((None, v));
                            }
                            ast::Arg::Named { name, value } => {
                                self.check_const_expr(value)?;
                                let v = self.const_lower(value)?;
                                args.push((Some(pstr(name)), v));
                            }
                            ast::Arg::Unpack(_) => {
                                return self.error("Cannot use unpacking in attribute argument list");
                            }
                            _ => return self.error("Cannot create Closure as attribute argument"),
                        }
                    }
                }
                out.push(Rc::new(AttrDef {
                    name: pstr(&a.name.resolved),
                    args,
                    line: a.line,
                }));
            }
        }
        Ok(out)
    }

    fn has_attr(attrs: &[Rc<AttrDef>], name: &str) -> bool {
        attrs.iter().any(|a| a.name.eq_ignore_ascii_case(name.as_bytes()))
    }

    // ----- constant expressions ----------------------------------------------------

    /// `zend_is_allowed_in_const_expr` check.
    pub(crate) fn check_const_expr(&mut self, e: &ast::Expr) -> CResult<()> {
        self.check_const_expr_ex(e, true)
    }

    fn check_const_expr_ex(&mut self, e: &ast::Expr, allow_new: bool) -> CResult<()> {
        use ast::ExprKind as K;
        let ok = match &e.kind {
            K::Int(_) | K::Float(_) | K::String(_) | K::Const(_) | K::MagicConst(_) => true,
            K::Interpolated(parts) => {
                for p in parts {
                    if let ast::StrPart::Expr { expr, .. } = p {
                        self.check_const_expr_ex(expr, allow_new)?;
                    }
                }
                true
            }
            K::ClassConst { class, name } => {
                match class {
                    ast::ClassRef::Name(n) => {
                        if let Some(ast::SpecialClass::Static) = n.special_class() {
                            self.line = e.line;
                            return self.error("\"static::\" is not allowed in compile-time constants");
                        }
                    }
                    ast::ClassRef::Expr(_) => {
                        self.line = e.line;
                        return self.error("Dynamic class names are not allowed in compile-time class constant references");
                    }
                }
                if let ast::ClassConstName::Expr(x) = name {
                    self.check_const_expr_ex(x, allow_new)?;
                }
                true
            }
            K::ClassName(c) => match c {
                ast::ClassRef::Name(n) => {
                    if let Some(ast::SpecialClass::Static) = n.special_class() {
                        self.line = e.line;
                        return self.error("static::class cannot be used for compile-time class name resolution");
                    }
                    true
                }
                ast::ClassRef::Expr(_) => {
                    self.line = e.line;
                    return self.error("(expression)::class cannot be used in constant expressions");
                }
            },
            K::Array { items, .. } => {
                for it in items.iter().flatten() {
                    if it.by_ref {
                        return self.error("Constant expression contains invalid operations");
                    }
                    if let Some(k) = &it.key {
                        self.check_const_expr_ex(k, allow_new)?;
                    }
                    self.check_const_expr_ex(&it.value, allow_new)?;
                }
                true
            }
            K::Dim { base, index } => {
                self.check_const_expr_ex(base, allow_new)?;
                match index {
                    Some(i) => self.check_const_expr_ex(i, allow_new)?,
                    None => return self.error("Cannot use [] for reading"),
                }
                true
            }
            K::Prop { object, name, .. } => {
                self.check_const_expr_ex(object, allow_new)?;
                if let ast::MemberName::Expr(x) = name {
                    self.check_const_expr_ex(x, allow_new)?;
                }
                true
            }
            K::Binary { left, right, op } => {
                if *op == ast::BinaryOp::Pipe {
                    false
                } else {
                    self.check_const_expr_ex(left, allow_new)?;
                    self.check_const_expr_ex(right, allow_new)?;
                    true
                }
            }
            K::Unary { expr, .. } => {
                self.check_const_expr_ex(expr, allow_new)?;
                true
            }
            K::Cast { kind, expr } => {
                if *kind == ast::CastKind::Object {
                    self.line = e.line;
                    return self.error("Object casts are not supported in this context");
                }
                self.check_const_expr_ex(expr, allow_new)?;
                true
            }
            K::Ternary {
                cond, then, else_, ..
            } => {
                self.check_const_expr_ex(cond, allow_new)?;
                if let Some(t) = then {
                    self.check_const_expr_ex(t, allow_new)?;
                }
                self.check_const_expr_ex(else_, allow_new)?;
                true
            }
            K::New { class, args } => {
                if !allow_new {
                    self.line = e.line;
                    return self.error("New expressions are not supported in this context");
                }
                match class {
                    ast::NewClass::Name(_) => {}
                    ast::NewClass::Expr(_) => {
                        self.line = e.line;
                        return self.error("Cannot use dynamic class name in constant expression");
                    }
                    ast::NewClass::Anonymous(_) => {
                        self.line = e.line;
                        return self.error("Cannot use anonymous class in constant expression");
                    }
                }
                for a in &args.args {
                    match a {
                        ast::Arg::Positional(x) | ast::Arg::Named { value: x, .. } => {
                            self.check_const_expr_ex(x, allow_new)?
                        }
                        ast::Arg::Unpack(_) => {
                            self.line = e.line;
                            return self.error("Argument unpacking in constant expressions is not supported");
                        }
                        _ => {}
                    }
                }
                true
            }
            K::Closure(c) => {
                if !c.is_static {
                    self.line = e.line;
                    return self.error("Closures in constant expressions must be static");
                }
                if !c.uses.is_empty() {
                    self.line = e.line;
                    return self.error("Cannot use(...) variables in constant expression");
                }
                true
            }
            K::Call { callee, args } if args.is_callable_convert() => match callee {
                ast::Callee::Name(_) => true,
                ast::Callee::Expr(_) => {
                    self.line = e.line;
                    return self.error("Cannot use dynamic function name in constant expression");
                }
            },
            K::StaticCall { class, name, args } if args.is_callable_convert() => {
                if let ast::ClassRef::Expr(_) = class {
                    self.line = e.line;
                    return self.error("Cannot use dynamic class name in constant expression");
                }
                if let ast::MemberName::Expr(_) = name {
                    self.line = e.line;
                    return self.error("Cannot use dynamic method name in constant expression");
                }
                true
            }
            _ => false,
        };
        if !ok {
            self.line = e.line;
            return self.error("Constant expression contains invalid operations");
        }
        Ok(())
    }

    /// Lowers a (checked) constant expression.
    pub(crate) fn const_lower(&mut self, e: &ast::Expr) -> CResult<Expr> {
        let saved = self.in_const_expr;
        self.in_const_expr = true;
        let r = self.expr(e);
        self.in_const_expr = saved;
        r
    }

    fn const_expr_rc(&mut self, e: &ast::Expr, allow_new: bool) -> CResult<Rc<ConstExpr>> {
        self.check_const_expr_ex(e, allow_new)?;
        let x = self.const_lower(e)?;
        let literal = match &x {
            Expr::Lit(v) => Some(v.clone()),
            _ => None,
        };
        Ok(Rc::new(ConstExpr {
            expr: x,
            literal,
            file: self.file.clone(),
            line: e.line,
        }))
    }

    // ----- functions ---------------------------------------------------------------

    /// Compiles a parameter list into the current (new) function state.
    fn params(&mut self, params: &[ast::Param], func_display: &str, is_ctor: bool, is_abstract: bool) -> CResult<Vec<ParamInfo>> {
        let last_required = params
            .iter()
            .rposition(|p| p.default.is_none() && !p.variadic);
        let mut out = Vec::with_capacity(params.len());
        let mut seen_variadic = false;
        for (i, p) in params.iter().enumerate() {
            self.line = p.line;
            if is_auto_global(&p.name) {
                return self.error(format!("Cannot re-assign auto-global variable {}", p.name));
            }
            if p.name.as_bytes() == b"this" {
                return self.error("Cannot use $this as parameter");
            }
            if self.fs.cvs.lookup(&p.name).is_some() {
                return self.error(format!("Redefinition of parameter ${}", p.name));
            }
            let slot = self.cv(&p.name);
            debug_assert_eq!(slot as usize, i);
            if seen_variadic {
                return self.error("Only the last parameter can be variadic");
            }
            if p.variadic {
                seen_variadic = true;
                if p.default.is_some() {
                    return self.error("Variadic parameter cannot have a default value");
                }
            }
            let attributes = self.attributes(&p.attributes)?;
            let mut default = None;
            let mut default_const = None;
            let mut default_text = None;
            if let Some(d) = &p.default {
                self.check_const_expr_ex(d, true)?;
                let x = self.const_lower(d)?;
                if let Expr::Lit(v) = &x {
                    default_const = Some(v.clone());
                }
                default_text = Some(PhpStr::from_vec(php_syntax::export_expr_bytes(d)));
                default = Some(x);
            }
            let is_promoted = !p.modifiers.is_empty() || p.hooks.is_some();
            let default_is_null = matches!(default_const, Some(Value::Null));
            let mut forced_nullable = false;
            let ty = match &p.ty {
                Some(t) => {
                    let mut info = self.type_info(t)?;
                    if default_is_null && !is_promoted && info.decl.mask & mask::NULL == 0 && !info.decl.is_mixed() {
                        info.decl.mask |= mask::NULL;
                        info.text = PhpStr::from(info.decl.to_type_string());
                        forced_nullable = true;
                        self.diag(
                            level::E_DEPRECATED,
                            format!(
                                "{func_display}(): Implicitly marking parameter ${} as nullable is deprecated, the explicit nullable type must be used instead",
                                p.name
                            ),
                        )?;
                    }
                    if info.decl.mask & mask::VOID != 0 {
                        return self.error("void cannot be used as a parameter type");
                    }
                    if info.decl.mask & mask::NEVER != 0 {
                        return self.error("never cannot be used as a parameter type");
                    }
                    if let Some(v) = &default_const {
                        if !forced_nullable && !is_valid_default(&info.decl, v) {
                            return self.error(format!(
                                "Cannot use {} as default value for parameter ${} of type {}",
                                const_type_name(v),
                                p.name,
                                info.text
                            ));
                        }
                    }
                    Some(info)
                }
                None => None,
            };
            if let Some(lr) = last_required {
                if i < lr && default.is_some() {
                    if !forced_nullable {
                        let req = &params[lr].name;
                        self.diag(
                            level::E_DEPRECATED,
                            format!(
                                "{func_display}(): Optional parameter ${} declared before required parameter ${req} is implicitly treated as a required parameter",
                                p.name
                            ),
                        )?;
                    }
                    default = None;
                    default_const = None;
                }
            }
            let promote = if is_promoted {
                if !is_ctor {
                    return self.error("Cannot declare promoted property outside a constructor");
                }
                if is_abstract || self.class.as_ref().is_some_and(|c| c.kind == ClassKindIr::Interface) {
                    return self.error("Cannot declare promoted property in an abstract constructor");
                }
                if p.variadic {
                    return self.error("Cannot declare variadic promoted property");
                }
                let mut flags = modifiers_to_flags(p.modifiers);
                if flags & member_flags::PPP_MASK == 0 {
                    flags |= member_flags::PUBLIC;
                }
                Some(flags | member_flags::PROMOTED)
            } else {
                None
            };
            out.push(ParamInfo {
                name: pstr(&p.name),
                slot,
                by_ref: p.by_ref,
                variadic: p.variadic,
                ty,
                default,
                default_const,
                default_text,
                promote,
                attributes,
                doc_comment: p.doc_comment.as_ref().map(|d| pstr(d)),
                line: p.line,
            });
        }
        Ok(out)
    }

    fn begin_fn(&mut self, kind: FuncKind, name: PhpStr, common: &ast::FunctionCommon) -> FnState {
        let mut fs = FnState::new(kind, name);
        if self.strict {
            fs.flags |= code_flags::STRICT;
        }
        if common.by_ref {
            fs.flags |= code_flags::RETURNS_REF;
        }
        if common.is_generator {
            fs.flags |= code_flags::GENERATOR;
            fs.is_generator = true;
        }
        fs.return_type = common.return_type.clone();
        std::mem::replace(&mut self.fs, fs)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_fn(
        &mut self,
        saved: FnState,
        name: PhpStr,
        kind: FuncKind,
        common: &ast::FunctionCommon,
        params: Vec<ParamInfo>,
        body: Block,
        return_type: Option<TypeInfo>,
        uses: Vec<UseBinding>,
        attributes: Vec<Rc<AttrDef>>,
    ) -> CResult<Rc<FuncCode>> {
        self.finish_gotos()?;
        let fs = std::mem::replace(&mut self.fs, saved);
        let mut flags = fs.flags;
        if fs.is_generator {
            flags |= code_flags::GENERATOR;
        }
        if Self::has_attr(&attributes, "NoDiscard") {
            flags |= code_flags::NODISCARD;
        }
        if Self::has_attr(&attributes, "Deprecated") {
            flags |= code_flags::DEPRECATED;
        }
        Ok(Rc::new(FuncCode {
            name,
            kind,
            flags,
            cvs: Rc::new(fs.cvs),
            params,
            body,
            return_type,
            file: self.file.clone(),
            start_line: common.start_line,
            end_line: common.end_line,
            uses,
            num_statics: fs.num_statics,
            class_name: self.class.as_ref().map(|c| c.name.clone()),
            in_trait: self.class.as_ref().is_some_and(|c| c.kind == ClassKindIr::Trait),
            doc_comment: common.doc_comment.as_ref().map(|d| pstr(d)),
            attributes,
            labels: fs.labels,
            namespace: self.namespace.clone(),
        }))
    }

    fn return_type_info(&mut self, common: &ast::FunctionCommon, display: &str, is_tostring: bool) -> CResult<Option<TypeInfo>> {
        match &common.return_type {
            Some(t) => {
                let info = self.type_info(t)?;
                if info.decl.mask & mask::VOID != 0 && common.by_ref {
                    self.diag(
                        level::E_DEPRECATED,
                        format!("{display}(): Returning by reference from a void function is deprecated"),
                    )?;
                }
                if common.is_generator {
                    let ok = info.decl.is_mixed()
                        || info.decl.mask & (mask::OBJECT) != 0
                        || info.decl.classes.iter().any(|c| match c {
                            ClassType::Name(n) => {
                                let l = n.to_ascii_lowercase();
                                matches!(l.as_bytes(), b"generator" | b"iterator" | b"traversable")
                            }
                            _ => false,
                        });
                    if !ok || info.decl.mask & !(mask::OBJECT | mask::ARRAY | mask::NULL) & mask::ANY != 0 && !info.decl.is_mixed() {
                        if !(info.decl.mask & mask::ARRAY != 0 && info.decl.classes.iter().any(|c| matches!(c, ClassType::Name(n) if n.eq_ignore_ascii_case(b"Traversable")))) || !ok {
                            return self.error(format!(
                                "Generator return type must be a supertype of Generator, {} given",
                                info.text
                            ));
                        }
                    }
                }
                Ok(Some(info))
            }
            None if is_tostring => {
                let decl = TypeDecl::from_mask(mask::STRING);
                Ok(Some(TypeInfo {
                    text: PhpStr::from("string"),
                    decl,
                }))
            }
            None => Ok(None),
        }
    }

    pub(crate) fn function_decl(&mut self, f: &ast::FunctionDecl) -> CResult<Rc<FuncDeclDef>> {
        let name = if f.fq_name.is_empty() {
            pstr(&f.name)
        } else {
            pstr(&f.fq_name)
        };
        let lc_name = name.to_ascii_lowercase();
        if lc_name.as_bytes() == b"__autoload" {
            self.line = f.common.start_line;
            return self.error("__autoload() is no longer supported, use spl_autoload_register() instead");
        }
        let saved_class = self.class.take();
        let saved = self.begin_fn(FuncKind::Function, name.clone(), &f.common);
        let display = name.to_string();
        let res = self.function_body_inner(&f.common, &f.body, &display);
        let (params, rt, body, attributes) = match res {
            Ok(x) => x,
            Err(e) => {
                self.fs = saved;
                self.class = saved_class;
                return Err(e);
            }
        };
        let code = self.finish_fn(saved, name.clone(), FuncKind::Function, &f.common, params, body, rt, Vec::new(), attributes);
        self.class = saved_class;
        let code = code?;
        Ok(Rc::new(FuncDeclDef {
            name,
            lc_name,
            code,
            func: OnceCell::new(),
        }))
    }

    fn function_body_inner(
        &mut self,
        common: &ast::FunctionCommon,
        body: &[ast::Stmt],
        display: &str,
    ) -> CResult<(Vec<ParamInfo>, Option<TypeInfo>, Block, Vec<Rc<AttrDef>>)> {
        let attributes = self.attributes(&common.attributes)?;
        let params = self.params(&common.params, display, false, false)?;
        let rt = self.return_type_info(common, display, false)?;
        if Self::has_attr(&attributes, "NoDiscard") {
            if let Some(t) = &rt {
                if t.decl.mask & mask::VOID != 0 {
                    return self.error("A void function does not return a value, but #[\\NoDiscard] requires a return value");
                }
                if t.decl.mask & mask::NEVER != 0 {
                    return self.error("A never returning function does not return a value, but #[\\NoDiscard] requires a return value");
                }
            }
        }
        let body = self.block(body)?;
        Ok((params, rt, body, attributes))
    }

    /// The `{closure:...}` name (`zend_begin_func_decl`).
    fn closure_name(&self, start_line: u32) -> PhpStr {
        let parent = if self.fs.kind == FuncKind::Main || self.fs.name.is_empty() {
            self.file.to_string()
        } else if self.in_closure() {
            self.fs.name.to_string()
        } else {
            match (&self.class, self.fs.kind) {
                (Some(c), FuncKind::Method) | (Some(c), FuncKind::Hook) => format!("{}::{}()", c.name, self.fs.name),
                _ => format!("{}()", self.fs.name),
            }
        };
        PhpStr::from(format!("{{closure:{parent}:{start_line}}}"))
    }

    pub(crate) fn closure(&mut self, c: &ast::ClosureDecl, line: u32) -> CResult<Expr> {
        let name = self.closure_name(c.common.start_line);
        // Bind the used variables in the defining scope first.
        let mut outer = Vec::with_capacity(c.uses.len());
        let mut seen: HashSet<&[u8]> = HashSet::new();
        for u in &c.uses {
            self.line = u.line;
            if u.name.as_bytes() == b"this" {
                return self.error("Cannot use $this as lexical variable");
            }
            if is_auto_global(&u.name) {
                return self.error("Cannot use auto-global as lexical variable");
            }
            if !seen.insert(u.name.as_bytes()) {
                return self.error(format!("Cannot use variable ${} twice", u.name));
            }
            outer.push(self.cv(&u.name));
        }
        let saved = self.begin_fn(FuncKind::Closure, name.clone(), &c.common);
        if c.is_static {
            self.fs.flags |= code_flags::STATIC_CLOSURE;
        }
        let display = name.to_string();
        let res = (|| -> CResult<_> {
            let attributes = self.attributes(&c.common.attributes)?;
            let params = self.params(&c.common.params, &display, false, false)?;
            let mut uses = Vec::with_capacity(c.uses.len());
            for (u, o) in c.uses.iter().zip(&outer) {
                self.line = u.line;
                if self.fs.cvs.lookup(&u.name).is_some() {
                    return self.error(format!("Cannot use lexical variable ${} as a parameter name", u.name));
                }
                let inner = self.cv(&u.name);
                uses.push(UseBinding {
                    name: pstr(&u.name),
                    outer: *o,
                    inner,
                    by_ref: u.by_ref,
                    line: u.line,
                });
            }
            let rt = self.return_type_info(&c.common, &display, false)?;
            let body = self.block(&c.body)?;
            Ok((params, uses, rt, body, attributes))
        })();
        let (params, uses, rt, body, attributes) = match res {
            Ok(x) => x,
            Err(e) => {
                self.fs = saved;
                return Err(e);
            }
        };
        let code = self.finish_fn(saved, name, FuncKind::Closure, &c.common, params, body, rt, uses, attributes)?;
        Ok(Expr::Closure(
            Rc::new(ClosureExpr {
                code,
                is_static: c.is_static,
                auto_capture: Vec::new(),
            }),
            line,
        ))
    }

    pub(crate) fn arrow_fn(&mut self, a: &ast::ArrowFnDecl, line: u32) -> CResult<Expr> {
        let name = self.closure_name(a.common.start_line);
        let mut names = Vec::new();
        find_implicit_binds(&a.common.params, &a.body, &mut names);
        let outer: Vec<Slot> = names.iter().map(|n| self.cv(n)).collect();
        let saved = self.begin_fn(FuncKind::ArrowFn, name.clone(), &a.common);
        if a.is_static {
            self.fs.flags |= code_flags::STATIC_CLOSURE;
        }
        let display = name.to_string();
        let res = (|| -> CResult<_> {
            let attributes = self.attributes(&a.common.attributes)?;
            let params = self.params(&a.common.params, &display, false, false)?;
            let mut caps = Vec::with_capacity(names.len());
            for (n, o) in names.iter().zip(&outer) {
                let inner = self.cv(n);
                caps.push((*o, inner));
            }
            let rt = self.return_type_info(&a.common, &display, false)?;
            self.line = a.body.line;
            let returns_never = rt.as_ref().is_some_and(|t| t.decl.mask & mask::NEVER != 0);
            let e = if self.fs.flags & code_flags::RETURNS_REF != 0 && is_variable_like(&a.body) {
                self.ref_source(&a.body)?
            } else {
                self.expr(&a.body)?
            };
            let body = if returns_never {
                vec![Stmt {
                    kind: StmtKind::Expr(e),
                    line: a.body.line,
                }]
            } else {
                vec![Stmt {
                    kind: StmtKind::Return(Some(e)),
                    line: a.body.line,
                }]
            };
            Ok((params, caps, rt, body, attributes))
        })();
        let (params, caps, rt, body, attributes) = match res {
            Ok(x) => x,
            Err(e) => {
                self.fs = saved;
                return Err(e);
            }
        };
        let code = self.finish_fn(saved, name, FuncKind::ArrowFn, &a.common, params, body, rt, Vec::new(), attributes)?;
        Ok(Expr::Closure(
            Rc::new(ClosureExpr {
                code,
                is_static: a.is_static,
                auto_capture: caps,
            }),
            line,
        ))
    }

    // ----- classes -------------------------------------------------------------------

    pub(crate) fn class_decl(&mut self, c: &ast::ClassDecl, toplevel: bool) -> CResult<Rc<ClassDef>> {
        let _ = toplevel;
        self.line = c.start_line;
        if self.class.is_some() && c.name.is_some() {
            return self.error("Class declarations may not be nested");
        }
        let kind = match c.kind {
            ast::ClassKind::Class => ClassKindIr::Class,
            ast::ClassKind::Interface => ClassKindIr::Interface,
            ast::ClassKind::Trait => ClassKindIr::Trait,
            ast::ClassKind::Enum => ClassKindIr::Enum,
        };
        let is_anon = c.name.is_none();
        let parent = c.extends.as_ref().map(|n| pstr(&n.resolved));
        let name = if is_anon {
            let prefix = match (&c.extends, c.implements.first()) {
                (Some(p), _) => pstr(&p.resolved),
                (None, Some(i)) => pstr(&i.resolved),
                _ => PhpStr::from("class"),
            };
            let counter = self.vm.next_rtd_key();
            let mut v = prefix.to_vec();
            v.extend_from_slice(b"@anonymous\0");
            v.extend_from_slice(&self.file);
            v.extend_from_slice(format!(":{}${:x}", c.start_line, counter).as_bytes());
            PhpStr::from_vec(v)
        } else if c.fq_name.is_empty() {
            pstr(c.name.as_ref().expect("named"))
        } else {
            pstr(&c.fq_name)
        };
        let lc_name = name.to_ascii_lowercase();
        let mut flags = 0;
        if c.modifiers.is_abstract {
            flags |= class_flags::ABSTRACT;
        }
        if c.modifiers.is_final {
            flags |= class_flags::FINAL;
        }
        if c.modifiers.is_readonly {
            flags |= class_flags::READONLY;
        }
        if is_anon {
            flags |= class_flags::ANON;
        }
        match kind {
            ClassKindIr::Interface => flags |= class_flags::INTERFACE,
            ClassKindIr::Trait => flags |= class_flags::TRAIT,
            ClassKindIr::Enum => flags |= class_flags::ENUM | class_flags::FINAL,
            ClassKindIr::Class => {}
        }
        let mut interfaces: Vec<PhpStr> = c.implements.iter().map(|n| pstr(&n.resolved)).collect();
        let mut enum_backing = None;
        if kind == ClassKindIr::Enum {
            interfaces.insert(0, PhpStr::from("UnitEnum"));
            if let Some(t) = &c.enum_backing_type {
                let b = match t {
                    ast::TypeHint::Builtin { ty: ast::BuiltinType::Int, .. } => EnumBacking::Int,
                    ast::TypeHint::Builtin { ty: ast::BuiltinType::String, .. } => EnumBacking::String,
                    other => {
                        let d = self.type_decl(other)?;
                        return self.error(format!(
                            "Enum backing type must be int or string, {} given",
                            d.to_type_string()
                        ));
                    }
                };
                enum_backing = Some(b);
                interfaces.insert(1, PhpStr::from("BackedEnum"));
            } else {
                enum_backing = Some(EnumBacking::Pure);
            }
        }
        let saved_class = self.class.replace(ClassCtx {
            name: name.clone(),
            kind,
            parent: parent.clone(),
            is_anon,
        });
        let saved_fs = std::mem::replace(&mut self.fs, FnState::new(FuncKind::Main, PhpStr::new()));
        let saved_depth = self.depth;
        let res = self.class_members(c, &name, kind, flags);
        self.depth = saved_depth;
        self.fs = saved_fs;
        self.class = saved_class;
        let (consts, props, methods, traits, cases, attributes, has_tostring) = res?;
        if has_tostring && kind != ClassKindIr::Trait && !interfaces.iter().any(|i| i.eq_ignore_ascii_case(b"Stringable")) {
            interfaces.push(PhpStr::from("Stringable"));
        }
        let mut flags = flags;
        if Self::has_attr(&attributes, "AllowDynamicProperties") {
            flags |= class_flags::ALLOW_DYNAMIC_PROPERTIES;
        }
        Ok(Rc::new(ClassDef {
            name,
            lc_name,
            kind,
            flags,
            parent,
            interfaces,
            traits,
            consts,
            props,
            methods,
            cases,
            enum_backing,
            attributes,
            doc_comment: c.doc_comment.as_ref().map(|d| pstr(d)),
            file: self.file.clone(),
            start_line: c.start_line,
            end_line: c.end_line,
            declared: RefCell::new(None),
            early_bound: Cell::new(false),
        }))
    }

    #[allow(clippy::type_complexity)]
    fn class_members(
        &mut self,
        c: &ast::ClassDecl,
        class_name: &PhpStr,
        kind: ClassKindIr,
        class_flags_v: u32,
    ) -> CResult<(
        Vec<ClassConstDef>,
        Vec<PropDef>,
        Vec<MethodDef>,
        Vec<TraitUseDef>,
        Vec<EnumCaseDef>,
        Vec<Rc<AttrDef>>,
        bool,
    )> {
        let attributes = self.attributes(&c.attributes)?;
        let mut consts: Vec<ClassConstDef> = Vec::new();
        let mut props: Vec<PropDef> = Vec::new();
        let mut methods: Vec<MethodDef> = Vec::new();
        let mut traits = Vec::new();
        let mut cases: Vec<EnumCaseDef> = Vec::new();
        let mut has_tostring = false;
        let is_interface = kind == ClassKindIr::Interface;
        let readonly_class = class_flags_v & class_flags::READONLY != 0;
        for m in &c.members {
            self.line = m.line;
            match &m.kind {
                ast::ClassMemberKind::Const(g) => {
                    let mut flags = modifiers_to_flags(g.modifiers);
                    if flags & member_flags::PPP_MASK == 0 {
                        flags |= member_flags::PUBLIC;
                    }
                    if is_interface && flags & (member_flags::PRIVATE | member_flags::PROTECTED) != 0 {
                        return self.error("Access type for interface constant %s::%s must be public".replace("%s::%s", &format!("{}::{}", class_name, g.consts[0].name)));
                    }
                    if flags & member_flags::PRIVATE != 0 && flags & member_flags::FINAL != 0 {
                        return self.error(format!(
                            "Private constant {}::{} cannot be final as it is not visible to other classes",
                            class_name, g.consts[0].name
                        ));
                    }
                    let ty = match &g.ty {
                        Some(t) => {
                            let info = self.type_info(t)?;
                            if info.decl.mask & (mask::VOID | mask::NEVER | mask::CALLABLE) != 0 {
                                return self.error(format!(
                                    "Class constant {}::{} cannot have type {}",
                                    class_name, g.consts[0].name, info.text
                                ));
                            }
                            Some(info)
                        }
                        None => None,
                    };
                    let attrs = self.attributes(&g.attributes)?;
                    for e in &g.consts {
                        self.line = e.line;
                        if consts.iter().any(|x| x.name.as_bytes() == e.name.as_bytes())
                            || cases.iter().any(|x| x.name.as_bytes() == e.name.as_bytes())
                        {
                            return self.error(format!("Cannot redefine class constant {}::{}", class_name, e.name));
                        }
                        if e.name.eq_ignore_case(b"class") {
                            return self.error("A class constant must not be called 'class'; it is reserved for class name fetching");
                        }
                        let value = self.const_expr_rc(&e.value, false)?;
                        if let (Some(t), Some(v)) = (&ty, &value.literal) {
                            if !is_valid_default(&t.decl, v) || (t.decl.mask & mask::FLOAT == 0 && matches!(v, Value::Int(_)) && t.decl.mask & mask::INT == 0 && !t.decl.is_mixed()) {
                                return self.error(format!(
                                    "Cannot use {} as value for class constant {}::{} of type {}",
                                    const_type_name(v),
                                    class_name,
                                    e.name,
                                    t.text
                                ));
                            }
                        }
                        consts.push(ClassConstDef {
                            name: pstr(&e.name),
                            flags,
                            ty: ty.clone(),
                            value,
                            doc_comment: e.doc_comment.as_ref().map(|d| pstr(d)),
                            attributes: attrs.clone(),
                            line: e.line,
                        });
                    }
                }
                ast::ClassMemberKind::Property(g) => {
                    // zend_compile_prop_decl
                    if kind == ClassKindIr::Enum {
                        return self.error(format!("Enum {class_name} cannot include properties"));
                    }
                    let mut flags = modifiers_to_flags(g.modifiers);
                    if flags & member_flags::FINAL != 0 && flags & member_flags::PRIVATE != 0 {
                        return self.error("Property cannot be both final and private");
                    }
                    if is_interface {
                        if flags & member_flags::FINAL != 0 {
                            return self.error("Property in interface cannot be final");
                        }
                        if flags & (member_flags::PROTECTED | member_flags::PRIVATE) != 0 {
                            return self.error("Property in interface cannot be protected or private");
                        }
                        if flags & member_flags::ABSTRACT != 0 {
                            return self.error(
                                "Property in interface cannot be explicitly abstract. All interface members are implicitly abstract",
                            );
                        }
                        flags |= member_flags::ABSTRACT;
                    }
                    let ty = match &g.ty {
                        Some(t) => Some(self.type_info(t)?),
                        None => None,
                    };
                    let attrs = self.attributes(&g.attributes)?;
                    for p in &g.props {
                        self.line = p.line;
                        let mut flags = flags;
                        if p.hooks.is_none() {
                            if is_interface {
                                return self.error("Interfaces may only include hooked properties");
                            }
                            if flags & member_flags::ABSTRACT != 0 {
                                return self.error("Only hooked properties may be declared abstract");
                            }
                        }
                        if let Some(t) = &ty {
                            if t.decl.mask & mask::CALLABLE != 0 || t.decl.mask & (mask::VOID | mask::NEVER) != 0 {
                                return self.error(format!(
                                    "Property {}::${} cannot have type {}",
                                    class_name, p.name, t.text
                                ));
                            }
                        }
                        if props.iter().any(|x| x.name.as_bytes() == p.name.as_bytes()) {
                            return self.error(format!("Cannot redeclare {}::${}", class_name, p.name));
                        }
                        let default = match &p.default {
                            Some(d) => {
                                let ce = self.const_expr_rc(d, false)?;
                                if let (Some(t), Some(v)) = (&ty, &ce.literal) {
                                    if matches!(v, Value::Null) && t.decl.mask & mask::NULL == 0 && !t.decl.is_mixed() {
                                        return self.error(format!(
                                            "Default value for property of type {} may not be null. Use the nullable type ?{} to allow null default value",
                                            t.text, t.text
                                        ));
                                    }
                                    if !is_valid_default(&t.decl, v) {
                                        return self.error(format!(
                                            "Cannot use {} as default value for property {}::${} of type {}",
                                            const_type_name(v),
                                            class_name,
                                            p.name,
                                            t.text
                                        ));
                                    }
                                }
                                Some(ce)
                            }
                            None => None,
                        };
                        if readonly_class {
                            flags |= member_flags::READONLY;
                        }
                        if flags & member_flags::READONLY != 0 {
                            if ty.is_none() {
                                return self.error(format!("Readonly property {}::${} must have type", class_name, p.name));
                            }
                            if flags & member_flags::STATIC != 0 {
                                return self.error(format!("Static property {}::${} cannot be readonly", class_name, p.name));
                            }
                        }
                        flags = self.declared_property_flags(flags, class_name, &pstr(&p.name), ty.is_some())?;
                        let hooks = match &p.hooks {
                            Some(hs) => {
                                let (h, uses) = self.hooks(hs, &pstr(&p.name), flags, ty.as_ref(), &g.ty)?;
                                if is_interface || !uses {
                                    flags |= member_flags::VIRTUAL;
                                }
                                h
                            }
                            None => {
                                if is_interface {
                                    flags |= member_flags::VIRTUAL;
                                }
                                Vec::new()
                            }
                        };
                        if Self::has_attr(&attrs, "Override") {
                            flags |= member_flags::OVERRIDE;
                        }
                        props.push(PropDef {
                            name: pstr(&p.name),
                            flags,
                            ty: ty.clone(),
                            default,
                            hooks,
                            doc_comment: p.doc_comment.as_ref().map(|d| pstr(d)),
                            attributes: attrs.clone(),
                            line: p.line,
                        });
                    }
                }
                ast::ClassMemberKind::Method(md) => {
                    let lc = md.name.to_ascii_lowercase();
                    if methods.iter().any(|x| x.name.eq_ignore_ascii_case(&md.name)) {
                        return self.error(format!("Cannot redeclare {}::{}()", class_name, md.name));
                    }
                    if lc.as_bytes() == b"__tostring" {
                        has_tostring = true;
                    }
                    let mut flags = modifiers_to_flags(md.modifiers);
                    if flags & member_flags::PPP_MASK == 0 {
                        flags |= member_flags::PUBLIC;
                    }
                    if is_interface {
                        if flags & (member_flags::PRIVATE | member_flags::PROTECTED) != 0 {
                            return self.error(format!(
                                "Access type for interface method {}::{}() must be public",
                                class_name, md.name
                            ));
                        }
                        if flags & member_flags::FINAL != 0 {
                            return self.error(format!(
                                "Interface method {}::{}() must not be final",
                                class_name, md.name
                            ));
                        }
                        if flags & member_flags::ABSTRACT != 0 {
                            return self.error(format!(
                                "Interface method {}::{}() must not be abstract",
                                class_name, md.name
                            ));
                        }
                        flags |= member_flags::ABSTRACT;
                    }
                    if flags & member_flags::ABSTRACT != 0 {
                        if flags & member_flags::PRIVATE != 0 && kind != ClassKindIr::Trait {
                            return self.error(format!(
                                "{} function {}::{}() cannot be declared private",
                                if is_interface { "Interface" } else { "Abstract" },
                                class_name,
                                md.name
                            ));
                        }
                        if md.body.is_some() {
                            return self.error(format!(
                                "{} function {}::{}() cannot contain body",
                                if is_interface { "Interface" } else { "Abstract" },
                                class_name,
                                md.name
                            ));
                        }
                        if kind == ClassKindIr::Enum || (kind == ClassKindIr::Class && class_flags_v & class_flags::ANON != 0) {
                            if kind == ClassKindIr::Enum {
                                return self.error(format!(
                                    "Enum method {}::{}() must not be abstract",
                                    class_name, md.name
                                ));
                            }
                            return self.error(format!(
                                "Anonymous class method {}() must not be abstract",
                                md.name
                            ));
                        }
                        if kind == ClassKindIr::Class && class_flags_v & class_flags::ABSTRACT == 0 {
                            return self.error(format!(
                                "Class {} declares abstract method {}() and must therefore be declared abstract",
                                class_name, md.name
                            ));
                        }
                    } else if md.body.is_none() {
                        return self.error(format!(
                            "Non-abstract method {}::{}() must contain body",
                            class_name, md.name
                        ));
                    }
                    let code = self.method(md, flags, class_name, kind)?;
                    // Promoted constructor parameters declare properties.
                    if lc.as_bytes() == b"__construct" {
                        for p in &code.params {
                            if let Some(pflags) = p.promote {
                                if props.iter().any(|x| x.name == p.name) {
                                    self.line = p.line;
                                    return self.error(format!("Cannot redeclare {}::${}", class_name, p.name));
                                }
                                if let Some(t) = &p.ty {
                                    if t.decl.mask & mask::CALLABLE != 0 {
                                        self.line = p.line;
                                        return self.error(format!(
                                            "Property {}::${} cannot have type {}",
                                            class_name, p.name, t.text
                                        ));
                                    }
                                }
                                let mut pf = pflags;
                                if readonly_class {
                                    pf |= member_flags::READONLY;
                                }
                                if pf & member_flags::READONLY != 0 && p.ty.is_none() {
                                    self.line = p.line;
                                    return self.error(format!(
                                        "Readonly property {}::${} must have type",
                                        class_name, p.name
                                    ));
                                }
                                // A promoted parameter's type without implicit nullability.
                                let ty = p.ty.clone();
                                self.line = p.line;
                                pf = self.declared_property_flags(pf, class_name, &p.name, ty.is_some())?;
                                let ast_param = md.common.params.iter().find(|ap| ap.name.as_bytes() == p.name.as_bytes());
                                let hooks = match ast_param.and_then(|ap| ap.hooks.as_ref().map(|h| (h, ap))) {
                                    Some((hs, ap)) => {
                                        let (h, uses) = self.hooks(hs, &p.name, pf, ty.as_ref(), &ap.ty)?;
                                        if !uses {
                                            pf |= member_flags::VIRTUAL;
                                        }
                                        h
                                    }
                                    None => Vec::new(),
                                };
                                props.push(PropDef {
                                    name: p.name.clone(),
                                    flags: pf,
                                    ty,
                                    default: None,
                                    hooks,
                                    doc_comment: p.doc_comment.clone(),
                                    attributes: p.attributes.clone(),
                                    line: p.line,
                                });
                            }
                        }
                    }
                    methods.push(MethodDef {
                        name: pstr(&md.name),
                        flags,
                        code,
                        line: m.line,
                    });
                }
                ast::ClassMemberKind::TraitUse(t) => {
                    if is_interface {
                        return self.error(format!(
                            "Cannot use traits inside of interfaces. {} is used in {}",
                            t.traits.first().map(|n| n.resolved.to_string_lossy()).unwrap_or_default(),
                            class_name
                        ));
                    }
                    let names: Vec<PhpStr> = t.traits.iter().map(|n| pstr(&n.resolved)).collect();
                    let mut rules = Vec::new();
                    for a in t.adaptations.iter().flatten() {
                        match a {
                            ast::TraitAdaptation::Precedence {
                                trait_name,
                                method,
                                insteadof,
                                line,
                            } => rules.push(TraitRule::Precedence {
                                trait_name: pstr(&trait_name.resolved),
                                method: pstr(method),
                                insteadof: insteadof.iter().map(|n| pstr(&n.resolved)).collect(),
                                line: *line,
                            }),
                            ast::TraitAdaptation::Alias {
                                trait_name,
                                method,
                                modifiers,
                                alias,
                                line,
                            } => {
                                let f = modifiers_to_flags(*modifiers);
                                if f & member_flags::STATIC != 0 {
                                    self.line = *line;
                                    return self.error("Cannot use \"static\" as method modifier in trait alias");
                                }
                                if f & member_flags::ABSTRACT != 0 {
                                    self.line = *line;
                                    return self.error("Cannot use \"abstract\" as method modifier in trait alias");
                                }
                                if f & member_flags::READONLY != 0 {
                                    self.line = *line;
                                    return self.error("Cannot use \"readonly\" as method modifier in trait alias");
                                }
                                rules.push(TraitRule::Alias {
                                    trait_name: trait_name.as_ref().map(|n| pstr(&n.resolved)),
                                    method: pstr(method),
                                    flags: f,
                                    alias: alias.as_ref().map(|a| pstr(a)),
                                    line: *line,
                                })
                            }
                        }
                    }
                    traits.push(TraitUseDef {
                        traits: names,
                        rules,
                        line: m.line,
                    });
                }
                ast::ClassMemberKind::EnumCase(ec) => {
                    if kind != ClassKindIr::Enum {
                        return self.error("Case can only be used in enums");
                    }
                    if consts.iter().any(|x| x.name.as_bytes() == ec.name.as_bytes())
                        || cases.iter().any(|x| x.name.as_bytes() == ec.name.as_bytes())
                    {
                        return self.error(format!("Cannot redefine class constant {}::{}", class_name, ec.name));
                    }
                    let backed = c.enum_backing_type.is_some();
                    let value = match &ec.value {
                        Some(v) => {
                            if !backed {
                                return self.error(format!(
                                    "Case {} of non-backed enum {} must not have a value",
                                    ec.name, class_name
                                ));
                            }
                            Some(self.const_expr_rc(v, false)?)
                        }
                        None => {
                            if backed {
                                return self.error(format!(
                                    "Case {} of backed enum {} must have a value",
                                    ec.name, class_name
                                ));
                            }
                            None
                        }
                    };
                    if let (Some(v), Some(t)) = (&value, &c.enum_backing_type) {
                        if let Some(lit) = &v.literal {
                            let want_int = matches!(t, ast::TypeHint::Builtin { ty: ast::BuiltinType::Int, .. });
                            let ok = if want_int {
                                matches!(lit, Value::Int(_))
                            } else {
                                matches!(lit, Value::String(_))
                            };
                            if !ok {
                                return self.error(format!(
                                    "Enum case type {} does not match enum backing type {}",
                                    lit.type_name(),
                                    if want_int { "int" } else { "string" }
                                ));
                            }
                        }
                    }
                    let attrs = self.attributes(&ec.attributes)?;
                    cases.push(EnumCaseDef {
                        name: pstr(&ec.name),
                        value,
                        doc_comment: ec.doc_comment.as_ref().map(|d| pstr(d)),
                        attributes: attrs,
                        line: m.line,
                    });
                }
            }
        }
        Ok((consts, props, methods, traits, cases, attributes, has_tostring))
    }

    fn method(&mut self, md: &ast::MethodDecl, flags: u32, class_name: &PhpStr, kind: ClassKindIr) -> CResult<Rc<FuncCode>> {
        let _ = kind;
        let name = pstr(&md.name);
        let lc = name.to_ascii_lowercase();
        let saved = self.begin_fn(FuncKind::Method, name.clone(), &md.common);
        self.fs.is_method = true;
        self.fs.is_static = flags & member_flags::STATIC != 0;
        if md.body.is_none() {
            self.fs.flags |= code_flags::ABSTRACT;
        }
        let display = format!("{class_name}::{name}");
        let is_ctor = lc.as_bytes() == b"__construct";
        let res = (|| -> CResult<_> {
            let attributes = self.attributes(&md.common.attributes)?;
            let params = self.params(&md.common.params, &display, is_ctor, md.body.is_none())?;
            let rt = self.return_type_info(&md.common, &display, lc.as_bytes() == b"__tostring")?;
            if self.fs.is_generator {
                if is_ctor {
                    self.diag(level::E_DEPRECATED, "Making a constructor a Generator is deprecated")?;
                } else if lc.as_bytes() == b"__destruct" {
                    self.diag(level::E_DEPRECATED, "Making a destructor a Generator is deprecated")?;
                }
            }
            let body = match &md.body {
                Some(b) => self.block(b)?,
                None => Vec::new(),
            };
            Ok((params, rt, body, attributes))
        })();
        let (params, rt, body, attributes) = match res {
            Ok(x) => x,
            Err(e) => {
                self.fs = saved;
                return Err(e);
            }
        };
        self.line = md.common.start_line;
        let code = self.finish_fn(saved, name, FuncKind::Method, &md.common, params, body, rt, Vec::new(), attributes)?;
        crate::interp::decl::check_magic_method(self, &code, flags, class_name)?;
        Ok(code)
    }

    /// The access flags of a declared property (`zend_declare_typed_property`):
    /// implicit `public`, implicit `protected(set)` for public readonly
    /// properties, asymmetric visibility checks.
    fn declared_property_flags(&mut self, mut flags: u32, class_name: &PhpStr, name: &PhpStr, typed: bool) -> CResult<u32> {
        const SET: u32 = member_flags::PUBLIC_SET | member_flags::PROTECTED_SET | member_flags::PRIVATE_SET;
        if flags & member_flags::PPP_MASK == 0 {
            flags |= member_flags::PUBLIC;
        }
        if flags & (member_flags::PUBLIC | member_flags::READONLY | SET) == (member_flags::PUBLIC | member_flags::READONLY) {
            flags |= member_flags::PROTECTED_SET;
        } else if flags & SET != 0 {
            if !typed {
                return self.error(format!(
                    "Property with asymmetric visibility {}::${} must have type",
                    class_name, name
                ));
            }
            let get_as_set = (flags & member_flags::PPP_MASK) << 7;
            if get_as_set > flags & SET {
                return self.error(format!(
                    "Visibility of property {}::${} must not be weaker than set visibility",
                    class_name, name
                ));
            }
            if get_as_set == flags & SET {
                flags &= !SET;
            }
            if flags & member_flags::PRIVATE_SET != 0 {
                flags |= member_flags::FINAL;
            }
        }
        Ok(flags)
    }

    /// `zend_compile_property_hooks`. Returns the hooks and whether a hook
    /// body uses the backing value (`!zend_property_is_virtual`).
    fn hooks(
        &mut self,
        hooks: &[ast::PropertyHook],
        prop: &PhpStr,
        prop_flags: u32,
        ty: Option<&TypeInfo>,
        ast_ty: &Option<ast::TypeHint>,
    ) -> CResult<(Vec<HookDef>, bool)> {
        let class_name = self.class.as_ref().map(|c| c.name.clone()).unwrap_or_default();
        let is_interface = self.class.as_ref().is_some_and(|c| c.kind == ClassKindIr::Interface);
        if prop_flags & member_flags::READONLY != 0 {
            return self.error("Hooked properties cannot be readonly");
        }
        if hooks.is_empty() {
            return self.error("Property hook list must not be empty");
        }
        let mut out: Vec<HookDef> = Vec::new();
        let mut uses_prop = false;
        for h in hooks {
            self.line = h.start_line;
            let mut flags = modifiers_to_flags(h.modifiers);
            // Non-private hooks are always public.
            flags |= if prop_flags & member_flags::PPP_MASK != member_flags::PRIVATE {
                member_flags::PUBLIC
            } else {
                member_flags::PRIVATE
            };
            if prop_flags & member_flags::STATIC != 0 {
                return self.error("Cannot declare hooks for static property");
            }
            if flags & member_flags::FINAL != 0 && flags & member_flags::PRIVATE != 0 {
                return self.error("Property hook cannot be both final and private");
            }
            let has_body = !matches!(h.body, ast::HookBody::Abstract);
            if is_interface || (prop_flags & member_flags::ABSTRACT != 0 && !has_body) {
                flags |= member_flags::ABSTRACT;
                if has_body {
                    return self.error("Abstract property hook cannot have body");
                }
                if flags & member_flags::PRIVATE != 0 {
                    return self.error("Property hook cannot be both abstract and private");
                }
                if flags & member_flags::FINAL != 0 {
                    return self.error("Property hook cannot be both abstract and final");
                }
            } else if !has_body {
                return self.error("Non-abstract property hook must have a body");
            }
            let kind = if h.name.eq_ignore_case(b"get") {
                HookKind::Get
            } else if h.name.eq_ignore_case(b"set") {
                HookKind::Set
            } else {
                return self.error(format!(
                    "Unknown hook \"{}\" for property {}::${}, expected \"get\" or \"set\"",
                    h.name, class_name, prop
                ));
            };
            if kind == HookKind::Set && matches!(h.body, ast::HookBody::Expr(_)) {
                uses_prop = true;
            }
            if kind == HookKind::Get {
                if h.params.is_some() {
                    return self.error(format!(
                        "get hook of property {}::${} must not have a parameter list",
                        class_name, prop
                    ));
                }
            } else if let Some(ps) = &h.params {
                if ps.len() != 1 {
                    return self.error(format!(
                        "{} hook of property {}::${} must accept exactly one parameters",
                        h.name, class_name, prop
                    ));
                }
                let vp = &ps[0];
                if vp.by_ref {
                    return self.error(format!(
                        "Parameter ${} of {} hook {}::${} must not be pass-by-reference",
                        vp.name, h.name, class_name, prop
                    ));
                }
                if vp.variadic {
                    return self.error(format!(
                        "Parameter ${} of {} hook {}::${} must not be variadic",
                        vp.name, h.name, class_name, prop
                    ));
                }
                if vp.default.is_some() {
                    return self.error(format!(
                        "Parameter ${} of {} hook {}::${} must not have a default value",
                        vp.name, h.name, class_name, prop
                    ));
                }
                if ast_ty.is_some() != vp.ty.is_some() {
                    return self.error(format!(
                        "Type of parameter ${} of hook {}::${}::set must be compatible with property type",
                        vp.name, class_name, prop
                    ));
                }
            }
            let name = PhpStr::from(format!(
                "${prop}::{}",
                if kind == HookKind::Get { "get" } else { "set" }
            ));
            let common = ast::FunctionCommon {
                by_ref: h.by_ref,
                params: h.params.clone().unwrap_or_default(),
                return_type: None,
                attributes: h.attributes.clone(),
                doc_comment: h.doc_comment.clone(),
                start_line: h.start_line,
                end_line: h.end_line,
                is_generator: h.is_generator,
            };
            let saved = self.begin_fn(FuncKind::Hook, name.clone(), &common);
            self.fs.is_method = true;
            self.fs.hook_prop = Some(prop.clone());
            self.fs.hook_kind = Some(kind);
            if !has_body {
                self.fs.flags |= code_flags::ABSTRACT;
            }
            let display = format!("{class_name}::{name}");
            let res = (|| -> CResult<_> {
                let attributes = self.attributes(&h.attributes)?;
                let mut params = Vec::new();
                if kind == HookKind::Set {
                    match &h.params {
                        Some(ps) => {
                            params = self.params(ps, &display, false, !has_body)?;
                        }
                        None => {
                            let slot = self.cv(b"value");
                            params.push(ParamInfo {
                                name: PhpStr::from("value"),
                                slot,
                                by_ref: false,
                                variadic: false,
                                ty: ty.cloned(),
                                default: None,
                                default_const: None,
                                default_text: None,
                                promote: None,
                                attributes: Vec::new(),
                                doc_comment: None,
                                line: h.start_line,
                            });
                        }
                    }
                }
                let body = match &h.body {
                    ast::HookBody::Block(b) => self.block(b)?,
                    ast::HookBody::Expr(e) => {
                        let x = self.expr(e)?;
                        if kind == HookKind::Get {
                            vec![Stmt {
                                kind: StmtKind::Return(Some(x)),
                                line: e.line,
                            }]
                        } else {
                            // `set => expr` assigns the backing value.
                            vec![Stmt {
                                kind: StmtKind::Expr(Expr::Assign(
                                    Box::new(LVal::Prop(
                                        Box::new(LVal::This),
                                        Member::Name(prop.clone()),
                                        false,
                                        e.line,
                                    )),
                                    Box::new(x),
                                    e.line,
                                )),
                                line: e.line,
                            }]
                        }
                    }
                    ast::HookBody::Abstract => Vec::new(),
                };
                Ok((params, body, attributes))
            })();
            let (params, body, attributes) = match res {
                Ok(x) => x,
                Err(e) => {
                    self.fs = saved;
                    return Err(e);
                }
            };
            if self.fs.hook_uses_prop {
                uses_prop = true;
            }
            let rt = if kind == HookKind::Get {
                ty.cloned()
            } else {
                Some(TypeInfo {
                    decl: TypeDecl::from_mask(mask::VOID),
                    text: PhpStr::from("void"),
                })
            };
            let code = self.finish_fn(saved, name, FuncKind::Hook, &common, params, body, rt, Vec::new(), attributes)?;
            if out.iter().any(|x| x.kind == kind) {
                return self.error(format!("Cannot redeclare property hook \"{}\"", h.name));
            }
            out.push(HookDef {
                kind,
                code: Some(code),
                flags,
                line: h.start_line,
            });
        }
        Ok((out, uses_prop))
    }
}

/// `zend_is_valid_default_value`.
fn is_valid_default(t: &TypeDecl, v: &Value) -> bool {
    if t.is_mixed() {
        return true;
    }
    if t.matches_exact(v) {
        return true;
    }
    match v {
        Value::Int(_) => t.mask & mask::FLOAT != 0,
        Value::Array(_) => t.classes.iter().any(|c| matches!(c, ClassType::Name(n) if n.eq_ignore_ascii_case(b"Traversable"))) && t.mask & mask::ARRAY != 0,
        // Constant-AST values (non-literals) are checked at runtime.
        _ => false,
    }
}

/// `zend_get_type_by_const`.
fn const_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null | Value::Undef => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        _ => "unknown",
    }
}
