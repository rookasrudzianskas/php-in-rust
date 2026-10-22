//! ext/pdo_sqlite — the SQLite driver for PDO (`pdo_sqlite.c`,
//! `sqlite_driver.c`, `sqlite_statement.c`, `sqlite_sql_parser.re`) and the
//! `Pdo\Sqlite` class.
//!
//! The SQLite library is reached through [`crate::sqlite3::capi`].

pub mod blob;
pub mod driver;
pub mod statement;

use std::ffi::c_int;

use php_runtime::class::{ClassConstant, ClassEntry, Function, class_flags, member_flags};
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use crate::sqlite3::capi::ffi;

use super::consts::ATTR_DRIVER_SPECIFIC;
use super::dbh::{PDO_CLASS, checked_dbh, dbh_of};
use super::{error, finish, undefined_method};
use driver::{SqliteDriver, SqliteHandle, with_handle};

/// The driver-specific PDO subclass.
pub const SPECIFIC_CLASS: &str = "Pdo\\Sqlite";

/// `PDO_SQLITE_ATTR_*` (driver-specific attributes).
pub const ATTR_OPEN_FLAGS: i64 = ATTR_DRIVER_SPECIFIC;
pub const ATTR_READONLY_STATEMENT: i64 = ATTR_DRIVER_SPECIFIC + 1;
pub const ATTR_EXTENDED_RESULT_CODES: i64 = ATTR_DRIVER_SPECIFIC + 2;
pub const ATTR_BUSY_STATEMENT: i64 = ATTR_DRIVER_SPECIFIC + 3;
pub const ATTR_EXPLAIN_STATEMENT: i64 = ATTR_DRIVER_SPECIFIC + 4;
pub const ATTR_TRANSACTION_MODE: i64 = ATTR_DRIVER_SPECIFIC + 5;

/// `enum pdo_sqlite_transaction_mode`.
pub const TRANSACTION_MODE_DEFERRED: i64 = 0;
pub const TRANSACTION_MODE_IMMEDIATE: i64 = 1;
pub const TRANSACTION_MODE_EXCLUSIVE: i64 = 2;

/// The driver instance registered with PDO.
pub static DRIVER: SqliteDriver = SqliteDriver;

/// Deprecated `PDO::SQLITE_*` aliases (`REGISTER_PDO_SQLITE_CLASS_CONST_LONG_DEPRECATED_ALIAS_85`).
pub static PDO_CONSTANTS: &[(&str, i64, &str)] = &[
    (
        "SQLITE_DETERMINISTIC",
        ffi::SQLITE_DETERMINISTIC as i64,
        "use Pdo\\Sqlite::DETERMINISTIC instead",
    ),
    (
        "SQLITE_ATTR_OPEN_FLAGS",
        ATTR_OPEN_FLAGS,
        "use Pdo\\Sqlite::ATTR_OPEN_FLAGS instead",
    ),
    (
        "SQLITE_OPEN_READONLY",
        ffi::SQLITE_OPEN_READONLY as i64,
        "use Pdo\\Sqlite::OPEN_READONLY instead",
    ),
    (
        "SQLITE_OPEN_READWRITE",
        ffi::SQLITE_OPEN_READWRITE as i64,
        "use Pdo\\Sqlite::OPEN_READWRITE instead",
    ),
    (
        "SQLITE_OPEN_CREATE",
        ffi::SQLITE_OPEN_CREATE as i64,
        "use Pdo\\Sqlite::OPEN_CREATE instead",
    ),
    (
        "SQLITE_ATTR_READONLY_STATEMENT",
        ATTR_READONLY_STATEMENT,
        "use Pdo\\Sqlite::ATTR_READONLY_STATEMENT instead",
    ),
    (
        "SQLITE_ATTR_EXTENDED_RESULT_CODES",
        ATTR_EXTENDED_RESULT_CODES,
        "use Pdo\\Sqlite::ATTR_EXTENDED_RESULT_CODES instead",
    ),
];

/// The SQLite handle of `$this` for the `Pdo\Sqlite` methods.
fn sqlite_this(vm: &mut Vm, args: &Args) -> PResult<std::rc::Rc<super::dbh::Dbh>> {
    let (this, dbh) = checked_dbh(vm, args)?;
    if driver::handle(&dbh).is_none() {
        return Err(vm.throw_error(
            "Error",
            format!("{} object is uninitialized", this.class().name),
        ));
    }
    Ok(dbh)
}

/// Driver methods on plain `PDO` objects exist only for SQLite handles.
fn legacy_this(
    vm: &mut Vm,
    args: &Args,
    name: &str,
    replacement: &str,
) -> PResult<std::rc::Rc<super::dbh::Dbh>> {
    let this = args.this().cloned().expect("PDO method without $this");
    let dbh = dbh_of(&this).filter(|d| d.driver.get().is_some() && driver::handle(d).is_some());
    let Some(dbh) = dbh else {
        return Err(undefined_method(vm, &this, name));
    };
    vm.engine_deprecated(format!(
        "Method PDO::{name}() is deprecated since 8.5, use {replacement}() instead"
    ))?;
    Ok(dbh)
}

