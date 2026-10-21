//! Hand-written recursive-descent parser (Pratt parsing for expressions)
//! for the grammar in `Zend/zend_language_parser.y`.
//!
//! The parser accepts exactly the language bison accepts. When it finds a
//! syntax error, the exact bison message (`syntax error, unexpected ...,
//! expecting ...`) and line are recovered by replaying the token stream
//! through an LALR(1) automaton built from the same grammar (see
//! [`lalr`]), so messages match php-src even where a hand-written parser
//! would naturally report a different position.

mod decl;
mod expr;
pub(crate) mod lalr;
mod stmt;

use crate::ast::*;
use crate::lexer::{LexError, LexToken, Lexer, TokVal};
use crate::token::{Token, TokenKind};
use crate::{Diagnostic, ParseError, ParseErrorKind, ParseOptions};

/// Internal failure: either a syntax error at a token index (the message
/// is computed afterwards) or a fully formed error from a semantic action.
#[derive(Debug)]
pub(crate) enum Fail {
    Syntax(usize),
    Error(ParseError),
    /// Recursion limit reached (see [`Parser::max_depth`]).
    TooDeep,
}

pub(crate) type PResult<T> = Result<T, Fail>;

pub(crate) struct Parser<'a> {
    pub(crate) src: &'a [u8],
    pub(crate) toks: Vec<LexToken>,
    pub(crate) pos: usize,
    lex_error: Option<LexError>,
    doc_comments: Vec<(u32, u32)>,
    /// Token index up to which doc-comment events have been applied.
    doc_applied: usize,
    /// Emulation of `CG(doc_comment)` (1-based index, 0 = none).
    cur_doc: u32,
    /// Generator flags of the enclosing function bodies
    /// (`CG(extra_fn_flags)`).
    fn_flags: Vec<bool>,
    pub(crate) diagnostics: Vec<(usize, Diagnostic)>,
    pub(crate) record: Option<Vec<Token>>,
    halt_offset: Option<usize>,
    depth: u32,
    /// Maximum recursion depth before giving up with [`Fail::TooDeep`].
    pub(crate) max_depth: u32,
}

/// Result of lexing a whole file in parser mode.
pub(crate) struct Lexed {
    pub toks: Vec<LexToken>,
    pub lex_error: Option<LexError>,
    pub doc_comments: Vec<(u32, u32)>,
    pub diagnostics: Vec<(usize, Diagnostic)>,
    pub record: Option<Vec<Token>>,
    pub halt_offset: Option<usize>,
}

/// Everything a successful parse produces.
pub(crate) struct Output {
    pub stmts: Vec<Stmt>,
    pub diagnostics: Vec<Diagnostic>,
    pub halt_offset: Option<usize>,
    pub record: Option<Vec<Token>>,
}

/// Nesting depth handled on the caller's stack (worst case about 20 KB per
/// level in unoptimised builds, 4 KB optimised, so this fits a 2 MB thread).
/// Deeper input is first checked against bison's own stack limit, then
/// parsed on a thread with a large stack.
const SHALLOW_DEPTH: u32 = 64;
const DEEP_STACK: usize = 512 << 20;

/// Lex and parse `src`; `post` runs on the resulting statements (on the
/// same thread, so deep trees are processed on the large stack too).
pub(crate) fn run(
    src: &[u8],
    opts: &ParseOptions,
    record: bool,
    post: &(dyn Fn(&mut Output) -> Result<(), ParseError> + Sync),
) -> Result<Output, ParseError> {
    let once = |max_depth: u32| -> Result<Output, Option<ParseError>> {
        let mut p = Parser::new(src, lex_for_parser(src, opts, record));
        p.max_depth = max_depth;
        match p.parse_file() {
            Ok(stmts) => {
                let mut out = Output {
                    stmts,
                    diagnostics: p.diagnostics.drain(..).map(|(_, d)| d).collect(),
                    halt_offset: p.halt_offset(),
                    record: p.record.take(),
                };
                post(&mut out).map_err(Some)?;
                Ok(out)
            }
            Err(Fail::TooDeep) if p.lalr_accepts() => Err(None),
            Err(f) => Err(Some(p.finish_error(f))),
        }
    };
    match once(SHALLOW_DEPTH) {
        Ok(out) => Ok(out),
        Err(Some(e)) => Err(e),
        Err(None) => std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(DEEP_STACK)
                .spawn_scoped(scope, || once(u32::MAX))
                .expect("spawn parser thread")
                .join()
                .expect("parser thread panicked")
        })
        .map_err(|e| e.expect("deep parse cannot report TooDeep")),
    }
}

