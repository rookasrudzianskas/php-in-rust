//! PCRE2-compatible regular expression engine used by ext/pcre.
//!
//! This is a pure-Rust port of the parts of PCRE2 10.44 that PHP's `preg_*`
//! functions use: the pattern compiler (with PCRE2's exact error codes, texts
//! and offsets), the backtracking interpreter (`pcre2_match()` without JIT,
//! with PCRE2's match/depth limit accounting), auto-possessification and the
//! start-of-match optimisations, plus Unicode 15 property tables.
//!
//! # Overview
//!
//! ```
//! use php_regex::{CompileFlags, MatchOptions, Regex};
//!
//! let re = Regex::new(b"(?<year>\\d{4})-(\\d\\d)", CompileFlags::empty()).unwrap();
//! let caps = re
//!     .match_at(b"on 2024-05!", 0, MatchOptions::default())
//!     .unwrap()
//!     .unwrap();
//! assert_eq!(caps.get(0), Some((3, 10)));
//! assert_eq!(caps.get(1), Some((3, 7)));
//! assert_eq!(re.capture_count(), 2);
//! assert_eq!(re.capture_names(), vec![(1, b"year".to_vec())]);
//! ```
//!
//! PHP-style patterns (delimiters and modifiers) are handled by
//! [`compile_php_pattern`], which mirrors `pcre_get_compiled_regex_cache_ex()`:
//!
//! ```
//! use php_regex::compile_php_pattern;
//!
//! let p = compile_php_pattern(b"/h(e)llo/iu").unwrap();
//! assert!(p.modifiers.utf);
//! assert!(p.regex.is_match(b"HELLO"));
//! let e = compile_php_pattern(b"/(ab/").unwrap_err();
//! assert_eq!(e.to_string(), "Compilation failed: missing closing parenthesis at offset 3");
//! ```
//!
//! # Matching semantics
//!
//! * Subjects are byte strings and all offsets are byte offsets. With
//!   [`CompileFlags::UTF`] the pattern and subject are UTF-8 and matching works
//!   on code points; invalid subjects give [`MatchError::BadUtf8`] and start
//!   offsets inside a character give [`MatchError::BadUtf8Offset`]
//!   (PHP's `PREG_BAD_UTF8_ERROR` / `PREG_BAD_UTF8_OFFSET_ERROR`).
//! * [`MatchOptions::match_limit`] / [`MatchOptions::depth_limit`] are
//!   PCRE2's match and depth limits (`pcre.backtrack_limit`,
//!   `pcre.recursion_limit`); they count exactly what PCRE2's interpreter
//!   counts.
//! * [`Captures`] reports every group; unset groups are `None`, and
//!   [`Captures::count`] is PCRE2's return value (one more than the highest
//!   group that was set), which PHP uses to trim trailing unset groups.

#![forbid(unsafe_code)]

mod chars;
mod compile;
mod error;
mod exec;
mod lookbehind;
mod ops;
mod opts;
mod parse;
mod php;
mod possess;
mod scriptrun;
mod study;
pub mod unicode;

use std::fmt;
use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

pub use error::{CompileError, MatchError, PregError, codes as error_codes, error_message};
pub use php::{
    PhpModifiers, PhpPattern, PhpPatternError, compile_php_pattern, parse_php_pattern, preg_quote,
};

/// Compile options (a bit set). The values are PCRE2's option bits, except
/// [`CompileFlags::CASELESS_RESTRICT`] which is PCRE2's
/// `PCRE2_EXTRA_CASELESS_RESTRICT` extra option (PHP's `r` modifier).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CompileFlags(u64);

