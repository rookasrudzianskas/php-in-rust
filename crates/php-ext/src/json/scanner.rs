//! Port of `ext/json/json_scanner.re` (the re2c scanner behind `json_decode()`).
//!
//! The scanner follows the re2c automaton's longest-match semantics: numbers,
//! keywords and string escapes are recognised exactly as the generated code
//! does, and every failure reports the same error code. Positions (`token`,
//! `line`, `line_start`) are tracked as in `php_json_scanner` because
//! `json_last_error_msg()` reports the location of the failing token.

use super::error::ErrorCode;
use super::{BIGINT_AS_STRING, INVALID_UTF8_IGNORE, INVALID_UTF8_SUBSTITUTE};

/// A token (`PHP_JSON_T_*`). `PHP_JSON_T_STRING` and `PHP_JSON_T_ESTRING` are
/// merged: the grammar treats them identically.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Null,
    True,
    False,
    Int(i64),
    Double(f64),
    Str(Vec<u8>),
    Eoi,
    /// `PHP_JSON_T_ERROR`; the code is in [`Scanner::errcode`].
    Error,
}

pub struct Scanner<'a> {
    src: &'a [u8],
    /// Cursor; `src.len() + 1` after the end-of-input "NUL terminator" was consumed.
    cursor: usize,
    /// Start of the token being (or last) scanned.
    token: usize,
    line_start: usize,
    pub line: u64,
    options: i32,
    pub errcode: ErrorCode,
    /// Whether string values are materialised (`false` for `json_validate()`).
    build: bool,
}

/// `UTF8_2 | UTF8_3 | UTF8_4` of the re2c definitions: the length of a valid
/// UTF-8 sequence starting at `s[0]` (which must be `>= 0x80`), if any.
fn utf8_sequence_len(s: &[u8]) -> Option<usize> {
    let trail = |i: usize, lo: u8, hi: u8| s.get(i).is_some_and(|&b| (lo..=hi).contains(&b));
    let t = |i: usize| trail(i, 0x80, 0xBF);
    match s[0] {
        0xC2..=0xDF if t(1) => Some(2),
        0xE0 if trail(1, 0xA0, 0xBF) && t(2) => Some(3),
        0xE1..=0xEC if t(1) && t(2) => Some(3),
        0xED if trail(1, 0x80, 0x9F) && t(2) => Some(3),
        0xEE..=0xEF if t(1) && t(2) => Some(3),
        0xF0 if trail(1, 0x90, 0xBF) && t(2) && t(3) => Some(4),
        0xF1..=0xF3 if t(1) && t(2) && t(3) => Some(4),
        0xF4 if trail(1, 0x80, 0x8F) && t(2) && t(3) => Some(4),
        _ => None,
    }
}

