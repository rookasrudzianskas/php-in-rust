//! `zend_object_iterator` for internal classes and the `InternalIterator`
//! class of `Zend/zend_interfaces.c`.
//!
//! Both live in the runtime (`php_runtime::interp::internal_iterator`, one
//! class and one state type); this module re-exports them for the
//! extensions. [`ObjectIterator`] is the Rust form of
//! `zend_object_iterator_funcs`; [`create_internal_iterator`] wraps one in
//! an `InternalIterator` object (`SplFixedArray::getIterator()`,
//! `DatePeriod::getIterator()`). A class whose `get_iterator` handler
//! returns `iterator_any(iter)` is iterated natively by `foreach`.

use php_runtime::Registry;

pub use php_runtime::class::ObjectIterator;
pub use php_runtime::interp::internal_iterator::{
    INTERNAL_ITERATOR, by_ref_error, create_internal_iterator, iterator_any, iterator_from_any,
};

/// Makes sure the runtime's `InternalIterator` class is registered (the
/// interpreter registers it in Core; bare test VMs get it here).
pub fn register(r: &mut Registry) {
    php_runtime::interp::internal_iterator::register(r);
}
