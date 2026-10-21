//! Parse the `--FILE--` section of every `.phpt` test under a php-src
//! checkout and compare with the expected outcome.
//!
//! ```text
//! cargo run --release -p php-syntax --example corpus -- [php-src dir] [--verbose] [--filter substr]
//! ```
//!
//! A test is expected to fail parsing when its expected output contains
//! `Parse error`; otherwise it must parse. Compile errors raised while
//! parsing (`ParseErrorKind::Compile`) count as correct when the expected
//! output contains `Fatal error: <message>`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use php_syntax::{ParseErrorKind, grammar_accepts, parse};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "phpt") {
            out.push(p);
        }
    }
}

/// Split a .phpt file into `(section name, content)`.
fn sections(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for line in data.split_inclusive(|&b| b == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
        if trimmed.len() > 4
            && trimmed.starts_with(b"--")
            && trimmed.ends_with(b"--")
            && trimmed[2..trimmed.len() - 2]
                .iter()
                .all(|c| c.is_ascii_uppercase() || *c == b'_')
        {
            out.push((
                String::from_utf8_lossy(&trimmed[2..trimmed.len() - 2]).into_owned(),
                Vec::new(),
            ));
            continue;
        }
        if let Some(last) = out.last_mut() {
            last.1.extend_from_slice(line);
        }
    }
    out
}

/// Minimal EXPECTF matching: `%s`/`%a` match anything, `%d` digits.
fn wildcard_match(pat: &[u8], s: &[u8]) -> bool {
    if pat.is_empty() {
        return s.is_empty();
    }
    if pat.len() >= 2 && pat[0] == b'%' {
        match pat[1] {
            b's' | b'a' | b'A' | b'S' => {
                return (0..=s.len()).any(|i| wildcard_match(&pat[2..], &s[i..]));
            }
            b'd' | b'i' => {
                let n = s.iter().take_while(|c| c.is_ascii_digit()).count();
                return (1..=n).any(|i| wildcard_match(&pat[2..], &s[i..]));
            }
            _ => {}
        }
    }
    !s.is_empty() && pat[0] == s[0] && wildcard_match(&pat[1..], &s[1..])
}

fn main() {
    // Deeply nested test inputs need a big stack, as php-src's parser does.
    let t = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(run)
        .expect("thread");
    t.join().expect("join");
}

fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../php-src-ref").to_string());
    let verbose = args.iter().any(|a| a == "--verbose");
    let filter = args
        .iter()
        .position(|a| a == "--filter")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);

    let (mut ok_valid, mut bad_valid, mut ok_invalid, mut bad_invalid) = (0, 0, 0, 0);
    let mut msg_match = 0;
    let mut msg_total = 0;
    let mut panics = 0;
    let mut oracle_mismatch = 0;
    let mut missed_resolution = 0;
    let mut total_bytes = 0usize;
    let mut parse_time = std::time::Duration::ZERO;
    let start = Instant::now();
    let mut report = String::new();

    for f in &files {
        let path = f.to_string_lossy();
        if let Some(flt) = &filter
            && !path.contains(flt.as_str())
        {
            continue;
        }
        let Ok(data) = std::fs::read(f) else { continue };
        let secs = sections(&data);
        let file = secs.iter().find(|(n, _)| n == "FILE" || n == "FILEEOF");
        let Some((name, src)) = file else { continue };
        let mut src = src.clone();
        if name == "FILEEOF" {
            while matches!(src.last(), Some(b'\n' | b'\r')) {
                src.pop();
            }
        }
        let expected: Vec<u8> = secs
            .iter()
            .filter(|(n, _)| n == "EXPECT" || n == "EXPECTF" || n == "EXPECTREGEX")
            .flat_map(|(_, c)| c.clone())
            .collect();
        let expected = String::from_utf8_lossy(&expected).into_owned();
        let expect_parse_error = expected.contains("Parse error");
        total_bytes += src.len();

        if std::env::var_os("CORPUS_TRACE").is_some() {
            eprintln!("{path}");
        }
        let t0 = Instant::now();
        let result = std::panic::catch_unwind(|| parse(&src, "test.php"));
        let dt = t0.elapsed();
        parse_time += dt;
        if dt.as_millis() > 20 {
            eprintln!("SLOW {:?} {} bytes {path}", dt, src.len());
        }
        let result = match result {
            Ok(r) => r,
            Err(_) => {
                panics += 1;
                report.push_str(&format!("PANIC {path}\n"));
                continue;
            }
        };
        // Cross-check acceptance with the bison grammar oracle (syntax only:
        // compile errors raised from grammar actions are not modelled).
        let syntax_ok = match &result {
            Ok(_) => true,
            Err(e) => e.kind == ParseErrorKind::Compile,
        };
        if syntax_ok != grammar_accepts(&src) {
            oracle_mismatch += 1;
            report.push_str(&format!("ORACLE {path} (ours: {syntax_ok})\n"));
        }
        match (&result, expect_parse_error) {
            (Ok(_), false) => {
                ok_valid += 1;
                // Compile errors that name resolution is responsible for.
                const RESOLUTION_ERRORS: [&str; 12] = [
                    "because the name is already in use",
                    "is an invalid class name",
                    "Namespace declaration statement has to be",
                    "No code may exist outside of namespace",
                    "Cannot mix bracketed namespace",
                    "Namespace declarations cannot be nested",
                    "is a special class name",
                    "as it is reserved",
                    "(previously declared as local import)",
                    "must be unqualified",
                    "as namespace name",
                    "can only be used from the outermost scope",
                ];
                if let Some(l) = expected.lines().find(|l| {
                    l.contains("Fatal error:") && RESOLUTION_ERRORS.iter().any(|m| l.contains(m))
                }) {
                    missed_resolution += 1;
                    report.push_str(&format!("MISSED {path}\n    expected: {l}\n"));
                }
            }
            (Err(e), true) => {
                ok_invalid += 1;
                msg_total += 1;
                let got = format!("{} in %s on line {}", e.message, e.line);
                let matched = expected
                    .lines()
                    .filter(|l| l.contains("Parse error"))
                    .any(|l| {
                        let l = l.trim_end();
                        let Some(p) = l.find("Parse error:") else {
                            return false;
                        };
                        let pat = l[p + "Parse error:".len()..].trim_start();
                        // Compare the message and the line separately.
                        let (pmsg, pline) = match pat.rfind(" on line ") {
                            Some(i) => (&pat[..i], &pat[i + 9..]),
                            None => (pat, "%d"),
                        };
                        let (gmsg, gline) = got.rsplit_once(" on line ").expect("line");
                        let pmsg = pmsg.rsplit_once(" in ").map_or(pmsg, |x| x.0);
                        let gmsg = gmsg.rsplit_once(" in ").map_or(gmsg, |x| x.0);
                        wildcard_match(pmsg.as_bytes(), gmsg.as_bytes())
                            && (pline == "%d" || pline == gline)
                    });
                if matched {
                    msg_match += 1;
                } else {
                    report.push_str(&format!(
                        "MSG {path}\n    got: {} (line {})\n    expected: {}\n",
                        e.message,
                        e.line,
                        expected
                            .lines()
                            .find(|l| l.contains("Parse error"))
                            .unwrap_or("")
                    ));
                }
            }
            (Err(e), false) => {
                let fatal_expected = expected.lines().any(|l| {
                    l.find("Fatal error: ").is_some_and(|p| {
                        let pat = &l[p + "Fatal error: ".len()..];
                        let pat = pat.rsplit_once(" on line ").map_or(pat, |x| x.0);
                        let pat = pat.rsplit_once(" in ").map_or(pat, |x| x.0);
                        wildcard_match(pat.as_bytes(), e.message.as_bytes())
                    })
                });
                if e.kind == ParseErrorKind::Compile && fatal_expected {
                    ok_valid += 1;
                } else {
                    bad_valid += 1;
                    report.push_str(&format!(
                        "REJECT {path}\n    {} (line {})\n",
                        e.message, e.line
                    ));
                }
            }
            (Ok(_), true) => {
                bad_invalid += 1;
                report.push_str(&format!(
                    "ACCEPT {path}\n    expected: {}\n",
                    expected
                        .lines()
                        .find(|l| l.contains("Parse error"))
                        .unwrap_or("")
                ));
            }
        }
    }
    let elapsed = start.elapsed();
    let valid = ok_valid + bad_valid;
    let invalid = ok_invalid + bad_invalid;
    if verbose {
        print!("{report}");
    } else {
        let out = std::env::temp_dir().join("php-syntax-corpus-report.txt");
        let _ = std::fs::write(&out, &report);
        println!("report written to {}", out.display());
    }
    println!(
        "valid:   {ok_valid}/{valid} accepted ({:.2}%)",
        100.0 * ok_valid as f64 / valid.max(1) as f64
    );
    println!("invalid: {ok_invalid}/{invalid} rejected");
    println!("parse error messages matching: {msg_match}/{msg_total}");
    println!("panics: {panics}");
    println!("disagreements with the grammar oracle: {oracle_mismatch}");
    println!("expected name-resolution errors not raised: {missed_resolution}");
    println!(
        "parsed {:.1} MB in {:.2?} (total run {:.2?})",
        total_bytes as f64 / 1e6,
        parse_time,
        elapsed
    );
}
