//! Compile and match error types, with PCRE2's exact message texts.

use std::fmt;

/// PCRE2 compile error codes (the numeric values PCRE2 reports, i.e. 100 +
/// the internal `ERRn` number). Only the ones this engine can produce are
/// listed, but [`CompileError::message`] knows every PCRE2 10.44 text.
#[allow(missing_docs)]
pub mod codes {
    pub const END_BACKSLASH: i32 = 101;
    pub const END_BACKSLASH_C: i32 = 102;
    pub const UNKNOWN_ESCAPE: i32 = 103;
    pub const QUANTIFIER_OUT_OF_ORDER: i32 = 104;
    pub const QUANTIFIER_TOO_BIG: i32 = 105;
    pub const MISSING_SQUARE_BRACKET: i32 = 106;
    pub const ESCAPE_INVALID_IN_CLASS: i32 = 107;
    pub const CLASS_RANGE_ORDER: i32 = 108;
    pub const QUANTIFIER_INVALID: i32 = 109;
    pub const INTERNAL_UNEXPECTED_REPEAT: i32 = 110;
    pub const INVALID_AFTER_PARENS_QUERY: i32 = 111;
    pub const POSIX_CLASS_NOT_IN_CLASS: i32 = 112;
    pub const POSIX_NO_SUPPORT_COLLATING: i32 = 113;
    pub const MISSING_CLOSING_PARENTHESIS: i32 = 114;
    pub const BAD_SUBPATTERN_REFERENCE: i32 = 115;
    pub const MISSING_COMMENT_CLOSING: i32 = 118;
    pub const PARENTHESES_NEST_TOO_DEEP: i32 = 119;
    pub const PATTERN_TOO_LARGE: i32 = 120;
    pub const UNMATCHED_CLOSING_PARENTHESIS: i32 = 122;
    pub const MISSING_CONDITION_CLOSING: i32 = 124;
    pub const LOOKBEHIND_NOT_FIXED_LENGTH: i32 = 125;
    pub const ZERO_RELATIVE_REFERENCE: i32 = 126;
    pub const TOO_MANY_CONDITION_BRANCHES: i32 = 127;
    pub const CONDITION_ASSERTION_EXPECTED: i32 = 128;
    pub const BAD_RELATIVE_REFERENCE: i32 = 129;
    pub const UNKNOWN_POSIX_CLASS: i32 = 130;
    pub const CODE_POINT_TOO_BIG: i32 = 134;
    pub const LOOKBEHIND_TOO_COMPLICATED: i32 = 135;
    pub const LOOKBEHIND_INVALID_BACKSLASH_C: i32 = 136;
    pub const UNSUPPORTED_ESCAPE_SEQUENCE: i32 = 137;
    pub const CALLOUT_NUMBER_TOO_BIG: i32 = 138;
    pub const MISSING_CALLOUT_CLOSING: i32 = 139;
    pub const ESCAPE_INVALID_IN_VERB: i32 = 140;
    pub const UNRECOGNIZED_AFTER_QUERY_P: i32 = 141;
    pub const MISSING_NAME_TERMINATOR: i32 = 142;
    pub const DUPLICATE_SUBPATTERN_NAME: i32 = 143;
    pub const INVALID_SUBPATTERN_NAME: i32 = 144;
    pub const MALFORMED_UNICODE_PROPERTY: i32 = 146;
    pub const UNKNOWN_UNICODE_PROPERTY: i32 = 147;
    pub const SUBPATTERN_NAME_TOO_LONG: i32 = 148;
    pub const TOO_MANY_NAMED_SUBPATTERNS: i32 = 149;
    pub const CLASS_INVALID_RANGE: i32 = 150;
    pub const OCTAL_BYTE_TOO_BIG: i32 = 151;
    pub const INTERNAL_MISSING_SUBPATTERN: i32 = 153;
    pub const DEFINE_TOO_MANY_BRANCHES: i32 = 154;
    pub const BACKSLASH_O_MISSING_BRACE: i32 = 155;
    pub const BACKSLASH_G_SYNTAX: i32 = 157;
    pub const PARENS_QUERY_R_MISSING_CLOSING: i32 = 158;
    pub const VERB_UNKNOWN: i32 = 160;
    pub const SUBPATTERN_NUMBER_TOO_BIG: i32 = 161;
    pub const SUBPATTERN_NAME_EXPECTED: i32 = 162;
    pub const INVALID_OCTAL: i32 = 164;
    pub const SUBPATTERN_NAMES_MISMATCH: i32 = 165;
    pub const MARK_MISSING_ARGUMENT: i32 = 166;
    pub const INVALID_HEXADECIMAL: i32 = 167;
    pub const BACKSLASH_C_SYNTAX: i32 = 168;
    pub const BACKSLASH_K_SYNTAX: i32 = 169;
    pub const BACKSLASH_N_IN_CLASS: i32 = 171;
    pub const CALLOUT_STRING_TOO_LONG: i32 = 172;
    pub const UNICODE_DISALLOWED_CODE_POINT: i32 = 173;
    pub const UTF_IS_DISABLED: i32 = 174;
    pub const UCP_IS_DISABLED: i32 = 175;
    pub const VERB_NAME_TOO_LONG: i32 = 176;
    pub const BACKSLASH_U_CODE_POINT_TOO_BIG: i32 = 177;
    pub const MISSING_OCTAL_OR_HEX_DIGITS: i32 = 178;
    pub const VERSION_CONDITION_SYNTAX: i32 = 179;
    pub const CALLOUT_NO_STRING_DELIMITER: i32 = 181;
    pub const CALLOUT_BAD_STRING_DELIMITER: i32 = 182;
    pub const BACKSLASH_C_CALLER_DISABLED: i32 = 183;
    pub const QUERY_BARJX_NEST_TOO_DEEP: i32 = 184;
    pub const PATTERN_TOO_COMPLICATED: i32 = 186;
    pub const LOOKBEHIND_TOO_LONG: i32 = 187;
    pub const INTERNAL_BAD_CODE: i32 = 189;
    pub const NO_SURROGATES_IN_UTF16: i32 = 191;
    pub const SUPPORTED_ONLY_IN_UNICODE: i32 = 193;
    pub const INVALID_HYPHEN_IN_OPTIONS: i32 = 194;
    pub const ALPHA_ASSERTION_UNKNOWN: i32 = 195;
    pub const TOO_MANY_CAPTURES: i32 = 197;
    pub const CONDITION_ATOMIC_ASSERTION_EXPECTED: i32 = 198;
    pub const BACKSLASH_K_IN_LOOKAROUND: i32 = 199;
    pub const VARLOOKBEHIND_TOO_LONG: i32 = 200;
    pub const PATTERN_COMPILED_SIZE_TOO_BIG: i32 = 201;
}

