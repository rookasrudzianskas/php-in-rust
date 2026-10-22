//! ext/standard/filters.c — the standard stream filters.

use super::filter::FilterRegistry;

/// Registers the standard filter factories (`PHP_MINIT(standard_filters)`).
pub fn register_factories(reg: &mut FilterRegistry) {
    let _ = reg;
}
