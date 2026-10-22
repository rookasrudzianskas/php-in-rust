//! The lowered, executable tree (the equivalent of `zend_op_array`).
//!
//! The lowering pass ([`super::lower`]) turns the php-syntax AST into this
//! representation: local variables are resolved to compiled-variable slots,
//! names are resolved and pre-interned as [`PhpStr`], literals are already
//! [`Value`]s, and every node that can raise a diagnostic carries the line
//! php-src would attribute to the corresponding opline (`op_line`).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::class::Function;
use crate::types::TypeDecl;
use crate::value::{PhpStr, Value};

/// A compiled-variable slot index.
pub type Slot = u32;

/// Names of the compiled variables of a function (`op_array->vars`).
#[derive(Debug, Default)]
pub struct CvTable {
    pub names: Vec<PhpStr>,
    pub index: HashMap<PhpStr, Slot>,
}

impl CvTable {
    pub fn lookup(&self, name: &[u8]) -> Option<Slot> {
        self.index.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// A compiled file or `eval()`'d string.
pub struct Unit {
    pub file: PhpStr,
    /// The pseudo-main function (top-level code).
    pub main: Rc<FuncCode>,
    pub strict: bool,
    pub halt_offset: Option<usize>,
}

/// What kind of code a [`FuncCode`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FuncKind {
    /// Top-level code of a file or `eval()`.
    Main,
    Function,
    Method,
    Closure,
    ArrowFn,
    /// A property hook body.
    Hook,
}

/// Function flags known at compile time.
pub mod code_flags {
    /// Uses `$this`.
    pub const USES_THIS: u32 = 1 << 0;
    /// Needs a name → slot map at runtime (`$$x`, `compact`, `extract`,
    /// `get_defined_vars`, `include`, `eval`).
    pub const DYNAMIC_VARS: u32 = 1 << 1;
    /// Contains `yield`.
    pub const GENERATOR: u32 = 1 << 2;
    pub const RETURNS_REF: u32 = 1 << 3;
    pub const STRICT: u32 = 1 << 4;
    pub const STATIC_CLOSURE: u32 = 1 << 5;
    /// `#[\NoDiscard]`.
    pub const NODISCARD: u32 = 1 << 6;
    /// `#[\Deprecated]`.
    pub const DEPRECATED: u32 = 1 << 7;
    /// Abstract / interface method (no body).
    pub const ABSTRACT: u32 = 1 << 8;
    /// Uses `func_get_args()` & co.
    pub const VARIADIC_ACCESS: u32 = 1 << 9;
}

/// A compiled function body (shared by every closure object created from it).
pub struct FuncCode {
    pub name: PhpStr,
    pub kind: FuncKind,
    pub flags: u32,
    pub cvs: Rc<CvTable>,
    pub params: Vec<ParamInfo>,
    pub body: Vec<Stmt>,
    pub return_type: Option<TypeInfo>,
    pub file: PhpStr,
    pub start_line: u32,
    pub end_line: u32,
    /// Closure `use` bindings.
    pub uses: Vec<UseBinding>,
    /// Number of `static $x` declarations (indexes into the per-function table).
    pub num_statics: u32,
    /// Declaring class name (for methods and closures declared in classes).
    pub class_name: Option<PhpStr>,
    /// The class is a trait (`__CLASS__` resolves at runtime).
    pub in_trait: bool,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<AttrDef>>,
    /// Goto labels: label id → name (for error messages).
    pub labels: Vec<PhpStr>,
    /// Nested function/class declarations (bound at runtime).
    pub namespace: PhpStr,
}

impl FuncCode {
    pub fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    pub fn is_generator(&self) -> bool {
        self.has(code_flags::GENERATOR)
    }

    pub fn returns_ref(&self) -> bool {
        self.has(code_flags::RETURNS_REF)
    }

    pub fn strict(&self) -> bool {
        self.has(code_flags::STRICT)
    }
}

/// A closure `use ($x, &$y)` binding: copy `outer` of the defining scope into `inner`.
#[derive(Clone, Debug)]
pub struct UseBinding {
    pub name: PhpStr,
    /// Slot in the defining function (`None` when the defining code has no such CV,
    /// e.g. dynamic scope; looked up by name).
    pub outer: Slot,
    pub inner: Slot,
    pub by_ref: bool,
    pub line: u32,
}

/// A declared type, with the AST spelling kept for messages.
#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub decl: TypeDecl,
    /// The type as written, resolved (`zend_type_to_string`), cached for messages.
    pub text: PhpStr,
}