impl CompileFlags {
    /// `i`: caseless matching.
    pub const CASELESS: CompileFlags = CompileFlags(opts::PCRE2_CASELESS as u64);
    /// `m`: `^`/`$` match at internal newlines.
    pub const MULTILINE: CompileFlags = CompileFlags(opts::PCRE2_MULTILINE as u64);
    /// `s`: `.` matches newlines.
    pub const DOTALL: CompileFlags = CompileFlags(opts::PCRE2_DOTALL as u64);
    /// `x`: ignore white space and `#` comments.
    pub const EXTENDED: CompileFlags = CompileFlags(opts::PCRE2_EXTENDED as u64);
    /// `xx`: extended, also ignoring space and tab in classes.
    pub const EXTENDED_MORE: CompileFlags = CompileFlags(opts::PCRE2_EXTENDED_MORE as u64);
    /// `A`: anchored at the start offset.
    pub const ANCHORED: CompileFlags = CompileFlags(opts::PCRE2_ANCHORED as u64);
    /// `D`: `$` matches only at the very end.
    pub const DOLLAR_ENDONLY: CompileFlags = CompileFlags(opts::PCRE2_DOLLAR_ENDONLY as u64);
    /// `U`: invert greediness.
    pub const UNGREEDY: CompileFlags = CompileFlags(opts::PCRE2_UNGREEDY as u64);
    /// UTF-8 mode.
    pub const UTF: CompileFlags = CompileFlags(opts::PCRE2_UTF as u64);
    /// Unicode properties for `\d`, `\w`, `\s`, `\b` and POSIX classes.
    pub const UCP: CompileFlags = CompileFlags(opts::PCRE2_UCP as u64);
    /// `J`: allow duplicate group names.
    pub const DUPNAMES: CompileFlags = CompileFlags(opts::PCRE2_DUPNAMES as u64);
    /// `n`: plain parentheses do not capture.
    pub const NO_AUTO_CAPTURE: CompileFlags = CompileFlags(opts::PCRE2_NO_AUTO_CAPTURE as u64);
    /// Forbid `\C` (PHP sets it together with UTF).
    pub const NEVER_BACKSLASH_C: CompileFlags = CompileFlags(opts::PCRE2_NEVER_BACKSLASH_C as u64);
    /// Forbid UTF mode (also via `(*UTF)`).
    pub const NEVER_UTF: CompileFlags = CompileFlags(opts::PCRE2_NEVER_UTF as u64);
    /// Forbid UCP mode.
    pub const NEVER_UCP: CompileFlags = CompileFlags(opts::PCRE2_NEVER_UCP as u64);
    /// Disable auto-possessification.
    pub const NO_AUTO_POSSESS: CompileFlags = CompileFlags(opts::PCRE2_NO_AUTO_POSSESS as u64);
    /// Disable the start-of-match optimisations.
    pub const NO_START_OPTIMIZE: CompileFlags = CompileFlags(opts::PCRE2_NO_START_OPTIMIZE as u64);
    /// Disable automatic anchoring for `.*`.
    pub const NO_DOTSTAR_ANCHOR: CompileFlags = CompileFlags(opts::PCRE2_NO_DOTSTAR_ANCHOR as u64);
    /// A back reference to an unset group matches the empty string.
    pub const MATCH_UNSET_BACKREF: CompileFlags =
        CompileFlags(opts::PCRE2_MATCH_UNSET_BACKREF as u64);
    /// Allow `[]` (never matches) and `[^]` (matches any character).
    pub const ALLOW_EMPTY_CLASS: CompileFlags = CompileFlags(opts::PCRE2_ALLOW_EMPTY_CLASS as u64);
    /// Skip the UTF-8 validity check of the pattern.
    pub const NO_UTF_CHECK: CompileFlags = CompileFlags(opts::PCRE2_NO_UTF_CHECK as u64);
    /// `r`: caseless matching never mixes ASCII and non-ASCII characters.
    pub const CASELESS_RESTRICT: CompileFlags =
        CompileFlags((opts::PCRE2_EXTRA_CASELESS_RESTRICT as u64) << 32);

    /// No flags.
    pub const fn empty() -> CompileFlags {
        CompileFlags(0)
    }

