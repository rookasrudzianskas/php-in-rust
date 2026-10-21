//! Expressions: a Pratt parser following the precedence declarations of
//! `zend_language_parser.y`, plus the `variable` / dereferencing grammar.

use super::{PResult, Parser, min_line};
use crate::ast::*;
use crate::lexer::TokVal;
use crate::numeric;
use crate::token::TokenKind as T;

/// Precedence levels, lowest first (one per precedence declaration).
pub(crate) mod prec {
    pub const THROW: u8 = 1;
    pub const ARROW_FUNCTION: u8 = 2;
    pub const INCLUDE: u8 = 3;
    pub const LOGICAL_OR: u8 = 4;
    pub const LOGICAL_XOR: u8 = 5;
    pub const LOGICAL_AND: u8 = 6;
    pub const PRINT: u8 = 7;
    pub const YIELD: u8 = 8;
    pub const DOUBLE_ARROW: u8 = 9;
    pub const YIELD_FROM: u8 = 10;
    pub const ASSIGN: u8 = 11;
    pub const TERNARY: u8 = 12;
    pub const COALESCE: u8 = 13;
    pub const BOOLEAN_OR: u8 = 14;
    pub const BOOLEAN_AND: u8 = 15;
    pub const BIT_OR: u8 = 16;
    pub const BIT_XOR: u8 = 17;
    pub const BIT_AND: u8 = 18;
    pub const EQUALITY: u8 = 19;
    pub const COMPARISON: u8 = 20;
    pub const PIPE: u8 = 21;
    pub const CONCAT: u8 = 22;
    pub const SHIFT: u8 = 23;
    pub const ADDITIVE: u8 = 24;
    pub const MULTIPLICATIVE: u8 = 25;
    pub const NOT: u8 = 26;
    pub const INSTANCEOF: u8 = 27;
    pub const UNARY: u8 = 28;
    pub const POW: u8 = 29;
    pub const CLONE: u8 = 30;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Assoc {
    Left,
    Right,
    NonAssociative,
}

enum Infix {
    Binary(BinaryOp),
    Ternary,
    Instanceof,
}

fn infix_info(k: T) -> Option<(u8, Assoc, Infix)> {
    use Assoc::*;
    use BinaryOp as B;
    use prec::*;
    let b = Infix::Binary;
    Some(match k {
        T::LogicalOr => (LOGICAL_OR, Left, b(B::BooleanOr)),
        T::LogicalXor => (LOGICAL_XOR, Left, b(B::LogicalXor)),
        T::LogicalAnd => (LOGICAL_AND, Left, b(B::BooleanAnd)),
        T::Question => (TERNARY, Left, Infix::Ternary),
        T::Coalesce => (COALESCE, Right, b(B::Coalesce)),
        T::BooleanOr => (BOOLEAN_OR, Left, b(B::BooleanOr)),
        T::BooleanAnd => (BOOLEAN_AND, Left, b(B::BooleanAnd)),
        T::Bar => (BIT_OR, Left, b(B::BitOr)),
        T::Caret => (BIT_XOR, Left, b(B::BitXor)),
        T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg => {
            (BIT_AND, Left, b(B::BitAnd))
        }
        T::IsEqual => (EQUALITY, NonAssociative, b(B::Equal)),
        T::IsNotEqual => (EQUALITY, NonAssociative, b(B::NotEqual)),
        T::IsIdentical => (EQUALITY, NonAssociative, b(B::Identical)),
        T::IsNotIdentical => (EQUALITY, NonAssociative, b(B::NotIdentical)),
        T::Spaceship => (EQUALITY, NonAssociative, b(B::Spaceship)),
        T::Lt => (COMPARISON, NonAssociative, b(B::Less)),
        T::IsSmallerOrEqual => (COMPARISON, NonAssociative, b(B::LessEqual)),
        T::Gt => (COMPARISON, NonAssociative, b(B::Greater)),
        T::IsGreaterOrEqual => (COMPARISON, NonAssociative, b(B::GreaterEqual)),
        T::Pipe => (PIPE, Left, b(B::Pipe)),
        T::Dot => (CONCAT, Left, b(B::Concat)),
        T::Sl => (SHIFT, Left, b(B::ShiftLeft)),
        T::Sr => (SHIFT, Left, b(B::ShiftRight)),
        T::Plus => (ADDITIVE, Left, b(B::Add)),
        T::Minus => (ADDITIVE, Left, b(B::Sub)),
        T::Star => (MULTIPLICATIVE, Left, b(B::Mul)),
        T::Slash => (MULTIPLICATIVE, Left, b(B::Div)),
        T::Percent => (MULTIPLICATIVE, Left, b(B::Mod)),
        T::Instanceof => (INSTANCEOF, Left, Infix::Instanceof),
        T::Pow => (POW, Right, b(B::Pow)),
        _ => return None,
    })
}

fn assign_op(k: T) -> Option<Option<BinaryOp>> {
    use BinaryOp as B;
    Some(Some(match k {
        T::Equals => return Some(None),
        T::PlusEqual => B::Add,
        T::MinusEqual => B::Sub,
        T::MulEqual => B::Mul,
        T::PowEqual => B::Pow,
        T::DivEqual => B::Div,
        T::ConcatEqual => B::Concat,
        T::ModEqual => B::Mod,
        T::AndEqual => B::BitAnd,
        T::OrEqual => B::BitOr,
        T::XorEqual => B::BitXor,
        T::SlEqual => B::ShiftLeft,
        T::SrEqual => B::ShiftRight,
        T::CoalesceEqual => return Some(Some(B::Coalesce)),
        _ => return None,
    }))
}

/// `FIRST(expr)`: tokens that can start an expression.
/// Tokens handled by `unary_prefix`.
fn is_prefix_start(k: T, next: T) -> bool {
    matches!(
        k,
        T::Bang
            | T::Tilde
            | T::Plus
            | T::Minus
            | T::At
            | T::IntCast
            | T::DoubleCast
            | T::StringCast
            | T::ArrayCast
            | T::ObjectCast
            | T::BoolCast
            | T::UnsetCast
            | T::Print
            | T::Throw
            | T::Include
            | T::IncludeOnce
            | T::Require
            | T::RequireOnce
            | T::YieldFrom
            | T::Yield
            | T::Inc
            | T::Dec
            | T::Clone
            | T::Function
            | T::Fn
            | T::Attribute
            | T::List
    ) || (k == T::Static && matches!(next, T::Function | T::Fn))
}

pub(crate) fn can_start_expr(k: T) -> bool {
    matches!(
        k,
        T::Variable
            | T::Dollar
            | T::String
            | T::NameQualified
            | T::NameFullyQualified
            | T::NameRelative
            | T::Static
            | T::LParen
            | T::Array
            | T::LBracket
            | T::ConstantEncapsedString
            | T::DoubleQuote
            | T::New
            | T::Readonly
            | T::Line
            | T::File
            | T::Dir
            | T::TraitC
            | T::MethodC
            | T::FuncC
            | T::PropertyC
            | T::NsC
            | T::ClassC
            | T::List
            | T::Clone
            | T::Inc
            | T::Dec
            | T::Plus
            | T::Minus
            | T::Bang
            | T::Tilde
            | T::IntCast
            | T::DoubleCast
            | T::StringCast
            | T::ArrayCast
            | T::ObjectCast
            | T::BoolCast
            | T::UnsetCast
            | T::Exit
            | T::At
            | T::LNumber
            | T::DNumber
            | T::StartHeredoc
            | T::Backtick
            | T::Print
            | T::Yield
            | T::YieldFrom
            | T::Throw
            | T::Function
            | T::Fn
            | T::Attribute
            | T::Match
            | T::Isset
            | T::Empty
            | T::Include
            | T::IncludeOnce
            | T::Eval
            | T::Require
            | T::RequireOnce
    )
}

/// What kind of grammar symbol an expression was reduced as; decides which
/// dereferencing operators may follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// `callable_variable`: `$a`, `$a[0]`, calls. Assignable, callable.
    Var,
    /// `$a->b`, `A::$b`: assignable but not callable.
    Prop,
    /// `callable_expr` that is not a variable: `(expr)`, dereferenceable
    /// scalars, `new A()`.
    Callable,
    /// A short array literal `[...]` (may become a destructuring target).
    ShortArray,
    /// `'(' expr ')'` before we know which grammar rule it belongs to.
    Paren,
    /// `constant`: a name.
    Constant,
    /// A magic constant.
    MagicConst,
    /// `class_constant`
    ClassConstant,
    /// Anything else (no dereferencing allowed).
    Plain,
}