/// A parameter of a user function.
pub struct ParamInfo {
    pub name: PhpStr,
    pub slot: Slot,
    pub by_ref: bool,
    pub variadic: bool,
    pub ty: Option<TypeInfo>,
    pub default: Option<Expr>,
    /// The default is a compile-time constant (precomputed).
    pub default_const: Option<Value>,
    /// Source text of the default (Reflection / error messages).
    pub default_text: Option<PhpStr>,
    /// Constructor promotion (`public int $x`): property flags.
    pub promote: Option<u32>,
    pub attributes: Vec<Rc<AttrDef>>,
    pub doc_comment: Option<PhpStr>,
    pub line: u32,
}

/// An attribute (`#[Name(args)]`).
pub struct AttrDef {
    pub name: PhpStr,
    pub args: Vec<(Option<PhpStr>, Expr)>,
    pub line: u32,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

pub struct Stmt {
    pub kind: StmtKind,
    pub line: u32,
}

pub type Block = Vec<Stmt>;

pub enum StmtKind {
    Expr(Expr),
    Echo(Vec<Expr>),
    InlineHtml(PhpStr),
    If(Vec<(Expr, Block)>, Option<Block>),
    While(Expr, Block),
    DoWhile(Block, Expr),
    For(Box<ForStmt>),
    Foreach(Box<ForeachStmt>),
    Switch(Box<SwitchStmt>),
    Break(u32),
    Continue(u32),
    Return(Option<Expr>),
    Global(Vec<GlobalVar>),
    Static(Vec<StaticVarDecl>),
    Unset(Vec<LVal>),
    Block(Block),
    Try(Box<TryStmt>),
    Goto(u32),
    Label(u32),
    /// Runtime function declaration (conditional or nested).
    FuncDecl(Rc<FuncDeclDef>),
    /// Runtime class declaration.
    ClassDecl(Rc<ClassDef>),
    /// Class declared at compile time (statement is a no-op at runtime).
    Nop,
    Const(Vec<(PhpStr, Expr)>),
    /// `declare(ticks=N)` blocks: run tick functions after each statement.
    Ticks(u32, Block),
    HaltCompiler,
}

pub struct ForStmt {
    pub init: Vec<Expr>,
    pub cond: Vec<Expr>,
    pub step: Vec<Expr>,
    pub body: Block,
}

pub struct ForeachStmt {
    pub expr: Expr,
    /// The subject is a variable iterated by reference.
    pub subject_place: Option<LVal>,
    pub key: Option<LVal>,
    pub value: LVal,
    pub by_ref: bool,
    pub body: Block,
    pub line: u32,
}

pub struct SwitchStmt {
    pub subject: Expr,
    pub cases: Vec<SwitchCase>,
    /// Index of the `default` case.
    pub default: Option<usize>,
}

pub struct SwitchCase {
    pub cond: Option<Expr>,
    pub body: Block,
    pub line: u32,
}

pub struct TryStmt {
    pub body: Block,
    pub catches: Vec<CatchClause>,
    pub finally: Option<Block>,
}

pub struct CatchClause {
    /// Class names (resolved).
    pub types: Vec<PhpStr>,
    pub var: Option<Slot>,
    pub body: Block,
    pub line: u32,
}

pub enum GlobalVar {
    Name(PhpStr, Slot),
    Dynamic(Expr),
}

pub struct StaticVarDecl {
    pub slot: Slot,
    /// Index into the function's static table.
    pub index: u32,
    pub init: Option<Expr>,
    pub name: PhpStr,
}

/// A runtime function declaration.
pub struct FuncDeclDef {
    pub name: PhpStr,
    pub lc_name: PhpStr,
    pub code: Rc<FuncCode>,
    /// The function object, created once (declarations in loops redeclare the same one).
    pub func: OnceCell<Rc<Function>>,
}

// ---------------------------------------------------------------------------
// Classes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKindIr {
    Class,
    Interface,
    Trait,
    Enum,
}

pub struct ClassDef {
    pub name: PhpStr,
    pub lc_name: PhpStr,
    pub kind: ClassKindIr,
    /// `class_flags::*` (abstract/final/readonly/anon).
    pub flags: u32,
    pub parent: Option<PhpStr>,
    pub interfaces: Vec<PhpStr>,
    pub traits: Vec<TraitUseDef>,
    pub consts: Vec<ClassConstDef>,
    pub props: Vec<PropDef>,
    pub methods: Vec<MethodDef>,
    pub cases: Vec<EnumCaseDef>,
    pub enum_backing: Option<crate::class::EnumBacking>,
    pub attributes: Vec<Rc<AttrDef>>,
    pub doc_comment: Option<PhpStr>,
    pub file: PhpStr,
    pub start_line: u32,
    pub end_line: u32,
    /// Anonymous classes are declared once and reused.
    pub declared: RefCell<Option<crate::class::ClassRef>>,
    /// Bumped when the class was early-bound at compile time.
    pub early_bound: Cell<bool>,
    /// The declaring file uses `strict_types=1` (attribute arguments).
    pub strict: bool,
}

