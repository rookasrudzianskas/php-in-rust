//! The `SQLite3` class (`PHP_METHOD(SQLite3, ...)` in `sqlite3.c`) and the
//! callbacks it registers with SQLite (functions, aggregates, collations and
//! the authorizer).

use std::cell::{Cell, RefCell};
use std::ffi::c_int;
use std::rc::{Rc, Weak};

use php_runtime::class::{ClassConstant, ClassEntry, Function, class_flags, member_flags};
use php_runtime::types::TypeDecl;
use php_runtime::{
    Args, ArrayKey, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry,
    Value, Vm,
};

use super::capi::{
    self, AggContext, AuthorizerFn, CollationFn, Connection, FinalFn, FnOutput, ScalarFn, StepFn,
    StepOutput, ValueRef, ffi,
};
use super::result::ResultState;
use super::stmt::{self, StmtCore, StmtState};
use super::{EXCEPTION_CLASS, Pending, clone_forbidden, column_to_value, not_initialised};
use crate::standard::streams::plain;

pub const CLASS: &str = "SQLite3";

/// The part of `php_sqlite3_db_object` that SQLite callbacks need.
#[derive(Default)]
pub(crate) struct DbShared {
    /// `db_obj->exception` (`enableExceptions()`).
    pub exceptions: Cell<bool>,
    /// `db_obj->in_callback`: depth of userland callbacks being run.
    pub in_callback: Cell<u32>,
    /// `db_obj->authorizer_fcc`.
    pub authorizer: RefCell<Option<Value>>,
}

/// Native state of an `SQLite3` object (`php_sqlite3_db_object`).
#[derive(Default)]
pub(crate) struct DbState {
    pub shared: Rc<DbShared>,
    /// The connection while initialised.
    pub conn: Option<Connection>,
    pub initialised: bool,
    /// `db_obj->db != NULL`: php-src keeps the (dangling) handle after
    /// `close()`, so a closed object still passes that check.
    pub handle_seen: bool,
    /// `db_obj->free_list`: statements finalized by `close()`.
    pub free_list: Vec<Weak<RefCell<StmtCore>>>,
}

/// What a method needs from a database object, copied out so that no borrow
/// is held while SQLite runs.
pub(crate) struct Db {
    pub conn: Option<Connection>,
    pub shared: Rc<DbShared>,
    pub initialised: bool,
    pub handle_seen: bool,
}

pub(crate) fn snapshot(obj: &ObjRef) -> Db {
    match obj.native::<DbState>() {
        Some(s) => Db {
            conn: s.conn.clone(),
            shared: s.shared.clone(),
            initialised: s.initialised,
            handle_seen: s.handle_seen,
        },
        None => Db {
            conn: None,
            shared: Rc::default(),
            initialised: false,
            handle_seen: false,
        },
    }
}

/// `SQLITE3_CHECK_INITIALIZED(db_obj, db_obj->initialised, SQLite3)`.
pub(crate) fn open_db(vm: &mut Vm, obj: &ObjRef) -> PResult<(Connection, Rc<DbShared>)> {
    let d = snapshot(obj);
    match d.conn {
        Some(c) if d.initialised => Ok((c, d.shared)),
        _ => Err(not_initialised(vm, CLASS)),
    }
}

/// `zend_llist_add_element(&db_obj->free_list, &stmt_obj)`.
pub(crate) fn add_to_free_list(db: &ObjRef, core: &Rc<RefCell<StmtCore>>) {
    if let Some(mut s) = db.native_mut::<DbState>() {
        s.free_list.retain(|w| {
            w.upgrade()
                .is_some_and(|c| c.try_borrow().is_ok_and(|c| c.initialised))
        });
        s.free_list.push(Rc::downgrade(core));
    }
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("SQLite3 method called without $this")
}

fn bytes_msg(prefix: &str, b: &[u8]) -> PhpStr {
    let mut m = prefix.as_bytes().to_vec();
    m.extend_from_slice(b);
    PhpStr::from_vec(m)
}

fn create_db(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let o = ObjRef::new(ce.clone());
    o.set_native(DbState::default());
    Ok(o)
}

pub(crate) fn find_class(vm: &Vm, name: &str) -> ClassRef {
    vm.find_class(name.as_bytes())
        .unwrap_or_else(|| panic!("{name} is registered by ext/sqlite3"))
}

