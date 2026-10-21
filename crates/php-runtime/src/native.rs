//! Native (built-in) functions: the extension contract (ARCHITECTURE.md §2.5).
//!
//! A built-in is a [`NativeFunction`]: its exact php-src stub signature plus a
//! Rust handler. Before the handler runs, [`Vm::prepare_native_args`] applies
//! `zend_parse_parameters` semantics with php-src's exact messages: arity,
//! named arguments, defaults, weak/strict scalar coercion, the
//! "Passing null to parameter" deprecation, class/array/callable checks and
//! by-reference parameters. Handlers then read already-coerced values:
//!
//! ```ignore
//! fn str_repeat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
//!     let s = args.str(0).clone();
//!     let times = args.int(1);
//!     if times < 0 {
//!         return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
//!     }
//!     Ok(Value::from(s.repeat(times as usize)))
//! }
//! pub static STR_REPEAT: NativeFunction =
//!     NativeFunction { sig: "str_repeat(string $string, int $times): string", handler: str_repeat };
//! ```

use std::cell::OnceCell;
use std::fmt;
use std::rc::Rc;

use crate::class::{ClassEntry, ClassRef, Function, FunctionKind, fn_flags};
use crate::constants::{Constant, const_flags};
use crate::ini::IniDef;
use crate::object::ObjRef;
use crate::types::{CoerceMode, TypeDecl, mask};
use crate::util::lower;
use crate::value::{PhpArrayRef, PhpRef, PhpStr, Value};
use crate::vm::{Frame, PResult, Vm};

/// Handler signature of a built-in function.
pub type NativeHandler = fn(&mut Vm, &mut Args) -> PResult<Value>;

/// A built-in function: stub signature + handler.
///
/// Copy the declaration from the php-src stub verbatim (the `function`
/// keyword and trailing `{}` are optional). Include a `/** @prefer-ref $x */`
/// comment when the stub's docblock has one.
pub struct NativeFunction {
    /// Exact signature from php-src `*.stub.php`, e.g.
    /// `"str_pad(string $string, int $length, string $pad_string = \" \", int $pad_type = STR_PAD_RIGHT): string"`.
    pub sig: &'static str,
    pub handler: NativeHandler,
}

impl NativeFunction {
    /// The function name from the signature.
    pub fn name(&self) -> &'static str {
        sig_name(self.sig)
    }
}

impl fmt::Debug for NativeFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NativeFunction({})", self.sig)
    }
}

fn sig_name(sig: &str) -> &str {
    let mut s = sig.trim_start();
    while s.starts_with("/*") {
        s = s.find("*/").map_or("", |i| s[i + 2..].trim_start());
    }
    loop {
        let before = s;
        for kw in [
            "final",
            "abstract",
            "public",
            "protected",
            "private",
            "static",
            "function",
        ] {
            if let Some(rest) = s.strip_prefix(kw) {
                if rest.starts_with(char::is_whitespace) {
                    s = rest.trim_start();
                }
            }
        }
        if s == before {
            break;
        }
    }
    let s = s.strip_prefix('&').map(str::trim_start).unwrap_or(s);
    let end = s
        .find(|c: char| c == '(' || c.is_whitespace())
        .unwrap_or(s.len());
    &s[..end]
}

// ---------------------------------------------------------------------------
// Signatures
// ---------------------------------------------------------------------------

/// A parsed function signature (arginfo).
#[derive(Debug)]
pub struct Signature {
    pub name: String,
    /// Declared parameters, including a trailing variadic one.
    pub params: Vec<Param>,
    pub return_type: Option<TypeDecl>,
    pub returns_ref: bool,
    /// `required_num_args`.
    pub required: usize,
}

/// A parameter (`zend_arg_info`).
#[derive(Debug)]
pub struct Param {
    pub name: PhpStr,
    pub ty: Option<TypeDecl>,
    pub by_ref: bool,
    pub variadic: bool,
    pub default: Option<DefaultValue>,
    /// `@prefer-ref`: by-reference if possible, by-value accepted silently.
    pub prefer_ref: bool,
    cached_default: OnceCell<Value>,
}

impl Param {
    pub fn new(name: impl Into<PhpStr>, ty: Option<TypeDecl>) -> Param {
        Param {
            name: name.into(),
            ty,
            by_ref: false,
            variadic: false,
            default: None,
            prefer_ref: false,
            cached_default: OnceCell::new(),
        }
    }

    pub fn is_optional(&self) -> bool {
        self.default.is_some() || self.variadic
    }

    /// Type string for messages (`mixed` when untyped).
    pub fn type_string(&self) -> String {
        self.ty
            .as_ref()
            .map_or_else(|| "mixed".to_string(), TypeDecl::to_type_string)
    }
}

/// A default value from a stub.
#[derive(Debug, Clone)]
pub enum DefaultValue {
    /// `= UNKNOWN`: optional without a known default (`is_passed()` tells).
    Unknown,
    Expr(ConstExpr),
}

/// The constant-expression subset used by stub defaults.
#[derive(Debug, Clone)]
pub enum ConstExpr {
    Literal(Value),
    EmptyArray,
    Const(String),
    ClassConst(String, String),
    ClassName(String),
    Neg(Box<ConstExpr>),
    Binary(BinOp, Box<ConstExpr>, Box<ConstExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Add,
    Sub,
    Mul,
    Shl,
}

impl Signature {
    /// An empty signature (used as a placeholder for user functions).
    pub fn empty(name: &str) -> Signature {
        Signature {
            name: name.to_string(),
            params: Vec::new(),
            return_type: None,
            returns_ref: false,
            required: 0,
        }
    }

    /// Builds a signature from parameters (for userland functions).
    pub fn new(
        name: &str,
        params: Vec<Param>,
        return_type: Option<TypeDecl>,
        returns_ref: bool,
    ) -> Signature {
        let required = params
            .iter()
            .rposition(|p| !p.is_optional())
            .map_or(0, |i| i + 1);
        Signature {
            name: name.to_string(),
            params,
            return_type,
            returns_ref,
            required,
        }
    }

    /// Number of non-variadic parameters (`num_args`).
    pub fn num_args(&self) -> usize {
        self.params.iter().filter(|p| !p.variadic).count()
    }

    pub fn variadic(&self) -> Option<&Param> {
        self.params.last().filter(|p| p.variadic)
    }

    pub fn param_index(&self, name: &[u8]) -> Option<usize> {
        self.params
            .iter()
            .position(|p| !p.variadic && p.name.as_bytes() == name)
    }

