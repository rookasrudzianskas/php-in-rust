//! PHP pattern front-end: delimiters and modifiers, mirroring
//! `pcre_get_compiled_regex_cache_ex()` in ext/pcre/php_pcre.c, plus
//! `preg_quote()`.

use std::fmt;

use crate::{CompileError, CompileFlags, Regex};

/// The PHP-level modifiers that were present on a pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhpModifiers {
    /// `i`
    pub caseless: bool,
    /// `m`
    pub multiline: bool,
    /// `n`
    pub no_auto_capture: bool,
    /// `s`
    pub dotall: bool,
    /// `x`
    pub extended: bool,
    /// `A`
    pub anchored: bool,
    /// `D`
    pub dollar_endonly: bool,
    /// `r` (PCRE2_EXTRA_CASELESS_RESTRICT)
    pub caseless_restrict: bool,
    /// `S` (accepted and ignored)
    pub study: bool,
    /// `X` (accepted and ignored)
    pub extra: bool,
    /// `U`
    pub ungreedy: bool,
    /// `u` (UTF + UCP + NEVER_BACKSLASH_C)
    pub utf: bool,
    /// `J`
    pub dupnames: bool,
}

/// A compiled PHP pattern.
#[derive(Clone, Debug)]
pub struct PhpPattern {
    /// The compiled regex.
    pub regex: Regex,
    /// Modifiers that were given.
    pub modifiers: PhpModifiers,
    /// The compile flags derived from the modifiers.
    pub flags: CompileFlags,
    /// The pattern between the delimiters.
    pub pattern: Vec<u8>,
}

/// Errors from [`compile_php_pattern`]. `Display` gives PHP's warning text
/// (without the `preg_xxx(): ` prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhpPatternError {
    /// "Empty regular expression"
    Empty,
    /// "Delimiter must not be alphanumeric, backslash, or NUL byte"
    BadDelimiter,
    /// "No ending delimiter '%c' found"
    NoEndingDelimiter(u8),
    /// "No ending matching delimiter '%c' found"
    NoEndingMatchingDelimiter(u8),
    /// "Unknown modifier '%c'" (this includes the removed `e` modifier)
    UnknownModifier(u8),
    /// "NUL byte is not a valid modifier"
    NulModifier,
    /// "Compilation failed: %s at offset %zu"
    Compile(CompileError),
}

impl PhpPatternError {
    /// The warning text as raw bytes (the delimiter/modifier may be a
    /// non-UTF-8 byte, which PHP prints verbatim).
    pub fn message_bytes(&self) -> Vec<u8> {
        match self {
            PhpPatternError::NoEndingDelimiter(c) => {
                let mut v = b"No ending delimiter '".to_vec();
                v.push(*c);
                v.extend_from_slice(b"' found");
                v
            }
            PhpPatternError::NoEndingMatchingDelimiter(c) => {
                let mut v = b"No ending matching delimiter '".to_vec();
                v.push(*c);
                v.extend_from_slice(b"' found");
                v
            }
            PhpPatternError::UnknownModifier(c) => {
                let mut v = b"Unknown modifier '".to_vec();
                v.push(*c);
                v.push(b'\'');
                v
            }
            other => other.to_string().into_bytes(),
        }
    }
}

impl fmt::Display for PhpPatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhpPatternError::Empty => f.write_str("Empty regular expression"),
            PhpPatternError::BadDelimiter => {
                f.write_str("Delimiter must not be alphanumeric, backslash, or NUL byte")
            }
            PhpPatternError::NoEndingDelimiter(c) => {
                write!(f, "No ending delimiter '{}' found", *c as char)
            }
            PhpPatternError::NoEndingMatchingDelimiter(c) => {
                write!(f, "No ending matching delimiter '{}' found", *c as char)
            }
            PhpPatternError::UnknownModifier(c) => write!(f, "Unknown modifier '{}'", *c as char),
            PhpPatternError::NulModifier => f.write_str("NUL byte is not a valid modifier"),
            PhpPatternError::Compile(e) => {
                write!(
                    f,
                    "Compilation failed: {} at offset {}",
                    e.php_message(),
                    e.offset
                )
            }
        }
    }
}

impl std::error::Error for PhpPatternError {}