// ---------------------------------------------------------------------------
// open / close
// ---------------------------------------------------------------------------

/// `SQLite3::open()` (also `__construct`).
fn open(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let filename = args.str(0).clone();
    let flags = args.int(1);
    if filename.contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }
    if snapshot(&this).initialised {
        return Err(vm.throw_error("Exception", "Already initialised DB Object"));
    }
    let fullpath = if !filename.is_empty() && filename.as_bytes() != b":memory:" {
        let Some(full) = plain::expand_filepath(&filename) else {
            return Err(vm.throw_error("Exception", "Unable to expand filepath"));
        };
        if plain::check_open_basedir(vm, &full, true)? {
            let msg = bytes_msg("open_basedir prohibits opening ", &full);
            return Err(vm.throw_error("Exception", msg));
        }
        full
    } else {
        // "" (a private temporary database) or ":memory:".
        filename.to_vec()
    };
    let conn = match Connection::open(&fullpath, flags as c_int) {
        Ok(c) => c,
        Err((_, msg)) => {
            if let Some(mut s) = this.native_mut::<DbState>() {
                s.handle_seen = true;
            }
            return Err(vm.throw_error("Exception", format!("Unable to open database: {msg}")));
        }
    };
    let shared = snapshot(&this).shared;
    *shared.authorizer.borrow_mut() = None;
    conn.set_authorizer(Some(authorizer(shared)));
    if vm.ini_bool("sqlite3.defensive") {
        conn.db_config(ffi::SQLITE_DBCONFIG_DEFENSIVE, 1);
    }
    if let Some(mut s) = this.native_mut::<DbState>() {
        s.conn = Some(conn);
        s.initialised = true;
        s.handle_seen = true;
    }
    Ok(Value::Null)
}

/// `php_sqlite3_free_list_dtor` for every statement of the database.
fn finalize_statements(obj: &ObjRef) {
    let list = match obj.native_mut::<DbState>() {
        Some(mut s) => std::mem::take(&mut s.free_list),
        None => return,
    };
    for w in list {
        if let Some(core) = w.upgrade() {
            stmt::free_list_dtor(&core);
        }
    }
}