fn hex_val(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

/// Four hex digits at `s[at..at + 4]`.
fn hex4(s: &[u8], at: usize) -> Option<u32> {
    let d = s.get(at..at + 4)?;
    let mut v = 0;
    for &b in d {
        v = (v << 4) | hex_val(b)?;
    }
    Some(v)
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r')
}

impl<'a> Scanner<'a> {
    /// `php_json_scanner_init()`.
    pub fn new(src: &'a [u8], options: i32, build: bool) -> Scanner<'a> {
        Scanner {
            src,
            cursor: 0,
            token: 0,
            line_start: 0,
            line: 1,
            options,
            errcode: ErrorCode::None,
            build,
        }
    }

    /// `php_json_compute_error_column()`: 1-based column of the last token,
    /// counting characters (UTF-8 continuation bytes are folded away).
    pub fn error_column(&self) -> u64 {
        let end = self.token.min(self.src.len());
        let start = self.line_start.min(end);
        1 + self.src[start..end]
            .iter()
            .filter(|&&b| b & 0b1100_0000 != 0b1000_0000)
            .count() as u64
    }

    fn error(&mut self, code: ErrorCode) -> Tok {
        self.errcode = code;
        Tok::Error
    }

    /// `php_json_scan()`: scans the next token.
    pub fn scan(&mut self) -> Tok {
        loop {
            self.token = self.cursor;
            let Some(&c) = self.src.get(self.cursor) else {
                // The NUL terminator of the C buffer: `limit < cursor` after consuming it.
                self.cursor = self.src.len() + 1;
                return Tok::Eoi;
            };
            match c {
                b'{' => return self.single(Tok::LBrace),
                b'}' => return self.single(Tok::RBrace),
                b'[' => return self.single(Tok::LBracket),
                b']' => return self.single(Tok::RBracket),
                b':' => return self.single(Tok::Colon),
                b',' => return self.single(Tok::Comma),
                b'n' if self.src[self.cursor..].starts_with(b"null") => {
                    self.cursor += 4;
                    return Tok::Null;
                }
                b't' if self.src[self.cursor..].starts_with(b"true") => {
                    self.cursor += 4;
                    return Tok::True;
                }
                b'f' if self.src[self.cursor..].starts_with(b"false") => {
                    self.cursor += 5;
                    return Tok::False;
                }
                b'-' | b'0'..=b'9' => {
                    if let Some(t) = self.scan_number() {
                        return t;
                    }
                    // A lone `-` is matched by the `UTF8` rule.
                    return self.error(ErrorCode::Syntax);
                }
                b'\n' => {
                    self.cursor += 1;
                    self.line += 1;
                    self.line_start = self.cursor;
                }
                c if is_ws(c) => {
                    self.cursor += 1;
                }
                b'"' => {
                    self.cursor += 1;
                    return match self.scan_string() {
                        Ok(s) => Tok::Str(s),
                        Err(code) => self.error(code),
                    };
                }
                // `<JS>CTRL` (a NUL byte inside the input is not the terminator).
                0x00..=0x1F => return self.error(ErrorCode::CtrlChar),
                // `<JS>UTF8_1`.
                0x20..=0x7F => return self.error(ErrorCode::Syntax),
                _ => {
                    return if utf8_sequence_len(&self.src[self.cursor..]).is_some() {
                        self.error(ErrorCode::Syntax)
                    } else {
                        self.error(ErrorCode::Utf8)
                    };
                }
            }
        }
    }

    fn single(&mut self, t: Tok) -> Tok {
        self.cursor += 1;
        t
    }

    /// `INT | FLOAT | EXP` with longest-match semantics. `None` if no number
    /// starts here (a `-` that is not followed by a digit).
    fn scan_number(&mut self) -> Option<Tok> {
        let s = self.src;
        let start = self.cursor;
        let digit = |i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
        let mut i = start;
        let negative = s[i] == b'-';
        if negative {
            i += 1;
        }
        if !digit(i) {
            return None;
        }
        if s[i] == b'0' {
            i += 1;
        } else {
            while digit(i) {
                i += 1;
            }
        }
        let mut end = i;
        let mut is_float = false;
        // FLOAT = INT "." DIGIT+
        if s.get(i) == Some(&b'.') && digit(i + 1) {
            i += 1;
            while digit(i) {
                i += 1;
            }
            end = i;
            is_float = true;
        }
        // EXP = (INT | FLOAT) [eE] [+-]? DIGIT+
        if matches!(s.get(i), Some(b'e' | b'E')) {
            let mut k = i + 1;
            if matches!(s.get(k), Some(b'+' | b'-')) {
                k += 1;
            }
            if digit(k) {
                while digit(k) {
                    k += 1;
                }
                end = k;
                is_float = true;
            }
        }
        self.cursor = end;
        let text = &s[start..end];
        if !is_float {
            let digits = text.len() - negative as usize;
            const INT_MAX_LENGTH: usize = 19; // MAX_LENGTH_OF_LONG - 1
            const LONG_MIN_DIGITS: &[u8] = b"9223372036854775808";
            let bigint = if digits < INT_MAX_LENGTH {
                false
            } else if digits == INT_MAX_LENGTH {
                match text[negative as usize..].cmp(LONG_MIN_DIGITS) {
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => !negative,
                    std::cmp::Ordering::Greater => true,
                }
            } else {
                true
            };
            if !bigint {
                let n = std::str::from_utf8(text)
                    .ok()
                    .and_then(|t| t.parse::<i64>().ok())
                    .unwrap_or(0);
                return Some(Tok::Int(n));
            }
            if self.options & BIGINT_AS_STRING != 0 {
                return Some(Tok::Str(text.to_vec()));
            }
        }
        let d = std::str::from_utf8(text)
            .ok()
            .and_then(|t| t.parse::<f64>().ok())
            .unwrap_or(0.0);
        Some(Tok::Double(d))
    }

    /// The `STR_P1`/`STR_P2` states: scans a string body (the opening quote is
    /// already consumed) and returns its decoded bytes.
    fn scan_string(&mut self) -> Result<Vec<u8>, ErrorCode> {
        let s = self.src;
        let mut out: Vec<u8> = Vec::new();
        let mut invalid_count: i64 = 0;
        loop {
            let Some(&c) = s.get(self.cursor) else {
                // EOI with `limit < cursor`.
                self.cursor = s.len() + 1;
                return Err(ErrorCode::Syntax);
            };
            match c {
                0x00..=0x1F => return Err(ErrorCode::CtrlChar),
                b'"' => {
                    self.cursor += 1;
                    return Ok(out);
                }
                b'\\' => self.scan_escape(&mut out)?,
                0x20..=0x7F => {
                    if self.build {
                        out.push(c);
                    }
                    self.cursor += 1;
                }
                _ => match utf8_sequence_len(&s[self.cursor..]) {
                    Some(n) => {
                        if self.build {
                            out.extend_from_slice(&s[self.cursor..self.cursor + n]);
                        }
                        self.cursor += n;
                    }
                    None => {
                        if self.options & (INVALID_UTF8_IGNORE | INVALID_UTF8_SUBSTITUTE) == 0 {
                            return Err(ErrorCode::Utf8);
                        }
                        if self.options & INVALID_UTF8_SUBSTITUTE != 0 {
                            if invalid_count > i32::MAX as i64 - 2 {
                                return Err(ErrorCode::Utf8);
                            }
                            invalid_count += 2;
                            if self.build {
                                out.extend_from_slice("\u{fffd}".as_bytes());
                            }
                        }
                        self.cursor += 1;
                    }
                },
            }
        }
    }

    /// An escape sequence at `\`: `UTF16_1..4`, `UCS2`, `ESC` and `ESCPREF`.
    fn scan_escape(&mut self, out: &mut Vec<u8>) -> Result<(), ErrorCode> {
        let s = self.src;
        let at = self.cursor;
        match s.get(at + 1) {
            Some(b'u') => {
                let Some(first) = hex4(s, at + 2) else {
                    // `\u` without four hex digits: only `ESCPREF` matches.
                    return Err(ErrorCode::Syntax);
                };
                // UTF16_4: a high surrogate escape directly followed by a low one.
                let low = (0xD800..=0xDBFF)
                    .contains(&first)
                    .then(|| s.get(at + 6..at + 8).filter(|b| *b == b"\\u"))
                    .flatten()
                    .and_then(|_| hex4(s, at + 8))
                    .filter(|second| (0xDC00..=0xDFFF).contains(second));
                if let Some(second) = low {
                    let cp = 0x10000 + ((first & 0x3FF) << 10) + (second & 0x3FF);
                    if self.build {
                        push_utf8(out, cp);
                    }
                    self.cursor = at + 12;
                    return Ok(());
                }
                if (0xD800..=0xDFFF).contains(&first) {
                    // UCS2: a lone surrogate.
                    return Err(ErrorCode::Utf16);
                }
                if self.build {
                    push_utf8(out, first);
                }
                self.cursor = at + 6;
                Ok(())
            }
            Some(&e) => {
                let b = match e {
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'\\' | b'/' | b'"' => e,
                    _ => return Err(ErrorCode::Syntax),
                };
                if self.build {
                    out.push(b);
                }
                self.cursor = at + 2;
                Ok(())
            }
            None => Err(ErrorCode::Syntax),
        }
    }
}

/// Encodes a code point (never a surrogate here) as UTF-8 like the scanner's
/// hand-written conversions.
fn push_utf8(out: &mut Vec<u8>, cp: u32) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp < 0x800 {
        out.push((0xC0 | (cp >> 6)) as u8);
        out.push((0x80 | (cp & 0x3F)) as u8);
    } else if cp < 0x10000 {
        out.push((0xE0 | (cp >> 12)) as u8);
        out.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
        out.push((0x80 | (cp & 0x3F)) as u8);
    } else {
        out.push((0xF0 | (cp >> 18)) as u8);
        out.push((0x80 | ((cp >> 12) & 0x3F)) as u8);
        out.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
        out.push((0x80 | (cp & 0x3F)) as u8);
    }
}
