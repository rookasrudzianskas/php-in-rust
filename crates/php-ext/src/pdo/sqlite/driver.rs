//! `sqlite_driver.c`: the SQLite connection handle, error mapping, the
//! handle factory and the user-defined function/aggregate/collation/
//! authorizer callbacks.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::ffi::c_int;
use std::rc::Rc;

use php_runtime::{Flow, PResult, PhpArray, PhpArrayRef, PhpStr, Value, Vm};

use crate::sqlite3::capi::{self, AggContext, Connection, FnOutput, StepOutput, ValueRef, ffi};

use super::super::consts::{
    ATTR_CLIENT_VERSION, ATTR_CURSOR, ATTR_SERVER_VERSION, ATTR_TIMEOUT, CURSOR_FWDONLY,
};
use super::super::dbh::{Dbh, ERRMODE_EXCEPTION, ERRMODE_WARNING, attr_lval, get_long_param};
use super::super::driver::{AttrValue, DbhMethods, Driver, ParamEvent, Scanner};
use super::super::error::{self, ERR_NONE, sqlstate};
use super::super::parser;
use super::super::stmt::{PLACEHOLDER_NAMED, PLACEHOLDER_POSITIONAL, Stmt};
use super::statement::SqliteStmt;
use super::{
    ATTR_EXTENDED_RESULT_CODES, ATTR_OPEN_FLAGS, ATTR_TRANSACTION_MODE, PDO_CONSTANTS, PDO_METHODS,
    SPECIFIC_CLASS, TRANSACTION_MODE_DEFERRED, TRANSACTION_MODE_EXCLUSIVE,
    TRANSACTION_MODE_IMMEDIATE,
};

/// `pdo_sqlite_error_info`.
#[derive(Default)]
pub struct ErrorInfo {
    pub errcode: c_int,
    pub errmsg: Option<String>,
}

/// `pdo_sqlite_db_handle`.
pub struct SqliteHandle {
    pub db: Connection,
    pub einfo: RefCell<ErrorInfo>,
    /// The userland authorizer (`authorizer_fcc`), shared with the installed
    /// SQLite authorizer.
    pub authorizer: Rc<RefCell<Option<Value>>>,
    pub transaction_mode: Cell<i64>,
}

impl SqliteHandle {
    /// `_pdo_sqlite_error`: records the connection's error on the statement
    /// (or handle) and returns the SQLite code (`0` without error).
    pub fn error(&self, vm: &mut Vm, dbh: &Dbh, stmt: Option<&Stmt>) -> PResult<c_int> {
        let errcode = self.db.errcode();
        let mut einfo = self.einfo.borrow_mut();
        einfo.errcode = errcode;
        let set = |code| match stmt {
            Some(s) => s.error_code.set(code),
            None => dbh.error_code.set(code),
        };
        if errcode == ffi::SQLITE_OK {
            set(ERR_NONE);
            return Ok(0);
        }
        einfo.errmsg = Some(self.db.errmsg());
        let state = state_for(errcode);
        set(sqlstate(state));
        if dbh.methods().is_none() {
            let msg = einfo.errmsg.clone().unwrap_or_default();
            drop(einfo);
            return Err(error::throw_with_info(
                vm,
                errcode as i64,
                &msg,
                &sqlstate(state),
            ));
        }
        Ok(errcode)
    }
}

/// The SQLSTATE of an SQLite result code (`_pdo_sqlite_error`).
fn state_for(errcode: c_int) -> &'static str {
    match errcode {
        ffi::SQLITE_NOTFOUND => "42S02",
        ffi::SQLITE_INTERRUPT => "01002",
        ffi::SQLITE_NOLFS => "HYC00",
        ffi::SQLITE_TOOBIG => "22001",
        ffi::SQLITE_CONSTRAINT => "23000",
        _ => "HY000",
    }
}

/// The SQLite handle of a PDO handle (`dbh->driver_data`).
pub fn handle(dbh: &Dbh) -> Option<Rc<dyn DbhMethods>> {
    dbh.methods().filter(|m| m.as_any().is::<SqliteHandle>())
}

/// Runs `f` with the SQLite handle of `dbh`.
pub fn with_handle<R>(dbh: &Dbh, f: impl FnOnce(&SqliteHandle) -> R) -> Option<R> {
    let m = handle(dbh)?;
    let h = m.as_any().downcast_ref::<SqliteHandle>()?;
    Some(f(h))
}

