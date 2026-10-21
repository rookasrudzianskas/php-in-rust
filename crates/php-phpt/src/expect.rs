//! Output normalisation and expectation matching.
//!
//! * [`normalize_output`] / [`normalize_expected`] mirror the `trim()` +
//!   CRLF handling of run-tests.php.
//! * [`expectf_to_pattern`] is the port of `expectf_to_regex()`.
//! * [`pcre_to_rust`] translates the PCRE dialect used inside `%r...%r`
//!   blocks and `--EXPECTREGEX--` sections into the syntax of the Rust
//!   `regex` crate.  PHP matches these patterns without the `u` flag, i.e.
//!   byte-wise, so everything is compiled as a non-Unicode *bytes* regex and
//!   every non-ASCII byte is emitted as a `\xHH` escape.
//!
//! PCRE features that cannot be expressed by a linear-time regex engine
//! (lookaround, backreferences, recursion, ...) are reported as a
//! [`PatternError`]; across the php-src corpus only one out-of-scope test
//! (`ext/zip/tests/bug76524.phpt`) needs one.

use std::fmt;

use regex::bytes::{Regex, RegexBuilder};

/// A pattern the runner cannot translate or compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PatternError {}

fn err<T>(msg: impl Into<String>) -> Result<T, PatternError> {
    Err(PatternError(msg.into()))
}

// ---------------------------------------------------------------------------
// Normalisation
// ---------------------------------------------------------------------------

/// PHP's `trim()` on bytes: strips space, `\t`, `\n`, `\r`, NUL and `\x0B`.
pub fn php_trim(data: &[u8]) -> &[u8] {
    const WS: &[u8] = b" \t\n\r\0\x0B";
    let start = data
        .iter()
        .position(|b| !WS.contains(b))
        .unwrap_or(data.len());
    let end = data
        .iter()
        .rposition(|b| !WS.contains(b))
        .map_or(start, |p| p + 1);
    &data[start..end]
}

/// `preg_replace('/\r\n/', "\n", ...)`.
pub fn crlf_to_lf(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
            out.push(b'\n');
            i += 2;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// Normalise the actual output: `preg_replace("/\r\n/", "\n", trim($out))`.
pub fn normalize_output(raw: &[u8]) -> Vec<u8> {
    crlf_to_lf(php_trim(raw))
}

/// Normalise an expectation section: trim, then CRLF to LF.
pub fn normalize_expected(section: &[u8]) -> Vec<u8> {
    crlf_to_lf(php_trim(section))
}

// ---------------------------------------------------------------------------
// EXPECTF -> pattern
// ---------------------------------------------------------------------------

/// Replacement for a `%X` placeholder, in Rust-regex syntax.
fn placeholder(c: u8) -> Option<&'static str> {
    Some(match c {
        b'e' => r"\x2F",
        b's' => r"[^\r\n]+",
        b'S' => r"[^\r\n]*",
        b'a' => r".+?",
        b'A' => r".*?",
        b'w' => r"\s*",
        b'i' => r"[+-]?\d+",
        b'd' => r"\d+",
        b'x' => r"[0-9a-fA-F]+",
        // run-tests: [+-]?(?:\d+|(?=\.\d))(?:\.\d+)?(?:[Ee][+-]?\d+)?
        // rewritten without the lookahead.
        b'f' => r"[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:[Ee][+-]?\d+)?",
        b'c' => r".",
        b'0' => r"\x00",
        _ => return None,
    })
}

fn push_hex(out: &mut String, b: u8) {
    use std::fmt::Write;
    let _ = write!(out, "\\x{b:02X}");
}

/// Append a literal run, quoting every byte and expanding `%X` placeholders.
fn push_literal(out: &mut String, text: &[u8]) {
    let mut i = 0;
    while i < text.len() {
        let b = text[i];
        if b == b'%'
            && let Some(rep) = text.get(i + 1).and_then(|c| placeholder(*c))
        {
            out.push_str(rep);
            i += 2;
            continue;
        }
        if b.is_ascii_alphanumeric() || b == b'_' {
            out.push(b as char);
        } else {
            push_hex(out, b);
        }
        i += 1;
    }
}

/// Expand `%X` placeholders in raw regex text (the `strtr()` of run-tests).
fn expand_placeholders(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'%'
            && let Some(rep) = text.get(i + 1).and_then(|c| placeholder(*c))
        {
            out.extend_from_slice(rep.as_bytes());
            i += 2;
            continue;
        }
        out.push(text[i]);
        i += 1;
    }
    out
}