    /// Parses php-src stub syntax (`function` keyword optional).
    pub fn parse(sig: &str) -> Result<Signature, String> {
        let mut p = SigParser {
            s: sig.as_bytes(),
            i: 0,
        };
        p.ws();
        for kw in [
            "final",
            "abstract",
            "public",
            "protected",
            "private",
            "static",
            "final",
        ] {
            p.keyword(kw);
            p.ws();
        }
        p.keyword("function");
        p.ws();
        let returns_ref = p.eat(b'&');
        p.ws();
        let name = p.ident().ok_or("missing function name")?;
        p.ws();
        if !p.eat(b'(') {
            return Err("expected '('".into());
        }
        let mut params = Vec::new();
        loop {
            p.ws();
            if p.eat(b')') {
                break;
            }
            params.push(p.param()?);
            p.ws();
            if p.eat(b',') {
                continue;
            }
            p.ws();
            if p.eat(b')') {
                break;
            }
            return Err(format!("unexpected character at {} in {sig:?}", p.i));
        }
        p.ws();
        let return_type = if p.eat(b':') {
            p.ws();
            let t = p.type_text();
            Some(TypeDecl::parse(&t)?)
        } else {
            None
        };
        let mut params = params;
        // `/** @prefer-ref $name */` annotations (php-src stubs).
        let mut rest = sig;
        while let Some(i) = rest.find("@prefer-ref") {
            rest = rest[i + 11..].trim_start();
            if let Some(n) = rest.strip_prefix('$') {
                let end = n
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(n.len());
                if let Some(p) = params
                    .iter_mut()
                    .find(|p| p.name.as_bytes() == &n.as_bytes()[..end])
                {
                    p.prefer_ref = true;
                }
            }
        }
        Ok(Signature::new(&name, params, return_type, returns_ref))
    }
}

struct SigParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl SigParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn starts_with(&self, t: &str) -> bool {
        self.s[self.i..].starts_with(t.as_bytes())
    }

    fn ws(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.starts_with("/*") {
                match self.s[self.i + 2..].windows(2).position(|w| w == b"*/") {
                    Some(n) => self.i += n + 4,
                    None => self.i = self.s.len(),
                }
                continue;
            }
            if self.starts_with("#[") {
                let mut depth = 0;
                while let Some(c) = self.peek() {
                    self.i += 1;
                    match c {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                continue;
            }
            break;
        }
    }

    fn keyword(&mut self, kw: &str) {
        if self.starts_with(kw)
            && self
                .s
                .get(self.i + kw.len())
                .is_some_and(|c| c.is_ascii_whitespace())
        {
            self.i += kw.len();
        }
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80)
        {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    /// Reads a type up to the parameter's `&`, `...` or `$`.
    fn type_text(&mut self) -> String {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == b'$' || c == b',' || c == b')' || c == b'{' || c == b';' || c == b'=' {
                break;
            }
            if c == b'&' {
                let mut j = self.i + 1;
                while self.s.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
                    j += 1;
                }
                if self.s.get(j) == Some(&b'$') || self.s[j..].starts_with(b"...") {
                    break;
                }
            }
            if self.starts_with("...") {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i])
            .trim()
            .to_string()
    }

    fn param(&mut self) -> Result<Param, String> {
        self.ws();
        let ty_text =
            if self.peek() == Some(b'$') || self.peek() == Some(b'&') || self.starts_with("...") {
                String::new()
            } else {
                self.type_text()
            };
        self.ws();
        let by_ref = self.eat(b'&');
        self.ws();
        let variadic = if self.starts_with("...") {
            self.i += 3;
            true
        } else {
            false
        };
        self.ws();
        if !self.eat(b'$') {
            return Err(format!("expected parameter name at {}", self.i));
        }
        let name = self.ident().ok_or("missing parameter name")?;
        self.ws();
        let default = if self.eat(b'=') {
            self.ws();
            let text = self.default_text();
            Some(parse_default(text.trim())?)
        } else {
            None
        };
        let ty = if ty_text.is_empty() {
            None
        } else {
            Some(TypeDecl::parse(&ty_text)?)
        };
        Ok(Param {
            name: PhpStr::from(name),
            ty,
            by_ref,
            variadic,
            default,
            prefer_ref: false,
            cached_default: OnceCell::new(),
        })
    }

    fn default_text(&mut self) -> String {
        let start = self.i;
        let mut depth = 0i32;
        while let Some(c) = self.peek() {
            match c {
                b'"' | b'\'' => {
                    self.i += 1;
                    while let Some(d) = self.peek() {
                        self.i += 1;
                        if d == b'\\' {
                            self.i += 1;
                        } else if d == c {
                            break;
                        }
                    }
                    continue;
                }
                b'(' | b'[' => depth += 1,
                b')' | b']' if depth > 0 => depth -= 1,
                b',' | b')' if depth == 0 => break,
                _ => {}
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }
}

/// Parses a stub default value.
fn parse_default(text: &str) -> Result<DefaultValue, String> {
    if text == "UNKNOWN" {
        return Ok(DefaultValue::Unknown);
    }
    let mut p = ExprParser {
        s: text.as_bytes(),
        i: 0,
    };
    let e = p.expr(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("unsupported default value {text:?}"));
    }
    Ok(DefaultValue::Expr(e))
}

struct ExprParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl ExprParser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn peek_op(&mut self) -> Option<(BinOp, u8, usize)> {
        self.ws();
        let rest = &self.s[self.i..];
        if rest.starts_with(b"<<") {
            return Some((BinOp::Shl, 3, 2));
        }
        match rest.first()? {
            b'|' => Some((BinOp::Or, 1, 1)),
            b'&' => Some((BinOp::And, 2, 1)),
            b'+' => Some((BinOp::Add, 4, 1)),
            b'-' => Some((BinOp::Sub, 4, 1)),
            b'*' => Some((BinOp::Mul, 5, 1)),
            _ => None,
        }
    }