fn close(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let d = snapshot(&this);
    if !d.initialised {
        return Ok(Value::Bool(true));
    }
    if d.shared.in_callback.get() > 0 {
        return Err(vm.throw_error(
            "Error",
            "Cannot close SQLite3 database while inside a callback",
        ));
    }
    finalize_statements(&this);
    if let Some(conn) = &d.conn {
        let rc = conn.close();
        if rc != ffi::SQLITE_OK {
            let mut p = Pending::default();
            let msg = format!("Unable to close database: {}", conn.errmsg());
            p.error(vm, d.shared.exceptions.get(), i64::from(rc), msg);
            return p.finish(Value::Bool(false));
        }
    }
    if let Some(mut s) = this.native_mut::<DbState>() {
        s.initialised = false;
        s.conn = None;
    }
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// Simple accessors
// ---------------------------------------------------------------------------

fn exec(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (conn, shared) = open_db(vm, &this(args))?;
    let sql = args.str(0).clone();
    let mut p = Pending::new(&conn);
    let ok = match conn.exec(vm, &sql) {
        Ok(()) => true,
        Err((rc, msg)) => {
            let msg = msg.unwrap_or_else(|| "(null)".to_string());
            p.error(vm, shared.exceptions.get(), i64::from(rc), msg);
            false
        }
    };
    p.finish(Value::Bool(ok))
}

fn version(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let mut a = PhpArray::new();
    a.insert_str("versionString", capi::libversion());
    a.insert_str("versionNumber", capi::libversion_number());
    Ok(Value::array(a))
}

fn last_insert_row_id(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (conn, _) = open_db(vm, &this(args))?;
    Ok(Value::Int(conn.last_insert_rowid()))
}

/// `SQLITE3_CHECK_INITIALIZED(db_obj, db_obj->db, SQLite3)`: the connection
/// if still open, `None` once closed.
fn handle_checked(vm: &mut Vm, args: &Args) -> PResult<Option<Connection>> {
    let d = snapshot(&this(args));
    if !d.handle_seen {
        return Err(not_initialised(vm, CLASS));
    }
    Ok(if d.initialised { d.conn } else { None })
}

fn last_error_code(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let conn = handle_checked(vm, args)?;
    Ok(Value::Int(conn.map_or(0, |c| i64::from(c.errcode()))))
}

fn last_extended_error_code(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let conn = handle_checked(vm, args)?;
    Ok(Value::Int(
        conn.map_or(0, |c| i64::from(c.extended_errcode())),
    ))
}

fn enable_extended_result_codes(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let enable = args.bool(0);
    let conn = handle_checked(vm, args)?;
    Ok(Value::Bool(conn.is_some_and(|c| {
        c.extended_result_codes(enable) == ffi::SQLITE_OK
    })))
}

fn last_error_msg(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let conn = handle_checked(vm, args)?;
    Ok(match conn {
        Some(c) => Value::from(c.errmsg()),
        None => Value::empty_string(),
    })
}

fn busy_timeout(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ms = args.int(0);
    let (conn, _) = open_db(vm, &this(args))?;
    // php-src only checks the result when built with SQLITE_ENABLE_API_ARMOR.
    conn.busy_timeout(ms as c_int);
    Ok(Value::Bool(true))
}

fn load_extension(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let (conn, shared) = open_db(vm, &this(args))?;
    let exceptions = shared.exceptions.get();
    let mut p = Pending::new(&conn);
    let extension_dir = vm
        .ini
        .get(b"sqlite3.extension_dir")
        .and_then(|e| e.value.clone());
    let Some(extension_dir) = extension_dir else {
        p.error(vm, exceptions, 0, "SQLite Extensions are disabled");
        return p.finish(Value::Bool(false));
    };
    if name.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let mut lib_path = extension_dir.to_vec();
    if !lib_path.ends_with(b"/") {
        lib_path.push(b'/');
    }
    lib_path.extend_from_slice(&name);
    let Some(fullpath) = plain::real_path(&lib_path) else {
        let mut m = b"Unable to load extension at '".to_vec();
        m.extend_from_slice(&lib_path);
        m.push(b'\'');
        p.error(vm, exceptions, 0, PhpStr::from_vec(m));
        return p.finish(Value::Bool(false));
    };
    if !fullpath.starts_with(&extension_dir) {
        p.error(
            vm,
            exceptions,
            0,
            "Unable to open extensions outside the defined directory",
        );
        return p.finish(Value::Bool(false));
    }
    // Extension loading is only enabled while loading.
    conn.enable_load_extension(true);
    let r = conn.load_extension(&fullpath);
    let ok = match r {
        Ok(()) => true,
        Err((_, msg)) => {
            let msg = msg.unwrap_or_else(|| "(null)".to_string());
            p.error(vm, exceptions, 0, msg);
            false
        }
    };
    conn.enable_load_extension(false);
    p.finish(Value::Bool(ok))
}

fn changes(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (conn, _) = open_db(vm, &this(args))?;
    Ok(Value::Int(conn.changes()))
}

/// `sqlite3_mprintf("%q", s)`: doubles single quotes.
fn escape_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    if s.contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }
    let mut out = Vec::with_capacity(s.len());
    for &c in s.as_bytes() {
        out.push(c);
        if c == b'\'' {
            out.push(b'\'');
        }
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

// ---------------------------------------------------------------------------
// prepare / query / querySingle
// ---------------------------------------------------------------------------

/// `object_init_ex(stmt, php_sqlite3_stmt_entry)` bound to `db`.
fn new_stmt_object(vm: &mut Vm, db: &ObjRef, shared: &Rc<DbShared>) -> PResult<ObjRef> {
    let ce = find_class(vm, stmt::CLASS);
    let obj = vm.instantiate(&ce)?;
    if let Some(mut st) = obj.native_mut::<StmtState>() {
        st.db_obj = Some(db.clone());
        st.shared = Some(shared.clone());
    }
    Ok(obj)
}

fn stmt_core(obj: &ObjRef) -> Rc<RefCell<StmtCore>> {
    obj.native::<StmtState>()
        .map(|s| s.core.clone())
        .unwrap_or_default()
}

fn prepare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (conn, shared) = open_db(vm, &this)?;
    let sql = args.str(0).clone();
    if sql.is_empty() {
        return Ok(Value::Bool(false));
    }
    let stmt_obj = new_stmt_object(vm, &this, &shared)?;
    let mut p = Pending::new(&conn);
    match conn.prepare(vm, &sql) {
        Err(rc) => {
            let msg = format!("Unable to prepare statement: {}", conn.errmsg());
            p.error(vm, shared.exceptions.get(), i64::from(rc), msg);
            drop(stmt_obj);
            p.finish(Value::Bool(false))
        }
        Ok(s) => {
            let core = stmt_core(&stmt_obj);
            {
                let mut c = core.borrow_mut();
                c.conn = Some(conn.clone());
                c.stmt = Some(s);
                c.initialised = true;
            }
            add_to_free_list(&this, &core);
            p.finish(Value::Object(stmt_obj))
        }
    }
}

