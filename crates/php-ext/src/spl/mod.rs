//! ext/spl — the Standard PHP Library.
//!
//! One module per php-src source file:
//!
//! * [`functions`] — `php_spl.c`: `class_implements()`, `spl_object_id()`,
//!   the autoloader stack (`spl_autoload_register()`, ...).
//! * [`exceptions`] — `spl_exceptions.c`.
//! * [`iterators`] — `spl_iterators.c`: the iterator interfaces and classes,
//!   `iterator_to_array()`, `iterator_count()`, `iterator_apply()`.
//! * [`array`] — `spl_array.c` (`ArrayObject`, `ArrayIterator`, ...).
//! * [`directory`] — `spl_directory.c` (`SplFileInfo`, `SplFileObject`, ...).
//! * [`dllist`] — `spl_dllist.c` (`SplDoublyLinkedList`, `SplQueue`, `SplStack`).
//! * [`heap`] — `spl_heap.c` (`SplHeap`, `SplPriorityQueue`, ...).
//! * [`fixedarray`] — `spl_fixedarray.c` (`SplFixedArray`).
//! * [`observer`] — `spl_observer.c` (`SplObjectStorage`, `MultipleIterator`, ...).
//! * [`engine`] — shared helpers (`spl_functions.c`, class building, the
//!   `zend_object_iterator` glue of `zend_interfaces.c`).
//!
//! # Interpreter contract
//! SPL objects keep their state in [`php_runtime::ObjRef::native`] and are
//! created by their class's `create_object` handler, which user subclasses
//! inherit. Every call php-src makes through a possibly overridden method
//! goes through [`php_runtime::Vm::call_method`], and Traversables are
//! walked through their `Iterator` / `IteratorAggregate` methods
//! ([`engine::ObjIterator`]). The hooks the interpreter must call are
//! listed in [`functions`] (autoloading, `spl_autoload()` includes,
//! `class_uses()` traits) and in the documentation of each class module.

pub mod array;
pub mod directory;
pub mod dllist;
pub mod engine;
pub mod exceptions;
pub mod fixedarray;
pub mod functions;
pub mod heap;
pub mod iterators;
pub mod observer;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;

use php_runtime::Registry;

/// `PHP_MINIT_FUNCTION(spl)`: classes in php-src's registration order, then
/// the functions of `php_spl.c`.
pub fn register(r: &mut Registry) {
    exceptions::register(r);
    iterators::register(r);
    array::register(r);
    directory::register(r);
    dllist::register(r);
    heap::register(r);
    fixedarray::register(r);
    observer::register(r);
    functions::register(r);
}