/// C `isspace()` in the "C" locale.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Parse delimiters and modifiers of a PHP pattern (e.g. `/abc/i`, `#x#`,
/// `{x}`, `(x)`, `[x]`, `<x>`) without compiling. Returns the regex source,
/// modifiers and compile flags.
pub fn parse_php_pattern(
    regex: &[u8],
) -> Result<(Vec<u8>, PhpModifiers, CompileFlags), PhpPatternError> {
    let end = regex.len();
    let mut p = 0;
    while p < end && is_c_space(regex[p]) {
        p += 1;
    }
    if p >= end {
        return Err(PhpPatternError::Empty);
    }
    let delimiter = regex[p];
    p += 1;
    if delimiter.is_ascii_alphanumeric() || delimiter == b'\\' || delimiter == 0 {
        return Err(PhpPatternError::BadDelimiter);
    }
    let start_delimiter = delimiter;
    let end_delimiter = match delimiter {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        d => d,
    };
    let mut pp = p;
    if start_delimiter == end_delimiter {
        while pp < end {
            if regex[pp] == b'\\' && pp + 1 < end {
                pp += 1;
            } else if regex[pp] == end_delimiter {
                break;
            }
            pp += 1;
        }
    } else {
        let mut brackets = 1;
        while pp < end {
            if regex[pp] == b'\\' && pp + 1 < end {
                pp += 1;
            } else if regex[pp] == end_delimiter {
                brackets -= 1;
                if brackets <= 0 {
                    break;
                }
            } else if regex[pp] == start_delimiter {
                brackets += 1;
            }
            pp += 1;
        }
    }
    if pp >= end {
        return Err(if start_delimiter == end_delimiter {
            PhpPatternError::NoEndingDelimiter(end_delimiter)
        } else {
            PhpPatternError::NoEndingMatchingDelimiter(end_delimiter)
        });
    }
    let pattern = regex[p..pp].to_vec();
    pp += 1;

    let mut m = PhpModifiers::default();
    let mut flags = CompileFlags::empty();
    while pp < end {
        let c = regex[pp];
        pp += 1;
        match c {
            b'i' => {
                m.caseless = true;
                flags |= CompileFlags::CASELESS;
            }
            b'm' => {
                m.multiline = true;
                flags |= CompileFlags::MULTILINE;
            }
            b'n' => {
                m.no_auto_capture = true;
                flags |= CompileFlags::NO_AUTO_CAPTURE;
            }
            b's' => {
                m.dotall = true;
                flags |= CompileFlags::DOTALL;
            }
            b'x' => {
                m.extended = true;
                flags |= CompileFlags::EXTENDED;
            }
            b'A' => {
                m.anchored = true;
                flags |= CompileFlags::ANCHORED;
            }
            b'D' => {
                m.dollar_endonly = true;
                flags |= CompileFlags::DOLLAR_ENDONLY;
            }
            b'r' => {
                m.caseless_restrict = true;
                flags |= CompileFlags::CASELESS_RESTRICT;
            }
            b'S' => m.study = true,
            b'X' => m.extra = true,
            b'U' => {
                m.ungreedy = true;
                flags |= CompileFlags::UNGREEDY;
            }
            b'u' => {
                m.utf = true;
                flags |= CompileFlags::UTF | CompileFlags::UCP | CompileFlags::NEVER_BACKSLASH_C;
            }
            b'J' => {
                m.dupnames = true;
                flags |= CompileFlags::DUPNAMES;
            }
            b' ' | b'\n' | b'\r' => {}
            0 => return Err(PhpPatternError::NulModifier),
            other => return Err(PhpPatternError::UnknownModifier(other)),
        }
    }
    Ok((pattern, m, flags))
}

/// Parse and compile a PHP pattern string with delimiters and modifiers,
/// like `pcre_get_compiled_regex_cache_ex()`.
pub fn compile_php_pattern(regex: &[u8]) -> Result<PhpPattern, PhpPatternError> {
    let (pattern, modifiers, flags) = parse_php_pattern(regex)?;
    let re = Regex::new(&pattern, flags).map_err(PhpPatternError::Compile)?;
    Ok(PhpPattern {
        regex: re,
        modifiers,
        flags,
        pattern,
    })
}

/// PHP's `preg_quote()`: escape regex special characters (and `delimiter`).
/// NUL bytes become `\000`.
pub fn preg_quote(s: &[u8], delimiter: Option<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for &c in s {
        match c {
            b'.' | b'\\' | b'+' | b'*' | b'?' | b'[' | b'^' | b']' | b'$' | b'(' | b')' | b'{'
            | b'}' | b'=' | b'!' | b'>' | b'<' | b'|' | b':' | b'-' | b'#' => {
                out.push(b'\\');
                out.push(c);
            }
            0 => out.extend_from_slice(b"\\000"),
            _ => {
                if Some(c) == delimiter {
                    out.push(b'\\');
                }
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiters() {
        assert_eq!(
            compile_php_pattern(b"").unwrap_err(),
            PhpPatternError::Empty
        );
        assert_eq!(
            compile_php_pattern(b"   ").unwrap_err(),
            PhpPatternError::Empty
        );
        assert_eq!(
            compile_php_pattern(b"abc").unwrap_err(),
            PhpPatternError::BadDelimiter
        );
        assert_eq!(
            compile_php_pattern(b"\\abc\\").unwrap_err(),
            PhpPatternError::BadDelimiter
        );
        assert_eq!(
            compile_php_pattern(b"/abc").unwrap_err(),
            PhpPatternError::NoEndingDelimiter(b'/')
        );
        assert_eq!(
            compile_php_pattern(b"{abc").unwrap_err(),
            PhpPatternError::NoEndingMatchingDelimiter(b'}')
        );
        assert_eq!(
            compile_php_pattern(b"/abc/q").unwrap_err(),
            PhpPatternError::UnknownModifier(b'q')
        );
        assert_eq!(
            compile_php_pattern(b"/abc/e").unwrap_err().to_string(),
            "Unknown modifier 'e'"
        );
        assert_eq!(
            compile_php_pattern(b"/abc/\0").unwrap_err(),
            PhpPatternError::NulModifier
        );
        let p = compile_php_pattern(b"  {a{1,2}b}i\n").unwrap();
        assert_eq!(p.pattern, b"a{1,2}b");
        assert!(p.modifiers.caseless);
        let p = compile_php_pattern(b"(a(b)c)").unwrap();
        assert_eq!(p.pattern, b"a(b)c");
        let p = compile_php_pattern(b"/a\\/b/").unwrap();
        assert_eq!(p.pattern, b"a\\/b");
    }

    #[test]
    fn quote() {
        assert_eq!(preg_quote(b"Hello.World?", None), b"Hello\\.World\\?");
        assert_eq!(preg_quote(b"a/b#c\0", Some(b'/')), b"a\\/b\\#c\\000");
    }
}
