//! A thin, safe layer over the SQLite C API (`rusqlite::ffi`, the bundled
//! amalgamation), shared by ext/sqlite3 and ext/pdo_sqlite — the role
//! libsqlite3 plays for both extensions in php-src.
//!
//! rusqlite's high-level API ties statements to a borrowed connection, which
//! cannot express PHP objects (`SQLite3Stmt`, `PDOStatement`) that live
//! independently of the connection object and step across separate calls.
//! This module therefore wraps the raw handles directly; all `unsafe` code of
//! the database layer lives here.
//!
//! * [`Connection`] owns a `sqlite3*` (shared by its statements, blobs and
//!   registered callbacks through an `Rc`). [`Connection::close`] is
//!   `sqlite3_close()` (fails with `SQLITE_BUSY` while statements are alive);
//!   dropping the last handle closes the database.
//! * [`Stmt`] owns a `sqlite3_stmt*`. Calls that can run user callbacks take
//!   the [`Vm`] (`step`, `exec`, `prepare`), so SQL functions, aggregates,
//!   collations and the authorizer can call back into PHP.
//! * A PHP exception thrown by a callback cannot unwind through SQLite: the
//!   trampoline stores it on the connection ([`Connection::take_exception`])
//!   and reports an SQLite error instead; the extension rethrows it once the
//!   SQLite call returns. While an exception is pending no further callback
//!   is invoked (like `zend_call_function()` with `EG(exception)` set).

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;

use php_runtime::{Flow, Value, Vm};

pub use rusqlite::ffi;

// ---------------------------------------------------------------------------
// The VM seen by callbacks
// ---------------------------------------------------------------------------

thread_local! {
    static ACTIVE_VM: Cell<*mut Vm> = const { Cell::new(ptr::null_mut()) };
}

/// Runs `f` (a call into SQLite) with `vm` reachable by callback trampolines.
fn with_vm<R>(vm: &mut Vm, f: impl FnOnce() -> R) -> R {
    struct Restore(*mut Vm);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE_VM.with(|c| c.set(self.0));
        }
    }
    let prev = ACTIVE_VM.with(|c| c.replace(vm as *mut Vm));
    let _restore = Restore(prev);
    f()
}

/// Runs `f` with the VM of the innermost [`with_vm`] call, if any.
fn with_active_vm<R>(f: impl FnOnce(&mut Vm) -> R) -> Option<R> {
    let p = ACTIVE_VM.with(Cell::get);
    // SAFETY: `p` was derived from a `&mut Vm` by `with_vm`, whose caller does
    // not touch the VM while SQLite runs; the reference created here is the
    // only live one until `f` returns (nested calls re-borrow from it).
    unsafe { p.as_mut() }.map(f)
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A column or function-argument value with its SQLite storage class.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueRef<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a [u8]),
    Blob(&'a [u8]),
}

/// Copies `n` bytes at `p` (`NULL` or `n <= 0` is the empty string).
///
/// # Safety
/// `p` must be valid for `n` bytes.
unsafe fn bytes_at<'a>(p: *const c_void, n: c_int) -> &'a [u8] {
    if p.is_null() || n <= 0 {
        &[]
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { std::slice::from_raw_parts(p as *const u8, n as usize) }
    }
}

/// A NUL-terminated C string as bytes (`None` for `NULL`).
///
/// # Safety
/// `p` must be `NULL` or point to a NUL-terminated string.
unsafe fn cstr_opt<'a>(p: *const c_char) -> Option<&'a [u8]> {
    if p.is_null() {
        None
    } else {
        // SAFETY: guaranteed by the caller.
        Some(unsafe { CStr::from_ptr(p) }.to_bytes())
    }
}

/// `CString` from PHP bytes, truncated at the first NUL like `ZSTR_VAL()`
/// seen through a C API.
pub fn c_string(b: &[u8]) -> CString {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    CString::new(&b[..end]).expect("no interior NUL")
}

/// # Safety
/// `v` must be a valid protected or unprotected `sqlite3_value*`.
unsafe fn value_ref<'a>(v: *mut ffi::sqlite3_value) -> ValueRef<'a> {
    // SAFETY: `v` is valid (caller); the accessors follow the documented
    // type-then-pointer-then-length order.
    unsafe {
        match ffi::sqlite3_value_type(v) {
            ffi::SQLITE_INTEGER => ValueRef::Integer(ffi::sqlite3_value_int64(v)),
            ffi::SQLITE_FLOAT => ValueRef::Real(ffi::sqlite3_value_double(v)),
            ffi::SQLITE_NULL => ValueRef::Null,
            ffi::SQLITE_TEXT => {
                let p = ffi::sqlite3_value_text(v);
                ValueRef::Text(bytes_at(p as *const c_void, ffi::sqlite3_value_bytes(v)))
            }
            _ => {
                let p = ffi::sqlite3_value_blob(v);
                ValueRef::Blob(bytes_at(p, ffi::sqlite3_value_bytes(v)))
            }
        }
    }
}

/// `sqlite3_errstr()`.
pub fn errstr(rc: c_int) -> String {
    // SAFETY: sqlite3_errstr returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(ffi::sqlite3_errstr(rc)) }
        .to_string_lossy()
        .into_owned()
}

/// `sqlite3_libversion()`.
pub fn libversion() -> &'static str {
    // SAFETY: sqlite3_libversion returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(ffi::sqlite3_libversion()) }
        .to_str()
        .unwrap_or("")
}

/// `sqlite3_libversion_number()`.
pub fn libversion_number() -> i64 {
    // SAFETY: no preconditions.
    i64::from(unsafe { ffi::sqlite3_libversion_number() })
}

// ---------------------------------------------------------------------------
// Callbacks
// ---------------------------------------------------------------------------