pub(crate) fn lex_for_parser(src: &[u8], opts: &ParseOptions, record: bool) -> Lexed {
    let mut lexer = Lexer::new(src, true);
    if opts.start_in_scripting {
        lexer.start_in_scripting();
    }
    if opts.short_open_tag {
        lexer.set_short_tags(true);
    }
    if opts.skip_shebang && !opts.start_in_scripting {
        lexer.skip_shebang();
    }
    if record {
        lexer.record = Some(Vec::new());
    }
    let mut toks = Vec::with_capacity(src.len() / 4 + 16);
    let mut diagnostics = Vec::new();
    loop {
        let t = lexer.next_token();
        let kind = t.kind;
        for d in lexer.diagnostics.drain(..) {
            diagnostics.push((toks.len(), d));
        }
        toks.push(t);
        if kind == TokenKind::End || kind == TokenKind::Error {
            break;
        }
    }
    Lexed {
        toks,
        lex_error: lexer.error.take(),
        doc_comments: std::mem::take(&mut lexer.doc_comments),
        diagnostics,
        record: lexer.record.take(),
        halt_offset: lexer.halt_offset,
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn new(src: &'a [u8], lexed: Lexed) -> Self {
        Parser {
            src,
            toks: lexed.toks,
            pos: 0,
            lex_error: lexed.lex_error,
            doc_comments: lexed.doc_comments,
            doc_applied: 0,
            cur_doc: 0,
            fn_flags: vec![false],
            diagnostics: lexed.diagnostics,
            record: lexed.record,
            halt_offset: lexed.halt_offset,
            depth: 0,
            max_depth: u32::MAX,
        }
    }

    /// Enter a recursive production (paired with [`Parser::leave`]).
    #[inline]
    pub(crate) fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > self.max_depth {
            Err(Fail::TooDeep)
        } else {
            Ok(())
        }
    }

    #[inline]
    pub(crate) fn leave(&mut self) {
        self.depth -= 1;
    }

    pub(crate) fn lex_error(&self) -> Option<&LexError> {
        self.lex_error.as_ref()
    }

    // ---------------------------------------------------------------------
    // Token access
    // ---------------------------------------------------------------------

    #[inline]
    pub(crate) fn peek(&self) -> TokenKind {
        self.toks[self.pos].kind
    }

    #[inline]
    pub(crate) fn peek_at(&self, n: usize) -> TokenKind {
        let i = (self.pos + n).min(self.toks.len() - 1);
        self.toks[i].kind
    }

    #[inline]
    pub(crate) fn at(&self, k: TokenKind) -> bool {
        self.peek() == k
    }

    /// Consume the current token, returning its index.
    #[inline]
    pub(crate) fn bump(&mut self) -> usize {
        let i = self.pos;
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        self.sync_doc(self.pos);
        i
    }

    pub(crate) fn eat(&mut self, k: TokenKind) -> bool {
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, k: TokenKind) -> PResult<usize> {
        if self.at(k) {
            Ok(self.bump())
        } else {
            Err(self.syntax_error())
        }
    }

    pub(crate) fn syntax_error(&self) -> Fail {
        Fail::Syntax(self.pos)
    }

    /// Start line of the current token.
    #[inline]
    pub(crate) fn line(&self) -> Line {
        self.toks[self.pos].line
    }

    /// `CG(zend_lineno)` after the last consumed token.
    pub(crate) fn cur_line(&self) -> Line {
        if self.pos == 0 {
            self.toks[0].line
        } else {
            self.toks[self.pos - 1].end_line
        }
    }

    /// `CG(zend_lineno)` after the lookahead token has been scanned.
    pub(crate) fn la_line(&self) -> Line {
        self.toks[self.pos].end_line
    }

    pub(crate) fn tok_line(&self, i: usize) -> Line {
        self.toks[i].line
    }

    /// Raw source text of token `i`.
    pub(crate) fn text(&self, i: usize) -> &'a [u8] {
        let t = &self.toks[i];
        &self.src[t.start as usize..t.end as usize]
    }

    /// The semantic string of a name-like token (variable without `$`,
    /// names without leading `\` / `namespace\`).
    pub(crate) fn name_text(&self, i: usize) -> ByteStr {
        let t = &self.toks[i];
        let (s, e) = (t.start as usize, t.end as usize);
        let off = match t.kind {
            TokenKind::Variable | TokenKind::NameFullyQualified => 1,
            TokenKind::NameRelative => 10,
            _ => 0,
        };
        ByteStr::from(&self.src[s + off..e])
    }

    pub(crate) fn tok_value(&self, i: usize) -> &TokVal {
        &self.toks[i].value
    }

    pub(crate) fn str_value(&self, i: usize) -> ByteStr {
        match &self.toks[i].value {
            TokVal::Str(s) => ByteStr::from(s.as_slice()),
            _ => ByteStr::default(),
        }
    }

    // ---------------------------------------------------------------------
    // Identifiers (`identifier: T_STRING | semi_reserved`)
    // ---------------------------------------------------------------------

    pub(crate) fn is_identifier(k: TokenKind) -> bool {
        k == TokenKind::String || k.is_semi_reserved()
    }

    /// Consume an `identifier`, applying `zend_lex_tstring()`.
    pub(crate) fn identifier(&mut self) -> PResult<ByteStr> {
        let k = self.peek();
        if k == TokenKind::String {
            let i = self.bump();
            return Ok(self.name_text(i));
        }
        if k.is_semi_reserved() {
            let i = self.pos;
            let s = self.lex_tstring(i)?;
            self.bump();
            return Ok(s);
        }
        Err(self.syntax_error())
    }

    /// `zend_lex_tstring()`: convert a keyword token into an identifier.
    pub(crate) fn lex_tstring(&mut self, i: usize) -> PResult<ByteStr> {
        let text = self.text(i);
        let n = text
            .iter()
            .take_while(|c| c.is_ascii_alphabetic() || **c == b'_')
            .count();
        if n == 0 {
            return Err(self.error_at(
                ParseErrorKind::Parse,
                "Cannot use \"<?=\" as an identifier",
                self.toks[i].end_line,
            ));
        }
        self.feedback(i);
        Ok(ByteStr::from(&text[..n]))
    }

    /// `ON_FEEDBACK`: the token was reinterpreted as `T_STRING`.
    pub(crate) fn feedback(&mut self, i: usize) {
        let rec = self.toks[i].rec;
        if let Some(r) = self.record.as_mut()
            && let Some(t) = r.get_mut(rec as usize)
        {
            t.kind = TokenKind::String;
        }
    }

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    pub(crate) fn error_at(
        &self,
        kind: ParseErrorKind,
        msg: impl Into<String>,
        line: Line,
    ) -> Fail {
        Fail::Error(ParseError {
            kind,
            message: msg.into(),
            line,
            diagnostics: Vec::new(),
        })
    }

    /// A `CompileError` thrown from a grammar action (reported as a fatal
    /// error), at the line of the current lookahead.
    pub(crate) fn compile_error(&self, msg: impl Into<String>) -> Fail {
        self.error_at(ParseErrorKind::Compile, msg, self.la_line())
    }

    // ---------------------------------------------------------------------
    // Doc comments (`CG(doc_comment)` emulation)
    // ---------------------------------------------------------------------

    fn sync_doc(&mut self, upto: usize) {
        while self.doc_applied < upto && self.doc_applied < self.toks.len() {
            let t = &self.toks[self.doc_applied];
            if t.doc != 0 {
                self.cur_doc = t.doc;
            }
            if t.kind == TokenKind::RBrace {
                self.cur_doc = 0;
            }
            self.doc_applied += 1;
        }
    }

    /// `backup_doc_comment`: take the pending doc comment. With
    /// `with_lookahead`, the lookahead token has already been scanned by
    /// bison at this point (so a doc comment right before it counts).
    pub(crate) fn take_doc(&mut self, with_lookahead: bool) -> Option<ByteStr> {
        if with_lookahead {
            self.sync_doc(self.pos + 1);
        }
        let d = std::mem::take(&mut self.cur_doc);
        self.doc_text(d)
    }

    pub(crate) fn take_doc_id(&mut self) -> u32 {
        std::mem::take(&mut self.cur_doc)
    }

    pub(crate) fn restore_doc_id(&mut self, id: u32) {
        self.cur_doc = id;
    }

    pub(crate) fn reset_doc(&mut self, with_lookahead: bool) {
        if with_lookahead {
            self.sync_doc(self.pos + 1);
        }
        self.cur_doc = 0;
    }

    fn doc_text(&self, id: u32) -> Option<ByteStr> {
        if id == 0 {
            return None;
        }
        let (s, e) = self.doc_comments[id as usize - 1];
        Some(ByteStr::from(&self.src[s as usize..e as usize]))
    }

    // ---------------------------------------------------------------------
    // Function flags (`CG(extra_fn_flags)`)
    // ---------------------------------------------------------------------

    pub(crate) fn push_fn(&mut self) {
        self.fn_flags.push(false);
    }

    pub(crate) fn pop_fn(&mut self) -> bool {
        self.fn_flags.pop().unwrap_or(false)
    }

    pub(crate) fn mark_generator(&mut self) {
        if let Some(f) = self.fn_flags.last_mut() {
            *f = true;
        }
    }

    pub(crate) fn halt_offset(&self) -> Option<usize> {
        self.halt_offset
    }

    pub(crate) fn add_diagnostic(&mut self, d: Diagnostic) {
        self.diagnostics.push((self.pos, d));
    }

    /// Turn an internal failure into the public error.
    pub(crate) fn finish_error(&mut self, f: Fail) -> ParseError {
        let (mut err, idx) = match f {
            Fail::Error(e) => (e, self.pos),
            Fail::Syntax(i) => {
                let (kind, message, line, idx) = self.locate_syntax_error(i);
                (
                    ParseError {
                        kind,
                        message,
                        line,
                        diagnostics: Vec::new(),
                    },
                    idx,
                )
            }
            Fail::TooDeep => {
                let (kind, message, line, idx) = self.locate_syntax_error(self.pos);
                (
                    ParseError {
                        kind,
                        message,
                        line,
                        diagnostics: Vec::new(),
                    },
                    idx,
                )
            }
        };
        err.diagnostics = self
            .diagnostics
            .iter()
            .filter(|(i, _)| *i <= idx)
            .map(|(_, d)| d.clone())
            .collect();
        err
    }

    /// Parse a whole file.
    pub(crate) fn parse_file(&mut self) -> PResult<Vec<Stmt>> {
        let stmts = self.top_statement_list(TokenKind::End)?;
        if !self.at(TokenKind::End) {
            return Err(self.syntax_error());
        }
        Ok(stmts)
    }
}

/// Minimum line of two optional lines (`zend_ast_create` semantics).
pub(crate) fn min_line(a: Line, b: Line) -> Line {
    a.min(b)
}