    /// Does `self` contain all of `other`?
    pub const fn contains(self, other: CompileFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Add flags.
    pub fn insert(&mut self, other: CompileFlags) {
        self.0 |= other.0;
    }

    /// Build flags from raw PCRE2 compile option bits and PCRE2 "extra"
    /// option bits (`pcre2_set_compile_extra_options()`).
    pub const fn from_pcre2(options: u32, extra_options: u32) -> CompileFlags {
        CompileFlags(options as u64 | ((extra_options as u64) << 32))
    }

    /// The raw PCRE2 compile option bits.
    pub const fn pcre2_options(self) -> u32 {
        self.0 as u32
    }

    /// The raw PCRE2 extra option bits.
    pub const fn pcre2_extra_options(self) -> u32 {
        (self.0 >> 32) as u32
    }

    fn options(self) -> u32 {
        self.0 as u32
    }

    fn extra_options(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

impl BitOr for CompileFlags {
    type Output = CompileFlags;
    fn bitor(self, rhs: CompileFlags) -> CompileFlags {
        CompileFlags(self.0 | rhs.0)
    }
}

impl BitOrAssign for CompileFlags {
    fn bitor_assign(&mut self, rhs: CompileFlags) {
        self.0 |= rhs.0;
    }
}

/// PHP's default `pcre.backtrack_limit`.
pub const DEFAULT_BACKTRACK_LIMIT: u32 = 1_000_000;
/// PHP's default `pcre.recursion_limit`.
pub const DEFAULT_RECURSION_LIMIT: u32 = 100_000;
/// PCRE2's default heap limit in KiB (as configured by PHP's bundled PCRE2).
pub const DEFAULT_HEAP_LIMIT: u32 = 20_000_000;

/// Options for one match attempt (`pcre2_match()` options and limits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchOptions {
    /// `PCRE2_ANCHORED`: the match must start at the start offset.
    pub anchored: bool,
    /// `PCRE2_NOTEMPTY`: an empty match is not valid.
    pub not_empty: bool,
    /// `PCRE2_NOTEMPTY_ATSTART`: an empty match at the start offset is not valid.
    pub not_empty_atstart: bool,
    /// `PCRE2_NOTBOL`: the subject start is not the beginning of a line.
    pub not_bol: bool,
    /// `PCRE2_NOTEOL`: the subject end is not the end of a line.
    pub not_eol: bool,
    /// `PCRE2_NO_UTF_CHECK`: the caller guarantees the subject is valid UTF-8.
    pub no_utf_check: bool,
    /// Match (backtrack) limit; PHP's `pcre.backtrack_limit`.
    pub match_limit: u32,
    /// Depth limit; PHP's `pcre.recursion_limit`.
    pub depth_limit: u32,
    /// Heap limit in KiB.
    pub heap_limit: u32,
}

impl Default for MatchOptions {
    /// No flags, and PHP's default limits.
    fn default() -> Self {
        MatchOptions {
            anchored: false,
            not_empty: false,
            not_empty_atstart: false,
            not_bol: false,
            not_eol: false,
            no_utf_check: false,
            match_limit: DEFAULT_BACKTRACK_LIMIT,
            depth_limit: DEFAULT_RECURSION_LIMIT,
            heap_limit: DEFAULT_HEAP_LIMIT,
        }
    }
}

impl MatchOptions {
    fn exec_options(&self) -> exec::ExecOptions {
        let mut m = 0;
        if self.anchored {
            m |= opts::PCRE2_ANCHORED;
        }
        if self.not_empty {
            m |= opts::PCRE2_NOTEMPTY;
        }
        if self.not_empty_atstart {
            m |= opts::PCRE2_NOTEMPTY_ATSTART;
        }
        if self.not_bol {
            m |= opts::PCRE2_NOTBOL;
        }
        if self.not_eol {
            m |= opts::PCRE2_NOTEOL;
        }
        exec::ExecOptions {
            moptions: m,
            match_limit: self.match_limit,
            depth_limit: self.depth_limit,
            heap_limit: self.heap_limit,
            no_utf_check: self.no_utf_check,
        }
    }
}

/// The result of a successful match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captures {
    groups: Vec<Option<(usize, usize)>>,
    count: usize,
    mark: Option<Vec<u8>>,
}

impl Captures {
    /// Byte range of group `i` (0 = whole match), or `None` if the group is
    /// unset (or does not exist). An empty match is `Some((n, n))`.
    pub fn get(&self, i: usize) -> Option<(usize, usize)> {
        self.groups.get(i).copied().flatten()
    }

    /// Number of slots: `capture_count() + 1`.
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Always false (group 0 always exists).
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// PCRE2's return value: one more than the number of the highest group
    /// that was set (1 if only the whole match is set).
    pub fn count(&self) -> usize {
        self.count
    }

    /// The (*MARK) name in effect at the end of the match, if any.
    pub fn mark(&self) -> Option<&[u8]> {
        self.mark.as_deref()
    }

    /// Start of the whole match. Note that `\K` can make it differ from the
    /// position where matching started.
    pub fn start(&self) -> usize {
        self.groups[0].map_or(0, |g| g.0)
    }

    /// End of the whole match.
    pub fn end(&self) -> usize {
        self.groups[0].map_or(0, |g| g.1)
    }

    /// Iterate over all groups.
    pub fn iter(&self) -> impl Iterator<Item = Option<(usize, usize)>> + '_ {
        self.groups.iter().copied()
    }
}

/// A compiled regular expression.
#[derive(Clone)]
pub struct Regex {
    prog: Arc<compile::Program>,
}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Regex")
            .field("capture_count", &self.capture_count())
            .field("utf", &self.is_utf())
            .finish()
    }
}