/// PCRE2 compile-time error texts, indexed by `code - 100`.
const COMPILE_ERROR_TEXTS: [&str; 102] = [
    "no error",
    "\\ at end of pattern",
    "\\c at end of pattern",
    "unrecognized character follows \\",
    "numbers out of order in {} quantifier",
    // 5
    "number too big in {} quantifier",
    "missing terminating ] for character class",
    "escape sequence is invalid in character class",
    "range out of order in character class",
    "quantifier does not follow a repeatable item",
    // 10
    "internal error: unexpected repeat",
    "unrecognized character after (? or (?-",
    "POSIX named classes are supported only within a class",
    "POSIX collating elements are not supported",
    "missing closing parenthesis",
    // 15
    "reference to non-existent subpattern",
    "pattern passed as NULL",
    "unrecognised compile-time option bit(s)",
    "missing ) after (?# comment",
    "parentheses are too deeply nested",
    // 20
    "regular expression is too large",
    "failed to allocate heap memory",
    "unmatched closing parenthesis",
    "internal error: code overflow",
    "missing closing parenthesis for condition",
    // 25
    "length of lookbehind assertion is not limited",
    "a relative value of zero is not allowed",
    "conditional subpattern contains more than two branches",
    "assertion expected after (?( or (?(?C)",
    "digit expected after (?+ or (?-",
    // 30
    "unknown POSIX class name",
    "internal error in pcre2_study(): should not occur",
    "this version of PCRE2 does not have Unicode support",
    "parentheses are too deeply nested (stack check)",
    "character code point value in \\x{} or \\o{} is too large",
    // 35
    "lookbehind is too complicated",
    "\\C is not allowed in a lookbehind assertion in UTF-8 mode",
    "PCRE2 does not support \\F, \\L, \\l, \\N{name}, \\U, or \\u",
    "number after (?C is greater than 255",
    "closing parenthesis for (?C expected",
    // 40
    "invalid escape sequence in (*VERB) name",
    "unrecognized character after (?P",
    "syntax error in subpattern name (missing terminator?)",
    "two named subpatterns have the same name (PCRE2_DUPNAMES not set)",
    "subpattern name must start with a non-digit",
    // 45
    "this version of PCRE2 does not have support for \\P, \\p, or \\X",
    "malformed \\P or \\p sequence",
    "unknown property after \\P or \\p",
    "subpattern name is too long (maximum 128 code units)",
    "too many named subpatterns (maximum 10000)",
    // 50
    "invalid range in character class",
    "octal value is greater than \\377 in 8-bit non-UTF-8 mode",
    "internal error: overran compiling workspace",
    "internal error: previously-checked referenced subpattern not found",
    "DEFINE subpattern contains more than one branch",
    // 55
    "missing opening brace after \\o",
    "internal error: unknown newline setting",
    "\\g is not followed by a braced, angle-bracketed, or quoted name/number or by a plain number",
    "(?R (recursive pattern call) must be followed by )",
    "obsolete error (should not occur)",
    // 60
    "(*VERB) not recognized or malformed",
    "subpattern number is too big",
    "subpattern name expected",
    "internal error: parsed pattern overflow",
    "non-octal character in \\o{} (closing brace missing?)",
    // 65
    "different names for subpatterns of the same number are not allowed",
    "(*MARK) must have an argument",
    "non-hex character in \\x{} (closing brace missing?)",
    "\\c must be followed by a printable ASCII character",
    "\\k is not followed by a braced, angle-bracketed, or quoted name",
    // 70
    "internal error: unknown meta code in check_lookbehinds()",
    "\\N is not supported in a class",
    "callout string is too long",
    "disallowed Unicode code point (>= 0xd800 && <= 0xdfff)",
    "using UTF is disabled by the application",
    // 75
    "using UCP is disabled by the application",
    "name is too long in (*MARK), (*PRUNE), (*SKIP), or (*THEN)",
    "character code point value in \\u.... sequence is too large",
    "digits missing in \\x{} or \\o{} or \\N{U+}",
    "syntax error or number too big in (?(VERSION condition",
    // 80
    "internal error: unknown opcode in auto_possessify()",
    "missing terminating delimiter for callout with string argument",
    "unrecognized string delimiter follows (?C",
    "using \\C is disabled by the application",
    "(?| and/or (?J: or (?x: parentheses are too deeply nested",
    // 85
    "using \\C is disabled in this PCRE2 library",
    "regular expression is too complicated",
    "lookbehind assertion is too long",
    "pattern string is longer than the limit set by the application",
    "internal error: unknown code in parsed pattern",
    // 90
    "internal error: bad code value in parsed_skip()",
    "PCRE2_EXTRA_ALLOW_SURROGATE_ESCAPES is not allowed in UTF-16 mode",
    "invalid option bits with PCRE2_LITERAL",
    "\\N{U+dddd} is supported only in Unicode (UTF) mode",
    "invalid hyphen in option setting",
    // 95
    "(*alpha_assertion) not recognized",
    "script runs require Unicode support, which this version of PCRE2 does not have",
    "too many capturing groups (maximum 65535)",
    "atomic assertion expected after (?( or (?(?C)",
    "\\K is not allowed in lookarounds (but see PCRE2_EXTRA_ALLOW_LOOKAROUND_BSK)",
    // 100
    "branch too long in variable-length lookbehind assertion",
    "compiled pattern would be longer than the limit set by the application",
];

