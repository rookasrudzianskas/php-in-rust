//! Statements (`top_statement`, `inner_statement`, `statement`).

use super::{PResult, Parser, min_line};
use crate::ast::*;
use crate::token::TokenKind as T;
use crate::{Diagnostic, DiagnosticLevel};

impl<'a> Parser<'a> {
    /// `top_statement_list`, terminated by `end` (`END` or `}`).
    pub(crate) fn top_statement_list(&mut self, end: T) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while !self.at(end) && !self.at(T::End) {
            let s = self.top_statement()?;
            stmts.push(s);
        }
        Ok(stmts)
    }

    fn top_statement(&mut self) -> PResult<Stmt> {
        match self.peek() {
            T::HaltCompiler => self.halt_compiler(true),
            T::Namespace => self.namespace_decl(),
            T::Use => self.use_decl(),
            T::Const => {
                let line = self.line();
                let consts = self.const_stmt()?;
                Ok(Stmt {
                    kind: StmtKind::Const(Box::new(consts)),
                    line,
                })
            }
            T::Attribute => {
                let attrs = self.attributes()?;
                if self.at(T::Const) {
                    let line = self.line();
                    let mut c = self.const_stmt()?;
                    c.attributes = attrs;
                    return Ok(Stmt {
                        kind: StmtKind::Const(Box::new(c)),
                        line,
                    });
                }
                self.attributed_statement_or_closure(attrs)
            }
            _ => self.inner_statement(),
        }
    }

    /// `inner_statement_list` ending before any token in `ends`.
    pub(crate) fn inner_statement_list(&mut self, ends: &[T]) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while !ends.contains(&self.peek()) && !self.at(T::End) {
            let s = self.inner_statement()?;
            stmts.push(s);
        }
        Ok(stmts)
    }

    fn inner_statement(&mut self) -> PResult<Stmt> {
        match self.peek() {
            T::HaltCompiler => self.halt_compiler(false),
            T::Attribute => {
                let attrs = self.attributes()?;
                self.attributed_statement_or_closure(attrs)
            }
            T::Function if self.is_function_decl() => self.function_decl(Vec::new()),
            T::Class | T::Trait | T::Interface | T::Enum => self.class_like_decl(Vec::new()),
            T::Abstract | T::Final => self.class_like_decl(Vec::new()),
            T::Readonly if !matches!(self.peek_at(1), T::LParen) => {
                self.class_like_decl(Vec::new())
            }
            _ => self.statement(),
        }
    }

    /// After `attributes`: a function/class declaration or an attributed
    /// closure used as an expression statement.
    fn attributed_statement_or_closure(&mut self, attrs: Vec<AttributeGroup>) -> PResult<Stmt> {
        match self.peek() {
            T::Function if self.is_function_decl() => self.function_decl(attrs),
            T::Class | T::Trait | T::Interface | T::Enum | T::Abstract | T::Final => {
                self.class_like_decl(attrs)
            }
            T::Readonly if !matches!(self.peek_at(1), T::LParen) => self.class_like_decl(attrs),
            T::Function | T::Fn | T::Static => {
                let e = self.attributed_inline_function(attrs)?;
                let e = self.continue_expr(e, 0)?;
                self.expr_statement_end(e)
            }
            _ => Err(self.syntax_error()),
        }
    }

    /// `function [&] name` vs a closure `function [&] (`.
    fn is_function_decl(&self) -> bool {
        let mut n = 1;
        if matches!(
            self.peek_at(1),
            T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg
        ) {
            n = 2;
        }
        matches!(self.peek_at(n), T::String | T::Readonly)
    }

    fn halt_compiler(&mut self, top: bool) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        self.expect(T::RParen)?;
        self.expect(T::Semicolon)?;
        let line = self.cur_line();
        if !top {
            return Err(self.error_at(
                crate::ParseErrorKind::Compile,
                "__HALT_COMPILER() can only be used from the outermost scope",
                line,
            ));
        }
        let offset = self.halt_offset().unwrap_or(self.src.len());
        Ok(Stmt {
            kind: StmtKind::HaltCompiler { offset },
            line,
        })
    }

    fn namespace_decl(&mut self) -> PResult<Stmt> {
        let line = self.line();
        self.bump();
        if self.at(T::LBrace) {
            self.reset_doc(true);
            self.bump();
            let body = self.top_statement_list(T::RBrace)?;
            self.expect(T::RBrace)?;
            return Ok(Stmt {
                kind: StmtKind::Namespace {
                    name: None,
                    body: Some(body),
                },
                line,
            });
        }
        let name = if self.at(T::NameQualified) {
            let i = self.bump();
            self.name_text(i)
        } else {
            self.identifier()?
        };
        match self.peek() {
            T::Semicolon => {
                self.bump();
                self.reset_doc(false);
                Ok(Stmt {
                    kind: StmtKind::Namespace {
                        name: Some(name),
                        body: None,
                    },
                    line,
                })
            }
            T::LBrace => {
                self.reset_doc(true);
                self.bump();
                let body = self.top_statement_list(T::RBrace)?;
                self.expect(T::RBrace)?;
                Ok(Stmt {
                    kind: StmtKind::Namespace {
                        name: Some(name),
                        body: Some(body),
                    },
                    line,
                })
            }
            _ => Err(self.syntax_error()),
        }
    }

    fn use_type(&mut self) -> Option<UseKind> {
        match self.peek() {
            T::Function => {
                self.bump();
                Some(UseKind::Function)
            }
            T::Const => {
                self.bump();
                Some(UseKind::Const)
            }
            _ => None,
        }
    }

    fn use_decl(&mut self) -> PResult<Stmt> {
        let line = self.line();
        self.bump();
        let kind = self.use_type();
        // `legacy_namespace_name`
        if !matches!(
            self.peek(),
            T::String | T::NameQualified | T::NameFullyQualified
        ) {
            return Err(self.syntax_error());
        }
        let first = self.bump();
        let first_name = self.name_text(first);
        let first_line = self.tok_line(first);
        if self.at(T::NsSeparator) {
            // group use
            self.bump();
            self.expect(T::LBrace)?;
            let mut uses = Vec::new();
            loop {
                let item_kind = if kind.is_none() {
                    self.use_type()
                } else {
                    None
                };
                if !matches!(self.peek(), T::String | T::NameQualified) {
                    return Err(self.syntax_error());
                }
                let i = self.bump();
                let name = self.name_text(i);
                let alias = self.use_alias()?;
                uses.push(UseItem {
                    name,
                    alias,
                    kind: if kind.is_none() {
                        Some(item_kind.unwrap_or(UseKind::Class))
                    } else {
                        None
                    },
                    line: self.tok_line(i),
                });
                if self.eat(T::Comma) {
                    if self.at(T::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
            self.expect(T::RBrace)?;
            self.expect(T::Semicolon)?;
            return Ok(Stmt {
                kind: StmtKind::GroupUse {
                    prefix: first_name,
                    kind,
                    uses,
                },
                line,
            });
        }
        let mut uses = Vec::new();
        let alias = self.use_alias()?;
        uses.push(UseItem {
            name: first_name,
            alias,
            kind: None,
            line: first_line,
        });
        while self.eat(T::Comma) {
            if !matches!(
                self.peek(),
                T::String | T::NameQualified | T::NameFullyQualified
            ) {
                return Err(self.syntax_error());
            }
            let i = self.bump();
            let name = self.name_text(i);
            let alias = self.use_alias()?;
            uses.push(UseItem {
                name,
                alias,
                kind: None,
                line: self.tok_line(i),
            });
        }
        self.expect(T::Semicolon)?;
        Ok(Stmt {
            kind: StmtKind::Use {
                kind: kind.unwrap_or(UseKind::Class),
                uses,
            },
            line,
        })
    }

    fn use_alias(&mut self) -> PResult<Option<ByteStr>> {
        if self.eat(T::As) {
            let i = self.expect(T::String)?;
            Ok(Some(self.name_text(i)))
        } else {
            Ok(None)
        }
    }

    /// `T_CONST const_list ';'` (top level).
    fn const_stmt(&mut self) -> PResult<ConstStmt> {
        self.bump();
        let mut consts = Vec::new();
        loop {
            let i = self.expect(T::String)?;
            self.expect(T::Equals)?;
            let value = self.expr()?;
            let doc_comment = self.take_doc(true);
            let name = self.name_text(i);
            consts.push(ConstElem {
                fq_name: name.clone(),
                name,
                value,
                doc_comment,
                line: self.tok_line(i),
            });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::Semicolon)?;
        Ok(ConstStmt {
            consts,
            attributes: Vec::new(),
        })
    }

    /// `statement`
    pub(crate) fn statement(&mut self) -> PResult<Stmt> {
        self.enter()?;
        let r = self.statement_inner();
        self.leave();
        r
    }

    /// Dispatch on the statement keyword; each form lives in its own
    /// (small-framed) function since statements nest recursively.
    fn statement_inner(&mut self) -> PResult<Stmt> {
        match self.peek() {
            T::LBrace => self.block_stmt(),
            T::If => self.if_stmt(),
            T::While => self.while_stmt(),
            T::Do => self.do_stmt(),
            T::For => self.for_stmt(),
            T::Switch => self.switch_stmt(),
            T::Break | T::Continue | T::Return => self.jump_stmt(),
            T::Global => self.global_stmt(),
            T::Static if self.peek_at(1) == T::Variable => self.static_stmt(),
            T::Echo => self.echo_stmt(),
            T::InlineHtml => {
                let line = self.line();
                let i = self.bump();
                Ok(Stmt {
                    kind: StmtKind::InlineHtml(ByteStr::from(self.text(i))),
                    line,
                })
            }
            T::Unset => self.unset_stmt(),
            T::Foreach => self.foreach_stmt(),
            T::Declare => self.declare_stmt(),
            T::Semicolon => {
                self.bump();
                Ok(Stmt {
                    kind: StmtKind::Nop,
                    line: self.cur_line(),
                })
            }
            T::Try => self.try_stmt(),
            T::Goto => self.goto_stmt(),
            T::String if self.peek_at(1) == T::Colon => {
                let line = self.line();
                let i = self.bump();
                self.bump();
                Ok(Stmt {
                    kind: StmtKind::Label(self.name_text(i)),
                    line,
                })
            }
            T::VoidCast => self.void_cast_stmt(),
            _ => self.expr_stmt(),
        }
    }

    #[inline(never)]
    fn block_stmt(&mut self) -> PResult<Stmt> {
        let line = self.line();
        self.bump();
        let body = self.inner_statement_list(&[T::RBrace])?;
        self.expect(T::RBrace)?;
        Ok(Stmt {
            kind: StmtKind::Block(body),
            line,
        })
    }

    #[inline(never)]
    fn while_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        let cond = self.expr()?;
        self.expect(T::RParen)?;
        let body = self.alt_body(T::EndWhile)?;
        let line = cond.line;
        Ok(Stmt {
            kind: StmtKind::While { cond, body },
            line,
        })
    }

    #[inline(never)]
    fn do_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        let body = self.statement_body()?;
        self.expect(T::While)?;
        self.expect(T::LParen)?;
        let cond = self.expr()?;
        self.expect(T::RParen)?;
        self.expect(T::Semicolon)?;
        let line = body
            .first()
            .map_or(cond.line, |s| min_line(s.line, cond.line));
        Ok(Stmt {
            kind: StmtKind::DoWhile { body, cond },
            line,
        })
    }

    #[inline(never)]
    fn jump_stmt(&mut self) -> PResult<Stmt> {
        let k = self.peek();
        self.bump();
        let e = if self.at(T::Semicolon) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect(T::Semicolon)?;
        let line = e.as_ref().map_or(self.cur_line(), |e| e.line);
        let kind = match k {
            T::Break => StmtKind::Break(e),
            T::Continue => StmtKind::Continue(e),
            _ => StmtKind::Return(e),
        };
        Ok(Stmt { kind, line })
    }

    #[inline(never)]
    fn global_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        let mut vars = Vec::new();
        loop {
            vars.push(self.simple_variable_expr()?);
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::Semicolon)?;
        let line = vars[0].line;
        Ok(Stmt {
            kind: StmtKind::Global(vars),
            line,
        })
    }

    #[inline(never)]
    fn static_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        let mut vars = Vec::new();
        loop {
            let i = self.expect(T::Variable)?;
            let default = if self.eat(T::Equals) {
                Some(self.expr()?)
            } else {
                None
            };
            vars.push(StaticVar {
                name: self.name_text(i),
                default,
                line: self.tok_line(i),
            });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::Semicolon)?;
        let line = vars[0].line;
        Ok(Stmt {
            kind: StmtKind::Static(vars),
            line,
        })
    }

    #[inline(never)]
    fn echo_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        let mut exprs = Vec::new();
        loop {
            exprs.push(self.expr()?);
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::Semicolon)?;
        let line = exprs[0].line;
        Ok(Stmt {
            kind: StmtKind::Echo(exprs),
            line,
        })
    }

    #[inline(never)]
    fn unset_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        let mut vars = Vec::new();
        loop {
            vars.push(self.variable()?);
            if self.eat(T::Comma) {
                if self.at(T::RParen) {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(T::RParen)?;
        self.expect(T::Semicolon)?;
        let line = vars[0].line;
        Ok(Stmt {
            kind: StmtKind::Unset(vars),
            line,
        })
    }

    #[inline(never)]
    fn goto_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        let i = self.expect(T::String)?;
        self.expect(T::Semicolon)?;
        Ok(Stmt {
            kind: StmtKind::Goto(self.name_text(i)),
            line: self.tok_line(i),
        })
    }

    #[inline(never)]
    fn void_cast_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        let e = self.expr()?;
        self.expect(T::Semicolon)?;
        let line = e.line;
        let e = Expr::new(ExprKind::VoidCast(Box::new(e)), line);
        Ok(Stmt {
            kind: StmtKind::Expr(e),
            line,
        })
    }

    #[inline(never)]
    fn expr_stmt(&mut self) -> PResult<Stmt> {
        let e = self.expr()?;
        self.expr_statement_end(e)
    }

    pub(crate) fn expr_statement_end(&mut self, e: Expr) -> PResult<Stmt> {
        self.expect(T::Semicolon)?;
        let line = e.line;
        Ok(Stmt {
            kind: StmtKind::Expr(e),
            line,
        })
    }

    /// A `statement` used as a body: braces are flattened.
    pub(crate) fn statement_body(&mut self) -> PResult<Block> {
        let s = self.statement()?;
        Ok(match s.kind {
            StmtKind::Block(b) => b,
            StmtKind::Nop => Vec::new(),
            _ => vec![s],
        })
    }

    /// `statement | ':' inner_statement_list END_KW ';'`
    fn alt_body(&mut self, end_kw: T) -> PResult<Block> {
        if self.eat(T::Colon) {
            let body = self.inner_statement_list(&[end_kw])?;
            self.expect(end_kw)?;
            self.expect(T::Semicolon)?;
            Ok(body)
        } else {
            self.statement_body()
        }
    }

    #[inline(never)]
    fn if_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        let cond = self.expr()?;
        self.expect(T::RParen)?;
        let line = cond.line;
        if self.eat(T::Colon) {
            // alternative syntax
            let body = self.inner_statement_list(&[T::ElseIf, T::Else, T::EndIf])?;
            let mut branches = vec![IfBranch {
                line: cond.line,
                cond,
                body,
            }];
            let mut else_branch = None;
            loop {
                match self.peek() {
                    T::ElseIf => {
                        self.bump();
                        self.expect(T::LParen)?;
                        let cond = self.expr()?;
                        self.expect(T::RParen)?;
                        self.expect(T::Colon)?;
                        let body = self.inner_statement_list(&[T::ElseIf, T::Else, T::EndIf])?;
                        branches.push(IfBranch {
                            line: cond.line,
                            cond,
                            body,
                        });
                    }
                    T::Else => {
                        self.bump();
                        self.expect(T::Colon)?;
                        let body = self.inner_statement_list(&[T::EndIf])?;
                        else_branch = Some(wrap_single_if(body));
                        break;
                    }
                    _ => break,
                }
            }
            self.expect(T::EndIf)?;
            self.expect(T::Semicolon)?;
            return Ok(Stmt {
                kind: StmtKind::If(Box::new(IfStmt {
                    branches,
                    else_branch,
                })),
                line,
            });
        }
        let body = self.statement_body()?;
        let mut branches = vec![IfBranch {
            line: cond.line,
            cond,
            body,
        }];
        let mut else_branch = None;
        loop {
            match self.peek() {
                T::ElseIf => {
                    self.bump();
                    self.expect(T::LParen)?;
                    let cond = self.expr()?;
                    self.expect(T::RParen)?;
                    let body = self.statement_body()?;
                    branches.push(IfBranch {
                        line: cond.line,
                        cond,
                        body,
                    });
                }
                T::Else => {
                    self.bump();
                    let braced = self.at(T::LBrace);
                    let body = self.statement_body()?;
                    else_branch = Some(if braced { wrap_single_if(body) } else { body });
                    break;
                }
                _ => break,
            }
        }
        Ok(Stmt {
            kind: StmtKind::If(Box::new(IfStmt {
                branches,
                else_branch,
            })),
            line,
        })
    }

    fn for_exprs(&mut self, end: T, is_cond: bool) -> PResult<Vec<Expr>> {
        let mut v = Vec::new();
        if self.at(end) {
            return Ok(v);
        }
        loop {
            if self.at(T::VoidCast) {
                self.bump();
                let e = self.expr()?;
                let line = e.line;
                v.push(Expr::new(ExprKind::VoidCast(Box::new(e)), line));
                if is_cond && !self.at(T::Comma) {
                    // `for_cond_exprs` requires the last element to be a plain expr.
                    return Err(self.syntax_error());
                }
            } else {
                v.push(self.expr()?);
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        Ok(v)
    }

    #[inline(never)]
    fn for_stmt(&mut self) -> PResult<Stmt> {
        let kw_line = self.line();
        self.bump();
        self.expect(T::LParen)?;
        let init = self.for_exprs(T::Semicolon, false)?;
        self.expect(T::Semicolon)?;
        let cond = self.for_exprs(T::Semicolon, true)?;
        self.expect(T::Semicolon)?;
        let step = self.for_exprs(T::RParen, false)?;
        self.expect(T::RParen)?;
        let body = self.alt_body(T::EndFor)?;
        let line = init
            .first()
            .or(cond.first())
            .or(step.first())
            .map(|e| e.line)
            .or(body.first().map(|s| s.line))
            .unwrap_or(kw_line);
        Ok(Stmt {
            kind: StmtKind::For(Box::new(ForStmt {
                init,
                cond,
                step,
                body,
            })),
            line,
        })
    }

    fn foreach_target(&mut self) -> PResult<ForeachTarget> {
        match self.peek() {
            T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg => {
                self.bump();
                Ok(ForeachTarget {
                    expr: self.variable()?,
                    by_ref: true,
                })
            }
            T::List => {
                let line = self.line();
                self.bump();
                self.expect(T::LParen)?;
                let items = self.array_pair_list(T::RParen)?;
                let expr = Expr::new(
                    ExprKind::Array {
                        items,
                        syntax: ArraySyntax::List,
                    },
                    line,
                );
                Ok(ForeachTarget {
                    expr,
                    by_ref: false,
                })
            }
            T::LBracket => {
                let line = self.line();
                self.bump();
                let items = self.array_pair_list(T::RBracket)?;
                let expr = Expr::new(
                    ExprKind::Array {
                        items,
                        syntax: ArraySyntax::Short,
                    },
                    line,
                );
                if matches!(self.peek(), T::RParen | T::DoubleArrow) {
                    return Ok(ForeachTarget {
                        expr,
                        by_ref: false,
                    });
                }
                // `[...][0]` etc.: a variable starting with an array literal.
                let e = self.postfix_from(expr, super::expr::Shape::Callable)?;
                if !e.1.is_variable() {
                    return Err(self.syntax_error());
                }
                Ok(ForeachTarget {
                    expr: e.0,
                    by_ref: false,
                })
            }
            _ => Ok(ForeachTarget {
                expr: self.variable()?,
                by_ref: false,
            }),
        }
    }

    #[inline(never)]
    fn foreach_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        let expr = self.expr()?;
        self.expect(T::As)?;
        let first = self.foreach_target()?;
        let (key, value) = if self.eat(T::DoubleArrow) {
            let v = self.foreach_target()?;
            (Some(first), v)
        } else {
            (None, first)
        };
        self.expect(T::RParen)?;
        let body = self.alt_body(T::EndForeach)?;
        let line = expr.line;
        Ok(Stmt {
            kind: StmtKind::Foreach(Box::new(ForeachStmt {
                expr,
                key,
                value,
                body,
            })),
            line,
        })
    }

    #[inline(never)]
    fn switch_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        let subject = self.expr()?;
        self.expect(T::RParen)?;
        let alt = match self.peek() {
            T::LBrace => false,
            T::Colon => true,
            _ => return Err(self.syntax_error()),
        };
        self.bump();
        self.eat(T::Semicolon);
        let end = if alt { T::EndSwitch } else { T::RBrace };
        let mut cases = Vec::new();
        loop {
            match self.peek() {
                T::Case => {
                    let line = self.line();
                    self.bump();
                    let cond = self.expr()?;
                    let semicolon = match self.peek() {
                        T::Colon => false,
                        T::Semicolon => true,
                        _ => return Err(self.syntax_error()),
                    };
                    self.bump();
                    let body = self.inner_statement_list(&[T::Case, T::Default, end])?;
                    let line = min_line(line, cond.line);
                    cases.push(SwitchCase {
                        cond: Some(cond),
                        body,
                        semicolon,
                        line,
                    });
                }
                T::Default => {
                    let line = self.line();
                    self.bump();
                    let semicolon = match self.peek() {
                        T::Colon => false,
                        T::Semicolon => true,
                        _ => return Err(self.syntax_error()),
                    };
                    self.bump();
                    let body = self.inner_statement_list(&[T::Case, T::Default, end])?;
                    cases.push(SwitchCase {
                        cond: None,
                        body,
                        semicolon,
                        line,
                    });
                }
                k if k == end => break,
                _ => return Err(self.syntax_error()),
            }
        }
        self.bump();
        if alt {
            self.expect(T::Semicolon)?;
        }
        let line = subject.line;
        Ok(Stmt {
            kind: StmtKind::Switch { subject, cases },
            line,
        })
    }

    #[inline(never)]
    fn declare_stmt(&mut self) -> PResult<Stmt> {
        self.bump();
        self.expect(T::LParen)?;
        let mut directives = Vec::new();
        loop {
            let i = self.expect(T::String)?;
            self.expect(T::Equals)?;
            let value = self.expr()?;
            // `backup_doc_comment` of the const_decl.
            self.take_doc(true);
            directives.push(DeclareDirective {
                name: self.name_text(i),
                value,
                line: self.tok_line(i),
            });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::RParen)?;
        // zend_handle_encoding_declaration(), run while parsing.
        for d in &directives {
            if d.name.eq_ignore_case(b"encoding") {
                if !matches!(
                    d.value.kind,
                    ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::String(_)
                ) {
                    return Err(self.error_at(
                        crate::ParseErrorKind::Compile,
                        "Encoding must be a literal",
                        self.cur_line(),
                    ));
                }
                let line = self.cur_line();
                self.add_diagnostic(Diagnostic {
                    level: DiagnosticLevel::CompileWarning,
                    message: "declare(encoding=...) ignored because Zend multibyte feature is turned off by settings"
                        .into(),
                    line,
                });
            }
        }
        let body = if self.eat(T::Colon) {
            let body = self.inner_statement_list(&[T::EndDeclare])?;
            self.expect(T::EndDeclare)?;
            self.expect(T::Semicolon)?;
            Some(body)
        } else {
            let s = self.statement()?;
            match s.kind {
                StmtKind::Nop => None,
                StmtKind::Block(b) => Some(b),
                _ => Some(vec![s]),
            }
        };
        let line = directives[0].line;
        Ok(Stmt {
            kind: StmtKind::Declare { directives, body },
            line,
        })
    }

    #[inline(never)]
    fn try_stmt(&mut self) -> PResult<Stmt> {
        let line = self.line();
        self.bump();
        self.expect(T::LBrace)?;
        let body = self.inner_statement_list(&[T::RBrace])?;
        self.expect(T::RBrace)?;
        let mut catches = Vec::new();
        while self.at(T::Catch) {
            let cline = self.line();
            self.bump();
            self.expect(T::LParen)?;
            let mut types = vec![self.class_name()?];
            while self.eat(T::Bar) {
                types.push(self.class_name()?);
            }
            let var = if self.at(T::Variable) {
                let i = self.bump();
                Some(self.name_text(i))
            } else {
                None
            };
            self.expect(T::RParen)?;
            self.expect(T::LBrace)?;
            let cbody = self.inner_statement_list(&[T::RBrace])?;
            self.expect(T::RBrace)?;
            catches.push(Catch {
                types,
                var,
                body: cbody,
                line: cline,
            });
        }
        let finally = if self.eat(T::Finally) {
            self.expect(T::LBrace)?;
            let f = self.inner_statement_list(&[T::RBrace])?;
            self.expect(T::RBrace)?;
            Some(f)
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::Try(Box::new(TryStmt {
                body,
                catches,
                finally,
            })),
            line,
        })
    }

    /// `global_var: simple_variable`
    fn simple_variable_expr(&mut self) -> PResult<Expr> {
        let line = self.line();
        let name = self.simple_variable()?;
        Ok(Expr::new(ExprKind::Var(name), line))
    }
}

/// php-src keeps `else { if ... }` as a statement list but `else if` as a
/// bare `if`; preserve the difference (it matters for AST export).
fn wrap_single_if(body: Block) -> Block {
    if body.len() == 1 && matches!(body[0].kind, StmtKind::If(_)) {
        let line = body[0].line;
        vec![Stmt {
            kind: StmtKind::Block(body),
            line,
        }]
    } else {
        body
    }
}
