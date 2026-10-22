//! ext/standard/array.c
//!
//! The array functions, split into submodules by theme:
//!
//! * `sort`: `zend_sort`, the comparison functions and every sort function
//!   (`sort`, `usort`, `ksort`, `natsort`, `array_multisort`, ...);
//! * `unique`: `array_unique`;
//! * `iter`: the internal pointer functions and `array_walk*`;
//! * `minmax`: `min`, `max`;
//! * `access`: searching and reading (`in_array`, `array_keys`,
//!   `array_key_exists`, `array_column`, ...);
//! * `build`: constructing and reshaping arrays and the stack functions;
//! * `range`: `range()`;
//! * `merge`: `array_merge*` and `array_replace*`;
//! * `setops`: the `array_diff*` / `array_intersect*` family;
//! * `funcs`: `count`, `array_map`, `array_filter`, `array_reduce`,
//!   `array_find*`, `array_sum`, `array_product`.
//!
//! `compact()`, `extract()`, `shuffle()` and `array_rand()` live elsewhere
//! (the interpreter and the random extension).

// Let-chains are avoided to match the runtime crate's conventions.
#![allow(clippy::collapsible_if)]

use php_runtime::Registry;

pub mod access;
pub mod build;
pub mod funcs;
pub mod iter;
pub mod merge;
pub mod minmax;
pub mod range;
pub mod setops;
pub mod sort;
pub mod unique;
mod util;

#[cfg(test)]
mod tests;

/// `EXTR_*` constants (the `extract()` function itself is implemented by the
/// interpreter, which needs access to the symbol table).
const EXTR_CONSTANTS: &[(&str, i64)] = &[
    ("EXTR_OVERWRITE", 0),
    ("EXTR_SKIP", 1),
    ("EXTR_PREFIX_SAME", 2),
    ("EXTR_PREFIX_ALL", 3),
    ("EXTR_PREFIX_INVALID", 4),
    ("EXTR_PREFIX_IF_EXISTS", 5),
    ("EXTR_IF_EXISTS", 6),
    ("EXTR_REFS", 0x100),
];

pub fn register(r: &mut Registry) {
    for (name, value) in EXTR_CONSTANTS {
        r.constant(name, *value);
    }
    r.constant("SORT_ASC", sort::flags::SORT_ASC);
    r.constant("SORT_DESC", sort::flags::SORT_DESC);
    r.constant("SORT_REGULAR", sort::flags::SORT_REGULAR);
    r.constant("SORT_NUMERIC", sort::flags::SORT_NUMERIC);
    r.constant("SORT_STRING", sort::flags::SORT_STRING);
    r.deprecated_constant(
        "SORT_LOCALE_STRING",
        sort::flags::SORT_LOCALE_STRING,
        "since 8.6, use one of the Collator::*sort*() methods instead",
    );
    r.constant("SORT_NATURAL", sort::flags::SORT_NATURAL);
    r.constant("SORT_FLAG_CASE", sort::flags::SORT_FLAG_CASE);
    r.constant("CASE_LOWER", 0i64);
    r.constant("CASE_UPPER", 1i64);
    r.constant("COUNT_NORMAL", 0i64);
    r.constant("COUNT_RECURSIVE", 1i64);
    r.constant("ARRAY_FILTER_USE_VALUE", 0i64);
    r.constant("ARRAY_FILTER_USE_BOTH", 1i64);
    r.constant("ARRAY_FILTER_USE_KEY", 2i64);

    for table in [
        sort::FUNCTIONS,
        unique::FUNCTIONS,
        iter::FUNCTIONS,
        minmax::FUNCTIONS,
        access::FUNCTIONS,
        build::FUNCTIONS,
        range::FUNCTIONS,
        merge::FUNCTIONS,
        setops::FUNCTIONS,
        funcs::FUNCTIONS,
    ] {
        for f in table {
            r.function(f);
        }
    }
}