/// PCRE2 match-time / UTF error texts for negative codes, indexed by `-code`.
const MATCH_ERROR_TEXTS: [&str; 68] = [
    "no error",
    "no match",
    "partial match",
    "UTF-8 error: 1 byte missing at end",
    "UTF-8 error: 2 bytes missing at end",
    // 5
    "UTF-8 error: 3 bytes missing at end",
    "UTF-8 error: 4 bytes missing at end",
    "UTF-8 error: 5 bytes missing at end",
    "UTF-8 error: byte 2 top bits not 0x80",
    "UTF-8 error: byte 3 top bits not 0x80",
    // 10
    "UTF-8 error: byte 4 top bits not 0x80",
    "UTF-8 error: byte 5 top bits not 0x80",
    "UTF-8 error: byte 6 top bits not 0x80",
    "UTF-8 error: 5-byte character is not allowed (RFC 3629)",
    "UTF-8 error: 6-byte character is not allowed (RFC 3629)",
    // 15
    "UTF-8 error: code points greater than 0x10ffff are not defined",
    "UTF-8 error: code points 0xd800-0xdfff are not defined",
    "UTF-8 error: overlong 2-byte sequence",
    "UTF-8 error: overlong 3-byte sequence",
    "UTF-8 error: overlong 4-byte sequence",
    // 20
    "UTF-8 error: overlong 5-byte sequence",
    "UTF-8 error: overlong 6-byte sequence",
    "UTF-8 error: isolated byte with 0x80 bit set",
    "UTF-8 error: illegal byte (0xfe or 0xff)",
    "UTF-16 error: missing low surrogate at end",
    // 25
    "UTF-16 error: invalid low surrogate",
    "UTF-16 error: isolated low surrogate",
    "UTF-32 error: code points 0xd800-0xdfff are not defined",
    "UTF-32 error: code points greater than 0x10ffff are not defined",
    "bad data value",
    // 30
    "patterns do not all use the same character tables",
    "magic number missing",
    "pattern compiled in wrong mode: 8/16/32-bit error",
    "bad offset value",
    "bad option value",
    // 35
    "invalid replacement string",
    "bad offset into UTF string",
    "callout error code",
    "invalid data in workspace for DFA restart",
    "too much recursion for DFA matching",
    // 40
    "backreference condition or recursion test is not supported for DFA matching",
    "function is not supported for DFA matching",
    "pattern contains an item that is not supported for DFA matching",
    "workspace size exceeded in DFA matching",
    "internal error - pattern overwritten?",
    // 45
    "bad JIT option",
    "JIT stack limit reached",
    "match limit exceeded",
    "no more memory",
    "unknown substring",
    // 50
    "non-unique substring name",
    "NULL argument passed with non-zero length",
    "nested recursion at the same subject position",
    "matching depth limit exceeded",
    "requested value is not available",
    // 55
    "requested value is not set",
    "offset limit set without PCRE2_USE_OFFSET_LIMIT",
    "bad escape sequence in replacement string",
    "expected closing curly bracket in replacement string",
    "bad substitution in replacement string",
    // 60
    "match with end before start or start moved backwards is not supported",
    "too many replacements (more than INT_MAX)",
    "bad serialized data",
    "heap limit exceeded",
    "invalid syntax",
    // 65
    "internal error - duplicate substitution match",
    "PCRE2_MATCH_INVALID_UTF is not supported for DFA matching",
    "invalid offset in UTF-8 string",
];