/// `SQLite3::query()`. php-src runs the SQL through `sqlite3_exec()` when the
/// return value is unused (`USED_RET()`); the runtime does not expose that,
/// so the result object is always built.
fn query(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (conn, shared) = open_db(vm, &this)?;
    let exceptions = shared.exceptions.get();
    let sql = args.str(0).clone();
    if sql.is_empty() {
        return Ok(Value::Bool(false));
    }
    let stmt_obj = new_stmt_object(vm, &this, &shared)?;
    let mut p = Pending::new(&conn);
    let s = match conn.prepare(vm, &sql) {
        Ok(s) => s,
        Err(rc) => {
            let msg = format!("Unable to prepare statement: {}", conn.errmsg());
            p.error(vm, exceptions, i64::from(rc), msg);
            drop(stmt_obj);
            return p.finish(Value::Bool(false));
        }
    };
    let core = stmt_core(&stmt_obj);
    {
        let mut c = core.borrow_mut();
        c.conn = Some(conn.clone());
        c.stmt = Some(s);
        c.initialised = true;
    }
    let result_ce = find_class(vm, super::result::CLASS);
    let result = vm.instantiate(&result_ce)?;
    if let Some(mut r) = result.native_mut::<ResultState>() {
        r.shared = Some(shared.clone());
        r.stmt_obj = Some(stmt_obj.clone());
        r.core = Some(core.clone());
        r.column_names = None;
        r.column_count = -1;
    }
    match stmt::step(vm, &core) {
        ffi::SQLITE_ROW | ffi::SQLITE_DONE => {
            add_to_free_list(&this, &core);
            stmt::reset(&core);
            p.finish(Value::Object(result))
        }
        _ => {
            if !p.is_set() {
                let msg = format!("Unable to execute statement: {}", conn.errmsg());
                p.error(vm, exceptions, i64::from(conn.errcode()), msg);
            }
            stmt::free_list_dtor(&core);
            drop(result);
            p.finish(Value::Bool(false))
        }
    }
}

/// `SQLite3::querySingle()` (see [`query`] about `USED_RET()`).
fn query_single(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (conn, shared) = open_db(vm, &this(args))?;
    let exceptions = shared.exceptions.get();
    let sql = args.str(0).clone();
    let entire_row = args.bool(1);
    if sql.is_empty() {
        return Ok(Value::Bool(false));
    }
    let mut p = Pending::new(&conn);
    let mut s = match conn.prepare(vm, &sql) {
        Ok(s) => s,
        Err(rc) => {
            let msg = format!("Unable to prepare statement: {}", conn.errmsg());
            p.error(vm, exceptions, i64::from(rc), msg);
            return p.finish(Value::Bool(false));
        }
    };
    let ret = match s.step(vm) {
        ffi::SQLITE_ROW => {
            if !entire_row {
                column_to_value(&s, 0)
            } else {
                let mut a = PhpArray::new();
                for i in 0..s.data_count() {
                    let data = column_to_value(&s, i);
                    let name = s.column_name(i).unwrap_or_default();
                    a.insert(ArrayKey::from_bytes(name), data);
                }
                Value::array(a)
            }
        }
        ffi::SQLITE_DONE => {
            if !entire_row {
                Value::Null
            } else {
                Value::empty_array()
            }
        }
        _ => {
            if !p.is_set() {
                let msg = format!("Unable to execute statement: {}", conn.errmsg());
                p.error(vm, exceptions, i64::from(conn.errcode()), msg);
            }
            Value::Bool(false)
        }
    };
    drop(s);
    p.finish(ret)
}

