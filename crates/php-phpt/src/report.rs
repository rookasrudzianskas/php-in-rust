//! Test statuses, console/JSON reporting and passing-list bookkeeping.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Outcome of one test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Pass,
    Fail,
    Skip,
    /// Failed, and the test carries an `--XFAIL--` section.
    XFail,
    /// Passed although it carries an `--XFAIL--` section (run-tests: WARN).
    XPass,
    /// Malformed test or invalid SKIPIF / CLEAN output.
    Bork,
    Timeout,
    /// Passed, but with a caveat (e.g. only on retry).
    Warn,
}

impl Status {
    pub const ALL: [Status; 8] = [
        Status::Pass,
        Status::Fail,
        Status::Skip,
        Status::XFail,
        Status::XPass,
        Status::Bork,
        Status::Timeout,
        Status::Warn,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
            Status::XFail => "XFAIL",
            Status::XPass => "XPASS",
            Status::Bork => "BORK",
            Status::Timeout => "TIMEOUT",
            Status::Warn => "WARN",
        }
    }

    /// Statuses that make the run exit non-zero (like run-tests' FAILED/BORKED).
    pub fn is_failure(self) -> bool {
        matches!(self, Status::Fail | Status::Bork | Status::Timeout)
    }
}

/// Result of running one test.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub path: PathBuf,
    /// Display name: path relative to the repository root when possible.
    pub name: String,
    pub title: String,
    pub status: Status,
    /// Short machine-friendly explanation (empty for plain passes).
    pub reason: String,
    /// Suffix printed after the result line (run-tests' `$info`).
    pub info: String,
    pub duration: Duration,
    /// Diff of expected vs actual output for failures.
    pub diff: Option<Vec<u8>>,
}

impl TestResult {
    pub fn new(path: &Path, name: &str, status: Status) -> Self {
        Self {
            path: path.to_path_buf(),
            name: name.to_string(),
            title: String::new(),
            status,
            reason: String::new(),
            info: String::new(),
            duration: Duration::ZERO,
            diff: None,
        }
    }
}

/// Render the one-line console result, like run-tests' `show_result()`.
pub fn result_line(r: &TestResult, color: bool) -> String {
    let label = r.status.label();
    let label = if color {
        let code = match r.status {
            Status::Pass => "1;32",
            Status::Fail | Status::Bork | Status::Timeout => "1;31",
            _ => "1;33",
        };
        format!("\x1b[{code}m{label}\x1b[0m")
    } else {
        label.to_string()
    };
    let mut line = match r.status {
        Status::Bork => format!("{label} {} [{}]", r.reason, r.name),
        _ => format!("{label} {} [{}]", r.title, r.name),
    };
    if !r.info.is_empty() {
        line.push(' ');
        line.push_str(&r.info);
    }
    line
}

/// Per-status counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub pass: usize,
    pub fail: usize,
    pub skip: usize,
    pub xfail: usize,
    pub xpass: usize,
    pub bork: usize,
    pub timeout: usize,
    pub warn: usize,
}

impl Counts {
    pub fn of(results: &[TestResult]) -> Self {
        let mut c = Counts::default();
        for r in results {
            c.add(r.status);
        }
        c
    }

    pub fn add(&mut self, status: Status) {
        match status {
            Status::Pass => self.pass += 1,
            Status::Fail => self.fail += 1,
            Status::Skip => self.skip += 1,
            Status::XFail => self.xfail += 1,
            Status::XPass => self.xpass += 1,
            Status::Bork => self.bork += 1,
            Status::Timeout => self.timeout += 1,
            Status::Warn => self.warn += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.pass
            + self.fail
            + self.skip
            + self.xfail
            + self.xpass
            + self.bork
            + self.timeout
            + self.warn
    }

    /// Tests run-tests would report as WARNED.
    pub fn warned(&self) -> usize {
        self.warn + self.xpass
    }

    pub fn get(&self, status: Status) -> usize {
        match status {
            Status::Pass => self.pass,
            Status::Fail => self.fail,
            Status::Skip => self.skip,
            Status::XFail => self.xfail,
            Status::XPass => self.xpass,
            Status::Bork => self.bork,
            Status::Timeout => self.timeout,
            Status::Warn => self.warn,
        }
    }
}

fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}

