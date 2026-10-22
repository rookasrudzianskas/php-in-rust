//! Runs the ext/date `.phpt` files through [`super::minphp`], with
//! run-tests' semantics: `--SKIPIF--` output starting with `skip` skips the
//! test, output and expectation are trimmed, `--EXPECTF--` placeholders.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::minphp::{run_phpt, run_skipif};
use super::support::{expectf_match, read_phpt, section};

/// The outcome of one phpt.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Skip(String),
    Fail(String),
}

/// Runs one phpt.
pub fn check(name: &str) -> Outcome {
    if let Some(out) = run_skipif(name) {
        let t = out.trim_start();
        if t.len() >= 4 && t[..4].eq_ignore_ascii_case("skip") {
            return Outcome::Skip(t.to_string());
        }
    }
    let file = read_phpt(name);
    let mut p = run_phpt(name);
    let got = p.out();
    let got_t = got.replace("\r\n", "\n").trim().to_string();
    if let Some(e) = section(&file, "EXPECT") {
        let e = e.replace("\r\n", "\n").trim().to_string();
        if got_t == e {
            return Outcome::Pass;
        }
        return Outcome::Fail(format!("--- got ---\n{got}\n--- expected ---\n{e}"));
    }
    if let Some(e) = section(&file, "EXPECTF") {
        let e = e.replace("\r\n", "\n");
        if expectf_match(e.trim().as_bytes(), got_t.as_bytes()) {
            return Outcome::Pass;
        }
        return Outcome::Fail(format!(
            "--- got ---\n{got}\n--- expected (EXPECTF) ---\n{e}"
        ));
    }
    if let Some(e) = section(&file, "EXPECTREGEX") {
        let re = format!("^{}$", e.replace("\r\n", "\n").trim());
        let ok = php_regex::Regex::new(re.as_bytes(), php_regex::CompileFlags::DOTALL)
            .is_ok_and(|r| r.is_match(got_t.as_bytes()));
        if ok {
            return Outcome::Pass;
        }
        return Outcome::Fail(format!(
            "--- got ---\n{got}\n--- expected (EXPECTREGEX) ---\n{e}"
        ));
    }
    Outcome::Fail("no EXPECT/EXPECTF/EXPECTREGEX section".to_string())
}

/// Runs `name` on a thread with a large stack, catching panics.
pub fn check_isolated(name: &str) -> Outcome {
    let n = name.to_string();
    let r = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || catch_unwind(AssertUnwindSafe(|| check(&n))))
        .unwrap()
        .join()
        .unwrap();
    match r {
        Ok(o) => o,
        Err(e) => {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            Outcome::Fail(format!("panic: {msg}"))
        }
    }
}

/// The body of the generated per-phpt tests: passes or skips (`--SKIPIF--`).
pub fn assert_phpt(name: &str) {
    match check_isolated(name) {
        Outcome::Pass | Outcome::Skip(_) => {}
        Outcome::Fail(diff) => panic!("{name}.phpt failed\n{diff}"),
    }
}

/// Every phpt (run with `cargo test -p php-ext date::tests::phpt::all -- --ignored --nocapture`;
/// `PHPT=<substring>` filters, `PHPT_VERBOSE=1` prints the diffs): prints the
/// failures and a summary.
#[test]
#[ignore]
fn all() {
    let dir = format!(
        "{}/../../tests/phpt/ext/date/tests",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.ok()?.file_name().to_string_lossy().into_owned();
            n.strip_suffix(".phpt").map(str::to_string)
        })
        .collect();
    names.sort();
    let filter = std::env::var("PHPT").ok();
    let verbose = std::env::var("PHPT_VERBOSE").is_ok();
    let (mut pass, mut fail, mut skip) = (Vec::new(), Vec::new(), Vec::new());
    for n in &names {
        if let Some(f) = &filter
            && !n.contains(f.as_str())
        {
            continue;
        }
        match check_isolated(n) {
            Outcome::Pass => pass.push(n.clone()),
            Outcome::Skip(_) => skip.push(n.clone()),
            Outcome::Fail(diff) => {
                if verbose {
                    println!("=== FAIL {n}\n{diff}");
                }
                fail.push(n.clone());
            }
        }
    }
    println!(
        "PASS {} FAIL {} SKIP {}",
        pass.len(),
        fail.len(),
        skip.len()
    );
    println!("FAILING: {}", fail.join(" "));
    println!("SKIPPED: {}", skip.join(" "));
}
