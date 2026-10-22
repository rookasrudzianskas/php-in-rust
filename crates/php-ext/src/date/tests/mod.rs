//! Unit tests for ext/date, ported from `ext/date/tests/*.phpt`.
//!
//! [`minphp`] is a test-only evaluator for the subset of PHP the phpt
//! scripts use; [`phpt_list`] runs every phpt through it (one test each).

mod support;

mod minphp;
mod phpt;
mod phpt_list;

mod core;
mod smoke;
