//! Unit tests ported from php-src `ext/pdo/tests` (the generic tests run
//! against `sqlite::memory:`, i.e. `PDOTEST_DSN=sqlite::memory:`) and
//! `ext/pdo_sqlite/tests`. Each test replays the PHP script with native calls
//! and compares the output with the `.phpt` expectation.

mod errors;
mod fetch;
mod harness;
mod sqlite;
mod statement;
mod subclasses;

macro_rules! phpt {
    ($path:literal) => {
        include_str!(concat!("../../../../../tests/phpt/ext/", $path))
    };
}
pub(crate) use phpt;
