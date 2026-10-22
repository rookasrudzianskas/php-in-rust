//! Tests for `serialize()` / `unserialize()`, ported from
//! ext/standard/tests/serialize/*.phpt and Zend/tests (enums, Serializable).
//!
//! User classes are emulated with internal classes whose magic methods are
//! native functions; `var_dump()` output is reproduced by a small test-only
//! dumper so that the `.phpt` expectations can be compared verbatim.

mod classes;
mod harness;
mod objects;
mod scalars;
mod unserialize_errors;
