//! AST lowering (port of the parts of `Zend/zend_compile.c` that are not
//! about name resolution, which php-syntax already did).
//!
//! [`compile_program`] turns a parsed file into a [`ir::Unit`]: function
//! bodies get compiled-variable slots, names are interned, constant
//! sub-expressions are folded into literals, and zend_compile.c's
//! compile-time errors are reported with php-src's exact messages. Like
//! php-src, top-level functions and classes whose dependencies are
//! available are declared ("early bound") while the file is compiled.

pub mod ir;

mod decl;
mod expr;

use std::collections::HashMap;
use std::rc::Rc;

use php_syntax::ast;

use crate::errors::level;
use crate::value::{PhpStr, Value};
use crate::vm::{Flow, Vm};

use ir::*;

/// A compile-time fatal error (`E_COMPILE_ERROR`).
#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub line: u32,
}

/// Why compilation stopped.
#[derive(Debug, Clone)]
pub enum LowerError {
    Compile(CompileError),
    /// A user error handler threw (or exit/fatal) while a compile-time
    /// diagnostic was being reported.
    Flow(Flow),
}

impl From<Flow> for LowerError {
    fn from(f: Flow) -> Self {
        LowerError::Flow(f)
    }
}

pub type CResult<T> = Result<T, LowerError>;

/// Options for [`compile_program`].
#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    /// Code compiled by `eval()`.
    pub is_eval: bool,
}

/// Compiles a parsed file (or eval'd code) and early-binds its top-level
/// declarations.
pub fn compile_program(
    vm: &mut Vm,
    program: &ast::Program,
    file: PhpStr,
    opts: &CompileOptions,
) -> CResult<Rc<Unit>> {
    let mut l = Lowerer::new(vm, file.clone(), opts.clone());
    l.halt_offset = program.halt_offset;
    l.strict = detect_strict_types(&program.stmts);
    let main = l.compile_main(&program.stmts)?;
    Ok(Rc::new(Unit {
        file,
        strict: l.strict,
        main,
        halt_offset: program.halt_offset,
    }))
}

