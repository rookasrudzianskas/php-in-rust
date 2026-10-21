//! `.phpt` file parser: the port of run-tests.php's `TestFile` class.
//!
//! Sections are kept as raw bytes: test scripts and expectations may contain
//! arbitrary (non-UTF-8) data and must reach the PHP binary unchanged.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::expect::php_trim;

/// Sections run-tests.php accepts (`TestFile::ALLOWED_SECTIONS`).
pub const ALLOWED_SECTIONS: &[&str] = &[
    "EXPECT",
    "EXPECTF",
    "EXPECTREGEX",
    "EXPECTREGEX_EXTERNAL",
    "EXPECT_EXTERNAL",
    "EXPECTF_EXTERNAL",
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
    "FILEEOF",
    "FILE_EXTERNAL",
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

/// A malformed test ("borked" in run-tests terminology).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Borkage(pub String);

impl fmt::Display for Borkage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Borkage {}

/// A parsed `.phpt` file.
#[derive(Debug, Clone)]
pub struct TestFile {
    path: PathBuf,
    sections: HashMap<String, Vec<u8>>,
}

/// PHP truthiness of a string: non-empty and not `"0"`.
fn php_truthy(data: &[u8]) -> bool {
    !(data.is_empty() || data == b"0")
}

/// Match `^--([_A-Z]+)--` at the start of a line.
fn section_header(line: &[u8]) -> Option<&str> {
    let rest = line.strip_prefix(b"--")?;
    let len = rest
        .iter()
        .take_while(|b| b.is_ascii_uppercase() || **b == b'_')
        .count();
    if len == 0 || !rest[len..].starts_with(b"--") {
        return None;
    }
    // The characters are all ASCII, so this cannot fail.
    std::str::from_utf8(&rest[..len]).ok()
}

/// Match `^===DONE===\s*$`.
fn is_done_line(line: &[u8]) -> bool {
    line.strip_prefix(b"===DONE===")
        .is_some_and(|rest| rest.iter().all(|b| b.is_ascii_whitespace() || *b == 0x0B))
}

impl TestFile {
    /// Read and validate a test from disk.
    pub fn load(path: &Path) -> Result<Self, Borkage> {
        let bytes = std::fs::read(path).map_err(|e| {
            Borkage(format!(
                "Cannot open test file: {} ({e}) [{}]",
                path.display(),
                path.display()
            ))
        })?;
        Self::parse(path, &bytes)
    }

    /// Parse and validate test contents (`readFile` + `validateAndProcess`).
    pub fn parse(path: &Path, bytes: &[u8]) -> Result<Self, Borkage> {
        let mut lines = bytes.split_inclusive(|&b| b == b'\n');
        let Some(first) = lines.next() else {
            return Err(Borkage(format!("empty test [{}]", path.display())));
        };
        if !first.starts_with(b"--TEST--") {
            return Err(Borkage(format!(
                "tests must start with --TEST-- [{}]",
                path.display()
            )));
        }

        let mut sections: HashMap<String, Vec<u8>> = HashMap::new();
        sections.insert("TEST".to_string(), Vec::new());
        let mut current = "TEST".to_string();
        let mut secfile = false;
        let mut secdone = false;

        for line in lines {
            if let Some(name) = section_header(line) {
                if sections.get(name).is_some_and(|v| php_truthy(v)) {
                    return Err(Borkage(format!("duplicated {name} section")));
                }
                if !ALLOWED_SECTIONS.contains(&name) {
                    return Err(Borkage(format!("Unknown section \"{name}\"")));
                }
                current = name.to_string();
                sections.insert(current.clone(), Vec::new());
                secfile = matches!(name, "FILE" | "FILEEOF" | "FILE_EXTERNAL");
                secdone = false;
                continue;
            }
            if !secdone && let Some(section) = sections.get_mut(&current) {
                section.extend_from_slice(line);
            }
            if secfile && is_done_line(line) {
                secdone = true;
            }
        }

        let mut file = TestFile {
            path: path.to_path_buf(),
            sections,
        };
        file.validate_and_process()?;
        Ok(file)
    }

    fn validate_and_process(&mut self) -> Result<(), Borkage> {
        // REDIRECTTEST tests are never executed by this runner; accept them
        // as-is so the caller can report a precise SKIP reason.
        if self.has("REDIRECTTEST") {
            return Ok(());
        }
        let file_sections = ["FILE", "FILEEOF", "FILE_EXTERNAL"]
            .iter()
            .filter(|s| self.has(s))
            .count();
        if !self.has("PHPDBG") && file_sections != 1 {
            return Err(Borkage("missing section --FILE--".to_string()));
        }

        if let Some(eof) = self.sections.remove("FILEEOF") {
            let end = eof
                .iter()
                .rposition(|b| *b != b'\r' && *b != b'\n')
                .map_or(0, |p| p + 1);
            self.sections
                .insert("FILE".to_string(), eof[..end].to_vec());
        }

        for prefix in ["FILE", "EXPECT", "EXPECTF", "EXPECTREGEX"] {
            let key = format!("{prefix}_EXTERNAL");
            let Some(spec) = self.sections.get(&key) else {
                continue;
            };
            // Tests may only pull files from their own directory tree.
            let spec = String::from_utf8_lossy(spec).replace("..", "");
            let spec = crate::shell::trim_php(&spec).to_string();
            let dir = self.dir();
            let external = dir.join(&spec);
            match std::fs::read(&external) {
                Ok(data) => {
                    self.sections.insert(prefix.to_string(), data);
                }
                Err(_) => {
                    return Err(Borkage(format!(
                        "could not load --{key}-- {}",
                        external.display()
                    )));
                }
            }
        }

        let expectations = ["EXPECT", "EXPECTF", "EXPECTREGEX"]
            .iter()
            .filter(|s| self.has(s))
            .count();
        if expectations != 1 {
            return Err(Borkage(
                "missing section --EXPECT--, --EXPECTF-- or --EXPECTREGEX--".to_string(),
            ));
        }

        if self.has("PHPDBG") && !self.has("STDIN") {
            let mut stdin = self.sections["PHPDBG"].clone();
            stdin.push(b'\n');
            self.sections.insert("STDIN".to_string(), stdin);
        }
        Ok(())
    }

    /// Directory containing the test file.
    pub fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    /// Path of the `.phpt` file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a section is present (even if empty).
    pub fn has(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    /// Raw section contents.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.sections.get(name).map(Vec::as_slice)
    }

    /// Section contents as (lossy) text, empty if absent.
    pub fn text(&self, name: &str) -> String {
        self.get(name)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    }

    /// PHP's `!empty($sections[$name])`.
    pub fn not_empty(&self, name: &str) -> bool {
        self.get(name).is_some_and(php_truthy)
    }

    /// Replace or add a section (used for SKIPIF-injected XFAIL / FLAKY).
    pub fn set(&mut self, name: &str, value: &[u8]) {
        self.sections.insert(name.to_string(), value.to_vec());
    }

    /// The trimmed `--TEST--` title.
    pub fn title(&self) -> String {
        String::from_utf8_lossy(php_trim(self.get("TEST").unwrap_or(b""))).into_owned()
    }

    /// Whether the test needs the CGI SAPI (`TestFile::isCGI`).
    pub fn is_cgi(&self) -> bool {
        self.has("CGI")
            || [
                "GET",
                "POST",
                "GZIP_POST",
                "DEFLATE_POST",
                "POST_RAW",
                "PUT",
                "COOKIE",
                "EXPECTHEADERS",
            ]
            .iter()
            .any(|s| self.not_empty(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Result<TestFile, Borkage> {
        TestFile::parse(Path::new("/x/t.phpt"), src.as_bytes())
    }

    #[test]
    fn parses_basic_sections() {
        let t = parse("--TEST--\nHello\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n").unwrap();
        assert_eq!(t.title(), "Hello");
        assert_eq!(t.get("FILE"), Some(&b"<?php echo 1;\n"[..]));
        assert_eq!(t.get("EXPECT"), Some(&b"1\n"[..]));
        assert!(!t.has("SKIPIF"));
    }

    #[test]
    fn must_start_with_test_header() {
        let e = parse("--FILE--\nx\n--EXPECT--\nx\n").unwrap_err();
        assert!(e.0.starts_with("tests must start with --TEST--"));
        let e = parse("").unwrap_err();
        assert!(e.0.starts_with("empty test"));
    }

    #[test]
    fn rejects_unknown_and_duplicate_sections() {
        let e = parse("--TEST--\nt\n--FILE--\nx\n--BOGUS--\n--EXPECT--\nx\n").unwrap_err();
        assert_eq!(e.0, "Unknown section \"BOGUS\"");
        let e = parse("--TEST--\nt\n--FILE--\nx\n--FILE--\ny\n--EXPECT--\nx\n").unwrap_err();
        assert_eq!(e.0, "duplicated FILE section");
    }

    #[test]
    fn empty_duplicate_section_is_tolerated() {
        // run-tests only complains when the earlier section is non-empty.
        let t = parse("--TEST--\nt\n--SKIPIF--\n--SKIPIF--\nx\n--FILE--\nx\n--EXPECT--\nx\n");
        assert!(t.is_ok());
    }

    #[test]
    fn requires_file_and_expect() {
        let e = parse("--TEST--\nt\n--EXPECT--\nx\n").unwrap_err();
        assert_eq!(e.0, "missing section --FILE--");
        let e = parse("--TEST--\nt\n--FILE--\nx\n").unwrap_err();
        assert_eq!(
            e.0,
            "missing section --EXPECT--, --EXPECTF-- or --EXPECTREGEX--"
        );
        let e = parse("--TEST--\nt\n--FILE--\nx\n--EXPECT--\nx\n--EXPECTF--\nx\n").unwrap_err();
        assert!(e.0.starts_with("missing section --EXPECT--"));
    }

    #[test]
    fn two_file_sections_are_rejected() {
        let e = parse("--TEST--\nt\n--FILE--\nx\n--FILEEOF--\ny\n--EXPECT--\nx\n").unwrap_err();
        assert_eq!(e.0, "missing section --FILE--");
    }

    #[test]
    fn fileeof_strips_trailing_newlines() {
        let t = parse("--TEST--\nt\n--FILEEOF--\n<?php echo 1;\n\r\n\n--EXPECT--\n1\n").unwrap();
        assert_eq!(t.get("FILE"), Some(&b"<?php echo 1;"[..]));
        assert!(!t.has("FILEEOF"));
    }

    #[test]
    fn done_marker_truncates_file_section() {
        let t = parse(
            "--TEST--\nt\n--FILE--\n<?php\necho 1;\n===DONE===\n<?php garbage\n--EXPECT--\n1\n",
        )
        .unwrap();
        assert_eq!(t.get("FILE"), Some(&b"<?php\necho 1;\n===DONE===\n"[..]));
    }

    #[test]
    fn done_marker_only_applies_to_file_sections() {
        let t = parse("--TEST--\nt\n--FILE--\nx\n--EXPECT--\na\n===DONE===\nb\n").unwrap();
        assert_eq!(t.get("EXPECT"), Some(&b"a\n===DONE===\nb\n"[..]));
    }

    #[test]
    fn done_marker_must_be_a_whole_line() {
        let t = parse("--TEST--\nt\n--FILE--\n===DONE=== x\nmore\n--EXPECT--\nx\n").unwrap();
        assert_eq!(t.get("FILE"), Some(&b"===DONE=== x\nmore\n"[..]));
    }

    #[test]
    fn header_text_after_marker_is_ignored() {
        let t = parse("--TEST--\nt\n--FILE-- trailing junk\nx\n--EXPECT--\nx\n").unwrap();
        assert_eq!(t.get("FILE"), Some(&b"x\n"[..]));
    }

    #[test]
    fn lowercase_or_odd_headers_are_content() {
        let t =
            parse("--TEST--\nt\n--FILE--\n--not-a-section--\n---X--\nx\n--EXPECT--\nx\n").unwrap();
        assert_eq!(t.get("FILE"), Some(&b"--not-a-section--\n---X--\nx\n"[..]));
    }

    #[test]
    fn keeps_binary_bytes_and_crlf() {
        let bytes = b"--TEST--\r\nbin\r\n--FILE--\r\n\xff\xfe\r\n--EXPECT--\r\n\xff\r\n";
        let t = TestFile::parse(Path::new("/x/t.phpt"), bytes).unwrap();
        assert_eq!(t.get("FILE"), Some(&b"\xff\xfe\r\n"[..]));
        assert_eq!(t.title(), "bin");
    }

    #[test]
    fn not_empty_follows_php_empty() {
        let t = parse("--TEST--\nt\n--ENV--\n0\n--SKIPIF--\n<?php\n--FILE--\nx\n--EXPECT--\nx\n")
            .unwrap();
        // "0\n" is not the string "0", so it is non-empty.
        assert!(t.not_empty("ENV"));
        assert!(t.not_empty("SKIPIF"));
        assert!(!t.not_empty("INI"));
    }

    #[test]
    fn cgi_detection() {
        let t = parse("--TEST--\nt\n--GET--\nfoo=bar\n--FILE--\nx\n--EXPECT--\nx\n").unwrap();
        assert!(t.is_cgi());
        let t = parse("--TEST--\nt\n--GET--\n--FILE--\nx\n--EXPECT--\nx\n").unwrap();
        assert!(!t.is_cgi(), "an empty GET section does not make a CGI test");
        let t = parse("--TEST--\nt\n--CGI--\n--FILE--\nx\n--EXPECT--\nx\n").unwrap();
        assert!(t.is_cgi());
        let t = parse("--TEST--\nt\n--FILE--\nx\n--EXPECT--\nx\n").unwrap();
        assert!(!t.is_cgi());
    }

    #[test]
    fn redirecttest_skips_validation() {
        let t = parse("--TEST--\nt\n--REDIRECTTEST--\nreturn [];\n").unwrap();
        assert!(t.has("REDIRECTTEST"));
    }

    #[test]
    fn phpdbg_tests_need_no_file_section() {
        let t = parse("--TEST--\nt\n--PHPDBG--\nr\nq\n--EXPECT--\nx\n").unwrap();
        assert_eq!(t.get("STDIN"), Some(&b"r\nq\n\n"[..]));
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("php-phpt-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn external_sections_are_loaded() {
        let dir = scratch_dir("external");
        std::fs::write(dir.join("exp.txt"), "from file\n").unwrap();
        std::fs::write(dir.join("code.inc"), "<?php echo 'x';").unwrap();
        let src = "--TEST--\nt\n--FILE_EXTERNAL--\ncode.inc\n--EXPECT_EXTERNAL--\nexp.txt\n";
        let t = TestFile::parse(&dir.join("t.phpt"), src.as_bytes()).unwrap();
        assert_eq!(t.get("EXPECT"), Some(&b"from file\n"[..]));
        assert_eq!(t.get("FILE"), Some(&b"<?php echo 'x';"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_section_cannot_escape_directory() {
        let dir = scratch_dir("escape");
        let src = "--TEST--\nt\n--FILE--\nx\n--EXPECT_EXTERNAL--\n../secret.txt\n";
        let e = TestFile::parse(&dir.join("t.phpt"), src.as_bytes()).unwrap_err();
        assert!(e.0.starts_with("could not load --EXPECT_EXTERNAL--"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_external_file_borks() {
        let dir = scratch_dir("missing");
        let src = "--TEST--\nt\n--FILE--\nx\n--EXPECTF_EXTERNAL--\nnope.txt\n";
        let e = TestFile::parse(&dir.join("t.phpt"), src.as_bytes()).unwrap_err();
        assert!(e.0.contains("nope.txt"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