/// The result of a scalar function or an aggregate's final step.
pub enum FnOutput {
    Null,
    Int(i64),
    Float(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
    /// `sqlite3_result_error(ctx, msg, len)` (an empty message passes length 0).
    Error(Vec<u8>),
    /// A PHP exception: stored on the connection; `Some(msg)` also reports
    /// `sqlite3_result_error(msg)`, `None` leaves the result `NULL`.
    Exception(Flow, Option<Vec<u8>>),
}

/// The result of an aggregate's step callback.
pub enum StepOutput {
    Ok,
    Error(Vec<u8>),
    /// A PHP exception (stored on the connection, no SQLite error).
    Exception(Flow),
}

/// Per-group aggregate state (`sqlite3_aggregate_context`): the PHP context
/// value (`Undef` before the first step) and a row counter.
#[derive(Default)]
pub struct AggContext {
    pub value: Value,
    pub row_count: i64,
}

pub type ScalarFn = Box<dyn Fn(&mut Vm, &[ValueRef<'_>]) -> FnOutput>;
pub type StepFn = Box<dyn Fn(&mut Vm, &mut AggContext, &[ValueRef<'_>]) -> StepOutput>;
pub type FinalFn = Box<dyn Fn(&mut Vm, &mut AggContext) -> FnOutput>;
/// Collation callback: `Err` stores the exception and compares as `0`.
pub type CollationFn = Box<dyn Fn(&mut Vm, &[u8], &[u8]) -> Result<c_int, Flow>>;
/// Authorizer: `(action, arg1, arg2, database, trigger)`; `Err` stores the
/// exception and denies.
pub type AuthorizerFn = Box<dyn Fn(&mut Vm, c_int, [Option<&[u8]>; 4]) -> Result<c_int, Flow>>;

type Pending = Rc<RefCell<Option<Flow>>>;

fn store_pending(pending: &Pending, flow: Flow) {
    let mut p = pending.borrow_mut();
    if p.is_none() {
        *p = Some(flow);
    }
}

enum FuncKind {
    Scalar(ScalarFn),
    Aggregate(StepFn, FinalFn),
}

struct FuncData {
    kind: FuncKind,
    pending: Pending,
}

struct CollationData {
    cmp: CollationFn,
    pending: Pending,
}

struct AuthData {
    cb: AuthorizerFn,
    pending: Pending,
}

/// # Safety
/// `ctx` must be the context of a running function call.
unsafe fn set_result(ctx: *mut ffi::sqlite3_context, out: FnOutput, pending: &Pending) {
    // SAFETY: `ctx` is valid (caller); SQLITE_TRANSIENT makes SQLite copy
    // the buffers before they are dropped.
    unsafe {
        match out {
            FnOutput::Null => ffi::sqlite3_result_null(ctx),
            FnOutput::Int(i) => ffi::sqlite3_result_int64(ctx, i),
            FnOutput::Float(d) => ffi::sqlite3_result_double(ctx, d),
            FnOutput::Text(s) => ffi::sqlite3_result_text(
                ctx,
                s.as_ptr() as *const c_char,
                s.len() as c_int,
                ffi::SQLITE_TRANSIENT(),
            ),
            FnOutput::Blob(b) => ffi::sqlite3_result_blob(
                ctx,
                b.as_ptr() as *const c_void,
                b.len() as c_int,
                ffi::SQLITE_TRANSIENT(),
            ),
            FnOutput::Error(msg) => {
                ffi::sqlite3_result_error(ctx, msg.as_ptr() as *const c_char, msg.len() as c_int)
            }
            FnOutput::Exception(flow, msg) => {
                store_pending(pending, flow);
                if let Some(msg) = msg {
                    ffi::sqlite3_result_error(
                        ctx,
                        msg.as_ptr() as *const c_char,
                        msg.len() as c_int,
                    );
                }
            }
        }
    }
}

/// # Safety
/// `argv` must hold `argc` valid values.
unsafe fn args<'a>(argc: c_int, argv: *mut *mut ffi::sqlite3_value) -> Vec<ValueRef<'a>> {
    if argv.is_null() || argc <= 0 {
        return Vec::new();
    }
    // SAFETY: guaranteed by the caller.
    unsafe {
        std::slice::from_raw_parts(argv, argc as usize)
            .iter()
            .map(|&v| value_ref(v))
            .collect()
    }
}

/// Error reported when a callback cannot run (no VM, or an exception is
/// already pending): `zend_call_function()` leaves the result undefined and
/// php-src reports `sqlite3_result_error(ctx, "...", 0)`, an empty message.
unsafe fn result_skipped(ctx: *mut ffi::sqlite3_context) {
    // SAFETY: `ctx` is valid (caller).
    unsafe { ffi::sqlite3_result_error(ctx, c"".as_ptr(), 0) }
}

unsafe extern "C" fn scalar_trampoline(
    ctx: *mut ffi::sqlite3_context,
    argc: c_int,
    argv: *mut *mut ffi::sqlite3_value,
) {
    // SAFETY: user data is the `FuncData` registered with this function; it
    // lives until SQLite calls `destroy_func`.
    let data = unsafe { &*(ffi::sqlite3_user_data(ctx) as *const FuncData) };
    let FuncKind::Scalar(f) = &data.kind else {
        return;
    };
    if data.pending.borrow().is_some() {
        // SAFETY: valid context.
        unsafe { result_skipped(ctx) };
        return;
    }
    // SAFETY: SQLite passes `argc` valid values.
    let values = unsafe { args(argc, argv) };
    match with_active_vm(|vm| f(vm, &values)) {
        // SAFETY: valid context.
        Some(out) => unsafe { set_result(ctx, out, &data.pending) },
        None => unsafe { result_skipped(ctx) },
    }
}

/// The aggregate state of `ctx`, allocated on first use.
///
/// # Safety
/// `ctx` must be the context of a running aggregate.
unsafe fn agg_context<'a>(ctx: *mut ffi::sqlite3_context) -> Option<&'a mut AggContext> {
    // SAFETY: SQLite returns zeroed memory of the requested size on the first
    // call for each group and the same memory afterwards; it stores a pointer
    // to a boxed `AggContext` released by `final_trampoline`.
    unsafe {
        let slot =
            ffi::sqlite3_aggregate_context(ctx, std::mem::size_of::<*mut AggContext>() as c_int)
                as *mut *mut AggContext;
        if slot.is_null() {
            return None;
        }
        if (*slot).is_null() {
            *slot = Box::into_raw(Box::default());
        }
        Some(&mut **slot)
    }
}

unsafe extern "C" fn step_trampoline(
    ctx: *mut ffi::sqlite3_context,
    argc: c_int,
    argv: *mut *mut ffi::sqlite3_value,
) {
    // SAFETY: see `scalar_trampoline`.
    let data = unsafe { &*(ffi::sqlite3_user_data(ctx) as *const FuncData) };
    let FuncKind::Aggregate(step, _) = &data.kind else {
        return;
    };
    // SAFETY: running aggregate context.
    let Some(agg) = (unsafe { agg_context(ctx) }) else {
        return;
    };
    if data.pending.borrow().is_some() {
        return;
    }
    // SAFETY: SQLite passes `argc` valid values.
    let values = unsafe { args(argc, argv) };
    match with_active_vm(|vm| step(vm, agg, &values)) {
        Some(StepOutput::Ok) | None => {}
        Some(StepOutput::Error(msg)) => {
            // SAFETY: valid context; SQLite copies the message.
            unsafe {
                ffi::sqlite3_result_error(ctx, msg.as_ptr() as *const c_char, msg.len() as c_int)
            }
        }
        Some(StepOutput::Exception(flow)) => store_pending(&data.pending, flow),
    }
}

unsafe extern "C" fn final_trampoline(ctx: *mut ffi::sqlite3_context) {
    // SAFETY: see `scalar_trampoline`.
    let data = unsafe { &*(ffi::sqlite3_user_data(ctx) as *const FuncData) };
    let FuncKind::Aggregate(_, fini) = &data.kind else {
        return;
    };
    // SAFETY: running aggregate context; the box is taken back here, once.
    let state = unsafe {
        let slot =
            ffi::sqlite3_aggregate_context(ctx, std::mem::size_of::<*mut AggContext>() as c_int)
                as *mut *mut AggContext;
        if slot.is_null() {
            None
        } else if (*slot).is_null() {
            Some(Box::<AggContext>::default())
        } else {
            let b = Box::from_raw(*slot);
            *slot = ptr::null_mut();
            Some(b)
        }
    };
    let Some(mut state) = state else {
        return;
    };
    if data.pending.borrow().is_some() {
        // SAFETY: valid context.
        unsafe { result_skipped(ctx) };
        return;
    }
    match with_active_vm(|vm| fini(vm, &mut state)) {
        // SAFETY: valid context.
        Some(out) => unsafe { set_result(ctx, out, &data.pending) },
        None => unsafe { result_skipped(ctx) },
    }
}

unsafe extern "C" fn destroy_func(p: *mut c_void) {
    // SAFETY: `p` is the `Box<FuncData>` leaked at registration.
    drop(unsafe { Box::from_raw(p as *mut FuncData) });
}

unsafe extern "C" fn collation_trampoline(
    p: *mut c_void,
    alen: c_int,
    a: *const c_void,
    blen: c_int,
    b: *const c_void,
) -> c_int {
    // SAFETY: `p` is the registered `CollationData`, alive until destroyed.
    let data = unsafe { &*(p as *const CollationData) };
    if data.pending.borrow().is_some() {
        return 0;
    }
    // SAFETY: SQLite passes valid buffers of the given lengths.
    let (a, b) = unsafe { (bytes_at(a, alen), bytes_at(b, blen)) };
    match with_active_vm(|vm| (data.cmp)(vm, a, b)) {
        Some(Ok(r)) => r,
        Some(Err(flow)) => {
            store_pending(&data.pending, flow);
            0
        }
        None => 0,
    }
}

unsafe extern "C" fn destroy_collation(p: *mut c_void) {
    // SAFETY: `p` is the `Box<CollationData>` leaked at registration.
    drop(unsafe { Box::from_raw(p as *mut CollationData) });
}

unsafe extern "C" fn authorizer_trampoline(
    p: *mut c_void,
    action: c_int,
    a1: *const c_char,
    a2: *const c_char,
    a3: *const c_char,
    a4: *const c_char,
) -> c_int {
    // SAFETY: `p` points into the `AuthData` box owned by the connection,
    // replaced only after SQLite was given the new pointer.
    let data = unsafe { &*(p as *const AuthData) };
    if data.pending.borrow().is_some() {
        return ffi::SQLITE_DENY;
    }
    // SAFETY: SQLite passes NULL or NUL-terminated strings.
    let argv = unsafe { [cstr_opt(a1), cstr_opt(a2), cstr_opt(a3), cstr_opt(a4)] };
    match with_active_vm(|vm| (data.cb)(vm, action, argv)) {
        Some(Ok(r)) => r,
        Some(Err(flow)) => {
            store_pending(&data.pending, flow);
            ffi::SQLITE_DENY
        }
        None => ffi::SQLITE_DENY,
    }
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

struct DbInner {
    db: Cell<*mut ffi::sqlite3>,
    pending: Pending,
    authorizer: RefCell<Option<Box<AuthData>>>,
}

impl Drop for DbInner {
    fn drop(&mut self) {
        let db = self.db.replace(ptr::null_mut());
        if !db.is_null() {
            // SAFETY: `db` is an open handle owned by this struct; every
            // statement and blob holds an `Rc<DbInner>`, so none is alive.
            unsafe {
                ffi::sqlite3_close(db);
            }
        }
    }
}

/// An open database (`sqlite3*`); cloning shares the handle.
#[derive(Clone)]
pub struct Connection {
    inner: Rc<DbInner>,
}

/// Open error: result code and message (`sqlite3_errmsg()` of the failed
/// handle, else `sqlite3_errstr()`).
pub type OpenError = (c_int, String);

impl Connection {
    /// `sqlite3_open_v2(filename, &db, flags, NULL)`.
    pub fn open(filename: &[u8], flags: c_int) -> Result<Connection, OpenError> {
        let name = c_string(filename);
        let mut db: *mut ffi::sqlite3 = ptr::null_mut();
        // SAFETY: valid C string and out pointer.
        let rc = unsafe { ffi::sqlite3_open_v2(name.as_ptr(), &mut db, flags, ptr::null()) };
        if rc != ffi::SQLITE_OK {
            let msg = if db.is_null() {
                errstr(rc)
            } else {
                // SAFETY: `db` is a (failed) handle that must still be closed.
                unsafe {
                    let m = CStr::from_ptr(ffi::sqlite3_errmsg(db))
                        .to_string_lossy()
                        .into_owned();
                    ffi::sqlite3_close(db);
                    m
                }
            };
            return Err((rc, msg));
        }
        let conn = Connection {
            inner: Rc::new(DbInner {
                db: Cell::new(db),
                pending: Rc::new(RefCell::new(None)),
                authorizer: RefCell::new(None),
            }),
        };
        // The bundled amalgamation is built with SQLITE_DEFAULT_FOREIGN_KEYS=1;
        // restore the stock default (off) that php-src builds see.
        conn.db_config(ffi::SQLITE_DBCONFIG_ENABLE_FKEY, 0);
        Ok(conn)
    }

    fn raw(&self) -> *mut ffi::sqlite3 {
        self.inner.db.get()
    }

    /// Whether the handle is still open.
    pub fn is_open(&self) -> bool {
        !self.raw().is_null()
    }

    /// Whether two handles share the same database.
    pub fn ptr_eq(a: &Connection, b: &Connection) -> bool {
        Rc::ptr_eq(&a.inner, &b.inner)
    }

    /// `sqlite3_close()`: `SQLITE_BUSY` while statements or blobs are open.
    pub fn close(&self) -> c_int {
        let db = self.raw();
        if db.is_null() {
            return ffi::SQLITE_OK;
        }
        // SAFETY: open handle; on success it is never used again.
        let rc = unsafe { ffi::sqlite3_close(db) };
        if rc == ffi::SQLITE_OK {
            self.inner.db.set(ptr::null_mut());
            self.inner.authorizer.borrow_mut().take();
        }
        rc
    }

    /// The exception thrown by a callback during the last call, if any.
    pub fn take_exception(&self) -> Option<Flow> {
        self.inner.pending.borrow_mut().take()
    }

    /// Whether a callback exception is pending.
    pub fn has_exception(&self) -> bool {
        self.inner.pending.borrow().is_some()
    }

    /// `sqlite3_errcode()`.
    pub fn errcode(&self) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open handle.
        unsafe { ffi::sqlite3_errcode(self.raw()) }
    }

    /// `sqlite3_extended_errcode()`.
    pub fn extended_errcode(&self) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open handle.
        unsafe { ffi::sqlite3_extended_errcode(self.raw()) }
    }

    /// `sqlite3_errmsg()`.
    pub fn errmsg(&self) -> String {
        if !self.is_open() {
            return errstr(ffi::SQLITE_MISUSE);
        }
        // SAFETY: open handle; the message is copied before any other call.
        unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(self.raw())) }
            .to_string_lossy()
            .into_owned()
    }

    /// `sqlite3_exec(db, sql, NULL, NULL, &errmsg)`; the error carries the
    /// result code and the `errmsg` out-parameter.
    pub fn exec(&self, vm: &mut Vm, sql: &[u8]) -> Result<(), (c_int, Option<String>)> {
        if !self.is_open() {
            return Err((ffi::SQLITE_MISUSE, None));
        }
        let sql = c_string(sql);
        let mut err: *mut c_char = ptr::null_mut();
        let db = self.raw();
        // SAFETY: open handle, valid C string and out pointer; the error
        // message is copied and freed with sqlite3_free.
        let rc = with_vm(vm, || unsafe {
            ffi::sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), &mut err)
        });
        let msg = if err.is_null() {
            None
        } else {
            // SAFETY: `err` is a NUL-terminated string from sqlite3_malloc.
            unsafe {
                let m = CStr::from_ptr(err).to_string_lossy().into_owned();
                ffi::sqlite3_free(err as *mut c_void);
                Some(m)
            }
        };
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err((rc, msg))
        }
    }

    /// `sqlite3_prepare_v2(db, sql, len, &stmt, NULL)`. An empty statement
    /// (only whitespace or comments) yields a [`Stmt`] without a handle, on
    /// which every call behaves like the C API with a `NULL` statement.
    pub fn prepare(&self, vm: &mut Vm, sql: &[u8]) -> Result<Stmt, c_int> {
        if !self.is_open() {
            return Err(ffi::SQLITE_MISUSE);
        }
        let mut stmt: *mut ffi::sqlite3_stmt = ptr::null_mut();
        let db = self.raw();
        let len = c_int::try_from(sql.len()).map_err(|_| ffi::SQLITE_TOOBIG)?;
        // SAFETY: open handle, `sql` is valid for `len` bytes (SQLite stops at
        // the first NUL), out pointer valid.
        let rc = with_vm(vm, || unsafe {
            ffi::sqlite3_prepare_v2(
                db,
                sql.as_ptr() as *const c_char,
                len,
                &mut stmt,
                ptr::null_mut(),
            )
        });
        if rc != ffi::SQLITE_OK {
            if !stmt.is_null() {
                // SAFETY: a statement returned with an error must be finalized.
                unsafe { ffi::sqlite3_finalize(stmt) };
            }
            return Err(rc);
        }
        Ok(Stmt {
            raw: stmt,
            db: self.clone(),
        })
    }

    /// `sqlite3_changes()`.
    pub fn changes(&self) -> i64 {
        if !self.is_open() {
            return 0;
        }
        // SAFETY: open handle.
        i64::from(unsafe { ffi::sqlite3_changes(self.raw()) })
    }

    /// `sqlite3_last_insert_rowid()`.
    pub fn last_insert_rowid(&self) -> i64 {
        if !self.is_open() {
            return 0;
        }
        // SAFETY: open handle.
        unsafe { ffi::sqlite3_last_insert_rowid(self.raw()) }
    }

    /// `sqlite3_get_autocommit()`.
    pub fn get_autocommit(&self) -> bool {
        // SAFETY: open handle.
        self.is_open() && unsafe { ffi::sqlite3_get_autocommit(self.raw()) } != 0
    }

    /// `sqlite3_busy_timeout()`.
    pub fn busy_timeout(&self, ms: c_int) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open handle.
        unsafe { ffi::sqlite3_busy_timeout(self.raw(), ms) }
    }

    /// `sqlite3_extended_result_codes()`.
    pub fn extended_result_codes(&self, on: bool) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open handle.
        unsafe { ffi::sqlite3_extended_result_codes(self.raw(), c_int::from(on)) }
    }

    /// `sqlite3_db_config(db, op, value, NULL)` for the boolean options.
    pub fn db_config(&self, op: c_int, value: c_int) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open handle; boolean options take (int, int*).
        unsafe { ffi::sqlite3_db_config(self.raw(), op, value, ptr::null_mut::<c_int>()) }
    }

    /// `sqlite3_enable_load_extension()`.
    pub fn enable_load_extension(&self, on: bool) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open handle.
        unsafe { ffi::sqlite3_enable_load_extension(self.raw(), c_int::from(on)) }
    }

    /// `sqlite3_load_extension(db, file, NULL, &errmsg)`.
    pub fn load_extension(&self, file: &[u8]) -> Result<(), (c_int, Option<String>)> {
        if !self.is_open() {
            return Err((ffi::SQLITE_MISUSE, None));
        }
        let file = c_string(file);
        let mut err: *mut c_char = ptr::null_mut();
        // SAFETY: open handle, valid C string and out pointer.
        let rc = unsafe {
            ffi::sqlite3_load_extension(self.raw(), file.as_ptr(), ptr::null(), &mut err)
        };
        let msg = if err.is_null() {
            None
        } else {
            // SAFETY: NUL-terminated string from sqlite3_malloc.
            unsafe {
                let m = CStr::from_ptr(err).to_string_lossy().into_owned();
                ffi::sqlite3_free(err as *mut c_void);
                Some(m)
            }
        };
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err((rc, msg))
        }
    }

    fn register_function(&self, name: &[u8], nargs: c_int, flags: c_int, kind: FuncKind) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        type XFunc =
            unsafe extern "C" fn(*mut ffi::sqlite3_context, c_int, *mut *mut ffi::sqlite3_value);
        type XFinal = unsafe extern "C" fn(*mut ffi::sqlite3_context);
        let (x_func, x_step, x_final): (Option<XFunc>, Option<XFunc>, Option<XFinal>) = match kind {
            FuncKind::Scalar(_) => (Some(scalar_trampoline), None, None),
            FuncKind::Aggregate(..) => (None, Some(step_trampoline), Some(final_trampoline)),
        };
        let data = Box::into_raw(Box::new(FuncData {
            kind,
            pending: self.inner.pending.clone(),
        }));
        let name = c_string(name);
        // SAFETY: open handle; `data` is owned by SQLite from here on and
        // released through `destroy_func` (also when registration fails).
        unsafe {
            ffi::sqlite3_create_function_v2(
                self.raw(),
                name.as_ptr(),
                nargs,
                flags | ffi::SQLITE_UTF8,
                data as *mut c_void,
                x_func,
                x_step,
                x_final,
                Some(destroy_func),
            )
        }
    }

    /// `sqlite3_create_function_v2()` for a scalar function (`SQLITE_UTF8` is
    /// added to `flags`).
    pub fn create_function(&self, name: &[u8], nargs: c_int, flags: c_int, f: ScalarFn) -> c_int {
        self.register_function(name, nargs, flags, FuncKind::Scalar(f))
    }

    /// `sqlite3_create_function_v2()` for an aggregate.
    pub fn create_aggregate(
        &self,
        name: &[u8],
        nargs: c_int,
        flags: c_int,
        step: StepFn,
        fini: FinalFn,
    ) -> c_int {
        self.register_function(name, nargs, flags, FuncKind::Aggregate(step, fini))
    }

    /// `sqlite3_create_collation_v2(db, name, SQLITE_UTF8, ...)`.
    pub fn create_collation(&self, name: &[u8], cmp: CollationFn) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        let data = Box::into_raw(Box::new(CollationData {
            cmp,
            pending: self.inner.pending.clone(),
        }));
        let name = c_string(name);
        // SAFETY: open handle; SQLite owns `data` (freed by `destroy_collation`
        // when replaced or on close). On failure the destructor is not called.
        let rc = unsafe {
            ffi::sqlite3_create_collation_v2(
                self.raw(),
                name.as_ptr(),
                ffi::SQLITE_UTF8,
                data as *mut c_void,
                Some(collation_trampoline),
                Some(destroy_collation),
            )
        };
        if rc != ffi::SQLITE_OK {
            // SAFETY: SQLite did not take ownership.
            drop(unsafe { Box::from_raw(data) });
        }
        rc
    }

    /// `sqlite3_set_authorizer()`; `None` removes the authorizer.
    pub fn set_authorizer(&self, cb: Option<AuthorizerFn>) -> c_int {
        if !self.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        let new = cb.map(|cb| {
            Box::new(AuthData {
                cb,
                pending: self.inner.pending.clone(),
            })
        });
        let ptr_arg = new
            .as_ref()
            .map_or(ptr::null_mut(), |b| &**b as *const AuthData as *mut c_void);
        // SAFETY: open handle; the new box is stored below and outlives its
        // registration (it is only dropped after being replaced).
        let rc = unsafe {
            ffi::sqlite3_set_authorizer(
                self.raw(),
                if new.is_some() {
                    Some(authorizer_trampoline)
                } else {
                    None
                },
                ptr_arg,
            )
        };
        *self.inner.authorizer.borrow_mut() = new;
        rc
    }

    /// `sqlite3_blob_open()`.
    pub fn blob_open(
        &self,
        db_name: &[u8],
        table: &[u8],
        column: &[u8],
        rowid: i64,
        writable: bool,
    ) -> Result<Blob, c_int> {
        if !self.is_open() {
            return Err(ffi::SQLITE_MISUSE);
        }
        let (d, t, c) = (c_string(db_name), c_string(table), c_string(column));
        let mut blob: *mut ffi::sqlite3_blob = ptr::null_mut();
        // SAFETY: open handle, valid C strings and out pointer.
        let rc = unsafe {
            ffi::sqlite3_blob_open(
                self.raw(),
                d.as_ptr(),
                t.as_ptr(),
                c.as_ptr(),
                rowid,
                c_int::from(writable),
                &mut blob,
            )
        };
        if rc != ffi::SQLITE_OK {
            if !blob.is_null() {
                // SAFETY: release the partially opened handle.
                unsafe { ffi::sqlite3_blob_close(blob) };
            }
            return Err(rc);
        }
        Ok(Blob {
            raw: blob,
            _db: self.clone(),
        })
    }

    /// Copies database `src_name` of `self` into `dest_name` of `dest`
    /// (`sqlite3_backup_init/step(-1)/finish` as in `SQLite3::backup()`).
    pub fn backup_to(&self, dest: &Connection, dest_name: &[u8], src_name: &[u8]) -> c_int {
        if !self.is_open() || !dest.is_open() {
            return ffi::SQLITE_MISUSE;
        }
        let (dn, sn) = (c_string(dest_name), c_string(src_name));
        // SAFETY: both handles are open; the backup object is finished here.
        unsafe {
            let b = ffi::sqlite3_backup_init(dest.raw(), dn.as_ptr(), self.raw(), sn.as_ptr());
            if b.is_null() {
                return ffi::sqlite3_errcode(self.raw());
            }
            while ffi::sqlite3_backup_step(b, -1) == ffi::SQLITE_OK {}
            ffi::sqlite3_backup_finish(b)
        }
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// A prepared statement (`sqlite3_stmt*`); finalized on drop.
pub struct Stmt {
    raw: *mut ffi::sqlite3_stmt,
    db: Connection,
}

impl Drop for Stmt {
    fn drop(&mut self) {
        self.finalize();
    }
}

impl Stmt {
    /// The connection this statement belongs to.
    pub fn connection(&self) -> &Connection {
        &self.db
    }

    /// Whether the statement has a handle (it is not empty or finalized).
    pub fn is_live(&self) -> bool {
        !self.raw.is_null()
    }

    /// `sqlite3_finalize()` (idempotent).
    pub fn finalize(&mut self) -> c_int {
        let raw = std::mem::replace(&mut self.raw, ptr::null_mut());
        if raw.is_null() {
            return ffi::SQLITE_OK;
        }
        // SAFETY: live statement, never used again.
        unsafe { ffi::sqlite3_finalize(raw) }
    }

    /// `sqlite3_step()`.
    pub fn step(&mut self, vm: &mut Vm) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_MISUSE;
        }
        let raw = self.raw;
        // SAFETY: live statement of an open connection (the connection cannot
        // be closed while statements are alive).
        with_vm(vm, || unsafe { ffi::sqlite3_step(raw) })
    }

    /// `sqlite3_reset()`.
    pub fn reset(&mut self) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_OK;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_reset(self.raw) }
    }

    /// `sqlite3_clear_bindings()`.
    pub fn clear_bindings(&mut self) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_OK;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_clear_bindings(self.raw) }
    }

    /// `sqlite3_bind_null()` (1-based index).
    pub fn bind_null(&mut self, i: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_RANGE;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_bind_null(self.raw, i) }
    }

    /// `sqlite3_bind_int64()`.
    pub fn bind_int64(&mut self, i: c_int, v: i64) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_RANGE;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_bind_int64(self.raw, i, v) }
    }

    /// `sqlite3_bind_int()`.
    pub fn bind_int(&mut self, i: c_int, v: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_RANGE;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_bind_int(self.raw, i, v) }
    }

    /// `sqlite3_bind_double()`.
    pub fn bind_double(&mut self, i: c_int, v: f64) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_RANGE;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_bind_double(self.raw, i, v) }
    }

    /// `sqlite3_bind_text(stmt, i, s, len, SQLITE_TRANSIENT)`.
    pub fn bind_text(&mut self, i: c_int, s: &[u8]) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_RANGE;
        }
        let Ok(len) = c_int::try_from(s.len()) else {
            return ffi::SQLITE_TOOBIG;
        };
        // SAFETY: live statement; SQLite copies the buffer.
        unsafe {
            ffi::sqlite3_bind_text(
                self.raw,
                i,
                s.as_ptr() as *const c_char,
                len,
                ffi::SQLITE_TRANSIENT(),
            )
        }
    }

    /// `sqlite3_bind_blob(stmt, i, b, len, SQLITE_TRANSIENT)`.
    pub fn bind_blob(&mut self, i: c_int, b: &[u8]) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_RANGE;
        }
        let Ok(len) = c_int::try_from(b.len()) else {
            return ffi::SQLITE_TOOBIG;
        };
        // SAFETY: live statement; SQLite copies the buffer (a zero-length
        // blob still binds a non-NULL pointer).
        unsafe {
            ffi::sqlite3_bind_blob(
                self.raw,
                i,
                if b.is_empty() {
                    c"".as_ptr() as *const c_void
                } else {
                    b.as_ptr() as *const c_void
                },
                len,
                ffi::SQLITE_TRANSIENT(),
            )
        }
    }

    /// `sqlite3_bind_parameter_count()`.
    pub fn bind_parameter_count(&self) -> c_int {
        if self.raw.is_null() {
            return 0;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_bind_parameter_count(self.raw) }
    }

    /// `sqlite3_bind_parameter_index()` (0 when not found).
    pub fn bind_parameter_index(&self, name: &[u8]) -> c_int {
        if self.raw.is_null() {
            return 0;
        }
        let name = c_string(name);
        // SAFETY: live statement, valid C string.
        unsafe { ffi::sqlite3_bind_parameter_index(self.raw, name.as_ptr()) }
    }

    /// `sqlite3_bind_parameter_name()` (1-based).
    pub fn bind_parameter_name(&self, i: c_int) -> Option<&[u8]> {
        if self.raw.is_null() {
            return None;
        }
        // SAFETY: live statement; the name lives as long as the statement.
        unsafe { cstr_opt(ffi::sqlite3_bind_parameter_name(self.raw, i)) }
    }

    /// `sqlite3_column_count()`.
    pub fn column_count(&self) -> c_int {
        if self.raw.is_null() {
            return 0;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_column_count(self.raw) }
    }

    /// `sqlite3_data_count()`.
    pub fn data_count(&self) -> c_int {
        if self.raw.is_null() {
            return 0;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_data_count(self.raw) }
    }

    /// `sqlite3_column_type()`.
    pub fn column_type(&self, i: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_NULL;
        }
        // SAFETY: live statement (out-of-range columns yield SQLITE_NULL).
        unsafe { ffi::sqlite3_column_type(self.raw, i) }
    }

    /// `sqlite3_column_int64()`.
    pub fn column_int64(&self, i: c_int) -> i64 {
        if self.raw.is_null() {
            return 0;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_column_int64(self.raw, i) }
    }

    /// `sqlite3_column_double()`.
    pub fn column_double(&self, i: c_int) -> f64 {
        if self.raw.is_null() {
            return 0.0;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_column_double(self.raw, i) }
    }

    /// `sqlite3_column_text()` + `sqlite3_column_bytes()`.
    pub fn column_text(&self, i: c_int) -> &[u8] {
        if self.raw.is_null() {
            return &[];
        }
        // SAFETY: live statement; the buffer stays valid until the next
        // step/reset/finalize, which need `&mut self`.
        unsafe {
            let p = ffi::sqlite3_column_text(self.raw, i);
            bytes_at(p as *const c_void, ffi::sqlite3_column_bytes(self.raw, i))
        }
    }

    /// `sqlite3_column_blob()` + `sqlite3_column_bytes()`.
    pub fn column_blob(&self, i: c_int) -> &[u8] {
        if self.raw.is_null() {
            return &[];
        }
        // SAFETY: as for `column_text`.
        unsafe {
            let p = ffi::sqlite3_column_blob(self.raw, i);
            bytes_at(p, ffi::sqlite3_column_bytes(self.raw, i))
        }
    }

    /// `sqlite3_column_bytes()`.
    pub fn column_bytes(&self, i: c_int) -> c_int {
        if self.raw.is_null() {
            return 0;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_column_bytes(self.raw, i) }
    }

    /// The column value by its storage class (`sqlite3_column_type()` first,
    /// then the matching accessor).
    pub fn column_value(&self, i: c_int) -> ValueRef<'_> {
        match self.column_type(i) {
            ffi::SQLITE_INTEGER => ValueRef::Integer(self.column_int64(i)),
            ffi::SQLITE_FLOAT => ValueRef::Real(self.column_double(i)),
            ffi::SQLITE_NULL => ValueRef::Null,
            ffi::SQLITE_TEXT => ValueRef::Text(self.column_text(i)),
            _ => ValueRef::Blob(self.column_blob(i)),
        }
    }

    /// `sqlite3_column_name()`.
    pub fn column_name(&self, i: c_int) -> Option<&[u8]> {
        if self.raw.is_null() {
            return None;
        }
        // SAFETY: live statement; valid until the statement changes.
        unsafe { cstr_opt(ffi::sqlite3_column_name(self.raw, i)) }
    }

    /// `sqlite3_column_decltype()`.
    pub fn column_decltype(&self, i: c_int) -> Option<&[u8]> {
        if self.raw.is_null() {
            return None;
        }
        // SAFETY: live statement.
        unsafe { cstr_opt(ffi::sqlite3_column_decltype(self.raw, i)) }
    }

    /// `sqlite3_column_table_name()`.
    pub fn column_table_name(&self, i: c_int) -> Option<&[u8]> {
        if self.raw.is_null() {
            return None;
        }
        // SAFETY: live statement (built with SQLITE_ENABLE_COLUMN_METADATA).
        unsafe { cstr_opt(ffi::sqlite3_column_table_name(self.raw, i)) }
    }

    /// `sqlite3_stmt_readonly()` (true for an empty statement).
    pub fn readonly(&self) -> bool {
        // SAFETY: NULL is accepted by sqlite3_stmt_readonly.
        unsafe { ffi::sqlite3_stmt_readonly(self.raw) != 0 }
    }

    /// `sqlite3_stmt_busy()`.
    pub fn busy(&self) -> bool {
        // SAFETY: NULL is accepted by sqlite3_stmt_busy.
        unsafe { ffi::sqlite3_stmt_busy(self.raw) != 0 }
    }

    /// `sqlite3_stmt_isexplain()`.
    pub fn isexplain(&self) -> c_int {
        // SAFETY: NULL is accepted by sqlite3_stmt_isexplain.
        unsafe { ffi::sqlite3_stmt_isexplain(self.raw) }
    }

    /// `sqlite3_stmt_explain()`.
    pub fn explain(&mut self, mode: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_stmt_explain(self.raw, mode) }
    }

    /// [`Stmt::explain`] with `vm` reachable by callbacks: switching modes
    /// may re-prepare the statement, which consults the authorizer.
    pub fn explain_vm(&mut self, vm: &mut Vm, mode: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_MISUSE;
        }
        let raw = self.raw;
        // SAFETY: live statement.
        with_vm(vm, || unsafe { ffi::sqlite3_stmt_explain(raw, mode) })
    }

    /// `sqlite3_sql()`.
    pub fn sql(&self) -> Option<&[u8]> {
        if self.raw.is_null() {
            return None;
        }
        // SAFETY: live statement.
        unsafe { cstr_opt(ffi::sqlite3_sql(self.raw)) }
    }

    /// `sqlite3_expanded_sql()`.
    pub fn expanded_sql(&self) -> Option<Vec<u8>> {
        if self.raw.is_null() {
            return None;
        }
        // SAFETY: live statement; the copy is freed with sqlite3_free.
        unsafe {
            let p = ffi::sqlite3_expanded_sql(self.raw);
            if p.is_null() {
                return None;
            }
            let v = CStr::from_ptr(p).to_bytes().to_vec();
            ffi::sqlite3_free(p as *mut c_void);
            Some(v)
        }
    }
}

