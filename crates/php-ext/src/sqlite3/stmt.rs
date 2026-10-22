//! The `SQLite3Stmt` class (`PHP_METHOD(SQLite3Stmt, ...)`) and parameter
//! binding (`php_sqlite3_bind_params`).

use std::cell::RefCell;
use std::ffi::c_int;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::value::convert::{to_float_silent, to_int_silent};
use php_runtime::{
    Args, ClassRef, NativeFunction, ObjRef, PResult, PhpRef, PhpStr, Registry, Value, Vm,
};

use super::capi::{self, Connection, ffi};
use super::db::{self, DbShared, add_to_free_list, find_class, open_db};
use super::result::ResultState;
use super::{Pending, clone_forbidden, not_initialised};
use crate::standard::streams::{self, stream::COPY_ALL};

pub const CLASS: &str = "SQLite3Stmt";

/// Key of `stmt_obj->bound_params`: parameters bound by name are keyed by
/// their (`:`-prefixed) name, the others by number.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum ParamKey {
    Name(Vec<u8>),
    Index(i64),
}

/// `struct php_sqlite3_bound_param`. The value is a reference: the caller's
/// for `bindParam()`, a private one for `bindValue()` (conversions made when
/// binding stick to it, like `convert_to_long()` on the stored zval).
#[derive(Clone)]
pub(crate) struct BoundParam {
    pub number: i64,
    pub ty: i64,
    pub value: PhpRef,
}

/// The statement shared by an `SQLite3Stmt` object, its results and the
/// database's `free_list` (`php_sqlite3_stmt` minus the object).
#[derive(Default)]
pub(crate) struct StmtCore {
    /// `stmt_obj->stmt`; `None` once finalized (and while being stepped).
    pub stmt: Option<capi::Stmt>,
    /// The connection the statement was prepared on.
    pub conn: Option<Connection>,
    pub initialised: bool,
    pub bound: Vec<(ParamKey, BoundParam)>,
}

pub(crate) type StmtRef = Rc<RefCell<StmtCore>>;

/// Native state of an `SQLite3Stmt` object.
#[derive(Default)]
pub(crate) struct StmtState {
    /// `stmt_obj->db_obj` (a counted reference to the `SQLite3` object).
    pub db_obj: Option<ObjRef>,
    pub shared: Option<Rc<DbShared>>,
    pub core: StmtRef,
}

impl Drop for StmtState {
    /// `php_sqlite3_stmt_object_free_storage`.
    fn drop(&mut self) {
        if let Ok(mut c) = self.core.try_borrow_mut() {
            c.bound.clear();
            if c.initialised {
                c.initialised = false;
                c.stmt = None;
            }
        }
    }
}

/// `php_sqlite3_free_list_dtor`: finalizes the statement.
pub(crate) fn free_list_dtor(core: &StmtRef) {
    let stmt = match core.try_borrow_mut() {
        Ok(mut c) if c.initialised => {
            c.initialised = false;
            c.stmt.take()
        }
        _ => None,
    };
    drop(stmt);
}

/// `sqlite3_step()`. The handle is taken out of the core while SQLite runs
/// so that callbacks re-entering the extension never meet a borrowed core.
pub(crate) fn step(vm: &mut Vm, core: &StmtRef) -> c_int {
    let Some(mut s) = core.borrow_mut().stmt.take() else {
        return ffi::SQLITE_MISUSE;
    };
    let rc = s.step(vm);
    let mut c = core.borrow_mut();
    if c.initialised && c.stmt.is_none() {
        c.stmt = Some(s);
        drop(c);
    } else {
        // Finalized by a callback while running.
        drop(c);
        drop(s);
    }
    rc
}

/// `sqlite3_reset()`.
pub(crate) fn reset(core: &StmtRef) -> c_int {
    core.borrow_mut()
        .stmt
        .as_mut()
        .map_or(ffi::SQLITE_OK, capi::Stmt::reset)
}

