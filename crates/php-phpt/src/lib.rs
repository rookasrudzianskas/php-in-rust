//! `php-run-tests`: a Rust port of php-src's `run-tests.php`.
//!
//! The crate executes `.phpt` conformance tests against a PHP binary and
//! reports PASS / FAIL / SKIP / XFAIL / BORK / TIMEOUT per test.  It mirrors
//! the semantics of the upstream runner (section parsing, `--EXPECTF--`
//! translation, output normalisation, SKIPIF/CLEAN handling, environment and
//! INI setup) and adds a few conveniences for regression tracking
//! (`--passing-list`, `--update-passing`, `--json`, `--summary-by-dir`).
//!
//! Module overview:
//!
//! * [`phpt`] - `.phpt` section parser (`TestFile`)
//! * [`expect`] - output normalisation, `EXPECTF`/`EXPECTREGEX` to regex
//! * [`diff`] - line diff in the format run-tests writes to `.diff`
//! * [`ini`] / [`shell`] - `-d` setting bookkeeping and shell quoting helpers
//! * [`exec`] - child process execution with timeouts
//! * [`runner`] - executes one test (the port of `run_test()`)
//! * [`sched`] - parallel scheduling honouring `CONFLICTS`
//! * [`report`] - statuses, summaries, JSON output, passing lists
//! * [`cli`] - argument parsing and orchestration

pub mod cli;
pub mod diff;
pub mod exec;
pub mod expect;
pub mod ini;
pub mod phpt;
pub mod report;
pub mod runner;
pub mod sched;
pub mod shell;
