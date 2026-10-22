//! ext/sqlite3 (`ext/sqlite3/sqlite3.c`): the `SQLite3`, `SQLite3Stmt`,
//! `SQLite3Result` and `SQLite3Exception` classes, the `SQLITE3_*`
//! constants, the `sqlite3.*` ini entries and the blob stream.
//!
//! Object model (`php_sqlite3_structs.h`):
//!
//! * `SQLite3` ([`db::DbState`]) owns the [`capi::Connection`] while
//!   initialised and keeps weak handles to its statements (`free_list`), so
//!   `close()` can finalize them first. Flags that callbacks need
//!   (`exception`, `in_callback`, the userland authorizer) live in a shared
//!   [`db::DbShared`].
//! * `SQLite3Stmt` ([`stmt::StmtState`]) holds a reference to its `SQLite3`
//!   object and a shared [`stmt::StmtCore`] (`sqlite3_stmt*`, `initialised`,
//!   bound parameters).
//! * `SQLite3Result` ([`result::ResultState`]) holds a reference to the
//!   statement object (the internal one created by `query()`, or the user's
//!   `SQLite3Stmt` for `execute()`).
//!
//! No `RefCell` borrow of this state is held across calls into SQLite that
//! may run PHP callbacks (they may re-enter the extension).

pub mod capi;

mod blob;
mod db;
mod result;
mod stmt;
#[cfg(test)]
mod tests;

use php_runtime::class::ClassEntry;
use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{Flow, ObjRef, PResult, PhpStr, Registry, Value, Vm};

use capi::{Connection, ffi};

/// `PHP_SQLITE3_ASSOC` / `PHP_SQLITE3_NUM` / `PHP_SQLITE3_BOTH`.
pub const PHP_SQLITE3_ASSOC: i64 = 1;
pub const PHP_SQLITE3_NUM: i64 = 2;
pub const PHP_SQLITE3_BOTH: i64 = PHP_SQLITE3_ASSOC | PHP_SQLITE3_NUM;

pub const EXCEPTION_CLASS: &str = "SQLite3Exception";

/// `SQLITE3_CHECK_INITIALIZED*`: "The X object has not been correctly
/// initialised or is already closed".
fn not_initialised(vm: &mut Vm, class: &str) -> Flow {
    vm.throw_error(
        "Error",
        format!("The {class} object has not been correctly initialised or is already closed"),
    )
}

/// `clone_obj = NULL`.
fn clone_forbidden(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            old.class().name
        ),
    ))
}

/// `zend_exception_set_previous(exception, add_previous)`: appends
/// `add_previous` at the end of `exception`'s `previous` chain unless that
/// would create a cycle.
fn set_previous(exception: &ObjRef, add_previous: ObjRef) {
    if ObjRef::ptr_eq(exception, &add_previous) {
        return;
    }
    let mut ancestor = add_previous.get_prop(b"previous");
    while let Some(Value::Object(a)) = ancestor {
        if ObjRef::ptr_eq(&a, exception) {
            return;
        }
        ancestor = a.get_prop(b"previous");
    }
    let mut ex = exception.clone();
    loop {
        match ex.get_prop(b"previous") {
            Some(Value::Object(p)) => {
                if ObjRef::ptr_eq(&p, &add_previous) {
                    return;
                }
                ex = p;
            }
            _ => {
                ex.set_prop(b"previous", Value::Object(add_previous));
                return;
            }
        }
    }
}

/// An exception thrown while `prev` is pending (`zend_throw_exception_internal`):
/// the new exception wins and gets `prev` as its previous one; an unwinding
/// `exit` (or fatal error) is never replaced.
fn chain(new: Flow, prev: Flow) -> Flow {
    match (new, prev) {
        (Flow::Exception(n), Flow::Exception(p)) => {
            set_previous(&n, p);
            Flow::Exception(n)
        }
        (new, Flow::Exception(_)) => new,
        (_, prev) => prev,
    }
}

/// `EG(exception)` for the duration of one native call.
///
/// php-src keeps running after `zend_throw_exception()` (and after a
/// callback threw), checking `EG(exception)` at some points only; an
/// exception thrown while another is pending gets it as `previous`. A
/// callback exception stays parked on the connection (so SQLite does not
/// invoke further callbacks) until it is adopted by [`Pending::throw`] or
/// [`Pending::finish`].
#[derive(Default)]
struct Pending {
    flow: Option<Flow>,
    conn: Option<Connection>,
}