impl Shape {
    pub(crate) fn is_variable(self) -> bool {
        matches!(self, Shape::Var | Shape::Prop)
    }

    /// `array_object_dereferenceable` (`[`, `->`, `?->` allowed).
    fn array_object_deref(self) -> bool {
        !matches!(self, Shape::Plain)
    }

    /// `fully_dereferenceable` (may be followed by `::`).
    fn fully_deref(self) -> bool {
        matches!(
            self,
            Shape::Var
                | Shape::Prop
                | Shape::Callable
                | Shape::ShortArray
                | Shape::ClassConstant
                | Shape::Paren
        )
    }

    /// `callable_expr` (may be followed by an argument list).
    fn callable(self) -> bool {
        matches!(
            self,
            Shape::Var | Shape::Callable | Shape::ShortArray | Shape::Paren
        )
    }
}

fn bx(e: Expr) -> Box<Expr> {
    Box::new(e)
}

impl<'a> Parser<'a> {
    /// `expr`
    pub(crate) fn expr(&mut self) -> PResult<Expr> {
        self.expr_bp(0)
    }

    /// Parse an expression whose enclosing rule has precedence `ctx`.
    pub(crate) fn expr_bp(&mut self, ctx: u8) -> PResult<Expr> {
        self.enter()?;
        let left = self.unary()?;
        let r = self.binary_loop(left, ctx);
        self.leave();
        r
    }

    /// Continue parsing after an already-parsed operand.
    pub(crate) fn continue_expr(&mut self, left: Expr, ctx: u8) -> PResult<Expr> {
        self.binary_loop(left, ctx)
    }

    fn binary_loop(&mut self, mut left: Expr, ctx: u8) -> PResult<Expr> {
        loop {
            let k = self.peek();
            let Some((p, assoc, infix)) = infix_info(k) else {
                break;
            };
            if p < ctx || (p == ctx && assoc == Assoc::Left) {
                break;
            }
            if p == ctx && assoc == Assoc::NonAssociative {
                return Err(self.syntax_error());
            }
            self.bump();
            left = match infix {
                Infix::Binary(op) => {
                    let right = self.expr_bp(p)?;
                    self.make_binary(op, left, right)
                }
                Infix::Ternary => self.ternary_rest(left, p)?,
                Infix::Instanceof => self.instanceof_rest(left)?,
            };
        }
        Ok(left)
    }

    #[inline(never)]
    fn ternary_rest(&mut self, left: Expr, p: u8) -> PResult<Expr> {
        if self.eat(T::Colon) {
            let else_ = self.expr_bp(p)?;
            let line = min_line(left.line, else_.line);
            return Ok(Expr::new(
                ExprKind::Ternary {
                    cond: bx(left),
                    then: None,
                    else_: bx(else_),
                    parenthesized: false,
                },
                line,
            ));
        }
        let then = self.expr()?;
        self.expect(T::Colon)?;
        let else_ = self.expr_bp(p)?;
        let line = min_line(left.line, min_line(then.line, else_.line));
        Ok(Expr::new(
            ExprKind::Ternary {
                cond: bx(left),
                then: Some(bx(then)),
                else_: bx(else_),
                parenthesized: false,
            },
            line,
        ))
    }

    #[inline(never)]
    fn instanceof_rest(&mut self, left: Expr) -> PResult<Expr> {
        let class = self.class_name_reference()?;
        let line = min_line(left.line, class_ref_line(&class));
        Ok(Expr::new(
            ExprKind::Instanceof {
                expr: bx(left),
                class,
            },
            line,
        ))
    }

    fn make_binary(&mut self, op: BinaryOp, left: Expr, right: Expr) -> Expr {
        let line = min_line(left.line, right.line);
        if op == BinaryOp::Concat {
            // zend_ast_create_concat_op(): fold literal operands.
            if let (Some(a), Some(b)) = (literal_str(&left), literal_str(&right)) {
                let mut s = a;
                s.extend_from_slice(&b);
                return Expr::new(ExprKind::String(ByteStr::from(s)), self.la_line());
            }
        }
        Expr::new(
            ExprKind::Binary {
                op,
                left: bx(left),
                right: bx(right),
            },
            line,
        )
    }

    fn prefix(&mut self, p: u8, f: impl FnOnce(Box<Expr>) -> ExprKind) -> PResult<Expr> {
        self.bump();
        let e = self.expr_bp(p)?;
        let line = e.line;
        Ok(Expr::new(f(bx(e)), line))
    }

    /// Prefix operators, or an operand followed by assignment / `++`.
    fn unary(&mut self) -> PResult<Expr> {
        // Keep the operand path (taken at every nesting level) out of the
        // large prefix-operator frame.
        if is_prefix_start(self.peek(), self.peek_at(1)) {
            return self.unary_prefix();
        }
        let (e, shape) = self.postfix_expr()?;
        self.finish_operand(e, shape)
    }