// ---------------------------------------------------------------------------
// User-defined functions, aggregates, collations, authorizer
// ---------------------------------------------------------------------------

/// Calls a userland callback with `in_callback` raised (`close()` refuses
/// to run meanwhile).
fn call_user(vm: &mut Vm, shared: &DbShared, cb: &Value, args: Vec<Value>) -> PResult<Value> {
    shared.in_callback.set(shared.in_callback.get() + 1);
    let r = vm.call(cb, args);
    shared.in_callback.set(shared.in_callback.get() - 1);
    r
}

/// Argument conversion of `sqlite3_do_callback()`: blobs and text become strings.
fn arg_value(v: &ValueRef<'_>) -> Value {
    match *v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(d) => Value::Float(d),
        ValueRef::Text(b) | ValueRef::Blob(b) => Value::bytes(b),
    }
}

/// The SQL result of a scalar function or an aggregate's final callback.
fn fn_output(vm: &mut Vm, r: PResult<Value>) -> FnOutput {
    match r {
        // retval is UNDEF: sqlite3_result_error(ctx, "failed to invoke callback", 0).
        Err(flow) => FnOutput::Exception(flow, Some(Vec::new())),
        Ok(v) => match v.into_unref() {
            Value::Int(i) => FnOutput::Int(i),
            Value::Null => FnOutput::Null,
            Value::Float(d) => FnOutput::Float(d),
            other => match vm.to_php_string(&other) {
                Ok(s) => FnOutput::Text(s.to_vec()),
                Err(flow) => FnOutput::Exception(flow, None),
            },
        },
    }
}

fn scalar_fn(shared: Rc<DbShared>, cb: Value) -> ScalarFn {
    Box::new(move |vm, argv| {
        let args = argv.iter().map(arg_value).collect();
        let r = call_user(vm, &shared, &cb, args);
        fn_output(vm, r)
    })
}

/// The aggregate context passed to callbacks (`NULL` before the first step).
fn context_value(agg: &AggContext) -> Value {
    if agg.value.is_undef() {
        Value::Null
    } else {
        agg.value.clone()
    }
}

/// `php_sqlite3_callback_step`: the step's return value becomes the context.
fn step_fn(shared: Rc<DbShared>, cb: Value) -> StepFn {
    Box::new(move |vm, agg, argv| {
        agg.row_count += 1;
        let mut args = Vec::with_capacity(argv.len() + 2);
        args.push(context_value(agg));
        args.push(Value::Int(agg.row_count));
        args.extend(argv.iter().map(arg_value));
        match call_user(vm, &shared, &cb, args) {
            Ok(v) => {
                agg.value = v.into_unref();
                StepOutput::Ok
            }
            Err(flow) => {
                agg.value = Value::Undef;
                StepOutput::Exception(flow)
            }
        }
    })
}

/// `php_sqlite3_callback_final`.
fn final_fn(shared: Rc<DbShared>, cb: Value) -> FinalFn {
    Box::new(move |vm, agg| {
        agg.row_count = 0;
        let args = vec![context_value(agg), Value::Int(0)];
        let r = call_user(vm, &shared, &cb, args);
        let out = fn_output(vm, r);
        agg.value = Value::Undef;
        out
    })
}

/// `php_sqlite3_callback_compare`.
fn collation_fn(shared: Rc<DbShared>, cb: Value) -> CollationFn {
    Box::new(move |vm, a, b| {
        let r = call_user(vm, &shared, &cb, vec![Value::bytes(a), Value::bytes(b)])?;
        match r.into_unref() {
            Value::Int(i) => Ok(i as c_int),
            _ => {
                vm.warning(
                    "An error occurred while invoking the compare callback (invalid return type).  Collation behaviour is undefined.",
                )?;
                Ok(0)
            }
        }
    })
}

/// `php_sqlite3_error()` from inside a callback: the exception (if any) is
/// returned to be parked on the connection.
fn callback_error(vm: &mut Vm, shared: &DbShared, msg: impl Into<PhpStr>) -> PResult<()> {
    if shared.exceptions.get() {
        Err(Flow::Exception(vm.create_exception(
            EXCEPTION_CLASS,
            msg,
            0,
        )))
    } else {
        vm.warning(msg)
    }
}

