//! Expression and write-context (variable) lowering.

use std::cell::OnceCell;
use std::rc::Rc;

use php_syntax::ast;

use super::ir::*;
use super::*;
use crate::value::Value;

/// Whether `e` is part of a property/method/dim chain whose innermost
/// operand is a nullsafe access (`zend_ast_is_short_circuited`).
fn is_short_circuited(e: &ast::Expr) -> bool {
    match &e.kind {
        ast::ExprKind::Dim { base, .. } => is_short_circuited(base),
        ast::ExprKind::Prop {
            object, nullsafe, ..
        } => *nullsafe || is_short_circuited(object),
        ast::ExprKind::MethodCall {
            object, nullsafe, ..
        } => *nullsafe || is_short_circuited(object),
        ast::ExprKind::StaticProp {
            class: ast::ClassRef::Expr(c),
            ..
        } => is_short_circuited(c),
        ast::ExprKind::StaticCall {
            class: ast::ClassRef::Expr(c),
            ..
        } => is_short_circuited(c),
        _ => false,
    }
}

fn is_chain_kind(e: &ast::Expr) -> bool {
    matches!(
        e.kind,
        ast::ExprKind::Dim { .. }
            | ast::ExprKind::Prop { .. }
            | ast::ExprKind::MethodCall { .. }
            | ast::ExprKind::StaticProp { .. }
            | ast::ExprKind::StaticCall { .. }
    )
}

fn bin_op(op: ast::BinaryOp) -> Option<BinOp> {
    use ast::BinaryOp as A;
    Some(match op {
        A::Add => BinOp::Add,
        A::Sub => BinOp::Sub,
        A::Mul => BinOp::Mul,
        A::Div => BinOp::Div,
        A::Mod => BinOp::Mod,
        A::Pow => BinOp::Pow,
        A::Concat => BinOp::Concat,
        A::BitAnd => BinOp::BitAnd,
        A::BitOr => BinOp::BitOr,
        A::BitXor => BinOp::BitXor,
        A::ShiftLeft => BinOp::Shl,
        A::ShiftRight => BinOp::Shr,
        A::LogicalXor => BinOp::Xor,
        A::Identical => BinOp::Identical,
        A::NotIdentical => BinOp::NotIdentical,
        A::Equal => BinOp::Equal,
        A::NotEqual => BinOp::NotEqual,
        A::Less => BinOp::Less,
        A::LessEqual => BinOp::LessEqual,
        A::Greater => BinOp::Greater,
        A::GreaterEqual => BinOp::GreaterEqual,
        A::Spaceship => BinOp::Spaceship,
        A::BooleanAnd | A::BooleanOr | A::Coalesce | A::Pipe => return None,
    })
}

