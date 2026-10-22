//! ext/pdo — PHP Data Objects (`pdo.c`, `pdo_dbh.c`, `pdo_stmt.c`,
//! `pdo_sql_parser.re`, `pdo_sqlstate.c`) and the bundled ext/pdo_sqlite
//! driver ([`sqlite`]).
//!
//! * [`dbh`] — `PDO` (connections, attributes, transactions, `prepare`,
//!   `query`, `exec`, `quote`, `connect`).
//! * [`stmt`] — `PDOStatement` (binding, execution, every fetch mode).
//! * [`row`] — `PDORow` (`PDO::FETCH_LAZY`); [`iterator`] — the
//!   `InternalIterator` returned by `PDOStatement::getIterator()`.
//! * [`parser`] — placeholder scanning/rewriting for emulated prepares.
//! * [`driver`] — the driver interface; drivers are listed in [`DRIVERS`].

// Nested `if let` mirrors the C control flow (same choice as main/streams).
#![allow(clippy::collapsible_if)]

pub mod dbh;
pub mod driver;
pub mod error;
pub mod iterator;
pub mod parser;
pub mod props;
pub mod row;
pub mod sqlite;
pub mod sqlstate;
pub mod stmt;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::rc::Rc;

use php_runtime::{Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, Registry, Value, Vm};

use dbh::Dbh;
use driver::Driver;

/// `php_pdo_driver.h` constants.
pub mod consts {
    pub const PARAM_NULL: i64 = 0;
    pub const PARAM_INT: i64 = 1;
    pub const PARAM_STR: i64 = 2;
    pub const PARAM_LOB: i64 = 3;
    pub const PARAM_STMT: i64 = 4;
    pub const PARAM_BOOL: i64 = 5;
    pub const PARAM_INPUT_OUTPUT: i64 = 0x8000_0000;
    pub const PARAM_STR_NATL: i64 = 0x4000_0000;
    pub const PARAM_STR_CHAR: i64 = 0x2000_0000;
    /// `PDO_PARAM_FLAGS`.
    pub const PARAM_FLAGS: u32 = 0xFFFF_0000;

    pub const FETCH_USE_DEFAULT: i64 = 0;
    pub const FETCH_LAZY: i64 = 1;
    pub const FETCH_ASSOC: i64 = 2;
    pub const FETCH_NUM: i64 = 3;
    pub const FETCH_BOTH: i64 = 4;
    pub const FETCH_OBJ: i64 = 5;
    pub const FETCH_BOUND: i64 = 6;
    pub const FETCH_COLUMN: i64 = 7;
    pub const FETCH_CLASS: i64 = 8;
    pub const FETCH_INTO: i64 = 9;
    pub const FETCH_FUNC: i64 = 10;
    pub const FETCH_NAMED: i64 = 11;
    pub const FETCH_KEY_PAIR: i64 = 12;
    /// `PDO_FETCH_FLAGS`.
    pub const FETCH_FLAGS: i64 = 0xFFFF_FFF0;
    pub const FETCH_GROUP: i64 = 1 << 5;
    pub const FETCH_UNIQUE: i64 = 1 << 6;
    pub const FETCH_CLASSTYPE: i64 = 1 << 7;
    pub const FETCH_PROPS_LATE: i64 = 1 << 8;
    pub const FETCH_SERIALIZE: i64 = 1 << 9;
    pub const FIRST_INVALID_FLAG: i64 = 1 << 10;

    pub const FETCH_ORI_NEXT: i64 = 0;

    pub const ATTR_AUTOCOMMIT: i64 = 0;
    pub const ATTR_PREFETCH: i64 = 1;
    pub const ATTR_TIMEOUT: i64 = 2;
    pub const ATTR_ERRMODE: i64 = 3;
    pub const ATTR_SERVER_VERSION: i64 = 4;
    pub const ATTR_CLIENT_VERSION: i64 = 5;
    pub const ATTR_SERVER_INFO: i64 = 6;
    pub const ATTR_CONNECTION_STATUS: i64 = 7;
    pub const ATTR_CASE: i64 = 8;
    pub const ATTR_CURSOR_NAME: i64 = 9;
    pub const ATTR_CURSOR: i64 = 10;
    pub const ATTR_ORACLE_NULLS: i64 = 11;
    pub const ATTR_PERSISTENT: i64 = 12;
    pub const ATTR_STATEMENT_CLASS: i64 = 13;
    pub const ATTR_FETCH_TABLE_NAMES: i64 = 14;
    pub const ATTR_FETCH_CATALOG_NAMES: i64 = 15;
    pub const ATTR_DRIVER_NAME: i64 = 16;
    pub const ATTR_STRINGIFY_FETCHES: i64 = 17;
    pub const ATTR_MAX_COLUMN_LEN: i64 = 18;
    pub const ATTR_DEFAULT_FETCH_MODE: i64 = 19;
    pub const ATTR_EMULATE_PREPARES: i64 = 20;
    pub const ATTR_DEFAULT_STR_PARAM: i64 = 21;
    /// `PDO_ATTR_DRIVER_SPECIFIC`.
    pub const ATTR_DRIVER_SPECIFIC: i64 = 1000;

    pub const CURSOR_FWDONLY: i64 = 0;
}

/// The bundled drivers (`pdo_driver_hash`), in registration order.
pub static DRIVERS: &[&dyn Driver] = &[&sqlite::DRIVER];

/// Per-VM PDO state.
#[derive(Default)]
pub struct PdoGlobals {
    /// `EG(persistent_list)` entries of persistent handles.
    pub persistent: HashMap<Vec<u8>, Rc<Dbh>>,
    /// The `InternalIterator` class used by `PDOStatement::getIterator()`.
    pub iterator_class: Option<ClassRef>,
}

/// A registered class (classes of this module always exist).
pub fn find_class(vm: &mut Vm, name: &str) -> ClassRef {
    vm.find_class(name.as_bytes())
        .unwrap_or_else(|| panic!("class {name} is registered"))
}

/// Completes a PDO method: an exception thrown by a user callback while the
/// driver ran (pending in php-src's `EG(exception)`) is rethrown, chained
/// under any exception the method threw itself.
pub fn finish(vm: &mut Vm, dbh: Option<&Rc<Dbh>>, r: PResult<Value>) -> PResult<Value> {
    let _ = vm;
    let pending = dbh
        .and_then(|d| d.methods())
        .and_then(|m| m.take_exception());
    match (r, pending) {
        (r, None) => r,
        (Ok(_), Some(p)) => Err(p),
        (Err(e), Some(p)) => Err(error::chain(e, p)),
    }
}

/// "Call to undefined method Class::name()" for driver-specific methods
/// called on a handle of another driver.
pub fn undefined_method(vm: &mut Vm, obj: &ObjRef, name: &str) -> Flow {
    vm.throw_error(
        "Error",
        format!("Call to undefined method {}::{name}()", obj.class().name),
    )
}

fn pdo_drivers(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    dbh::available_drivers(vm, args)
}

static PDO_DRIVERS: NativeFunction = NativeFunction {
    sig: "function pdo_drivers(): array {}",
    handler: pdo_drivers,
};

pub fn register(r: &mut Registry) {
    r.function(&PDO_DRIVERS);
    error::register(r);
    dbh::register(r);
    stmt::register(r);
    row::register(r);
    iterator::register(r);
    let mut sqlite_module = Registry::new(&mut *r.vm, "pdo_sqlite");
    sqlite::register(&mut sqlite_module);
}