pub struct TraitUseDef {
    pub traits: Vec<PhpStr>,
    pub rules: Vec<TraitRule>,
    pub line: u32,
}

pub enum TraitRule {
    /// `T::m insteadof A, B`
    Precedence {
        trait_name: PhpStr,
        method: PhpStr,
        insteadof: Vec<PhpStr>,
        line: u32,
    },
    /// `[T::]m as [visibility] [alias]`
    Alias {
        trait_name: Option<PhpStr>,
        method: PhpStr,
        flags: u32,
        alias: Option<PhpStr>,
        line: u32,
    },
}

pub struct ClassConstDef {
    pub name: PhpStr,
    pub flags: u32,
    pub ty: Option<TypeInfo>,
    pub value: Rc<ConstExpr>,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<AttrDef>>,
    pub line: u32,
}

pub struct PropDef {
    pub name: PhpStr,
    pub flags: u32,
    pub ty: Option<TypeInfo>,
    /// `None`: no default (`null` for untyped, uninitialised for typed).
    pub default: Option<Rc<ConstExpr>>,
    pub hooks: Vec<HookDef>,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<AttrDef>>,
    pub line: u32,
}

pub struct HookDef {
    pub kind: HookKind,
    pub code: Option<Rc<FuncCode>>,
    pub flags: u32,
    pub line: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookKind {
    Get,
    Set,
}

pub struct MethodDef {
    pub name: PhpStr,
    pub flags: u32,
    pub code: Rc<FuncCode>,
    pub line: u32,
}

pub struct EnumCaseDef {
    pub name: PhpStr,
    pub value: Option<Rc<ConstExpr>>,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<AttrDef>>,
    pub line: u32,
}

/// A constant expression (class constant, property default, enum case value),
/// evaluated lazily in the declaring class's scope.
pub struct ConstExpr {
    pub expr: Expr,
    /// Precomputed value when the expression is a literal.
    pub literal: Option<Value>,
    pub file: PhpStr,
    pub line: u32,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// A class reference in `A::x`, `new A`, `instanceof A`.
pub enum ClassSpec {
    /// A resolved class name.
    Named(PhpStr),
    Self_,
    Parent,
    Static,
    /// A dynamic class (`$obj::x`, `$name::x`).
    Expr(Box<Expr>),
}

/// A property or method name.
pub enum Member {
    Name(PhpStr),
    Expr(Box<Expr>),
}

/// A function call target.
pub enum Callee {
    Named(Box<NamedCallee>),
    Expr(Box<Expr>),
}

pub struct NamedCallee {
    /// The resolved (namespaced) name.
    pub name: PhpStr,
    pub lc: PhpStr,
    /// Global fallback for unqualified calls inside a namespace.
    pub fallback: Option<(PhpStr, PhpStr)>,
    pub cache: OnceCell<Rc<Function>>,
}

pub struct ArgList {
    pub args: Vec<Arg>,
    pub has_spread: bool,
    pub has_named: bool,
}

pub enum Arg {
    Pos(Expr),
    Spread(Expr),
    Named(PhpStr, Expr),
}

pub struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
    pub by_ref: bool,
    pub spread: bool,
}

pub enum InterpPart {
    Lit(PhpStr),
    Expr(Expr),
}

/// A constant reference (`FOO`, `\A\FOO`).
pub struct ConstRef {
    pub name: PhpStr,
    /// Global fallback name for unqualified constants in a namespace.
    pub fallback: Option<PhpStr>,
    /// Name to show in "Undefined constant" errors.
    pub display: PhpStr,
    pub cache: OnceCell<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
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
    Shl,
    Shr,
    Xor,
    Identical,
    NotIdentical,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Spaceship,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    BitNot,
    Plus,
    Minus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastOp {
    Int,
    Float,
    String,
    Array,
    Object,
    Bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncDec {
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncludeOp {
    Include,
    IncludeOnce,
    Require,
    RequireOnce,
}

pub struct MatchArm {
    pub conds: Option<Vec<Expr>>,
    pub body: Expr,
}

pub struct CallExpr {
    pub callee: Callee,
    pub args: ArgList,
    pub line: u32,
}

pub struct MethodCallExpr {
    pub object: Expr,
    pub name: Member,
    pub args: ArgList,
    pub nullsafe: bool,
    pub line: u32,
}

pub struct StaticCallExpr {
    pub class: ClassSpec,
    pub name: Member,
    pub args: ArgList,
    pub line: u32,
}

pub struct NewExpr {
    pub class: NewTarget,
    pub args: ArgList,
    pub line: u32,
}

pub enum NewTarget {
    Class(ClassSpec),
    Anonymous(Rc<ClassDef>),
}

/// A first-class callable (`f(...)`, `$o->m(...)`, `A::m(...)`).
pub enum CallableConvert {
    Func(Callee),
    Method(Expr, Member, bool),
    Static(ClassSpec, Member),
}

pub struct ClosureExpr {
    pub code: Rc<FuncCode>,
    pub is_static: bool,
    /// Arrow functions capture by value automatically: (outer, inner) slots.
    pub auto_capture: Vec<(Slot, Slot)>,
}

pub enum Expr {
    Lit(Value),
    Interp(Vec<InterpPart>),
    /// `$x` (read).
    Var(Slot, u32),
    /// Any other readable place (`$a[x]`, `$o->p`, `A::$p`, `$$x`, ...).
    Place(Box<LVal>),
    This(u32),
    /// `$GLOBALS` (read-only copy).
    Globals,
    Array(Vec<ArrayItem>, u32),
    Const(Box<ConstRef>, u32),
    /// `__CLASS__` in traits/closures (runtime scope).
    ClassMagic,
    /// `__FUNCTION__`/`__METHOD__` are literals; this is `__PROPERTY__` in hooks.
    ClassConst(Box<(ClassSpec, Member)>, u32),
    ClassName(Box<ClassSpec>, u32),
    Call(Box<CallExpr>),
    MethodCall(Box<MethodCallExpr>),
    StaticCall(Box<StaticCallExpr>),
    New(Box<NewExpr>),
    CallableConvert(Box<CallableConvert>, u32),
    Assign(Box<LVal>, Box<Expr>, u32),
    AssignRef(Box<LVal>, Box<Expr>, u32),
    AssignOp(BinOp, Box<LVal>, Box<Expr>, u32),
    AssignCoalesce(Box<LVal>, Box<Expr>, u32),
    IncDec(IncDec, Box<LVal>, u32),
    Bin(BinOp, Box<Expr>, Box<Expr>, u32),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Coalesce(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>, u32),
    Cast(CastOp, Box<Expr>, u32),
    Silence(Box<Expr>),
    Isset(Vec<LVal>),
    Empty(Box<Expr>),
    Include(IncludeOp, Box<Expr>, u32),
    Eval(Box<Expr>, u32),
    Print(Box<Expr>),
    Exit(Option<Box<Expr>>, u32),
    Clone(Box<Expr>, Option<Box<Expr>>, u32),
    Closure(Rc<ClosureExpr>, u32),
    Ternary(Box<Expr>, Option<Box<Expr>>, Box<Expr>),
    Match(Box<Expr>, Vec<MatchArm>, u32),
    Instanceof(Box<Expr>, Box<ClassSpec>, u32),
    Throw(Box<Expr>, u32),
    Yield(Option<Box<Expr>>, Option<Box<Expr>>, u32),
    YieldFrom(Box<Expr>, u32),
    ShellExec(Vec<InterpPart>, u32),
    /// `assert(expr, msg)` with the exported source text.
    Assert(Box<ArgList>, PhpStr, u32),
    /// `$x |> f(...)`
    Pipe(Box<Expr>, Box<Expr>, u32),
    /// `(void) expr`
    Void(Box<Expr>),
    /// The outermost node of a nullsafe chain (`$a?->b->c`): a short-circuit
    /// inside evaluates the whole chain to `null`.
    Chain(Box<Expr>),
    /// A place fetched for reference-taking (`&$a[0]`): yields `Value::Ref`.
    MakeRef(Box<LVal>, u32),
    /// A place read in `BP_VAR_IS` mode (operand of `??` and `empty()`).
    PlaceIs(Box<LVal>),
    /// `obj->prop` / `obj?->prop` inside a constant expression
    /// (`ZEND_AST_PROP` in zend_ast_evaluate: enums only).
    ConstProp(Box<Expr>, Member, bool, u32),
}

/// List-destructuring pattern element.
pub struct ListItem {
    pub key: Option<Expr>,
    pub target: LVal,
    pub by_ref: bool,
}

/// A writable / referenceable place (`zend_compile_var` targets).
pub enum LVal {
    Var(Slot),
    /// `$$expr` / `${expr}`
    VarVar(Box<Expr>),
    /// Superglobal by name (`$_SERVER`, `$GLOBALS['x']` → global `x`).
    Global(Box<Expr>),
    This,
    Dim(Box<LVal>, Option<Box<Expr>>, u32),
    Prop(Box<LVal>, Member, bool, u32),
    StaticProp(Box<ClassSpec>, Member, u32),
    /// A temporary (function result, literal, ...).
    Temp(Box<Expr>),
    List(Vec<Option<ListItem>>, u32),
}

impl LVal {
    /// Whether the place is a plain compiled variable.
    pub fn as_var(&self) -> Option<Slot> {
        match self {
            LVal::Var(s) => Some(*s),
            _ => None,
        }
    }
}
