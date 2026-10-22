//! Internal option bits, using PCRE2's numeric values.

#![allow(dead_code)]

pub(crate) const PCRE2_ANCHORED: u32 = 0x8000_0000;
pub(crate) const PCRE2_NO_UTF_CHECK: u32 = 0x4000_0000;
pub(crate) const PCRE2_ENDANCHORED: u32 = 0x2000_0000;

pub(crate) const PCRE2_ALLOW_EMPTY_CLASS: u32 = 0x0000_0001;
pub(crate) const PCRE2_ALT_BSUX: u32 = 0x0000_0002;
pub(crate) const PCRE2_AUTO_CALLOUT: u32 = 0x0000_0004;
pub(crate) const PCRE2_CASELESS: u32 = 0x0000_0008;
pub(crate) const PCRE2_DOLLAR_ENDONLY: u32 = 0x0000_0010;
pub(crate) const PCRE2_DOTALL: u32 = 0x0000_0020;
pub(crate) const PCRE2_DUPNAMES: u32 = 0x0000_0040;
pub(crate) const PCRE2_EXTENDED: u32 = 0x0000_0080;
pub(crate) const PCRE2_FIRSTLINE: u32 = 0x0000_0100;
pub(crate) const PCRE2_MATCH_UNSET_BACKREF: u32 = 0x0000_0200;
pub(crate) const PCRE2_MULTILINE: u32 = 0x0000_0400;
pub(crate) const PCRE2_NEVER_UCP: u32 = 0x0000_0800;
pub(crate) const PCRE2_NEVER_UTF: u32 = 0x0000_1000;
pub(crate) const PCRE2_NO_AUTO_CAPTURE: u32 = 0x0000_2000;
pub(crate) const PCRE2_NO_AUTO_POSSESS: u32 = 0x0000_4000;
pub(crate) const PCRE2_NO_DOTSTAR_ANCHOR: u32 = 0x0000_8000;
pub(crate) const PCRE2_NO_START_OPTIMIZE: u32 = 0x0001_0000;
pub(crate) const PCRE2_UCP: u32 = 0x0002_0000;
pub(crate) const PCRE2_UNGREEDY: u32 = 0x0004_0000;
pub(crate) const PCRE2_UTF: u32 = 0x0008_0000;
pub(crate) const PCRE2_NEVER_BACKSLASH_C: u32 = 0x0010_0000;
pub(crate) const PCRE2_ALT_CIRCUMFLEX: u32 = 0x0020_0000;
pub(crate) const PCRE2_ALT_VERBNAMES: u32 = 0x0040_0000;
pub(crate) const PCRE2_USE_OFFSET_LIMIT: u32 = 0x0080_0000;
pub(crate) const PCRE2_EXTENDED_MORE: u32 = 0x0100_0000;
pub(crate) const PCRE2_LITERAL: u32 = 0x0200_0000;
pub(crate) const PCRE2_MATCH_INVALID_UTF: u32 = 0x0400_0000;

pub(crate) const PCRE2_EXTRA_ALLOW_SURROGATE_ESCAPES: u32 = 0x0000_0001;
pub(crate) const PCRE2_EXTRA_BAD_ESCAPE_IS_LITERAL: u32 = 0x0000_0002;
pub(crate) const PCRE2_EXTRA_MATCH_WORD: u32 = 0x0000_0004;
pub(crate) const PCRE2_EXTRA_MATCH_LINE: u32 = 0x0000_0008;
pub(crate) const PCRE2_EXTRA_ESCAPED_CR_IS_LF: u32 = 0x0000_0010;
pub(crate) const PCRE2_EXTRA_ALT_BSUX: u32 = 0x0000_0020;
pub(crate) const PCRE2_EXTRA_ALLOW_LOOKAROUND_BSK: u32 = 0x0000_0040;
pub(crate) const PCRE2_EXTRA_CASELESS_RESTRICT: u32 = 0x0000_0080;
pub(crate) const PCRE2_EXTRA_ASCII_BSD: u32 = 0x0000_0100;
pub(crate) const PCRE2_EXTRA_ASCII_BSS: u32 = 0x0000_0200;
pub(crate) const PCRE2_EXTRA_ASCII_BSW: u32 = 0x0000_0400;
pub(crate) const PCRE2_EXTRA_ASCII_POSIX: u32 = 0x0000_0800;
pub(crate) const PCRE2_EXTRA_ASCII_DIGIT: u32 = 0x0000_1000;

// Match options
pub(crate) const PCRE2_NOTBOL: u32 = 0x0000_0001;
pub(crate) const PCRE2_NOTEOL: u32 = 0x0000_0002;
pub(crate) const PCRE2_NOTEMPTY: u32 = 0x0000_0004;
pub(crate) const PCRE2_NOTEMPTY_ATSTART: u32 = 0x0000_0008;

// Internal pattern flags (re->flags)
pub(crate) const FLAG_FIRSTSET: u32 = 0x0000_0010;
pub(crate) const FLAG_FIRSTCASELESS: u32 = 0x0000_0020;
pub(crate) const FLAG_FIRSTMAPSET: u32 = 0x0000_0040;
pub(crate) const FLAG_LASTSET: u32 = 0x0000_0080;
pub(crate) const FLAG_LASTCASELESS: u32 = 0x0000_0100;
pub(crate) const FLAG_STARTLINE: u32 = 0x0000_0200;
pub(crate) const FLAG_JCHANGED: u32 = 0x0000_0400;
pub(crate) const FLAG_HASCRORLF: u32 = 0x0000_0800;
pub(crate) const FLAG_HASTHEN: u32 = 0x0000_1000;
pub(crate) const FLAG_MATCH_EMPTY: u32 = 0x0000_2000;
pub(crate) const FLAG_BSR_SET: u32 = 0x0000_4000;
pub(crate) const FLAG_NL_SET: u32 = 0x0000_8000;
pub(crate) const FLAG_NOTEMPTY_SET: u32 = 0x0001_0000;
pub(crate) const FLAG_NE_ATST_SET: u32 = 0x0002_0000;
pub(crate) const FLAG_NOJIT: u32 = 0x0008_0000;
pub(crate) const FLAG_HASBKPORX: u32 = 0x0010_0000;
pub(crate) const FLAG_DUPCAPUSED: u32 = 0x0020_0000;
pub(crate) const FLAG_HASBKC: u32 = 0x0040_0000;
pub(crate) const FLAG_HASACCEPT: u32 = 0x0080_0000;

// First/required code unit flags
pub(crate) const REQ_UNSET: u32 = 0xffff_ffff;
pub(crate) const REQ_NONE: u32 = 0xffff_fffe;
pub(crate) const REQ_CASELESS: u32 = 0x0000_0001;
pub(crate) const REQ_VARY: u32 = 0x0000_0002;