/// The counts block of `get_summary()`.
pub fn summary_text(results: &[TestResult], elapsed: Duration) -> String {
    let c = Counts::of(results);
    let n_total = c.total();
    let x_total = n_total - c.skip - c.bork;
    let row = |label: &str, n: usize, with_exec: bool| {
        let mut s = format!("{label} : {n:5} ({:5.1}%)", pct(n, n_total));
        if with_exec {
            let _ = write!(s, " ({:5.1}%)", pct(n, x_total));
        } else {
            s.push_str(" --------");
        }
        s
    };

    let mut out = String::new();
    out.push_str(&format!(
        "\nNumber of tests : {n_total:5}          {x_total:8}"
    ));
    if c.bork > 0 {
        out.push('\n');
        out.push_str(&row("Tests borked   ", c.bork, false));
    }
    out.push('\n');
    out.push_str(&row("Tests skipped  ", c.skip, false));
    out.push('\n');
    out.push_str(&row("Tests warned   ", c.warned(), true));
    out.push('\n');
    out.push_str(&row("Tests failed   ", c.fail, true));
    if c.timeout > 0 {
        out.push('\n');
        out.push_str(&row("Tests timed out", c.timeout, true));
    }
    if c.xfail > 0 {
        out.push('\n');
        out.push_str(&row("Expected fail  ", c.xfail, true));
    }
    out.push('\n');
    out.push_str(&row("Tests passed   ", c.pass, true));
    out.push_str(&format!(
        "\n---------------------------------------------------------------------\n\
         Time taken      : {:5.3} seconds\n\
         =====================================================================\n",
        elapsed.as_secs_f64()
    ));
    out
}

/// The `FAILED / BORKED / WARNED TEST SUMMARY` blocks of `get_summary()`.
pub fn failed_summary(results: &[TestResult]) -> String {
    const RULE: &str = "=====================================================================";
    const DASH: &str = "---------------------------------------------------------------------";
    let block = |title: &str, lines: Vec<String>| -> String {
        if lines.is_empty() {
            return String::new();
        }
        let mut s = format!("\n{RULE}\n{title}\n{DASH}\n");
        for l in lines {
            s.push_str(&l);
            s.push('\n');
        }
        s.push_str(RULE);
        s.push('\n');
        s
    };
    let describe = |r: &TestResult| {
        let mut s = format!("{} [{}]", r.title, r.name);
        if !r.reason.is_empty() {
            s.push_str("  -- ");
            s.push_str(&r.reason);
        }
        s
    };

    let borked = results
        .iter()
        .filter(|r| r.status == Status::Bork)
        .map(|r| format!("{} [{}]", r.reason, r.path.display()))
        .collect();
    let failed = results
        .iter()
        .filter(|r| matches!(r.status, Status::Fail | Status::Timeout))
        .map(|r| {
            if r.status == Status::Timeout {
                format!("{} (TIMEOUT)", describe(r))
            } else {
                describe(r)
            }
        })
        .collect();
    let warned = results
        .iter()
        .filter(|r| matches!(r.status, Status::Warn | Status::XPass))
        .map(|r| format!("{} {}", describe(r), r.info))
        .collect();

    let mut out = String::new();
    out.push_str(&block("BORKED TEST SUMMARY", borked));
    out.push_str(&block("FAILED TEST SUMMARY", failed));
    out.push_str(&block("WARNED TEST SUMMARY", warned));
    out
}

