//! Name resolution, ported from `Zend/zend_compile.c`.
//!
//! Walks the tree in compilation order (php-src compiles top-level
//! statements, including function bodies, in source order) and fills in
//! [`Name::resolved`] / [`Name::fallback`] and the `fq_name` of
//! declarations, applying:
//!
//! * `zend_resolve_class_name()` — `self`/`parent`/`static` stay as-is,
//!   `use` aliases (case-insensitive) and the current namespace apply;
//! * `zend_resolve_function_name()` / `zend_resolve_const_name()` —
//!   unqualified names in a namespace that are not imported resolve to
//!   `ns\name` with a runtime fallback to the global `name`;
//! * the compile-time errors of `use`, `namespace` and declarations
//!   (`Cannot use A\B as B because the name is already in use`, ...),
//!   reported as [`ParseErrorKind::Compile`] errors.

use std::collections::HashMap;

use crate::ast::*;
use crate::{Diagnostic, DiagnosticLevel, ParseError, ParseErrorKind};

/// Resolve all names in `program` in place.
pub fn resolve(program: &mut Program) -> Result<(), ParseError> {
    resolve_stmts(&mut program.stmts, &mut program.diagnostics)
}

/// Resolve names in a file's top-level statement list, appending
/// compile-time diagnostics (warnings/deprecations) to `diagnostics`.
pub fn resolve_stmts(
    stmts: &mut [Stmt],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ParseError> {
    let mut r = Resolver {
        ns: None,
        imports: HashMap::new(),
        imports_function: HashMap::new(),
        imports_const: HashMap::new(),
        seen: HashMap::new(),
        in_namespace: false,
        has_bracketed_namespaces: false,
        diagnostics,
    };
    let firsts = first_statement_flags(stmts);
    for (i, s) in stmts.iter_mut().enumerate() {
        r.top_stmt(s, firsts[i])?;
    }
    Ok(())
}

/// For each top-level statement: is it "the first statement" in the sense
/// of `zend_is_first_statement(ast, allow_nop = true)` (only `declare` and
/// empty statements before it)?
fn first_statement_flags(stmts: &[Stmt]) -> Vec<bool> {
    let mut out = Vec::with_capacity(stmts.len());
    let mut ok = true;
    for s in stmts {
        out.push(ok);
        if !matches!(s.kind, StmtKind::Declare { .. } | StmtKind::Nop) {
            ok = false;
        }
    }
    out
}

const SYMBOL_CLASS: u8 = 1;
const SYMBOL_FUNCTION: u8 = 2;
const SYMBOL_CONST: u8 = 4;

struct Resolver<'d> {
    /// `FC(current_namespace)`
    ns: Option<ByteStr>,
    /// `FC(imports)`: lowercase alias → name.
    imports: HashMap<Vec<u8>, ByteStr>,
    /// `FC(imports_function)`: lowercase alias → name.
    imports_function: HashMap<Vec<u8>, ByteStr>,
    /// `FC(imports_const)`: case-sensitive alias → name.
    imports_const: HashMap<Vec<u8>, ByteStr>,
    /// `FC(seen_symbols)`
    seen: HashMap<Vec<u8>, u8>,
    in_namespace: bool,
    has_bracketed_namespaces: bool,
    diagnostics: &'d mut Vec<Diagnostic>,
}

type RResult = Result<(), ParseError>;

fn fatal(msg: String, line: Line) -> ParseError {
    ParseError {
        kind: ParseErrorKind::Compile,
        message: msg,
        line,
        diagnostics: Vec::new(),
    }
}

fn lower(s: &[u8]) -> Vec<u8> {
    s.to_ascii_lowercase()
}

