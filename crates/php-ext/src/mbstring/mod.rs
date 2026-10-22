//! ext/mbstring

pub mod filters;
pub mod mbfl;
pub mod tables;

use php_runtime::Registry;

pub fn register(_r: &mut Registry) {}
