//! Runs the ext/date `.phpt` files through [`super::minphp`].

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::minphp::run_phpt;
use super::support::{expectf_match, read_phpt, section};

/// Runs one phpt; `Err` describes the mismatch.
pub fn check(name: &str) -> Result<(), String> {
    let file = read_phpt(name);
    let mut p = run_phpt(name);
    let got = p.out();
    let got_t = got.trim_end().replace("\r\n", "\n");
    if let Some(e) = section(&file, "EXPECT") {
        let e = e.trim_end().replace("\r\n", "\n");
        if got_t == e {
            return Ok(());
        }
        return Err(format!("--- got ---\n{got}\n--- expected ---\n{e}"));
    }
    if let Some(e) = section(&file, "EXPECTF") {
        if expectf_match(e.trim_end().as_bytes(), got_t.as_bytes()) {
            return Ok(());
        }
        return Err(format!(
            "--- got ---\n{got}\n--- expected (EXPECTF) ---\n{e}"
        ));
    }
    Err("no EXPECT/EXPECTF section".to_string())
}

/// Every phpt (run with `cargo test -p php-ext date::tests::phpt::all -- --ignored --nocapture`):
/// prints the failures and a summary.
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
    let (mut pass, mut fail) = (Vec::new(), Vec::new());
    for n in &names {
        if let Some(f) = &filter
            && !n.contains(f.as_str())
        {
            continue;
        }
        let r = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn({
                let n = n.clone();
                move || catch_unwind(AssertUnwindSafe(|| check(&n)))
            })
            .unwrap()
            .join()
            .unwrap();
        match r {
            Ok(Ok(())) => pass.push(n.clone()),
            Ok(Err(diff)) => {
                if verbose {
                    println!("=== FAIL {n}\n{diff}");
                }
                fail.push(n.clone());
            }
            Err(_) => {
                println!("=== PANIC {n}");
                fail.push(n.clone());
            }
        }
    }
    println!("PASS {} FAIL {}", pass.len(), fail.len());
    println!("FAILING: {}", fail.join(" "));
    if let Ok(path) = std::env::var("PHPT_PASS_FILE") {
        std::fs::write(path, pass.join("\n")).unwrap();
    }
}
