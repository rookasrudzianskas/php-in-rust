//! The PDO driver interface (`php_pdo_driver.h`).
//!
//! A driver is a [`Driver`] (`pdo_driver_t`): a name and a handle factory.
//! The factory installs the connection-level operations ([`DbhMethods`],
//! `struct pdo_dbh_methods`) on the [`Dbh`]; `prepare` installs the
//! statement-level operations ([`StmtMethods`], `struct pdo_stmt_methods`)
//! on the [`Stmt`]. Optional C callbacks (`NULL` function pointers) are
//! modelled by the `supports_*` queries and default method bodies.
//!
//! To add a driver: implement the three traits, add a `static` instance to
//! [`super::DRIVERS`] and register its classes/constants from
//! [`super::register`] (see `sqlite/` for the SQLite driver).

use std::any::Any;
use std::rc::Rc;

use php_runtime::{Flow, NativeFunction, PResult, PhpArray, PhpArrayRef, PhpStr, Value, Vm};

use super::dbh::Dbh;
use super::stmt::{BoundParam, ColumnData, Stmt};

/// `pdo_driver_t`.
pub trait Driver: Sync {
    /// The DSN prefix (`"sqlite"`).
    fn name(&self) -> &'static str;

    /// `db_handle_factory`: opens the connection and installs the handle
    /// methods on `dbh` (also on failure, like php-src). `Ok(false)` reports a
    /// failure without exception ("Constructor failed").
    fn handle_factory(
        &self,
        vm: &mut Vm,
        dbh: &Rc<Dbh>,
        options: Option<&PhpArrayRef>,
    ) -> PResult<bool>;

    /// The driver-specific PDO subclass (`php_pdo_register_driver_specific_ce`).
    fn specific_class(&self) -> Option<&'static str> {
        None
    }

    /// Driver-specific `PDO` methods (`get_driver_methods(dbh,
    /// PDO_DBH_DRIVER_METHOD_KIND_DBH)`). The runtime has no `get_method`
    /// object handler, so they are declared on `PDO` itself; each handler
    /// must reject handles of other drivers with [`super::undefined_method`].
    fn pdo_methods(&self) -> &'static [&'static NativeFunction] {
        &[]
    }

    /// Deprecated `PDO::` class constants owned by the driver:
    /// `(name, value, deprecation message)`.
    fn pdo_constants(&self) -> &'static [(&'static str, i64, &'static str)] {
        &[]
    }
}

/// Result of [`DbhMethods::get_attribute`] (`pdo_dbh_get_attr_func`).
pub enum AttrValue {
    /// `-1`: error while retrieving a supported attribute.
    Error,
    /// `0`: the attribute is not supported by the driver.
    Unsupported,
    /// The value.
    Value(Value),
}

/// A SQL scanner for placeholder parsing (`pdo_dbh_sql_scanner`).
pub type Scanner = fn(&[u8], usize) -> super::parser::Token;

/// `struct pdo_dbh_methods`: connection operations of a driver.
pub trait DbhMethods: Any {
    fn as_any(&self) -> &dyn Any;

    /// `closer`.
    fn close(&self, vm: &mut Vm, dbh: &Dbh) {
        let _ = (vm, dbh);
    }

    /// `preparer`: prepares `sql` and installs the statement methods on
    /// `stmt`. Returns `false` after setting the error code.
    fn prepare(
        &self,
        vm: &mut Vm,
        dbh: &Rc<Dbh>,
        sql: &PhpStr,
        stmt: &Stmt,
        options: Option<&PhpArrayRef>,
    ) -> PResult<bool>;

    /// `doer`: runs a statement without result set; `-1` on failure.
    fn exec(&self, vm: &mut Vm, dbh: &Dbh, sql: &PhpStr) -> PResult<i64>;

    /// Whether `quoter` is implemented.
    fn supports_quote(&self) -> bool {
        false
    }

    /// `quoter`: `None` on failure.
    fn quote(
        &self,
        vm: &mut Vm,
        dbh: &Dbh,
        unquoted: &PhpStr,
        param_type: i64,
    ) -> PResult<Option<PhpStr>> {
        let _ = (vm, dbh, unquoted, param_type);
        Ok(None)
    }

    /// Whether `begin`/`commit`/`rollback` are implemented.
    fn supports_transactions(&self) -> bool {
        false
    }

    fn begin(&self, vm: &mut Vm, dbh: &Dbh) -> PResult<bool> {
        let _ = (vm, dbh);
        Ok(false)
    }

    fn commit(&self, vm: &mut Vm, dbh: &Dbh) -> PResult<bool> {
        let _ = (vm, dbh);
        Ok(false)
    }

    fn rollback(&self, vm: &mut Vm, dbh: &Dbh) -> PResult<bool> {
        let _ = (vm, dbh);
        Ok(false)
    }

    /// Whether `set_attribute` is implemented.
    fn supports_set_attribute(&self) -> bool {
        false
    }

    /// `set_attribute`.
    fn set_attribute(&self, vm: &mut Vm, dbh: &Dbh, attr: i64, value: &Value) -> PResult<bool> {
        let _ = (vm, dbh, attr, value);
        Ok(false)
    }

    /// Whether `last_id` is implemented.
    fn supports_last_id(&self) -> bool {
        false
    }

