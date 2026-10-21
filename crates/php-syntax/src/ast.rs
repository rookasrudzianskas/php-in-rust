//! The abstract syntax tree.
//!
//! The tree mirrors php-src's `zend_ast` closely enough that the AST
//! exporter ([`crate::export`]) reproduces `zend_ast_export()` output, but uses
//! typed Rust enums so a tree-walking interpreter can lower it directly.
//!
//! Every [`Stmt`] and [`Expr`] carries the line number php-src would assign
//! to the corresponding `zend_ast` node (the smallest line of its children,
//! which is what runtime error messages report). Declarations carry start
//! and end lines plus their doc comment.

use std::fmt;
use std::ops::Deref;

/// Line number (1-based).
pub type Line = u32;

/// An immutable byte string. PHP source text and string literals are bytes,
/// not necessarily UTF-8.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ByteStr(Box<[u8]>);

impl ByteStr {
    pub fn new(bytes: impl Into<Box<[u8]>>) -> Self {
        ByteStr(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Lossy UTF-8 conversion (for diagnostics).
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    /// ASCII-lowercased copy.
    pub fn to_ascii_lowercase(&self) -> ByteStr {
        ByteStr(self.0.to_ascii_lowercase().into())
    }

    /// Case-insensitive (ASCII) comparison.
    pub fn eq_ignore_case(&self, other: &[u8]) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl Deref for ByteStr {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for ByteStr {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for ByteStr {
    fn from(b: &[u8]) -> Self {
        ByteStr(b.into())
    }
}

impl From<Vec<u8>> for ByteStr {
    fn from(b: Vec<u8>) -> Self {
        ByteStr(b.into_boxed_slice())
    }
}

impl From<&str> for ByteStr {
    fn from(s: &str) -> Self {
        ByteStr(s.as_bytes().into())
    }
}

impl PartialEq<[u8]> for ByteStr {
    fn eq(&self, other: &[u8]) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<str> for ByteStr {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other.as_bytes()
    }
}

impl PartialEq<&str> for ByteStr {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == other.as_bytes()
    }
}

impl fmt::Debug for ByteStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(&self.0))
    }
}

impl fmt::Display for ByteStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

/// A diagnostic (deprecation / compile warning) emitted while parsing.
pub use crate::{Diagnostic, DiagnosticLevel};

/// A parsed file.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
    /// Byte offset just past `__halt_compiler();` (the value of
    /// `__COMPILER_HALT_OFFSET__`), if present.
    pub halt_offset: Option<usize>,
    /// Deprecations and compile warnings emitted while parsing, in order.
    pub diagnostics: Vec<Diagnostic>,
}

// -------------------------------------------------------------------------
// Names
// -------------------------------------------------------------------------

/// How a name was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameKind {
    /// `Foo`
    Unqualified,
    /// `Foo\Bar`
    Qualified,
    /// `\Foo\Bar`
    FullyQualified,
    /// `namespace\Foo`
    Relative,
}

/// `self`, `parent` or `static` used as a class name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpecialClass {
    Self_,
    Parent,
    Static,
}

/// A class, function or constant name.
#[derive(Clone, Debug, PartialEq)]
pub struct Name {
    pub kind: NameKind,
    /// The name as written, without a leading `\` or `namespace\`.
    pub name: ByteStr,
    pub line: Line,
    /// The fully qualified name (no leading `\`), filled in by
    /// [`crate::resolve`]. Before resolution it equals `name`.
    pub resolved: ByteStr,
    /// For unqualified function and constant references inside a namespace:
    /// if `resolved` does not exist at runtime, fall back to the global
    /// `name` (`zend_resolve_function_name`'s `is_fully_qualified == false`).
    pub fallback: bool,
}

impl Name {
    pub fn new(kind: NameKind, name: ByteStr, line: Line) -> Self {
        Name {
            kind,
            resolved: name.clone(),
            name,
            line,
            fallback: false,
        }
    }

    /// `self` / `parent` / `static` (only when unqualified).
    pub fn special_class(&self) -> Option<SpecialClass> {
        if self.kind != NameKind::Unqualified {
            return None;
        }
        special_class(&self.name)
    }

    /// The text as written in the source (with `\` / `namespace\` prefix).
    pub fn source_text(&self) -> String {
        let n = self.name.to_string_lossy();
        match self.kind {
            NameKind::FullyQualified => format!("\\{n}"),
            NameKind::Relative => format!("namespace\\{n}"),
            _ => n,
        }
    }
}