impl SqliteHandle {
    fn exec_simple(&self, vm: &mut Vm, dbh: &Dbh, sql: &[u8]) -> PResult<bool> {
        if self.db.exec(vm, sql).is_err() {
            self.error(vm, dbh, None)?;
            return Ok(false);
        }
        Ok(true)
    }
}

impl DbhMethods for SqliteHandle {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn close(&self, _vm: &mut Vm, _dbh: &Dbh) {
        // `sqlite_handle_closer`: the callbacks and the connection are
        // released with the last user of the connection (`sqlite3_close_v2`).
        self.authorizer.borrow_mut().take();
    }

    /// `sqlite_handle_preparer`.
    fn prepare(
        &self,
        vm: &mut Vm,
        dbh: &Rc<Dbh>,
        sql: &PhpStr,
        stmt: &Stmt,
        options: Option<&PhpArrayRef>,
    ) -> PResult<bool> {
        stmt.supports_placeholders
            .set(PLACEHOLDER_POSITIONAL | PLACEHOLDER_NAMED);
        if attr_lval(options, ATTR_CURSOR, CURSOR_FWDONLY) != CURSOR_FWDONLY {
            self.einfo.borrow_mut().errcode = ffi::SQLITE_ERROR;
            self.error(vm, dbh, None)?;
            return Ok(false);
        }
        let h = dbh.methods().expect("the SQLite handle is installed");
        match self.db.prepare(vm, sql) {
            Ok(s) => {
                *stmt.driver.borrow_mut() = Some(Box::new(SqliteStmt::new(h, s)));
                Ok(true)
            }
            Err(_) => {
                self.error(vm, dbh, None)?;
                Ok(false)
            }
        }
    }

    /// `sqlite_handle_doer`.
    fn exec(&self, vm: &mut Vm, dbh: &Dbh, sql: &PhpStr) -> PResult<i64> {
        if self.db.exec(vm, sql).is_err() {
            self.error(vm, dbh, None)?;
            return Ok(-1);
        }
        Ok(self.db.changes())
    }

    fn supports_quote(&self) -> bool {
        true
    }

    /// `sqlite_handle_quoter` (`sqlite3_snprintf("'%q'")`).
    fn quote(
        &self,
        vm: &mut Vm,
        dbh: &Dbh,
        unquoted: &PhpStr,
        _param_type: i64,
    ) -> PResult<Option<PhpStr>> {
        if unquoted.len() > (i32::MAX as usize - 3) / 2 {
            return Ok(None);
        }
        if unquoted.contains(&0) {
            match dbh.error_mode.get() {
                ERRMODE_EXCEPTION => {
                    return Err(error::throw(
                        vm,
                        "SQLite PDO::quote does not support null bytes",
                    ));
                }
                ERRMODE_WARNING => {
                    error::warning(vm, dbh, "SQLite PDO::quote does not support null bytes")?;
                }
                _ => {}
            }
            return Ok(None);
        }
        let mut out = Vec::with_capacity(unquoted.len() + 2);
        out.push(b'\'');
        for &c in unquoted.as_bytes() {
            if c == b'\'' {
                out.push(b'\'');
            }
            out.push(c);
        }
        out.push(b'\'');
        Ok(Some(PhpStr::from_vec(out)))
    }

    fn supports_transactions(&self) -> bool {
        true
    }

    /// `sqlite_handle_begin`.
    fn begin(&self, vm: &mut Vm, dbh: &Dbh) -> PResult<bool> {
        let sql: &[u8] = match self.transaction_mode.get() {
            TRANSACTION_MODE_DEFERRED => b"BEGIN DEFERRED TRANSACTION",
            TRANSACTION_MODE_IMMEDIATE => b"BEGIN IMMEDIATE TRANSACTION",
            TRANSACTION_MODE_EXCLUSIVE => b"BEGIN EXCLUSIVE TRANSACTION",
            _ => b"BEGIN",
        };
        self.exec_simple(vm, dbh, sql)
    }

    fn commit(&self, vm: &mut Vm, dbh: &Dbh) -> PResult<bool> {
        self.exec_simple(vm, dbh, b"COMMIT")
    }