fn do_create_function(dbh: &super::dbh::Dbh, args: &Args) -> bool {
    let name = args.str(0).clone();
    let callback = args.get(1).clone();
    let (argc, flags) = (args.int(2), args.int(3));
    with_handle(dbh, |h| {
        driver::create_function(h, &name, callback, argc, flags)
    })
    .unwrap_or(false)
}

fn do_create_aggregate(dbh: &super::dbh::Dbh, args: &Args) -> bool {
    let name = args.str(0).clone();
    let (step, fini) = (args.get(1).clone(), args.get(2).clone());
    let argc = args.int(3);
    with_handle(dbh, |h| {
        driver::create_aggregate(h, &name, step, fini, argc)
    })
    .unwrap_or(false)
}

fn do_create_collation(dbh: &super::dbh::Dbh, args: &Args) -> bool {
    let name = args.str(0).clone();
    let callback = args.get(1).clone();
    with_handle(dbh, |h| driver::create_collation(h, &name, callback)).unwrap_or(false)
}

macro_rules! method {
    ($name:ident, $imp:ident) => {
        fn $name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let dbh = args.this().and_then(dbh_of);
            let r = $imp(vm, args);
            finish(vm, dbh.as_ref(), r)
        }
    };
}

fn create_function_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = sqlite_this(vm, args)?;
    Ok(Value::Bool(do_create_function(&dbh, args)))
}
method!(create_function, create_function_impl);

fn create_aggregate_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = sqlite_this(vm, args)?;
    Ok(Value::Bool(do_create_aggregate(&dbh, args)))
}
method!(create_aggregate, create_aggregate_impl);

fn create_collation_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = sqlite_this(vm, args)?;
    Ok(Value::Bool(do_create_collation(&dbh, args)))
}
method!(create_collation, create_collation_impl);

fn legacy_create_function_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = legacy_this(
        vm,
        args,
        "sqliteCreateFunction",
        "Pdo\\Sqlite::createFunction",
    )?;
    Ok(Value::Bool(do_create_function(&dbh, args)))
}
method!(legacy_create_function, legacy_create_function_impl);

fn legacy_create_aggregate_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = legacy_this(
        vm,
        args,
        "sqliteCreateAggregate",
        "Pdo\\Sqlite::createAggregate",
    )?;
    Ok(Value::Bool(do_create_aggregate(&dbh, args)))
}
method!(legacy_create_aggregate, legacy_create_aggregate_impl);

fn legacy_create_collation_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = legacy_this(
        vm,
        args,
        "sqliteCreateCollation",
        "Pdo\\Sqlite::createCollation",
    )?;
    Ok(Value::Bool(do_create_collation(&dbh, args)))
}
method!(legacy_create_collation, legacy_create_collation_impl);

/// `Pdo\Sqlite::loadExtension()`.
fn load_extension_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    if name.contains(&0) {
        return Err(vm.argument_type_error(1, "must not contain any null bytes"));
    }
    if name.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let dbh = sqlite_this(vm, args)?;
    let full = std::fs::canonicalize(std::ffi::OsStr::new(&*name.to_string_lossy()));
    let Ok(full) = full else {
        return Err(error::throw(
            vm,
            format!("Unable to load extension \"{name}\""),
        ));
    };
    let path = full.to_string_lossy().into_owned();
    let result = with_handle(&dbh, |h: &SqliteHandle| {
        // Only the C API is enabled, and only for the duration of the call.
        h.db.db_config(ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, 1);
        let r = h.db.load_extension(path.as_bytes());
        h.db.db_config(ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, 0);
        r
    });
    match result {
        Some(Err((_, msg))) => Err(error::throw(
            vm,
            format!("Unable to load extension \"{}\"", msg.unwrap_or_default()),
        )),
        _ => Ok(Value::Null),
    }
}
method!(load_extension, load_extension_impl);

/// `Pdo\Sqlite::openBlob()`.
fn open_blob_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dbh = sqlite_this(vm, args)?;
    let table = args.str(0).clone();
    let column = args.str(1).clone();
    let rowid = args.int(2);
    let dbname = args.str_opt(3).cloned().unwrap_or_default();
    let flags = args.int(4);
    for (i, s) in [(1, &table), (2, &column), (4, &dbname)] {
        if s.contains(&0) {
            return Err(vm.argument_type_error(i, "must not contain any null bytes"));
        }
    }
    let writable = flags & ffi::SQLITE_OPEN_READWRITE as i64 != 0;
    let opened = with_handle(&dbh, |h| {
        h.db.blob_open(&dbname, &table, &column, rowid, writable)
            .map_err(|_| h.db.errmsg())
    })
    .expect("checked SQLite handle");
    match opened {
        Ok(b) => {
            let mode: &[u8] = if writable { b"r+b" } else { b"rb" };
            let res = blob::open_stream(vm, b, flags as c_int, mode);
            Ok(Value::Resource(res))
        }
        Err(msg) => {
            vm.engine_warning(format!("Unable to open blob: {msg}"))?;
            Ok(Value::Bool(false))
        }
    }
}
method!(open_blob, open_blob_impl);

