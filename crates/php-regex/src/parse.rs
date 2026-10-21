//! First compile pass: a port of PCRE2's `parse_regex()`.
//!
//! The pattern is scanned once, escapes are interpreted, comments removed and
//! capturing groups numbered and named. The output is a flat vector of
//! [`Meta`] items (PCRE2's "parsed pattern"), plus the group/name tables.
//! All syntax errors are detected here with exactly PCRE2's error codes and
//! offsets.

use crate::chars::{self, CTYPE_LCLETTER, CTYPE_SPACE, CTYPE_WORD};
use crate::error::codes::*;
use crate::opts::*;
use crate::unicode;

pub(crate) const MAX_GROUP_NUMBER: u32 = 65535;
pub(crate) const MAX_REPEAT_COUNT: u32 = 65535;
pub(crate) const REPEAT_UNLIMITED: u32 = MAX_REPEAT_COUNT + 1;
pub(crate) const MAX_NAME_SIZE: usize = 128;
pub(crate) const MAX_NAME_COUNT: usize = 10000;
pub(crate) const MAX_MARK: usize = 255;
pub(crate) const PARENS_NEST_LIMIT: u32 = 250;
/// Number of `nest_save` entries that fit in PCRE2's compile workspace.
const NEST_STACK_SIZE: usize = 375;

/// Backslash escape identifiers, in PCRE2's `ESC_xxx` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(crate) enum Esc {
    A = 1,
    G,
    K,
    UpperB,
    LowerB,
    UpperD,
    LowerD,
    UpperS,
    LowerS,
    UpperW,
    LowerW,
    N,
    C,
    UpperP,
    LowerP,
    R,
    UpperH,
    LowerH,
    UpperV,
    LowerV,
    X,
    UpperZ,
    LowerZ,
    E,
    Q,
    G2, // \g (subroutine / backref by name)
    K2, // \k
}

impl Esc {
    /// Escapes between ESC_b and ESC_Z (exclusive) consume a character.
    pub(crate) fn consumes_char(self) -> bool {
        self > Esc::LowerB && self < Esc::UpperZ
    }
}

/// Kinds of quantifier, as in META_ASTERISK etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuantMode {
    Greedy,
    Possessive,
    Lazy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LbKind {
    Pos,
    Neg,
    NonAtomic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CondNameKind {
    Name,
    RName,
    RNumber,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerbKind {
    Mark,
    Commit,
    Prune,
    Skip,
    Then,
}

/// One item of the parsed pattern.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Meta {
    Lit(u32),
    End,
    /// `|`. The data is the branch length when inside a lookbehind.
    Alt(u32),
    Ket,
    Accept,
    Fail,
    Commit,
    Prune,
    Skip,
    Then,
    /// A verb with an argument: (*MARK:x), (*COMMIT:x), (*PRUNE:x), (*SKIP:x), (*THEN:x).
    VerbArg(VerbKind, Vec<u32>),
    Atomic,
    Capture(u32),
    NoCapture,
    Lookahead,
    LookaheadNot,
    LookaheadNa,
    /// Lookbehind; `len` is the first branch's max length, `minlen` the
    /// overall minimum (LOOKBEHIND_MAX when all branches are fixed).
    Lookbehind {
        kind: LbKind,
        offset: usize,
        len: u32,
        minlen: u32,
    },
    ScriptRun,
    CondAssert,
    CondDefine {
        offset: usize,
    },
    CondName {
        kind: CondNameKind,
        offset: usize,
        len: usize,
    },
    CondNumber {
        n: u32,
        offset: usize,
    },
    CondVersion {
        ge: bool,
        major: u32,
        minor: u32,
    },
    Circumflex,
    Dollar,
    Dot,
    Class(bool),
    ClassEmpty(bool),
    ClassEnd,
    Posix {
        negated: bool,
        class: u8,
    },
    RangeLiteral,
    RangeEscaped,
    Escape(Esc),
    /// \p or \P (negated = \P).
    Prop {
        negated: bool,
        ptype: u8,
        pvalue: u16,
    },
    Backref {
        n: u32,
        offset: usize,
    },
    BackrefByName {
        offset: usize,
        len: usize,
    },
    RecurseByName {
        offset: usize,
        len: usize,
    },
    Recurse {
        n: u32,
        offset: usize,
    },
    Options {
        options: u32,
        xoptions: u32,
    },
    Asterisk(QuantMode),
    Plus(QuantMode),
    Query(QuantMode),
    MinMax(u32, u32, QuantMode),
    Callout,
}

impl Meta {
    pub(crate) fn is_quantifier(&self) -> bool {
        matches!(
            self,
            Meta::Asterisk(_) | Meta::Plus(_) | Meta::Query(_) | Meta::MinMax(..)
        )
    }
}

/// A named group found during parsing.
#[derive(Clone, Debug)]
pub(crate) struct NamedGroup {
    pub name: Vec<u8>,
    pub number: u32,
    pub isdup: bool,
}

pub(crate) struct ParseResult {
    pub parsed: Vec<Meta>,
    pub bracount: u32,
    pub named_groups: Vec<NamedGroup>,
    pub has_lookbehind: bool,
    pub small_ref_offset: [usize; 10],
    pub external_flags: u32,
}

#[derive(Clone, Copy)]
struct NestSave {
    nest_depth: u32,
    reset_group: u32,
    max_group: u32,
    flags: u32,
    options: u32,
    xoptions: u32,
}

const NSF_RESET: u32 = 0x0001;
const NSF_CONDASSERT: u32 = 0x0002;
const NSF_ATOMICSR: u32 = 0x0004;

const PARSE_TRACKED_OPTIONS: u32 = PCRE2_CASELESS
    | PCRE2_DOTALL
    | PCRE2_DUPNAMES
    | PCRE2_EXTENDED
    | PCRE2_EXTENDED_MORE
    | PCRE2_MULTILINE
    | PCRE2_NO_AUTO_CAPTURE
    | PCRE2_UNGREEDY;

const PARSE_TRACKED_EXTRA_OPTIONS: u32 = PCRE2_EXTRA_CASELESS_RESTRICT
    | PCRE2_EXTRA_ASCII_BSD
    | PCRE2_EXTRA_ASCII_BSS
    | PCRE2_EXTRA_ASCII_BSW
    | PCRE2_EXTRA_ASCII_DIGIT
    | PCRE2_EXTRA_ASCII_POSIX;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum RangeState {
    No,
    Started,
    OkEscaped,
    OkLiteral,
}

/// Result of `check_escape`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum EscapeValue {
    /// A data character.
    Char(u32),
    /// A special escape such as \d.
    Special(Esc),
    /// A numbered back reference.
    Backref(u32),
}

type PResult<T> = Result<T, (i32, usize)>;

/// Newline conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Newline {
    Cr,
    Lf,
    CrLf,
    Any,
    AnyCrLf,
    Nul,
}

impl Newline {
    /// If there is a newline at `p`, return its length.
    #[inline]
    pub(crate) fn is_newline(self, s: &[u8], p: usize, end: usize, utf: bool) -> Option<usize> {
        if p >= end {
            return None;
        }
        match self {
            Newline::Lf => (s[p] == b'\n').then_some(1),
            Newline::Cr => (s[p] == b'\r').then_some(1),
            Newline::Nul => (s[p] == 0).then_some(1),
            Newline::CrLf => (p + 2 <= end && s[p] == b'\r' && s[p + 1] == b'\n').then_some(2),
            Newline::AnyCrLf => match s[p] {
                b'\n' => Some(1),
                b'\r' => Some(if p + 1 < end && s[p + 1] == b'\n' {
                    2
                } else {
                    1
                }),
                _ => None,
            },
            Newline::Any => {
                let c = if utf {
                    chars::decode_utf8(s, p).0
                } else {
                    s[p] as u32
                };
                match c {
                    0x0a..=0x0c => Some(1),
                    0x0d => Some(if p + 1 < end && s[p + 1] == b'\n' {
                        2
                    } else {
                        1
                    }),
                    0x85 => Some(if utf { 2 } else { 1 }),
                    0x2028 | 0x2029 => Some(3),
                    _ => None,
                }
            }
        }
    }

    /// If there is a newline ending just before `p`, return its length.
    #[inline]
    pub(crate) fn was_newline(self, s: &[u8], p: usize, start: usize, utf: bool) -> Option<usize> {
        match self {
            Newline::Lf => (p > start && s[p - 1] == b'\n').then_some(1),
            Newline::Cr => (p > start && s[p - 1] == b'\r').then_some(1),
            Newline::Nul => (p > start && s[p - 1] == 0).then_some(1),
            Newline::CrLf => {
                (p >= start + 2 && s[p - 2] == b'\r' && s[p - 1] == b'\n').then_some(2)
            }
            Newline::AnyCrLf | Newline::Any => {
                if p <= start {
                    return None;
                }
                let mut q = p - 1;
                let c = if utf {
                    while q > start && s[q] & 0xc0 == 0x80 {
                        q -= 1;
                    }
                    chars::decode_utf8(s, q).0
                } else {
                    s[q] as u32
                };
                let crlf = |q: usize| if q > start && s[q - 1] == b'\r' { 2 } else { 1 };
                if self == Newline::AnyCrLf {
                    match c {
                        0x0a => Some(crlf(q)),
                        0x0d => Some(1),
                        _ => None,
                    }
                } else {
                    match c {
                        0x0a => Some(crlf(q)),
                        0x0b..=0x0d => Some(1),
                        0x85 => Some(if utf { 2 } else { 1 }),
                        0x2028 | 0x2029 => Some(3),
                        _ => None,
                    }
                }
            }
        }
    }
}

