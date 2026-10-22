//! Regression gate: every `.phpt` listed in `tests/phpt/PASSING` must keep
//! passing.
//!
//! The test drives the already-built `php` and `php-run-tests` binaries
//! (`cargo build --release -p php-cli -p php-phpt`, or the debug profile).
//! Building them from inside a test would take minutes and fight over the
//! target-dir lock, so when either binary is missing the test prints a note
//! and passes instead of failing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/php-cli lives two levels below the workspace root")
        .to_path_buf()
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Finds a profile directory holding both binaries, preferring release.
fn find_binaries(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut target_dirs = Vec::new();
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let dir = PathBuf::from(dir);
        target_dirs.push(if dir.is_absolute() { dir } else { root.join(dir) });
    }
    target_dirs.push(root.join("target"));
    for target in target_dirs {
        for profile in ["release", "debug"] {
            let php = target.join(profile).join(exe("php"));
            let runner = target.join(profile).join(exe("php-run-tests"));
            if php.is_file() && runner.is_file() {
                return Some((php, runner));
            }
        }
    }
    None
}

#[test]
fn phpt_passing_list_still_passes() {
    let root = workspace_root();
    let list = root.join("tests/phpt/PASSING");
    if !list.is_file() {
        eprintln!("skipping: {} does not exist", list.display());
        return;
    }
    let Some((php, runner)) = find_binaries(&root) else {
        eprintln!(
            "skipping: build the binaries first \
             (cargo build --release -p php-cli -p php-phpt)"
        );
        return;
    };
    let output = Command::new(&runner)
        .current_dir(&root)
        .arg("-p")
        .arg(&php)
        .arg("-j4")
        .arg("-q")
        .arg("--no-color")
        .arg("--passing-list")
        .arg(&list)
        .env("NO_PHPTEST_SUMMARY", "1")
        .output()
        .expect("failed to spawn php-run-tests");
    if !output.status.success() {
        panic!(
            "tests listed in tests/phpt/PASSING regressed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
