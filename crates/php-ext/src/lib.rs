//! PHP extensions (`ext/*` in php-src), each a module with a `register` function.
//!
//! Every module registers its functions, constants, ini entries and classes
//! through [`php_runtime::Registry`]. [`register_all`] installs all of them
//! into a [`Vm`] in the order php-src's default CLI build loads them.

pub mod bcmath;
pub mod calendar;
pub mod ctype;
pub mod date;
pub mod filter;
pub mod hash;
pub mod json;
pub mod mbstring;
pub mod pcre;
pub mod pdo;
pub mod random;
pub mod reflection;
pub mod spl;
pub mod sqlite3;
pub mod standard;
pub mod tokenizer;

#[cfg(test)]
pub mod testing;

use php_runtime::{Registry, Vm};

/// An extension as reported by `php -m` and `extension_loaded()`.
pub struct Module {
    pub name: &'static str,
    pub register: fn(&mut Registry),
}

/// All bundled extensions, in php-src's module startup order.
pub const MODULES: &[Module] = &[
    Module { name: "date", register: date::register },
    Module { name: "pcre", register: pcre::register },
    Module { name: "random", register: random::register },
    Module { name: "hash", register: hash::register },
    Module { name: "json", register: json::register },
    Module { name: "Reflection", register: reflection::register },
    Module { name: "SPL", register: spl::register },
    Module { name: "standard", register: standard::register },
    Module { name: "bcmath", register: bcmath::register },
    Module { name: "calendar", register: calendar::register },
    Module { name: "ctype", register: ctype::register },
    Module { name: "filter", register: filter::register },
    Module { name: "mbstring", register: mbstring::register },
    Module { name: "PDO", register: pdo::register },
    Module { name: "sqlite3", register: sqlite3::register },
    Module { name: "tokenizer", register: tokenizer::register },
];

/// Register every bundled extension with `vm`.
pub fn register_all(vm: &mut Vm) {
    for module in MODULES {
        let mut registry = Registry::new(vm, module.name);
        (module.register)(&mut registry);
    }
}