/// Returns PCRE2's message text for any PCRE2 error code (positive compile
/// error codes, negative match/UTF error codes), like `pcre2_get_error_message`.
pub fn error_message(code: i32) -> &'static str {
    if code >= 100 {
        COMPILE_ERROR_TEXTS
            .get((code - 100) as usize)
            .copied()
            .unwrap_or("unknown error")
    } else if code <= 0 {
        MATCH_ERROR_TEXTS
            .get(code.unsigned_abs() as usize)
            .copied()
            .unwrap_or("unknown error")
    } else {
        "unknown error"
    }
}

/// A pattern compilation failure.
///
/// `code` is the PCRE2 error number (e.g. 114 for "missing closing
/// parenthesis"), or a negative PCRE2 UTF-8 error code when the pattern itself
/// is not valid UTF-8 in UTF mode. `offset` is the byte offset in the pattern
/// that PCRE2 reports. [`CompileError::message`] returns PCRE2's text, so PHP
/// can print `Compilation failed: <message> at offset <offset>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    /// PCRE2 error code.
    pub code: i32,
    /// Byte offset into the pattern.
    pub offset: usize,
}

impl CompileError {
    pub(crate) fn new(code: i32, offset: usize) -> Self {
        CompileError { code, offset }
    }

    /// PCRE2's message text for this error.
    pub fn message(&self) -> &'static str {
        error_message(self.code)
    }

    /// The message PHP prints after `Compilation failed: ` (PHP replaces the
    /// text of `PCRE2_ERROR_BACKSLASH_C_CALLER_DISABLED`).
    pub fn php_message(&self) -> &'static str {
        if self.code == codes::BACKSLASH_C_CALLER_DISABLED {
            "using \\C is incompatible with the 'u' modifier"
        } else {
            self.message()
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {}", self.message(), self.offset)
    }
}

impl std::error::Error for CompileError {}