// ---------------------------------------------------------------------------
// Blobs
// ---------------------------------------------------------------------------

/// An open BLOB handle (`sqlite3_blob*`); closed on drop.
pub struct Blob {
    raw: *mut ffi::sqlite3_blob,
    _db: Connection,
}

impl Drop for Blob {
    fn drop(&mut self) {
        self.close();
    }
}

impl Blob {
    /// `sqlite3_blob_close()` (idempotent).
    pub fn close(&mut self) -> c_int {
        let raw = std::mem::replace(&mut self.raw, ptr::null_mut());
        if raw.is_null() {
            return ffi::SQLITE_OK;
        }
        // SAFETY: open blob, never used again.
        unsafe { ffi::sqlite3_blob_close(raw) }
    }

    /// `sqlite3_blob_bytes()`.
    pub fn bytes(&self) -> c_int {
        if self.raw.is_null() {
            return 0;
        }
        // SAFETY: open blob.
        unsafe { ffi::sqlite3_blob_bytes(self.raw) }
    }

    /// `sqlite3_blob_read(blob, buf, buf.len(), offset)`.
    pub fn read(&mut self, buf: &mut [u8], offset: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open blob; `buf` is writable for its length.
        unsafe {
            ffi::sqlite3_blob_read(
                self.raw,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as c_int,
                offset,
            )
        }
    }