fn concat(a: &[u8], b: &[u8]) -> ByteStr {
    let mut v = Vec::with_capacity(a.len() + 1 + b.len());
    v.extend_from_slice(a);
    v.push(b'\\');
    v.extend_from_slice(b);
    ByteStr::from(v)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The part after the last `\`.
fn unqualified(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&c| c == b'\\') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// `zend_is_reserved_class_name()`
fn is_reserved_class_name(name: &[u8]) -> bool {
    const RESERVED: [&[u8]; 17] = [
        b"null",
        b"true",
        b"false",
        b"int",
        b"float",
        b"string",
        b"bool",
        b"void",
        b"never",
        b"iterable",
        b"object",
        b"mixed",
        b"array",
        b"callable",
        b"static",
        b"self",
        b"parent",
    ];
    let u = unqualified(name);
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(u))
}

fn use_type_str(kind: UseKind) -> &'static str {
    match kind {
        UseKind::Class => "",
        UseKind::Function => " function",
        UseKind::Const => " const",
    }
}

impl Resolver<'_> {
    fn diag(&mut self, level: DiagnosticLevel, message: String, line: Line) {
        self.diagnostics.push(Diagnostic {
            level,
            message,
            line,
        });
    }

    fn prefix_with_ns(&self, name: &[u8]) -> ByteStr {
        match &self.ns {
            Some(ns) => concat(ns, name),
            None => ByteStr::from(name),
        }
    }

    fn reset_imports(&mut self) {
        self.imports.clear();
        self.imports_function.clear();
        self.imports_const.clear();
        self.seen.clear();
    }

    fn register_seen(&mut self, name: Vec<u8>, kind: u8) {
        *self.seen.entry(name).or_insert(0) |= kind;
    }

    fn have_seen(&self, name: &[u8], kind: u8) -> bool {
        self.seen.get(name).is_some_and(|k| k & kind != 0)
    }

    // ---------------------------------------------------------------------
    // Name resolution
    // ---------------------------------------------------------------------

    /// `zend_resolve_class_name()`
    fn class_name(&self, n: &mut Name) -> RResult {
        if special_class(&n.name).is_some() {
            match n.kind {
                NameKind::FullyQualified => {
                    return Err(fatal(
                        format!("'\\{}' is an invalid class name", lossy(&n.name)),
                        n.line,
                    ));
                }
                NameKind::Relative => {
                    return Err(fatal(
                        format!("'namespace\\{}' is an invalid class name", lossy(&n.name)),
                        n.line,
                    ));
                }
                NameKind::Unqualified => {
                    n.resolved = n.name.clone();
                    n.fallback = false;
                    return Ok(());
                }
                NameKind::Qualified => {}
            }
        }
        n.fallback = false;
        n.resolved = match n.kind {
            NameKind::Relative => self.prefix_with_ns(&n.name),
            NameKind::FullyQualified => n.name.clone(),
            NameKind::Qualified => {
                let i = n
                    .name
                    .iter()
                    .position(|&c| c == b'\\')
                    .expect("qualified name");
                match self.imports.get(&lower(&n.name[..i])) {
                    Some(import) => concat(import, &n.name[i + 1..]),
                    None => self.prefix_with_ns(&n.name),
                }
            }
            NameKind::Unqualified => match self.imports.get(&lower(&n.name)) {
                Some(import) => import.clone(),
                None => self.prefix_with_ns(&n.name),
            },
        };
        Ok(())
    }

    /// `zend_resolve_const_class_name_reference()`: like
    /// [`Self::class_name`] but `self`/`parent`/`static` are rejected.
    fn const_class_ref(&self, n: &mut Name, what: &str) -> RResult {
        if n.kind != NameKind::FullyQualified && special_class(&n.name).is_some() {
            return Err(fatal(
                format!(
                    "Cannot use \"{}\" as {what}, as it is reserved",
                    lossy(&n.name)
                ),
                n.line,
            ));
        }
        self.class_name(n)
    }

    /// `zend_resolve_non_class_name()`; returns whether the name is fully
    /// qualified.
    fn non_class_name(&self, n: &mut Name, kind: UseKind) -> bool {
        let (resolved, fq) = match n.kind {
            NameKind::FullyQualified => (n.name.clone(), true),
            NameKind::Relative => (self.prefix_with_ns(&n.name), true),
            NameKind::Unqualified | NameKind::Qualified => {
                let import = match kind {
                    UseKind::Const => self.imports_const.get(n.name.as_bytes()),
                    _ => self.imports_function.get(&lower(&n.name)),
                };
                if let (Some(import), NameKind::Unqualified) = (import, n.kind) {
                    (import.clone(), true)
                } else if n.kind == NameKind::Qualified {
                    let i = n
                        .name
                        .iter()
                        .position(|&c| c == b'\\')
                        .expect("qualified name");
                    match self.imports.get(&lower(&n.name[..i])) {
                        Some(import) => (concat(import, &n.name[i + 1..]), true),
                        None => (self.prefix_with_ns(&n.name), true),
                    }
                } else {
                    (self.prefix_with_ns(&n.name), false)
                }
            }
        };
        n.resolved = resolved;
        n.fallback = !fq && self.ns.is_some();
        fq
    }

    fn function_name(&self, n: &mut Name) {
        self.non_class_name(n, UseKind::Function);
    }

    fn const_name(&self, n: &mut Name) {
        let fq = self.non_class_name(n, UseKind::Const);
        // `true`, `false` and `null` are substituted at compile time even
        // when unqualified inside a namespace (zend_try_ct_eval_const()).
        if !fq {
            let u = unqualified(&n.resolved);
            if [&b"true"[..], b"false", b"null"]
                .iter()
                .any(|s| s.eq_ignore_ascii_case(u))
            {
                n.resolved = n.name.clone();
                n.fallback = false;
            }
        }
    }

    // ---------------------------------------------------------------------
    // Statements
    // ---------------------------------------------------------------------

    /// `zend_compile_top_stmt()`
    fn top_stmt(&mut self, s: &mut Stmt, is_first: bool) -> RResult {
        let line = s.line;
        match &mut s.kind {
            StmtKind::Namespace { name, body } => {
                self.namespace(name.clone(), body.as_mut(), is_first, line)?;
            }
            StmtKind::HaltCompiler { .. } => {
                if self.has_bracketed_namespaces && self.in_namespace {
                    return Err(fatal(
                        "__HALT_COMPILER() can only be used from the outermost scope".into(),
                        line,
                    ));
                }
            }
            // An empty statement is a NULL node, skipped by zend_compile_top_stmt().
            StmtKind::Nop => {}
            _ => {
                self.stmt(s)?;
                if self.has_bracketed_namespaces && !self.in_namespace {
                    return Err(fatal(
                        "No code may exist outside of namespace {}".into(),
                        line,
                    ));
                }
            }
        }
        Ok(())
    }

    /// `zend_compile_namespace()`
    fn namespace(
        &mut self,
        name: Option<ByteStr>,
        body: Option<&mut Block>,
        is_first: bool,
        line: Line,
    ) -> RResult {
        let with_bracket = body.is_some();
        if !self.has_bracketed_namespaces {
            if self.ns.is_some() && with_bracket {
                return Err(fatal(
                    "Cannot mix bracketed namespace declarations with unbracketed namespace declarations".into(),
                    line,
                ));
            }
        } else if !with_bracket {
            return Err(fatal(
                "Cannot mix bracketed namespace declarations with unbracketed namespace declarations".into(),
                line,
            ));
        } else if self.ns.is_some() || self.in_namespace {
            return Err(fatal(
                "Namespace declarations cannot be nested".into(),
                line,
            ));
        }
        let is_first_namespace = (!with_bracket && self.ns.is_none())
            || (with_bracket && !self.has_bracketed_namespaces);
        if is_first_namespace && !is_first {
            return Err(fatal(
                "Namespace declaration statement has to be the very first statement or after any declare call in the script"
                    .into(),
                line,
            ));
        }
        if let Some(n) = &name
            && n.eq_ignore_case(b"namespace")
        {
            return Err(fatal(
                format!("Cannot use '{}' as namespace name", lossy(n)),
                line,
            ));
        }
        self.ns = name;
        self.reset_imports();
        self.in_namespace = true;
        if with_bracket {
            self.has_bracketed_namespaces = true;
        }
        if let Some(body) = body {
            for s in body.iter_mut() {
                match &mut s.kind {
                    StmtKind::Namespace { name, body } => {
                        let line = s.line;
                        self.namespace(name.clone(), body.as_mut(), false, line)?;
                    }
                    StmtKind::HaltCompiler { .. } => {
                        return Err(fatal(
                            "__HALT_COMPILER() can only be used from the outermost scope".into(),
                            s.line,
                        ));
                    }
                    _ => self.stmt(s)?,
                }
            }
            // zend_end_namespace()
            self.in_namespace = false;
            self.reset_imports();
            self.ns = None;
        }
        Ok(())
    }

    /// `zend_compile_use()` for one item.
    fn use_item(
        &mut self,
        kind: UseKind,
        old_name: ByteStr,
        alias: Option<&ByteStr>,
        line: Line,
    ) -> RResult {
        let new_name: ByteStr = match alias {
            Some(a) => a.clone(),
            None => {
                let u = unqualified(&old_name);
                if u.len() != old_name.len() {
                    ByteStr::from(u)
                } else {
                    if self.ns.is_none() {
                        self.diag(
                            DiagnosticLevel::Warning,
                            format!(
                                "The use statement with non-compound name '{}' has no effect",
                                lossy(&old_name)
                            ),
                            line,
                        );
                    }
                    old_name.clone()
                }
            }
        };
        let lookup: Vec<u8> = if kind == UseKind::Const {
            new_name.to_vec()
        } else {
            lower(&new_name)
        };
        if kind == UseKind::Class && is_reserved_class_name(&new_name) {
            return Err(fatal(
                format!(
                    "Cannot use {} as {} because '{}' is a special class name",
                    lossy(&old_name),
                    lossy(&new_name),
                    lossy(&new_name)
                ),
                line,
            ));
        }
        let underscore = new_name.as_bytes() == b"_";
        if underscore || new_name.eq_ignore_case(b"let") || new_name.eq_ignore_case(b"is") {
            let msg = match kind {
                UseKind::Class => Some(format!(
                    "Using \"{}\" as a class name is deprecated",
                    lossy(&new_name)
                )),
                UseKind::Const => Some(format!(
                    "Using \"{}\" as a constant name is deprecated since 8.6",
                    lossy(&new_name)
                )),
                UseKind::Function if !underscore => Some(format!(
                    "Using \"{}\" as a function name is deprecated since 8.6",
                    lossy(&new_name)
                )),
                UseKind::Function => None,
            };
            if let Some(m) = msg {
                self.diag(DiagnosticLevel::Deprecated, m, line);
            }
        }
        let sym = match kind {
            UseKind::Class => SYMBOL_CLASS,
            UseKind::Function => SYMBOL_FUNCTION,
            UseKind::Const => SYMBOL_CONST,
        };
        let check = match &self.ns {
            Some(ns) => {
                let mut v = lower(ns);
                v.push(b'\\');
                v.extend_from_slice(&lookup);
                v
            }
            None => lookup.clone(),
        };
        let already_in_use = || {
            fatal(
                format!(
                    "Cannot use{} {} as {} because the name is already in use",
                    use_type_str(kind),
                    lossy(&old_name),
                    lossy(&new_name)
                ),
                line,
            )
        };
        if self.have_seen(&check, sym) && !old_name.eq_ignore_case(&check) {
            return Err(already_in_use());
        }
        let table = match kind {
            UseKind::Class => &mut self.imports,
            UseKind::Function => &mut self.imports_function,
            UseKind::Const => &mut self.imports_const,
        };
        if table.contains_key(&lookup) {
            return Err(already_in_use());
        }
        table.insert(lookup, old_name);
        Ok(())
    }

    fn stmt(&mut self, s: &mut Stmt) -> RResult {
        let line = s.line;
        match &mut s.kind {
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Echo(es) | StmtKind::Global(es) | StmtKind::Unset(es) => self.exprs(es),
            StmtKind::InlineHtml(_)
            | StmtKind::Goto(_)
            | StmtKind::Label(_)
            | StmtKind::Nop
            | StmtKind::HaltCompiler { .. } => Ok(()),
            StmtKind::If(i) => {
                for b in &mut i.branches {
                    self.expr(&mut b.cond)?;
                    self.block(&mut b.body)?;
                }
                if let Some(e) = &mut i.else_branch {
                    self.block(e)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body } => {
                self.expr(cond)?;
                self.block(body)
            }
            StmtKind::DoWhile { body, cond } => {
                self.block(body)?;
                self.expr(cond)
            }
            StmtKind::For(f) => {
                self.exprs(&mut f.init)?;
                self.exprs(&mut f.cond)?;
                self.exprs(&mut f.step)?;
                self.block(&mut f.body)
            }
            StmtKind::Foreach(f) => {
                self.expr(&mut f.expr)?;
                if let Some(k) = &mut f.key {
                    self.expr(&mut k.expr)?;
                }
                self.expr(&mut f.value.expr)?;
                self.block(&mut f.body)
            }
            StmtKind::Switch { subject, cases } => {
                self.expr(subject)?;
                for c in cases {
                    if let Some(e) = &mut c.cond {
                        self.expr(e)?;
                    }
                    self.block(&mut c.body)?;
                }
                Ok(())
            }
            StmtKind::Break(e) | StmtKind::Continue(e) | StmtKind::Return(e) => self.opt_expr(e),
            StmtKind::Static(vars) => {
                for v in vars {
                    self.opt_expr(&mut v.default)?;
                }
                Ok(())
            }
            StmtKind::Block(b) => self.block(b),
            StmtKind::Try(t) => {
                self.block(&mut t.body)?;
                for c in &mut t.catches {
                    for n in &mut c.types {
                        self.class_name(n)?;
                    }
                    self.block(&mut c.body)?;
                }
                if let Some(f) = &mut t.finally {
                    self.block(f)?;
                }
                Ok(())
            }
            StmtKind::Function(f) => self.function_decl(f, line),
            StmtKind::Class(c) => self.class_decl(c, line),
            StmtKind::Const(c) => self.const_stmt(c),
            StmtKind::Namespace { .. } => {
                // Only valid at the top level; php-src's grammar rejects it
                // elsewhere.
                Ok(())
            }
            StmtKind::Use { kind, uses } => {
                let kind = *kind;
                for u in uses.iter() {
                    self.use_item(kind, u.name.clone(), u.alias.as_ref(), u.line)?;
                }
                Ok(())
            }
            StmtKind::GroupUse { prefix, kind, uses } => {
                for u in uses.iter() {
                    let k = kind.or(u.kind).unwrap_or(UseKind::Class);
                    self.use_item(k, concat(prefix, &u.name), u.alias.as_ref(), u.line)?;
                }
                Ok(())
            }
            StmtKind::Declare { directives, body } => {
                for d in directives {
                    self.expr(&mut d.value)?;
                }
                if let Some(b) = body {
                    self.block(b)?;
                }
                Ok(())
            }
        }
    }

    fn block(&mut self, b: &mut [Stmt]) -> RResult {
        for s in b {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn const_stmt(&mut self, c: &mut ConstStmt) -> RResult {
        self.attributes(&mut c.attributes)?;
        for e in &mut c.consts {
            self.expr(&mut e.value)?;
            let name = self.prefix_with_ns(&e.name);
            if let Some(import) = self.imports_const.get(e.name.as_bytes())
                && *import != name
            {
                return Err(fatal(
                    format!(
                        "Cannot declare const {} because the name is already in use",
                        lossy(&name)
                    ),
                    e.line,
                ));
            }
            self.register_seen(name.to_vec(), SYMBOL_CONST);
            e.fq_name = name;
        }
        Ok(())
    }

    /// `zend_begin_func_decl()` name checks for a named function.
    fn function_decl(&mut self, f: &mut FunctionDecl, line: Line) -> RResult {
        let name = self.prefix_with_ns(&f.name);
        let lcname = lower(&name);
        if let Some(import) = self.imports_function.get(&lower(&f.name))
            && !import.eq_ignore_case(&lcname)
        {
            return Err(fatal(
                format!(
                    "Cannot redeclare function {}() (previously declared as local import)",
                    lossy(&name)
                ),
                line,
            ));
        }
        if lcname == b"__autoload" {
            return Err(fatal(
                "__autoload() is no longer supported, use spl_autoload_register() instead".into(),
                line,
            ));
        }
        if f.name.eq_ignore_case(b"readonly")
            || f.name.eq_ignore_case(b"let")
            || f.name.eq_ignore_case(b"is")
        {
            self.diag(
                DiagnosticLevel::Deprecated,
                format!(
                    "Calling a function \"{}\" is deprecated since 8.6",
                    lossy(&f.name)
                ),
                line,
            );
        }
        if f.name.eq_ignore_case(b"assert") {
            return Err(fatal(
                "Defining a custom assert() function is not allowed, as the function has special semantics".into(),
                line,
            ));
        }
        self.register_seen(lcname, SYMBOL_FUNCTION);
        f.fq_name = name;
        self.function_common(&mut f.common)?;
        self.block(&mut f.body)
    }

    fn function_common(&mut self, c: &mut FunctionCommon) -> RResult {
        self.attributes(&mut c.attributes)?;
        for p in &mut c.params {
            self.param(p)?;
        }
        if let Some(t) = &mut c.return_type {
            self.type_hint(t)?;
        }
        Ok(())
    }

    fn param(&mut self, p: &mut Param) -> RResult {
        self.attributes(&mut p.attributes)?;
        if let Some(t) = &mut p.ty {
            self.type_hint(t)?;
        }
        self.opt_expr(&mut p.default)?;
        if let Some(h) = &mut p.hooks {
            self.hooks(h)?;
        }
        Ok(())
    }

    fn hooks(&mut self, hooks: &mut [PropertyHook]) -> RResult {
        for h in hooks {
            self.attributes(&mut h.attributes)?;
            if let Some(ps) = &mut h.params {
                for p in ps {
                    self.param(p)?;
                }
            }
            match &mut h.body {
                HookBody::Abstract => {}
                HookBody::Block(b) => self.block(b)?,
                HookBody::Expr(e) => self.expr(e)?,
            }
        }
        Ok(())
    }

    /// `zend_compile_class_decl()` name checks.
    fn class_decl(&mut self, c: &mut ClassDecl, line: Line) -> RResult {
        if let Some(unqualified_name) = c.name.clone() {
            let what = match c.kind {
                ClassKind::Enum => "an enum name",
                ClassKind::Interface => "an interface name",
                ClassKind::Trait => "a trait name",
                ClassKind::Class => "a class name",
            };
            // zend_assert_valid_class_name()
            if is_reserved_class_name(&unqualified_name) {
                return Err(fatal(
                    format!(
                        "Cannot use \"{}\" as {} as it is reserved",
                        lossy(&unqualified_name),
                        what
                    ),
                    line,
                ));
            }
            let u = unqualified_name.as_bytes();
            if u == b"_" || u.ends_with(b"\\_") {
                self.diag(
                    DiagnosticLevel::Deprecated,
                    format!("Using \"_\" as {what} is deprecated since 8.4"),
                    line,
                );
            } else if u.eq_ignore_ascii_case(b"let") || u.ends_with(b"\\let") {
                self.diag(
                    DiagnosticLevel::Deprecated,
                    format!("Using \"let\" as {what} is deprecated since 8.6"),
                    line,
                );
            } else if u.eq_ignore_ascii_case(b"is") || u.ends_with(b"\\is") {
                self.diag(
                    DiagnosticLevel::Deprecated,
                    format!("Using \"is\" as {what} is deprecated since 8.6"),
                    line,
                );
            }
            let name = self.prefix_with_ns(&unqualified_name);
            let lcname = lower(&name);
            if let Some(import) = self.imports.get(&lower(&unqualified_name))
                && !import.eq_ignore_case(&lcname)
            {
                return Err(fatal(
                    format!(
                        "Cannot redeclare class {} (previously declared as local import)",
                        lossy(&name)
                    ),
                    line,
                ));
            }
            self.register_seen(lcname, SYMBOL_CLASS);
            c.fq_name = name;
        }
        self.class_body(c)
    }

    fn class_body(&mut self, c: &mut ClassDecl) -> RResult {
        self.attributes(&mut c.attributes)?;
        if let Some(e) = &mut c.extends {
            self.const_class_ref(e, "class name")?;
        }
        for n in &mut c.implements {
            self.const_class_ref(n, "interface name")?;
        }
        if let Some(t) = &mut c.enum_backing_type {
            self.type_hint(t)?;
        }
        for m in &mut c.members {
            match &mut m.kind {
                ClassMemberKind::Property(p) => {
                    self.attributes(&mut p.attributes)?;
                    if let Some(t) = &mut p.ty {
                        self.type_hint(t)?;
                    }
                    for e in &mut p.props {
                        self.opt_expr(&mut e.default)?;
                        if let Some(h) = &mut e.hooks {
                            self.hooks(h)?;
                        }
                    }
                }
                ClassMemberKind::Const(g) => {
                    self.attributes(&mut g.attributes)?;
                    if let Some(t) = &mut g.ty {
                        self.type_hint(t)?;
                    }
                    for e in &mut g.consts {
                        self.expr(&mut e.value)?;
                    }
                }
                ClassMemberKind::Method(md) => {
                    self.function_common(&mut md.common)?;
                    if let Some(b) = &mut md.body {
                        self.block(b)?;
                    }
                }
                ClassMemberKind::TraitUse(t) => {
                    for n in &mut t.traits {
                        self.const_class_ref(n, "trait name")?;
                    }
                    for a in t.adaptations.iter_mut().flatten() {
                        match a {
                            TraitAdaptation::Precedence {
                                trait_name,
                                insteadof,
                                ..
                            } => {
                                self.const_class_ref(trait_name, "trait name")?;
                                for n in insteadof {
                                    self.const_class_ref(n, "trait name")?;
                                }
                            }
                            TraitAdaptation::Alias { trait_name, .. } => {
                                if let Some(n) = trait_name {
                                    self.const_class_ref(n, "trait name")?;
                                }
                            }
                        }
                    }
                }
                ClassMemberKind::EnumCase(ec) => {
                    self.attributes(&mut ec.attributes)?;
                    self.opt_expr(&mut ec.value)?;
                }
            }
        }
        Ok(())
    }

    fn attributes(&mut self, groups: &mut [AttributeGroup]) -> RResult {
        for g in groups {
            for a in g {
                self.class_name(&mut a.name)?;
                if let Some(args) = &mut a.args {
                    self.args(args)?;
                }
            }
        }
        Ok(())
    }

    fn type_hint(&mut self, t: &mut TypeHint) -> RResult {
        match t {
            TypeHint::Class(n) => {
                if n.kind != NameKind::Qualified {
                    let builtin = BuiltinType::from_name(&n.name)
                        .map(BuiltinType::name)
                        .or_else(|| {
                            ["array", "callable", "static"]
                                .into_iter()
                                .find(|s| n.name.eq_ignore_case(s.as_bytes()))
                        });
                    if let Some(b) = builtin
                        && n.kind != NameKind::Unqualified
                    {
                        return Err(fatal(
                            format!("Type declaration '{b}' must be unqualified"),
                            n.line,
                        ));
                    }
                }
                let is_special =
                    n.kind != NameKind::FullyQualified && special_class(&n.name).is_some();
                self.class_name(n)?;
                if !is_special {
                    // zend_assert_valid_class_name(class_name, "a type name")
                    if is_reserved_class_name(&n.resolved) {
                        return Err(fatal(
                            format!(
                                "Cannot use \"{}\" as a type name as it is reserved",
                                lossy(&n.resolved)
                            ),
                            n.line,
                        ));
                    }
                    let u = n.resolved.as_bytes();
                    let what = "a type name";
                    let line = n.line;
                    if u == b"_" || u.ends_with(b"\\_") {
                        self.diag(
                            DiagnosticLevel::Deprecated,
                            format!("Using \"_\" as {what} is deprecated since 8.4"),
                            line,
                        );
                    } else if u.eq_ignore_ascii_case(b"let") || u.ends_with(b"\\let") {
                        self.diag(
                            DiagnosticLevel::Deprecated,
                            format!("Using \"let\" as {what} is deprecated since 8.6"),
                            line,
                        );
                    } else if u.eq_ignore_ascii_case(b"is") || u.ends_with(b"\\is") {
                        self.diag(
                            DiagnosticLevel::Deprecated,
                            format!("Using \"is\" as {what} is deprecated since 8.6"),
                            line,
                        );
                    }
                }
                Ok(())
            }
            TypeHint::Nullable(t) => self.type_hint(t),
            TypeHint::Union(ts) | TypeHint::Intersection(ts) => {
                for t in ts {
                    self.type_hint(t)?;
                }
                Ok(())
            }
            TypeHint::Builtin { .. } | TypeHint::Array | TypeHint::Callable | TypeHint::Static => {
                Ok(())
            }
        }
    }

    // ---------------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------------

    fn exprs(&mut self, es: &mut [Expr]) -> RResult {
        for e in es {
            self.expr(e)?;
        }
        Ok(())
    }

    fn opt_expr(&mut self, e: &mut Option<Expr>) -> RResult {
        match e {
            Some(e) => self.expr(e),
            None => Ok(()),
        }
    }

    fn args(&mut self, a: &mut ArgList) -> RResult {
        for arg in &mut a.args {
            match arg {
                Arg::Positional(e) | Arg::Unpack(e) | Arg::Named { value: e, .. } => {
                    self.expr(e)?
                }
                Arg::Placeholder | Arg::NamedPlaceholder(_) | Arg::VariadicPlaceholder => {}
            }
        }
        Ok(())
    }

    fn class_ref(&mut self, c: &mut ClassRef) -> RResult {
        match c {
            ClassRef::Name(n) => self.class_name(n),
            ClassRef::Expr(e) => self.expr(e),
        }
    }

    fn var_name(&mut self, v: &mut VarName) -> RResult {
        match v {
            VarName::Name(_) => Ok(()),
            VarName::Expr(e) => self.expr(e),
        }
    }

    fn member_name(&mut self, m: &mut MemberName) -> RResult {
        match m {
            MemberName::Ident(_) => Ok(()),
            MemberName::Expr(e) => self.expr(e),
        }
    }

    fn parts(&mut self, parts: &mut [StrPart]) -> RResult {
        for p in parts {
            if let StrPart::Expr { expr, .. } = p {
                self.expr(expr)?;
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &mut Expr) -> RResult {
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::MagicConst(_) => Ok(()),
            ExprKind::Interpolated(parts) | ExprKind::ShellExec(parts) => self.parts(parts),
            ExprKind::Const(n) => {
                self.const_name(n);
                Ok(())
            }
            ExprKind::ClassConst { class, name } => {
                self.class_ref(class)?;
                match name {
                    ClassConstName::Ident(_) => Ok(()),
                    ClassConstName::Expr(e) => self.expr(e),
                }
            }
            ExprKind::ClassName(class) => self.class_ref(class),
            ExprKind::Var(v) => self.var_name(v),
            ExprKind::Array { items, .. } => {
                for item in items.iter_mut().flatten() {
                    if let Some(k) = &mut item.key {
                        self.expr(k)?;
                    }
                    self.expr(&mut item.value)?;
                }
                Ok(())
            }
            ExprKind::Dim { base, index } => {
                self.expr(base)?;
                match index {
                    Some(i) => self.expr(i),
                    None => Ok(()),
                }
            }
            ExprKind::Prop { object, name, .. } => {
                self.expr(object)?;
                self.member_name(name)
            }
            ExprKind::StaticProp { class, name, .. } => {
                self.class_ref(class)?;
                self.var_name(name)
            }
            ExprKind::Call { callee, args } => {
                match callee {
                    Callee::Name(n) => self.function_name(n),
                    Callee::Expr(e) => self.expr(e)?,
                }
                self.args(args)
            }
            ExprKind::MethodCall {
                object, name, args, ..
            } => {
                self.expr(object)?;
                self.member_name(name)?;
                self.args(args)
            }
            ExprKind::StaticCall { class, name, args } => {
                self.class_ref(class)?;
                self.member_name(name)?;
                self.args(args)
            }
            ExprKind::New { class, args } => {
                match class {
                    NewClass::Name(n) => self.class_name(n)?,
                    NewClass::Expr(e) => self.expr(e)?,
                    NewClass::Anonymous(c) => self.class_body(c)?,
                }
                self.args(args)
            }
            ExprKind::Assign { target, value }
            | ExprKind::AssignRef { target, value }
            | ExprKind::AssignOp { target, value, .. }
            | ExprKind::AssignCoalesce { target, value } => {
                self.expr(target)?;
                self.expr(value)
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left)?;
                self.expr(right)
            }
            ExprKind::IncDec { target: e, .. }
            | ExprKind::Unary { expr: e, .. }
            | ExprKind::Cast { expr: e, .. }
            | ExprKind::VoidCast(e)
            | ExprKind::Silence(e)
            | ExprKind::Empty(e)
            | ExprKind::Include { expr: e, .. }
            | ExprKind::Eval(e)
            | ExprKind::Print(e)
            | ExprKind::Throw(e)
            | ExprKind::YieldFrom(e) => self.expr(e),
            ExprKind::Isset(es) => self.exprs(es),
            ExprKind::Closure(c) => {
                self.function_common(&mut c.common)?;
                self.block(&mut c.body)
            }
            ExprKind::ArrowFn(f) => {
                self.function_common(&mut f.common)?;
                self.expr(&mut f.body)
            }
            ExprKind::Ternary {
                cond, then, else_, ..
            } => {
                self.expr(cond)?;
                if let Some(t) = then {
                    self.expr(t)?;
                }
                self.expr(else_)
            }
            ExprKind::Match { subject, arms } => {
                self.expr(subject)?;
                for arm in arms {
                    if let Some(cs) = &mut arm.conds {
                        self.exprs(cs)?;
                    }
                    self.expr(&mut arm.body)?;
                }
                Ok(())
            }
            ExprKind::Instanceof { expr, class } => {
                self.expr(expr)?;
                self.class_ref(class)
            }
            ExprKind::Yield { key, value } => {
                if let Some(k) = key {
                    self.expr(k)?;
                }
                if let Some(v) = value {
                    self.expr(v)?;
                }
                Ok(())
            }
        }
    }
}
