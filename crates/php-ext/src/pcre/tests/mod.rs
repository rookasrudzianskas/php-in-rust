//! Unit tests for ext/pcre.
//!
//! * `ported_a` .. `ported_d` transliterate the scripts of php-src's
//!   `ext/pcre/tests/*.phpt` (adopted under `tests/phpt/ext/pcre/tests/`) and
//!   compare the output with the expected output read from the same phpt
//!   file ([`phpt`]); `support` and `lit` hold the helpers (a PHP session
//!   around `TestVm`, `var_dump`/`print_r`/`var_export`, PHP string literals,
//!   closure-like native callbacks). Each file lists the tests it does not
//!   port and why.
//! * `basic`, `errors` and `semantics` check behaviour of php_pcre.c that the
//!   phpt files only touch lightly.

mod lit;
mod phpt;
mod support;

mod basic;
mod errors;
mod semantics;

mod ported_a;
mod ported_b;
mod ported_c;
mod ported_d;
