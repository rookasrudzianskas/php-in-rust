//! Reads the expected output of a php-src `.phpt` file from the repository
//! (`tests/phpt/ext/pcre/tests/`) and compares it with the output a Rust
//! transliteration of the script produced, with run-tests.php's `--EXPECT--`
//! and `--EXPECTF--` semantics.
//!
//! Warnings and notices carry the script's file name and line; line numbers
//! are not tracked by the unit tests, so `on line N` is compared as
//! `on line %d`. Everything works on bytes: expectations may contain
//! non-UTF-8 data.

use std::path::PathBuf;

use php_regex::{CompileFlags, MatchOptions, Regex, preg_quote};

/// The adopted php-src test `name` of ext/pcre.
fn phpt_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/phpt/ext/pcre/tests")
        .join(format!("{name}.phpt"))
}

/// Is `line` a section header such as `--EXPECTF--`?
fn section_name(line: &[u8]) -> Option<&[u8]> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let inner = line.strip_prefix(b"--")?.strip_suffix(b"--")?;
    (!inner.is_empty() && inner.iter().all(|c| c.is_ascii_uppercase() || *c == b'_'))
        .then_some(inner)
}

/// The text of section `--NAME--` of a phpt file.
fn section<'a>(phpt: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let mut found = None;
    let mut pos = 0;
    for line in phpt.split_inclusive(|&c| c == b'\n') {
        if let Some(n) = section_name(line) {
            if let Some(start) = found {
                return Some(&phpt[start..pos]);
            }
            if n == name.as_bytes() {
                found = Some(pos + line.len());
            }
        }
        pos += line.len();
    }
    found.map(|start| &phpt[start..])
}

