//! Parses every function/method signature of the php-src stubs (when the
//! reference checkout is present) to make sure the stub parser accepts the
//! real-world syntax.

use std::path::{Path, PathBuf};

use php_runtime::native::Signature;

fn stub_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "tests") {
                stub_files(&p, out);
            }
        } else if p.to_string_lossy().ends_with(".stub.php") {
            out.push(p);
        }
    }
}

/// Extracts `function name(...)[: type]` declarations.
fn declarations(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = src[i..].find("function ") {
        let start = i + off;
        i = start + 9;
        if start > 0
            && (b[start - 1].is_ascii_alphanumeric()
                || b[start - 1] == b'_'
                || b[start - 1] == b'$')
        {
            continue;
        }
        let Some(open) = src[i..].find('(').map(|o| i + o) else {
            break;
        };
        if src[i..open].contains(['\n', ';', '{', '=']) {
            continue;
        }
        let mut depth = 0;
        let mut j = open;
        let mut in_str: Option<u8> = None;
        while j < b.len() {
            let c = b[j];
            if let Some(q) = in_str {
                if c == b'\\' {
                    j += 1;
                } else if c == q {
                    in_str = None;
                }
            } else {
                match c {
                    b'"' | b'\'' => in_str = Some(c),
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        let mut end = j + 1;
        let rest = &src[end..];
        let trimmed = rest.trim_start();
        if let Some(ret) = trimmed.strip_prefix(':') {
            let stop = ret.find(['{', ';']).unwrap_or(ret.len());
            end += (rest.len() - trimmed.len()) + 1 + stop;
        }
        out.push(src[start..end].to_string());
        i = end;
    }
    out
}

#[test]
fn all_php_src_stub_signatures_parse() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../php-src-ref");
    if !root.exists() {
        eprintln!("php-src-ref not present; skipping");
        return;
    }
    let mut files = Vec::new();
    for d in ["Zend", "main", "ext"] {
        stub_files(&root.join(d), &mut files);
    }
    let mut total = 0;
    let mut failures = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap_or_default();
        for decl in declarations(&src) {
            total += 1;
            if let Err(e) = Signature::parse(&decl) {
                failures.push(format!("{}: {decl}: {e}", f.display()));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {total} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("parsed {total} stub signatures from {} files", files.len());
}