/// `php_sqlite3_authorizer`, installed when the database is opened: denies
/// `ATTACH` outside `open_basedir`, then defers to the userland authorizer.
fn authorizer(shared: Rc<DbShared>) -> AuthorizerFn {
    Box::new(move |vm, action, argv| {
        if action == ffi::SQLITE_ATTACH && vm.ini_get("open_basedir").is_some_and(|v| !v.is_empty())
        {
            let Some(file) = argv[0] else {
                return Ok(ffi::SQLITE_DENY);
            };
            if file != b":memory:"
                && !file.is_empty()
                && (file.starts_with(b"file:") || plain::check_open_basedir(vm, file, true)?)
            {
                return Ok(ffi::SQLITE_DENY);
            }
        }
        // Cloned out: the callback may replace or clear the authorizer.
        let Some(cb) = shared.authorizer.borrow().clone() else {
            return Ok(ffi::SQLITE_OK);
        };
        let mut args = Vec::with_capacity(5);
        args.push(Value::Int(i64::from(action)));
        args.extend(argv.iter().map(|a| a.map_or(Value::Null, Value::bytes)));
        match call_user(vm, &shared, &cb, args) {
            Err(flow) => {
                let r = callback_error(
                    vm,
                    &shared,
                    "An error occurred while invoking the authorizer callback",
                );
                Err(match r {
                    Ok(()) => flow,
                    Err(new) => super::chain(new, flow),
                })
            }
            Ok(v) => match v.into_unref() {
                Value::Int(i) => {
                    let ret = i as c_int;
                    if ret != ffi::SQLITE_OK && ret != ffi::SQLITE_IGNORE && ret != ffi::SQLITE_DENY
                    {
                        callback_error(
                            vm,
                            &shared,
                            format!("The authorizer callback returned an invalid value: {ret}"),
                        )?;
                        return Ok(ffi::SQLITE_DENY);
                    }
                    Ok(ret)
                }
                _ => {
                    callback_error(
                        vm,
                        &shared,
                        "The authorizer callback returned an invalid type: expected int",
                    )?;
                    Ok(ffi::SQLITE_DENY)
                }
            },
        }
    })
}

fn create_function(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let cb = args.get(1).clone();
    let nargs = args.int(2) as c_int;
    let flags = args.int(3) as c_int;
    let (conn, shared) = open_db(vm, &this(args))?;
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    let rc = conn.create_function(&name, nargs, flags, scalar_fn(shared, cb));
    Ok(Value::Bool(rc == ffi::SQLITE_OK))
}

fn create_aggregate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let step = args.get(1).clone();
    let fini = args.get(2).clone();
    let nargs = args.int(3) as c_int;
    let (conn, shared) = open_db(vm, &this(args))?;
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    let rc = conn.create_aggregate(
        &name,
        nargs,
        0,
        step_fn(shared.clone(), step),
        final_fn(shared, fini),
    );
    Ok(Value::Bool(rc == ffi::SQLITE_OK))
}

fn create_collation(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let cb = args.get(1).clone();
    let (conn, shared) = open_db(vm, &this(args))?;
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    let rc = conn.create_collation(&name, collation_fn(shared, cb));
    Ok(Value::Bool(rc == ffi::SQLITE_OK))
}

fn enable_exceptions(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let enable = args.bool(0);
    let shared = snapshot(&this(args)).shared;
    let old = shared.exceptions.get();
    let mut p = Pending::default();
    if !enable {
        let r = vm.deprecated("Use of warnings for SQLite3 is deprecated");
        p.check(r);
    }
    shared.exceptions.set(enable);
    p.finish(Value::Bool(old))
}

fn set_authorizer(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cb = args.opt(0).cloned();
    let (_, shared) = open_db(vm, &this(args))?;
    *shared.authorizer.borrow_mut() = cb;
    Ok(Value::Bool(true))
}