/// run-tests.php: `trim()` and CRLF to LF.
fn normalize(s: &[u8]) -> Vec<u8> {
    let is_ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0 | 0x0b);
    let start = s.iter().position(|c| !is_ws(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !is_ws(c)).map_or(start, |p| p + 1);
    let s = &s[start..end];
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\r' && s.get(i + 1) == Some(&b'\n') {
            i += 1;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// `on line 12` -> `on line 0` (line numbers are not tracked in the Rust ports;
/// an `--EXPECTF--` `on line %d` still matches).
fn blur_lines(s: &[u8]) -> Vec<u8> {
    const MARK: &[u8] = b"on line ";
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with(MARK) {
            out.extend_from_slice(MARK);
            i += MARK.len();
            let digits = s[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 {
                out.push(b'0');
                i += digits;
            }
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// `expectf_to_regex()`: the PCRE pattern body for an `--EXPECTF--` section.
fn expectf_to_pattern(wanted: &[u8]) -> Vec<u8> {
    const PLACEHOLDERS: &[(u8, &str)] = &[
        (b'e', "/"),
        (b's', "[^\\r\\n]+"),
        (b'S', "[^\\r\\n]*"),
        (b'a', ".+"),
        (b'A', ".*"),
        (b'w', "\\s*"),
        (b'i', "[+-]?\\d+"),
        (b'd', "\\d+"),
        (b'x', "[0-9a-fA-F]+"),
        (
            b'f',
            "[+-]?(?:\\d+|(?=\\.\\d))(?:\\.\\d+)?(?:[Ee][+-]?\\d+)?",
        ),
        (b'c', "."),
    ];
    let mut out = Vec::new();
    let mut i = 0;
    while i < wanted.len() {
        // `%r...%r`: a raw regular expression.
        let raw_len = if wanted[i..].starts_with(b"%r") {
            wanted[i + 2..].windows(2).position(|w| w == b"%r")
        } else {
            None
        };
        if let Some(len) = raw_len {
            out.push(b'(');
            out.extend_from_slice(&wanted[i + 2..i + 2 + len]);
            out.push(b')');
            i += 2 + len + 2;
            continue;
        }
        if wanted[i] == b'%' {
            let rep = wanted
                .get(i + 1)
                .and_then(|c| PLACEHOLDERS.iter().find(|(k, _)| k == c));
            if let Some((_, rep)) = rep {
                out.extend_from_slice(rep.as_bytes());
                i += 2;
                continue;
            }
        }
        out.extend_from_slice(&preg_quote(&wanted[i..=i], Some(b'/')));
        i += 1;
    }
    out
}

/// Does `actual` match the `--EXPECTF--` text `wanted`?
fn expectf_matches(wanted: &[u8], actual: &[u8]) -> bool {
    let mut pattern = b"^".to_vec();
    pattern.extend_from_slice(&expectf_to_pattern(wanted));
    pattern.push(b'$');
    let re = Regex::new(&pattern, CompileFlags::DOTALL).expect("EXPECTF pattern compiles");
    let opts = MatchOptions {
        match_limit: u32::MAX,
        depth_limit: u32::MAX,
        ..MatchOptions::default()
    };
    matches!(re.match_at(actual, 0, opts), Ok(Some(_)))
}

/// The `--FILE--` section (the PHP script) of phpt `name`, for tests that
/// need a large literal from the script as data.
pub fn file_section(name: &str) -> Vec<u8> {
    let path = phpt_path(name);
    let phpt =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    section(&phpt, "FILE")
        .unwrap_or_else(|| panic!("{name}.phpt has no --FILE-- section"))
        .to_vec()
}

/// The bytes of `haystack` between the first `start` marker and the next
/// `end` marker after it.
pub fn between(haystack: &[u8], start: &[u8], end: &[u8]) -> Vec<u8> {
    let find = |from: usize, needle: &[u8]| {
        haystack[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    };
    let a = find(0, start).expect("start marker") + start.len();
    let b = find(a, end).expect("end marker");
    haystack[a..b].to_vec()
}

/// Asserts that `actual` is what the phpt file `name` expects (the phpt
/// lives in `tests/phpt/ext/pcre/tests/<name>.phpt`).
#[track_caller]
pub fn assert_phpt(name: &str, actual: &[u8]) {
    let path = phpt_path(name);
    let phpt =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let actual = blur_lines(&normalize(actual));
    if let Some(expect) = section(&phpt, "EXPECT") {
        let expect = blur_lines(&normalize(expect));
        assert!(
            actual == expect,
            "{name}.phpt: output differs from --EXPECT--\n--- expected ---\n{}\n--- actual ---\n{}\n",
            String::from_utf8_lossy(&expect),
            String::from_utf8_lossy(&actual)
        );
    } else if let Some(expectf) = section(&phpt, "EXPECTF") {
        let expectf = blur_lines(&normalize(expectf));
        assert!(
            expectf_matches(&expectf, &actual),
            "{name}.phpt: output does not match --EXPECTF--\n--- expected ---\n{}\n--- actual ---\n{}\n",
            String::from_utf8_lossy(&expectf),
            String::from_utf8_lossy(&actual)
        );
    } else {
        panic!("{name}.phpt has no --EXPECT-- or --EXPECTF-- section");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections() {
        let phpt = b"--TEST--\nx\n--FILE--\n<?php\necho 1;\n--EXPECTF--\nint(%d)\n--INI--\na=b\n";
        assert_eq!(section(phpt, "FILE"), Some(&b"<?php\necho 1;\n"[..]));
        assert_eq!(section(phpt, "EXPECTF"), Some(&b"int(%d)\n"[..]));
        assert_eq!(section(phpt, "INI"), Some(&b"a=b\n"[..]));
        assert_eq!(section(phpt, "EXPECT"), None);
    }

    #[test]
    fn expectf() {
        assert!(expectf_matches(
            b"int(%d)\nWarning: %s in %sx.php on line %d",
            b"int(12)\nWarning: boo in /a/x.php on line 7"
        ));
        assert!(!expectf_matches(b"int(%d)", b"int(x)"));
        assert!(expectf_matches(b"a.b(c)", b"a.b(c)"));
        assert!(!expectf_matches(b"a.b", b"axb"));
        assert_eq!(
            blur_lines(b"in /t.php on line 3\nx on line 12"),
            b"in /t.php on line 0\nx on line 0"
        );
    }
}
