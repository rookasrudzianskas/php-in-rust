//! Port of `Zend/zend_language_scanner.l`.
//!
//! The lexer works in two modes, mirroring php-src's `PARSER_MODE()`:
//!
//! * **parser mode** skips whitespace, comments and open tags, returns `;`
//!   for `?>` and `T_ECHO` for `<?=`, attaches semantic values (unescaped
//!   strings, numbers) and stops at the first error with a `T_ERROR` token;
//! * **tokenizer mode** (`token_get_all()` without `TOKEN_PARSE`) returns
//!   every token and never fails.

use crate::numeric;
use crate::token::{Token, TokenKind};
use crate::{Diagnostic, DiagnosticLevel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Initial,
    InScripting,
    LookingForProperty,
    DoubleQuotes,
    Backquote,
    Heredoc,
    Nowdoc,
    EndHeredoc,
    LookingForVarname,
    VarOffset,
}

#[derive(Clone, Debug)]
struct HeredocLabel {
    label: Vec<u8>,
    indentation: usize,
    uses_spaces: bool,
}

/// Semantic value of a token (parser mode only).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum TokVal {
    #[default]
    None,
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
}

/// A token as seen by the parser.
#[derive(Clone, Debug)]
pub(crate) struct LexToken {
    pub kind: TokenKind,
    pub start: u32,
    pub end: u32,
    /// Line the token starts on.
    pub line: u32,
    /// `CG(zend_lineno)` right after the token was scanned.
    pub end_line: u32,
    pub value: TokVal,
    /// 1-based index of the doc comment scanned right before this token
    /// (0 = none).
    pub doc: u32,
    /// Index of this token in the `TOKEN_PARSE` token record.
    pub rec: u32,
}

/// A lexer error (thrown as `ParseError` by php-src).
#[derive(Clone, Debug)]
pub(crate) struct LexError {
    pub message: String,
    pub line: u32,
}

const USING_SPACES: u8 = 1;
const USING_TABS: u8 = 2;

#[inline]
fn is_label_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

#[inline]
fn is_label_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

#[inline]
fn is_oct(c: u8) -> bool {
    (b'0'..=b'7').contains(&c)
}

enum Lexed {
    /// A token to return.
    Tok(TokenKind, TokVal),
    /// A token that is skipped in parser mode (whitespace, comments, open tag).
    Skip(TokenKind),
    /// Rescan from the current position (state changed).
    Restart,
}