    /// `sqlite3_blob_write(blob, buf, buf.len(), offset)`.
    pub fn write(&mut self, buf: &[u8], offset: c_int) -> c_int {
        if self.raw.is_null() {
            return ffi::SQLITE_MISUSE;
        }
        // SAFETY: open blob; `buf` is readable for its length.
        unsafe {
            ffi::sqlite3_blob_write(
                self.raw,
                buf.as_ptr() as *const c_void,
                buf.len() as c_int,
                offset,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> Vm {
        Vm::new()
    }

    #[test]
    fn open_prepare_step_columns() {
        let mut vm = vm();
        let c = Connection::open(
            b":memory:",
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
        .expect("open");
        c.exec(
            &mut vm,
            b"CREATE TABLE t(a INTEGER, b TEXT, c REAL, d BLOB)",
        )
        .unwrap();
        c.exec(&mut vm, b"INSERT INTO t VALUES (1, 'x', 1.5, x'00ff')")
            .unwrap();
        assert_eq!(c.changes(), 1);
        assert_eq!(c.last_insert_rowid(), 1);
        let mut s = c
            .prepare(&mut vm, b"SELECT a, b, c, d, NULL FROM t")
            .unwrap();
        assert_eq!(s.column_count(), 5);
        assert_eq!(s.column_decltype(0), Some(&b"INTEGER"[..]));
        assert_eq!(s.column_table_name(1), Some(&b"t"[..]));
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ROW);
        assert_eq!(s.column_value(0), ValueRef::Integer(1));
        assert_eq!(s.column_value(1), ValueRef::Text(b"x"));
        assert_eq!(s.column_value(2), ValueRef::Real(1.5));
        assert_eq!(s.column_value(3), ValueRef::Blob(b"\x00\xff"));
        assert_eq!(s.column_value(4), ValueRef::Null);
        assert_eq!(s.step(&mut vm), ffi::SQLITE_DONE);
        // Statements keep the connection open.
        assert_eq!(c.close(), ffi::SQLITE_BUSY);
        drop(s);
        assert_eq!(c.close(), ffi::SQLITE_OK);
        assert!(!c.is_open());
    }

    #[test]
    fn errors_and_empty_statements() {
        let mut vm = vm();
        let c = Connection::open(
            b":memory:",
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
        .unwrap();
        assert_eq!(c.prepare(&mut vm, b"SELECT * FROM nope").err(), Some(1));
        assert_eq!(c.errmsg(), "no such table: nope");
        let (rc, msg) = c.exec(&mut vm, b"bogus").unwrap_err();
        assert_eq!(rc, ffi::SQLITE_ERROR);
        assert_eq!(msg.as_deref(), Some("near \"bogus\": syntax error"));
        let mut s = c.prepare(&mut vm, b"  -- nothing").unwrap();
        assert!(!s.is_live());
        assert_eq!(s.step(&mut vm), ffi::SQLITE_MISUSE);
        assert!(Connection::open(b"/nonexistent/dir/x.db", ffi::SQLITE_OPEN_READONLY).is_err());
        // Foreign keys are off by default, like stock SQLite builds.
        let mut s = c.prepare(&mut vm, b"PRAGMA foreign_keys").unwrap();
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ROW);
        assert_eq!(s.column_int64(0), 0);
    }

    #[test]
    fn functions_aggregates_collations() {
        let mut vm = vm();
        let c = Connection::open(
            b":memory:",
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
        .unwrap();
        let rc = c.create_function(
            b"twice",
            1,
            0,
            Box::new(|_vm, args| match args[0] {
                ValueRef::Integer(i) => FnOutput::Int(i * 2),
                _ => FnOutput::Error(b"bad".to_vec()),
            }),
        );
        assert_eq!(rc, ffi::SQLITE_OK);
        let rc = c.create_aggregate(
            b"cnt",
            -1,
            0,
            Box::new(|_vm, agg, _args| {
                agg.row_count += 1;
                StepOutput::Ok
            }),
            Box::new(|_vm, agg| FnOutput::Int(agg.row_count)),
        );
        assert_eq!(rc, ffi::SQLITE_OK);
        let rc = c.create_collation(b"rev", Box::new(|_vm, a, b| Ok(b.cmp(a) as c_int)));
        assert_eq!(rc, ffi::SQLITE_OK);
        let mut s = c
            .prepare(
                &mut vm,
                b"SELECT twice(21), (SELECT cnt(x) FROM (SELECT 1 x UNION SELECT 2))",
            )
            .unwrap();
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ROW);
        assert_eq!(s.column_int64(0), 42);
        assert_eq!(s.column_int64(1), 2);
        drop(s);
        let mut s = c
            .prepare(
                &mut vm,
                b"SELECT x FROM (SELECT 'a' x UNION SELECT 'b') ORDER BY x COLLATE rev",
            )
            .unwrap();
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ROW);
        assert_eq!(s.column_text(0), b"b");
        drop(s);
        let mut s = c.prepare(&mut vm, b"SELECT twice('x')").unwrap();
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ERROR);
        drop(s);
        assert_eq!(c.errmsg(), "bad");
    }

    #[test]
    fn callback_exceptions_are_parked() {
        let mut vm = vm();
        let c = Connection::open(
            b":memory:",
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
        .unwrap();
        c.create_function(
            b"boom",
            0,
            0,
            Box::new(|vm, _| {
                let e = vm.throw_error("Exception", "boom");
                FnOutput::Exception(e, Some(Vec::new()))
            }),
        );
        let mut s = c.prepare(&mut vm, b"SELECT boom()").unwrap();
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ERROR);
        assert!(c.has_exception());
        assert!(matches!(c.take_exception(), Some(Flow::Exception(_))));
        assert!(c.take_exception().is_none());
    }

    #[test]
    fn blobs_and_backup() {
        let mut vm = vm();
        let flags = ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE;
        let c = Connection::open(b":memory:", flags).unwrap();
        c.exec(
            &mut vm,
            b"CREATE TABLE t(b BLOB); INSERT INTO t VALUES (zeroblob(4))",
        )
        .unwrap();
        let mut b = c.blob_open(b"main", b"t", b"b", 1, true).unwrap();
        assert_eq!(b.bytes(), 4);
        assert_eq!(b.write(b"ab", 1), ffi::SQLITE_OK);
        let mut buf = [0u8; 4];
        assert_eq!(b.read(&mut buf, 0), ffi::SQLITE_OK);
        assert_eq!(&buf, b"\0ab\0");
        drop(b);
        let d = Connection::open(b":memory:", flags).unwrap();
        assert_eq!(c.backup_to(&d, b"main", b"main"), ffi::SQLITE_OK);
        let mut s = d.prepare(&mut vm, b"SELECT length(b) FROM t").unwrap();
        assert_eq!(s.step(&mut vm), ffi::SQLITE_ROW);
        assert_eq!(s.column_int64(0), 4);
    }
}
