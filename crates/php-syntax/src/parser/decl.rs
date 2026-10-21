//! Declarations: attributes, functions, closures, parameters, types and
//! class-like declarations.

use super::expr::prec;
use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind as T;

/// `zend_modifier_target`
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Property,
    Method,
    Constant,
    Cpp,
    PropertyHook,
}

impl Target {
    fn member(self) -> &'static str {
        match self {
            Target::Property => "property",
            Target::Method => "method",
            Target::Constant => "class constant",
            Target::Cpp => "parameter",
            Target::PropertyHook => "property hook",
        }
    }
}

fn modifier_name(k: T) -> &'static str {
    match k {
        T::Public => "public",
        T::Protected => "protected",
        T::Private => "private",
        T::Static => "static",
        T::Final => "final",
        T::Readonly => "readonly",
        T::Abstract => "abstract",
        T::PublicSet => "public(set)",
        T::ProtectedSet => "protected(set)",
        _ => "private(set)",
    }
}

/// `zend_modifier_token_to_flag()`
pub(crate) fn modifier_flag(target: Target, k: T) -> Result<Modifiers, String> {
    use Target::*;
    let flag = match k {
        T::Public if target != PropertyHook => Some(Modifiers::PUBLIC),
        T::Protected if target != PropertyHook => Some(Modifiers::PROTECTED),
        T::Private if target != PropertyHook => Some(Modifiers::PRIVATE),
        T::Readonly if matches!(target, Property | Cpp) => Some(Modifiers::READONLY),
        T::Abstract if matches!(target, Method | Property) => Some(Modifiers::ABSTRACT),
        T::Final => Some(Modifiers::FINAL),
        T::Static if matches!(target, Property | Method) => Some(Modifiers::STATIC),
        T::PublicSet if matches!(target, Property | Cpp) => Some(Modifiers::PUBLIC_SET),
        T::ProtectedSet if matches!(target, Property | Cpp) => Some(Modifiers::PROTECTED_SET),
        T::PrivateSet if matches!(target, Property | Cpp) => Some(Modifiers::PRIVATE_SET),
        _ => None,
    };
    flag.ok_or_else(|| {
        format!(
            "Cannot use the {} modifier on a {}",
            modifier_name(k),
            target.member()
        )
    })
}

/// `zend_add_member_modifier()`
fn add_member_modifier(
    flags: Modifiers,
    new: Modifiers,
    target: Target,
) -> Result<Modifiers, String> {
    let new_flags = flags | new;
    if flags.intersects(Modifiers::VISIBILITY) && new.intersects(Modifiers::VISIBILITY) {
        return Err("Multiple access type modifiers are not allowed".into());
    }
    if new_flags.contains(Modifiers::ABSTRACT) && new_flags.contains(Modifiers::FINAL) {
        if target == Target::Method {
            return Err("Cannot use the final modifier on an abstract method".into());
        }
        if target == Target::Property {
            return Err("Cannot use the final modifier on an abstract property".into());
        }
    }
    if matches!(target, Target::Property | Target::Cpp)
        && flags.intersects(Modifiers::SET_VISIBILITY)
        && new.intersects(Modifiers::SET_VISIBILITY)
    {
        return Err("Multiple access type modifiers are not allowed".into());
    }
    Ok(new_flags)
}

/// `zend_modifier_list_to_flags()`
pub(crate) fn modifier_list(target: Target, toks: &[T]) -> Result<Modifiers, String> {
    let mut flags = Modifiers::empty();
    for &k in toks {
        let new = modifier_flag(target, k)?;
        let duplicate = flags.intersects(new);
        flags = add_member_modifier(flags, new, target)?;
        if duplicate {
            return Err(format!(
                "Multiple {} modifiers are not allowed",
                modifier_name(k)
            ));
        }
    }
    Ok(flags)
}

fn starts_type(k: T) -> bool {
    matches!(
        k,
        T::Question
            | T::LParen
            | T::Array
            | T::Callable
            | T::String
            | T::NameQualified
            | T::NameFullyQualified
            | T::NameRelative
    )
}

fn is_amp(k: T) -> bool {
    matches!(
        k,
        T::AmpersandFollowedByVarOrVararg | T::AmpersandNotFollowedByVarOrVararg
    )
}