impl Regex {
    /// Compile `pattern` (the regex proper, without PHP delimiters) like
    /// `pcre2_compile()`.
    pub fn new(pattern: &[u8], flags: CompileFlags) -> Result<Regex, CompileError> {
        let prog = compile::compile(pattern, flags.options(), flags.extra_options())?;
        Ok(Regex {
            prog: Arc::new(prog),
        })
    }

    /// Match against `subject` starting at byte offset `start` (like
    /// `pcre2_match()`). Returns `Ok(None)` when there is no match.
    pub fn match_at(
        &self,
        subject: &[u8],
        start: usize,
        opts: MatchOptions,
    ) -> Result<Option<Captures>, MatchError> {
        let (res, _) = exec::pcre2_match(&self.prog, subject, start, opts.exec_options())?;
        Ok(res.map(|r| self.make_captures(r)))
    }

    /// Like [`Regex::match_at`], but on failure also returns the (*MARK)
    /// name that was last passed (PCRE2's "no match" mark).
    pub fn match_at_with_mark(
        &self,
        subject: &[u8],
        start: usize,
        opts: MatchOptions,
    ) -> Result<Result<Captures, Option<Vec<u8>>>, MatchError> {
        let (res, mark) = exec::pcre2_match(&self.prog, subject, start, opts.exec_options())?;
        Ok(match res {
            Some(r) => Ok(self.make_captures(r)),
            None => Err(mark.map(|m| self.prog.marks[m as usize].clone())),
        })
    }

    /// Convenience: does the regex match anywhere in `subject`?
    pub fn is_match(&self, subject: &[u8]) -> bool {
        matches!(
            self.match_at(subject, 0, MatchOptions::default()),
            Ok(Some(_))
        )
    }

    fn make_captures(&self, r: exec::ExecResult) -> Captures {
        let n = self.prog.top_bracket as usize + 1;
        let mut groups = Vec::with_capacity(n);
        for i in 0..n {
            let (s, e) = (r.ovector[2 * i], r.ovector[2 * i + 1]);
            groups.push(if s == exec::UNSET || i >= r.count {
                None
            } else {
                Some((s, e))
            });
        }
        Captures {
            groups,
            count: r.count,
            mark: r.mark.map(|m| self.prog.marks[m as usize].clone()),
        }
    }

    /// Number of capturing groups (`PCRE2_INFO_CAPTURECOUNT`).
    pub fn capture_count(&self) -> usize {
        self.prog.top_bracket as usize
    }

    /// The name table (`PCRE2_INFO_NAMETABLE`) in PCRE2's order: sorted by
    /// name, duplicates (with `J`) in order of appearance.
    pub fn capture_names(&self) -> Vec<(usize, Vec<u8>)> {
        self.prog
            .name_table
            .iter()
            .map(|(n, name)| (*n as usize, name.clone()))
            .collect()
    }

    /// Number of named groups (`PCRE2_INFO_NAMECOUNT`).
    pub fn name_count(&self) -> usize {
        self.prog.name_table.len()
    }

    /// Group number for `name` (the first one in the name table).
    pub fn group_number(&self, name: &[u8]) -> Option<usize> {
        self.prog
            .name_table
            .iter()
            .find(|(_, n)| n == name)
            .map(|(g, _)| *g as usize)
    }

    /// Is the pattern in UTF mode (from flags or `(*UTF)`)?
    pub fn is_utf(&self) -> bool {
        self.prog.utf
    }

    /// The highest back reference number (`PCRE2_INFO_BACKREFMAX`).
    pub fn backref_max(&self) -> usize {
        self.prog.top_backref as usize
    }

    /// Whether the pattern ended up anchored (explicitly or implicitly).
    pub fn is_anchored(&self) -> bool {
        self.prog.overall_options & opts::PCRE2_ANCHORED != 0
    }

    /// Minimum subject length (`PCRE2_INFO_MINLENGTH`).
    pub fn min_length(&self) -> usize {
        self.prog.minlength as usize
    }

    /// The length of the compiled code in PCRE2 code units (approximately
    /// `PCRE2_INFO_SIZE` minus the header).
    pub fn code_size(&self) -> usize {
        self.prog
            .ops
            .iter()
            .map(|o| o.pcre2_len(self.prog.utf, &self.prog.marks))
            .sum()
    }
}

#[cfg(test)]
mod tests;