/// Per-directory pass/fail table (`--summary-by-dir`).
pub fn dir_summary(results: &[TestResult]) -> String {
    let mut by_dir: BTreeMap<String, Counts> = BTreeMap::new();
    for r in results {
        let dir = Path::new(&r.name)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| ".".to_string());
        by_dir.entry(dir).or_default().add(r.status);
    }
    let width = by_dir.keys().map(String::len).max().unwrap_or(9).max(9);
    let mut out = format!(
        "{:<width$}  {:>6} {:>6} {:>6} {:>6} {:>6}  {:>6}\n",
        "Directory", "total", "pass", "fail", "skip", "other", "pass%"
    );
    out.push_str(&format!("{}\n", "-".repeat(width + 48)));
    for (dir, c) in &by_dir {
        let fail = c.fail + c.bork + c.timeout;
        let other = c.xfail + c.xpass + c.warn;
        let runnable = c.total() - c.skip;
        out.push_str(&format!(
            "{dir:<width$}  {:>6} {:>6} {:>6} {:>6} {:>6}  {:>5.1}%\n",
            c.total(),
            c.pass,
            fail,
            c.skip,
            other,
            pct(c.pass, runnable)
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Escape a string as a JSON string literal (including the quotes).
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Machine-readable results: a summary object plus one record per test.
pub fn json_report(php: &str, results: &[TestResult], elapsed: Duration) -> String {
    let counts = Counts::of(results);
    let mut out = String::from("{\n");
    let _ = writeln!(out, "  \"php\": {},", json_string(php));
    let _ = writeln!(
        out,
        "  \"duration_ms\": {},",
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    );
    let _ = writeln!(out, "  \"total\": {},", counts.total());
    out.push_str("  \"summary\": {");
    let parts: Vec<String> = Status::ALL
        .iter()
        .map(|s| format!("\"{}\": {}", s.label(), counts.get(*s)))
        .collect();
    out.push_str(&parts.join(", "));
    out.push_str("},\n  \"tests\": [");
    for (i, r) in results.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        let _ = write!(
            out,
            "    {{\"path\": {}, \"status\": {}, \"reason\": {}, \"duration_ms\": {}}}",
            json_string(&r.name),
            json_string(r.status.label()),
            json_string(&r.reason),
            u64::try_from(r.duration.as_millis()).unwrap_or(u64::MAX)
        );
    }
    out.push_str(if results.is_empty() {
        "]\n}\n"
    } else {
        "\n  ]\n}\n"
    });
    out
}

// ---------------------------------------------------------------------------
// Passing lists
// ---------------------------------------------------------------------------

/// Header written into freshly created passing lists.
const NEW_LIST_HEADER: &str = "\
# Tests that pass under php-run-tests, one path per line (relative to the repository root).
# Maintained by `php-run-tests --update-passing`; the list only grows.
";

/// Parse a passing list: one path per line, `#` comments and blanks ignored.
pub fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // Trailing ` # comment`.
            let line = match line.find(" #").or_else(|| line.find("\t#")) {
                Some(p) => line[..p].trim_end(),
                None => line,
            };
            (!line.is_empty()).then(|| line.to_string())
        })
        .collect()
}

/// Outcome of rewriting a passing list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListUpdate {
    pub text: String,
    pub added: Vec<String>,
    /// Entries dropped because they ran and did not pass, or vanished.
    pub removed: Vec<String>,
    pub total: usize,
}

