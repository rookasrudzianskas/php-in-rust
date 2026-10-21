//! PHP front-end: lexer, parser, AST, name resolution and AST export,
//! ported from php-src (`Zend/zend_language_scanner.l`,
//! `Zend/zend_language_parser.y`, `Zend/zend_ast.c`, name resolution from
//! `Zend/zend_compile.c`).
//!
//! # Usage
//!
//! ```
//! use php_syntax::{parse, ParseOptions};
//!
//! let program = parse(b"<?php echo 1 + 2;", "test.php").unwrap();
//! assert_eq!(program.stmts.len(), 1);
//! ```
//!
//! * [`parse`] / [`parse_with`] turn source bytes into a [`Program`] (with
//!   names resolved, see [`resolve`]) or a [`ParseError`]. A syntax error is
//!   reported like php-src does (`syntax error, unexpected token "}",
//!   expecting ";"`); the runtime prints it as
//!   `PHP Parse error:  {message} in {file} on line {line}`. Errors whose
//!   [`ParseError::kind`] is [`ParseErrorKind::Compile`] (raised by grammar
//!   actions or name resolution, e.g. `Multiple access type modifiers are
//!   not allowed`) are printed as `Fatal error` instead.
//! * [`tokenize`] is `token_get_all()`: every token with its numeric id
//!   ([`TokenKind::id`]), text and line. With [`TokenizeFlags::parse`] it
//!   behaves like `TOKEN_PARSE` (keywords used as identifiers are reported
//!   as `T_STRING`, and invalid code is an error).
//! * [`export_expr`] / [`export_expr_bytes`] are `zend_ast_export()` (used
//!   for `assert()` messages), [`export_stmt`] prints a statement.
//! * [`Program::diagnostics`] (and [`ParseError::diagnostics`]) carry the
//!   deprecations and warnings php-src emits while compiling (non-canonical
//!   casts, octal escape overflow, `use` without effect, ...).
//!
//! The AST lives in [`ast`]; every statement and expression carries the
//! line php-src assigns to the corresponding node, declarations carry
//! start/end lines and doc comments, names carry their resolved form.
//!
//! # Design
//!
//! The lexer is a direct port of the re2c scanner (states, heredoc
//! scan-ahead, nesting checks). The parser is hand-written recursive
//! descent with Pratt parsing for expressions, following the grammar's
//! precedence table. On a syntax error, the token stream is replayed through
//! an LALR(1) automaton generated at first use from php-src's grammar, which
//! reproduces bison's error state, and therefore its exact message and line,
//! as well as its `memory exhausted` stack limit. Deeply nested input is
//! parsed on a thread with a large stack, so parsing never overflows the
//! caller's stack.
//!
//! Not modelled: `zend.multibyte` script encodings (`declare(encoding=...)`
//! only produces php-src's "ignored" warning), and compile-time checks that
//! are not about names (they belong to the compiler in `php-runtime`).

pub mod ast;
pub mod export;
mod lexer;
mod numeric;
mod parser;
pub mod resolve;
pub mod token;

pub use ast::Program;
pub use export::{export_expr, export_expr_bytes, export_stmt};
pub use token::{Token, TokenKind};

use std::fmt;

/// Severity of a compile-time diagnostic emitted while parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticLevel {
    /// `E_DEPRECATED`
    Deprecated,
    /// `E_COMPILE_WARNING`
    CompileWarning,
    /// `E_WARNING` (emitted at compile time, e.g. by `use` statements)
    Warning,
}

/// A non-fatal diagnostic produced while lexing, parsing or resolving names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub line: u32,
}

/// Which exception class php-src would throw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// `ParseError` — printed as `Parse error: ...`.
    Parse,
    /// `CompileError` / `E_COMPILE_ERROR` — printed as `Fatal error: ...`.
    Compile,
}

/// A parse (or early compile) error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub message: String,
    pub line: u32,
    /// Diagnostics emitted before the error occurred (php-src prints them
    /// before the error).
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on line {}", self.message, self.line)
    }
}

impl std::error::Error for ParseError {}

/// Parser options.
#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    /// Start in PHP mode, as `eval()` does (no `<?php` needed).
    pub start_in_scripting: bool,
    /// `short_open_tag=On`: `<?` opens PHP mode.
    pub short_open_tag: bool,
    /// Skip a leading `#!` line (done by the CLI for the main script).
    pub skip_shebang: bool,
    /// Do not run name resolution (names keep `resolved == name`).
    pub no_resolve: bool,
}

/// Parse a PHP file. `filename` is only used for diagnostics.
pub fn parse(source: &[u8], filename: &str) -> Result<Program, ParseError> {
    parse_with(source, filename, &ParseOptions::default())
}

/// Parse with explicit options.
pub fn parse_with(
    source: &[u8],
    _filename: &str,
    opts: &ParseOptions,
) -> Result<Program, ParseError> {
    let resolve_names = !opts.no_resolve;
    let post = move |out: &mut parser::Output| -> Result<(), ParseError> {
        if resolve_names {
            resolve::resolve_stmts(&mut out.stmts, &mut out.diagnostics)?;
        }
        Ok(())
    };
    let out = parser::run(source, opts, false, &post)?;
    Ok(Program {
        stmts: out.stmts,
        halt_offset: out.halt_offset,
        diagnostics: out.diagnostics,
    })
}

/// Whether php-src's bison grammar (replayed with the LALR(1) automaton)
/// accepts the token stream of `source`. Used to cross-check the
/// recursive-descent parser.
#[doc(hidden)]
pub fn grammar_accepts(source: &[u8]) -> bool {
    let lexed = parser::lex_for_parser(source, &ParseOptions::default(), false);
    let p = parser::Parser::new(source, lexed);
    p.lalr_accepts()
}

/// Flags for [`tokenize`].
#[derive(Clone, Copy, Debug, Default)]
pub struct TokenizeFlags {
    /// `TOKEN_PARSE`
    pub parse: bool,
}

/// `token_get_all()`.
///
/// Without `TOKEN_PARSE` this never fails. With it, the source is parsed
/// and a parse error is returned for invalid code.
pub fn tokenize(source: &[u8], flags: TokenizeFlags) -> Result<Vec<Token>, ParseError> {
    if flags.parse {
        let out = parser::run(source, &ParseOptions::default(), true, &|_| Ok(()))?;
        return Ok(out.record.unwrap_or_default());
    }
    let mut lexer = lexer::Lexer::new(source, false);
    let mut out = Vec::new();
    let mut need_tokens: i32 = -1;
    loop {
        let (kind, start, end, line) = lexer.next_raw();
        if kind == TokenKind::End {
            break;
        }
        out.push(Token {
            kind,
            start: start as u32,
            len: (end - start) as u32,
            line,
        });
        if need_tokens != -1 {
            if !matches!(
                kind,
                TokenKind::Whitespace
                    | TokenKind::OpenTag
                    | TokenKind::Comment
                    | TokenKind::DocComment
            ) {
                need_tokens -= 1;
                if need_tokens == 0 {
                    let pos = lexer.position();
                    if pos < source.len() {
                        out.push(Token {
                            kind: TokenKind::InlineHtml,
                            start: pos as u32,
                            len: (source.len() - pos) as u32,
                            line,
                        });
                    }
                    break;
                }
            }
        } else if kind == TokenKind::HaltCompiler {
            need_tokens = 3;
        }
        lexer.apply_increment();
    }
    Ok(out)
}