impl Pending {
    /// Tracks the callback exceptions parked on `conn`.
    fn new(conn: &Connection) -> Pending {
        Pending {
            flow: None,
            conn: Some(conn.clone()),
        }
    }

    fn adopt_parked(&mut self) {
        if let Some(f) = self.conn.as_ref().and_then(Connection::take_exception) {
            self.flow = Some(match self.flow.take() {
                None => f,
                Some(prev) => chain(f, prev),
            });
        }
    }

    /// `EG(exception) != NULL`.
    fn is_set(&self) -> bool {
        self.flow.is_some() || self.conn.as_ref().is_some_and(Connection::has_exception)
    }

    /// Throws `flow` (chained to the pending exception, if any).
    fn throw(&mut self, flow: Flow) {
        self.adopt_parked();
        self.flow = Some(match self.flow.take() {
            None => flow,
            Some(prev) => chain(flow, prev),
        });
    }

    /// Keeps going after a fallible step like C code does with `EG(exception)`.
    fn check<T>(&mut self, r: PResult<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(f) => {
                self.throw(f);
                None
            }
        }
    }

    /// `php_error_docref(NULL, E_WARNING, ...)`.
    fn warning(&mut self, vm: &mut Vm, msg: impl Into<PhpStr>) {
        let r = vm.warning(msg);
        self.check(r);
    }

    /// `php_sqlite3_error()`: an `SQLite3Exception` when exceptions are
    /// enabled on the database object, a warning otherwise.
    fn error(&mut self, vm: &mut Vm, exceptions: bool, code: i64, msg: impl Into<PhpStr>) {
        if exceptions {
            let e = vm.create_exception(EXCEPTION_CLASS, msg, code);
            self.throw(Flow::Exception(e));
        } else {
            self.warning(vm, msg);
        }
    }

    /// The method's return value, unless an exception is pending.
    fn finish(mut self, v: Value) -> PResult<Value> {
        self.adopt_parked();
        match self.flow {
            Some(f) => Err(f),
            None => Ok(v),
        }
    }
}

/// `sqlite_value_to_zval()`.
fn column_to_value(stmt: &capi::Stmt, i: std::ffi::c_int) -> Value {
    match stmt.column_value(i) {
        capi::ValueRef::Null => Value::Null,
        capi::ValueRef::Integer(n) => Value::Int(n),
        capi::ValueRef::Real(d) => Value::Float(d),
        capi::ValueRef::Text(b) | capi::ValueRef::Blob(b) => Value::bytes(b),
    }
}

pub fn register(r: &mut Registry) {
    let exception = r
        .find_class("Exception")
        .expect("Exception is a core class");
    r.class(ClassEntry::new_subclass(EXCEPTION_CLASS, 0, &exception));
    db::register(r);
    stmt::register(r);
    result::register(r);

    r.ini(IniDef::new(
        "sqlite3.extension_dir",
        None,
        modifiable::SYSTEM,
    ));
    r.ini(IniDef::new(
        "sqlite3.defensive",
        Some("1"),
        modifiable::USER,
    ));

    r.constant("SQLITE3_ASSOC", PHP_SQLITE3_ASSOC);
    r.constant("SQLITE3_NUM", PHP_SQLITE3_NUM);
    r.constant("SQLITE3_BOTH", PHP_SQLITE3_BOTH);
    r.constant("SQLITE3_INTEGER", i64::from(ffi::SQLITE_INTEGER));
    r.constant("SQLITE3_FLOAT", i64::from(ffi::SQLITE_FLOAT));
    r.constant("SQLITE3_TEXT", i64::from(ffi::SQLITE_TEXT));
    r.constant("SQLITE3_BLOB", i64::from(ffi::SQLITE_BLOB));
    r.constant("SQLITE3_NULL", i64::from(ffi::SQLITE_NULL));
    r.constant(
        "SQLITE3_OPEN_READONLY",
        i64::from(ffi::SQLITE_OPEN_READONLY),
    );
    r.constant(
        "SQLITE3_OPEN_READWRITE",
        i64::from(ffi::SQLITE_OPEN_READWRITE),
    );
    r.constant("SQLITE3_OPEN_CREATE", i64::from(ffi::SQLITE_OPEN_CREATE));
    r.constant(
        "SQLITE3_DETERMINISTIC",
        i64::from(ffi::SQLITE_DETERMINISTIC),
    );
}