    fn rollback(&self, vm: &mut Vm, dbh: &Dbh) -> PResult<bool> {
        self.exec_simple(vm, dbh, b"ROLLBACK")
    }

    fn supports_set_attribute(&self) -> bool {
        true
    }

    /// `pdo_sqlite_set_attr`.
    fn set_attribute(&self, vm: &mut Vm, _dbh: &Dbh, attr: i64, value: &Value) -> PResult<bool> {
        match attr {
            ATTR_TIMEOUT => {
                let l = get_long_param(vm, value)?;
                self.db.busy_timeout(l.wrapping_mul(1000) as c_int);
                Ok(true)
            }
            ATTR_EXTENDED_RESULT_CODES => {
                let l = get_long_param(vm, value)?;
                self.db.extended_result_codes(l as c_int != 0);
                Ok(true)
            }
            ATTR_TRANSACTION_MODE => {
                let l = get_long_param(vm, value)?;
                match l {
                    TRANSACTION_MODE_DEFERRED
                    | TRANSACTION_MODE_IMMEDIATE
                    | TRANSACTION_MODE_EXCLUSIVE => {
                        self.transaction_mode.set(l);
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            _ => Ok(false),
        }
    }

    fn supports_last_id(&self) -> bool {
        true
    }

    fn last_insert_id(
        &self,
        _vm: &mut Vm,
        _dbh: &Dbh,
        _name: Option<&PhpStr>,
    ) -> PResult<Option<PhpStr>> {
        Ok(Some(PhpStr::from(self.db.last_insert_rowid().to_string())))
    }

    fn supports_fetch_err(&self) -> bool {
        true
    }

    /// `pdo_sqlite_fetch_error_func`.
    fn fetch_err(&self, _dbh: &Dbh, _stmt: Option<&Stmt>, info: &mut PhpArray) {
        let einfo = self.einfo.borrow();
        if einfo.errcode != 0 {
            let _ = info.append(Value::Int(einfo.errcode as u32 as i64));
            let _ = info.append(Value::from(einfo.errmsg.clone().unwrap_or_default()));
        }
    }

    fn supports_get_attribute(&self) -> bool {
        true
    }

    /// `pdo_sqlite_get_attribute`.
    fn get_attribute(&self, _vm: &mut Vm, _dbh: &Dbh, attr: i64) -> PResult<AttrValue> {
        Ok(match attr {
            ATTR_CLIENT_VERSION | ATTR_SERVER_VERSION => {
                AttrValue::Value(Value::from(capi::libversion()))
            }
            ATTR_TRANSACTION_MODE => AttrValue::Value(Value::Int(self.transaction_mode.get())),
            _ => AttrValue::Unsupported,
        })
    }

    /// `pdo_sqlite_in_transaction`.
    fn in_transaction(&self, _dbh: &Dbh) -> Option<bool> {
        Some(self.db.is_open() && !self.db.get_autocommit())
    }

    fn scanner(&self) -> Option<Scanner> {
        Some(parser::sqlite_scanner)
    }

    fn has_exception(&self) -> bool {
        self.db.has_exception()
    }

    fn take_exception(&self) -> Option<Flow> {
        self.db.take_exception()
    }
}

// ---------------------------------------------------------------------------
// The factory
// ---------------------------------------------------------------------------

/// `make_filename_safe`: `None` when open_basedir forbids the file.
pub fn make_filename_safe(vm: &mut Vm, filename: &[u8]) -> PResult<Option<Vec<u8>>> {
    use crate::standard::streams::plain;
    let open_basedir = vm.ini_get("open_basedir").is_some_and(|v| !v.is_empty());
    if !filename.is_empty() && filename.len() >= 5 && filename[..5].eq_ignore_ascii_case(b"file:") {
        if open_basedir {
            return Ok(None);
        }
        return Ok(Some(filename.to_vec()));
    }
    if !filename.is_empty() && filename != b":memory:" {
        let Some(full) = plain::expand_filepath(filename) else {
            return Ok(None);
        };
        if plain::check_open_basedir(vm, &full, true)? {
            return Ok(None);
        }
        return Ok(Some(full));
    }
    Ok(Some(filename.to_vec()))
}

/// `authorizer`: open_basedir checks for `ATTACH`, then the userland
/// callback set with `Pdo\Sqlite::setAuthorizer()`.
fn install_authorizer(conn: &Connection, slot: Rc<RefCell<Option<Value>>>) {
    conn.set_authorizer(Some(Box::new(move |vm, action, argv| {
        let open_basedir = vm.ini_get("open_basedir").is_some_and(|v| !v.is_empty());
        if open_basedir && action == ffi::SQLITE_ATTACH {
            let Some(file) = argv[0] else {
                return Ok(ffi::SQLITE_DENY);
            };
            if make_filename_safe(vm, file)?.is_none() {
                return Ok(ffi::SQLITE_DENY);
            }
        }
        let Some(cb) = slot.borrow().clone() else {
            return Ok(ffi::SQLITE_OK);
        };
        let mut args = vec![Value::Int(action as i64)];
        for a in argv {
            args.push(a.map_or(Value::Null, Value::from));
        }
        let ret = vm.call(&cb, args)?.into_unref();
        let func = vm.current_function_display_name();
        match ret {
            Value::Int(r) => {
                let r = r as c_int;
                if r != ffi::SQLITE_OK && r != ffi::SQLITE_IGNORE && r != ffi::SQLITE_DENY {
                    return Err(vm.throw_value_error(format!(
                        "{func}(): Return value of the authorizer callback must be one of Pdo\\Sqlite::OK, Pdo\\Sqlite::DENY, or Pdo\\Sqlite::IGNORE"
                    )));
                }
                Ok(r)
            }
            other => Err(vm.throw_type_error(format!(
                "{func}(): Return value of the authorizer callback must be of type int, {} returned",
                other.value_name()
            ))),
        }
    })));
}

/// `pdo_sqlite_handle_factory`.
pub fn handle_factory(vm: &mut Vm, dbh: &Rc<Dbh>, options: Option<&PhpArrayRef>) -> PResult<bool> {
    // Skip all but the EXEC_PRE parameter event.
    dbh.skip_param_evt
        .set(0x7F ^ (1 << ParamEvent::ExecPre as u8));
    let data_source = dbh.data_source.borrow().clone();
    let Some(filename) = make_filename_safe(vm, &data_source)? else {
        return Err(error::throw(
            vm,
            format!("open_basedir prohibits opening {data_source}"),
        ));
    };
    let mut flags = attr_lval(
        options,
        ATTR_OPEN_FLAGS,
        (ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE) as i64,
    ) as c_int;
    if vm.ini_get("open_basedir").is_none_or(|v| v.is_empty()) {
        flags |= ffi::SQLITE_OPEN_URI;
    }
    let conn = match Connection::open(&filename, flags) {
        Ok(c) => c,
        Err((rc, msg)) => {
            // `pdo_sqlite_error()` before `dbh->methods` is set throws.
            return Err(error::throw_with_info(
                vm,
                rc as i64,
                &msg,
                &sqlstate(state_for(rc)),
            ));
        }
    };
    let slot = Rc::new(RefCell::new(None));
    install_authorizer(&conn, slot.clone());
    let timeout = attr_lval(options, ATTR_TIMEOUT, 60);
    conn.busy_timeout(timeout.wrapping_mul(1000) as c_int);
    let handle = SqliteHandle {
        db: conn,
        einfo: RefCell::new(ErrorInfo::default()),
        authorizer: slot,
        transaction_mode: Cell::new(0),
    };
    dbh.alloc_own_columns.set(true);
    dbh.max_escaped_char_length.set(2);
    *dbh.methods.borrow_mut() = Some(Rc::new(handle));
    Ok(true)
}

/// `pdo_sqlite_driver`.
pub struct SqliteDriver;

impl Driver for SqliteDriver {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn handle_factory(
        &self,
        vm: &mut Vm,
        dbh: &Rc<Dbh>,
        options: Option<&PhpArrayRef>,
    ) -> PResult<bool> {
        handle_factory(vm, dbh, options)
    }

    fn specific_class(&self) -> Option<&'static str> {
        Some(SPECIFIC_CLASS)
    }

    fn pdo_methods(&self) -> &'static [&'static php_runtime::NativeFunction] {
        PDO_METHODS
    }

    fn pdo_constants(&self) -> &'static [(&'static str, i64, &'static str)] {
        PDO_CONSTANTS
    }
}

