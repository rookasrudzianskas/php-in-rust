//! Port of `zend_ast_export()` (`Zend/zend_ast.c`), used for `assert()`
//! failure messages: `assert($a > 0)` reports `assert($a > 0)` re-printed
//! from the AST, so spacing, parentheses (priorities) and quoting follow
//! php-src exactly.

use crate::ast::*;
use crate::numeric;

/// Export an expression (`zend_ast_export("", ast, "")`).
pub fn export_expr(e: &Expr) -> String {
    String::from_utf8_lossy(&export_expr_bytes(e)).into_owned()
}

/// Export an expression as bytes (strings in the AST may not be UTF-8).
pub fn export_expr_bytes(e: &Expr) -> Vec<u8> {
    let mut x = Exporter { out: Vec::new() };
    x.expr(e, 0, 0);
    x.out
}

/// Export a statement as `zend_ast_export_stmt()` prints it at indent 0
/// (including the trailing `;` and newline).
pub fn export_stmt(s: &Stmt) -> String {
    let mut x = Exporter { out: Vec::new() };
    x.stmt(s, 0);
    String::from_utf8_lossy(&x.out).into_owned()
}

struct Exporter {
    out: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Method,
    Property,
    Constant,
    Cpp,
}

fn binary_info(op: BinaryOp) -> (&'static str, i32, i32, i32) {
    use BinaryOp::*;
    match op {
        Add => (" + ", 200, 200, 201),
        Sub => (" - ", 200, 200, 201),
        Mul => (" * ", 210, 210, 211),
        Div => (" / ", 210, 210, 211),
        Mod => (" % ", 210, 210, 211),
        ShiftLeft => (" << ", 190, 190, 191),
        ShiftRight => (" >> ", 190, 190, 191),
        Concat => (" . ", 185, 185, 186),
        BitOr => (" | ", 140, 140, 141),
        BitAnd => (" & ", 160, 160, 161),
        BitXor => (" ^ ", 150, 150, 151),
        Identical => (" === ", 170, 171, 171),
        NotIdentical => (" !== ", 170, 171, 171),
        Equal => (" == ", 170, 171, 171),
        NotEqual => (" != ", 170, 171, 171),
        Less => (" < ", 180, 181, 181),
        LessEqual => (" <= ", 180, 181, 181),
        Greater => (" > ", 180, 181, 181),
        GreaterEqual => (" >= ", 180, 181, 181),
        Pow => (" ** ", 250, 251, 250),
        LogicalXor => (" xor ", 40, 40, 41),
        Spaceship => (" <=> ", 180, 181, 181),
        BooleanAnd => (" && ", 130, 130, 131),
        BooleanOr => (" || ", 120, 120, 121),
        Coalesce => (" ?? ", 110, 111, 110),
        Pipe => (" |> ", 183, 183, 184),
    }
}

fn assign_op_str(op: BinaryOp) -> &'static str {
    use BinaryOp::*;
    match op {
        Add => " += ",
        Sub => " -= ",
        Mul => " *= ",
        Div => " /= ",
        Mod => " %= ",
        ShiftLeft => " <<= ",
        ShiftRight => " >>= ",
        Concat => " .= ",
        BitOr => " |= ",
        BitAnd => " &= ",
        BitXor => " ^= ",
        Pow => " **= ",
        _ => " ??= ",
    }
}

/// `zend_ast_valid_var_name()`
fn valid_var_name(s: &[u8]) -> bool {
    let ok_first = |c: u8| c == b'_' || c >= 127 || c.is_ascii_alphabetic();
    match s.split_first() {
        None => false,
        Some((&f, rest)) => ok_first(f) && rest.iter().all(|&c| ok_first(c) || c.is_ascii_digit()),
    }
}