/// `sqlite3_errcode()` / `sqlite3_errmsg()` of `sqlite3_db_handle(stmt)`
/// (`NULL` for an empty statement, which reports `SQLITE_NOMEM`).
pub(crate) fn error_of(core: &StmtRef) -> (i64, String) {
    let c = core.borrow();
    match (&c.stmt, &c.conn) {
        (Some(s), Some(conn)) if s.is_live() => (i64::from(conn.errcode()), conn.errmsg()),
        _ => (
            i64::from(ffi::SQLITE_NOMEM),
            capi::errstr(ffi::SQLITE_NOMEM),
        ),
    }
}

fn with_stmt(core: &StmtRef, f: impl FnOnce(&mut capi::Stmt) -> c_int) -> c_int {
    match core.borrow_mut().stmt.as_mut() {
        Some(s) => f(s),
        None => ffi::SQLITE_MISUSE,
    }
}

/// `php_sqlite3_bind_params`: `false` is `FAILURE`.
fn bind_params(vm: &mut Vm, core: &StmtRef, exceptions: bool, p: &mut Pending) -> bool {
    let params: Vec<BoundParam> = core.borrow().bound.iter().map(|(_, b)| b.clone()).collect();
    for param in params {
        let n = param.number;
        let idx = n as c_int;
        let value = param.value.get();
        let bind_failed = |vm: &mut Vm, p: &mut Pending, rc: c_int| {
            if rc != ffi::SQLITE_OK {
                let msg = format!("Unable to bind parameter number {n}");
                p.error(vm, exceptions, i64::from(rc), msg);
            }
        };
        if value.is_null() {
            let rc = with_stmt(core, |s| s.bind_null(idx));
            bind_failed(vm, p, rc);
            continue;
        }
        match param.ty {
            t if t == i64::from(ffi::SQLITE_INTEGER) => {
                // convert_to_long() on the bound zval.
                let r = vm.to_int(&value);
                let i = p.check(r).unwrap_or_else(|| to_int_silent(&value));
                param.value.set(Value::Int(i));
                let rc = with_stmt(core, |s| s.bind_int64(idx, i));
                bind_failed(vm, p, rc);
            }
            t if t == i64::from(ffi::SQLITE_FLOAT) => {
                let r = vm.to_float(&value);
                let d = p.check(r).unwrap_or_else(|| to_float_silent(&value));
                param.value.set(Value::Float(d));
                let rc = with_stmt(core, |s| s.bind_double(idx, d));
                bind_failed(vm, p, rc);
            }
            t if t == i64::from(ffi::SQLITE_BLOB) => {
                let buffer = if let Value::Resource(res) = &value {
                    let r = streams::with_stream(vm, res, |vm, s| s.copy_to_mem(vm, COPY_ALL));
                    match r {
                        Ok(Some(buf)) => buf,
                        Ok(None) => {
                            let msg = format!("Unable to read stream for parameter {n}");
                            p.error(vm, exceptions, 0, msg);
                            return false;
                        }
                        Err(f) => {
                            p.throw(f);
                            None
                        }
                    }
                } else {
                    // zval_get_string(): an empty string after an exception.
                    let r = vm.to_php_string(&value);
                    Some(p.check(r).map(|s| s.to_vec()).unwrap_or_default())
                };
                let rc = match buffer {
                    Some(b) => with_stmt(core, |s| s.bind_blob(idx, &b)),
                    None => with_stmt(core, |s| s.bind_null(idx)),
                };
                bind_failed(vm, p, rc);
            }
            t if t == i64::from(ffi::SQLITE_TEXT) => {
                let r = vm.to_php_string(&value);
                let Some(s) = p.check(r) else {
                    return false;
                };
                let rc = with_stmt(core, |st| st.bind_text(idx, &s));
                bind_failed(vm, p, rc);
            }
            t if t == i64::from(ffi::SQLITE_NULL) => {
                let rc = with_stmt(core, |s| s.bind_null(idx));
                bind_failed(vm, p, rc);
            }
            t => {
                let msg = format!("Unknown parameter type: {t} for parameter {n}");
                p.error(vm, exceptions, 0, msg);
                return false;
            }
        }
    }
    true
}