impl Lowerer<'_> {
    /// An expression statement (result unused).
    pub(crate) fn expr_stmt(&mut self, e: &ast::Expr) -> CResult<Expr> {
        self.expr(e)
    }

    /// Lowers an expression in read context.
    pub(crate) fn expr(&mut self, e: &ast::Expr) -> CResult<Expr> {
        if is_chain_kind(e) && is_short_circuited(e) {
            let inner = self.expr_inner(e)?;
            return Ok(Expr::Chain(Box::new(inner)));
        }
        self.expr_inner(e)
    }

    /// The operand of a property/dim fetch inside a constant expression: a
    /// nullsafe short-circuit in it must skip the whole chain, so it is not
    /// wrapped in its own [`Expr::Chain`].
    fn const_chain_operand(&mut self, e: &ast::Expr) -> CResult<Expr> {
        if is_chain_kind(e) {
            self.expr_inner(e)
        } else {
            self.expr(e)
        }
    }

    pub(crate) fn boxed(&mut self, e: &ast::Expr) -> CResult<Box<Expr>> {
        Ok(Box::new(self.expr(e)?))
    }

    fn expr_inner(&mut self, e: &ast::Expr) -> CResult<Expr> {
        self.line = e.line;
        let line = e.line;
        use ast::ExprKind as K;
        Ok(match &e.kind {
            K::Int(i) => Expr::Lit(Value::Int(*i)),
            K::Float(f) => Expr::Lit(Value::Float(*f)),
            K::String(s) => Expr::Lit(Value::String(pstr(s))),
            K::Interpolated(parts) => Expr::Interp(self.interp_parts(parts)?),
            K::ShellExec(parts) => {
                let p = self.interp_parts(parts)?;
                Expr::ShellExec(p, self.line)
            }
            K::Const(name) => self.const_ref(name)?,
            K::MagicConst(m) => self.magic_const(*m, line)?,
            K::ClassConst { class, name } => {
                let cs = self.class_spec(class)?;
                let member = match name {
                    ast::ClassConstName::Ident(n) => Member::Name(pstr(n)),
                    ast::ClassConstName::Expr(e) => Member::Expr(self.boxed(e)?),
                };
                Expr::ClassConst(Box::new((cs, member)), self.line)
            }
            K::ClassName(class) => {
                if let ast::ClassRef::Name(n) = class {
                    if n.special_class().is_none() {
                        return Ok(Expr::Lit(Value::String(pstr(&n.resolved))));
                    }
                    if let Some(ast::SpecialClass::Self_) = n.special_class() {
                        if let Some(c) = &self.class {
                            if c.kind != ClassKindIr::Trait && !c.is_anon && !self.in_closure() {
                                return Ok(Expr::Lit(Value::String(c.name.clone())));
                            }
                        } else if !self.in_closure() {
                            return self.error("Cannot use \"self\" when no class scope is active");
                        }
                    }
                    if let Some(ast::SpecialClass::Static) = n.special_class() {
                        if self.class.is_none() && !self.in_closure() {
                            return self.error("Cannot use \"static\" when no class scope is active");
                        }
                    }
                    if let Some(ast::SpecialClass::Parent) = n.special_class() {
                        if self.class.is_none() && !self.in_closure() {
                            return self.error("Cannot use \"parent\" when no class scope is active");
                        }
                    }
                }
                let cs = self.class_spec(class)?;
                Expr::ClassName(Box::new(cs), self.line)
            }
            K::Var(v) => match v {
                ast::VarName::Name(n) => self.var_read(n, line)?,
                ast::VarName::Expr(_) => Expr::Place(Box::new(self.lval(e, LvMode::Isset)?)),
            },
            K::Array { items, syntax } => {
                if *syntax == ast::ArraySyntax::List {
                    return self.error("Cannot use list() as standalone expression");
                }
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    let Some(it) = it else {
                        return self.error("Cannot use empty array elements in arrays");
                    };
                    self.line = it.line;
                    let key = match &it.key {
                        Some(k) => Some(self.expr(k)?),
                        None => None,
                    };
                    let value = if it.by_ref {
                        self.ref_source(&it.value)?
                    } else {
                        self.expr(&it.value)?
                    };
                    out.push(ArrayItem {
                        key,
                        value,
                        by_ref: it.by_ref,
                        spread: it.unpack,
                    });
                }
                Expr::Array(out, self.line)
            }
            K::Dim { index: None, .. } => {
                return self.error("Cannot use [] for reading");
            }
            K::Prop {
                object,
                name,
                nullsafe,
            } if self.in_const_expr => {
                let obj = Box::new(self.const_chain_operand(object)?);
                let m = self.member(name)?;
                Expr::ConstProp(obj, m, *nullsafe, line)
            }
            K::Dim {
                base,
                index: Some(index),
            } if self.in_const_expr => {
                // zend_ast_evaluate: the base is evaluated as an expression
                // (so property fetches in it get the enum check).
                let b = LVal::Temp(Box::new(self.const_chain_operand(base)?));
                let i = self.boxed(index)?;
                Expr::Place(Box::new(LVal::Dim(Box::new(b), Some(i), line)))
            }
            K::Dim { .. } | K::Prop { .. } | K::StaticProp { .. } => {
                Expr::Place(Box::new(self.lval_read(e)?))
            }
            K::Call { callee, args } => self.call(callee, args, line)?,
            K::MethodCall {
                object,
                name,
                args,
                nullsafe,
            } => {
                if args.is_callable_convert() {
                    if *nullsafe {
                        return self.error("Cannot combine nullsafe operator with Closure creation");
                    }
                    let obj = self.expr_chain_base(object)?;
                    let m = self.member(name)?;
                    return Ok(Expr::CallableConvert(
                        Box::new(CallableConvert::Method(obj, m, false)),
                        line,
                    ));
                }
                let obj = self.expr_chain_base(object)?;
                let m = self.member(name)?;
                let a = self.args(args)?;
                Expr::MethodCall(Box::new(MethodCallExpr {
                    object: obj,
                    name: m,
                    args: a,
                    nullsafe: *nullsafe,
                    line,
                }))
            }
            K::StaticCall { class, name, args } => {
                let cs = self.class_spec(class)?;
                let m = self.member(name)?;
                if args.is_callable_convert() {
                    return Ok(Expr::CallableConvert(
                        Box::new(CallableConvert::Static(cs, m)),
                        line,
                    ));
                }
                let a = self.args(args)?;
                Expr::StaticCall(Box::new(StaticCallExpr {
                    class: cs,
                    name: m,
                    args: a,
                    line,
                }))
            }
            K::New { class, args } => {
                if args.is_callable_convert() {
                    return self.error("Cannot create Closure for new expression");
                }
                let target = match class {
                    ast::NewClass::Name(n) => NewTarget::Class(self.class_spec_name(n)?),
                    ast::NewClass::Expr(e) => NewTarget::Class(ClassSpec::Expr(self.boxed(e)?)),
                    ast::NewClass::Anonymous(decl) => {
                        let def = self.class_decl(decl, false)?;
                        NewTarget::Anonymous(def)
                    }
                };
                self.line = line;
                let a = self.args(args)?;
                Expr::New(Box::new(NewExpr {
                    class: target,
                    args: a,
                    line,
                }))
            }
            K::Assign { target, value } => self.assign(target, value)?,
            K::AssignRef { target, value } => {
                if is_this(target) {
                    return self.error("Cannot re-assign $this");
                }
                self.ensure_writable(target)?;
                if let K::Var(ast::VarName::Name(n)) = &value.kind {
                    if n.as_bytes() == b"GLOBALS" {
                        return self.error("Cannot acquire reference to $GLOBALS");
                    }
                }
                if is_short_circuited(value) {
                    return self.error("Cannot take reference of a nullsafe chain");
                }
                let t = self.lval(target, LvMode::Ref)?;
                let v = self.ref_source(value)?;
                Expr::AssignRef(Box::new(t), Box::new(v), line)
            }
            K::AssignOp { op, target, value } => {
                self.ensure_writable(target)?;
                if is_this(target) {
                    return self.error("Cannot re-assign $this");
                }
                let t = self.lval(target, LvMode::ReadWrite)?;
                let v = self.expr(value)?;
                let op = bin_op(*op).expect("compound op");
                Expr::AssignOp(op, Box::new(t), Box::new(v), self.line)
            }
            K::AssignCoalesce { target, value } => {
                self.ensure_writable(target)?;
                if is_this(target) {
                    return self.error("Cannot re-assign $this");
                }
                let t = self.lval(target, LvMode::ReadWrite)?;
                let v = self.expr(value)?;
                Expr::AssignCoalesce(Box::new(t), Box::new(v), line)
            }
            K::IncDec { op, target } => {
                self.ensure_writable(target)?;
                if is_this(target) {
                    return self.error("Cannot re-assign $this");
                }
                let t = self.lval(target, LvMode::ReadWrite)?;
                let op = match op {
                    ast::IncDecOp::PreInc => IncDec::PreInc,
                    ast::IncDecOp::PreDec => IncDec::PreDec,
                    ast::IncDecOp::PostInc => IncDec::PostInc,
                    ast::IncDecOp::PostDec => IncDec::PostDec,
                };
                Expr::IncDec(op, Box::new(t), self.line)
            }
            K::Binary { op, left, right } => match op {
                ast::BinaryOp::BooleanAnd => {
                    let l = self.boxed(left)?;
                    let r = self.boxed(right)?;
                    Expr::And(l, r)
                }
                ast::BinaryOp::BooleanOr => {
                    let l = self.boxed(left)?;
                    let r = self.boxed(right)?;
                    Expr::Or(l, r)
                }
                ast::BinaryOp::Coalesce => {
                    let l = self.isset_operand(left)?;
                    let r = self.boxed(right)?;
                    Expr::Coalesce(Box::new(l), r)
                }
                ast::BinaryOp::Pipe => {
                    let l = self.boxed(left)?;
                    if let K::ArrowFn(_) = right.kind {
                        if !matches!(&right.kind, K::ArrowFn(a) if a.parenthesized) {
                            return self.error(
                                "Arrow functions on the right hand side of |> must be parenthesized",
                            );
                        }
                    }
                    let r = self.boxed(right)?;
                    Expr::Pipe(l, r, self.line)
                }
                _ => {
                    let l = self.boxed(left)?;
                    let r = self.boxed(right)?;
                    Expr::Bin(bin_op(*op).expect("binary op"), l, r, self.line)
                }
            },
            K::Unary { op, expr } => {
                // Fold negative literals (`-1`, `-1.5`).
                if let (ast::UnaryOp::Minus, K::Int(i)) = (op, &expr.kind) {
                    return Ok(Expr::Lit(Value::Int(i.wrapping_neg())));
                }
                if let (ast::UnaryOp::Minus, K::Float(f)) = (op, &expr.kind) {
                    return Ok(Expr::Lit(Value::Float(-f)));
                }
                if let (ast::UnaryOp::Plus, K::Int(i)) = (op, &expr.kind) {
                    return Ok(Expr::Lit(Value::Int(*i)));
                }
                if let (ast::UnaryOp::Plus, K::Float(f)) = (op, &expr.kind) {
                    return Ok(Expr::Lit(Value::Float(*f)));
                }
                let inner = self.boxed(expr)?;
                let op = match op {
                    ast::UnaryOp::Not => UnOp::Not,
                    ast::UnaryOp::BitNot => UnOp::BitNot,
                    ast::UnaryOp::Plus => UnOp::Plus,
                    ast::UnaryOp::Minus => UnOp::Minus,
                };
                Expr::Unary(op, inner, self.line)
            }
            K::Cast { kind, expr } => {
                let op = match kind {
                    ast::CastKind::Int => CastOp::Int,
                    ast::CastKind::Float => CastOp::Float,
                    ast::CastKind::String => CastOp::String,
                    ast::CastKind::Array => CastOp::Array,
                    ast::CastKind::Object => CastOp::Object,
                    ast::CastKind::Bool => CastOp::Bool,
                    ast::CastKind::Unset => {
                        return self.error("The (unset) cast is no longer supported");
                    }
                };
                let inner = self.boxed(expr)?;
                Expr::Cast(op, inner, self.line)
            }
            K::VoidCast(inner) => Expr::Void(self.boxed(inner)?),
            K::Silence(inner) => Expr::Silence(self.boxed(inner)?),
            K::Isset(vars) => {
                let mut out = Vec::with_capacity(vars.len());
                for v in vars {
                    self.line = v.line;
                    if !is_variable_like(v) {
                        return self.error(
                            "Cannot use isset() on the result of an expression (you can use \"null !== expression\" instead)",
                        );
                    }
                    out.push(self.lval(v, LvMode::Isset)?);
                }
                Expr::Isset(out)
            }
            K::Empty(inner) => {
                let v = self.isset_operand(inner)?;
                Expr::Empty(Box::new(v))
            }
            K::Include { kind, expr } => {
                self.fs.flags |= code_flags::DYNAMIC_VARS;
                let inner = self.boxed(expr)?;
                let op = match kind {
                    ast::IncludeKind::Include => IncludeOp::Include,
                    ast::IncludeKind::IncludeOnce => IncludeOp::IncludeOnce,
                    ast::IncludeKind::Require => IncludeOp::Require,
                    ast::IncludeKind::RequireOnce => IncludeOp::RequireOnce,
                };
                Expr::Include(op, inner, line)
            }
            K::Eval(inner) => {
                self.fs.flags |= code_flags::DYNAMIC_VARS;
                let inner = self.boxed(inner)?;
                Expr::Eval(inner, line)
            }
            K::Print(inner) => Expr::Print(self.boxed(inner)?),
            K::Closure(c) => self.closure(c, line)?,
            K::ArrowFn(a) => self.arrow_fn(a, line)?,
            K::Ternary {
                cond, then, else_, ..
            } => {
                let c = self.boxed(cond)?;
                let t = match then {
                    Some(t) => Some(self.boxed(t)?),
                    None => None,
                };
                let el = self.boxed(else_)?;
                Expr::Ternary(c, t, el)
            }
            K::Match { subject, arms } => {
                let s = self.boxed(subject)?;
                let mut out = Vec::with_capacity(arms.len());
                let mut has_default = false;
                for a in arms {
                    self.line = a.line;
                    let conds = match &a.conds {
                        None => {
                            if has_default {
                                return self.error("Match expressions may only contain one default arm");
                            }
                            has_default = true;
                            None
                        }
                        Some(cs) => {
                            let mut v = Vec::with_capacity(cs.len());
                            for c in cs {
                                v.push(self.expr(c)?);
                            }
                            Some(v)
                        }
                    };
                    let body = self.expr(&a.body)?;
                    out.push(MatchArm { conds, body });
                }
                Expr::Match(s, out, line)
            }
            K::Instanceof { expr, class } => {
                let v = self.boxed(expr)?;
                let cs = match class {
                    ast::ClassRef::Name(n) => self.class_spec_name(n)?,
                    ast::ClassRef::Expr(e) => ClassSpec::Expr(self.boxed(e)?),
                };
                Expr::Instanceof(v, Box::new(cs), self.line)
            }
            K::Throw(inner) => {
                let v = self.boxed(inner)?;
                Expr::Throw(v, line)
            }
            K::Yield { key, value } => {
                self.fs.is_generator = true;
                self.fs.flags |= code_flags::GENERATOR;
                if self.fs.kind == FuncKind::Main {
                    return self.error("The \"yield\" expression can only be used inside a function");
                }
                let k = match key {
                    Some(k) => Some(self.boxed(k)?),
                    None => None,
                };
                let v = match value {
                    Some(v) => {
                        if self.fs.flags & code_flags::RETURNS_REF != 0 && is_variable_like(v) {
                            Some(Box::new(self.ref_source(v)?))
                        } else {
                            Some(self.boxed(v)?)
                        }
                    }
                    None => None,
                };
                Expr::Yield(k, v, line)
            }
            K::YieldFrom(inner) => {
                self.fs.is_generator = true;
                self.fs.flags |= code_flags::GENERATOR;
                if self.fs.kind == FuncKind::Main {
                    return self.error("The \"yield from\" expression can only be used inside a function");
                }
                if self.fs.flags & code_flags::RETURNS_REF != 0 {
                    return self.error("Cannot use \"yield from\" inside a by-reference generator");
                }
                let v = self.boxed(inner)?;
                Expr::YieldFrom(v, line)
            }
        })
    }

    pub(crate) fn in_closure(&self) -> bool {
        matches!(self.fs.kind, FuncKind::Closure | FuncKind::ArrowFn)
    }

    fn interp_parts(&mut self, parts: &[ast::StrPart]) -> CResult<Vec<InterpPart>> {
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            match p {
                ast::StrPart::Literal(s) => out.push(InterpPart::Lit(pstr(s))),
                ast::StrPart::Expr { expr, .. } => out.push(InterpPart::Expr(self.expr(expr)?)),
            }
        }
        Ok(out)
    }

    fn var_read(&mut self, n: &[u8], line: u32) -> CResult<Expr> {
        if n == b"this" {
            self.fs.flags |= code_flags::USES_THIS;
            return Ok(Expr::This(line));
        }
        if n == b"GLOBALS" {
            return Ok(Expr::Globals);
        }
        if is_auto_global(n) {
            return Ok(Expr::Place(Box::new(LVal::Global(Box::new(Expr::Lit(
                Value::String(pstr(n)),
            ))))));
        }
        if n == b"http_response_header" {
            self.diag(
                crate::errors::level::E_DEPRECATED,
                "The predefined locally scoped $http_response_header variable is deprecated, call http_get_last_response_headers() instead",
            )?;
        }
        Ok(Expr::Var(self.cv(n), line))
    }

    /// The object operand of a property/method chain (no chain wrapping).
    fn expr_chain_base(&mut self, e: &ast::Expr) -> CResult<Expr> {
        if is_chain_kind(e) {
            self.expr_inner(e)
        } else {
            self.expr(e)
        }
    }

    /// Operand of `??` / `empty()`: variables are fetched in `BP_VAR_IS` mode.
    fn isset_operand(&mut self, e: &ast::Expr) -> CResult<Expr> {
        if is_variable_like(e) && !is_this(e) {
            if let ast::ExprKind::Var(ast::VarName::Name(n)) = &e.kind {
                if n.as_bytes() == b"GLOBALS" {
                    return Ok(Expr::Globals);
                }
            }
            let lv = self.lval(e, LvMode::Isset)?;
            let inner = Expr::PlaceIs(Box::new(lv));
            if is_short_circuited(e) {
                return Ok(Expr::Chain(Box::new(inner)));
            }
            return Ok(inner);
        }
        self.expr(e)
    }

    fn magic_const(&mut self, m: ast::MagicConst, line: u32) -> CResult<Expr> {
        use ast::MagicConst as M;
        let s = |b: &PhpStr| Ok(Expr::Lit(Value::String(b.clone())));
        match m {
            M::Line => Ok(Expr::Lit(Value::Int(line as i64))),
            M::File => s(&self.file.clone()),
            M::Dir => s(&self.dir.clone()),
            M::Namespace => s(&self.namespace.clone()),
            M::Function => {
                if self.fs.kind == FuncKind::Main {
                    return s(&PhpStr::new());
                }
                if let Some(k) = self.fs.hook_kind {
                    let p = self.fs.hook_prop.clone().unwrap_or_default();
                    return s(&PhpStr::from(format!(
                        "${}::{}",
                        p,
                        if k == HookKind::Get { "get" } else { "set" }
                    )));
                }
                s(&self.fs.name.clone())
            }
            M::Method => {
                if self.fs.kind == FuncKind::Main {
                    if let Some(c) = &self.class {
                        if c.kind == ClassKindIr::Trait {
                            // Runtime, but php-src prints the trait name at compile time here.
                        }
                        return s(&c.name.clone());
                    }
                    return s(&PhpStr::new());
                }
                if self.in_closure() {
                    return s(&self.fs.name.clone());
                }
                if let Some(k) = self.fs.hook_kind {
                    let c = self.class.as_ref().map(|c| c.name.clone()).unwrap_or_default();
                    let p = self.fs.hook_prop.clone().unwrap_or_default();
                    return s(&PhpStr::from(format!(
                        "{}::${}::{}",
                        c,
                        p,
                        if k == HookKind::Get { "get" } else { "set" }
                    )));
                }
                match &self.class {
                    Some(c) if self.fs.kind == FuncKind::Method => {
                        s(&PhpStr::from(format!("{}::{}", c.name, self.fs.name)))
                    }
                    _ => s(&self.fs.name.clone()),
                }
            }
            M::Class => match &self.class {
                Some(c) if c.kind == ClassKindIr::Trait => Ok(Expr::ClassMagic),
                Some(c) => {
                    if self.fs.kind == FuncKind::Function {
                        return s(&PhpStr::new());
                    }
                    s(&c.name.clone())
                }
                None => {
                    if self.in_closure() {
                        // Closures outside classes: runtime scope (bindTo).
                        return Ok(Expr::ClassMagic);
                    }
                    s(&PhpStr::new())
                }
            },
            M::Trait => match &self.class {
                Some(c) if c.kind == ClassKindIr::Trait => s(&c.name.clone()),
                _ => s(&PhpStr::new()),
            },
            M::Property => match &self.fs.hook_prop {
                Some(p) => s(&p.clone()),
                None => self.error("Cannot use __PROPERTY__ outside a property hook"),
            },
        }
    }

    fn const_ref(&mut self, name: &ast::Name) -> CResult<Expr> {
        let n = &name.name;
        let unqualified = matches!(
            name.kind,
            ast::NameKind::Unqualified | ast::NameKind::FullyQualified
        );
        if unqualified && !n.contains(&b'\\') {
            if n.eq_ignore_case(b"true") {
                return Ok(Expr::Lit(Value::Bool(true)));
            }
            if n.eq_ignore_case(b"false") {
                return Ok(Expr::Lit(Value::Bool(false)));
            }
            if n.eq_ignore_case(b"null") {
                return Ok(Expr::Lit(Value::Null));
            }
            if n.as_bytes() == b"__COMPILER_HALT_OFFSET__" {
                if let Some(off) = self.halt_offset {
                    return Ok(Expr::Lit(Value::Int(off as i64)));
                }
            }
        }
        let resolved = pstr(&name.resolved);
        let fallback = if name.fallback {
            Some(pstr(&name.name))
        } else {
            None
        };
        let display = pstr(&name.resolved);
        Ok(Expr::Const(
            Box::new(ConstRef {
                name: resolved,
                fallback,
                display,
                cache: OnceCell::new(),
            }),
            self.line,
        ))
    }

    /// A class reference (`A::`, `new A`): `self`/`parent`/`static` checks.
    pub(crate) fn class_spec(&mut self, c: &ast::ClassRef) -> CResult<ClassSpec> {
        match c {
            ast::ClassRef::Name(n) => self.class_spec_name(n),
            ast::ClassRef::Expr(e) => Ok(ClassSpec::Expr(self.boxed(e)?)),
        }
    }

    pub(crate) fn class_spec_name(&mut self, n: &ast::Name) -> CResult<ClassSpec> {
        match n.special_class() {
            Some(ast::SpecialClass::Self_) => {
                self.ensure_class_scope("self")?;
                Ok(ClassSpec::Self_)
            }
            Some(ast::SpecialClass::Parent) => {
                self.ensure_class_scope("parent")?;
                Ok(ClassSpec::Parent)
            }
            Some(ast::SpecialClass::Static) => {
                self.ensure_class_scope("static")?;
                Ok(ClassSpec::Static)
            }
            None => Ok(ClassSpec::Named(pstr(&n.resolved))),
        }
    }

    /// `zend_ensure_valid_class_fetch_type`.
    fn ensure_class_scope(&mut self, what: &str) -> CResult<()> {
        if self.in_closure() || self.fs.kind == FuncKind::Main && self.class.is_none() && !self.in_const_expr {
            // Closures may be bound later; top-level code errors at runtime.
            if self.fs.kind == FuncKind::Main && !self.in_closure() && self.class.is_none() {
                // php-src: compile error only when the scope is known (not in
                // closures or top-level code, which may run inside a class via include).
            }
            return Ok(());
        }
        match &self.class {
            None => {
                if self.fs.kind == FuncKind::Function {
                    return self.error(format!("Cannot use \"{what}\" when no class scope is active"));
                }
                Ok(())
            }
            Some(c) => {
                if what == "parent"
                    && c.parent.is_none()
                    && c.kind != ClassKindIr::Trait
                    && !self.in_closure()
                {
                    return self.error("Cannot use \"parent\" when current class scope has no parent");
                }
                Ok(())
            }
        }
    }

    /// A class name used in `catch`/`instanceof`: resolved name or special.
    pub(crate) fn class_name_ref(&mut self, n: &ast::Name) -> CResult<PhpStr> {
        match n.special_class() {
            Some(ast::SpecialClass::Static) => {
                self.error("Bad class name in the catch statement")
            }
            Some(ast::SpecialClass::Self_) => Ok(PhpStr::from("self")),
            Some(ast::SpecialClass::Parent) => Ok(PhpStr::from("parent")),
            None => Ok(pstr(&n.resolved)),
        }
    }

    pub(crate) fn member(&mut self, m: &ast::MemberName) -> CResult<Member> {
        Ok(match m {
            ast::MemberName::Ident(n) => Member::Name(pstr(n)),
            ast::MemberName::Expr(e) => match &e.kind {
                ast::ExprKind::String(s) => Member::Name(pstr(s)),
                _ => Member::Expr(self.boxed(e)?),
            },
        })
    }

    // ----- calls -----------------------------------------------------------------

    pub(crate) fn args(&mut self, a: &ast::ArgList) -> CResult<ArgList> {
        let mut out = Vec::with_capacity(a.args.len());
        let mut has_spread = false;
        let mut has_named = false;
        let mut names: Vec<&[u8]> = Vec::new();
        for arg in &a.args {
            match arg {
                ast::Arg::Positional(e) => {
                    if has_named {
                        self.line = e.line;
                        return self.error("Cannot use positional argument after named argument");
                    }
                    if has_spread {
                        self.line = e.line;
                        return self.error("Cannot use positional argument after argument unpacking");
                    }
                    out.push(Arg::Pos(self.arg_expr(e)?));
                }
                ast::Arg::Unpack(e) => {
                    if has_named {
                        self.line = e.line;
                        return self.error("Cannot use argument unpacking after named arguments");
                    }
                    has_spread = true;
                    out.push(Arg::Spread(self.expr(e)?));
                }
                ast::Arg::Named { name, value } => {
                    if names.contains(&name.as_bytes()) {
                        self.line = value.line;
                        return self.error(format!("Duplicate named parameter ${name}"));
                    }
                    names.push(name.as_bytes());
                    has_named = true;
                    out.push(Arg::Named(pstr(name), self.arg_expr(value)?));
                }
                ast::Arg::Placeholder | ast::Arg::NamedPlaceholder(_) | ast::Arg::VariadicPlaceholder => {
                    return self.error("Partial function application is not supported");
                }
            }
        }
        Ok(ArgList {
            args: out,
            has_spread,
            has_named,
        })
    }

    /// An argument: variables are kept as places so they can be passed by reference.
    fn arg_expr(&mut self, e: &ast::Expr) -> CResult<Expr> {
        if let ast::ExprKind::Var(ast::VarName::Name(n)) = &e.kind {
            if n.as_bytes() != b"this" && n.as_bytes() != b"GLOBALS" && !is_auto_global(n) {
                return Ok(Expr::Var(self.cv(n), e.line));
            }
        }
        if matches!(
            e.kind,
            ast::ExprKind::Dim { .. } | ast::ExprKind::Prop { .. } | ast::ExprKind::StaticProp { .. }
        ) && !is_short_circuited(e)
        {
            if let ast::ExprKind::Dim { index: None, .. } = e.kind {
                // `f($a[])`: only valid for by-reference parameters.
                let lv = self.lval(e, LvMode::Write)?;
                return Ok(Expr::Place(Box::new(lv)));
            }
            let lv = self.lval(e, LvMode::Isset)?;
            return Ok(Expr::Place(Box::new(lv)));
        }
        self.expr(e)
    }

    fn call(&mut self, callee: &ast::Callee, args: &ast::ArgList, line: u32) -> CResult<Expr> {
        if let ast::Callee::Name(n) = callee {
            let lc = n.name.to_ascii_lowercase();
            let is_fq_builtin = n.kind == ast::NameKind::FullyQualified || !n.resolved.contains(&b'\\');
            if n.kind == ast::NameKind::FullyQualified && lc.as_bytes() == b"exit" && !args.is_callable_convert() {
                let a = self.args(args)?;
                let arg = match a.args.into_iter().next() {
                    Some(Arg::Pos(e)) => Some(Box::new(e)),
                    Some(_) => None,
                    None => None,
                };
                return Ok(Expr::Exit(arg, line));
            }
            if n.kind == ast::NameKind::FullyQualified && lc.as_bytes() == b"clone" && !args.is_callable_convert() {
                if args.args.len() == 1 {
                    if let ast::Arg::Positional(e) = &args.args[0] {
                        let obj = self.expr(e)?;
                        return Ok(Expr::Clone(Box::new(obj), None, self.line));
                    }
                }
                let a = self.args(args)?;
                if !a.has_named && !a.has_spread && a.args.len() == 2 {
                    let mut it = a.args.into_iter();
                    let (Some(Arg::Pos(o)), Some(Arg::Pos(w))) = (it.next(), it.next()) else {
                        unreachable!()
                    };
                    return Ok(Expr::Clone(Box::new(o), Some(Box::new(w)), line));
                }
                let name = NamedCallee {
                    name: PhpStr::from("clone"),
                    lc: PhpStr::from("clone"),
                    fallback: None,
                    cache: OnceCell::new(),
                };
                return Ok(Expr::Call(Box::new(CallExpr {
                    callee: Callee::Named(Box::new(name)),
                    args: a,
                    line,
                })));
            }
            if lc.as_bytes() == b"assert" && is_fq_builtin && !args.is_callable_convert() {
                let mut text = b"assert(".to_vec();
                if let Some(ast::Arg::Positional(e)) = args.args.first() {
                    text.extend_from_slice(&php_syntax::export_expr_bytes(e));
                }
                text.push(b')');
                if matches!(self.vm.ini_get("zend.assertions").as_deref(), Some(b"-1")) {
                    return Ok(Expr::Lit(Value::Bool(true)));
                }
                let a = self.args(args)?;
                return Ok(Expr::Assert(Box::new(a), PhpStr::from_vec(text), line));
            }
            if matches!(
                lc.as_bytes(),
                b"compact" | b"extract" | b"get_defined_vars" | b"func_get_args" | b"func_get_arg" | b"func_num_args"
            ) {
                self.fs.flags |= code_flags::DYNAMIC_VARS;
            }
            if args.is_callable_convert() {
                let c = self.named_callee(n);
                return Ok(Expr::CallableConvert(Box::new(CallableConvert::Func(c)), line));
            }
            let c = self.named_callee(n);
            let a = self.args(args)?;
            return Ok(Expr::Call(Box::new(CallExpr {
                callee: c,
                args: a,
                line,
            })));
        }
        let ast::Callee::Expr(e) = callee else {
            unreachable!()
        };
        let f = self.expr(e)?;
        if args.is_callable_convert() {
            return Ok(Expr::CallableConvert(
                Box::new(CallableConvert::Func(Callee::Expr(Box::new(f)))),
                line,
            ));
        }
        let a = self.args(args)?;
        Ok(Expr::Call(Box::new(CallExpr {
            callee: Callee::Expr(Box::new(f)),
            args: a,
            line,
        })))
    }

    fn named_callee(&mut self, n: &ast::Name) -> Callee {
        let name = pstr(&n.resolved);
        let lc = name.to_ascii_lowercase();
        let fallback = if n.fallback {
            let f = pstr(&n.name);
            let flc = f.to_ascii_lowercase();
            Some((f, flc))
        } else {
            None
        };
        Callee::Named(Box::new(NamedCallee {
            name,
            lc,
            fallback,
            cache: OnceCell::new(),
        }))
    }

    // ----- assignment ------------------------------------------------------------

    /// `zend_ensure_writable_variable`.
    fn ensure_writable(&mut self, e: &ast::Expr) -> CResult<()> {
        match &e.kind {
            ast::ExprKind::Call { .. } => self.error("Can't use function return value in write context"),
            ast::ExprKind::Binary {
                op: ast::BinaryOp::Pipe,
                ..
            } => self.error("Can't use function return value in write context"),
            ast::ExprKind::MethodCall { .. } | ast::ExprKind::StaticCall { .. } => {
                self.error("Can't use method return value in write context")
            }
            _ if is_short_circuited(e) => self.error("Can't use nullsafe operator in write context"),
            ast::ExprKind::Var(ast::VarName::Name(n)) if n.as_bytes() == b"GLOBALS" => self.error(
                "$GLOBALS can only be modified using the $GLOBALS[$name] = $value syntax",
            ),
            _ => Ok(()),
        }
    }

    fn assign(&mut self, target: &ast::Expr, value: &ast::Expr) -> CResult<Expr> {
        if is_this(target) {
            return self.error("Cannot re-assign $this");
        }
        self.ensure_writable(target)?;
        if let ast::ExprKind::Array { items, syntax } = &target.kind {
            if *syntax == ast::ArraySyntax::Long {
                return self.error("Cannot assign to array(), use [] instead");
            }
            let pat = self.list_pattern(items, *syntax, target.line)?;
            let has_refs = list_has_refs(&pat);
            let v = if has_refs {
                if !(is_variable_like(value) || is_call(value)) {
                    return self.error("Cannot assign reference to non referenceable value");
                }
                self.ref_source(value)?
            } else {
                self.expr(value)?
            };
            return Ok(Expr::Assign(Box::new(pat), Box::new(v), target.line));
        }
        let t = self.lval(target, LvMode::Write)?;
        let v = self.expr(value)?;
        self.line = target.line;
        Ok(Expr::Assign(Box::new(t), Box::new(v), self.line))
    }

    /// `list()` / `[]` destructuring pattern.
    pub(crate) fn list_pattern(
        &mut self,
        items: &[Option<ast::ArrayItem>],
        syntax: ast::ArraySyntax,
        line: u32,
    ) -> CResult<LVal> {
        let keyed = items
            .iter()
            .flatten()
            .next()
            .is_some_and(|i| i.key.is_some());
        let mut out = Vec::with_capacity(items.len());
        let mut has_elems = false;
        for it in items {
            let Some(it) = it else {
                if keyed {
                    return self.error("Cannot use empty array entries in keyed array assignment");
                }
                out.push(None);
                continue;
            };
            self.line = it.line;
            if it.unpack {
                return self.error("Spread operator is not supported in assignments");
            }
            if keyed != it.key.is_some() {
                return self.error("Cannot mix keyed and unkeyed array entries in assignments");
            }
            has_elems = true;
            let key = match &it.key {
                Some(k) => Some(self.expr(k)?),
                None => None,
            };
            let target = match &it.value.kind {
                ast::ExprKind::Array { items, syntax: s } => {
                    if *s == ast::ArraySyntax::Long {
                        return self.error("Cannot assign to array(), use [] instead");
                    }
                    if *s != syntax {
                        return self.error("Cannot mix [] and list()");
                    }
                    self.list_pattern(items, *s, it.value.line)?
                }
                _ => {
                    if is_this(&it.value) {
                        return self.error("Cannot re-assign $this");
                    }
                    if !is_variable_like(&it.value) {
                        return self.error("Assignments can only happen to writable values");
                    }
                    self.ensure_writable(&it.value)?;
                    self.lval(&it.value, if it.by_ref { LvMode::Ref } else { LvMode::Write })?
                }
            };
            out.push(Some(ListItem {
                key,
                target,
                by_ref: it.by_ref,
            }));
        }
        if !has_elems {
            return self.error("Cannot use empty list");
        }
        Ok(LVal::List(out, line))
    }

    /// An expression evaluated for reference-taking (`=&`, by-ref array items,
    /// by-ref returns/yields): variables become `MakeRef`, calls stay calls.
    pub(crate) fn ref_source(&mut self, e: &ast::Expr) -> CResult<Expr> {
        if is_variable_like(e) && !is_this(e) {
            if let ast::ExprKind::Var(ast::VarName::Name(n)) = &e.kind {
                if n.as_bytes() == b"GLOBALS" {
                    return self.error("Cannot acquire reference to $GLOBALS");
                }
            }
            let lv = self.lval(e, LvMode::Ref)?;
            return Ok(Expr::MakeRef(Box::new(lv), e.line));
        }
        self.expr(e)
    }

    // ----- variables in write context --------------------------------------------

    /// A place in read context (`$a[1]`, `$o->p`): base fetched in R mode.
    fn lval_read(&mut self, e: &ast::Expr) -> CResult<LVal> {
        self.lval(e, LvMode::Isset)
    }

    /// Lowers a variable-like expression to a place.
    pub(crate) fn lval(&mut self, e: &ast::Expr, mode: LvMode) -> CResult<LVal> {
        self.line = e.line;
        use ast::ExprKind as K;
        match &e.kind {
            K::Var(ast::VarName::Name(n)) => {
                if n.as_bytes() == b"this" {
                    self.fs.flags |= code_flags::USES_THIS;
                    return Ok(LVal::This);
                }
                if n.as_bytes() == b"GLOBALS" {
                    if mode == LvMode::Unset {
                        return Ok(LVal::Global(Box::new(Expr::Lit(Value::str("GLOBALS")))));
                    }
                    return Ok(LVal::Temp(Box::new(Expr::Globals)));
                }
                if is_auto_global(n) {
                    return Ok(LVal::Global(Box::new(Expr::Lit(Value::String(pstr(n))))));
                }
                Ok(LVal::Var(self.cv(n)))
            }
            K::Var(ast::VarName::Expr(inner)) => {
                self.fs.flags |= code_flags::DYNAMIC_VARS;
                let name = self.expr(inner)?;
                Ok(LVal::VarVar(Box::new(name)))
            }
            K::Dim { base, index } => {
                // `$GLOBALS['x']` is the global variable `x`.
                if let K::Var(ast::VarName::Name(n)) = &base.kind {
                    if n.as_bytes() == b"GLOBALS" {
                        let Some(index) = index else {
                            return self.error("Cannot append to $GLOBALS");
                        };
                        let name = self.expr(index)?;
                        return Ok(LVal::Global(Box::new(name)));
                    }
                }
                let b = self.lval_base(base, mode)?;
                if index.is_none() {
                    if mode == LvMode::Isset {
                        return self.error("Cannot use [] for reading");
                    }
                    if mode == LvMode::Unset {
                        return self.error("Cannot use [] for unsetting");
                    }
                }
                let idx = match index {
                    Some(i) => Some(self.boxed(i)?),
                    None => None,
                };
                Ok(LVal::Dim(Box::new(b), idx, self.line))
            }
            K::Prop {
                object,
                name,
                nullsafe,
            } => {
                if *nullsafe && mode != LvMode::Isset {
                    return self.error("Can't use nullsafe operator in write context");
                }
                let b = if is_this(object) {
                    self.fs.flags |= code_flags::USES_THIS;
                    LVal::This
                } else {
                    self.lval_base(object, mode)?
                };
                let m = self.member(name)?;
                Ok(LVal::Prop(Box::new(b), m, *nullsafe, self.line))
            }
            K::StaticProp { class, name, .. } => {
                let cs = self.class_spec(class)?;
                let m = match name {
                    ast::VarName::Name(n) => Member::Name(pstr(n)),
                    ast::VarName::Expr(e) => Member::Expr(self.boxed(e)?),
                };
                Ok(LVal::StaticProp(Box::new(cs), m, self.line))
            }
            K::Call { .. } | K::MethodCall { .. } | K::StaticCall { .. } => {
                if is_short_circuited(e) && mode != LvMode::Isset {
                    return self.error("Can't use nullsafe operator in write context");
                }
                let v = self.expr_chain_base(e)?;
                Ok(LVal::Temp(Box::new(v)))
            }
            K::Array {
                syntax: ast::ArraySyntax::List,
                ..
            } if mode != LvMode::Isset => self.error("Cannot use temporary expression in write context"),
            _ => {
                if mode == LvMode::Isset {
                    let v = self.expr(e)?;
                    return Ok(LVal::Temp(Box::new(v)));
                }
                if mode == LvMode::Ref || mode == LvMode::Write || mode == LvMode::ReadWrite || mode == LvMode::Unset {
                    return self.error("Cannot use temporary expression in write context");
                }
                let v = self.expr(e)?;
                Ok(LVal::Temp(Box::new(v)))
            }
        }
    }

    /// The container operand of a dim/prop place.
    fn lval_base(&mut self, e: &ast::Expr, mode: LvMode) -> CResult<LVal> {
        use ast::ExprKind as K;
        match &e.kind {
            K::Var(_) | K::Dim { .. } | K::Prop { .. } | K::StaticProp { .. } => {
                let m = if mode == LvMode::Ref { LvMode::Write } else { mode };
                self.lval(e, m)
            }
            K::Call { .. } | K::MethodCall { .. } | K::StaticCall { .. } => {
                let v = self.expr_chain_base(e)?;
                Ok(LVal::Temp(Box::new(v)))
            }
            _ => {
                if mode != LvMode::Isset && !matches!(e.kind, K::Const(_) | K::ClassConst { .. }) {
                    // `"str"[0] = ...`, `[1][0] = ...`
                    if matches!(mode, LvMode::Write | LvMode::ReadWrite | LvMode::Unset | LvMode::Ref) {
                        return self.error("Cannot use temporary expression in write context");
                    }
                }
                if mode != LvMode::Isset && matches!(e.kind, K::Const(_) | K::ClassConst { .. }) {
                    return self.error("Cannot use temporary expression in write context");
                }
                let v = self.expr(e)?;
                Ok(LVal::Temp(Box::new(v)))
            }
        }
    }

    /// Value of a global `const` statement: a constant expression.
    pub(crate) fn const_expr_inline(&mut self, e: &ast::Expr) -> CResult<Expr> {
        self.check_const_expr(e)?;
        let saved = self.in_const_expr;
        self.in_const_expr = true;
        let r = self.expr(e);
        self.in_const_expr = saved;
        r
    }
}

fn list_has_refs(l: &LVal) -> bool {
    match l {
        LVal::List(items, _) => items.iter().flatten().any(|i| i.by_ref || list_has_refs(&i.target)),
        _ => false,
    }
}

pub(crate) fn rc_code(c: FuncCode) -> Rc<FuncCode> {
    Rc::new(c)
}
