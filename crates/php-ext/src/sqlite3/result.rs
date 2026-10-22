//! The `SQLite3Result` class (`PHP_METHOD(SQLite3Result, ...)`).

use std::ffi::c_int;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value,
    Vm,
};

use super::capi::{self, ffi};
use super::db::DbShared;
use super::stmt::{self, StmtRef};
use super::{
    PHP_SQLITE3_ASSOC, PHP_SQLITE3_NUM, Pending, clone_forbidden, column_to_value, not_initialised,
};

pub const CLASS: &str = "SQLite3Result";

/// Native state of an `SQLite3Result` object (`php_sqlite3_result`).
pub(crate) struct ResultState {
    /// `result_obj->db_obj` (what php-src needs from it).
    pub shared: Option<Rc<DbShared>>,
    /// `result_obj->stmt_obj`: a counted reference to the statement object.
    pub stmt_obj: Option<ObjRef>,
    pub core: Option<StmtRef>,
    pub is_prepared: bool,
    /// Column count and names cached by `fetchArray()` / `fetchAll()`
    /// (cleared by `reset()` and `finalize()`).
    pub column_count: i32,
    pub column_names: Option<Vec<PhpStr>>,
}

impl Default for ResultState {
    fn default() -> Self {
        ResultState {
            shared: None,
            stmt_obj: None,
            core: None,
            is_prepared: false,
            column_count: -1,
            column_names: None,
        }
    }
}

impl Drop for ResultState {
    /// `php_sqlite3_result_object_free_storage`: resets a live statement
    /// before releasing it.
    fn drop(&mut self) {
        if let Some(core) = &self.core
            && let Ok(mut c) = core.try_borrow_mut()
            && c.initialised
            && let Some(s) = c.stmt.as_mut()
        {
            s.reset();
        }
    }
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("SQLite3Result method called without $this")
}

/// `SQLITE3_CHECK_INITIALIZED(result_obj->db_obj, result_obj->stmt_obj->initialised, SQLite3Result)`.
fn checked(vm: &mut Vm, this: &ObjRef) -> PResult<(StmtRef, Rc<DbShared>, Pending)> {
    let found = this.native::<ResultState>().and_then(|r| {
        let shared = r.shared.clone()?;
        let core = r.core.clone()?;
        Some((core, shared))
    });
    match found {
        Some((core, shared)) => {
            let (initialised, conn) = {
                let c = core.borrow();
                (c.initialised, c.conn.clone())
            };
            if !initialised {
                return Err(not_initialised(vm, CLASS));
            }
            let p = match &conn {
                Some(c) => Pending::new(c),
                None => Pending::default(),
            };
            Ok((core, shared, p))
        }
        None => Err(not_initialised(vm, CLASS)),
    }
}

fn with_stmt<R>(core: &StmtRef, default: R, f: impl FnOnce(&capi::Stmt) -> R) -> R {
    match core.borrow().stmt.as_ref() {
        Some(s) => f(s),
        None => default,
    }
}

fn clear_column_names_cache(this: &ObjRef) {
    if let Some(mut r) = this.native_mut::<ResultState>() {
        r.column_names = None;
        r.column_count = -1;
    }
}

/// `sqlite3result_fill_column_names_cache`.
fn column_names(core: &StmtRef, n: i32) -> Vec<PhpStr> {
    with_stmt(core, Vec::new(), |s| {
        (0..n)
            .map(|i| PhpStr::from_bytes(s.column_name(i).unwrap_or_default()))
            .collect()
    })
}

/// `php_sqlite3_fetch_one`.
fn fetch_one(core: &StmtRef, n_cols: i32, mode: i64, names: &[PhpStr]) -> PhpArray {
    let mut row = PhpArray::new();
    with_stmt(core, (), |s| {
        for i in 0..n_cols {
            let data = column_to_value(s, i);
            if mode & PHP_SQLITE3_NUM != 0 {
                row.insert(ArrayKey::Int(i64::from(i)), data.clone());
            }
            if mode & PHP_SQLITE3_ASSOC != 0 {
                // A later column with the same name wins (zend_symtable_update).
                let name = names.get(i as usize).cloned().unwrap_or_default();
                row.insert(ArrayKey::from_php_str(name), data);
            }
        }
    });
    row
}

fn statement_error(vm: &mut Vm, core: &StmtRef, exceptions: bool, p: &mut Pending, what: &str) {
    let (code, msg) = stmt::error_of(core);
    p.error(vm, exceptions, code, format!("{what}: {msg}"));
}

fn num_columns(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (core, _, _) = checked(vm, &this(args))?;
    let n = with_stmt(&core, 0, capi::Stmt::column_count);
    Ok(Value::Int(i64::from(n)))
}

fn column_name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let column = args.int(0) as c_int;
    let (core, _, _) = checked(vm, &this(args))?;
    let name = with_stmt(&core, None, |s| s.column_name(column).map(<[u8]>::to_vec));
    Ok(match name {
        Some(n) => Value::String(PhpStr::from_vec(n)),
        None => Value::Bool(false),
    })
}

fn column_type(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let column = args.int(0) as c_int;
    let (core, _, _) = checked(vm, &this(args))?;
    let ty = with_stmt(&core, None, |s| {
        (s.data_count() != 0).then(|| s.column_type(column))
    });
    Ok(match ty {
        Some(t) => Value::Int(i64::from(t)),
        None => Value::Bool(false),
    })
}