impl Exporter {
    fn s(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn b(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
    }

    fn c(&mut self, c: u8) {
        self.out.push(c);
    }

    fn indent(&mut self, indent: usize) {
        for _ in 0..indent {
            self.s("    ");
        }
    }

    /// `zend_ast_export_str()`: contents of a single-quoted string.
    fn export_str(&mut self, s: &[u8]) {
        for &c in s {
            if c == b'\'' || c == b'\\' {
                self.c(b'\\');
            }
            self.c(c);
        }
    }

    /// `zend_ast_export_qstr()`: contents of a double-quoted string.
    fn qstr(&mut self, quote: u8, s: &[u8]) {
        for &c in s {
            if c < b' ' {
                match c {
                    b'\n' => self.s("\\n"),
                    b'\r' => self.s("\\r"),
                    b'\t' => self.s("\\t"),
                    0x0c => self.s("\\f"),
                    0x0b => self.s("\\v"),
                    0x1b => self.s("\\e"),
                    _ => {
                        self.s("\\0");
                        self.c(b'0' + c / 8);
                        self.c(b'0' + c % 8);
                    }
                }
            } else {
                if c == quote || c == b'$' || c == b'\\' {
                    self.c(b'\\');
                }
                self.c(c);
            }
        }
    }

    /// `zend_ast_export_quoted_str()`
    fn quoted_str(&mut self, s: &[u8]) {
        if s.iter().any(|&c| c < b' ') {
            self.c(b'"');
            self.qstr(b'"', s);
            self.c(b'"');
        } else {
            self.c(b'\'');
            self.export_str(s);
            self.c(b'\'');
        }
    }

    /// `zend_ast_export_ns_name()` for a name.
    fn ns_name(&mut self, n: &Name) {
        match n.kind {
            NameKind::FullyQualified => self.c(b'\\'),
            NameKind::Relative => self.s("namespace\\"),
            _ => {}
        }
        self.b(&n.name);
    }

    /// `zend_ast_export_ns_name_or_expression()` for an expression.
    fn ns_name_or_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::String(s) => self.b(s),
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Var(_) => self.expr(e, 0, 0),
            _ => {
                self.c(b'(');
                self.expr(e, 0, 0);
                self.c(b')');
            }
        }
    }

    fn class_ref(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Name(n) => self.ns_name(n),
            ClassRef::Expr(e) => self.ns_name_or_expr(e),
        }
    }

    /// `zend_ast_export_var()` with a plain name.
    fn var_ident(&mut self, name: &[u8]) {
        if valid_var_name(name) {
            self.b(name);
        } else {
            self.s("{'");
            self.export_str(name);
            self.s("'}");
        }
    }

    /// `zend_ast_export_var()` with an expression child.
    fn var_expr(&mut self, e: &Expr, indent: usize) {
        match &e.kind {
            ExprKind::String(s) => self.var_ident(s),
            ExprKind::Var(_) => self.expr(e, 0, indent),
            _ => {
                self.c(b'{');
                self.name_expr(e, indent);
                self.c(b'}');
            }
        }
    }

    /// `zend_ast_export_name()` for an expression.
    fn name_expr(&mut self, e: &Expr, indent: usize) {
        match &e.kind {
            ExprKind::String(s) => self.b(s),
            _ => self.expr(e, 0, indent),
        }
    }

    fn var_name(&mut self, v: &VarName, indent: usize) {
        match v {
            VarName::Name(n) => self.var_ident(n),
            VarName::Expr(e) => self.var_expr(e, indent),
        }
    }

    fn member_name(&mut self, m: &MemberName, indent: usize) {
        match m {
            MemberName::Ident(n) => self.var_ident(n),
            MemberName::Expr(e) => self.var_expr(e, indent),
        }
    }

    fn visibility(&mut self, m: Modifiers, target: Target) {
        if m.contains(Modifiers::PUBLIC) {
            self.s("public ");
        } else if m.contains(Modifiers::PROTECTED) {
            self.s("protected ");
        } else if m.contains(Modifiers::PRIVATE) {
            self.s("private ");
        }
        if matches!(target, Target::Property | Target::Cpp) {
            if m.contains(Modifiers::PRIVATE_SET) {
                self.s("private(set) ");
            } else if m.contains(Modifiers::PROTECTED_SET) {
                self.s("protected(set) ");
            } else if m.contains(Modifiers::PUBLIC_SET) {
                self.s("public(set) ");
            }
        }
    }

    fn type_hint(&mut self, t: &TypeHint) {
        match t {
            TypeHint::Union(ts) | TypeHint::Intersection(ts) => {
                let sep = if matches!(t, TypeHint::Union(_)) {
                    b'|'
                } else {
                    b'&'
                };
                for (i, t) in ts.iter().enumerate() {
                    if i != 0 {
                        self.c(sep);
                    }
                    self.type_hint(t);
                }
            }
            TypeHint::Nullable(t) => {
                self.c(b'?');
                self.type_hint(t);
            }
            TypeHint::Builtin { source, .. } => self.b(source),
            TypeHint::Class(n) => self.ns_name(n),
            TypeHint::Array => self.s("array"),
            TypeHint::Callable => self.s("callable"),
            TypeHint::Static => self.s("static"),
        }
    }

    fn attributes(&mut self, groups: &[AttributeGroup], indent: usize, newlines: bool) {
        for g in groups {
            self.s("#[");
            for (i, a) in g.iter().enumerate() {
                if i != 0 {
                    self.s(", ");
                }
                self.ns_name(&a.name);
                if let Some(args) = &a.args {
                    self.c(b'(');
                    self.args(args, indent);
                    self.c(b')');
                }
            }
            self.c(b']');
            if newlines {
                self.c(b'\n');
                self.indent(indent);
            } else {
                self.c(b' ');
            }
        }
    }

    fn args(&mut self, a: &ArgList, indent: usize) {
        for (i, arg) in a.args.iter().enumerate() {
            if i != 0 {
                self.s(", ");
            }
            match arg {
                Arg::Positional(e) => self.expr(e, 20, indent),
                Arg::Unpack(e) => {
                    self.s("...");
                    self.expr(e, 20, indent);
                }
                Arg::Named { name, value } => {
                    self.b(name);
                    self.s(": ");
                    self.expr(value, 20, indent);
                }
                Arg::Placeholder => self.c(b'?'),
                Arg::NamedPlaceholder(name) => {
                    self.b(name);
                    self.s(": ?");
                }
                Arg::VariadicPlaceholder => self.s("..."),
            }
        }
    }

    fn encaps(&mut self, quote: u8, parts: &[StrPart], indent: usize) {
        for p in parts {
            match p {
                StrPart::Literal(s) => self.qstr(quote, s),
                StrPart::Expr { expr, .. } => {
                    self.c(b'{');
                    self.expr(expr, 0, indent);
                    self.c(b'}');
                }
            }
        }
    }

    fn params(&mut self, params: &[Param], indent: usize) {
        for (i, p) in params.iter().enumerate() {
            if i != 0 {
                self.s(", ");
            }
            self.attributes(&p.attributes, indent, false);
            self.visibility(p.modifiers, Target::Cpp);
            if p.modifiers.contains(Modifiers::FINAL) {
                self.s("final ");
            }
            if let Some(t) = &p.ty {
                self.type_hint(t);
                self.c(b' ');
            }
            if p.by_ref {
                self.c(b'&');
            }
            if p.variadic {
                self.s("...");
            }
            self.c(b'$');
            self.b(&p.name);
            if let Some(d) = &p.default {
                self.s(" = ");
                self.expr(d, 0, indent);
            }
            if let Some(h) = &p.hooks {
                self.hooks(h, indent);
            }
        }
    }

    /// `zend_ast_export_hook_list()`
    fn hooks(&mut self, hooks: &[PropertyHook], indent: usize) {
        self.s(" {\n");
        let indent = indent + 1;
        self.indent(indent);
        for (i, h) in hooks.iter().enumerate() {
            self.visibility(h.modifiers, Target::Property);
            if h.modifiers.contains(Modifiers::FINAL) {
                self.s("final ");
            }
            self.b(&h.name);
            match &h.body {
                HookBody::Abstract => self.c(b';'),
                HookBody::Expr(e) => {
                    self.s(" => ");
                    self.expr(e, 0, indent);
                    self.c(b';');
                }
                HookBody::Block(b) => {
                    self.s(" {\n");
                    self.stmts(b, indent + 1);
                    self.indent(indent);
                    self.c(b'}');
                }
            }
            if i + 1 < hooks.len() {
                self.c(b'\n');
                self.indent(indent);
            }
        }
        self.c(b'\n');
        self.indent(indent - 1);
        self.c(b'}');
    }

    /// Function-like declarations (`ZEND_AST_FUNC_DECL`, `CLOSURE`,
    /// `ARROW_FUNC`, `METHOD`).
    #[allow(clippy::too_many_arguments)]
    fn function_like(
        &mut self,
        kind: FnKind,
        common: &FunctionCommon,
        modifiers: Modifiers,
        name: Option<&[u8]>,
        uses: &[ClosureUse],
        body: FnBody<'_>,
        parenthesized: bool,
        indent: usize,
    ) {
        if parenthesized {
            self.c(b'(');
        }
        if !common.attributes.is_empty() {
            let newlines = !matches!(kind, FnKind::Closure | FnKind::Arrow);
            self.attributes(&common.attributes, indent, newlines);
        }
        self.visibility(modifiers, Target::Method);
        if modifiers.contains(Modifiers::STATIC) {
            self.s("static ");
        }
        if modifiers.contains(Modifiers::ABSTRACT) {
            self.s("abstract ");
        }
        if modifiers.contains(Modifiers::FINAL) {
            self.s("final ");
        }
        if kind == FnKind::Arrow {
            self.s("fn");
        } else {
            self.s("function ");
        }
        if common.by_ref {
            self.c(b'&');
        }
        if let Some(n) = name {
            self.b(n);
        }
        self.c(b'(');
        self.params(&common.params, indent);
        self.c(b')');
        if !uses.is_empty() {
            self.s(" use(");
            for (i, u) in uses.iter().enumerate() {
                if i != 0 {
                    self.s(", ");
                }
                if u.by_ref {
                    self.c(b'&');
                }
                self.c(b'$');
                self.b(&u.name);
            }
            self.c(b')');
        }
        if let Some(t) = &common.return_type {
            self.s(": ");
            self.type_hint(t);
        }
        match body {
            FnBody::Expr(e) => {
                self.s(" => ");
                self.expr(e, 0, indent);
                if parenthesized {
                    self.c(b')');
                }
            }
            FnBody::Block(b) => {
                self.s(" {\n");
                self.stmts(b, indent + 1);
                self.indent(indent);
                self.c(b'}');
                if kind != FnKind::Closure {
                    self.c(b'\n');
                }
            }
            FnBody::None => self.s(";\n"),
        }
    }

    fn class_decl(&mut self, c: &ClassDecl, indent: usize) {
        self.attributes(&c.attributes, indent, true);
        match c.kind {
            ClassKind::Interface => self.s("interface "),
            ClassKind::Trait => self.s("trait "),
            ClassKind::Enum => self.s("enum "),
            ClassKind::Class => {
                if c.modifiers.is_abstract {
                    self.s("abstract ");
                }
                if c.modifiers.is_final {
                    self.s("final ");
                }
                if c.modifiers.is_readonly {
                    self.s("readonly ");
                }
                self.s("class ");
            }
        }
        if let Some(n) = &c.name {
            self.b(n);
        }
        if c.kind == ClassKind::Enum
            && let Some(t) = &c.enum_backing_type
        {
            self.s(": ");
            self.type_hint(t);
        }
        self.class_no_header(c, indent);
        self.c(b'\n');
    }

    fn class_no_header(&mut self, c: &ClassDecl, indent: usize) {
        if let Some(e) = &c.extends {
            self.s(" extends ");
            self.ns_name(e);
        }
        if !c.implements.is_empty() {
            self.s(" implements ");
            self.name_list(&c.implements, ", ");
        }
        self.s(" {\n");
        for m in &c.members {
            self.member(m, indent + 1);
        }
        self.indent(indent);
        self.c(b'}');
    }

    /// `zend_ast_export_name_list()` (names printed as written, without the
    /// `\` prefix, like `zend_ast_export_name()`).
    fn name_list(&mut self, names: &[Name], sep: &str) {
        for (i, n) in names.iter().enumerate() {
            if i != 0 {
                self.s(sep);
            }
            self.b(&n.name);
        }
    }

    fn const_elems(&mut self, consts: &[ConstElem], indent: usize) {
        for (i, c) in consts.iter().enumerate() {
            if i != 0 {
                self.s(", ");
            }
            self.b(&c.name);
            self.s(" = ");
            self.expr(&c.value, 0, indent);
        }
    }

    fn member(&mut self, m: &ClassMember, indent: usize) {
        self.indent(indent);
        match &m.kind {
            ClassMemberKind::Property(p) => {
                self.attributes(&p.attributes, indent, true);
                self.visibility(p.modifiers, Target::Property);
                if p.modifiers.contains(Modifiers::STATIC) {
                    self.s("static ");
                }
                if p.modifiers.contains(Modifiers::READONLY) {
                    self.s("readonly ");
                }
                if let Some(t) = &p.ty {
                    self.type_hint(t);
                    self.c(b' ');
                }
                for (i, e) in p.props.iter().enumerate() {
                    if i != 0 {
                        self.s(", ");
                    }
                    self.c(b'$');
                    self.b(&e.name);
                    if let Some(d) = &e.default {
                        self.s(" = ");
                        self.expr(d, 0, indent + 1);
                    }
                    if let Some(h) = &e.hooks {
                        self.hooks(h, indent);
                    }
                }
                if p.props.first().is_none_or(|e| e.hooks.is_none()) {
                    self.c(b';');
                }
            }
            ClassMemberKind::Const(g) => {
                self.attributes(&g.attributes, indent, true);
                self.visibility(g.modifiers, Target::Constant);
                self.s("const ");
                if let Some(t) = &g.ty {
                    self.type_hint(t);
                    self.c(b' ');
                }
                self.const_elems(&g.consts, indent);
                self.c(b';');
            }
            ClassMemberKind::Method(md) => {
                let body = match &md.body {
                    Some(b) => FnBody::Block(b),
                    None => FnBody::None,
                };
                self.function_like(
                    FnKind::Method,
                    &md.common,
                    md.modifiers,
                    Some(&md.name),
                    &[],
                    body,
                    false,
                    indent,
                );
            }
            ClassMemberKind::TraitUse(t) => {
                self.s("use ");
                self.name_list(&t.traits, ", ");
                match &t.adaptations {
                    Some(list) => {
                        self.s(" {\n");
                        for a in list {
                            self.indent(indent + 1);
                            self.trait_adaptation(a);
                            self.s(";\n");
                        }
                        self.indent(indent);
                        self.c(b'}');
                    }
                    None => self.c(b';'),
                }
            }
            ClassMemberKind::EnumCase(ec) => {
                self.attributes(&ec.attributes, indent, true);
                self.s("case ");
                self.b(&ec.name);
                if let Some(v) = &ec.value {
                    self.s(" = ");
                    self.expr(v, 0, indent);
                }
                self.c(b';');
            }
        }
        self.c(b'\n');
    }

    fn trait_adaptation(&mut self, a: &TraitAdaptation) {
        match a {
            TraitAdaptation::Precedence {
                trait_name,
                method,
                insteadof,
                ..
            } => {
                self.b(&trait_name.name);
                self.s("::");
                self.b(method);
                self.s(" insteadof ");
                self.name_list(insteadof, ", ");
            }
            TraitAdaptation::Alias {
                trait_name,
                method,
                modifiers,
                alias,
                ..
            } => {
                if let Some(t) = trait_name {
                    self.b(&t.name);
                    self.s("::");
                }
                self.b(method);
                if modifiers.contains(Modifiers::PUBLIC) {
                    self.s(" as public");
                } else if modifiers.contains(Modifiers::PROTECTED) {
                    self.s(" as protected");
                } else if modifiers.contains(Modifiers::PRIVATE) {
                    self.s(" as private");
                } else if alias.is_some() {
                    self.s(" as");
                }
                if let Some(al) = alias {
                    self.c(b' ');
                    self.b(al);
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // Statements
    // ---------------------------------------------------------------------

    fn stmts(&mut self, b: &[Stmt], indent: usize) {
        for s in b {
            self.stmt(s, indent);
        }
    }

    /// One line of `zend_ast_export_stmt()`.
    fn line(&mut self, indent: usize, f: impl FnOnce(&mut Self), semicolon: bool) {
        self.indent(indent);
        f(self);
        if semicolon {
            self.c(b';');
        }
        self.c(b'\n');
    }

    /// `zend_ast_export_stmt()`
    fn stmt(&mut self, s: &Stmt, indent: usize) {
        match &s.kind {
            StmtKind::Nop => {}
            StmtKind::Block(b) => self.stmts(b, indent),
            StmtKind::Expr(e) => self.line(indent, |x| x.expr(e, 0, indent), true),
            StmtKind::Echo(es) => {
                for e in es {
                    self.line(
                        indent,
                        |x| {
                            x.s("echo ");
                            x.expr(e, 0, indent);
                        },
                        true,
                    );
                }
            }
            StmtKind::InlineHtml(h) => self.line(
                indent,
                |x| {
                    x.s("echo ");
                    x.quoted_str(h);
                },
                true,
            ),
            StmtKind::Global(es) => {
                for e in es {
                    self.line(
                        indent,
                        |x| {
                            x.s("global ");
                            x.expr(e, 0, indent);
                        },
                        true,
                    );
                }
            }
            StmtKind::Unset(es) => {
                for e in es {
                    self.line(
                        indent,
                        |x| {
                            x.s("unset(");
                            x.expr(e, 0, indent);
                            x.c(b')');
                        },
                        true,
                    );
                }
            }
            StmtKind::Static(vars) => {
                for v in vars {
                    self.line(
                        indent,
                        |x| {
                            x.s("static $");
                            x.b(&v.name);
                            if let Some(d) = &v.default {
                                x.s(" = ");
                                x.expr(d, 0, indent);
                            }
                        },
                        true,
                    );
                }
            }
            StmtKind::Break(e) | StmtKind::Continue(e) | StmtKind::Return(e) => {
                let kw = match &s.kind {
                    StmtKind::Break(_) => "break",
                    StmtKind::Continue(_) => "continue",
                    _ => "return",
                };
                self.line(
                    indent,
                    |x| {
                        x.s(kw);
                        if let Some(e) = e {
                            x.c(b' ');
                            x.expr(e, 0, indent);
                        }
                    },
                    true,
                );
            }
            StmtKind::If(i) => self.line(indent, |x| x.if_stmt(i, indent), false),
            StmtKind::While { cond, body } => self.line(
                indent,
                |x| {
                    x.s("while (");
                    x.expr(cond, 0, indent);
                    x.s(") {\n");
                    x.stmts(body, indent + 1);
                    x.indent(indent);
                    x.c(b'}');
                },
                false,
            ),
            StmtKind::DoWhile { body, cond } => self.line(
                indent,
                |x| {
                    x.s("do {\n");
                    x.stmts(body, indent + 1);
                    x.indent(indent);
                    x.s("} while (");
                    x.expr(cond, 0, indent);
                    x.c(b')');
                },
                true,
            ),
            StmtKind::For(f) => self.line(
                indent,
                |x| {
                    x.s("for (");
                    x.expr_list(&f.init, indent);
                    x.c(b';');
                    if !f.cond.is_empty() {
                        x.c(b' ');
                        x.expr_list(&f.cond, indent);
                    }
                    x.c(b';');
                    if !f.step.is_empty() {
                        x.c(b' ');
                        x.expr_list(&f.step, indent);
                    }
                    x.s(") {\n");
                    x.stmts(&f.body, indent + 1);
                    x.indent(indent);
                    x.c(b'}');
                },
                false,
            ),
            StmtKind::Foreach(f) => self.line(
                indent,
                |x| {
                    x.s("foreach (");
                    x.expr(&f.expr, 0, indent);
                    x.s(" as ");
                    if let Some(k) = &f.key {
                        x.foreach_target(k, indent);
                        x.s(" => ");
                    }
                    x.foreach_target(&f.value, indent);
                    x.s(") {\n");
                    x.stmts(&f.body, indent + 1);
                    x.indent(indent);
                    x.c(b'}');
                },
                false,
            ),
            StmtKind::Switch { subject, cases } => self.line(
                indent,
                |x| {
                    x.s("switch (");
                    x.expr(subject, 0, indent);
                    x.s(") {\n");
                    for c in cases {
                        x.indent(indent + 1);
                        match &c.cond {
                            Some(e) => {
                                x.s("case ");
                                x.expr(e, 0, indent + 1);
                                x.s(":\n");
                            }
                            None => x.s("default:\n"),
                        }
                        x.stmts(&c.body, indent + 2);
                    }
                    x.indent(indent);
                    x.c(b'}');
                },
                false,
            ),
            StmtKind::Try(t) => self.line(
                indent,
                |x| {
                    x.s("try {\n");
                    x.stmts(&t.body, indent + 1);
                    x.indent(indent);
                    for c in &t.catches {
                        x.s("} catch (");
                        x.name_list(&c.types, "|");
                        if let Some(v) = &c.var {
                            x.s(" $");
                            x.var_ident(v);
                        }
                        x.s(") {\n");
                        x.stmts(&c.body, indent + 1);
                        x.indent(indent);
                    }
                    if let Some(f) = &t.finally {
                        x.s("} finally {\n");
                        x.stmts(f, indent + 1);
                        x.indent(indent);
                    }
                    x.c(b'}');
                },
                false,
            ),
            StmtKind::Goto(l) => self.line(
                indent,
                |x| {
                    x.s("goto ");
                    x.b(l);
                },
                true,
            ),
            StmtKind::Label(l) => self.line(
                indent,
                |x| {
                    x.b(l);
                    x.c(b':');
                },
                false,
            ),
            StmtKind::Function(f) => self.line(
                indent,
                |x| {
                    x.function_like(
                        FnKind::Function,
                        &f.common,
                        Modifiers::empty(),
                        Some(&f.name),
                        &[],
                        FnBody::Block(&f.body),
                        false,
                        indent,
                    )
                },
                false,
            ),
            StmtKind::Class(c) => self.line(indent, |x| x.class_decl(c, indent), false),
            StmtKind::Const(c) => self.line(
                indent,
                |x| {
                    x.attributes(&c.attributes, indent, true);
                    x.s("const ");
                    x.const_elems(&c.consts, indent);
                },
                true,
            ),
            StmtKind::Namespace { name, body } => self.line(
                indent,
                |x| {
                    x.s("namespace");
                    if let Some(n) = name {
                        x.c(b' ');
                        x.b(n);
                    }
                    match body {
                        Some(b) => {
                            x.s(" {\n");
                            x.stmts(b, indent + 1);
                            x.indent(indent);
                            x.s("}\n");
                        }
                        None => x.c(b';'),
                    }
                },
                false,
            ),
            StmtKind::Use { kind, uses } => self.line(
                indent,
                |x| {
                    x.s("use ");
                    x.use_kind(Some(*kind));
                    x.use_items(uses);
                },
                true,
            ),
            StmtKind::GroupUse { prefix, kind, uses } => self.line(
                indent,
                |x| {
                    x.s("use ");
                    x.use_kind(*kind);
                    x.b(prefix);
                    x.s("\\{");
                    for (i, u) in uses.iter().enumerate() {
                        if i != 0 {
                            x.s(", ");
                        }
                        x.use_kind(u.kind);
                        x.use_items(std::slice::from_ref(u));
                    }
                    x.c(b'}');
                },
                true,
            ),
            StmtKind::Declare { directives, body } => self.line(
                indent,
                |x| {
                    x.s("declare(");
                    for (i, d) in directives.iter().enumerate() {
                        if i != 0 {
                            x.s(", ");
                        }
                        x.b(&d.name);
                        x.s(" = ");
                        x.expr(&d.value, 0, indent);
                    }
                    x.c(b')');
                    match body {
                        Some(b) => {
                            x.s(" {\n");
                            x.stmts(b, indent + 1);
                            x.indent(indent);
                            x.c(b'}');
                        }
                        None => x.c(b';'),
                    }
                },
                false,
            ),
            StmtKind::HaltCompiler { .. } => self.line(indent, |x| x.s("__HALT_COMPILER()"), true),
        }
    }

    fn use_kind(&mut self, kind: Option<UseKind>) {
        match kind {
            Some(UseKind::Function) => self.s("function "),
            Some(UseKind::Const) => self.s("const "),
            _ => {}
        }
    }

    fn use_items(&mut self, uses: &[UseItem]) {
        for (i, u) in uses.iter().enumerate() {
            if i != 0 {
                self.s(", ");
            }
            self.b(&u.name);
            if let Some(a) = &u.alias {
                self.s(" as ");
                self.b(a);
            }
        }
    }

    fn foreach_target(&mut self, t: &ForeachTarget, indent: usize) {
        if t.by_ref {
            self.c(b'&');
        }
        self.expr(&t.expr, 0, indent);
    }

    /// `zend_ast_export_if_stmt()`
    fn if_stmt(&mut self, i: &IfStmt, indent: usize) {
        for (k, b) in i.branches.iter().enumerate() {
            if k == 0 {
                self.s("if (");
            } else {
                self.indent(indent);
                self.s("} elseif (");
            }
            self.expr(&b.cond, 0, indent);
            self.s(") {\n");
            self.stmts(&b.body, indent + 1);
        }
        if let Some(e) = &i.else_branch {
            self.indent(indent);
            self.s("} else ");
            if let [
                Stmt {
                    kind: StmtKind::If(inner),
                    ..
                },
            ] = e.as_slice()
            {
                // `else if`: php-src tail-calls into the nested if list.
                self.if_stmt(inner, indent);
                return;
            }
            self.s("{\n");
            self.stmts(e, indent + 1);
        }
        self.indent(indent);
        self.c(b'}');
    }

    fn expr_list(&mut self, es: &[Expr], indent: usize) {
        for (i, e) in es.iter().enumerate() {
            if i != 0 {
                self.s(", ");
            }
            self.expr(e, 20, indent);
        }
    }

    // ---------------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------------

    fn binary(
        &mut self,
        op: &str,
        (p, pl, pr): (i32, i32, i32),
        l: &Expr,
        r: &Expr,
        priority: i32,
        indent: usize,
    ) {
        if priority > p {
            self.c(b'(');
        }
        self.expr(l, pl, indent);
        self.s(op);
        self.expr(r, pr, indent);
        if priority > p {
            self.c(b')');
        }
    }

    fn prefix(&mut self, op: &str, p: i32, pl: i32, e: &Expr, priority: i32, indent: usize) {
        if priority > p {
            self.c(b'(');
        }
        self.s(op);
        self.expr(e, pl, indent);
        if priority > p {
            self.c(b')');
        }
    }

    fn func_op(&mut self, op: &str, e: &Expr, indent: usize) {
        self.s(op);
        self.c(b'(');
        self.expr(e, 0, indent);
        self.c(b')');
    }

    /// `isset($a, $b)` is `ISSET($a) && ISSET($b)` in php-src's AST.
    fn isset_chain(&mut self, vars: &[Expr], priority: i32, indent: usize) {
        match vars {
            [] => {}
            [one] => self.func_op("isset", one, indent),
            [init @ .., last] => {
                let (p, pl, pr) = (130, 130, 131);
                if priority > p {
                    self.c(b'(');
                }
                self.isset_chain(init, pl, indent);
                self.s(" && ");
                let _ = pr;
                self.func_op("isset", last, indent);
                if priority > p {
                    self.c(b')');
                }
            }
        }
    }

    fn expr(&mut self, e: &Expr, priority: i32, indent: usize) {
        match &e.kind {
            ExprKind::Int(i) => self.s(&i.to_string()),
            ExprKind::Float(f) => self.s(&numeric::format_double_zero_fraction(*f, 14)),
            ExprKind::String(s) => self.quoted_str(s),
            ExprKind::Interpolated(parts) => {
                self.c(b'"');
                self.encaps(b'"', parts, indent);
                self.c(b'"');
            }
            ExprKind::ShellExec(parts) => {
                self.c(b'`');
                self.encaps(b'`', parts, indent);
                self.c(b'`');
            }
            ExprKind::Const(n) => self.ns_name(n),
            ExprKind::MagicConst(m) => self.s(m.name()),
            ExprKind::ClassConst { class, name } => {
                self.class_ref(class);
                self.s("::");
                match name {
                    ClassConstName::Ident(n) => self.b(n),
                    ClassConstName::Expr(e) => self.name_expr(e, indent),
                }
            }
            ExprKind::ClassName(class) => {
                self.class_ref(class);
                self.s("::class");
            }
            ExprKind::Var(v) => {
                self.c(b'$');
                self.var_name(v, indent);
            }
            ExprKind::Array { items, .. } => {
                self.c(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i != 0 {
                        self.s(", ");
                    }
                    if let Some(item) = item {
                        self.array_item(item, indent);
                    }
                }
                self.c(b']');
            }
            ExprKind::Dim { base, index } => {
                self.expr(base, 260, indent);
                self.c(b'[');
                if let Some(i) = index {
                    self.expr(i, 0, indent);
                }
                self.c(b']');
            }
            ExprKind::Prop {
                object,
                name,
                nullsafe,
            } => {
                self.expr(object, 0, indent);
                self.s(if *nullsafe { "?->" } else { "->" });
                self.member_name(name, indent);
            }
            ExprKind::StaticProp { class, name, .. } => {
                self.class_ref(class);
                self.s("::$");
                self.var_name(name, indent);
            }
            ExprKind::Call { callee, args } => {
                match callee {
                    Callee::Name(n) => self.ns_name(n),
                    Callee::Expr(e) => self.ns_name_or_expr(e),
                }
                self.c(b'(');
                self.args(args, indent);
                self.c(b')');
            }
            ExprKind::MethodCall {
                object,
                name,
                args,
                nullsafe,
            } => {
                self.expr(object, 0, indent);
                self.s(if *nullsafe { "?->" } else { "->" });
                self.member_name(name, indent);
                self.c(b'(');
                self.args(args, indent);
                self.c(b')');
            }
            ExprKind::StaticCall { class, name, args } => {
                match class {
                    ClassRef::Expr(c) if is_parent_hook_call(c, name) => self.expr(c, 0, indent),
                    _ => self.class_ref(class),
                }
                self.s("::");
                self.member_name(name, indent);
                self.c(b'(');
                self.args(args, indent);
                self.c(b')');
            }
            ExprKind::New { class, args } => {
                self.s("new ");
                match class {
                    NewClass::Anonymous(decl) => {
                        self.attributes(&decl.attributes, indent, false);
                        self.s("class");
                        if !args.args.is_empty() {
                            self.c(b'(');
                            self.args(args, indent);
                            self.c(b')');
                        }
                        self.class_no_header(decl, indent);
                    }
                    NewClass::Name(n) => {
                        self.ns_name(n);
                        self.c(b'(');
                        self.args(args, indent);
                        self.c(b')');
                    }
                    NewClass::Expr(c) => {
                        self.ns_name_or_expr(c);
                        self.c(b'(');
                        self.args(args, indent);
                        self.c(b')');
                    }
                }
            }
            ExprKind::Assign { target, value } => {
                self.binary(" = ", (90, 91, 90), target, value, priority, indent)
            }
            ExprKind::AssignRef { target, value } => {
                self.binary(" =& ", (90, 91, 90), target, value, priority, indent)
            }
            ExprKind::AssignOp { op, target, value } => self.binary(
                assign_op_str(*op),
                (90, 91, 90),
                target,
                value,
                priority,
                indent,
            ),
            ExprKind::AssignCoalesce { target, value } => {
                self.binary(" ??= ", (90, 91, 90), target, value, priority, indent)
            }
            ExprKind::IncDec { op, target } => match op {
                IncDecOp::PreInc => self.prefix("++", 240, 241, target, priority, indent),
                IncDecOp::PreDec => self.prefix("--", 240, 241, target, priority, indent),
                IncDecOp::PostInc | IncDecOp::PostDec => {
                    if priority > 240 {
                        self.c(b'(');
                    }
                    self.expr(target, 241, indent);
                    self.s(if *op == IncDecOp::PostInc { "++" } else { "--" });
                    if priority > 240 {
                        self.c(b')');
                    }
                }
            },
            ExprKind::Binary { op, left, right } => {
                let (s, p, pl, pr) = binary_info(*op);
                self.binary(s, (p, pl, pr), left, right, priority, indent);
            }
            ExprKind::Unary { op, expr } => {
                let s = match op {
                    UnaryOp::Not => "!",
                    UnaryOp::BitNot => "~",
                    UnaryOp::Plus => "+",
                    UnaryOp::Minus => "-",
                };
                self.prefix(s, 240, 241, expr, priority, indent);
            }
            ExprKind::Cast { kind, expr } => {
                let s = match kind {
                    CastKind::Unset => "(unset)",
                    CastKind::Bool => "(bool)",
                    CastKind::Int => "(int)",
                    CastKind::Float => "(float)",
                    CastKind::String => "(string)",
                    CastKind::Array => "(array)",
                    CastKind::Object => "(object)",
                };
                self.prefix(s, 240, 241, expr, priority, indent);
            }
            ExprKind::VoidCast(expr) => self.prefix("(void)", 240, 241, expr, priority, indent),
            ExprKind::Silence(expr) => self.prefix("@", 240, 241, expr, priority, indent),
            ExprKind::Isset(vars) => self.isset_chain(vars, priority, indent),
            ExprKind::Empty(expr) => self.func_op("empty", expr, indent),
            ExprKind::Include { kind, expr } => {
                let op = match kind {
                    IncludeKind::Include => "include",
                    IncludeKind::IncludeOnce => "include_once",
                    IncludeKind::Require => "require",
                    IncludeKind::RequireOnce => "require_once",
                };
                self.func_op(op, expr, indent);
            }
            ExprKind::Eval(expr) => self.func_op("eval", expr, indent),
            ExprKind::Print(expr) => self.prefix("print ", 60, 61, expr, priority, indent),
            ExprKind::Throw(expr) => {
                self.s("throw ");
                self.expr(expr, priority, indent);
            }
            ExprKind::Yield { key, value } => {
                if priority > 70 {
                    self.c(b'(');
                }
                self.s("yield ");
                if let Some(v) = value {
                    if let Some(k) = key {
                        self.expr(k, 70, indent);
                        self.s(" => ");
                    }
                    self.expr(v, 70, indent);
                }
                if priority > 70 {
                    self.c(b')');
                }
            }
            ExprKind::YieldFrom(expr) => self.prefix("yield from ", 85, 86, expr, priority, indent),
            ExprKind::Ternary {
                cond, then, else_, ..
            } => {
                if priority > 100 {
                    self.c(b'(');
                }
                self.expr(cond, 100, indent);
                match then {
                    Some(t) => {
                        self.s(" ? ");
                        self.expr(t, 101, indent);
                        self.s(" : ");
                    }
                    None => self.s(" ?: "),
                }
                self.expr(else_, 101, indent);
                if priority > 100 {
                    self.c(b')');
                }
            }
            ExprKind::Instanceof { expr, class } => {
                self.expr(expr, 0, indent);
                self.s(" instanceof ");
                self.class_ref(class);
            }
            ExprKind::Closure(c) => {
                let m = if c.is_static {
                    Modifiers::STATIC
                } else {
                    Modifiers::empty()
                };
                self.function_like(
                    FnKind::Closure,
                    &c.common,
                    m,
                    None,
                    &c.uses,
                    FnBody::Block(&c.body),
                    false,
                    indent,
                );
            }
            ExprKind::ArrowFn(f) => {
                let m = if f.is_static {
                    Modifiers::STATIC
                } else {
                    Modifiers::empty()
                };
                self.function_like(
                    FnKind::Arrow,
                    &f.common,
                    m,
                    None,
                    &[],
                    FnBody::Expr(&f.body),
                    f.parenthesized,
                    indent,
                );
            }
            ExprKind::Match { subject, arms } => {
                self.s("match (");
                self.expr(subject, 0, indent);
                self.s(") {\n");
                for arm in arms {
                    self.indent(indent + 1);
                    match &arm.conds {
                        Some(conds) => {
                            for (i, c) in conds.iter().enumerate() {
                                if i != 0 {
                                    self.s(", ");
                                }
                                self.expr(c, 0, indent + 1);
                            }
                            self.s(" => ");
                        }
                        None => self.s("default => "),
                    }
                    self.expr(&arm.body, 0, 0);
                    self.s(",\n");
                }
                self.indent(indent);
                self.c(b'}');
            }
        }
    }

    fn array_item(&mut self, item: &ArrayItem, indent: usize) {
        if item.unpack {
            self.s("...");
            self.expr(&item.value, 20, indent);
            return;
        }
        if let Some(k) = &item.key {
            self.expr(k, 80, indent);
            self.s(" => ");
        }
        if item.by_ref {
            self.c(b'&');
        }
        self.expr(&item.value, 80, indent);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FnKind {
    Function,
    Method,
    Closure,
    Arrow,
}

enum FnBody<'a> {
    Block(&'a [Stmt]),
    Expr(&'a Expr),
    None,
}

/// `zend_ast_is_parent_hook_call()`: `parent::$prop::get()`.
fn is_parent_hook_call(class: &Expr, method: &MemberName) -> bool {
    let ExprKind::StaticProp {
        class: ClassRef::Name(n),
        name: VarName::Name(_),
        parenthesized: false,
    } = &class.kind
    else {
        return false;
    };
    n.special_class() == Some(SpecialClass::Parent)
        && matches!(method, MemberName::Ident(m) if m.eq_ignore_case(b"get") || m.eq_ignore_case(b"set"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParseOptions, parse_with};

    /// Parse `<?php assert(EXPR);` and export the assertion argument.
    fn assert_export(code: &str) -> String {
        let src = format!("<?php assert({code});");
        let p = parse_with(
            src.as_bytes(),
            "t.php",
            &ParseOptions {
                no_resolve: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{code}: {e}"));
        let StmtKind::Expr(Expr {
            kind: ExprKind::Call { args, .. },
            ..
        }) = &p.stmts[0].kind
        else {
            panic!("not a call")
        };
        let Arg::Positional(e) = &args.args[0] else {
            panic!("not positional")
        };
        export_expr(e)
    }

    fn roundtrip(code: &str) {
        assert_eq!(assert_export(code), code);
    }

    #[test]
    fn operators_and_priorities() {
        roundtrip("$a > 0");
        roundtrip("$a + $b * $c");
        roundtrip("($a + $b) * $c");
        roundtrip("$a ** $b ** $c");
        roundtrip("($a ** $b) ** $c");
        roundtrip("!$a && $b || $c xor $d");
        roundtrip("$a ?? $b ?? $c");
        roundtrip("$a ? $b : $c");
        roundtrip("$a ?: $b");
        roundtrip("-$a");
        roundtrip("(int)$a . (string)$b");
        roundtrip("$a instanceof Foo");
        roundtrip("$x |> strlen(...)");
        roundtrip("$a <=> $b");
        roundtrip("@$a");
        roundtrip("$a = $b += 1");
        assert_eq!(assert_export("$a and $b or $c"), "$a && $b || $c");
        assert_eq!(assert_export("(($a))"), "$a");
        assert_eq!(assert_export("1 + (2 + 3)"), "1 + (2 + 3)");
        assert_eq!(assert_export("(1 + 2) + 3"), "1 + 2 + 3");
    }

    #[test]
    fn literals() {
        roundtrip("'abc'");
        assert_eq!(assert_export("\"a\\nb\""), "\"a\\nb\"");
        assert_eq!(assert_export("'it\\'s'"), "'it\\'s'");
        assert_eq!(assert_export("1.0"), "1.0");
        assert_eq!(assert_export("0.1"), "0.1");
        assert_eq!(assert_export("1e100"), "1.0E+100");
        assert_eq!(assert_export("0x1F"), "31");
        assert_eq!(assert_export("'a' . 'b'"), "'ab'");
        assert_eq!(
            assert_export("\"x $a {$b->c} ${d}\""),
            "\"x {$a} {$b->c} {$d}\""
        );
        assert_eq!(assert_export("\"\\$\""), "'$'");
        assert_eq!(assert_export("\"a\\x01\""), "\"a\\001\"");
    }

    #[test]
    fn variables_and_calls() {
        roundtrip("$a[0]->b?->c");
        assert_eq!(assert_export("$a->b::$d"), "($a->b)::$d");
        roundtrip("foo(1, ...$args)");
        roundtrip("\\foo\\bar(a: 1)");
        roundtrip("namespace\\foo()");
        roundtrip("$obj->method()");
        roundtrip("A::B");
        roundtrip("A::class");
        roundtrip("static::create()");
        roundtrip("$$a");
        roundtrip("foo(...)");
        roundtrip("[1, 2, 'a' => 3, ...$x]");
        roundtrip("[$a, , $b]");
        roundtrip("isset($a) && isset($b)");
        assert_eq!(assert_export("isset($a, $b)"), "isset($a) && isset($b)");
        assert_eq!(assert_export("array(1, 2)"), "[1, 2]");
        assert_eq!(assert_export("${'a b'}"), "${'a b'}");
        assert_eq!(assert_export("$a->{'b c'}"), "$a->{'b c'}");
        assert_eq!(assert_export("new Foo"), "new Foo()");
        assert_eq!(assert_export("exit"), "\\exit()");
        assert_eq!(assert_export("clone $a"), "\\clone($a)");
        assert_eq!(assert_export("include 'x.php'"), "include('x.php')");
        assert_eq!(assert_export("print 1"), "print 1");
        assert_eq!(assert_export("'Foo'::bar()"), "Foo::bar()");
        assert_eq!(assert_export("($a->b)()"), "($a->b)()");
    }

    #[test]
    fn functions_and_classes() {
        roundtrip("function ($a, &$b = 1) use($c, &$d): int {\n    return $a;\n}");
        roundtrip("fn($x) => $x + 1");
        roundtrip("static fn(int ...$x): ?int => null");
        assert_eq!(
            assert_export("new class(1) extends A implements B { public $x = 1; }"),
            "new class(1) extends A implements B {\n    public $x = 1;\n}"
        );
        assert_eq!(
            assert_export("function () { if ($a) { b(); } elseif ($c) { d(); } else { e(); } }"),
            "function () {\n    if ($a) {\n        b();\n    } elseif ($c) {\n        d();\n    } else {\n        e();\n    }\n}"
        );
        assert_eq!(
            assert_export("match ($x) { 1, 2 => 'a', default => 'b' }"),
            "match ($x) {\n    1, 2 => 'a',\n    default => 'b',\n}"
        );
    }

    /// `Zend/tests/assert/expect_015.phpt`
    #[test]
    fn expect_015() {
        let code = r#"0 && ($a = function () {
	global $a, $$b;
	static $c, $d = 0;
	unset($e);
	$x = isset($a) && !empty($b) || eval($c);
	$x = $a ? $b : $c;
	$x = $a ?: $c;
	$x = $a ?? $b;
	[$a, $b, $c] = [1, 2=>'x', 'z'=>'c'];
	@foo();
	$y = clone $x;
	yield 1 => 2;
	yield from $x;
})"#;
        let expected = "0 && ($a = function () {\n    global $a;\n    global $$b;\n    static $c;\n    static $d = 0;\n    unset($e);\n    $x = isset($a) && !empty($b) || eval($c);\n    $x = $a ? $b : $c;\n    $x = $a ?: $c;\n    $x = $a ?? $b;\n    [$a, $b, $c] = [1, 2 => 'x', 'z' => 'c'];\n    @foo();\n    $y = \\clone($x);\n    yield 1 => 2;\n    yield from $x;\n})";
        assert_eq!(assert_export(code), expected);
    }
}