/// Compute the new contents of a passing list after a run.
///
/// The result is every test that passed in this run, plus every existing
/// entry that was *not* part of the run and still exists on disk (so partial
/// runs never shrink the list).  Entries that ran without passing are dropped
/// and reported in [`ListUpdate::removed`].
pub fn update_list(
    existing: Option<&str>,
    results: &[TestResult],
    exists: impl Fn(&str) -> bool,
) -> ListUpdate {
    let old: BTreeSet<String> = existing
        .map(parse_list)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let ran: BTreeSet<&str> = results.iter().map(|r| r.name.as_str()).collect();
    let mut new: BTreeSet<String> = results
        .iter()
        .filter(|r| r.status == Status::Pass)
        .map(|r| r.name.clone())
        .collect();
    for entry in &old {
        if !ran.contains(entry.as_str()) && exists(entry) {
            new.insert(entry.clone());
        }
    }

    // Keep the leading comment block of an existing file.
    let header: String = match existing {
        Some(text) => text
            .lines()
            .take_while(|l| {
                let l = l.trim();
                l.is_empty() || l.starts_with('#')
            })
            .map(|l| format!("{l}\n"))
            .collect(),
        None => NEW_LIST_HEADER.to_string(),
    };

    let mut text = header;
    for entry in &new {
        text.push_str(entry);
        text.push('\n');
    }
    ListUpdate {
        added: new.difference(&old).cloned().collect(),
        removed: old.difference(&new).cloned().collect(),
        total: new.len(),
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(name: &str, status: Status) -> TestResult {
        let mut r = TestResult::new(Path::new("/r").join(name).as_path(), name, status);
        r.title = format!("title of {name}");
        r
    }

    #[test]
    fn counts_and_totals() {
        let rs = vec![
            res("a", Status::Pass),
            res("b", Status::Fail),
            res("c", Status::Skip),
            res("d", Status::XPass),
            res("e", Status::Warn),
            res("f", Status::Timeout),
        ];
        let c = Counts::of(&rs);
        assert_eq!(c.total(), 6);
        assert_eq!(c.warned(), 2);
        assert!(
            Status::Fail.is_failure() && Status::Bork.is_failure() && Status::Timeout.is_failure()
        );
        assert!(
            !Status::XPass.is_failure()
                && !Status::XFail.is_failure()
                && !Status::Skip.is_failure()
        );
    }

    #[test]
    fn summary_matches_run_tests_layout() {
        let rs = vec![
            res("a", Status::Pass),
            res("b", Status::Pass),
            res("c", Status::Fail),
            res("d", Status::Skip),
        ];
        let s = summary_text(&rs, Duration::from_millis(1500));
        let header = format!("Number of tests :     4{}3", " ".repeat(17));
        assert!(s.contains(&header), "{s}");
        assert!(
            s.contains("Tests skipped   :     1 ( 25.0%) --------"),
            "{s}"
        );
        assert!(
            s.contains("Tests failed    :     1 ( 25.0%) ( 33.3%)"),
            "{s}"
        );
        assert!(
            s.contains("Tests passed    :     2 ( 50.0%) ( 66.7%)"),
            "{s}"
        );
        assert!(s.contains("Time taken      : 1.500 seconds"), "{s}");
        assert!(!s.contains("borked"));
        assert!(!s.contains("timed out"));
        assert!(!s.contains("Expected fail"));
    }

    #[test]
    fn summary_shows_optional_rows_when_present() {
        let rs = vec![
            res("a", Status::Bork),
            res("b", Status::Timeout),
            res("c", Status::XFail),
        ];
        let s = summary_text(&rs, Duration::ZERO);
        assert!(s.contains("Tests borked    :     1"), "{s}");
        assert!(s.contains("Tests timed out :     1"), "{s}");
        assert!(s.contains("Expected fail   :     1"), "{s}");
    }

    #[test]
    fn summary_of_nothing_does_not_divide_by_zero() {
        let s = summary_text(&[], Duration::ZERO);
        assert!(s.contains("Number of tests :     0"), "{s}");
    }

    #[test]
    fn failed_summary_lists_failures() {
        let mut f = res("dir/f.phpt", Status::Fail);
        f.reason = "line 3".into();
        let mut b = res("dir/b.phpt", Status::Bork);
        b.reason = "missing section --FILE--".into();
        let s = failed_summary(&[f, b, res("p.phpt", Status::Pass)]);
        assert!(s.contains("FAILED TEST SUMMARY"), "{s}");
        assert!(
            s.contains("title of dir/f.phpt [dir/f.phpt]  -- line 3"),
            "{s}"
        );
        assert!(s.contains("BORKED TEST SUMMARY"), "{s}");
        assert!(
            s.contains("missing section --FILE-- [/r/dir/b.phpt]"),
            "{s}"
        );
        assert!(!s.contains("p.phpt"));
        assert_eq!(failed_summary(&[res("p", Status::Pass)]), "");
    }

    #[test]
    fn result_lines() {
        let mut r = res("a/b.phpt", Status::Pass);
        assert_eq!(result_line(&r, false), "PASS title of a/b.phpt [a/b.phpt]");
        r.status = Status::Skip;
        r.info = "reason: no ext".into();
        assert_eq!(
            result_line(&r, false),
            "SKIP title of a/b.phpt [a/b.phpt] reason: no ext"
        );
        r.status = Status::Bork;
        r.reason = "bad".into();
        r.info.clear();
        assert_eq!(result_line(&r, false), "BORK bad [a/b.phpt]");
        r.status = Status::Pass;
        assert!(result_line(&r, true).starts_with("\x1b[1;32mPASS\x1b[0m"));
        r.status = Status::Fail;
        assert!(result_line(&r, true).starts_with("\x1b[1;31mFAIL\x1b[0m"));
    }

    #[test]
    fn dir_summary_groups_by_directory() {
        let rs = vec![
            res("Zend/tests/a.phpt", Status::Pass),
            res("Zend/tests/b.phpt", Status::Fail),
            res("Zend/tests/c.phpt", Status::Skip),
            res("ext/json/tests/d.phpt", Status::Pass),
        ];
        let s = dir_summary(&rs);
        let zend = s.lines().find(|l| l.starts_with("Zend/tests")).unwrap();
        let cols: Vec<&str> = zend.split_whitespace().collect();
        assert_eq!(&cols[1..6], &["3", "1", "1", "1", "0"]);
        assert_eq!(cols[6], "50.0%");
        assert!(s.contains("ext/json/tests"));
        assert!(s.lines().next().unwrap().starts_with("Directory"));
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(
            json_string("a\"b\\c\n\t\r\u{1}é"),
            "\"a\\\"b\\\\c\\n\\t\\r\\u0001é\""
        );
    }

    #[test]
    fn json_report_shape() {
        let mut r = res("a.phpt", Status::Fail);
        r.reason = "oops \"x\"".into();
        r.duration = Duration::from_millis(12);
        let j = json_report(
            "/bin/php",
            &[r, res("b.phpt", Status::Pass)],
            Duration::from_secs(2),
        );
        assert!(j.contains("\"php\": \"/bin/php\""), "{j}");
        assert!(j.contains("\"total\": 2"), "{j}");
        assert!(j.contains("\"FAIL\": 1"), "{j}");
        assert!(
            j.contains("{\"path\": \"a.phpt\", \"status\": \"FAIL\", \"reason\": \"oops \\\"x\\\"\", \"duration_ms\": 12}"),
            "{j}"
        );
        assert!(j.trim_end().ends_with('}'));
        assert!(json_report("p", &[], Duration::ZERO).contains("\"tests\": []"));
    }

    #[test]
    fn parse_list_handles_comments_and_blanks() {
        let text = "# header\n\n  a/b.phpt  \nc/d.phpt # trailing note\n#x/y.phpt\n";
        assert_eq!(parse_list(text), vec!["a/b.phpt", "c/d.phpt"]);
    }

    #[test]
    fn update_list_unions_and_sorts() {
        let existing = "# keep me\n\nb.phpt\nz.phpt\ngone.phpt\nfailing.phpt\n";
        let results = vec![
            res("c.phpt", Status::Pass),
            res("a.phpt", Status::Pass),
            res("failing.phpt", Status::Fail),
            res("skipped.phpt", Status::Skip),
        ];
        let u = update_list(Some(existing), &results, |p| p != "gone.phpt");
        // b and z were not run and exist: kept. gone vanished. failing regressed.
        assert_eq!(u.text, "# keep me\n\na.phpt\nb.phpt\nc.phpt\nz.phpt\n");
        assert_eq!(u.added, vec!["a.phpt", "c.phpt"]);
        assert_eq!(u.removed, vec!["failing.phpt", "gone.phpt"]);
        assert_eq!(u.total, 4);
    }

    #[test]
    fn update_list_creates_header_for_new_file() {
        let u = update_list(None, &[res("a.phpt", Status::Pass)], |_| true);
        assert!(u.text.starts_with("# Tests that pass"));
        assert!(u.text.ends_with("a.phpt\n"));
        assert_eq!(parse_list(&u.text), vec!["a.phpt"]);
    }

    #[test]
    fn warn_and_xpass_do_not_count_as_passing() {
        let u = update_list(
            None,
            &[res("w.phpt", Status::Warn), res("x.phpt", Status::XPass)],
            |_| true,
        );
        assert_eq!(u.total, 0);
    }
}