    fn expr(&mut self, min_prec: u8) -> Result<ConstExpr, String> {
        let mut lhs = self.unary()?;
        while let Some((op, prec, len)) = self.peek_op() {
            if prec < min_prec {
                break;
            }
            self.i += len;
            let rhs = self.expr(prec + 1)?;
            lhs = ConstExpr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<ConstExpr, String> {
        self.ws();
        match self.s.get(self.i) {
            Some(b'-') => {
                self.i += 1;
                let e = self.unary()?;
                Ok(match e {
                    ConstExpr::Literal(Value::Int(i)) => {
                        ConstExpr::Literal(Value::Int(i.wrapping_neg()))
                    }
                    ConstExpr::Literal(Value::Float(d)) => ConstExpr::Literal(Value::Float(-d)),
                    e => ConstExpr::Neg(Box::new(e)),
                })
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            Some(b'(') => {
                self.i += 1;
                let e = self.expr(0)?;
                self.ws();
                if self.s.get(self.i) != Some(&b')') {
                    return Err("expected ')'".into());
                }
                self.i += 1;
                Ok(e)
            }
            Some(b'[') => {
                self.i += 1;
                self.ws();
                if self.s.get(self.i) != Some(&b']') {
                    return Err("only [] array defaults are supported".into());
                }
                self.i += 1;
                Ok(ConstExpr::EmptyArray)
            }
            Some(b'"') | Some(b'\'') => self.string(),
            Some(c) if c.is_ascii_digit() || *c == b'.' => self.number(),
            Some(_) => self.name(),
            None => Err("empty default".into()),
        }
    }

    fn number(&mut self) -> Result<ConstExpr, String> {
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'_')
        {
            if (self.s[self.i] == b'e' || self.s[self.i] == b'E')
                && matches!(self.s.get(self.i + 1), Some(b'+' | b'-'))
            {
                self.i += 1;
            }
            self.i += 1;
        }
        let t: String = String::from_utf8_lossy(&self.s[start..self.i]).replace('_', "");
        let lt = t.to_ascii_lowercase();
        let v = if let Some(h) = lt.strip_prefix("0x") {
            i64::from_str_radix(h, 16)
                .map(Value::Int)
                .map_err(|e| e.to_string())?
        } else if let Some(b) = lt.strip_prefix("0b") {
            i64::from_str_radix(b, 2)
                .map(Value::Int)
                .map_err(|e| e.to_string())?
        } else if let Some(o) = lt.strip_prefix("0o") {
            i64::from_str_radix(o, 8)
                .map(Value::Int)
                .map_err(|e| e.to_string())?
        } else if lt.contains(['.', 'e']) {
            Value::Float(lt.parse::<f64>().map_err(|e| e.to_string())?)
        } else if lt.len() > 1 && lt.starts_with('0') {
            i64::from_str_radix(&lt[1..], 8)
                .map(Value::Int)
                .map_err(|e| e.to_string())?
        } else {
            match lt.parse::<i64>() {
                Ok(i) => Value::Int(i),
                Err(_) => Value::Float(lt.parse::<f64>().map_err(|e| e.to_string())?),
            }
        };
        Ok(ConstExpr::Literal(v))
    }

    fn string(&mut self) -> Result<ConstExpr, String> {
        let q = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.i) else {
                return Err("unterminated string".into());
            };
            self.i += 1;
            if c == q {
                break;
            }
            if c != b'\\' {
                out.push(c);
                continue;
            }
            let Some(&e) = self.s.get(self.i) else {
                return Err("bad escape".into());
            };
            self.i += 1;
            if q == b'\'' {
                match e {
                    b'\\' | b'\'' => out.push(e),
                    _ => {
                        out.push(b'\\');
                        out.push(e);
                    }
                }
                continue;
            }
            match e {
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'v' => out.push(0x0b),
                b'f' => out.push(0x0c),
                b'e' => out.push(0x1b),
                b'\\' => out.push(b'\\'),
                b'$' => out.push(b'$'),
                b'"' => out.push(b'"'),
                b'x' => {
                    let start = self.i;
                    while self.i < start + 2
                        && self.s.get(self.i).is_some_and(|c| c.is_ascii_hexdigit())
                    {
                        self.i += 1;
                    }
                    let h = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("0");
                    out.push(u8::from_str_radix(h, 16).unwrap_or(0));
                }
                b'0'..=b'7' => {
                    let start = self.i - 1;
                    while self.i < start + 3
                        && self
                            .s
                            .get(self.i)
                            .is_some_and(|c| (b'0'..=b'7').contains(c))
                    {
                        self.i += 1;
                    }
                    let o = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("0");
                    out.push(u32::from_str_radix(o, 8).unwrap_or(0) as u8);
                }
                _ => {
                    out.push(b'\\');
                    out.push(e);
                }
            }
        }
        Ok(ConstExpr::Literal(Value::String(PhpStr::from_vec(out))))
    }

    fn name(&mut self) -> Result<ConstExpr, String> {
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'\\')
        {
            self.i += 1;
        }
        if self.i == start {
            return Err(format!("unexpected {:?}", self.s[self.i] as char));
        }
        let name = String::from_utf8_lossy(&self.s[start..self.i])
            .trim_start_matches('\\')
            .to_string();
        if self.s[self.i..].starts_with(b"::") {
            self.i += 2;
            let mstart = self.i;
            while self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                self.i += 1;
            }
            let member = String::from_utf8_lossy(&self.s[mstart..self.i]).into_owned();
            if member.eq_ignore_ascii_case("class") {
                return Ok(ConstExpr::ClassName(name));
            }
            return Ok(ConstExpr::ClassConst(name, member));
        }
        Ok(match name.to_ascii_lowercase().as_str() {
            "null" => ConstExpr::Literal(Value::Null),
            "true" => ConstExpr::Literal(Value::Bool(true)),
            "false" => ConstExpr::Literal(Value::Bool(false)),
            _ => ConstExpr::Const(name),
        })
    }
}

impl Vm {
    /// Evaluates a stub default expression (constants via the constant table).
    pub fn eval_const_expr(&mut self, e: &ConstExpr) -> PResult<Value> {
        Ok(match e {
            ConstExpr::Literal(v) => v.clone(),
            ConstExpr::EmptyArray => Value::empty_array(),
            ConstExpr::Const(name) => match self.constant(name.as_bytes()) {
                Some(v) => v,
                None => {
                    return Err(self.throw_error("Error", format!("Undefined constant \"{name}\"")));
                }
            },
            ConstExpr::ClassName(c) => match self.find_class(c.as_bytes()) {
                Some(ce) => Value::String(ce.name.clone()),
                None => Value::String(PhpStr::from(c.as_str())),
            },
            ConstExpr::ClassConst(c, n) => match self.class_constant(c.as_bytes(), n.as_bytes())? {
                Some(v) => v,
                None => {
                    return Err(self.throw_error("Error", format!("Undefined constant {c}::{n}")));
                }
            },
            ConstExpr::Neg(inner) => {
                let v = self.eval_const_expr(inner)?;
                self.mul(&v, &Value::Int(-1))?
            }
            ConstExpr::Binary(op, a, b) => {
                let a = self.eval_const_expr(a)?;
                let b = self.eval_const_expr(b)?;
                match op {
                    BinOp::Or => self.bitwise_or(&a, &b)?,
                    BinOp::And => self.bitwise_and(&a, &b)?,
                    BinOp::Add => self.add(&a, &b)?,
                    BinOp::Sub => self.sub(&a, &b)?,
                    BinOp::Mul => self.mul(&a, &b)?,
                    BinOp::Shl => self.shift_left(&a, &b)?,
                }
            }
        })
    }