/// `register_bound_parameter_to_sqlite`.
fn register_param(
    core: &StmtRef,
    name: Option<PhpStr>,
    mut number: i64,
    ty: i64,
    value: PhpRef,
) -> bool {
    let mut c = core.borrow_mut();
    // A ':' prefix is needed to resolve a name to a parameter number.
    let name = name.map(|n| match n.first() {
        Some(b':') | Some(b'@') => n.to_vec(),
        _ => {
            let mut v = Vec::with_capacity(n.len() + 1);
            v.push(b':');
            v.extend_from_slice(&n);
            v
        }
    });
    if let Some(name) = &name {
        number = c
            .stmt
            .as_ref()
            .map_or(0, |s| i64::from(s.bind_parameter_index(name)));
    }
    if number < 1 {
        return false;
    }
    c.bound.retain(|(k, _)| *k != ParamKey::Index(number));
    let key = match name {
        Some(n) => ParamKey::Name(n),
        None => ParamKey::Index(number),
    };
    let param = BoundParam { number, ty, value };
    match c.bound.iter_mut().find(|(k, _)| *k == key) {
        Some(e) => e.1 = param,
        None => c.bound.push((key, param)),
    }
    true
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("SQLite3Stmt method called without $this")
}

struct Ctx {
    core: StmtRef,
    shared: Rc<DbShared>,
    conn: Option<Connection>,
}

/// `SQLITE3_CHECK_INITIALIZED(stmt_obj->db_obj, stmt_obj->initialised, SQLite3)`.
fn checked(vm: &mut Vm, this: &ObjRef) -> PResult<Ctx> {
    let ctx = this.native::<StmtState>().and_then(|st| {
        let shared = st.shared.clone()?;
        st.db_obj.as_ref()?;
        let c = st.core.borrow();
        c.initialised.then(|| Ctx {
            core: st.core.clone(),
            shared,
            conn: c.conn.clone(),
        })
    });
    ctx.ok_or_else(|| not_initialised(vm, db::CLASS))
}

/// [`checked`] + `SQLITE3_CHECK_INITIALIZED_STMT(stmt_obj->stmt, SQLite3Stmt)`.
fn checked_stmt(vm: &mut Vm, this: &ObjRef) -> PResult<Ctx> {
    let ctx = checked(vm, this)?;
    let live = ctx
        .core
        .borrow()
        .stmt
        .as_ref()
        .is_some_and(capi::Stmt::is_live);
    if !live {
        return Err(not_initialised(vm, CLASS));
    }
    Ok(ctx)
}

fn pending(ctx: &Ctx) -> Pending {
    match &ctx.conn {
        Some(c) => Pending::new(c),
        None => Pending::default(),
    }
}

fn create_stmt(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let o = ObjRef::new(ce.clone());
    o.set_native(StmtState::default());
    Ok(o)
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let db_obj = args.object(0).clone();
    let sql = args.str(1).clone();
    let (conn, shared) = open_db(vm, &db_obj)?;
    if sql.is_empty() {
        return Ok(Value::Bool(false));
    }
    let core = {
        let Some(mut st) = this.native_mut::<StmtState>() else {
            return Ok(Value::Null);
        };
        st.db_obj = Some(db_obj.clone());
        st.shared = Some(shared.clone());
        st.core.clone()
    };
    let mut p = Pending::new(&conn);
    match conn.prepare(vm, &sql) {
        Err(rc) => {
            let msg = format!("Unable to prepare statement: {}", conn.errmsg());
            p.error(vm, shared.exceptions.get(), i64::from(rc), msg);
        }
        Ok(s) => {
            {
                let mut c = core.borrow_mut();
                c.conn = Some(conn.clone());
                c.stmt = Some(s);
                c.initialised = true;
            }
            add_to_free_list(&db_obj, &core);
        }
    }
    p.finish(Value::Null)
}

fn param_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked_stmt(vm, &this(args))?;
    let n = ctx
        .core
        .borrow()
        .stmt
        .as_ref()
        .map_or(0, capi::Stmt::bind_parameter_count);
    Ok(Value::Int(i64::from(n)))
}