pub(crate) struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    state: State,
    stack: Vec<State>,
    heredocs: Vec<HeredocLabel>,
    nesting: Vec<(u8, u32)>,
    pub line: u32,
    increment_line: bool,
    parser_mode: bool,
    short_tags: bool,
    scan_ahead: bool,
    scan_ahead_indent: usize,
    scan_ahead_spaces: bool,
    /// Set whenever php-src would throw a `ParseError`.
    pub error: Option<LexError>,
    pub diagnostics: Vec<Diagnostic>,
    /// Doc comments seen so far (parser mode), as byte ranges.
    pub doc_comments: Vec<(u32, u32)>,
    pending_doc: u32,
    /// Every token, including whitespace and comments (`TOKEN_PARSE`).
    pub record: Option<Vec<Token>>,
    /// Recent significant token kinds, to detect `__halt_compiler();`.
    halt_seq: u8,
    pub halt_offset: Option<usize>,
    halted: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [u8], parser_mode: bool) -> Self {
        Lexer {
            src,
            pos: 0,
            state: State::Initial,
            stack: Vec::new(),
            heredocs: Vec::new(),
            nesting: Vec::new(),
            line: 1,
            increment_line: false,
            parser_mode,
            short_tags: false,
            scan_ahead: false,
            scan_ahead_indent: 0,
            scan_ahead_spaces: false,
            error: None,
            diagnostics: Vec::new(),
            doc_comments: Vec::new(),
            pending_doc: 0,
            record: None,
            halt_seq: 0,
            halt_offset: None,
            halted: false,
        }
    }

    /// Start in `ST_IN_SCRIPTING` (as `eval()` does).
    pub fn start_in_scripting(&mut self) {
        self.state = State::InScripting;
    }

    pub fn set_short_tags(&mut self, on: bool) {
        self.short_tags = on;
    }

    /// Skip a leading `#!...` line (`CG(skip_shebang)`).
    pub fn skip_shebang(&mut self) {
        if self.src.starts_with(b"#!") {
            let mut p = 2;
            while p < self.src.len() && self.src[p] != b'\n' && self.src[p] != b'\r' {
                p += 1;
            }
            if p < self.src.len() {
                if self.src[p] == b'\r' && self.src.get(p + 1) == Some(&b'\n') {
                    p += 1;
                }
                self.pos = p + 1;
                self.line += 1;
            }
        }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    /// Apply a pending `CG(increment_lineno)` (done between tokens).
    pub fn apply_increment(&mut self) {
        if self.increment_line {
            self.line += 1;
            self.increment_line = false;
        }
    }

    #[inline]
    fn at(&self, i: usize) -> u8 {
        self.src.get(i).copied().unwrap_or(0)
    }

    fn starts_with_ci(&self, p: usize, s: &[u8]) -> bool {
        self.src.len() >= p + s.len() && self.src[p..p + s.len()].eq_ignore_ascii_case(s)
    }

    fn throw(&mut self, message: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(LexError {
                message: message.into(),
                line: self.line,
            });
        }
    }

    fn diag(&mut self, level: DiagnosticLevel, message: String) {
        if !self.scan_ahead {
            self.diagnostics.push(Diagnostic {
                level,
                message,
                line: self.line,
            });
        }
    }

    fn handle_newlines(&mut self, start: usize, end: usize) {
        let mut p = start;
        while p < end {
            let c = self.src[p];
            if c == b'\n' || (c == b'\r' && self.at(p + 1) != b'\n') {
                self.line += 1;
            }
            p += 1;
        }
    }

    fn push_state(&mut self, s: State) {
        self.stack.push(self.state);
        self.state = s;
    }

    fn pop_state(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    fn enter_nesting(&mut self, c: u8) {
        self.nesting.push((c, self.line));
    }

    fn exit_nesting(&mut self, closing: u8) -> bool {
        match self.nesting.last() {
            None => {
                self.throw(format!("Unmatched '{}'", closing as char));
                false
            }
            Some(&(opening, line)) => {
                let ok = matches!(
                    (opening, closing),
                    (b'{', b'}') | (b'[', b']') | (b'(', b')')
                );
                if !ok {
                    self.report_bad_nesting(opening, line, closing);
                    return false;
                }
                self.nesting.pop();
                true
            }
        }
    }

    fn report_bad_nesting(&mut self, opening: u8, opening_line: u32, closing: u8) {
        let mut msg = format!("Unclosed '{}'", opening as char);
        if opening_line != self.line {
            msg.push_str(&format!(" on line {opening_line}"));
        }
        if closing != 0 {
            msg.push_str(&format!(" does not match '{}'", closing as char));
        }
        self.throw(msg);
    }

    fn end_token(&mut self) -> Lexed {
        if let Some(&(opening, line)) = self.nesting.last() {
            self.report_bad_nesting(opening, line, 0);
            if self.parser_mode {
                return Lexed::Tok(TokenKind::Error, TokVal::None);
            }
        }
        Lexed::Tok(TokenKind::End, TokVal::None)
    }

    fn exit_nesting_token(&mut self, c: u8, kind: TokenKind) -> Lexed {
        if !self.exit_nesting(c) && self.parser_mode {
            return Lexed::Tok(TokenKind::Error, TokVal::None);
        }
        Lexed::Tok(kind, TokVal::None)
    }

    fn error_token(&self) -> Lexed {
        Lexed::Tok(TokenKind::Error, TokVal::None)
    }

    fn scan_label(&self, mut p: usize) -> usize {
        while p < self.src.len() && is_label_char(self.src[p]) {
            p += 1;
        }
        p
    }

    // ---------------------------------------------------------------------
    // Driver
    // ---------------------------------------------------------------------

    /// One `lex_scan()` call in tokenizer mode: returns the next token
    /// (including whitespace) as `(kind, start, end, line)`.
    pub fn next_raw(&mut self) -> (TokenKind, usize, usize, u32) {
        loop {
            let start = self.pos;
            let line = self.line;
            match self.scan(start) {
                Lexed::Tok(kind, _) | Lexed::Skip(kind) => return (kind, start, self.pos, line),
                Lexed::Restart => continue,
            }
        }
    }

    /// The equivalent of `zendlex()` in parser mode.
    pub fn next_token(&mut self) -> LexToken {
        if self.increment_line {
            self.line += 1;
            self.increment_line = false;
        }
        if self.halted {
            self.halted = false;
            self.pos = self.src.len();
            self.state = State::InScripting;
        }
        loop {
            let start = self.pos;
            let line = self.line;
            match self.scan(start) {
                Lexed::Restart => continue,
                Lexed::Skip(kind) => {
                    if kind == TokenKind::DocComment {
                        self.doc_comments.push((start as u32, self.pos as u32));
                        self.pending_doc = self.doc_comments.len() as u32;
                    }
                    self.record_token(kind, start, self.pos, line);
                }
                Lexed::Tok(kind, value) => {
                    let rec = self.record_token(kind, start, self.pos, line);
                    let doc = std::mem::take(&mut self.pending_doc);
                    self.track_halt(kind);
                    return LexToken {
                        kind,
                        start: start as u32,
                        end: self.pos as u32,
                        line,
                        end_line: self.line,
                        value,
                        doc,
                        rec,
                    };
                }
            }
        }
    }

    fn record_token(&mut self, kind: TokenKind, start: usize, end: usize, line: u32) -> u32 {
        let Some(rec) = self.record.as_mut() else {
            return u32::MAX;
        };
        if kind == TokenKind::End {
            return u32::MAX;
        }
        let kind = match kind {
            TokenKind::Semicolon if end - start > 1 => TokenKind::CloseTag,
            TokenKind::Echo if end - start == 3 && self.src[start] == b'<' => {
                TokenKind::OpenTagWithEcho
            }
            k => k,
        };
        rec.push(Token {
            kind,
            start: start as u32,
            len: (end - start) as u32,
            line,
        });
        (rec.len() - 1) as u32
    }

    fn track_halt(&mut self, kind: TokenKind) {
        self.halt_seq = match (self.halt_seq, kind) {
            (_, TokenKind::HaltCompiler) => 1,
            (1, TokenKind::LParen) => 2,
            (2, TokenKind::RParen) => 3,
            (3, TokenKind::Semicolon) => {
                self.halt_offset = Some(self.pos);
                self.halted = true;
                if let Some(rec) = self.record.as_mut()
                    && self.pos < self.src.len()
                {
                    rec.push(Token {
                        kind: TokenKind::InlineHtml,
                        start: self.pos as u32,
                        len: (self.src.len() - self.pos) as u32,
                        line: self.line,
                    });
                }
                0
            }
            _ => 0,
        };
    }

    fn scan(&mut self, start: usize) -> Lexed {
        match self.state {
            State::Initial => self.scan_initial(start),
            State::InScripting => self.scan_scripting(start),
            State::LookingForProperty => self.scan_looking_for_property(start),
            State::DoubleQuotes | State::Backquote | State::Heredoc => {
                self.scan_interpolated(start)
            }
            State::Nowdoc => self.scan_nowdoc(start),
            State::EndHeredoc => self.scan_end_heredoc(start),
            State::LookingForVarname => self.scan_looking_for_varname(start),
            State::VarOffset => self.scan_var_offset(start),
        }
    }

    // ---------------------------------------------------------------------
    // INITIAL
    // ---------------------------------------------------------------------

    fn scan_initial(&mut self, start: usize) -> Lexed {
        let len = self.src.len();
        if start >= len {
            return self.end_token();
        }
        if self.src[start..].starts_with(b"<?=") {
            self.pos = start + 3;
            self.state = State::InScripting;
            let kind = if self.parser_mode {
                TokenKind::Echo
            } else {
                TokenKind::OpenTagWithEcho
            };
            return Lexed::Tok(kind, TokVal::None);
        }
        if self.starts_with_ci(start, b"<?php") {
            let c = self.at(start + 5);
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                let mut end = start + 6;
                if c == b'\r' && self.at(start + 6) == b'\n' {
                    end += 1;
                }
                if c == b'\n' || c == b'\r' {
                    self.line += 1;
                }
                self.pos = end;
                self.state = State::InScripting;
                return Lexed::Skip(TokenKind::OpenTag);
            }
            if start + 5 == len {
                self.pos = len;
                self.state = State::InScripting;
                return Lexed::Skip(TokenKind::OpenTag);
            }
            if self.short_tags {
                self.pos = start + 2;
                self.state = State::InScripting;
                return Lexed::Skip(TokenKind::OpenTag);
            }
            return self.inline_html(start, start + 5);
        }
        if self.src[start..].starts_with(b"<?") {
            if self.short_tags {
                self.pos = start + 2;
                self.state = State::InScripting;
                return Lexed::Skip(TokenKind::OpenTag);
            }
            return self.inline_html(start, start + 2);
        }
        self.inline_html(start, start + 1)
    }

    fn inline_html(&mut self, start: usize, mut cur: usize) -> Lexed {
        let len = self.src.len();
        loop {
            match memchr(b'<', &self.src[cur.min(len)..]) {
                None => {
                    cur = len;
                    break;
                }
                Some(i) => cur += i + 1,
            }
            if cur >= len {
                break;
            }
            if self.src[cur] == b'?'
                && (self.short_tags
                    || self.at(cur + 1) == b'='
                    || (self.starts_with_ci(cur + 1, b"php")
                        && (cur + 4 == len
                            || matches!(self.at(cur + 4), b' ' | b'\t' | b'\n' | b'\r'))))
            {
                cur -= 1;
                break;
            }
        }
        self.pos = cur;
        self.handle_newlines(start, cur);
        Lexed::Tok(TokenKind::InlineHtml, TokVal::None)
    }

    // ---------------------------------------------------------------------
    // ST_IN_SCRIPTING
    // ---------------------------------------------------------------------

    fn scan_scripting(&mut self, start: usize) -> Lexed {
        let len = self.src.len();
        if start >= len {
            return self.end_token();
        }
        let c = self.src[start];
        let c1 = self.at(start + 1);
        let c2 = self.at(start + 2);
        let tok = |lx: &mut Self, n: usize, k: TokenKind| {
            lx.pos = start + n;
            Lexed::Tok(k, TokVal::None)
        };
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => self.whitespace(start),
            b'$' => {
                if is_label_start(c1) {
                    let end = self.scan_label(start + 1);
                    self.pos = end;
                    self.var_token()
                } else {
                    tok(self, 1, TokenKind::Dollar)
                }
            }
            b'b' | b'B' if c1 == b'\'' => self.single_quoted(start, 1),
            b'b' | b'B' if c1 == b'"' => self.double_quoted(start, 1),
            b'b' | b'B' if c1 == b'<' && self.src[start + 1..].starts_with(b"<<<") => {
                match self.heredoc_start(start, 1) {
                    Some(l) => l,
                    None => self.label_or_keyword(start),
                }
            }
            c if is_label_start(c) => self.label_or_keyword(start),
            b'\\' => {
                if is_label_start(c1) {
                    let mut end = self.scan_label(start + 1);
                    while self.at(end) == b'\\' && is_label_start(self.at(end + 1)) {
                        end = self.scan_label(end + 1);
                    }
                    self.pos = end;
                    self.str_token(TokenKind::NameFullyQualified)
                } else {
                    tok(self, 1, TokenKind::NsSeparator)
                }
            }
            b'0'..=b'9' => self.number(start),
            b'.' => {
                if c1.is_ascii_digit() {
                    self.number(start)
                } else if c1 == b'.' && c2 == b'.' {
                    tok(self, 3, TokenKind::Ellipsis)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::ConcatEqual)
                } else {
                    tok(self, 1, TokenKind::Dot)
                }
            }
            b'\'' => self.single_quoted(start, 0),
            b'"' => self.double_quoted(start, 0),
            b'`' => {
                self.state = State::Backquote;
                tok(self, 1, TokenKind::Backtick)
            }
            b'<' => {
                if c1 == b'<'
                    && c2 == b'<'
                    && let Some(l) = self.heredoc_start(start, 0)
                {
                    return l;
                }
                if c1 == b'<' && c2 == b'=' {
                    tok(self, 3, TokenKind::SlEqual)
                } else if c1 == b'<' {
                    tok(self, 2, TokenKind::Sl)
                } else if c1 == b'=' && c2 == b'>' {
                    tok(self, 3, TokenKind::Spaceship)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::IsSmallerOrEqual)
                } else if c1 == b'>' {
                    tok(self, 2, TokenKind::IsNotEqual)
                } else {
                    tok(self, 1, TokenKind::Lt)
                }
            }
            b'>' => {
                if c1 == b'>' && c2 == b'=' {
                    tok(self, 3, TokenKind::SrEqual)
                } else if c1 == b'>' {
                    tok(self, 2, TokenKind::Sr)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::IsGreaterOrEqual)
                } else {
                    tok(self, 1, TokenKind::Gt)
                }
            }
            b'=' => {
                if c1 == b'=' && c2 == b'=' {
                    tok(self, 3, TokenKind::IsIdentical)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::IsEqual)
                } else if c1 == b'>' {
                    tok(self, 2, TokenKind::DoubleArrow)
                } else {
                    tok(self, 1, TokenKind::Equals)
                }
            }
            b'!' => {
                if c1 == b'=' && c2 == b'=' {
                    tok(self, 3, TokenKind::IsNotIdentical)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::IsNotEqual)
                } else {
                    tok(self, 1, TokenKind::Bang)
                }
            }
            b'+' => match c1 {
                b'+' => tok(self, 2, TokenKind::Inc),
                b'=' => tok(self, 2, TokenKind::PlusEqual),
                _ => tok(self, 1, TokenKind::Plus),
            },
            b'-' => match c1 {
                b'>' => {
                    self.push_state(State::LookingForProperty);
                    tok(self, 2, TokenKind::ObjectOperator)
                }
                b'-' => tok(self, 2, TokenKind::Dec),
                b'=' => tok(self, 2, TokenKind::MinusEqual),
                _ => tok(self, 1, TokenKind::Minus),
            },
            b'*' => {
                if c1 == b'*' && c2 == b'=' {
                    tok(self, 3, TokenKind::PowEqual)
                } else if c1 == b'*' {
                    tok(self, 2, TokenKind::Pow)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::MulEqual)
                } else {
                    tok(self, 1, TokenKind::Star)
                }
            }
            b'/' => match c1 {
                b'*' => self.block_comment(start),
                b'/' => self.line_comment(start, 2),
                b'=' => tok(self, 2, TokenKind::DivEqual),
                _ => tok(self, 1, TokenKind::Slash),
            },
            b'%' => {
                if c1 == b'=' {
                    tok(self, 2, TokenKind::ModEqual)
                } else {
                    tok(self, 1, TokenKind::Percent)
                }
            }
            b'&' => {
                if c1 == b'&' {
                    tok(self, 2, TokenKind::BooleanAnd)
                } else if c1 == b'=' {
                    tok(self, 2, TokenKind::AndEqual)
                } else {
                    let p = self.skip_ws_or_comments(start + 1);
                    if self.at(p) == b'$' || self.src[p.min(len)..].starts_with(b"...") {
                        tok(self, 1, TokenKind::AmpersandFollowedByVarOrVararg)
                    } else {
                        tok(self, 1, TokenKind::AmpersandNotFollowedByVarOrVararg)
                    }
                }
            }
            b'|' => match c1 {
                b'|' => tok(self, 2, TokenKind::BooleanOr),
                b'=' => tok(self, 2, TokenKind::OrEqual),
                b'>' => tok(self, 2, TokenKind::Pipe),
                _ => tok(self, 1, TokenKind::Bar),
            },
            b'^' => {
                if c1 == b'=' {
                    tok(self, 2, TokenKind::XorEqual)
                } else {
                    tok(self, 1, TokenKind::Caret)
                }
            }
            b'?' => {
                if c1 == b'-' && c2 == b'>' {
                    self.push_state(State::LookingForProperty);
                    tok(self, 3, TokenKind::NullsafeObjectOperator)
                } else if c1 == b'?' && c2 == b'=' {
                    tok(self, 3, TokenKind::CoalesceEqual)
                } else if c1 == b'?' {
                    tok(self, 2, TokenKind::Coalesce)
                } else if c1 == b'>' {
                    let mut end = start + 2;
                    match self.at(end) {
                        b'\n' => end += 1,
                        b'\r' => {
                            end += 1;
                            if self.at(end) == b'\n' {
                                end += 1;
                            }
                        }
                        _ => {}
                    }
                    self.state = State::Initial;
                    if end > start + 2 {
                        self.increment_line = true;
                    }
                    self.pos = end;
                    if self.parser_mode {
                        Lexed::Tok(TokenKind::Semicolon, TokVal::None)
                    } else {
                        Lexed::Tok(TokenKind::CloseTag, TokVal::None)
                    }
                } else {
                    tok(self, 1, TokenKind::Question)
                }
            }
            b':' => {
                if c1 == b':' {
                    tok(self, 2, TokenKind::PaamayimNekudotayim)
                } else {
                    tok(self, 1, TokenKind::Colon)
                }
            }
            b'#' => {
                if c1 == b'[' {
                    self.enter_nesting(b'[');
                    tok(self, 2, TokenKind::Attribute)
                } else {
                    self.line_comment(start, 1)
                }
            }
            b'(' => {
                if let Some(l) = self.cast(start) {
                    return l;
                }
                self.enter_nesting(b'(');
                tok(self, 1, TokenKind::LParen)
            }
            b')' => {
                self.pos = start + 1;
                self.exit_nesting_token(b')', TokenKind::RParen)
            }
            b']' => {
                self.pos = start + 1;
                self.exit_nesting_token(b']', TokenKind::RBracket)
            }
            b'[' => {
                self.enter_nesting(b'[');
                tok(self, 1, TokenKind::LBracket)
            }
            b'{' => {
                self.push_state(State::InScripting);
                self.enter_nesting(b'{');
                tok(self, 1, TokenKind::LBrace)
            }
            b'}' => {
                self.pending_doc = 0;
                if !self.stack.is_empty() {
                    self.pop_state();
                }
                self.pos = start + 1;
                self.exit_nesting_token(b'}', TokenKind::RBrace)
            }
            b';' => tok(self, 1, TokenKind::Semicolon),
            b',' => tok(self, 1, TokenKind::Comma),
            b'~' => tok(self, 1, TokenKind::Tilde),
            b'@' => tok(self, 1, TokenKind::At),
            _ => tok(self, 1, TokenKind::BadCharacter),
        }
    }

    fn whitespace(&mut self, start: usize) -> Lexed {
        let mut p = start;
        while matches!(self.at(p), b' ' | b'\t' | b'\n' | b'\r') {
            p += 1;
        }
        self.pos = p;
        self.handle_newlines(start, p);
        Lexed::Skip(TokenKind::Whitespace)
    }

    fn var_token(&self) -> Lexed {
        Lexed::Tok(TokenKind::Variable, TokVal::None)
    }

    /// Tokens whose value is derived from the source text by the parser.
    fn str_token(&self, kind: TokenKind) -> Lexed {
        Lexed::Tok(kind, TokVal::None)
    }

    /// `{WHITESPACE_OR_COMMENTS}*` as used in lookahead patterns; returns the
    /// end position.
    fn skip_ws_or_comments(&self, mut p: usize) -> usize {
        loop {
            match self.at(p) {
                b' ' | b'\t' | b'\n' | b'\r' => p += 1,
                b'/' if self.at(p + 1) == b'*' => {
                    let mut q = p + 2;
                    loop {
                        match self.at(q) {
                            0 => return p,
                            b'*' if self.at(q + 1) == b'/' => {
                                q += 2;
                                break;
                            }
                            _ => q += 1,
                        }
                    }
                    p = q;
                }
                b'/' if self.at(p + 1) == b'/' => match self.line_comment_end(p + 2) {
                    Some(q) => p = q,
                    None => return p,
                },
                b'#' => {
                    let n = self.at(p + 1);
                    if n == b'\n' || n == b'\r' {
                        p += 2;
                    } else if n == b'[' || n == 0 {
                        return p;
                    } else {
                        match self.line_comment_end(p + 2) {
                            Some(q) => p = q,
                            None => return p,
                        }
                    }
                }
                _ => return p,
            }
        }
    }

    /// End of `[^\x00\n\r]*[\n\r]` starting at `p`, if it matches.
    fn line_comment_end(&self, mut p: usize) -> Option<usize> {
        loop {
            match self.at(p) {
                0 => return None,
                b'\n' | b'\r' => return Some(p + 1),
                _ => p += 1,
            }
        }
    }

    fn label_or_keyword(&mut self, start: usize) -> Lexed {
        let end = self.scan_label(start);
        // Qualified names take precedence (longest match).
        if self.at(end) == b'\\' && is_label_start(self.at(end + 1)) {
            let mut e = self.scan_label(end + 1);
            while self.at(e) == b'\\' && is_label_start(self.at(e + 1)) {
                e = self.scan_label(e + 1);
            }
            self.pos = e;
            if self.src[start..end].eq_ignore_ascii_case(b"namespace") {
                return self.str_token(TokenKind::NameRelative);
            }
            return self.str_token(TokenKind::NameQualified);
        }
        let word = &self.src[start..end];
        let Some(kind) = keyword(word) else {
            self.pos = end;
            return self.str_token(TokenKind::String);
        };
        match kind {
            TokenKind::Yield => {
                let p = self.skip_ws_or_comments(end);
                if p > end && self.starts_with_ci(p, b"from") && !is_label_char(self.at(p + 4)) {
                    self.pos = p + 4;
                    self.handle_newlines(start, p + 4);
                    return Lexed::Tok(TokenKind::YieldFrom, TokVal::None);
                }
            }
            TokenKind::Enum => {
                let p = self.skip_ws_or_comments(end);
                self.pos = end;
                if p > end {
                    if self.starts_with_ci(p, b"extends") || self.starts_with_ci(p, b"implements") {
                        return self.str_token(TokenKind::String);
                    }
                    let n = self.at(p);
                    if n.is_ascii_alphabetic() || n == b'_' || n >= 0x80 {
                        return Lexed::Tok(TokenKind::Enum, TokVal::None);
                    }
                }
                return self.str_token(TokenKind::String);
            }
            TokenKind::Public | TokenKind::Protected | TokenKind::Private
                if self.starts_with_ci(end, b"(set)") =>
            {
                self.pos = end + 5;
                let k = match kind {
                    TokenKind::Public => TokenKind::PublicSet,
                    TokenKind::Protected => TokenKind::ProtectedSet,
                    _ => TokenKind::PrivateSet,
                };
                return Lexed::Tok(k, TokVal::None);
            }
            _ => {}
        }
        self.pos = end;
        Lexed::Tok(kind, TokVal::None)
    }

    fn cast(&mut self, start: usize) -> Option<Lexed> {
        let mut p = start + 1;
        while matches!(self.at(p), b' ' | b'\t') {
            p += 1;
        }
        let ws = p;
        while self.at(p).is_ascii_alphabetic() {
            p += 1;
        }
        let word_end = p;
        while matches!(self.at(p), b' ' | b'\t') {
            p += 1;
        }
        if self.at(p) != b')' || word_end == ws {
            return None;
        }
        let word = self.src[ws..word_end].to_ascii_lowercase();
        let (kind, deprecated): (TokenKind, Option<(&str, &str)>) = match &word[..] {
            b"int" => (TokenKind::IntCast, None),
            b"integer" => (TokenKind::IntCast, Some(("integer", "int"))),
            b"float" => (TokenKind::DoubleCast, None),
            b"double" => (TokenKind::DoubleCast, Some(("double", "float"))),
            b"real" => {
                self.pos = p + 1;
                if self.parser_mode {
                    self.throw("The (real) cast has been removed, use (float) instead");
                    return Some(self.error_token());
                }
                return Some(Lexed::Tok(TokenKind::DoubleCast, TokVal::None));
            }
            b"string" => (TokenKind::StringCast, None),
            b"binary" => (TokenKind::StringCast, Some(("binary", "string"))),
            b"array" => (TokenKind::ArrayCast, None),
            b"object" => (TokenKind::ObjectCast, None),
            b"bool" => (TokenKind::BoolCast, None),
            b"boolean" => (TokenKind::BoolCast, Some(("boolean", "bool"))),
            b"unset" => (TokenKind::UnsetCast, None),
            b"void" => (TokenKind::VoidCast, None),
            _ => return None,
        };
        self.pos = p + 1;
        if let (Some((bad, good)), true) = (deprecated, self.parser_mode) {
            self.diag(
                DiagnosticLevel::Deprecated,
                format!("Non-canonical cast ({bad}) is deprecated, use the ({good}) cast instead"),
            );
        }
        Some(Lexed::Tok(kind, TokVal::None))
    }

    fn line_comment(&mut self, start: usize, prefix: usize) -> Lexed {
        let len = self.src.len();
        let mut p = start + prefix;
        while p < len {
            match self.src[p] {
                b'\r' | b'\n' => break,
                b'?' if self.at(p + 1) == b'>' => break,
                _ => p += 1,
            }
        }
        self.pos = p;
        Lexed::Skip(TokenKind::Comment)
    }

    fn block_comment(&mut self, start: usize) -> Lexed {
        let len = self.src.len();
        let mut p = start + 2;
        let mut doc = false;
        if self.at(p) == b'*' && matches!(self.at(p + 1), b' ' | b'\t' | b'\n' | b'\r') {
            doc = true;
            p += 1;
            while matches!(self.at(p), b' ' | b'\t' | b'\n' | b'\r') {
                p += 1;
            }
        }
        let mut closed = false;
        while p < len {
            let c = self.src[p];
            p += 1;
            if c == b'*' && self.at(p) == b'/' {
                closed = true;
                break;
            }
        }
        if closed {
            p += 1;
        } else {
            p = len;
            self.throw(format!("Unterminated comment starting line {}", self.line));
            if self.parser_mode {
                self.pos = p;
                return self.error_token();
            }
        }
        self.pos = p;
        self.handle_newlines(start, p);
        Lexed::Skip(if doc {
            TokenKind::DocComment
        } else {
            TokenKind::Comment
        })
    }

    // ---------------------------------------------------------------------
    // Numbers
    // ---------------------------------------------------------------------

    fn scan_digits_with_underscores(&self, mut p: usize, is_digit: impl Fn(u8) -> bool) -> usize {
        while is_digit(self.at(p)) {
            p += 1;
        }
        while self.at(p) == b'_' && is_digit(self.at(p + 1)) {
            p += 1;
            while is_digit(self.at(p)) {
                p += 1;
            }
        }
        p
    }

    fn number(&mut self, start: usize) -> Lexed {
        let c1 = self.at(start + 1) | 0x20;
        if self.src[start] == b'0' {
            let digit_ok: Option<fn(u8) -> bool> = match c1 {
                b'x' => Some(|c: u8| c.is_ascii_hexdigit()),
                b'b' => Some(|c: u8| c == b'0' || c == b'1'),
                b'o' => Some(is_oct),
                _ => None,
            };
            if let Some(f) = digit_ok
                && f(self.at(start + 2))
            {
                let end = self.scan_digits_with_underscores(start + 2, f);
                self.pos = end;
                let digits = &self.src[start + 2..end];
                let v = match c1 {
                    b'x' => numeric::hex_literal(digits),
                    b'b' => numeric::bin_literal(digits),
                    _ => numeric::oct_literal(digits),
                };
                return self.number_token(v);
            }
        }
        // LNUM / DNUM / EXPONENT_DNUM (longest match).
        let lnum_end = if self.src[start] == b'.' {
            start
        } else {
            self.scan_digits_with_underscores(start, |c| c.is_ascii_digit())
        };
        let mut q = lnum_end;
        let mut is_float = false;
        if self.at(q) == b'.' && (q > start || self.at(q + 1).is_ascii_digit()) {
            q += 1;
            is_float = true;
            if self.at(q).is_ascii_digit() {
                q = self.scan_digits_with_underscores(q, |c| c.is_ascii_digit());
            }
        }
        if self.at(q) | 0x20 == b'e' {
            let mut r = q + 1;
            if matches!(self.at(r), b'+' | b'-') {
                r += 1;
            }
            if self.at(r).is_ascii_digit() {
                q = self.scan_digits_with_underscores(r, |c| c.is_ascii_digit());
                is_float = true;
            }
        }
        if is_float {
            self.pos = q;
            let v = numeric::float_literal(&self.src[start..q]);
            return self.number_token(numeric::Number::Float(v));
        }
        self.pos = lnum_end;
        let text = &self.src[start..lnum_end];
        match numeric::decimal_literal(text) {
            Ok(v) => self.number_token(v),
            Err(prefix_value) => {
                self.throw("Invalid numeric literal");
                if self.parser_mode {
                    return self.error_token();
                }
                self.number_token(prefix_value)
            }
        }
    }

    fn number_token(&self, v: numeric::Number) -> Lexed {
        match v {
            numeric::Number::Int(i) => Lexed::Tok(TokenKind::LNumber, TokVal::Int(i)),
            numeric::Number::Float(f) => Lexed::Tok(TokenKind::DNumber, TokVal::Float(f)),
        }
    }

    // ---------------------------------------------------------------------
    // Strings
    // ---------------------------------------------------------------------

    fn single_quoted(&mut self, start: usize, bprefix: usize) -> Lexed {
        let len = self.src.len();
        let mut p = start + bprefix + 1;
        loop {
            if p < len {
                if self.src[p] == b'\'' {
                    p += 1;
                    break;
                }
                let c = self.src[p];
                p += 1;
                if c == b'\\' && p < len {
                    p += 1;
                }
            } else {
                // Unterminated: php-src returns T_ENCAPSED_AND_WHITESPACE
                // spanning the rest of the input.
                self.pos = len;
                return Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::None);
            }
        }
        self.pos = p;
        let content = &self.src[start + bprefix + 1..p - 1];
        let mut out = Vec::with_capacity(content.len());
        let mut i = 0;
        while i < content.len() {
            let mut c = content[i];
            if c == b'\\' && i + 1 < content.len() {
                i += 1;
                c = content[i];
                if c != b'\\' && c != b'\'' {
                    out.push(b'\\');
                }
            }
            out.push(c);
            if c == b'\n' || (c == b'\r' && content.get(i + 1) != Some(&b'\n')) {
                self.line += 1;
            }
            i += 1;
        }
        let value = if self.parser_mode {
            TokVal::Str(out)
        } else {
            TokVal::None
        };
        Lexed::Tok(TokenKind::ConstantEncapsedString, value)
    }

    /// Scan interpolated string content starting at `p` until the closing
    /// quote or the start of an interpolation; returns the end position.
    fn scan_string_content(&self, mut p: usize, quote: u8) -> usize {
        let len = self.src.len();
        while p < len {
            let c = self.src[p];
            p += 1;
            match c {
                c if c == quote => return p - 1,
                b'$' if is_label_start(self.at(p)) || self.at(p) == b'{' => return p - 1,
                b'{' if self.at(p) == b'$' => return p - 1,
                b'\\' if p < len => {
                    p += 1;
                }
                _ => {}
            }
        }
        p
    }

    fn double_quoted(&mut self, start: usize, bprefix: usize) -> Lexed {
        let len = self.src.len();
        let open = start + bprefix + 1;
        let end = self.scan_string_content(open, b'"');
        if end < len && self.src[end] == b'"' {
            self.pos = end + 1;
            let content = self.src[open..end].to_vec();
            return match self.scan_escape_string(&content, b'"') {
                Some(v) => {
                    let value = if self.parser_mode {
                        TokVal::Str(v)
                    } else {
                        TokVal::None
                    };
                    Lexed::Tok(TokenKind::ConstantEncapsedString, value)
                }
                None if self.parser_mode => self.error_token(),
                None => Lexed::Tok(TokenKind::ConstantEncapsedString, TokVal::None),
            };
        }
        self.pos = open;
        self.state = State::DoubleQuotes;
        Lexed::Tok(TokenKind::DoubleQuote, TokVal::None)
    }

    /// Port of `zend_scan_escape_string()`. Returns `None` after throwing.
    fn scan_escape_string(&mut self, s: &[u8], quote: u8) -> Option<Vec<u8>> {
        let at = |i: usize| s.get(i).copied().unwrap_or(0);
        let n = s.len();
        let mut out = Vec::with_capacity(n);
        let mut i = 0;
        while i < n {
            let c = s[i];
            if c == b'\\' {
                i += 1;
                if i >= n {
                    out.push(b'\\');
                    break;
                }
                let e = s[i];
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'f' => out.push(0x0c),
                    b'v' => out.push(0x0b),
                    b'e' => out.push(0x1b),
                    b'"' | b'`' if e != quote => {
                        out.push(b'\\');
                        out.push(e);
                    }
                    b'"' | b'`' | b'\\' | b'$' => out.push(e),
                    b'x' | b'X' => {
                        if at(i + 1).is_ascii_hexdigit() {
                            i += 1;
                            let mut v = hex_val(s[i]);
                            if at(i + 1).is_ascii_hexdigit() {
                                i += 1;
                                v = v * 16 + hex_val(s[i]);
                            }
                            out.push(v);
                        } else {
                            out.push(b'\\');
                            out.push(e);
                        }
                    }
                    b'u' => {
                        if at(i + 1) != b'{' {
                            out.push(b'\\');
                            out.push(b'u');
                        } else {
                            let digits_start = i + 2;
                            let mut j = digits_start;
                            while at(j).is_ascii_hexdigit() {
                                j += 1;
                            }
                            if at(j) != b'}' || j == digits_start {
                                self.throw("Invalid UTF-8 codepoint escape sequence");
                                return None;
                            }
                            let mut cp: u64 = 0;
                            let mut overflow = false;
                            for &d in &s[digits_start..j] {
                                match cp.checked_mul(16) {
                                    Some(v) => cp = v + u64::from(hex_val(d)),
                                    None => overflow = true,
                                }
                            }
                            if overflow || cp > 0x10FFFF {
                                self.throw(
                                    "Invalid UTF-8 codepoint escape sequence: Codepoint too large",
                                );
                                return None;
                            }
                            encode_utf8(cp as u32, &mut out);
                            i = j;
                        }
                    }
                    _ if is_oct(e) => {
                        let mut digits = vec![e];
                        if is_oct(at(i + 1)) {
                            i += 1;
                            digits.push(s[i]);
                            if is_oct(at(i + 1)) {
                                i += 1;
                                digits.push(s[i]);
                            }
                        }
                        if digits.len() == 3 && digits[0] > b'3' && !self.scan_ahead {
                            let d = String::from_utf8_lossy(&digits).into_owned();
                            self.diag(
                                DiagnosticLevel::CompileWarning,
                                format!(
                                    "Octal escape sequence overflow \\{d} is greater than \\377"
                                ),
                            );
                        }
                        let v = digits
                            .iter()
                            .fold(0u32, |a, &d| a * 8 + u32::from(d - b'0'));
                        out.push(v as u8);
                    }
                    _ => {
                        out.push(b'\\');
                        out.push(e);
                    }
                }
            } else {
                out.push(c);
            }
            let cur = s[i];
            if cur == b'\n' || (cur == b'\r' && at(i + 1) != b'\n') {
                self.line += 1;
            }
            i += 1;
        }
        Some(out)
    }

    fn heredoc_start(&mut self, start: usize, bprefix: usize) -> Option<Lexed> {
        let mut p = start + bprefix + 3;
        while matches!(self.at(p), b' ' | b'\t') {
            p += 1;
        }
        let quote = match self.at(p) {
            q @ (b'\'' | b'"') => {
                p += 1;
                Some(q)
            }
            _ => None,
        };
        if !is_label_start(self.at(p)) {
            return None;
        }
        let label_start = p;
        let label_end = self.scan_label(p);
        p = label_end;
        if let Some(q) = quote {
            if self.at(p) != q {
                return None;
            }
            p += 1;
        }
        match self.at(p) {
            b'\n' => p += 1,
            b'\r' => {
                p += 1;
                if self.at(p) == b'\n' {
                    p += 1;
                }
            }
            _ => return None,
        }
        let len = self.src.len();
        self.line += 1;
        let label = self.src[label_start..label_end].to_vec();
        let is_heredoc = quote != Some(b'\'');
        self.state = if is_heredoc {
            State::Heredoc
        } else {
            State::Nowdoc
        };
        self.heredocs.push(HeredocLabel {
            label: label.clone(),
            indentation: 0,
            uses_spaces: false,
        });
        self.pos = p;
        let saved = p;

        let mut spacing = 0u8;
        let mut indentation = 0usize;
        let mut q = p;
        while q < len && matches!(self.src[q], b' ' | b'\t') {
            spacing |= if self.src[q] == b'\t' {
                USING_TABS
            } else {
                USING_SPACES
            };
            q += 1;
            indentation += 1;
        }
        if q == len {
            return Some(Lexed::Tok(TokenKind::StartHeredoc, TokVal::None));
        }
        if label.len() < len - q
            && self.src[q..q + label.len()] == label[..]
            && !is_label_char(self.at(q + label.len()))
        {
            if spacing == USING_SPACES | USING_TABS {
                self.throw("Invalid indentation - tabs and spaces cannot be mixed");
                if self.parser_mode {
                    return Some(self.error_token());
                }
            }
            if let Some(top) = self.heredocs.last_mut() {
                top.indentation = indentation;
            }
            self.state = State::EndHeredoc;
            return Some(Lexed::Tok(TokenKind::StartHeredoc, TokVal::None));
        }

        if is_heredoc && !self.scan_ahead {
            let mut sub = Lexer {
                src: self.src,
                pos: saved,
                state: self.state,
                stack: self.stack.clone(),
                heredocs: self.heredocs.clone(),
                nesting: self.nesting.clone(),
                line: self.line,
                increment_line: false,
                parser_mode: false,
                short_tags: self.short_tags,
                scan_ahead: true,
                scan_ahead_indent: 0,
                scan_ahead_spaces: false,
                error: None,
                diagnostics: Vec::new(),
                doc_comments: Vec::new(),
                pending_doc: 0,
                record: None,
                halt_seq: 0,
                halt_offset: None,
                halted: false,
            };
            let mut level = 1;
            let mut first = None;
            while level > 0 {
                let (kind, ..) = sub.next_raw();
                if sub.error.is_some() {
                    break;
                }
                first.get_or_insert(kind);
                match kind {
                    TokenKind::StartHeredoc => level += 1,
                    TokenKind::EndHeredoc => level -= 1,
                    TokenKind::End => level = 0,
                    _ => {}
                }
            }
            let mut error = false;
            if matches!(
                first,
                Some(TokenKind::Variable | TokenKind::DollarOpenCurlyBraces | TokenKind::CurlyOpen)
            ) && sub.scan_ahead_indent > 0
            {
                self.throw(format!(
                    "Invalid body indentation level (expecting an indentation level of at least {})",
                    sub.scan_ahead_indent
                ));
                error = true;
            }
            if let Some(top) = self.heredocs.last_mut() {
                top.indentation = sub.scan_ahead_indent;
                top.uses_spaces = sub.scan_ahead_spaces;
            }
            if self.parser_mode && error {
                return Some(self.error_token());
            }
        }
        Some(Lexed::Tok(TokenKind::StartHeredoc, TokVal::None))
    }

    // ---------------------------------------------------------------------
    // Interpolation states
    // ---------------------------------------------------------------------

    fn scan_interpolated(&mut self, start: usize) -> Lexed {
        let c = self.at(start);
        let c1 = self.at(start + 1);
        if c == b'$' && c1 == b'{' {
            self.pos = start + 2;
            self.push_state(State::LookingForVarname);
            self.enter_nesting(b'{');
            return Lexed::Tok(TokenKind::DollarOpenCurlyBraces, TokVal::None);
        }
        if c == b'$' && is_label_start(c1) {
            let end = self.scan_label(start + 1);
            self.pos = end;
            let n0 = self.at(end);
            if n0 == b'-' && self.at(end + 1) == b'>' {
                let n = self.at(end + 2);
                if n.is_ascii_alphabetic() || n == b'_' || n >= 0x80 {
                    self.push_state(State::LookingForProperty);
                }
            } else if n0 == b'?' && self.at(end + 1) == b'-' && self.at(end + 2) == b'>' {
                let n = self.at(end + 3);
                if n.is_ascii_alphabetic() || n == b'_' || n >= 0x80 {
                    self.push_state(State::LookingForProperty);
                }
            } else if n0 == b'[' {
                self.push_state(State::VarOffset);
            }
            return self.var_token();
        }
        if c == b'{' && c1 == b'$' {
            self.pos = start + 1;
            self.push_state(State::InScripting);
            self.enter_nesting(b'{');
            return Lexed::Tok(TokenKind::CurlyOpen, TokVal::None);
        }
        match self.state {
            State::DoubleQuotes => {
                if c == b'"' && start < self.src.len() {
                    self.pos = start + 1;
                    self.state = State::InScripting;
                    return Lexed::Tok(TokenKind::DoubleQuote, TokVal::None);
                }
                self.quoted_content(start, b'"')
            }
            State::Backquote => {
                if c == b'`' && start < self.src.len() {
                    self.pos = start + 1;
                    self.state = State::InScripting;
                    return Lexed::Tok(TokenKind::Backtick, TokVal::None);
                }
                self.quoted_content(start, b'`')
            }
            _ => self.heredoc_body(start),
        }
    }

    fn quoted_content(&mut self, start: usize, quote: u8) -> Lexed {
        let len = self.src.len();
        if start >= len {
            return self.end_token();
        }
        let mut p = start + 1;
        if self.src[start] == b'\\' && p < len {
            p += 1;
        }
        let end = self.scan_string_content(p, quote);
        self.pos = end;
        let content = self.src[start..end].to_vec();
        match self.scan_escape_string(&content, quote) {
            Some(v) => {
                let value = if self.parser_mode {
                    TokVal::Str(v)
                } else {
                    TokVal::None
                };
                Lexed::Tok(TokenKind::EncapsedAndWhitespace, value)
            }
            None if self.parser_mode => self.error_token(),
            None => Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::None),
        }
    }

    fn heredoc_body(&mut self, start: usize) -> Lexed {
        let len = self.src.len();
        if start >= len {
            return self.end_token();
        }
        let Some(top) = self.heredocs.last() else {
            return self.end_token();
        };
        let label = top.label.clone();
        let (label_indent, label_spaces) = (top.indentation, top.uses_spaces);
        let mut p = start;
        let mut newline = 0usize;
        while p < len {
            let c = self.src[p];
            p += 1;
            match c {
                b'\r' | b'\n' => {
                    if c == b'\r' && self.at(p) == b'\n' {
                        p += 1;
                    }
                    let mut indentation = 0usize;
                    let mut spacing = 0u8;
                    while p < len && matches!(self.src[p], b' ' | b'\t') {
                        spacing |= if self.src[p] == b'\t' {
                            USING_TABS
                        } else {
                            USING_SPACES
                        };
                        p += 1;
                        indentation += 1;
                    }
                    if p == len {
                        self.pos = p;
                        self.handle_newlines(start, p);
                        return Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::None);
                    }
                    if is_label_start(self.src[p])
                        && label.len() < len - p
                        && self.src[p..p + label.len()] == label[..]
                    {
                        if is_label_char(self.at(p + label.len())) {
                            continue;
                        }
                        if spacing == USING_SPACES | USING_TABS {
                            self.throw("Invalid indentation - tabs and spaces cannot be mixed");
                            if self.parser_mode {
                                self.pos = p;
                                return self.error_token();
                            }
                        }
                        newline = if p >= indentation + 2
                            && self.src[p - indentation - 2] == b'\r'
                            && self.src[p - indentation - 1] == b'\n'
                        {
                            2
                        } else {
                            1
                        };
                        self.increment_line = true;
                        if self.scan_ahead {
                            self.scan_ahead_indent = indentation;
                            self.scan_ahead_spaces = spacing == USING_SPACES;
                        } else {
                            p -= indentation;
                        }
                        self.state = State::EndHeredoc;
                        break;
                    }
                }
                b'$' if is_label_start(self.at(p)) || self.at(p) == b'{' => {
                    p -= 1;
                    break;
                }
                b'{' if self.at(p) == b'$' => {
                    p -= 1;
                    break;
                }
                b'\\' if p < len && self.src[p] != b'\n' && self.src[p] != b'\r' => {
                    p += 1;
                }
                _ => {}
            }
        }
        self.pos = p;
        let raw_end = p - newline;
        if !self.scan_ahead && self.error.is_none() && self.parser_mode {
            let newline_at_start = start > 0 && matches!(self.src[start - 1], b'\n' | b'\r');
            let raw = self.src[start..raw_end].to_vec();
            let Some(stripped) = self.strip_indentation(
                &raw,
                label_indent,
                label_spaces,
                newline_at_start,
                newline != 0,
            ) else {
                return self.error_token();
            };
            match self.scan_escape_string(&stripped, 0) {
                Some(v) => Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::Str(v)),
                None => self.error_token(),
            }
        } else {
            self.handle_newlines(start, raw_end);
            Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::None)
        }
    }

    fn scan_nowdoc(&mut self, start: usize) -> Lexed {
        let len = self.src.len();
        if start >= len {
            return self.end_token();
        }
        let label = self
            .heredocs
            .last()
            .map(|h| h.label.clone())
            .unwrap_or_default();
        let mut p = start;
        let mut newline = 0usize;
        let mut indentation = 0usize;
        let mut spacing: Option<u8> = None;
        while p < len {
            let c = self.src[p];
            p += 1;
            if c != b'\r' && c != b'\n' {
                continue;
            }
            if c == b'\r' && self.at(p) == b'\n' {
                p += 1;
            }
            indentation = 0;
            let mut sp = 0u8;
            while p < len && matches!(self.src[p], b' ' | b'\t') {
                sp |= if self.src[p] == b'\t' {
                    USING_TABS
                } else {
                    USING_SPACES
                };
                p += 1;
                indentation += 1;
            }
            spacing = Some(sp);
            if p == len {
                self.pos = p;
                self.handle_newlines(start, p);
                return Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::None);
            }
            if is_label_start(self.src[p])
                && label.len() < len - p
                && self.src[p..p + label.len()] == label[..]
            {
                if is_label_char(self.at(p + label.len())) {
                    continue;
                }
                if sp == USING_SPACES | USING_TABS {
                    self.throw("Invalid indentation - tabs and spaces cannot be mixed");
                    if self.parser_mode {
                        self.pos = p;
                        return self.error_token();
                    }
                }
                newline = if p >= indentation + 2
                    && self.src[p - indentation - 2] == b'\r'
                    && self.src[p - indentation - 1] == b'\n'
                {
                    2
                } else {
                    1
                };
                self.increment_line = true;
                p -= indentation;
                if let Some(top) = self.heredocs.last_mut() {
                    top.indentation = indentation;
                }
                self.state = State::EndHeredoc;
                break;
            }
        }
        self.pos = p;
        let raw_end = p - newline;
        let mut value = TokVal::None;
        if self.parser_mode {
            let raw = self.src[start..raw_end].to_vec();
            value = TokVal::Str(raw.clone());
            if self.error.is_none()
                && let Some(sp) = spacing
            {
                let newline_at_start = start > 0 && matches!(self.src[start - 1], b'\n' | b'\r');
                match self.strip_indentation(
                    &raw,
                    indentation,
                    sp == USING_SPACES,
                    newline_at_start,
                    newline != 0,
                ) {
                    Some(v) => value = TokVal::Str(v),
                    None => return self.error_token(),
                }
            }
        }
        self.handle_newlines(start, raw_end);
        Lexed::Tok(TokenKind::EncapsedAndWhitespace, value)
    }

    /// Port of `strip_multiline_string_indentation()`.
    fn strip_indentation(
        &mut self,
        s: &[u8],
        indentation: usize,
        using_spaces: bool,
        newline_at_start: bool,
        newline_at_end: bool,
    ) -> Option<Vec<u8>> {
        let end = s.len();
        let mut out = Vec::with_capacity(end);
        let mut newline_count = 0u32;
        let mut pos;
        let mut nl: Option<usize>;
        if !newline_at_start {
            match next_newline(s, 0) {
                None => return Some(s.to_vec()),
                Some((i, l)) => {
                    out.extend_from_slice(&s[..i + l]);
                    pos = i + l;
                    newline_count += 1;
                    nl = Some(i);
                }
            }
        } else {
            pos = 0;
            nl = Some(0);
        }
        while pos <= end && nl.is_some() {
            let (found, nl_len) = match next_newline(s, pos) {
                Some((i, l)) => (Some(i), l),
                None if newline_at_end => (Some(end), 0),
                None => (None, 0),
            };
            nl = found;
            let mut skip = 0;
            while skip < indentation {
                if Some(pos) == nl {
                    break;
                }
                if pos == end || (s[pos] != b' ' && s[pos] != b'\t') {
                    self.line += newline_count;
                    self.throw(format!(
                        "Invalid body indentation level (expecting an indentation level of at least {indentation})"
                    ));
                    return None;
                }
                if (!using_spaces && s[pos] == b' ') || (using_spaces && s[pos] == b'\t') {
                    self.line += newline_count;
                    self.throw("Invalid indentation - tabs and spaces cannot be mixed");
                    return None;
                }
                skip += 1;
                pos += 1;
            }
            if pos == end {
                break;
            }
            let l = match nl {
                Some(n) => n - pos + nl_len,
                None => end - pos,
            };
            out.extend_from_slice(&s[pos..pos + l]);
            pos += l;
            newline_count += 1;
        }
        Some(out)
    }

    fn scan_end_heredoc(&mut self, start: usize) -> Lexed {
        let label = self.heredocs.pop();
        let n = label
            .map(|h| h.indentation + h.label.len())
            .unwrap_or(1)
            .max(1);
        self.pos = (start + n).min(self.src.len());
        self.state = State::InScripting;
        Lexed::Tok(TokenKind::EndHeredoc, TokVal::None)
    }

    fn scan_looking_for_property(&mut self, start: usize) -> Lexed {
        let c = self.at(start);
        let c1 = self.at(start + 1);
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => self.whitespace(start),
            b'-' if c1 == b'>' => {
                self.pos = start + 2;
                Lexed::Tok(TokenKind::ObjectOperator, TokVal::None)
            }
            b'?' if c1 == b'-' && self.at(start + 2) == b'>' => {
                self.pos = start + 3;
                Lexed::Tok(TokenKind::NullsafeObjectOperator, TokVal::None)
            }
            c if is_label_start(c) => {
                let end = self.scan_label(start);
                self.pos = end;
                self.pop_state();
                self.str_token(TokenKind::String)
            }
            b'#' => self.line_comment(start, 1),
            b'/' if c1 == b'/' => self.line_comment(start, 2),
            b'/' if c1 == b'*' => self.block_comment(start),
            _ => {
                self.pos = start;
                self.pop_state();
                Lexed::Restart
            }
        }
    }

    fn scan_looking_for_varname(&mut self, start: usize) -> Lexed {
        if is_label_start(self.at(start)) {
            let end = self.scan_label(start);
            if matches!(self.at(end), b'[' | b'}') {
                self.pos = end;
                self.pop_state();
                self.push_state(State::InScripting);
                return self.str_token(TokenKind::StringVarname);
            }
        }
        self.pos = start;
        self.pop_state();
        self.push_state(State::InScripting);
        Lexed::Restart
    }

    fn scan_var_offset(&mut self, start: usize) -> Lexed {
        let len = self.src.len();
        if start >= len {
            return self.end_token();
        }
        let c = self.src[start];
        if c.is_ascii_digit() {
            // `[0]|([1-9][0-9]*)` competes with LNUM/HNUM/BNUM/ONUM.
            let simple_end = if c == b'0' {
                start + 1
            } else {
                let mut p = start;
                while self.at(p).is_ascii_digit() {
                    p += 1;
                }
                p
            };
            let mut other_end = self.scan_digits_with_underscores(start, |c| c.is_ascii_digit());
            if c == b'0' {
                let f: Option<fn(u8) -> bool> = match self.at(start + 1) | 0x20 {
                    b'x' => Some(|c: u8| c.is_ascii_hexdigit()),
                    b'b' => Some(|c: u8| c == b'0' || c == b'1'),
                    b'o' => Some(is_oct),
                    _ => None,
                };
                if let Some(f) = f
                    && f(self.at(start + 2))
                {
                    other_end = other_end.max(self.scan_digits_with_underscores(start + 2, f));
                }
            }
            if simple_end >= other_end {
                self.pos = simple_end;
                let digits = &self.src[start..simple_end];
                let value = match numeric::offset_literal(digits) {
                    Some(i) => TokVal::Int(i),
                    None => TokVal::Str(digits.to_vec()),
                };
                return Lexed::Tok(TokenKind::NumString, value);
            }
            self.pos = other_end;
            return self.str_token(TokenKind::NumString);
        }
        if c == b'$' && is_label_start(self.at(start + 1)) {
            let end = self.scan_label(start + 1);
            self.pos = end;
            return self.var_token();
        }
        if c == b']' {
            self.pos = start + 1;
            self.pop_state();
            return Lexed::Tok(TokenKind::RBracket, TokVal::None);
        }
        if let Some(k) = char_token(c) {
            self.pos = start + 1;
            return Lexed::Tok(k, TokVal::None);
        }
        if matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\\' | b'\'' | b'#') {
            self.pos = start;
            self.pop_state();
            return Lexed::Tok(TokenKind::EncapsedAndWhitespace, TokVal::None);
        }
        if is_label_start(c) {
            let end = self.scan_label(start);
            self.pos = end;
            return self.str_token(TokenKind::String);
        }
        self.pos = start + 1;
        Lexed::Tok(TokenKind::BadCharacter, TokVal::None)
    }
}