fn backup(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dest = args.object(0).clone();
    let source_name = args.str(1).clone();
    let dest_name = args.str(2).clone();
    if source_name.contains(&0) {
        return Err(vm.argument_value_error(2, "must not contain any null bytes"));
    }
    if dest_name.contains(&0) {
        return Err(vm.argument_value_error(3, "must not contain any null bytes"));
    }
    let (source, shared) = open_db(vm, &this(args))?;
    let (destination, _) = open_db(vm, &dest)?;
    let rc = source.backup_to(&destination, &dest_name, &source_name);
    if rc == ffi::SQLITE_OK {
        return Ok(Value::Bool(true));
    }
    let msg = match rc {
        ffi::SQLITE_BUSY => "Backup failed: source database is busy".to_string(),
        ffi::SQLITE_LOCKED => "Backup failed: source database is locked".to_string(),
        _ => format!("Backup failed: {}", source.errmsg()),
    };
    let mut p = Pending::new(&source);
    p.error(vm, shared.exceptions.get(), i64::from(rc), msg);
    p.finish(Value::Bool(false))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

macro_rules! methods {
    ($($id:ident: $sig:literal => $handler:path;)*) => {
        $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

methods! {
    CONSTRUCT: "public function __construct(string $filename, int $flags = SQLITE3_OPEN_READWRITE | SQLITE3_OPEN_CREATE, #[\\SensitiveParameter] string $encryptionKey = \"\") {}" => open;
    OPEN: "public function open(string $filename, int $flags = SQLITE3_OPEN_READWRITE | SQLITE3_OPEN_CREATE, #[\\SensitiveParameter] string $encryptionKey = \"\"): void {}" => open;
    CLOSE: "public function close(): bool {}" => close;
    VERSION: "public static function version(): array {}" => version;
    LAST_INSERT_ROW_ID: "public function lastInsertRowID(): int {}" => last_insert_row_id;
    LAST_ERROR_CODE: "public function lastErrorCode(): int {}" => last_error_code;
    LAST_EXTENDED_ERROR_CODE: "public function lastExtendedErrorCode(): int {}" => last_extended_error_code;
    LAST_ERROR_MSG: "public function lastErrorMsg(): string {}" => last_error_msg;
    CHANGES: "public function changes(): int {}" => changes;
    BUSY_TIMEOUT: "public function busyTimeout(int $milliseconds): bool {}" => busy_timeout;
    LOAD_EXTENSION: "public function loadExtension(string $name): bool {}" => load_extension;
    BACKUP: "public function backup(SQLite3 $destination, string $sourceDatabase = \"main\", string $destinationDatabase = \"main\"): bool {}" => backup;
    ESCAPE_STRING: "public static function escapeString(string $string): string {}" => escape_string;
    PREPARE: "public function prepare(string $query): SQLite3Stmt|false {}" => prepare;
    EXEC: "public function exec(string $query): bool {}" => exec;
    QUERY: "public function query(string $query): SQLite3Result|false {}" => query;
    QUERY_SINGLE: "public function querySingle(string $query, bool $entireRow = false): mixed {}" => query_single;
    CREATE_FUNCTION: "public function createFunction(string $name, callable $callback, int $argCount = -1, int $flags = 0): bool {}" => create_function;
    CREATE_AGGREGATE: "public function createAggregate(string $name, callable $stepCallback, callable $finalCallback, int $argCount = -1): bool {}" => create_aggregate;
    CREATE_COLLATION: "public function createCollation(string $name, callable $callback): bool {}" => create_collation;
    OPEN_BLOB: "public function openBlob(string $table, string $column, int $rowid, string $database = \"main\", int $flags = SQLITE3_OPEN_READONLY) {}" => super::blob::open_blob;
    ENABLE_EXCEPTIONS: "public function enableExceptions(bool $enable = false): bool {}" => enable_exceptions;
    ENABLE_EXTENDED_RESULT_CODES: "public function enableExtendedResultCodes(bool $enable = true): bool {}" => enable_extended_result_codes;
    SET_AUTHORIZER: "public function setAuthorizer(?callable $callback): bool {}" => set_authorizer;
}

/// `SQLite3::*` authorizer constants (`sqlite3.stub.php`).
const CONSTANTS: &[(&str, c_int)] = &[
    ("OK", ffi::SQLITE_OK),
    ("DENY", ffi::SQLITE_DENY),
    ("IGNORE", ffi::SQLITE_IGNORE),
    ("CREATE_INDEX", ffi::SQLITE_CREATE_INDEX),
    ("CREATE_TABLE", ffi::SQLITE_CREATE_TABLE),
    ("CREATE_TEMP_INDEX", ffi::SQLITE_CREATE_TEMP_INDEX),
    ("CREATE_TEMP_TABLE", ffi::SQLITE_CREATE_TEMP_TABLE),
    ("CREATE_TEMP_TRIGGER", ffi::SQLITE_CREATE_TEMP_TRIGGER),
    ("CREATE_TEMP_VIEW", ffi::SQLITE_CREATE_TEMP_VIEW),
    ("CREATE_TRIGGER", ffi::SQLITE_CREATE_TRIGGER),
    ("CREATE_VIEW", ffi::SQLITE_CREATE_VIEW),
    ("DELETE", ffi::SQLITE_DELETE),
    ("DROP_INDEX", ffi::SQLITE_DROP_INDEX),
    ("DROP_TABLE", ffi::SQLITE_DROP_TABLE),
    ("DROP_TEMP_INDEX", ffi::SQLITE_DROP_TEMP_INDEX),
    ("DROP_TEMP_TABLE", ffi::SQLITE_DROP_TEMP_TABLE),
    ("DROP_TEMP_TRIGGER", ffi::SQLITE_DROP_TEMP_TRIGGER),
    ("DROP_TEMP_VIEW", ffi::SQLITE_DROP_TEMP_VIEW),
    ("DROP_TRIGGER", ffi::SQLITE_DROP_TRIGGER),
    ("DROP_VIEW", ffi::SQLITE_DROP_VIEW),
    ("INSERT", ffi::SQLITE_INSERT),
    ("PRAGMA", ffi::SQLITE_PRAGMA),
    ("READ", ffi::SQLITE_READ),
    ("SELECT", ffi::SQLITE_SELECT),
    ("TRANSACTION", ffi::SQLITE_TRANSACTION),
    ("UPDATE", ffi::SQLITE_UPDATE),
    ("ATTACH", ffi::SQLITE_ATTACH),
    ("DETACH", ffi::SQLITE_DETACH),
    ("ALTER_TABLE", ffi::SQLITE_ALTER_TABLE),
    ("REINDEX", ffi::SQLITE_REINDEX),
    ("ANALYZE", ffi::SQLITE_ANALYZE),
    ("CREATE_VTABLE", ffi::SQLITE_CREATE_VTABLE),
    ("DROP_VTABLE", ffi::SQLITE_DROP_VTABLE),
    ("FUNCTION", ffi::SQLITE_FUNCTION),
    ("SAVEPOINT", ffi::SQLITE_SAVEPOINT),
    ("COPY", ffi::SQLITE_COPY),
    ("RECURSIVE", ffi::SQLITE_RECURSIVE),
];

/// Adds typed `public const int` class constants.
pub(crate) fn add_int_constants(ce: &mut ClassEntry, class: &str, constants: &[(&str, c_int)]) {
    let int = TypeDecl::parse("int").expect("valid type");
    for &(name, value) in constants {
        let mut c = ClassConstant::new(name, Value::Int(i64::from(value)), class);
        c.ty = Some(int.clone());
        ce.add_constant(c);
    }
}

pub(crate) fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(CLASS, class_flags::NOT_SERIALIZABLE);
    add_int_constants(&mut ce, CLASS, CONSTANTS);
    let public = member_flags::PUBLIC;
    let static_ = member_flags::PUBLIC | member_flags::STATIC;
    for (nf, flags) in [
        (&CONSTRUCT, public),
        (&OPEN, public),
        (&CLOSE, public),
        (&VERSION, static_),
        (&LAST_INSERT_ROW_ID, public),
        (&LAST_ERROR_CODE, public),
        (&LAST_EXTENDED_ERROR_CODE, public),
        (&LAST_ERROR_MSG, public),
        (&CHANGES, public),
        (&BUSY_TIMEOUT, public),
        (&LOAD_EXTENSION, public),
        (&BACKUP, public),
        (&ESCAPE_STRING, static_),
        (&PREPARE, public),
        (&EXEC, public),
        (&QUERY, public),
        (&QUERY_SINGLE, public),
        (&CREATE_FUNCTION, public),
        (&CREATE_AGGREGATE, public),
        (&CREATE_COLLATION, public),
        (&OPEN_BLOB, public),
        (&ENABLE_EXCEPTIONS, public),
        (&ENABLE_EXTENDED_RESULT_CODES, public),
        (&SET_AUTHORIZER, public),
    ] {
        ce.add_method(Function::native_method(nf, CLASS, flags));
    }
    ce.handlers.create_object = Some(create_db);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}