/// `Pdo\Sqlite::setAuthorizer()`.
fn set_authorizer_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cb = args.opt(0).cloned();
    let dbh = sqlite_this(vm, args)?;
    with_handle(&dbh, |h| *h.authorizer.borrow_mut() = cb);
    Ok(Value::Null)
}
method!(set_authorizer, set_authorizer_impl);

macro_rules! natives {
    ($($id:ident: $sig:literal => $h:ident;)*) => {
        $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $h };)*
    };
}

natives! {
    CREATE_AGGREGATE: "public function createAggregate(string $name, callable $step, callable $finalize, int $numArgs = -1): bool" => create_aggregate;
    CREATE_COLLATION: "public function createCollation(string $name, callable $callback): bool" => create_collation;
    CREATE_FUNCTION: "public function createFunction(string $function_name, callable $callback, int $num_args = -1, int $flags = 0): bool" => create_function;
    LOAD_EXTENSION: "public function loadExtension(string $name): void" => load_extension;
    OPEN_BLOB: "public function openBlob(string $table, string $column, int $rowid, ?string $dbname = \"main\", int $flags = \\Pdo\\Sqlite::OPEN_READONLY)" => open_blob;
    SET_AUTHORIZER: "public function setAuthorizer(?callable $callback): void" => set_authorizer;
    SQLITE_CREATE_FUNCTION: "public function sqliteCreateFunction(string $name, callable $callback, int $numArgs = -1, int $flags = 0): bool" => legacy_create_function;
    SQLITE_CREATE_AGGREGATE: "public function sqliteCreateAggregate(string $name, callable $step, callable $finalize, int $numArgs = -1): bool" => legacy_create_aggregate;
    SQLITE_CREATE_COLLATION: "public function sqliteCreateCollation(string $name, callable $callback): bool" => legacy_create_collation;
}

/// `class_PDO_SQLite_Ext_methods`: the driver methods of plain `PDO`
/// objects connected to SQLite (deprecated in 8.5).
pub static PDO_METHODS: &[&NativeFunction] = &[
    &SQLITE_CREATE_FUNCTION,
    &SQLITE_CREATE_AGGREGATE,
    &SQLITE_CREATE_COLLATION,
];

/// Registers `Pdo\Sqlite` (module `pdo_sqlite`).
pub fn register(r: &mut Registry) {
    let parent = r.find_class(PDO_CLASS).expect("PDO is registered first");
    let mut ce = ClassEntry::new_subclass(SPECIFIC_CLASS, class_flags::NOT_SERIALIZABLE, &parent);
    for (name, value) in [
        ("DETERMINISTIC", ffi::SQLITE_DETERMINISTIC as i64),
        ("OPEN_READONLY", ffi::SQLITE_OPEN_READONLY as i64),
        ("OPEN_READWRITE", ffi::SQLITE_OPEN_READWRITE as i64),
        ("OPEN_CREATE", ffi::SQLITE_OPEN_CREATE as i64),
        ("ATTR_OPEN_FLAGS", ATTR_OPEN_FLAGS),
        ("ATTR_READONLY_STATEMENT", ATTR_READONLY_STATEMENT),
        ("ATTR_EXTENDED_RESULT_CODES", ATTR_EXTENDED_RESULT_CODES),
        ("ATTR_BUSY_STATEMENT", ATTR_BUSY_STATEMENT),
        ("ATTR_EXPLAIN_STATEMENT", ATTR_EXPLAIN_STATEMENT),
        ("ATTR_TRANSACTION_MODE", ATTR_TRANSACTION_MODE),
        ("TRANSACTION_MODE_DEFERRED", TRANSACTION_MODE_DEFERRED),
        ("TRANSACTION_MODE_IMMEDIATE", TRANSACTION_MODE_IMMEDIATE),
        ("TRANSACTION_MODE_EXCLUSIVE", TRANSACTION_MODE_EXCLUSIVE),
        ("EXPLAIN_MODE_PREPARED", 0),
        ("EXPLAIN_MODE_EXPLAIN", 1),
        ("EXPLAIN_MODE_EXPLAIN_QUERY_PLAN", 2),
        ("OK", ffi::SQLITE_OK as i64),
        ("DENY", ffi::SQLITE_DENY as i64),
        ("IGNORE", ffi::SQLITE_IGNORE as i64),
    ] {
        ce.add_constant(ClassConstant::new(name, Value::Int(value), SPECIFIC_CLASS));
    }
    for nf in [
        &CREATE_AGGREGATE,
        &CREATE_COLLATION,
        &CREATE_FUNCTION,
        &LOAD_EXTENSION,
        &OPEN_BLOB,
        &SET_AUTHORIZER,
    ] {
        ce.add_method(Function::native_method(
            nf,
            SPECIFIC_CLASS,
            member_flags::PUBLIC,
        ));
    }
    r.class(ce);
}