#[inline]
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[inline]
fn xdigit(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

/// `read_number()`: read a possibly signed number. `allow_sign < 0` means no
/// sign allowed; otherwise a sign makes the value relative to `allow_sign`.
/// Returns Ok(Some(n)) if a number was read, Ok(None) if there was no number,
/// Err(code) on error. `ptr` is always updated as PCRE2 does.
fn read_number(
    pat: &[u8],
    ptr: &mut usize,
    end: usize,
    allow_sign: i32,
    max_value: u32,
    max_error: i32,
) -> Result<Option<i32>, i32> {
    let mut sign = 0i32;
    let mut n: u32 = 0;
    let mut max_value = max_value;
    let mut p = *ptr;
    if allow_sign >= 0 && p < end {
        if pat[p] == b'+' {
            sign = 1;
            max_value -= allow_sign as u32;
            p += 1;
        } else if pat[p] == b'-' {
            sign = -1;
            p += 1;
        }
    }
    if p >= end || !is_digit(pat[p]) {
        // PCRE2 does not update the pointer in this case.
        return Ok(None);
    }
    while p < end && is_digit(pat[p]) {
        n = n * 10 + (pat[p] - b'0') as u32;
        p += 1;
        if n > max_value {
            *ptr = p;
            return Err(max_error);
        }
    }
    if allow_sign >= 0 && sign != 0 {
        if n == 0 {
            *ptr = p;
            return Err(ZERO_RELATIVE_REFERENCE);
        }
        if sign > 0 {
            n += allow_sign as u32;
        } else if n as i32 > allow_sign {
            *ptr = p;
            return Err(BAD_SUBPATTERN_REFERENCE);
        } else {
            n = (allow_sign + 1) as u32 - n;
        }
    }
    *ptr = p;
    Ok(Some(n as i32))
}

/// `read_repeat_counts()`. On entry `ptr` is after '{'.
/// Ok(Some((min,max))) with ptr updated past '}'; Ok(None) if not a quantifier
/// (ptr unchanged); Err(code) with ptr updated on error.
pub(crate) fn read_repeat_counts(
    pat: &[u8],
    ptr: &mut usize,
    end: usize,
) -> Result<Option<(u32, u32)>, i32> {
    let mut p = *ptr;
    let is_sp = |b: u8| b == b' ' || b == b'\t';
    while p < end && is_sp(pat[p]) {
        p += 1;
    }
    let mut pp = p;
    let mut had_minimum = false;
    if pp < end && is_digit(pat[pp]) {
        had_minimum = true;
        pp += 1;
        while pp < end && is_digit(pat[pp]) {
            pp += 1;
        }
    }
    while pp < end && is_sp(pat[pp]) {
        pp += 1;
    }
    if pp >= end {
        return Ok(None);
    }
    if pat[pp] == b'}' {
        if !had_minimum {
            return Ok(None);
        }
    } else {
        if pat[pp] != b',' {
            return Ok(None);
        }
        pp += 1;
        while pp < end && is_sp(pat[pp]) {
            pp += 1;
        }
        if pp >= end {
            return Ok(None);
        }
        if is_digit(pat[pp]) {
            pp += 1;
            while pp < end && is_digit(pat[pp]) {
                pp += 1;
            }
        } else if !had_minimum {
            return Ok(None);
        }
        while pp < end && is_sp(pat[pp]) {
            pp += 1;
        }
        if pp >= end || pat[pp] != b'}' {
            return Ok(None);
        }
    }

    let min: u32;
    let mut max: u32 = REPEAT_UNLIMITED;
    match read_number(pat, &mut p, end, -1, MAX_REPEAT_COUNT, QUANTIFIER_TOO_BIG) {
        Err(e) => {
            *ptr = p;
            return Err(e);
        }
        Ok(None) => {
            min = 0;
            p += 1; // skip comma
            while p < end && is_sp(pat[p]) {
                p += 1;
            }
            match read_number(pat, &mut p, end, -1, MAX_REPEAT_COUNT, QUANTIFIER_TOO_BIG) {
                Err(e) => {
                    *ptr = p;
                    return Err(e);
                }
                Ok(Some(m)) => max = m as u32,
                Ok(None) => {}
            }
        }
        Ok(Some(n)) => {
            min = n as u32;
            while p < end && is_sp(pat[p]) {
                p += 1;
            }
            if pat[p] == b'}' {
                max = min;
            } else {
                p += 1;
                while p < end && is_sp(pat[p]) {
                    p += 1;
                }
                match read_number(pat, &mut p, end, -1, MAX_REPEAT_COUNT, QUANTIFIER_TOO_BIG) {
                    Err(e) => {
                        *ptr = p;
                        return Err(e);
                    }
                    Ok(Some(m)) => max = m as u32,
                    Ok(None) => {}
                }
                if max < min {
                    *ptr = p;
                    return Err(QUANTIFIER_OUT_OF_ORDER);
                }
            }
        }
    }
    while p < end && is_sp(pat[p]) {
        p += 1;
    }
    p += 1;
    *ptr = p;
    Ok(Some((min, max)))
}

/// `check_posix_syntax()`: `ptr` is at the char after '['.
fn check_posix_syntax(pat: &[u8], ptr: usize, end: usize) -> Option<usize> {
    let terminator = pat[ptr];
    let mut p = ptr + 1;
    while end - p >= 2 {
        if pat[p] == b'\\' && (pat[p + 1] == b']' || pat[p + 1] == b'\\') {
            p += 1;
        } else if (pat[p] == b'[' && pat[p + 1] == terminator) || pat[p] == b']' {
            return None;
        } else if pat[p] == terminator && pat[p + 1] == b']' {
            return Some(p);
        }
        p += 1;
    }
    None
}

pub(crate) const POSIX_NAMES: [&[u8]; 14] = [
    b"alpha", b"lower", b"upper", b"alnum", b"ascii", b"blank", b"cntrl", b"digit", b"graph",
    b"print", b"punct", b"space", b"word", b"xdigit",
];
pub(crate) const PC_DIGIT: u8 = 7;
pub(crate) const PC_GRAPH: u8 = 8;
pub(crate) const PC_PRINT: u8 = 9;
pub(crate) const PC_PUNCT: u8 = 10;
pub(crate) const PC_XDIGIT: u8 = 13;

fn check_posix_name(name: &[u8]) -> Option<u8> {
    POSIX_NAMES.iter().position(|n| *n == name).map(|i| i as u8)
}

/// UCP substitutes for POSIX classes: (ptype, pvalue); ptype < 0 special.
const POSIX_SUBSTITUTES: [(i32, u16); 14] = [
    (unicode::PT_GC as i32, unicode::GT_L as u16), // alpha
    (unicode::PT_PC as i32, unicode::UCP_LL as u16), // lower
    (unicode::PT_PC as i32, unicode::UCP_LU as u16), // upper
    (unicode::PT_ALNUM as i32, 0),                 // alnum
    (-1, 0),                                       // ascii
    (-1, 1),                                       // blank
    (unicode::PT_PC as i32, unicode::UCP_CC as u16), // cntrl
    (unicode::PT_PC as i32, unicode::UCP_ND as u16), // digit
    (unicode::PT_PXGRAPH as i32, 0),               // graph
    (unicode::PT_PXPRINT as i32, 0),               // print
    (unicode::PT_PXPUNCT as i32, 0),               // punct
    (unicode::PT_PXSPACE as i32, 0),               // space
    (unicode::PT_WORD as i32, 0),                  // word
    (unicode::PT_PXXDIGIT as i32, 0),              // xdigit
];

#[derive(Clone, Copy)]
struct Verb {
    name: &'static [u8],
    meta: VerbMeta,
    has_arg: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VerbMeta {
    Mark,
    Accept,
    Fail,
    Commit,
    Prune,
    Skip,
    Then,
}

const VERBS: [Verb; 9] = [
    Verb {
        name: b"",
        meta: VerbMeta::Mark,
        has_arg: 1,
    },
    Verb {
        name: b"MARK",
        meta: VerbMeta::Mark,
        has_arg: 1,
    },
    Verb {
        name: b"ACCEPT",
        meta: VerbMeta::Accept,
        has_arg: -1,
    },
    Verb {
        name: b"F",
        meta: VerbMeta::Fail,
        has_arg: -1,
    },
    Verb {
        name: b"FAIL",
        meta: VerbMeta::Fail,
        has_arg: -1,
    },
    Verb {
        name: b"COMMIT",
        meta: VerbMeta::Commit,
        has_arg: 0,
    },
    Verb {
        name: b"PRUNE",
        meta: VerbMeta::Prune,
        has_arg: 0,
    },
    Verb {
        name: b"SKIP",
        meta: VerbMeta::Skip,
        has_arg: 0,
    },
    Verb {
        name: b"THEN",
        meta: VerbMeta::Then,
        has_arg: 0,
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Alas {
    Lookahead,
    Lookbehind,
    LookaheadNa,
    LookbehindNa,
    LookaheadNot,
    LookbehindNot,
    Atomic,
    ScriptRun,
    AtomicScriptRun,
}

const ALAS: [(&[u8], Alas); 17] = [
    (b"pla", Alas::Lookahead),
    (b"plb", Alas::Lookbehind),
    (b"napla", Alas::LookaheadNa),
    (b"naplb", Alas::LookbehindNa),
    (b"nla", Alas::LookaheadNot),
    (b"nlb", Alas::LookbehindNot),
    (b"positive_lookahead", Alas::Lookahead),
    (b"positive_lookbehind", Alas::Lookbehind),
    (b"non_atomic_positive_lookahead", Alas::LookaheadNa),
    (b"non_atomic_positive_lookbehind", Alas::LookbehindNa),
    (b"negative_lookahead", Alas::LookaheadNot),
    (b"negative_lookbehind", Alas::LookbehindNot),
    (b"atomic", Alas::Atomic),
    (b"sr", Alas::ScriptRun),
    (b"asr", Alas::AtomicScriptRun),
    (b"script_run", Alas::ScriptRun),
    (b"atomic_script_run", Alas::AtomicScriptRun),
];

/// The parser state (a port of parse_regex's locals plus the compile block
/// fields it uses).
pub(crate) struct Parser<'a> {
    pat: &'a [u8],
    end: usize,
    utf: bool,
    newline: Newline,
    external_options: u32,
    parsed: Vec<Meta>,
    bracount: u32,
    named_groups: Vec<NamedGroup>,
    has_lookbehind: bool,
    small_ref_offset: [usize; 10],
    external_flags: u32,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(pat: &'a [u8], utf: bool, newline: Newline, external_options: u32) -> Self {
        Parser {
            pat,
            end: pat.len(),
            utf,
            newline,
            external_options,
            parsed: Vec::with_capacity(pat.len() + 1),
            bracount: 0,
            named_groups: Vec::new(),
            has_lookbehind: false,
            small_ref_offset: [usize::MAX; 10],
            external_flags: 0,
        }
    }

    /// Get the character at `p` and its length (GETCHARINCTEST).
    #[inline]
    fn getchar(&self, p: usize) -> (u32, usize) {
        if self.utf {
            chars::decode_utf8(self.pat, p)
        } else {
            (self.pat[p] as u32, 1)
        }
    }

    // ------------------------------------------------------------------
    // check_escape()
    // ------------------------------------------------------------------

    /// Port of PRIV(check_escape). `ptr` points after the backslash and is
    /// updated as PCRE2 updates it (also on error).
    pub(crate) fn check_escape(
        &self,
        ptr: &mut usize,
        options: u32,
        xoptions: u32,
        isclass: bool,
    ) -> Result<EscapeValue, i32> {
        let pat = self.pat;
        let end = self.end;
        let utf = options & PCRE2_UTF != 0;
        let alt_bsux = (options & PCRE2_ALT_BSUX) != 0 || (xoptions & PCRE2_EXTRA_ALT_BSUX) != 0;

        if *ptr >= end {
            return Err(END_BACKSLASH);
        }
        let mut p = *ptr;
        let (mut c, l) = self.getchar(p);
        p += l;
        let mut escape: Option<Esc> = None;
        let mut err = 0i32;

        // Characters outside '0'..='z' are always literal.
        if !(b'0' as u32..=b'z' as u32).contains(&c) {
            *ptr = p;
            return Ok(EscapeValue::Char(c));
        }

        let simple: Option<Result<u32, Esc>> = match c as u8 {
            b':' | b';' | b'<' | b'=' | b'>' | b'?' | b'@' | b'[' | b'\\' | b']' | b'^' | b'_'
            | b'`' => Some(Ok(c)),
            b'A' => Some(Err(Esc::A)),
            b'B' => Some(Err(Esc::UpperB)),
            b'C' => Some(Err(Esc::C)),
            b'D' => Some(Err(Esc::UpperD)),
            b'E' => Some(Err(Esc::E)),
            b'G' => Some(Err(Esc::G)),
            b'H' => Some(Err(Esc::UpperH)),
            b'K' => Some(Err(Esc::K)),
            b'N' => Some(Err(Esc::N)),
            b'P' => Some(Err(Esc::UpperP)),
            b'Q' => Some(Err(Esc::Q)),
            b'R' => Some(Err(Esc::R)),
            b'S' => Some(Err(Esc::UpperS)),
            b'V' => Some(Err(Esc::UpperV)),
            b'W' => Some(Err(Esc::UpperW)),
            b'X' => Some(Err(Esc::X)),
            b'Z' => Some(Err(Esc::UpperZ)),
            b'a' => Some(Ok(7)),
            b'b' => Some(Err(Esc::LowerB)),
            b'd' => Some(Err(Esc::LowerD)),
            b'e' => Some(Ok(0x1b)),
            b'f' => Some(Ok(0x0c)),
            b'h' => Some(Err(Esc::LowerH)),
            b'k' => Some(Err(Esc::K2)),
            b'n' => Some(Ok(0x0a)),
            b'p' => Some(Err(Esc::LowerP)),
            b'r' => Some(Ok(0x0d)),
            b's' => Some(Err(Esc::LowerS)),
            b't' => Some(Ok(0x09)),
            b'v' => Some(Err(Esc::LowerV)),
            b'w' => Some(Err(Esc::LowerW)),
            b'z' => Some(Err(Esc::LowerZ)),
            _ => None,
        };

        match simple {
            Some(Ok(v)) => {
                c = v;
                if c == 0x0d && xoptions & PCRE2_EXTRA_ESCAPED_CR_IS_LF != 0 {
                    c = 0x0a;
                }
            }
            Some(Err(e)) => {
                escape = Some(e);
                if e == Esc::N && p < end && pat[p] == b'{' {
                    let mut q = p + 1;
                    while q < end && (pat[q] == b' ' || pat[q] == b'\t') {
                        q += 1;
                    }
                    if end - q > 1 && pat[q] == b'U' && pat[q + 1] == b'+' {
                        if utf {
                            p = q + 2;
                            escape = None;
                            // Handled as \x{...} below.
                            match self.read_braced_hex(&mut p, utf, xoptions) {
                                Ok(v) => c = v,
                                Err(e) => err = e,
                            }
                        } else {
                            err = SUPPORTED_ONLY_IN_UNICODE;
                        }
                    } else {
                        match read_repeat_counts(pat, &mut q, end) {
                            Ok(Some(_)) => {}
                            Ok(None) => err = UNSUPPORTED_ESCAPE_SEQUENCE,
                            Err(e) => err = e,
                        }
                    }
                }
            }
            None => {
                match c as u8 {
                    b'F' | b'l' | b'L' => err = UNSUPPORTED_ESCAPE_SEQUENCE,
                    b'u' => {
                        if !alt_bsux {
                            err = UNSUPPORTED_ESCAPE_SEQUENCE;
                        } else {
                            // ALT_BSUX is never set by PHP; minimal support.
                            if end - p >= 4 {
                                let d: Vec<Option<u32>> =
                                    (0..4).map(|i| xdigit(pat[p + i])).collect();
                                if d.iter().all(|x| x.is_some()) {
                                    c = d.iter().fold(0, |a, x| (a << 4) | x.unwrap());
                                    p += 4;
                                }
                            }
                        }
                    }
                    b'U' => {
                        if !alt_bsux {
                            err = UNSUPPORTED_ESCAPE_SEQUENCE;
                        }
                    }
                    b'g' => {
                        if !isclass {
                            if p >= end {
                                err = BACKSLASH_G_SYNTAX;
                            } else if pat[p] == b'<' || pat[p] == b'\'' {
                                escape = Some(Esc::G2);
                            } else if pat[p] == b'{' {
                                let mut q = p + 1;
                                while q < end && (pat[q] == b' ' || pat[q] == b'\t') {
                                    q += 1;
                                }
                                match read_number(
                                    pat,
                                    &mut q,
                                    end,
                                    self.bracount as i32,
                                    MAX_GROUP_NUMBER,
                                    SUBPATTERN_NUMBER_TOO_BIG,
                                ) {
                                    Err(e) => err = e,
                                    Ok(None) => escape = Some(Esc::K2),
                                    Ok(Some(s)) => {
                                        while q < end && (pat[q] == b' ' || pat[q] == b'\t') {
                                            q += 1;
                                        }
                                        if q >= end || pat[q] != b'}' {
                                            err = BACKSLASH_G_SYNTAX;
                                        } else {
                                            p = q + 1;
                                            if s <= 0 {
                                                err = BAD_SUBPATTERN_REFERENCE;
                                            } else {
                                                *ptr = p;
                                                return Ok(EscapeValue::Backref(s as u32));
                                            }
                                        }
                                    }
                                }
                            } else {
                                match read_number(
                                    pat,
                                    &mut p,
                                    end,
                                    self.bracount as i32,
                                    MAX_GROUP_NUMBER,
                                    SUBPATTERN_NUMBER_TOO_BIG,
                                ) {
                                    Err(e) => err = e,
                                    Ok(None) => err = BACKSLASH_G_SYNTAX,
                                    Ok(Some(s)) => {
                                        if s <= 0 {
                                            err = BAD_SUBPATTERN_REFERENCE;
                                        } else {
                                            *ptr = p;
                                            return Ok(EscapeValue::Backref(s as u32));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    b'1'..=b'9' | b'0' => {
                        let mut octal = true;
                        if c != b'0' as u32 && !isclass {
                            let oldptr = p;
                            let mut q = p - 1;
                            let r =
                                read_number(pat, &mut q, end, -1, (i32::MAX / 10 - 1) as u32, 0);
                            if let Ok(Some(s)) = r
                                && (s < 10 || pat[oldptr - 1] >= b'8' || s as u32 <= self.bracount)
                            {
                                if s as u32 > MAX_GROUP_NUMBER {
                                    err = SUBPATTERN_NUMBER_TOO_BIG;
                                    p = q;
                                } else {
                                    *ptr = q;
                                    return Ok(EscapeValue::Backref(s as u32));
                                }
                                octal = false;
                            }
                            if octal {
                                p = oldptr;
                            }
                        }
                        if octal {
                            if c >= b'8' as u32 {
                                // Literal 8 or 9.
                            } else {
                                c -= b'0' as u32;
                                let mut i = 0;
                                while i < 2 && p < end && (b'0'..=b'7').contains(&pat[p]) {
                                    c = c * 8 + (pat[p] - b'0') as u32;
                                    p += 1;
                                    i += 1;
                                }
                                if !utf && c > 0xff {
                                    err = OCTAL_BYTE_TOO_BIG;
                                }
                            }
                        }
                    }
                    b'o' => {
                        if p >= end || pat[p] != b'{' {
                            // ptr++ then ptr-- in PCRE2 (no increment at the end)
                            if p >= end {
                                p -= 1;
                            }
                            err = BACKSLASH_O_MISSING_BRACE;
                        } else {
                            p += 1;
                            while p < end && (pat[p] == b' ' || pat[p] == b'\t') {
                                p += 1;
                            }
                            if p >= end || pat[p] == b'}' {
                                err = MISSING_OCTAL_OR_HEX_DIGITS;
                            } else {
                                c = 0;
                                let mut overflow = false;
                                while p < end && (b'0'..=b'7').contains(&pat[p]) {
                                    let cc = pat[p] as u32;
                                    p += 1;
                                    if c == 0 && cc == b'0' as u32 {
                                        continue;
                                    }
                                    c = (c << 3) + (cc - b'0' as u32);
                                    if c > (if utf { 0x10ffff } else { 0xff }) {
                                        overflow = true;
                                        break;
                                    }
                                }
                                while p < end && (pat[p] == b' ' || pat[p] == b'\t') {
                                    p += 1;
                                }
                                if overflow {
                                    while p < end && (b'0'..=b'7').contains(&pat[p]) {
                                        p += 1;
                                    }
                                    err = CODE_POINT_TOO_BIG;
                                } else if p < end && pat[p] == b'}' {
                                    p += 1;
                                    if utf
                                        && (0xd800..=0xdfff).contains(&c)
                                        && xoptions & PCRE2_EXTRA_ALLOW_SURROGATE_ESCAPES == 0
                                    {
                                        p -= 1;
                                        err = UNICODE_DISALLOWED_CODE_POINT;
                                    }
                                } else {
                                    // ptr++ then ptr--: points at the bad char
                                    if p >= end {
                                        p -= 1;
                                    }
                                    err = INVALID_OCTAL;
                                }
                            }
                        }
                    }
                    b'x' => {
                        if alt_bsux {
                            if end - p >= 2
                                && let (Some(a), Some(b)) = (xdigit(pat[p]), xdigit(pat[p + 1]))
                            {
                                c = (a << 4) | b;
                                p += 2;
                            }
                        } else if p < end && pat[p] == b'{' {
                            p += 1;
                            while p < end && (pat[p] == b' ' || pat[p] == b'\t') {
                                p += 1;
                            }
                            match self.read_braced_hex(&mut p, utf, xoptions) {
                                Ok(v) => c = v,
                                Err(e) => err = e,
                            }
                        } else {
                            c = 0;
                            if p < end
                                && let Some(d) = xdigit(pat[p])
                            {
                                p += 1;
                                c = d;
                                if p < end
                                    && let Some(d2) = xdigit(pat[p])
                                {
                                    p += 1;
                                    c = (c << 4) | d2;
                                }
                            }
                        }
                    }
                    b'c' => {
                        if p >= end {
                            err = END_BACKSLASH_C;
                        } else {
                            let mut cc = pat[p] as u32;
                            if (b'a' as u32..=b'z' as u32).contains(&cc) {
                                cc -= 32;
                            }
                            if !(32..=126).contains(&cc) {
                                err = BACKSLASH_C_SYNTAX;
                            } else {
                                c = cc ^ 0x40;
                                p += 1;
                            }
                        }
                    }
                    _ => {
                        *ptr = p - 1;
                        return Err(UNKNOWN_ESCAPE);
                    }
                }
            }
        }

        *ptr = p;
        if err != 0 {
            return Err(err);
        }
        match escape {
            Some(e) => Ok(EscapeValue::Special(e)),
            None => Ok(EscapeValue::Char(c)),
        }
    }

    /// The `\x{...}` body (after `{` and optional spaces). Also used for `\N{U+`.
    fn read_braced_hex(&self, ptr: &mut usize, utf: bool, xoptions: u32) -> Result<u32, i32> {
        let pat = self.pat;
        let end = self.end;
        let mut p = *ptr;
        if p >= end || pat[p] == b'}' {
            *ptr = p;
            return Err(MISSING_OCTAL_OR_HEX_DIGITS);
        }
        let mut c: u32 = 0;
        let mut overflow = false;
        while p < end {
            let Some(cc) = xdigit(pat[p]) else { break };
            p += 1;
            if c == 0 && cc == 0 {
                continue;
            }
            c = (c << 4) | cc;
            if (utf && c > 0x10ffff) || (!utf && c > 0xff) {
                overflow = true;
                break;
            }
        }
        while p < end && (pat[p] == b' ' || pat[p] == b'\t') {
            p += 1;
        }
        let r = if overflow {
            while p < end && xdigit(pat[p]).is_some() {
                p += 1;
            }
            Err(CODE_POINT_TOO_BIG)
        } else if p < end && pat[p] == b'}' {
            p += 1;
            if utf
                && (0xd800..=0xdfff).contains(&c)
                && xoptions & PCRE2_EXTRA_ALLOW_SURROGATE_ESCAPES == 0
            {
                p -= 1;
                Err(UNICODE_DISALLOWED_CODE_POINT)
            } else {
                Ok(c)
            }
        } else {
            if p >= end {
                p -= 1;
            }
            Err(INVALID_HEXADECIMAL)
        };
        *ptr = p;
        r
    }

    // ------------------------------------------------------------------
    // get_ucp()
    // ------------------------------------------------------------------

    /// Port of get_ucp(). `ptr` is after the P or p. Returns (negated, ptype, pvalue).
    fn get_ucp(&self, ptr: &mut usize) -> Result<(bool, u8, u16), i32> {
        let pat = self.pat;
        let end = self.end;
        let mut p = *ptr;
        let mut negated = false;
        let mut name: Vec<u8> = Vec::with_capacity(16);
        let mut vpos: Option<usize> = None;

        let malformed = |p: usize, ptr: &mut usize| {
            *ptr = p;
            Err(MALFORMED_UNICODE_PROPERTY)
        };

        if p >= end {
            return malformed(p, ptr);
        }
        let c = pat[p];
        p += 1;
        if c == b'{' {
            if p >= end {
                return malformed(p, ptr);
            }
            if pat[p] == b'^' {
                negated = true;
                p += 1;
            }
            let mut closed = false;
            while name.len() < 49 {
                if p >= end {
                    return malformed(p, ptr);
                }
                let mut c = pat[p];
                p += 1;
                while c == b'_' || c == b'-' || is_c_isspace(c) {
                    if p >= end {
                        return malformed(p, ptr);
                    }
                    c = pat[p];
                    p += 1;
                }
                if c == 0 {
                    return malformed(p, ptr);
                }
                if c == b'}' {
                    closed = true;
                    break;
                }
                if (c == b':' || c == b'=') && vpos.is_none() {
                    vpos = Some(name.len());
                }
                name.push(c.to_ascii_lowercase());
            }
            if !closed {
                return malformed(p, ptr);
            }
        } else if c.is_ascii_alphabetic() {
            name.push(c.to_ascii_lowercase());
        } else {
            return malformed(p, ptr);
        }
        *ptr = p;

        let mut ptscript: Option<u8> = None;
        if let Some(v) = vpos {
            let prefix = &name[..v];
            let value = name[v + 1..].to_vec();
            if prefix == b"bidiclass" || prefix == b"bc" {
                let mut n = b"bidi".to_vec();
                n.extend_from_slice(&value);
                name = n;
            } else if prefix == b"script" || prefix == b"sc" {
                ptscript = Some(unicode::PT_SC);
                name = value;
            } else if prefix == b"scriptextensions" || prefix == b"scx" {
                ptscript = Some(unicode::PT_SCX);
                name = value;
            } else {
                return Err(UNKNOWN_UNICODE_PROPERTY);
            }
        }

        match unicode::lookup_property(&name) {
            Some((ptype, pvalue)) => {
                if vpos.is_none() || ptscript.is_none() {
                    return Ok((negated, ptype, pvalue));
                }
                match ptype {
                    unicode::PT_SC => Ok((negated, unicode::PT_SC, pvalue)),
                    unicode::PT_SCX => Ok((negated, ptscript.unwrap(), pvalue)),
                    _ => Err(UNKNOWN_UNICODE_PROPERTY),
                }
            }
            None => Err(UNKNOWN_UNICODE_PROPERTY),
        }
    }

    // ------------------------------------------------------------------
    // read_name()
    // ------------------------------------------------------------------

    /// Port of read_name(). `ptr` points at the character before the name
    /// ('*' for a verb/alpha assertion). Returns (offset, len).
    fn read_name(&self, ptr: &mut usize, terminator: u8) -> Result<(usize, usize), i32> {
        let pat = self.pat;
        let end = self.end;
        let mut p = *ptr;
        let is_group = pat[p] != b'*';
        p += 1;
        let is_braced = terminator == b'}';
        if is_braced {
            while p < end && (pat[p] == b' ' || pat[p] == b'\t') {
                p += 1;
            }
        }
        if p >= end {
            *ptr = p;
            return Err(if is_group {
                SUBPATTERN_NAME_EXPECTED
            } else {
                VERB_UNKNOWN
            });
        }
        let start = p;
        if self.utf && is_group {
            let (mut c, mut l) = chars::decode_utf8(pat, p);
            let mut t = unicode::chartype(c);
            if t == unicode::UCP_ND {
                *ptr = p;
                return Err(INVALID_SUBPATTERN_NAME);
            }
            loop {
                if t != unicode::UCP_ND && unicode::gentype(t) != unicode::GT_L && c != b'_' as u32
                {
                    break;
                }
                p += l;
                if p >= end {
                    break;
                }
                let d = chars::decode_utf8(pat, p);
                c = d.0;
                l = d.1;
                t = unicode::chartype(c);
            }
        } else {
            if is_group && is_digit(pat[p]) {
                *ptr = p;
                return Err(INVALID_SUBPATTERN_NAME);
            }
            while p < end && chars::ctype(pat[p] as u32) & CTYPE_WORD != 0 {
                p += 1;
            }
        }
        if p > start + MAX_NAME_SIZE {
            *ptr = p;
            return Err(SUBPATTERN_NAME_TOO_LONG);
        }
        let len = p - start;
        if is_group {
            if len == 0 {
                *ptr = p;
                return Err(SUBPATTERN_NAME_EXPECTED);
            }
            if is_braced {
                while p < end && (pat[p] == b' ' || pat[p] == b'\t') {
                    p += 1;
                }
            }
            if p >= end || pat[p] != terminator {
                *ptr = p;
                return Err(MISSING_NAME_TERMINATOR);
            }
            p += 1;
        }
        *ptr = p;
        Ok((start, len))
    }

    fn push_escdsw(&mut self, escape: Esc, options: u32, xoptions: u32) {
        let (ascii_option, negated) = match escape {
            Esc::UpperD => (PCRE2_EXTRA_ASCII_BSD, true),
            Esc::LowerD => (PCRE2_EXTRA_ASCII_BSD, false),
            Esc::UpperS => (PCRE2_EXTRA_ASCII_BSS, true),
            Esc::LowerS => (PCRE2_EXTRA_ASCII_BSS, false),
            Esc::UpperW => (PCRE2_EXTRA_ASCII_BSW, true),
            _ => (PCRE2_EXTRA_ASCII_BSW, false),
        };
        if options & PCRE2_UCP == 0 || xoptions & ascii_option != 0 {
            self.parsed.push(Meta::Escape(escape));
        } else {
            let (ptype, pvalue) = match escape {
                Esc::UpperD | Esc::LowerD => (unicode::PT_PC, unicode::UCP_ND as u16),
                Esc::UpperS | Esc::LowerS => (unicode::PT_SPACE, 0),
                _ => (unicode::PT_WORD, 0),
            };
            self.parsed.push(Meta::Prop {
                negated,
                ptype,
                pvalue,
            });
        }
    }

    fn is_newline_at(&self, p: usize) -> Option<usize> {
        self.newline.is_newline(self.pat, p, self.end, self.utf)
    }

    // ------------------------------------------------------------------
    // parse_regex()
    // ------------------------------------------------------------------

    /// Run the parse. `start` is the offset of the regex proper (after any
    /// leading (*XXX) settings).
    pub(crate) fn parse(
        mut self,
        start: usize,
        xoptions_in: u32,
    ) -> Result<ParseResult, (i32, usize)> {
        match self.parse_inner(start, xoptions_in) {
            Ok(()) => Ok(ParseResult {
                parsed: self.parsed,
                bracount: self.bracount,
                named_groups: self.named_groups,
                has_lookbehind: self.has_lookbehind,
                small_ref_offset: self.small_ref_offset,
                external_flags: self.external_flags,
            }),
            Err(e) => Err(e),
        }
    }

    fn parse_inner(&mut self, start: usize, xoptions_in: u32) -> PResult<()> {
        let pat = self.pat;
        let end = self.end;
        let utf = self.utf;
        let mut ptr = start;
        let mut options = self.external_options;
        let mut xoptions = xoptions_in;
        let mut nest_depth: u32 = 0;
        let mut nests: Vec<NestSave> = Vec::new();
        let mut inescq = false;
        let mut inverbname = false;
        let mut verbnamestart = 0usize;
        let mut verb_index = 0usize; // index of the VerbArg meta being filled
        let mut add_after_mark: Option<Meta> = None;
        let mut expect_cond_assert: i32 = 0;
        let mut okquantifier = false;
        let mut meta_quantifier = false;
        let mut this_parsed_item = 0usize;
        let mut prev_parsed_item = 0usize;
        let mut verbstartptr = 0usize;

        if options & PCRE2_EXTENDED_MORE != 0 {
            options |= PCRE2_EXTENDED;
        }

        macro_rules! fail {
            ($code:expr, $p:expr) => {
                return Err(($code, $p))
            };
        }

        while ptr < end {
            if nest_depth > PARENS_NEST_LIMIT {
                fail!(PARENTHESES_NEST_TOO_DEEP, ptr);
            }
            if this_parsed_item != self.parsed.len() {
                prev_parsed_item = this_parsed_item;
                this_parsed_item = self.parsed.len();
            }

            let (c, l) = self.getchar(ptr);
            ptr += l;

            // \Q...\E literal mode
            if inescq {
                if c == b'\\' as u32 && ptr < end && pat[ptr] == b'E' {
                    inescq = false;
                    ptr += 1;
                } else {
                    if expect_cond_assert > 0 {
                        fail!(CONDITION_ASSERTION_EXPECTED, ptr - 1);
                    }
                    if inverbname {
                        if let Some(Meta::VerbArg(_, name)) = self.parsed.get_mut(verb_index) {
                            name.push(c);
                        }
                    } else {
                        self.parsed.push(Meta::Lit(c));
                        okquantifier = true;
                    }
                    meta_quantifier = false;
                }
                continue;
            }

            // Verb name characters.
            if inverbname
                && ((options & (PCRE2_EXTENDED | PCRE2_ALT_VERBNAMES))
                    != (PCRE2_EXTENDED | PCRE2_ALT_VERBNAMES)
                    || (c > 255 && (c | 1) != 0x200f && (c | 1) != 0x2029)
                    || (c < 256
                        && c != b'#' as u32
                        && chars::ctype(c) & CTYPE_SPACE == 0
                        && c != chars::CHAR_NEL))
            {
                match c {
                    0x29 => {
                        // ')'
                        inverbname = false;
                        if ptr - verbnamestart - 1 > MAX_MARK {
                            fail!(VERB_NAME_TOO_LONG, ptr - 1);
                        }
                        if let Some(m) = add_after_mark.take() {
                            self.parsed.push(m);
                        }
                    }
                    0x5c if options & PCRE2_ALT_VERBNAMES != 0 => {
                        let mut p = ptr;
                        match self.check_escape(&mut p, options, xoptions, false) {
                            Err(e) => fail!(e, p),
                            Ok(EscapeValue::Char(ch)) => {
                                ptr = p;
                                if let Some(Meta::VerbArg(_, name)) =
                                    self.parsed.get_mut(verb_index)
                                {
                                    name.push(ch);
                                }
                            }
                            Ok(EscapeValue::Special(Esc::Q)) => {
                                ptr = p;
                                inescq = true;
                            }
                            Ok(EscapeValue::Special(Esc::E)) => ptr = p,
                            Ok(_) => fail!(ESCAPE_INVALID_IN_VERB, p),
                        }
                    }
                    _ => {
                        if let Some(Meta::VerbArg(_, name)) = self.parsed.get_mut(verb_index) {
                            name.push(c);
                        }
                    }
                }
                continue;
            }

            // \Q and \E at top level.
            if c == b'\\' as u32 && ptr < end && (pat[ptr] == b'Q' || pat[ptr] == b'E') {
                inescq = pat[ptr] == b'Q';
                ptr += 1;
                continue;
            }

            // Extended mode white space and comments.
            if options & PCRE2_EXTENDED != 0 {
                if c < 256 && chars::ctype(c) & CTYPE_SPACE != 0 {
                    continue;
                }
                if c == chars::CHAR_NEL || (c | 1) == 0x200f || (c | 1) == 0x2029 {
                    continue;
                }
                if c == b'#' as u32 {
                    while ptr < end {
                        if let Some(nl) = self.is_newline_at(ptr) {
                            ptr += nl;
                            break;
                        }
                        ptr += 1;
                        if utf {
                            while ptr < end && pat[ptr] & 0xc0 == 0x80 {
                                ptr += 1;
                            }
                        }
                    }
                    continue;
                }
            }

            // Bracketed comments (?#...)
            if c == b'(' as u32 && end - ptr >= 2 && pat[ptr] == b'?' && pat[ptr + 1] == b'#' {
                ptr += 1;
                loop {
                    ptr += 1;
                    if ptr >= end || pat[ptr] == b')' {
                        break;
                    }
                }
                if ptr >= end {
                    fail!(MISSING_COMMENT_CLOSING, ptr);
                }
                ptr += 1;
                continue;
            }

            // Expecting a conditional assertion?
            if expect_cond_assert > 0 {
                let mut ok =
                    c == b'(' as u32 && end - ptr >= 3 && (pat[ptr] == b'?' || pat[ptr] == b'*');
                if ok {
                    if pat[ptr] == b'*' {
                        ok = chars::ctype(pat[ptr + 1] as u32) & CTYPE_LCLETTER != 0;
                    } else {
                        ok = match pat[ptr + 1] {
                            b'C' => expect_cond_assert == 2,
                            b'=' | b'!' => true,
                            b'<' => pat[ptr + 2] == b'=' || pat[ptr + 2] == b'!',
                            _ => false,
                        };
                    }
                }
                if !ok {
                    fail!(CONDITION_ASSERTION_EXPECTED, ptr - 1);
                }
            }

            let prev_expect_cond_assert = expect_cond_assert;
            expect_cond_assert = 0;
            let prev_okquantifier = okquantifier;
            let prev_meta_quantifier = meta_quantifier;
            okquantifier = false;
            meta_quantifier = false;

            // Quantifier modifiers + and ?
            if prev_meta_quantifier && (c == b'?' as u32 || c == b'+' as u32) {
                let mode = if c == b'?' as u32 {
                    QuantMode::Lazy
                } else {
                    QuantMode::Possessive
                };
                if let Some(
                    Meta::Asterisk(m) | Meta::Plus(m) | Meta::Query(m) | Meta::MinMax(_, _, m),
                ) = self.parsed.last_mut()
                {
                    *m = mode;
                }
                continue;
            }

            let mut quant: Option<Meta> = None;

            match c {
                0x5c => {
                    // Backslash
                    let mut p = ptr;
                    let r = self.check_escape(&mut p, options, xoptions, false);
                    ptr = p;
                    match r {
                        Err(e) => fail!(e, ptr),
                        Ok(EscapeValue::Char(ch)) => {
                            self.parsed.push(Meta::Lit(ch));
                            okquantifier = true;
                        }
                        Ok(EscapeValue::Backref(n)) => {
                            let offset = ptr - 1;
                            if n < 10 && self.small_ref_offset[n as usize] == usize::MAX {
                                self.small_ref_offset[n as usize] = offset;
                            }
                            self.parsed.push(Meta::Backref { n, offset });
                            okquantifier = true;
                        }
                        Ok(EscapeValue::Special(esc)) => match esc {
                            Esc::C => {
                                if options & PCRE2_NEVER_BACKSLASH_C != 0 {
                                    fail!(BACKSLASH_C_CALLER_DISABLED, ptr);
                                }
                                okquantifier = true;
                                self.parsed.push(Meta::Escape(esc));
                            }
                            Esc::X
                            | Esc::UpperH
                            | Esc::LowerH
                            | Esc::N
                            | Esc::R
                            | Esc::UpperV
                            | Esc::LowerV => {
                                if esc == Esc::X {
                                    self.external_flags |= FLAG_HASBKPORX;
                                }
                                okquantifier = true;
                                self.parsed.push(Meta::Escape(esc));
                            }
                            Esc::UpperD
                            | Esc::LowerD
                            | Esc::UpperS
                            | Esc::LowerS
                            | Esc::UpperW
                            | Esc::LowerW => {
                                okquantifier = true;
                                self.push_escdsw(esc, options, xoptions);
                            }
                            Esc::UpperP | Esc::LowerP => {
                                self.external_flags |= FLAG_HASBKPORX;
                                let mut p = ptr;
                                match self.get_ucp(&mut p) {
                                    Err(e) => fail!(e, p),
                                    Ok((neg, ptype, pvalue)) => {
                                        ptr = p;
                                        let negated = (esc == Esc::UpperP) != neg;
                                        self.parsed.push(Meta::Prop {
                                            negated,
                                            ptype,
                                            pvalue,
                                        });
                                        okquantifier = true;
                                    }
                                }
                            }
                            Esc::G2 | Esc::K2 => {
                                if ptr >= end
                                    || (pat[ptr] != b'{' && pat[ptr] != b'<' && pat[ptr] != b'\'')
                                {
                                    fail!(
                                        if esc == Esc::G2 {
                                            BACKSLASH_G_SYNTAX
                                        } else {
                                            BACKSLASH_K_SYNTAX
                                        },
                                        ptr
                                    );
                                }
                                let terminator = match pat[ptr] {
                                    b'<' => b'>',
                                    b'\'' => b'\'',
                                    _ => b'}',
                                };
                                if esc == Esc::G2 && terminator != b'}' {
                                    let mut p = ptr + 1;
                                    match read_number(
                                        pat,
                                        &mut p,
                                        end,
                                        self.bracount as i32,
                                        MAX_GROUP_NUMBER,
                                        SUBPATTERN_NUMBER_TOO_BIG,
                                    ) {
                                        Ok(Some(i)) => {
                                            if p >= end || pat[p] != terminator {
                                                fail!(BACKSLASH_G_SYNTAX, ptr);
                                            }
                                            ptr = p;
                                            // SET_RECURSION
                                            let offset = ptr;
                                            ptr += 1;
                                            self.parsed.push(Meta::Recurse {
                                                n: i as u32,
                                                offset,
                                            });
                                            okquantifier = true;
                                            continue;
                                        }
                                        Ok(None) => {}
                                        Err(e) => fail!(e, ptr),
                                    }
                                }
                                let mut p = ptr;
                                match self.read_name(&mut p, terminator) {
                                    Err(e) => fail!(e, p),
                                    Ok((offset, len)) => {
                                        ptr = p;
                                        if esc == Esc::K2 || terminator == b'}' {
                                            self.parsed.push(Meta::BackrefByName { offset, len });
                                        } else {
                                            self.parsed.push(Meta::RecurseByName { offset, len });
                                        }
                                        okquantifier = true;
                                    }
                                }
                            }
                            _ => {
                                // \A, \B, \b, \G, \K, \Z, \z, \E, \Q (can't be quantified)
                                self.parsed.push(Meta::Escape(esc));
                            }
                        },
                    }
                }

                0x5e => self.parsed.push(Meta::Circumflex),
                0x24 => self.parsed.push(Meta::Dollar),
                0x2e => {
                    self.parsed.push(Meta::Dot);
                    okquantifier = true;
                }
                0x2a => quant = Some(Meta::Asterisk(QuantMode::Greedy)),
                0x2b => quant = Some(Meta::Plus(QuantMode::Greedy)),
                0x3f => quant = Some(Meta::Query(QuantMode::Greedy)),
                0x7b => {
                    let mut p = ptr;
                    match read_repeat_counts(pat, &mut p, end) {
                        Err(e) => fail!(e, p),
                        Ok(None) => {
                            self.parsed.push(Meta::Lit(c));
                            okquantifier = true;
                        }
                        Ok(Some((min, max))) => {
                            ptr = p;
                            quant = Some(Meta::MinMax(min, max, QuantMode::Greedy));
                        }
                    }
                }

                0x5b => {
                    // Character class
                    okquantifier = true;
                    if end - ptr >= 6
                        && (&pat[ptr..ptr + 6] == b"[:<:]]" || &pat[ptr..ptr + 6] == b"[:>:]]")
                    {
                        self.parsed.push(Meta::Escape(Esc::LowerB));
                        if pat[ptr + 2] == b'<' {
                            self.parsed.push(Meta::Lookahead);
                        } else {
                            self.parsed.push(Meta::Lookbehind {
                                kind: LbKind::Pos,
                                offset: 0,
                                len: 0,
                                minlen: 0,
                            });
                            self.has_lookbehind = true;
                        }
                        if options & PCRE2_UCP == 0 {
                            self.parsed.push(Meta::Escape(Esc::LowerW));
                        } else {
                            self.parsed.push(Meta::Prop {
                                negated: false,
                                ptype: unicode::PT_WORD,
                                pvalue: 0,
                            });
                        }
                        self.parsed.push(Meta::Ket);
                        ptr += 6;
                    } else {
                        self.parse_class(&mut ptr, options, xoptions, &mut inescq)?;
                    }
                }

                0x28 => {
                    // Opening parenthesis
                    if ptr >= end {
                        fail!(MISSING_CLOSING_PARENTHESIS, ptr);
                    }
                    if pat[ptr] != b'?' {
                        if pat[ptr] != b'*' {
                            nest_depth += 1;
                            if options & PCRE2_NO_AUTO_CAPTURE == 0 {
                                if self.bracount >= MAX_GROUP_NUMBER {
                                    fail!(TOO_MANY_CAPTURES, ptr);
                                }
                                self.bracount += 1;
                                self.parsed.push(Meta::Capture(self.bracount));
                            } else {
                                self.parsed.push(Meta::NoCapture);
                            }
                        } else if end - ptr <= 1 || pat[ptr + 1] == b')' {
                            // (* followed by end or ): treated so that a "bad
                            // quantifier" error is given.
                        } else if chars::ctype(pat[ptr + 1] as u32) & CTYPE_LCLETTER != 0 {
                            // Alpha assertion
                            let mut p = ptr;
                            let (noff, nlen) = match self.read_name(&mut p, 0) {
                                Ok(v) => v,
                                Err(e) => fail!(e, p),
                            };
                            ptr = p;
                            if ptr >= end || pat[ptr] != b':' {
                                fail!(ALPHA_ASSERTION_UNKNOWN, ptr);
                            }
                            let name = &pat[noff..noff + nlen];
                            let Some(&(_, alas)) = ALAS.iter().find(|(n, _)| *n == name) else {
                                fail!(ALPHA_ASSERTION_UNKNOWN, ptr);
                            };
                            if prev_expect_cond_assert > 0
                                && !matches!(
                                    alas,
                                    Alas::Lookahead
                                        | Alas::LookaheadNot
                                        | Alas::Lookbehind
                                        | Alas::LookbehindNot
                                )
                            {
                                fail!(
                                    if matches!(alas, Alas::LookaheadNa | Alas::LookbehindNa) {
                                        CONDITION_ATOMIC_ASSERTION_EXPECTED
                                    } else {
                                        CONDITION_ASSERTION_EXPECTED
                                    },
                                    ptr
                                );
                            }
                            match alas {
                                Alas::Atomic => {
                                    self.parsed.push(Meta::Atomic);
                                    nest_depth += 1;
                                    ptr += 1;
                                }
                                Alas::Lookahead | Alas::LookaheadNa | Alas::LookaheadNot => {
                                    self.parsed.push(match alas {
                                        Alas::Lookahead => Meta::Lookahead,
                                        Alas::LookaheadNa => Meta::LookaheadNa,
                                        _ => Meta::LookaheadNot,
                                    });
                                    ptr += 1;
                                    self.post_assertion(
                                        &mut nest_depth,
                                        &mut nests,
                                        prev_expect_cond_assert,
                                        options,
                                        xoptions,
                                        ptr,
                                    )?;
                                }
                                Alas::Lookbehind | Alas::LookbehindNot | Alas::LookbehindNa => {
                                    // ptr is at ':'; PCRE2 does ptr-- then POST_LOOKBEHIND
                                    // which takes offset = ptr - 2 and does ptr += 2.
                                    ptr -= 1;
                                    let kind = match alas {
                                        Alas::Lookbehind => LbKind::Pos,
                                        Alas::LookbehindNot => LbKind::Neg,
                                        _ => LbKind::NonAtomic,
                                    };
                                    self.has_lookbehind = true;
                                    let offset = ptr - 2;
                                    self.parsed.push(Meta::Lookbehind {
                                        kind,
                                        offset,
                                        len: 0,
                                        minlen: 0,
                                    });
                                    ptr += 2;
                                    self.post_assertion(
                                        &mut nest_depth,
                                        &mut nests,
                                        prev_expect_cond_assert,
                                        options,
                                        xoptions,
                                        ptr,
                                    )?;
                                }
                                Alas::ScriptRun | Alas::AtomicScriptRun => {
                                    self.parsed.push(Meta::ScriptRun);
                                    nest_depth += 1;
                                    ptr += 1;
                                    if alas == Alas::AtomicScriptRun {
                                        self.parsed.push(Meta::Atomic);
                                        if nests.len() >= NEST_STACK_SIZE {
                                            fail!(QUERY_BARJX_NEST_TOO_DEEP, ptr);
                                        }
                                        nests.push(NestSave {
                                            nest_depth,
                                            reset_group: 0,
                                            max_group: 0,
                                            flags: NSF_ATOMICSR,
                                            options: options & PARSE_TRACKED_OPTIONS,
                                            xoptions: xoptions & PARSE_TRACKED_EXTRA_OPTIONS,
                                        });
                                    }
                                }
                            }
                        } else {
                            // (*VERB)
                            let mut p = ptr;
                            let (noff, nlen) = match self.read_name(&mut p, 0) {
                                Ok(v) => v,
                                Err(e) => fail!(e, p),
                            };
                            ptr = p;
                            if ptr >= end || (pat[ptr] != b':' && pat[ptr] != b')') {
                                fail!(VERB_UNKNOWN, ptr);
                            }
                            let name = &pat[noff..noff + nlen];
                            let Some(verb) = VERBS.iter().find(|v| v.name == name).copied() else {
                                fail!(VERB_UNKNOWN, ptr);
                            };
                            if pat[ptr] == b':' && ptr + 1 < end && pat[ptr + 1] == b')' {
                                ptr += 1;
                            }
                            if verb.has_arg > 0 && pat[ptr] != b':' {
                                fail!(MARK_MISSING_ARGUMENT, ptr);
                            }
                            verbstartptr = self.parsed.len();
                            okquantifier = verb.meta == VerbMeta::Accept;
                            let had_colon = pat[ptr] == b':';
                            ptr += 1;
                            if had_colon {
                                if verb.has_arg < 0 {
                                    add_after_mark = Some(match verb.meta {
                                        VerbMeta::Accept => Meta::Accept,
                                        _ => Meta::Fail,
                                    });
                                    self.parsed.push(Meta::VerbArg(VerbKind::Mark, Vec::new()));
                                } else {
                                    let kind = match verb.meta {
                                        VerbMeta::Mark => VerbKind::Mark,
                                        VerbMeta::Commit => VerbKind::Commit,
                                        VerbMeta::Prune => VerbKind::Prune,
                                        VerbMeta::Skip => VerbKind::Skip,
                                        _ => VerbKind::Then,
                                    };
                                    self.parsed.push(Meta::VerbArg(kind, Vec::new()));
                                }
                                verb_index = self.parsed.len() - 1;
                                verbnamestart = ptr;
                                inverbname = true;
                            } else {
                                self.parsed.push(match verb.meta {
                                    VerbMeta::Accept => Meta::Accept,
                                    VerbMeta::Fail => Meta::Fail,
                                    VerbMeta::Commit => Meta::Commit,
                                    VerbMeta::Prune => Meta::Prune,
                                    VerbMeta::Skip => Meta::Skip,
                                    VerbMeta::Then => Meta::Then,
                                    VerbMeta::Mark => Meta::VerbArg(VerbKind::Mark, Vec::new()),
                                });
                            }
                        }
                    } else {
                        // (? items
                        ptr += 1;
                        if ptr >= end {
                            fail!(MISSING_CLOSING_PARENTHESIS, ptr);
                        }
                        self.parse_query_group(
                            &mut ptr,
                            &mut options,
                            &mut xoptions,
                            &mut nest_depth,
                            &mut nests,
                            &mut okquantifier,
                            &mut expect_cond_assert,
                            prev_expect_cond_assert,
                        )?;
                    }
                }

                0x7c => {
                    if let Some(top) = nests.last_mut()
                        && top.nest_depth == nest_depth
                        && top.flags & NSF_RESET != 0
                    {
                        if self.bracount > top.max_group {
                            top.max_group = self.bracount;
                        }
                        self.bracount = top.reset_group;
                    }
                    self.parsed.push(Meta::Alt(0));
                }

                0x29 => {
                    okquantifier = true;
                    if let Some(top) = nests.last().copied()
                        && top.nest_depth == nest_depth
                    {
                        options = (options & !PARSE_TRACKED_OPTIONS) | top.options;
                        xoptions = (xoptions & !PARSE_TRACKED_EXTRA_OPTIONS) | top.xoptions;
                        if top.flags & NSF_RESET != 0 && top.max_group > self.bracount {
                            self.bracount = top.max_group;
                        }
                        if top.flags & NSF_CONDASSERT != 0 {
                            okquantifier = false;
                        }
                        if top.flags & NSF_ATOMICSR != 0 {
                            self.parsed.push(Meta::Ket);
                        }
                        nests.pop();
                    }
                    if nest_depth == 0 {
                        fail!(UNMATCHED_CLOSING_PARENTHESIS, ptr - 1);
                    }
                    nest_depth -= 1;
                    self.parsed.push(Meta::Ket);
                }

                _ => {
                    self.parsed.push(Meta::Lit(c));
                    okquantifier = true;
                }
            }

            // Quantifier post-processing (CHECK_QUANTIFIER).
            if let Some(q) = quant {
                if !prev_okquantifier {
                    fail!(QUANTIFIER_INVALID, ptr - 1);
                }
                if matches!(self.parsed.get(prev_parsed_item), Some(Meta::Accept)) {
                    self.parsed.insert(verbstartptr, Meta::NoCapture);
                    self.parsed.push(Meta::Ket);
                }
                self.parsed.push(q);
                meta_quantifier = true;
            }
        }

        if inverbname && ptr >= end {
            fail!(VERB_UNKNOWN, ptr);
        }

        self.parsed.push(Meta::End);
        if nest_depth == 0 {
            return Ok(());
        }
        fail!(MISSING_CLOSING_PARENTHESIS, ptr);
    }

    #[allow(clippy::too_many_arguments)]
    fn post_assertion(
        &mut self,
        nest_depth: &mut u32,
        nests: &mut Vec<NestSave>,
        prev_expect_cond_assert: i32,
        options: u32,
        xoptions: u32,
        ptr: usize,
    ) -> PResult<()> {
        *nest_depth += 1;
        if prev_expect_cond_assert > 0 {
            if nests.len() >= NEST_STACK_SIZE {
                return Err((QUERY_BARJX_NEST_TOO_DEEP, ptr));
            }
            nests.push(NestSave {
                nest_depth: *nest_depth,
                reset_group: 0,
                max_group: 0,
                flags: NSF_CONDASSERT,
                options: options & PARSE_TRACKED_OPTIONS,
                xoptions: xoptions & PARSE_TRACKED_EXTRA_OPTIONS,
            });
        }
        Ok(())
    }

    /// Handle everything after "(?". `ptr` points at the character after '?'.
    #[allow(clippy::too_many_arguments)]
    fn parse_query_group(
        &mut self,
        ptrp: &mut usize,
        optionsp: &mut u32,
        xoptionsp: &mut u32,
        nest_depth: &mut u32,
        nests: &mut Vec<NestSave>,
        okquantifier: &mut bool,
        expect_cond_assert: &mut i32,
        prev_expect_cond_assert: i32,
    ) -> PResult<()> {
        let pat = self.pat;
        let end = self.end;
        let mut ptr = *ptrp;
        let mut options = *optionsp;
        let mut xoptions = *xoptionsp;

        macro_rules! fail {
            ($code:expr, $p:expr) => {
                return Err(($code, $p))
            };
        }

        let ch = pat[ptr];
        enum Next {
            Done,
            Recursion(i32),
            RecurseByName,
            DefineName(u8),
            Lookahead(Meta),
            Lookbehind(LbKind),
        }
        let mut next = Next::Done;

        match ch {
            b'P' => {
                ptr += 1;
                if ptr >= end {
                    fail!(MISSING_CLOSING_PARENTHESIS, ptr);
                }
                if pat[ptr] == b'<' {
                    next = Next::DefineName(b'>');
                } else if pat[ptr] == b'>' {
                    next = Next::RecurseByName;
                } else if pat[ptr] != b'=' {
                    fail!(UNRECOGNIZED_AFTER_QUERY_P, ptr);
                } else {
                    let mut p = ptr;
                    match self.read_name(&mut p, b')') {
                        Err(e) => fail!(e, p),
                        Ok((offset, len)) => {
                            ptr = p;
                            self.parsed.push(Meta::BackrefByName { offset, len });
                            *okquantifier = true;
                        }
                    }
                }
            }
            b'R' => {
                ptr += 1;
                if ptr >= end || pat[ptr] != b')' {
                    fail!(PARENS_QUERY_R_MISSING_CLOSING, ptr);
                }
                next = Next::Recursion(0);
            }
            b'+' | b'0'..=b'9' => {
                if ch == b'+' && (end - ptr < 2 || !is_digit(pat[ptr + 1])) {
                    fail!(BAD_RELATIVE_REFERENCE, ptr);
                }
                let i = self.recursion_by_number(&mut ptr)?;
                next = Next::Recursion(i);
            }
            b'-' if end - ptr > 1 && is_digit(pat[ptr + 1]) => {
                let i = self.recursion_by_number(&mut ptr)?;
                next = Next::Recursion(i);
            }
            b'&' => next = Next::RecurseByName,
            b'C' => {
                ptr += 1;
                if ptr >= end {
                    fail!(MISSING_CLOSING_PARENTHESIS, ptr);
                }
                *expect_cond_assert = prev_expect_cond_assert - 1;
                if pat[ptr] != b')' && !is_digit(pat[ptr]) {
                    let startptr = ptr;
                    let delimiter = match pat[ptr] {
                        b'`' => b'`',
                        b'\'' => b'\'',
                        b'"' => b'"',
                        b'^' => b'^',
                        b'%' => b'%',
                        b'#' => b'#',
                        b'$' => b'$',
                        b'{' => b'}',
                        _ => fail!(CALLOUT_BAD_STRING_DELIMITER, ptr),
                    };
                    loop {
                        ptr += 1;
                        if ptr >= end {
                            fail!(CALLOUT_NO_STRING_DELIMITER, startptr);
                        }
                        if pat[ptr] == delimiter {
                            ptr += 1;
                            if ptr >= end || pat[ptr] != delimiter {
                                break;
                            }
                        }
                    }
                } else {
                    let mut n = 0u32;
                    while ptr < end && is_digit(pat[ptr]) {
                        n = n * 10 + (pat[ptr] - b'0') as u32;
                        ptr += 1;
                        if n > 255 {
                            fail!(CALLOUT_NUMBER_TOO_BIG, ptr);
                        }
                    }
                }
                if ptr >= end || pat[ptr] != b')' {
                    fail!(MISSING_CALLOUT_CLOSING, ptr);
                }
                ptr += 1;
                self.parsed.push(Meta::Callout);
            }
            b'(' => {
                ptr += 1;
                if ptr >= end {
                    fail!(MISSING_CLOSING_PARENTHESIS, ptr);
                }
                *nest_depth += 1;
                if pat[ptr] == b'?' || pat[ptr] == b'*' {
                    self.parsed.push(Meta::CondAssert);
                    ptr -= 1;
                    *expect_cond_assert = 2;
                    *ptrp = ptr;
                    return Ok(());
                }
                let mut p = ptr;
                match read_number(
                    pat,
                    &mut p,
                    end,
                    self.bracount as i32,
                    MAX_GROUP_NUMBER,
                    SUBPATTERN_NUMBER_TOO_BIG,
                ) {
                    Err(e) => fail!(e, p),
                    Ok(Some(i)) => {
                        ptr = p;
                        if i <= 0 {
                            fail!(BAD_SUBPATTERN_REFERENCE, ptr);
                        }
                        let offset = ptr - 2;
                        self.parsed.push(Meta::CondNumber {
                            n: i as u32,
                            offset,
                        });
                    }
                    Ok(None) => {
                        if end - ptr >= 10
                            && &pat[ptr..ptr + 7] == b"VERSION"
                            && pat[ptr + 7] != b')'
                        {
                            ptr += 7;
                            let mut ge = false;
                            if pat[ptr] == b'>' {
                                ge = true;
                                ptr += 1;
                            }
                            let bad = |p: usize| Err((VERSION_CONDITION_SYNTAX, p));
                            if pat[ptr] != b'=' {
                                return bad(ptr);
                            }
                            ptr += 1;
                            if ptr >= end || !is_digit(pat[ptr]) {
                                return bad(ptr);
                            }
                            let major = match read_number(
                                pat,
                                &mut ptr,
                                end,
                                -1,
                                1000,
                                VERSION_CONDITION_SYNTAX,
                            ) {
                                Ok(Some(v)) => v as u32,
                                Ok(None) => 0,
                                Err(e) => fail!(e, ptr),
                            };
                            if ptr >= end {
                                return bad(ptr);
                            }
                            let mut minor = 0u32;
                            if pat[ptr] == b'.' {
                                ptr += 1;
                                if ptr >= end || !is_digit(pat[ptr]) {
                                    return bad(ptr);
                                }
                                minor = (pat[ptr] - b'0') as u32 * 10;
                                ptr += 1;
                                if ptr >= end {
                                    return bad(ptr);
                                }
                                if is_digit(pat[ptr]) {
                                    minor += (pat[ptr] - b'0') as u32;
                                    ptr += 1;
                                }
                                if ptr >= end || pat[ptr] != b')' {
                                    return bad(ptr);
                                }
                            }
                            self.parsed.push(Meta::CondVersion { ge, major, minor });
                        } else {
                            let mut was_r_ampersand = false;
                            let terminator;
                            if pat[ptr] == b'R' && end - ptr > 1 && pat[ptr + 1] == b'&' {
                                terminator = b')';
                                was_r_ampersand = true;
                                ptr += 1;
                            } else if pat[ptr] == b'<' {
                                terminator = b'>';
                            } else if pat[ptr] == b'\'' {
                                terminator = b'\'';
                            } else {
                                terminator = b')';
                                ptr -= 1;
                            }
                            let mut p = ptr;
                            let (offset, len) = match self.read_name(&mut p, terminator) {
                                Ok(v) => v,
                                Err(e) => fail!(e, p),
                            };
                            ptr = p;
                            if was_r_ampersand {
                                self.parsed.push(Meta::CondName {
                                    kind: CondNameKind::RName,
                                    offset,
                                    len,
                                });
                                ptr -= 1;
                            } else if terminator == b')' {
                                let name = &pat[offset..offset + len];
                                if name == b"DEFINE" {
                                    self.parsed.push(Meta::CondDefine { offset });
                                } else {
                                    let all_digits = name[1..].iter().all(|b| is_digit(*b));
                                    let kind = if name[0] == b'R' && all_digits {
                                        CondNameKind::RNumber
                                    } else {
                                        CondNameKind::Name
                                    };
                                    self.parsed.push(Meta::CondName { kind, offset, len });
                                }
                                ptr -= 1;
                            } else {
                                self.parsed.push(Meta::CondName {
                                    kind: CondNameKind::Name,
                                    offset,
                                    len,
                                });
                            }
                        }
                    }
                }
                if ptr >= end || pat[ptr] != b')' {
                    fail!(MISSING_CONDITION_CLOSING, ptr);
                }
                ptr += 1;
            }
            b'>' => {
                self.parsed.push(Meta::Atomic);
                *nest_depth += 1;
                ptr += 1;
            }
            b'=' => next = Next::Lookahead(Meta::Lookahead),
            b'*' => next = Next::Lookahead(Meta::LookaheadNa),
            b'!' => next = Next::Lookahead(Meta::LookaheadNot),
            b'<' => {
                if end - ptr <= 1
                    || (pat[ptr + 1] != b'=' && pat[ptr + 1] != b'!' && pat[ptr + 1] != b'*')
                {
                    next = Next::DefineName(b'>');
                } else {
                    next = Next::Lookbehind(match pat[ptr + 1] {
                        b'=' => LbKind::Pos,
                        b'!' => LbKind::Neg,
                        _ => LbKind::NonAtomic,
                    });
                }
            }
            b'\'' => next = Next::DefineName(b'\''),
            _ => {
                // (?| or option setting
                *nest_depth += 1;
                if nests.len() >= NEST_STACK_SIZE {
                    fail!(QUERY_BARJX_NEST_TOO_DEEP, ptr);
                }
                nests.push(NestSave {
                    nest_depth: *nest_depth,
                    reset_group: 0,
                    max_group: 0,
                    flags: 0,
                    options: options & PARSE_TRACKED_OPTIONS,
                    xoptions: xoptions & PARSE_TRACKED_EXTRA_OPTIONS,
                });
                if pat[ptr] == b'|' {
                    let top = nests.last_mut().unwrap();
                    top.reset_group = self.bracount;
                    top.max_group = self.bracount;
                    top.flags |= NSF_RESET;
                    self.external_flags |= FLAG_DUPCAPUSED;
                    self.parsed.push(Meta::NoCapture);
                    ptr += 1;
                } else {
                    let mut hyphenok = true;
                    let oldoptions = options;
                    let oldxoptions = xoptions;
                    let mut set = 0u32;
                    let mut unset = 0u32;
                    let mut xset = 0u32;
                    let mut xunset = 0u32;
                    let mut in_unset = false;

                    if ptr < end && pat[ptr] == b'^' {
                        options &= !(PCRE2_CASELESS
                            | PCRE2_MULTILINE
                            | PCRE2_NO_AUTO_CAPTURE
                            | PCRE2_DOTALL
                            | PCRE2_EXTENDED
                            | PCRE2_EXTENDED_MORE);
                        xoptions &= !PCRE2_EXTRA_CASELESS_RESTRICT;
                        hyphenok = false;
                        ptr += 1;
                    }
                    while ptr < end && pat[ptr] != b')' && pat[ptr] != b':' {
                        let oc = pat[ptr];
                        ptr += 1;
                        let (optset, xoptset) = if in_unset {
                            (&mut unset, &mut xunset)
                        } else {
                            (&mut set, &mut xset)
                        };
                        match oc {
                            b'-' => {
                                if !hyphenok {
                                    fail!(INVALID_HYPHEN_IN_OPTIONS, ptr - 1);
                                }
                                in_unset = true;
                                hyphenok = false;
                            }
                            b'a' => {
                                let mut handled = false;
                                if ptr < end {
                                    let bit = match pat[ptr] {
                                        b'D' => Some(PCRE2_EXTRA_ASCII_BSD),
                                        b'P' => {
                                            Some(PCRE2_EXTRA_ASCII_POSIX | PCRE2_EXTRA_ASCII_DIGIT)
                                        }
                                        b'S' => Some(PCRE2_EXTRA_ASCII_BSS),
                                        b'T' => Some(PCRE2_EXTRA_ASCII_DIGIT),
                                        b'W' => Some(PCRE2_EXTRA_ASCII_BSW),
                                        _ => None,
                                    };
                                    if let Some(b) = bit {
                                        *xoptset |= b;
                                        ptr += 1;
                                        handled = true;
                                    }
                                }
                                if !handled {
                                    *xoptset |= PCRE2_EXTRA_ASCII_BSD
                                        | PCRE2_EXTRA_ASCII_BSS
                                        | PCRE2_EXTRA_ASCII_BSW
                                        | PCRE2_EXTRA_ASCII_DIGIT
                                        | PCRE2_EXTRA_ASCII_POSIX;
                                }
                            }
                            b'J' => {
                                *optset |= PCRE2_DUPNAMES;
                                self.external_flags |= FLAG_JCHANGED;
                            }
                            b'i' => *optset |= PCRE2_CASELESS,
                            b'm' => *optset |= PCRE2_MULTILINE,
                            b'n' => *optset |= PCRE2_NO_AUTO_CAPTURE,
                            b'r' => *xoptset |= PCRE2_EXTRA_CASELESS_RESTRICT,
                            b's' => *optset |= PCRE2_DOTALL,
                            b'U' => *optset |= PCRE2_UNGREEDY,
                            b'x' => {
                                *optset |= PCRE2_EXTENDED;
                                if ptr < end && pat[ptr] == b'x' {
                                    *optset |= PCRE2_EXTENDED_MORE;
                                    ptr += 1;
                                }
                            }
                            _ => fail!(INVALID_AFTER_PARENS_QUERY, ptr - 1),
                        }
                    }
                    if (set & (PCRE2_EXTENDED | PCRE2_EXTENDED_MORE)) == PCRE2_EXTENDED
                        || unset & PCRE2_EXTENDED != 0
                    {
                        unset |= PCRE2_EXTENDED_MORE;
                    }
                    options = (options | set) & !unset;
                    xoptions = (xoptions | xset) & !xunset;

                    if ptr >= end {
                        fail!(MISSING_CLOSING_PARENTHESIS, ptr);
                    }
                    let closer = pat[ptr];
                    ptr += 1;
                    if closer == b')' {
                        *nest_depth -= 1;
                        let n = nests.len();
                        if n > 1 && nests[n - 2].nest_depth == *nest_depth {
                            nests.pop();
                        } else {
                            nests[n - 1].nest_depth = *nest_depth;
                        }
                    } else {
                        self.parsed.push(Meta::NoCapture);
                    }
                    if options != oldoptions || xoptions != oldxoptions {
                        self.parsed.push(Meta::Options { options, xoptions });
                    }
                }
            }
        }

        match next {
            Next::Done => {}
            Next::Recursion(i) => {
                // SET_RECURSION
                let offset = ptr;
                ptr += 1;
                self.parsed.push(Meta::Recurse {
                    n: i as u32,
                    offset,
                });
                *okquantifier = true;
            }
            Next::RecurseByName => {
                let mut p = ptr;
                match self.read_name(&mut p, b')') {
                    Err(e) => fail!(e, p),
                    Ok((offset, len)) => {
                        ptr = p;
                        self.parsed.push(Meta::RecurseByName { offset, len });
                        *okquantifier = true;
                    }
                }
            }
            Next::Lookahead(m) => {
                self.parsed.push(m);
                ptr += 1;
                self.post_assertion(
                    nest_depth,
                    nests,
                    prev_expect_cond_assert,
                    options,
                    xoptions,
                    ptr,
                )?;
            }
            Next::Lookbehind(kind) => {
                self.has_lookbehind = true;
                let offset = ptr - 2;
                self.parsed.push(Meta::Lookbehind {
                    kind,
                    offset,
                    len: 0,
                    minlen: 0,
                });
                ptr += 2;
                self.post_assertion(
                    nest_depth,
                    nests,
                    prev_expect_cond_assert,
                    options,
                    xoptions,
                    ptr,
                )?;
            }
            Next::DefineName(terminator) => {
                let mut p = ptr;
                let (offset, len) = match self.read_name(&mut p, terminator) {
                    Ok(v) => v,
                    Err(e) => fail!(e, p),
                };
                ptr = p;
                if self.bracount >= MAX_GROUP_NUMBER {
                    fail!(TOO_MANY_CAPTURES, ptr);
                }
                self.bracount += 1;
                self.parsed.push(Meta::Capture(self.bracount));
                *nest_depth += 1;
                if self.named_groups.len() >= MAX_NAME_COUNT {
                    fail!(TOO_MANY_NAMED_SUBPATTERNS, ptr);
                }
                let name = &pat[offset..offset + len];
                let mut isdupname = false;
                let mut ignore = false;
                let bracount = self.bracount;
                for ng in self.named_groups.iter_mut() {
                    if ng.name == name {
                        if ng.number == bracount {
                            ignore = true;
                            break;
                        }
                        if options & PCRE2_DUPNAMES == 0 {
                            fail!(DUPLICATE_SUBPATTERN_NAME, ptr);
                        }
                        ng.isdup = true;
                        isdupname = true;
                    } else if ng.number == bracount {
                        fail!(SUBPATTERN_NAMES_MISMATCH, ptr);
                    }
                }
                if !ignore {
                    self.named_groups.push(NamedGroup {
                        name: name.to_vec(),
                        number: bracount,
                        isdup: isdupname,
                    });
                }
            }
        }

        *ptrp = ptr;
        *optionsp = options;
        *xoptionsp = xoptions;
        Ok(())
    }

    /// RECURSION_BYNUMBER: `ptr` at the sign or first digit.
    fn recursion_by_number(&self, ptr: &mut usize) -> PResult<i32> {
        let pat = self.pat;
        let end = self.end;
        let allow = if is_digit(pat[*ptr]) {
            -1
        } else {
            self.bracount as i32
        };
        let mut p = *ptr;
        let i = match read_number(
            pat,
            &mut p,
            end,
            allow,
            MAX_GROUP_NUMBER,
            SUBPATTERN_NUMBER_TOO_BIG,
        ) {
            Err(e) => return Err((e, p)),
            Ok(None) => return Err((0, p)), // cannot happen: we know a digit follows
            Ok(Some(i)) => i,
        };
        if i < 0 {
            return Err((BAD_SUBPATTERN_REFERENCE, p - 1));
        }
        if p >= end || pat[p] != b')' {
            return Err((MISSING_CLOSING_PARENTHESIS, p));
        }
        *ptr = p;
        Ok(i)
    }

    /// Parse a character class; `ptr` is after the '['.
    fn parse_class(
        &mut self,
        ptrp: &mut usize,
        options: u32,
        xoptions: u32,
        inescq: &mut bool,
    ) -> PResult<()> {
        let pat = self.pat;
        let end = self.end;
        let mut ptr = *ptrp;

        macro_rules! fail {
            ($code:expr, $p:expr) => {
                return Err(($code, $p))
            };
        }

        if ptr < end
            && (pat[ptr] == b':' || pat[ptr] == b'.' || pat[ptr] == b'=')
            && check_posix_syntax(pat, ptr, end).is_some()
        {
            fail!(
                if pat[ptr] == b':' {
                    POSIX_CLASS_NOT_IN_CLASS
                } else {
                    POSIX_NO_SUPPORT_COLLATING
                },
                ptr - 1
            );
        }

        let mut negate_class = false;
        // As in PCRE2, `c` keeps the last character read (initially '[').
        let mut c: u32 = b'[' as u32;
        while ptr < end {
            let (ch, l) = self.getchar(ptr);
            ptr += l;
            c = ch;
            if c == b'\\' as u32 {
                if ptr < end && pat[ptr] == b'E' {
                    ptr += 1;
                } else if end - ptr >= 3 && &pat[ptr..ptr + 3] == b"Q\\E" {
                    ptr += 3;
                } else {
                    break;
                }
            } else if options & PCRE2_EXTENDED_MORE != 0 && (c == b' ' as u32 || c == b'\t' as u32)
            {
                continue;
            } else if !negate_class && c == b'^' as u32 {
                negate_class = true;
            } else {
                break;
            }
        }

        if c == b']' as u32 && self.external_options & PCRE2_ALLOW_EMPTY_CLASS != 0 {
            self.parsed.push(Meta::ClassEmpty(negate_class));
            *ptrp = ptr;
            return Ok(());
        }

        self.parsed.push(Meta::Class(negate_class));
        let mut class_range_state = RangeState::No;

        loop {
            let mut char_is_literal = true;
            // The body of the PCRE2 class loop; "goto CLASS_CONTINUE" is `continue_class`.
            let mut literal: Option<u32> = None;

            if *inescq {
                if c == b'\\' as u32 && ptr < end && pat[ptr] == b'E' {
                    *inescq = false;
                    ptr += 1;
                } else {
                    literal = Some(c);
                }
            } else if options & PCRE2_EXTENDED_MORE != 0 && (c == b' ' as u32 || c == b'\t' as u32)
            {
                // skip
            } else if c == b'[' as u32
                && end - ptr >= 3
                && (pat[ptr] == b':' || pat[ptr] == b'.' || pat[ptr] == b'=')
                && check_posix_syntax(pat, ptr, end).is_some()
            {
                let tempptr = check_posix_syntax(pat, ptr, end).unwrap();
                if class_range_state == RangeState::Started {
                    fail!(CLASS_INVALID_RANGE, ptr);
                }
                if pat[ptr] != b':' {
                    fail!(POSIX_NO_SUPPORT_COLLATING, ptr - 1);
                }
                ptr += 1;
                let mut posix_negate = false;
                if pat[ptr] == b'^' {
                    posix_negate = true;
                    ptr += 1;
                }
                let Some(posix_class) = check_posix_name(&pat[ptr..tempptr]) else {
                    fail!(UNKNOWN_POSIX_CLASS, ptr);
                };
                ptr = tempptr + 2;
                if ptr + 1 < end && pat[ptr] == b'-' && pat[ptr + 1] != b']' {
                    fail!(CLASS_INVALID_RANGE, ptr);
                }
                class_range_state = RangeState::No;

                let mut done = false;
                if options & PCRE2_UCP != 0
                    && xoptions & PCRE2_EXTRA_ASCII_POSIX == 0
                    && !(xoptions & PCRE2_EXTRA_ASCII_DIGIT != 0
                        && (posix_class == PC_DIGIT || posix_class == PC_XDIGIT))
                {
                    let (ptype, pvalue) = POSIX_SUBSTITUTES[posix_class as usize];
                    if ptype >= 0 {
                        self.parsed.push(Meta::Prop {
                            negated: posix_negate,
                            ptype: ptype as u8,
                            pvalue,
                        });
                        done = true;
                    } else if pvalue != 0 {
                        self.parsed.push(Meta::Escape(if posix_negate {
                            Esc::UpperH
                        } else {
                            Esc::LowerH
                        }));
                        done = true;
                    }
                }
                if !done {
                    self.parsed.push(Meta::Posix {
                        negated: posix_negate,
                        class: posix_class,
                    });
                }
            } else if c == b'-' as u32 && class_range_state >= RangeState::OkEscaped {
                self.parsed
                    .push(if class_range_state == RangeState::OkLiteral {
                        Meta::RangeLiteral
                    } else {
                        Meta::RangeEscaped
                    });
                class_range_state = RangeState::Started;
            } else if c != b'\\' as u32 {
                literal = Some(c);
            } else {
                // Escape in a class
                let mut p = ptr;
                let r = self.check_escape(&mut p, options, xoptions, true);
                ptr = p;
                match r {
                    Err(e) => fail!(e, ptr),
                    Ok(EscapeValue::Char(ch)) => {
                        char_is_literal = false;
                        literal = Some(ch);
                    }
                    Ok(EscapeValue::Backref(_)) => {
                        // Cannot happen inside a class (digits are octal).
                        fail!(ESCAPE_INVALID_IN_CLASS, ptr - 1);
                    }
                    Ok(EscapeValue::Special(esc)) => match esc {
                        Esc::LowerB => {
                            char_is_literal = false;
                            literal = Some(0x08);
                        }
                        Esc::Q => *inescq = true,
                        Esc::E => {}
                        Esc::UpperB | Esc::R | Esc::X => fail!(ESCAPE_INVALID_IN_CLASS, ptr - 1),
                        _ => {
                            if class_range_state == RangeState::Started {
                                fail!(CLASS_INVALID_RANGE, ptr);
                            }
                            class_range_state = RangeState::No;
                            match esc {
                                Esc::N => fail!(BACKSLASH_N_IN_CLASS, ptr),
                                Esc::UpperH | Esc::LowerH | Esc::UpperV | Esc::LowerV => {
                                    self.parsed.push(Meta::Escape(esc));
                                }
                                Esc::UpperD
                                | Esc::LowerD
                                | Esc::UpperS
                                | Esc::LowerS
                                | Esc::UpperW
                                | Esc::LowerW => {
                                    self.push_escdsw(esc, options, xoptions);
                                }
                                Esc::UpperP | Esc::LowerP => {
                                    self.external_flags |= FLAG_HASBKPORX;
                                    let mut p = ptr;
                                    match self.get_ucp(&mut p) {
                                        Err(e) => fail!(e, p),
                                        Ok((neg, ptype, pvalue)) => {
                                            ptr = p;
                                            self.parsed.push(Meta::Prop {
                                                negated: (esc == Esc::UpperP) != neg,
                                                ptype,
                                                pvalue,
                                            });
                                        }
                                    }
                                }
                                _ => fail!(ESCAPE_INVALID_IN_CLASS, ptr - 1),
                            }
                            if ptr + 1 < end && pat[ptr] == b'-' && pat[ptr + 1] != b']' {
                                fail!(CLASS_INVALID_RANGE, ptr);
                            }
                        }
                    },
                }
            }

            // CLASS_LITERAL handling
            if let Some(lc) = literal {
                if class_range_state == RangeState::Started {
                    let n = self.parsed.len();
                    let start_char = match self.parsed[n - 2] {
                        Meta::Lit(x) => x,
                        _ => 0,
                    };
                    if lc == start_char {
                        self.parsed.pop(); // remove range meta: one-char range
                    } else if start_char > lc {
                        fail!(CLASS_RANGE_ORDER, ptr - 1);
                    } else {
                        if !char_is_literal && self.parsed[n - 1] == Meta::RangeLiteral {
                            self.parsed[n - 1] = Meta::RangeEscaped;
                        }
                        self.parsed.push(Meta::Lit(lc));
                    }
                    class_range_state = RangeState::No;
                } else {
                    class_range_state = if char_is_literal {
                        RangeState::OkLiteral
                    } else {
                        RangeState::OkEscaped
                    };
                    self.parsed.push(Meta::Lit(lc));
                }
            }

            // CLASS_CONTINUE
            if ptr >= end {
                fail!(MISSING_SQUARE_BRACKET, ptr);
            }
            let (ch, l) = self.getchar(ptr);
            ptr += l;
            c = ch;
            if c == b']' as u32 && !*inescq {
                break;
            }
        }

        if class_range_state == RangeState::Started {
            let n = self.parsed.len();
            self.parsed[n - 1] = Meta::Lit(b'-' as u32);
        }
        self.parsed.push(Meta::ClassEnd);
        *ptrp = ptr;
        Ok(())
    }
}

#[inline]
fn is_c_isspace(c: u8) -> bool {
    matches!(c, 0x09..=0x0d | 0x20)
}
