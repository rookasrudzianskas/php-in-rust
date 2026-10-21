//! Parses every `.phpt` file of the php-src reference checkout and verifies
//! that only tests run-tests.php itself would bork are reported as borked.
//!
//! Skipped when `php-src-ref/` is not present.  Set `PHPT_CORPUS_DUMP=<file>`
//! to write one line per test (verdict plus a digest of every section) for
//! cross-checking against another implementation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use php_phpt::cli::find_tests;
use php_phpt::expect::{Matcher, normalize_expected};
use php_phpt::phpt::TestFile;

fn corpus_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../php-src-ref");
    root.join("run-tests.php").is_file().then_some(root)
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Section names of a parsed test (everything run-tests keeps).
const NAMES: &[&str] = &[
    "TEST",
    "EXPECT",
    "EXPECTF",
    "EXPECTREGEX",
    "EXPECTHEADERS",
    "POST",
    "POST_RAW",
    "GZIP_POST",
    "DEFLATE_POST",
    "PUT",
    "GET",
    "COOKIE",
    "ARGS",
    "FILE",
    "REDIRECTTEST",
    "CAPTURE_STDIO",
    "STDIN",
    "CGI",
    "PHPDBG",
    "INI",
    "ENV",
    "EXTENSIONS",
    "SKIPIF",
    "XFAIL",
    "XLEAK",
    "CLEAN",
    "CREDITS",
    "DESCRIPTION",
    "CONFLICTS",
    "WHITESPACE_SENSITIVE",
    "FLAKY",
];

#[test]
fn php_src_corpus_parses_like_run_tests() {
    let Some(root) = corpus_root() else {
        eprintln!("php-src-ref not found; skipping corpus test");
        return;
    };
    let root = std::fs::canonicalize(root).unwrap();
    let mut tests = Vec::new();
    find_tests(&root, &mut tests).unwrap();
    tests.sort();
    assert!(
        tests.len() > 20_000,
        "unexpectedly small corpus: {}",
        tests.len()
    );

    let mut borks: BTreeMap<String, String> = BTreeMap::new();
    let mut dump = String::new();
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    // Expectations the regex translator cannot handle.
    let mut untranslatable: BTreeMap<String, String> = BTreeMap::new();
    for path in &tests {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        match TestFile::load(path) {
            Ok(tf) => {
                let mut line = format!("{rel}\tOK");
                for name in NAMES {
                    if let Some(data) = tf.get(name) {
                        line.push_str(&format!("\t{name}:{}:{:016x}", data.len(), fnv1a(data)));
                    }
                }
                dump.push_str(&line);
                dump.push('\n');
                let compiled = if let Some(section) = tf.get("EXPECTF") {
                    *kinds.entry("expectf").or_default() += 1;
                    Some(Matcher::expectf(&normalize_expected(section)))
                } else if let Some(section) = tf.get("EXPECTREGEX") {
                    *kinds.entry("expectregex").or_default() += 1;
                    Some(Matcher::regex(&normalize_expected(section)))
                } else {
                    None
                };
                if let Some(Err(e)) = compiled {
                    untranslatable.insert(rel.clone(), e.0);
                }
                if tf.is_cgi() {
                    *kinds.entry("cgi").or_default() += 1;
                }
                if tf.has("PHPDBG") {
                    *kinds.entry("phpdbg").or_default() += 1;
                }
                if tf.has("REDIRECTTEST") {
                    *kinds.entry("redirecttest").or_default() += 1;
                }
            }
            Err(e) => {
                // Messages embed absolute paths; make them root-relative.
                let msg = e.0.replace(&format!("{}/", root.display()), "");
                dump.push_str(&format!("{rel}\tBORK\t{msg}\n"));
                borks.insert(rel, msg);
            }
        }
    }

    if let Ok(file) = std::env::var("PHPT_CORPUS_DUMP") {
        std::fs::write(file, &dump).unwrap();
    }
    eprintln!(
        "parsed {} tests, {} borked; skipped-by-section counts: {kinds:?}",
        tests.len(),
        borks.len()
    );
    for (path, msg) in &borks {
        eprintln!("  BORK {path}: {msg}");
    }

    // An independent transcription of run-tests.php's TestFile (see the
    // development notes) accepts every test in the corpus, so none may bork.
    assert!(borks.is_empty(), "unexpected BORKs: {borks:#?}");

    for (path, msg) in &untranslatable {
        eprintln!("  UNTRANSLATABLE {path}: {msg}");
    }
    // The only expectation needing PCRE lookaround lives in the (out of
    // scope) zip extension.
    let unexpected: Vec<_> = untranslatable
        .keys()
        .filter(|p| p.as_str() != "ext/zip/tests/bug76524.phpt")
        .collect();
    assert!(
        unexpected.is_empty(),
        "untranslatable expectations: {untranslatable:#?}"
    );
}