/// PHP's `preg_last_error()` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PregError {
    /// `PREG_NO_ERROR` (0)
    NoError = 0,
    /// `PREG_INTERNAL_ERROR` (1)
    Internal = 1,
    /// `PREG_BACKTRACK_LIMIT_ERROR` (2)
    BacktrackLimit = 2,
    /// `PREG_RECURSION_LIMIT_ERROR` (3)
    RecursionLimit = 3,
    /// `PREG_BAD_UTF8_ERROR` (4)
    BadUtf8 = 4,
    /// `PREG_BAD_UTF8_OFFSET_ERROR` (5)
    BadUtf8Offset = 5,
    /// `PREG_JIT_STACKLIMIT_ERROR` (6)
    JitStackLimit = 6,
}

impl PregError {
    /// The text returned by `preg_last_error_msg()`.
    pub fn message(self) -> &'static str {
        match self {
            PregError::NoError => "No error",
            PregError::Internal => "Internal error",
            PregError::BadUtf8 => "Malformed UTF-8 characters, possibly incorrectly encoded",
            PregError::BadUtf8Offset => {
                "The offset did not correspond to the beginning of a valid UTF-8 code point"
            }
            PregError::BacktrackLimit => "Backtrack limit exhausted",
            PregError::RecursionLimit => "Recursion limit exhausted",
            PregError::JitStackLimit => "JIT stack limit exhausted",
        }
    }
}

/// A failure while matching (not "no match", which is `Ok(None)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchError {
    /// `PCRE2_ERROR_MATCHLIMIT`: the backtrack (match) limit was reached.
    MatchLimit,
    /// `PCRE2_ERROR_DEPTHLIMIT` (a.k.a. RECURSIONLIMIT): the depth limit was reached.
    DepthLimit,
    /// `PCRE2_ERROR_HEAPLIMIT`: the backtracking frames exceeded the heap limit.
    HeapLimit,
    /// The subject is not valid UTF-8 (UTF mode). `code` is the PCRE2
    /// `PCRE2_ERROR_UTF8_ERRn` value (-3 ..= -23) and `offset` the byte offset
    /// of the offending character.
    BadUtf8 {
        /// Negative PCRE2 UTF-8 error code.
        code: i32,
        /// Byte offset of the bad character.
        offset: usize,
    },
    /// `PCRE2_ERROR_BADUTFOFFSET`: the start offset is not on a character boundary.
    BadUtf8Offset,
    /// `PCRE2_ERROR_BADOFFSET`: the start offset is past the end of the subject.
    BadOffset,
    /// `PCRE2_ERROR_RECURSELOOP`: nested recursion at the same subject position.
    RecurseLoop,
    /// `PCRE2_ERROR_INTERNAL`.
    Internal,
}

impl MatchError {
    /// The negative PCRE2 error code.
    pub fn pcre2_code(&self) -> i32 {
        match *self {
            MatchError::MatchLimit => -47,
            MatchError::DepthLimit => -53,
            MatchError::HeapLimit => -63,
            MatchError::BadUtf8 { code, .. } => code,
            MatchError::BadUtf8Offset => -36,
            MatchError::BadOffset => -33,
            MatchError::RecurseLoop => -52,
            MatchError::Internal => -44,
        }
    }

    /// PCRE2's message text.
    pub fn message(&self) -> &'static str {
        error_message(self.pcre2_code())
    }

    /// The `preg_last_error()` value PHP derives from this error
    /// (mirrors `pcre_handle_exec_error`).
    pub fn preg_error(&self) -> PregError {
        match self {
            MatchError::MatchLimit => PregError::BacktrackLimit,
            MatchError::DepthLimit => PregError::RecursionLimit,
            MatchError::BadUtf8Offset => PregError::BadUtf8Offset,
            MatchError::BadUtf8 { .. } => PregError::BadUtf8,
            _ => PregError::Internal,
        }
    }
}

impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for MatchError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(error_message(114), "missing closing parenthesis");
        assert_eq!(
            error_message(109),
            "quantifier does not follow a repeatable item"
        );
        assert_eq!(
            error_message(199),
            "\\K is not allowed in lookarounds (but see PCRE2_EXTRA_ALLOW_LOOKAROUND_BSK)"
        );
        assert_eq!(
            error_message(-22),
            "UTF-8 error: isolated byte with 0x80 bit set"
        );
        assert_eq!(error_message(-47), "match limit exceeded");
        assert_eq!(error_message(-67), "invalid offset in UTF-8 string");
        let e = CompileError::new(114, 3);
        assert_eq!(e.to_string(), "missing closing parenthesis at offset 3");
    }
}