fn close(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked(vm, &this(args))?;
    free_list_dtor(&ctx.core);
    Ok(Value::Bool(true))
}

fn reset_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked_stmt(vm, &this(args))?;
    let mut p = pending(&ctx);
    if reset(&ctx.core) != ffi::SQLITE_OK {
        let (code, msg) = error_of(&ctx.core);
        let msg = format!("Unable to reset statement: {msg}");
        p.error(vm, ctx.shared.exceptions.get(), code, msg);
        return p.finish(Value::Bool(false));
    }
    p.finish(Value::Bool(true))
}

fn clear(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked_stmt(vm, &this(args))?;
    let mut p = pending(&ctx);
    if with_stmt(&ctx.core, capi::Stmt::clear_bindings) != ffi::SQLITE_OK {
        let (code, msg) = error_of(&ctx.core);
        let msg = format!("Unable to clear statement: {msg}");
        p.error(vm, ctx.shared.exceptions.get(), code, msg);
        return p.finish(Value::Bool(false));
    }
    let bound = std::mem::take(&mut ctx.core.borrow_mut().bound);
    drop(bound);
    p.finish(Value::Bool(true))
}

fn read_only(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked_stmt(vm, &this(args))?;
    let ro = ctx
        .core
        .borrow()
        .stmt
        .as_ref()
        .is_some_and(capi::Stmt::readonly);
    Ok(Value::Bool(ro))
}

fn busy(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked_stmt(vm, &this(args))?;
    let b = ctx
        .core
        .borrow()
        .stmt
        .as_ref()
        .is_some_and(capi::Stmt::busy);
    Ok(Value::Bool(b))
}

fn explain(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ctx = checked_stmt(vm, &this(args))?;
    let m = ctx
        .core
        .borrow()
        .stmt
        .as_ref()
        .map_or(0, capi::Stmt::isexplain);
    Ok(Value::Int(i64::from(m)))
}

fn set_explain(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(0);
    if !(0..=2).contains(&mode) {
        return Err(vm.argument_value_error(
            1,
            "must be one of the SQLite3Stmt::EXPLAIN_MODE_* constants",
        ));
    }
    let ctx = checked_stmt(vm, &this(args))?;
    let rc = with_stmt(&ctx.core, |s| s.explain(mode as c_int));
    Ok(Value::Bool(rc == ffi::SQLITE_OK))
}

fn get_sql(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let expand = args.bool(0);
    let ctx = checked_stmt(vm, &this(args))?;
    let mut p = pending(&ctx);
    let ok = bind_params(vm, &ctx.core, ctx.shared.exceptions.get(), &mut p);
    if !ok || p.is_set() {
        return p.finish(Value::Bool(false));
    }
    let sql = {
        let c = ctx.core.borrow();
        let s = c.stmt.as_ref();
        if expand {
            s.and_then(capi::Stmt::expanded_sql).unwrap_or_default()
        } else {
            s.and_then(capi::Stmt::sql).unwrap_or_default().to_vec()
        }
    };
    p.finish(Value::String(PhpStr::from_vec(sql)))
}

/// `PHP_SQLITE3_SET_TYPE`: the type used when `bindValue()` gets none.
fn inferred_type(v: &Value) -> i64 {
    i64::from(match v {
        Value::Int(_) | Value::Bool(_) => ffi::SQLITE_INTEGER,
        Value::Float(_) => ffi::SQLITE_FLOAT,
        Value::Null => ffi::SQLITE_NULL,
        _ => ffi::SQLITE_TEXT,
    })
}

/// `sqlite3stmt_bind()`, shared by `bindParam()` and `bindValue()`.
fn bind(vm: &mut Vm, args: &mut Args, by_ref: bool) -> PResult<Value> {
    let this = this(args);
    let (name, number) = match args.get(0) {
        Value::String(s) => (Some(s.clone()), -1),
        v => (None, v.as_int().unwrap_or(-1)),
    };
    let value = if by_ref {
        args.by_ref(1)
    } else {
        PhpRef::new(args.get(1).unref())
    };
    let explicit_type = args.is_passed(2).then(|| args.int(2));
    let ctx = checked_stmt(vm, &this)?;
    // The zval of a by-reference argument is the reference itself, which
    // PHP_SQLITE3_SET_TYPE maps to SQLITE3_TEXT.
    let ty = explicit_type.unwrap_or_else(|| {
        if by_ref {
            i64::from(ffi::SQLITE_TEXT)
        } else {
            inferred_type(&value.get())
        }
    });
    Ok(Value::Bool(register_param(
        &ctx.core, name, number, ty, value,
    )))
}