/// `SQLite3Result::fetchArray()`. php-src skips building the row when the
/// return value is unused (`USED_RET()`), which is not observable.
fn fetch_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(0);
    let this = this(args);
    let (core, shared, mut p) = checked(vm, &this)?;
    match stmt::step(vm, &core) {
        ffi::SQLITE_ROW => {
            let (n_cols, cached) = {
                let mut r = this.native_mut::<ResultState>().expect("result state");
                if r.column_count == -1 {
                    r.column_count = with_stmt(&core, 0, capi::Stmt::column_count);
                }
                (r.column_count, r.column_names.clone())
            };
            // Column names are cached to speed up repeated fetches.
            let names = match cached {
                Some(n) => n,
                None if mode & PHP_SQLITE3_ASSOC != 0 => {
                    let n = column_names(&core, n_cols);
                    if let Some(mut r) = this.native_mut::<ResultState>() {
                        r.column_names = Some(n.clone());
                    }
                    n
                }
                None => Vec::new(),
            };
            let row = fetch_one(&core, n_cols, mode, &names);
            p.finish(Value::array(row))
        }
        ffi::SQLITE_DONE => p.finish(Value::Bool(false)),
        _ => {
            let exceptions = shared.exceptions.get();
            statement_error(vm, &core, exceptions, &mut p, "Unable to execute statement");
            p.finish(Value::Bool(false))
        }
    }
}

fn fetch_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(0);
    let this = this(args);
    let (core, shared, mut p) = checked(vm, &this)?;
    let nb_cols = with_stmt(&core, 0, capi::Stmt::column_count);
    let names = {
        let cached = this
            .native::<ResultState>()
            .and_then(|r| r.column_names.clone());
        match cached {
            Some(n) => n,
            None if mode & PHP_SQLITE3_ASSOC != 0 => {
                let n = column_names(&core, nb_cols);
                if let Some(mut r) = this.native_mut::<ResultState>() {
                    r.column_names = Some(n.clone());
                }
                n
            }
            None => Vec::new(),
        }
    };
    if let Some(mut r) = this.native_mut::<ResultState>() {
        r.column_count = nb_cols;
    }
    let mut rows = PhpArray::new();
    loop {
        match stmt::step(vm, &core) {
            ffi::SQLITE_ROW => {
                let row = fetch_one(&core, nb_cols, mode, &names);
                let _ = rows.append(Value::array(row));
            }
            ffi::SQLITE_DONE => break,
            _ => {
                if !p.is_set() {
                    let exceptions = shared.exceptions.get();
                    statement_error(vm, &core, exceptions, &mut p, "Unable to execute statement");
                }
                return p.finish(Value::Bool(false));
            }
        }
    }
    p.finish(Value::array(rows))
}

fn reset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (core, shared, mut p) = checked(vm, &this)?;
    clear_column_names_cache(&this);
    if stmt::reset(&core) != ffi::SQLITE_OK {
        let exceptions = shared.exceptions.get();
        statement_error(
            vm,
            &core,
            exceptions,
            &mut p,
            "Statement reset reported an error",
        );
        return p.finish(Value::Bool(false));
    }
    p.finish(Value::Bool(true))
}

fn finalize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (core, shared, mut p) = checked(vm, &this)?;
    clear_column_names_cache(&this);
    let is_prepared = this.native::<ResultState>().is_some_and(|r| r.is_prepared);
    if !is_prepared {
        // The internal statement of query() is finalized.
        stmt::free_list_dtor(&core);
    } else if stmt::reset(&core) != ffi::SQLITE_OK {
        let exceptions = shared.exceptions.get();
        statement_error(
            vm,
            &core,
            exceptions,
            &mut p,
            "Statement reset reported an error",
        );
    }
    p.finish(Value::Bool(true))
}

fn construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "SQLite3Result cannot be directly instantiated"))
}

fn create_result(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let o = ObjRef::new(ce.clone());
    o.set_native(ResultState::default());
    Ok(o)
}

macro_rules! methods {
    ($($id:ident: $sig:literal => $handler:path;)*) => {
        $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

methods! {
    CONSTRUCT: "private function __construct() {}" => construct;
    NUM_COLUMNS: "public function numColumns(): int {}" => num_columns;
    COLUMN_NAME: "public function columnName(int $column): string|false {}" => column_name;
    COLUMN_TYPE: "public function columnType(int $column): int|false {}" => column_type;
    FETCH_ARRAY: "public function fetchArray(int $mode = SQLITE3_BOTH): array|false {}" => fetch_array;
    FETCH_ALL: "public function fetchAll(int $mode = SQLITE3_BOTH): array|false {}" => fetch_all;
    RESET: "public function reset(): bool {}" => reset;
    FINALIZE: "public function finalize(): true {}" => finalize;
}

pub(crate) fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(CLASS, class_flags::NOT_SERIALIZABLE);
    ce.add_method(Function::native_method(
        &CONSTRUCT,
        CLASS,
        member_flags::PRIVATE,
    ));
    for nf in [
        &NUM_COLUMNS,
        &COLUMN_NAME,
        &COLUMN_TYPE,
        &FETCH_ARRAY,
        &FETCH_ALL,
        &RESET,
        &FINALIZE,
    ] {
        ce.add_method(Function::native_method(nf, CLASS, member_flags::PUBLIC));
    }
    ce.handlers.create_object = Some(create_result);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);
}
