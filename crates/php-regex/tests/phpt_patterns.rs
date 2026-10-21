//! Runs every regex pattern literal that appears as the pattern argument of a
//! `preg_*()` call in php-src's `ext/pcre/tests/*.phpt` through the PHP
//! delimiter/modifier parser and the compiler. A pattern must compile unless
//! the test's expected output contains the corresponding warning.

use std::fs;
use std::path::PathBuf;

use php_regex::{PhpPatternError, compile_php_pattern};

fn tests_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../php-src-ref/ext/pcre/tests")
}

/// Split a .phpt file into its sections.
fn section<'a>(text: &'a str, names: &[&str]) -> Option<&'a str> {
    for name in names {
        let tag = format!("--{}--\n", name);
        if let Some(start) = text.find(&tag) {
            let body = &text[start + tag.len()..];
            let end = body
                .match_indices("\n--")
                .find(|(i, _)| {
                    let rest = &body[i + 3..];
                    let upto = rest.find("--").unwrap_or(0);
                    upto > 0
                        && rest[..upto]
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c == '_')
                })
                .map(|(i, _)| i + 1)
                .unwrap_or(body.len());
            return Some(&body[..end]);
        }
    }
    None
}

/// A PHP string literal found in the source.
struct Lit {
    start: usize,
    end: usize,
    value: Option<Vec<u8>>,
}