// ---------------------------------------------------------------------------
// User callbacks (`do_callback`, collation, aggregate steps)
// ---------------------------------------------------------------------------

/// Arguments of a PDO SQLite callback: integers go through
/// `sqlite3_value_int()` (32-bit), text and blobs become strings.
fn callback_args(values: &[ValueRef<'_>]) -> Vec<Value> {
    values
        .iter()
        .map(|v| match *v {
            ValueRef::Integer(i) => Value::Int(i as i32 as i64),
            ValueRef::Real(d) => Value::Float(d),
            ValueRef::Null => Value::Null,
            ValueRef::Text(s) | ValueRef::Blob(s) => Value::bytes(s),
        })
        .collect()
}

/// The SQLite result of a callback's return value (`sqlite3_result_int()`
/// is 32-bit; other values are converted to strings).
fn callback_result(vm: &mut Vm, ret: PResult<Value>) -> FnOutput {
    match ret {
        Err(flow) => FnOutput::Exception(flow, Some(Vec::new())),
        Ok(v) => match v.into_unref() {
            Value::Int(i) => FnOutput::Int(i as i32 as i64),
            Value::Null => FnOutput::Null,
            Value::Float(d) => FnOutput::Float(d),
            other => match vm.to_php_string(&other) {
                Ok(s) => FnOutput::Text(s.to_vec()),
                Err(flow) => FnOutput::Exception(flow, None),
            },
        },
    }
}

/// The aggregate context: a reference created on first use
/// (`ZVAL_NEW_REF(&agg_context->val, &EG(uninitialized_zval))`).
fn agg_ref(agg: &mut AggContext) -> Value {
    if agg.value.is_undef() {
        agg.value = Value::Ref(php_runtime::PhpRef::new(Value::Null));
    }
    agg.value.clone()
}

/// `sqlite3_create_function()` for `createFunction()`/`sqliteCreateFunction()`.
pub fn create_function(
    h: &SqliteHandle,
    name: &[u8],
    callback: Value,
    argc: i64,
    flags: i64,
) -> bool {
    let rc = h.db.create_function(
        name,
        argc as c_int,
        flags as c_int,
        Box::new(move |vm, values| {
            let args = callback_args(values);
            let r = vm.call(&callback, args);
            callback_result(vm, r)
        }),
    );
    rc == ffi::SQLITE_OK
}

/// `sqlite3_create_function()` with step/final callbacks.
pub fn create_aggregate(
    h: &SqliteHandle,
    name: &[u8],
    step: Value,
    fini: Value,
    argc: i64,
) -> bool {
    let rc = h.db.create_aggregate(
        name,
        argc as c_int,
        0,
        Box::new(move |vm, agg, values| {
            let ctx = agg_ref(agg);
            agg.row_count += 1;
            let mut args = vec![ctx.clone(), Value::Int(agg.row_count)];
            args.extend(callback_args(values));
            match vm.call(&step, args) {
                Err(flow) => StepOutput::Exception(flow),
                Ok(v) => {
                    if let Value::Ref(r) = &ctx {
                        r.set(v.into_unref());
                    }
                    StepOutput::Ok
                }
            }
        }),
        Box::new(move |vm, agg| {
            let ctx = agg_ref(agg);
            agg.row_count += 1;
            let args = vec![ctx, Value::Int(agg.row_count)];
            let r = vm.call(&fini, args);
            let out = callback_result(vm, r);
            agg.value = Value::Undef;
            out
        }),
    );
    rc == ffi::SQLITE_OK
}

/// `sqlite3_create_collation()` with `php_sqlite3_collation_callback`.
pub fn create_collation(h: &SqliteHandle, name: &[u8], callback: Value) -> bool {
    let rc = h.db.create_collation(
        name,
        Box::new(move |vm, a, b| {
            let ret = vm.call(&callback, vec![Value::bytes(a), Value::bytes(b)])?;
            match ret.into_unref() {
                Value::Int(i) => Ok(i.signum() as c_int),
                other => {
                    let func = vm.current_function_display_name();
                    Err(vm.throw_type_error(format!(
                        "{func}(): Return value of the collation callback must be of type int, {} returned",
                        other.value_name()
                    )))
                }
            }
        }),
    );
    rc == ffi::SQLITE_OK
}
