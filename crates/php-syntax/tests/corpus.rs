//! Parse the `--FILE--` section of every `.phpt` in php-src and compare
//! with the expected outcome (run with `cargo test -p php-syntax --release
//! --test corpus -- --ignored`; see also `examples/corpus.rs` for a
//! detailed report).

use std::path::{Path, PathBuf};

use php_syntax::{ParseErrorKind, grammar_accepts, parse};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "phpt") {
            out.push(p);
        }
    }
}

fn section(data: &str, names: &[&'static str]) -> Option<(&'static str, String)> {
    let mut found: Option<(&'static str, String)> = None;
    for line in data.split_inclusive('\n') {
        let t = line.trim_end();
        if t.len() > 4
            && t.starts_with("--")
            && t.ends_with("--")
            && t[2..t.len() - 2]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_')
        {
            if found.is_some() {
                break;
            }
            if let Some(n) = names.iter().find(|n| **n == &t[2..t.len() - 2]) {
                found = Some((n, String::new()));
            }
            continue;
        }
        if let Some((_, s)) = found.as_mut() {
            s.push_str(line);
        }
    }
    found
}

#[test]
#[ignore = "walks the whole php-src test suite"]
fn php_src_corpus() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../php-src-ref"));
    let mut files = Vec::new();
    collect(root, &mut files);
    assert!(!files.is_empty(), "php-src-ref not found");
    let (mut valid, mut accepted, mut invalid, mut rejected, mut oracle) = (0, 0, 0, 0, 0);
    let mut rejected_valid = Vec::new();
    for f in &files {
        let Ok(data) = std::fs::read(f) else { continue };
        let data = String::from_utf8_lossy(&data);
        let Some((name, mut src)) = section(&data, &["FILE", "FILEEOF"]) else {
            continue;
        };
        if name == "FILEEOF" {
            src.truncate(src.trim_end_matches(['\r', '\n']).len());
        }
        let expected: String = ["EXPECT", "EXPECTF", "EXPECTREGEX"]
            .iter()
            .filter_map(|s| section(&data, &[s]).map(|x| x.1))
            .collect();
        let raw = std::fs::read(f).unwrap();
        // Use the original bytes of the FILE section (it may not be UTF-8).
        let src_bytes = extract_bytes(&raw, name).unwrap_or_else(|| src.into_bytes());
        let result = parse(&src_bytes, "t.php");
        let syntax_ok = result
            .as_ref()
            .map_or_else(|e| e.kind == ParseErrorKind::Compile, |_| true);
        if syntax_ok != grammar_accepts(&src_bytes)
            && !data.contains("<?=")
            && !data.contains("__HALT_COMPILER")
        {
            oracle += 1;
        }
        if expected.contains("Parse error") {
            invalid += 1;
            rejected += usize::from(result.is_err());
        } else {
            valid += 1;
            match &result {
                Ok(_) => accepted += 1,
                Err(e) if e.kind == ParseErrorKind::Compile && expected.contains("Fatal error") => {
                    accepted += 1
                }
                Err(e) => rejected_valid.push(format!("{}: {}", f.display(), e)),
            }
        }
    }
    let rate = accepted as f64 / valid as f64;
    println!(
        "valid: {accepted}/{valid} ({:.2}%), invalid rejected: {rejected}/{invalid}, oracle mismatches: {oracle}",
        rate * 100.0
    );
    assert!(rate >= 0.995, "acceptance {rate}: {rejected_valid:#?}");
    assert_eq!(oracle, 0);
}

fn extract_bytes(raw: &[u8], name: &str) -> Option<Vec<u8>> {
    let marker = format!("--{name}--");
    let start = raw
        .windows(marker.len())
        .position(|w| w == marker.as_bytes())?
        + marker.len();
    let rest = &raw[start..];
    let rest = rest
        .strip_prefix(b"\r\n")
        .or_else(|| rest.strip_prefix(b"\n"))
        .unwrap_or(rest);
    let mut end = rest.len();
    let mut i = 0;
    while i < rest.len() {
        let line_end = rest[i..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |p| i + p);
        let line = &rest[i..line_end];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.len() > 4
            && line.starts_with(b"--")
            && line.ends_with(b"--")
            && line[2..line.len() - 2]
                .iter()
                .all(|c| c.is_ascii_uppercase() || *c == b'_')
        {
            end = i;
            break;
        }
        i = line_end + 1;
    }
    let mut v = rest[..end].to_vec();
    if name == "FILEEOF" {
        while matches!(v.last(), Some(b'\n' | b'\r')) {
            v.pop();
        }
    }
    Some(v)
}