    /// `last_id`: `None` indicates an error.
    fn last_insert_id(
        &self,
        vm: &mut Vm,
        dbh: &Dbh,
        name: Option<&PhpStr>,
    ) -> PResult<Option<PhpStr>> {
        let _ = (vm, dbh, name);
        Ok(None)
    }

    /// Whether `fetch_err` is implemented.
    fn supports_fetch_err(&self) -> bool {
        false
    }

    /// `fetch_err`: appends the native error code and message to `info`.
    fn fetch_err(&self, dbh: &Dbh, stmt: Option<&Stmt>, info: &mut PhpArray) {
        let _ = (dbh, stmt, info);
    }

    /// Whether `get_attribute` is implemented.
    fn supports_get_attribute(&self) -> bool {
        false
    }

    /// `get_attribute`.
    fn get_attribute(&self, vm: &mut Vm, dbh: &Dbh, attr: i64) -> PResult<AttrValue> {
        let _ = (vm, dbh, attr);
        Ok(AttrValue::Unsupported)
    }

    /// `in_transaction`; `None` lets PDO use its own tracking (`in_txn`).
    fn in_transaction(&self, dbh: &Dbh) -> Option<bool> {
        let _ = dbh;
        None
    }

    /// `check_liveness` for persistent handles.
    fn check_liveness(&self, dbh: &Dbh) -> bool {
        let _ = dbh;
        true
    }

    /// `persistent_shutdown`.
    fn persistent_shutdown(&self, dbh: &Dbh) {
        let _ = dbh;
    }

    /// `scanner`: the driver's SQL scanner for placeholder parsing.
    fn scanner(&self) -> Option<Scanner> {
        None
    }

    /// Whether a user callback run by the driver threw an exception that is
    /// still pending (php-src's `EG(exception)` during driver calls).
    fn has_exception(&self) -> bool {
        false
    }

    /// Takes the pending callback exception (see [`DbhMethods::has_exception`]).
    fn take_exception(&self) -> Option<Flow> {
        None
    }
}

/// Parameter events passed to [`StmtMethods::param_hook`] (`enum pdo_param_event`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ParamEvent {
    Alloc = 0,
    Free = 1,
    ExecPre = 2,
    ExecPost = 3,
    FetchPre = 4,
    FetchPost = 5,
    Normalize = 6,
}

/// `struct pdo_stmt_methods`: statement operations of a driver.
pub trait StmtMethods: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// `executer`.
    fn execute(&mut self, vm: &mut Vm, stmt: &Stmt) -> PResult<bool>;

    /// `fetcher`: advances to the next row.
    fn fetch(&mut self, vm: &mut Vm, stmt: &Stmt, ori: i64, offset: i64) -> PResult<bool>;

    /// `describer`: column metadata (`None` on failure).
    fn describe(&mut self, vm: &mut Vm, stmt: &Stmt, colno: i32) -> PResult<Option<ColumnData>>;

    /// `get_col`: the value of column `colno` of the current row (`Null` on
    /// failure). `ty` is the type requested through `bindColumn()`.
    fn get_col(&mut self, vm: &mut Vm, stmt: &Stmt, colno: i32, ty: Option<i32>) -> PResult<Value>;

    /// Whether `param_hook` is implemented.
    fn supports_param_hook(&self) -> bool {
        false
    }

    /// `param_hook`.
    fn param_hook(
        &mut self,
        vm: &mut Vm,
        stmt: &Stmt,
        param: &mut BoundParam,
        event: ParamEvent,
    ) -> PResult<bool> {
        let _ = (vm, stmt, param, event);
        Ok(true)
    }

    /// Whether `set_attribute` is implemented.
    fn supports_set_attribute(&self) -> bool {
        false
    }

    fn set_attribute(
        &mut self,
        vm: &mut Vm,
        stmt: &Stmt,
        attr: i64,
        value: &Value,
    ) -> PResult<bool> {
        let _ = (vm, stmt, attr, value);
        Ok(false)
    }

    /// Whether `get_attribute` is implemented.
    fn supports_get_attribute(&self) -> bool {
        false
    }

    fn get_attribute(&mut self, vm: &mut Vm, stmt: &Stmt, attr: i64) -> PResult<AttrValue> {
        let _ = (vm, stmt, attr);
        Ok(AttrValue::Unsupported)
    }

    /// Whether `get_column_meta` is implemented.
    fn supports_column_meta(&self) -> bool {
        false
    }

    /// `get_column_meta`: `None` is FAILURE.
    fn column_meta(&mut self, vm: &mut Vm, stmt: &Stmt, colno: i64) -> PResult<Option<PhpArray>> {
        let _ = (vm, stmt, colno);
        Ok(None)
    }

    /// Whether `next_rowset` is implemented.
    fn supports_next_rowset(&self) -> bool {
        false
    }

    fn next_rowset(&mut self, vm: &mut Vm, stmt: &Stmt) -> PResult<bool> {
        let _ = (vm, stmt);
        Ok(false)
    }

    /// Whether `cursor_closer` is implemented.
    fn supports_cursor_closer(&self) -> bool {
        false
    }

    fn close_cursor(&mut self, vm: &mut Vm, stmt: &Stmt) -> PResult<bool> {
        let _ = (vm, stmt);
        Ok(true)
    }
}