fn bind_param(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bind(vm, args, true)
}

fn bind_value(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bind(vm, args, false)
}

fn execute(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let ctx = checked(vm, &this)?;
    let exceptions = ctx.shared.exceptions.get();
    let mut p = pending(&ctx);
    // Always reset before execution, see bug #77051.
    reset(&ctx.core);
    let ok = bind_params(vm, &ctx.core, exceptions, &mut p);
    if !ok || p.is_set() {
        return p.finish(Value::Bool(false));
    }
    match step(vm, &ctx.core) {
        ffi::SQLITE_ROW | ffi::SQLITE_DONE => {
            reset(&ctx.core);
            let ce = find_class(vm, super::result::CLASS);
            let result = vm.instantiate(&ce)?;
            if let Some(mut r) = result.native_mut::<ResultState>() {
                r.is_prepared = true;
                r.shared = Some(ctx.shared.clone());
                r.stmt_obj = Some(this.clone());
                r.core = Some(ctx.core.clone());
                r.column_names = None;
                r.column_count = -1;
            }
            p.finish(Value::Object(result))
        }
        rc => {
            if rc == ffi::SQLITE_ERROR {
                reset(&ctx.core);
            }
            if !p.is_set() {
                let (code, msg) = error_of(&ctx.core);
                let msg = format!("Unable to execute statement: {msg}");
                p.error(vm, exceptions, code, msg);
            }
            p.finish(Value::Bool(false))
        }
    }
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
    CONSTRUCT: "private function __construct(SQLite3 $sqlite3, string $query) {}" => construct;
    BIND_PARAM: "public function bindParam(string|int $param, mixed &$var, int $type = SQLITE3_TEXT): bool {}" => bind_param;
    BIND_VALUE: "public function bindValue(string|int $param, mixed $value, int $type = SQLITE3_TEXT): bool {}" => bind_value;
    CLEAR: "public function clear(): bool {}" => clear;
    CLOSE: "public function close(): true {}" => close;
    EXECUTE: "public function execute(): SQLite3Result|false {}" => execute;
    GET_SQL: "public function getSQL(bool $expand = false): string|false {}" => get_sql;
    PARAM_COUNT: "public function paramCount(): int {}" => param_count;
    READ_ONLY: "public function readOnly(): bool {}" => read_only;
    RESET: "public function reset(): bool {}" => reset_method;
    BUSY: "public function busy(): bool {}" => busy;
    EXPLAIN: "public function explain(): int {}" => explain;
    SET_EXPLAIN: "public function setExplain(int $mode): bool {}" => set_explain;
}

pub(crate) fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(CLASS, class_flags::NOT_SERIALIZABLE);
    db::add_int_constants(
        &mut ce,
        CLASS,
        &[
            ("EXPLAIN_MODE_PREPARED", 0),
            ("EXPLAIN_MODE_EXPLAIN", 1),
            ("EXPLAIN_MODE_EXPLAIN_QUERY_PLAN", 2),
        ],
    );
    ce.add_method(Function::native_method(
        &CONSTRUCT,
        CLASS,
        member_flags::PRIVATE,
    ));
    for nf in [
        &BIND_PARAM,
        &BIND_VALUE,
        &CLEAR,
        &CLOSE,
        &EXECUTE,
        &GET_SQL,
        &PARAM_COUNT,
        &READ_ONLY,
        &RESET,
        &BUSY,
        &EXPLAIN,
        &SET_EXPLAIN,
    ] {
        ce.add_method(Function::native_method(nf, CLASS, member_flags::PUBLIC));
    }
    ce.handlers.create_object = Some(create_stmt);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}
