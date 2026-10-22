//! ext/spl/spl_observer.c — the `SplObserver` and `SplSubject` interfaces,
//! `SplObjectStorage` ([`object_storage`]) and `MultipleIterator`
//! ([`multiple_iterator`]), which share one native state ([`storage`]).
//!
//! # Interpreter contract
//! Object handlers installed on both classes (inherited by user subclasses):
//! * `create_object` — allocates the storage state; a subclass overriding
//!   `getHash()` gets string keys, and every `SplObjectStorage` subclass
//!   routes `$s[...]` through its `offset*()` methods (php-src's
//!   `SOS_OVERRIDDEN_*` bits).
//! * `read_dimension(obj, offset, fetch)` — `$s[$o]`: returns the info
//!   (never a reference: for `DimFetch::Write`/`ReadWrite` the interpreter
//!   must report "Indirect modification of overloaded element of %s has no
//!   effect"); with `DimFetch::IsSet` (`??`) a missing object yields `null`
//!   instead of the "Object not found" `UnexpectedValueException`.
//! * `write_dimension(obj, offset, value)` — `$s[$o] = $v` (`offset` is
//!   `None` for `$s[] = $v`).
//! * `has_dimension(obj, offset, check_empty)` — `isset($s[$o])` with
//!   `check_empty == false`; for `empty($s[$o])` it is called with `true`
//!   and returns whether the info is *truthy* (php-src's convention; the
//!   interpreter negates it).
//! * `unset_dimension(obj, offset)` — `unset($s[$o])`.
//! * `compare(a, b)` — `==`/`<=>`; `1` also means "uncomparable".
//! * `clone_obj(obj)` — returns the new object with storage and properties
//!   copied; the interpreter then calls `__clone()`.
//! * `count_elements(obj)` (`SplObjectStorage` only) — always `Some(n)`;
//!   a user `count()` override is called by the handler.
//!
//! `var_dump()` reaches the storage through the `__debugInfo()` methods
//! (php-src's standard `get_debug_info` handler calls them, so user
//! overrides win); no `get_debug_info` handler is installed. `foreach` uses
//! the `Iterator` methods.
//!
//! `getHash()` calls are counted per VM: modifying any storage while a
//! user `getHash()` runs throws "Modification of SplObjectStorage during
//! getHash() is prohibited". The counter is decremented on every return
//! path, so a fatal error cannot leave it stuck; the interpreter should
//! still call [`reset_get_hash_depth`] at request startup (`PHP_RINIT`).
//!
//! The `#[\Deprecated]` methods `attach()`, `detach()` and `contains()`
//! carry `fn_flags::DEPRECATED` and emit their deprecation themselves (after
//! argument parsing; php-src emits it before).

pub mod multiple_iterator;
pub mod object_storage;
pub mod storage;

#[cfg(test)]
mod tests;

use php_runtime::{NativeFunction, Registry, Vm};

use super::engine::{abstract_method, interface};

pub const SPL_OBSERVER: &str = "SplObserver";
pub const SPL_SUBJECT: &str = "SplSubject";
pub const SPL_OBJECT_STORAGE: &str = "SplObjectStorage";

/// Per-VM state of spl_observer.c.
#[derive(Default)]
pub struct ObserverGlobals {
    /// `spl_object_storage_get_hash_depth`: user `getHash()` calls running.
    pub get_hash_depth: u32,
}

fn globals(vm: &mut Vm) -> &mut ObserverGlobals {
    vm.ext_data::<ObserverGlobals>()
}

/// `spl_object_storage_reset_get_hash_depth()` (called by `PHP_RINIT(spl)`).
pub fn reset_get_hash_depth(vm: &mut Vm) {
    globals(vm).get_hash_depth = 0;
}

static OBSERVER_UPDATE: NativeFunction = NativeFunction {
    sig: "public function update(SplSubject $subject): void;",
    handler: abstract_method,
};
static SUBJECT_ATTACH: NativeFunction = NativeFunction {
    sig: "public function attach(SplObserver $observer): void;",
    handler: abstract_method,
};
static SUBJECT_DETACH: NativeFunction = NativeFunction {
    sig: "public function detach(SplObserver $observer): void;",
    handler: abstract_method,
};
static SUBJECT_NOTIFY: NativeFunction = NativeFunction {
    sig: "public function notify(): void;",
    handler: abstract_method,
};

/// `PHP_MINIT_FUNCTION(spl_observer)`.
pub fn register(r: &mut Registry) {
    interface(r, SPL_OBSERVER, &[], &[&OBSERVER_UPDATE]);
    interface(
        r,
        SPL_SUBJECT,
        &[],
        &[&SUBJECT_ATTACH, &SUBJECT_DETACH, &SUBJECT_NOTIFY],
    );
    object_storage::register(r);
    multiple_iterator::register(r);
}