/// `declare(strict_types=1)` as the first statement.
fn detect_strict_types(stmts: &[ast::Stmt]) -> bool {
    for s in stmts {
        match &s.kind {
            ast::StmtKind::Declare { directives, .. } => {
                for d in directives {
                    if d.name.eq_ignore_case(b"strict_types") {
                        if let ast::ExprKind::Int(1) = d.value.kind {
                            return true;
                        }
                    }
                }
            }
            ast::StmtKind::Nop => {}
            _ => return false,
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoopKind {
    Loop,
    Switch,
}

/// Per-function compilation state (`zend_oparray_context` + `CG(active_op_array)`).
pub(crate) struct FnState {
    kind: FuncKind,
    name: PhpStr,
    cvs: CvTable,
    flags: u32,
    num_statics: u32,
    loops: Vec<LoopKind>,
    labels: Vec<PhpStr>,
    label_ids: HashMap<PhpStr, u32>,
    /// Labels defined, with their loop-nesting path and finally depth.
    label_defs: HashMap<u32, (Vec<u32>, u32)>,
    gotos: Vec<(u32, u32, Vec<u32>, u32)>,
    /// Identifiers of the enclosing loops (for goto validation).
    loop_ids: Vec<u32>,
    next_loop_id: u32,
    finally_depth: u32,
    /// Return type (for `return` checks).
    return_type: Option<ast::TypeHint>,
    is_generator: bool,
    /// Method of a class (`$this` guaranteed if not static).
    is_method: bool,
    is_static: bool,
    /// Property hook context: property name.
    hook_prop: Option<PhpStr>,
    hook_kind: Option<HookKind>,
    /// The hook body accesses `$this->{hook_prop}` (the property is backed;
    /// `zend_property_hook_uses_property`).
    hook_uses_prop: bool,
}

impl FnState {
    fn new(kind: FuncKind, name: PhpStr) -> FnState {
        FnState {
            kind,
            name,
            cvs: CvTable::default(),
            flags: 0,
            num_statics: 0,
            loops: Vec::new(),
            labels: Vec::new(),
            label_ids: HashMap::new(),
            label_defs: HashMap::new(),
            gotos: Vec::new(),
            loop_ids: Vec::new(),
            next_loop_id: 1,
            finally_depth: 0,
            return_type: None,
            is_generator: false,
            is_method: false,
            is_static: false,
            hook_prop: None,
            hook_kind: None,
            hook_uses_prop: false,
        }
    }
}

/// Class compilation context (`CG(active_class_entry)`).
#[derive(Clone)]
pub(crate) struct ClassCtx {
    name: PhpStr,
    kind: ClassKindIr,
    parent: Option<PhpStr>,
    is_anon: bool,
}

pub(crate) struct Lowerer<'v> {
    pub(crate) vm: &'v mut Vm,
    file: PhpStr,
    dir: PhpStr,
    opts: CompileOptions,
    strict: bool,
    halt_offset: Option<usize>,
    namespace: PhpStr,
    fs: FnState,
    class: Option<ClassCtx>,
    /// `CG(zend_lineno)`.
    line: u32,
    /// Inside a constant expression (class constant, default, ...).
    in_const_expr: bool,
    /// Nesting depth of top-level statement compilation (0 = top level of file).
    depth: u32,
}

/// `zend_is_auto_global`.
pub(crate) fn is_auto_global(name: &[u8]) -> bool {
    matches!(
        name,
        b"GLOBALS" | b"_GET" | b"_POST" | b"_COOKIE" | b"_SERVER" | b"_ENV" | b"_REQUEST" | b"_FILES"
    )
}

pub(crate) fn err<T>(msg: impl Into<String>, line: u32) -> CResult<T> {
    Err(LowerError::Compile(CompileError {
        message: msg.into(),
        line,
    }))
}

pub(crate) fn pstr(b: &[u8]) -> PhpStr {
    PhpStr::from_bytes(b)
}

impl<'v> Lowerer<'v> {
    fn new(vm: &'v mut Vm, file: PhpStr, opts: CompileOptions) -> Lowerer<'v> {
        let dir = dirname(&file);
        Lowerer {
            vm,
            file,
            dir,
            opts,
            strict: false,
            halt_offset: None,
            namespace: PhpStr::new(),
            fs: FnState::new(FuncKind::Main, PhpStr::new()),
            class: None,
            line: 0,
            in_const_expr: false,
            depth: 0,
        }
    }

    pub(crate) fn error<T>(&self, msg: impl Into<String>) -> CResult<T> {
        err(msg, self.line)
    }

    /// Emits a non-fatal compile-time diagnostic at the current line.
    pub(crate) fn diag(&mut self, ty: i64, msg: impl Into<PhpStr>) -> CResult<()> {
        let file = self.file.clone();
        let line = self.line;
        self.vm.zend_error_at(ty, file, line, msg.into())?;
        Ok(())
    }

    /// `lookup_cv`.
    pub(crate) fn cv(&mut self, name: &[u8]) -> Slot {
        if let Some(s) = self.fs.cvs.lookup(name) {
            return s;
        }
        let s = self.fs.cvs.names.len() as Slot;
        let n = pstr(name);
        self.fs.cvs.names.push(n.clone());
        self.fs.cvs.index.insert(n, s);
        s
    }

    fn compile_main(&mut self, stmts: &[ast::Stmt]) -> CResult<Rc<FuncCode>> {
        if self.strict {
            self.fs.flags |= code_flags::STRICT;
        }
        let body = self.top_block(stmts)?;
        self.finish_gotos()?;
        let fs = std::mem::replace(&mut self.fs, FnState::new(FuncKind::Main, PhpStr::new()));
        Ok(Rc::new(FuncCode {
            name: PhpStr::new(),
            kind: FuncKind::Main,
            flags: fs.flags | code_flags::DYNAMIC_VARS,
            cvs: Rc::new(fs.cvs),
            params: Vec::new(),
            body,
            return_type: None,
            file: self.file.clone(),
            start_line: 1,
            end_line: self.line,
            uses: Vec::new(),
            num_statics: fs.num_statics,
            class_name: None,
            in_trait: false,
            doc_comment: None,
            attributes: Vec::new(),
            labels: fs.labels,
            namespace: self.namespace.clone(),
        }))
    }

    // ----- statements ----------------------------------------------------------

    /// Top-level statement list of a file (handles early binding).
    fn top_block(&mut self, stmts: &[ast::Stmt]) -> CResult<Block> {
        let mut out = Vec::with_capacity(stmts.len());
        for (i, s) in stmts.iter().enumerate() {
            if let ast::StmtKind::HaltCompiler { .. } = s.kind {
                out.push(Stmt {
                    kind: StmtKind::HaltCompiler,
                    line: s.line,
                });
                break;
            }
            self.top_stmt(s, i == 0 || stmts[..i].iter().all(is_declare_or_nop), &mut out)?;
        }
        Ok(out)
    }

    fn top_stmt(&mut self, s: &ast::Stmt, first: bool, out: &mut Block) -> CResult<()> {
        self.line = s.line;
        match &s.kind {
            ast::StmtKind::Function(f) if self.depth == 0 => {
                let def = self.function_decl(f)?;
                // Early binding: declared while compiling.
                self.line = f.common.start_line;
                crate::interp::decl::declare_function_compile_time(self, &def)?;
                self.line = f.common.end_line;
                out.push(Stmt {
                    kind: StmtKind::Nop,
                    line: s.line,
                });
                Ok(())
            }
            ast::StmtKind::Class(c) if self.depth == 0 => {
                let def = self.class_decl(c, true)?;
                self.line = c.end_line;
                if crate::interp::decl::try_early_bind(self, &def)? {
                    out.push(Stmt {
                        kind: StmtKind::Nop,
                        line: s.line,
                    });
                } else {
                    out.push(Stmt {
                        kind: StmtKind::ClassDecl(def),
                        line: s.line,
                    });
                }
                Ok(())
            }
            ast::StmtKind::Namespace { name, body } => {
                self.namespace = name.as_ref().map(|n| pstr(n)).unwrap_or_default();
                if let Some(body) = body {
                    for (i, s) in body.iter().enumerate() {
                        if let ast::StmtKind::HaltCompiler { .. } = s.kind {
                            out.push(Stmt {
                                kind: StmtKind::HaltCompiler,
                                line: s.line,
                            });
                            break;
                        }
                        let first = body[..i].iter().all(is_declare_or_nop);
                        self.top_stmt(s, first, out)?;
                    }
                    self.namespace = PhpStr::new();
                }
                Ok(())
            }
            ast::StmtKind::Declare { directives, body } => {
                self.declare(directives, body.as_ref(), first, s.line, out)
            }
            _ => {
                let st = self.stmt(s)?;
                out.push(st);
                Ok(())
            }
        }
    }

    fn declare(
        &mut self,
        directives: &[ast::DeclareDirective],
        body: Option<&ast::Block>,
        first: bool,
        line: u32,
        out: &mut Block,
    ) -> CResult<()> {
        let mut ticks: Option<u32> = None;
        for d in directives {
            self.line = d.line;
            let lit = match &d.value.kind {
                ast::ExprKind::Int(_)
                | ast::ExprKind::Float(_)
                | ast::ExprKind::String(_)
                | ast::ExprKind::Const(_) => true,
                ast::ExprKind::Unary { op, expr } => {
                    matches!(op, ast::UnaryOp::Minus | ast::UnaryOp::Plus)
                        && matches!(expr.kind, ast::ExprKind::Int(_) | ast::ExprKind::Float(_))
                }
                _ => false,
            };
            if !lit {
                return self.error(format!("declare({}) value must be a literal", d.name));
            }
            if d.name.eq_ignore_case(b"ticks") {
                let v = match &d.value.kind {
                    ast::ExprKind::Int(i) => *i,
                    ast::ExprKind::String(s) => crate::value::convert::string_to_int(s),
                    ast::ExprKind::Float(f) => *f as i64,
                    _ => 0,
                };
                ticks = Some(v.max(0) as u32);
            } else if d.name.eq_ignore_case(b"encoding") {
                if !first || self.depth > 0 {
                    return self.error(
                        "Encoding declaration pragma must be the very first statement in the script",
                    );
                }
            } else if d.name.eq_ignore_case(b"strict_types") {
                if !first || self.depth > 0 || self.fs.kind != FuncKind::Main {
                    return self.error(
                        "strict_types declaration must be the very first statement in the script",
                    );
                }
                if body.is_some() {
                    return self.error("strict_types declaration must not use block mode");
                }
                if !matches!(d.value.kind, ast::ExprKind::Int(0) | ast::ExprKind::Int(1)) {
                    return self.error("strict_types declaration must have 0 or 1 as its value");
                }
            } else {
                self.diag(
                    level::E_COMPILE_WARNING,
                    format!("Unsupported declare '{}'", d.name),
                )?;
            }
        }
        if let Some(body) = body {
            let inner = if self.depth == 0 && self.fs.kind == FuncKind::Main {
                let mut b = Vec::new();
                for s in body {
                    self.top_stmt(s, false, &mut b)?;
                }
                b
            } else {
                self.block(body)?
            };
            match ticks {
                Some(n) if n > 0 => out.push(Stmt {
                    kind: StmtKind::Ticks(n, inner),
                    line,
                }),
                _ => out.push(Stmt {
                    kind: StmtKind::Block(inner),
                    line,
                }),
            }
        } else if let Some(n) = ticks {
            // `declare(ticks=N);` applies to the rest of the file: handled by
            // wrapping subsequent statements at runtime.
            out.push(Stmt {
                kind: StmtKind::Ticks(n, Vec::new()),
                line,
            });
        }
        Ok(())
    }

    pub(crate) fn block(&mut self, stmts: &[ast::Stmt]) -> CResult<Block> {
        self.depth += 1;
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            if let ast::StmtKind::Declare { directives, body } = &s.kind {
                self.declare(directives, body.as_ref(), false, s.line, &mut out)?;
                continue;
            }
            if let ast::StmtKind::HaltCompiler { .. } = s.kind {
                self.line = s.line;
                self.depth -= 1;
                return self.error("__HALT_COMPILER() can only be used from the outermost scope");
            }
            let st = self.stmt(s)?;
            out.push(st);
        }
        self.depth -= 1;
        Ok(out)
    }

    fn loop_body(&mut self, body: &[ast::Stmt], kind: LoopKind) -> CResult<Block> {
        self.fs.loops.push(kind);
        let id = self.fs.next_loop_id;
        self.fs.next_loop_id += 1;
        self.fs.loop_ids.push(id);
        let r = self.block(body);
        self.fs.loop_ids.pop();
        self.fs.loops.pop();
        r
    }

    pub(crate) fn stmt(&mut self, s: &ast::Stmt) -> CResult<Stmt> {
        self.line = s.line;
        let line = s.line;
        let kind = match &s.kind {
            ast::StmtKind::Expr(e) => {
                if let ast::ExprKind::VoidCast(inner) = &e.kind {
                    StmtKind::Expr(Expr::Void(Box::new(self.expr(inner)?)))
                } else {
                    StmtKind::Expr(self.expr_stmt(e)?)
                }
            }
            ast::StmtKind::Echo(es) => {
                let mut v = Vec::with_capacity(es.len());
                for e in es {
                    v.push(self.expr(e)?);
                }
                StmtKind::Echo(v)
            }
            ast::StmtKind::InlineHtml(h) => StmtKind::InlineHtml(pstr(h)),
            ast::StmtKind::If(i) => {
                let mut branches = Vec::with_capacity(i.branches.len());
                for b in &i.branches {
                    self.line = b.line;
                    let c = self.expr(&b.cond)?;
                    let body = self.block(&b.body)?;
                    branches.push((c, body));
                }
                let els = match &i.else_branch {
                    Some(b) => Some(self.block(b)?),
                    None => None,
                };
                StmtKind::If(branches, els)
            }
            ast::StmtKind::While { cond, body } => {
                let c = self.expr(cond)?;
                let b = self.loop_body(body, LoopKind::Loop)?;
                StmtKind::While(c, b)
            }
            ast::StmtKind::DoWhile { body, cond } => {
                let b = self.loop_body(body, LoopKind::Loop)?;
                let c = self.expr(cond)?;
                StmtKind::DoWhile(b, c)
            }
            ast::StmtKind::For(f) => {
                let init = self.expr_list(&f.init)?;
                let cond = self.expr_list(&f.cond)?;
                let step = self.expr_list(&f.step)?;
                let body = self.loop_body(&f.body, LoopKind::Loop)?;
                StmtKind::For(Box::new(ForStmt {
                    init,
                    cond,
                    step,
                    body,
                }))
            }
            ast::StmtKind::Foreach(f) => self.foreach(f, line)?,
            ast::StmtKind::Switch { subject, cases } => {
                let subj = self.expr(subject)?;
                let mut out = Vec::with_capacity(cases.len());
                let mut default = None;
                self.fs.loops.push(LoopKind::Switch);
                let id = self.fs.next_loop_id;
                self.fs.next_loop_id += 1;
                self.fs.loop_ids.push(id);
                for (i, c) in cases.iter().enumerate() {
                    self.line = c.line;
                    if c.cond.is_none() {
                        if default.is_some() {
                            self.fs.loops.pop();
                            self.fs.loop_ids.pop();
                            return self.error("Switch statements may only contain one default clause");
                        }
                        default = Some(i);
                    }
                    if c.semicolon {
                        self.diag(
                            level::E_DEPRECATED,
                            "Case statements followed by a semicolon (;) are deprecated, use a colon (:) instead",
                        )?;
                    }
                    let cond = match &c.cond {
                        Some(e) => Some(self.expr(e)?),
                        None => None,
                    };
                    let body = self.block(&c.body)?;
                    out.push(SwitchCase {
                        cond,
                        body,
                        line: c.line,
                    });
                }
                self.fs.loop_ids.pop();
                self.fs.loops.pop();
                StmtKind::Switch(Box::new(SwitchStmt {
                    subject: subj,
                    cases: out,
                    default,
                }))
            }
            ast::StmtKind::Break(e) => self.break_continue(true, e.as_ref())?,
            ast::StmtKind::Continue(e) => self.break_continue(false, e.as_ref())?,
            ast::StmtKind::Return(e) => self.return_stmt(e.as_ref())?,
            ast::StmtKind::Global(vars) => {
                let mut out = Vec::with_capacity(vars.len());
                for v in vars {
                    self.line = v.line;
                    match &v.kind {
                        ast::ExprKind::Var(ast::VarName::Name(n)) => {
                            if n.as_bytes() == b"this" {
                                return self.error("Cannot use $this as global variable");
                            }
                            if is_auto_global(n) {
                                // `global $_SERVER;` is a no-op in php-src.
                                continue;
                            }
                            let slot = self.cv(n);
                            out.push(GlobalVar::Name(pstr(n), slot));
                        }
                        ast::ExprKind::Var(ast::VarName::Expr(e)) => {
                            self.fs.flags |= code_flags::DYNAMIC_VARS;
                            out.push(GlobalVar::Dynamic(self.expr(e)?));
                        }
                        _ => return self.error("Cannot use $this as global variable"),
                    }
                }
                StmtKind::Global(out)
            }
            ast::StmtKind::Static(vars) => {
                let mut out = Vec::with_capacity(vars.len());
                for v in vars {
                    self.line = v.line;
                    if v.name.as_bytes() == b"this" {
                        return self.error("Cannot use $this as static variable");
                    }
                    let slot = self.cv(&v.name);
                    let index = self.fs.num_statics;
                    self.fs.num_statics += 1;
                    let init = match &v.default {
                        Some(e) => Some(self.expr(e)?),
                        None => None,
                    };
                    out.push(StaticVarDecl {
                        slot,
                        index,
                        init,
                        name: pstr(&v.name),
                    });
                }
                StmtKind::Static(out)
            }
            ast::StmtKind::Unset(vars) => {
                let mut out = Vec::with_capacity(vars.len());
                for v in vars {
                    self.line = v.line;
                    out.push(self.unset_target(v)?);
                }
                StmtKind::Unset(out)
            }
            ast::StmtKind::Block(b) => StmtKind::Block(self.block(b)?),
            ast::StmtKind::Try(t) => self.try_stmt(t)?,
            ast::StmtKind::Goto(label) => {
                let id = self.label_id(label);
                let path = self.fs.loop_ids.clone();
                let fd = self.fs.finally_depth;
                self.fs.gotos.push((id, line, path, fd));
                StmtKind::Goto(id)
            }
            ast::StmtKind::Label(label) => {
                let id = self.label_id(label);
                if self.fs.label_defs.contains_key(&id) {
                    return self.error(format!("Label '{label}' already defined"));
                }
                self.fs
                    .label_defs
                    .insert(id, (self.fs.loop_ids.clone(), self.fs.finally_depth));
                StmtKind::Label(id)
            }
            ast::StmtKind::Function(f) => {
                let def = self.function_decl(f)?;
                self.line = f.common.end_line;
                StmtKind::FuncDecl(def)
            }
            ast::StmtKind::Class(c) => {
                if self.fs.kind != FuncKind::Main || self.class.is_some() {
                    // php-src allows class declarations inside functions.
                }
                let def = self.class_decl(c, false)?;
                StmtKind::ClassDecl(def)
            }
            ast::StmtKind::Const(c) => {
                let mut out = Vec::with_capacity(c.consts.len());
                for e in &c.consts {
                    self.line = e.line;
                    let v = self.const_expr_inline(&e.value)?;
                    let name = if e.fq_name.is_empty() {
                        pstr(&e.name)
                    } else {
                        pstr(&e.fq_name)
                    };
                    out.push((name, v));
                }
                StmtKind::Const(out)
            }
            ast::StmtKind::Namespace { name, body } => {
                self.namespace = name.as_ref().map(|n| pstr(n)).unwrap_or_default();
                match body {
                    Some(b) => StmtKind::Block(self.block(b)?),
                    None => StmtKind::Nop,
                }
            }
            ast::StmtKind::Use { .. } | ast::StmtKind::GroupUse { .. } => StmtKind::Nop,
            ast::StmtKind::Declare { directives, body } => {
                let mut out = Vec::new();
                self.declare(directives, body.as_ref(), false, line, &mut out)?;
                StmtKind::Block(out)
            }
            ast::StmtKind::HaltCompiler { .. } => StmtKind::HaltCompiler,
            ast::StmtKind::Nop => StmtKind::Nop,
        };
        Ok(Stmt { kind, line })
    }

    fn expr_list(&mut self, es: &[ast::Expr]) -> CResult<Vec<Expr>> {
        let mut v = Vec::with_capacity(es.len());
        for e in es {
            if let ast::ExprKind::VoidCast(inner) = &e.kind {
                v.push(Expr::Void(Box::new(self.expr(inner)?)));
            } else {
                v.push(self.expr_stmt(e)?);
            }
        }
        Ok(v)
    }

    fn label_id(&mut self, label: &[u8]) -> u32 {
        let name = pstr(label).to_ascii_lowercase();
        if let Some(&id) = self.fs.label_ids.get(&name) {
            return id;
        }
        let id = self.fs.labels.len() as u32;
        self.fs.labels.push(pstr(label));
        self.fs.label_ids.insert(name, id);
        id
    }

    /// `zend_resolve_goto_label` checks, done once the function is compiled.
    fn finish_gotos(&mut self) -> CResult<()> {
        let gotos = std::mem::take(&mut self.fs.gotos);
        for (id, line, path, fdepth) in gotos {
            let name = self.fs.labels[id as usize].clone();
            let Some((def_path, def_fdepth)) = self.fs.label_defs.get(&id).cloned() else {
                return err(format!("'goto' to undefined label '{name}'"), line);
            };
            // The label's loop path must be a prefix of the goto's path.
            if def_path.len() > path.len() || def_path[..] != path[..def_path.len()] {
                return err("'goto' into loop or switch statement is disallowed", line);
            }
            if def_fdepth > fdepth {
                return err("jump into a finally block is disallowed", line);
            }
            if def_fdepth < fdepth {
                return err("jump out of a finally block is disallowed", line);
            }
        }
        Ok(())
    }

    fn break_continue(&mut self, is_break: bool, e: Option<&ast::Expr>) -> CResult<StmtKind> {
        let name = if is_break { "break" } else { "continue" };
        let depth = match e {
            None => 1,
            Some(e) => match &e.kind {
                ast::ExprKind::Int(i) => {
                    if *i < 1 {
                        return self.error(format!("'{name}' operator accepts only positive integers"));
                    }
                    *i as u32
                }
                _ => {
                    return self.error(format!(
                        "'{name}' operator with non-integer operand is no longer supported"
                    ));
                }
            },
        };
        let n = self.fs.loops.len() as u32;
        if n == 0 {
            return self.error(format!("'{name}' not in the 'loop' or 'switch' context"));
        }
        if depth > n {
            return self.error(format!(
                "Cannot '{name}' {depth} level{}",
                if depth == 1 { "" } else { "s" }
            ));
        }
        if !is_break {
            let target = self.fs.loops[(n - depth) as usize];
            if target == LoopKind::Switch {
                if depth == 1 {
                    let msg = if self.fs.loops[..(n - depth) as usize]
                        .iter()
                        .any(|k| *k == LoopKind::Loop)
                    {
                        "\"continue\" targeting switch is equivalent to \"break\". Did you mean to use \"continue 2\"?"
                            .to_string()
                    } else {
                        "\"continue\" targeting switch is equivalent to \"break\"".to_string()
                    };
                    self.diag(level::E_COMPILE_WARNING, msg)?;
                } else {
                    self.diag(
                        level::E_COMPILE_WARNING,
                        format!(
                            "\"continue {depth}\" targeting switch is equivalent to \"break {depth}\". Did you mean to use \"continue {}\"?",
                            depth + 1
                        ),
                    )?;
                }
            }
        }
        Ok(if is_break {
            StmtKind::Break(depth)
        } else {
            StmtKind::Continue(depth)
        })
    }

    fn return_stmt(&mut self, e: Option<&ast::Expr>) -> CResult<StmtKind> {
        if !self.fs.is_generator {
            if let Some(rt) = &self.fs.return_type {
                let what = if self.class.is_some() && self.fs.kind == FuncKind::Method {
                    "method"
                } else {
                    "function"
                };
                if decl::type_is_void(rt) {
                    if let Some(e) = e {
                        let is_null = matches!(&e.kind, ast::ExprKind::Const(n) if n.name.eq_ignore_case(b"null"));
                        return self.error(if is_null {
                            format!("A void {what} must not return a value (did you mean \"return;\" instead of \"return null;\"?)")
                        } else {
                            format!("A void {what} must not return a value")
                        });
                    }
                } else if decl::type_is_never(rt) {
                    return self.error(format!("A never-returning {what} must not return"));
                } else if e.is_none() && self.fs.kind != FuncKind::Main {
                    return self.error(format!("A {what} with return type must return a value"));
                }
            }
        }
        let v = match e {
            None => None,
            Some(e) => {
                if self.fs.flags & code_flags::RETURNS_REF != 0 && is_variable_like(e) {
                    Some(self.ref_source(e)?)
                } else {
                    Some(self.expr(e)?)
                }
            }
        };
        Ok(StmtKind::Return(v))
    }

    fn try_stmt(&mut self, t: &ast::TryStmt) -> CResult<StmtKind> {
        if t.catches.is_empty() && t.finally.is_none() {
            return self.error("Cannot use try without catch or finally");
        }
        let body = self.block(&t.body)?;
        let mut catches = Vec::with_capacity(t.catches.len());
        for c in &t.catches {
            self.line = c.line;
            let mut types = Vec::with_capacity(c.types.len());
            for n in &c.types {
                types.push(self.class_name_ref(n)?);
            }
            let var = match &c.var {
                Some(v) => {
                    if v.as_bytes() == b"this" {
                        return self.error("Cannot re-assign $this");
                    }
                    Some(self.cv(v))
                }
                None => None,
            };
            let body = self.block(&c.body)?;
            catches.push(CatchClause {
                types,
                var,
                body,
                line: c.line,
            });
        }
        let finally = match &t.finally {
            Some(f) => {
                self.fs.finally_depth += 1;
                let b = self.block(f);
                self.fs.finally_depth -= 1;
                Some(b?)
            }
            None => None,
        };
        Ok(StmtKind::Try(Box::new(TryStmt {
            body,
            catches,
            finally,
        })))
    }

    fn foreach(&mut self, f: &ast::ForeachStmt, line: u32) -> CResult<StmtKind> {
        let by_ref = f.value.by_ref;
        if let Some(k) = &f.key {
            if k.by_ref {
                return self.error("Key element cannot be a reference");
            }
            if let ast::ExprKind::Array { .. } = k.expr.kind {
                return self.error("Cannot use list as key element");
            }
        }
        let expr = if by_ref && is_variable_like(&f.expr) {
            if let ast::ExprKind::Var(ast::VarName::Name(n)) = &f.expr.kind {
                if n.as_bytes() == b"GLOBALS" {
                    return self.error("Cannot acquire reference to $GLOBALS");
                }
            }
            Expr::Lit(Value::Null)
        } else {
            self.expr(&f.expr)?
        };
        let subject_place = if by_ref && is_variable_like(&f.expr) {
            Some(self.lval(&f.expr, LvMode::Write)?)
        } else {
            None
        };
        self.line = f.value.expr.line;
        let value = match &f.value.expr.kind {
            ast::ExprKind::Array { items, syntax } => {
                if *syntax == ast::ArraySyntax::Long {
                    return self.error("Cannot assign to array(), use [] instead");
                }
                self.list_pattern(items, *syntax, f.value.expr.line)?
            }
            _ => {
                if is_this(&f.value.expr) {
                    return self.error("Cannot re-assign $this");
                }
                self.lval(&f.value.expr, LvMode::Write)?
            }
        };
        let key = match &f.key {
            Some(k) => {
                if is_this(&k.expr) {
                    return self.error("Cannot re-assign $this");
                }
                Some(self.lval(&k.expr, LvMode::Write)?)
            }
            None => None,
        };
        let body = self.loop_body(&f.body, LoopKind::Loop)?;
        Ok(StmtKind::Foreach(Box::new(ForeachStmt {
            expr,
            subject_place,
            key,
            value,
            by_ref,
            body,
            line,
        })))
    }

    fn unset_target(&mut self, e: &ast::Expr) -> CResult<LVal> {
        if is_this(e) {
            return self.error("Cannot unset $this");
        }
        match &e.kind {
            ast::ExprKind::Dim { index: None, .. } => self.error("Cannot use [] for unsetting"),
            ast::ExprKind::Var(ast::VarName::Name(n)) if n.as_bytes() == b"GLOBALS" => {
                // unset($GLOBALS) is a compile-time no-op error-free in php-src? It
                // is compiled as an unset of the auto-global.
                Ok(LVal::Global(Box::new(Expr::Lit(Value::str("GLOBALS")))))
            }
            _ => self.lval(e, LvMode::Unset),
        }
    }
}

fn is_declare_or_nop(s: &ast::Stmt) -> bool {
    matches!(s.kind, ast::StmtKind::Declare { .. } | ast::StmtKind::Nop)
}

pub(crate) fn is_this(e: &ast::Expr) -> bool {
    matches!(&e.kind, ast::ExprKind::Var(ast::VarName::Name(n)) if n.as_bytes() == b"this")
}

/// `zend_is_variable` (+ calls for by-ref contexts).
pub(crate) fn is_variable_like(e: &ast::Expr) -> bool {
    matches!(
        e.kind,
        ast::ExprKind::Var(_)
            | ast::ExprKind::Dim { .. }
            | ast::ExprKind::Prop { .. }
            | ast::ExprKind::StaticProp { .. }
    )
}

pub(crate) fn is_call(e: &ast::Expr) -> bool {
    matches!(
        e.kind,
        ast::ExprKind::Call { .. } | ast::ExprKind::MethodCall { .. } | ast::ExprKind::StaticCall { .. }
    )
}

/// `dirname()` of a script path (for `__DIR__`).
pub fn dirname(path: &[u8]) -> PhpStr {
    match path.iter().rposition(|&c| c == b'/') {
        Some(0) => PhpStr::from("/"),
        Some(i) => PhpStr::from_bytes(&path[..i]),
        None => PhpStr::from("."),
    }
}

/// Lvalue lowering mode (`BP_VAR_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LvMode {
    Write,
    ReadWrite,
    Unset,
    Isset,
    Ref,
}