impl<'a> Parser<'a> {
    // ---------------------------------------------------------------------
    // Attributes
    // ---------------------------------------------------------------------

    /// `attributes`: one or more `#[...]` groups.
    #[inline(never)]
    pub(crate) fn attributes(&mut self) -> PResult<Vec<AttributeGroup>> {
        let mut groups = Vec::new();
        while self.at(T::Attribute) {
            self.bump();
            let saved = self.take_doc_id();
            let mut group = Vec::new();
            loop {
                let name = self.class_name()?;
                let args = if self.at(T::LParen) {
                    Some(self.argument_list()?)
                } else {
                    None
                };
                let line = name.line;
                group.push(Attribute { name, args, line });
                if self.eat(T::Comma) {
                    if self.at(T::RBracket) {
                        break;
                    }
                    continue;
                }
                break;
            }
            self.expect(T::RBracket)?;
            self.restore_doc_id(saved);
            groups.push(group);
        }
        Ok(groups)
    }

    // ---------------------------------------------------------------------
    // Functions
    // ---------------------------------------------------------------------

    #[inline(never)]
    pub(crate) fn function_decl(&mut self, attributes: Vec<AttributeGroup>) -> PResult<Stmt> {
        let start_line = self.line();
        self.bump();
        let by_ref = is_amp(self.peek());
        if by_ref {
            self.bump();
        }
        let name = match self.peek() {
            T::String => {
                let i = self.bump();
                self.name_text(i)
            }
            T::Readonly => {
                let i = self.pos;
                let s = self.lex_tstring(i)?;
                self.bump();
                s
            }
            _ => return Err(self.syntax_error()),
        };
        let doc_comment = self.take_doc(false);
        let params = self.parameter_list()?;
        let return_type = self.return_type()?;
        self.push_fn();
        self.expect(T::LBrace)?;
        let body = self.inner_statement_list(&[T::RBrace])?;
        self.expect(T::RBrace)?;
        let is_generator = self.pop_fn();
        let end_line = self.cur_line();
        let decl = FunctionDecl {
            fq_name: name.clone(),
            name,
            common: FunctionCommon {
                by_ref,
                params,
                return_type,
                attributes,
                doc_comment,
                start_line,
                end_line,
                is_generator,
            },
            body,
        };
        Ok(Stmt {
            kind: StmtKind::Function(Box::new(decl)),
            line: start_line,
        })
    }