fn find_sub(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Port of `expectf_to_regex()`.
///
/// Returns the body of a Rust regex (no anchors): literal text is quoted,
/// `%X` placeholders are expanded and `%r...%r` sections are embedded as
/// capture groups after PCRE translation.
pub fn expectf_to_pattern(wanted: &[u8]) -> Result<String, PatternError> {
    let wanted = crlf_to_lf(wanted);
    let len = wanted.len();
    let mut out = String::new();
    let mut offset = 0;
    while offset < len {
        let (start, end) = match find_sub(&wanted, b"%r", offset) {
            // Found a start tag; look for the matching end tag.
            Some(start) => match find_sub(&wanted, b"%r", start + 2) {
                Some(end) => (start, end),
                // Unbalanced tag: ignore it.
                None => (len, len),
            },
            None => (len, len),
        };
        push_literal(&mut out, &wanted[offset..start]);
        if end > start {
            let inner = expand_placeholders(&wanted[start + 2..end]);
            out.push('(');
            out.push_str(&pcre_to_rust(&inner)?);
            out.push(')');
        }
        offset = end + 2;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// A compiled `^...$` matcher for one expectation.
#[derive(Debug, Clone)]
pub struct Matcher {
    re: Regex,
}

fn build_regex(body: &str, anchored: bool) -> Result<Regex, PatternError> {
    let full = if anchored {
        format!("^{body}$")
    } else {
        body.to_string()
    };
    RegexBuilder::new(&full)
        .unicode(false)
        .dot_matches_new_line(true)
        .size_limit(512 << 20)
        .dfa_size_limit(256 << 20)
        .nest_limit(1000)
        .build()
        .map_err(|e| PatternError(format!("regex compile error: {e}")))
}

impl Matcher {
    /// Matcher for an `--EXPECTF--` section (`wanted` is already trimmed).
    pub fn expectf(wanted: &[u8]) -> Result<Self, PatternError> {
        Ok(Self {
            re: build_regex(&expectf_to_pattern(wanted)?, true)?,
        })
    }

    /// Matcher for an `--EXPECTREGEX--` section: the text is used verbatim as
    /// a PCRE pattern (`'/^' . $wanted_re . '$/s'`, so top-level alternation
    /// binds looser than the anchors, exactly like in PHP).
    pub fn regex(wanted: &[u8]) -> Result<Self, PatternError> {
        let wanted = crlf_to_lf(wanted);
        Ok(Self {
            re: build_regex(&pcre_to_rust(&wanted)?, true)?,
        })
    }

    /// Does the (normalised) output match?
    pub fn is_match(&self, output: &[u8]) -> bool {
        self.re.is_match(output)
    }
}

/// Compile a free-standing PCRE fragment (used by tests and tooling).
pub fn compile_pcre(pattern: &[u8]) -> Result<Regex, PatternError> {
    build_regex(&pcre_to_rust(pattern)?, false)
}

// ---------------------------------------------------------------------------
// PCRE -> Rust regex translation
// ---------------------------------------------------------------------------

/// Translate a PCRE (non-UTF mode) pattern to the Rust `regex` dialect.
pub fn pcre_to_rust(pattern: &[u8]) -> Result<String, PatternError> {
    let mut t = Translator {
        src: pattern,
        pos: 0,
        out: String::with_capacity(pattern.len() * 2),
    };
    t.run()?;
    Ok(t.out)
}

/// Result of parsing one backslash escape.
enum Escape {
    /// A single literal byte.
    Byte(u8),
    /// A construct that is already valid Rust syntax outside a class
    /// (`\d`, `\b`, `[...]`, `(?:...)`).
    Raw(String),
    /// A set usable both inside and outside a class: the body without brackets.
    Set(String),
    /// Nothing to emit (stray `\E`).
    Nothing,
}

#[derive(Debug)]
enum ClassItem {
    Char(u8),
    Set(String),
    Dash,
}

struct Translator<'a> {
    src: &'a [u8],
    pos: usize,
    out: String,
}

impl Translator<'_> {
    fn peek(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn run(&mut self) -> Result<(), PatternError> {
        while let Some(b) = self.peek(0) {
            match b {
                b'\\' => self.escape_outside()?,
                b'[' => self.class()?,
                b'(' => self.group()?,
                b'{' => self.brace(),
                b'}' => {
                    self.out.push_str("\\}");
                    self.pos += 1;
                }
                b'*' | b'+' | b'?' => {
                    self.out.push(b as char);
                    self.pos += 1;
                    self.quantifier_suffix();
                }
                b'.' | b'^' | b'$' | b'|' | b')' => {
                    self.out.push(b as char);
                    self.pos += 1;
                }
                0x20..=0x7E => {
                    self.out.push(b as char);
                    self.pos += 1;
                }
                _ => {
                    push_hex(&mut self.out, b);
                    self.pos += 1;
                }
            }
        }
        Ok(())
    }

    /// After a quantifier: drop a possessive `+`, keep a lazy `?`.
    fn quantifier_suffix(&mut self) {
        match self.peek(0) {
            Some(b'+') => self.pos += 1,
            Some(b'?') => {
                self.out.push('?');
                self.pos += 1;
            }
            _ => {}
        }
    }

    /// `{n}`, `{n,}`, `{n,m}` are quantifiers; any other `{` is a literal.
    fn brace(&mut self) {
        let rest = &self.src[self.pos..];
        let mut i = 1;
        let digits = |i: &mut usize| {
            let start = *i;
            while rest.get(*i).is_some_and(u8::is_ascii_digit) {
                *i += 1;
            }
            *i > start
        };
        let valid = digits(&mut i) && {
            if rest.get(i) == Some(&b',') {
                i += 1;
                digits(&mut i);
            }
            rest.get(i) == Some(&b'}')
        };
        if valid {
            let text = &rest[..=i];
            // Digits and punctuation only, so this is valid UTF-8.
            self.out.push_str(&String::from_utf8_lossy(text));
            self.pos += i + 1;
            self.quantifier_suffix();
        } else {
            self.out.push_str("\\{");
            self.pos += 1;
        }
    }

    fn group(&mut self) -> Result<(), PatternError> {
        // self.src[self.pos] == b'('
        if self.peek(1) == Some(b'*') {
            return err("unsupported PCRE feature: backtracking control verb");
        }
        if self.peek(1) != Some(b'?') {
            self.out.push('(');
            self.pos += 1;
            return Ok(());
        }
        let c = self.peek(2);
        match c {
            Some(b'#') => {
                // Comment: skip to the closing parenthesis.
                match self.src[self.pos..].iter().position(|b| *b == b')') {
                    Some(p) => self.pos += p + 1,
                    None => return err("missing ) after (?# comment"),
                }
            }
            Some(b':') => {
                self.out.push_str("(?:");
                self.pos += 3;
            }
            // Atomic group: approximate with a plain group.
            Some(b'>') => {
                self.out.push_str("(?:");
                self.pos += 3;
            }
            Some(b'=' | b'!') => return err("unsupported PCRE feature: lookahead assertion"),
            Some(b'<') => match self.peek(3) {
                Some(b'=' | b'!') => return err("unsupported PCRE feature: lookbehind assertion"),
                _ => {
                    self.pos += 3;
                    self.named_group(b'>')?;
                }
            },
            Some(b'P') => match self.peek(3) {
                Some(b'<') => {
                    self.pos += 4;
                    self.named_group(b'>')?;
                }
                _ => return err("unsupported PCRE feature: named backreference or recursion"),
            },
            Some(b'\'') => {
                self.pos += 3;
                self.named_group(b'\'')?;
            }
            Some(b'|') => return err("unsupported PCRE feature: branch reset group"),
            Some(b'(') => return err("unsupported PCRE feature: conditional group"),
            Some(b'R' | b'&' | b'0'..=b'9' | b'+') => {
                return err("unsupported PCRE feature: recursion or subroutine call");
            }
            Some(b'^') => return err("unsupported PCRE feature: (?^ flag reset"),
            Some(b'-') if self.peek(3).is_some_and(|d| d.is_ascii_digit()) => {
                return err("unsupported PCRE feature: relative subroutine call");
            }
            Some(_) => self.flag_group()?,
            None => return err("unterminated group"),
        }
        Ok(())
    }

    /// `self.pos` is just after the opening delimiter of the group name.
    fn named_group(&mut self, close: u8) -> Result<(), PatternError> {
        let start = self.pos;
        while self.peek(0).is_some_and(|b| b != close) {
            self.pos += 1;
        }
        if self.peek(0) != Some(close) {
            return err("unterminated group name");
        }
        let name = &self.src[start..self.pos];
        if name.is_empty() || !name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            return err("unsupported group name");
        }
        self.pos += 1;
        self.out.push_str("(?P<");
        self.out.push_str(&String::from_utf8_lossy(name));
        self.out.push('>');
        Ok(())
    }

    /// `(?flags)` or `(?flags:`.
    fn flag_group(&mut self) -> Result<(), PatternError> {
        let start = self.pos + 2;
        let mut i = start;
        while self
            .src
            .get(i)
            .is_some_and(|b| matches!(b, b'i' | b'm' | b's' | b'x' | b'U' | b'-' | b'J' | b'X'))
        {
            i += 1;
        }
        let close = self.src.get(i).copied();
        if i == start || !matches!(close, Some(b')' | b':')) {
            return err("unsupported PCRE group syntax");
        }
        // J and X have no equivalent (and no effect on matching here): drop.
        let flags: String = self.src[start..i]
            .iter()
            .filter(|b| !matches!(b, b'J' | b'X'))
            .map(|b| *b as char)
            .collect();
        let flags = flags.trim_end_matches('-').to_string();
        let close = close.unwrap_or(b')') as char;
        if flags.is_empty() {
            // Nothing left of the flag group: `(?)` is invalid, `(?:` is fine.
            if close == ':' {
                self.out.push_str("(?:");
            }
        } else {
            self.out.push_str("(?");
            self.out.push_str(&flags);
            self.out.push(close);
        }
        self.pos = i + 1;
        Ok(())
    }

    fn escape_outside(&mut self) -> Result<(), PatternError> {
        if self.peek(1) == Some(b'Q') {
            self.pos += 2;
            while self.pos < self.src.len() {
                if self.src[self.pos..].starts_with(b"\\E") {
                    self.pos += 2;
                    return Ok(());
                }
                push_hex(&mut self.out, self.src[self.pos]);
                self.pos += 1;
            }
            return Ok(());
        }
        match self.parse_escape(false)? {
            Escape::Byte(b) => push_hex(&mut self.out, b),
            Escape::Raw(s) => self.out.push_str(&s),
            Escape::Set(s) => {
                self.out.push('[');
                self.out.push_str(&s);
                self.out.push(']');
            }
            Escape::Nothing => {}
        }
        Ok(())
    }

    /// Parse the escape at `self.pos` (which points at the backslash).
    fn parse_escape(&mut self, in_class: bool) -> Result<Escape, PatternError> {
        let Some(c) = self.peek(1) else {
            return err("\\ at end of pattern");
        };
        self.pos += 2;
        let set = |s: &str| Escape::Set(s.to_string());
        Ok(match c {
            b'd' | b'D' | b's' | b'S' | b'w' | b'W' => Escape::Raw(format!("\\{}", c as char)),
            b'n' => Escape::Byte(b'\n'),
            b'r' => Escape::Byte(b'\r'),
            b't' => Escape::Byte(b'\t'),
            b'f' => Escape::Byte(0x0C),
            b'a' => Escape::Byte(0x07),
            b'e' => Escape::Byte(0x1B),
            b'h' => set(r"\x20\x09\xA0"),
            b'v' => set(r"\x0A\x0B\x0C\x0D\x85"),
            // Nested negated classes are a union inside a Rust class.
            b'H' => Escape::Raw(r"[^\x20\x09\xA0]".to_string()),
            b'V' => Escape::Raw(r"[^\x0A\x0B\x0C\x0D\x85]".to_string()),
            b'N' if !in_class => Escape::Raw(r"[^\n]".to_string()),
            b'R' if !in_class => Escape::Raw(r"(?:\r\n|[\n\x0B\x0C\r\x85])".to_string()),
            b'b' if in_class => Escape::Byte(0x08),
            b'b' | b'B' if !in_class => Escape::Raw(format!("\\{}", c as char)),
            b'A' if !in_class => Escape::Raw(r"\A".to_string()),
            b'z' if !in_class => Escape::Raw(r"\z".to_string()),
            b'Z' if !in_class => Escape::Raw(r"(?:\n?\z)".to_string()),
            b'E' => Escape::Nothing,
            b'x' => Escape::Byte(self.hex_escape()?),
            b'c' => {
                let Some(ch) = self.peek(0).filter(u8::is_ascii) else {
                    return err("\\c must be followed by an ASCII character");
                };
                self.pos += 1;
                Escape::Byte(ch.to_ascii_uppercase() ^ 0x40)
            }
            b'0' => Escape::Byte(self.octal_escape(0, 2)),
            b'1'..=b'7' if in_class => Escape::Byte(self.octal_escape(c - b'0', 2)),
            // `\12`..`\77`: PCRE reads these as octal unless that many groups
            // exist; without backreference support octal is the only option.
            b'1'..=b'7' if self.peek(0).is_some_and(|d| (b'0'..=b'7').contains(&d)) => {
                Escape::Byte(self.octal_escape(c - b'0', 2))
            }
            b'1'..=b'9' => return err("unsupported PCRE feature: backreference"),
            b'p' | b'P' | b'X' | b'C' | b'G' | b'K' | b'g' | b'k' | b'o' | b'u' | b'L' | b'l'
            | b'U' => {
                return err(format!("unsupported PCRE escape \\{}", c as char));
            }
            c if c.is_ascii_alphanumeric() => {
                return err(format!("unrecognized PCRE escape \\{}", c as char));
            }
            // Escaped punctuation, whitespace or a high byte: a literal.
            c => Escape::Byte(c),
        })
    }

    /// `\xHH`, `\xH` or `\x{H...}` (values above 0xFF need UTF mode).
    fn hex_escape(&mut self) -> Result<u8, PatternError> {
        if self.peek(0) == Some(b'{') {
            let close = self.src[self.pos..].iter().position(|b| *b == b'}');
            let Some(close) = close else {
                return err("missing } after \\x{");
            };
            let digits = &self.src[self.pos + 1..self.pos + close];
            let text = String::from_utf8_lossy(digits);
            let value = u32::from_str_radix(&text, 16)
                .map_err(|_| PatternError(format!("invalid \\x{{{text}}} escape")))?;
            self.pos += close + 1;
            return u8::try_from(value)
                .map_err(|_| PatternError("character code above 255 needs UTF mode".to_string()));
        }
        let mut value = 0u32;
        for _ in 0..2 {
            match self.peek(0).and_then(|b| (b as char).to_digit(16)) {
                Some(d) => {
                    value = value * 16 + d;
                    self.pos += 1;
                }
                None => break,
            }
        }
        Ok(value as u8)
    }

    /// Up to `more` further octal digits after the initial value.
    fn octal_escape(&mut self, initial: u8, more: usize) -> u8 {
        let mut value = u32::from(initial);
        for _ in 0..more {
            match self.peek(0) {
                Some(d @ b'0'..=b'7') => {
                    value = value * 8 + u32::from(d - b'0');
                    self.pos += 1;
                }
                _ => break,
            }
        }
        (value & 0xFF) as u8
    }

    /// Translate a bracketed class. `self.pos` points at `[`.
    fn class(&mut self) -> Result<(), PatternError> {
        self.pos += 1;
        let negate = self.peek(0) == Some(b'^');
        if negate {
            self.pos += 1;
        }
        let mut items: Vec<ClassItem> = Vec::new();
        let mut at_start = true;
        loop {
            let Some(b) = self.peek(0) else {
                return err("missing terminating ] for character class");
            };
            match b {
                b']' if !at_start => {
                    self.pos += 1;
                    break;
                }
                b'[' if self.peek(1) == Some(b':') => match self.posix_class() {
                    Some(set) => items.push(ClassItem::Set(set)),
                    None => {
                        items.push(ClassItem::Char(b'['));
                        self.pos += 1;
                    }
                },
                b'\\' => match self.parse_escape(true)? {
                    Escape::Byte(b) => items.push(ClassItem::Char(b)),
                    Escape::Raw(s) => items.push(ClassItem::Set(s)),
                    Escape::Set(s) => items.push(ClassItem::Set(s)),
                    Escape::Nothing => {}
                },
                b'-' => {
                    items.push(ClassItem::Dash);
                    self.pos += 1;
                }
                _ => {
                    items.push(ClassItem::Char(b));
                    self.pos += 1;
                }
            }
            at_start = false;
        }

        self.out.push('[');
        if negate {
            self.out.push('^');
        }
        let mut i = 0;
        while i < items.len() {
            match &items[i] {
                ClassItem::Char(a) => {
                    push_hex(&mut self.out, *a);
                    if let (Some(ClassItem::Dash), Some(ClassItem::Char(b))) =
                        (items.get(i + 1), items.get(i + 2))
                    {
                        self.out.push('-');
                        push_hex(&mut self.out, *b);
                        i += 3;
                        continue;
                    }
                }
                ClassItem::Set(s) => self.out.push_str(s),
                ClassItem::Dash => self.out.push_str("\\x2D"),
            }
            i += 1;
        }
        self.out.push(']');
        Ok(())
    }

    /// `[:name:]` / `[:^name:]` inside a class; advances past it on success.
    fn posix_class(&mut self) -> Option<String> {
        let rest = &self.src[self.pos + 2..];
        let end = rest.windows(2).position(|w| w == b":]")?;
        let name = &rest[..end];
        let body = name.strip_prefix(b"^").unwrap_or(name);
        if body.is_empty() || !body.iter().all(u8::is_ascii_lowercase) {
            return None;
        }
        let text = String::from_utf8_lossy(name).into_owned();
        self.pos += 2 + end + 2;
        Some(format!("[:{text}:]"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expectf(wanted: &str, output: &str) -> bool {
        Matcher::expectf(wanted.as_bytes())
            .unwrap()
            .is_match(output.as_bytes())
    }

    fn regex(wanted: &str, output: &str) -> bool {
        Matcher::regex(wanted.as_bytes())
            .unwrap()
            .is_match(output.as_bytes())
    }

    // ---- normalisation ----------------------------------------------------

    #[test]
    fn trim_matches_php() {
        assert_eq!(php_trim(b" \t\r\n x \n\0\x0B"), b"x");
        assert_eq!(php_trim(b""), b"");
        assert_eq!(php_trim(b" \n "), b"");
        assert_eq!(php_trim(b"\xff a \xfe"), b"\xff a \xfe");
    }

    #[test]
    fn crlf_normalisation() {
        assert_eq!(crlf_to_lf(b"a\r\nb\r\n"), b"a\nb\n");
        // A lone CR is preserved, as is the CR of CR CR LF.
        assert_eq!(crlf_to_lf(b"a\rb"), b"a\rb");
        assert_eq!(crlf_to_lf(b"a\r\r\nb"), b"a\r\nb");
        assert_eq!(
            normalize_output(b"\n  out\r\nline2 \r\n\r\n"),
            b"out\nline2"
        );
        assert_eq!(normalize_expected(b"\r\nexp\r\n"), b"exp");
    }

    // ---- EXPECTF placeholders ---------------------------------------------

    #[test]
    fn s_on_line_d() {
        let w = "Warning: foo() in %s on line %d";
        assert!(expectf(w, "Warning: foo() in /tmp/x/test.php on line 12"));
        assert!(!expectf(w, "Warning: foo() in  on line 12"));
        assert!(!expectf(w, "Warning: foo() in /tmp/x.php on line x"));
        assert!(!expectf(
            w,
            "Warning: foo() in /tmp/x.php on line 12\nextra"
        ));
    }

    #[test]
    fn s_does_not_cross_lines_but_a_does() {
        assert!(!expectf("a%sb", "a\nb"));
        assert!(expectf("a%ab", "a\nxx\nb"));
        assert!(expectf("a%Ab", "ab"));
        assert!(!expectf("a%ab", "ab"));
        assert!(expectf("a%Sb", "ab"));
        assert!(!expectf("a%sb", "ab"));
    }

    #[test]
    fn numeric_placeholders() {
        assert!(expectf("%d", "0123"));
        assert!(!expectf("%d", "-1"));
        assert!(expectf("%i", "-12"));
        assert!(expectf("%i", "+12"));
        assert!(expectf("%x", "dEaD01"));
        assert!(!expectf("%x", "xyz"));
        for ok in ["1", "1.5", "-1.5e10", ".5", "+3E-2", "10"] {
            assert!(expectf("%f", ok), "{ok}");
        }
        for bad in ["", ".", "e5", "1.", "--1"] {
            assert!(!expectf("%f", bad), "{bad}");
        }
    }

    #[test]
    fn misc_placeholders() {
        assert!(expectf("a%wb", "a \t\n b"));
        assert!(expectf("a%wb", "ab"));
        assert!(expectf("%c%c", "xy"));
        assert!(!expectf("%c", "xy"));
        assert!(expectf("a%eb", "a/b"));
        assert!(!expectf("a%eb", "a\\b"));
        assert!(expectf("a%0b", "a\0b"));
    }

    #[test]
    fn unknown_percent_sequences_are_literal() {
        assert!(expectf("100%", "100%"));
        assert!(expectf("%z %% %", "%z %% %"));
        assert!(expectf("%%s", "%x"), "%% leaves the second % to start %s");
        assert!(!expectf("%%s", "%\n"));
    }

    #[test]
    fn regex_metacharacters_are_quoted() {
        let w = r"a.b*c+d?e[f]g(h)i{2}j|k^l$m\n/n-o#p:q<r>s=t!u";
        assert!(expectf(w, w));
        assert!(!expectf("a.c", "abc"));
        assert!(!expectf("a+", "aa"));
        assert!(!expectf("(a)", "a"));
        assert!(expectf("(a)", "(a)"));
        assert!(!expectf("a|b", "a"));
        assert!(expectf("<>", "<>"), "angle brackets must stay literal");
        assert!(expectf(r"\d", r"\d"), "backslash is literal in EXPECTF");
    }

    #[test]
    fn whole_output_must_match() {
        assert!(!expectf("abc", "xabc"));
        assert!(!expectf("abc", "abcx"));
        assert!(expectf("abc", "abc"));
        assert!(expectf("", ""));
        assert!(!expectf("", "x"));
    }

    #[test]
    fn multiline_expectations() {
        let w = "int(%d)\nstring(%d) \"%s\"\nbool(true)";
        assert!(expectf(w, "int(5)\nstring(3) \"abc\"\nbool(true)"));
        assert!(!expectf(w, "int(5)\nstring(3) \"abc\"\nbool(false)"));
        // CRLF in the template is normalised.
        assert!(expectf("a%d\r\nb", "a1\nb"));
    }

    // ---- %r blocks --------------------------------------------------------

    #[test]
    fn r_blocks_are_raw_regex() {
        assert!(expectf("a%r[0-9]{2,3}%rb", "a123b"));
        assert!(!expectf("a%r[0-9]{2,3}%rb", "a1b"));
        assert!(expectf("%r(foo|bar)%r!", "bar!"));
        assert!(expectf("v%r\\d+(\\.\\d+)*%r", "v8.4.1"));
    }

    #[test]
    fn r_block_contents_are_not_quoted_but_text_around_is() {
        assert!(expectf("1.2%r.%r3", "1.2x3"));
        assert!(!expectf("1.2%r\\.%r3", "1.2x3"));
        assert!(expectf("1.2%r\\.%r3", "1.2.3"));
        // Outside the block the dot is quoted.
        assert!(!expectf("a.b%r.%r", "axbc"));
    }

    #[test]
    fn multiple_and_empty_r_blocks() {
        assert!(expectf("%r\\d%r-%r\\d%r", "1-2"));
        assert!(expectf("a%r%rb", "ab"));
    }

    #[test]
    fn unbalanced_r_is_literal() {
        assert!(expectf("a%rb", "a%rb"));
        assert!(expectf("x%r[0-9]%ry%rz", "x5y%rz"));
    }

    #[test]
    fn placeholders_inside_r_blocks_expand() {
        assert!(expectf("%r(%d|none)%r", "none"));
        assert!(expectf("%r(%d|none)%r", "42"));
    }

    #[test]
    fn r_block_with_lookahead_is_reported_unsupported() {
        let e = Matcher::expectf(b"%r(?=a)a%r").unwrap_err();
        assert!(e.0.contains("lookahead"), "{e}");
    }

    // ---- bytes -------------------------------------------------------------

    #[test]
    fn non_utf8_bytes_match_byte_wise() {
        let m = Matcher::expectf(b"a\xff%sb\xe9").unwrap();
        assert!(m.is_match(b"a\xffXb\xe9"));
        assert!(!m.is_match(b"a\xfeXb\xe9"));
        let m = Matcher::expectf(b"%a").unwrap();
        assert!(m.is_match(b"\xff\xfe\x00\xc3\x28"));
        let m = Matcher::expectf("é%d".as_bytes()).unwrap();
        assert!(m.is_match("é1".as_bytes()));
        assert!(!m.is_match("e1".as_bytes()));
    }

    #[test]
    fn dot_matches_a_single_byte_not_a_codepoint() {
        assert!(expectf("%c", "a"));
        let m = Matcher::expectf(b"%c").unwrap();
        assert!(m.is_match(b"\xe9"));
        assert!(!m.is_match("é".as_bytes()), "é is two bytes in PHP");
    }

    // ---- EXPECTREGEX -------------------------------------------------------

    #[test]
    fn expectregex_is_anchored_and_dotall() {
        assert!(regex("a.c", "a\nc"));
        assert!(!regex("a", "ab"));
        assert!(regex("a\\d+z", "a12z"));
        assert!(regex(
            "Fatal error: .* in %s on line \\d+",
            "Fatal error: x\ny in %s on line 3"
        ));
    }

    #[test]
    fn expectregex_top_level_alternation_binds_loosest() {
        // PHP builds '/^a|b$/s': 'a' anchored at the start OR 'b' at the end.
        assert!(regex("a|b", "axx"));
        assert!(regex("a|b", "xxb"));
        assert!(!regex("a|b", "xxx"));
    }

    #[test]
    fn expectregex_does_not_expand_percent_placeholders() {
        assert!(regex("%d", "%d"));
        assert!(!regex("%d", "5"));
    }

    // ---- PCRE translation --------------------------------------------------

    fn translates(pat: &str) -> String {
        pcre_to_rust(pat.as_bytes()).unwrap()
    }

    fn unsupported(pat: &str) -> String {
        pcre_to_rust(pat.as_bytes()).unwrap_err().0
    }

    #[test]
    fn pcre_escapes() {
        assert!(regex(r"a\/b", "a/b"));
        assert!(regex(r"\<tag\>", "<tag>"));
        assert!(regex(r"a\-b\#c\:d\=e\!f", "a-b#c:d=e!f"));
        assert!(regex(r"\e\a", "\x1b\x07"));
        assert!(regex(r"\x41\x{42}\103\0", "ABC\0"));
        assert!(regex(r"\cA", "\x01"));
        assert!(regex(r"\Qa.b*\E+", "a.b**"));
        assert!(regex(r"\h+", " \t "));
        assert!(regex(r"\R", "\r\n"));
        assert!(regex(r"\A\w+\z", "ab_9"));
        assert!(regex(r"a\Z", "a"));
    }

    #[test]
    fn pcre_possessive_and_atomic_are_approximated() {
        assert!(regex(r"a++b", "aab"));
        assert!(regex(r"a*+b", "aab"));
        assert!(regex(r"a?+b", "ab"));
        assert!(regex(r"a{1,2}+b", "aab"));
        assert!(regex(r"(?>a+)b", "aab"));
        assert!(regex(r"a+?b", "aab"));
    }

    #[test]
    fn pcre_literal_braces() {
        assert!(regex(r"a{", "a{"));
        assert!(regex(r"a{x}", "a{x}"));
        assert!(regex(r"{", "{"));
        assert!(regex(r"a}", "a}"));
        assert!(regex(r"a{2}", "aa"));
        assert!(regex(r"a{2,}", "aaaa"));
        assert!(regex(r"a{,3}", "a{,3}"));
    }

    #[test]
    fn pcre_character_classes() {
        assert!(regex(r"[abc]+", "abcab"));
        assert!(regex(r"[^abc]", "d"));
        assert!(regex(r"[]a]+", "]a]"), "leading ] is literal");
        assert!(regex(r"[^]a]", "b"));
        assert!(regex(r"[a-c]+", "abc"));
        assert!(regex(r"[\w-]+", "ab-c_"));
        assert!(regex(r"[a\-z]+", "a-z"));
        assert!(!regex(r"[a\-z]+", "b"));
        assert!(regex(r"[-a]+", "-a"));
        assert!(regex(r"[a-]+", "a-"));
        assert!(regex(r"[[]", "["), "[ is literal inside a class");
        assert!(regex(r"[[:alpha:]]+", "abc"));
        assert!(regex(r"[[:^digit:]]+", "abc"));
        assert!(regex(r"[\d.]+", "1.5"));
        assert!(
            regex(r"[a&&b]+", "a&b"),
            "&& is not an intersection in PCRE"
        );
        assert!(regex(r"[a~~b]+", "a~b"));
        assert!(regex(r"[\h]", " "));
        assert!(regex(r"[\]]", "]"));
        assert!(regex(r"[\\]", "\\"));
        assert!(regex(r"[\x00-\x1f]", "\x07"));
        assert!(
            regex(r"[é]+", "é"),
            "bytes of a multibyte char are a byte class"
        );
    }

    #[test]
    fn pcre_groups_and_flags() {
        assert!(regex(r"(?i)abc", "ABC"));
        assert!(regex(r"(?i:a)b", "Ab"));
        assert!(!regex(r"(?i:a)b", "AB"));
        assert!(regex(r"(?P<n>a)b", "ab"));
        assert!(regex(r"(?<n>a)b", "ab"));
        assert!(regex(r"(?'n'a)b", "ab"));
        assert!(regex(r"a(?#comment)b", "ab"));
        assert!(regex(r"(?s:.)", "\n"));
        assert!(regex(r"(?x) a b ", "ab"));
        assert!(regex(r"(?-i)a", "a"));
    }

    #[test]
    fn pcre_unsupported_constructs_have_clear_errors() {
        assert!(unsupported(r"a(?=b)").contains("lookahead"));
        assert!(unsupported(r"a(?!b)").contains("lookahead"));
        assert!(unsupported(r"(?<=a)b").contains("lookbehind"));
        assert!(unsupported(r"(?<!a)b").contains("lookbehind"));
        assert!(unsupported(r"(a)\1").contains("backreference"));
        assert!(unsupported(r"(?R)").contains("recursion"));
        assert!(unsupported(r"(*SKIP)a").contains("verb"));
        assert!(unsupported(r"\p{L}").contains("\\p"));
        assert!(unsupported(r"a\").contains("end of pattern"));
        assert!(unsupported(r"[abc").contains("terminating ]"));
        assert!(unsupported(r"\y").contains("unrecognized"));
        assert!(unsupported(r"\x{100}").contains("255"));
    }

    #[test]
    fn invalid_regex_is_a_pattern_error_not_a_panic() {
        assert!(Matcher::regex(b"(unclosed").is_err());
        assert!(Matcher::regex(b"a)").is_err());
        assert!(Matcher::regex(b"a{2,1}").is_err());
    }

    #[test]
    fn translation_output_is_ascii() {
        let out = translates("é\u{1}[é-ÿ]");
        assert!(out.is_ascii(), "{out}");
    }

    #[test]
    fn large_template_compiles() {
        let mut wanted = String::new();
        let mut output = String::new();
        for i in 0..3000 {
            wanted.push_str(&format!("line {i}: %s (%d)\n"));
            output.push_str(&format!("line {i}: value (7)\n"));
        }
        assert!(expectf(wanted.trim(), output.trim()));
    }

    #[test]
    fn compile_pcre_unanchored_fragment() {
        let re = compile_pcre(br"\d+").unwrap();
        assert!(re.is_match(b"ab12"));
    }
}