    fn param_default(&mut self, p: &Param) -> PResult<Option<Value>> {
        if let Some(v) = p.cached_default.get() {
            return Ok(Some(v.clone()));
        }
        match &p.default {
            Some(DefaultValue::Expr(e)) => {
                let v = self.eval_const_expr(e)?;
                if !matches!(v, Value::Object(_)) {
                    let _ = p.cached_default.set(v.clone());
                }
                Ok(Some(v))
            }
            _ => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

/// Arguments of a native call after ZPP.
pub struct Args {
    vals: Vec<Value>,
    passed: Vec<bool>,
    num_passed: usize,
    num_slots: usize,
    rest_start: usize,
    named_rest: Vec<(PhpStr, Value)>,
    this: Option<ObjRef>,
    scope: Option<ClassRef>,
    func: Rc<Function>,
    undef: Value,
}

impl Args {
    /// `ZEND_NUM_ARGS()`: number of arguments passed (positional + named slots).
    pub fn len(&self) -> usize {
        self.num_passed
    }

    pub fn is_empty(&self) -> bool {
        self.num_passed == 0
    }

    /// Whether argument `i` (0-based) was passed explicitly.
    pub fn is_passed(&self, i: usize) -> bool {
        self.passed.get(i).copied().unwrap_or(false)
    }

    /// Argument `i` (default-filled); `Undef` if absent (e.g. `= UNKNOWN`).
    pub fn get(&self, i: usize) -> &Value {
        self.vals.get(i).unwrap_or(&self.undef)
    }

    /// Takes ownership of argument `i`, leaving `Undef`.
    pub fn take(&mut self, i: usize) -> Value {
        self.vals.get_mut(i).map(std::mem::take).unwrap_or_default()
    }

    /// Like [`Args::get`] but `None` for absent or `null`.
    pub fn opt(&self, i: usize) -> Option<&Value> {
        match self.vals.get(i) {
            None | Some(Value::Undef) | Some(Value::Null) => None,
            Some(v) => Some(v),
        }
    }

    pub fn is_null(&self, i: usize) -> bool {
        self.opt(i).is_none()
    }

    #[track_caller]
    fn mismatch(&self, i: usize, want: &str) -> ! {
        panic!(
            "{}(): argument {} is not {want} after ZPP (got {:?}); check the stub signature",
            self.func.display_name(),
            i,
            self.get(i)
        )
    }

    /// A `string` parameter.
    #[track_caller]
    pub fn str(&self, i: usize) -> &PhpStr {
        match self.get(i) {
            Value::String(s) => s,
            _ => self.mismatch(i, "a string"),
        }
    }

    /// A `?string` parameter.
    #[track_caller]
    pub fn str_opt(&self, i: usize) -> Option<&PhpStr> {
        match self.get(i) {
            Value::String(s) => Some(s),
            Value::Null | Value::Undef => None,
            _ => self.mismatch(i, "a string or null"),
        }
    }

    /// An `int` parameter.
    #[track_caller]
    pub fn int(&self, i: usize) -> i64 {
        match self.get(i) {
            Value::Int(n) => *n,
            Value::Bool(b) => *b as i64,
            _ => self.mismatch(i, "an int"),
        }
    }

    #[track_caller]
    pub fn int_opt(&self, i: usize) -> Option<i64> {
        match self.get(i) {
            Value::Int(n) => Some(*n),
            Value::Null | Value::Undef => None,
            _ => self.mismatch(i, "an int or null"),
        }
    }

    /// A `float` parameter (ints are accepted).
    #[track_caller]
    pub fn float(&self, i: usize) -> f64 {
        match self.get(i) {
            Value::Float(d) => *d,
            Value::Int(n) => *n as f64,
            _ => self.mismatch(i, "a float"),
        }
    }

    #[track_caller]
    pub fn float_opt(&self, i: usize) -> Option<f64> {
        match self.get(i) {
            Value::Float(d) => Some(*d),
            Value::Int(n) => Some(*n as f64),
            Value::Null | Value::Undef => None,
            _ => self.mismatch(i, "a float or null"),
        }
    }

    /// A `bool` parameter.
    #[track_caller]
    pub fn bool(&self, i: usize) -> bool {
        match self.get(i) {
            Value::Bool(b) => *b,
            _ => self.mismatch(i, "a bool"),
        }
    }

    #[track_caller]
    pub fn bool_opt(&self, i: usize) -> Option<bool> {
        match self.get(i) {
            Value::Bool(b) => Some(*b),
            Value::Null | Value::Undef => None,
            _ => self.mismatch(i, "a bool or null"),
        }
    }

    /// An `array` parameter.
    #[track_caller]
    pub fn array(&self, i: usize) -> &PhpArrayRef {
        match self.get(i) {
            Value::Array(a) => a,
            _ => self.mismatch(i, "an array"),
        }
    }

    #[track_caller]
    pub fn array_opt(&self, i: usize) -> Option<&PhpArrayRef> {
        match self.get(i) {
            Value::Array(a) => Some(a),
            Value::Null | Value::Undef => None,
            _ => self.mismatch(i, "an array or null"),
        }
    }

    /// An object (or class-typed) parameter.
    #[track_caller]
    pub fn object(&self, i: usize) -> &ObjRef {
        match self.get(i) {
            Value::Object(o) => o,
            _ => self.mismatch(i, "an object"),
        }
    }

    #[track_caller]
    pub fn object_opt(&self, i: usize) -> Option<&ObjRef> {
        match self.get(i) {
            Value::Object(o) => Some(o),
            Value::Null | Value::Undef => None,
            _ => self.mismatch(i, "an object or null"),
        }
    }

    /// The reference passed to a by-reference parameter (`&$param`).
    #[track_caller]
    pub fn by_ref(&self, i: usize) -> PhpRef {
        match self.get(i) {
            Value::Ref(r) => r.clone(),
            _ => self.mismatch(i, "a reference"),
        }
    }

    /// Variadic arguments starting at parameter index `i` (normally the
    /// variadic parameter's index).
    pub fn rest(&self, i: usize) -> &[Value] {
        let i = i.max(self.rest_start).min(self.vals.len());
        &self.vals[i..]
    }

    /// Extra named arguments collected by a variadic parameter.
    pub fn named_rest(&self) -> &[(PhpStr, Value)] {
        &self.named_rest
    }

    /// `$this` for native methods.
    pub fn this(&self) -> Option<&ObjRef> {
        self.this.as_ref()
    }

    /// Called scope (`static::class`) for native static methods.
    pub fn scope(&self) -> Option<&ClassRef> {
        self.scope.as_ref()
    }

    /// The function being called.
    pub fn function(&self) -> &Rc<Function> {
        &self.func
    }

    /// All argument slots.
    pub fn values(&self) -> &[Value] {
        &self.vals
    }

    /// The passed arguments as shown in backtraces (after coercion).
    pub fn trace_args(&self) -> Vec<(Option<PhpStr>, Value)> {
        let mut out: Vec<(Option<PhpStr>, Value)> = self.vals[..self.num_slots]
            .iter()
            .chain(self.vals[self.rest_start..].iter())
            .map(|v| (None, v.unref()))
            .collect();
        out.extend(
            self.named_rest
                .iter()
                .map(|(n, v)| (Some(n.clone()), v.unref())),
        );
        out
    }
}

/// Arguments as supplied by a caller (before ZPP).
#[derive(Default, Clone)]
pub struct CallArgs {
    pub positional: Vec<Value>,
    pub named: Vec<(PhpStr, Value)>,
    pub this: Option<ObjRef>,
    pub scope: Option<ClassRef>,
    /// `strict_types` of the calling code.
    pub strict: bool,
    /// A dynamic call (`zend_call_function`: callbacks, `call_user_func`):
    /// by-value arguments for by-reference parameters emit
    /// "must be passed by reference, value given".
    pub dynamic: bool,
}

/// Arguments placed into parameter slots, before ZPP.
pub struct PlacedArgs {
    slots: Vec<Option<Value>>,
    extra: Vec<Value>,
    named_rest: Vec<(PhpStr, Value)>,
    this: Option<ObjRef>,
    scope: Option<ClassRef>,
    strict: bool,
}

impl PlacedArgs {
    /// Arguments as they appear in a backtrace (slot order; extra named
    /// arguments carry their names).
    pub fn trace_args(&self) -> Vec<(Option<PhpStr>, Value)> {
        let mut out: Vec<(Option<PhpStr>, Value)> = self
            .slots
            .iter()
            .flatten()
            .chain(self.extra.iter())
            .map(|v| (None, v.unref()))
            .collect();
        out.extend(
            self.named_rest
                .iter()
                .map(|(n, v)| (Some(n.clone()), v.unref())),
        );
        out
    }
}

impl CallArgs {
    pub fn new(positional: Vec<Value>) -> CallArgs {
        CallArgs {
            positional,
            ..CallArgs::default()
        }
    }
}

fn arg_label(sig: &Signature, i: usize) -> String {
    match sig.params.get(i) {
        Some(p) if !p.variadic => format!("#{} (${})", i + 1, p.name),
        _ => format!("#{}", i + 1),
    }
}

impl Vm {
    /// Throws `class` with message `"fname(): Argument #N ($name) msg"` for the
    /// active native function (`zend_argument_error`).
    pub fn argument_error(&mut self, class: &str, arg_num: usize, msg: &str) -> crate::vm::Flow {
        let (fname, label) = match self.frames.last().and_then(|f| f.function.clone()) {
            Some(func) => {
                let sig = func.signature().clone();
                (
                    func.display_name(),
                    arg_label(&sig, arg_num.saturating_sub(1)),
                )
            }
            None => ("main".to_string(), format!("#{arg_num}")),
        };
        self.throw_error(class, format!("{fname}(): Argument {label} {msg}"))
    }

    /// `zend_argument_type_error`.
    pub fn argument_type_error(&mut self, arg_num: usize, msg: &str) -> crate::vm::Flow {
        self.argument_error("TypeError", arg_num, msg)
    }

    /// `zend_argument_value_error`.
    pub fn argument_value_error(&mut self, arg_num: usize, msg: &str) -> crate::vm::Flow {
        self.argument_error("ValueError", arg_num, msg)
    }

    /// `zend_argument_count_error`-style ArgumentCountError for the active function.
    pub fn argument_count_error(&mut self, msg: impl Into<PhpStr>) -> crate::vm::Flow {
        self.throw_error("ArgumentCountError", msg)
    }

    fn zpp_type_error(
        &mut self,
        fname: &str,
        sig: &Signature,
        i: usize,
        p: &Param,
        v: &Value,
    ) -> crate::vm::Flow {
        let msg = format!(
            "{fname}(): Argument {} must be of type {}, {} given",
            arg_label(sig, i),
            p.type_string(),
            v.value_name()
        );
        self.throw_error("TypeError", msg)
    }

    /// Coerces one argument according to its parameter (ZPP semantics).
    fn zpp_param(
        &mut self,
        fname: &str,
        sig: &Signature,
        i: usize,
        p: &Param,
        v: Value,
        strict: bool,
    ) -> PResult<Value> {
        let Some(ty) = &p.ty else {
            return Ok(v);
        };
        if ty.is_mixed() {
            return Ok(v);
        }
        if ty.matches_exact(&v) {
            return Ok(v);
        }
        if ty.mask & mask::CALLABLE != 0 {
            let host = self.host.clone();
            match host.check_callable(self, &v) {
                Ok(_) => return Ok(v),
                Err(err) => {
                    if ty.mask & !(mask::CALLABLE | mask::NULL) == 0 && ty.classes.is_empty() {
                        let or_null = if ty.allows_null() { " or null" } else { "" };
                        let msg = format!(
                            "{fname}(): Argument {} must be a valid callback{or_null}, {err}",
                            arg_label(sig, i)
                        );
                        return Err(self.throw_error("TypeError", msg));
                    }
                }
            }
        }
        if v.is_null() {
            if !strict && ty.mask & mask::SCALAR != 0 {
                let mut msg = format!("{fname}(): Passing null to parameter #{}", i + 1);
                if let Some(p) = sig.params.get(i).filter(|p| !p.variadic) {
                    msg.push_str(&format!(" (${})", p.name));
                }
                msg.push_str(&format!(" of type {} is deprecated", ty.to_type_string()));
                self.engine_deprecated(msg)?;
                let m = ty.mask;
                return Ok(if m & mask::INT != 0 {
                    Value::Int(0)
                } else if m & mask::FLOAT != 0 {
                    Value::Float(0.0)
                } else if m & mask::STRING != 0 {
                    Value::empty_string()
                } else {
                    Value::Bool(false)
                });
            }
            return Err(self.zpp_type_error(fname, sig, i, p, &v));
        }
        let mode = CoerceMode {
            strict,
            silent: false,
        };
        match self.coerce_to_type(ty, v.clone(), mode)? {
            Some(c) => Ok(c),
            None => Err(self.zpp_type_error(fname, sig, i, p, &v)),
        }
    }

    /// `zend_param_must_be_ref`: a by-value argument for a by-reference
    /// parameter in a dynamic call (`call_user_func` & co.).
    fn param_must_be_ref(&mut self, func: &Function, sig: &Signature, i: usize) -> PResult<()> {
        let mut msg = format!("{}(): Argument #{}", func.display_name(), i + 1);
        if let Some(p) = sig.params.get(i).filter(|p| !p.variadic) {
            msg.push_str(&format!(" (${})", p.name));
        }
        msg.push_str(" must be passed by reference, value given");
        self.engine_warning(msg)
    }

    /// Places positional and named arguments into parameter slots
    /// (`zend_handle_named_arg`). Runs in the caller's frame, so errors such as
    /// "Unknown named parameter" do not include the callee in the trace.
    pub fn place_native_args(
        &mut self,
        func: &Rc<Function>,
        call: CallArgs,
    ) -> PResult<PlacedArgs> {
        let sig = func.signature().clone();
        let num_decl = sig.num_args();
        let variadic = sig.variadic();
        let CallArgs {
            positional,
            named,
            this,
            scope,
            strict,
            dynamic,
        } = call;
        let needs_ref = |i: usize| {
            let p = if i < num_decl {
                sig.params.get(i)
            } else {
                variadic
            };
            p.is_some_and(|p| p.by_ref && !p.prefer_ref)
        };

        let mut slots: Vec<Option<Value>> = Vec::with_capacity(positional.len().max(num_decl));
        let mut extra: Vec<Value> = Vec::new();
        for (i, v) in positional.into_iter().enumerate() {
            if dynamic && needs_ref(i) && !v.is_ref() {
                self.param_must_be_ref(func, &sig, i)?;
            }
            if i < num_decl {
                slots.push(Some(v));
            } else {
                extra.push(v);
            }
        }
        let mut named_rest: Vec<(PhpStr, Value)> = Vec::new();
        for (name, v) in named {
            match sig.param_index(&name) {
                Some(idx) => {
                    if slots.get(idx).is_some_and(Option::is_some) {
                        return Err(self.throw_error(
                            "Error",
                            format!("Named parameter ${name} overwrites previous argument"),
                        ));
                    }
                    if dynamic && needs_ref(idx) && !v.is_ref() {
                        self.param_must_be_ref(func, &sig, idx)?;
                    }
                    if slots.len() <= idx {
                        slots.resize(idx + 1, None);
                    }
                    slots[idx] = Some(v);
                }
                None if variadic.is_some() => {
                    if named_rest.iter().any(|(n, _)| *n == name) {
                        return Err(self.throw_error(
                            "Error",
                            format!("Named parameter ${name} overwrites previous argument"),
                        ));
                    }
                    named_rest.push((name, v));
                }
                None => {
                    return Err(
                        self.throw_error("Error", format!("Unknown named parameter ${name}"))
                    );
                }
            }
        }
        Ok(PlacedArgs {
            slots,
            extra,
            named_rest,
            this,
            scope,
            strict,
        })
    }

    /// Applies `zend_parse_parameters` semantics to placed arguments: missing
    /// arguments (`zend_handle_undef_args`), arity, defaults, coercion. The
    /// function's frame must already be pushed (errors name the function).
    pub fn finish_native_args(&mut self, func: &Rc<Function>, placed: PlacedArgs) -> PResult<Args> {
        let sig = func.signature().clone();
        let fname = func.display_name();
        let num_decl = sig.num_args();
        let variadic = sig.variadic();
        let PlacedArgs {
            mut slots,
            extra,
            mut named_rest,
            this,
            scope,
            strict,
        } = placed;

        let num_slots = slots.len();
        let num_passed = num_slots + extra.len();

        // Undefined slots left by named arguments (`zend_handle_undef_args`).
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            let p = &sig.params[i];
            if i < sig.required {
                return Err(self.throw_error(
                    "ArgumentCountError",
                    format!("{fname}(): Argument {} not passed", arg_label(&sig, i)),
                ));
            }
            match &p.default {
                Some(DefaultValue::Expr(_)) => {
                    let v = self.param_default(p)?.unwrap_or(Value::Null);
                    *slot = Some(v);
                }
                _ => {
                    return Err(self.throw_error(
                        "ArgumentCountError",
                        format!(
                            "{fname}(): Argument {} must be passed explicitly, because the default value is not known",
                            arg_label(&sig, i)
                        ),
                    ));
                }
            }
        }

        // Arity (`zend_wrong_parameters_count_error`).
        let min = sig.required;
        let max = if variadic.is_some() {
            usize::MAX
        } else {
            num_decl
        };
        if num_passed < min || num_passed > max {
            let (word, n) = if min == max {
                ("exactly", min)
            } else if num_passed < min {
                ("at least", min)
            } else {
                ("at most", max)
            };
            let msg = format!(
                "{fname}() expects {word} {n} argument{}, {num_passed} given",
                if n == 1 { "" } else { "s" }
            );
            return Err(self.throw_error("ArgumentCountError", msg));
        }

        let mut vals = Vec::with_capacity(num_decl.max(num_slots) + extra.len());
        let mut passed = Vec::with_capacity(vals.capacity());
        for (i, slot) in slots.into_iter().enumerate() {
            let v = slot.expect("filled above");
            let p = &sig.params[i];
            let v = self.zpp_arg(&fname, &sig, i, p, v, strict)?;
            vals.push(v);
            passed.push(true);
        }
        // Defaults for unpassed trailing parameters.
        for i in num_slots..num_decl {
            let p = &sig.params[i];
            let v = match &p.default {
                Some(DefaultValue::Expr(_)) => self.param_default(p)?.unwrap_or(Value::Null),
                _ => Value::Undef,
            };
            let v = if p.by_ref && !v.is_undef() {
                Value::Ref(PhpRef::new(v))
            } else {
                v
            };
            vals.push(v);
            passed.push(false);
        }
        // `Z_PARAM_VARIADIC` rejects extra named arguments unless the
        // function collects them (`ZPP_ERROR_UNEXPECTED_EXTRA_NAMED`).
        if !named_rest.is_empty() && !func.has_flag(fn_flags::NAMED_VARIADIC) {
            return Err(self.throw_error(
                "ArgumentCountError",
                format!("Internal function {fname}() does not accept named variadic arguments"),
            ));
        }
        let rest_start = vals.len();
        if let Some(vp) = variadic {
            for (j, v) in extra.into_iter().enumerate() {
                let idx = num_decl + j;
                let v = self.zpp_arg(&fname, &sig, idx, vp, v, strict)?;
                vals.push(v);
                passed.push(true);
            }
            let mut converted = Vec::with_capacity(named_rest.len());
            for (name, v) in named_rest {
                let v = self.zpp_arg(&fname, &sig, num_decl, vp, v, strict)?;
                converted.push((name, v));
            }
            named_rest = converted;
        }

        Ok(Args {
            vals,
            passed,
            num_passed,
            num_slots,
            rest_start,
            named_rest,
            this,
            scope,
            func: func.clone(),
            undef: Value::Undef,
        })
    }

    /// [`Vm::place_native_args`] followed by [`Vm::finish_native_args`]
    /// (the function's frame must already be pushed).
    pub fn prepare_native_args(&mut self, func: &Rc<Function>, call: CallArgs) -> PResult<Args> {
        let placed = self.place_native_args(func, call)?;
        self.finish_native_args(func, placed)
    }

    fn zpp_arg(
        &mut self,
        fname: &str,
        sig: &Signature,
        i: usize,
        p: &Param,
        v: Value,
        strict: bool,
    ) -> PResult<Value> {
        if p.by_ref {
            let r = match v {
                Value::Ref(r) => r,
                other => PhpRef::new(other),
            };
            if let Some(ty) = &p.ty {
                if !ty.is_mixed() {
                    let cur = r.get();
                    let coerced = self.zpp_param(fname, sig, i, p, cur, strict)?;
                    if !Value::identical(&coerced, &r.get()) {
                        r.set(coerced);
                    }
                }
            }
            return Ok(Value::Ref(r));
        }
        self.zpp_param(fname, sig, i, p, v.into_unref(), strict)
    }

    /// Calls a native function `func` (`zend_call_function` for internal functions).
    pub fn invoke_native(&mut self, func: &Rc<Function>, call: CallArgs) -> PResult<Value> {
        let FunctionKind::Native(nf) = &func.kind else {
            let host = self.host.clone();
            return host.call_user_function(self, func, call);
        };
        let nf: &'static NativeFunction = nf;
        let class = func
            .scope
            .clone()
            .or_else(|| call.this.as_ref().map(|o| o.class().name.clone()));
        let call_type = match (&call.this, &func.scope) {
            (Some(_), _) => Some("->"),
            (None, Some(_)) => Some("::"),
            _ => None,
        };
        let this = call.this.clone();
        let call_site = if self.in_native_frame() {
            None
        } else {
            Some((self.current_file.clone(), self.current_line))
        };
        let placed = self.place_native_args(func, call)?;
        let frame = Frame {
            function: Some(func.clone()),
            name: func.name.clone(),
            class,
            call_type,
            this,
            call_site,
            args: placed.trace_args(),
            is_internal: true,
        };
        self.push_frame(frame);
        let result = self.finish_native_args(func, placed).and_then(|mut args| {
            if let Some(f) = self.frames.last_mut() {
                f.args = args.trace_args();
            }
            (nf.handler)(self, &mut args)
        });
        self.pop_frame();
        result
    }

    /// Calls a registered function by name with positional arguments
    /// (weak mode), e.g. `vm.call_native("strlen", vec!["abc".into()])`.
    pub fn call_native(&mut self, name: &str, args: Vec<Value>) -> PResult<Value> {
        match self.find_function(name.as_bytes()) {
            Some(f) => self.invoke_native(&f, CallArgs::new(args)),
            None => Err(self.throw_error("Error", format!("Call to undefined function {name}()"))),
        }
    }

    /// Like [`Vm::call_native`] with named arguments.
    pub fn call_native_named(
        &mut self,
        name: &str,
        args: Vec<Value>,
        named: Vec<(&str, Value)>,
    ) -> PResult<Value> {
        let Some(f) = self.find_function(name.as_bytes()) else {
            return Err(self.throw_error("Error", format!("Call to undefined function {name}()")));
        };
        let call = CallArgs {
            positional: args,
            named: named
                .into_iter()
                .map(|(n, v)| (PhpStr::from(n), v))
                .collect(),
            ..CallArgs::default()
        };
        self.invoke_native(&f, call)
    }
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

/// Module registration context: `pub fn register(r: &mut Registry)`.
pub struct Registry<'vm> {
    pub vm: &'vm mut Vm,
    pub module: &'static str,
}

/// A module's registration entry point.
pub type ModuleRegister = fn(&mut Registry<'_>);

impl<'vm> Registry<'vm> {
    pub fn new(vm: &'vm mut Vm, module: &'static str) -> Registry<'vm> {
        if !vm.modules.contains(&module) {
            vm.modules.push(module);
        }
        Registry { vm, module }
    }

    /// Registers a built-in function (replacing an existing one of the same name).
    pub fn function(&mut self, nf: &'static NativeFunction) -> Rc<Function> {
        self.function_with_flags(nf, 0)
    }

    /// Registers a built-in function with extra `fn_flags` (e.g. `NAMED_VARIADIC`, `DEPRECATED`).
    pub fn function_with_flags(&mut self, nf: &'static NativeFunction, flags: u32) -> Rc<Function> {
        let mut f = Function::native(nf);
        f.flags |= flags;
        let f = Rc::new(f);
        // Surface malformed stub signatures at registration time in debug builds.
        #[cfg(debug_assertions)]
        let _ = f.signature();
        self.vm.functions.insert(lower(&f.name), f.clone());
        f
    }

    pub fn functions(&mut self, fs: &[&'static NativeFunction]) {
        for f in fs {
            self.function(f);
        }
    }

    /// Registers a persistent constant.
    pub fn constant(&mut self, name: &str, value: impl Into<Value>) {
        self.vm.constants.define(Constant {
            name: PhpStr::from(name),
            value: value.into(),
            flags: const_flags::PERSISTENT,
            module: Some(self.module),
            deprecation: None,
        });
    }

    /// Registers a deprecated constant (`#[\Deprecated]`); `message` is the
    /// deprecation suffix (e.g. "since 8.4, the error level was removed").
    pub fn deprecated_constant(
        &mut self,
        name: &str,
        value: impl Into<Value>,
        message: &'static str,
    ) {
        self.vm.constants.define(Constant {
            name: PhpStr::from(name),
            value: value.into(),
            flags: const_flags::PERSISTENT | const_flags::DEPRECATED,
            module: Some(self.module),
            deprecation: Some(message),
        });
    }

    pub fn ini(&mut self, def: IniDef) {
        self.vm.ini.register(&def, self.module);
        self.vm.sync_ini(def.name.as_bytes());
    }

    pub fn ini_entries(&mut self, defs: &[IniDef]) {
        for d in defs {
            self.ini(*d);
        }
    }

    /// Registers an internal class (marked `INTERNAL`, owned by this module).
    pub fn class(&mut self, mut ce: ClassEntry) -> ClassRef {
        ce.flags |= crate::class::class_flags::INTERNAL;
        ce.module = Some(self.module);
        if !ce.methods.is_empty() {
            ce.link_magic_methods();
        }
        let rc = Rc::new(ce);
        self.vm.register_class(rc.clone());
        rc
    }

    /// Finds an already registered class (e.g. a parent).
    pub fn find_class(&self, name: &str) -> Option<ClassRef> {
        self.vm.find_class(name.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_signatures() {
        let s = Signature::parse("function strlen(string $string): int {}").unwrap();
        assert_eq!(s.name, "strlen");
        assert_eq!(s.params.len(), 1);
        assert_eq!(s.required, 1);
        assert_eq!(s.return_type.as_ref().unwrap().to_type_string(), "int");

        let s = Signature::parse(
            "str_pad(string $string, int $length, string $pad_string = \" \", int $pad_type = STR_PAD_RIGHT): string",
        )
        .unwrap();
        assert_eq!(s.name, "str_pad");
        assert_eq!(s.required, 2);
        assert_eq!(s.params.len(), 4);
        assert!(matches!(
            &s.params[2].default,
            Some(DefaultValue::Expr(ConstExpr::Literal(Value::String(x)))) if x == " "
        ));
        assert!(
            matches!(&s.params[3].default, Some(DefaultValue::Expr(ConstExpr::Const(c))) if c == "STR_PAD_RIGHT")
        );
    }

    #[test]
    fn parse_refs_variadics_and_unions() {
        let s =
            Signature::parse("function sort(array &$array, int $flags = SORT_REGULAR): true {}")
                .unwrap();
        assert!(s.params[0].by_ref);
        assert_eq!(s.return_type.unwrap().to_type_string(), "true");

        let s = Signature::parse("function array_push(array &$array, mixed ...$values): int {}")
            .unwrap();
        assert!(s.params[1].variadic);
        assert_eq!(s.num_args(), 1);
        assert_eq!(s.required, 1);

        let s = Signature::parse(
            "function sscanf(string $string, string $format, mixed &...$vars): array|int|null {}",
        )
        .unwrap();
        assert!(s.params[2].by_ref && s.params[2].variadic);
        assert_eq!(s.return_type.unwrap().to_type_string(), "array|int|null");

        let s = Signature::parse("function preg_match(string $pattern, string $subject, &$matches = null, int $flags = 0, int $offset = 0): int|false {}").unwrap();
        assert!(s.params[2].by_ref && s.params[2].ty.is_none());

        let s = Signature::parse("function htmlspecialchars(string $string, int $flags = ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401, ?string $encoding = null, bool $double_encode = true): string {}").unwrap();
        assert!(matches!(
            &s.params[1].default,
            Some(DefaultValue::Expr(ConstExpr::Binary(BinOp::Or, _, _)))
        ));
        assert_eq!(s.params[2].type_string(), "?string");

        let s = Signature::parse(
            "function trim(string $string, string $characters = \" \\n\\r\\t\\v\\0\"): string {}",
        )
        .unwrap();
        assert!(matches!(
            &s.params[1].default,
            Some(DefaultValue::Expr(ConstExpr::Literal(Value::String(x)))) if x.as_bytes() == b" \n\r\t\x0b\0"
        ));

        let s = Signature::parse("function array_keys(array $array, mixed $filter_value = UNKNOWN, bool $strict = false): array {}").unwrap();
        assert!(matches!(s.params[1].default, Some(DefaultValue::Unknown)));

        let s = Signature::parse("public function __construct(#[\\SensitiveParameter] string $message = \"\", int $code = 0, ?Throwable $previous = null) {}").unwrap();
        assert_eq!(s.name, "__construct");
        assert_eq!(s.params[0].name, "message");

        let s = Signature::parse(
            "__construct(string $message = \"\", int $code = 0, ?Throwable $previous = null)",
        )
        .unwrap();
        assert_eq!(s.params[2].type_string(), "?Throwable");
        assert_eq!(s.required, 0);

        let s = Signature::parse("function &getRef(int|string $a = -1, float $f = 1.5, int $m = 2 * 1024 * 1024, array $x = []): static").unwrap();
        assert!(s.returns_ref);
        assert!(matches!(
            &s.params[0].default,
            Some(DefaultValue::Expr(ConstExpr::Literal(Value::Int(-1))))
        ));
        assert_eq!(s.params[0].type_string(), "string|int");
    }

    #[test]
    fn sig_name_extraction() {
        assert_eq!(sig_name("function strlen(string $s): int {}"), "strlen");
        assert_eq!(sig_name("strlen(string $s): int"), "strlen");
        assert_eq!(sig_name("function &foo()"), "foo");
        assert_eq!(sig_name("public static function bar(): int"), "bar");
        assert_eq!(sig_name("/** @prefer-ref $a */ function baz(&$a)"), "baz");
        let s = Signature::parse(
            "/** @prefer-ref $array */ function array_multisort(&$array, &...$rest): bool",
        )
        .unwrap();
        assert!(s.params[0].prefer_ref);
        assert!(!s.params[1].prefer_ref);
    }
}