/// Single characters returned as-is in `ST_VAR_OFFSET`
/// (`{TOKENS}|[[(){}"`]`).
fn char_token(c: u8) -> Option<TokenKind> {
    use TokenKind::*;
    Some(match c {
        b';' => Semicolon,
        b':' => Colon,
        b',' => Comma,
        b'.' => Dot,
        b'|' => Bar,
        b'^' => Caret,
        b'&' => Amp,
        b'+' => Plus,
        b'-' => Minus,
        b'/' => Slash,
        b'*' => Star,
        b'=' => Equals,
        b'%' => Percent,
        b'!' => Bang,
        b'~' => Tilde,
        b'$' => Dollar,
        b'<' => Lt,
        b'>' => Gt,
        b'?' => Question,
        b'@' => At,
        b'[' => LBracket,
        b'(' => LParen,
        b')' => RParen,
        b'{' => LBrace,
        b'}' => RBrace,
        b'"' => DoubleQuote,
        b'`' => Backtick,
        _ => return None,
    })
}

fn next_newline(s: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i < s.len() {
        match s[i] {
            b'\r' => return Some((i, if s.get(i + 1) == Some(&b'\n') { 2 } else { 1 })),
            b'\n' => return Some((i, 1)),
            _ => i += 1,
        }
    }
    None
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

fn encode_utf8(cp: u32, out: &mut Vec<u8>) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp <= 0x7FF {
        out.push(((cp >> 6) + 0xC0) as u8);
        out.push(((cp & 0x3F) + 0x80) as u8);
    } else if cp <= 0xFFFF {
        out.push(((cp >> 12) + 0xE0) as u8);
        out.push((((cp >> 6) & 0x3F) + 0x80) as u8);
        out.push(((cp & 0x3F) + 0x80) as u8);
    } else {
        out.push(((cp >> 18) + 0xF0) as u8);
        out.push((((cp >> 12) & 0x3F) + 0x80) as u8);
        out.push((((cp >> 6) & 0x3F) + 0x80) as u8);
        out.push(((cp & 0x3F) + 0x80) as u8);
    }
}

fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == needle)
}

/// Case-insensitive keyword lookup.
fn keyword(word: &[u8]) -> Option<TokenKind> {
    use TokenKind::*;
    if word.len() < 2 || word.len() > 15 {
        return None;
    }
    let mut buf = [0u8; 15];
    for (d, s) in buf.iter_mut().zip(word) {
        *d = s.to_ascii_lowercase();
    }
    Some(match &buf[..word.len()] {
        b"exit" | b"die" => Exit,
        b"fn" => Fn,
        b"function" => Function,
        b"const" => Const,
        b"return" => Return,
        b"yield" => Yield,
        b"try" => Try,
        b"catch" => Catch,
        b"finally" => Finally,
        b"throw" => Throw,
        b"if" => If,
        b"elseif" => ElseIf,
        b"endif" => EndIf,
        b"else" => Else,
        b"while" => While,
        b"endwhile" => EndWhile,
        b"do" => Do,
        b"for" => For,
        b"endfor" => EndFor,
        b"foreach" => Foreach,
        b"endforeach" => EndForeach,
        b"declare" => Declare,
        b"enddeclare" => EndDeclare,
        b"instanceof" => Instanceof,
        b"as" => As,
        b"switch" => Switch,
        b"match" => Match,
        b"endswitch" => EndSwitch,
        b"case" => Case,
        b"default" => Default,
        b"break" => Break,
        b"continue" => Continue,
        b"goto" => Goto,
        b"echo" => Echo,
        b"print" => Print,
        b"class" => Class,
        b"interface" => Interface,
        b"trait" => Trait,
        b"enum" => Enum,
        b"extends" => Extends,
        b"implements" => Implements,
        b"new" => New,
        b"clone" => Clone,
        b"var" => Var,
        b"eval" => Eval,
        b"include" => Include,
        b"include_once" => IncludeOnce,
        b"require" => Require,
        b"require_once" => RequireOnce,
        b"namespace" => Namespace,
        b"use" => Use,
        b"insteadof" => Insteadof,
        b"global" => Global,
        b"isset" => Isset,
        b"empty" => Empty,
        b"__halt_compiler" => HaltCompiler,
        b"static" => Static,
        b"abstract" => Abstract,
        b"final" => Final,
        b"private" => Private,
        b"protected" => Protected,
        b"public" => Public,
        b"readonly" => Readonly,
        b"unset" => Unset,
        b"list" => List,
        b"array" => Array,
        b"callable" => Callable,
        b"or" => LogicalOr,
        b"and" => LogicalAnd,
        b"xor" => LogicalXor,
        b"__class__" => ClassC,
        b"__trait__" => TraitC,
        b"__function__" => FuncC,
        b"__property__" => PropertyC,
        b"__method__" => MethodC,
        b"__line__" => Line,
        b"__file__" => File,
        b"__dir__" => Dir,
        b"__namespace__" => NsC,
        _ => return None,
    })
}