/// Parse a PHP string literal starting at `i` (at the quote). Returns the
/// literal with its decoded value (None for interpolated double-quoted
/// strings).
fn parse_literal(src: &[u8], i: usize) -> Lit {
    let q = src[i];
    let mut j = i + 1;
    let mut v = Vec::new();
    let mut interpolated = false;
    while j < src.len() && src[j] != q {
        let c = src[j];
        if c == b'\\' && j + 1 < src.len() {
            let n = src[j + 1];
            if q == b'\'' {
                if n == b'\\' || n == b'\'' {
                    v.push(n);
                } else {
                    v.push(b'\\');
                    v.push(n);
                }
                j += 2;
                continue;
            }
            j += 2;
            match n {
                b'n' => v.push(b'\n'),
                b't' => v.push(b'\t'),
                b'r' => v.push(b'\r'),
                b'v' => v.push(0x0b),
                b'e' => v.push(0x1b),
                b'f' => v.push(0x0c),
                b'\\' => v.push(b'\\'),
                b'$' => v.push(b'$'),
                b'"' => v.push(b'"'),
                b'0'..=b'7' => {
                    let mut val = (n - b'0') as u32;
                    let mut k = 0;
                    while k < 2 && j < src.len() && (b'0'..=b'7').contains(&src[j]) {
                        val = val * 8 + (src[j] - b'0') as u32;
                        j += 1;
                        k += 1;
                    }
                    v.push(val as u8);
                }
                b'x' if j < src.len() && src[j].is_ascii_hexdigit() => {
                    let mut val = 0u32;
                    let mut k = 0;
                    while k < 2 && j < src.len() && src[j].is_ascii_hexdigit() {
                        val = val * 16 + (src[j] as char).to_digit(16).unwrap();
                        j += 1;
                        k += 1;
                    }
                    v.push(val as u8);
                }
                b'u' if j < src.len() && src[j] == b'{' => {
                    let close = src[j..].iter().position(|&b| b == b'}').map(|p| p + j);
                    if let Some(close) = close {
                        let hex = std::str::from_utf8(&src[j + 1..close]).unwrap_or("0");
                        let cp = u32::from_str_radix(hex, 16).unwrap_or(0);
                        let mut buf = [0u8; 4];
                        if let Some(ch) = char::from_u32(cp) {
                            v.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        j = close + 1;
                    }
                }
                other => {
                    v.push(b'\\');
                    v.push(other);
                }
            }
            continue;
        }
        if q == b'"'
            && c == b'$'
            && j + 1 < src.len()
            && (src[j + 1].is_ascii_alphabetic() || src[j + 1] == b'_' || src[j + 1] == b'{')
        {
            interpolated = true;
        }
        if q == b'"' && c == b'{' && j + 1 < src.len() && src[j + 1] == b'$' {
            interpolated = true;
        }
        v.push(c);
        j += 1;
    }
    Lit {
        start: i,
        end: (j + 1).min(src.len()),
        value: if interpolated { None } else { Some(v) },
    }
}

/// Collect the literal patterns of the first argument of each preg_* call.
fn pattern_literals(src: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let funcs: [&[u8]; 7] = [
        b"preg_match_all",
        b"preg_match",
        b"preg_replace_callback_array",
        b"preg_replace_callback",
        b"preg_replace",
        b"preg_split",
        b"preg_grep",
    ];
    let mut i = 0;
    while i < src.len() {
        // Skip string literals and comments globally.
        let c = src[i];
        if c == b'\'' || c == b'"' {
            i = parse_literal(src, i).end;
            continue;
        }
        let mut matched: Option<&[u8]> = None;
        for f in funcs.iter() {
            if src[i..].starts_with(f)
                && (i == 0 || !(src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_'))
            {
                matched = Some(f);
                break;
            }
        }
        let Some(f) = matched else {
            i += 1;
            continue;
        };
        let mut j = i + f.len();
        while j < src.len() && src[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= src.len() || src[j] != b'(' {
            i += f.len();
            continue;
        }
        // Scan the first argument. Only literals that are the argument
        // itself, or elements of array literals, are patterns.
        let is_cb_array = f == b"preg_replace_callback_array";
        j += 1;
        let mut stack: Vec<bool> = Vec::new(); // true = array literal
        while j < src.len() {
            let c = src[j];
            if c == b'\'' || c == b'"' {
                let lit = parse_literal(src, j);
                let mut k = lit.end;
                while k < src.len() && src[k].is_ascii_whitespace() {
                    k += 1;
                }
                let is_key = src[k..].starts_with(b"=>");
                let followed_ok = !is_cb_array || is_key;
                let before = src[..lit.start]
                    .iter()
                    .rev()
                    .find(|b| !b.is_ascii_whitespace());
                let concat = before == Some(&b'.') || src.get(k) == Some(&b'.');
                let in_arrays = stack.iter().all(|&a| a);
                if followed_ok
                    && !concat
                    && in_arrays
                    && let Some(v) = lit.value
                {
                    out.push(v);
                }
                j = lit.end;
                continue;
            }
            match c {
                b'(' => {
                    let is_array = src[..j].ends_with(b"array");
                    stack.push(is_array);
                }
                b'[' => stack.push(true),
                b'{' => stack.push(false),
                b')' | b']' | b'}' => {
                    if stack.pop().is_none() {
                        break;
                    }
                }
                b',' if stack.is_empty() => break,
                _ => {}
            }
            j += 1;
        }
        i = j;
    }
    out
}

fn php_warning(e: &PhpPatternError) -> String {
    String::from_utf8_lossy(&e.message_bytes()).into_owned()
}

#[test]
fn phpt_patterns_compile() {
    let dir = tests_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        eprintln!("skipping: {} not found", dir.display());
        return;
    };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.retain(|p| p.extension().is_some_and(|x| x == "phpt"));
    files.sort();
    let mut total = 0;
    let mut compiled = 0;
    let mut expected_failures = 0;
    let mut problems = Vec::new();
    for path in &files {
        let raw = fs::read(path).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let Some(file) = section(&text, &["FILE"]) else {
            continue;
        };
        let expect = section(&text, &["EXPECT", "EXPECTF", "EXPECTREGEX"]).unwrap_or("");
        // Work on the raw bytes of the FILE section to keep non-UTF-8 bytes.
        let fstart = raw
            .windows(9)
            .position(|w| w == b"--FILE--\n")
            .map(|p| p + 9)
            .unwrap_or(0);
        let fbytes = &raw[fstart..fstart + file.len().min(raw.len() - fstart)];
        for pat in pattern_literals(fbytes) {
            total += 1;
            match compile_php_pattern(&pat) {
                Ok(_) => compiled += 1,
                Err(e) => {
                    let msg = php_warning(&e);
                    let prefix = msg.split(" at offset").next().unwrap_or(&msg).to_string();
                    // Calls that throw before compiling the pattern.
                    let not_reached = expect.contains("Argument #1 ($pattern)");
                    let ok = not_reached
                        || expect.contains(&msg)
                        || (expect.contains(&prefix) && matches!(e, PhpPatternError::Compile(_)))
                        || (prefix.contains("Compilation failed")
                            && expect.contains("Compilation failed: %s"));
                    // Where the expected output spells out the offset, it must match.
                    let needle = format!("{} at offset ", prefix);
                    let offset_mismatch = expect.match_indices(&needle).any(|(pos, _)| {
                        let rest = &expect[pos + needle.len()..];
                        let digits: String =
                            rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                        !digits.is_empty() && !msg.ends_with(&format!(" at offset {}", digits))
                    }) && !expect.contains(&msg);
                    if ok && !offset_mismatch {
                        expected_failures += 1;
                    } else {
                        problems.push(format!(
                            "{}: pattern {:?} failed: {}",
                            path.file_name().unwrap().to_string_lossy(),
                            String::from_utf8_lossy(&pat),
                            msg
                        ));
                    }
                }
            }
        }
    }
    eprintln!(
        "phpt patterns: {} total, {} compiled, {} expected failures, {} problems",
        total,
        compiled,
        expected_failures,
        problems.len()
    );
    assert!(total > 100, "too few patterns found ({total})");
    assert!(
        problems.is_empty(),
        "unexpected failures:\n{}",
        problems.join("\n")
    );
}