    /// Closures and arrow functions (`inline_function`).
    #[inline(never)]
    pub(crate) fn inline_function(
        &mut self,
        is_static: bool,
        attributes: Vec<AttributeGroup>,
    ) -> PResult<Expr> {
        let is_arrow = self.at(T::Fn);
        let start_line = self.line();
        self.bump();
        let by_ref = is_amp(self.peek());
        if by_ref {
            self.bump();
        }
        let doc_comment = self.take_doc(!by_ref);
        let params = self.parameter_list()?;
        if is_arrow {
            let return_type = self.return_type()?;
            self.expect(T::DoubleArrow)?;
            self.push_fn();
            let body = self.expr_bp(prec::ARROW_FUNCTION)?;
            let is_generator = self.pop_fn();
            let end_line = self.la_line();
            let common = FunctionCommon {
                by_ref,
                params,
                return_type,
                attributes,
                doc_comment,
                start_line,
                end_line,
                is_generator,
            };
            let decl = ArrowFnDecl {
                is_static,
                common,
                body: Box::new(body),
                parenthesized: false,
            };
            return Ok(Expr::new(ExprKind::ArrowFn(Box::new(decl)), start_line));
        }
        let mut uses = Vec::new();
        if self.eat(T::Use) {
            self.expect(T::LParen)?;
            loop {
                let by_ref = is_amp(self.peek());
                if by_ref {
                    self.bump();
                }
                let i = self.expect(T::Variable)?;
                uses.push(ClosureUse {
                    name: self.name_text(i),
                    by_ref,
                    line: self.tok_line(i),
                });
                if self.eat(T::Comma) {
                    if self.at(T::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
            self.expect(T::RParen)?;
        }
        let return_type = self.return_type()?;
        self.push_fn();
        self.expect(T::LBrace)?;
        let body = self.inner_statement_list(&[T::RBrace])?;
        self.expect(T::RBrace)?;
        let is_generator = self.pop_fn();
        let end_line = self.cur_line();
        let common = FunctionCommon {
            by_ref,
            params,
            return_type,
            attributes,
            doc_comment,
            start_line,
            end_line,
            is_generator,
        };
        let decl = ClosureDecl {
            is_static,
            common,
            uses,
            body,
        };
        Ok(Expr::new(ExprKind::Closure(Box::new(decl)), start_line))
    }

    /// `'(' parameter_list ')'`
    pub(crate) fn parameter_list(&mut self) -> PResult<Vec<Param>> {
        self.expect(T::LParen)?;
        let mut params = Vec::new();
        if self.eat(T::RParen) {
            return Ok(params);
        }
        loop {
            params.push(self.parameter()?);
            if self.eat(T::Comma) {
                if self.at(T::RParen) {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(T::RParen)?;
        Ok(params)
    }

    fn collect_modifiers(&mut self) -> Vec<T> {
        let mut mods = Vec::new();
        while self.peek().is_member_modifier() {
            mods.push(self.peek());
            self.bump();
        }
        mods
    }

    fn validate_modifiers(&self, target: Target, mods: &[T]) -> PResult<Modifiers> {
        modifier_list(target, mods).map_err(|m| self.compile_error(m))
    }

    fn parameter(&mut self) -> PResult<Param> {
        let attributes = if self.at(T::Attribute) {
            self.attributes()?
        } else {
            Vec::new()
        };
        let line = self.line();
        let mods = self.collect_modifiers();
        let modifiers = if mods.is_empty() {
            Modifiers::empty()
        } else {
            self.validate_modifiers(Target::Cpp, &mods)?
        };
        let ty = if starts_type(self.peek()) {
            Some(self.type_expr(false)?)
        } else {
            None
        };
        let by_ref = self.eat(T::AmpersandFollowedByVarOrVararg);
        let variadic = self.eat(T::Ellipsis);
        let i = self.expect(T::Variable)?;
        let default = if self.eat(T::Equals) {
            Some(self.expr()?)
        } else {
            None
        };
        let doc_comment = self.take_doc(true);
        let hooks = if self.at(T::LBrace) {
            Some(self.property_hook_list()?)
        } else {
            None
        };
        Ok(Param {
            name: self.name_text(i),
            ty,
            default,
            by_ref,
            variadic,
            modifiers,
            attributes,
            doc_comment,
            hooks,
            line,
        })
    }

    pub(crate) fn return_type(&mut self) -> PResult<Option<TypeHint>> {
        if self.eat(T::Colon) {
            Ok(Some(self.type_expr(true)?))
        } else {
            Ok(None)
        }
    }

    // ---------------------------------------------------------------------
    // Types
    // ---------------------------------------------------------------------

    /// `type_expr` / `type_expr_without_static`
    pub(crate) fn type_expr(&mut self, allow_static: bool) -> PResult<TypeHint> {
        if self.eat(T::Question) {
            let t = self.single_type(allow_static)?;
            return Ok(TypeHint::Nullable(Box::new(t)));
        }
        let first = if self.at(T::LParen) {
            let t = self.paren_intersection(allow_static)?;
            if !self.at(T::Bar) {
                return Err(self.syntax_error());
            }
            t
        } else {
            self.single_type(allow_static)?
        };
        if self.at(T::Bar) {
            let mut types = vec![first];
            while self.eat(T::Bar) {
                let t = if self.at(T::LParen) {
                    self.paren_intersection(allow_static)?
                } else {
                    self.single_type(allow_static)?
                };
                types.push(t);
            }
            return Ok(TypeHint::Union(types));
        }
        if self.at(T::AmpersandNotFollowedByVarOrVararg) {
            let mut types = vec![first];
            while self.eat(T::AmpersandNotFollowedByVarOrVararg) {
                types.push(self.single_type(allow_static)?);
            }
            return Ok(TypeHint::Intersection(types));
        }
        Ok(first)
    }

    fn paren_intersection(&mut self, allow_static: bool) -> PResult<TypeHint> {
        self.expect(T::LParen)?;
        let mut types = vec![self.single_type(allow_static)?];
        self.expect(T::AmpersandNotFollowedByVarOrVararg)?;
        types.push(self.single_type(allow_static)?);
        while self.eat(T::AmpersandNotFollowedByVarOrVararg) {
            types.push(self.single_type(allow_static)?);
        }
        self.expect(T::RParen)?;
        Ok(TypeHint::Intersection(types))
    }

    fn single_type(&mut self, allow_static: bool) -> PResult<TypeHint> {
        match self.peek() {
            T::Array => {
                self.bump();
                Ok(TypeHint::Array)
            }
            T::Callable => {
                self.bump();
                Ok(TypeHint::Callable)
            }
            T::Static if allow_static => {
                self.bump();
                Ok(TypeHint::Static)
            }
            T::String | T::NameQualified | T::NameFullyQualified | T::NameRelative => {
                let name = self.class_name()?;
                if name.kind == NameKind::Unqualified
                    && let Some(ty) = BuiltinType::from_name(&name.name)
                {
                    return Ok(TypeHint::Builtin {
                        ty,
                        source: name.name,
                    });
                }
                Ok(TypeHint::Class(name))
            }
            _ => Err(self.syntax_error()),
        }
    }

    // ---------------------------------------------------------------------
    // Classes
    // ---------------------------------------------------------------------

    fn class_name_list(&mut self) -> PResult<Vec<Name>> {
        let mut v = vec![self.class_name()?];
        while self.eat(T::Comma) {
            v.push(self.class_name()?);
        }
        Ok(v)
    }

    #[inline(never)]
    pub(crate) fn class_like_decl(&mut self, attributes: Vec<AttributeGroup>) -> PResult<Stmt> {
        let decl = match self.peek() {
            T::Trait => {
                let start_line = self.line();
                self.bump();
                let i = self.expect(T::String)?;
                let doc_comment = self.take_doc(false);
                let (members, end_line) = self.class_body()?;
                ClassDecl {
                    kind: ClassKind::Trait,
                    name: Some(self.name_text(i)),
                    fq_name: self.name_text(i),
                    modifiers: ClassModifiers::default(),
                    extends: None,
                    implements: Vec::new(),
                    enum_backing_type: None,
                    members,
                    attributes,
                    doc_comment,
                    start_line,
                    end_line,
                }
            }
            T::Interface => {
                let start_line = self.line();
                self.bump();
                let i = self.expect(T::String)?;
                let implements = if self.eat(T::Extends) {
                    self.class_name_list()?
                } else {
                    Vec::new()
                };
                let doc_comment = self.take_doc(true);
                let (members, end_line) = self.class_body()?;
                ClassDecl {
                    kind: ClassKind::Interface,
                    name: Some(self.name_text(i)),
                    fq_name: self.name_text(i),
                    modifiers: ClassModifiers::default(),
                    extends: None,
                    implements,
                    enum_backing_type: None,
                    members,
                    attributes,
                    doc_comment,
                    start_line,
                    end_line,
                }
            }
            T::Enum => {
                let start_line = self.line();
                self.bump();
                let i = self.expect(T::String)?;
                let enum_backing_type = if self.eat(T::Colon) {
                    Some(self.type_expr(true)?)
                } else {
                    None
                };
                let implements = if self.eat(T::Implements) {
                    self.class_name_list()?
                } else {
                    Vec::new()
                };
                let doc_comment = self.take_doc(true);
                let (members, end_line) = self.class_body()?;
                ClassDecl {
                    kind: ClassKind::Enum,
                    name: Some(self.name_text(i)),
                    fq_name: self.name_text(i),
                    modifiers: ClassModifiers {
                        is_final: true,
                        ..Default::default()
                    },
                    extends: None,
                    implements,
                    enum_backing_type,
                    members,
                    attributes,
                    doc_comment,
                    start_line,
                    end_line,
                }
            }
            _ => {
                let modifiers = self.class_modifiers()?;
                let start_line = self.line();
                self.expect(T::Class)?;
                let i = self.expect(T::String)?;
                let extends = if self.eat(T::Extends) {
                    Some(self.class_name()?)
                } else {
                    None
                };
                let implements = if self.eat(T::Implements) {
                    self.class_name_list()?
                } else {
                    Vec::new()
                };
                let doc_comment = self.take_doc(true);
                let (members, end_line) = self.class_body()?;
                ClassDecl {
                    kind: ClassKind::Class,
                    name: Some(self.name_text(i)),
                    fq_name: self.name_text(i),
                    modifiers,
                    extends,
                    implements,
                    enum_backing_type: None,
                    members,
                    attributes,
                    doc_comment,
                    start_line,
                    end_line,
                }
            }
        };
        let line = decl.start_line;
        Ok(Stmt {
            kind: StmtKind::Class(Box::new(decl)),
            line,
        })
    }

    /// `class_modifiers` with `zend_add_class_modifier()` checks.
    fn class_modifiers(&mut self) -> PResult<ClassModifiers> {
        let mut m = ClassModifiers::default();
        let mut first = true;
        while matches!(self.peek(), T::Abstract | T::Final | T::Readonly) {
            let k = self.peek();
            self.bump();
            if !first {
                let err = match k {
                    T::Abstract if m.is_abstract => {
                        Some("Multiple abstract modifiers are not allowed")
                    }
                    T::Final if m.is_final => Some("Multiple final modifiers are not allowed"),
                    T::Readonly if m.is_readonly => {
                        Some("Multiple readonly modifiers are not allowed")
                    }
                    T::Abstract if m.is_final => {
                        Some("Cannot use the final modifier on an abstract class")
                    }
                    T::Final if m.is_abstract => {
                        Some("Cannot use the final modifier on an abstract class")
                    }
                    _ => None,
                };
                if let Some(e) = err {
                    return Err(self.compile_error(e));
                }
            }
            first = false;
            match k {
                T::Abstract => m.is_abstract = true,
                T::Final => m.is_final = true,
                _ => m.is_readonly = true,
            }
        }
        Ok(m)
    }

    /// `T_NEW anonymous_class`; returns the declaration and constructor
    /// arguments.
    #[inline(never)]
    pub(crate) fn anonymous_class(
        &mut self,
        attributes: Vec<AttributeGroup>,
    ) -> PResult<(ClassDecl, ArgList)> {
        let mut modifiers = ClassModifiers::default();
        while matches!(self.peek(), T::Abstract | T::Final | T::Readonly) {
            let k = self.peek();
            self.bump();
            let err = match k {
                T::Abstract => Some("Cannot use the abstract modifier on an anonymous class"),
                T::Final => Some("Cannot use the final modifier on an anonymous class"),
                _ if modifiers.is_readonly => Some("Multiple readonly modifiers are not allowed"),
                _ => None,
            };
            if let Some(e) = err {
                return Err(self.compile_error(e));
            }
            modifiers.is_readonly = true;
        }
        let start_line = self.line();
        self.expect(T::Class)?;
        let args = if self.at(T::LParen) {
            self.argument_list()?
        } else {
            ArgList {
                args: Vec::new(),
                line: self.la_line(),
            }
        };
        let extends = if self.eat(T::Extends) {
            Some(self.class_name()?)
        } else {
            None
        };
        let implements = if self.eat(T::Implements) {
            self.class_name_list()?
        } else {
            Vec::new()
        };
        let doc_comment = self.take_doc(true);
        let (members, end_line) = self.class_body()?;
        let decl = ClassDecl {
            kind: ClassKind::Class,
            name: None,
            fq_name: ByteStr::default(),
            modifiers,
            extends,
            implements,
            enum_backing_type: None,
            members,
            attributes,
            doc_comment,
            start_line,
            end_line,
        };
        Ok((decl, args))
    }

    fn class_body(&mut self) -> PResult<(Vec<ClassMember>, Line)> {
        self.expect(T::LBrace)?;
        let mut members = Vec::new();
        while !self.at(T::RBrace) {
            members.push(self.class_member()?);
        }
        self.bump();
        Ok((members, self.cur_line()))
    }

    #[inline(never)]
    fn class_member(&mut self) -> PResult<ClassMember> {
        let line = self.line();
        let has_attrs = self.at(T::Attribute);
        let attributes = if has_attrs {
            self.attributes()?
        } else {
            Vec::new()
        };
        if !has_attrs && self.at(T::Use) {
            self.bump();
            let traits = self.class_name_list()?;
            let adaptations = self.trait_adaptations()?;
            return Ok(ClassMember {
                kind: ClassMemberKind::TraitUse(TraitUse {
                    traits,
                    adaptations,
                }),
                line,
            });
        }
        if self.eat(T::Var) {
            return self.property_decl(Modifiers::PUBLIC, attributes, line);
        }
        let mods = self.collect_modifiers();
        match self.peek() {
            T::Const => {
                let mut flags = self.validate_modifiers(Target::Constant, &mods)?;
                if !flags.intersects(Modifiers::VISIBILITY) {
                    flags.insert(Modifiers::PUBLIC);
                }
                self.bump();
                let ty = if Self::is_identifier(self.peek()) && self.peek_at(1) == T::Equals {
                    None
                } else {
                    Some(self.type_expr(true)?)
                };
                let mut consts = Vec::new();
                loop {
                    let cline = self.line();
                    let name = self.identifier()?;
                    self.expect(T::Equals)?;
                    let value = self.expr()?;
                    let doc_comment = self.take_doc(true);
                    consts.push(ConstElem {
                        fq_name: name.clone(),
                        name,
                        value,
                        doc_comment,
                        line: cline,
                    });
                    if !self.eat(T::Comma) {
                        break;
                    }
                }
                self.expect(T::Semicolon)?;
                Ok(ClassMember {
                    kind: ClassMemberKind::Const(ClassConstGroup {
                        modifiers: flags,
                        ty,
                        consts,
                        attributes,
                    }),
                    line,
                })
            }
            T::Function => {
                let mut flags = self.validate_modifiers(Target::Method, &mods)?;
                if !flags.intersects(Modifiers::VISIBILITY) {
                    flags.insert(Modifiers::PUBLIC);
                }
                let start_line = self.line();
                self.bump();
                let by_ref = is_amp(self.peek());
                if by_ref {
                    self.bump();
                }
                let name = self.identifier()?;
                let doc_comment = self.take_doc(false);
                let params = self.parameter_list()?;
                let return_type = self.return_type()?;
                self.push_fn();
                let body = if self.eat(T::Semicolon) {
                    None
                } else {
                    self.expect(T::LBrace)?;
                    let b = self.inner_statement_list(&[T::RBrace])?;
                    self.expect(T::RBrace)?;
                    Some(b)
                };
                let is_generator = self.pop_fn();
                let end_line = self.cur_line();
                let m = MethodDecl {
                    name,
                    modifiers: flags,
                    common: FunctionCommon {
                        by_ref,
                        params,
                        return_type,
                        attributes,
                        doc_comment,
                        start_line,
                        end_line,
                        is_generator,
                    },
                    body,
                };
                Ok(ClassMember {
                    kind: ClassMemberKind::Method(Box::new(m)),
                    line: start_line,
                })
            }
            T::Case if mods.is_empty() => {
                self.bump();
                let doc_comment = self.take_doc(false);
                let name = self.identifier()?;
                let value = if self.eat(T::Equals) {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect(T::Semicolon)?;
                Ok(ClassMember {
                    kind: ClassMemberKind::EnumCase(EnumCase {
                        name,
                        value,
                        attributes,
                        doc_comment,
                    }),
                    line,
                })
            }
            _ if !mods.is_empty() => {
                let flags = self.validate_modifiers(Target::Property, &mods)?;
                self.property_decl(flags, attributes, line)
            }
            _ => Err(self.syntax_error()),
        }
    }

    fn property_decl(
        &mut self,
        modifiers: Modifiers,
        attributes: Vec<AttributeGroup>,
        line: Line,
    ) -> PResult<ClassMember> {
        let ty = if starts_type(self.peek()) {
            Some(self.type_expr(false)?)
        } else {
            None
        };
        let mut props = Vec::new();
        loop {
            let i = self.expect(T::Variable)?;
            let default = if self.eat(T::Equals) {
                Some(self.expr()?)
            } else {
                None
            };
            let doc_comment = self.take_doc(true);
            let pline = self.tok_line(i);
            let name = self.name_text(i);
            if props.is_empty() && self.at(T::LBrace) {
                let hooks = self.property_hook_list()?;
                props.push(PropertyElem {
                    name,
                    default,
                    doc_comment,
                    hooks: Some(hooks),
                    line: pline,
                });
                let group = PropertyGroup {
                    modifiers,
                    ty,
                    props,
                    attributes,
                };
                return Ok(ClassMember {
                    kind: ClassMemberKind::Property(group),
                    line,
                });
            }
            props.push(PropertyElem {
                name,
                default,
                doc_comment,
                hooks: None,
                line: pline,
            });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::Semicolon)?;
        let group = PropertyGroup {
            modifiers,
            ty,
            props,
            attributes,
        };
        Ok(ClassMember {
            kind: ClassMemberKind::Property(group),
            line,
        })
    }

    /// `'{' property_hook_list '}'`
    fn property_hook_list(&mut self) -> PResult<Vec<PropertyHook>> {
        self.expect(T::LBrace)?;
        let mut hooks = Vec::new();
        while !self.at(T::RBrace) {
            let attributes = if self.at(T::Attribute) {
                self.attributes()?
            } else {
                Vec::new()
            };
            let mods = self.collect_modifiers();
            let modifiers = if mods.is_empty() {
                Modifiers::empty()
            } else {
                self.validate_modifiers(Target::PropertyHook, &mods)?
            };
            let by_ref = is_amp(self.peek());
            if by_ref {
                self.bump();
            }
            let i = self.expect(T::String)?;
            let doc_comment = self.take_doc(false);
            let start_line = self.toks[i].end_line;
            let params = if self.at(T::LParen) {
                Some(self.parameter_list()?)
            } else {
                None
            };
            self.push_fn();
            let body = match self.peek() {
                T::Semicolon => {
                    self.bump();
                    HookBody::Abstract
                }
                T::LBrace => {
                    self.bump();
                    let b = self.inner_statement_list(&[T::RBrace])?;
                    self.expect(T::RBrace)?;
                    HookBody::Block(b)
                }
                T::DoubleArrow => {
                    self.bump();
                    let e = self.expr()?;
                    self.expect(T::Semicolon)?;
                    HookBody::Expr(e)
                }
                _ => return Err(self.syntax_error()),
            };
            let is_generator = self.pop_fn();
            let end_line = self.cur_line();
            hooks.push(PropertyHook {
                name: self.name_text(i),
                modifiers,
                by_ref,
                params,
                body,
                attributes,
                doc_comment,
                start_line,
                end_line,
                is_generator,
            });
        }
        self.bump();
        Ok(hooks)
    }

    fn trait_adaptations(&mut self) -> PResult<Option<Vec<TraitAdaptation>>> {
        if self.eat(T::Semicolon) {
            return Ok(None);
        }
        self.expect(T::LBrace)?;
        if self.eat(T::RBrace) {
            return Ok(None);
        }
        let mut list = Vec::new();
        while !self.at(T::RBrace) {
            list.push(self.trait_adaptation()?);
        }
        self.bump();
        Ok(Some(list))
    }

    fn trait_adaptation(&mut self) -> PResult<TraitAdaptation> {
        let line = self.line();
        let is_absolute = matches!(
            self.peek(),
            T::Static | T::String | T::NameQualified | T::NameFullyQualified | T::NameRelative
        ) && self.peek_at(1) == T::PaamayimNekudotayim;
        let trait_name = if is_absolute {
            let n = self.class_name()?;
            self.bump();
            Some(n)
        } else {
            None
        };
        let method = self.identifier()?;
        if let (Some(trait_name), true) = (&trait_name, self.at(T::Insteadof)) {
            self.bump();
            let insteadof = self.class_name_list()?;
            self.expect(T::Semicolon)?;
            return Ok(TraitAdaptation::Precedence {
                trait_name: trait_name.clone(),
                method,
                insteadof,
                line,
            });
        }
        self.expect(T::As)?;
        let k = self.peek();
        let (modifiers, alias) = if k == T::String {
            let i = self.bump();
            (Modifiers::empty(), Some(self.name_text(i)))
        } else if k.is_member_modifier() {
            self.bump();
            let alias = if Self::is_identifier(self.peek()) {
                Some(self.identifier()?)
            } else {
                None
            };
            let m = modifier_flag(Target::Method, k).map_err(|m| self.compile_error(m))?;
            (m, alias)
        } else if k.is_reserved_non_modifier() {
            let i = self.pos;
            let s = self.lex_tstring(i)?;
            self.bump();
            (Modifiers::empty(), Some(s))
        } else {
            return Err(self.syntax_error());
        };
        self.expect(T::Semicolon)?;
        Ok(TraitAdaptation::Alias {
            trait_name,
            method,
            modifiers,
            alias,
            line,
        })
    }
}