    #[inline(never)]
    fn unary_prefix(&mut self) -> PResult<Expr> {
        use prec::*;
        match self.peek() {
            T::Bang => self.prefix(NOT, |e| ExprKind::Unary {
                op: UnaryOp::Not,
                expr: e,
            }),
            T::Tilde => self.prefix(UNARY, |e| ExprKind::Unary {
                op: UnaryOp::BitNot,
                expr: e,
            }),
            T::Plus => self.prefix(UNARY, |e| ExprKind::Unary {
                op: UnaryOp::Plus,
                expr: e,
            }),
            T::Minus => self.prefix(UNARY, |e| ExprKind::Unary {
                op: UnaryOp::Minus,
                expr: e,
            }),
            T::At => self.prefix(UNARY, ExprKind::Silence),
            T::IntCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::Int,
                expr: e,
            }),
            T::DoubleCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::Float,
                expr: e,
            }),
            T::StringCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::String,
                expr: e,
            }),
            T::ArrayCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::Array,
                expr: e,
            }),
            T::ObjectCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::Object,
                expr: e,
            }),
            T::BoolCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::Bool,
                expr: e,
            }),
            T::UnsetCast => self.prefix(UNARY, |e| ExprKind::Cast {
                kind: CastKind::Unset,
                expr: e,
            }),
            T::Print => self.prefix(PRINT, ExprKind::Print),
            T::Throw => self.prefix(THROW, ExprKind::Throw),
            T::Include => self.prefix(INCLUDE, |e| ExprKind::Include {
                kind: IncludeKind::Include,
                expr: e,
            }),
            T::IncludeOnce => self.prefix(INCLUDE, |e| ExprKind::Include {
                kind: IncludeKind::IncludeOnce,
                expr: e,
            }),
            T::Require => self.prefix(INCLUDE, |e| ExprKind::Include {
                kind: IncludeKind::Require,
                expr: e,
            }),
            T::RequireOnce => self.prefix(INCLUDE, |e| ExprKind::Include {
                kind: IncludeKind::RequireOnce,
                expr: e,
            }),
            T::YieldFrom => {
                self.mark_generator();
                self.prefix(YIELD_FROM, ExprKind::YieldFrom)
            }
            T::Yield => self.yield_expr(),
            T::Inc | T::Dec => self.pre_inc_dec(),
            T::Clone => self.clone_expr(),
            T::Function | T::Fn => self.inline_function(false, Vec::new()),
            T::Static if matches!(self.peek_at(1), T::Function | T::Fn) => {
                self.bump();
                self.inline_function(true, Vec::new())
            }
            T::Attribute => {
                let attrs = self.attributes()?;
                self.attributed_inline_function(attrs)
            }
            T::List => self.list_assign(),
            _ => {
                let (e, shape) = self.postfix_expr()?;
                self.finish_operand(e, shape)
            }
        }
    }

    #[inline(never)]
    fn yield_expr(&mut self) -> PResult<Expr> {
        use prec::*;
        self.bump();
        self.mark_generator();
        if !can_start_expr(self.peek()) {
            return Ok(Expr::new(
                ExprKind::Yield {
                    key: None,
                    value: None,
                },
                self.cur_line(),
            ));
        }
        let v = self.expr_bp(YIELD)?;
        if self.eat(T::DoubleArrow) {
            let value = self.expr_bp(DOUBLE_ARROW)?;
            let line = min_line(v.line, value.line);
            return Ok(Expr::new(
                ExprKind::Yield {
                    key: Some(bx(v)),
                    value: Some(bx(value)),
                },
                line,
            ));
        }
        let line = v.line;
        Ok(Expr::new(
            ExprKind::Yield {
                key: None,
                value: Some(bx(v)),
            },
            line,
        ))
    }

    #[inline(never)]
    fn pre_inc_dec(&mut self) -> PResult<Expr> {
        let op = if self.at(T::Inc) {
            IncDecOp::PreInc
        } else {
            IncDecOp::PreDec
        };
        self.bump();
        let target = self.variable()?;
        let line = target.line;
        Ok(Expr::new(
            ExprKind::IncDec {
                op,
                target: bx(target),
            },
            line,
        ))
    }

    /// `T_LIST '(' array_pair_list ')' '=' expr`
    #[inline(never)]
    fn list_assign(&mut self) -> PResult<Expr> {
        let line = self.line();
        self.bump();
        self.expect(T::LParen)?;
        let items = self.array_pair_list(T::RParen)?;
        let target = Expr::new(
            ExprKind::Array {
                items,
                syntax: ArraySyntax::List,
            },
            line,
        );
        self.expect(T::Equals)?;
        let value = self.expr_bp(prec::ASSIGN)?;
        let line = min_line(line, value.line);
        Ok(Expr::new(
            ExprKind::Assign {
                target: bx(target),
                value: bx(value),
            },
            line,
        ))
    }

    /// `attributes inline_function` / `attributes T_STATIC inline_function`
    pub(crate) fn attributed_inline_function(
        &mut self,
        attrs: Vec<AttributeGroup>,
    ) -> PResult<Expr> {
        match self.peek() {
            T::Function | T::Fn => self.inline_function(false, attrs),
            T::Static if matches!(self.peek_at(1), T::Function | T::Fn) => {
                self.bump();
                self.inline_function(true, attrs)
            }
            _ => Err(self.syntax_error()),
        }
    }

    /// After an operand: assignment, destructuring or postfix `++`/`--`.
    pub(crate) fn finish_operand(&mut self, e: Expr, shape: Shape) -> PResult<Expr> {
        let k = self.peek();
        if shape.is_variable() {
            if let Some(op) = assign_op(k) {
                self.bump();
                return self.assignment(e, op);
            }
            if k == T::Inc || k == T::Dec {
                self.bump();
                let op = if k == T::Inc {
                    IncDecOp::PostInc
                } else {
                    IncDecOp::PostDec
                };
                let line = e.line;
                return Ok(Expr::new(ExprKind::IncDec { op, target: bx(e) }, line));
            }
        } else if shape == Shape::ShortArray && k == T::Equals {
            self.bump();
            let value = self.expr_bp(prec::ASSIGN)?;
            let line = min_line(e.line, value.line);
            return Ok(Expr::new(
                ExprKind::Assign {
                    target: bx(e),
                    value: bx(value),
                },
                line,
            ));
        }
        Ok(e)
    }

    fn assignment(&mut self, target: Expr, op: Option<BinaryOp>) -> PResult<Expr> {
        let tline = target.line;
        match op {
            None => {
                if matches!(
                    self.peek(),
                    T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg
                ) {
                    self.bump();
                    let value = self.variable()?;
                    let line = min_line(tline, value.line);
                    return Ok(Expr::new(
                        ExprKind::AssignRef {
                            target: bx(target),
                            value: bx(value),
                        },
                        line,
                    ));
                }
                let value = self.expr_bp(prec::ASSIGN)?;
                let line = min_line(tline, value.line);
                Ok(Expr::new(
                    ExprKind::Assign {
                        target: bx(target),
                        value: bx(value),
                    },
                    line,
                ))
            }
            Some(BinaryOp::Coalesce) => {
                let value = self.expr_bp(prec::ASSIGN)?;
                let line = min_line(tline, value.line);
                Ok(Expr::new(
                    ExprKind::AssignCoalesce {
                        target: bx(target),
                        value: bx(value),
                    },
                    line,
                ))
            }
            Some(op) => {
                let value = self.expr_bp(prec::ASSIGN)?;
                let line = min_line(tline, value.line);
                Ok(Expr::new(
                    ExprKind::AssignOp {
                        op,
                        target: bx(target),
                        value: bx(value),
                    },
                    line,
                ))
            }
        }
    }

    /// `variable`
    pub(crate) fn variable(&mut self) -> PResult<Expr> {
        let (e, shape) = self.postfix_expr()?;
        if shape.is_variable() {
            Ok(e)
        } else {
            Err(self.syntax_error())
        }
    }

    #[inline(never)]
    fn clone_expr(&mut self) -> PResult<Expr> {
        let clone_line = self.line();
        self.bump();
        let callee = |line| {
            Callee::Name(Name::new(
                NameKind::FullyQualified,
                ByteStr::from("clone"),
                line,
            ))
        };
        if self.at(T::LParen) {
            // `clone_argument_list` vs `clone (expr)`.
            let after = self.peek_at(1);
            let named = Self::is_identifier(after) && self.peek_at(2) == T::Colon;
            if after == T::RParen || after == T::Ellipsis || after == T::Question || named {
                let args = self.argument_list()?;
                let line = args.line;
                return Ok(Expr::new(
                    ExprKind::Call {
                        callee: callee(line),
                        args,
                    },
                    line,
                ));
            }
            let open_line = self.line();
            self.bump();
            let first = self.expr()?;
            if self.at(T::Comma) {
                let mut args = vec![Arg::Positional(first)];
                self.bump();
                if !self.at(T::RParen) {
                    loop {
                        args.push(self.argument()?);
                        if self.eat(T::Comma) {
                            if self.at(T::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(T::RParen)?;
                let line = min_line(open_line, arg_line(&args[0]));
                let args = ArgList { args, line };
                return Ok(Expr::new(
                    ExprKind::Call {
                        callee: callee(line),
                        args,
                    },
                    line,
                ));
            }
            self.expect(T::RParen)?;
            let (e, shape) = self.postfix_from(first, Shape::Paren)?;
            let operand = self.finish_operand(e, shape)?;
            let operand = self.binary_loop(operand, prec::CLONE)?;
            return Ok(self.clone_call(operand, clone_line));
        }
        let operand = self.expr_bp(prec::CLONE)?;
        Ok(self.clone_call(operand, clone_line))
    }

    fn clone_call(&self, operand: Expr, _clone_line: Line) -> Expr {
        let line = operand.line;
        let name = Name::new(NameKind::FullyQualified, ByteStr::from("clone"), line);
        Expr::new(
            ExprKind::Call {
                callee: Callee::Name(name),
                args: ArgList {
                    args: vec![Arg::Positional(operand)],
                    line,
                },
            },
            line,
        )
    }

    // ---------------------------------------------------------------------
    // Primary expressions and dereferencing
    // ---------------------------------------------------------------------

    /// A primary expression followed by any dereferencing operators.
    pub(crate) fn postfix_expr(&mut self) -> PResult<(Expr, Shape)> {
        let (e, shape) = self.primary()?;
        self.postfix_from(e, shape)
    }

    pub(crate) fn postfix_from(&mut self, mut e: Expr, mut shape: Shape) -> PResult<(Expr, Shape)> {
        if shape == Shape::Paren {
            // Which `'(' expr ')'` rule applies decides the parenthesization
            // flags php-src records: `fully_dereferenceable` marks static
            // properties, plain `expr` marks ternaries and arrow functions,
            // `callable_expr` marks nothing.
            match self.peek() {
                T::LParen => {}
                T::LBracket
                | T::ObjectOperator
                | T::NullsafeObjectOperator
                | T::PaamayimNekudotayim => {
                    if let ExprKind::StaticProp { parenthesized, .. } = &mut e.kind {
                        *parenthesized = true;
                    }
                }
                _ => {
                    return Ok((mark_parenthesized(e), Shape::Callable));
                }
            }
            shape = Shape::Callable;
        }
        loop {
            (e, shape) = match self.peek() {
                T::LBracket if shape.array_object_deref() => (self.dim_rest(e)?, Shape::Var),
                T::ObjectOperator | T::NullsafeObjectOperator if shape.array_object_deref() => {
                    self.arrow_rest(e)?
                }
                T::PaamayimNekudotayim if shape.fully_deref() => {
                    self.static_member(ClassRef::Expr(bx(e)))?
                }
                T::LParen if shape.callable() => (self.call_rest(e)?, Shape::Var),
                _ => break,
            };
        }
        Ok((e, shape))
    }

    /// `base '[' optional_expr ']'`
    #[inline(never)]
    fn dim_rest(&mut self, e: Expr) -> PResult<Expr> {
        self.bump();
        let index = if self.at(T::RBracket) {
            None
        } else {
            Some(bx(self.expr()?))
        };
        self.expect(T::RBracket)?;
        let line = index.as_ref().map_or(e.line, |i| min_line(e.line, i.line));
        Ok(Expr::new(ExprKind::Dim { base: bx(e), index }, line))
    }

    /// `base -> property_name [argument_list]` (also `?->`).
    #[inline(never)]
    fn arrow_rest(&mut self, e: Expr) -> PResult<(Expr, Shape)> {
        let nullsafe = self.at(T::NullsafeObjectOperator);
        self.bump();
        let name = self.property_name()?;
        let nline = member_name_line(&name, self.cur_line());
        if self.at(T::LParen) {
            let args = self.argument_list()?;
            let line = min_line(e.line, min_line(nline, args.line));
            return Ok((
                Expr::new(
                    ExprKind::MethodCall {
                        object: bx(e),
                        name,
                        args,
                        nullsafe,
                    },
                    line,
                ),
                Shape::Var,
            ));
        }
        let line = min_line(e.line, nline);
        Ok((
            Expr::new(
                ExprKind::Prop {
                    object: bx(e),
                    name,
                    nullsafe,
                },
                line,
            ),
            Shape::Prop,
        ))
    }

    /// `callable_expr argument_list`
    #[inline(never)]
    fn call_rest(&mut self, e: Expr) -> PResult<Expr> {
        let call_line = self.la_line();
        let args = self.argument_list()?;
        Ok(Expr::new(
            ExprKind::Call {
                callee: Callee::Expr(bx(e)),
                args,
            },
            call_line,
        ))
    }

    /// After a class reference, at `::`.
    #[inline(never)]
    fn static_member(&mut self, class: ClassRef) -> PResult<(Expr, Shape)> {
        let cline = class_ref_line(&class);
        self.expect(T::PaamayimNekudotayim)?;
        match self.peek() {
            T::Variable | T::Dollar => {
                let vline = self.line();
                let name = self.simple_variable()?;
                if self.at(T::LParen) {
                    let args = self.argument_list()?;
                    let mname = MemberName::Expr(bx(Expr::new(ExprKind::Var(name), vline)));
                    let line = min_line(cline, min_line(vline, args.line));
                    return Ok((
                        Expr::new(
                            ExprKind::StaticCall {
                                class,
                                name: mname,
                                args,
                            },
                            line,
                        ),
                        Shape::Var,
                    ));
                }
                let line = min_line(cline, vline);
                Ok((
                    Expr::new(
                        ExprKind::StaticProp {
                            class,
                            name,
                            parenthesized: false,
                        },
                        line,
                    ),
                    Shape::Prop,
                ))
            }
            T::LBrace => {
                self.bump();
                let inner = self.expr()?;
                self.expect(T::RBrace)?;
                let iline = inner.line;
                if self.at(T::LParen) {
                    let args = self.argument_list()?;
                    let line = min_line(cline, min_line(iline, args.line));
                    let name = MemberName::Expr(bx(inner));
                    return Ok((
                        Expr::new(ExprKind::StaticCall { class, name, args }, line),
                        Shape::Var,
                    ));
                }
                let line = min_line(cline, iline);
                Ok((
                    Expr::new(
                        ExprKind::ClassConst {
                            class,
                            name: ClassConstName::Expr(bx(inner)),
                        },
                        line,
                    ),
                    Shape::ClassConstant,
                ))
            }
            k if Self::is_identifier(k) => {
                let iline = self.line();
                let ident = self.identifier()?;
                if self.at(T::LParen) {
                    let args = self.argument_list()?;
                    let line = min_line(cline, min_line(iline, args.line));
                    return Ok((
                        Expr::new(
                            ExprKind::StaticCall {
                                class,
                                name: MemberName::Ident(ident),
                                args,
                            },
                            line,
                        ),
                        Shape::Var,
                    ));
                }
                let line = min_line(cline, iline);
                if ident.eq_ignore_case(b"class") {
                    return Ok((
                        Expr::new(ExprKind::ClassName(class), line),
                        Shape::ClassConstant,
                    ));
                }
                Ok((
                    Expr::new(
                        ExprKind::ClassConst {
                            class,
                            name: ClassConstName::Ident(ident),
                        },
                        line,
                    ),
                    Shape::ClassConstant,
                ))
            }
            _ => Err(self.syntax_error()),
        }
    }

    /// `property_name: T_STRING | '{' expr '}' | simple_variable`
    fn property_name(&mut self) -> PResult<MemberName> {
        match self.peek() {
            T::String => {
                let i = self.bump();
                Ok(MemberName::Ident(self.name_text(i)))
            }
            T::LBrace => {
                self.bump();
                let e = self.expr()?;
                self.expect(T::RBrace)?;
                Ok(match e.kind {
                    ExprKind::String(s) => MemberName::Ident(s),
                    kind => MemberName::Expr(bx(Expr { kind, line: e.line })),
                })
            }
            T::Variable | T::Dollar => {
                let line = self.line();
                let v = self.simple_variable()?;
                Ok(MemberName::Expr(bx(Expr::new(ExprKind::Var(v), line))))
            }
            _ => Err(self.syntax_error()),
        }
    }

    /// `simple_variable: T_VARIABLE | '$' '{' expr '}' | '$' simple_variable`
    pub(crate) fn simple_variable(&mut self) -> PResult<VarName> {
        self.enter()?;
        let r = self.simple_variable_inner();
        self.leave();
        r
    }

    fn simple_variable_inner(&mut self) -> PResult<VarName> {
        match self.peek() {
            T::Variable => {
                let i = self.bump();
                Ok(VarName::Name(self.name_text(i)))
            }
            T::Dollar => {
                self.bump();
                if self.eat(T::LBrace) {
                    let e = self.expr()?;
                    self.expect(T::RBrace)?;
                    return Ok(match e.kind {
                        ExprKind::String(s) => VarName::Name(s),
                        kind => VarName::Expr(bx(Expr { kind, line: e.line })),
                    });
                }
                let line = self.line();
                let inner = self.simple_variable()?;
                Ok(VarName::Expr(bx(Expr::new(ExprKind::Var(inner), line))))
            }
            _ => Err(self.syntax_error()),
        }
    }

    fn name_token(&mut self) -> Name {
        let i = self.bump();
        let kind = match self.toks[i].kind {
            T::NameQualified => NameKind::Qualified,
            T::NameFullyQualified => NameKind::FullyQualified,
            T::NameRelative => NameKind::Relative,
            _ => NameKind::Unqualified,
        };
        Name::new(kind, self.name_text(i), self.tok_line(i))
    }

    /// `class_name: T_STATIC | name`
    pub(crate) fn class_name(&mut self) -> PResult<Name> {
        match self.peek() {
            T::Static => {
                let i = self.bump();
                Ok(Name::new(
                    NameKind::Unqualified,
                    ByteStr::from("static"),
                    self.tok_line(i),
                ))
            }
            T::String | T::NameQualified | T::NameFullyQualified | T::NameRelative => {
                Ok(self.name_token())
            }
            _ => Err(self.syntax_error()),
        }
    }

    /// `class_name_reference: class_name | new_variable | '(' expr ')'`
    pub(crate) fn class_name_reference(&mut self) -> PResult<ClassRef> {
        match self.peek() {
            T::Static | T::String | T::NameQualified | T::NameFullyQualified | T::NameRelative => {
                let name = self.class_name()?;
                if self.at(T::PaamayimNekudotayim) {
                    let e = self.new_variable_static(ClassRef::Name(name))?;
                    return self.new_variable_rest(e).map(|e| ClassRef::Expr(bx(e)));
                }
                Ok(ClassRef::Name(name))
            }
            T::Variable | T::Dollar => {
                let line = self.line();
                let v = self.simple_variable()?;
                let e = Expr::new(ExprKind::Var(v), line);
                self.new_variable_rest(e).map(|e| ClassRef::Expr(bx(e)))
            }
            T::LParen => {
                self.bump();
                let e = self.expr()?;
                self.expect(T::RParen)?;
                Ok(ClassRef::Expr(bx(e)))
            }
            _ => Err(self.syntax_error()),
        }
    }

    fn new_variable_static(&mut self, class: ClassRef) -> PResult<Expr> {
        let cline = class_ref_line(&class);
        self.expect(T::PaamayimNekudotayim)?;
        let vline = self.line();
        let name = self.simple_variable()?;
        Ok(Expr::new(
            ExprKind::StaticProp {
                class,
                name,
                parenthesized: false,
            },
            min_line(cline, vline),
        ))
    }

    /// The dereferencing part of `new_variable`.
    fn new_variable_rest(&mut self, mut e: Expr) -> PResult<Expr> {
        loop {
            match self.peek() {
                T::LBracket => {
                    self.bump();
                    let index = if self.at(T::RBracket) {
                        None
                    } else {
                        Some(bx(self.expr()?))
                    };
                    self.expect(T::RBracket)?;
                    let line = index.as_ref().map_or(e.line, |i| min_line(e.line, i.line));
                    e = Expr::new(ExprKind::Dim { base: bx(e), index }, line);
                }
                T::ObjectOperator | T::NullsafeObjectOperator => {
                    let nullsafe = self.at(T::NullsafeObjectOperator);
                    self.bump();
                    let name = self.property_name()?;
                    let line = min_line(e.line, member_name_line(&name, self.cur_line()));
                    e = Expr::new(
                        ExprKind::Prop {
                            object: bx(e),
                            name,
                            nullsafe,
                        },
                        line,
                    );
                }
                T::PaamayimNekudotayim => {
                    e = self.new_variable_static(ClassRef::Expr(bx(e)))?;
                }
                _ => return Ok(e),
            }
        }
    }

    /// Primary expressions. The common recursive cases get their own small
    /// frames so deeply nested input does not need a huge stack.
    fn primary(&mut self) -> PResult<(Expr, Shape)> {
        match self.peek() {
            T::Variable | T::Dollar => self.var_primary(),
            T::String | T::NameQualified | T::NameFullyQualified | T::NameRelative => {
                self.name_primary()
            }
            T::LParen => self.paren_primary(),
            T::LBracket => self.short_array_primary(),
            _ => self.other_primary(),
        }
    }

    #[inline(never)]
    fn var_primary(&mut self) -> PResult<(Expr, Shape)> {
        let line = self.line();
        let v = self.simple_variable()?;
        Ok((Expr::new(ExprKind::Var(v), line), Shape::Var))
    }

    #[inline(never)]
    fn name_primary(&mut self) -> PResult<(Expr, Shape)> {
        let line = self.line();
        let name = self.name_token();
        match self.peek() {
            T::LParen => {
                let args = self.argument_list()?;
                let line = min_line(name.line, args.line);
                Ok((
                    Expr::new(
                        ExprKind::Call {
                            callee: Callee::Name(name),
                            args,
                        },
                        line,
                    ),
                    Shape::Var,
                ))
            }
            T::PaamayimNekudotayim => self.static_member(ClassRef::Name(name)),
            _ => Ok((Expr::new(ExprKind::Const(name), line), Shape::Constant)),
        }
    }

    #[inline(never)]
    fn paren_primary(&mut self) -> PResult<(Expr, Shape)> {
        self.bump();
        let e = self.expr()?;
        self.expect(T::RParen)?;
        Ok((e, Shape::Paren))
    }

    #[inline(never)]
    fn short_array_primary(&mut self) -> PResult<(Expr, Shape)> {
        self.bump();
        let items = self.array_pair_list(T::RBracket)?;
        let line = array_line(&items, self.cur_line());
        Ok((
            Expr::new(
                ExprKind::Array {
                    items,
                    syntax: ArraySyntax::Short,
                },
                line,
            ),
            Shape::ShortArray,
        ))
    }

    #[inline(never)]
    fn other_primary(&mut self) -> PResult<(Expr, Shape)> {
        let line = self.line();
        let k = self.peek();
        match k {
            T::Static => {
                let name = self.class_name()?;
                if !self.at(T::PaamayimNekudotayim) {
                    return Err(self.syntax_error());
                }
                self.static_member(ClassRef::Name(name))
            }
            T::Readonly if self.peek_at(1) == T::LParen => {
                let i = self.pos;
                let s = self.lex_tstring(i)?;
                self.bump();
                let name = Name::new(NameKind::Unqualified, s, line);
                let args = self.argument_list()?;
                let line = min_line(line, args.line);
                Ok((
                    Expr::new(
                        ExprKind::Call {
                            callee: Callee::Name(name),
                            args,
                        },
                        line,
                    ),
                    Shape::Var,
                ))
            }
            T::Line
            | T::File
            | T::Dir
            | T::ClassC
            | T::TraitC
            | T::MethodC
            | T::FuncC
            | T::PropertyC
            | T::NsC => {
                self.bump();
                let m = match k {
                    T::Line => MagicConst::Line,
                    T::File => MagicConst::File,
                    T::Dir => MagicConst::Dir,
                    T::ClassC => MagicConst::Class,
                    T::TraitC => MagicConst::Trait,
                    T::MethodC => MagicConst::Method,
                    T::FuncC => MagicConst::Function,
                    T::PropertyC => MagicConst::Property,
                    _ => MagicConst::Namespace,
                };
                Ok((
                    Expr::new(ExprKind::MagicConst(m), self.cur_line()),
                    Shape::MagicConst,
                ))
            }
            T::LNumber | T::DNumber => {
                let i = self.bump();
                let kind = match self.tok_value(i) {
                    TokVal::Int(v) => ExprKind::Int(*v),
                    TokVal::Float(f) => ExprKind::Float(*f),
                    _ => ExprKind::Int(0),
                };
                Ok((Expr::new(kind, line), Shape::Plain))
            }
            T::ConstantEncapsedString => {
                let i = self.bump();
                Ok((
                    Expr::new(ExprKind::String(self.str_value(i)), line),
                    Shape::Callable,
                ))
            }
            T::DoubleQuote => {
                self.bump();
                let parts = self.encaps_list(T::DoubleQuote)?;
                self.expect(T::DoubleQuote)?;
                let line = parts_line(&parts).unwrap_or(line);
                Ok((
                    Expr::new(ExprKind::Interpolated(parts), line),
                    Shape::Callable,
                ))
            }
            T::StartHeredoc => {
                self.bump();
                let mut parts = self.encaps_list(T::EndHeredoc)?;
                self.expect(T::EndHeredoc)?;
                let e = if parts.is_empty() {
                    Expr::new(ExprKind::String(ByteStr::default()), self.cur_line())
                } else if parts.len() == 1 && matches!(parts[0], StrPart::Literal(_)) {
                    let Some(StrPart::Literal(s)) = parts.pop() else {
                        unreachable!()
                    };
                    Expr::new(ExprKind::String(s), line)
                } else {
                    let line = parts_line(&parts).unwrap_or(line);
                    Expr::new(ExprKind::Interpolated(parts), line)
                };
                Ok((e, Shape::Plain))
            }
            T::Backtick => {
                self.bump();
                let parts = self.encaps_list(T::Backtick)?;
                self.expect(T::Backtick)?;
                let line = parts_line(&parts).unwrap_or(line);
                Ok((Expr::new(ExprKind::ShellExec(parts), line), Shape::Plain))
            }
            T::LBracket => {
                self.bump();
                let items = self.array_pair_list(T::RBracket)?;
                let line = array_line(&items, self.cur_line());
                Ok((
                    Expr::new(
                        ExprKind::Array {
                            items,
                            syntax: ArraySyntax::Short,
                        },
                        line,
                    ),
                    Shape::ShortArray,
                ))
            }
            T::Array if self.peek_at(1) == T::LParen => {
                self.bump();
                self.bump();
                let items = self.array_pair_list(T::RParen)?;
                let line = array_line(&items, self.cur_line());
                Ok((
                    Expr::new(
                        ExprKind::Array {
                            items,
                            syntax: ArraySyntax::Long,
                        },
                        line,
                    ),
                    Shape::Callable,
                ))
            }
            T::LParen => {
                self.bump();
                let e = self.expr()?;
                self.expect(T::RParen)?;
                Ok((e, Shape::Paren))
            }
            T::New => self.new_expr(),
            T::Exit => {
                self.bump();
                let args = if self.at(T::LParen) {
                    self.argument_list()?
                } else {
                    ArgList {
                        args: Vec::new(),
                        line: self.cur_line(),
                    }
                };
                let name = Name::new(
                    NameKind::FullyQualified,
                    ByteStr::from("exit"),
                    self.cur_line(),
                );
                let line = min_line(name.line, args.line);
                Ok((
                    Expr::new(
                        ExprKind::Call {
                            callee: Callee::Name(name),
                            args,
                        },
                        line,
                    ),
                    Shape::Plain,
                ))
            }
            T::Isset => {
                self.bump();
                self.expect(T::LParen)?;
                let mut vars = Vec::new();
                loop {
                    vars.push(self.expr()?);
                    if self.eat(T::Comma) {
                        if self.at(T::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
                self.expect(T::RParen)?;
                let line = vars[0].line;
                Ok((Expr::new(ExprKind::Isset(vars), line), Shape::Plain))
            }
            T::Empty => {
                self.bump();
                self.expect(T::LParen)?;
                let e = self.expr()?;
                self.expect(T::RParen)?;
                let line = e.line;
                Ok((Expr::new(ExprKind::Empty(bx(e)), line), Shape::Plain))
            }
            T::Eval => {
                self.bump();
                self.expect(T::LParen)?;
                let e = self.expr()?;
                self.expect(T::RParen)?;
                let line = e.line;
                Ok((Expr::new(ExprKind::Eval(bx(e)), line), Shape::Plain))
            }
            T::Match => self.match_expr().map(|e| (e, Shape::Plain)),
            _ => Err(self.syntax_error()),
        }
    }

    #[inline(never)]
    fn match_expr(&mut self) -> PResult<Expr> {
        self.bump();
        self.expect(T::LParen)?;
        let subject = self.expr()?;
        self.expect(T::RParen)?;
        self.expect(T::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(T::RBrace) {
            let aline = self.line();
            let conds = if self.at(T::Default) {
                self.bump();
                self.eat(T::Comma);
                None
            } else {
                let mut conds = vec![self.expr()?];
                while self.eat(T::Comma) {
                    if self.at(T::DoubleArrow) {
                        break;
                    }
                    conds.push(self.expr()?);
                }
                Some(conds)
            };
            self.expect(T::DoubleArrow)?;
            let body = self.expr()?;
            let line = conds.as_ref().map_or(min_line(aline, body.line), |c| {
                min_line(c[0].line, body.line)
            });
            arms.push(MatchArm { conds, body, line });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::RBrace)?;
        let line = subject.line;
        Ok(Expr::new(
            ExprKind::Match {
                subject: bx(subject),
                arms,
            },
            line,
        ))
    }

    #[inline(never)]
    fn new_expr(&mut self) -> PResult<(Expr, Shape)> {
        self.bump();
        let mut attrs = Vec::new();
        if self.at(T::Attribute) {
            attrs = self.attributes()?;
            if !matches!(self.peek(), T::Class | T::Readonly | T::Abstract | T::Final) {
                return Err(self.syntax_error());
            }
        }
        if matches!(self.peek(), T::Class | T::Readonly | T::Abstract | T::Final) {
            let decl = self.anonymous_class(attrs)?;
            let (decl, args) = decl;
            let line = min_line(decl.start_line, args.line);
            let e = Expr::new(
                ExprKind::New {
                    class: NewClass::Anonymous(Box::new(decl)),
                    args,
                },
                line,
            );
            return Ok((e, Shape::Callable));
        }
        let class = match self.class_name_reference()? {
            ClassRef::Name(n) => NewClass::Name(n),
            ClassRef::Expr(e) => NewClass::Expr(e),
        };
        let cline = match &class {
            NewClass::Name(n) => n.line,
            NewClass::Expr(e) => e.line,
            NewClass::Anonymous(d) => d.start_line,
        };
        if self.at(T::LParen) {
            let args = self.argument_list()?;
            let line = min_line(cline, args.line);
            return Ok((
                Expr::new(ExprKind::New { class, args }, line),
                Shape::Callable,
            ));
        }
        let args = ArgList {
            args: Vec::new(),
            line: self.la_line(),
        };
        let line = min_line(cline, args.line);
        Ok((Expr::new(ExprKind::New { class, args }, line), Shape::Plain))
    }

    // ---------------------------------------------------------------------
    // Arguments
    // ---------------------------------------------------------------------

    /// `argument_list`
    pub(crate) fn argument_list(&mut self) -> PResult<ArgList> {
        self.expect(T::LParen)?;
        if self.eat(T::RParen) {
            return Ok(ArgList {
                args: Vec::new(),
                line: self.cur_line(),
            });
        }
        let mut args = Vec::new();
        loop {
            args.push(self.argument()?);
            if self.eat(T::Comma) {
                if self.at(T::RParen) {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(T::RParen)?;
        let line = args
            .first()
            .map_or(self.cur_line(), |a| min_line(arg_line(a), self.cur_line()));
        Ok(ArgList { args, line })
    }

    pub(crate) fn argument(&mut self) -> PResult<Arg> {
        match self.peek() {
            T::Ellipsis => {
                self.bump();
                if can_start_expr(self.peek()) {
                    Ok(Arg::Unpack(self.expr()?))
                } else {
                    Ok(Arg::VariadicPlaceholder)
                }
            }
            T::Question => {
                self.bump();
                Ok(Arg::Placeholder)
            }
            k if Self::is_identifier(k) && self.peek_at(1) == T::Colon => {
                let name = self.identifier()?;
                self.bump();
                if self.eat(T::Question) {
                    return Ok(Arg::NamedPlaceholder(name));
                }
                Ok(Arg::Named {
                    name,
                    value: self.expr()?,
                })
            }
            _ => Ok(Arg::Positional(self.expr()?)),
        }
    }

    // ---------------------------------------------------------------------
    // Arrays
    // ---------------------------------------------------------------------

    /// `array_pair_list` up to (and including) `close`.
    pub(crate) fn array_pair_list(&mut self, close: T) -> PResult<Vec<Option<ArrayItem>>> {
        let mut items = Vec::new();
        loop {
            if self.at(T::Comma) || self.at(close) {
                items.push(None);
            } else {
                items.push(Some(self.array_pair()?));
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        if matches!(items.last(), Some(None)) {
            items.pop();
        }
        self.expect(close)?;
        Ok(items)
    }

    fn array_pair(&mut self) -> PResult<ArrayItem> {
        let line = self.line();
        match self.peek() {
            T::Ellipsis => {
                self.bump();
                let value = self.expr()?;
                let line = value.line;
                return Ok(ArrayItem {
                    key: None,
                    value,
                    by_ref: false,
                    unpack: true,
                    line,
                });
            }
            T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg => {
                self.bump();
                let value = self.variable()?;
                let line = value.line;
                return Ok(ArrayItem {
                    key: None,
                    value,
                    by_ref: true,
                    unpack: false,
                    line,
                });
            }
            T::List => {
                if let Some(value) = self.nested_list()? {
                    return Ok(ArrayItem {
                        key: None,
                        value,
                        by_ref: false,
                        unpack: false,
                        line,
                    });
                }
            }
            _ => {}
        }
        let first = self.expr()?;
        if !self.eat(T::DoubleArrow) {
            let line = first.line;
            return Ok(ArrayItem {
                key: None,
                value: first,
                by_ref: false,
                unpack: false,
                line,
            });
        }
        let line = first.line;
        match self.peek() {
            T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg => {
                self.bump();
                let value = self.variable()?;
                Ok(ArrayItem {
                    key: Some(first),
                    value,
                    by_ref: true,
                    unpack: false,
                    line,
                })
            }
            T::List => match self.nested_list()? {
                Some(value) => Ok(ArrayItem {
                    key: Some(first),
                    value,
                    by_ref: false,
                    unpack: false,
                    line,
                }),
                None => unreachable!("nested_list returns None only before consuming"),
            },
            _ => {
                let value = self.expr()?;
                Ok(ArrayItem {
                    key: Some(first),
                    value,
                    by_ref: false,
                    unpack: false,
                    line,
                })
            }
        }
    }

    /// `T_LIST '(' array_pair_list ')'` inside an array pair. If it is
    /// followed by `=`, it is a destructuring assignment expression instead.
    #[inline(never)]
    fn nested_list(&mut self) -> PResult<Option<Expr>> {
        let line = self.line();
        self.bump();
        self.expect(T::LParen)?;
        let items = self.array_pair_list(T::RParen)?;
        let target = Expr::new(
            ExprKind::Array {
                items,
                syntax: ArraySyntax::List,
            },
            line,
        );
        if self.eat(T::Equals) {
            let value = self.expr_bp(prec::ASSIGN)?;
            let line = min_line(line, value.line);
            let e = Expr::new(
                ExprKind::Assign {
                    target: bx(target),
                    value: bx(value),
                },
                line,
            );
            return Ok(Some(self.continue_expr(e, 0)?));
        }
        Ok(Some(target))
    }

    // ---------------------------------------------------------------------
    // Interpolated strings
    // ---------------------------------------------------------------------

    #[inline(never)]
    pub(crate) fn encaps_list(&mut self, end: T) -> PResult<Vec<StrPart>> {
        let mut parts = Vec::new();
        while !self.at(end) {
            let line = self.line();
            match self.peek() {
                T::EncapsedAndWhitespace => {
                    let i = self.bump();
                    parts.push(StrPart::Literal(self.str_value(i)));
                }
                T::Variable => {
                    let i = self.bump();
                    let var = Expr::new(ExprKind::Var(VarName::Name(self.name_text(i))), line);
                    let expr = match self.peek() {
                        T::LBracket => {
                            self.bump();
                            let index = self.encaps_var_offset()?;
                            self.expect(T::RBracket)?;
                            Expr::new(
                                ExprKind::Dim {
                                    base: bx(var),
                                    index: Some(bx(index)),
                                },
                                line,
                            )
                        }
                        T::ObjectOperator | T::NullsafeObjectOperator => {
                            let nullsafe = self.at(T::NullsafeObjectOperator);
                            self.bump();
                            let p = self.expect(T::String)?;
                            let name = MemberName::Ident(self.name_text(p));
                            Expr::new(
                                ExprKind::Prop {
                                    object: bx(var),
                                    name,
                                    nullsafe,
                                },
                                line,
                            )
                        }
                        _ => var,
                    };
                    parts.push(StrPart::Expr {
                        expr,
                        syntax: InterpolationSyntax::Simple,
                    });
                }
                T::DollarOpenCurlyBraces => {
                    self.bump();
                    if self.at(T::StringVarname) {
                        let i = self.bump();
                        let vline = self.tok_line(i);
                        let var = Expr::new(ExprKind::Var(VarName::Name(self.name_text(i))), vline);
                        let expr = if self.eat(T::LBracket) {
                            let index = self.expr()?;
                            self.expect(T::RBracket)?;
                            let line = min_line(vline, index.line);
                            Expr::new(
                                ExprKind::Dim {
                                    base: bx(var),
                                    index: Some(bx(index)),
                                },
                                line,
                            )
                        } else {
                            var
                        };
                        self.expect(T::RBrace)?;
                        parts.push(StrPart::Expr {
                            expr,
                            syntax: InterpolationSyntax::DollarCurly,
                        });
                    } else {
                        let inner = self.expr()?;
                        self.expect(T::RBrace)?;
                        let iline = inner.line;
                        let name = match inner.kind {
                            ExprKind::String(s) => VarName::Name(s),
                            kind => VarName::Expr(bx(Expr { kind, line: iline })),
                        };
                        let expr = Expr::new(ExprKind::Var(name), iline);
                        parts.push(StrPart::Expr {
                            expr,
                            syntax: InterpolationSyntax::DollarCurlyVarVar,
                        });
                    }
                }
                T::CurlyOpen => {
                    self.bump();
                    let expr = self.variable()?;
                    self.expect(T::RBrace)?;
                    parts.push(StrPart::Expr {
                        expr,
                        syntax: InterpolationSyntax::Simple,
                    });
                }
                _ => return Err(self.syntax_error()),
            }
        }
        Ok(parts)
    }

    fn encaps_var_offset(&mut self) -> PResult<Expr> {
        let line = self.line();
        match self.peek() {
            T::String => {
                let i = self.bump();
                Ok(Expr::new(ExprKind::String(self.name_text(i)), line))
            }
            T::NumString => {
                let i = self.bump();
                Ok(self.num_string(i, false))
            }
            T::Minus => {
                self.bump();
                let i = self.expect(T::NumString)?;
                Ok(self.num_string(i, true))
            }
            T::Variable => {
                let i = self.bump();
                Ok(Expr::new(
                    ExprKind::Var(VarName::Name(self.name_text(i))),
                    line,
                ))
            }
            _ => Err(self.syntax_error()),
        }
    }

    fn num_string(&self, i: usize, negate: bool) -> Expr {
        let line = self.tok_line(i);
        match self.tok_value(i) {
            TokVal::Int(v) => {
                if negate {
                    if *v == 0 {
                        Expr::new(ExprKind::String(ByteStr::from("-0")), line)
                    } else {
                        Expr::new(ExprKind::Int(-*v), line)
                    }
                } else {
                    Expr::new(ExprKind::Int(*v), line)
                }
            }
            _ => {
                let text = self.text(i);
                let mut s = Vec::with_capacity(text.len() + 1);
                if negate {
                    s.push(b'-');
                }
                s.extend_from_slice(text);
                Expr::new(ExprKind::String(ByteStr::from(s)), line)
            }
        }
    }
}

/// `expr: '(' expr ')'` marks conditionals and arrow functions.
fn mark_parenthesized(mut e: Expr) -> Expr {
    match &mut e.kind {
        ExprKind::Ternary { parenthesized, .. } => *parenthesized = true,
        ExprKind::ArrowFn(f) => f.parenthesized = true,
        _ => {}
    }
    e
}

/// String value of a literal operand for compile-time concatenation.
fn literal_str(e: &Expr) -> Option<Vec<u8>> {
    match &e.kind {
        ExprKind::String(s) => Some(s.to_vec()),
        ExprKind::Int(i) => Some(i.to_string().into_bytes()),
        ExprKind::Float(f) => Some(numeric::gcvt(*f, 14).into_bytes()),
        _ => None,
    }
}

pub(crate) fn class_ref_line(c: &ClassRef) -> Line {
    match c {
        ClassRef::Name(n) => n.line,
        ClassRef::Expr(e) => e.line,
    }
}

fn member_name_line(n: &MemberName, fallback: Line) -> Line {
    match n {
        MemberName::Ident(_) => fallback,
        MemberName::Expr(e) => e.line,
    }
}

pub(crate) fn arg_line(a: &Arg) -> Line {
    match a {
        Arg::Positional(e) | Arg::Unpack(e) | Arg::Named { value: e, .. } => e.line,
        _ => Line::MAX,
    }
}

fn parts_line(parts: &[StrPart]) -> Option<Line> {
    parts.iter().find_map(|p| match p {
        StrPart::Expr { expr, .. } => Some(expr.line),
        StrPart::Literal(_) => None,
    })
}

fn array_line(items: &[Option<ArrayItem>], fallback: Line) -> Line {
    items
        .iter()
        .flatten()
        .map(|i| i.line)
        .next()
        .map_or(fallback, |l| l.min(fallback))
}