pub(crate) fn special_class(name: &[u8]) -> Option<SpecialClass> {
    if name.eq_ignore_ascii_case(b"self") {
        Some(SpecialClass::Self_)
    } else if name.eq_ignore_ascii_case(b"parent") {
        Some(SpecialClass::Parent)
    } else if name.eq_ignore_ascii_case(b"static") {
        Some(SpecialClass::Static)
    } else {
        None
    }
}

// -------------------------------------------------------------------------
// Statements
// -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: Line,
}

/// A statement list (body of a block, function, loop...).
pub type Block = Vec<Stmt>;

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// `expr;`
    Expr(Expr),
    /// `echo a, b;` (also `<?= a ?>`)
    Echo(Vec<Expr>),
    /// Text outside `<?php ... ?>`.
    InlineHtml(ByteStr),
    If(Box<IfStmt>),
    While {
        cond: Expr,
        body: Block,
    },
    DoWhile {
        body: Block,
        cond: Expr,
    },
    For(Box<ForStmt>),
    Foreach(Box<ForeachStmt>),
    Switch {
        subject: Expr,
        cases: Vec<SwitchCase>,
    },
    /// `break;` / `break 2;`
    Break(Option<Expr>),
    Continue(Option<Expr>),
    Return(Option<Expr>),
    /// `global $a, $$b;` — each entry is a `Var` expression.
    Global(Vec<Expr>),
    /// `static $a = 1, $b;`
    Static(Vec<StaticVar>),
    /// `unset($a, $b[0]);`
    Unset(Vec<Expr>),
    /// A nested `{ ... }` block.
    Block(Block),
    Try(Box<TryStmt>),
    Goto(ByteStr),
    Label(ByteStr),
    Function(Box<FunctionDecl>),
    Class(Box<ClassDecl>),
    /// Top-level `const A = 1, B = 2;`
    Const(Box<ConstStmt>),
    /// `namespace Foo;` (body `None`) or `namespace Foo { ... }`.
    Namespace {
        name: Option<ByteStr>,
        body: Option<Block>,
    },
    /// `use A\B as C, D;` / `use function ...;` / `use const ...;`
    Use {
        kind: UseKind,
        uses: Vec<UseItem>,
    },
    /// `use A\{B, function c, const D};` — `kind` is `Some` for
    /// `use function A\{...}` / `use const A\{...}`, `None` for mixed groups
    /// (each item then carries its own kind).
    GroupUse {
        prefix: ByteStr,
        kind: Option<UseKind>,
        uses: Vec<UseItem>,
    },
    /// `declare(strict_types=1);` (`body` `None`) or with a block/statement.
    Declare {
        directives: Vec<DeclareDirective>,
        body: Option<Block>,
    },
    /// `__halt_compiler();`
    HaltCompiler {
        offset: usize,
    },
    /// The empty statement `;`.
    Nop,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfStmt {
    /// `if` and `elseif` branches, in order.
    pub branches: Vec<IfBranch>,
    pub else_branch: Option<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfBranch {
    pub cond: Expr,
    pub body: Block,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForStmt {
    pub init: Vec<Expr>,
    pub cond: Vec<Expr>,
    pub step: Vec<Expr>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForeachStmt {
    pub expr: Expr,
    pub key: Option<ForeachTarget>,
    pub value: ForeachTarget,
    pub body: Block,
}

/// A `foreach` key or value target.
#[derive(Clone, Debug, PartialEq)]
pub struct ForeachTarget {
    /// A variable, or an `Array` expression for `list()`/`[]` destructuring.
    pub expr: Expr,
    /// `&$v`
    pub by_ref: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    /// `None` for `default:`.
    pub cond: Option<Expr>,
    pub body: Block,
    /// Written as `case x;` (deprecated) instead of `case x:`.
    pub semicolon: bool,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticVar {
    pub name: ByteStr,
    pub default: Option<Expr>,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TryStmt {
    pub body: Block,
    pub catches: Vec<Catch>,
    pub finally: Option<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Catch {
    pub types: Vec<Name>,
    /// `None` for `catch (Exception)` without a variable.
    pub var: Option<ByteStr>,
    pub body: Block,
    pub line: Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UseKind {
    Class,
    Function,
    Const,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UseItem {
    /// The imported name, without a leading `\` (for group uses: the part
    /// inside the braces).
    pub name: ByteStr,
    pub alias: Option<ByteStr>,
    /// Per-item kind inside a mixed group use.
    pub kind: Option<UseKind>,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclareDirective {
    pub name: ByteStr,
    pub value: Expr,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstStmt {
    pub consts: Vec<ConstElem>,
    pub attributes: Vec<AttributeGroup>,
}

/// `NAME = expr` in a `const` statement or class constant declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstElem {
    pub name: ByteStr,
    /// Namespaced name of a top-level constant (filled by resolution).
    pub fq_name: ByteStr,
    pub value: Expr,
    pub doc_comment: Option<ByteStr>,
    pub line: Line,
}

// -------------------------------------------------------------------------
// Declarations
// -------------------------------------------------------------------------

/// Member and parameter modifiers (a bit set).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(pub u16);

impl Modifiers {
    pub const PUBLIC: Modifiers = Modifiers(1);
    pub const PROTECTED: Modifiers = Modifiers(1 << 1);
    pub const PRIVATE: Modifiers = Modifiers(1 << 2);
    pub const STATIC: Modifiers = Modifiers(1 << 3);
    pub const ABSTRACT: Modifiers = Modifiers(1 << 4);
    pub const FINAL: Modifiers = Modifiers(1 << 5);
    pub const READONLY: Modifiers = Modifiers(1 << 6);
    pub const PUBLIC_SET: Modifiers = Modifiers(1 << 7);
    pub const PROTECTED_SET: Modifiers = Modifiers(1 << 8);
    pub const PRIVATE_SET: Modifiers = Modifiers(1 << 9);
    /// Mask of the visibility modifiers.
    pub const VISIBILITY: Modifiers = Modifiers(0b111);
    /// Mask of the set-visibility modifiers.
    pub const SET_VISIBILITY: Modifiers = Modifiers(0b111 << 7);

    pub fn empty() -> Self {
        Modifiers(0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn intersects(self, other: Modifiers) -> bool {
        self.0 & other.0 != 0
    }
    pub fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 10] = [
            "public",
            "protected",
            "private",
            "static",
            "abstract",
            "final",
            "readonly",
            "public(set)",
            "protected(set)",
            "private(set)",
        ];
        let names: Vec<_> = NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, n)| *n)
            .collect();
        write!(f, "Modifiers({})", names.join(" "))
    }
}

/// A parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: ByteStr,
    pub ty: Option<TypeHint>,
    pub default: Option<Expr>,
    pub by_ref: bool,
    pub variadic: bool,
    /// Constructor promotion modifiers (visibility, `readonly`, set
    /// visibility, `final`). Empty for ordinary parameters.
    pub modifiers: Modifiers,
    pub attributes: Vec<AttributeGroup>,
    pub doc_comment: Option<ByteStr>,
    /// Property hooks on a promoted parameter.
    pub hooks: Option<Vec<PropertyHook>>,
    pub line: Line,
}

/// Data shared by every function-like declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCommon {
    pub by_ref: bool,
    pub params: Vec<Param>,
    pub return_type: Option<TypeHint>,
    pub attributes: Vec<AttributeGroup>,
    pub doc_comment: Option<ByteStr>,
    pub start_line: Line,
    pub end_line: Line,
    /// The body contains `yield` (`ZEND_ACC_GENERATOR`).
    pub is_generator: bool,
}

/// `function foo() {}`
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDecl {
    pub name: ByteStr,
    /// Namespaced name (filled by resolution).
    pub fq_name: ByteStr,
    pub common: FunctionCommon,
    pub body: Block,
}

/// `function () use ($x) {}`
#[derive(Clone, Debug, PartialEq)]
pub struct ClosureDecl {
    pub is_static: bool,
    pub common: FunctionCommon,
    pub uses: Vec<ClosureUse>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClosureUse {
    pub name: ByteStr,
    pub by_ref: bool,
    pub line: Line,
}

/// `fn () => expr`
#[derive(Clone, Debug, PartialEq)]
pub struct ArrowFnDecl {
    pub is_static: bool,
    pub common: FunctionCommon,
    pub body: Box<Expr>,
    /// Written inside parentheses (affects export only).
    pub parenthesized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

/// Class-level modifiers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ClassModifiers {
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
}

/// A class, interface, trait, enum or anonymous class.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassDecl {
    pub kind: ClassKind,
    /// `None` for anonymous classes.
    pub name: Option<ByteStr>,
    /// Namespaced name (filled by resolution).
    pub fq_name: ByteStr,
    pub modifiers: ClassModifiers,
    /// Parent class (classes only).
    pub extends: Option<Name>,
    /// Implemented interfaces; for interfaces, the extended interfaces.
    pub implements: Vec<Name>,
    /// Backing type of a backed enum.
    pub enum_backing_type: Option<TypeHint>,
    pub members: Vec<ClassMember>,
    pub attributes: Vec<AttributeGroup>,
    pub doc_comment: Option<ByteStr>,
    pub start_line: Line,
    pub end_line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassMember {
    pub kind: ClassMemberKind,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClassMemberKind {
    Property(PropertyGroup),
    Const(ClassConstGroup),
    Method(Box<MethodDecl>),
    TraitUse(TraitUse),
    EnumCase(EnumCase),
}

/// `public int $a = 1, $b;`
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyGroup {
    /// `var` is stored as `PUBLIC`.
    pub modifiers: Modifiers,
    pub ty: Option<TypeHint>,
    pub props: Vec<PropertyElem>,
    pub attributes: Vec<AttributeGroup>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyElem {
    pub name: ByteStr,
    pub default: Option<Expr>,
    pub doc_comment: Option<ByteStr>,
    /// `{ get; set { ... } }`
    pub hooks: Option<Vec<PropertyHook>>,
    pub line: Line,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyHook {
    /// `get`, `set` (other names are rejected at compile time).
    pub name: ByteStr,
    /// Only `final` is valid.
    pub modifiers: Modifiers,
    pub by_ref: bool,
    /// `set(Type $value)` parameter list; `None` when omitted.
    pub params: Option<Vec<Param>>,
    pub body: HookBody,
    pub attributes: Vec<AttributeGroup>,
    pub doc_comment: Option<ByteStr>,
    pub start_line: Line,
    pub end_line: Line,
    pub is_generator: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HookBody {
    /// `get;`
    Abstract,
    Block(Block),
    /// `get => expr;`
    Expr(Expr),
}

/// `final public const int A = 1, B = 2;`
#[derive(Clone, Debug, PartialEq)]
pub struct ClassConstGroup {
    pub modifiers: Modifiers,
    pub ty: Option<TypeHint>,
    pub consts: Vec<ConstElem>,
    pub attributes: Vec<AttributeGroup>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDecl {
    pub name: ByteStr,
    pub modifiers: Modifiers,
    pub common: FunctionCommon,
    /// `None` for abstract/interface methods (`;` body).
    pub body: Option<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraitUse {
    pub traits: Vec<Name>,
    /// `None` for `use A;` (no braces), `Some(vec![])` for `use A {}`.
    pub adaptations: Option<Vec<TraitAdaptation>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TraitAdaptation {
    /// `A::foo insteadof B, C;`
    Precedence {
        trait_name: Name,
        method: ByteStr,
        insteadof: Vec<Name>,
        line: Line,
    },
    /// `foo as protected bar;` / `A::foo as bar;`
    Alias {
        trait_name: Option<Name>,
        method: ByteStr,
        modifiers: Modifiers,
        alias: Option<ByteStr>,
        line: Line,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumCase {
    pub name: ByteStr,
    pub value: Option<Expr>,
    pub attributes: Vec<AttributeGroup>,
    pub doc_comment: Option<ByteStr>,
}

/// One `#[...]` group.
pub type AttributeGroup = Vec<Attribute>;

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: Name,
    /// `None` for `#[Foo]`, `Some` for `#[Foo(...)]`.
    pub args: Option<ArgList>,
    pub line: Line,
}

// -------------------------------------------------------------------------
// Types
// -------------------------------------------------------------------------

/// Built-in type names (only recognised when unqualified).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinType {
    Null,
    True,
    False,
    Int,
    Float,
    String,
    Bool,
    Void,
    Never,
    Iterable,
    Object,
    Mixed,
}

impl BuiltinType {
    pub fn from_name(name: &[u8]) -> Option<BuiltinType> {
        use BuiltinType::*;
        const ALL: [(&[u8], BuiltinType); 12] = [
            (b"null", Null),
            (b"true", True),
            (b"false", False),
            (b"int", Int),
            (b"float", Float),
            (b"string", String),
            (b"bool", Bool),
            (b"void", Void),
            (b"never", Never),
            (b"iterable", Iterable),
            (b"object", Object),
            (b"mixed", Mixed),
        ];
        ALL.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, t)| *t)
    }

    pub fn name(self) -> &'static str {
        use BuiltinType::*;
        match self {
            Null => "null",
            True => "true",
            False => "false",
            Int => "int",
            Float => "float",
            String => "string",
            Bool => "bool",
            Void => "void",
            Never => "never",
            Iterable => "iterable",
            Object => "object",
            Mixed => "mixed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeHint {
    /// `int`, `string`, ... (unqualified built-in names). `source` keeps the
    /// spelling as written.
    Builtin {
        ty: BuiltinType,
        source: ByteStr,
    },
    /// A class name (including `self` / `parent`).
    Class(Name),
    Array,
    Callable,
    Static,
    /// `?T`
    Nullable(Box<TypeHint>),
    /// `A|B`
    Union(Vec<TypeHint>),
    /// `A&B`
    Intersection(Vec<TypeHint>),
}

// -------------------------------------------------------------------------
// Expressions
// -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: Line,
}

impl Expr {
    pub fn new(kind: ExprKind, line: Line) -> Self {
        Expr { kind, line }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    String(ByteStr),
    /// `"a $b {$c}"` / heredoc with interpolation.
    Interpolated(Vec<StrPart>),
    /// `` `cmd $x` ``
    ShellExec(Vec<StrPart>),
    /// A constant (`FOO`, `true`, `null`, `\A\B`).
    Const(Name),
    MagicConst(MagicConst),
    /// `A::B`, `$x::B`, `A::{expr}`
    ClassConst {
        class: ClassRef,
        name: ClassConstName,
    },
    /// `A::class`, `$x::class`
    ClassName(ClassRef),
    /// `$x`, `$$x`, `${expr}`
    Var(VarName),
    /// `[...]`, `array(...)`, `list(...)`. `None` items are skipped
    /// elements (`[, $b] = ...`).
    Array {
        items: Vec<Option<ArrayItem>>,
        syntax: ArraySyntax,
    },
    /// `$a[i]`, `$a[]` (index `None`).
    Dim {
        base: Box<Expr>,
        index: Option<Box<Expr>>,
    },
    /// `$a->b`, `$a?->b`
    Prop {
        object: Box<Expr>,
        name: MemberName,
        nullsafe: bool,
    },
    /// `A::$b`
    StaticProp {
        class: ClassRef,
        name: VarName,
        parenthesized: bool,
    },
    /// `foo(...)`, `$f(...)`. Placeholder arguments (`foo(...)`) make this a
    /// first-class callable / partial application.
    Call {
        callee: Callee,
        args: ArgList,
    },
    MethodCall {
        object: Box<Expr>,
        name: MemberName,
        args: ArgList,
        nullsafe: bool,
    },
    StaticCall {
        class: ClassRef,
        name: MemberName,
        args: ArgList,
    },
    /// `new A(...)`, `new $x`, `new class {...}`
    New {
        class: NewClass,
        args: ArgList,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    AssignRef {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    AssignOp {
        op: BinaryOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `$a ??= b`
    AssignCoalesce {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    IncDec {
        op: IncDecOp,
        target: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Cast {
        kind: CastKind,
        expr: Box<Expr>,
    },
    /// `(void) expr` — only valid as a statement or in `for` expression lists.
    VoidCast(Box<Expr>),
    /// `@expr`
    Silence(Box<Expr>),
    /// `isset($a, $b)`
    Isset(Vec<Expr>),
    Empty(Box<Expr>),
    Include {
        kind: IncludeKind,
        expr: Box<Expr>,
    },
    Eval(Box<Expr>),
    Print(Box<Expr>),
    Closure(Box<ClosureDecl>),
    ArrowFn(Box<ArrowFnDecl>),
    /// `a ? b : c`, `a ?: c` (`then` `None`).
    Ternary {
        cond: Box<Expr>,
        then: Option<Box<Expr>>,
        else_: Box<Expr>,
        parenthesized: bool,
    },
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Instanceof {
        expr: Box<Expr>,
        class: ClassRef,
    },
    Throw(Box<Expr>),
    Yield {
        key: Option<Box<Expr>>,
        value: Option<Box<Expr>>,
    },
    YieldFrom(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MagicConst {
    Line,
    File,
    Dir,
    Class,
    Trait,
    Method,
    Function,
    Property,
    Namespace,
}

impl MagicConst {
    pub fn name(self) -> &'static str {
        match self {
            MagicConst::Line => "__LINE__",
            MagicConst::File => "__FILE__",
            MagicConst::Dir => "__DIR__",
            MagicConst::Class => "__CLASS__",
            MagicConst::Trait => "__TRAIT__",
            MagicConst::Method => "__METHOD__",
            MagicConst::Function => "__FUNCTION__",
            MagicConst::Property => "__PROPERTY__",
            MagicConst::Namespace => "__NAMESPACE__",
        }
    }
}

/// A part of an interpolated string.
#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Literal(ByteStr),
    Expr {
        expr: Expr,
        syntax: InterpolationSyntax,
    },
}

/// How an interpolated expression was written (the `${...}` forms are
/// deprecated at compile time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpolationSyntax {
    /// `$a`, `$a[0]`, `$a->b`, `{$expr}`
    Simple,
    /// `${name}` / `${name[expr]}`
    DollarCurly,
    /// `${expr}` (variable variable)
    DollarCurlyVarVar,
}

/// The class part of `A::x`, `new A`, `instanceof A`.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassRef {
    /// A class name, including `self`, `parent` and `static`.
    Name(Name),
    /// A dynamic class (`$obj::x`, `(expr)::x`).
    Expr(Box<Expr>),
}

/// The class of a `new` expression.
#[derive(Clone, Debug, PartialEq)]
pub enum NewClass {
    Name(Name),
    Expr(Box<Expr>),
    Anonymous(Box<ClassDecl>),
}

/// A variable name: `$x` or `${expr}` / `$$x`.
#[derive(Clone, Debug, PartialEq)]
pub enum VarName {
    Name(ByteStr),
    Expr(Box<Expr>),
}

/// A property or method name: `->foo`, `->$x`, `->{expr}`.
#[derive(Clone, Debug, PartialEq)]
pub enum MemberName {
    Ident(ByteStr),
    Expr(Box<Expr>),
}

/// A class constant name: `A::B` or `A::{expr}`.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassConstName {
    Ident(ByteStr),
    Expr(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Callee {
    /// `foo()`, `\A\foo()`, `exit()`, `clone()`.
    Name(Name),
    Expr(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArraySyntax {
    /// `[...]`
    Short,
    /// `array(...)`
    Long,
    /// `list(...)`
    List,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
    pub by_ref: bool,
    /// `...$x`
    pub unpack: bool,
    pub line: Line,
}

/// A call argument list.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ArgList {
    pub args: Vec<Arg>,
    pub line: Line,
}

impl ArgList {
    /// Whether this is a first-class callable / partial application
    /// (`ZEND_AST_CALLABLE_CONVERT`): some argument is a placeholder.
    pub fn is_callable_convert(&self) -> bool {
        self.args.iter().any(Arg::is_placeholder)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Positional(Expr),
    /// `...$args`
    Unpack(Expr),
    /// `name: expr`
    Named {
        name: ByteStr,
        value: Expr,
    },
    /// `?` (partial function application)
    Placeholder,
    /// `name: ?`
    NamedPlaceholder(ByteStr),
    /// `...` (first-class callable syntax)
    VariadicPlaceholder,
}

impl Arg {
    pub fn is_placeholder(&self) -> bool {
        matches!(
            self,
            Arg::Placeholder | Arg::NamedPlaceholder(_) | Arg::VariadicPlaceholder
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    /// `None` for `default`.
    pub conds: Option<Vec<Expr>>,
    pub body: Expr,
    pub line: Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    /// `&&` / `and`
    BooleanAnd,
    /// `||` / `or`
    BooleanOr,
    /// `xor`
    LogicalXor,
    Identical,
    NotIdentical,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Spaceship,
    /// `??`
    Coalesce,
    /// `|>`
    Pipe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `!`
    Not,
    /// `~`
    BitNot,
    /// `+`
    Plus,
    /// `-`
    Minus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IncDecOp {
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CastKind {
    Int,
    Float,
    String,
    Array,
    Object,
    Bool,
    /// `(unset)` — rejected at compile time.
    Unset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IncludeKind {
    Include,
    IncludeOnce,
    Require,
    RequireOnce,
}
